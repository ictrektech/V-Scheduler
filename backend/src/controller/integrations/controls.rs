//! Optional application-owned HTTP controls. Start once after admission;
//! persist uncertain outcomes and stop with the existing native-proof ledger.
use super::*;
use serde::Serialize;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Control {
    executor_id: String,
    #[serde(default)]
    start_url: Option<String>,
    stop_url: String,
}
fn permitted_url(executor: &Executor, value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    value.len() <= 2048
        && ["http", "https"].contains(&url.scheme())
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && executor
            .upstream_urls
            .iter()
            .any(|base| reqwest::Url::parse(base).is_ok_and(|base| base.origin() == url.origin()))
}
pub(super) fn validate(
    a: &App,
    p: &Principal,
    executors: &[String],
    controls: &[Control],
) -> Api<()> {
    if controls.len() > 64 {
        return Err(invalid());
    }
    let mut seen = std::collections::BTreeSet::new();
    for c in controls {
        let e = a
            .config
            .executors
            .iter()
            .find(|e| {
                e.id == c.executor_id
                    && (owned(p, e) || p.role == "admin" && e.app_id == p.app_id)
                    && executors.contains(&e.id)
            })
            .ok_or(Error(StatusCode::FORBIDDEN, "EXECUTOR_NOT_ALLOWED"))?;
        if (c.start_url.is_some() && e.app_id != p.app_id)
            || !seen.insert(&c.executor_id)
            || !permitted_url(e, &c.stop_url)
            || c.start_url.as_ref().is_some_and(|s| !permitted_url(e, s))
        {
            return Err(invalid());
        }
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Start {
    #[serde(default)]
    app_id: Option<String>,
    request: PermitRequest,
    input: Value,
    #[serde(default)]
    idempotency_key: Option<String>,
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}
async fn callback(client: &reqwest::Client, url: &str, body: &Value) -> Result<Value, ()> {
    let response = client
        .post(url)
        .header(
            "idempotency-key",
            body["command_id"].as_str().unwrap_or_default(),
        )
        .json(body)
        .send()
        .await
        .map_err(|_| ())?;
    if !response.status().is_success() || response.content_length().is_some_and(|v| v > 65536) {
        return Err(());
    }
    let mut response = response;
    let mut bytes = vec![];
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if bytes.len() + chunk.len() > 65536 {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    Ok(value.get("data").cloned().unwrap_or(value))
}
pub(in crate::controller) async fn start_execution(
    State(a): State<App>,
    mut h: HeaderMap,
    Json(b): Json<Start>,
) -> Api<Json<Value>> {
    if let Some(key) = &b.idempotency_key {
        if key.is_empty()
            || key.len() > 256
            || h.get("idempotency-key")
                .is_some_and(|v| v.to_str().ok() != Some(key.as_str()))
        {
            return Err(invalid());
        }
        h.insert("idempotency-key", key.parse().map_err(|_| invalid())?);
    }
    let actor = auth(&a, &h).await?;
    require(&actor, "submitter")?;
    if !(32..=128).contains(&b.request.execution_token.len())
        || serde_json::to_vec(&b.input).map_err(|_| invalid())?.len() > 262144
    {
        return Err(invalid());
    }
    let application = b.app_id.as_deref().unwrap_or(&actor.app_id);
    if actor.role != "admin" && application != actor.app_id {
        return Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    let registration=sqlx::query("SELECT manifest,heartbeat>now()-interval '20 seconds' AS fresh FROM vrs_integrations WHERE app_id=$1").bind(application).fetch_optional(&a.db).await.map_err(unavailable)?.ok_or(Error(StatusCode::NOT_FOUND,"INTEGRATION_NOT_REGISTERED"))?;
    if !registration.get::<bool, _>("fresh") {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "APPLICATION_HEARTBEAT_STALE",
        ));
    }
    let manifest: Value = registration.get("manifest");
    let controls: Vec<Control> = serde_json::from_value(
        manifest["controls"]
            .as_array()
            .cloned()
            .map(Value::Array)
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|_| invalid())?;
    let control = controls
        .iter()
        .find(|c| c.executor_id == b.request.executor_id && c.start_url.is_some())
        .ok_or(Error(StatusCode::CONFLICT, "START_ADAPTER_NOT_REGISTERED"))?;
    let mut owner = actor.clone();
    owner.app_id = application.to_owned();
    validate(
        &a,
        &owner,
        &[b.request.executor_id.clone()],
        std::slice::from_ref(control),
    )?;
    let input_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&b.input).map_err(|_| invalid())?)
    );
    let execution_token = b.request.execution_token.clone();
    let executor = b.request.executor_id.clone();
    let upstream = b.request.upstream_url.clone();
    let model = b.request.model_id.clone();
    let granted = acquire_for(&a, &owner, &h, b.request, true).await?;
    let permit = granted.0["data"]["permit_id"]
        .as_str()
        .ok_or_else(invalid)?
        .to_owned();
    // Publish a single start intent under the permit row lock before any HTTP.
    // Retrying an ambiguous POST never starts a second execution.
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    let row = sqlx::query(
        "SELECT metadata,state,lease_until>now() AS valid FROM vrs_permits WHERE id=$1 FOR UPDATE",
    )
    .bind(&permit)
    .fetch_one(&mut *tx)
    .await
    .map_err(unavailable)?;
    let mut metadata: Value = row.get("metadata");
    if row.get::<String, _>("state") != "active" || !row.get::<bool, _>("valid") {
        return Err(Error(StatusCode::CONFLICT, "PERMIT_NOT_ACTIVE"));
    }
    if metadata["managed_control"].is_object() {
        if metadata["managed_control"]["input_hash"] != input_hash {
            return Err(Error(StatusCode::CONFLICT, "IDEMPOTENCY_CONFLICT"));
        }
        return Ok(ok(
            json!({"permit_id":permit,"phase":metadata["managed_control"]["phase"],"replayed":false,"resources_released":false}),
        ));
    }
    if !metadata["execution_id"].is_null() {
        return Err(Error(StatusCode::CONFLICT, "PERMIT_ALREADY_CLAIMED"));
    }
    let execution = Uuid::new_v4().to_string();
    metadata["managed_control"] = json!({"phase":"starting","input_hash":input_hash,"planned_execution_id":execution,"started_at":Utc::now(),"control":control,"app_id":application});
    sqlx::query("UPDATE vrs_permits SET metadata=$2 WHERE id=$1")
        .bind(&permit)
        .bind(&metadata)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    management::audit(&mut tx, &actor, "integration.start-request", &permit).await?;
    tx.commit().await.map_err(unavailable)?;
    let body = json!({"protocol_version":1,"command_id":format!("start:{permit}"),"action":"start","permit_id":permit,"executor_id":executor,"model_id":model,"upstream_url":upstream,"execution_id":execution,"execution_token":execution_token,"input":b.input});
    let reply = callback(&client(), control.start_url.as_ref().unwrap(), &body).await;
    let accepted = reply.as_ref().is_ok_and(|r| {
        r["permit_id"] == permit && r["execution_id"] == execution && r["started"] == true
    });
    let claimed: bool = sqlx::query_scalar(
        "SELECT COALESCE(metadata->>'execution_id'=$2,false) FROM vrs_permits WHERE id=$1",
    )
    .bind(&permit)
    .bind(&execution)
    .fetch_one(&a.db)
    .await
    .map_err(unavailable)?;
    let phase = if accepted && claimed {
        "running"
    } else {
        "unknown"
    };
    sqlx::query("UPDATE vrs_permits SET metadata=jsonb_set(metadata,'{managed_control,phase}',to_jsonb($2::text)) WHERE id=$1 AND state='active'").bind(&permit).bind(phase).execute(&a.db).await.map_err(unavailable)?;
    if phase == "unknown" {
        stop_intent(&a, &permit, application).await?;
    }
    Ok(ok(
        json!({"permit_id":permit,"execution_id":execution,"phase":phase,"resources_released":false}),
    ))
}
async fn stop_intent(a: &App, permit: &str, application: &str) -> Api<()> {
    sqlx::query("INSERT INTO vrs_stop_requests(permit_id,app_id,nonce) VALUES($1,$2,$3) ON CONFLICT(permit_id) DO NOTHING").bind(permit).bind(application).bind(Uuid::new_v4().to_string()).execute(&a.db).await.map_err(unavailable)?;
    Ok(())
}
async fn tick(a: &App, client: &reqwest::Client) -> Api<()> {
    // A DB advisory lease prevents two controller processes dispatching the same
    // native command concurrently. No callback runs inside a node/permit lock.
    let mut connection = a.db.begin().await.map_err(unavailable)?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(746394281)")
        .fetch_one(&mut *connection)
        .await
        .map_err(unavailable)?;
    if !locked {
        return Ok(());
    }
    let result = reconcile(a, client).await;
    connection.commit().await.map_err(unavailable)?;
    result
}
async fn reconcile(a: &App, client: &reqwest::Client) -> Api<()> {
    // Stop-only registrations also cover execution started by the application
    // or gateway. A previously authorized stop intent chooses the receiver.
    sqlx::query("WITH pending AS (SELECT p.id,s.app_id,c AS control FROM vrs_permits p JOIN vrs_stop_requests s ON s.permit_id=p.id AND s.confirmed_at IS NULL JOIN vrs_integrations i ON i.app_id=s.app_id CROSS JOIN LATERAL jsonb_array_elements(COALESCE(i.manifest->'controls','[]'::jsonb)) c WHERE p.state<>'released' AND NOT p.metadata ? 'managed_control' AND c->>'executor_id'=p.executor) UPDATE vrs_permits p SET metadata=jsonb_set(p.metadata,'{managed_control}',jsonb_build_object('phase','stop_requested','control',pending.control,'app_id',pending.app_id,'planned_execution_id',p.metadata->'execution_id')) FROM pending WHERE p.id=pending.id").execute(&a.db).await.map_err(unavailable)?;
    // Fence stale/uncertain execution before renewal; do not revive an expired
    // lease. Controller restarts cannot silently replay an ambiguous start.
    sqlx::query("INSERT INTO vrs_stop_requests(permit_id,app_id,nonce) SELECT p.id,p.app,gen_random_uuid()::text FROM vrs_permits p JOIN vrs_integrations i ON i.app_id=p.metadata->'managed_control'->>'app_id' WHERE p.state<>'released' AND p.metadata ? 'managed_control' AND (i.heartbeat<now()-interval '20 seconds' OR p.lease_until<=now() OR p.state='suspect' OR p.metadata->'managed_control'->>'phase'='unknown' OR (p.metadata->'managed_control'->>'phase'='starting' AND COALESCE((p.metadata->'managed_control'->>'started_at')::timestamptz,p.created_at)<now()-interval '15 seconds') OR EXISTS(SELECT 1 FROM vrs_nodes n WHERE (n.id=p.node OR EXISTS(SELECT 1 FROM vrs_permit_placements x WHERE x.permit_id=p.id AND x.node_id=n.id)) AND (n.heartbeat IS NULL OR n.heartbeat<now()-interval '12 seconds'))) ON CONFLICT(permit_id) DO NOTHING").execute(&a.db).await.map_err(unavailable)?;
    sqlx::query("UPDATE vrs_permits p SET lease_until=now()+interval '30 seconds' FROM vrs_integrations i WHERE i.app_id=p.metadata->'managed_control'->>'app_id' AND p.metadata ? 'managed_control' AND p.state='active' AND p.lease_until>now() AND i.heartbeat>now()-interval '20 seconds' AND NOT EXISTS(SELECT 1 FROM vrs_stop_requests s WHERE s.permit_id=p.id AND s.confirmed_at IS NULL)").execute(&a.db).await.map_err(unavailable)?;
    let rows=sqlx::query("SELECT p.id,s.app_id AS app,p.executor,p.metadata,s.nonce FROM vrs_permits p JOIN vrs_integrations i ON i.app_id=p.metadata->'managed_control'->>'app_id' JOIN vrs_stop_requests s ON s.permit_id=p.id AND s.confirmed_at IS NULL WHERE p.state<>'released' AND p.metadata ? 'managed_control' ORDER BY COALESCE((p.metadata->'managed_control'->>'last_stop_attempt')::timestamptz,p.created_at) LIMIT 16").fetch_all(&a.db).await.map_err(unavailable)?;
    use futures_util::{StreamExt, stream};
    let results = stream::iter(rows)
        .map(|row| stop_callback(a, client, row))
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
    for result in results {
        result?;
    }
    Ok(())
}
async fn stop_callback(a: &App, client: &reqwest::Client, row: sqlx::postgres::PgRow) -> Api<()> {
    let permit: String = row.get("id");
    let application: String = row.get("app");
    let metadata: Value = row.get("metadata");
    let nonce: String = row.get("nonce");
    let c: Control = serde_json::from_value(metadata["managed_control"]["control"].clone())
        .map_err(|_| invalid())?;
    let owner = Principal {
        token: String::new(),
        role: "submitter".into(),
        app_id: application.clone(),
        node_id: String::new(),
        delegated_executors: vec![],
        permit_executors: vec![c.executor_id.clone()],
    };
    sqlx::query("UPDATE vrs_permits SET metadata=jsonb_set(metadata,'{managed_control,last_stop_attempt}',to_jsonb(now())) WHERE id=$1").bind(&permit).execute(&a.db).await.map_err(unavailable)?;
    if validate(
        a,
        &owner,
        &[c.executor_id.clone()],
        std::slice::from_ref(&c),
    )
    .is_err()
    {
        return Ok(());
    }
    let body = json!({"protocol_version":1,"command_id":nonce,"nonce":nonce,"action":"cancel-and-unload","permit_id":permit,"executor_id":row.get::<String,_>("executor"),"model_id":metadata["model_id"],"execution_id":metadata["execution_id"],"planned_execution_id":metadata["managed_control"]["planned_execution_id"]});
    if let Ok(reply) = callback(client, &c.stop_url, &body).await {
        if let Ok(proof) = serde_json::from_value::<Stopped>(reply) {
            let _ = confirm_stopped(a, &owner, &permit, proof).await;
        }
    }
    Ok(())
}
pub(in crate::controller) fn start_loop(shared: SharedApp) {
    tokio::spawn(async move {
        let client = client();
        loop {
            let a = App::from_ref(&shared);
            let _ = tick(&a, &client).await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_are_bound_to_approved_origin_and_no_redirect_credentials() {
        let e:Executor=serde_json::from_value(json!({"id":"task","contract_version":"1","task_type":"compute","node_id":"node","app_id":"app","resources":{"cpu_millicores":1,"memory_mib":1},"upstream_urls":["http://compute:9000/v1"]})).unwrap();
        assert!(permitted_url(&e, "http://compute:9000/start"));
        for bad in [
            "http://other:9000/start",
            "http://compute:9001/start",
            "http://user:secret@compute:9000/start",
            "http://compute:9000/start?token=x",
            "file:///tmp/start",
            "http://compute:9000/start#x",
        ] {
            assert!(!permitted_url(&e, bad));
        }
    }
}
