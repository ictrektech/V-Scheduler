//! Cooperative scheduling: opt-in victims, native stop proof, no HTTP replay.
use super::*;

pub(super) async fn authorize(a: &App, p: &Principal, h: &HeaderMap, options: &Scheduling) -> Api<Option<Principal>> {
    if !options.force_admission { return Ok(None); }
    let operator = if p.role == "admin" { p.clone() } else {
        let value = h.get("x-vscheduler-admin-authorization")
            .ok_or(Error(StatusCode::FORBIDDEN, "FORCE_ADMISSION_REQUIRES_ADMIN"))?;
        let mut headers = HeaderMap::new();
        headers.insert("authorization", value.clone());
        auth(a, &headers).await?
    };
    if operator.role != "admin" { return Err(Error(StatusCode::FORBIDDEN, "FORCE_ADMISSION_REQUIRES_ADMIN")); }
    Ok(Some(operator))
}
pub(super) fn task_options(body: &Value) -> Scheduling {
    serde_json::from_value(body["scheduling"].clone()).unwrap_or_default()
}
// Node rows must already be locked. This only asks for a stop; reservations
// remain charged until the existing attempt/permit proof path confirms exit.
pub(super) async fn preempt(tx: &mut Transaction<'_, Postgres>, node: &str, requester: &str, options: &Scheduling) -> Api<bool> {
    if !options.allow_preemption || options.priority == Priority::Background { return Ok(false); }
    // Aging applies to victims too: a repeatedly delayed task becomes protected.
    let victim = sqlx::query("SELECT id FROM vrs_tasks WHERE node=$1 AND id<>$2 AND status IN ('assigned','running') AND body->'scheduling'->>'preemptible'='true' AND body->'_scheduler_policy'->>'retry_mode'='idempotent' AND fence<COALESCE((body->'_scheduler_policy'->>'max_attempts')::bigint,1) AND vrs_priority(body->'scheduling')+floor(extract(epoch FROM (now()-created_at))/30)<$3 ORDER BY vrs_priority(body->'scheduling'),created_at DESC LIMIT 1 FOR UPDATE")
        .bind(node).bind(requester).bind(options.priority.score()).fetch_optional(&mut **tx).await.map_err(unavailable)?;
    if let Some(victim) = victim {
        let id: String = victim.get("id");
        sqlx::query("UPDATE vrs_tasks SET status='cancelling',body=jsonb_set(body,'{_scheduler_preemption}',jsonb_build_object('requester',$2::text)),version=version+1,updated_at=now() WHERE id=$1")
            .bind(&id).bind(requester).execute(&mut **tx).await.map_err(unavailable)?;
        event(tx, &id).await?;
        return Ok(true);
    }
    // Synchronous requests cannot be replayed. Only their owning application
    // can confirm a cooperative stop, after which the caller may resubmit.
    let victim = sqlx::query("SELECT p.id,p.executor FROM vrs_permits p WHERE p.node=$1 AND p.state='active' AND p.parent_attempt IS NULL AND p.metadata->'scheduling'->>'preemptible'='true' AND vrs_priority(p.metadata->'scheduling')+floor(extract(epoch FROM (now()-p.created_at))/30)<$2 AND NOT EXISTS(SELECT 1 FROM vrs_stop_requests s WHERE s.permit_id=p.id AND s.confirmed_at IS NULL) ORDER BY vrs_priority(p.metadata->'scheduling'),p.created_at DESC LIMIT 1 FOR UPDATE OF p")
        .bind(node).bind(options.priority.score()).fetch_optional(&mut **tx).await.map_err(unavailable)?;
    let Some(victim) = victim else { return Ok(false); };
    let id: String = victim.get("id");
    let executor: String = victim.get("executor");
    let receivers: Vec<String> = sqlx::query_scalar("SELECT app_id FROM vrs_integrations i WHERE heartbeat>now()-interval '20 seconds' AND EXISTS(SELECT 1 FROM jsonb_array_elements(i.manifest->'executors') e WHERE e->>'id'=$1)")
        .bind(executor).fetch_all(&mut **tx).await.map_err(unavailable)?;
    if receivers.len()!=1 { return Ok(false); }
    sqlx::query("INSERT INTO vrs_stop_requests(permit_id,app_id,nonce) VALUES($1,$2,$3) ON CONFLICT(permit_id) DO NOTHING")
        .bind(&id).bind(&receivers[0]).bind(Uuid::new_v4().to_string()).execute(&mut **tx).await.map_err(unavailable)?;
    sqlx::query("UPDATE vrs_permits SET metadata=jsonb_set(metadata,'{preempted_by}',to_jsonb($2::text)) WHERE id=$1")
        .bind(id).bind(requester).execute(&mut **tx).await.map_err(unavailable)?;
    Ok(true)
}
