use std::{net::Ipv4Addr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use clap::Parser;
use modelinfer::{
    db,
    error::{ModelinferError, codes},
    nodes::{self, NodeCreate, NodeUpdate, NodeView},
    ssh::{RealSshExec, SshExec},
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
    let app = Router::new()
        .route("/nodes", get(list).post(create))
        .route("/nodes/{id}", axum::routing::put(update).delete(delete))
        .route("/nodes/{id}/rescan", post(rescan))
        .route("/nodes/{id}/resources", get(resources))
        .route("/nodes/{id}/runtime", post(runtime))
        .with_state(Arc::new(Manager {
            db,
            ssh: RealSshExec::node_manager(),
            test_image: config.test_image,
        }));
    let socket = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, config.port)).await?;
    axum::serve(socket, app).await?;
    Ok(())
}

async fn list(State(app): App) -> Result<Vec<NodeView>> {
    Ok(Json(nodes::list_nodes(&app.db).await?))
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
    Ok(Json(json!({"removed": true})))
}

async fn rescan(State(app): App, Path(id): Path<String>) -> Result<NodeView> {
    // Record failed connections as offline. Refresh hardware only after the
    // stored SSH credentials have been verified.
    nodes::test_node(&app.db, &app.ssh, &id).await?;
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
