use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{net::Ipv4Addr, sync::OnceLock, time::Duration};

#[path = "../../platform/shared/application_backend.rs"]
mod wire;
use wire::{Instance, Operation, PORT_MAX, PORT_MIN, RuntimeState};

#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    kind: String,
    id: String,
    profile: String,
}

#[derive(serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Access {
    targets: Vec<Target>,
    repositories: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Create {
    app_id: Option<String>,
    request_key: String,
    target: Target,
    image: String,
    container_port: u16,
    host_port: Option<u16>,
    cpu_millicores: Option<u32>,
    memory_mib: Option<u32>,
    #[serde(default)]
    config: serde_json::Map<String, Value>,
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct Deployment {
    id: String,
    app_id: String,
    request_key: String,
    target_kind: String,
    target_id: String,
    profile: String,
    host: String,
    host_port: i32,
    requested_host_port: Option<i32>,
    cpu_millicores: Option<i32>,
    memory_mib: Option<i32>,
    config: Value,
    container_port: i32,
    image: String,
    nvidia: bool,
    state: String,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl Deployment {
    fn view(&self) -> Value {
        let mut value = json!(self);
        value["endpoint"] = if self.state == "running" && self.last_error.is_none() {
            json!(format!("http://{}:{}", self.host, self.host_port))
        } else {
            Value::Null
        };
        value
    }

    fn matches(&self, input: &Create) -> bool {
        self.target_id == input.target.id
            && self.target_kind == input.target.kind
            && self.profile == input.target.profile
            && self.image == input.image
            && self.container_port == i32::from(input.container_port)
            && self.requested_host_port == input.host_port.map(i32::from)
            && self.cpu_millicores == input.cpu_millicores.map(|value| value as i32)
            && self.memory_mib == input.memory_mib.map(|value| value as i32)
            && self.config.as_object() == Some(&input.config)
    }

    async fn execute(&self, operation: Operation) -> Result<RuntimeState, &'static str> {
        const UNAVAILABLE: &str = "BACKEND_RUNTIME_UNAVAILABLE";
        let instance = Instance {
            id: self.id.clone(),
            host: self
                .host
                .parse()
                .map_err(|_| "BACKEND_TARGET_UNAVAILABLE")?,
            image: self.image.clone(),
            container_port: self.container_port.try_into().map_err(|_| UNAVAILABLE)?,
            host_port: self.host_port.try_into().map_err(|_| UNAVAILABLE)?,
            nvidia: self.nvidia,
            cpu_millicores: self
                .cpu_millicores
                .map(u32::try_from)
                .transpose()
                .map_err(|_| UNAVAILABLE)?,
            memory_mib: self
                .memory_mib
                .map(u32::try_from)
                .transpose()
                .map_err(|_| UNAVAILABLE)?,
            config_b64: Some(
                STANDARD.encode(serde_json::to_vec(&self.config).map_err(|_| UNAVAILABLE)?),
            ),
            operation,
        };
        let endpoint = if self.target_kind == "ssh" {
            format!(
                "{}/nodes/{}/application-backend",
                crate::managed_nodes::origin(),
                self.target_id
            )
        } else {
            format!(
                "{}/api/application-backend/{}/dispatch",
                crate::cluster_runtime::ORIGIN,
                self.target_id
            )
        };
        let response = client()
            .post(endpoint)
            .json(&instance)
            .send()
            .await
            .map_err(|_| UNAVAILABLE)?;
        let status = response.status();
        let data = response.json::<Value>().await.map_err(|_| UNAVAILABLE)?;
        if self.target_kind == "cluster" {
            return match data["code"].as_i64() {
                Some(0) if status.is_success() => {
                    serde_json::from_value(data["data"].clone()).map_err(|_| UNAVAILABLE)
                }
                Some(20501 | 20505) => Err("BACKEND_TARGET_UNAVAILABLE"),
                Some(20509) => Err("BACKEND_CLUSTER_PEER_UNAVAILABLE"),
                Some(_) => Err("BACKEND_CLUSTER_PROJECT_FAILED"),
                None => Err(UNAVAILABLE),
            };
        }
        if let Some(code) = data["code"].as_i64() {
            return Err(match data["msg"].as_str() {
                Some("backend_image_pull_failed") => "BACKEND_IMAGE_PULL_FAILED",
                Some("backend_host_port_busy") => "BACKEND_PORT_IN_USE",
                Some("backend_docker_access_denied") => "BACKEND_DOCKER_ACCESS_DENIED",
                _ if code == 20704
                    && data["msg"].as_str().is_some_and(|msg| msg.contains("认证")) =>
                {
                    "BACKEND_SSH_AUTH_REJECTED"
                }
                _ if code == 20704 => "BACKEND_SSH_UNREACHABLE",
                _ if code == 20701 || code == 20702 => "BACKEND_TARGET_UNAVAILABLE",
                _ => "BACKEND_START_FAILED",
            });
        }
        if !status.is_success() {
            return Err(UNAVAILABLE);
        }
        serde_json::from_value(data).map_err(|_| UNAVAILABLE)
    }
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(165))
            .build()
            .expect("loopback backend deployment client")
    })
}

fn identifier(value: &str) -> bool {
    (1..=96).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn valid_target(target: &Target) -> bool {
    matches!(target.kind.as_str(), "ssh" | "cluster")
        && identifier(&target.id)
        && identifier(&target.profile)
}

fn denied() -> Error {
    Error(StatusCode::FORBIDDEN, "BACKEND_DEPLOYMENT_FORBIDDEN")
}
fn missing() -> Error {
    Error(StatusCode::NOT_FOUND, "BACKEND_DEPLOYMENT_NOT_FOUND")
}
fn conflict(code: &'static str) -> Error {
    Error(StatusCode::CONFLICT, code)
}
fn dependency() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "BACKEND_TARGET_UNAVAILABLE",
    )
}

async fn access(db: &PgPool, app_id: &str) -> Api<Access> {
    let row: Option<(
        sqlx::types::Json<Vec<Target>>,
        sqlx::types::Json<Vec<String>>,
    )> = sqlx::query_as(
        "SELECT targets,repositories FROM vrs_backend_deployment_access WHERE app_id=$1",
    )
    .bind(app_id)
    .fetch_optional(db)
    .await
    .map_err(unavailable)?;
    Ok(match row {
        Some((targets, repositories)) => Access {
            targets: targets.0,
            repositories: repositories.0,
        },
        None => Access {
            targets: Vec::new(),
            repositories: Vec::new(),
        },
    })
}

fn permits(access: &Access, target: &Target, repository: &str) -> bool {
    access.repositories.iter().any(|repo| repo == repository)
        && access.targets.iter().any(|allowed| {
            allowed.kind == target.kind
                && allowed.id == target.id
                && (allowed.profile == target.profile
                    || allowed.profile == "dgx-spark" && target.profile == "jetson-thor")
        })
}

pub(super) async fn get_access(
    State(app): State<App>,
    h: HeaderMap,
    Path(app_id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&app, &h).await?;
    if p.role != "admin" && (p.role != "submitter" || p.app_id != app_id) {
        return Err(denied());
    }
    Ok(ok(json!(access(&app.db, &app_id).await?)))
}

pub(super) async fn list_access(State(app): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    require(&auth(&app, &h).await?, "admin")?;
    let items: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('app_id',a.app_id,'targets',a.targets,'repositories',a.repositories)
         FROM vrs_backend_deployment_access a ORDER BY a.app_id",
    ).fetch_all(&app.db).await.map_err(unavailable)?;
    let applications: Vec<String> = sqlx::query_scalar(
        "SELECT app_id FROM vrs_service_identities WHERE revoked_at IS NULL
         UNION SELECT app_id FROM vrs_backend_deployment_access ORDER BY app_id",
    )
    .fetch_all(&app.db)
    .await
    .map_err(unavailable)?;
    Ok(ok(json!({"items":items,"applications":applications})))
}

pub(super) async fn set_access(
    State(app): State<App>,
    h: HeaderMap,
    Path(app_id): Path<String>,
    Json(input): Json<Access>,
) -> Api<Json<Value>> {
    let principal = auth(&app, &h).await?;
    require(&principal, "admin")?;
    if !identifier(&app_id)
        || input.targets.len() > 64
        || input.repositories.len() > 64
        || !input.targets.iter().all(valid_target)
        || !input
            .repositories
            .iter()
            .all(|repository| wire::approved_repository(repository))
    {
        return Err(invalid());
    }
    let profiles: Vec<String> = sqlx::query_scalar("SELECT name FROM vrs_compute_profiles")
        .fetch_all(&app.db)
        .await
        .map_err(unavailable)?;
    if input
        .targets
        .iter()
        .any(|target| !profiles.contains(&target.profile))
    {
        return Err(invalid());
    }
    let mut tx = app.db.begin().await.map_err(unavailable)?;
    sqlx::query("INSERT INTO vrs_backend_deployment_access (app_id,targets,repositories) VALUES ($1,$2,$3)
                 ON CONFLICT (app_id) DO UPDATE SET targets=excluded.targets,repositories=excluded.repositories")
        .bind(&app_id).bind(json!(&input.targets)).bind(json!(&input.repositories))
        .execute(&mut *tx).await.map_err(unavailable)?;
    management::audit(&mut tx, &principal, "backend_deployment.access", &app_id).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(json!(input)))
}

async fn locate(target: &Target) -> Api<Ipv4Addr> {
    if target.kind == "cluster" {
        if !crate::cluster_runtime::enabled() {
            return Err(dependency());
        }
        let members = crate::cluster_runtime::members(client())
            .await
            .ok_or_else(dependency)?;
        return members
            .iter()
            .find(|row| row["member_id"] == target.id)
            .and_then(|row| row["node_ip"].as_str()?.parse().ok())
            .ok_or_else(dependency);
    }
    if !crate::managed_nodes::enabled() {
        return Err(dependency());
    }
    let nodes = client()
        .get(format!("{}/nodes", crate::managed_nodes::origin()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|_| dependency())?
        .error_for_status()
        .map_err(|_| dependency())?
        .json::<Vec<Value>>()
        .await
        .map_err(|_| dependency())?;
    let node = nodes
        .iter()
        .find(|row| row["id"] == target.id)
        .ok_or_else(dependency)?;
    if !node["compatible_profiles"]
        .as_array()
        .is_some_and(|profiles| profiles.iter().any(|profile| profile == &target.profile))
    {
        return Err(conflict("BACKEND_PROFILE_UNSUPPORTED"));
    }
    node["host"]
        .as_str()
        .and_then(|host| host.parse().ok())
        .ok_or_else(dependency)
}

pub(super) async fn create(
    State(app): State<App>,
    h: HeaderMap,
    Json(input): Json<Create>,
) -> Api<Json<Value>> {
    let principal = auth(&app, &h).await?;
    let app_id = match (principal.role.as_str(), input.app_id.as_deref()) {
        ("admin", Some(app_id)) if identifier(app_id) => app_id,
        ("submitter", None) => &principal.app_id,
        _ => return Err(denied()),
    };
    if !identifier(&input.request_key)
        || !valid_target(&input.target)
        || input.container_port == 0
        || input
            .host_port
            .is_some_and(|port| !(PORT_MIN..=PORT_MAX).contains(&port))
        || !wire::valid_limits(input.cpu_millicores, input.memory_mib)
        || serde_json::to_vec(&input.config)
            .map_err(|_| invalid())?
            .len()
            > 8192
    {
        return Err(invalid());
    }
    let repository = wire::image_repository(&input.image).ok_or_else(invalid)?;
    if let Some(row) = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM vrs_backend_deployments WHERE app_id=$1 AND request_key=$2",
    )
    .bind(app_id)
    .bind(&input.request_key)
    .fetch_optional(&app.db)
    .await
    .map_err(unavailable)?
    {
        return if row.matches(&input) {
            Ok(ok(row.view()))
        } else {
            Err(conflict("BACKEND_REQUEST_CONFLICT"))
        };
    }
    if !permits(&access(&app.db, app_id).await?, &input.target, repository) {
        return Err(denied());
    }
    let host = locate(&input.target).await?.to_string();
    let nvidia = matches!(
        input.target.profile.as_str(),
        "arm-cuda" | "amd-cuda" | "jetson-orin" | "jetson-thor" | "dgx-spark"
    );
    let uuid = Uuid::new_v4();
    let offset = i32::from(u16::from_be_bytes([uuid.as_bytes()[0], uuid.as_bytes()[1]])) % 1000;
    let mut tx = app.db.begin().await.map_err(unavailable)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&host)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    if let Some(row) = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM vrs_backend_deployments WHERE app_id=$1 AND request_key=$2",
    )
    .bind(app_id)
    .bind(&input.request_key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(unavailable)?
    {
        return if row.matches(&input) {
            Ok(ok(row.view()))
        } else {
            Err(conflict("BACKEND_REQUEST_CONFLICT"))
        };
    }
    let port: i32 = sqlx::query_scalar(
        "SELECT port FROM generate_series($1::int,$2::int) AS candidates(port)
         WHERE ($5::int IS NULL OR port=$5) AND NOT EXISTS(SELECT 1 FROM vrs_backend_deployments d WHERE d.host=$3 AND d.host_port=port AND d.state<>'removed')
         ORDER BY (port - $1 + $4) % ($2 - $1 + 1) LIMIT 1",
    ).bind(i32::from(PORT_MIN)).bind(i32::from(PORT_MAX)).bind(&host).bind(offset).bind(input.host_port.map(i32::from))
        .fetch_optional(&mut *tx).await.map_err(unavailable)?.ok_or_else(|| conflict(if input.host_port.is_some() { "BACKEND_PORT_IN_USE" } else { "BACKEND_NO_PORTS" }))?;
    sqlx::query(
        "INSERT INTO vrs_backend_deployments(id,app_id,request_key,target_id,target_kind,profile,host,host_port,container_port,image,nvidia,state,requested_host_port,cpu_millicores,memory_mib,config)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'pending',$12,$13,$14,$15) ON CONFLICT(app_id,request_key) DO NOTHING",
    ).bind(uuid.simple().to_string()).bind(app_id).bind(&input.request_key)
        .bind(&input.target.id).bind(&input.target.kind).bind(&input.target.profile).bind(&host)
        .bind(port).bind(i32::from(input.container_port)).bind(&input.image).bind(nvidia).bind(input.host_port.map(i32::from))
        .bind(input.cpu_millicores.map(|value| value as i32)).bind(input.memory_mib.map(|value| value as i32)).bind(json!(&input.config))
        .execute(&mut *tx).await.map_err(unavailable)?;
    let row = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM vrs_backend_deployments WHERE app_id=$1 AND request_key=$2",
    )
    .bind(app_id)
    .bind(&input.request_key)
    .fetch_one(&mut *tx)
    .await
    .map_err(unavailable)?;
    if !row.matches(&input) {
        return Err(conflict("BACKEND_REQUEST_CONFLICT"));
    }
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(row.view()))
}

fn owner(principal: &Principal) -> Api<Option<&str>> {
    match principal.role.as_str() {
        "admin" => Ok(None),
        "submitter" => Ok(Some(&principal.app_id)),
        _ => Err(denied()),
    }
}

pub(super) async fn list(State(app): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    let principal = auth(&app, &h).await?;
    let rows = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM vrs_backend_deployments WHERE ($1::text IS NULL OR app_id=$1) ORDER BY created_at DESC,id LIMIT 200",
    ).bind(owner(&principal)?).fetch_all(&app.db).await.map_err(unavailable)?;
    Ok(ok(
        json!({"items": rows.iter().map(Deployment::view).collect::<Vec<_>>() }),
    ))
}

pub(super) async fn detail(
    State(app): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let principal = auth(&app, &h).await?;
    let row = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM vrs_backend_deployments WHERE id=$1 AND ($2::text IS NULL OR app_id=$2)",
    )
    .bind(id)
    .bind(owner(&principal)?)
    .fetch_optional(&app.db)
    .await
    .map_err(unavailable)?
    .ok_or_else(missing)?;
    Ok(ok(row.view()))
}

async fn change(app: App, h: HeaderMap, id: String, desired: &str) -> Api<Json<Value>> {
    let principal = auth(&app, &h).await?;
    let mut tx = app.db.begin().await.map_err(unavailable)?;
    let mut row = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM vrs_backend_deployments WHERE id=$1 AND ($2::text IS NULL OR app_id=$2) FOR UPDATE NOWAIT",
    ).bind(&id).bind(owner(&principal)?).fetch_optional(&mut *tx).await.map_err(|error| {
        if error.as_database_error().and_then(|error| error.code()).as_deref() == Some("55P03") {
            conflict("BACKEND_BUSY")
        } else {
            unavailable(error)
        }
    })?.ok_or_else(missing)?;
    let desired = match (desired, row.state.as_str()) {
        ("pending", "stopped" | "failed") => "pending",
        ("pending", "pending" | "running") => &row.state,
        ("stopping", "stopped" | "stopping" | "removed" | "deleting") => &row.state,
        ("stopping", _) => "stopping",
        ("deleting", "removed" | "deleting") => &row.state,
        ("deleting", _) => "deleting",
        _ => return Err(conflict("BACKEND_STATE_CONFLICT")),
    };
    if desired != row.state {
        row = sqlx::query_as::<_, Deployment>(
            "UPDATE vrs_backend_deployments SET state=$1,retry_at=now(),last_error=NULL,updated_at=now() WHERE id=$2 RETURNING *",
        ).bind(desired).bind(id).fetch_one(&mut *tx).await.map_err(unavailable)?;
    }
    tx.commit().await.map_err(unavailable)?;
    Ok(ok(row.view()))
}

pub(super) async fn start(
    State(app): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    change(app, h, id, "pending").await
}

pub(super) async fn stop(
    State(app): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    change(app, h, id, "stopping").await
}

pub(super) async fn remove(
    State(app): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    change(app, h, id, "deleting").await
}

pub(super) fn run(db: PgPool) {
    tokio::spawn(async move {
        loop {
            let worked = tick(&db).await.unwrap_or(false);
            if !worked {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    });
}

async fn tick(db: &PgPool) -> anyhow::Result<bool> {
    let mut tx = db.begin().await?;
    let Some(job) = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM vrs_backend_deployments
         WHERE state IN ('pending','running','stopping','deleting') AND retry_at <= now()
         ORDER BY retry_at,id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };
    let operation = match job.state.as_str() {
        "pending" => Operation::Ensure,
        "running" => Operation::Status,
        _ => Operation::Remove,
    };
    let grant = if operation == Operation::Ensure {
        let policy = access(db, &job.app_id).await.ok();
        let target = Target {
            kind: job.target_kind.clone(),
            id: job.target_id.clone(),
            profile: job.profile.clone(),
        };
        policy
            .as_ref()
            .zip(wire::image_repository(&job.image))
            .is_some_and(|(policy, repo)| permits(policy, &target, repo))
    } else {
        true
    };
    let (state, error) = if !grant {
        ("failed", Some("BACKEND_DEPLOYMENT_FORBIDDEN"))
    } else {
        match (operation, job.execute(operation).await) {
            (Operation::Ensure | Operation::Status, Ok(RuntimeState::Running)) => ("running", None),
            (Operation::Ensure, Ok(RuntimeState::Pending)) => ("pending", None),
            (Operation::Status, Ok(RuntimeState::Absent | RuntimeState::Stopped)) => {
                ("stopped", None)
            }
            (Operation::Status, Ok(RuntimeState::Pending)) => {
                ("running", Some("BACKEND_RUNTIME_PENDING"))
            }
            (Operation::Status, Err(code)) => ("running", Some(code)),
            (Operation::Remove, Ok(RuntimeState::Absent)) => (
                if job.state == "deleting" {
                    "removed"
                } else {
                    "stopped"
                },
                None,
            ),
            (Operation::Remove, Ok(RuntimeState::Pending)) => (job.state.as_str(), None),
            (Operation::Remove, _) => ("cleanup_failed", Some("BACKEND_CLEANUP_FAILED")),
            (Operation::Ensure, Err(code)) => ("failed", Some(code)),
            _ if job.target_kind == "cluster" => ("failed", Some("BACKEND_CLUSTER_PROJECT_FAILED")),
            _ => ("failed", Some("BACKEND_START_FAILED")),
        }
    };
    sqlx::query(
        "UPDATE vrs_backend_deployments SET state=$1,last_error=$2,updated_at=now(),
         retry_at=now()+CASE WHEN $1='running' THEN interval '30 seconds' ELSE interval '4 seconds' END WHERE id=$3",
    ).bind(state).bind(error).bind(&job.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}
