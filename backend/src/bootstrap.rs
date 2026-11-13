use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use serde_json::{Value, json};
use sqlx::{
    Connection,
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
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result?;
    Ok(())
}

pub fn database(config: &Path) -> anyhow::Result<Option<String>> {
    let path = config.with_file_name("database.env");
    let file = match std::fs::read_to_string(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    file.lines()
        .find_map(|line| {
            line.strip_prefix("SCHEDULER_DATABASE_URL=")
                .filter(|s| !s.is_empty())
        })
        .map(|url| Some(url.to_owned()))
        .ok_or_else(|| anyhow::anyhow!("invalid Scheduler database configuration"))
}

fn provision_url(record: &Value) -> anyhow::Result<String> {
    use anyhow::{Context, ensure};
    let role = record["role"].as_str().context("missing role")?;
    let password = record["password"].as_str().context("missing password")?;
    ensure!(
        role.starts_with("v_scheduler_")
            && role.len() <= 63
            && role
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "invalid role"
    );
    ensure!(
        password.len() >= 32 && password.chars().all(|c| c.is_ascii_alphanumeric()),
        "invalid password"
    );
    Ok(format!(
        "postgresql://{role}:{password}@pgv:5432/v_scheduler"
    ))
}

pub fn pending_matches(config: &Path, url: &str) -> bool {
    pending_url(config).as_deref() == Some(url)
}

pub fn pending_url(config: &Path) -> Option<String> {
    let file = std::fs::read(config.with_file_name("database-provision.json")).ok()?;
    let record: Value = serde_json::from_slice(&file).ok()?;
    // Older releases left their provision record behind after setup.
    if record["in_progress"] != true {
        return None;
    }
    provision_url(&record).ok()
}

pub fn finish(config: &Path, url: &str) -> anyhow::Result<()> {
    if pending_matches(config, url) {
        let dir = config
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        // Consume the one-use handoff before marking provisioning complete.
        match std::fs::remove_file(dir.join("pgv-bootstrap.json")) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        std::fs::remove_file(dir.join("database-provision.json"))?;
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

pub fn database_credentials_failed(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| matches!(code.as_ref(), "3D000" | "28P01" | "28000"))
}

async fn initialize(
    State(state): State<Setup>,
    headers: HeaderMap,
) -> Result<Json<Value>, Response> {
    crate::identity::verify(&headers).await.map_err(|status| {
        crate::contract::failure(
            status,
            "VOS_ADMIN_REQUIRED",
            "A VOS administrator is required to initialize the database",
            false,
        )
    })?;
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
        return Err(crate::contract::failure(
            StatusCode::SERVICE_UNAVAILABLE,
            code,
            code,
            false,
        ));
    }
    if let Some(sender) = shutdown.take() {
        let _ = sender.send(());
    }
    Ok(Json(json!({"code":"OK","data":{"ready":true}})))
}

async fn provision(config: &Path, headers: &HeaderMap) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    let saved = database(config)?;
    let dir = config
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let pending = dir.join("database-provision.json");
    let previous: Option<Value> = match std::fs::read(&pending) {
        Ok(file) => Some(serde_json::from_slice(&file)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    ensure!(
        match saved.as_deref() {
            Some(url) => pending_matches(config, url),
            None => !config.try_exists()? || pending_url(config).is_some(),
        },
        "an existing database configuration requires recovery"
    );
    let base = crate::identity::base().context("VOS endpoint missing")?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let token = crate::identity::bearer(headers).context("VOS bearer missing")?;
    let response = client
        .get(format!(
            "{}/v1000/app/detail?app_id=com.ictrek.pgv",
            base.trim_end_matches('/')
        ))
        .bearer_auth(token)
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
    let mut admin =
        tokio::time::timeout(Duration::from_secs(5), PgConnection::connect_with(&options))
            .await?
            .map_err(|error| {
                if error
                    .as_database_error()
                    .and_then(|db| db.code())
                    .as_deref()
                    == Some("28P01")
                {
                    anyhow::Error::new(PgvCredentialRequired)
                } else {
                    anyhow::Error::new(error)
                }
            })?;
    // Serialize cooperating installations even when an old container is still
    // running. A session lock also covers CREATE DATABASE outside a transaction.
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(1448301384, 1)")
        .fetch_one(&mut admin)
        .await?;
    ensure!(locked, "another database setup is in progress");
    ensure!(
        database(config)? == saved
            && std::fs::read(&pending)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                == previous,
        "database configuration changed during setup"
    );
    std::fs::create_dir_all(dir)?;
    let mut record = previous.unwrap_or_else(|| {
        json!({"role": format!("v_scheduler_{}", &Uuid::new_v4().simple().to_string()[..12]),
            "password": format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())})
    });
    let url = provision_url(&record)?;
    if record["in_progress"] != true {
        record["in_progress"] = json!(true);
        private(&pending, &serde_json::to_vec(&record)?)?;
    }
    let role = record["role"].as_str().unwrap();
    let password = record["password"].as_str().unwrap();
    // Check ownership before any DDL so an unrelated database leaves no new role.
    let owner: Option<String> = sqlx::query_scalar(
        "SELECT pg_get_userbyid(datdba)::text FROM pg_database WHERE datname='v_scheduler'",
    )
    .fetch_optional(&mut admin)
    .await?;
    ensure!(
        owner.as_deref().is_none_or(|owner| owner == role),
        "existing database ownership differs"
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
    if owner.is_none() {
        sqlx::query(&format!("CREATE DATABASE v_scheduler OWNER {role}"))
            .execute(&mut admin)
            .await?;
    }
    let own = tokio::time::timeout(Duration::from_secs(5), PgConnection::connect(&url)).await??;
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
    // Hold the lock until the completed connection configuration is durable.
    admin.close().await?;
    Ok(())
}

pub fn status(ready: bool, error: Option<&str>) -> Json<Value> {
    let mut data = json!({"ready":ready,"database_ready":ready,
        "version":std::env::var("VOS_APP_VERSION").unwrap_or_else(|_|env!("CARGO_PKG_VERSION").into())});
    if let Some(error) = error {
        data["error_code"] = json!(error);
    }
    Json(crate::contract::envelope(data, "OK", "OK"))
}

pub fn waiting(error: Option<&'static str>) -> Router {
    Router::new()
        .route("/api/v1/setup", get(move || async move { status(false, Some(error.unwrap_or("TEMPORARILY_UNAVAILABLE"))) }))
        .route("/api/v1/setup", post(|| async { StatusCode::METHOD_NOT_ALLOWED }))
        .route("/api/v1/openapi.json", get(openapi))
        .fallback(move || async move {
            let code = error.unwrap_or("TEMPORARILY_UNAVAILABLE");
            crate::contract::failure(StatusCode::SERVICE_UNAVAILABLE, code, code, false)
        })
}
async fn openapi() -> Json<Value> {
    Json(serde_json::from_str(include_str!("../../api/openapi-v1.json")).expect("valid OpenAPI"))
}
pub async fn wait(
    config: PathBuf,
    listen: &str,
    ui: Option<PathBuf>,
    gateway: Option<&crate::router::Runtime>,
) -> anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let state = Setup {
        config,
        shutdown: Arc::new(Mutex::new(Some(tx))),
    };
    let api = Router::new()
        .route(
            "/setup",
            get(|| async { status(false, None) }).post(initialize),
        )
        .route("/openapi.json", get(openapi))
        .with_state(state);
    let mut router = Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"status":"setup_required"})) }),
        )
        .nest("/api/v1", api.clone())
        .nest("/scheduler/api/v1", api);

    if let Some(ui) = ui.as_ref() {
        router = router.fallback_service(ServeDir::new(ui));
    }
    if let Some(gateway) = gateway {
        gateway.set_controller(router.clone(), false);
    }
    axum::serve(tokio::net::TcpListener::bind(listen).await?, router)
        .with_graceful_shutdown(async {
            let _ = rx.await;
        })
        .await?;
    Ok(())
}
