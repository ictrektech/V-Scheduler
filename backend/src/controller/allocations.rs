//! Atomic per-node accounting for a fixed distributed inference template.
use super::*;
use sqlx::postgres::PgRow;

pub(super) const RESOURCES_SQL: &str = "SELECT resources FROM vrs_reservations WHERE node=$1 AND state<>'released' UNION ALL SELECT resources FROM vrs_permits WHERE node=$1 AND state<>'released' UNION ALL SELECT x.resources FROM vrs_permit_placements x JOIN vrs_permits p ON p.id=x.permit_id WHERE x.node_id=$1 AND p.state<>'released'";

pub(super) async fn lock_nodes(
    tx: &mut Transaction<'_, Postgres>,
    executor: &Executor,
) -> Api<Vec<PgRow>> {
    let ids = executor
        .allocations()
        .into_iter()
        .map(|p| p.node_id)
        .collect::<Vec<_>>();
    let rows=sqlx::query("SELECT *,heartbeat>now()-interval '12 seconds' AS fresh FROM vrs_nodes WHERE id=ANY($1) ORDER BY id FOR UPDATE").bind(&ids).fetch_all(&mut **tx).await.map_err(unavailable)?;
    if rows.len() != ids.len() {
        return Err(Error(StatusCode::FORBIDDEN, "EXECUTOR_NODE_NOT_APPROVED"));
    }
    Ok(rows)
}
pub(super) fn ready(node: &PgRow, executor: &str) -> Api<()> {
    if !node.get::<bool, _>("management_enabled")
        || node.get::<bool, _>("draining")
        || !node.get::<Option<bool>, _>("fresh").unwrap_or(false)
        || !report_fresh(&node.get::<Value, _>("report"))
        || !probe_ready(&node.get::<Value, _>("report"), executor)
    {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "WAIT_RESOURCE_OR_NODE",
        ));
    }
    Ok(())
}
pub(super) async fn room(
    tx: &mut Transaction<'_, Postgres>,
    node: &PgRow,
    cost: &Resources,
    executor: &str,
    memory: Option<i64>,
    queue_id: Option<&str>,
    app_id: &str,
    options: &Scheduling,
) -> Api<()> {
    ready(node, executor)?;
    let report = admission_report(&node.get("report"), memory);
    let report = ollama::idle_report(tx, &node.get::<String, _>("id"), &report, None).await?;
    let id: String = node.get("id");
    let capacity: Resources =
        serde_json::from_value(node.get("capacities")).map_err(|_| invalid())?;
    let mut used: Resources =
        serde_json::from_value(node.get("resident_baseline")).map_err(|_| invalid())?;
    for row in sqlx::query(RESOURCES_SQL)
        .bind(&id)
        .fetch_all(&mut **tx)
        .await
        .map_err(unavailable)?
    {
        used.add(&serde_json::from_value(row.get("resources")).map_err(|_| invalid())?);
    }
    let queued: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM vrs_tasks t WHERE node=$1 AND status='queued' AND vrs_priority(t.body->'scheduling')+floor(extract(epoch FROM (now()-t.created_at))/30)>=COALESCE((SELECT priority+floor(extract(epoch FROM (now()-arrived_at))/30) FROM vrs_inference_waiters WHERE app=$2 AND id=$3 AND node=$1 AND executor=$4 AND valid_until>now()),$5))",
    )
    .bind(&id)
    .bind(app_id)
    .bind(queue_id)
    .bind(executor)
    .bind(options.priority.score())
    .fetch_one(&mut **tx)
    .await
    .map_err(unavailable)?;
    let reserve: Resources =
        serde_json::from_value(node.get("safety_reserve")).map_err(|_| invalid())?;
    if app_id == ollama::OWNER
        && (cost.cpu_millicores.saturating_add(used.cpu_millicores) > capacity.cpu_millicores
            || cost.gpu_slots.saturating_add(used.gpu_slots) > capacity.gpu_slots) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "WAIT_COMPUTE_RESOURCE"));
    }
    if queued || !cost.fits(&used, &capacity) || !cost.fits_guarded_with_policy(&used, &report, &reserve, options.force_admission) {
        let pending_stop: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_permits WHERE node=$1 AND state='suspect') OR EXISTS(SELECT 1 FROM vrs_reservations WHERE node=$1 AND state='suspect')")
            .bind(&id).fetch_one(&mut **tx).await.map_err(unavailable)?;
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            if pending_stop {
                "WAIT_STOP_CONFIRMATION"
            } else if !queued
                && cost.fits(&used, &capacity)
                && report["memory_available_mib"].as_i64().unwrap_or(0)
                    < cost.host_memory_need_with_policy(&used, &report, &reserve, options.force_admission)
            {
                "WAIT_MEMORY_HEADROOM"
            } else {
                "WAIT_RESOURCE"
            },
        ));
    }
    Ok(())
}

// Refresh only local host RAM; never make stale CPU/GPU telemetry look fresh.
pub(super) fn admission_report(snapshot: &Value, memory: Option<i64>) -> Value {
    let mut report = snapshot.clone();
    if let Some(memory) = memory.filter(|n| *n >= 0) {
        report["memory_available_mib"] = json!(memory);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_local_memory_recovers_after_unload_without_refreshing_other_telemetry() {
        let old = json!({"observed_at":"old-sample","memory_available_mib":1024,"memory_total_mib":16384,"cpu_logical_count":8,"cpu_used_percent":10,"gpus":[]});
        let current = admission_report(&old, Some(5120));
        assert_eq!(current["observed_at"], old["observed_at"]);
        assert_eq!(current["gpus"], old["gpus"]);
        let request = Resources {
            memory_mib: 2048,
            ..Default::default()
        };
        assert!(!request.fits_guarded(&Resources::default(), &old, &Resources::default()));
        assert!(request.fits_guarded(&Resources::default(), &current, &Resources::default()));
        assert_eq!(admission_report(&old, None), old);
        assert!(!request.fits_guarded(
            &Resources::default(),
            &admission_report(&old, Some(512)),
            &Resources::default()
        ));
    }
}
