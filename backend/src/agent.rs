use crate::model::{Command, Executor};
use anyhow::{Context, ensure};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use uuid::Uuid;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Adapter {
    id: String,
    url: String,
    token: String,
    #[serde(default)]
    health_url: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    controller: String,
    #[serde(default)]
    ca_certificate: Option<PathBuf>,
    node_id: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    enrollment_token: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    journal: PathBuf,
    executors: Vec<Adapter>,
    #[serde(default)]
    registrations: Vec<Executor>,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    session_id: String,
    command: Option<Command>,
}
fn save(path: &Path, journal: &Journal) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)?;
    serde_json::to_writer(&file, journal)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)?;
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct EnrollmentIdentity {
    enrollment_hash: String,
    token: String,
    registered: bool,
}
fn save_identity(path: &Path, identity: &EnrollmentIdentity) -> anyhow::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let temporary = path.with_extension(format!("{}.pending", Uuid::new_v4().simple()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(identity)?)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    std::fs::File::open(path.parent().context("missing identity directory")?)?.sync_all()?;
    Ok(())
}
async fn post(c: &Client, url: &str, token: &str, body: Value) -> anyhow::Result<Value> {
    let response = c.post(url).bearer_auth(token).json(&body).send().await?;
    ensure!(
        response.status().is_success(),
        "remote operation rejected ({})",
        response.status()
    );
    let data: Value = response.json().await?;
    Ok(data.get("data").cloned().unwrap_or(data))
}
#[derive(Clone, Copy)]
pub(crate) struct CpuCounters {
    total: u64,
    idle: u64,
}
pub(crate) type CpuSnapshot = std::collections::BTreeMap<String, CpuCounters>;
fn parse_cpu_sample(stat: &str) -> CpuSnapshot {
    stat.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let id = fields.next()?;
            if id != "cpu"
                && !id
                    .strip_prefix("cpu")
                    .is_some_and(|n| !n.is_empty() && n.parse::<u32>().is_ok())
            {
                return None;
            }
            let values = fields
                .take(8)
                .map(str::parse::<u64>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            if values.len() < 5 {
                return None;
            }
            Some((
                id.to_owned(),
                CpuCounters {
                    total: values.iter().sum(),
                    idle: values[3] + values[4],
                },
            ))
        })
        .collect()
}
fn cpu_sample() -> Option<CpuSnapshot> {
    Some(parse_cpu_sample(
        &std::fs::read_to_string("/proc/stat").ok()?,
    ))
}
fn cpu_load(previous: Option<&CpuSnapshot>, current: &CpuSnapshot, id: &str) -> Option<f64> {
    let old = previous?.get(id)?;
    let now = current.get(id)?;
    let delta = now.total.checked_sub(old.total)?;
    let idle = now.idle.checked_sub(old.idle)?;
    (delta > 0 && idle <= delta).then(|| 100.0 * (delta - idle) as f64 / delta as f64)
}
fn jetson_load_percent(raw: &str) -> Option<u64> {
    let load = raw.trim().parse::<u64>().ok()?;
    (load <= 1000).then_some((load + 5) / 10)
}
fn jetson_gpu_utilization() -> Option<u64> {
    // nvgpu-based Jetsons report N/A through nvidia-smi. The kernel's load
    // counter is per-mille; do not infer zero when it cannot be read.
    for path in [
        "/sys/devices/platform/17000000.gpu/load",
        "/sys/devices/gpu.0/load",
    ] {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Some(percent) = jetson_load_percent(&raw) {
                return Some(percent);
            }
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::{cpu_load, jetson_load_percent, parse_cpu_sample};
    #[test]
    fn per_core_usage_is_measured_individually_and_reset_is_unknown() {
        let before = parse_cpu_sample(
            "cpu 100 0 0 100 0 0 0 0\ncpu0 100 0 0 0 0 0 0 0\ncpu1 0 0 0 100 0 0 0 0\n",
        );
        let after = parse_cpu_sample(
            "cpu 200 0 0 200 0 0 0 0\ncpu0 200 0 0 0 0 0 0 0\ncpu1 0 0 0 200 0 0 0 0\n",
        );
        assert_eq!(cpu_load(Some(&before), &after, "cpu"), Some(50.0));
        assert_eq!(cpu_load(Some(&before), &after, "cpu0"), Some(100.0));
        assert_eq!(cpu_load(Some(&before), &after, "cpu1"), Some(0.0));
        assert_eq!(cpu_load(None, &after, "cpu1"), None);
        assert_eq!(cpu_load(Some(&after), &before, "cpu0"), None);
    }
    #[test]
    fn jetson_load_is_per_mille_and_invalid_values_stay_unknown() {
        assert_eq!(jetson_load_percent("0\n"), Some(0));
        assert_eq!(jetson_load_percent("475\n"), Some(48));
        assert_eq!(jetson_load_percent("1000\n"), Some(100));
        assert_eq!(jetson_load_percent("1001\n"), None);
        assert_eq!(jetson_load_percent("[N/A]\n"), None);
    }

    #[tokio::test]
    async fn timed_out_gpu_probe_is_killed_and_reaped_before_returning() {
        let marker = std::env::temp_dir().join(format!("gpu-probe-{}", uuid::Uuid::new_v4()));
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "echo $$ > \"$1\"; exec sleep 60", "gpu-probe"]);
        command.arg(&marker);
        assert!(
            super::gpu_output(command, std::time::Duration::from_millis(200))
                .await
                .is_none()
        );
        let pid: i32 = std::fs::read_to_string(&marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        std::fs::remove_file(marker).unwrap();
    }

    #[tokio::test]
    async fn completed_gpu_probe_returns_its_output() {
        let mut command = tokio::process::Command::new("printf");
        command.arg("gpu sample\n");
        assert_eq!(
            super::gpu_output(command, std::time::Duration::from_secs(1))
                .await
                .as_deref(),
            Some("gpu sample")
        );
    }
}
pub(crate) fn host_probe(previous: &mut Option<CpuSnapshot>) -> Value {
    let observed_at = chrono::Utc::now();
    let current = cpu_sample().unwrap_or_default();
    let cpu_used_percent = cpu_load(previous.as_ref(), &current, "cpu");
    let mut cpu_cores = current
        .keys()
        .filter_map(|id| {
            let number = id.strip_prefix("cpu")?.parse::<u32>().ok()?;
            Some(json!({"id":number,"utilization_percent":cpu_load(previous.as_ref(),&current,id)}))
        })
        .collect::<Vec<_>>();
    cpu_cores.sort_by_key(|core| core["id"].as_u64());
    let host_cores = cpu_cores.len();
    *previous = Some(current);
    let mem = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let read = |key: &str| {
        mem.lines()
            .find(|line| line.starts_with(key))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            / 1024
    };
    json!({"observed_at":observed_at,"cpu_logical_count":host_cores,"cpu_count_source":"host-proc-stat","cpu_used_percent":cpu_used_percent,"cpu_cores":cpu_cores,"arch":std::env::consts::ARCH,"memory_total_mib":read("MemTotal:"),"memory_available_mib":read("MemAvailable:"),"gpus":[],"agent_version":"0.1.0"})
}

async fn gpu_output(
    mut command: tokio::process::Command,
    timeout: std::time::Duration,
) -> Option<String> {
    use tokio::io::AsyncReadExt;
    command
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().ok()?;
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => status.ok()?,
        Err(_) => {
            let _ = child.start_kill();
            // A driver syscall can remain uninterruptible after SIGKILL. Keep
            // ownership until reaped: never accumulate another nvidia-smi.
            let _ = child.wait().await;
            return None;
        }
    };
    if !status.success() {
        return None;
    }
    let mut bytes = Vec::new();
    child
        .stdout
        .take()?
        .take(65_536)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).trim().to_owned())
}

pub(crate) async fn probe(previous: &mut Option<CpuSnapshot>) -> Value {
    let mut report = host_probe(previous);
    let mut gpu_query = tokio::process::Command::new("nvidia-smi");
    gpu_query.args([
        "--query-gpu=uuid,name,memory.total,memory.free,utilization.gpu",
        "--format=csv,noheader,nounits",
    ]);
    let gpu = gpu_output(gpu_query, std::time::Duration::from_secs(5)).await;
    let mut gpus = gpu.map(|lines| lines.lines().filter_map(|line| {
        let columns: Vec<_> = line.split(',').map(str::trim).collect();
        if columns.len() != 5 { return None; }
        Some(json!({"uuid":columns[0],"name":columns[1],"total_mib":columns[2].parse::<u64>().ok(),"free_mib":columns[3].parse::<u64>().ok(),"utilization_percent":columns[4].parse::<u64>().ok(),"memory_source":"nvidia-smi"}))
    }).collect::<Vec<_>>()).unwrap_or_default();
    if gpus.len() <= 1 {
        let missing_memory = gpus.first().is_none_or(|gpu| {
            gpu["total_mib"].as_u64().is_none() || gpu["free_mib"].as_u64().is_none()
        });
        if let Ok(Some(info)) =
            tokio::task::spawn_blocking(move || crate::gpu::telemetry(missing_memory)).await
        {
            if gpus.is_empty() {
                gpus.push(json!({"total_mib":null,"free_mib":null,"utilization_percent":null}));
            }
            if gpus[0]["name"].is_null() {
                gpus[0]["name"] = json!(info.name);
            }
            if let Some(integrated) = info.integrated {
                gpus[0]["memory_type"] = json!(if integrated { "unified" } else { "dedicated" });
                gpus[0]["memory_type_source"] = json!("cuda-device-attribute");
                if integrated {
                    gpus[0]["memory_pool_id"] = json!("system");
                }
            }
            if let Some((total, free)) = info.memory {
                gpus[0]["total_mib"] = json!(total);
                gpus[0]["free_mib"] = json!(free);
                gpus[0]["memory_source"] = json!("cuda-driver");
            }
        }
    }
    if gpus.len() == 1 && gpus[0]["utilization_percent"].as_u64().is_none() {
        if let Some(percent) = jetson_gpu_utilization() {
            gpus[0]["utilization_percent"] = json!(percent);
            gpus[0]["utilization_source"] = json!("jetson-sysfs");
        }
    }
    report["gpus"] = json!(gpus);
    report
}
pub async fn run(path: PathBuf) -> anyhow::Result<()> {
    let mut cfg: Config = serde_json::from_slice(&std::fs::read(&path)?)?;

    let mut builder = Client::builder().timeout(std::time::Duration::from_secs(8));
    if let Some(path) = &cfg.ca_certificate {
        let bytes = std::fs::read(path)?;
        ensure!(bytes.len() <= 1_048_576, "CA certificate is too large");
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&bytes)?);
    }
    let client = builder.build()?;
    let mut journal = if cfg.journal.exists() {
        serde_json::from_slice(&std::fs::read(&cfg.journal)?)?
    } else {
        Journal {
            session_id: Uuid::new_v4().to_string(),
            command: None,
        }
    };
    save(&cfg.journal, &journal)?;
    let identity_path = cfg.journal.with_extension("identity.json");
    let mut enrollment = if let Some(ticket) = &cfg.enrollment_token {
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            ticket.len() >= 32 && !ticket.contains("REPLACE"),
            "invalid enrollment credential"
        );
        let digest = format!("{:x}", Sha256::digest(ticket.as_bytes()));
        let old = if identity_path.exists() {
            ensure!(
                std::fs::symlink_metadata(&identity_path)?.is_file()
                    && std::fs::metadata(&identity_path)?.permissions().mode() & 0o077 == 0,
                "worker identity must be a private regular file"
            );
            Some(serde_json::from_slice::<EnrollmentIdentity>(
                &std::fs::read(&identity_path)?,
            )?)
        } else {
            None
        };
        let identity = match old {
            Some(old) if old.enrollment_hash == digest => old,
            _ => {
                ensure!(
                    journal.command.is_none(),
                    "finish existing execution before changing worker identity"
                );
                EnrollmentIdentity {
                    enrollment_hash: digest,
                    token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
                    registered: false,
                }
            }
        };
        save_identity(&identity_path, &identity)?;
        cfg.token = identity.token.clone();
        Some(identity)
    } else {
        None
    };
    ensure!(
        cfg.token.len() >= 32 && !cfg.token.contains("REPLACE"),
        "invalid worker credential"
    );
    // Registration proposals have their own loop and cannot delay execution leases.
    let proposal_path = path.clone();
    let proposal_client = client.clone();
    let proposal_base = cfg.controller.trim_end_matches('/').to_owned();
    let proposal_node = cfg.node_id.clone();
    let proposal_token = cfg.token.clone();
    tokio::spawn(async move {
        loop {
            if let Ok(bytes) = std::fs::read(&proposal_path) {
                if let Ok(latest) = serde_json::from_slice::<Config>(&bytes) {
                    if latest.node_id == proposal_node && !latest.registrations.is_empty() {
                        let _ = post(
                            &proposal_client,
                            &format!("{proposal_base}/workers/{proposal_node}/executors"),
                            &proposal_token,
                            json!({"executors":latest.registrations}),
                        )
                        .await;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });
    eprintln!("V-Scheduler Agent started for {}", cfg.node_id);
    // GPU driver probes can stall during model/context initialization. Keep
    // telemetry in one background sampler so it cannot starve execution leases.
    // Preserve the sample timestamp; stale data denies new work, never invents
    // free resources. A stalled driver creates no additional sampling threads.
    let sampled = std::sync::Arc::new(tokio::sync::RwLock::new(json!({})));
    let writer = sampled.clone();
    let health_client = client.clone();
    let adapters_path = path.clone();
    tokio::spawn(async move {
        let mut previous = cpu_sample();
        loop {
            let mut report = probe(&mut previous).await;
            let mut executor_health = serde_json::Map::new();
            let adapters = std::fs::read(&adapters_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Config>(&bytes).ok())
                .map(|config| config.executors)
                .unwrap_or_default();
            for adapter in &adapters {
                if let Some(url) = &adapter.health_url {
                    let healthy = health_client
                        .get(url)
                        .timeout(std::time::Duration::from_secs(2))
                        .send()
                        .await
                        .is_ok_and(|response| response.status().is_success());
                    executor_health.insert(
                        adapter.id.clone(),
                        json!({"healthy":healthy,"observed_at":chrono::Utc::now()}),
                    );
                }
            }
            report["executor_health"] = Value::Object(executor_health);
            *writer.write().await = report;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
    loop {
        if let Some(identity) = &mut enrollment {
            if !identity.registered {
                let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                    .unwrap_or_else(|_| "unknown".into());
                let result=post(&client,&format!("{}/workers/register",cfg.controller.trim_end_matches('/')),cfg.enrollment_token.as_deref().unwrap_or_default(),json!({"protocol_version":1,"node_id":cfg.node_id,"display_name":cfg.display_name.as_deref().unwrap_or(&cfg.node_id),"instance_id":journal.session_id,"boot_id":boot.trim(),"session_id":journal.session_id,"worker_token":identity.token})).await;
                if result.is_ok() {
                    identity.registered = true;
                    save_identity(&identity_path, identity)?;
                } else {
                    eprintln!("worker enrollment unavailable; waiting for valid invitation");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            }
        }
        // 只热更新执行器，保留节点身份、会话及当前命令；原子交接不会中断在途任务。
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(updated) = serde_json::from_slice::<Config>(&bytes) {
                if updated.node_id == cfg.node_id
                    && (updated.token == cfg.token
                        || (cfg.enrollment_token.is_some()
                            && updated.enrollment_token == cfg.enrollment_token))
                {
                    cfg.executors = updated.executors;
                }
            }
        }
        let base = cfg.controller.trim_end_matches('/');
        let report = sampled.read().await.clone();
        let heartbeat = post(
            &client,
            &format!("{base}/workers/{}/heartbeat", cfg.node_id),
            &cfg.token,
            json!({"session_id":journal.session_id,"report":report}),
        )
        .await;
        if let Err(e) = heartbeat {
            eprintln!("heartbeat unavailable: {e}");
        }
        // The executor watchdog is independent of this Agent process and network.
        if journal.command.is_none() && !cfg.executors.is_empty() {
            if let Ok(v) = post(
                &client,
                &format!("{base}/workers/{}/commands/claim", cfg.node_id),
                &cfg.token,
                json!({"session_id":journal.session_id}),
            )
            .await
            {
                if let Some(value) = v["commands"].as_array().and_then(|c| c.first()) {
                    journal.command = Some(serde_json::from_value(value.clone())?);
                    save(&cfg.journal, &journal)?;
                }
            }
        }
        if let Some(cmd) = journal.command.clone() {
            let adapter = cfg
                .executors
                .iter()
                .find(|e| e.id == cmd.executor_id)
                .context("command has unknown adapter; reservation remains held")?;
            let lease = post(
                &client,
                &format!("{base}/attempts/{}/renew", cmd.attempt_id),
                &cfg.token,
                json!({"session_id":journal.session_id,"fence":cmd.fence}),
            )
            .await;
            let action = if lease.is_ok() { "start" } else { "cancel" };
            let body = json!({"attempt_id":cmd.attempt_id,"task_id":cmd.task_id,"fence":cmd.fence,"lease_seconds":cmd.lease_seconds,"parameters":cmd.parameters});
            let url = format!(
                "{}/scheduler/executions/{}/{}",
                adapter.url.trim_end_matches('/'),
                cmd.attempt_id,
                action
            );
            if let Err(e) = post(&client, &url, &adapter.token, body.clone()).await {
                eprintln!("executor {action} unavailable: {e}");
            }
            let inspect = format!(
                "{}/scheduler/executions/{}/inspect",
                adapter.url.trim_end_matches('/'),
                cmd.attempt_id
            );
            if let Ok(v) = post(&client, &inspect, &adapter.token, body).await {
                let status = v["status"].as_str().unwrap_or("unknown");
                let stopped = v["stopped"].as_bool().unwrap_or(false);
                let terminal = ["succeeded", "failed", "cancelled"].contains(&status) && stopped;
                let report=post(&client,&format!("{base}/attempts/{}/report",cmd.attempt_id),&cfg.token,json!({"session_id":journal.session_id,"fence":cmd.fence,"status":status,"stopped":stopped,"progress":v["progress"],"stage":v["stage"],"message":v["message"],"result":v["result"]})).await;
                if terminal && report.is_ok() {
                    journal.command = None;
                    save(&cfg.journal, &journal)?;
                }
            } else {
                eprintln!("execution state unavailable; reservation retained");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}
