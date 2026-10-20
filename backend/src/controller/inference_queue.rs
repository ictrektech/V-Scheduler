//! Short-lived admission hints coordinate the gateway with native task polling.
//! They reserve no resources and cannot cause execution or request replay.
use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Waiter {
    id: String,
    executor_id: String,
    priority: i16,
    #[serde(default)]
    remove: bool,
}
pub(super) async fn update(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<Waiter>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "submitter")?;
    if b.id.is_empty()
        || b.id.len() > 96
        || !b
            .id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        || !(0..=1).contains(&b.priority)
    {
        return Err(invalid());
    }
    if b.remove {
        sqlx::query("DELETE FROM vrs_inference_waiters WHERE app=$1 AND id=$2")
            .bind(&p.app_id)
            .bind(&b.id)
            .execute(&a.db)
            .await
            .map_err(unavailable)?;
    } else {
        let e = a
            .config
            .executors
            .iter()
            .find(|e| e.id == b.executor_id && e.app_id == p.app_id && e.task_type == "model.infer")
            .ok_or(Error(StatusCode::FORBIDDEN, "EXECUTOR_NOT_ALLOWED"))?;
        let mut tx = a.db.begin().await.map_err(unavailable)?;
        allocations::lock_nodes(&mut tx, e).await?;
        management::admit(&mut tx, e, &p).await?;
        sqlx::query("DELETE FROM vrs_inference_waiters WHERE valid_until<now()")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM vrs_inference_waiters WHERE app=$1")
                .bind(&p.app_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(unavailable)?;
        if count >= 64 {
            return Err(Error(StatusCode::TOO_MANY_REQUESTS, "QUEUE_FULL"));
        }
        sqlx::query("INSERT INTO vrs_inference_waiters(app,id,executor,node,priority,valid_until) VALUES($1,$2,$3,$4,$5,now()+interval '5 seconds') ON CONFLICT(app,id) DO UPDATE SET valid_until=excluded.valid_until WHERE vrs_inference_waiters.executor=excluded.executor AND vrs_inference_waiters.priority=excluded.priority")
            .bind(&p.app_id).bind(&b.id).bind(&e.id).bind(&e.node_id).bind(b.priority).execute(&mut *tx).await.map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
    }
    Ok(ok(json!({"updated":true})))
}
