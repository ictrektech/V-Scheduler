use std::time::Duration;

use futures_util::TryStreamExt;
use sqlx::{ConnectOptions, Connection, PgConnection, postgres::PgConnectOptions};
use tokio::io::AsyncReadExt;

use super::*;

// All identifiers in SQL below come from this allowlist, never from an API caller.
const TABLES: &[&str] = &[
    "modelinfer_nodes",
    "modelinfer_node_monitors",
    "modelinfer_deployments",
    "modelinfer_plaza_sources",
    "modelinfer_plaza_models",
    "modelinfer_runs",
    "modelinfer_run_logs",
    "modelinfer_run_node_data",
    "modelinfer_run_global_data",
    "modelinfer_run_steps",
    "modelinfer_deployment_containers",
    "modelinfer_settings",
    "modelinfer_comfyui_weights",
];

const SOURCE_UNAVAILABLE: Error =
    Error(StatusCode::SERVICE_UNAVAILABLE, "LEGACY_MODELINFER_DATABASE_UNAVAILABLE");
const TARGET_UNAVAILABLE: Error =
    Error(StatusCode::SERVICE_UNAVAILABLE, "MODELINFER_IMPORT_TARGET_UNAVAILABLE");
const FILES_REQUIRED: Error =
    Error(StatusCode::CONFLICT, "MODELINFER_MODEL_CARD_FILES_REQUIRED");

async fn snapshot(connection: &mut PgConnection) -> Api<Transaction<'_, Postgres>> {
    let mut source = connection
        .begin_with("BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await
        .map_err(|_| SOURCE_UNAVAILABLE)?;
    sqlx::query("SET LOCAL statement_timeout = '10s'")
        .execute(&mut *source)
        .await
        .map_err(|_| SOURCE_UNAVAILABLE)?;
    Ok(source)
}

async fn source_tables(source: &mut PgConnection) -> Api<Vec<String>> {
    sqlx::query_scalar(
        "SELECT tablename::text FROM pg_catalog.pg_tables WHERE schemaname='public' AND tablename::text=ANY($1)",
    )
    .bind(TABLES)
    .fetch_all(source)
    .await
    .map_err(|_| SOURCE_UNAVAILABLE)
}

async fn connect_legacy() -> Api<PgConnection> {
    let required = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
    let host = required("VOS_HOST_GW_IP").ok_or(SOURCE_UNAVAILABLE)?;
    let password = required("SCHEDULER_PGV_BOOTSTRAP_PASSWORD").ok_or(SOURCE_UNAVAILABLE)?;
    let user = required("VOS_POSTGRES_USER").unwrap_or_else(|| "postgres".into());
    let database = required("VOS_POSTGRES_DB").unwrap_or_else(|| "postgres".into());
    let port = match required("VOS_POSTGRES_PORT") {
        Some(value) => value.parse().ok().filter(|port| *port != 0).ok_or(SOURCE_UNAVAILABLE)?,
        None => 5432,
    };
    let options = PgConnectOptions::new()
        .host(&host)
        .port(port)
        .username(&user)
        .password(&password)
        .database(&database)
        .disable_statement_logging();
    tokio::time::timeout(Duration::from_secs(5), options.connect())
        .await
        .map_err(|_| SOURCE_UNAVAILABLE)?
        .map_err(|_| SOURCE_UNAVAILABLE)
}

pub(super) async fn preview(State(app): State<App>, headers: HeaderMap) -> Api<Json<Value>> {
    super::require(&auth(&app, &headers).await?, "admin")?;
    let mut connection = connect_legacy().await?;
    let mut source = snapshot(&mut connection).await?;
    let present = source_tables(&mut source).await?;
    let mut table_counts = serde_json::Map::new();
    for table in TABLES {
        let count = if present.iter().any(|name| name == table) {
            let query = format!("SELECT count(*) FROM public.{table}");
            Some(sqlx::query_scalar::<_, i64>(&query)
                .fetch_one(&mut *source)
                .await
                .map_err(|_| SOURCE_UNAVAILABLE)?)
        } else {
            None
        };
        table_counts.insert((*table).into(), json!(count));
    }

    let deployment_states: Vec<(String, i64)> = if present.iter().any(|name| name == "modelinfer_deployments") {
        sqlx::query_as("SELECT state, count(*) FROM public.modelinfer_deployments GROUP BY state ORDER BY state")
            .fetch_all(&mut *source)
            .await
            .map_err(|_| SOURCE_UNAVAILABLE)?
    } else {
        Vec::new()
    };
    source.rollback().await.map_err(|_| SOURCE_UNAVAILABLE)?;
    Ok(ok(json!({
        "source_present": !present.is_empty(),
        "tables": table_counts,
        "deployments_by_state": deployment_states.into_iter().collect::<std::collections::BTreeMap<_, _>>(),
    })))
}

/// Import a frozen source snapshot before starting the embedded worker.
/// The origin is the local VOS database; callers never supply DSNs or SQL.
pub(super) async fn import(State(app): State<App>, headers: HeaderMap) -> Api<Json<Value>> {
    require(&auth(&app, &headers).await?, "admin")?;
    if crate::inference_runtime::enabled() || crate::inference_runtime::explicitly_enabled() {
        return Err(Error(StatusCode::CONFLICT, "MODELINFER_IMPORT_RUNTIME_ENABLED"));
    }
    let url = crate::identity::base().ok_or(SOURCE_UNAVAILABLE)?;
    let token = crate::identity::bearer(&headers).ok_or(SOURCE_UNAVAILABLE)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| SOURCE_UNAVAILABLE)?;
    let legacy = crate::vos::get(
        &client,
        &VosEndpoint { url, token: token.into() },
        "/v1000/app/detail?app_id=com.ictrek.modelinfer",
    )
    .await
    .map_err(|_| SOURCE_UNAVAILABLE)?;
    if legacy["state"] != "stopped" {
        return Err(Error(StatusCode::CONFLICT, "MODELINFER_IMPORT_LEGACY_NOT_STOPPED"));
    }
    let mut connection = connect_legacy().await?;
    let mut source = snapshot(&mut connection).await?;
    let present = source_tables(&mut source).await?;
    if !present.iter().any(|table| table == "modelinfer_deployments") {
        return Err(Error(StatusCode::NOT_FOUND, "LEGACY_MODELINFER_DATA_NOT_FOUND"));
    }
    let busy: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.modelinfer_deployments WHERE state IN ('starting','stopping'))",
    )
    .fetch_one(&mut *source)
    .await
    .map_err(|_| SOURCE_UNAVAILABLE)?;
    if busy {
        return Err(Error(StatusCode::CONFLICT, "MODELINFER_IMPORT_WORK_IN_PROGRESS"));
    }
    verify_card_files(&mut source, &present).await?;

    let mut destination = app.db.begin().await.map_err(|_| TARGET_UNAVAILABLE)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('v-scheduler:modelinfer:import', 0))")
        .execute(&mut *destination)
        .await
        .map_err(|_| TARGET_UNAVAILABLE)?;
    sqlx::query("CREATE TABLE IF NOT EXISTS public.vrs_modelinfer_migration (id boolean PRIMARY KEY CHECK (id), counts jsonb NOT NULL)")
        .execute(&mut *destination)
        .await
        .map_err(|_| TARGET_UNAVAILABLE)?;
    if let Some(counts) = sqlx::query_scalar::<_, sqlx::types::Json<Value>>(
        "SELECT counts FROM public.vrs_modelinfer_migration WHERE id",
    )
    .fetch_optional(&mut *destination)
    .await
    .map_err(|_| TARGET_UNAVAILABLE)?
    {
        return Ok(ok(json!({"already_imported":true, "tables":counts.0})));
    }
    let mut counts = serde_json::Map::new();
    for table in TABLES {
        if present.iter().any(|name| name == table) {
            let count = copy_table(&mut source, &mut destination, table).await?;
            counts.insert((*table).into(), json!(count));
        }
    }
    sqlx::query("INSERT INTO public.vrs_modelinfer_migration (id, counts) VALUES (true, $1)")
        .bind(sqlx::types::Json(&counts))
        .execute(&mut *destination)
        .await
        .map_err(|_| TARGET_UNAVAILABLE)?;
    source.rollback().await.map_err(|_| SOURCE_UNAVAILABLE)?;
    destination.commit().await.map_err(|_| TARGET_UNAVAILABLE)?;
    Ok(ok(json!({"already_imported":false, "tables":counts})))
}

async fn columns(connection: &mut PgConnection, table: &str) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT attname::text FROM pg_attribute WHERE attrelid=to_regclass($1) AND attnum>0 AND NOT attisdropped ORDER BY attnum",
    )
    .bind(format!("public.{table}"))
    .fetch_all(connection)
    .await
}

async fn copy_table(source: &mut PgConnection, destination: &mut PgConnection, table: &str) -> Api<u64> {
    let from = columns(source, table).await.map_err(|_| SOURCE_UNAVAILABLE)?;
    let to = columns(destination, table).await.map_err(|_| TARGET_UNAVAILABLE)?;
    if from.is_empty() || from.iter().any(|name| !to.contains(name)) {
        return Err(Error(StatusCode::CONFLICT, "MODELINFER_IMPORT_SCHEMA_MISMATCH"));
    }
    let names = from.iter()
        .map(|name| format!("\"{}\"", name.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let select = format!("SELECT to_jsonb(source_row) FROM public.{table} AS source_row");
    let insert = format!(
        "INSERT INTO public.{table} ({names}) SELECT {names} FROM jsonb_populate_recordset(NULL::public.{table}, $1)"
    );
    let mut rows = sqlx::query_scalar::<_, sqlx::types::Json<Value>>(&select).fetch(source);
    let mut buffer = Vec::with_capacity(64);
    let mut count = 0;
    while let Some(value) = rows.try_next().await.map_err(|_| SOURCE_UNAVAILABLE)? {
        buffer.push(value.0);
        if buffer.len() == 64 {
            count += insert_batch(destination, &insert, &buffer).await?;
            buffer.clear();
        }
    }
    if !buffer.is_empty() {
        count += insert_batch(destination, &insert, &buffer).await?;
    }
    Ok(count)
}

async fn insert_batch(destination: &mut PgConnection, insert: &str, rows: &[Value]) -> Api<u64> {
    sqlx::query(insert)
        .bind(sqlx::types::Json(rows))
        .execute(destination)
        .await
        .map(|result| result.rows_affected())
        .map_err(|error| {
            if error.as_database_error().is_some_and(|error| error.is_unique_violation()) {
                Error(StatusCode::CONFLICT, "MODELINFER_IMPORT_CONFLICT")
            } else {
                TARGET_UNAVAILABLE
            }
        })
}

async fn verify_card_files(source: &mut PgConnection, tables: &[String]) -> Api<()> {
    if !tables.iter().any(|table| table == "modelinfer_plaza_models") {
        return Ok(());
    }
    let cards: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT path, package_path, package_sha256 FROM public.modelinfer_plaza_models",
    )
    .fetch_all(source)
    .await
    .map_err(|_| SOURCE_UNAVAILABLE)?;
    if cards.is_empty() {
        return Ok(());
    }
    let root = tokio::fs::canonicalize("/var/lib/modelinfer/plaza")
        .await
        .map_err(|_| FILES_REQUIRED)?;
    for (extracted, package, expected) in cards {
        let extracted = tokio::fs::canonicalize(extracted).await.map_err(|_| FILES_REQUIRED)?;
        let package = tokio::fs::canonicalize(package).await.map_err(|_| FILES_REQUIRED)?;
        if !extracted.starts_with(&root) || !extracted.is_dir()
            || !package.starts_with(&root) || !package.is_file()
            || expected.len() != 64
        {
            return Err(FILES_REQUIRED);
        }
        let mut file = tokio::fs::File::open(package).await.map_err(|_| FILES_REQUIRED)?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0; 65536];
        loop {
            let bytes = file.read(&mut buffer).await.map_err(|_| FILES_REQUIRED)?;
            if bytes == 0 {
                break;
            }
            digest.update(&buffer[..bytes]);
        }
        if !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(&expected) {
            return Err(FILES_REQUIRED);
        }
    }
    Ok(())
}
