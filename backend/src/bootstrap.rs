use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde_json::{Value, json};
use sqlx::{
    Connection, Row,
    postgres::{PgConnectOptions, PgConnection},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, oneshot};
use tower_http::services::ServeDir;
use uuid::Uuid;

#[derive(Debug)]
struct PgvCredentialRequired;
impl std::fmt::Display for PgvCredentialRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PGV_CREDENTIAL_REQUIRED")
    }
}
impl std::error::Error for PgvCredentialRequired {}

#[derive(Clone)]
struct Setup {
    config: PathBuf,
    shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

fn private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn database(config: &Path) -> Option<String> {
    std::fs::read_to_string(config.parent()?.join("database.env"))
        .ok()?
        .lines()
        .find_map(|s| s.strip_prefix("SCHEDULER_DATABASE_URL=").map(str::to_owned))
}

async fn initialize(
    State(state): State<Setup>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    crate::identity::verify(&headers)
        .await
        .map_err(|status| (status, Json(json!({"code":"VOS_ADMIN_REQUIRED"}))))?;
    // Serialize setup requests; only a verified VOS administrator may provision.
    let mut shutdown = state.shutdown.lock().await;
    if shutdown.is_none() {
        return Ok(Json(json!({"code":"OK","data":{"ready":true}})));
    }
    if let Err(error) = provision(&state.config, &headers).await {
        // Connection strings, upstream responses and credentials never reach the UI.
        let code = if error.downcast_ref::<PgvCredentialRequired>().is_some() {
            "PGV_CREDENTIAL_REQUIRED"
        } else {
            "PGV_SETUP_UNAVAILABLE"
        };
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":code})),
        ));
    }
    if let Some(sender) = shutdown.take() {
        let _ = sender.send(());
    }
    Ok(Json(json!({"code":"OK","data":{"ready":true}})))
}

async fn provision(config: &Path, headers: &HeaderMap) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    let base = crate::identity::base().context("VOS endpoint missing")?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .get(format!(
            "{}/v1000/app/detail?app_id=com.ictrek.pgv",
            base.trim_end_matches('/')
        ))
        .header("Authorization", headers["authorization"].to_str()?)
        .send()
        .await?;
    ensure!(response.status().is_success(), "PGV discovery failed");
    let detail: Value = response.json().await?;
    ensure!(
        detail["code"] == 0 && detail["data"]["state"] == "installed",
        "PGV must be installed"
    );
    // VOS app/detail intentionally masks secrets. The Compose environment gets
    // the actual VOS-injected value, which PGV uses for its own superuser.
    let credential_path = config
        .parent()
        .context("config directory missing")?
        .join("pgv-bootstrap.json");
    let supplied: Option<Value> = if credential_path.exists() {
        Some(serde_json::from_slice(&std::fs::read(&credential_path)?)?)
    } else {
        None
    };
    let default_password = std::env::var("SCHEDULER_PGV_BOOTSTRAP_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty());
    let empty = json!({});
    let env = supplied.as_ref().unwrap_or(&empty);
    let setting = |key: &str, default: &str| -> String {
        env[key]
            .as_str()
            .filter(|v| !v.is_empty())
            .unwrap_or(default)
            .to_owned()
    };
    let options = PgConnectOptions::new()
        .host("pgv")
        .port(5432)
        .username(&setting("POSTGRES_SUPERUSER", "postgres"))
        .password(&setting(
            "POSTGRES_PASSWORD",
            default_password.as_deref().unwrap_or("postgres"),
        ))
        .database(&setting("POSTGRES_DB", "postgres"));
    // Older PGV installations can still differ from the injected platform
    // password; identify that case without exposing credentials.
    let mut admin = PgConnection::connect_with(&options).await.map_err(|error| {
        if error.as_database_error().and_then(|db| db.code()).as_deref() == Some("28P01") {
            anyhow::Error::new(PgvCredentialRequired)
        } else {
            anyhow::Error::new(error)
        }
    })?;
    let dir = config.parent().context("config directory missing")?;
    std::fs::create_dir_all(dir)?;
    let pending = dir.join("database-provision.json");
    let record: Value = if pending.exists() {
        serde_json::from_slice(&std::fs::read(&pending)?)?
    } else {
        let r = json!({"role": format!("v_scheduler_{}", &Uuid::new_v4().simple().to_string()[..12]),
            "password": format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())});
        private(&pending, &serde_json::to_vec(&r)?)?;
        r
    };
    let role = record["role"].as_str().context("missing role")?;
    let password = record["password"].as_str().context("missing password")?;
    ensure!(
        role.starts_with("v_scheduler_")
            && role.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "invalid role"
    );
    ensure!(
        password.len() >= 32 && password.chars().all(|c| c.is_ascii_alphanumeric()),
        "invalid password"
    );
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_roles WHERE rolname=$1)")
        .bind(role)
        .fetch_one(&mut admin)
        .await?;
    if !exists {
        sqlx::query(&format!(
            "CREATE ROLE {role} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD '{password}'"
        ))
        .execute(&mut admin)
        .await?;
    }
    let owner = sqlx::query(
        "SELECT pg_get_userbyid(datdba) AS owner FROM pg_database WHERE datname='v_scheduler'",
    )
    .fetch_optional(&mut admin)
    .await?;
    if let Some(row) = owner {
        ensure!(
            row.get::<String, _>("owner") == role,
            "existing database ownership differs"
        );
    } else {
        sqlx::query(&format!("CREATE DATABASE v_scheduler OWNER {role}"))
            .execute(&mut admin)
            .await?;
    }
    // The privileged PGV connection ends here. Controller uses only its own ordinary role.
    admin.close().await?;
    let url = format!("postgresql://{role}:{password}@pgv:5432/v_scheduler");
    let own = PgConnection::connect(&url).await?;
    own.close().await?;
    if !config.exists() {
        let value = json!({"principals":[{"token":format!("{}{}",Uuid::new_v4().simple(),Uuid::new_v4().simple()),"role":"admin","app_id":"bootstrap"}],"nodes":[],"executors":[]});
        private(config, &serde_json::to_vec_pretty(&value)?)?;
    }
    let envfile = dir.join("database.env");
    if !envfile.exists() {
        private(
            &envfile,
            format!("SCHEDULER_DATABASE_URL={url}\n").as_bytes(),
        )?;
    }
    if credential_path.exists() {
        std::fs::remove_file(&credential_path)?;
    }
    Ok(())
}

pub async fn wait(config: PathBuf, listen: &str, ui: Option<PathBuf>) -> anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let state = Setup {
        config,
        shutdown: Arc::new(Mutex::new(Some(tx))),
    };
    let mut router = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"setup_required"})) }))
        .route("/api/v1/setup", get(|| async { Json(json!({"code":"OK","data":{"ready":false,"version":std::env::var("VOS_APP_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into())}})) }).post(initialize))
        .route("/api/v1/openapi.json", get(|| async { Json(serde_json::from_str::<Value>(include_str!("../../api/openapi-v1.json")).expect("valid OpenAPI")) }))
        .route("/scheduler/api/v1/setup", get(|| async { Json(json!({"code":"OK","data":{"ready":false,"version":std::env::var("VOS_APP_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into())}})) }).post(initialize))
        .route("/scheduler/api/v1/openapi.json", get(|| async { Json(serde_json::from_str::<Value>(include_str!("../../api/openapi-v1.json")).expect("valid OpenAPI")) }))
        .with_state(state);
    if let Some(ui) = ui {
        router = router.fallback_service(ServeDir::new(ui));
    }
    axum::serve(tokio::net::TcpListener::bind(listen).await?, router)
        .with_graceful_shutdown(async {
            let _ = rx.await;
        })
        .await?;
    Ok(())
}
