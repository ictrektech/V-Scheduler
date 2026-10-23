//! Bounded admission waiting. Payloads and credentials never enter queue views.
use super::*;
use std::time::Instant;

pub(super) fn scheduling(headers: &HeaderMap, id: &Identity) -> Api<crate::model::Scheduling> {
    let mut options = crate::model::Scheduling { priority: crate::model::Priority::Interactive, ..Default::default() };
    if let Some(priority) = headers.get("x-vscheduler-priority") {
        options.priority = serde_json::from_value(json!(priority.to_str().map_err(|_| Failure::new(422,"invalid priority"))?))
            .map_err(|_| Failure::new(422,"priority must be background, interactive or high"))?;
    }
    for (name, field) in [("x-vscheduler-preemptible", &mut options.preemptible), ("x-vscheduler-allow-preemption", &mut options.allow_preemption), ("x-vscheduler-force-admission", &mut options.force_admission)] {
        if let Some(value) = headers.get(name) {
            *field = match value.to_str().unwrap_or("") { "true" => true, "false" => false, _ => return Err(Failure::new(422,"scheduling flags must be true or false")) };
        }
    }
    if options.force_admission && !id.admin { return Err(Failure::new(403,"force admission requires a verified administrator")); }
    Ok(options)
}

#[derive(Default)]
pub(super) struct Queue {
    entries: Vec<Entry>,
    sequence: u64,
    cancelled: Vec<(String, String, Instant)>,
}
struct Entry {
    id: String,
    owner: String,
    app: String,
    model: String,
    priority: i64,
    sequence: u64,
    created: Instant,
    cancelled: bool,
    reason: String,
}
fn owner(id: &Identity) -> String {
    format!("{}\0{}", id.service_app, id.owner)
}
fn failure(status: u16, code: &str, message: &str) -> Failure {
    let mut error = Failure::new(status, message);
    error.code = Some(code.into());
    error
}
impl Queue {
    fn ordered(&self) -> Vec<&Entry> {
        let now = Instant::now();
        let mut entries: Vec<_> = self.entries.iter().filter(|e| !e.cancelled).collect();
        // Every 30 seconds adds one priority level. Background work eventually
        // overtakes new interactive work; equal scores retain arrival order.
        entries.sort_by_key(|e| {
            (
                -(e.priority + now.duration_since(e.created).as_secs() as i64 / 30),
                e.sequence,
            )
        });
        entries
    }
    pub fn snapshot(&self, id: &Identity) -> Value {
        let entries: Vec<_> = self.ordered().into_iter().enumerate()
            .filter(|(_, e)| id.admin || e.owner == owner(id))
            .map(|(position, e)| json!({"request_id":e.id,"app_id":e.app,"model_id":e.model,
                "force_admission":e.priority>=1_000_000,"priority":if e.priority>=2 {"high"} else if e.priority==1 {"interactive"} else {"background"},
                "position":position+1,"wait_ms":e.created.elapsed().as_millis() as u64,"reason":e.reason}))
            .collect();
        json!({"items":entries,"durable":false,"max_wait_seconds":300,"aging_seconds":30})
    }
    pub fn cancel(&mut self, id: &Identity, request: &str) -> Api<Value> {
        if request.is_empty()
            || request.len() > 96
            || !request
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(Failure::new(422, "invalid queue request ID"));
        }
        if id.owner.is_empty() && !id.admin {
            return Err(Failure::new(
                401,
                "queue cancellation requires a verified identity",
            ));
        }
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == request) {
            if !id.admin && entry.owner != owner(id) {
                return Err(Failure::new(404, "queued request not found"));
            }
            entry.cancelled = true;
            return Ok(json!({"request_id":request,"cancelled":true,"execution_started":false}));
        }
        self.cancelled
            .retain(|(_, _, at)| at.elapsed() < Duration::from_secs(300));
        if self.cancelled.len() >= 256 {
            return Err(failure(
                429,
                "QUEUE_FULL",
                "pending cancellation capacity exhausted",
            ));
        }
        // Cancellation can race ahead of POST authentication/body parsing. Keep
        // a bounded owner-scoped tombstone so that late arrival cannot execute.
        self.cancelled
            .push((owner(id), request.into(), Instant::now()));
        Ok(json!({"request_id":request,"cancelled":true,"execution_started":null}))
    }
}
pub(super) struct Ticket {
    g: Arc<Gateway>,
    pub id: String,
    deadline: Instant,
    priority: i64,
    force: bool,
    operator: Option<axum::http::HeaderValue>,
    shadow: Mutex<Option<(String, String, Instant)>>,
}
impl Ticket {
    pub fn enter(
        g: Arc<Gateway>,
        id: &Identity,
        headers: &HeaderMap,
        model: &str,
    ) -> Api<Option<Self>> {
        let wait = match headers.get("x-vscheduler-queue-wait-seconds") {
            None => 0,
            Some(v) => v
                .to_str()
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v <= 300)
                .ok_or_else(|| {
                    Failure::new(422, "queue wait must be an integer from 0 to 300 seconds")
                })?,
        };
        if wait == 0 {
            return Ok(None);
        }
        if id.owner.is_empty() && !id.admin {
            return Err(Failure::new(
                401,
                "queued inference requires a verified user or application identity",
            ));
        }
        let mut priority = match headers
            .get("x-vscheduler-priority")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("interactive")
        {
            "interactive" => 1,
            "background" => 0,
            "high" => 2,
            _ => {
                return Err(Failure::new(
                    422,
                    "priority must be background, interactive or high",
                ));
            }
        };
        let options = scheduling(headers, id)?;
        let normal_priority = priority;
        if options.force_admission { priority = options.score(); }
        let request = headers
            .get("x-vscheduler-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if request.is_empty()
            || request.len() > 96
            || !request
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(Failure::new(422, "invalid queue request ID"));
        }
        {
            let mut q = g.queue.lock().unwrap();
            q.cancelled
                .retain(|(_, _, at)| at.elapsed() < Duration::from_secs(300));
            if q.cancelled
                .iter()
                .any(|(scope, key, _)| scope == &owner(id) && key == &request)
            {
                return Err(failure(
                    409,
                    "QUEUE_CANCELLED",
                    "request was cancelled before queue registration",
                ));
            }
            if q.entries.iter().any(|e| e.id == request) {
                return Err(failure(
                    409,
                    "QUEUE_ID_IN_USE",
                    "queue request ID is already in use; inference was not replayed",
                ));
            }
            if q.entries.len() >= 64
                || q.entries.iter().filter(|e| e.owner == owner(id)).count() >= 8
            {
                return Err(failure(
                    429,
                    "QUEUE_FULL",
                    "inference waiting queue is full",
                ));
            }
            q.sequence += 1;
            let sequence = q.sequence;
            q.entries.push(Entry {
                id: request.clone(),
                owner: owner(id),
                app: id.service_app.clone(),
                model: model.into(),
                priority,
                sequence,
                created: Instant::now(),
                cancelled: false,
                reason: "WAIT_TURN".into(),
            });
        }
        Ok(Some(Self {
            g,
            id: request,
            deadline: Instant::now() + Duration::from_secs(wait),
            priority: normal_priority,
            force: options.force_admission,
            operator: if options.force_admission { headers.get("authorization").cloned() } else { None },
            shadow: Mutex::new(None),
        }))
    }
    pub async fn heartbeat(&self, token: &str, executor: &str) -> Api<()> {
        if self
            .shadow
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() < Duration::from_secs(1))
        {
            return Ok(());
        }
        lease::call_authorized(
            &self.g,
            token,
            "/inference-waiters",
            json!({"id":self.id,"executor_id":executor,"priority":self.priority,"force_admission":self.force}),
            false, self.operator.as_ref(),
        )
        .await?;
        *self.shadow.lock().unwrap() = Some((token.into(), executor.into(), Instant::now()));
        Ok(())
    }
    pub fn check(&self) -> Api<bool> {
        let q = self.g.queue.lock().unwrap();
        if q.entries
            .iter()
            .find(|e| e.id == self.id)
            .is_none_or(|e| e.cancelled)
        {
            return Err(failure(
                409,
                "QUEUE_CANCELLED",
                "queued inference cancelled before execution",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(failure(
                429,
                "QUEUE_TIMEOUT",
                "inference queue deadline exceeded; execution did not start",
            ));
        }
        Ok(q.ordered().first().is_some_and(|e| e.id == self.id))
    }
    pub fn reason(&self, reason: &str) {
        if let Some(e) = self
            .g
            .queue
            .lock()
            .unwrap()
            .entries
            .iter_mut()
            .find(|e| e.id == self.id)
        {
            e.reason = reason.into();
        }
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.g
            .queue
            .lock()
            .unwrap()
            .entries
            .retain(|e| e.id != self.id);
        if let Some((token, executor, _)) = self.shadow.lock().unwrap().take() {
            let g = self.g.clone();
            let id = self.id.clone();
            tokio::spawn(async move {
                let _ = lease::call(
                    &g,
                    &token,
                    "/inference-waiters",
                    json!({"id":id,"executor_id":executor,"priority":0,"remove":true}),
                    false,
                )
                .await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn priority_aging_and_owner_scoped_cancellation() {
        let mut q = Queue::default();
        let alice = Identity {
            owner: "alice".into(),
            ..Default::default()
        };
        let bob = Identity {
            owner: "bob".into(),
            ..Default::default()
        };
        for (id, priority, age) in [("background", 0, 0), ("chat", 1, 0)] {
            q.entries.push(Entry {
                id: id.into(),
                owner: owner(&alice),
                app: "app".into(),
                model: "m".into(),
                priority,
                sequence: q.entries.len() as u64,
                created: Instant::now() - Duration::from_secs(age),
                cancelled: false,
                reason: "WAIT_RESOURCE".into(),
            });
        }
        assert_eq!(q.ordered()[0].id, "chat");
        q.entries[0].created = Instant::now() - Duration::from_secs(61);
        assert_eq!(q.ordered()[0].id, "background");
        assert!(q.snapshot(&bob)["items"].as_array().unwrap().is_empty());
        assert!(q.cancel(&bob, "background").is_err());
        q.cancel(&alice, "background").unwrap();
        assert_eq!(q.ordered()[0].id, "chat");
        assert_eq!(q.snapshot(&alice)["items"][0]["position"], 1);
    }
}
