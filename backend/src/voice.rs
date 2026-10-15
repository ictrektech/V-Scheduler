//! 同一 VOS 内 V-Voice 的受限授权交接。发现声明不能替代资源批准。
use crate::model::{Config, Executor, Principal, Resources};
use anyhow::{Context, ensure};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
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

#[derive(Clone)]
pub struct Runtime {
    base: Arc<Config>,
    current: Arc<RwLock<Arc<Config>>>,
    pub ready: Arc<AtomicBool>,
}
impl Runtime {
    pub fn restore(path: &Path, base: Config) -> anyhow::Result<Self> {
        let runtime = Self {
            base: Arc::new(base.clone()),
            current: Arc::new(RwLock::new(Arc::new(base))),
            ready: Arc::new(AtomicBool::new(false)),
        };
        let state = path
            .parent()
            .context("missing config directory")?
            .join("voice-binding.json");
        if state.exists() {
            runtime.install(&read_private(&state)?)?;
        }
        Ok(runtime)
    }
    pub fn snapshot(&self) -> Arc<Config> {
        self.current.read().expect("configuration lock").clone()
    }
    pub fn is_current(&self, config: &Arc<Config>) -> bool {
        Arc::ptr_eq(config, &self.snapshot())
    }
    fn install(&self, state: &Value) -> anyhow::Result<()> {
        let mut config = (*self.base).clone();
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
        *self.current.write().expect("configuration lock") = Arc::new(config);
        Ok(())
    }
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
    let node_id = runtime
        .base
        .local_agent
        .as_ref()
        .and_then(|v| v["node_id"].as_str())
        .context("local execution node required")?;
    let node = runtime
        .base
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .context("unknown local node")?;
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
    let mut policies = serde_json::Map::new();
    let mut executors = Vec::new();
    let mut adapters = Vec::new();
    let mut gateway_ids = Vec::new();
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
                        .any(|row| row["id"] == *id && row["task_type"] == task_type)
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
                    policies.insert(format!("app-v-voice/{model}"),json!({"mode":"permit","executor_id":gateway,"completion":"synchronous","max_input_bytes":max_input,"limits":approval["limits"],"secondary_models":secondary}));
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
    let mut state = json!({"credential_epoch":epoch,"binding":{"protocol_version":1,"binding_revision":1,"app_id":APP,"url":URL,"token":token,"executor_token":executor_token,"upstream_url":UPSTREAM,"models":models,"gateway_executors":gateway_executors,"secondary_models":secondary_models},
        "principals":[{"token":token,"role":"submitter","app_id":APP,"permit_executors":gateway_ids},{"token":gateway_token,"role":"submitter","app_id":"v-scheduler.voice-gateway"}],
        "executors":executors,"adapters":adapters,
        "gateway":{"url":URL,"token":gateway_token,"require_model_policy":true,"models":policies}});
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

async fn reconcile(path: &Path, runtime: &Runtime, db: &PgPool) -> anyhow::Result<()> {
    let directory = path.parent().context("missing config directory")?;
    let gateway = std::env::var("SCHEDULER_GATEWAY_DATA")
        .unwrap_or_else(|_| "/gateway-data".into());
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
    let identity = client
        .get(format!(
            "{}/v1000/oauth2/userinfo",
            vos.trim_end_matches('/')
        ))
        .bearer_auth(token.trim())
        .send()
        .await?;
    ensure!(
        identity.status().is_success(),
        "application identity verification unavailable"
    );
    let identity: Value = identity.json().await?;
    ensure!(
        identity["sub"] == "com.ictrek.v-scheduler" && identity["enable"] == 1,
        "Scheduler application identity required"
    );
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
    let node_id = runtime
        .base
        .local_agent
        .as_ref()
        .and_then(|v| v["node_id"].as_str())
        .context("missing local node")?;
    let report: Value = sqlx::query_scalar("SELECT report FROM vrs_nodes WHERE id=$1")
        .bind(node_id)
        .fetch_one(db)
        .await?;
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
    let state = build(runtime, &contract, &approvals, old.as_ref())?;
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
        };
        let contract = json!({"protocol_version":1,"app_id":APP,"executor_base_url":ORIGIN,"upstream_url":UPSTREAM,"binding_endpoint":"/api/v1/scheduler/binding","provider":"cpu","cpu_models":[],"num_threads":4,"max_concurrent_inference":2,"_execution_hardware":[],"_ready_sync":["asr","speaker"],"_ready_async":["tts"],"sync_models":[{"id":"asr","task_type":"model.infer"},{"id":"speaker","task_type":"model.infer"}],"async_models":[{"id":"tts","task_type":"voice.generate","weight_bytes":1}]});
        let approvals = json!({"protocol_version":1,"approvals":[{"id":"measured","architecture":std::env::consts::ARCH,"provider":"cpu","cpu_models":[],"num_threads":4,"max_concurrent_inference":2,"hardware":[],"calibration":{"evidence_id":"unit-fixture","peak_memory_mib":1024},"resources":{"cpu_millicores":4000,"memory_mib":2048},"max_input_bytes":1048576,"limits":{},"sync_models":["asr","speaker"],"async_models":["tts"],"secondary_models":{"asr":["speaker"]}}]});
        (runtime, contract, approvals)
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
