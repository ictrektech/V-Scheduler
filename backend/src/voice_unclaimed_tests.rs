use super::*;
use sqlx::{Row, postgres::PgPoolOptions};

#[tokio::test]
async fn unknown_voice_versions_never_recover_permits() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://localhost:1/no_network")
        .unwrap();
    assert_eq!(
        recover_unclaimed_direct(&pool, &json!({}), "0.0.20")
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        recover_unclaimed_direct(&pool, &json!({}), "unknown")
            .await
            .unwrap(),
        0
    );
}

// Run explicitly against a disposable PostgreSQL database, never an installed app.
#[tokio::test]
#[ignore = "requires SCHEDULER_TEST_DATABASE_URL pointing at disposable PostgreSQL"]
async fn unclaimed_voice_recovery_preserves_claims_and_fences_late_claims() {
    let url = std::env::var("SCHEDULER_TEST_DATABASE_URL").unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let schema = format!("voice_recovery_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = format!("SET search_path TO {schema}");
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |conn, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&search).execute(conn).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE vrs_permits(id text primary key, app text, executor text, node text, parent_attempt text, delegated_resources jsonb NOT NULL DEFAULT '{}', state text, lease_until timestamptz, metadata jsonb, evidence text, released_at timestamptz); CREATE TABLE vrs_permit_placements(permit_id text);").execute(&pool).await.unwrap();
    let state = json!({"binding":{"models":{"kokoro-multilang":"voice.cpu.infer","async":"voice.cpu.task"}},"executors":[{"id":"voice.cpu.infer","app_id":APP,"task_type":"model.infer","node_id":"local"},{"id":"voice.cpu.task","app_id":APP,"task_type":"voice.generate","node_id":"local"}]});
    let base = json!({"model_id":"kokoro-multilang","placements":[],"upstream_url":UPSTREAM,"execution_token_hash":"a".repeat(64)});
    for name in [
        "orphan",
        "fresh",
        "claimed",
        "other-app",
        "gateway",
        "unknown-executor",
        "wrong-model",
        "wrong-upstream",
        "no-proof",
        "empty-proof",
        "task",
        "parent",
        "delegated",
        "placement",
        "wrong-node",
        "extra-metadata",
        "already-released",
    ] {
        let mut metadata = base.clone();
        match name {
            "claimed" => metadata["execution_id"] = json!("native-started"),
            "wrong-model" => metadata["model_id"] = json!("unknown"),
            "wrong-upstream" => metadata["upstream_url"] = json!("http://elsewhere/v1"),
            "no-proof" => {
                metadata
                    .as_object_mut()
                    .unwrap()
                    .remove("execution_token_hash");
            }
            "empty-proof" => {
                metadata["execution_token_hash"] =
                    json!("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
            }
            "task" => metadata["model_id"] = json!("async"),
            "extra-metadata" => metadata["unknown_execution"] = json!(true),
            _ => {}
        }
        let app = if name == "other-app" { "other" } else { APP };
        let executor = match name {
            "gateway" => "v-scheduler.voice.test",
            "unknown-executor" => "unknown.infer",
            "task" => "voice.cpu.task",
            _ => "voice.cpu.infer",
        };
        sqlx::query("INSERT INTO vrs_permits(id,app,executor,node,parent_attempt,delegated_resources,state,lease_until,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,clock_timestamp()+($8::text||' seconds')::interval,$9)")
            .bind(name).bind(app).bind(executor).bind(if name=="wrong-node" {"other"}else{"local"})
            .bind(if name=="parent" {Some("parent")}else{None})
            .bind(if name=="delegated" {json!({"memory_mib":1})}else{json!({})})
            .bind(if name=="already-released" {"released"}else{"suspect"})
            .bind(if name=="fresh" {"600"}else{"-600"}).bind(metadata).execute(&pool).await.unwrap();
        if name == "placement" {
            sqlx::query("INSERT INTO vrs_permit_placements VALUES($1)")
                .bind(name)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    assert_eq!(
        recover_unclaimed_direct(&pool, &state, "0.0.20")
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        recover_unclaimed_direct(&pool, &state, "0.0.21")
            .await
            .unwrap(),
        1
    );
    let rows = sqlx::query("SELECT id,state,evidence FROM vrs_permits")
        .fetch_all(&pool)
        .await
        .unwrap();
    for row in rows {
        let id: String = row.get("id");
        let status: String = row.get("state");
        assert_eq!(
            status,
            if ["orphan", "already-released"].contains(&id.as_str()) {
                "released"
            } else {
                "suspect"
            },
            "{id}"
        );
        if id == "orphan" {
            assert_eq!(
                row.get::<String, _>("evidence"),
                "vvoice_expired_without_execution_claim"
            );
        }
    }
    // 安装参数迁移版本复用相同原生镜像，仍只回收未认领且过期的直接许可。
    sqlx::query("INSERT INTO vrs_permits(id,app,executor,node,state,lease_until,metadata) VALUES('orphan-22',$1,'voice.cpu.infer','local','suspect',clock_timestamp()-interval '600 seconds',$2)")
        .bind(APP).bind(&base).execute(&pool).await.unwrap();
    assert_eq!(recover_unclaimed_direct(&pool, &state, "0.0.22").await.unwrap(), 1);
    assert_eq!(recover_unclaimed_direct(&pool, &state, "0.0.22").await.unwrap(), 0);
    // A claim that commits while recovery waits must remain reserved.
    sqlx::query("INSERT INTO vrs_permits(id,app,executor,node,state,lease_until,metadata) VALUES('claim-race',$1,'voice.cpu.infer','local','active',clock_timestamp()-interval '1 second',$2)").bind(APP).bind(&base).execute(&pool).await.unwrap();
    let mut claim = pool.begin().await.unwrap();
    sqlx::query("UPDATE vrs_permits SET metadata=jsonb_set(metadata,'{execution_id}','\"claimed-before-recovery\"') WHERE id='claim-race'").execute(&mut *claim).await.unwrap();
    let other = pool.clone();
    let binding = state.clone();
    let recovering = tokio::spawn(async move {
        recover_unclaimed_direct(&other, &binding, "0.0.22")
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!recovering.is_finished());
    claim.commit().await.unwrap();
    assert_eq!(recovering.await.unwrap(), 0);
    let status: String = sqlx::query_scalar("SELECT state FROM vrs_permits WHERE id='claim-race'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
    // Recovery wins the other ordering: a late claim cannot satisfy active+lease.
    let changed=sqlx::query("UPDATE vrs_permits SET metadata=jsonb_set(metadata,'{execution_id}','\"late\"') WHERE id='orphan' AND state='active' AND lease_until>clock_timestamp()").execute(&pool).await.unwrap().rows_affected();
    assert_eq!(changed, 0);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
