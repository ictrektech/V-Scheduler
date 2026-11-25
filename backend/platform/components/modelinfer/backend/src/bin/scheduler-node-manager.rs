#[path = "../../../../../shared/application_backend.rs"]
mod application_backend;
#[path = "../../../../../shared/application_backend_channel.rs"]
mod backend_channel;

use base64::{Engine as _, engine::general_purpose::STANDARD};

use application_backend::{Instance, Operation, RuntimeState};
use std::{collections::HashMap, net::Ipv4Addr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use clap::Parser;
use modelinfer::{
    db,
    error::{ModelinferError, codes},
    nfs::{self, ResolveInput},
    nodes::{self, NodeCreate, NodeUpdate, NodeView},
    platform::{RealPlatformClient, context_token_path, platform_base_from_env},
    ssh::{RealSshExec, SshExec, SshTarget},
};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};

#[derive(Parser)]
struct Config {
    #[arg(long, env = "SCHEDULER_NODE_DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[arg(long, default_value_t = 38121)]
    port: u16,
    #[arg(
        long,
        env = "SCHEDULER_NODE_TEST_IMAGE",
        default_value = "docker.io/library/busybox:1.37.0"
    )]
    test_image: String,
}

struct Manager {
    db: DatabaseConnection,
    ssh: RealSshExec,
    test_image: String,
    connection_errors: tokio::sync::Mutex<HashMap<String, &'static str>>,
}

type App = State<Arc<Manager>>;
type Result<T> = std::result::Result<Json<T>, ModelinferError>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    anyhow::ensure!(
        valid_image(&config.test_image),
        "invalid node runtime test image"
    );
    let db = db::connect(&config.database_url)
        .await
        .map_err(|error| anyhow::anyhow!("node database initialization failed ({})", error.code))?;
    let manager = Arc::new(Manager {
        db,
        ssh: RealSshExec::node_manager(),
        test_image: config.test_image,
        connection_errors: tokio::sync::Mutex::new(HashMap::new()),
    });
    tokio::spawn(recheck_connections(manager.clone()));
    let app = Router::new()
        .route("/nodes", get(list).post(create))
        .route("/nodes/{id}", axum::routing::put(update).delete(delete))
        .route("/nodes/{id}/rescan", post(rescan))
        .route("/nodes/{id}/resources", get(resources))
        .route("/nodes/{id}/runtime", post(runtime))
        .route("/nodes/{id}/application-backend", post(application_backend))
        .route("/nodes/{id}/application-backend/channel", post(application_backend_channel))
        .with_state(manager);
    let socket = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, config.port)).await?;
    axum::serve(socket, app).await?;
    Ok(())
}

fn connection_code(error: &ModelinferError) -> &'static str {
    if error.msg.contains("认证") {
        "NODE_SSH_AUTH_REJECTED"
    } else if error.code == codes::NODE_SUDO_FAILED {
        "NODE_SSH_SUDO_FAILED"
    } else if error.msg.contains("超时") {
        "NODE_SSH_CHECK_TIMEOUT"
    } else {
        "NODE_SSH_UNREACHABLE"
    }
}

async fn recheck_connections(app: Arc<Manager>) {
    loop {
        tokio::time::sleep(Duration::from_secs(45)).await;
        let Ok(mut nodes) = nodes::list_nodes(&app.db).await else {
            continue;
        };
        nodes.sort_by(|left, right| left.last_checked_at.cmp(&right.last_checked_at));
        let mut checks = tokio::task::JoinSet::new();
        for node in nodes.into_iter().take(8) {
            let app = app.clone();
            checks.spawn(async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(14),
                    nodes::test_node(&app.db, &app.ssh, &node.id),
                )
                .await;
                let mut errors = app.connection_errors.lock().await;
                match result {
                    Ok(Ok(_)) => {
                        errors.remove(&node.id);
                    }
                    Ok(Err(error)) => {
                        errors.insert(node.id, connection_code(&error));
                    }
                    Err(_) => {
                        errors.insert(node.id, "NODE_SSH_CHECK_TIMEOUT");
                    }
                }
            });
        }
        while checks.join_next().await.is_some() {}
    }
}

async fn list(State(app): App) -> Result<Vec<Value>> {
    let nodes = nodes::list_nodes(&app.db).await?;
    let errors = app.connection_errors.lock().await;
    Ok(Json(
        nodes
            .iter()
            .map(|node| {
                let mut value = json!(node);
                value["connection_error"] = json!(errors.get(&node.id));
                value
            })
            .collect(),
    ))
}

async fn create(State(app): App, Json(req): Json<NodeCreate>) -> Result<NodeView> {
    Ok(Json(nodes::create_node(&app.db, &app.ssh, req).await?))
}

async fn update(
    State(app): App,
    Path(id): Path<String>,
    Json(req): Json<NodeUpdate>,
) -> Result<NodeView> {
    Ok(Json(nodes::update_node(&app.db, &app.ssh, &id, req).await?))
}

async fn delete(State(app): App, Path(id): Path<String>) -> Result<Value> {
    nodes::delete_node(&app.db, &id).await?;
    app.connection_errors.lock().await.remove(&id);
    Ok(Json(json!({"removed": true})))
}

async fn rescan(State(app): App, Path(id): Path<String>) -> Result<NodeView> {
    // Record failed connections as offline. Refresh hardware only after the
    // stored SSH credentials have been verified.
    if let Err(error) = nodes::test_node(&app.db, &app.ssh, &id).await {
        app.connection_errors
            .lock()
            .await
            .insert(id, connection_code(&error));
        return Err(error);
    }
    app.connection_errors.lock().await.remove(&id);
    Ok(Json(nodes::rescan_node(&app.db, &app.ssh, &id).await?))
}

async fn resources(State(app): App, Path(id): Path<String>) -> Result<Value> {
    let node = nodes::get_node(&app.db, &id).await?;
    let script = include_str!("../../assets/scheduler-host-resources.sh");
    let target = nodes::ssh_target_of(&node).with_sudo_password("");
    // SshExec invokes the system shell and quotes the command once.
    let output = app
        .ssh
        .run_with_timeout(&target, script, Duration::from_secs(6))
        .await?;
    let mut sample = json!({"observed_at": chrono::Utc::now().to_rfc3339(), "gpus": []});
    let mut gpus = Vec::new();
    for line in output.stdout.lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["cpu", cores, used] => {
                sample["cpu_logical_count"] = json!(cores.parse::<u64>().ok().filter(|n| *n > 0));
                sample["cpu_used_percent"] = json!(
                    used.parse::<f64>()
                        .ok()
                        .filter(|n| (0.0..=100.0).contains(n))
                );
            }
            ["mem", total, available] => {
                let total = total.parse::<u64>().ok().filter(|n| *n > 0);
                let available = available
                    .parse::<u64>()
                    .ok()
                    .filter(|n| total.is_some_and(|t| *n <= t));
                sample["memory_total_mib"] = json!(total.map(|n| n / 1024));
                sample["memory_available_mib"] = json!(available.map(|n| n / 1024));
            }
            ["gpu", name, used, total, free, kind] => {
                let total = total.parse::<u64>().ok().filter(|n| *n > 0);
                let free = free
                    .parse::<u64>()
                    .ok()
                    .filter(|n| total.is_some_and(|t| *n <= t));
                gpus.push(json!({
                    "name": name, "memory_type": kind,
                    "utilization_percent": used.parse::<f64>().ok().filter(|n| (0.0..=100.0).contains(n)),
                    "total_mib": total, "free_mib": free,
                }));
            }
            _ => {}
        }
    }
    if sample["cpu_logical_count"].is_null()
        && sample["memory_total_mib"].is_null()
        && gpus.is_empty()
    {
        return Err(ModelinferError::new(
            codes::SSH_COMMAND_FAILED,
            "resource_sample_unavailable",
        ));
    }
    if gpus.is_empty() {
        gpus = NodeView::from(node)
            .gpu_devices
            .into_iter()
            .map(|gpu| {
                json!({"name": gpu.name, "memory_type": "unknown", "utilization_percent": null,
                "total_mib": Some(gpu.memory_total_mib).filter(|n| *n > 0), "free_mib": null})
            })
            .collect();
    }
    sample["gpus"] = json!(gpus);
    Ok(Json(sample))
}

async fn runtime(State(app): App, Path(id): Path<String>) -> Result<Value> {
    let node = nodes::get_node(&app.db, &id).await?;
    let target = nodes::ssh_target_of(&node);
    let image = &app.test_image;
    let result = app
        .ssh
        .run_with_timeout(
            &target,
            &format!("docker pull '{image}' >/dev/null"),
            Duration::from_secs(150),
        )
        .await?;
    if !result.success() {
        return Err(ModelinferError::new(
            codes::SSH_COMMAND_FAILED,
            "image_pull_failed",
        ));
    }
    let name = format!("v-scheduler-node-check-{}", ulid::Ulid::new());
    let result = app
        .ssh
        .run_with_timeout(
            &target,
            &format!(
                "trap 'docker rm -f {name} >/dev/null 2>&1 || true' EXIT; \
                 timeout 20s docker run --rm --pull=never --name {name} --network none \
                 --read-only --cap-drop ALL --entrypoint /bin/true '{image}'"
            ),
            Duration::from_secs(40),
        )
        .await?;
    if !result.success() {
        return Err(ModelinferError::new(
            codes::SSH_COMMAND_FAILED,
            "container_start_failed",
        ));
    }
    Ok(Json(
        json!({"image": image, "pulled": true, "started": true}),
    ))
}

fn valid_image(value: &str) -> bool {
    (1..=256).contains(&value.len())
        && value.starts_with(|ch: char| ch.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"./_:@-".contains(&ch))
}

fn model_hub_mount_dir(deployment_id: &str) -> String {
    format!("/var/lib/v-scheduler/model-hub/{deployment_id}")
}

async fn mount_model_hub(target: &SshTarget, deployment_id: &str) -> Option<String> {
    let env = std::env::vars().collect::<HashMap<_, _>>();
    let token_path = context_token_path(&env).filter(|path| path.is_file())?;
    let platform = RealPlatformClient::new(
        platform_base_from_env(&env), Some(token_path), "com.ictrek.v-scheduler",
    );
    let node_hosts = [target.host.clone()];
    let input = ResolveInput {
        config_name: nfs::MODEL_HUB_CONFIG,
        fallback_hub_path: env.get("MODELINFER_MODEL_HUB_PATH").map_or("", String::as_str),
        nfs_server_override: env.get("MODELINFER_NFS_SERVER").map_or("", String::as_str),
        node_hosts: &node_hosts,
    };
    let resolved = tokio::time::timeout(Duration::from_secs(8), nfs::resolve_nfs(&platform, &input))
        .await.ok()?.ok()?;
    let path = std::path::Path::new(&resolved.host_path);
    let export = std::path::Path::new(&resolved.nfs_export_path);
    if !path.is_absolute() || !export.is_absolute() || !path.starts_with(export)
        || path.components().any(|component| matches!(component, std::path::Component::ParentDir))
        || resolved.host_path.contains(['\'', ',', '\n', '\r', '\0'])
    {
        return None;
    }
    let server = resolved.nfs_server.parse::<Ipv4Addr>().ok()?;
    let source = format!("{server}:{}", resolved.host_path);
    let dir = model_hub_mount_dir(deployment_id);
    let mount = nfs::build_mount_cmd(&dir, &source, true, false);
    let command = format!(
        "{mount}\n\
         test \"$(findmnt -rn -M '{dir}' -o SOURCE)\" = '{source}' && \
         findmnt -rn -M '{dir}' -o OPTIONS | grep -Eq '(^|,)ro(,|$)'",
    );
    RealSshExec::new().run_with_timeout(target, &command, Duration::from_secs(15))
        .await.ok()?.success().then_some(dir)
}

async fn application_backend(
    State(app): App,
    Path(id): Path<String>,
    Json(instance): Json<Instance>,
) -> Result<RuntimeState> {
    let node = nodes::get_node(&app.db, &id).await?;
    if !instance.valid() || node.host.parse::<Ipv4Addr>() != Ok(instance.host) {
        return Err(ModelinferError::new(
            codes::PARAM_INVALID,
            "invalid_backend_target",
        ));
    }
    let name = instance.name();
    let target = nodes::ssh_target_of(&node);
    let state_check = format!(
        "docker info >/dev/null || exit 1\n\
         current=$(docker ps -aq --filter 'name=^/{name}$') || exit 1\n\
         if test -n \"$current\"; then\n\
           owner=$(docker inspect --format '{{{{index .Config.Labels \"com.ictrek.v-scheduler.backend\"}}}}' \"$current\") || exit 1\n\
           test \"$owner\" = '{}' || exit 1\n\
           running=$(docker inspect --format '{{{{.State.Running}}}}' \"$current\") || exit 1\n\
         fi\n",
        instance.id,
    );
    let operation = match instance.operation {
        Operation::Status => String::from(
            "if test -z \"$current\"; then echo absent; elif test \"$running\" = true; then echo running; else echo stopped; fi",
        ),
        Operation::Remove => {
            let dir = model_hub_mount_dir(&instance.id);
            format!(
                "if test -n \"$current\"; then docker rm -f \"$current\" >/dev/null || exit 1; fi; \
                 if mountpoint -q '{dir}'; then umount '{dir}' 2>/dev/null || sudo -n umount '{dir}' || exit 1; fi; \
                 rmdir '{dir}' 2>/dev/null || true; echo absent"
            )
        }
        Operation::Ensure => {
            let gpu = if instance.nvidia {
                "--runtime=nvidia --env NVIDIA_VISIBLE_DEVICES=all --env NVIDIA_DRIVER_CAPABILITIES=compute,utility "
            } else {
                ""
            };
            let mut limits = String::new();
            if let Some(cpu) = instance.cpu_millicores {
                limits.push_str(&format!("--cpus {:.3} ", f64::from(cpu) / 1000.0));
            }
            if let Some(memory) = instance.memory_mib {
                limits.push_str(&format!("--memory {memory}m "));
            }
            let config = instance.config_b64.as_deref().unwrap_or("e30=");
            let channel_env = instance.channel_token.as_deref()
                .map(|token| format!("--mount 'type=volume,source={name}-data,target=/data' --env 'V_SCHEDULER_BACKEND_CHANNEL_TOKEN={token}' "))
                .unwrap_or_default();
            let model_hub = if instance.channel_token.is_some() {
                mount_model_hub(&target, &instance.id).await
            } else {
                None
            };
            let model_hub_mount = model_hub.map(|dir| {
                format!("--mount 'type=bind,src={dir},target=/modelhub-shared,readonly' ")
            }).unwrap_or_default();
            format!(
                "if test -n \"$current\"; then\n\
                   test \"$running\" = true || docker start \"$current\" >/dev/null || exit 1\n\
                 else\n\
                   if ! docker image inspect '{}' >/dev/null 2>&1; then docker pull '{}' >/dev/null || {{ echo image_pull_failed; exit 1; }}; fi
\
                   docker run --detach --pull=never --name '{name}' --restart unless-stopped \
                     --label 'com.ictrek.v-scheduler.backend={}' --security-opt no-new-privileges \
                     -p '{}:{}/tcp' {gpu}{limits}{channel_env}{model_hub_mount}--env 'V_SCHEDULER_BACKEND_CONFIG_B64={config}' '{}' >/dev/null || exit 1\n\
                 fi\n\
                 docker inspect --format '{{{{.State.Running}}}}' '{name}' | grep -qx true || exit 1\n\
                 echo running",
                instance.image, instance.image, instance.id, instance.host_port, instance.container_port, instance.image,
            )
        }
    };
    let output = app
        .ssh
        .run_with_timeout(
            &target,
            &format!("{state_check}{operation}"),
            Duration::from_secs(if instance.operation == Operation::Ensure {
                150
            } else {
                15
            }),
        )
        .await?;
    match (output.success(), output.stdout.lines().last()) {
        (true, Some("running")) => Ok(Json(RuntimeState::Running)),
        (true, Some("stopped")) => Ok(Json(RuntimeState::Stopped)),
        (true, Some("absent")) => Ok(Json(RuntimeState::Absent)),
        (false, Some("image_pull_failed")) => Err(ModelinferError::new(
            codes::SSH_COMMAND_FAILED,
            "backend_image_pull_failed",
        )),
        _ if output.stderr.contains("permission denied")
            && output.stderr.contains("Docker daemon") =>
        {
            Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                "backend_docker_access_denied",
            ))
        }
        _ if output.stderr.contains("port is already allocated")
            || output.stderr.contains("address already in use") =>
        {
            Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                "backend_host_port_busy",
            ))
        }
        _ => Err(ModelinferError::new(
            codes::SSH_COMMAND_FAILED,
            "backend_container_operation_failed",
        )),
    }
}

async fn application_backend_channel(
    State(app): App,
    Path(id): Path<String>,
    Json(packet): Json<backend_channel::Packet>,
) -> Result<backend_channel::Reply> {
    fn unavailable() -> ModelinferError {
        ModelinferError::new(codes::SSH_COMMAND_FAILED, "application backend channel unavailable")
    }
    let node = nodes::get_node(&app.db, &id).await?;
    if !packet.valid_target() || node.host.parse::<Ipv4Addr>() != Ok(packet.host) {
        return Err(ModelinferError::new(codes::PARAM_INVALID, "invalid backend channel target"));
    }
    let bytes = STANDARD.decode(&packet.body_b64).map_err(|_| unavailable())?;
    let (method, path, content_type) = packet.operation.request(bytes.len()).ok_or_else(unavailable)?;
    let mut input = packet.token.into_bytes();
    input.push(b'\n');
    input.extend_from_slice(&bytes);
    // Pass the bearer outside command arguments. The destination and path come
    // from the port allocator and the fixed operation enum, never caller URLs.
    let data = content_type.map(|kind| format!("--header 'Content-Type: {kind}' --data-binary @- "))
        .unwrap_or_default();
    let timeout = packet.operation.timeout_seconds();
    let limit = packet.operation.max_reply_bytes();
    let command = format!(
        "umask 077; IFS= read -r token || exit 1; \
         headers=$(mktemp) || exit 1; trap 'rm -f \"$headers\"' EXIT; \
         printf 'Authorization: Bearer %s\\n' \"$token\" >\"$headers\" || exit 1; unset token; \
         curl --silent --max-time {timeout} --max-filesize {limit} --noproxy '*' \
         --request {method} --header @\"$headers\" {data}--write-out '\\n%{{http_code}}' \
         'http://127.0.0.1:{}{path}'",
        packet.host_port
    );
    let output = app.ssh.run_with_input(&nodes::ssh_target_of(&node), &command, input, Duration::from_secs(timeout + 5))
        .await.map_err(|_| unavailable())?;
    if !output.success() {
        return Err(unavailable());
    }
    let (body, status) = output.stdout.rsplit_once('\n').ok_or_else(unavailable)?;
    if body.len() > limit { return Err(unavailable()); }
    let status = status.parse::<u16>().ok().filter(|status| (200..=599).contains(status))
        .ok_or_else(unavailable)?;
    let data = if (200..300).contains(&status) {
        serde_json::from_str(body).map_err(|_| unavailable())?
    } else {
        backend_channel::shared_unavailable(status, body.as_bytes())
    };
    Ok(Json(backend_channel::Reply { status, data }))
}
