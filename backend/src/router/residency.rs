//! A warm model keeps its original resource reservation. Never infer free
//! memory from a completed HTTP response or an expired keep_alive timer.
use super::*;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

pub(super) struct Residency {
    seconds: AtomicU64,
    cached: tokio::sync::Mutex<Option<Resident>>,
}
struct Resident {
    lease: lease::Lease,
    upstream: String,
    model: String,
    origin: String,
    since: Instant,
}
impl Residency {
    pub fn open(state: &Path) -> anyhow::Result<Self> {
        let path = state.with_file_name("residency.json");
        let seconds = if path.exists() {
            private_json(&path, 4096)?["idle_seconds"]
                .as_u64()
                .filter(|v| *v <= 600)
                .ok_or_else(|| anyhow::anyhow!("invalid residency policy"))?
        } else {
            60
        };
        Ok(Self {
            seconds: AtomicU64::new(seconds),
            cached: Default::default(),
        })
    }
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            seconds: AtomicU64::new(0),
            cached: Default::default(),
        }
    }
    pub async fn abandon(&self) {
        self.cached.lock().await.take();
    }
    pub fn seconds(&self) -> u64 {
        self.seconds.load(Ordering::Acquire)
    }
    pub async fn snapshot(&self) -> Value {
        let cache = self.cached.lock().await;
        json!({"idle_seconds":self.seconds(),"budget_policy":"retain-full-reservation",
            "resident":cache.as_ref().map(|r|json!({"model_id":r.model,"upstream_id":r.upstream,
                "permit_id":r.lease.id,"idle_seconds":r.since.elapsed().as_secs(),"state":"resident"}))})
    }
    pub fn configure(&self, g: &Gateway, seconds: u64) -> Api<()> {
        if seconds > 600 {
            return Err(Failure::new(422, "idle_seconds must be 0..600"));
        }
        let path = g.state_path.with_file_name("residency.json");
        let temporary = path.with_extension(format!("{}.pending", uuid::Uuid::new_v4()));
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(
                serde_json::to_string(&json!({"idle_seconds":seconds}))
                    .unwrap()
                    .as_bytes(),
            )?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)
        })();
        let _ = std::fs::remove_file(&temporary);
        result.map_err(|_| Failure::new(503, "residency policy could not be saved"))?;
        self.seconds.store(seconds, Ordering::Release);
        Ok(())
    }
}
pub(super) fn native_origin(u: &Upstream) -> Option<String> {
    let role = u.id.strip_prefix("v-scheduler/ollama-")?;
    if u.source != "v-scheduler"
        || !["qa", "embedding", "rerank"].contains(&role)
        || u.base_url != format!("http://v-scheduler-ollama-{role}:11535/v1")
    {
        return None;
    }
    Some(format!("http://v-scheduler-ollama-{role}:11434"))
}
async fn evict(g: &Gateway, mut resident: Resident) -> bool {
    // An uncertain unload leaves a suspect permit for the existing native
    // recovery loop. The cache never releases on timeout or an empty /api/ps.
    resident.lease.stopped.store(false, Ordering::Release);
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let result = g
        .client
        .post(format!("{}/api/scheduler/confirm-stopped", resident.origin))
        .timeout(Duration::from_secs(5))
        .json(&json!({"model":resident.model,"nonce":nonce}))
        .send()
        .await;
    if let Ok(response) = result {
        if response.status().is_success() {
            if let Ok(value) = read_json(response, 65536).await {
                if value["protocol_version"] == 1
                    && value["model"] == resident.model
                    && value["nonce"] == nonce
                    && value["stopped"] == true
                    && value["native_exit_barrier"] == true
                {
                    return resident
                        .lease
                        .release_confirmed("builtin_ollama_idle_unload_confirmed")
                        .await;
                }
            }
        }
    }
    false
}
pub(super) async fn stop_idle(g: &Arc<Gateway>, role: &str, model: &str) -> Value {
    let mut cache = g.residency.cached.lock().await;
    let upstream = format!("v-scheduler/ollama-{role}");
    let confirmed = if cache
        .as_ref()
        .is_some_and(|r| r.model == model && r.upstream == upstream)
    {
        evict(g, cache.take().unwrap()).await
    } else {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        match g
            .client
            .post(format!(
                "http://v-scheduler-ollama-{role}:11434/api/scheduler/confirm-stopped"
            ))
            .timeout(Duration::from_secs(5))
            .json(&json!({"model":model,"nonce":nonce}))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                read_json(response, 65536).await.is_ok_and(|v| {
                    v["protocol_version"] == 1
                        && v["model"] == model
                        && v["nonce"] == nonce
                        && v["stopped"] == true
                        && v["native_exit_barrier"] == true
                })
            }
            _ => false,
        }
    };
    json!({"service_id":format!("v-scheduler-ollama-{role}"),"model":model,"accepted":true,"confirmed":confirmed,"stopped":confirmed,"target_state":"stopped","managed":true})
}
pub(super) async fn yield_idle(g: &Arc<Gateway>) {
    let mut cache = g.residency.cached.lock().await;
    if let Some(resident) = cache.take() {
        evict(g, resident).await;
    }
}
pub(super) async fn take(
    g: &Arc<Gateway>,
    approval: &lease::Approval,
    u: &Upstream,
    model: &str,
) -> Api<Option<lease::Lease>> {
    let mut cache = g.residency.cached.lock().await;
    let Some(mut resident) = cache.take() else {
        return Ok(None);
    };
    if native_origin(u).is_some()
        && resident.upstream == u.id
        && resident.model == model
        && resident.lease.executor == text(&approval.policy, "executor_id")
        && safe_secret_eq(&resident.lease.token, text(&approval.config, "token"))
        && g.residency.seconds() > 0
        && !*resident.lease.failed.borrow()
    {
        // Controller checks the current identity/executor/node again, atomically
        // rotates the proof, and refuses reuse of claimed third-party execution.
        let proof = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        if lease::call(
            g,
            &resident.lease.token,
            &format!("/permits/{}/reuse-resident", resident.lease.id),
            json!({"executor_id":resident.lease.executor,"upstream_url":u.base_url,"model_id":model,
                "execution_token":resident.lease.proof,"next_execution_token":proof}),
            false,
        )
        .await
        .is_ok()
        {
            resident.lease.proof = proof;
            // Until begin(), a dropped request must still account for warm memory.
            resident.lease.stopped.store(false, Ordering::Release);
            return Ok(Some(resident.lease));
        }
    }
    evict(g, resident).await;
    Ok(None)
}
pub(super) async fn complete(
    g: Arc<Gateway>,
    u: Upstream,
    model: String,
    status: u16,
    retained: bool,
    mut lease: lease::Lease,
    evidence: &'static str,
) {
    if retained {
        if let Some(origin) = native_origin(&u) {
            lease.stopped.store(false, Ordering::Release);
            let resident = Resident {
                lease,
                upstream: u.id,
                model,
                origin,
                since: Instant::now(),
            };
            let mut cache = g.residency.cached.lock().await;
            if (200..300).contains(&status)
                && g.residency.seconds() > 0
                && !*resident.lease.failed.borrow()
            {
                if let Some(old) = cache.replace(resident) {
                    evict(&g, old).await;
                }
            } else {
                evict(&g, resident).await;
            }
            return;
        }
    }
    lease.complete(evidence);
    lease.release_confirmed(evidence).await;
}
pub(super) fn start(g: Arc<Gateway>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let mut cache = g.residency.cached.lock().await;
            let evict_now = cache.as_ref().is_some_and(|r| {
                r.since.elapsed().as_secs() >= g.residency.seconds() || *r.lease.failed.borrow()
            });
            if evict_now {
                if let Some(resident) = cache.take() {
                    evict(&g, resident).await;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retention_only_controls_fixed_owned_backends() {
        let mut u = Upstream {
            id: "v-scheduler/ollama-qa".into(),
            source: "v-scheduler".into(),
            base_url: "http://v-scheduler-ollama-qa:11535/v1".into(),
            ..Default::default()
        };
        assert!(native_origin(&u).is_some());
        u.base_url = "http://other:11535/v1".into();
        assert!(native_origin(&u).is_none());
        u.id = "v-scheduler/ollama-other".into();
        assert!(native_origin(&u).is_none());
    }
}
