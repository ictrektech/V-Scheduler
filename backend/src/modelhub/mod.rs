mod archive;
mod download;
mod runtime;
mod scan;
mod specification;
mod storage;
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    sync::Arc,
};
use storage::*;
type Active = tokio::sync::Mutex<HashMap<String, (String, tokio::sync::watch::Sender<bool>)>>;
pub struct Hub {
    store: Store,
    client: reqwest::Client,
    active: Active,
    model_locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::RwLock<()>>>>,
}
static ACTIVE_HUB: std::sync::OnceLock<std::sync::Weak<Hub>> = std::sync::OnceLock::new();
fn model_lock_key(id:&str)->String {
    if let Some(name)=id.strip_prefix("ollama://") {
        if !name.rsplit('/').next().unwrap_or("").contains(':') {return format!("{id}:latest");}
    }
    id.into()
}
impl Hub {
    async fn model_lock(&self,id:&str)->Arc<tokio::sync::RwLock<()>> {
        self.model_locks.lock().await.entry(model_lock_key(id))
            .or_insert_with(||Arc::new(tokio::sync::RwLock::new(()))).clone()
    }
    async fn lock_model(&self,id:&str)->tokio::sync::OwnedRwLockWriteGuard<()> {
        self.model_lock(id).await.write_owned().await
    }
    async fn lock_model_change(&self,id:&str)->Result<tokio::sync::OwnedRwLockWriteGuard<()>> {
        let lock=self.model_lock(id).await;
        if id.starts_with("ollama://") && std::env::var("V_ROUTER_ADDR").is_ok_and(|v|!v.is_empty()) {
            return lock.try_write_owned().map_err(|_|anyhow::anyhow!("model has queued or active inference; stop its requests before changing weights"));
        }
        Ok(lock.write_owned().await)
    }
}
pub(crate) async fn inference_guard(native:&str)->Result<Option<tokio::sync::OwnedRwLockReadGuard<()>>> {
    let Some(hub)=ACTIVE_HUB.get().and_then(std::sync::Weak::upgrade) else {return Ok(None)};
    let id=model_lock_key(&format!("ollama://{native}"));
    let guard=hub.model_lock(&id).await.try_read_owned()
        .map_err(|_|anyhow::anyhow!("model weights are being changed; retry after model management completes"))?;
    ensure!(!hub.active.lock().await.values().any(|(model,_)|model_lock_key(model)==id),"model weights are downloading; retry after the download task completes");
    Ok(Some(guard))
}

struct Error(StatusCode, String);
impl From<anyhow::Error> for Error {
    fn from(error: anyhow::Error) -> Self {
        let message = error.to_string();
        let safe = if message.contains("http") || error.downcast_ref::<reqwest::Error>().is_some() {
            "Upstream request failed; verify endpoint, permissions and network.".into()
        } else {
            message
        };
        let status = if safe.contains("not found") {
            StatusCode::NOT_FOUND
        } else if safe.starts_with("invalid ") || safe.starts_with("missing ") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::CONFLICT
        };
        Self(status, safe)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        crate::contract::failure(self.0, crate::contract::code(self.0), &self.1, true)
    }
}
type Api<T> = std::result::Result<T, Error>;
fn bad(text: &str) -> Error {
    Error(StatusCode::BAD_REQUEST, text.into())
}
fn missing(text: &str) -> Error {
    Error(StatusCode::NOT_FOUND, text.into())
}
fn parameter<'a>(query: &'a HashMap<String, String>, key: &str) -> Api<&'a str> {
    query
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| bad(&format!("missing {key}")))
}
pub async fn start(listen: String) -> Result<tokio::task::JoinHandle<Result<()>>> {
    let socket = tokio::net::TcpListener::bind(&listen).await?;
    let store = Store::new(download::env("MODELS_BASE_DIR", "/data").into())?;
    let hub = Arc::new(Hub {
        store,
        client: reqwest::Client::builder()
            .user_agent("v-scheduler-model-hub")
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()?,
        active: Default::default(),
        model_locks: Default::default(),
    });
    let _=ACTIVE_HUB.set(Arc::downgrade(&hub));
    // Do not mark another application's active tasks failed. Only recover our own interrupted workers.
    for mut task in hub.store.tasks()? {
        if task["_owner"] == "v-scheduler" && !terminal(&task) {
            if task["_previous"].is_object() {
                hub.store.put_model(task["_previous"].clone())?;
            } else if let Ok(mut model) = hub.store.model(task["model_id"].as_str().unwrap_or("")) {
                if model["status"] == "pulling" {
                    model["status"] = json!("failed");
                    hub.store.put_model(model)?;
                }
            }
            task["phase"] = json!("FAILED");
            task["error_msg"] = json!("Worker restarted. Retry to resume the download.");
            hub.store.put_task(&task)?;
        }
    }
    let scanner = hub.clone();
    tokio::spawn(async move {
        loop {
            let h = scanner.clone();
            let _ = tokio::task::spawn_blocking(move || scan::sync(&h)).await;
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
    Ok(tokio::spawn(async move {
        axum::serve(socket, router(hub))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        Ok(())
    }))
}
pub async fn serve(listen: String) -> Result<()> {
    start(listen).await?.await?
}
fn router(hub: Arc<Hub>) -> Router {
    Router::new()
 .route("/health",get(||async {Json(json!({"status":"ok","implementation":"v-scheduler-rust"}))}))
 .route("/api/v1/models",get(models).delete(delete_model))
 .route("/api/v1/models/pull",post(pull))
 .route("/api/v1/models/update",post(update))
 .route("/api/v1/models/check-update",post(check_update))
 .route("/api/v1/models/check-updates",post(check_updates))
 .route("/api/v1/models/sync",post(sync))
 .route("/api/v1/models/cleanup",post(cleanup))
 .route("/api/v1/models/files",get(files))
 .route("/api/v1/models/repository-files",get(remote_files))
 .route("/api/v1/models/import-archive",post(import).layer(DefaultBodyLimit::disable()))
 .route("/api/v1/tasks",get(tasks))
 .route("/api/v1/tasks/detail/{id}",get(task))
 .route("/api/v1/tasks/{id}",delete(delete_task))
 .route("/api/v1/tasks/{id}/cancel",post(cancel_task))
 .route("/api/v1/tasks/{id}/retry",post(retry_task))
 .route("/api/v1/aliases",get(aliases).post(create_alias))
 .route("/api/v1/aliases/{id}",delete(delete_alias))
 .route("/api/v1/containers",get(containers))
 .route("/api/v1/containers/bind",post(bind))
 .route("/api/v1/containers/unbind",delete(unbind))
 .route("/api/v1/inference-services",get(services))
 .route("/api/v1/inference-services/register",post(register))
 .route("/api/v1/inference-services/{id}",get(service).delete(delete_service))
 .route("/api/v1/inference-services/{id}/models/start",post(start_service))
 .route("/api/v1/inference-services/{id}/models/stop",post(stop_service))
 .route("/api/v1/ollama/runtime",get(runtimes))
 .route("/api/v1/ollama/runtime/start",post(start_runtime))
 .route("/api/v1/ollama/runtime/stop",post(stop_runtime))
 .route("/api/v1/integration",get(integration))
 .route("/api/v1/version",get(||async{Json(specification::version())}))
 .route("/openapi.json",get(||async{Json(specification::openapi())}))
 .route("/docs",get(||async{axum::response::Html("<!doctype html><html><meta charset=utf-8><title>Model management API</title><h1>V-Scheduler model management API</h1><p>Open the Model management API tab in V-Scheduler for the guide and interactive operation reference.</p><a href=\"openapi.json\">OpenAPI 3.1 JSON</a></html>")}))
 .layer(DefaultBodyLimit::max(2*1024*1024))
 .layer(axum::middleware::from_fn(crate::contract::normalize_errors)).layer(tower_http::cors::CorsLayer::permissive()).with_state(hub)
}
async fn models(
    State(h): State<Arc<Hub>>,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Json<Value>> {
    let status = q.get("status").map(String::as_str).unwrap_or("ready");
    if !["all", "ready", "pulling", "failed", "deleting"].contains(&status) {
        return Err(bad("invalid status"));
    }
    let mut values = vec![];
    for (id, model) in h.store.entries("models")?.as_object().unwrap() {
        if status != "all" && model["status"] != status {
            continue;
        }
        let mut model = model.clone();
        model["model_id"] = json!(id);
        if let Ok(path) = h.store.access(&model) {
            model["access_path"] = json!(if std::path::Path::new(
                model["path"].as_str().unwrap_or("")
            )
            .join("current")
            .exists()
            {
                format!("{}/current", model["path"].as_str().unwrap())
            } else {
                path.to_string_lossy().into_owned()
            });
        }
        values.push(model);
    }
    Ok(Json(json!(values)))
}
async fn pull(State(h): State<Arc<Hub>>, Json(mut v): Json<Value>) -> Api<Json<Value>> {
    if let Some(fields) = v.as_object_mut() {
        fields.retain(|key, _| !key.starts_with('_'));
    }
    Ok(Json(download::create(h, v).await?))
}
async fn update(
    State(h): State<Arc<Hub>>,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Json<Value>> {
    let m = h.store.model(parameter(&q, "model_id")?)?;
    if m["status"] != "ready" {
        return Err(bad("only ready models can be updated"));
    }
    Ok(Json(
        download::create(
            h,
            json!({"model_id":m["model_id"],"source":m["source"],"force":true,"files":m["files"]}),
        )
        .await?,
    ))
}
async fn check(h: &Hub, id: &str) -> Result<Value> {
    let m = h.store.model(id)?;
    let source = m["source"].as_str().unwrap_or("");
    let current = m["version"].as_str().unwrap_or("");
    let latest = if source == "ollama" {
        let name = id.trim_start_matches("ollama://");
        let (name, tag) = name.rsplit_once(':').unwrap_or((name, "latest"));
        let name = if name.contains('/') {
            name.to_owned()
        } else {
            format!("library/{name}")
        };
        let r = h
            .client
            .get(format!(
                "{}/v2/{name}/manifests/{tag}",
                download::env("OLLAMA_REGISTRY_URL", "https://registry.ollama.ai")
            ))
            .send()
            .await?
            .error_for_status()?;
        r.headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned()
    } else {
        download::remote(h, source, id).await?["version"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    };
    Ok(
        json!({"model_id":id,"has_update":!latest.is_empty()&&latest!=current,"current_version":current,"latest_version":latest}),
    )
}
async fn check_update(
    State(h): State<Arc<Hub>>,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Json<Value>> {
    Ok(Json(check(&h, parameter(&q, "model_id")?).await?))
}
async fn check_updates(State(h): State<Arc<Hub>>) -> Api<Json<Value>> {
    let mut values = vec![];
    for (id, m) in h.store.entries("models")?.as_object().unwrap() {
        if m["status"] == "ready" {
            let value=check(&h,id).await.unwrap_or(json!({"model_id":id,"error":"update check unavailable","has_update":false,"current_version":m["version"],"latest_version":null}));
            values.push(value);
        }
    }
    Ok(Json(json!({"results":values})))
}
async fn sync(State(h): State<Arc<Hub>>) -> Api<Json<Value>> {
    let result = tokio::task::spawn_blocking(move || scan::sync(&h))
        .await
        .map_err(|_| bad("scan failed"))??;
    Ok(Json(result))
}
async fn files(
    State(h): State<Arc<Hub>>,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Json<Value>> {
    let m = h.store.model(parameter(&q, "model_id")?)?;
    let items = h.store.local_files(&m)?;
    Ok(Json(
        json!({"model_id":m["model_id"],"source":m["source"],"total_size":items.iter().map(|v|v["size"].as_u64().unwrap_or(0)).sum::<u64>(),"files":items}),
    ))
}
async fn remote_files(
    State(h): State<Arc<Hub>>,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Json<Value>> {
    Ok(Json(
        download::remote(&h, parameter(&q, "source")?, parameter(&q, "model_id")?).await?,
    ))
}
async fn tasks(State(h): State<Arc<Hub>>) -> Api<Json<Value>> {
    let mut seen = HashSet::new();
    Ok(Json(json!(
        h.store
            .tasks()?
            .into_iter()
            .filter(|v| seen.insert(v["model_id"].clone().to_string()))
            .map(public_task)
            .collect::<Vec<_>>()
    )))
}
async fn task(State(h): State<Arc<Hub>>, Path(id): Path<String>) -> Api<Json<Value>> {
    Ok(Json(public_task(
        h.store.task(&id).map_err(|_| missing("task not found"))?,
    )))
}
async fn cancel(h: &Hub, id: &str) -> Result<bool> {
    let task = h.store.task(id)?;
    if terminal(&task) {
        return Ok(false);
    }
    let active = h.active.lock().await;
    if let Some((_, tx)) = active.get(id) {
        let _ = tx.send(true);
        drop(active);
        for _ in 0..200 {
            if !h.active.lock().await.contains_key(id) {
                return Ok(true);
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        anyhow::bail!("waiting for worker cancellation");
    }
    drop(active);
    if terminal(&h.store.task(id)?) {
        return Ok(false);
    }
    anyhow::bail!("task belongs to another application; cancel it in its owner first")
}
async fn cancel_task(State(h): State<Arc<Hub>>, Path(id): Path<String>) -> Api<Json<Value>> {
    Ok(Json(
        json!({"task_id":id,"cancelled":cancel(&h,&id).await?}),
    ))
}
async fn delete_task(State(h): State<Arc<Hub>>, Path(id): Path<String>) -> Api<Json<Value>> {
    cancel(&h, &id).await?;
    fs::remove_file(h.store.task_path(&id)?).map_err(anyhow::Error::from)?;
    Ok(Json(json!({"task_id":id,"deleted":true})))
}
async fn retry_task(State(h): State<Arc<Hub>>, Path(id): Path<String>) -> Api<Json<Value>> {
    let t = h.store.task(&id)?;
    if t["phase"] != "FAILED" {
        return Err(bad("only failed tasks can be retried"));
    }
    let mut request = t["_request"].clone();
    if !request.is_object() {
        request = json!({"model_id":t["model_id"],"source":t["model_id"].as_str().unwrap_or("").split("://").next().unwrap_or(""),"files":t["files"],"append":t["append"]});
    }
    request["force"] = json!(true);
    request["_resume"] = t["_request"]["_resume"]
        .as_str()
        .map(|s| json!(s))
        .unwrap_or(json!(id));
    Ok(Json(download::create(h, request).await?))
}
async fn aliases(State(h): State<Arc<Hub>>) -> Api<Json<Value>> {
    Ok(Json(json!(
        h.store
            .entries("aliases")?
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| json!({"alias":k,"target_model_id":v}))
            .collect::<Vec<_>>()
    )))
}
async fn create_alias(State(h): State<Arc<Hub>>, Json(v): Json<Value>) -> Api<Json<Value>> {
    let alias = v["alias"]
        .as_str()
        .filter(|x| !x.trim().is_empty())
        .ok_or_else(|| bad("alias is required"))?;
    let target = v["target_model_id"]
        .as_str()
        .ok_or_else(|| bad("target_model_id is required"))?;
    h.store.model(target)?;
    h.store.registry("aliases", |data| {
        ensure!(data[alias].is_null(), "alias already exists");
        data[alias] = json!(target);
        Ok(())
    })?;
    Ok(Json(v))
}
async fn delete_alias(State(h): State<Arc<Hub>>, Path(id): Path<String>) -> Api<Json<Value>> {
    let target = h.store.registry("aliases", |data| {
        data.as_object_mut()
            .unwrap()
            .remove(&id)
            .ok_or_else(|| anyhow::anyhow!("alias not found"))
    })?;
    Ok(Json(json!({"alias":id,"target_model_id":target})))
}
async fn containers(
    State(h): State<Arc<Hub>>,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Json<Value>> {
    Ok(Json(json!(
        h.store
            .entries("containers")?
            .as_object()
            .unwrap()
            .values()
            .filter(|v| q.get("model_id").is_none_or(|id| v["model_id"] == *id))
            .collect::<Vec<_>>()
    )))
}
async fn bind(State(h): State<Arc<Hub>>, Json(mut v): Json<Value>) -> Api<Json<Value>> {
    let model = h.store.model(v["model_id"].as_str().unwrap_or(""))?;
    let canonical = model["model_id"].as_str().unwrap();
    let _guard = h.lock_model(canonical).await;
    h.store.model(canonical)?;
    v["model_id"] = json!(canonical);
    let id = v["container_id"]
        .as_str()
        .filter(|x| !x.is_empty())
        .ok_or_else(|| bad("container_id is required"))?
        .to_owned();
    if v["ref_count"].is_null() {
        v["ref_count"] = json!(1);
    }
    if v["ref_count"].as_u64().unwrap_or(0) < 1 {
        return Err(bad("invalid ref_count"));
    }
    v["mounted_at"] = json!(now());
    h.store.registry("containers", |data| {
        data[&id] = v.clone();
        Ok(())
    })?;
    Ok(Json(v))
}
async fn unbind(State(h): State<Arc<Hub>>, Json(v): Json<Value>) -> Api<Json<Value>> {
    let id = v["container_id"]
        .as_str()
        .ok_or_else(|| bad("container_id is required"))?;
    let record = h.store.registry("containers", |data| {
        data.as_object_mut()
            .unwrap()
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("binding not found"))
    })?;
    Ok(Json(
        json!({"container_id":id,"model_id":record["model_id"],"ref_count":0}),
    ))
}
async fn services(
    State(h): State<Arc<Hub>>,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Json<Value>> {
    Ok(Json(json!(runtime::services(
        &h,
        q.get("include_builtin").is_some_and(|x| x == "true")
    )?)))
}
async fn register(State(h): State<Arc<Hub>>, Json(v): Json<Value>) -> Api<Json<Value>> {
    let mut value = runtime::validate_registration(v)?;
    if let Some(links)=value["metadata"]["model_bindings"].as_object() {
        for target in links.values() {h.store.model(target.as_str().unwrap())?;}
    }
    let id = value["service_id"].as_str().unwrap().to_owned();
    h.store.registry("inference_services", |data| {
        value["registered_at"] = data[&id]["registered_at"]
            .as_str()
            .map(|s| json!(s))
            .unwrap_or(json!(now()));
        data[&id] = value.clone();
        Ok(())
    })?;
    Ok(Json(value))
}
async fn service(State(h): State<Arc<Hub>>, Path(id): Path<String>) -> Api<Json<Value>> {
    Ok(Json(
        runtime::inspect(&h, &runtime::service(&h, &id)?).await,
    ))
}
async fn delete_service(State(h): State<Arc<Hub>>, Path(id): Path<String>) -> Api<Json<Value>> {
    h.store.registry("inference_services", |data| {
        ensure!(
            data.as_object_mut().unwrap().remove(&id).is_some(),
            "service not found"
        );
        Ok(())
    })?;
    Ok(Json(json!({"service_id":id,"deleted":true})))
}
async fn start_service(
    State(h): State<Arc<Hub>>,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> Api<Json<Value>> {
    Ok(Json(
        runtime::action(&h, &runtime::service(&h, &id)?, v, true).await?,
    ))
}
async fn stop_service(
    State(h): State<Arc<Hub>>,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> Api<Json<Value>> {
    Ok(Json(
        runtime::action(&h, &runtime::service(&h, &id)?, v, false).await?,
    ))
}
async fn runtimes(State(h): State<Arc<Hub>>) -> Api<Json<Value>> {
    let results = futures_util::future::join_all(["qa", "embedding", "rerank"].map(|id| {
        let h = h.clone();
        async move { runtime::inspect(&h, &runtime::runtime(id).unwrap()).await }
    }))
    .await;
    Ok(Json(json!({"runtimes":results})))
}
async fn start_runtime(State(h): State<Arc<Hub>>, Json(v): Json<Value>) -> Api<Json<Value>> {
    Ok(Json(
        runtime::action(
            &h,
            &runtime::runtime(v["runtime"].as_str().unwrap_or(""))?,
            v.clone(),
            true,
        )
        .await?,
    ))
}
async fn stop_runtime(State(h): State<Arc<Hub>>, Json(v): Json<Value>) -> Api<Json<Value>> {
    Ok(Json(
        runtime::action(
            &h,
            &runtime::runtime(v["runtime"].as_str().unwrap_or(""))?,
            v.clone(),
            false,
        )
        .await?,
    ))
}
async fn import(State(h): State<Arc<Hub>>, mut multipart: Multipart) -> Api<Json<Value>> {
    let result = archive::upload(h, &mut multipart).await?;
    Ok(Json(result))
}
async fn delete_model(
    State(h): State<Arc<Hub>>,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Json<Value>> {
    delete_model_impl(&h, parameter(&q, "model_id")?)
        .await
        .map(Json)
        .map_err(Into::into)
}
async fn delete_model_impl(h: &Hub, model_id: &str) -> Result<Value> {
    let canonical = h.store.model(model_id)?["model_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let _guard = h.lock_model_change(&canonical).await?;
    let model = h.store.model(model_id)?;
    let id = model["model_id"].as_str().unwrap();
    ensure!(
        !h.active.lock().await.values().any(|(m, _)| m == id),
        "model has active download"
    );
    for task in h.store.tasks()? {
        ensure!(
            task["model_id"] != id || terminal(&task),
            "model has an active task"
        );
    }
    ensure!(
        !h.store
            .entries("containers")?
            .as_object()
            .unwrap()
            .values()
            .any(|v| v["model_id"] == id),
        "model is referenced by a container"
    );
    let freed = if model["source"] == "ollama" {
        crate::router::prepare_model_change(id.trim_start_matches("ollama://")).await?;
        let r = h
            .client
            .delete(format!(
                "{}/api/delete",
                runtime::runtime("qa")?["base_url"].as_str().unwrap()
            ))
            .json(&json!({"model":id.trim_start_matches("ollama://")}))
            .send()
            .await
            .map_err(anyhow::Error::from)?;
        ensure!(r.status().is_success(), "Ollama delete failed");
        model["size"].as_u64().unwrap_or(0)
    } else {
        let root = std::path::Path::new(model["path"].as_str().unwrap())
            .canonicalize()
            .map_err(anyhow::Error::from)?;
        let shared = h.store.root.canonicalize().map_err(anyhow::Error::from)?;
        ensure!(
            root.starts_with(&shared)
                && root != shared
                && root.components().count() >= shared.components().count() + 3,
            "unsafe model delete path"
        );
        let bytes = size(&root);
        fs::remove_dir_all(root).map_err(anyhow::Error::from)?;
        bytes
    };
    h.store.registry("models", |data| {
        data.as_object_mut().unwrap().remove(id);
        Ok(())
    })?;
    h.store.registry("aliases", |data| {
        data.as_object_mut().unwrap().retain(|_, v| v != id);
        Ok(())
    })?;
    if model["source"]=="ollama" {crate::router::forget_builtin_model(id.trim_start_matches("ollama://"));}
    Ok(json!({"model_id":id,"deleted":true,"freed_bytes":freed,"message":"Model deleted"}))
}
async fn cleanup(State(h): State<Arc<Hub>>) -> Api<Json<Value>> {
    let mut count = 0;
    let mut bytes = 0;
    for e in fs::read_dir(h.store.root.join("shared/tmp")).map_err(anyhow::Error::from)? {
        let e = e.map_err(anyhow::Error::from)?;
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("task-") {
            continue;
        }
        if h.store.task(&name).map_or(true, |t| terminal(&t))
            && !h.active.lock().await.contains_key(&name)
        {
            bytes += size(&e.path());
            fs::remove_dir_all(e.path()).map_err(anyhow::Error::from)?;
            count += 1;
        }
    }
    Ok(Json(json!({"cleaned_count":count,"freed_bytes":bytes})))
}
async fn integration() -> Json<Value> {
    Json(specification::integration())
}

#[cfg(test)]
mod mutation_tests {
    use super::*;
    #[tokio::test]
    async fn latest_alias_shares_inference_mutation_lock() {
        assert_eq!(model_lock_key("ollama://org/model"), model_lock_key("ollama://org/model:latest"));
        assert_ne!(model_lock_key("ollama://model:2b"), model_lock_key("ollama://model:latest"));
        let lock = Arc::new(tokio::sync::RwLock::new(()));
        let reader = lock.clone().try_read_owned().unwrap();
        assert!(lock.clone().try_write_owned().is_err());
        drop(reader);
        let writer = lock.clone().try_write_owned().unwrap();
        assert!(lock.clone().try_read_owned().is_err());
        drop(writer);
        assert!(lock.try_read_owned().is_ok());
    }
}
