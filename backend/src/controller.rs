mod allocations;
mod app_bindings;
mod gateway_identity;
mod management;
mod ollama;
mod vsub_bindings;
use crate::model::*;
use anyhow::{Context, ensure};
use axum::extract::FromRef;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tower_http::services::ServeDir;
use uuid::Uuid;
#[derive(Clone)]
struct App {
    db: PgPool,
    config: Arc<Config>,
    voice_ready: Arc<std::sync::atomic::AtomicBool>,
    voice_config: crate::voice::Runtime,
}
#[derive(Clone)]
struct SharedApp {
    app: App,
    config: crate::voice::Runtime,
}
impl FromRef<SharedApp> for App {
    fn from_ref(shared: &SharedApp) -> Self {
        Self {
            db: shared.app.db.clone(),
            config: shared.config.snapshot(),
            voice_ready: shared.config.ready.clone(),
            voice_config: shared.config.clone(),
        }
    }
}
struct Error(StatusCode, &'static str);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        crate::contract::failure(self.0, self.1, self.1, false)
    }
}
type Api<T> = Result<T, Error>;
fn envelope(data: Value, code: &str) -> Value {
    crate::contract::envelope(data, code, code)
}
fn ok(v: Value) -> Json<Value> {
    Json(envelope(v, "OK"))
}
fn invalid() -> Error {
    Error(StatusCode::UNPROCESSABLE_ENTITY, "INVALID_REQUEST")
}
fn unavailable(_: sqlx::Error) -> Error {
    Error(StatusCode::SERVICE_UNAVAILABLE, "TEMPORARILY_UNAVAILABLE")
}
async fn auth(a: &App, h: &HeaderMap) -> Api<Principal> {
    let token =
        crate::identity::bearer(h).ok_or(Error(StatusCode::UNAUTHORIZED, "UNAUTHENTICATED"))?;
    let digest = Sha256::digest(token.as_bytes());
    if let Some(p) = a
        .config
        .principals
        .iter()
        .find(|p| Sha256::digest(p.token.as_bytes()) == digest)
    {
        if p.role == "worker" {
            let enabled: Option<bool> =
                sqlx::query_scalar("SELECT management_enabled FROM vrs_nodes WHERE id=$1")
                    .bind(&p.node_id)
                    .fetch_optional(&a.db)
                    .await
                    .map_err(unavailable)?;
            if enabled == Some(false) {
                return Err(Error(StatusCode::UNAUTHORIZED, "CREDENTIAL_REVOKED"));
            }
        }
        return Ok(p.clone());
    }
    if let Some(p) = management::authenticate(a, token).await? {
        return Ok(p);
    }
    crate::identity::verify(h).await.map_err(|status| {
        Error(
            status,
            if status == StatusCode::FORBIDDEN {
                "FORBIDDEN"
            } else {
                "UNAUTHENTICATED"
            },
        )
    })
}
fn require(p: &Principal, role: &str) -> Api<()> {
    if p.role == role || p.role == "admin" {
        Ok(())
    } else {
        Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"))
    }
}
fn worker(p: &Principal, node: &str) -> Api<()> {
    if p.role == "worker" && p.node_id == node {
        Ok(())
    } else {
        Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"))
    }
}
fn visible(p: &Principal, app: &str) -> bool {
    p.role == "admin" || (p.role == "submitter" && p.app_id == app)
}
fn voice_admission(a: &App, executor: &str) -> Api<()> {
    if (executor.starts_with("v-voice.auto.") || executor.starts_with("v-scheduler.voice."))
        && (!a.voice_ready.load(std::sync::atomic::Ordering::Acquire)
            || !a.voice_config.is_current(&a.config))
    {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "APPLICATION_BINDING_UNAVAILABLE",
        ));
    }
    Ok(())
}
fn timestamp_fresh(value: &Value, seconds: i64) -> bool {
    value
        .as_str()
        .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| {
            (0..seconds).contains(&(Utc::now() - at.with_timezone(&Utc)).num_seconds())
        })
}
fn report_fresh(report: &Value) -> bool {
    report["observed_at"].is_null() || timestamp_fresh(&report["observed_at"], 30)
}
fn probe_ready(report: &Value, executor: &str) -> bool {
    if (executor.starts_with("v-voice.auto.") || executor.starts_with("v-scheduler.voice."))
        && report["executor_health"][executor].is_null()
    {
        return false;
    }
    report["executor_health"][executor].is_null()
        || (report["executor_health"][executor]["healthy"] == true
            && timestamp_fresh(&report["executor_health"][executor]["observed_at"], 15))
}
fn node_state(view: &Value) -> &'static str {
    if view["management_enabled"] == false {
        return "revoked";
    }
    if view["managed"] == false {
        return "observing";
    }
    if view["draining"] == true {
        return "draining";
    }
    let fresh = timestamp_fresh(&view["heartbeat"], 12);
    if !fresh {
        return "unavailable";
    }
    let report = &view["report"];
    let degraded_probe = report["executor_health"]
        .as_object()
        .is_some_and(|probes| probes.iter().any(|(id, _)| !probe_ready(report, id)));
    let gpu_unknown = view["capacities"]["gpu_slots"].as_i64().unwrap_or(0) > 0
        && !report["gpus"]
            .as_array()
            .and_then(|g| g.first())
            .is_some_and(|g| g["memory_type"] == "unified" || g["memory_type"] == "dedicated");
    if degraded_probe
        || gpu_unknown
        || !report_fresh(report)
        || view["vos_metrics"]["fresh"] == false
    {
        "degraded"
    } else {
        "ready"
    }
}
fn wait_reason(node: &Value, used: &Resources, executor: Option<&Executor>) -> &'static str {
    if node["draining"] == true {
        return "WAIT_NODE_DRAINING";
    }
    let fresh = timestamp_fresh(&node["heartbeat"], 12);
    if !fresh {
        return "WAIT_NODE_OFFLINE";
    }
    if !report_fresh(&node["report"]) {
        return "WAIT_TELEMETRY_STALE";
    }
    let Some(executor) = executor else {
        return "WAIT_EXECUTOR_UNAVAILABLE";
    };
    if !probe_ready(&node["report"], &executor.id) {
        return "WAIT_EXECUTOR_UNHEALTHY";
    }
    let capacity: Resources =
        serde_json::from_value(node["capacities"].clone()).unwrap_or_default();
    let mut committed = used.clone();
    committed.add(
        &serde_json::from_value::<Resources>(node["resident_baseline"].clone()).unwrap_or_default(),
    );
    if !executor.resources.fits(&committed, &capacity) {
        return "WAIT_CAPACITY";
    }
    let reserve =
        serde_json::from_value::<Resources>(node["safety_reserve"].clone()).unwrap_or_default();
    if !executor
        .resources
        .fits_guarded(&committed, &node["report"], &reserve)
    {
        return "WAIT_OBSERVED_PRESSURE";
    }
    "WAIT_PRIOR_EXECUTION"
}
fn task_value(r: &sqlx::postgres::PgRow) -> Value {
    let status: String = r.get("status");
    let body: Value = r.get("body");
    let policy = &body["_scheduler_policy"];
    let can_retry = status == "failed"
        && policy["retry_mode"] == "idempotent"
        && r.get::<i64, _>("fence") < policy["max_attempts"].as_i64().unwrap_or(1);
    json!({"task_id":r.get::<String,_>("id"),"app_task_id":body["app_task_id"],"executor_id":body["executor"]["id"],"can_retry":can_retry,"attempt_count":r.get::<i64,_>("fence"),"app_id":r.get::<String,_>("app"),"status":status,"version":r.get::<i64,_>("version"),"attempt_id":r.get::<Option<String>,_>("attempt"),"node_id":r.get::<Option<String>,_>("node"),"fence":r.get::<i64,_>("fence"),"progress":r.get::<Value,_>("progress"),"result":r.get::<Value,_>("result"),"created_at":r.get::<DateTime<Utc>,_>("created_at"),"wait_reasons":if status=="queued" {json!(["WAIT_RESOURCE_OR_NODE"])} else if status=="retry_wait" {json!(["WAIT_RETRY_BACKOFF"])} else {json!([])}})
}
async fn task_values(a: &App, rows: &[sqlx::postgres::PgRow]) -> Api<Vec<Value>> {
    let mut values: Vec<Value> = rows.iter().map(task_value).collect();
    if !rows
        .iter()
        .any(|r| r.get::<String, _>("status") == "queued")
    {
        return Ok(values);
    }
    let nodes = sqlx::query("SELECT id,capacities,resident_baseline,safety_reserve,report,heartbeat,draining FROM vrs_nodes").fetch_all(&a.db).await.map_err(unavailable)?;
    let mut node_map = HashMap::new();
    for node in nodes {
        let id: String = node.get("id");
        node_map.insert(id, json!({"capacities":node.get::<Value,_>("capacities"),"resident_baseline":node.get::<Value,_>("resident_baseline"),"safety_reserve":node.get::<Value,_>("safety_reserve"),"report":node.get::<Value,_>("report"),"heartbeat":node.get::<Option<DateTime<Utc>>,_>("heartbeat"),"draining":node.get::<bool,_>("draining")}));
    }
    let reservations = sqlx::query("SELECT node,resources FROM vrs_reservations WHERE state<>'released' UNION ALL SELECT node,resources FROM vrs_permits WHERE state<>'released' UNION ALL SELECT x.node_id AS node,x.resources FROM vrs_permit_placements x JOIN vrs_permits p ON p.id=x.permit_id WHERE p.state<>'released'").fetch_all(&a.db).await.map_err(unavailable)?;
    let mut used = HashMap::<String, Resources>::new();
    for row in reservations {
        let resources: Resources =
            serde_json::from_value(row.get("resources")).map_err(|_| invalid())?;
        used.entry(row.get("node")).or_default().add(&resources);
    }
    for (row, value) in rows.iter().zip(&mut values) {
        if row.get::<String, _>("status") != "queued" {
            continue;
        }
        let Some(node_id) = row.get::<Option<String>, _>("node") else {
            continue;
        };
        let Some(node) = node_map.get(&node_id) else {
            continue;
        };
        let executor_id = value["executor_id"].as_str().unwrap_or_default();
        let executor = a
            .config
            .executors
            .iter()
            .find(|e| e.node_id == node_id && e.id == executor_id);
        value["wait_reasons"] = json!([wait_reason(
            node,
            used.get(&node_id).unwrap_or(&Resources::default()),
            executor
        )]);
    }
    Ok(values)
}
async fn event(tx: &mut Transaction<'_, Postgres>, id: &str) -> Api<()> {
    sqlx::query("INSERT INTO vrs_events(task,app,status,version,progress) SELECT id,app,status,version,progress FROM vrs_tasks WHERE id=$1").bind(id).execute(&mut **tx).await.map_err(unavailable)?;
    Ok(())
}
async fn expire(a: &App) -> Api<()> {
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let rows=sqlx::query("UPDATE vrs_tasks t SET status='recovering',version=version+1,updated_at=now() FROM vrs_reservations r WHERE r.task=t.id AND r.state IN ('held','active') AND r.lease_until<now() AND t.status IN ('assigned','running','cancelling') RETURNING t.id").fetch_all(&mut *tx).await.map_err(unavailable)?;
    sqlx::query("UPDATE vrs_reservations SET state='suspect' WHERE state IN ('held','active') AND lease_until<now()").execute(&mut *tx).await.map_err(unavailable)?;
    for r in rows {
        event(&mut tx, &r.get::<String, _>("id")).await?;
    }
    sqlx::query(
        "UPDATE vrs_permits p SET state='suspect' WHERE p.state='active' AND (p.lease_until<now() OR (EXISTS(SELECT 1 FROM vrs_permit_placements x WHERE x.permit_id=p.id) AND EXISTS(SELECT 1 FROM vrs_nodes n WHERE (n.id=p.node OR EXISTS(SELECT 1 FROM vrs_permit_placements x WHERE x.permit_id=p.id AND x.node_id=n.id)) AND (n.heartbeat IS NULL OR n.heartbeat<now()-interval '12 seconds'))))",
    )
    .execute(&mut *tx)
    .await
    .map_err(unavailable)?;
    sqlx::query("UPDATE vrs_reservations r SET state='released' WHERE state<>'released' AND EXISTS(SELECT 1 FROM vrs_attempts a WHERE a.id=r.attempt AND a.stopped_at IS NOT NULL) AND NOT EXISTS(SELECT 1 FROM vrs_permits p WHERE p.parent_attempt=r.attempt AND p.state<>'released')").execute(&mut *tx).await.map_err(unavailable)?;
    let ready=sqlx::query("UPDATE vrs_tasks t SET status='queued',version=version+1,updated_at=now() WHERE status='retry_wait' AND retry_at<=now() AND NOT EXISTS(SELECT 1 FROM vrs_reservations r WHERE r.task=t.id AND r.state<>'released') RETURNING id").fetch_all(&mut *tx).await.map_err(unavailable)?;
    for row in ready {
        event(&mut tx, &row.get::<String, _>("id")).await?;
    }
    tx.commit().await.map_err(unavailable)?;
    Ok(())
}
pub async fn serve(
    path: PathBuf,
    url: String,
    listen: String,
    ui: Option<PathBuf>,
    gateway: Option<&crate::router::Runtime>,
) -> anyhow::Result<()> {
    let socket = tokio::net::TcpListener::bind(&listen).await?;
    let embedded = crate::local::prepare(&path, &listen).await?;
    let config: Config = serde_json::from_slice(&std::fs::read(&path)?)?;
    let runtime = crate::voice::Runtime::restore(&path, config)?;
    let config = runtime.snapshot();
    ensure!(
        !config.principals.is_empty(),
        "at least one principal is required"
    );
    for p in &config.principals {
        ensure!(
            p.token.len() >= 32 && !p.token.contains("REPLACE"),
            "principal token must be at least 32 characters and non-placeholder"
        );
        ensure!(
            ["admin", "submitter", "worker"].contains(&p.role.as_str()),
            "invalid role"
        );
    }
    for n in &config.nodes {
        ensure!(n.capacities.valid(), "invalid capacity");
        ensure!(
            n.resident_baseline
                .fits(&Resources::default(), &n.capacities),
            "resident baseline exceeds node capacity"
        );
        ensure!(n.safety_reserve.valid(), "invalid system safety reserve");
    }
    for e in &config.executors {
        ensure!(
            e.resources.cpu_millicores > 0 && e.resources.memory_mib > 0,
            "compute executor requires nonzero CPU and host-memory budgets"
        );
        ensure!(
            e.resources.gpu_memory_mib == 0 || e.resources.gpu_slots > 0,
            "GPU memory budget requires a GPU slot"
        );
        ensure!(
            (1..=10).contains(&e.max_attempts),
            "max_attempts must be 1..10"
        );
        ensure!(
            ["", "none", "idempotent"].contains(&e.retry_mode.as_str()),
            "invalid retry policy"
        );
        ensure!(
            e.valid_placements(),
            "invalid distributed inference placements"
        );
        for part in &e.placements {
            let node = config
                .nodes
                .iter()
                .find(|node| node.id == part.node_id)
                .context("unknown placement node")?;
            ensure!(
                part.resources
                    .fits(&node.resident_baseline, &node.capacities),
                "placement exceeds node capacity"
            );
        }
        let n = config
            .nodes
            .iter()
            .find(|n| n.id == e.node_id)
            .context("unknown executor node")?;
        ensure!(
            e.resources.fits(&n.resident_baseline, &n.capacities),
            "executor exceeds node capacity"
        );
    }
    for p in &config.principals {
        for executor in &p.delegated_executors {
            ensure!(
                p.role == "submitter" && config.executors.iter().any(|e| e.id == *executor),
                "invalid delegated executor approval"
            );
        }
    }
    let db = PgPoolOptions::new()
        .max_connections(12)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect(&url)
        .await
        .context("database connection failed")?;
    sqlx::migrate!().run(&db).await?;
    for n in &config.nodes {
        sqlx::query("INSERT INTO vrs_nodes(id,capacities,resident_baseline,safety_reserve) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO UPDATE SET capacities=excluded.capacities,resident_baseline=excluded.resident_baseline,safety_reserve=excluded.safety_reserve").bind(&n.id).bind(json!(n.capacities)).bind(json!(n.resident_baseline)).bind(json!(n.safety_reserve)).execute(&db).await?;
    }
    management::restore(&db, &runtime).await?;
    let config = runtime.snapshot();
    let a = App {
        db,
        config,
        voice_ready: runtime.ready.clone(),
        voice_config: runtime.clone(),
    };
    crate::vos::spawn(a.db.clone(), a.config.clone())?;
    crate::local::start(embedded);
    let shared = SharedApp {
        app: a.clone(),
        config: runtime.clone(),
    };
    app_bindings::start(shared.clone(), path.clone());
    gateway_identity::start(path.clone());
    vsub_bindings::start(shared.clone(), path.clone());
    crate::voice::start(path, runtime, a.db.clone());
    ollama::start(shared.clone());
    let ticker_state = shared.clone();
    tokio::spawn(async move {
        loop {
            let ticker = App::from_ref(&ticker_state);
            let _ = expire(&ticker).await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
    let api = Router::<SharedApp>::new()
        .route("/setup", get(|| async { ok(json!({"ready":true,"version":std::env::var("VOS_APP_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into())})) }))
        .route("/discovery", get(discovery))
        .route(
            "/openapi.json",
            get(|| async {
                Json(
                    serde_json::from_str::<Value>(include_str!("../../api/openapi-v1.json"))
                        .expect("validated OpenAPI"),
                )
            }),
        )
        .route("/management", get(management::overview))
        .route("/node-sources/modelinfer/{id}", post(management::source_settings))
        .route("/node-sources/modelinfer/{id}/control", post(management::source_control))
        .route("/enrollments", post(management::enrollment))
        .route("/enrollments/{id}/revoke", post(management::revoke_enrollment))
        .route("/workers/register", post(management::register))
        .route("/workers/{id}/executors", post(management::propose_executors))
        .route("/service-identities", post(management::create_identity))
        .route("/service-identities/{id}/revoke", post(management::revoke_identity))
        .route("/executors/{id}", post(management::update_executor))
        .route("/nodes/{id}", post(management::update_node))
        .route("/nodes/{id}/approve", post(management::approve_node))
        .route("/nodes/{id}/revoke", post(management::revoke_node))
        .route("/executors", get(executors))
        .route("/statistics", get(statistics))
        .route("/tasks", post(submit).get(tasks))
        .route("/tasks/{id}", get(detail).delete(delete_task))
        .route("/tasks/{id}/retry", post(retry_task))
        .route("/tasks/{id}/attempts", get(attempts))
        .route("/tasks/{id}/events", get(task_events))
        .route("/tasks/{id}/cancel", post(cancel))
        .route("/events", get(events))
        .route("/permits", post(permit_acquire).get(permits))
        .route("/permits/{id}/renew", post(permit_renew))
        .route("/permits/{id}/verify", post(permit_verify))
        .route("/permits/{id}/release", post(permit_release))
        .route("/permits/{id}/delegate", post(permit_delegate))
        .route("/permits/{id}/confirm-stopped", post(permit_confirm))
        .route("/delegations/verify", post(delegation_verify))
        .route("/nodes", get(nodes))
        .route("/nodes/{id}/drain", post(drain))
        .route("/reservations", get(reservations))
        .route("/workers/{id}/heartbeat", post(heartbeat))
        .route("/workers/{id}/commands/claim", post(claim))
        .route("/attempts/{id}/renew", post(renew))
        .route("/attempts/{id}/report", post(report));
    let mut router = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .nest("/api/v1", api.clone())
        .nest("/scheduler/api/v1", api)
        .layer(axum::extract::DefaultBodyLimit::max(262144))
        .with_state(shared);
    if let Some(path) = ui.as_ref() {
        router = router.fallback_service(ServeDir::new(path));
    }
    let router = router.layer(axum::middleware::from_fn(crate::contract::normalize_errors));
    if let Some(gateway) = gateway {
        gateway.set_controller(router.clone(), true);
    }
    eprintln!("V-Scheduler controller listening on {listen}");
    axum::serve(socket, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
async fn discovery() -> Json<Value> {
    ok(
        json!({"protocol_version":1,"implementation_version":"0.1.0","features":["tasks","idempotency","reservations","events","static-executors","dynamic-enrollment","node-management","modelinfer-discovery","vos-app-source-metrics","source-budget-planning","multi-node-permits","executor-approval","service-identities","agent-image-studio","permits","backend-permit-verification","permit-history","statistics","permit-release-times","task-timeline","delegated-permits","safety-headroom","retry","manual-delete","attempt-history"],"unsupported":["artifacts","ha","vos-remote-control"],"authentication":"scoped-bearer","max_body_bytes":262144,"openapi_url":"openapi.json"}),
    )
}
async fn executors(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    Ok(ok(json!(
        a.config
            .executors
            .iter()
            .filter(|e| visible(&p, &e.app_id))
            .collect::<Vec<_>>()
    )))
}
async fn submit(State(a): State<App>, h: HeaderMap, Json(body): Json<TaskSubmit>) -> Api<Response> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let key = h
        .get("idempotency-key")
        .and_then(|x| x.to_str().ok())
        .ok_or(invalid())?;
    if key.is_empty()
        || key.len() > 256
        || body.idempotency_key.as_ref().is_some_and(|v| v != key)
        || body.protocol_version != 1
        || body.app_task_id.is_empty()
        || !body.parameters.is_object()
        || !body.inputs.is_empty()
    {
        return Err(invalid());
    }
    let e = a
        .config
        .executors
        .iter()
        .find(|e| {
            e.id == body.executor.id
                && e.contract_version == body.executor.contract_version
                && e.task_type == body.executor.task_type
                && e.app_id == p.app_id
        })
        .ok_or(Error(StatusCode::FORBIDDEN, "EXECUTOR_NOT_ALLOWED"))?;
    if e.task_type == "model.infer" {
        return Err(invalid());
    }
    voice_admission(&a, &e.id)?;
    let mut value = json!(body);
    let hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&value).map_err(|_| invalid())?)
    );
    value["_scheduler_policy"] = json!({"retry_mode":e.retry_mode,"max_attempts":e.max_attempts});
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&p.app_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    let id = Uuid::new_v4().to_string();
    sqlx::query("SELECT id FROM vrs_nodes WHERE id=$1 FOR UPDATE")
        .bind(&e.node_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
    management::admit(&mut tx, e, &p).await?;
    let insert=sqlx::query("INSERT INTO vrs_tasks(id,app,idem,hash,body,node) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(app,idem) DO NOTHING RETURNING *").bind(&id).bind(&p.app_id).bind(key).bind(&hash).bind(value).bind(&e.node_id).fetch_optional(&mut *tx).await.map_err(unavailable)?;
    let fresh = insert.is_some();
    if fresh {
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM vrs_tasks WHERE app=$1 AND status IN ('queued','assigned','running','cancelling','recovering')").bind(&p.app_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
        if count > 1000 {
            return Err(Error(StatusCode::TOO_MANY_REQUESTS, "QUOTA_EXCEEDED"));
        }
    }
    let row = if let Some(r) = insert {
        event(&mut tx, &id).await?;
        r
    } else {
        let r = sqlx::query("SELECT * FROM vrs_tasks WHERE app=$1 AND idem=$2")
            .bind(&p.app_id)
            .bind(key)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
        if r.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some() {
            return Err(Error(StatusCode::CONFLICT, "TASK_DELETED"));
        }
        if r.get::<String, _>("hash") != hash {
            return Err(Error(StatusCode::CONFLICT, "IDEMPOTENCY_CONFLICT"));
        }
        r
    };
    tx.commit().await.map_err(unavailable)?;
    Ok((
        if fresh {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        ok(task_value(&row)),
    )
        .into_response())
}
#[derive(Deserialize, Default)]
struct Page {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}
fn page(q: &Page) -> Api<(i64, i64)> {
    let offset = q
        .cursor
        .as_deref()
        .unwrap_or("0")
        .parse::<i64>()
        .map_err(|_| invalid())?;
    let limit = q.limit.unwrap_or(50);
    if offset < 0 || !(1..=200).contains(&limit) {
        return Err(invalid());
    }
    Ok((offset, limit))
}
async fn tasks(State(a): State<App>, h: HeaderMap, Query(q): Query<Page>) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let (offset, limit) = page(&q)?;
    let rs=sqlx::query("SELECT * FROM vrs_tasks WHERE deleted_at IS NULL AND ($1 OR app=$2) ORDER BY created_at DESC,id LIMIT $3 OFFSET $4").bind(p.role=="admin").bind(&p.app_id).bind(limit).bind(offset).fetch_all(&a.db).await.map_err(unavailable)?;
    let items = task_values(&a, &rs).await?;
    Ok(ok(
        json!({"items":items,"next_cursor":if rs.len()==limit as usize{Some((offset+limit).to_string())}else{None}}),
    ))
}
async fn get_task(a: &App, p: &Principal, id: &str) -> Api<sqlx::postgres::PgRow> {
    let r = sqlx::query("SELECT * FROM vrs_tasks WHERE id=$1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(&a.db)
        .await
        .map_err(unavailable)?
        .ok_or(Error(StatusCode::NOT_FOUND, "NOT_FOUND"))?;
    if !visible(p, &r.get::<String, _>("app")) {
        return Err(Error(StatusCode::NOT_FOUND, "NOT_FOUND"));
    }
    Ok(r)
}
async fn detail(State(a): State<App>, h: HeaderMap, Path(id): Path<String>) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let row = get_task(&a, &p, &id).await?;
    Ok(ok(task_values(&a, &[row]).await?.remove(0)))
}
async fn cancel(State(a): State<App>, h: HeaderMap, Path(id): Path<String>) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    get_task(&a, &p, &id).await?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let r=sqlx::query("UPDATE vrs_tasks SET status=CASE WHEN status IN ('queued','retry_wait') THEN 'cancelled' ELSE 'cancelling' END,version=version+1,updated_at=now() WHERE id=$1 AND status IN ('queued','retry_wait','assigned','running','recovering') RETURNING *").bind(&id).fetch_optional(&mut *tx).await.map_err(unavailable)?;
    if r.is_some() {
        event(&mut tx, &id).await?;
    }
    tx.commit().await.map_err(unavailable)?;
    let row = get_task(&a, &p, &id).await?;
    Ok(ok(task_values(&a, &[row]).await?.remove(0)))
}
async fn events(State(a): State<App>, h: HeaderMap, Query(q): Query<Page>) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let (c, l) = page(&q)?;
    let rs=sqlx::query("SELECT cursor,task,status,version,progress FROM vrs_events WHERE cursor>$1 AND ($2 OR app=$3) ORDER BY cursor LIMIT $4").bind(c).bind(p.role=="admin").bind(&p.app_id).bind(l).fetch_all(&a.db).await.map_err(unavailable)?;
    let cursor = rs.last().map(|r| r.get::<i64, _>("cursor")).unwrap_or(c);
    Ok(ok(
        json!({"items":rs.iter().map(|r|json!({"cursor":r.get::<i64,_>("cursor"),"task_id":r.get::<String,_>("task"),"status":r.get::<String,_>("status"),"version":r.get::<i64,_>("version"),"progress":r.get::<Value,_>("progress")})).collect::<Vec<_>>(),"next_cursor":cursor.to_string()}),
    ))
}
async fn task_events(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Page>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    get_task(&a, &p, &id).await?;
    let (before, limit) = page(&q)?;
    let rows = sqlx::query("SELECT cursor,status,version,progress,created_at FROM vrs_events WHERE task=$1 AND ($2=0 OR cursor<$2) ORDER BY cursor DESC LIMIT $3")
        .bind(&id).bind(before).bind(limit).fetch_all(&a.db).await.map_err(unavailable)?;
    let next = if rows.len() == limit as usize {
        rows.last().map(|r| r.get::<i64, _>("cursor").to_string())
    } else {
        None
    };
    Ok(ok(
        json!({"items":rows.iter().map(|r|json!({"cursor":r.get::<i64,_>("cursor"),"task_id":id,"status":r.get::<String,_>("status"),"version":r.get::<i64,_>("version"),"progress":r.get::<Value,_>("progress"),"created_at":r.get::<DateTime<Utc>,_>("created_at")})).collect::<Vec<_>>(),"next_cursor":next}),
    ))
}
async fn nodes(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    if p.app_id == "vos-administrator" {
        if a.config.vos_cluster.is_none() {
            tokio::join!(
                crate::vos::refresh_platform_inventory(&a.db, &h),
                crate::modelinfer::refresh(&a.db, &h)
            );
        } else {
            crate::modelinfer::refresh(&a.db, &h).await;
        }
    }
    let rs = sqlx::query("SELECT * FROM vrs_nodes ORDER BY id")
        .fetch_all(&a.db)
        .await
        .map_err(unavailable)?;
    let mut views: Vec<Value> = rs.iter().map(|r|json!({"id":r.get::<String,_>("id"),"capacities":r.get::<Value,_>("capacities"),"resident_baseline":r.get::<Value,_>("resident_baseline"),"safety_reserve":r.get::<Value,_>("safety_reserve"),"report":r.get::<Value,_>("report"),"management_enabled":r.get::<bool,_>("management_enabled"),"draining":r.get::<bool,_>("draining"),"heartbeat":r.get::<Option<DateTime<Utc>>,_>("heartbeat")})).collect();
    crate::vos::decorate(&a.db, &a.config, &mut views)
        .await
        .map_err(unavailable)?;
    crate::modelinfer::decorate(&a.db, &a.config, &mut views)
        .await
        .map_err(unavailable)?;
    management::decorate(&a, &mut views).await?;
    for view in &mut views {
        view["health_state"] = json!(node_state(view));
        let reserve: Resources =
            serde_json::from_value(view["safety_reserve"].clone()).unwrap_or_default();
        view["effective_safety_reserve"] = json!(reserve.safety_floor(&view["report"]));
    }
    Ok(ok(json!(views)))
}
async fn reservations(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let rs =
        sqlx::query("SELECT attempt,task,node,resources,state,lease_until,created_at FROM vrs_reservations WHERE state<>'released' UNION ALL SELECT id AS attempt, 'permit:' || id AS task,node,resources,state,lease_until,created_at FROM vrs_permits WHERE state<>'released' UNION ALL SELECT p.id AS attempt,'permit:' || p.id AS task,x.node_id AS node,x.resources,p.state,p.lease_until,p.created_at FROM vrs_permit_placements x JOIN vrs_permits p ON p.id=x.permit_id WHERE p.state<>'released' ORDER BY created_at")
            .fetch_all(&a.db)
            .await
            .map_err(unavailable)?;
    Ok(ok(json!(rs.iter().map(|r|json!({"attempt_id":r.get::<String,_>("attempt"),"task_id":r.get::<String,_>("task"),"node_id":r.get::<String,_>("node"),"resources":r.get::<Value,_>("resources"),"state":r.get::<String,_>("state"),"lease_until":r.get::<DateTime<Utc>,_>("lease_until")})).collect::<Vec<_>>())))
}
async fn drain(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let d = body
        .get("draining")
        .and_then(Value::as_bool)
        .ok_or(invalid())?;
    let changed =
        sqlx::query("UPDATE vrs_nodes SET draining=$2 WHERE id=$1 AND management_enabled=true")
            .bind(&id)
            .bind(d)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?
            .rows_affected();
    if changed == 0 {
        return Err(Error(StatusCode::CONFLICT, "NODE_NOT_APPROVED"));
    }
    management::audit(
        &mut tx,
        &p,
        if d { "node.draining" } else { "node.resumed" },
        &id,
    )
    .await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(json!({"draining":d})))
}
#[derive(Deserialize)]
struct Heartbeat {
    session_id: String,
    report: Value,
}
async fn heartbeat(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<Heartbeat>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    worker(&p, &id)?;
    let changed=sqlx::query("UPDATE vrs_nodes SET heartbeat=now(),session=$2,report=$3 WHERE id=$1 AND (session IS NULL OR session=$2)").bind(&id).bind(b.session_id).bind(&b.report).execute(&a.db).await.map_err(unavailable)?.rows_affected();
    if changed == 0 {
        return Err(Error(StatusCode::CONFLICT, "WORKER_SESSION_CONFLICT"));
    }
    if a.config.local_agent.as_ref().is_some_and(|local| {
        local["node_id"].as_str() == Some(id.as_str()) && local["automatic_capacity"] == true
    }) {
        if let Some(resources) = crate::local::capacity(&b.report) {
            sqlx::query("UPDATE vrs_nodes SET capacities=$2 WHERE id=$1")
                .bind(&id)
                .bind(json!(resources))
                .execute(&a.db)
                .await
                .map_err(unavailable)?;
        }
    }
    Ok(ok(json!({"lease_seconds":30})))
}
#[derive(Deserialize)]
struct Session {
    session_id: String,
}
fn command(r: &sqlx::postgres::PgRow) -> Command {
    let body: Value = r.get("body");
    let attempt: String = r.get("attempt");
    Command {
        command_id: attempt.clone(),
        task_id: r.get("id"),
        attempt_id: attempt,
        fence: r.get("fence"),
        executor_id: body["executor"]["id"].as_str().unwrap_or("").into(),
        parameters: body["parameters"].clone(),
        lease_seconds: 30,
        cancel: r.get::<String, _>("status") == "cancelling",
    }
}
async fn claim(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<Session>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    worker(&p, &id)?;
    expire(&a).await?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let node=sqlx::query("SELECT *,heartbeat>now()-interval '12 seconds' AS fresh FROM vrs_nodes WHERE id=$1 FOR UPDATE").bind(&id).fetch_one(&mut *tx).await.map_err(unavailable)?;
    if node.get::<Option<String>, _>("session").as_deref() != Some(&b.session_id) {
        return Err(Error(StatusCode::CONFLICT, "WORKER_SESSION_CONFLICT"));
    }
    let existing=sqlx::query("SELECT * FROM vrs_tasks WHERE node=$1 AND status IN ('assigned','running','cancelling','recovering') ORDER BY created_at").bind(&id).fetch_all(&mut *tx).await.map_err(unavailable)?;
    // MVP Agent has one execution lane; no hidden queue holding GPU reservations.
    if !existing.is_empty() {
        tx.commit().await.map_err(unavailable)?;
        return Ok(ok(
            json!({"commands":existing.iter().map(command).collect::<Vec<_>>()}),
        ));
    }
    if !node.get::<bool, _>("management_enabled")
        || node.get::<bool, _>("draining")
        || !node.get::<Option<bool>, _>("fresh").unwrap_or(false)
        || !report_fresh(&node.get::<Value, _>("report"))
    {
        return Ok(ok(json!({"commands":[]})));
    }
    let capacity: Resources =
        serde_json::from_value(node.get("capacities")).map_err(|_| invalid())?;
    let mut used: Resources =
        serde_json::from_value(node.get("resident_baseline")).map_err(|_| invalid())?;
    for r in
        sqlx::query("SELECT resources FROM vrs_reservations WHERE node=$1 AND state<>'released' UNION ALL SELECT resources FROM vrs_permits WHERE node=$1 AND state<>'released' UNION ALL SELECT x.resources FROM vrs_permit_placements x JOIN vrs_permits p ON p.id=x.permit_id WHERE x.node_id=$1 AND p.state<>'released'")
            .bind(&id)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?
    {
        used.add(&serde_json::from_value(r.get("resources")).map_err(|_| invalid())?);
    }
    let queued=sqlx::query("SELECT * FROM vrs_tasks WHERE node=$1 AND status='queued' ORDER BY created_at,id LIMIT 32 FOR UPDATE SKIP LOCKED").bind(&id).fetch_all(&mut *tx).await.map_err(unavailable)?;
    for r in queued {
        let prior: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM vrs_reservations WHERE task=$1 AND state<>'released')",
        )
        .bind(r.get::<String, _>("id"))
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if prior {
            continue;
        }
        let body: Value = r.get("body");
        let Some(e) = a
            .config
            .executors
            .iter()
            .find(|e| e.id == body["executor"]["id"] && e.node_id == id)
        else {
            continue;
        };
        if management::admit(&mut tx, e, &p).await.is_err() {
            continue;
        }
        if voice_admission(&a, &e.id).is_err() {
            continue;
        }
        if !probe_ready(&node.get::<Value, _>("report"), &e.id) {
            continue;
        }
        if !e.resources.fits(&used, &capacity) {
            continue;
        }
        let observed: Value = node.get("report");
        let reserve: Resources =
            serde_json::from_value(node.get("safety_reserve")).map_err(|_| invalid())?;
        if !e.resources.fits_guarded(&used, &observed, &reserve) {
            continue;
        }
        let task: String = r.get("id");
        let attempt = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO vrs_reservations(attempt,task,node,resources) VALUES($1,$2,$3,$4)",
        )
        .bind(&attempt)
        .bind(&task)
        .bind(&id)
        .bind(json!(e.resources))
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let row=sqlx::query("UPDATE vrs_tasks SET status='assigned',attempt=$2,fence=fence+1,version=version+1,updated_at=now() WHERE id=$1 RETURNING *").bind(&task).bind(attempt).fetch_one(&mut *tx).await.map_err(unavailable)?;
        sqlx::query("INSERT INTO vrs_attempts(id,task,fence,node) VALUES($1,$2,$3,$4)")
            .bind(row.get::<String, _>("attempt"))
            .bind(&task)
            .bind(row.get::<i64, _>("fence"))
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        event(&mut tx, &task).await?;
        tx.commit().await.map_err(unavailable)?;
        return Ok(ok(json!({"commands":[command(&row)]})));
    }
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(json!({"commands":[]})))
}
#[derive(Deserialize)]
struct Renewal {
    session_id: String,
    fence: i64,
}
async fn renew(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<Renewal>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    if p.role != "worker" {
        return Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    let changed=sqlx::query("UPDATE vrs_reservations r SET lease_until=now()+interval '30 seconds' FROM vrs_tasks t,vrs_nodes n WHERE r.attempt=$1 AND t.attempt=r.attempt AND t.fence=$2 AND n.id=r.node AND n.session=$3 AND r.node=$4 AND r.state IN ('held','active') AND r.lease_until>now() AND t.status IN ('assigned','running')").bind(id).bind(b.fence).bind(b.session_id).bind(&p.node_id).execute(&a.db).await.map_err(unavailable)?.rows_affected();
    if p.role != "worker" {
        return Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    if changed == 0 {
        return Err(Error(StatusCode::CONFLICT, "LEASE_NOT_RENEWABLE"));
    }
    Ok(ok(json!({"lease_seconds":30})))
}
#[derive(Deserialize)]
struct Report {
    session_id: String,
    fence: i64,
    status: String,
    #[serde(default)]
    stopped: bool,
    #[serde(default)]
    progress: Value,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    result: Value,
}
fn reported_progress(
    mut progress: Value,
    stage: Option<&str>,
    message: Option<&str>,
) -> Api<Value> {
    if progress.is_null() {
        progress = json!({});
    }
    let fields = progress.as_object_mut().ok_or_else(invalid)?;
    if let Some(percent) = fields.get("percent") {
        let value = percent.as_f64().ok_or_else(invalid)?;
        if !(0.0..=1.0).contains(&value) {
            return Err(invalid());
        }
    }
    for (key, input, limit) in [("stage", stage, 128), ("message", message, 512)] {
        if let Some(value) = input {
            fields.insert(key.to_string(), json!(value));
        }
        if let Some(value) = fields.get(key) {
            let value = value.as_str().ok_or_else(invalid)?;
            if value.len() > limit {
                return Err(invalid());
            }
        }
    }
    Ok(progress)
}
async fn report(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<Report>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    if p.role != "worker" {
        return Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    if !["running", "succeeded", "failed", "cancelled", "unknown"].contains(&b.status.as_str()) {
        return Err(invalid());
    }
    let terminal = ["succeeded", "failed", "cancelled"].contains(&b.status.as_str());
    if terminal && !b.stopped {
        return Err(Error(StatusCode::CONFLICT, "STOP_NOT_CONFIRMED"));
    }
    let progress = reported_progress(b.progress, b.stage.as_deref(), b.message.as_deref())?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let r=sqlx::query("SELECT t.* FROM vrs_tasks t JOIN vrs_nodes n ON n.id=t.node WHERE t.attempt=$1 AND t.fence=$2 AND t.node=$3 AND n.session=$4 FOR UPDATE OF t").bind(&id).bind(b.fence).bind(&p.node_id).bind(b.session_id).fetch_optional(&mut *tx).await.map_err(unavailable)?.ok_or(Error(StatusCode::CONFLICT,"STALE_ATTEMPT"))?;
    let released: bool =
        sqlx::query_scalar("SELECT state='released' FROM vrs_reservations WHERE attempt=$1")
            .bind(&id)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
    if released {
        return Ok(ok(task_value(&r)));
    }
    let old: String = r.get("status");
    let task: String = r.get("id");
    if ["succeeded", "failed", "cancelled"].contains(&old.as_str()) {
        return Ok(ok(task_value(&r)));
    }
    let body: Value = r.get("body");
    let executor = a
        .config
        .executors
        .iter()
        .find(|e| e.id == body["executor"]["id"]);
    let auto_retry = terminal
        && b.status == "failed"
        && old != "cancelling"
        && b.result["retryable"].as_bool().unwrap_or(false)
        && executor.is_some_and(|e| {
            e.retry_mode == "idempotent" && r.get::<i64, _>("fence") < e.max_attempts
        });
    let status = if auto_retry {
        "retry_wait"
    } else if terminal && old == "cancelling" {
        "cancelled"
    } else if terminal {
        &b.status
    } else if old == "cancelling" {
        "cancelling"
    } else if b.status == "unknown" || old == "recovering" {
        "recovering"
    } else {
        "running"
    };
    sqlx::query("UPDATE vrs_tasks SET status=$2,progress=$3,result=$4,version=version+1,updated_at=now() WHERE id=$1").bind(&task).bind(status).bind(progress).bind(if status=="cancelled" {json!({"reason":"cancelled"})}else{b.result.clone()}).execute(&mut *tx).await.map_err(unavailable)?;
    sqlx::query("UPDATE vrs_attempts SET status=$2,result=$3,stopped_at=CASE WHEN $4 THEN now() ELSE stopped_at END WHERE id=$1").bind(&id).bind(&b.status).bind(&b.result).bind(terminal).execute(&mut *tx).await.map_err(unavailable)?;
    if auto_retry {
        sqlx::query("UPDATE vrs_tasks SET retry_at=now()+interval '3 seconds' * power(2,least(fence-1,4)) WHERE id=$1").bind(&task).execute(&mut *tx).await.map_err(unavailable)?;
    }
    if terminal {
        sqlx::query(
            "UPDATE vrs_reservations SET state=CASE WHEN EXISTS(SELECT 1 FROM vrs_permits WHERE parent_attempt=$1 AND state<>'released') THEN 'suspect' ELSE 'released' END,stopped_at=now() WHERE attempt=$1",
        )
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    } else if status == "running" {
        sqlx::query("UPDATE vrs_reservations SET state='active' WHERE attempt=$1 AND state='held'")
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
    }
    event(&mut tx, &task).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(json!({"status":status})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermitRequest {
    executor_id: String,
    upstream_url: String,
    #[serde(default)]
    model_id: String,
    #[serde(default)]
    execution_token: String,
}
fn approved_upstream(executor: &Executor, value: &str) -> bool {
    let canonical = |value: &str| -> Option<String> {
        let parsed = reqwest::Url::parse(value).ok()?;
        if !["http", "https"].contains(&parsed.scheme())
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return None;
        }
        Some(parsed.as_str().trim_end_matches('/').to_owned())
    };
    let Some(target) = canonical(value) else {
        return false;
    };
    executor
        .upstream_urls
        .iter()
        .any(|allowed| canonical(allowed).as_deref() == Some(target.as_str()))
}
async fn permit_acquire(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<PermitRequest>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    voice_admission(&a, &b.executor_id)?;
    if !b.execution_token.is_empty() && !(32..=128).contains(&b.execution_token.len()) {
        return Err(invalid());
    }
    let key = h
        .get("idempotency-key")
        .and_then(|s| s.to_str().ok())
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or(invalid())?;
    let e = a
        .config
        .executors
        .iter()
        .find(|e| e.id == b.executor_id && e.app_id == p.app_id && e.task_type == "model.infer")
        .ok_or(Error(StatusCode::FORBIDDEN, "EXECUTOR_NOT_ALLOWED"))?;
    if !approved_upstream(e, &b.upstream_url) || b.model_id.len() > 512 {
        return Err(Error(StatusCode::FORBIDDEN, "UPSTREAM_NOT_APPROVED"));
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let nodes = allocations::lock_nodes(&mut tx, e).await?;
    management::admit(&mut tx, e, &p).await?;
    voice_admission(&a, &e.id)?;
    if let Some(old) =
        sqlx::query("SELECT *,lease_until>now() AS valid FROM vrs_permits WHERE app=$1 AND idem=$2")
            .bind(&p.app_id)
            .bind(key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
    {
        let metadata: Value = old.get("metadata");
        if old.get::<String, _>("executor") != b.executor_id
            || metadata["upstream_url"] != b.upstream_url
            || metadata["model_id"] != b.model_id
            || (!b.execution_token.is_empty()
                && metadata["execution_token_hash"]
                    != format!("{:x}", Sha256::digest(b.execution_token.as_bytes())))
        {
            return Err(Error(StatusCode::CONFLICT, "IDEMPOTENCY_CONFLICT"));
        }
        if old.get::<String, _>("state") != "active" || !old.get::<bool, _>("valid") {
            return Err(Error(StatusCode::CONFLICT, "PERMIT_NOT_ACTIVE"));
        }
        return Ok(ok(
            json!({"permit_id":old.get::<String,_>("id"),"lease_seconds":30,"node_id":e.node_id}),
        ));
    }
    for part in e.allocations() {
        let node = nodes
            .iter()
            .find(|node| node.get::<String, _>("id") == part.node_id)
            .ok_or(invalid())?;
        allocations::room(&mut tx, node, &part.resources, &e.id).await?;
    }
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO vrs_permits(id,app,idem,executor,node,resources,metadata) VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(&id)
    .bind(&p.app_id)
    .bind(key)
    .bind(&e.id)
    .bind(&e.node_id)
    .bind(json!(e.resources))
    .bind(json!({"model_id":b.model_id,"upstream_url":b.upstream_url,"placements":e.placements,"execution_token_hash":format!("{:x}",Sha256::digest(b.execution_token.as_bytes()))}))
    .execute(&mut *tx)
    .await
    .map_err(unavailable)?;
    for part in &e.placements {
        sqlx::query(
            "INSERT INTO vrs_permit_placements(permit_id,node_id,resources) VALUES($1,$2,$3)",
        )
        .bind(&id)
        .bind(&part.node_id)
        .bind(json!(part.resources))
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    }
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"permit_id":id,"lease_seconds":30,"node_id":e.node_id,"node_ids":e.allocations().iter().map(|p|p.node_id.clone()).collect::<Vec<_>>()}),
    ))
}
#[derive(Deserialize)]
struct PermitPage {
    cursor: Option<String>,
    limit: Option<i64>,
    state: Option<String>,
}
async fn statistics(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    Ok(ok(crate::statistics::read(
        &a.db,
        p.role == "admin",
        &p.app_id,
    )
    .await
    .map_err(unavailable)?))
}
async fn permits(
    State(a): State<App>,
    h: HeaderMap,
    Query(q): Query<PermitPage>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let (offset, limit) = page(&Page {
        cursor: q.cursor,
        limit: q.limit,
    })?;
    if q.state
        .as_deref()
        .is_some_and(|state| !["active", "suspect", "released"].contains(&state))
    {
        return Err(invalid());
    }
    let rows=sqlx::query("SELECT * FROM vrs_permits WHERE ($1 OR app=$2) AND ($3::text IS NULL OR state=$3) ORDER BY created_at DESC,id LIMIT $4 OFFSET $5")
        .bind(p.role=="admin").bind(&p.app_id).bind(q.state.as_deref()).bind(limit).bind(offset).fetch_all(&a.db).await.map_err(unavailable)?;
    let items:Vec<Value>=rows.iter().map(|r|{
        let metadata:Value=r.get("metadata");
        json!({"permit_id":r.get::<String,_>("id"),"app_id":r.get::<String,_>("app"),"executor_id":r.get::<String,_>("executor"),"node_id":r.get::<String,_>("node"),"state":r.get::<String,_>("state"),"resources":r.get::<Value,_>("resources"),"delegated_resources":r.get::<Value,_>("delegated_resources"),"placements":metadata["placements"],"model_id":metadata["model_id"],"parent_task_id":metadata["parent_task_id"],"evidence":r.get::<Option<String>,_>("evidence"),"created_at":r.get::<DateTime<Utc>,_>("created_at"),"released_at":r.get::<Option<DateTime<Utc>>,_>("released_at"),"lease_until":r.get::<DateTime<Utc>,_>("lease_until")})
    }).collect();
    Ok(ok(
        json!({"items":items,"next_cursor":if rows.len()==limit as usize{Some((offset+limit).to_string())}else{None}}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermitVerify {
    executor_id: String,
    upstream_url: String,
    model_id: String,
    execution_token: String,
    execution_id: String,
    claim: bool,
}

async fn permit_verify(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<PermitVerify>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    if !(32..=128).contains(&b.execution_token.len())
        || b.execution_id.is_empty()
        || b.execution_id.len() > 128
    {
        return Err(invalid());
    }
    let executor = a
        .config
        .executors
        .iter()
        .find(|e| e.id == b.executor_id && e.task_type == "model.infer")
        .ok_or(Error(StatusCode::FORBIDDEN, "EXECUTOR_NOT_ALLOWED"))?;
    if (executor.app_id != p.app_id && !p.permit_executors.contains(&executor.id))
        || !approved_upstream(executor, &b.upstream_url)
    {
        return Err(Error(StatusCode::FORBIDDEN, "EXECUTOR_NOT_ALLOWED"));
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let row=sqlx::query("SELECT p.*,p.lease_until>now() AS valid FROM vrs_permits p WHERE id=$1 AND executor=$2 AND node=$3 AND (parent_attempt IS NULL OR EXISTS(SELECT 1 FROM vrs_tasks t JOIN vrs_reservations r ON r.attempt=t.attempt WHERE t.attempt=p.parent_attempt AND t.status='running' AND r.state='active' AND r.lease_until>now())) FOR UPDATE OF p")
        .bind(&id).bind(&executor.id).bind(&executor.node_id).fetch_optional(&mut *tx).await.map_err(unavailable)?
        .ok_or(Error(StatusCode::NOT_FOUND,"NOT_FOUND"))?;
    management::principal_active(&mut tx, &p).await?;
    let mut metadata: Value = row.get("metadata");
    if row.get::<String, _>("state") != "active" || !row.get::<bool, _>("valid") {
        return Err(Error(StatusCode::CONFLICT, "PERMIT_NOT_ACTIVE"));
    }
    if metadata["upstream_url"] != b.upstream_url
        || metadata["model_id"] != b.model_id
        || metadata["execution_token_hash"]
            != format!("{:x}", Sha256::digest(b.execution_token.as_bytes()))
    {
        return Err(Error(StatusCode::FORBIDDEN, "PERMIT_BINDING_MISMATCH"));
    }
    // 首次调用原子认领；相同许可不能启动第二个执行。后续核验只确认原执行仍有租约。
    if b.claim {
        if !metadata["execution_id"].is_null() {
            return Err(Error(StatusCode::CONFLICT, "PERMIT_ALREADY_CLAIMED"));
        }
        for part in executor.allocations() {
            let node=sqlx::query("SELECT *,heartbeat>now()-interval '12 seconds' AS fresh FROM vrs_nodes WHERE id=$1").bind(&part.node_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
            allocations::ready(&node, &executor.id)?;
        }
        metadata["execution_id"] = json!(b.execution_id);
        sqlx::query("UPDATE vrs_permits SET metadata=$2 WHERE id=$1")
            .bind(&id)
            .bind(&metadata)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
    } else if metadata["execution_id"] != b.execution_id {
        return Err(Error(StatusCode::FORBIDDEN, "PERMIT_BINDING_MISMATCH"));
    }
    let until: DateTime<Utc> = row.get("lease_until");
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"permit_id":id,"executor_id":executor.id,"node_id":executor.node_id,"model_id":b.model_id,
        "state":"active","lease_seconds":(until-Utc::now()).num_seconds().max(0)}),
    ))
}

async fn permit_renew(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let count=sqlx::query("UPDATE vrs_permits p SET lease_until=now()+interval '30 seconds' WHERE id=$1 AND app=$2 AND state='active' AND lease_until>now() AND (parent_attempt IS NULL OR EXISTS(SELECT 1 FROM vrs_tasks t JOIN vrs_reservations r ON r.attempt=t.attempt WHERE t.attempt=p.parent_attempt AND t.status='running' AND r.state='active' AND r.lease_until>now()))").bind(id).bind(&p.app_id).execute(&a.db).await.map_err(unavailable)?.rows_affected();
    if count != 1 {
        return Err(Error(StatusCode::CONFLICT, "PERMIT_NOT_ACTIVE"));
    }
    Ok(ok(json!({"lease_seconds":30})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermitRelease {
    stopped: bool,
    evidence: String,
}
async fn permit_release(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<PermitRelease>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    if b.evidence.is_empty() || b.evidence.len() > 512 {
        return Err(invalid());
    }
    let state = if b.stopped { "released" } else { "suspect" };
    let count = sqlx::query(
        "UPDATE vrs_permits SET state=$3,evidence=$4,released_at=CASE WHEN $3='released' THEN COALESCE(released_at,now()) ELSE released_at END WHERE id=$1 AND app=$2 AND state<>'released'",
    )
    .bind(&id)
    .bind(&p.app_id)
    .bind(state)
    .bind(&b.evidence)
    .execute(&a.db)
    .await
    .map_err(unavailable)?
    .rows_affected();
    if count == 0 {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_permits WHERE id=$1 AND app=$2)")
                .bind(&id)
                .bind(&p.app_id)
                .fetch_one(&a.db)
                .await
                .map_err(unavailable)?;
        if !exists {
            return Err(Error(StatusCode::NOT_FOUND, "NOT_FOUND"));
        }
    }
    Ok(ok(json!({"permit_id":id,"state":state})))
}
async fn permit_confirm(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<PermitRelease>,
) -> Api<Json<Value>> {
    require(&auth(&a, &h).await?, "admin")?;
    if !b.stopped || b.evidence.trim().is_empty() {
        return Err(invalid());
    }
    sqlx::query(
        "UPDATE vrs_permits SET state='released',evidence=$2,released_at=COALESCE(released_at,now()) WHERE id=$1 AND state='suspect'",
    )
    .bind(&id)
    .bind(b.evidence)
    .execute(&a.db)
    .await
    .map_err(unavailable)?;
    Ok(ok(json!({"permit_id":id,"state":"released"})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermitDelegate {
    task_id: String,
    app_task_id: String,
}
// The gateway holds a bridge permit before asking an asynchronous application
// to create work. Transfer responsibility only to an approved, durable task on
// the same node; never treat the application's "delegated" label as evidence.
async fn permit_delegate(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<PermitDelegate>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let node_id: Option<String> =
        sqlx::query_scalar("SELECT node FROM vrs_permits WHERE id=$1 AND app=$2")
            .bind(&id)
            .bind(&p.app_id)
            .fetch_optional(&a.db)
            .await
            .map_err(unavailable)?;
    let node_id = node_id.ok_or(Error(StatusCode::NOT_FOUND, "NOT_FOUND"))?;
    let multi: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_permit_placements WHERE permit_id=$1)")
            .bind(&id)
            .fetch_one(&a.db)
            .await
            .map_err(unavailable)?;
    if multi {
        return Err(Error(
            StatusCode::CONFLICT,
            "MULTI_NODE_HANDOFF_NOT_SUPPORTED",
        ));
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    sqlx::query("SELECT id FROM vrs_nodes WHERE id=$1 FOR UPDATE")
        .bind(&node_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
    let permit = sqlx::query(
        "SELECT state,evidence,metadata FROM vrs_permits WHERE id=$1 AND app=$2 FOR UPDATE",
    )
    .bind(&id)
    .bind(&p.app_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(unavailable)?;
    if !permit.get::<Value, _>("metadata")["execution_id"].is_null() {
        return Err(Error(StatusCode::CONFLICT, "PERMIT_ALREADY_CLAIMED"));
    }
    let task = sqlx::query(
        "SELECT body,node,app,status FROM vrs_tasks WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(&b.task_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(unavailable)?
    .ok_or(Error(StatusCode::FORBIDDEN, "DELEGATION_NOT_ALLOWED"))?;
    let body: Value = task.get("body");
    let executor = body["executor"]["id"].as_str().unwrap_or("");
    if !p.delegated_executors.iter().any(|e| e == executor)
        || body["app_task_id"] != b.app_task_id
        || task.get::<Option<String>, _>("node").as_deref() != Some(&node_id)
        || !a.config.executors.iter().any(|e| {
            e.id == executor && e.app_id == task.get::<String, _>("app") && e.node_id == node_id
        })
    {
        return Err(Error(StatusCode::FORBIDDEN, "DELEGATION_NOT_ALLOWED"));
    }
    let evidence = format!("delegated_task:{}", b.task_id);
    if permit.get::<String, _>("state") == "released"
        && permit.get::<String, _>("evidence") != evidence
    {
        return Err(Error(StatusCode::CONFLICT, "PERMIT_ALREADY_RELEASED"));
    }
    sqlx::query("UPDATE vrs_permits SET state='released',evidence=$2,released_at=COALESCE(released_at,now()) WHERE id=$1 AND state<>'released'")
        .bind(&id)
        .bind(&evidence)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"permit_id":id,"task_id":b.task_id,"state":"delegated"}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegationVerify {
    task_id: String,
    executor_id: String,
    owner_id: String,
    upstream_url: String,
    #[serde(default)]
    model_id: String,
    #[serde(default)]
    execution_token: String,
}
async fn delegation_verify(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<DelegationVerify>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    if !b.execution_token.is_empty() && !(32..=128).contains(&b.execution_token.len()) {
        return Err(invalid());
    }
    let infer = a
        .config
        .executors
        .iter()
        .find(|e| e.id == b.executor_id && e.app_id == p.app_id && e.task_type == "model.infer")
        .ok_or(Error(StatusCode::FORBIDDEN, "DELEGATION_NOT_ALLOWED"))?;
    if !infer.placements.is_empty() {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "MULTI_NODE_HANDOFF_NOT_SUPPORTED",
        ));
    }
    if !approved_upstream(infer, &b.upstream_url) || b.model_id.len() > 512 {
        return Err(Error(StatusCode::FORBIDDEN, "UPSTREAM_NOT_APPROVED"));
    }
    let key = h
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 256)
        .ok_or(invalid())?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let node = sqlx::query("SELECT *,heartbeat>now()-interval '12 seconds' AS fresh FROM vrs_nodes WHERE id=$1 FOR UPDATE")
        .bind(&infer.node_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
    management::admit(&mut tx, infer, &p).await?;
    let row = sqlx::query("SELECT t.body,t.node,t.attempt,r.resources FROM vrs_tasks t JOIN vrs_reservations r ON r.attempt=t.attempt WHERE t.id=$1 AND t.status='running' AND t.deleted_at IS NULL AND r.state='active' AND r.lease_until>now() FOR UPDATE OF t,r")
        .bind(&b.task_id).fetch_optional(&mut *tx).await.map_err(unavailable)?
        .ok_or(Error(StatusCode::FORBIDDEN, "DELEGATION_NOT_ALLOWED"))?;
    let body: Value = row.get("body");
    let resources: Resources =
        serde_json::from_value(row.get("resources")).map_err(|_| invalid())?;
    let owner = body["parameters"]["owner_id"]
        .as_str()
        .or_else(|| body["parameters"]["user_key"].as_str());
    if b.owner_id.is_empty()
        || owner != Some(b.owner_id.as_str())
        || !p
            .delegated_executors
            .iter()
            .any(|id| body["executor"]["id"] == *id)
        || row.get::<Option<String>, _>("node").as_deref() != Some(infer.node_id.as_str())
        || !infer.resources.fits(&Resources::default(), &resources)
    {
        return Err(Error(StatusCode::FORBIDDEN, "DELEGATION_NOT_ALLOWED"));
    }
    let attempt: String = row.get("attempt");
    if let Some(old) = sqlx::query("SELECT id,executor,parent_attempt,state,metadata,lease_until>now() AS valid FROM vrs_permits WHERE app=$1 AND idem=$2")
        .bind(&p.app_id).bind(key).fetch_optional(&mut *tx).await.map_err(unavailable)? {
        if old.get::<String,_>("executor")!=infer.id || old.get::<Option<String>,_>("parent_attempt").as_deref()!=Some(&attempt) {return Err(Error(StatusCode::CONFLICT,"IDEMPOTENCY_CONFLICT"));}
        let metadata:Value=old.get("metadata");
        if metadata["model_id"]!=b.model_id || metadata["upstream_url"]!=b.upstream_url
            || (!b.execution_token.is_empty() && metadata["execution_token_hash"]!=format!("{:x}",Sha256::digest(b.execution_token.as_bytes()))) {
            return Err(Error(StatusCode::CONFLICT,"IDEMPOTENCY_CONFLICT"));
        }
        if old.get::<String,_>("state")!="active"||!old.get::<bool,_>("valid"){return Err(Error(StatusCode::CONFLICT,"PERMIT_NOT_ACTIVE"));}
        return Ok(ok(json!({"permit_id":old.get::<String,_>("id"),"lease_seconds":30,"node_id":infer.node_id,"parent_task_id":b.task_id})));
    }
    let observed: Value = node.get("report");
    let reserve: Resources =
        serde_json::from_value(node.get("safety_reserve")).map_err(|_| invalid())?;
    if !node.get::<Option<bool>, _>("fresh").unwrap_or(false)
        || !report_fresh(&observed)
        || !probe_ready(&observed, &infer.id)
        || !infer
            .resources
            .fits_guarded(&Resources::default(), &observed, &reserve)
    {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "WAIT_RESOURCE"));
    }
    let mut children = Resources::default();
    for child in sqlx::query(
        "SELECT delegated_resources FROM vrs_permits WHERE parent_attempt=$1 AND state<>'released'",
    )
    .bind(&attempt)
    .fetch_all(&mut *tx)
    .await
    .map_err(unavailable)?
    {
        children
            .add(&serde_json::from_value(child.get("delegated_resources")).map_err(|_| invalid())?);
    }
    if !infer.resources.fits(&children, &resources) {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "DELEGATED_BUDGET_BUSY",
        ));
    }
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO vrs_permits(id,app,idem,executor,node,resources,parent_attempt,delegated_resources,metadata) VALUES($1,$2,$3,$4,$5,'{}',$6,$7,$8)")
        .bind(&id).bind(&p.app_id).bind(key).bind(&infer.id).bind(&infer.node_id).bind(&attempt).bind(json!(infer.resources)).bind(json!({"model_id":b.model_id,"upstream_url":b.upstream_url,"parent_task_id":b.task_id,"execution_token_hash":format!("{:x}",Sha256::digest(b.execution_token.as_bytes()))})).execute(&mut *tx).await.map_err(unavailable)?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"permit_id":id,"lease_seconds":30,"node_id":infer.node_id,"parent_task_id":b.task_id}),
    ))
}

async fn attempts(State(a): State<App>, h: HeaderMap, Path(id): Path<String>) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    get_task(&a, &p, &id).await?;
    let rows = sqlx::query("SELECT * FROM vrs_attempts WHERE task=$1 ORDER BY fence")
        .bind(id)
        .fetch_all(&a.db)
        .await
        .map_err(unavailable)?;
    Ok(ok(json!(rows.iter().map(|r|json!({"attempt_id":r.get::<String,_>("id"),"fence":r.get::<i64,_>("fence"),"node_id":r.get::<String,_>("node"),"status":r.get::<String,_>("status"),"result":r.get::<Value,_>("result"),"created_at":r.get::<DateTime<Utc>,_>("created_at"),"stopped_at":r.get::<Option<DateTime<Utc>>,_>("stopped_at")})).collect::<Vec<_>>())))
}
async fn retry_task(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    get_task(&a, &p, &id).await?;
    let key = h
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or(invalid())?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let row = sqlx::query("SELECT * FROM vrs_tasks WHERE id=$1 FOR UPDATE")
        .bind(&id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
    let seen: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM vrs_actions WHERE task=$1 AND key=$2 AND action='retry')",
    )
    .bind(&id)
    .bind(key)
    .fetch_one(&mut *tx)
    .await
    .map_err(unavailable)?;
    if seen {
        return Ok(ok(task_value(&row)));
    }
    let body: Value = row.get("body");
    let e = a
        .config
        .executors
        .iter()
        .find(|e| e.id == body["executor"]["id"])
        .ok_or(invalid())?;
    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM vrs_reservations WHERE task=$1 AND state<>'released')",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(unavailable)?;
    if occupied
        || row.get::<String, _>("status") != "failed"
        || e.retry_mode != "idempotent"
        || row.get::<i64, _>("fence") >= e.max_attempts
    {
        return Err(Error(StatusCode::CONFLICT, "RETRY_NOT_ALLOWED"));
    }
    sqlx::query("INSERT INTO vrs_actions(task,key,action) VALUES($1,$2,'retry')")
        .bind(&id)
        .bind(key)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    let new=sqlx::query("UPDATE vrs_tasks SET status='queued',retry_at=NULL,progress='{}',result='{}',version=version+1,updated_at=now() WHERE id=$1 RETURNING *").bind(&id).fetch_one(&mut *tx).await.map_err(unavailable)?;
    event(&mut tx, &id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(task_value(&new)))
}
async fn delete_task(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let row = sqlx::query("SELECT * FROM vrs_tasks WHERE id=$1 FOR UPDATE")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        .ok_or(Error(StatusCode::NOT_FOUND, "NOT_FOUND"))?;
    if !visible(&p, &row.get::<String, _>("app")) {
        return Err(Error(StatusCode::NOT_FOUND, "NOT_FOUND"));
    }
    if row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some() {
        return Ok(ok(json!({"task_id":id,"deleted":true})));
    }
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM vrs_reservations WHERE task=$1 AND state<>'released')",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(unavailable)?;
    if active
        || !["failed", "cancelled", "succeeded"].contains(&row.get::<String, _>("status").as_str())
    {
        return Err(Error(StatusCode::CONFLICT, "TASK_NOT_STOPPED"));
    }
    sqlx::query("UPDATE vrs_tasks SET status='deleted',deleted_at=now(),version=version+1,updated_at=now() WHERE id=$1").bind(&id).execute(&mut *tx).await.map_err(unavailable)?;
    event(&mut tx, &id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"task_id":id,"deleted":true,"outputs_deleted":false,"audit_retained":true}),
    ))
}

#[cfg(test)]
mod status_progress_tests {
    use super::*;

    #[test]
    fn accepts_optional_stage_and_message_without_requiring_percent() {
        let progress = reported_progress(Value::Null, Some("decode"), Some("segment 2/4"));
        assert_eq!(
            progress.ok(),
            Some(json!({"stage":"decode","message":"segment 2/4"}))
        );
    }

    #[test]
    fn preserves_existing_progress_and_rejects_invalid_percent() {
        let progress = reported_progress(
            json!({"percent":0.5,"stage":"old","step":2}),
            Some("new"),
            None,
        );
        assert_eq!(
            progress.ok(),
            Some(json!({"percent":0.5,"stage":"new","step":2}))
        );
        assert!(reported_progress(json!({"percent":75}), None, None).is_err());
        assert!(reported_progress(json!([1, 2]), None, None).is_err());
    }
}
