use super::*;
use crate::model::Executor;
use std::path::Path as FsPath;

const OWNER: &str = "com.ictrek.image-studio";
pub(super) const EXECUTOR_PREFIX: &str = "app-remote-image-studio-";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registration {
    model_id: String,
    transfer_id: String,
}

fn unavailable_target() -> Error {
    Error(StatusCode::CONFLICT, "BACKEND_EXECUTOR_TARGET_NOT_READY")
}

fn missing_approval() -> Error {
    Error(StatusCode::CONFLICT, "BACKEND_MODEL_NOT_CALIBRATED")
}

fn approvals() -> anyhow::Result<Value> {
    let path = FsPath::new("/config/app-approvals.json");
    if path.exists() {
        app_bindings::read_private(path)
    } else {
        Ok(serde_json::from_slice(&std::fs::read("/opt/v-scheduler/app-calibrations.json")?)?)
    }
}

pub(super) async fn admit(tx: &mut Transaction<'_, Postgres>, executor: &Executor) -> Api<()> {
    if !executor.id.starts_with(EXECUTOR_PREFIX) {
        return Ok(());
    }
    let ready: Option<bool> = sqlx::query_scalar(
        "SELECT d.state='running' AND d.last_error IS NULL FROM vrs_backend_deployments d
         JOIN vrs_backend_executor_bindings b ON b.deployment_id=d.id
         WHERE b.executor_id=$1 FOR SHARE OF d",
    ).bind(&executor.id).fetch_optional(&mut **tx).await.map_err(unavailable)?;
    if ready != Some(true) {
        return Err(Error(StatusCode::CONFLICT, "BACKEND_STATE_CONFLICT"));
    }
    Ok(())
}

pub(super) async fn register(
    State(app): State<App>,
    headers: HeaderMap,
    Path(deployment_id): Path<String>,
    Json(request): Json<Registration>,
) -> Api<Json<Value>> {
    let (kind, target_id, profile) = backend_deployments::image_studio_target(&app, &headers, &deployment_id).await?;
    let authn = auth(&app, &headers).await?;
    let reply = backend_deployments::exchange(&app, &headers, &deployment_id,
        backend_transfers::wire::Operation::ReadTransfer { transfer_id: request.transfer_id.clone() }, &[]).await?.0;
    if reply["data"]["model_id"] != request.model_id || reply["data"]["verified"] != true {
        return Err(Error(StatusCode::CONFLICT, "BACKEND_MODEL_NOT_VERIFIED"));
    }
    let capability = backend_deployments::exchange(&app, &headers, &deployment_id,
        backend_transfers::wire::Operation::Capabilities, &[]).await?.0;
    let mut candidates = app.config.nodes.iter().filter(|node| match kind.as_str() {
        "cluster" => node.vos_member_id == target_id,
        "ssh" => node.discovery_source == "modelinfer" && node.discovery_id == target_id,
        _ => false,
    });
    let node = candidates.next().ok_or_else(unavailable_target)?;
    if candidates.next().is_some() { return Err(unavailable_target()); }

    let mut tx = app.db.begin().await.map_err(unavailable)?;
    let sample = sqlx::query(
        "SELECT report,capacities,resident_baseline FROM vrs_nodes WHERE id=$1 AND management_enabled
         AND NOT draining AND heartbeat>now()-interval '12 seconds' FOR UPDATE",
    )
    .bind(&node.id).fetch_optional(&mut *tx).await.map_err(unavailable)?.ok_or_else(unavailable_target)?;
    let report: Value = sample.get("report");
    if !report_fresh(&report) || report["application_backend_dispatch"] != true {
        return Err(unavailable_target());
    }
    let architecture = report["arch"].as_str().ok_or_else(unavailable_target)?;
    let (task_type, approval) = app_bindings::remote_image_approval(
        &profile, architecture, &report, &capability["data"], &request.model_id,
        &approvals().map_err(|_| missing_approval())?,
    ).ok_or_else(missing_approval)?;
    let resources: Resources = serde_json::from_value(approval["resources"].clone()).map_err(|_| missing_approval())?;
    let capacity: Resources = serde_json::from_value(sample.get("capacities")).map_err(|_| unavailable_target())?;
    let baseline: Resources = serde_json::from_value(sample.get("resident_baseline")).map_err(|_| unavailable_target())?;
    if !resources.valid() || resources.cpu_millicores <= 0 || resources.memory_mib <= 0
        || !resources.fits(&baseline, &capacity) {
        return Err(missing_approval());
    }

    let digest = format!("{:x}", Sha256::digest(
        serde_json::to_vec(&json!([deployment_id, node.id, request.model_id, approval]))
            .map_err(|_| invalid())?,
    ));
    let executor_id = format!("{EXECUTOR_PREFIX}{}", &digest[..24]);
    let executor = Executor {
        id: executor_id.clone(),
        contract_version: "1".into(),
        task_type,
        node_id: node.id.clone(),
        app_id: OWNER.into(),
        resources,
        upstream_urls: vec![],
        placements: vec![],
        retry_mode: "none".into(),
        max_attempts: 1,
    };
    // Serialize registration with stop/remove and concurrent requests for the same deployment.
    let running: Option<String> = sqlx::query_scalar("SELECT state FROM vrs_backend_deployments WHERE id=$1 FOR UPDATE")
        .bind(&deployment_id).fetch_optional(&mut *tx).await.map_err(unavailable)?;
    if running.as_deref() != Some("running") {
        return Err(Error(StatusCode::CONFLICT, "BACKEND_STATE_CONFLICT"));
    }
    if let Some((prior, prior_transfer)) = sqlx::query_as::<_, (String, String)>(
        "SELECT executor_id,transfer_id FROM vrs_backend_executor_bindings WHERE deployment_id=$1 AND model_id=$2",
    )
    .bind(&deployment_id).bind(&request.model_id).fetch_optional(&mut *tx).await.map_err(unavailable)?
    {
        if prior != executor_id { return Err(Error(StatusCode::CONFLICT, "BACKEND_MODEL_BINDING_CONFLICT")); }
        if prior_transfer != request.transfer_id {
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM vrs_tasks WHERE body->'executor'->>'id'=$1
                 AND status IN ('queued','assigned','running','cancelling','recovering'))",
            ).bind(&executor_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
            if active {
                return Err(Error(StatusCode::CONFLICT, "BACKEND_TASKS_ACTIVE"));
            }
            sqlx::query("UPDATE vrs_backend_executor_bindings SET transfer_id=$2 WHERE executor_id=$1")
                .bind(&executor_id).bind(&request.transfer_id).execute(&mut *tx).await.map_err(unavailable)?;
        }
    }
    let definition = json!(executor);
    sqlx::query("INSERT INTO vrs_executor_settings(id,node_id,definition,state) VALUES($1,$2,$3,'enabled') ON CONFLICT(id) DO NOTHING")
        .bind(&executor_id).bind(&node.id).bind(&definition).execute(&mut *tx).await.map_err(unavailable)?;
    let enabled: bool = sqlx::query_scalar("SELECT state='enabled' AND definition=$2 FROM vrs_executor_settings WHERE id=$1")
        .bind(&executor_id).bind(&definition).fetch_one(&mut *tx).await.map_err(unavailable)?;
    if !enabled { return Err(Error(StatusCode::FORBIDDEN, "EXECUTOR_NOT_ALLOWED")); }
    sqlx::query("INSERT INTO vrs_backend_executor_bindings(executor_id,deployment_id,transfer_id,model_id,profile,model_limits,model_parameters) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(executor_id) DO NOTHING")
        .bind(&executor_id).bind(&deployment_id).bind(&request.transfer_id).bind(&request.model_id)
        .bind(&profile).bind(&approval["model_limits"]).bind(&approval["model_parameters"]).execute(&mut *tx).await.map_err(unavailable)?;
    management::audit(&mut tx, &authn, "backend_model.bind", &executor_id).await?;
    tx.commit().await.map_err(unavailable)?;
    management::restore(&app.db, &app.voice_config).await
        .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "BACKEND_EXECUTOR_UNAVAILABLE"))?;
    Ok(ok(json!({
        "executor_id": executor_id, "node_id": node.id, "profile": profile,
        "model_limits": approval["model_limits"], "model_parameters": approval["model_parameters"]
    })))
}

#[derive(sqlx::FromRow)]
struct Attempt {
    task_id: String,
    executor_id: String,
    deployment_id: String,
    transfer_id: String,
    node_id: String,
    session: Option<String>,
    fence: i64,
    status: String,
    lease_active: Option<bool>,
    parameters: Value,
    preempted: bool,
    model_limits: Value,
    model_parameters: Value,
}

async fn attempt(db: &PgPool, id: &str) -> Api<Attempt> {
    sqlx::query_as(
        "SELECT t.id AS task_id,b.executor_id,b.deployment_id,b.transfer_id,t.node AS node_id,n.session,
         t.fence,t.status,(r.state IN ('held','active') AND r.lease_until>now()) AS lease_active,
         t.body->'parameters' AS parameters,(t.body ? '_scheduler_preemption') AS preempted,
         b.model_limits,b.model_parameters FROM vrs_tasks t
         JOIN vrs_backend_executor_bindings b ON b.executor_id=t.body->'executor'->>'id'
         JOIN vrs_nodes n ON n.id=t.node
         LEFT JOIN vrs_reservations r ON r.attempt=t.attempt
         WHERE t.attempt=$1 AND t.app=$2",
    )
    .bind(id).bind(OWNER).fetch_optional(db).await.map_err(unavailable)?
    .ok_or(Error(StatusCode::NOT_FOUND, "BACKEND_ATTEMPT_NOT_FOUND"))
}

fn verify_action(record: &Attempt, fence: i64, action: backend_transfers::wire::Action) -> Api<()> {
    if record.fence != fence || (matches!(action, backend_transfers::wire::Action::Start)
        && (record.lease_active != Some(true) || !matches!(record.status.as_str(), "assigned"|"running"))) {
        return Err(Error(StatusCode::CONFLICT, "STALE_ATTEMPT"));
    }
    Ok(())
}

fn payload(record: &Attempt) -> Value {
    json!({
        "task_id": record.task_id, "executor_id": record.executor_id, "fence": record.fence,
        "lease_seconds":30,"parameters":record.parameters,"preempted":record.preempted
    })
}

pub(super) async fn execute(
    State(app): State<App>, headers: HeaderMap,
    Path((id, attempt_id, action)): Path<(String, String, backend_transfers::wire::Action)>,
    Json(input): Json<Value>,
) -> Api<Json<Value>> {
    backend_deployments::image_studio_target(&app, &headers, &id).await?;
    let record = attempt(&app.db,&attempt_id).await?;
    if record.deployment_id != id || input["task_id"] != record.task_id {
        return Err(Error(StatusCode::NOT_FOUND, "BACKEND_ATTEMPT_NOT_FOUND"));
    }
    verify_action(&record, input["fence"].as_i64().ok_or_else(invalid)?, action)?;
    let mut body = payload(&record);
    if matches!(action, backend_transfers::wire::Action::Start) {
        if !input["job"].is_object() { return Err(invalid()); }
        body["transfer_id"] = json!(record.transfer_id);
        body["job"] = input["job"].clone();
        body["reference"] = input["reference"].clone();
        body["model_limits"] = record.model_limits.clone();
        body["model_parameters"] = record.model_parameters.clone();
    }
    let bytes = serde_json::to_vec(&body).map_err(|_| invalid())?;
    backend_deployments::exchange(&app,&headers,&id,
        backend_transfers::wire::Operation::Execute { attempt_id, action }, &bytes).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IoOffset { offset: u64 }

async fn verify_io(app: &App, headers: &HeaderMap, id: &str, attempt_id: &str, write: bool) -> Api<()> {
    backend_deployments::image_studio_target(app,headers,id).await?;
    let record = attempt(&app.db,attempt_id).await?;
    if record.deployment_id != id {
        return Err(Error(StatusCode::NOT_FOUND, "BACKEND_ATTEMPT_NOT_FOUND"));
    }
    let fence: i64 = headers.get("x-scheduler-fence")
        .and_then(|value| value.to_str().ok()).and_then(|value| value.parse().ok()).ok_or_else(invalid)?;
    verify_action(&record, fence, if write { backend_transfers::wire::Action::Start } else { backend_transfers::wire::Action::Inspect })
}

pub(super) async fn reference(
    State(app): State<App>, headers: HeaderMap,
    Path((id, attempt_id)): Path<(String, String)>, Query(IoOffset {offset}): Query<IoOffset>,
    bytes: axum::body::Bytes,
) -> Api<Json<Value>> {
    if headers.get(axum::http::header::CONTENT_TYPE).and_then(|value| value.to_str().ok()) != Some("application/octet-stream") {
        return Err(Error(StatusCode::UNSUPPORTED_MEDIA_TYPE,"BACKEND_TRANSFER_CONTENT_TYPE"));
    }
    verify_io(&app,&headers,&id,&attempt_id,true).await?;
    backend_deployments::exchange(&app,&headers,&id,
        backend_transfers::wire::Operation::WriteReference{attempt_id,offset},&bytes).await
}

pub(super) async fn output(
    State(app): State<App>, headers: HeaderMap,
    Path((id, attempt_id, file_id)): Path<(String, String, String)>, Query(IoOffset {offset}): Query<IoOffset>,
) -> Api<Json<Value>> {
    verify_io(&app,&headers,&id,&attempt_id,false).await?;
    backend_deployments::exchange(&app,&headers,&id,
        backend_transfers::wire::Operation::ReadOutput{attempt_id,file_id,offset},&[]).await
}

#[derive(Deserialize)]
pub(super) struct WorkerDispatch { session_id:String, fence:i64 }

pub(super) async fn dispatch(
    State(app): State<App>, headers: HeaderMap,
    Path((node_id, attempt_id, action)): Path<(String, String, backend_transfers::wire::Action)>,
    Json(input): Json<WorkerDispatch>,
) -> Api<Json<Value>> {
    let principal = auth(&app,&headers).await?;
    worker(&principal,&node_id)?;
    let record = attempt(&app.db,&attempt_id).await?;
    if record.node_id != node_id || record.session.as_deref() != Some(input.session_id.as_str()) {
        return Err(Error(StatusCode::CONFLICT,"WORKER_SESSION_CONFLICT"));
    }
    verify_action(&record,input.fence,action)?;
    let failure = || Error(StatusCode::BAD_GATEWAY,"BACKEND_EXECUTOR_UNAVAILABLE");
    let token = app_bindings::image_studio_executor_token().map_err(|_| failure())?;
    let endpoint = format!("http://image-studio-backend:5000/api/v1/scheduler/executions/{attempt_id}/{}", action.segment());
    let client = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::none()).build().map_err(|_| failure())?;
    let response = client.post(endpoint).bearer_auth(token).json(&payload(&record)).send().await.map_err(|_| failure())?;
    let value = app_bindings::bounded_json(response).await.map_err(|_| failure())?;
    // The existing Agent client expects the Scheduler response envelope.
    Ok(ok(value))
}
