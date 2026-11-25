use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{net::Ipv4Addr, sync::OnceLock, time::Duration};

#[path = "../../platform/shared/application_backend.rs"]
mod wire;
use wire::{Instance, Operation, PORT_MAX, PORT_MIN, RuntimeState};
use super::backend_transfers::wire as channel;

const IMAGE_STUDIO: &str = "com.ictrek.image-studio";
type PublishedImages = std::collections::BTreeMap<String, String>;

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
    #[serde(skip_serializing)]
    channel_token: Option<String>,
    #[serde(skip_serializing)]
    automatic_access: bool,
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
            channel_token: (operation == Operation::Ensure).then(|| self.channel_token.clone()).flatten(),
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

fn published_images(app_id: &str, compose: &str) -> anyhow::Result<PublishedImages> {
    const DECLARATION: &str = "x-v-scheduler-backend-images:";
    let mut declarations = compose.lines().filter_map(|line| line.strip_prefix(DECLARATION));
    let mut images: PublishedImages = if let Some(declaration) = declarations.next() {
        anyhow::ensure!(declarations.next().is_none(), "duplicate backend image declaration");
        serde_json::from_str(declaration.trim())?
    } else if app_id == IMAGE_STUDIO {
        #[derive(Deserialize)]
        struct LegacyImage { image: String }
        let legacy = compose.lines().find_map(|line| line.trim_start()
            .strip_prefix("IMAGE_STUDIO_BACKEND_IMAGES:"));
        match legacy {
            Some(value) => serde_json::from_str::<std::collections::BTreeMap<String, LegacyImage>>(
                value.trim().trim_matches('\''),
            )?.into_iter().map(|(profile, entry)| (profile, entry.image)).collect(),
            None => PublishedImages::new(),
        }
    } else {
        PublishedImages::new()
    };
    anyhow::ensure!(images.len() <= 64 && images.iter().all(|(profile, image)|
        identifier(profile) && wire::image_repository(image).is_some()), "invalid published backend image");
    if app_id == IMAGE_STUDIO {
        if let Some(thor) = images.get("jetson-thor").cloned() {
            images.entry("dgx-spark".into()).or_insert(thor);
        }
    }
    Ok(images)
}

async fn installed_images(app_id: &str) -> Api<PublishedImages> {
    let Some(vos) = crate::identity::base() else { return Ok(PublishedImages::new()); };
    let token = match tokio::fs::read_to_string("/var/lib/vos-context/access-token").await {
        Ok(token) => token,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(PublishedImages::new()),
        Err(_) => return Err(dependency()),
    };
    let response = client().get(format!("{vos}/v1000/app/detail"))
        .query(&[("app_id", app_id)])
        .bearer_auth(token.trim())
        .timeout(Duration::from_secs(5))
        .send().await.map_err(|_| dependency())?;
    if response.status() == StatusCode::NOT_FOUND { return Ok(PublishedImages::new()); }
    let envelope = app_bindings::bounded_vos_installation(response).await.map_err(|_| dependency())?;
    if envelope["code"] == 10067 { return Ok(PublishedImages::new()); }
    if envelope["code"] != 0 { return Err(dependency()); }
    let installed = &envelope["data"];
    if installed["state"] != "installed" || installed["app_id"] != app_id {
        return Ok(PublishedImages::new());
    }
    let compose = installed["compose_yaml"].as_str().unwrap_or_default();
    published_images(app_id, compose).map_err(|_| conflict("BACKEND_IMAGE_CATALOG_INVALID"))
}

fn fixed_targets(nodes: &[Value], members: &[Value]) -> Vec<(Target, Ipv4Addr)> {
    let fixed = nodes.iter().filter_map(|node| {
        let id = node["id"].as_str()?;
        let profile = node["profile"].as_str()?;
        let host = node["host"].as_str()?.parse::<Ipv4Addr>().ok()?;
        let target = Target { kind: "ssh".into(), id: id.into(), profile: profile.into() };
        valid_target(&target).then_some((target, host))
    }).collect::<Vec<_>>();
    let cluster = members.iter().filter_map(|member| {
        let id = member["member_id"].as_str()?;
        let host = member["node_ip"].as_str()?.parse::<Ipv4Addr>().ok()?;
        let mut matches = fixed.iter().filter(|(_, address)| *address == host);
        let (ssh, _) = matches.next()?;
        if matches.next().is_some() { return None; }
        let target = Target { kind: "cluster".into(), id: id.into(), profile: ssh.profile.clone() };
        valid_target(&target).then_some((target, host))
    }).collect::<Vec<_>>();
    fixed.into_iter().chain(cluster).collect()
}

async fn automatic_targets() -> Api<Vec<(Target, Ipv4Addr)>> {
    if !crate::managed_nodes::enabled() {
        return Ok(Vec::new());
    }
    let nodes = managed_inventory().await?;
    let members = if crate::cluster_runtime::enabled() {
        crate::cluster_runtime::members(client()).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(fixed_targets(&nodes, &members))
}

#[cfg(test)]
mod authorization_tests {
    use super::*;

    #[test]
    fn published_app_gets_matching_nodes_without_operator_grants() {
        let images = published_images("com.example.worker", concat!(
            "services: {}\n",
            "x-v-scheduler-backend-images: {\"dgx-spark\":\"registry.example.com/demo/backend:1\"}\n",
        )).unwrap();
        let nodes = [
            json!({"id":"spark-a","host":"192.0.2.1","profile":"dgx-spark"}),
            json!({"id":"spark-b","host":"192.0.2.2","profile":"dgx-spark","status":"offline"}),
            json!({"id":"orin","host":"192.0.2.3","profile":"jetson-orin"}),
        ];
        let members = [json!({"member_id":"spark-member","node_ip":"192.0.2.2"})];
        let policy = effective_access(Access { targets: vec![], repositories: vec![] },
            images, &fixed_targets(&nodes, &members));
        assert_eq!(policy["automatic"], true);
        assert_eq!(policy["targets"].as_array().unwrap().len(), 3);
        assert_eq!(policy["targets"][1]["id"], "spark-b");
        assert_eq!(policy["targets"][2]["kind"], "cluster");
    }

    #[test]
    fn installed_catalog_validates_images_and_preserves_legacy_studio_fallback() {
        let legacy = "  IMAGE_STUDIO_BACKEND_IMAGES: '{\"jetson-thor\":{\"profile\":\"thor\",\"image\":\"registry.example.com/demo/backend:thor\"}}'";
        assert!(published_images("com.example.worker", legacy).unwrap().is_empty());
        assert_eq!(published_images(IMAGE_STUDIO, legacy).unwrap()["dgx-spark"],
            "registry.example.com/demo/backend:thor");
        let invalid = "x-v-scheduler-backend-images: {\"dgx-spark\":\"http://untrusted/script\"}";
        assert!(published_images("com.example.worker", invalid).is_err());
    }
}

async fn automatic_host(target: &Target) -> Api<Ipv4Addr> {
    let (candidate, host) = automatic_targets().await?.into_iter()
        .find(|(candidate, _)| candidate.kind == target.kind && candidate.id == target.id)
        .ok_or_else(dependency)?;
    if candidate.profile != target.profile {
        return Err(conflict("BACKEND_PROFILE_UNSUPPORTED"));
    }
    Ok(host)
}

fn effective_access(mut policy: Access, images: PublishedImages, targets: &[(Target, Ipv4Addr)]) -> Value {
    if !images.is_empty() {
        policy.targets.clear();
        policy.repositories.clear();
    }
    for (target, _) in targets {
        if images.contains_key(&target.profile) && !policy.targets.contains(target) {
            policy.targets.push(target.clone());
        }
    }
    for repository in images.values().filter_map(|image| wire::image_repository(image)) {
        if !policy.repositories.iter().any(|existing| existing == repository) {
            policy.repositories.push(repository.into());
        }
    }
    json!({"targets":policy.targets,"repositories":policy.repositories,
           "automatic":!images.is_empty(),"images":images})
}

pub(super) async fn get_access(
    State(app): State<App>,
    h: HeaderMap,
    Path(app_id): Path<String>,
) -> Api<Json<Value>> {
    let p = auth(&app, &h).await?;
    if !identifier(&app_id) || p.role != "admin" && (p.role != "submitter" || p.app_id != app_id) {
        return Err(denied());
    }
    let policy = access(&app.db, &app_id).await?;
    let images = installed_images(&app_id).await?;
    let targets = if images.is_empty() { Vec::new() } else { automatic_targets().await? };
    Ok(ok(effective_access(policy, images, &targets)))
}

pub(super) async fn list_access(State(app): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    use futures_util::{StreamExt, stream};
    require(&auth(&app, &h).await?, "admin")?;
    let applications: Vec<String> = sqlx::query_scalar(
        "SELECT app_id FROM vrs_service_identities WHERE revoked_at IS NULL
         UNION SELECT app_id FROM vrs_backend_deployment_access ORDER BY app_id",
    ).fetch_all(&app.db).await.map_err(unavailable)?;
    let policies: Vec<_> = stream::iter(applications.clone().into_iter().map(|app_id| {
        let db = app.db.clone();
        async move {
            let policy = access(&db, &app_id).await?;
            let images = installed_images(&app_id).await?;
            Ok::<_, Error>((app_id, policy, images))
        }
    })).buffered(8).collect().await;
    let policies = policies.into_iter().collect::<Api<Vec<_>>>()?;
    let targets = if policies.iter().any(|(_, _, images)| !images.is_empty()) {
        automatic_targets().await?
    } else { Vec::new() };
    let items: Vec<_> = policies.into_iter().filter_map(|(app_id, policy, images)| {
        if policy.targets.is_empty() && policy.repositories.is_empty() && images.is_empty() {
            return None;
        }
        let mut row = effective_access(policy, images, &targets);
        row["app_id"] = json!(app_id);
        Some(row)
    }).collect();
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

async fn managed_inventory() -> Api<Vec<Value>> {
    client()
        .get(format!("{}/nodes", crate::managed_nodes::origin()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|_| dependency())?
        .error_for_status()
        .map_err(|_| dependency())?
        .json::<Vec<Value>>()
        .await
        .map_err(|_| dependency())
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
    let nodes = managed_inventory().await?;
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
    let images = installed_images(app_id).await?;
    let (host, automatic_access) = if images.is_empty() {
        if !permits(&access(&app.db, app_id).await?, &input.target, repository) {
            return Err(denied());
        }
        (locate(&input.target).await?, false)
    } else {
        if images.get(&input.target.profile) != Some(&input.image) {
            return Err(conflict("BACKEND_IMAGE_NOT_PUBLISHED"));
        }
        (automatic_host(&input.target).await?, true)
    };
    let host = host.to_string();
    let nvidia = matches!(
        input.target.profile.as_str(),
        "arm-cuda" | "amd-cuda" | "jetson-orin" | "jetson-thor" | "dgx-spark"
    );
    let channel_token = (app_id == IMAGE_STUDIO
        && input.config.get("mode").and_then(Value::as_str) == Some("image-studio-worker-v1"))
        .then(|| format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()));
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
        "INSERT INTO vrs_backend_deployments(id,app_id,request_key,target_id,target_kind,profile,host,host_port,container_port,image,nvidia,state,requested_host_port,cpu_millicores,memory_mib,config,channel_token,automatic_access)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'pending',$12,$13,$14,$15,$16,$17) ON CONFLICT(app_id,request_key) DO NOTHING",
    ).bind(uuid.simple().to_string()).bind(app_id).bind(&input.request_key)
        .bind(&input.target.id).bind(&input.target.kind).bind(&input.target.profile).bind(&host)
        .bind(port).bind(i32::from(input.container_port)).bind(&input.image).bind(nvidia).bind(input.host_port.map(i32::from))
        .bind(input.cpu_millicores.map(|value| value as i32)).bind(input.memory_mib.map(|value| value as i32)).bind(json!(&input.config)).bind(channel_token).bind(automatic_access)
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

pub(super) async fn image_studio_target(
    app: &App,
    headers: &HeaderMap,
    id: &str,
) -> Api<(String, String, String)> {
    let principal = auth(app, headers).await?;
    let row = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM vrs_backend_deployments WHERE id=$1 AND ($2::text IS NULL OR app_id=$2)",
    )
    .bind(id).bind(owner(&principal)?)
    .fetch_optional(&app.db).await.map_err(unavailable)?.ok_or_else(missing)?;
    if row.app_id != "com.ictrek.image-studio" || row.channel_token.is_none() {
        return Err(denied());
    }
    if row.state != "running" || row.last_error.is_some() {
        return Err(conflict("BACKEND_STATE_CONFLICT"));
    }
    let profile = match row.profile.as_str() {
        "amd-cpu" => "amd",
        "arm-cpu" => "arm",
        "jetson-orin" => "l4t",
        "jetson-thor" | "dgx-spark" => "thor",
        "amd-cuda" => "amd-cuda",
        "arm-cuda" => "arm-cuda",
        _ => return Err(conflict("BACKEND_PROFILE_UNSUPPORTED")),
    };
    if row.config["profile"] != profile {
        return Err(conflict("BACKEND_PROFILE_MISMATCH"));
    }
    Ok((row.target_kind, row.target_id, profile.to_owned()))
}

pub(super) async fn exchange(
    app: &App,
    headers: &HeaderMap,
    id: &str,
    operation: channel::Operation,
    bytes: &[u8],
) -> Api<Json<Value>> {
    if operation.request(bytes.len()).is_none() {
        return Err(invalid());
    }
    let principal = auth(app, headers).await?;
    let row = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM vrs_backend_deployments WHERE id=$1 AND ($2::text IS NULL OR app_id=$2)",
    )
    .bind(id).bind(owner(&principal)?)
    .fetch_optional(&app.db).await.map_err(unavailable)?.ok_or_else(missing)?;
    let token = row.channel_token.ok_or_else(denied)?;
    if row.state != "running" || row.last_error.is_some() {
        return Err(conflict("BACKEND_STATE_CONFLICT"));
    }
    let target = Target { kind: row.target_kind, id: row.target_id, profile: row.profile };
    let repo = wire::image_repository(&row.image).ok_or_else(denied)?;
    if !row.automatic_access
        && !permits(&access(&app.db, &row.app_id).await?, &target, repo) {
        return Err(denied());
    }
    let failed = || Error(StatusCode::BAD_GATEWAY, "BACKEND_CHANNEL_UNAVAILABLE");
    let packet = channel::Packet {
        deployment_id: row.id,
        host: row.host.parse().map_err(|_| failed())?,
        host_port: row.host_port.try_into().map_err(|_| failed())?,
        token,
        operation,
        body_b64: STANDARD.encode(bytes),
    };
    let endpoint = if target.kind == "cluster" {
        format!("{}/api/application-backend/{}/channel", crate::cluster_runtime::ORIGIN, target.id)
    } else {
        format!("{}/nodes/{}/application-backend/channel", crate::managed_nodes::origin(), target.id)
    };
    let response = client().post(endpoint).json(&packet).timeout(Duration::from_secs(packet.operation.timeout_seconds() + 15))
        .send().await.map_err(|_| failed())?.error_for_status().map_err(|_| failed())?;
    let payload = response.json::<Value>().await.map_err(|_| failed())?;
    let data = if target.kind == "cluster" {
        if payload["code"] != 0 { return Err(failed()); }
        payload["data"].clone()
    } else {
        payload
    };
    let reply: channel::Reply = serde_json::from_value(data).map_err(|_| failed())?;
    match reply.status {
        200..=299 => Ok(ok(reply.data)),
        400 | 422 => Err(Error(StatusCode::UNPROCESSABLE_ENTITY, "BACKEND_TRANSFER_INVALID")),
        404 => Err(Error(StatusCode::NOT_FOUND, "BACKEND_TRANSFER_NOT_FOUND")),
        409 if reply.data["detail"] == "WEIGHT_SHARED_UNAVAILABLE" => {
            Err(conflict("BACKEND_SHARED_MODEL_HUB_UNAVAILABLE"))
        }
        409 => Err(conflict("BACKEND_TRANSFER_CONFLICT")),
        413 => Err(Error(StatusCode::PAYLOAD_TOO_LARGE, "BACKEND_TRANSFER_TOO_LARGE")),
        429 => Err(Error(StatusCode::TOO_MANY_REQUESTS, "BACKEND_TRANSFER_BUSY")),
        _ => Err(failed()),
    }
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
        if matches!(desired, "stopping" | "deleting") {
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM vrs_backend_executor_bindings b
                 JOIN vrs_tasks t ON t.body->'executor'->>'id'=b.executor_id
                 WHERE b.deployment_id=$1 AND t.status IN ('queued','assigned','running','cancelling','recovering'))",
            ).bind(&id).fetch_one(&mut *tx).await.map_err(unavailable)?;
            if active {
                return Err(conflict("BACKEND_TASKS_ACTIVE"));
            }
        }
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
        let manual = policy
            .as_ref()
            .zip(wire::image_repository(&job.image))
            .is_some_and(|(policy, repo)| permits(policy, &target, repo));
        let automatic = || async { matches!(
            (automatic_host(&target).await, job.host.parse::<Ipv4Addr>()),
            (Ok(actual), Ok(expected)) if actual == expected
        ) };
        if job.automatic_access { automatic().await } else { manual }
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
