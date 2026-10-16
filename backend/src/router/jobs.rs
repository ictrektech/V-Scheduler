//! Durable asynchronous receipts retain admission across HTTP responses and
//! restarts. The on-disk schema is readable by the previous gateway for rollback.
use super::*;
use base64::Engine;
use std::collections::BTreeMap;
fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    path.split('.')
        .filter(|s| !s.is_empty())
        .fold(v, |v, k| &v[k])
}
fn root(g: &Gateway) -> PathBuf {
    g.state_path.parent().unwrap().join("scheduler-jobs")
}
fn save(g: &Gateway, job: &Value) -> Api<()> {
    let _lock = g.jobs_lock.lock().unwrap();
    save_locked(g, job)
}
fn save_locked(g: &Gateway, job: &Value) -> Api<()> {
    let id = text(job, "permit_id");
    if id.is_empty() || id.contains(['/', '\\']) {
        return Err(Failure::new(503, "invalid async permit"));
    }
    let dir = root(g);
    std::fs::create_dir_all(&dir)
        .map_err(|_| Failure::new(503, "cannot persist async admission"))?;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| Failure::new(503, "cannot protect async admission"))?;
    let path = dir.join(format!("{id}.json"));
    let temp = dir.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    let mut job = job.clone();
    if let Ok(current) = private_json(&path, 34 << 20) {
        if current["consumed"] == true {
            job["consumed"] = json!(true);
        }
        if current["released"] == true {
            job["released"] = json!(true);
        }
    }
    let result = (|| -> anyhow::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        f.write_all(&serde_json::to_vec(&job)?)?;
        f.sync_all()?;
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
        return Err(Failure::new(503, "cannot persist async admission"));
    }
    Ok(())
}
fn remove(g: &Gateway, job: &Value) {
    let id = text(job, "permit_id");
    if !id.is_empty() && !id.contains(['/', '\\']) {
        let _ = std::fs::remove_file(root(g).join(format!("{id}.json")));
    }
}
pub(super) fn creation(u: &Upstream, path: &str, input: &Value, method: &str) -> bool {
    if method != "POST" {
        return false;
    }
    match u.protocol.as_str() {
        "image_studio" => [
            "/images/generations",
            "/images/edits",
            "/videos/generations",
        ]
        .contains(&path),
        "ml_server" => path == "/ml/video/structure",
        "async_task" => path == "/images/generations",
        "video_structure" => path.ends_with("/jobs"),
        _ => {
            path == "/responses" && input["background"] == true
                || path.ends_with("/batches")
                || path.ends_with("/fine_tuning/jobs")
                || path == "/videos"
                || path == "/videos/generations"
        }
    }
}
pub(super) fn prepare(
    e: &proxy::Exchange,
    creation: String,
    headers: &HeaderMap,
) -> Api<Option<Value>> {
    let Some(lease) = e.lease.as_ref() else {
        return Ok(None);
    };
    let mut saved_headers = BTreeMap::<String, Vec<String>>::new();
    if !e.u.api_key.is_empty() {
        saved_headers.insert(
            "Authorization".into(),
            vec![format!("Bearer {}", e.u.api_key)],
        );
    } else if e.u.source == "image-studio" {
        if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
            saved_headers.insert("Authorization".into(), vec![auth.into()]);
        }
    }
    let (protocol, status, terminal) = match e.u.protocol.as_str() {
        "image_studio" => (
            "image_studio",
            "status",
            vec!["succeeded", "failed", "cancelled"],
        ),
        "ml_server" => ("ml_server", "data.status", vec!["succeeded", "failed"]),
        "video_structure" => (
            "video_structure",
            "data.status",
            vec!["succeeded", "failed"],
        ),
        "async_task" => (
            "async_task",
            text(e.u.async_task.as_ref().unwrap(), "status_path"),
            vec![],
        ),
        _ => (
            "native",
            "status",
            vec![
                "completed",
                "succeeded",
                "failed",
                "cancelled",
                "canceled",
                "expired",
                "incomplete",
            ],
        ),
    };
    let mut contract = e.u.async_task.clone().unwrap_or(Value::Null);
    if let Some(map) = contract.as_object_mut() {
        map.remove("default_input");
    }
    let terminal = if protocol == "async_task" {
        strings(&contract["success_states"])
            .into_iter()
            .chain(strings(&contract["failure_states"]))
            .collect::<Vec<_>>()
    } else {
        terminal.into_iter().map(str::to_owned).collect()
    };
    let job = json!({"permit_id":lease.id,"upstream_id":e.u.id,"owner_id":owner_scope(&e.id.owner),"protocol":protocol,"base_url":e.u.base_url,"creation_url":creation,"job_id":"","poll_url":"","task_id":"","status_path":status,"terminal":terminal,"headers":saved_headers,"poll_contract":contract,"model_id":e.native,"updated":Utc::now(),"completed":false,"released":false,"consumed":false});
    save(&e.g, &job)?;
    Ok(Some(job))
}
const PATH_SEGMENT: percent_encoding::AsciiSet = percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');
fn segment(id: &str) -> String {
    percent_encoding::utf8_percent_encode(id, &PATH_SEGMENT).to_string()
}
pub(super) async fn receipt(e: &mut proxy::Exchange, job: Option<Value>, value: &Value) -> Api<()> {
    let Some(mut job) = job else {
        return Ok(());
    };
    let base = text(&job, "base_url").trim_end_matches('/').to_string();
    let (job_id, poll) = match text(&job, "protocol") {
        "image_studio" => {
            let id = text(value, "job_id");
            let api = if base.ends_with("/api/v1") {
                base.clone()
            } else {
                format!("{base}/api/v1")
            };
            job["task_id"] = at(value, "metadata.scheduler_task_id").clone();
            (id.to_string(), format!("{api}/jobs/{}", segment(id)))
        }
        "ml_server" => {
            let id = at(value, "data.task_id").as_str().unwrap_or("");
            (
                id.into(),
                format!("{base}/api/v1/video/structure/tasks/{}", segment(id)),
            )
        }
        "video_structure" => {
            let id = at(value, "data.job_id").as_str().unwrap_or("");
            (
                id.into(),
                format!(
                    "{}/{}",
                    text(&job, "creation_url").trim_end_matches('/'),
                    segment(id)
                ),
            )
        }
        "async_task" => {
            let c = &job["poll_contract"];
            let id = at(value, text(c, "task_id_path")).as_str().unwrap_or("");
            let mut query = reqwest::Url::parse(&format!(
                "{base}/{}",
                text(c, "query_path").trim_start_matches('/')
            ))
            .map_err(|_| Failure::new(503, "invalid async poll endpoint"))?;
            query.query_pairs_mut().append_pair(
                if text(c, "query_task_param").is_empty() {
                    "taskId"
                } else {
                    text(c, "query_task_param")
                },
                id,
            );
            (id.into(), query.to_string())
        }
        _ => {
            let id = text(value, "id");
            let creation = text(&job, "creation_url")
                .trim_end_matches('/')
                .trim_end_matches("/generations");
            (id.into(), format!("{creation}/{}", segment(id)))
        }
    };
    if job_id.is_empty() {
        if job["protocol"] == "native" && e.status != 202
            || job["protocol"] == "video_structure" && value.get("code").is_some_and(|v| v != 0)
        {
            remove(&e.g, &job);
            if let Some(lease) = e.lease.as_mut() {
                lease.complete("async_creation_not_started_or_synchronously_completed");
            }
            return Ok(());
        }
        return Err(Failure::new(503, "async job receipt cannot be reconciled"));
    }
    let mut url =
        reqwest::Url::parse(&poll).map_err(|_| Failure::new(503, "invalid async poll endpoint"))?;
    // A job identifier may never redirect polling to a different authority.
    let origin =
        reqwest::Url::parse(&base).map_err(|_| Failure::new(503, "invalid async origin"))?;
    if url.origin() != origin.origin() {
        return Err(Failure::new(503, "async poll origin changed"));
    }
    url.set_fragment(None);
    job["job_id"] = json!(job_id);
    job["poll_url"] = json!(url.as_str());
    update(&mut job, value, e.status);
    save(&e.g, &job)?;
    if let Some(lease) = e.lease.take() {
        let g = e.g.clone();
        tokio::spawn(async move {
            track(g, lease, job).await;
        });
    }
    Ok(())
}
fn update(job: &mut Value, value: &Value, status: u16) {
    job["latest"] =
        json!(base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(value).unwrap()));
    job["http_status"] = json!(status);
    job["updated"] = json!(Utc::now());
    let state = at(value, text(job, "status_path")).as_str().unwrap_or("");
    let completed = strings(&job["terminal"])
        .iter()
        .any(|v| v.eq_ignore_ascii_case(state));
    job["completed"] = json!(completed);
    if text(job, "protocol") == "image_studio" {
        if let Some(id) = at(value, "metadata.scheduler_task_id").as_str() {
            job["task_id"] = json!(id);
        }
    }
}
async fn track(g: Arc<Gateway>, mut lease: lease::Lease, mut job: Value) {
    loop {
        if !text(&job, "task_id").is_empty()
            && lease
                .delegate(text(&job, "task_id"), text(&job, "job_id"))
                .await
        {
            remove(&g, &job);
            return;
        }
        if job["completed"] == true {
            if lease
                .release_confirmed("async_terminal_state_confirmed")
                .await
            {
                job["released"] = json!(true);
                if ["image_studio", "video_structure"].contains(&text(&job, "protocol")) {
                    remove(&g, &job);
                } else {
                    let _ = save(&g, &job);
                }
                return;
            }
        }
        if text(&job, "job_id").is_empty() || *lease.failed.borrow() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let mut request = g
            .client
            .get(text(&job, "poll_url"))
            .timeout(Duration::from_secs(10));
        if let Some(headers) = job["headers"].as_object() {
            for (name, values) in headers {
                if let Some(value) = values
                    .as_array()
                    .and_then(|v| v.first())
                    .and_then(Value::as_str)
                {
                    request = request.header(name, value);
                }
            }
        }
        if let Ok(response) = request.send().await {
            let status = response.status();
            if status.is_success() {
                if let Ok(value) = read_json(response, 32 << 20).await {
                    update(&mut job, &value, status.as_u16());
                    let _ = save(&g, &job);
                }
            }
        }
    }
}
pub(super) fn resume(g: Arc<Gateway>) -> Vec<tokio::task::JoinHandle<()>> {
    let mut tasks = vec![];
    let Ok(entries) = std::fs::read_dir(root(&g)) else {
        return tasks;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|s| s != "json") {
            continue;
        }
        let Ok(job) = private_json(&path, 34 << 20) else {
            continue;
        };
        if job["released"] == true {
            continue;
        }
        let upstream = text(&job, "upstream_id");
        let root = g.state_path.parent().unwrap();
        let binding = match upstream {
            "app-v-voice" => root.join("voice-scheduler.local.json"),
            "app-image-studio" => root.join("app-bindings/image-studio.json"),
            "app-v-music" => root.join("app-bindings/v-music.json"),
            _ => root.join("scheduler.local.json"),
        };
        let Ok(config) = private_json(&binding, 262144) else {
            continue;
        };
        let token = text(&config, "token");
        if token.len() < 32 {
            continue;
        }
        let lease = lease::resume(
            g.clone(),
            token.into(),
            text(&job, "permit_id").into(),
            String::new(),
            String::new(),
        );
        let g = g.clone();
        tasks.push(tokio::spawn(async move {
            track(g, lease, job).await;
        }));
    }
    tasks
}
pub(super) fn cached(g: &Gateway, id: &Identity, u: &Upstream, url: &str) -> Api<Option<Response>> {
    let _lock = g.jobs_lock.lock().unwrap();
    let Ok(entries) = std::fs::read_dir(root(g)) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|s| s != "json") {
            continue;
        }
        let Ok(mut job) = private_json(&path, 34 << 20) else {
            continue;
        };
        if text(&job, "upstream_id") != u.id
            || text(&job, "poll_url") != url
            || ["image_studio", "video_structure"].contains(&text(&job, "protocol"))
        {
            continue;
        }
        if text(&job, "owner_id") != owner_scope(&id.owner) || job["consumed"] == true {
            return Err(Failure::new(404, "task not found"));
        }
        if job["released"] == true
            && DateTime::parse_from_rfc3339(text(&job, "updated"))
                .is_ok_and(|at| Utc::now() - at.with_timezone(&Utc) > chrono::Duration::hours(1))
        {
            remove(g, &job);
            return Err(Failure::new(404, "task not found"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text(&job, "latest"))
            .map_err(|_| Failure::new(503, "async receipt cache is invalid"))?;
        if job["completed"] == true && job["protocol"] == "ml_server" {
            job["consumed"] = json!(true);
            save_locked(g, &job)?;
        }
        let mut response = Response::new(Body::from(bytes));
        *response.status_mut() =
            StatusCode::from_u16(job["http_status"].as_u64().unwrap_or(200) as u16)
                .unwrap_or(StatusCode::OK);
        response
            .headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return Ok(Some(response));
    }
    Ok(None)
}
