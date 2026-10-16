use super::*;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Clone)]
pub(super) struct Approval {
    pub config: Value,
    pub policy: Value,
}
pub(super) const RESOURCE_PARAMETERS: &[&str] = &[
    "max_tokens",
    "max_output_tokens",
    "max_completion_tokens",
    "max_new_tokens",
    "n",
    "batch_size",
    "batch_count",
    "steps",
    "num_inference_steps",
    "video_frames",
    "fps",
    "width",
    "height",
    "duration",
    "duration_seconds",
    "num_frames",
    "options.num_ctx",
    "options.num_batch",
    "options.num_gpu",
    "options.num_thread",
    "context_length",
    "num_ctx",
    "num_batch",
    "num_gpu",
    "num_thread",
    "max_length",
    "max_model_len",
    "max_seq_length",
    "dimensions",
];
pub(super) struct Lease {
    g: Arc<Gateway>,
    token: String,
    pub id: String,
    pub proof: String,
    pub executor: String,
    renewal: tokio::task::JoinHandle<()>,
    pub failed: tokio::sync::watch::Receiver<bool>,
    pub stopped: Arc<AtomicBool>,
    pub evidence: &'static str,
    release_on_drop: bool,
}
pub(super) async fn guard<T, F>(lease: Option<&Lease>, future: F) -> Api<T>
where
    F: std::future::Future<Output = Api<T>>,
{
    let Some(lease) = lease else {
        return future.await;
    };
    let mut failed = lease.failed.clone();
    if *failed.borrow() {
        return Err(Failure::new(
            503,
            "local resource lease could not be renewed",
        ));
    }
    tokio::select! {result=future=>result,_=async{loop{if failed.changed().await.is_err()||*failed.borrow(){break;}}}=>Err(Failure::new(503,"local resource lease could not be renewed"))}
}
impl Lease {
    pub fn begin(&self) {
        self.stopped.store(false, Ordering::Release);
    }
    pub fn complete(&mut self, evidence: &'static str) {
        self.stopped.store(true, Ordering::Release);
        self.evidence = evidence;
    }
    pub async fn release_confirmed(&mut self, evidence: &'static str) -> bool {
        if call(
            &self.g,
            &self.token,
            &format!("/permits/{}/release", self.id),
            json!({"stopped":true,"evidence":evidence}),
            false,
        )
        .await
        .is_ok()
        {
            self.complete(evidence);
            self.release_on_drop = false;
            self.renewal.abort();
            true
        } else {
            false
        }
    }
    pub async fn delegate(&mut self, task: &str, job: &str) -> bool {
        let result = call(
            &self.g,
            &self.token,
            &format!("/permits/{}/delegate", self.id),
            json!({"task_id":task,"app_task_id":job}),
            false,
        )
        .await;
        if result.is_ok_and(|v| v["state"] == "delegated" && v["task_id"] == task) {
            self.release_on_drop = false;
            self.renewal.abort();
            true
        } else {
            false
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.renewal.abort();
        if !self.release_on_drop {
            return;
        }
        let g = self.g.clone();
        let token = self.token.clone();
        let id = self.id.clone();
        let stopped = self.stopped.load(Ordering::Acquire);
        let evidence = if stopped {
            self.evidence
        } else {
            "transport_closed_without_stop_confirmation"
        };
        tokio::spawn(async move {
            let _ = call(
                &g,
                &token,
                &format!("/permits/{id}/release"),
                json!({"stopped":stopped,"evidence":evidence}),
                false,
            )
            .await;
        });
    }
}
pub(super) async fn call(
    g: &Gateway,
    token: &str,
    path: &str,
    body: Value,
    idempotent: bool,
) -> Api<Value> {
    let mut req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/v1{path}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if idempotent {
        req = req.header("idempotency-key", uuid::Uuid::new_v4().to_string());
    }
    let request = req
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .map_err(|_| Failure::new(503, "invalid internal admission request"))?;
    let response = tokio::time::timeout(Duration::from_secs(3), g.controller().oneshot(request))
        .await
        .map_err(|_| Failure::new(503, "scheduler admission timed out"))?
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .map_err(|_| Failure::new(503, "scheduler admission response unavailable"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Failure::new(503, "invalid scheduler admission response"))?;
    if !status.is_success() {
        let mut error = Failure::new(
            if status == StatusCode::TOO_MANY_REQUESTS {
                429
            } else {
                503
            },
            if value["code"] == "WAIT_STOP_CONFIRMATION" {
                "previous inference is awaiting native stop confirmation; recovery is in progress"
            } else if status == StatusCode::TOO_MANY_REQUESTS {
                "local resources busy; retry later"
            } else {
                "scheduler did not authorize local execution"
            },
        );
        error.code = value["code"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 96
                    && s.as_bytes()[0].is_ascii_uppercase()
                    && s.bytes()
                        .all(|c| c.is_ascii_uppercase() || c == b'_' || c.is_ascii_digit())
            })
            .map(str::to_owned);
        error.trace = value["request_id"].as_str().map(str::to_owned);
        return Err(error);
    }
    Ok(value["data"].clone())
}
fn bounded_config(_g: &Gateway, path: &Path) -> Api<Value> {
    private_json(path, 262144)
        .map_err(|_| Failure::new(503, "local scheduler binding is unavailable or invalid"))
}
pub(super) async fn classify_cloud(u: &mut Upstream) -> Api<()> {
    if env("V_ROUTER_SCHEDULER_REQUIRED", "") != "true" || u.kind != "cloud" {
        return Ok(());
    }
    let policy = u.scheduler.as_ref();
    if policy.is_some_and(|p| {
        p["mode"] == "cloud"
            && text(p, "upstream_url").trim_end_matches('/') == u.base_url.trim_end_matches('/')
    }) {
        return Ok(());
    }
    let url = reqwest::Url::parse(&u.base_url)
        .map_err(|_| Failure::new(503, "invalid cloud endpoint"))?;
    let host = url
        .host_str()
        .ok_or_else(|| Failure::new(503, "invalid cloud endpoint"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| Failure::new(503, "invalid cloud endpoint"))?;
    let addresses = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| Failure::new(502, "cloud origin resolution timed out"))?
    .map_err(|_| Failure::new(502, "cloud origin could not be resolved"))?;
    if addresses.into_iter().any(|a| match a.ip() {
        std::net::IpAddr::V4(a) => {
            a.is_private() || a.is_loopback() || a.is_link_local() || a.is_unspecified()
        }
        std::net::IpAddr::V6(a) => {
            a.is_loopback()
                || a.is_unique_local()
                || a.is_unicast_link_local()
                || a.is_unspecified()
                || a.to_ipv4_mapped()
                    .is_some_and(|a| a.is_private() || a.is_loopback() || a.is_link_local())
        }
    }) {
        u.kind = "manual".into();
    }
    Ok(())
}
pub(super) fn approval(
    g: &Gateway,
    u: &Upstream,
    model: &str,
    method: &str,
    path: &str,
    websocket: bool,
) -> Api<Option<Approval>> {
    let required = env("V_ROUTER_SCHEDULER_REQUIRED", "") == "true";
    let declared = u.scheduler.clone().unwrap_or_default();
    if required
        && declared["mode"] == "cloud"
        && text(&declared, "upstream_url").trim_end_matches('/') != u.base_url.trim_end_matches('/')
    {
        return Err(Failure::new(
            503,
            "cloud origin approval no longer matches upstream",
        ));
    }
    if u.kind == "cloud" && declared.get("mode").is_none() || declared["mode"] == "cloud" {
        let url = reqwest::Url::parse(&u.base_url)
            .map_err(|_| Failure::new(503, "invalid cloud endpoint"))?;
        let host = url.host_str().unwrap_or("");
        let private = host.parse::<std::net::IpAddr>().is_ok_and(|a| match a {
            std::net::IpAddr::V4(a) => a.is_private() || a.is_loopback() || a.is_link_local(),
            std::net::IpAddr::V6(a) => {
                a.is_loopback() || a.is_unique_local() || a.is_unicast_link_local()
            }
        }) || !host.contains('.');
        let bound = text(&declared, "upstream_url").trim_end_matches('/')
            == u.base_url.trim_end_matches('/');
        if !required || !private || bound {
            return Ok(None);
        }
    }
    let root = g.state_path.parent().unwrap();
    let key = format!("{}/{model}", u.id);
    let config = if u.source == "v-scheduler" {
        let hash = format!("{:x}", Sha256::digest(key.as_bytes()));
        let cfg = bounded_config(g, &root.join("ollama-bindings").join(hash + ".json"))?;
        if cfg["url"] != "http://v-scheduler:38120/api/v1"
            || cfg["require_model_policy"] != true
            || cfg["models"].as_object().is_none_or(|m| m.len() != 1)
            || !text(&cfg["models"][&key], "executor_id").starts_with("builtin-ollama-")
        {
            return Err(Failure::new(
                503,
                "invalid built-in model scheduler binding",
            ));
        }
        cfg
    } else if u.id == "app-v-voice" && root.join("voice-scheduler.local.json").exists() {
        bounded_config(g, &root.join("voice-scheduler.local.json"))?
    } else if u.source == "v-sub" {
        let snapshot = bounded_config(g, &root.join("vsub-scheduler.local.json"))?;
        let namespace = format!("{:x}", Sha256::digest(format!("v-sub\0{}", u.owner)));
        let owner_hash = format!("{:x}", Sha256::digest(u.owner.as_bytes()));
        if snapshot["protocol_version"] != 1
            || snapshot["app_id"] != "com.ictrek.v-sub"
            || u.id != format!("app-v-sub-{owner_hash}")
            || !u.base_url.ends_with(&format!("/inference/{namespace}/v1"))
            || u.owner.is_empty()
        {
            return Err(Failure::new(503, "invalid private V-Sub binding"));
        }
        let mut config = snapshot["namespaces"][&namespace].clone();
        let policies = config["models"]
            .as_object()
            .ok_or_else(|| Failure::new(503, "private namespace is not calibrated"))?;
        let policies = policies
            .iter()
            .map(|(k, v)| (format!("{}/{k}", u.id), v.clone()))
            .collect::<serde_json::Map<_, _>>();
        config["models"] = json!(policies);
        config
    } else if ["app-image-studio", "app-v-music", "app-desensitize"].contains(&u.id.as_str()) {
        let slug = u.id.trim_start_matches("app-");
        let cfg = bounded_config(g, &root.join("app-bindings").join(format!("{slug}.json")))?;
        let fixed = match slug {
            "image-studio" => "http://image-studio-backend:5000",
            "v-music" => "http://v-music-backend:7080/v1",
            _ => "http://desensitize-backend:5000/v1",
        };
        if u.base_url != fixed || cfg["url"] != "http://v-router:8080/scheduler/api/v1" {
            return Err(Failure::new(
                503,
                "application origin no longer matches binding",
            ));
        }
        cfg
    } else {
        let configured = env("V_ROUTER_SCHEDULER_CONFIG", "");
        let path = if configured.is_empty() {
            root.join("scheduler.local.json")
        } else {
            PathBuf::from(configured)
        };
        if !path.exists() && !required {
            return Ok(None);
        }
        bounded_config(g, &path)?
    };
    if text(&config, "token").len() < 32 {
        return Err(Failure::new(503, "invalid scheduler identity"));
    }
    let model_policy = config["models"].get(&key).cloned();
    let strict = config["require_model_policy"] == true || required;
    if strict && model_policy.is_none() && (method != "GET" || websocket) && method != "DELETE" {
        return Err(Failure::new(
            503,
            "local model resource policy not approved",
        ));
    }
    let policy = model_policy
        .or_else(|| config["upstreams"].get(&u.id).cloned())
        .or_else(|| u.scheduler.clone())
        .or_else(|| (u.kind == "local").then(|| config["default_local"].clone()))
        .ok_or_else(|| Failure::new(503, "declare scheduler policy for local upstream"))?;
    if method == "GET" && !websocket || method == "DELETE" || path.ends_with("/cancel") {
        return Ok(None);
    }
    if policy["mode"] == "cloud"
        && (!required
            || text(&policy, "upstream_url").trim_end_matches('/')
                == u.base_url.trim_end_matches('/'))
    {
        return Ok(None);
    }
    if !["permit", "delegated"].contains(&text(&policy, "mode"))
        || text(&policy, "executor_id").is_empty()
    {
        return Err(Failure::new(503, "local permit resource policy missing"));
    }
    Ok(Some(Approval { config, policy }))
}
pub(super) fn validate(
    approval: &Approval,
    u: &Upstream,
    value: &mut Value,
    bytes: usize,
) -> Api<()> {
    let p = &approval.policy;
    let automatic =
        u.source == "v-scheduler" && text(p, "executor_id").starts_with("builtin-ollama-auto-");
    let maximum = p["max_input_bytes"].as_u64().unwrap_or(0);
    if maximum == 0 || maximum > 32 << 20 {
        return Err(Failure::new(
            503,
            "model policy requires a bounded input budget within 32 MiB",
        ));
    }
    if bytes as u64 > maximum {
        return Err(Failure::new(
            413,
            "model input exceeds approved byte budget",
        ));
    }
    if u.source == "v-scheduler" {
        value["keep_alive"] = json!(0);
        if !automatic {
            if value["options"].is_null() {
                value["options"] = json!({});
            }
            if !value["options"].is_object() {
                return Err(Failure::new(422, "options must be an object"));
            }
            if value["options"]["num_ctx"].is_null() {
                value["options"]["num_ctx"] = p["limits"]["options.num_ctx"].clone();
            }
            if u.role == "generate" {
                if value["max_tokens"].is_null() && value["max_output_tokens"].is_null() {
                    value["max_tokens"] = p["limits"]["max_tokens"].clone();
                }
                if value["options"]["num_predict"].is_null() {
                    value["options"]["num_predict"] = p["limits"]["max_tokens"].clone();
                }
            }
        }
    }
    if let Some(limits) = p["limits"].as_object() {
        for (key, limit) in limits {
            let current = key.split('.').fold(value as &Value, |v, k| &v[k]);
            if let Some(n) = current.as_f64() {
                if !n.is_finite() || n < 0.0 || n > limit.as_f64().unwrap_or(0.0) {
                    return Err(Failure::new(
                        422,
                        format!("{key} exceeds approved model input budget"),
                    ));
                }
            }
        }
    }
    let secondary = text(value, "speaker_model");
    if !secondary.is_empty()
        && !strings(&p["secondary_models"])
            .iter()
            .any(|s| s == secondary)
    {
        return Err(Failure::new(
            422,
            "secondary model resource budget not approved",
        ));
    }
    if !automatic {
        for &key in RESOURCE_PARAMETERS {
            let current = value
                .get(key)
                .unwrap_or_else(|| key.split('.').fold(value as &Value, |v, k| &v[k]));
            if current.is_null() {
                continue;
            }
            let limit = p["limits"][key]
                .as_f64()
                .filter(|n| *n >= 0.0)
                .ok_or_else(|| {
                    Failure::new(
                        422,
                        format!("resource parameter requires an approved model limit: {key}"),
                    )
                })?;
            let number = current
                .as_f64()
                .or_else(|| current.as_str().and_then(|s| s.parse().ok()))
                .filter(|n| n.is_finite() && *n >= 0.0 && *n <= limit)
                .ok_or_else(|| {
                    Failure::new(
                        422,
                        format!("resource parameter exceeds approved model limit: {key}"),
                    )
                })?;
            let _ = number;
        }
        let size = text(value, "size");
        if !size.is_empty() && size != "auto" {
            let (width, height) = size.split_once('x').ok_or_else(|| {
                Failure::new(422, "image dimensions require an approved model limit")
            })?;
            for (key, raw) in [("width", width), ("height", height)] {
                let n = raw
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite() && *n > 0.0)
                    .ok_or_else(|| Failure::new(422, "invalid image dimensions"))?;
                if p["limits"][key].as_f64().is_none_or(|limit| limit < n) {
                    return Err(Failure::new(
                        422,
                        "image dimensions exceed approved model limit",
                    ));
                }
            }
        }
    }
    Ok(())
}
pub(super) async fn acquire(
    g: Arc<Gateway>,
    approval: &Approval,
    u: &Upstream,
    model: &str,
    headers: &HeaderMap,
    id: &Identity,
    asynchronous: bool,
) -> Api<Lease> {
    let token = text(&approval.config, "token").to_string();
    let policy = &approval.policy;
    let proof = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let executor = text(policy, "executor_id").to_string();
    let mut body = json!({"executor_id":executor,"upstream_url":u.base_url,"model_id":model,"execution_token":proof});
    let task = headers
        .get("x-vscheduler-task-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty());
    let path = if policy["mode"] == "delegated" && task.is_some() {
        let task = task.unwrap();
        let owner = owner_scope(&id.owner);
        if owner.is_empty() {
            return Err(Failure::new(
                503,
                "delegated execution requires verified owner",
            ));
        }
        body["task_id"] = json!(task);
        body["owner_id"] = json!(owner);
        "/delegations/verify"
    } else {
        if policy["mode"] == "delegated" && !asynchronous {
            return Err(Failure::new(
                503,
                "delegated execution requires an active task proof",
            ));
        }
        "/permits"
    };
    let response = call(&g, &token, path, body, true).await?;
    let permit = text(&response, "permit_id").to_string();
    if permit.is_empty() {
        return Err(Failure::new(503, "invalid local permit"));
    }
    let mut lease = resume(g, token, permit, proof, executor);
    lease.stopped.store(true, Ordering::Release);
    lease.evidence = "inference_request_not_started";
    Ok(lease)
}
pub(super) fn resume(
    g: Arc<Gateway>,
    token: String,
    permit: String,
    proof: String,
    executor: String,
) -> Lease {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let renew_g = g.clone();
    let renew_token = token.clone();
    let renew_id = permit.clone();
    let renewal = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if call(
                &renew_g,
                &renew_token,
                &format!("/permits/{renew_id}/renew"),
                json!({}),
                false,
            )
            .await
            .is_err()
            {
                let _ = tx.send(true);
                break;
            }
        }
    });
    Lease {
        g,
        token,
        id: permit,
        proof,
        executor,
        renewal,
        failed: rx,
        stopped: Arc::new(AtomicBool::new(false)),
        evidence: "transport_closed_without_stop_confirmation",
        release_on_drop: true,
    }
}
