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
    force_admission: bool,
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
    if b.force_admission && !b.remove {
        scheduling::authorize(&a, &p, &h, &Scheduling { force_admission: true, ..Default::default() }).await?;
    }
    if b.id.is_empty()
        || b.id.len() > 96
        || !b
            .id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        || !(0..=2).contains(&b.priority)
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
            sqlx::query_scalar("SELECT count(*) FROM vrs_inference_waiters WHERE app=$1 AND id<>$2")
                .bind(&p.app_id).bind(&b.id)
                .fetch_one(&mut *tx)
                .await
                .map_err(unavailable)?;
        if count >= 64 {
            return Err(Error(StatusCode::TOO_MANY_REQUESTS, "QUEUE_FULL"));
        }
        sqlx::query("INSERT INTO vrs_inference_waiters(app,id,executor,node,priority,force_admission,valid_until) VALUES($1,$2,$3,$4,$5,$6,now()+interval '5 seconds') ON CONFLICT(app,id) DO UPDATE SET valid_until=excluded.valid_until WHERE vrs_inference_waiters.executor=excluded.executor AND vrs_inference_waiters.priority=excluded.priority AND vrs_inference_waiters.force_admission=excluded.force_admission")
            .bind(&p.app_id).bind(&b.id).bind(&e.id).bind(&e.node_id).bind(b.priority).bind(b.force_admission).execute(&mut *tx).await.map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
    }
    Ok(ok(json!({"updated":true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeWait {
    request: PermitRequest,
    wait_seconds: u64,
}
struct HintGuard { db: PgPool, app: String, id: String }
impl Drop for HintGuard {
    fn drop(&mut self) {
        let (db, app, id) = (self.db.clone(), self.app.clone(), self.id.clone());
        tokio::spawn(async move {
            let _ = sqlx::query("DELETE FROM vrs_inference_waiters WHERE app=$1 AND id=$2")
                .bind(app).bind(id).execute(&db).await;
        });
    }
}
// Wait only for admission. The controller never receives or replays inference.
pub(super) async fn acquire(State(mut a): State<App>, h: HeaderMap, Json(mut b): Json<NativeWait>) -> Api<Json<Value>> {
    if b.wait_seconds > 300 { return Err(invalid()); }
    b.request.queue_id = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(b.wait_seconds);
    let first = permit_acquire(State(a.clone()), h.clone(), Json(b.request.clone())).await;
    if b.wait_seconds == 0 || !matches!(&first, Err(e) if e.0 == StatusCode::TOO_MANY_REQUESTS) { return first; }
    let p = auth(&a, &h).await?;
    let id = Uuid::new_v4().to_string();
    let _hint = HintGuard { db: a.db.clone(), app: p.app_id.clone(), id: id.clone() };
    b.request.queue_id = Some(id.clone());
    loop {
        if std::time::Instant::now() >= deadline { return Err(Error(StatusCode::TOO_MANY_REQUESTS, "QUEUE_TIMEOUT")); }
        // Revocation, executor changes and administrator proof are rechecked.
        a.config = a.voice_config.snapshot();
        update(State(a.clone()), h.clone(), Json(Waiter { id: id.clone(), executor_id: b.request.executor_id.clone(),
            priority: b.request.scheduling.priority.score() as i16, force_admission: b.request.scheduling.force_admission, remove: false })).await?;
        match permit_acquire(State(a.clone()), h.clone(), Json(b.request.clone())).await {
            Err(e) if e.0 == StatusCode::TOO_MANY_REQUESTS => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            result => return result,
        }
    }
}

pub(super) async fn ahead(tx: &mut Transaction<'_, Postgres>, node: &str, app: &str, id: Option<&str>, executor: &str, priority: i64) -> Api<bool> {
    sqlx::query_scalar("WITH own AS (SELECT arrived_at,vrs_wait_priority(priority,force_admission,arrived_at) AS rank FROM vrs_inference_waiters WHERE app=$2 AND id=$3 AND node=$1 AND executor=$4 AND valid_until>now()) SELECT EXISTS(SELECT 1 FROM vrs_inference_waiters w WHERE w.node=$1 AND w.valid_until>now() AND NOT(w.app=$2 AND w.id=COALESCE($3,'')) AND (vrs_wait_priority(w.priority,w.force_admission,w.arrived_at)>COALESCE((SELECT rank FROM own),$5) OR (vrs_wait_priority(w.priority,w.force_admission,w.arrived_at)=COALESCE((SELECT rank FROM own),$5) AND w.arrived_at<COALESCE((SELECT arrived_at FROM own),now()))))")
        .bind(node).bind(app).bind(id).bind(executor).bind(priority).fetch_one(&mut **tx).await.map_err(unavailable)
}
