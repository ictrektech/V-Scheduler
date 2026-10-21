//! 同一 VOS 内 V-Voice 的受限授权交接。发现声明不能替代资源批准。
use crate::model::{Config, Executor, ManagedConfig, Node, Principal, Resources};
use anyhow::{Context, ensure};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const APP: &str = "com.ictrek.v-voice";
const ORIGIN: &str = "http://v-voice-backend:7070";
const UPSTREAM: &str = "http://v-voice-backend:7070/v1";
const URL: &str = "http://v-router:8080/scheduler/api/v1";

#[derive(Default)]
struct Overlays {
    managed: ManagedConfig,
    binding: Option<Value>,
}

#[derive(Clone)]
pub struct Runtime {
    base: Arc<Config>,
    current: Arc<RwLock<Arc<Config>>>,
    overlays: Arc<RwLock<Overlays>>,
    pub management_lock: Arc<tokio::sync::Mutex<()>>,
    pub ready: Arc<AtomicBool>,
}
impl Runtime {
    pub fn restore(path: &Path, base: Config) -> anyhow::Result<Self> {
        let runtime = Self {
            base: Arc::new(base.clone()),
            current: Arc::new(RwLock::new(Arc::new(base))),
            ready: Arc::new(AtomicBool::new(false)),
            overlays: Arc::new(RwLock::new(Overlays::default())),
            management_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let state = path
            .parent()
            .context("missing config directory")?
            .join("voice-binding.json");
        if state.exists() {
            runtime.overlays.write().expect("overlay lock").binding = Some(read_private(&state)?);
        }
        Ok(runtime)
    }
    pub fn snapshot(&self) -> Arc<Config> {
        self.current.read().expect("configuration lock").clone()
    }
    pub fn is_current(&self, config: &Arc<Config>) -> bool {
        Arc::ptr_eq(config, &self.snapshot())
    }
    pub fn is_static_node(&self, id: &str) -> bool {
        self.base.nodes.iter().any(|node| node.id == id)
    }
    pub fn is_static_executor(&self, id: &str) -> bool {
        self.base.executors.iter().any(|executor| executor.id == id)
    }
    fn base_with_management(&self, managed: &ManagedConfig) -> Config {
        let mut config = (*self.base).clone();
        for node in &managed.nodes {
            let mut node = node.clone();
            if node.vos.is_none() {
                node.vos = self
                    .base
                    .nodes
                    .iter()
                    .find(|entry| entry.id == node.id)
                    .and_then(|entry| entry.vos.clone());
            }
            config.nodes.retain(|entry| entry.id != node.id);
            config.nodes.push(node.clone());
            if let Some(local) = &mut config.local_agent {
                if local["node_id"] == node.id {
                    local["automatic_capacity"] = json!(false);
                }
            }
        }
        config
            .executors
            .retain(|entry| !managed.disabled_executors.contains(&entry.id));
        for executor in &managed.executors {
            config.executors.retain(|entry| entry.id != executor.id);
            config.executors.push(executor.clone());
        }
        config
    }
    pub fn set_management(&self, managed: ManagedConfig) -> anyhow::Result<()> {
        let mut overlays = self.overlays.write().expect("overlay lock");
        let mut config = self.base_with_management(&managed);
        if let Some(binding) = &overlays.binding {
            config = with_binding(config, binding)?;
        }
        overlays.managed = managed;
        *self.current.write().expect("configuration lock") = Arc::new(config);
        Ok(())
    }
    fn install(&self, state: &Value) -> anyhow::Result<()> {
        let mut overlays = self.overlays.write().expect("overlay lock");
        let config = with_binding(self.base_with_management(&overlays.managed), state)?;
        overlays.binding = Some(state.clone());
        *self.current.write().expect("configuration lock") = Arc::new(config);
        Ok(())
    }
}

fn with_binding(mut config: Config, state: &Value) -> anyhow::Result<Config> {
    let principals: Vec<Principal> = serde_json::from_value(state["principals"].clone())?;
    let executors: Vec<Executor> = serde_json::from_value(state["executors"].clone())?;
    for p in &principals {
        ensure!(
            p.role == "submitter"
                && [APP, "v-scheduler.voice-gateway"].contains(&p.app_id.as_str()),
            "invalid binding principal"
        );
        ensure!(
            p.token.len() >= 32
                && !config
                    .principals
                    .iter()
                    .any(|existing| existing.token == p.token),
            "invalid binding credential"
        );
    }
    for executor in &executors {
        ensure!(
            [APP, "v-scheduler.voice-gateway"].contains(&executor.app_id.as_str()),
            "invalid executor owner"
        );
        ensure!(
            !config
                .executors
                .iter()
                .any(|existing| existing.id == executor.id),
            "duplicate executor"
        );
        ensure!(
            config.nodes.iter().any(|node| node.id == executor.node_id
                && executor
                    .resources
                    .fits(&node.resident_baseline, &node.capacities)),
            "unapproved execution node"
        );
    }
    config.principals.extend(principals);
    config.executors.extend(executors);
    Ok(config)
}

fn read_private(path: &Path) -> anyhow::Result<Value> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file()
            && metadata.permissions().mode() & 0o077 == 0
            && metadata.len() <= 262144,
        "binding file must be private and bounded"
    );
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn write_private(path: &Path, value: &Value) -> anyhow::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let temporary = path.with_extension(format!("{}.pending", uuid::Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec_pretty(value)?;
    ensure!(bytes.len() <= 262144, "binding snapshot exceeds limit");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    std::fs::File::open(path.parent().context("missing parent")?)?.sync_all()?;
    Ok(())
}
fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}
fn declaration(discovery: &Value) -> anyhow::Result<&Value> {
    ensure!(discovery["source"] == ORIGIN, "untrusted discovery origin");
    let observed = DateTime::parse_from_rfc3339(
        discovery["observed_at"]
            .as_str()
            .context("missing observation")?,
    )?;
    ensure!(
        (0..90).contains(&(Utc::now() - observed.with_timezone(&Utc)).num_seconds()),
        "stale discovery"
    );
    let contract = &discovery["scheduler"];
    ensure!(
        contract["protocol_version"] == 1
            && contract["app_id"] == APP
            && contract["executor_base_url"] == ORIGIN
            && contract["upstream_url"] == UPSTREAM
            && contract["binding_endpoint"] == "/api/v1/scheduler/binding",
        "untrusted binding endpoint"
    );
    Ok(contract)
}

// 只接受管理员保存的校准策略；模型发现中的权重、provider 或线程数不授予资源。
#[cfg(test)]
fn build(
    runtime: &Runtime,
    contract: &Value,
    approvals: &Value,
    old: Option<&Value>,
) -> anyhow::Result<Value> {
    ensure!(
        approvals["protocol_version"] == 1,
        "unsupported approval format"
    );
    let current = runtime.snapshot();
    let node_id = current
        .local_agent
        .as_ref()
        .and_then(|v| v["node_id"].as_str())
        .context("local execution node required")?;
    let node = current
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .context("unknown local node")?;
    build_for_node(node, contract, approvals, old)
}

fn build_for_node(
    node: &Node,
    contract: &Value,
    approvals: &Value,
    old: Option<&Value>,
) -> anyhow::Result<Value> {
    ensure!(approvals["protocol_version"] == 1, "unsupported approval format");
    let node_id = &node.id;
    let epoch = approvals["credential_epoch"].as_u64().unwrap_or(0);
    ensure!(
        old.is_none_or(|state| epoch >= state["credential_epoch"].as_u64().unwrap_or(0)),
        "credential epoch cannot roll back"
    );
    let old_credentials =
        old.filter(|state| state["credential_epoch"].as_u64().unwrap_or(0) == epoch);
    let token = old_credentials
        .and_then(|state| state["binding"]["token"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(secret);
    let executor_token = old_credentials
        .and_then(|state| state["binding"]["executor_token"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(secret);
    let gateway_token = old_credentials
        .and_then(|state| state["gateway"]["token"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(secret);
    let mut models = serde_json::Map::new();
    let mut gateway_executors = serde_json::Map::new();
    let mut secondary_models = serde_json::Map::new();
    let mut model_limits = serde_json::Map::new();
    let mut legacy_unbounded_models = std::collections::BTreeSet::new();
    let mut policies = serde_json::Map::new();
    let mut executors = Vec::new();
    let mut adapters = Vec::new();
    let mut gateway_ids = Vec::new();
    let mut music_settings = None;
    for approval in approvals["approvals"]
        .as_array()
        .context("missing approvals")?
    {
        // 校准必须绑定实际硬件和本次应用运行方式；改变 provider/线程数必须重新校准。
        if approval["architecture"].as_str() != Some(std::env::consts::ARCH)
            || approval["provider"] != contract["provider"]
            || approval["cpu_models"] != contract["cpu_models"]
            || approval["num_threads"] != contract["num_threads"]
            || approval["max_concurrent_inference"] != contract["max_concurrent_inference"]
            || approval["hardware"] != contract["_execution_hardware"]
            || !approval["hardware"].is_array()
        {
            continue;
        }
        if let Some(versions) = approval.get("application_versions") {
            let versions = versions.as_array().context("invalid calibrated application versions")?;
            ensure!(!versions.is_empty() && versions.len() <= 16 && versions.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 32)), "invalid calibrated application versions");
            if !versions.iter().any(|v| v == &contract["_application_version"]) {
                continue;
            }
        }
        let bounded_inputs = !approval["input_limits_version"].is_null();
        if bounded_inputs {
            ensure!(
                approval["input_limits_version"] == 1,
                "unsupported model input limits"
            );
            if contract["input_limits_version"] != 1 {
                continue;
            }
        }
        let id = approval["id"]
            .as_str()
            .context("missing approval identity")?;
        ensure!(identifier(id), "invalid approval identity");
        let evidence = &approval["calibration"];
        ensure!(
            evidence["evidence_id"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
                && evidence["peak_memory_mib"].as_u64().is_some_and(|n| n > 0),
            "calibration evidence required"
        );
        let resources: Resources = serde_json::from_value(approval["resources"].clone())?;
        ensure!(
            resources.valid()
                && resources.cpu_millicores > 0
                && resources.memory_mib > 0
                && (resources.gpu_memory_mib == 0 || resources.gpu_slots > 0),
            "invalid approved resources"
        );
        ensure!(
            resources.memory_mib >= evidence["peak_memory_mib"].as_i64().unwrap_or(i64::MAX),
            "budget below measured peak"
        );
        ensure!(
            resources.fits(&node.resident_baseline, &node.capacities),
            "approved resources exceed actual node"
        );
        let max_input = approval["max_input_bytes"]
            .as_u64()
            .context("missing input bound")?;
        ensure!(
            (1..=32 * 1024 * 1024).contains(&max_input),
            "invalid input bound"
        );
        ensure!(
            approval["limits"].as_object().is_some_and(|limits| limits
                .values()
                .all(|value| value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0))),
            "invalid approved input limits"
        );
        for (list, task_type) in [
            ("sync_models", "model.infer"),
            ("async_models", "voice.generate"),
        ] {
            let declared = contract[list]
                .as_array()
                .context("missing model declaration")?;
            let requested = approval[list]
                .as_array()
                .context("missing approved model list")?;
            let approved: Vec<&str> = requested
                .iter()
                .filter_map(Value::as_str)
                .filter(|id| {
                    declared
                        .iter()
                        .any(|row| row["id"] == *id && row["task_type"] == task_type
                            && (row["kind"] != "music" || (task_type == "model.infer"
                                && bounded_inputs && approval["music"].is_object()
                                && approval["music"] == contract["execution_settings"]["music"])))
                        && contract[if task_type == "model.infer" {
                            "_ready_sync"
                        } else {
                            "_ready_async"
                        }]
                        .as_array()
                        .is_some_and(|ready| ready.iter().any(|model| model == *id))
                })
                .collect();
            if approved.is_empty() {
                continue;
            }
            ensure!(
                approved.iter().all(|id| identifier(id)),
                "invalid model identity"
            );
            let executor = format!(
                "v-voice.auto.{id}.{}",
                if task_type == "model.infer" {
                    "infer"
                } else {
                    "generate"
                }
            );
            executors.push(json!({"id":executor,"app_id":APP,"node_id":node_id,"contract_version":"1","task_type":task_type,"resources":resources,"upstream_urls":if task_type=="model.infer" {vec![UPSTREAM]} else {vec![]},"retry_mode":"none","max_attempts":1}));
            adapters.push(json!({"id":executor,"url":ORIGIN,"token":executor_token,"health_url":format!("{ORIGIN}/health")}));
            for model in approved {
                ensure!(!models.contains_key(model), "duplicate model approval");
                let is_music = declared.iter().any(|row| row["id"] == model && row["kind"] == "music");
                if bounded_inputs {
                    let limits = approval["model_limits"][model]
                        .as_object()
                        .context("calibrated per-model input limits required")?;
                    ensure!(
                        !limits.is_empty()
                            && limits.values().all(|value| value
                                .as_f64()
                                .is_some_and(|n| n.is_finite() && n >= 0.0)),
                        "invalid calibrated model input limits"
                    );
                    if is_music {
                        ensure!(limits.len() == 3
                            && limits.get("duration_seconds").and_then(Value::as_f64).is_some_and(|v| (10.0..=300.0).contains(&v))
                            && limits.get("prompt_characters").and_then(Value::as_f64).is_some_and(|v| (1.0..=4000.0).contains(&v))
                            && limits.get("negative_prompt_characters").and_then(Value::as_f64).is_some_and(|v| (0.0..=4000.0).contains(&v)),
                            "complete calibrated music input limits required");
                        music_settings = Some(approval["music"].clone());
                    }
                    model_limits.insert(model.to_owned(), json!(limits));
                } else {
                    legacy_unbounded_models.insert(model.to_owned());
                }
                models.insert(model.to_owned(), json!(executor));
                if task_type == "model.infer" {
                    let gateway = format!("v-scheduler.voice.{id}.infer");
                    gateway_executors.insert(model.to_owned(), json!(gateway));
                    let secondary = approval["secondary_models"][model]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter(|id| {
                                    contract["_ready_sync"]
                                        .as_array()
                                        .is_some_and(|ready| ready.contains(id))
                                })
                                .cloned()
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    secondary_models.insert(model.to_owned(), json!(secondary));
                    let mut limits = approval["limits"].clone();
                    if is_music {
                        limits["duration"] = approval["model_limits"][model]["duration_seconds"].clone();
                        limits["duration_seconds"] = limits["duration"].clone();
                    }
                    policies.insert(format!("app-v-voice/{model}"),json!({"mode":"permit","executor_id":gateway,"completion":"synchronous","max_input_bytes":max_input,"limits":limits,"secondary_models":secondary}));
                }
            }
            if task_type == "model.infer" {
                let gateway = format!("v-scheduler.voice.{id}.infer");
                gateway_ids.push(gateway.clone());
                executors.push(json!({"id":gateway,"app_id":"v-scheduler.voice-gateway","node_id":node_id,"contract_version":"1","task_type":"model.infer","resources":resources,"upstream_urls":[UPSTREAM],"retry_mode":"none","max_attempts":1}));
                adapters.push(json!({"id":gateway,"url":ORIGIN,"token":executor_token,"health_url":format!("{ORIGIN}/health")}));
            }
        }
    }
    ensure!(!models.is_empty(), "no calibrated model available");
    for (model, secondary) in &secondary_models {
        for other in secondary.as_array().context("invalid secondary models")? {
            let other = other.as_str().context("invalid secondary identity")?;
            ensure!(
                models.get(model) == models.get(other),
                "combined model requires one measured budget"
            );
        }
    }
    // 0.0.22 仅改变安装地址，复用经核验的 0.0.21 原生镜像。
    let controller_url = if ["0.0.22", "0.0.23", "0.0.24", "0.0.25"].contains(&contract["_application_version"].as_str().unwrap_or("")) {
        "http://v-scheduler:8080/scheduler/api/v1"
    } else { URL };
    let mut state = json!({"credential_epoch":epoch,"binding":{"protocol_version":1,"binding_revision":1,"app_id":APP,"url":controller_url,"token":token,"executor_token":executor_token,"upstream_url":UPSTREAM,"models":models,"gateway_executors":gateway_executors,"secondary_models":secondary_models},
        "principals":[{"token":token,"role":"submitter","app_id":APP,"permit_executors":gateway_ids},{"token":gateway_token,"role":"submitter","app_id":"v-scheduler.voice-gateway"}],
        "executors":executors,"adapters":adapters,
        "gateway":{"url":controller_url,"token":gateway_token,"require_model_policy":true,"models":policies}});
    if !model_limits.is_empty() && !legacy_unbounded_models.is_empty() {
        ensure!(
            contract["mixed_input_limits_version"] == 1,
            "mixed model input limits capability required"
        );
        state["binding"]["mixed_input_limits_version"] = json!(1);
        state["binding"]["legacy_unbounded_models"] = json!(legacy_unbounded_models);
    }
    if !model_limits.is_empty() {
        state["binding"]["input_limits_version"] = json!(1);
        state["binding"]["model_limits"] = json!(model_limits);
    }
    if let Some(music) = music_settings {
        state["binding"]["execution_settings"] = json!({"music":music});
    }
    if contract["lifecycle_protocol_version"] == 1 {
        state["binding"]["lifecycle_protocol_version"] = json!(1);
    }
    if let Some(old) = old {
        let revision = old["binding"]["binding_revision"]
            .as_u64()
            .context("invalid persisted revision")?;
        state["binding"]["binding_revision"] = json!(revision);
        if state != *old {
            state["binding"]["binding_revision"] =
                json!(revision.checked_add(1).context("revision exhausted")?);
        }
    }
    Ok(state)
}

// V-Voice 0.0.21's audited Python direct path must atomically claim before
// loading/running a model. A lost acquire receipt can therefore be recovered
// after expiry only while no claim exists. Do not apply this to Router or tasks.
async fn recover_unclaimed_direct(
    db: &PgPool,
    state: &Value,
    version: &str,
) -> anyhow::Result<u64> {
    if !["0.0.21", "0.0.22", "0.0.23", "0.0.24", "0.0.25"].contains(&version) {
        return Ok(0);
    }
    let node = state["executors"]
        .as_array()
        .context("executor definitions required")?
        .iter()
        .filter(|executor| {
            executor["app_id"] == APP
                && executor["task_type"] == "model.infer"
                && executor["id"]
                    .as_str()
                    .is_some_and(|id| id.ends_with(".infer"))
        })
        .filter_map(|executor| Some((executor["id"].as_str()?, executor["node_id"].as_str()?)))
        .collect::<std::collections::HashMap<_, _>>();
    let mapping = state["binding"]["models"]
        .as_object()
        .context("direct model mappings required")?;
    let mut tx = db.begin().await?;
    let mut released = 0;
    for (model, executor) in mapping {
        let Some(executor) = executor.as_str() else {
            continue;
        };
        let Some(node) = node.get(executor) else {
            continue;
        };
        // PostgreSQL rechecks this predicate under the same row lock used by
        // claim/renew. A committed claim wins even if its HTTP reply was lost;
        // once recovery wins, every late claim sees a released permit.
        released += sqlx::query("UPDATE vrs_permits SET state='released',evidence='vvoice_expired_without_execution_claim',released_at=clock_timestamp() WHERE app=$1 AND executor=$2 AND node=$3 AND parent_attempt IS NULL AND delegated_resources='{}'::jsonb AND NOT EXISTS(SELECT 1 FROM vrs_permit_placements x WHERE x.permit_id=vrs_permits.id) AND metadata->'placements'='[]'::jsonb AND (metadata-ARRAY['model_id','placements','upstream_url','execution_token_hash'])='{}'::jsonb AND state IN ('active','suspect') AND lease_until<=clock_timestamp() AND NOT(metadata ? 'execution_id') AND metadata->>'model_id'=$4 AND metadata->>'upstream_url'=$5 AND metadata->>'execution_token_hash' ~ '^[0-9a-f]{64}$' AND metadata->>'execution_token_hash'<>'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'")
            .bind(APP).bind(executor).bind(node).bind(model).bind(UPSTREAM)
            .execute(&mut *tx).await?.rows_affected();
    }
    tx.commit().await?;
    Ok(released)
}

async fn reconcile(path: &Path, runtime: &Runtime, db: &PgPool) -> anyhow::Result<()> {
    let directory = path.parent().context("missing config directory")?;
    let gateway =
        std::env::var("SCHEDULER_GATEWAY_DATA").unwrap_or_else(|_| "/gateway-data".into());
    let snapshot: Value = serde_json::from_slice(&std::fs::read(
        Path::new(&gateway).join("v-voice-discovery.json"),
    )?)?;
    let mut contract = declaration(&snapshot)?.clone();
    contract["_ready_sync"] = json!(
        snapshot["models"]
            .as_array()
            .map(|models| models
                .iter()
                .filter_map(|model| model["id"].as_str())
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
    let token = std::fs::read_to_string("/var/lib/vos-context/access-token")?;
    ensure!(!token.trim().is_empty(), "missing application identity");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let inventory = client
        .get(format!("{ORIGIN}/api/v1/creative-models"))
        .send()
        .await?;
    ensure!(
        inventory.status().is_success(),
        "creative model readiness unavailable"
    );
    let inventory: Value = inventory.json().await?;
    contract["_ready_async"] = json!(
        inventory
            .as_array()
            .context("invalid creative model inventory")?
            .iter()
            .filter(|row| row["status"] == "ready")
            .filter_map(|row| row["key"].as_str()?.strip_prefix("audiocpp-"))
            .collect::<Vec<_>>()
    );
    let vos = crate::identity::base().context("VOS discovery unavailable")?;
    crate::identity::application(&client, &vos, &token, "com.ictrek.v-scheduler").await?;
    let response = client
        .get(format!(
            "{}/v1000/app/detail?app_id={APP}",
            vos.trim_end_matches('/')
        ))
        .bearer_auth(token.trim())
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "installed application discovery denied"
    );
    let detail: Value = response.json().await?;
    ensure!(
        detail["code"] == 0 && detail["data"]["state"] == "installed",
        "application must be installed on this VOS"
    );
    contract["_application_version"] = detail["data"]["manifest"]["version"].clone();
    let node_id = runtime
        .base
        .local_agent
        .as_ref()
        .and_then(|v| v["node_id"].as_str())
        .context("missing local node")?;
    // 与其他执行应用一致，读取心跳更新后的容量；启动快照可能尚未发现 GPU。
    let row = sqlx::query("SELECT capacities,resident_baseline,report,management_enabled,heartbeat FROM vrs_nodes WHERE id=$1")
        .bind(node_id)
        .fetch_one(db)
        .await?;
    ensure!(row.get::<bool, _>("management_enabled"), "local node disabled");
    let heartbeat: Option<DateTime<Utc>> = row.get("heartbeat");
    ensure!(heartbeat.is_some_and(|at| (0..12).contains(&(Utc::now() - at).num_seconds())), "local node telemetry stale");
    let current = runtime.snapshot();
    let mut node = current.nodes.iter().find(|node| node.id == node_id)
        .context("unknown local node")?.clone();
    node.capacities = serde_json::from_value(row.get("capacities"))?;
    node.resident_baseline = serde_json::from_value(row.get("resident_baseline"))?;
    let report: Value = row.get("report");
    let hardware = report["gpus"]
        .as_array()
        .map(|gpus| {
            gpus.iter()
                .map(|gpu| json!({"name":gpu["name"],"memory_type":gpu["memory_type"]}))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    contract["_execution_hardware"] = json!(hardware);
    let approval_path = directory.join("voice-approvals.json");
    let approvals = if approval_path.exists() {
        read_private(&approval_path)?
    } else {
        serde_json::from_slice(&std::fs::read("/opt/v-scheduler/voice-calibrations.json")?)?
    };
    let state_path = directory.join("voice-binding.json");
    let old = if state_path.exists() {
        Some(read_private(&state_path)?)
    } else {
        None
    };
    let state = build_for_node(&node, &contract, &approvals, old.as_ref())?;
    recover_unclaimed_direct(
        db,
        &state,
        contract["_application_version"].as_str().unwrap_or(""),
    )
    .await?;
    if old.as_ref() != Some(&state) {
        // 轮换不能使未知计算失去预算；保持旧授权，待所有活动许可/任务明确停止。
        runtime.ready.store(false, Ordering::Release);
        let mut tx = db.begin().await?;
        sqlx::query("SELECT id FROM vrs_nodes WHERE id=$1 FOR UPDATE")
            .bind(node_id)
            .fetch_one(&mut *tx)
            .await?;
        let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_permits WHERE app IN ('com.ictrek.v-voice','v-scheduler.voice-gateway') AND state<>'released') OR EXISTS(SELECT 1 FROM vrs_reservations r JOIN vrs_tasks t ON t.id=r.task WHERE t.app='com.ictrek.v-voice' AND r.state<>'released')").fetch_one(&mut *tx).await?;
        ensure!(!busy, "binding update waits for active execution");
        write_private(&state_path, &state)?;
        runtime.install(&state)?;
        tx.commit().await?;
    }
    let agent_path = directory.join("local-agent.json");
    let adapter_guard = runtime.management_lock.lock().await;
    let mut agent = read_private(&agent_path)?;
    let adapters = agent["executors"]
        .as_array_mut()
        .context("missing local adapters")?;
    adapters.retain(|entry| {
        !entry["id"].as_str().is_some_and(|id| {
            id.starts_with("v-voice.auto.") || id.starts_with("v-scheduler.voice.")
        })
    });
    adapters.extend(
        state["adapters"]
            .as_array()
            .context("invalid adapters")?
            .clone(),
    );
    write_private(&agent_path, &agent)?;
    drop(adapter_guard);
    write_private(
        &Path::new(&gateway).join("voice-scheduler.local.json"),
        &state["gateway"],
    )?;
    let response = client
        .post(format!("{ORIGIN}/api/v1/scheduler/binding"))
        .bearer_auth(token.trim())
        .json(&state["binding"])
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "application binding temporarily rejected"
    );
    Ok(())
}

pub fn start(path: PathBuf, runtime: Runtime, db: PgPool) {
    if std::env::var_os("VOS_APP_VERSION").is_none() {
        return;
    }
    tokio::spawn(async move {
        loop {
            // 发现缺席和未校准均保持拒绝；不记录响应正文、令牌或连接串。
            let ready = reconcile(&path, &runtime, &db).await.is_ok();
            runtime.ready.store(ready, Ordering::Release);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixtures() -> (Runtime, Value, Value) {
        let base: Config = serde_json::from_value(json!({"principals":[],"nodes":[{"id":"local","capacities":{"cpu_millicores":8000,"memory_mib":16384,"gpu_slots":1,"gpu_memory_mib":16384}}],"executors":[],"local_agent":{"node_id":"local"}})).unwrap();
        let runtime = Runtime {
            base: Arc::new(base.clone()),
            current: Arc::new(RwLock::new(Arc::new(base))),
            ready: Arc::new(AtomicBool::new(false)),
            overlays: Arc::new(RwLock::new(Overlays::default())),
            management_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let contract = json!({"protocol_version":1,"app_id":APP,"executor_base_url":ORIGIN,"upstream_url":UPSTREAM,"binding_endpoint":"/api/v1/scheduler/binding","provider":"cpu","cpu_models":[],"num_threads":4,"max_concurrent_inference":2,"_execution_hardware":[],"_ready_sync":["asr","speaker"],"_ready_async":["tts"],"sync_models":[{"id":"asr","task_type":"model.infer"},{"id":"speaker","task_type":"model.infer"}],"async_models":[{"id":"tts","task_type":"voice.generate","weight_bytes":1}]});
        let approvals = json!({"protocol_version":1,"approvals":[{"id":"measured","architecture":std::env::consts::ARCH,"provider":"cpu","cpu_models":[],"num_threads":4,"max_concurrent_inference":2,"hardware":[],"calibration":{"evidence_id":"unit-fixture","peak_memory_mib":1024},"resources":{"cpu_millicores":4000,"memory_mib":2048},"max_input_bytes":1048576,"limits":{},"sync_models":["asr","speaker"],"async_models":["tts"],"secondary_models":{"asr":["speaker"]}}]});
        (runtime, contract, approvals)
    }
    #[test]
    fn voice_binding_uses_heartbeat_capacity_after_cpu_only_startup() {
        let (runtime, contract, mut approvals) = fixtures();
        let mut startup = (*runtime.snapshot()).clone();
        startup.nodes[0].capacities.gpu_slots = 0;
        startup.nodes[0].capacities.gpu_memory_mib = 0;
        *runtime.current.write().unwrap() = Arc::new(startup.clone());
        approvals["approvals"][0]["resources"]["gpu_slots"] = json!(1);
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        let mut measured = startup.nodes[0].clone();
        measured.capacities.gpu_slots = 1;
        measured.capacities.gpu_memory_mib = 8192;
        let state = build_for_node(&measured, &contract, &approvals, None).unwrap();
        assert_eq!(state["executors"][0]["resources"]["gpu_slots"], 1);
        // 仍按当前节点预算核验，不能借更新容量绕过准入。
        measured.capacities.memory_mib = 1024;
        assert!(build_for_node(&measured, &contract, &approvals, None).is_err());
    }

    #[test]
    fn packaging_only_voice_upgrade_preserves_identity_and_budget() {
        let (runtime, mut contract, approvals) = fixtures();
        contract["_application_version"] = json!("0.0.21");
        let before = build(&runtime, &contract, &approvals, None).unwrap();
        contract["_application_version"] = json!("0.0.22");
        let after = build(&runtime, &contract, &approvals, Some(&before)).unwrap();
        assert_eq!(after["binding"]["url"], "http://v-scheduler:8080/scheduler/api/v1");
        assert_eq!(after["binding"]["binding_revision"], 2);
        assert_eq!(after["binding"]["token"], before["binding"]["token"]);
        assert_eq!(after["executors"], before["executors"]);
        assert_eq!(after["adapters"], before["adapters"]);
    }

    #[test]
    fn lifecycle_negotiation_preserves_identity_and_approved_resources() {
        let (runtime, mut contract, approvals) = fixtures();
        contract["_application_version"] = json!("0.0.22");
        let before = build(&runtime, &contract, &approvals, None).unwrap();
        assert!(before["binding"]["lifecycle_protocol_version"].is_null());
        contract["_application_version"] = json!("0.0.23");
        contract["lifecycle_protocol_version"] = json!(1);
        let after = build(&runtime, &contract, &approvals, Some(&before)).unwrap();
        assert_eq!(after["binding"]["lifecycle_protocol_version"], 1);
        assert_eq!(after["binding"]["binding_revision"], 2);
        assert_eq!(after["binding"]["token"], before["binding"]["token"]);
        assert_eq!(after["executors"], before["executors"]);
        let same = build(&runtime, &contract, &approvals, Some(&after)).unwrap();
        assert_eq!(same["binding"]["binding_revision"], 2);
    }

    #[test]
    fn new_voice_input_approvals_require_capability_and_bound_each_ready_model() {
        let (runtime, mut contract, mut approvals) = fixtures();
        approvals["approvals"][0]["input_limits_version"] = json!(1);
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        contract["input_limits_version"] = json!(1);
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        approvals["approvals"][0]["model_limits"] = json!({"asr":{"duration_seconds":20},"speaker":{"input_bytes":1024},"tts":{"text_characters":512,"speed_min":1.0,"speed_max":1.0}});
        let all = build(&runtime, &contract, &approvals, None).unwrap();
        assert_eq!(all["binding"]["input_limits_version"], 1);
        assert_eq!(
            all["binding"]["model_limits"]["tts"]["text_characters"],
            512
        );
        contract["_ready_sync"] = json!(["asr"]);
        contract["_ready_async"] = json!([]);
        let ready = build(&runtime, &contract, &approvals, None).unwrap();
        assert_eq!(
            ready["binding"]["model_limits"].as_object().unwrap().len(),
            1
        );
        assert!(ready["binding"]["model_limits"].get("speaker").is_none());
    }

    #[test]
    fn version_specific_voice_measurements_do_not_authorize_other_installed_versions() {
        let (runtime, mut contract, mut approvals) = fixtures();
        approvals["approvals"][0]["application_versions"] = json!(["0.0.20"]);
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        contract["_application_version"] = json!("0.0.20");
        let approved = build(&runtime, &contract, &approvals, None).unwrap();
        assert!(!approved["binding"]["models"].as_object().unwrap().is_empty());
        contract["_application_version"] = json!("0.0.21");
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        approvals["approvals"][0]["application_versions"] = json!([null]);
        assert!(build(&runtime, &contract, &approvals, None).is_err());
    }

    #[test]
    fn music_binding_requires_measured_runtime_and_complete_input_limits() {
        let (runtime, mut contract, mut approvals) = fixtures();
        contract["input_limits_version"] = json!(1);
        contract["sync_models"].as_array_mut().unwrap().push(json!({"id":"music-fixture","kind":"music","task_type":"model.infer"}));
        contract["_ready_sync"].as_array_mut().unwrap().push(json!("music-fixture"));
        let music = json!({"provider":"cpu","torch_threads":4,"torch_interop_threads":1,"max_concurrent_jobs":1,"max_loaded_models":1,"steps":8,"runtime":"pytorch-music-v1"});
        contract["execution_settings"] = json!({"music":music});
        let mut measured = approvals["approvals"][0].clone();
        measured["id"] = json!("measured-music");
        measured["sync_models"] = json!(["music-fixture"]);
        measured["async_models"] = json!([]);
        measured["secondary_models"] = json!({});
        measured["input_limits_version"] = json!(1);
        measured["model_limits"] = json!({"music-fixture":{"duration_seconds":30,"prompt_characters":512,"negative_prompt_characters":0}});
        approvals["approvals"].as_array_mut().unwrap().push(measured);
        let missing = build(&runtime, &contract, &approvals, None).unwrap();
        assert!(missing["binding"]["models"].get("music-fixture").is_none());
        approvals["approvals"][1]["music"] = music.clone();
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        contract["mixed_input_limits_version"] = json!(2);
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        contract["mixed_input_limits_version"] = json!(1);
        let approved = build(&runtime, &contract, &approvals, None).unwrap();
        assert_eq!(approved["binding"]["mixed_input_limits_version"], 1);
        assert_eq!(approved["binding"]["legacy_unbounded_models"], json!(["asr", "speaker", "tts"]));
        assert_eq!(approved["binding"]["model_limits"].as_object().unwrap().len(), 1);
        assert_eq!(approved["binding"]["execution_settings"]["music"], music);
        assert!(approved["binding"]["models"].get("music-fixture").is_some());
        assert_eq!(approved["gateway"]["models"]["app-v-voice/music-fixture"]["limits"]["duration"],30);
        contract["execution_settings"]["music"]["steps"] = json!(16);
        let changed = build(&runtime, &contract, &approvals, None).unwrap();
        assert!(changed["binding"]["models"].get("music-fixture").is_none());
        assert!(changed["binding"]["models"].get("asr").is_some());
        contract["execution_settings"]["music"] = music;
        approvals["approvals"][1]["model_limits"]["music-fixture"]["prompt_characters"] = json!(0);
        assert!(build(&runtime, &contract, &approvals, None).is_err());
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn bundled_thor_music_approvals_preserve_legacy_voice_and_version_gate() {
        let (runtime, mut contract, _) = fixtures();
        let approvals: Value = serde_json::from_str(include_str!("../../config/voice-calibrations.json")).unwrap();
        let voice = &approvals["approvals"][0];
        contract["provider"] = voice["provider"].clone();
        contract["cpu_models"] = voice["cpu_models"].clone();
        contract["_execution_hardware"] = voice["hardware"].clone();
        contract["_application_version"] = json!("0.0.21");
        contract["input_limits_version"] = json!(1);
        contract["mixed_input_limits_version"] = json!(1);
        contract["execution_settings"] = json!({"music":approvals["approvals"][5]["music"]});
        let mut sync = Vec::new();
        let mut async_models = Vec::new();
        for approval in approvals["approvals"].as_array().unwrap().iter().filter(|a| a["hardware"] == voice["hardware"]) {
            for model in approval["sync_models"].as_array().unwrap() {
                sync.push(json!({"id":model,"task_type":"model.infer","kind":if approval["music"].is_object() {"music"} else {"voice"}}));
            }
            for model in approval["async_models"].as_array().unwrap() {
                async_models.push(json!({"id":model,"task_type":"voice.generate"}));
            }
        }
        contract["_ready_sync"] = json!(sync.iter().map(|r| r["id"].clone()).collect::<Vec<_>>());
        contract["_ready_async"] = json!(async_models.iter().map(|r| r["id"].clone()).collect::<Vec<_>>());
        contract["sync_models"] = json!(sync);
        contract["async_models"] = json!(async_models);
        let state = build(&runtime, &contract, &approvals, None).unwrap();
        assert_eq!(state["binding"]["models"].as_object().unwrap().len(), 11);
        assert_eq!(state["binding"]["model_limits"].as_object().unwrap().len(), 4);
        let legacy = state["binding"]["legacy_unbounded_models"].as_array().unwrap();
        assert_eq!(legacy.len(), 7);
        assert!(legacy.contains(&json!("kokoro-multilang")));
        assert!(legacy.contains(&json!("kokoro-82m-q8")));
        assert!(!legacy.contains(&json!("musicgen-small")));
        contract["_application_version"] = json!("0.0.20");
        let old = build(&runtime, &contract, &approvals, None).unwrap();
        assert_eq!(old["binding"]["models"].as_object().unwrap().len(), 7);
        assert!(old["binding"].get("model_limits").is_none());
        assert!(old["binding"].get("legacy_unbounded_models").is_none());
    }

    #[test]
    fn voice_approval_uses_saved_management_capacity() {
        let (runtime, contract, mut approvals) = fixtures();
        approvals["approvals"][0]["resources"]["cpu_millicores"] = json!(4500);
        let mut managed = ManagedConfig::default();
        managed.nodes.push(serde_json::from_value(json!({"id":"local","capacities":{"cpu_millicores":4000,"memory_mib":16384,"gpu_slots":1,"gpu_memory_mib":16384}})).unwrap());
        runtime.set_management(managed.clone()).unwrap();
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        managed.nodes[0].capacities.cpu_millicores = 5000;
        runtime.set_management(managed).unwrap();
        assert!(build(&runtime, &contract, &approvals, None).is_ok());
    }

    #[test]
    fn management_and_voice_bindings_preserve_each_other() {
        let (runtime, contract, approvals) = fixtures();
        let state = build(&runtime, &contract, &approvals, None).unwrap();
        runtime.install(&state).unwrap();
        let initial = runtime.snapshot().executors.len();
        let mut managed = ManagedConfig::default();
        managed.nodes.push(
            serde_json::from_value(
                json!({"id":"remote","capacities":{"cpu_millicores":4000,"memory_mib":8192}}),
            )
            .unwrap(),
        );
        managed.executors.push(serde_json::from_value(json!({"id":"remote.task","node_id":"remote","app_id":"example","contract_version":"1","task_type":"example.run","resources":{"cpu_millicores":1000,"memory_mib":1024}})).unwrap());
        runtime.set_management(managed).unwrap();
        assert_eq!(runtime.snapshot().executors.len(), initial + 1);
        runtime.install(&state).unwrap();
        assert_eq!(runtime.snapshot().executors.len(), initial + 1);
        assert!(
            runtime
                .snapshot()
                .nodes
                .iter()
                .any(|node| node.id == "remote")
        );
    }
    #[test]
    fn binding_is_scoped_idempotent_and_revision_survives_restart() {
        let (runtime, contract, mut approvals) = fixtures();
        let first = build(&runtime, &contract, &approvals, None).unwrap();
        let same = build(&runtime, &contract, &approvals, Some(&first)).unwrap();
        assert_eq!(first, same);
        assert_eq!(first["binding"]["binding_revision"], 1);
        assert_eq!(
            first["binding"]["models"]["asr"],
            first["binding"]["models"]["speaker"]
        );
        assert_ne!(first["binding"]["token"], first["gateway"]["token"]);
        assert!(
            first["principals"][0]["permit_executors"]
                .as_array()
                .unwrap()
                .contains(&first["binding"]["gateway_executors"]["asr"])
        );
        runtime.install(&first).unwrap();
        assert_eq!(runtime.snapshot().executors.len(), 3);
        approvals["approvals"][0]["max_input_bytes"] = json!(524288);
        let changed = build(&runtime, &contract, &approvals, Some(&first)).unwrap();
        assert_eq!(changed["binding"]["binding_revision"], 2);
        assert_eq!(changed["binding"]["token"], first["binding"]["token"]);
    }
    #[test]
    fn declarations_never_approve_resources_or_redirect_credentials() {
        let (runtime, mut contract, approvals) = fixtures();
        let mut discovery = json!({"source":ORIGIN,"observed_at":Utc::now(),"scheduler":contract});
        assert!(declaration(&discovery).is_ok());
        discovery["scheduler"]["executor_base_url"] = json!("http://attacker.invalid");
        assert!(declaration(&discovery).is_err());
        discovery["scheduler"] = contract.clone();
        discovery["source"] = json!("http://host.docker.internal:22700");
        assert!(declaration(&discovery).is_err());
        contract["provider"] = json!("cuda");
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        assert!(
            build(
                &runtime,
                &contract,
                &json!({"protocol_version":1,"approvals":[]}),
                None
            )
            .is_err()
        );
    }
    #[test]
    fn unmeasured_and_underbudget_models_remain_denied() {
        let (runtime, contract, mut approvals) = fixtures();
        approvals["approvals"][0]["calibration"] = json!({});
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        approvals["approvals"][0]["calibration"] =
            json!({"evidence_id":"test","peak_memory_mib":4096});
        assert!(build(&runtime, &contract, &approvals, None).is_err());
        approvals["approvals"][0]["calibration"]["peak_memory_mib"] = json!(1024);
        approvals["approvals"][0]["sync_models"] = json!(["asr"]);
        assert!(build(&runtime, &contract, &approvals, None).is_err());
    }
    #[test]
    fn only_ready_models_are_bound_and_rotation_revokes_old_snapshots() {
        let (runtime, mut contract, mut approvals) = fixtures();
        contract["_ready_sync"] = json!(["asr"]);
        contract["_ready_async"] = json!([]);
        let first = build(&runtime, &contract, &approvals, None).unwrap();
        assert!(first["binding"]["models"].get("speaker").is_none());
        assert!(first["binding"]["models"].get("tts").is_none());
        assert_eq!(first["binding"]["secondary_models"]["asr"], json!([]));
        let before = runtime.snapshot();
        runtime.install(&first).unwrap();
        assert!(!runtime.is_current(&before));
        approvals["credential_epoch"] = json!(1);
        let rotated = build(&runtime, &contract, &approvals, Some(&first)).unwrap();
        assert_eq!(rotated["binding"]["binding_revision"], 2);
        assert_ne!(rotated["binding"]["token"], first["binding"]["token"]);
        approvals["credential_epoch"] = json!(0);
        assert!(build(&runtime, &contract, &approvals, Some(&rotated)).is_err());
    }
}

#[cfg(test)]
#[path = "voice_unclaimed_tests.rs"]
mod unclaimed_tests;
