use crate::model::*;
use anyhow::{Context, ensure};
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
use std::{path::PathBuf, sync::Arc};
use tower_http::services::ServeDir;
use uuid::Uuid;
#[derive(Clone)]
struct App {
    db: PgPool,
    config: Arc<Config>,
}
struct Error(StatusCode, &'static str);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(envelope(json!({}), self.1))).into_response()
    }
}
type Api<T> = Result<T, Error>;
fn envelope(data: Value, code: &str) -> Value {
    json!({"code":code,"category":"v-scheduler","data":data,"msg":code,"version":"1","request_id":Uuid::new_v4().to_string()})
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
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(Error(StatusCode::UNAUTHORIZED, "UNAUTHENTICATED"))?;
    let digest = Sha256::digest(token.as_bytes());
    if let Some(p) = a
        .config
        .principals
        .iter()
        .find(|p| Sha256::digest(p.token.as_bytes()) == digest)
    {
        return Ok(p.clone());
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
fn task_value(r: &sqlx::postgres::PgRow) -> Value {
    let status: String = r.get("status");
    let body: Value = r.get("body");
    let policy = &body["_scheduler_policy"];
    let can_retry = status == "failed"
        && policy["retry_mode"] == "idempotent"
        && r.get::<i64, _>("fence") < policy["max_attempts"].as_i64().unwrap_or(1);
    json!({"task_id":r.get::<String,_>("id"),"app_task_id":body["app_task_id"],"executor_id":body["executor"]["id"],"can_retry":can_retry,"attempt_count":r.get::<i64,_>("fence"),"app_id":r.get::<String,_>("app"),"status":status,"version":r.get::<i64,_>("version"),"attempt_id":r.get::<Option<String>,_>("attempt"),"node_id":r.get::<Option<String>,_>("node"),"fence":r.get::<i64,_>("fence"),"progress":r.get::<Value,_>("progress"),"result":r.get::<Value,_>("result"),"created_at":r.get::<DateTime<Utc>,_>("created_at"),"wait_reasons":if status=="queued" {json!(["WAIT_RESOURCE_OR_NODE"])} else {json!([])}})
}
async fn event(tx: &mut Transaction<'_, Postgres>, id: &str) -> Api<()> {
    sqlx::query("INSERT INTO vrs_events(task,app,status,version) SELECT id,app,status,version FROM vrs_tasks WHERE id=$1").bind(id).execute(&mut **tx).await.map_err(unavailable)?;
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
        "UPDATE vrs_permits SET state='suspect' WHERE state='active' AND lease_until<now()",
    )
    .execute(&mut *tx)
    .await
    .map_err(unavailable)?;
    let ready=sqlx::query("UPDATE vrs_tasks SET status='queued',version=version+1,updated_at=now() WHERE status='retry_wait' AND retry_at<=now() RETURNING id").fetch_all(&mut *tx).await.map_err(unavailable)?;
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
) -> anyhow::Result<()> {
    let config: Config = serde_json::from_slice(&std::fs::read(path)?)?;
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
    }
    for e in &config.executors {
        ensure!(
            (1..=10).contains(&e.max_attempts),
            "max_attempts must be 1..10"
        );
        ensure!(
            ["", "none", "idempotent"].contains(&e.retry_mode.as_str()),
            "invalid retry policy"
        );
        let n = config
            .nodes
            .iter()
            .find(|n| n.id == e.node_id)
            .context("unknown executor node")?;
        ensure!(
            e.resources.fits(&Resources::default(), &n.capacities),
            "executor exceeds node capacity"
        );
    }
    let db = PgPoolOptions::new()
        .max_connections(12)
        .connect(&url)
        .await
        .context("database connection failed")?;
    sqlx::migrate!().run(&db).await?;
    for n in &config.nodes {
        sqlx::query("INSERT INTO vrs_nodes(id,capacities) VALUES($1,$2) ON CONFLICT(id) DO UPDATE SET capacities=excluded.capacities").bind(&n.id).bind(json!(n.capacities)).execute(&db).await?;
    }
    let a = App {
        db,
        config: Arc::new(config),
    };
    crate::vos::spawn(a.db.clone(), a.config.clone())?;
    let ticker = a.clone();
    tokio::spawn(async move {
        loop {
            let _ = expire(&ticker).await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
    let api = Router::new()
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
        .route("/executors", get(executors))
        .route("/tasks", post(submit).get(tasks))
        .route("/tasks/{id}", get(detail).delete(delete_task))
        .route("/tasks/{id}/retry", post(retry_task))
        .route("/tasks/{id}/attempts", get(attempts))
        .route("/tasks/{id}/cancel", post(cancel))
        .route("/events", get(events))
        .route("/permits", post(permit_acquire))
        .route("/permits/{id}/renew", post(permit_renew))
        .route("/permits/{id}/release", post(permit_release))
        .route("/permits/{id}/confirm-stopped", post(permit_confirm))
        .route("/nodes", get(nodes))
        .route("/nodes/{id}/drain", post(drain))
        .route("/reservations", get(reservations))
        .route("/workers/{id}/heartbeat", post(heartbeat))
        .route("/workers/{id}/commands/claim", post(claim))
        .route("/attempts/{id}/renew", post(renew))
        .route("/attempts/{id}/report", post(report));
    let mut router = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .nest("/api/v1", api)
        .layer(axum::extract::DefaultBodyLimit::max(262144))
        .with_state(a);
    if let Some(path) = ui {
        router = router.fallback_service(ServeDir::new(path));
    }
    let socket = tokio::net::TcpListener::bind(&listen).await?;
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
        json!({"protocol_version":1,"implementation_version":"0.1.0","features":["tasks","idempotency","reservations","events","static-executors","agent-image-studio","permits","retry","manual-delete","attempt-history"],"unsupported":["artifacts","dynamic-enrollment","ha"],"authentication":"scoped-bearer","max_body_bytes":262144,"openapi_url":"openapi.json"}),
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
    Ok(ok(
        json!({"items":rs.iter().map(task_value).collect::<Vec<_>>(),"next_cursor":if rs.len()==limit as usize{Some((offset+limit).to_string())}else{None}}),
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
    Ok(ok(task_value(&get_task(&a, &p, &id).await?)))
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
    Ok(ok(task_value(&get_task(&a, &p, &id).await?)))
}
async fn events(State(a): State<App>, h: HeaderMap, Query(q): Query<Page>) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let (c, l) = page(&q)?;
    let rs=sqlx::query("SELECT cursor,task,status,version FROM vrs_events WHERE cursor>$1 AND ($2 OR app=$3) ORDER BY cursor LIMIT $4").bind(c).bind(p.role=="admin").bind(&p.app_id).bind(l).fetch_all(&a.db).await.map_err(unavailable)?;
    let cursor = rs.last().map(|r| r.get::<i64, _>("cursor")).unwrap_or(c);
    Ok(ok(
        json!({"items":rs.iter().map(|r|json!({"cursor":r.get::<i64,_>("cursor"),"task_id":r.get::<String,_>("task"),"status":r.get::<String,_>("status"),"version":r.get::<i64,_>("version")})).collect::<Vec<_>>(),"next_cursor":cursor.to_string()}),
    ))
}
async fn nodes(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let rs = sqlx::query("SELECT * FROM vrs_nodes ORDER BY id")
        .fetch_all(&a.db)
        .await
        .map_err(unavailable)?;
    let mut views: Vec<Value> = rs.iter().map(|r|json!({"id":r.get::<String,_>("id"),"capacities":r.get::<Value,_>("capacities"),"report":r.get::<Value,_>("report"),"draining":r.get::<bool,_>("draining"),"heartbeat":r.get::<Option<DateTime<Utc>>,_>("heartbeat")})).collect();
    crate::vos::decorate(&a.db, &a.config, &mut views)
        .await
        .map_err(unavailable)?;
    Ok(ok(json!(views)))
}
async fn reservations(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let rs =
        sqlx::query("SELECT attempt,task,node,resources,state,lease_until,created_at FROM vrs_reservations WHERE state<>'released' UNION ALL SELECT id AS attempt, 'permit:' || id AS task,node,resources,state,lease_until,created_at FROM vrs_permits WHERE state<>'released' ORDER BY created_at")
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
    require(&auth(&a, &h).await?, "admin")?;
    let d = body
        .get("draining")
        .and_then(Value::as_bool)
        .ok_or(invalid())?;
    sqlx::query("UPDATE vrs_nodes SET draining=$2 WHERE id=$1")
        .bind(id)
        .bind(d)
        .execute(&a.db)
        .await
        .map_err(unavailable)?;
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
    let changed=sqlx::query("UPDATE vrs_nodes SET heartbeat=now(),session=$2,report=$3 WHERE id=$1 AND (session IS NULL OR session=$2)").bind(id).bind(b.session_id).bind(b.report).execute(&a.db).await.map_err(unavailable)?.rows_affected();
    if changed == 0 {
        return Err(Error(StatusCode::CONFLICT, "WORKER_SESSION_CONFLICT"));
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
    if node.get::<bool, _>("draining") || !node.get::<Option<bool>, _>("fresh").unwrap_or(false) {
        return Ok(ok(json!({"commands":[]})));
    }
    let capacity: Resources =
        serde_json::from_value(node.get("capacities")).map_err(|_| invalid())?;
    let mut used = Resources::default();
    for r in
        sqlx::query("SELECT resources FROM vrs_reservations WHERE node=$1 AND state<>'released' UNION ALL SELECT resources FROM vrs_permits WHERE node=$1 AND state<>'released'")
            .bind(&id)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?
    {
        used.add(&serde_json::from_value(r.get("resources")).map_err(|_| invalid())?);
    }
    let queued=sqlx::query("SELECT * FROM vrs_tasks WHERE node=$1 AND status='queued' ORDER BY created_at,id LIMIT 32 FOR UPDATE SKIP LOCKED").bind(&id).fetch_all(&mut *tx).await.map_err(unavailable)?;
    for r in queued {
        let body: Value = r.get("body");
        let Some(e) = a
            .config
            .executors
            .iter()
            .find(|e| e.id == body["executor"]["id"] && e.node_id == id)
        else {
            continue;
        };
        if !e.resources.fits(&used, &capacity) {
            continue;
        }
        let observed: Value = node.get("report");
        if !e.resources.fits_observed(&observed) {
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
    result: Value,
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
    sqlx::query("UPDATE vrs_tasks SET status=$2,progress=$3,result=$4,version=version+1,updated_at=now() WHERE id=$1").bind(&task).bind(status).bind(b.progress).bind(if status=="cancelled" {json!({"reason":"cancelled"})}else{b.result.clone()}).execute(&mut *tx).await.map_err(unavailable)?;
    sqlx::query("UPDATE vrs_attempts SET status=$2,result=$3,stopped_at=CASE WHEN $4 THEN now() ELSE stopped_at END WHERE id=$1").bind(&id).bind(&b.status).bind(&b.result).bind(terminal).execute(&mut *tx).await.map_err(unavailable)?;
    if auto_retry {
        sqlx::query("UPDATE vrs_tasks SET retry_at=now()+interval '3 seconds' * power(2,least(fence-1,4)) WHERE id=$1").bind(&task).execute(&mut *tx).await.map_err(unavailable)?;
    }
    if terminal {
        sqlx::query(
            "UPDATE vrs_reservations SET state='released',stopped_at=now() WHERE attempt=$1",
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
}
async fn permit_acquire(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<PermitRequest>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
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
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let node=sqlx::query("SELECT *,heartbeat>now()-interval '12 seconds' AS fresh FROM vrs_nodes WHERE id=$1 FOR UPDATE").bind(&e.node_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
    if let Some(old) =
        sqlx::query("SELECT *,lease_until>now() AS valid FROM vrs_permits WHERE app=$1 AND idem=$2")
            .bind(&p.app_id)
            .bind(key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
    {
        if old.get::<String, _>("executor") != b.executor_id {
            return Err(Error(StatusCode::CONFLICT, "IDEMPOTENCY_CONFLICT"));
        }
        if old.get::<String, _>("state") != "active" || !old.get::<bool, _>("valid") {
            return Err(Error(StatusCode::CONFLICT, "PERMIT_NOT_ACTIVE"));
        }
        return Ok(ok(
            json!({"permit_id":old.get::<String,_>("id"),"lease_seconds":30,"node_id":e.node_id}),
        ));
    }
    if node.get::<bool, _>("draining") || !node.get::<Option<bool>, _>("fresh").unwrap_or(false) {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "WAIT_RESOURCE_OR_NODE",
        ));
    }
    let cap: Resources = serde_json::from_value(node.get("capacities")).map_err(|_| invalid())?;
    let mut used = Resources::default();
    for r in sqlx::query("SELECT resources FROM vrs_reservations WHERE node=$1 AND state<>'released' UNION ALL SELECT resources FROM vrs_permits WHERE node=$1 AND state<>'released'").bind(&e.node_id).fetch_all(&mut *tx).await.map_err(unavailable)?{used.add(&serde_json::from_value(r.get("resources")).map_err(|_|invalid())?);}
    let queued: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM vrs_tasks WHERE node=$1 AND status='queued')",
    )
    .bind(&e.node_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(unavailable)?;
    let observed: Value = node.get("report");
    if queued || !e.resources.fits(&used, &cap) || !e.resources.fits_observed(&observed) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "WAIT_RESOURCE"));
    }
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO vrs_permits(id,app,idem,executor,node,resources) VALUES($1,$2,$3,$4,$5,$6)",
    )
    .bind(&id)
    .bind(&p.app_id)
    .bind(key)
    .bind(&e.id)
    .bind(&e.node_id)
    .bind(json!(e.resources))
    .execute(&mut *tx)
    .await
    .map_err(unavailable)?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"permit_id":id,"lease_seconds":30,"node_id":e.node_id}),
    ))
}
async fn permit_renew(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let count=sqlx::query("UPDATE vrs_permits SET lease_until=now()+interval '30 seconds' WHERE id=$1 AND app=$2 AND state='active' AND lease_until>now()").bind(id).bind(&p.app_id).execute(&a.db).await.map_err(unavailable)?.rows_affected();
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
        "UPDATE vrs_permits SET state=$3,evidence=$4 WHERE id=$1 AND app=$2 AND state<>'released'",
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
        "UPDATE vrs_permits SET state='released',evidence=$2 WHERE id=$1 AND state='suspect'",
    )
    .bind(&id)
    .bind(b.evidence)
    .execute(&a.db)
    .await
    .map_err(unavailable)?;
    Ok(ok(json!({"permit_id":id,"state":"released"})))
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
