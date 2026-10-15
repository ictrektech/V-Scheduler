//! Dynamic enrollment and operator-approved resources. No remote shell execution.
use super::*;
use axum::http::{HeaderValue, header};
use serde::Serialize;

fn conflict(code: &'static str) -> Error {
    Error(StatusCode::CONFLICT, code)
}
fn denied() -> Error {
    Error(StatusCode::FORBIDDEN, "FORBIDDEN")
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}
fn label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
fn secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn bearer(h: &HeaderMap) -> Api<&str> {
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| (32..=256).contains(&v.len()))
        .ok_or(Error(StatusCode::UNAUTHORIZED, "UNAUTHENTICATED"))
}
fn secret_ok(value: Value) -> Response {
    let mut response = ok(value).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
pub(super) async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    p: &Principal,
    action: &str,
    resource: &str,
) -> Api<()> {
    sqlx::query("INSERT INTO vrs_management_events(actor,action,resource) VALUES($1,$2,$3)")
        .bind(&p.app_id)
        .bind(action)
        .bind(resource)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    Ok(())
}
async fn idle_node(tx: &mut Transaction<'_, Postgres>, node: &str) -> Api<()> {
    sqlx::query("SELECT id FROM vrs_nodes WHERE id=$1 FOR UPDATE")
        .bind(node)
        .fetch_optional(&mut **tx)
        .await
        .map_err(unavailable)?;
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_reservations WHERE node=$1 AND state<>'released') OR EXISTS(SELECT 1 FROM vrs_permits WHERE node=$1 AND state<>'released') OR EXISTS(SELECT 1 FROM vrs_permit_placements x JOIN vrs_permits p ON p.id=x.permit_id WHERE x.node_id=$1 AND p.state<>'released')")
        .bind(node).fetch_one(&mut **tx).await.map_err(unavailable)?;
    if busy {
        return Err(conflict("NODE_HAS_UNSTOPPED_EXECUTION"));
    }
    Ok(())
}
async fn idle_executor(tx: &mut Transaction<'_, Postgres>, id: &str) -> Api<()> {
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_tasks WHERE body->'executor'->>'id'=$1 AND status NOT IN ('succeeded','failed','cancelled')) OR EXISTS(SELECT 1 FROM vrs_permits WHERE executor=$1 AND state<>'released')")
        .bind(id).fetch_one(&mut **tx).await.map_err(unavailable)?;
    if busy {
        return Err(conflict("EXECUTOR_HAS_UNFINISHED_WORK"));
    }
    Ok(())
}

pub(super) async fn restore(db: &PgPool, runtime: &crate::voice::Runtime) -> anyhow::Result<()> {
    let mut managed = ManagedConfig::default();
    for row in sqlx::query("SELECT definition FROM vrs_node_settings ORDER BY node_id")
        .fetch_all(db)
        .await?
    {
        managed
            .nodes
            .push(serde_json::from_value(row.get("definition"))?);
    }
    for row in sqlx::query("SELECT id,definition,state FROM vrs_executor_settings ORDER BY id")
        .fetch_all(db)
        .await?
    {
        if row.get::<String, _>("state") == "enabled" {
            managed
                .executors
                .push(serde_json::from_value(row.get("definition"))?);
        } else {
            managed.disabled_executors.push(row.get("id"));
        }
    }
    for n in &managed.nodes {
        sqlx::query(
            "UPDATE vrs_nodes SET capacities=$2,resident_baseline=$3,safety_reserve=$4 WHERE id=$1",
        )
        .bind(&n.id)
        .bind(json!(n.capacities))
        .bind(json!(n.resident_baseline))
        .bind(json!(n.safety_reserve))
        .execute(db)
        .await?;
    }
    runtime.set_management(managed)?;
    Ok(())
}
async fn publish(a: &App) -> Api<()> {
    restore(&a.db, &a.voice_config).await.map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "MANAGEMENT_CONFIGURATION_UNAVAILABLE",
        )
    })
}

pub(super) async fn authenticate(a: &App, token: &str) -> Api<Option<Principal>> {
    let digest = hash(token);
    if let Some(row)=sqlx::query("SELECT r.node_id,r.state,n.management_enabled FROM vrs_worker_registrations r LEFT JOIN vrs_nodes n ON n.id=r.node_id WHERE r.token_hash=$1")
        .bind(&digest).fetch_optional(&a.db).await.map_err(unavailable)? {
        if row.get::<String,_>("state")!="approved" || row.get::<Option<bool>,_>("management_enabled")!=Some(true) {
            return Err(Error(StatusCode::UNAUTHORIZED,"WORKER_NOT_APPROVED"));
        }
        return Ok(Some(Principal{token:token.into(),role:"worker".into(),app_id:"registered-agent".into(),node_id:row.get("node_id"),delegated_executors:vec![],permit_executors:vec![]}));
    }
    if let Some(row) = sqlx::query("SELECT * FROM vrs_service_identities WHERE token_hash=$1")
        .bind(digest)
        .fetch_optional(&a.db)
        .await
        .map_err(unavailable)?
    {
        if row.get::<Option<DateTime<Utc>>, _>("revoked_at").is_some() {
            return Err(Error(StatusCode::UNAUTHORIZED, "CREDENTIAL_REVOKED"));
        }
        return Ok(Some(Principal {
            token: token.into(),
            role: "submitter".into(),
            app_id: row.get("app_id"),
            node_id: String::new(),
            delegated_executors: serde_json::from_value(row.get("delegated_executors"))
                .map_err(|_| invalid())?,
            permit_executors: serde_json::from_value(row.get("permit_executors"))
                .map_err(|_| invalid())?,
        }));
    }
    Ok(None)
}
// Called while the execution node is locked: a stale in-flight config snapshot
// must not admit work after an administrator changes/revokes its approval.
pub(super) async fn admit(
    tx: &mut Transaction<'_, Postgres>,
    e: &Executor,
    p: &Principal,
) -> Api<()> {
    if let Some(row) = sqlx::query("SELECT state,definition FROM vrs_executor_settings WHERE id=$1")
        .bind(&e.id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(unavailable)?
    {
        if row.get::<String, _>("state") != "enabled"
            || row.get::<Value, _>("definition") != json!(e)
        {
            return Err(conflict("EXECUTOR_APPROVAL_CHANGED"));
        }
    }
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT management_enabled FROM vrs_nodes WHERE id=$1")
            .bind(&e.node_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(unavailable)?;
    if enabled != Some(true) {
        return Err(denied());
    }
    for placement in &e.placements {
        let enabled: Option<bool> =
            sqlx::query_scalar("SELECT management_enabled FROM vrs_nodes WHERE id=$1")
                .bind(&placement.node_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(unavailable)?;
        if enabled != Some(true) {
            return Err(denied());
        }
    }
    principal_active(tx, p).await
}
pub(super) async fn principal_active(tx: &mut Transaction<'_, Postgres>, p: &Principal) -> Api<()> {
    if !p.token.is_empty() {
        let row = sqlx::query(
            "SELECT revoked_at FROM vrs_service_identities WHERE token_hash=$1 FOR SHARE",
        )
        .bind(hash(&p.token))
        .fetch_optional(&mut **tx)
        .await
        .map_err(unavailable)?;
        if row.is_some_and(|r| r.get::<Option<DateTime<Utc>>, _>("revoked_at").is_some()) {
            return Err(Error(StatusCode::UNAUTHORIZED, "CREDENTIAL_REVOKED"));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EnrollmentRequest {
    node_id: String,
    #[serde(default = "enrollment_ttl")]
    ttl_seconds: i64,
    #[serde(default)]
    discovery_source: String,
    #[serde(default)]
    discovery_id: String,
}
fn enrollment_ttl() -> i64 {
    900
}
pub(super) async fn enrollment(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<EnrollmentRequest>,
) -> Api<Response> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    if !identifier(&b.node_id) || !(60..=86400).contains(&b.ttl_seconds) {
        return Err(invalid());
    }
    if !b.discovery_source.is_empty() || !b.discovery_id.is_empty() {
        if b.discovery_source != "modelinfer" || !identifier(&b.discovery_id) {
            return Err(invalid());
        }
        let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_observations o,jsonb_array_elements(CASE WHEN jsonb_typeof(o.data)='array' THEN o.data ELSE '[]'::jsonb END) entry WHERE o.id='modelinfer:inventory' AND o.last_error IS NULL AND o.observed_at>now()-interval '90 seconds' AND entry->>'id'=$1)").bind(&b.discovery_id).fetch_one(&a.db).await.map_err(unavailable)?;
        if !found {
            return Err(conflict("DISCOVERY_SOURCE_NOT_FRESH"));
        }
        let bound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_worker_registrations r JOIN vrs_enrollments e ON e.id=r.enrollment_id WHERE e.discovery_source=$1 AND e.discovery_id=$2 AND r.node_id<>$3 AND r.state<>'revoked')").bind(&b.discovery_source).bind(&b.discovery_id).bind(&b.node_id).fetch_one(&a.db).await.map_err(unavailable)?;
        if bound {
            return Err(conflict("DISCOVERY_NODE_ALREADY_BOUND"));
        }
    }
    let _guard = a.voice_config.management_lock.lock().await;
    if a.voice_config.is_static_node(&b.node_id) {
        return Err(conflict("NODE_ID_ALREADY_EXISTS"));
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let state: Option<String> = sqlx::query_scalar(
        "SELECT state FROM vrs_worker_registrations WHERE node_id=$1 FOR UPDATE",
    )
    .bind(&b.node_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(unavailable)?;
    if state.is_some_and(|s| s != "revoked") {
        return Err(conflict("NODE_ID_ALREADY_EXISTS"));
    }
    idle_node(&mut tx, &b.node_id).await?;
    // Only the newest outstanding invitation for an ID may be consumed.
    sqlx::query("UPDATE vrs_enrollments SET revoked_at=now() WHERE node_id=$1 AND used_at IS NULL AND revoked_at IS NULL").bind(&b.node_id).execute(&mut *tx).await.map_err(unavailable)?;
    let token = secret();
    let id = Uuid::new_v4().to_string();
    let expires:DateTime<Utc>=sqlx::query_scalar("INSERT INTO vrs_enrollments(id,node_id,token_hash,expires_at,discovery_source,discovery_id) VALUES($1,$2,$3,now()+$4*interval '1 second',$5,$6) RETURNING expires_at")
        .bind(&id).bind(&b.node_id).bind(hash(&token)).bind(b.ttl_seconds).bind(&b.discovery_source).bind(&b.discovery_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
    audit(&mut tx, &p, "enrollment.created", &b.node_id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(secret_ok(
        json!({"id":id,"node_id":b.node_id,"enrollment_token":token,"expires_at":expires}),
    ))
}
pub(super) async fn revoke_enrollment(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let changed=sqlx::query("UPDATE vrs_enrollments SET revoked_at=now() WHERE id=$1 AND used_at IS NULL RETURNING node_id").bind(&id).fetch_optional(&mut *tx).await.map_err(unavailable)?;
    if changed.is_none() {
        return Err(conflict("ENROLLMENT_ALREADY_USED"));
    }
    audit(&mut tx, &p, "enrollment.revoked", &id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(json!({"revoked":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RegisterRequest {
    protocol_version: i64,
    node_id: String,
    display_name: String,
    instance_id: String,
    boot_id: String,
    session_id: String,
    worker_token: String,
}
pub(super) async fn register(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<RegisterRequest>,
) -> Api<Json<Value>> {
    let enrollment_token = bearer(&h)?;
    if b.protocol_version != 1
        || !identifier(&b.node_id)
        || !label(&b.display_name)
        || !identifier(&b.instance_id)
        || !identifier(&b.boot_id)
        || !identifier(&b.session_id)
        || !(32..=128).contains(&b.worker_token.len())
        || b.worker_token == enrollment_token
    {
        return Err(invalid());
    }
    let _guard = a.voice_config.management_lock.lock().await;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let ticket = sqlx::query(
        "SELECT *,expires_at>now() AS valid FROM vrs_enrollments WHERE token_hash=$1 FOR UPDATE",
    )
    .bind(hash(enrollment_token))
    .fetch_optional(&mut *tx)
    .await
    .map_err(unavailable)?
    .ok_or(Error(StatusCode::UNAUTHORIZED, "ENROLLMENT_INVALID"))?;
    if ticket.get::<String, _>("node_id") != b.node_id
        || ticket
            .get::<Option<DateTime<Utc>>, _>("revoked_at")
            .is_some()
    {
        return Err(Error(StatusCode::UNAUTHORIZED, "ENROLLMENT_INVALID"));
    }
    let old = sqlx::query("SELECT * FROM vrs_worker_registrations WHERE node_id=$1 FOR UPDATE")
        .bind(&b.node_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
    let ticket_id: String = ticket.get("id");
    if ticket.get::<Option<DateTime<Utc>>, _>("used_at").is_some() {
        if let Some(old) = old {
            if old.get::<String, _>("enrollment_id") == ticket_id
                && old.get::<String, _>("token_hash") == hash(&b.worker_token)
                && old.get::<String, _>("instance_id") == b.instance_id
                && old.get::<String, _>("session_id") == b.session_id
            {
                return Ok(ok(
                    json!({"node_id":b.node_id,"state":old.get::<String,_>("state"),"session_id":b.session_id}),
                ));
            }
        }
        return Err(conflict("ENROLLMENT_ALREADY_USED"));
    }
    if !ticket.get::<bool, _>("valid") {
        return Err(Error(StatusCode::UNAUTHORIZED, "ENROLLMENT_EXPIRED"));
    }
    if a.voice_config.is_static_node(&b.node_id)
        || old.is_some_and(|r| r.get::<String, _>("state") != "revoked")
    {
        return Err(conflict("NODE_ID_ALREADY_EXISTS"));
    }
    sqlx::query("INSERT INTO vrs_worker_registrations(node_id,enrollment_id,display_name,instance_id,boot_id,session_id,token_hash,state) VALUES($1,$2,$3,$4,$5,$6,$7,'pending') ON CONFLICT(node_id) DO UPDATE SET enrollment_id=excluded.enrollment_id,display_name=excluded.display_name,instance_id=excluded.instance_id,boot_id=excluded.boot_id,session_id=excluded.session_id,token_hash=excluded.token_hash,state='pending',version=vrs_worker_registrations.version+1,updated_at=now()")
        .bind(&b.node_id).bind(&ticket_id).bind(b.display_name.trim()).bind(&b.instance_id).bind(&b.boot_id).bind(&b.session_id).bind(hash(&b.worker_token)).execute(&mut *tx).await.map_err(unavailable)?;
    sqlx::query("UPDATE vrs_enrollments SET used_at=now() WHERE id=$1")
        .bind(ticket_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    let actor = Principal {
        token: String::new(),
        role: "worker".into(),
        app_id: "enrolling-agent".into(),
        node_id: b.node_id.clone(),
        delegated_executors: vec![],
        permit_executors: vec![],
    };
    audit(&mut tx, &actor, "worker.pending", &b.node_id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"node_id":b.node_id,"state":"pending","session_id":b.session_id}),
    ))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NodeEdit {
    version: i64,
    display_name: String,
    capacities: Resources,
    #[serde(default)]
    resident_baseline: Resources,
    #[serde(default)]
    safety_reserve: Resources,
    #[serde(default)]
    vos_member_id: String,
}
fn node_definition(current: &Config, id: &str, b: &NodeEdit) -> Api<Node> {
    if !identifier(id)
        || !label(&b.display_name)
        || !b.capacities.valid()
        || b.capacities.cpu_millicores < 1
        || b.capacities.memory_mib < 1
        || (b.capacities.gpu_slots == 0 && b.capacities.gpu_memory_mib != 0)
        || !b
            .resident_baseline
            .fits(&Resources::default(), &b.capacities)
        || !b.safety_reserve.valid()
        || (!b.vos_member_id.is_empty() && !identifier(&b.vos_member_id))
    {
        return Err(invalid());
    }
    if current.executors.iter().any(|e| {
        e.allocations().iter().any(|part| {
            part.node_id == id && !part.resources.fits(&b.resident_baseline, &b.capacities)
        })
    }) {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "BUDGET_BELOW_APPROVED_EXECUTOR",
        ));
    }
    Ok(Node {
        id: id.into(),
        capacities: b.capacities.clone(),
        resident_baseline: b.resident_baseline.clone(),
        safety_reserve: b.safety_reserve.clone(),
        vos_member_id: b.vos_member_id.clone(),
        vos: None,
        discovery_source: current
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.discovery_source.clone())
            .unwrap_or_default(),
        discovery_id: current
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.discovery_id.clone())
            .unwrap_or_default(),
    })
}
async fn save_node(
    a: App,
    h: HeaderMap,
    id: String,
    b: NodeEdit,
    approve: bool,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let _guard = a.voice_config.management_lock.lock().await;
    let current = a.voice_config.snapshot();
    let mut definition = node_definition(&current, &id, &b)?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    idle_node(&mut tx, &id).await?;
    let registration = sqlx::query(
        "SELECT r.state,r.version,r.session_id,e.discovery_source,e.discovery_id FROM vrs_worker_registrations r JOIN vrs_enrollments e ON e.id=r.enrollment_id WHERE r.node_id=$1 FOR UPDATE OF r",
    )
    .bind(&id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(unavailable)?;
    if approve {
        let r = registration.as_ref().ok_or(conflict("NODE_NOT_PENDING"))?;
        definition.discovery_source = r.get("discovery_source");
        definition.discovery_id = r.get("discovery_id");
        if !definition.discovery_source.is_empty() {
            let bound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_node_settings WHERE node_id<>$1 AND definition->>'discovery_source'=$2 AND definition->>'discovery_id'=$3)").bind(&id).bind(&definition.discovery_source).bind(&definition.discovery_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
            if bound {
                return Err(conflict("DISCOVERY_NODE_ALREADY_BOUND"));
            }
        }
        if r.get::<String, _>("state") != "pending" || r.get::<i64, _>("version") != b.version {
            return Err(conflict("MANAGEMENT_VERSION_CONFLICT"));
        }
    } else {
        if !current.nodes.iter().any(|n| n.id == id) {
            return Err(Error(StatusCode::NOT_FOUND, "NODE_NOT_FOUND"));
        }
        if registration
            .as_ref()
            .is_some_and(|r| r.get::<String, _>("state") != "approved")
        {
            return Err(conflict("NODE_NOT_APPROVED"));
        }
        let version: Option<i64> =
            sqlx::query_scalar("SELECT version FROM vrs_node_settings WHERE node_id=$1 FOR UPDATE")
                .bind(&id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if version.unwrap_or(0) != b.version {
            return Err(conflict("MANAGEMENT_VERSION_CONFLICT"));
        }
    }
    sqlx::query("INSERT INTO vrs_nodes(id,capacities,resident_baseline,safety_reserve) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO UPDATE SET capacities=excluded.capacities,resident_baseline=excluded.resident_baseline,safety_reserve=excluded.safety_reserve")
        .bind(&id).bind(json!(definition.capacities)).bind(json!(definition.resident_baseline)).bind(json!(definition.safety_reserve)).execute(&mut *tx).await.map_err(unavailable)?;
    let version:i64=sqlx::query_scalar("INSERT INTO vrs_node_settings(node_id,display_name,definition) VALUES($1,$2,$3) ON CONFLICT(node_id) DO UPDATE SET display_name=excluded.display_name,definition=excluded.definition,version=vrs_node_settings.version+1,updated_at=now() RETURNING version")
        .bind(&id).bind(b.display_name.trim()).bind(json!(definition)).fetch_one(&mut *tx).await.map_err(unavailable)?;
    if approve {
        let session: String = registration.as_ref().unwrap().get("session_id");
        sqlx::query("UPDATE vrs_worker_registrations SET state='approved',version=version+1,updated_at=now() WHERE node_id=$1").bind(&id).execute(&mut *tx).await.map_err(unavailable)?;
        sqlx::query("UPDATE vrs_nodes SET session=$2,heartbeat=NULL,report='{}',management_enabled=true,draining=false WHERE id=$1").bind(&id).bind(session).execute(&mut *tx).await.map_err(unavailable)?;
    }
    audit(
        &mut tx,
        &p,
        if approve {
            "node.approved"
        } else {
            "node.budget.updated"
        },
        &id,
    )
    .await?;
    tx.commit().await.map_err(unavailable)?;
    publish(&a).await?;
    Ok(ok(json!({"node_id":id,"version":version})))
}
pub(super) async fn approve_node(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<NodeEdit>,
) -> Api<Json<Value>> {
    save_node(a, h, id, b, true).await
}
pub(super) async fn update_node(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<NodeEdit>,
) -> Api<Json<Value>> {
    save_node(a, h, id, b, false).await
}
pub(super) async fn revoke_node(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let _guard = a.voice_config.management_lock.lock().await;
    if a.voice_config.is_static_node(&id) {
        return Err(conflict("CONFIGURED_NODE_CREDENTIAL_IS_PROTECTED"));
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    idle_node(&mut tx, &id).await?;
    let changed=sqlx::query("UPDATE vrs_worker_registrations SET state='revoked',version=version+1,updated_at=now() WHERE node_id=$1 RETURNING node_id").bind(&id).fetch_optional(&mut *tx).await.map_err(unavailable)?;
    if changed.is_none() {
        return Err(Error(StatusCode::NOT_FOUND, "NODE_NOT_FOUND"));
    }
    sqlx::query("UPDATE vrs_nodes SET management_enabled=false,draining=true WHERE id=$1")
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    sqlx::query("UPDATE vrs_executor_settings SET state='disabled',version=version+1,updated_at=now() WHERE node_id=$1").bind(&id).execute(&mut *tx).await.map_err(unavailable)?;
    audit(&mut tx, &p, "node.revoked", &id).await?;
    tx.commit().await.map_err(unavailable)?;
    publish(&a).await?;
    Ok(ok(json!({"node_id":id,"state":"revoked"})))
}
fn executor_valid(e: &Executor) -> bool {
    e.valid_placements()
        && e.placements.iter().all(|p| identifier(&p.node_id))
        && identifier(&e.id)
        && identifier(&e.node_id)
        && identifier(&e.app_id)
        && identifier(&e.contract_version)
        && identifier(&e.task_type)
        && !e.id.starts_with("v-voice.auto.")
        && !e.id.starts_with("v-scheduler.voice.")
        && e.resources.valid()
        && e.resources.cpu_millicores > 0
        && e.resources.memory_mib > 0
        && (e.resources.gpu_memory_mib == 0 || e.resources.gpu_slots > 0)
        && ["", "none", "idempotent"].contains(&e.retry_mode.as_str())
        && (1..=10).contains(&e.max_attempts)
        && e.upstream_urls.len() <= 32
        && e.upstream_urls.iter().all(|v| {
            v.len() <= 2048
                && reqwest::Url::parse(v).is_ok_and(|u| {
                    ["http", "https"].contains(&u.scheme())
                        && u.host_str().is_some()
                        && u.username().is_empty()
                        && u.password().is_none()
                        && u.query().is_none()
                        && u.fragment().is_none()
                })
        })
        && (e.task_type != "model.infer" || !e.upstream_urls.is_empty())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutorProposal {
    executors: Vec<Executor>,
}
pub(super) async fn propose_executors(
    State(a): State<App>,
    h: HeaderMap,
    Path(node): Path<String>,
    Json(b): Json<ExecutorProposal>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    worker(&p, &node)?;
    if b.executors.is_empty()
        || b.executors.len() > 64
        || b.executors
            .iter()
            .any(|e| e.node_id != node || !executor_valid(e))
    {
        return Err(invalid());
    }
    let _guard = a.voice_config.management_lock.lock().await;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    sqlx::query("SELECT id FROM vrs_nodes WHERE id=$1 FOR UPDATE")
        .bind(&node)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
    let enabled: bool = sqlx::query_scalar("SELECT management_enabled FROM vrs_nodes WHERE id=$1")
        .bind(&node)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
    if !enabled {
        return Err(denied());
    }
    for e in &b.executors {
        if a.voice_config.is_static_executor(&e.id)
            || a.voice_config
                .snapshot()
                .executors
                .iter()
                .any(|old| old.id == e.id && old.node_id != node)
        {
            return Err(conflict("EXECUTOR_ID_ALREADY_EXISTS"));
        }
        let existing = sqlx::query(
            "SELECT node_id,definition FROM vrs_executor_settings WHERE id=$1 FOR UPDATE",
        )
        .bind(&e.id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if let Some(old) = existing {
            if old.get::<String, _>("node_id") != node
                || old.get::<Value, _>("definition") != json!(e)
            {
                return Err(conflict("EXECUTOR_DEFINITION_CHANGED"));
            }
            continue;
        }
        sqlx::query("INSERT INTO vrs_executor_settings(id,node_id,definition,state) VALUES($1,$2,$3,'pending')").bind(&e.id).bind(&node).bind(json!(e)).execute(&mut *tx).await.map_err(unavailable)?;
        audit(&mut tx, &p, "executor.pending", &e.id).await?;
    }
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(json!({"accepted":true,"approval_required":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutorEdit {
    version: i64,
    enabled: bool,
    definition: Executor,
}
pub(super) async fn update_executor(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<ExecutorEdit>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    if b.definition.id != id || !executor_valid(&b.definition) {
        return Err(invalid());
    }
    let _guard = a.voice_config.management_lock.lock().await;
    let current = a.voice_config.snapshot();
    let e = &b.definition;
    if !e.allocations().iter().all(|part| {
        current.nodes.iter().any(|node| {
            node.id == part.node_id
                && part
                    .resources
                    .fits(&node.resident_baseline, &node.capacities)
        })
    }) {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "EXECUTOR_EXCEEDS_NODE_BUDGET",
        ));
    }
    if current.executors.iter().any(|old| {
        old.id == id
            && (old.node_id != e.node_id
                || old.app_id != e.app_id
                || old.task_type != e.task_type
                || old.contract_version != e.contract_version)
    }) {
        return Err(conflict("EXECUTOR_IDENTITY_IS_IMMUTABLE"));
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    for part in e.allocations() {
        idle_node(&mut tx, &part.node_id).await?;
    }
    idle_executor(&mut tx, &id).await?;
    for part in e.allocations() {
        let enabled: bool =
            sqlx::query_scalar("SELECT management_enabled FROM vrs_nodes WHERE id=$1")
                .bind(&part.node_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(unavailable)?;
        if !enabled {
            return Err(denied());
        }
    }
    let old = sqlx::query(
        "SELECT node_id,version,definition,state FROM vrs_executor_settings WHERE id=$1 FOR UPDATE",
    )
    .bind(&id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(unavailable)?;
    if old
        .as_ref()
        .is_some_and(|r| r.get::<String, _>("node_id") != e.node_id)
    {
        return Err(conflict("EXECUTOR_NODE_IS_IMMUTABLE"));
    }
    if let Some(row) = &old {
        let definition: Value = row.get("definition");
        if row.get::<String, _>("state") != "pending"
            && (definition["app_id"] != e.app_id
                || definition["task_type"] != e.task_type
                || definition["contract_version"] != e.contract_version)
        {
            return Err(conflict("EXECUTOR_IDENTITY_IS_IMMUTABLE"));
        }
    }
    if old
        .as_ref()
        .map(|r| r.get::<i64, _>("version"))
        .unwrap_or(0)
        != b.version
    {
        return Err(conflict("MANAGEMENT_VERSION_CONFLICT"));
    }
    let version:i64=sqlx::query_scalar("INSERT INTO vrs_executor_settings(id,node_id,definition,state) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO UPDATE SET definition=excluded.definition,state=excluded.state,version=vrs_executor_settings.version+1,updated_at=now() RETURNING version")
        .bind(&id).bind(&e.node_id).bind(json!(e)).bind(if b.enabled{"enabled"}else{"disabled"}).fetch_one(&mut *tx).await.map_err(unavailable)?;
    audit(
        &mut tx,
        &p,
        if b.enabled {
            "executor.enabled"
        } else {
            "executor.disabled"
        },
        &id,
    )
    .await?;
    tx.commit().await.map_err(unavailable)?;
    publish(&a).await?;
    Ok(ok(
        json!({"id":id,"version":version,"state":if b.enabled{"enabled"}else{"disabled"}}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IdentityRequest {
    app_id: String,
    #[serde(default)]
    permit_executors: Vec<String>,
    #[serde(default)]
    delegated_executors: Vec<String>,
}
pub(super) async fn create_identity(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<IdentityRequest>,
) -> Api<Response> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    if !identifier(&b.app_id) || b.permit_executors.len() > 64 || b.delegated_executors.len() > 64 {
        return Err(invalid());
    }
    let current = a.voice_config.snapshot();
    if b.permit_executors
        .iter()
        .chain(&b.delegated_executors)
        .any(|id| !current.executors.iter().any(|e| e.id == *id))
    {
        return Err(invalid());
    }
    let id = Uuid::new_v4().to_string();
    let token = secret();
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    sqlx::query("INSERT INTO vrs_service_identities(id,app_id,token_hash,permit_executors,delegated_executors) VALUES($1,$2,$3,$4,$5)").bind(&id).bind(&b.app_id).bind(hash(&token)).bind(json!(b.permit_executors)).bind(json!(b.delegated_executors)).execute(&mut *tx).await.map_err(unavailable)?;
    audit(&mut tx, &p, "identity.created", &b.app_id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(secret_ok(json!({"id":id,"app_id":b.app_id,"token":token})))
}
pub(super) async fn revoke_identity(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let app: String =
        sqlx::query_scalar("SELECT app_id FROM vrs_service_identities WHERE id=$1 FOR UPDATE")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .ok_or(Error(StatusCode::NOT_FOUND, "IDENTITY_NOT_FOUND"))?;
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_tasks WHERE app=$1 AND status NOT IN ('succeeded','failed','cancelled')) OR EXISTS(SELECT 1 FROM vrs_permits WHERE app=$1 AND state<>'released')").bind(&app).fetch_one(&mut *tx).await.map_err(unavailable)?;
    if busy {
        return Err(conflict("IDENTITY_HAS_UNFINISHED_WORK"));
    }
    sqlx::query(
        "UPDATE vrs_service_identities SET revoked_at=COALESCE(revoked_at,now()) WHERE id=$1",
    )
    .bind(&id)
    .execute(&mut *tx)
    .await
    .map_err(unavailable)?;
    audit(&mut tx, &p, "identity.revoked", &app).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(json!({"revoked":true})))
}

pub(super) async fn overview(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    require(&auth(&a, &h).await?, "admin")?;
    let current = a.voice_config.snapshot();
    let settings = sqlx::query("SELECT node_id,display_name,version FROM vrs_node_settings")
        .fetch_all(&a.db)
        .await
        .map_err(unavailable)?;
    let registrations=sqlx::query("SELECT r.node_id,r.display_name,r.state,r.version,r.created_at,e.discovery_source,e.discovery_id FROM vrs_worker_registrations r JOIN vrs_enrollments e ON e.id=r.enrollment_id ORDER BY r.created_at").fetch_all(&a.db).await.map_err(unavailable)?;
    let mut nodes = Vec::new();
    for n in &current.nodes {
        let reg = registrations
            .iter()
            .find(|r| r.get::<String, _>("node_id") == n.id);
        let setting = settings
            .iter()
            .find(|r| r.get::<String, _>("node_id") == n.id);
        nodes.push(json!({"id":n.id,"display_name":setting.map(|r|r.get::<String,_>("display_name")).unwrap_or_default(),"definition":{"capacities":n.capacities,"resident_baseline":n.resident_baseline,"safety_reserve":n.safety_reserve,"vos_member_id":n.vos_member_id},"version":setting.map(|r|r.get::<i64,_>("version")).unwrap_or(0),"state":reg.map(|r|r.get::<String,_>("state")).unwrap_or_else(||"configured".into()),"source":if reg.is_some(){"registered"}else{"configuration"},"can_revoke":reg.is_some()}));
    }
    for r in &registrations {
        let id: String = r.get("node_id");
        if !nodes.iter().any(|n| n["id"] == id) || r.get::<String, _>("state") == "pending" {
            nodes.retain(|n| n["id"] != id);
            nodes.push(json!({"id":id,"display_name":r.get::<String,_>("display_name"),"state":r.get::<String,_>("state"),"version":r.get::<i64,_>("version"),"source":"registered","discovery_source":r.get::<String,_>("discovery_source"),"discovery_id":r.get::<String,_>("discovery_id"),"can_revoke":true,"definition":null}));
        }
    }
    let mut executors = Vec::new();
    let es =
        sqlx::query("SELECT id,definition,state,version FROM vrs_executor_settings ORDER BY id")
            .fetch_all(&a.db)
            .await
            .map_err(unavailable)?;
    for e in &current.executors {
        if !es.iter().any(|r| r.get::<String, _>("id") == e.id) {
            executors.push(json!({"id":e.id,"definition":e,"state":"enabled","version":0,"source":"configuration","editable":!e.id.starts_with("v-voice.auto.")&&!e.id.starts_with("v-scheduler.voice.")}));
        }
    }
    for r in es {
        executors.push(json!({"id":r.get::<String,_>("id"),"definition":r.get::<Value,_>("definition"),"state":r.get::<String,_>("state"),"version":r.get::<i64,_>("version"),"source":"management","editable":true}));
    }
    let invites=sqlx::query("SELECT id,node_id,expires_at,used_at,revoked_at FROM vrs_enrollments ORDER BY created_at DESC LIMIT 100").fetch_all(&a.db).await.map_err(unavailable)?;
    let identities=sqlx::query("SELECT id,app_id,permit_executors,delegated_executors,revoked_at,created_at FROM vrs_service_identities ORDER BY created_at DESC LIMIT 100").fetch_all(&a.db).await.map_err(unavailable)?;
    let discovery: Option<Value> = sqlx::query_scalar(
        "SELECT data FROM vrs_observations WHERE id='vos:platform-inventory-status'",
    )
    .fetch_optional(&a.db)
    .await
    .map_err(unavailable)?;
    let modelinfer: Option<Value> =
        sqlx::query_scalar("SELECT data FROM vrs_observations WHERE id='modelinfer:status'")
            .fetch_optional(&a.db)
            .await
            .map_err(unavailable)?;
    let events=sqlx::query("SELECT cursor,action,resource,created_at FROM vrs_management_events ORDER BY cursor DESC LIMIT 100").fetch_all(&a.db).await.map_err(unavailable)?;
    Ok(ok(
        json!({"modelinfer_discovery":modelinfer,"cluster_discovery":discovery,"nodes":nodes,"executors":executors,"enrollments":invites.iter().map(|r|json!({"id":r.get::<String,_>("id"),"node_id":r.get::<String,_>("node_id"),"expires_at":r.get::<DateTime<Utc>,_>("expires_at"),"used_at":r.get::<Option<DateTime<Utc>>,_>("used_at"),"revoked_at":r.get::<Option<DateTime<Utc>>,_>("revoked_at")})).collect::<Vec<_>>(),"service_identities":identities.iter().map(|r|json!({"id":r.get::<String,_>("id"),"app_id":r.get::<String,_>("app_id"),"permit_executors":r.get::<Value,_>("permit_executors"),"delegated_executors":r.get::<Value,_>("delegated_executors"),"revoked_at":r.get::<Option<DateTime<Utc>>,_>("revoked_at"),"created_at":r.get::<DateTime<Utc>,_>("created_at")})).collect::<Vec<_>>(),"events":events.iter().map(|r|json!({"cursor":r.get::<i64,_>("cursor"),"action":r.get::<String,_>("action"),"resource":r.get::<String,_>("resource"),"created_at":r.get::<DateTime<Utc>,_>("created_at")})).collect::<Vec<_>>()}),
    ))
}

pub(super) async fn decorate(a: &App, views: &mut [Value]) -> Api<()> {
    let settings = sqlx::query("SELECT node_id,display_name,version FROM vrs_node_settings")
        .fetch_all(&a.db)
        .await
        .map_err(unavailable)?;
    for view in views {
        if let Some(r) = settings
            .iter()
            .find(|r| view["id"] == r.get::<String, _>("node_id"))
        {
            view["display_name"] = json!(r.get::<String, _>("display_name"));
            view["management_version"] = json!(r.get::<i64, _>("version"));
        }
    }
    Ok(())
}

/// VOS App-only management: record planning without creating a worker identity,
/// heartbeat, executable node or reservation entitlement.
pub(super) async fn source_settings(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<NodeEdit>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    if b.version < 0
        || !identifier(&id)
        || !label(&b.display_name)
        || !b.capacities.valid()
        || !b.resident_baseline.fits(&b.safety_reserve, &b.capacities)
        || !b.safety_reserve.valid()
    {
        return Err(invalid());
    }
    let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_observations o, jsonb_array_elements(o.data) n WHERE o.id='modelinfer:inventory' AND n->>'id'=$1)").bind(&id).fetch_one(&a.db).await.map_err(unavailable)?;
    if !found {
        return Err(Error(StatusCode::NOT_FOUND, "SOURCE_NODE_NOT_FOUND"));
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    // Optimistic insert/update atomically detects concurrent editors.
    let version: Option<i64> = if b.version == 0 {
        sqlx::query_scalar("INSERT INTO vrs_source_node_settings(source,source_id,display_name,definition) VALUES('modelinfer',$1,$2,$3) ON CONFLICT DO NOTHING RETURNING version").bind(&id).bind(&b.display_name).bind(json!({"capacities":b.capacities,"resident_baseline":b.resident_baseline,"safety_reserve":b.safety_reserve})).fetch_optional(&mut *tx).await.map_err(unavailable)?
    } else {
        sqlx::query_scalar("UPDATE vrs_source_node_settings SET display_name=$2,definition=$3,version=version+1,updated_at=now() WHERE source='modelinfer' AND source_id=$1 AND version=$4 RETURNING version").bind(&id).bind(&b.display_name).bind(json!({"capacities":b.capacities,"resident_baseline":b.resident_baseline,"safety_reserve":b.safety_reserve})).bind(b.version).fetch_optional(&mut *tx).await.map_err(unavailable)?
    };
    let version = version.ok_or(conflict("MANAGEMENT_VERSION_CONFLICT"))?;
    audit(&mut tx, &p, "source.plan.updated", &id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"version":version,"execution_enabled":false,"control_state":"pending_adapter_verification"}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceAction {
    action: String,
}
/// Reserved App-to-App control contract. Never run an unverified remote action.
pub(super) async fn source_control(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<SourceAction>,
) -> Api<Json<Value>> {
    require(&auth(&a, &h).await?, "admin")?;
    if !identifier(&id) || !["pause", "resume", "confirm-stopped"].contains(&b.action.as_str()) {
        return Err(invalid());
    }
    Err(Error(
        StatusCode::NOT_IMPLEMENTED,
        "VOS_NODE_CONTROL_PENDING_VERIFICATION",
    ))
}
