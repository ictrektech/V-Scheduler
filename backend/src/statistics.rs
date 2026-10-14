//! Read-only operational samples. Release is stop/handoff evidence, not a model
//! success signal; do not derive inference failure rates from permit state.
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgRow};

const P95_MIN_SAMPLES: i64 = 20;

fn timing(row: &PgRow) -> Value {
    let samples: i64 = row.get("samples");
    json!({
        "samples": samples,
        "p50_ms": row.get::<Option<f64>, _>("p50_ms"),
        "p95_ms": if samples >= P95_MIN_SAMPLES { row.get::<Option<f64>, _>("p95_ms") } else { None },
    })
}

pub async fn read(db: &PgPool, admin: bool, app: &str) -> Result<Value, sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let until: chrono::DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *tx)
        .await?;
    let since = until - Duration::hours(24);
    // One sample per task: its first assignment minus submission. Retries do
    // not turn the whole task age into a second queue-wait measurement.
    let queue = sqlx::query(
        "WITH samples AS (
          SELECT extract(epoch FROM (first.created_at-t.created_at))*1000 AS ms
          FROM vrs_tasks t
          JOIN LATERAL (
            SELECT min(a.created_at) AS created_at FROM vrs_attempts a WHERE a.task=t.id
          ) first ON true
          WHERE ($1 OR t.app=$2) AND t.created_at >= $3 AND t.created_at <= $4
            AND first.created_at >= t.created_at AND first.created_at <= $4
        ) SELECT count(*) AS samples,
          percentile_cont(0.5) WITHIN GROUP (ORDER BY ms::double precision) AS p50_ms,
          percentile_cont(0.95) WITHIN GROUP (ORDER BY ms::double precision) AS p95_ms FROM samples",
    )
    .bind(admin).bind(app).bind(since).bind(until)
    .fetch_one(&mut *tx).await?;
    // Assignment-to-confirmed-stop measures resource occupancy, including
    // dispatch and stop verification. It is not pure model runtime.
    let attempts = sqlx::query(
        "WITH samples AS (
          SELECT a.status, extract(epoch FROM (a.stopped_at-a.created_at))*1000 AS ms
          FROM vrs_attempts a JOIN vrs_tasks t ON t.id=a.task
          WHERE ($1 OR t.app=$2) AND a.stopped_at >= $3 AND a.stopped_at <= $4
            AND a.stopped_at >= a.created_at
            AND a.status IN ('succeeded','failed','cancelled')
        ) SELECT count(*) AS samples,
          count(*) FILTER (WHERE status='succeeded') AS succeeded,
          count(*) FILTER (WHERE status='failed') AS failed,
          count(*) FILTER (WHERE status='cancelled') AS cancelled,
          percentile_cont(0.5) WITHIN GROUP (ORDER BY ms::double precision) AS p50_ms,
          percentile_cont(0.95) WITHIN GROUP (ORDER BY ms::double precision) AS p95_ms FROM samples",
    )
    .bind(admin).bind(app).bind(since).bind(until)
    .fetch_one(&mut *tx).await?;
    let permits = sqlx::query(
        "WITH samples AS (
          SELECT extract(epoch FROM (released_at-created_at))*1000 AS ms
          FROM vrs_permits WHERE ($1 OR app=$2) AND state='released'
            AND released_at >= $3 AND released_at <= $4 AND released_at >= created_at
        ) SELECT count(*) AS samples,
          percentile_cont(0.5) WITHIN GROUP (ORDER BY ms::double precision) AS p50_ms,
          percentile_cont(0.95) WITHIN GROUP (ORDER BY ms::double precision) AS p95_ms FROM samples",
    )
    .bind(admin).bind(app).bind(since).bind(until)
    .fetch_one(&mut *tx).await?;
    let succeeded: i64 = attempts.get("succeeded");
    let failed: i64 = attempts.get("failed");
    let completed = succeeded + failed;
    let failure_rate = (completed > 0).then(|| failed as f64 / completed as f64);
    tx.commit().await?;
    Ok(json!({
        "window": {"hours":24,"from":since,"until":until},
        "min_p95_samples": P95_MIN_SAMPLES,
        "first_queue_wait": timing(&queue),
        "attempts": {
            "succeeded":succeeded,"failed":failed,"cancelled":attempts.get::<i64,_>("cancelled"),
            "failure_rate":failure_rate,"occupancy":timing(&attempts),
        },
        "permits": {"occupancy":timing(&permits)},
    }))
}
