//! One opt-in app lifecycle contract over existing node and execution ledgers.
//! No client supplied URL is called and registration never grants a budget.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registration {
    protocol_version: u32,
    revision: i64,
    executors: Vec<String>,
    stop_protocol: String,
}
fn owned(p: &Principal, e: &Executor) -> bool {
    p.role == "submitter" && (e.app_id == p.app_id || p.permit_executors.contains(&e.id))
}
fn definitions(a: &App, p: &Principal, ids: &[String]) -> Api<Value> {
    if ids.is_empty() || ids.len() > 64 {
        return Err(invalid());
    }
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(invalid());
        }
        let e = a
            .config
            .executors
            .iter()
            .find(|e| &e.id == id && owned(p, e))
            .ok_or(Error(StatusCode::FORBIDDEN, "EXECUTOR_NOT_ALLOWED"))?;
        result.push(json!(e));
    }
    result.sort_by_key(|e| e["id"].as_str().unwrap_or_default().to_owned());
    Ok(json!(result))
}
pub(super) async fn register(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<Registration>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    if p.role != "submitter" {
        return Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    if b.protocol_version != 1 || b.revision < 0 || b.stop_protocol != "permit-native-exit-v1" {
        return Err(invalid());
    }
    let defs = definitions(&a, &p, &b.executors)?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    management::principal_active(&mut tx, &p).await?;
    // Serialize first registrations as well as updates for this app identity.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&p.app_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    for value in defs.as_array().unwrap() {
        let id = value["id"].as_str().ok_or_else(invalid)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,1))")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_integrations i,jsonb_array_elements(i.manifest->'executors') x WHERE i.app_id<>$1 AND x->>'id'=$2)").bind(&p.app_id).bind(id).fetch_one(&mut *tx).await.map_err(unavailable)?;
        if exists {
            return Err(Error(StatusCode::CONFLICT, "EXECUTOR_ALREADY_REGISTERED"));
        }
        let e: Executor = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
        management::admit(&mut tx, &e, &p).await?;
    }
    let previous =
        sqlx::query("SELECT revision,manifest FROM vrs_integrations WHERE app_id=$1 FOR UPDATE")
            .bind(&p.app_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
    let current = previous
        .as_ref()
        .map(|r| r.get::<i64, _>("revision"))
        .unwrap_or(0);
    if b.revision != current {
        return Err(Error(StatusCode::CONFLICT, "INTEGRATION_REVISION_CONFLICT"));
    }
    let manifest = json!({"protocol_version":1,"executors":defs,"stop_protocol":b.stop_protocol});
    let changed = previous
        .as_ref()
        .is_none_or(|r| r.get::<Value, _>("manifest") != manifest);
    if changed && current > 0 {
        let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_stop_requests WHERE app_id=$1 AND confirmed_at IS NULL) OR EXISTS(SELECT 1 FROM vrs_permits p,vrs_integrations i,jsonb_array_elements(i.manifest->'executors') x WHERE i.app_id=$1 AND x->>'id'=p.executor AND p.state<>'released') OR EXISTS(SELECT 1 FROM vrs_tasks t,vrs_integrations i,jsonb_array_elements(i.manifest->'executors') x WHERE i.app_id=$1 AND x->>'id'=t.body->'executor'->>'id' AND t.status NOT IN ('succeeded','failed','cancelled','deleted'))").bind(&p.app_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
        if busy {
            return Err(Error(StatusCode::CONFLICT, "INTEGRATION_STOP_PENDING"));
        }
    }
    let revision = current + i64::from(changed);
    sqlx::query("INSERT INTO vrs_integrations(app_id,manifest,revision) VALUES($1,$2,$3) ON CONFLICT(app_id) DO UPDATE SET manifest=$2,revision=$3,heartbeat=now()")
        .bind(&p.app_id).bind(manifest).bind(revision).execute(&mut *tx).await.map_err(unavailable)?;
    management::audit(&mut tx, &p, "integration.register", &p.app_id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"app_id":p.app_id,"revision":revision,"heartbeat_after_seconds":5,"stale_after_seconds":20,"budget_granted":false}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Heartbeat {
    revision: i64,
}
pub(super) async fn heartbeat(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<Heartbeat>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    if p.role != "submitter" {
        return Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    management::principal_active(&mut tx, &p).await?;
    let row =
        sqlx::query("SELECT manifest,revision FROM vrs_integrations WHERE app_id=$1 FOR UPDATE")
            .bind(&p.app_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .ok_or(Error(StatusCode::NOT_FOUND, "INTEGRATION_NOT_REGISTERED"))?;
    if row.get::<i64, _>("revision") != b.revision {
        return Err(Error(StatusCode::CONFLICT, "INTEGRATION_REVISION_CONFLICT"));
    }
    let manifest: Value = row.get("manifest");
    let ids: Vec<String> = manifest["executors"]
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .filter_map(|e| e["id"].as_str().map(str::to_owned))
        .collect();
    let current = definitions(&a, &p, &ids)?;
    if current != manifest["executors"] {
        return Err(Error(StatusCode::CONFLICT, "EXECUTOR_APPROVAL_CHANGED"));
    }
    for e in current.as_array().unwrap() {
        management::admit(
            &mut tx,
            &serde_json::from_value(e.clone()).map_err(|_| invalid())?,
            &p,
        )
        .await?;
    }
    sqlx::query("UPDATE vrs_integrations SET heartbeat=now() WHERE app_id=$1")
        .bind(&p.app_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    let rows=sqlx::query("SELECT s.permit_id,s.nonce,p.executor,p.node,p.metadata FROM vrs_stop_requests s JOIN vrs_permits p ON p.id=s.permit_id WHERE s.app_id=$1 AND s.confirmed_at IS NULL AND p.state<>'released' ORDER BY s.requested_at LIMIT 64")
        .bind(&p.app_id).fetch_all(&mut *tx).await.map_err(unavailable)?;
    let commands:Vec<Value>=rows.iter().map(|r|{let m:Value=r.get("metadata");json!({"permit_id":r.get::<String,_>("permit_id"),"nonce":r.get::<String,_>("nonce"),"executor_id":r.get::<String,_>("executor"),"node_id":r.get::<String,_>("node"),"model_id":m["model_id"],"execution_id":m["execution_id"],"action":"cancel-and-unload"})}).collect();
    let outstanding=sqlx::query("SELECT p.id,p.executor,p.state,p.metadata FROM vrs_permits p WHERE p.state<>'released' AND EXISTS(SELECT 1 FROM jsonb_array_elements($1::jsonb) x WHERE x->>'id'=p.executor) ORDER BY p.created_at LIMIT 257")
        .bind(&current).fetch_all(&mut *tx).await.map_err(unavailable)?;
    let executions:Vec<Value>=outstanding.iter().take(256).map(|r|{let m:Value=r.get("metadata");json!({"permit_id":r.get::<String,_>("id"),"executor_id":r.get::<String,_>("executor"),"state":r.get::<String,_>("state"),"execution_id":m["execution_id"]})}).collect();
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"revision":b.revision,"heartbeat_after_seconds":5,"stop_requests":commands,"executions":executions,"complete_execution_snapshot":outstanding.len()<=256}),
    ))
}
// Only opted-in executors gain this liveness requirement. Legacy contracts stay valid.
pub(super) async fn admit(tx: &mut Transaction<'_, Postgres>, e: &Executor) -> Api<()> {
    let rows=sqlx::query("SELECT manifest,heartbeat>now()-interval '20 seconds' AS fresh FROM vrs_integrations i WHERE EXISTS(SELECT 1 FROM jsonb_array_elements(i.manifest->'executors') x WHERE x->>'id'=$1) FOR SHARE OF i")
        .bind(&e.id).fetch_all(&mut **tx).await.map_err(unavailable)?;
    let blocked = rows.iter().any(|r| {
        let manifest: Value = r.get("manifest");
        !r.get::<bool, _>("fresh")
            || !manifest["executors"]
                .as_array()
                .is_some_and(|items| items.contains(&json!(e)))
    });
    if blocked {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "APPLICATION_HEARTBEAT_STALE",
        ));
    }
    Ok(())
}
pub(super) async fn stop_pending(tx: &mut Transaction<'_, Postgres>, id: &str) -> Api<()> {
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_stop_requests WHERE permit_id=$1 AND confirmed_at IS NULL)").bind(id).fetch_one(&mut **tx).await.map_err(unavailable)?;
    if pending {
        return Err(Error(StatusCode::CONFLICT, "STOP_REQUESTED"));
    }
    Ok(())
}
pub(super) async fn request_stop(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let row = sqlx::query("SELECT app,executor,state FROM vrs_permits WHERE id=$1 FOR UPDATE")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        .ok_or(Error(StatusCode::NOT_FOUND, "NOT_FOUND"))?;
    if !visible(&p, &row.get::<String, _>("app")) {
        return Err(Error(StatusCode::NOT_FOUND, "NOT_FOUND"));
    }
    if row.get::<String, _>("state") == "released" {
        return Ok(ok(json!({"permit_id":id,"state":"released"})));
    }
    let executor: String = row.get("executor");
    let receivers:Vec<String>=sqlx::query_scalar("SELECT app_id FROM vrs_integrations i WHERE EXISTS(SELECT 1 FROM jsonb_array_elements(i.manifest->'executors') x WHERE x->>'id'=$1) ORDER BY app_id").bind(&executor).fetch_all(&mut *tx).await.map_err(unavailable)?;
    // Never deliver an execution stop command to an ambiguous receiver.
    if receivers.len() != 1 {
        return Err(Error(StatusCode::CONFLICT, "STOP_ADAPTER_NOT_REGISTERED"));
    }
    sqlx::query("INSERT INTO vrs_stop_requests(permit_id,app_id,nonce) VALUES($1,$2,$3) ON CONFLICT(permit_id) DO NOTHING")
        .bind(&id).bind(&receivers[0]).bind(Uuid::new_v4().to_string()).execute(&mut *tx).await.map_err(unavailable)?;
    management::audit(&mut tx, &p, "integration.stop-request", &id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(
        json!({"permit_id":id,"state":"stop_requested","resources_released":false}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stopped {
    nonce: String,
    execution_id: Option<String>,
    execution_token: String,
    native_exit_barrier: bool,
    model_unloaded: bool,
    evidence: String,
}
pub(super) async fn stopped(
    State(a): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<Stopped>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    if p.role != "submitter"
        || !b.native_exit_barrier
        || !b.model_unloaded
        || b.evidence.trim().is_empty()
        || b.evidence.len() > 512
        || !(32..=128).contains(&b.execution_token.len())
        || b.execution_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 128)
    {
        return Err(invalid());
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    // Same lock order as request_stop. No HTTP success or expiry alone releases resources.
    let permit = sqlx::query("SELECT executor,metadata FROM vrs_permits WHERE id=$1 FOR UPDATE")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        .ok_or(Error(StatusCode::NOT_FOUND, "NOT_FOUND"))?;
    let request =
        sqlx::query("SELECT app_id,nonce FROM vrs_stop_requests WHERE permit_id=$1 FOR UPDATE")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .ok_or(Error(StatusCode::NOT_FOUND, "NOT_FOUND"))?;
    let eid: String = permit.get("executor");
    a.config
        .executors
        .iter()
        .find(|e| e.id == eid && owned(&p, e))
        .ok_or(Error(StatusCode::FORBIDDEN, "EXECUTOR_NOT_ALLOWED"))?;
    let m: Value = permit.get("metadata");
    if request.get::<String, _>("app_id") != p.app_id
        || request.get::<String, _>("nonce") != b.nonce
        || m["execution_id"] != json!(b.execution_id)
        || m["execution_token_hash"]
            != format!("{:x}", Sha256::digest(b.execution_token.as_bytes()))
    {
        return Err(Error(StatusCode::FORBIDDEN, "STOP_PROOF_MISMATCH"));
    }
    management::principal_active(&mut tx, &p).await?;
    sqlx::query("UPDATE vrs_permits SET state='released',evidence=$2,released_at=COALESCE(released_at,now()) WHERE id=$1").bind(&id).bind(&b.evidence).execute(&mut *tx).await.map_err(unavailable)?;
    sqlx::query(
        "UPDATE vrs_stop_requests SET confirmed_at=COALESCE(confirmed_at,now()) WHERE permit_id=$1",
    )
    .bind(&id)
    .execute(&mut *tx)
    .await
    .map_err(unavailable)?;
    management::audit(&mut tx, &p, "integration.stop-confirmed", &id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(json!({"permit_id":id,"state":"released"})))
}
pub(super) async fn list(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    let rows=sqlx::query("SELECT app_id,revision,manifest,heartbeat,heartbeat>now()-interval '20 seconds' AS fresh FROM vrs_integrations WHERE $1 OR app_id=$2 ORDER BY app_id").bind(p.role=="admin").bind(&p.app_id).fetch_all(&a.db).await.map_err(unavailable)?;
    let mut items = Vec::new();
    for row in rows {
        let manifest: Value = row.get("manifest");
        let mut executors = Vec::new();
        for e in manifest["executors"].as_array().into_iter().flatten() {
            let node=sqlx::query("SELECT heartbeat,heartbeat>now()-interval '12 seconds' AS fresh,draining,management_enabled,capacities,resident_baseline,safety_reserve FROM vrs_nodes WHERE id=$1").bind(e["node_id"].as_str()).fetch_optional(&a.db).await.map_err(unavailable)?;
            let node=node.map(|n|json!({"heartbeat":n.get::<Option<DateTime<Utc>>,_>("heartbeat"),"fresh":n.get::<Option<bool>,_>("fresh").unwrap_or(false),"draining":n.get::<bool,_>("draining"),"management_enabled":n.get::<bool,_>("management_enabled"),"capacities":n.get::<Value,_>("capacities"),"resident_baseline":n.get::<Value,_>("resident_baseline"),"safety_reserve":n.get::<Value,_>("safety_reserve")}));
            let current = a
                .config
                .executors
                .iter()
                .find(|x| Some(x.id.as_str()) == e["id"].as_str());
            executors.push(json!({"definition":e,"approval_current":current.is_some_and(|x|json!(x)==*e),"node":node}));
        }
        let application: String = row.get("app_id");
        let running=sqlx::query("SELECT p.id,p.executor,p.metadata,p.state,s.requested_at FROM vrs_permits p LEFT JOIN vrs_stop_requests s ON s.permit_id=p.id AND s.confirmed_at IS NULL WHERE p.state<>'released' AND ($1 OR p.app=$2) AND EXISTS(SELECT 1 FROM vrs_integrations i,jsonb_array_elements(i.manifest->'executors') x WHERE i.app_id=$3 AND x->>'id'=p.executor) ORDER BY p.created_at LIMIT 64")
            .bind(p.role=="admin").bind(&p.app_id).bind(&application).fetch_all(&a.db).await.map_err(unavailable)?;
        let executions:Vec<Value>=running.iter().map(|r|{let m:Value=r.get("metadata");json!({"permit_id":r.get::<String,_>("id"),"executor_id":r.get::<String,_>("executor"),"model_id":m["model_id"],"state":r.get::<String,_>("state"),"stop_requested":r.get::<Option<DateTime<Utc>>,_>("requested_at").is_some()})}).collect();
        let task_rows=sqlx::query("SELECT id,app,status,body FROM vrs_tasks WHERE status IN ('queued','assigned','running','cancelling','recovering') AND EXISTS(SELECT 1 FROM jsonb_array_elements($1::jsonb) x WHERE x->>'id'=body->'executor'->>'id') AND ($2 OR app=$3) ORDER BY created_at LIMIT 64")
            .bind(&manifest["executors"]).bind(p.role=="admin").bind(&p.app_id).fetch_all(&a.db).await.map_err(unavailable)?;
        let tasks:Vec<Value>=task_rows.iter().map(|r|{let body:Value=r.get("body");json!({"task_id":r.get::<String,_>("id"),"status":r.get::<String,_>("status"),"executor_id":body["executor"]["id"]})}).collect();
        items.push(json!({"app_id":row.get::<String,_>("app_id"),"revision":row.get::<i64,_>("revision"),"heartbeat":row.get::<DateTime<Utc>,_>("heartbeat"),"fresh":row.get::<bool,_>("fresh"),"executors":executors,"stop_protocol":manifest["stop_protocol"],"executions":executions,"tasks":tasks}));
    }
    Ok(ok(
        json!({"items":items,"node_heartbeat_seconds":2,"node_stale_seconds":12,"application_heartbeat_seconds":5,"application_stale_seconds":20}),
    ))
}

/// Retiring the opt-in contract never abandons a live execution or stop command.
pub(super) async fn unregister(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    if p.role != "submitter" {
        return Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    management::principal_active(&mut tx, &p).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&p.app_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    sqlx::query("SELECT app_id FROM vrs_integrations WHERE app_id=$1 FOR UPDATE")
        .bind(&p.app_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_stop_requests WHERE app_id=$1 AND confirmed_at IS NULL) OR EXISTS(SELECT 1 FROM vrs_permits p,vrs_integrations i,jsonb_array_elements(i.manifest->'executors') x WHERE i.app_id=$1 AND x->>'id'=p.executor AND p.state<>'released') OR EXISTS(SELECT 1 FROM vrs_tasks t,vrs_integrations i,jsonb_array_elements(i.manifest->'executors') x WHERE i.app_id=$1 AND x->>'id'=t.body->'executor'->>'id' AND t.status NOT IN ('succeeded','failed','cancelled','deleted'))")
        .bind(&p.app_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
    if busy {
        return Err(Error(StatusCode::CONFLICT, "INTEGRATION_STOP_PENDING"));
    }
    sqlx::query("DELETE FROM vrs_integrations WHERE app_id=$1")
        .bind(&p.app_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    management::audit(&mut tx, &p, "integration.unregister", &p.app_id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(json!({"app_id":p.app_id,"unregistered":true})))
}
