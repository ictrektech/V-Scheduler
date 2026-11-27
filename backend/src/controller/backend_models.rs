use super::*;
use crate::model::Executor;
use std::path::Path as FsPath;

const LEGACY_STUDIO: &str = "com.ictrek.image-studio";
const LEGACY_PREFIX: &str = "app-remote-image-studio-";
pub(super) const EXECUTOR_PREFIX: &str = crate::model::APPLICATION_BACKEND_EXECUTOR_PREFIX;

pub(super) fn check_task_parameters(executor: &Executor, parameters: &Value) -> Api<()> {
    if executor.id.starts_with(EXECUTOR_PREFIX)
        && !executor.id.starts_with(LEGACY_PREFIX)
        && serde_json::to_vec(parameters).map_err(|_| invalid())?.len() > 32 * 1024
    {
        return Err(Error(StatusCode::PAYLOAD_TOO_LARGE, "BACKEND_EXECUTION_PARAMETERS_TOO_LARGE"));
    }
    Ok(())
}


fn remote_approval(
    target: &backend_deployments::ChannelTarget,
    architecture: &str,
    report: &Value,
    capabilities: &Value,
    model_id: &str,
    approvals: &Value,
) -> Option<(String, Value)> {
    if target.legacy_studio {
        return app_bindings::remote_image_approval(
            &target.profile, architecture, report, capabilities, model_id, approvals,
        );
    }
    if approvals["protocol_version"] != 1 {
        return None;
    }
    let (task_type, _) = model_capability(capabilities, model_id, false)?;
    let hardware = app_bindings::hardware(report);
    let mut matching = approvals["application_backends"].as_array()?.iter().filter(|policy| {
        policy["app_id"] == target.app_id
            && policy["image"] == target.image
            && policy["profile"] == target.profile
            && policy["architecture"] == architecture
            && policy["hardware"] == hardware
            && policy["model_id"] == model_id
            && policy["task_type"] == task_type
            && policy["execution_settings"].is_object()
            && policy["execution_settings"] == capabilities["execution_settings"]
            && policy["evidence"]["confirmed_stopped"] == true
            && policy["evidence"]["id"].as_str().is_some_and(|id| !id.is_empty())
            && policy["model_limits"].is_object()
            && policy["model_parameters"].is_object()
    });
    let policy = matching.next()?;
    if matching.next().is_some()
        || serde_json::to_vec(&json!([policy["model_limits"], policy["model_parameters"]])).ok()?.len() > 16 * 1024
    {
        return None;
    }
    Some((task_type.to_owned(), policy.clone()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registration {
    model_id: String,
    transfer_id: String,
    budget: Option<RegistrationBudget>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationBudget {
    cpu_millicores: Option<i64>,
    memory_mib: Option<i64>,
    gpu_memory_mib: Option<i64>,
}

impl RegistrationBudget {
    fn apply(&self, resources: &mut Resources, target: &backend_deployments::ChannelTarget, calibrated: bool) -> Api<()> {
        let invalid_budget = || Error(StatusCode::CONFLICT, "BACKEND_MODEL_INVALID_BUDGET");
        for (specified, effective, minimum, maximum) in [
            (self.cpu_millicores, &mut resources.cpu_millicores, 1, target.cpu_millicores.map(i64::from)),
            (self.memory_mib, &mut resources.memory_mib, 1, target.memory_mib.map(i64::from)),
            (self.gpu_memory_mib, &mut resources.gpu_memory_mib, 0, (!target.nvidia).then_some(0)),
        ] {
            if let Some(value) = specified {
                if value < minimum || maximum.is_some_and(|max| value > max) || (calibrated && value < *effective) {
                    return Err(invalid_budget());
                }
                *effective = value;
            }
        }
        Ok(())
    }
}

fn unavailable_target() -> Error {
    Error(StatusCode::CONFLICT, "BACKEND_EXECUTOR_TARGET_NOT_READY")
}

fn model_capability<'a>(capabilities: &'a Value, model_id: &str, legacy_studio: bool) -> Option<(&'a str, &'a Value)> {
    if capabilities["protocol_version"] != 1 {
        return None;
    }
    let mut models = capabilities["async_models"].as_array()?.iter().filter(|row| row["id"] == model_id);
    let model = models.next()?;
    let task_type = model["task_type"].as_str()?;
    if models.next().is_some() || !backend_transfers::wire::identifier(task_type) || task_type == "model.infer"
        || (legacy_studio && !matches!(task_type, "image.generate" | "video.generate"))
    {
        return None;
    }
    Some((task_type, model))
}

fn direct_policy(
    target: &backend_deployments::ChannelTarget, model: &Value, capacity: &Resources, baseline: &Resources,
) -> Api<Value> {
    let limits = match &model["model_limits"] {
        Value::Null => json!({}),
        Value::Object(_) => model["model_limits"].clone(),
        _ => return Err(invalid()),
    };
    if serde_json::to_vec(&limits).map_err(|_| invalid())?.len() > 16 * 1024 {
        return Err(invalid());
    }
    let resources = Resources {
        cpu_millicores: target.cpu_millicores.map(i64::from)
            .unwrap_or_else(|| 4000.min(capacity.cpu_millicores.saturating_sub(baseline.cpu_millicores))),
        memory_mib: target.memory_mib.map(i64::from)
            .unwrap_or_else(|| 4096.min(capacity.memory_mib.saturating_sub(baseline.memory_mib))),
        gpu_slots: i64::from(target.nvidia),
        gpu_memory_mib: 0,
    };
    Ok(json!({"resources": resources, "model_limits": limits, "model_parameters": {"policy":"direct"}}))
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
    let ready: Option<(bool, String)> = sqlx::query_as(
        "SELECT d.state='running' AND d.last_error IS NULL, d.id FROM vrs_backend_deployments d
         JOIN vrs_backend_executor_bindings b ON b.deployment_id=d.id
         WHERE b.executor_id=$1 FOR SHARE OF d",
    ).bind(&executor.id).fetch_optional(&mut **tx).await.map_err(unavailable)?;
    let Some((true, deployment_id)) = ready else {
        return Err(Error(StatusCode::CONFLICT, "BACKEND_STATE_CONFLICT"));
    };
    let admissible: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM vrs_backend_executor_bindings b
         WHERE b.executor_id=$1 AND b.deployment_id=$2
         AND NOT EXISTS(SELECT 1 FROM vrs_backend_model_deletions m
                        WHERE m.deployment_id=b.deployment_id AND m.model_id=b.model_id))",
    ).bind(&executor.id).bind(deployment_id).fetch_one(&mut **tx).await.map_err(unavailable)?;
    if !admissible { return Err(Error(StatusCode::CONFLICT, "BACKEND_MODEL_DELETING")); }
    Ok(())
}

// A deployment exposes multiple models through one worker. Wait for stop
// confirmation before giving it a second attempt, even if the node has spare resources.
pub(super) async fn worker_busy(tx: &mut Transaction<'_, Postgres>, executor: &Executor) -> Api<bool> {
    if !executor.id.starts_with(EXECUTOR_PREFIX) {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM vrs_backend_executor_bindings selected
          JOIN vrs_backend_executor_bindings active USING (deployment_id)
          JOIN vrs_tasks t ON t.body->'executor'->>'id'=active.executor_id AND t.node=$2
          JOIN vrs_reservations r ON r.task=t.id AND r.state<>'released'
          WHERE selected.executor_id=$1)",
    ).bind(&executor.id).bind(&executor.node_id).fetch_one(&mut **tx).await.map_err(unavailable)
}

pub(super) async fn ensure_model_not_deleting(
    tx: &mut Transaction<'_, Postgres>, deployment: &str, model: &str,
) -> Api<()> {
    if model_deleting(tx, deployment, model).await? {
        return Err(Error(StatusCode::CONFLICT, "BACKEND_MODEL_DELETING"));
    }
    Ok(())
}

async fn model_deleting(tx: &mut Transaction<'_, Postgres>, deployment: &str, model: &str) -> Api<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM vrs_backend_model_deletions WHERE deployment_id=$1 AND model_id=$2)",
    ).bind(deployment).bind(model).fetch_one(&mut **tx).await.map_err(unavailable)
}

async fn lock_running(tx: &mut Transaction<'_, Postgres>, deployment: &str) -> Api<()> {
    let running: Option<bool> = sqlx::query_scalar(
        "SELECT state='running' AND last_error IS NULL FROM vrs_backend_deployments WHERE id=$1 FOR UPDATE",
    ).bind(deployment).fetch_optional(&mut **tx).await.map_err(unavailable)?;
    if running != Some(true) { return Err(Error(StatusCode::CONFLICT, "BACKEND_STATE_CONFLICT")); }
    Ok(())
}

async fn verify_transfer(app: &App, headers: &HeaderMap, deployment: &str, request: &Registration) -> Api<()> {
    let reply = backend_deployments::exchange(app, headers, deployment,
        backend_transfers::wire::Operation::ReadTransfer { transfer_id: request.transfer_id.clone() }, &[]).await?.0;
    if reply["data"]["model_id"] != request.model_id || reply["data"]["verified"] != true {
        return Err(Error(StatusCode::CONFLICT, "BACKEND_MODEL_NOT_VERIFIED"));
    }
    Ok(())
}

pub(super) async fn remove(
    State(app): State<App>, headers: HeaderMap, Path((deployment, model)): Path<(String, String)>,
) -> Api<Json<Value>> {
    if !backend_transfers::wire::model_identifier(&model) { return Err(invalid()); }
    backend_deployments::channel_target(&app, &headers, &deployment).await?;
    let principal = auth(&app, &headers).await?;
    let capabilities = backend_deployments::exchange(&app, &headers, &deployment,
        backend_transfers::wire::Operation::Capabilities, &[]).await?.0;
    if capabilities["data"]["weight_model_delete"] != true {
        return Err(Error(StatusCode::CONFLICT, "BACKEND_MODEL_DELETE_UNSUPPORTED"));
    }
    let mut tx = app.db.begin().await.map_err(unavailable)?;
    lock_running(&mut tx, &deployment).await?;
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM vrs_backend_executor_bindings b JOIN vrs_tasks t
         ON t.body->'executor'->>'id'=b.executor_id WHERE b.deployment_id=$1 AND b.model_id=$2
         AND t.status IN ('queued','retry_wait','assigned','running','cancelling','recovering'))",
    ).bind(&deployment).bind(&model).fetch_one(&mut *tx).await.map_err(unavailable)?;
    if active { return Err(Error(StatusCode::CONFLICT, "BACKEND_TASKS_ACTIVE")); }
    sqlx::query("INSERT INTO vrs_backend_model_deletions(deployment_id,model_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(&deployment).bind(&model).execute(&mut *tx).await.map_err(unavailable)?;
    tx.commit().await.map_err(unavailable)?;

    // The deletion marker continues to block admission if the worker or the controller is interrupted.
    let mut tx = app.db.begin().await.map_err(unavailable)?;
    lock_running(&mut tx, &deployment).await?;
    if !model_deleting(&mut tx, &deployment, &model).await? {
        return Ok(ok(json!({"deleted_transfers":0})));
    }
    let reply = backend_deployments::exchange(&app, &headers, &deployment,
        backend_transfers::wire::Operation::DeleteModel { model_id: model.clone() }, &[]).await?.0;
    let deleted = reply["data"]["deleted_transfers"].as_u64()
        .ok_or(Error(StatusCode::BAD_GATEWAY, "BACKEND_CHANNEL_UNAVAILABLE"))?;
    let executor: Option<String> = sqlx::query_scalar(
        "DELETE FROM vrs_backend_executor_bindings WHERE deployment_id=$1 AND model_id=$2 RETURNING executor_id",
    ).bind(&deployment).bind(&model).fetch_optional(&mut *tx).await.map_err(unavailable)?;
    if let Some(executor) = executor {
        sqlx::query("DELETE FROM vrs_executor_settings WHERE id=$1")
            .bind(&executor).execute(&mut *tx).await.map_err(unavailable)?;
    }
    sqlx::query("DELETE FROM vrs_backend_model_deletions WHERE deployment_id=$1 AND model_id=$2")
        .bind(&deployment).bind(&model).execute(&mut *tx).await.map_err(unavailable)?;
    management::audit(&mut tx, &principal, "backend_model.delete", &deployment).await?;
    tx.commit().await.map_err(unavailable)?;
    management::restore(&app.db, &app.voice_config).await
        .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "BACKEND_EXECUTOR_UNAVAILABLE"))?;
    Ok(ok(json!({"deleted_transfers":deleted})))
}

pub(super) async fn list(
    State(app): State<App>,
    headers: HeaderMap,
    Path(deployment): Path<String>,
) -> Api<Json<Value>> {
    backend_deployments::channel_target(&app, &headers, &deployment).await?;
    let items: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT b.model_id,b.executor_id,e.node_id,b.profile,e.definition->>'task_type'
         FROM vrs_backend_executor_bindings b
         JOIN vrs_executor_settings e ON e.id=b.executor_id AND e.state='enabled'
         JOIN vrs_backend_deployments d ON d.id=b.deployment_id
         JOIN vrs_nodes n ON n.id=e.node_id
         WHERE b.deployment_id=$1 AND d.state='running' AND d.last_error IS NULL
         AND n.management_enabled AND NOT n.draining AND n.heartbeat>now()-interval '12 seconds'
         AND NOT EXISTS(SELECT 1 FROM vrs_backend_model_deletions m
             WHERE m.deployment_id=b.deployment_id AND m.model_id=b.model_id)
         ORDER BY b.model_id",
    ).bind(&deployment).fetch_all(&app.db).await.map_err(unavailable)?;
    let items: Vec<_> = items.into_iter().map(|(model_id, executor_id, node_id, profile, task_type)| {
        json!({ "deployment_id": deployment, "model_id": model_id, "executor_id": executor_id,
            "node_id": node_id, "profile": profile, "task_type": task_type })
    }).collect();
    Ok(ok(json!({"items": items})))
}

pub(super) async fn register(
    State(app): State<App>,
    headers: HeaderMap,
    Path(deployment_id): Path<String>,
    Json(request): Json<Registration>,
) -> Api<Json<Value>> {
    if !backend_transfers::wire::model_identifier(&request.model_id)
        || !backend_transfers::wire::identifier(&request.transfer_id) {
        return Err(invalid());
    }
    let target = backend_deployments::channel_target(&app, &headers, &deployment_id).await?;
    let authn = auth(&app, &headers).await?;
    verify_transfer(&app, &headers, &deployment_id, &request).await?;
    let capability = backend_deployments::exchange(&app, &headers, &deployment_id,
        backend_transfers::wire::Operation::Capabilities, &[]).await?.0;
    let mut candidates = app.config.nodes.iter().filter(|node| match target.kind.as_str() {
        "cluster" => node.vos_member_id == target.id,
        "ssh" => node.discovery_source == "modelinfer" && node.discovery_id == target.id,
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
    let capabilities = &capability["data"];
    let (task_type, model) = model_capability(capabilities, &request.model_id, target.legacy_studio)
        .ok_or(Error(StatusCode::CONFLICT, "BACKEND_MODEL_NOT_ADVERTISED"))?;
    let capacity: Resources = serde_json::from_value(sample.get("capacities")).map_err(|_| unavailable_target())?;
    let baseline: Resources = serde_json::from_value(sample.get("resident_baseline")).map_err(|_| unavailable_target())?;
    let measured = approvals().ok().and_then(|policies| {
        remote_approval(&target, architecture, &report, capabilities, &request.model_id, &policies)
    });
    let calibrated = measured.is_some();
    let mut approval = if let Some((_, approval)) = measured { approval }
        else { direct_policy(&target, model, &capacity, &baseline)? };
    let invalid_budget = || Error(StatusCode::CONFLICT, "BACKEND_MODEL_EXCEEDS_NODE_BUDGET");
    let mut resources: Resources = serde_json::from_value(approval["resources"].clone()).map_err(|_| invalid_budget())?;
    if let Some(budget) = &request.budget {
        budget.apply(&mut resources, &target, calibrated)?;
        approval["resources"] = json!(resources);
    }
    if !resources.valid() || resources.cpu_millicores <= 0 || resources.memory_mib <= 0
        || !resources.fits(&baseline, &capacity) {
        return Err(invalid_budget());
    }

    let digest = format!("{:x}", Sha256::digest(
        serde_json::to_vec(&json!([deployment_id, node.id, request.model_id, approval]))
            .map_err(|_| invalid())?,
    ));
    let prefix = if target.legacy_studio { LEGACY_PREFIX } else { EXECUTOR_PREFIX };
    let executor_id = format!("{prefix}{}", &digest[..24]);
    let executor = Executor {
        id: executor_id.clone(),
        contract_version: "1".into(),
        task_type: task_type.to_owned(),
        node_id: node.id.clone(),
        app_id: target.app_id.clone(),
        resources: resources.clone(),
        upstream_urls: vec![],
        placements: vec![],
        retry_mode: "none".into(),
        max_attempts: 1,
    };
    // Serialize registration with stop/remove and concurrent requests for the same deployment.
    lock_running(&mut tx, &deployment_id).await?;
    ensure_model_not_deleting(&mut tx, &deployment_id, &request.model_id).await?;
    verify_transfer(&app, &headers, &deployment_id, &request).await?;
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
        .bind(&target.profile).bind(&approval["model_limits"]).bind(&approval["model_parameters"]).execute(&mut *tx).await.map_err(unavailable)?;
    management::audit(&mut tx, &authn, "backend_model.bind", &executor_id).await?;
    tx.commit().await.map_err(unavailable)?;
    management::restore(&app.db, &app.voice_config).await
        .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "BACKEND_EXECUTOR_UNAVAILABLE"))?;
    Ok(ok(json!({
        "executor_id": executor_id, "node_id": node.id, "profile": target.profile,
        "model_limits": approval["model_limits"], "model_parameters": approval["model_parameters"],
        "resources": resources
    })))
}

#[derive(sqlx::FromRow)]
struct Attempt {
    task_id: String,
    app_id: String,
    model_id: String,
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
        "SELECT t.id AS task_id,t.app AS app_id,b.model_id,b.executor_id,b.deployment_id,b.transfer_id,t.node AS node_id,n.session,
         t.fence,t.status,(r.state IN ('held','active') AND r.lease_until>now()) AS lease_active,
         t.body->'parameters' AS parameters,(t.body ? '_scheduler_preemption') AS preempted,
         b.model_limits,b.model_parameters FROM vrs_tasks t
         JOIN vrs_backend_executor_bindings b ON b.executor_id=t.body->'executor'->>'id'
         JOIN vrs_nodes n ON n.id=t.node
         JOIN vrs_backend_deployments d ON d.id=b.deployment_id AND d.app_id=t.app
         LEFT JOIN vrs_reservations r ON r.attempt=t.attempt
         WHERE t.attempt=$1",
    )
    .bind(id).fetch_optional(db).await.map_err(unavailable)?
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
    let target = backend_deployments::channel_target(&app, &headers, &id).await?;
    if !target.legacy_studio {
        return Err(backend_deployments::denied());
    }
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
    backend_deployments::channel_target(app,headers,id).await?;
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
    if !record.executor_id.starts_with(LEGACY_PREFIX) {
        let mut body = payload(&record);
        if matches!(action, backend_transfers::wire::Action::Start) {
            body["model_id"] = json!(record.model_id);
            body["transfer_id"] = json!(record.transfer_id);
            body["model_limits"] = record.model_limits.clone();
            body["model_parameters"] = record.model_parameters.clone();
        }
        let bytes = serde_json::to_vec(&body).map_err(|_| invalid())?;
        return backend_deployments::dispatch_channel(
            &app, &record.deployment_id, &record.app_id,
            backend_transfers::wire::Operation::Execute { attempt_id, action }, &bytes,
        ).await;
    }
    if record.app_id != LEGACY_STUDIO {
        return Err(backend_deployments::denied());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_approval_matches_the_application_runtime_and_measured_target() {
        let mut target = backend_deployments::ChannelTarget {
            app_id: "com.example.speech".into(),
            profile: "jetson-orin".into(),
            image: "registry.example.com/example/speech:v1".into(),
            kind: "ssh".into(),
            id: "example-orin".into(),
            legacy_studio: false,
            cpu_millicores: None,
            memory_mib: None,
            nvidia: true,
        };
        let report = json!({});
        let capabilities = json!({
            "protocol_version": 1,
            "async_models": [{"id":"speech-model","task_type":"speech.render"}],
            "execution_settings": {"provider":"cuda","threads":4}
        });
        let mut policy = json!({
            "app_id":target.app_id, "image":target.image, "profile":target.profile,
            "architecture":"aarch64", "hardware":app_bindings::hardware(&report),
            "model_id":"speech-model", "task_type":"speech.render",
            "execution_settings":capabilities["execution_settings"],
            "evidence":{"id":"sample-calibration","confirmed_stopped":true},
            "model_limits":{}, "model_parameters":{}
        });
        let approval = |target: &backend_deployments::ChannelTarget, policy: &Value| remote_approval(target, "aarch64", &report,
            &capabilities, "speech-model", &json!({"protocol_version":1,"application_backends":[policy]}));
        assert_eq!(approval(&target, &policy).unwrap().0, "speech.render");
        target.app_id = "com.example.other".into();
        assert!(approval(&target, &policy).is_none());
        policy["app_id"] = json!(target.app_id);
        assert!(approval(&target, &policy).is_some());
        policy["execution_settings"]["provider"] = json!("cpu");
        assert!(approval(&target, &policy).is_none());
    }

    #[test]
    fn direct_policy_uses_node_budget_and_unique_verified_worker_capability() {
        let target = backend_deployments::ChannelTarget {
            app_id: "com.example.speech".into(),
            profile: "dgx-spark".into(),
            image: "registry.example.com/example/speech:v1".into(),
            kind: "ssh".into(),
            id: "example-spark".into(),
            legacy_studio: false,
            cpu_millicores: Some(2000),
            memory_mib: None,
            nvidia: true,
        };
        let row = json!({"id":"speech-model","task_type":"voice.transcribe","model_limits":{"duration_seconds":20}});
        let capabilities = json!({"protocol_version":1,"async_models":[row.clone()]});
        let (task_type, model) = model_capability(&capabilities, "speech-model", false).unwrap();
        assert_eq!(task_type, "voice.transcribe");
        assert!(model_capability(&capabilities, "missing", false).is_none());
        assert!(model_capability(&json!({"protocol_version":1,"async_models":[row.clone(),row]}), "speech-model", false).is_none());
        let policy = direct_policy(
            &target, model,
            &Resources { cpu_millicores: 8000, memory_mib: 4096, gpu_slots: 1, ..Resources::default() },
            &Resources { memory_mib: 1024, ..Resources::default() },
        ).ok().unwrap();
        assert_eq!(policy["resources"]["cpu_millicores"], 2000);
        assert_eq!(policy["resources"]["memory_mib"], 3072);
        assert_eq!(policy["resources"]["gpu_slots"], 1);
        assert_eq!(policy["model_limits"]["duration_seconds"], 20);
        assert_eq!(policy["model_parameters"], json!({"policy":"direct"}));

        let mut reservation: Resources = serde_json::from_value(policy["resources"].clone()).unwrap();
        let parse = |budget| serde_json::from_value::<RegistrationBudget>(budget).unwrap();
        assert!(parse(json!({"memory_mib":3500,"gpu_memory_mib":1024}))
            .apply(&mut reservation, &target, false).is_ok());
        assert_eq!((reservation.cpu_millicores, reservation.memory_mib, reservation.gpu_slots, reservation.gpu_memory_mib),
            (2000, 3500, 1, 1024));
        assert!(parse(json!({"cpu_millicores":2001})).apply(&mut reservation, &target, false).is_err());
        assert!(parse(json!({"memory_mib":0})).apply(&mut reservation, &target, false).is_err());
        assert!(parse(json!({"gpu_memory_mib":512})).apply(&mut reservation, &target, true).is_err());
        assert!(serde_json::from_value::<RegistrationBudget>(json!({"gpu_slots":0})).is_err());
    }

    #[tokio::test]
    #[ignore = "requires SCHEDULER_TEST_DATABASE_URL pointing at disposable PostgreSQL"]
    async fn another_model_waits_for_the_same_worker_to_stop() {
        let url = std::env::var("SCHEDULER_TEST_DATABASE_URL").unwrap();
        let db = sqlx::PgPool::connect(&url).await.unwrap();
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(
            "CREATE TEMP TABLE vrs_backend_executor_bindings(executor_id text, deployment_id text);
             CREATE TEMP TABLE vrs_tasks(id text, node text, body jsonb);
             CREATE TEMP TABLE vrs_reservations(task text, state text);",
        ).execute(&mut *tx).await.unwrap();
        let mut executor: Executor = serde_json::from_value(json!({
            "id":format!("{EXECUTOR_PREFIX}second"), "contract_version":"1", "task_type":"voice.transcribe",
            "node_id":"selected-node","app_id":"com.example.speech","resources":{}
        })).unwrap();
        sqlx::query("INSERT INTO vrs_backend_executor_bindings VALUES ($1,'same-worker'),($2,'same-worker'),($3,'different-worker')")
            .bind(format!("{EXECUTOR_PREFIX}first")).bind(&executor.id).bind(format!("{EXECUTOR_PREFIX}third"))
            .execute(&mut *tx).await.unwrap();
        assert_eq!(worker_busy(&mut tx, &executor).await.ok(), Some(false));
        sqlx::query("INSERT INTO vrs_tasks VALUES ('active','selected-node',$1)")
            .bind(json!({"executor":{"id":format!("{EXECUTOR_PREFIX}first")}}))
            .execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO vrs_reservations VALUES ('active','suspect')")
            .execute(&mut *tx).await.unwrap();
        assert_eq!(worker_busy(&mut tx, &executor).await.ok(), Some(true));
        let chosen = std::mem::replace(&mut executor.id, format!("{EXECUTOR_PREFIX}third"));
        assert_eq!(worker_busy(&mut tx, &executor).await.ok(), Some(false));
        executor.id = chosen;
        sqlx::query("UPDATE vrs_reservations SET state='released'")
            .execute(&mut *tx).await.unwrap();
        assert_eq!(worker_busy(&mut tx, &executor).await.ok(), Some(false));
    }
}
