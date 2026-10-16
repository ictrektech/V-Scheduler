//! In-process inference gateway. The controller and gateway share one Rust
//! runtime; the old TCP listeners and persisted gateway schema stay compatible.
mod adapters;
mod auth;
mod bridge;
mod discovery;
mod jobs;
mod lease;
mod manage;
mod proxy;
mod registry;
#[cfg(test)]
mod tests;
mod usage;
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use registry::{Model, Registry, Upstream};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct Identity {
    owner: String,
    username: String,
    email: String,
    admin: bool,
    service_app: String,
}
#[derive(Debug)]
struct Failure {
    status: StatusCode,
    message: String,
    code: Option<String>,
    messages: bool,
    trace: Option<String>,
}
impl Failure {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            message: message.into(),
            code: None,
            messages: false,
            trace: None,
        }
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let code = self
            .code
            .as_deref()
            .unwrap_or(crate::contract::code(self.status));
        let kind = match self.status.as_u16() {
            400 | 422 => "invalid_request_error",
            401 => "authentication_error",
            403 => "permission_error",
            429 => "rate_limit_error",
            _ => "api_error",
        };
        let mut value = crate::contract::envelope(json!({}), code, &self.message);
        value["error"] = json!({"code":code,"message":self.message,"type":kind});
        if self.messages {
            value["type"] = json!("error");
        }
        if let Some(trace) = self.trace.filter(|s| {
            !s.is_empty()
                && s.len() <= 96
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        }) {
            value["request_id"] = json!(trace);
        }
        let trace = text(&value, "request_id").to_string();
        let mut response = (self.status, axum::Json(value)).into_response();
        response
            .headers_mut()
            .insert("x-request-id", trace.parse().unwrap());
        if self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert("www-authenticate", "Bearer".parse().unwrap());
        }
        if self.status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert("retry-after", "1".parse().unwrap());
        }
        response
    }
}
type Api<T> = Result<T, Failure>;
struct Gateway {
    registry: Mutex<Registry>,
    state_path: PathBuf,
    controller: RwLock<Router>,
    client: reqwest::Client,
    identities: Mutex<std::collections::HashMap<String, (Identity, std::time::Instant)>>,
    usage: Mutex<usage::Usage>,
    slots: Arc<tokio::sync::Semaphore>,
    buffered: Arc<tokio::sync::Semaphore>,
    jobs_lock: Mutex<()>,
    ui: Option<PathBuf>,
}
fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}
fn number(key: &str, default: usize) -> usize {
    env(key, "").parse().unwrap_or(default)
}
fn version() -> String {
    env("VOS_APP_VERSION", env!("CARGO_PKG_VERSION"))
}
fn owner_scope(owner: &str) -> String {
    let mut out = String::new();
    let mut invalid = false;
    for c in owner.trim().chars() {
        if c.is_ascii_alphanumeric() || "_.-".contains(c) {
            out.push(c);
            invalid = false;
        } else if !invalid {
            out.push('-');
            invalid = true;
        }
        if out.len() >= 80 {
            break;
        }
    }
    out
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
}
async fn read_limited(mut response: reqwest::Response, limit: usize) -> Api<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(Failure::new(
            502,
            "upstream response exceeds the gateway limit",
        ));
    }
    let mut bytes = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Failure::new(502, "upstream response unavailable"))?
    {
        if bytes.len() + chunk.len() > limit {
            return Err(Failure::new(
                502,
                "upstream response exceeds the gateway limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn read_json(response: reqwest::Response, limit: usize) -> Api<Value> {
    serde_json::from_slice(&read_limited(response, limit).await?)
        .map_err(|_| Failure::new(502, "invalid upstream JSON response"))
}
fn safe_secret_eq(a: &str, b: &str) -> bool {
    use sha2::{Digest, Sha256};
    let a = Sha256::digest(a.as_bytes());
    let b = Sha256::digest(b.as_bytes());
    a.iter().zip(b).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}
fn private_json(path: &Path, limit: u64) -> anyhow::Result<Value> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        meta.is_file() && meta.permissions().mode() & 0o077 == 0 && meta.len() <= limit,
        "private bounded binding required"
    );
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
impl Gateway {
    fn controller(&self) -> Router {
        self.controller.read().unwrap().clone()
    }
}
pub(crate) struct Runtime {
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
    background: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    state: Arc<Gateway>,
    ready: AtomicBool,
}
impl Runtime {
    pub fn abort(self) {
        self.server.abort();
        for task in self.background.into_inner().unwrap() {
            task.abort();
        }
    }
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub fn set_controller(&self, router: Router, ready: bool) {
        *self.state.controller.write().unwrap() = router;
        if ready && !self.ready.swap(true, Ordering::AcqRel) {
            self.background
                .lock()
                .unwrap()
                .extend(jobs::resume(self.state.clone()));
        }
    }
}
pub(crate) async fn start(
    controller: Router,
    ui: Option<PathBuf>,
) -> anyhow::Result<Option<Runtime>> {
    let listen = env("V_ROUTER_ADDR", "");
    if listen.is_empty() {
        return Ok(None);
    }
    let listen = if listen.starts_with(':') {
        format!("0.0.0.0{listen}")
    } else {
        listen
    };
    let state_path = PathBuf::from(env("V_ROUTER_STATE_PATH", "/gateway-data/state.json"));
    let state = Arc::new(Gateway {
        registry: Mutex::new(Registry::open(&state_path)?),
        usage: Mutex::new(usage::Usage::open(&PathBuf::from(env(
            "V_ROUTER_USAGE_PATH",
            "/gateway-data/usage.jsonl",
        )))?),
        state_path,
        controller: RwLock::new(controller),
        ui,
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()?,
        identities: Mutex::new(Default::default()),
        slots: Arc::new(tokio::sync::Semaphore::new(number(
            "V_ROUTER_MAX_ACTIVE_REQUESTS",
            8,
        ))),
        buffered: Arc::new(tokio::sync::Semaphore::new(
            number("V_ROUTER_MAX_BUFFERED_INPUT_MIB", 64) * 1024,
        )),
        jobs_lock: Mutex::new(()),
    });
    let router = Router::new().fallback(handle).with_state(state.clone());
    let socket = tokio::net::TcpListener::bind(&listen).await?;
    let background = discovery::start(state.clone());
    eprintln!("V-Scheduler Rust router listening on {listen}");
    Ok(Some(Runtime {
        server: tokio::spawn(async move {
            axum::serve(socket, router).await?;
            Ok(())
        }),
        background: Mutex::new(background),
        state,
        ready: AtomicBool::new(false),
    }))
}
async fn handle(State(g): State<Arc<Gateway>>, request: Request) -> Response {
    let messages = request.uri().path().starts_with("/v1/messages");
    match dispatch(g, request).await {
        Ok(response) => response,
        Err(mut e) => {
            e.messages = messages;
            e.into_response()
        }
    }
}
fn route(path: &str, method: &str) -> Option<Value> {
    route_specs()
        .iter()
        .filter(|r| text(r, "method") == method && matches_path(text(r, "path"), path))
        .max_by_key(|r| {
            let pattern = text(r, "path");
            (
                if pattern.contains("...") { 0 } else { 1 },
                pattern.chars().filter(|c| *c == '/').count(),
                pattern.len(),
            )
        })
        .cloned()
}
fn route_specs() -> &'static [Value] {
    static ROUTES: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    ROUTES.get_or_init(|| {
        serde_json::from_str(include_str!("../../../api/router-routes.json")).unwrap()
    })
}
fn matches_path(pattern: &str, path: &str) -> bool {
    let mut actual = path.split('/');
    for part in pattern.split('/') {
        if part.starts_with('{') && part.ends_with("...}") {
            return actual.next().is_some();
        }
        let Some(value) = actual.next() else {
            return false;
        };
        if part.starts_with('{') && part.ends_with('}') {
            if value.is_empty() {
                return false;
            }
        } else if part != value {
            return false;
        }
    }
    actual.next().is_none()
}
async fn dispatch(g: Arc<Gateway>, mut request: Request) -> Api<Response> {
    let path = request.uri().path().to_string();
    if path == "/healthz" {
        return Ok((StatusCode::OK, "ok").into_response());
    }
    if path.starts_with("/scheduler/") {
        let suffix = request
            .uri()
            .path_and_query()
            .unwrap()
            .as_str()
            .trim_start_matches("/scheduler");
        *request.uri_mut() = suffix
            .parse()
            .map_err(|_| Failure::new(400, "invalid controller path"))?;
        return Ok(g.controller().oneshot(request).await.unwrap());
    }
    if path == "/api/v1/auth-probe" {
        return auth::probe(&g).await;
    }
    let spec = if path.starts_with("/model-hub/") {
        None
    } else {
        route(&path, request.method().as_str())
    };
    if spec.is_none() && (path.starts_with("/v1/") || path.starts_with("/api/")) {
        if ["GET", "POST", "PUT", "PATCH", "DELETE"]
            .iter()
            .any(|m| route(&path, m).is_some())
        {
            return Err(Failure::new(405, "method not allowed"));
        }
        return Err(Failure::new(404, "unknown API endpoint"));
    }
    // The UI must load before JavaScript can obtain a VOS token. Only API
    // routes pass through identity verification; static assets are public.
    if spec.is_none() && !path.starts_with("/model-hub/") {
        if let Some(ui) = &g.ui {
            let response = tower_http::services::ServeDir::new(ui)
                .oneshot(request)
                .await
                .unwrap();
            return Ok(response.map(Body::new));
        }
        return Err(Failure::new(404, "not found"));
    }
    let management =
        spec.as_ref().is_some_and(|r| r["auth"] == "manage") || path.starts_with("/model-hub/api/");
    let id = auth::identity(&g, request.headers(), management).await?;
    if spec.as_ref().is_some_and(|r| r["group"] == "resource")
        && !request.headers().contains_key("x-vrouter-upstream")
    {
        return Err(Failure::new(
            400,
            "resource APIs require X-VRouter-Upstream",
        ));
    }
    if path.starts_with("/model-hub/") {
        return proxy::management(&g, request, &path).await;
    }
    if path.starts_with("/api/v1/")
        && !path.starts_with("/api/v1/scenes/")
        && !path.contains("video-structure/status")
    {
        return manage::handle(&g, &id, request).await;
    }
    if path.starts_with("/v1/")
        || path.starts_with("/api/v2/")
        || path == "/api/v1/scenes/detect"
        || path == "/api/v1/video-structure/status"
    {
        if !id.service_app.is_empty()
            && !(path == "/v1/models" && request.method() == "GET"
                || request.method() == "POST"
                    && ["/v1/chat/completions", "/v1/embeddings", "/v1/rerank"]
                        .contains(&path.as_str()))
        {
            return Err(Failure::new(
                403,
                "application credential does not permit this endpoint",
            ));
        }
        return proxy::handle(g, id, request).await;
    }
    if path.starts_with("/api/") {
        return Err(Failure::new(404, "unknown API endpoint"));
    }
    if let Some(ui) = &g.ui {
        let response = tower_http::services::ServeDir::new(ui)
            .oneshot(request)
            .await
            .unwrap();
        return Ok(response.map(Body::new));
    }
    Err(Failure::new(404, "not found"))
}
