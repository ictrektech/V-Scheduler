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
) -> Api<()> {
    ready(node, executor)?;
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
        "SELECT EXISTS(SELECT 1 FROM vrs_tasks WHERE node=$1 AND status='queued')",
    )
    .bind(&id)
    .fetch_one(&mut **tx)
    .await
    .map_err(unavailable)?;
    let reserve: Resources =
        serde_json::from_value(node.get("safety_reserve")).map_err(|_| invalid())?;
    if queued
        || !cost.fits(&used, &capacity)
        || !cost.fits_guarded(&used, &node.get("report"), &reserve)
    {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "WAIT_RESOURCE"));
    }
    Ok(())
}
