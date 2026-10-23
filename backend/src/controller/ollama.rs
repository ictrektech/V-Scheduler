//! Installed Ollama backends belong to this application, not third-party enrollment.
//! Register scoped inference identities automatically; retain the normal permit ledger.
use super::*;
use anyhow::Context;
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, time::Duration};

pub(super) const OWNER: &str = "com.ictrek.v-scheduler.ollama";

pub(super) fn binding_path(directory: &Path, model: &str) -> PathBuf {
    directory
        .join("ollama-bindings")
        .join(format!("{:x}.json", Sha256::digest(model.as_bytes())))
}

pub(super) fn directory() -> PathBuf {
    PathBuf::from(
        std::env::var("SCHEDULER_GATEWAY_DATA").unwrap_or_else(|_| "/gateway-data".into()),
    )
}

pub(super) fn origin(role: &str) -> String {
    format!("http://v-scheduler-ollama-{role}:11535/v1")
}

pub(super) fn save(path: &Path, value: &Value) -> anyhow::Result<()> {
    let directory = path.parent().context("binding directory missing")?;
    std::fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(".{}.pending", Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    let _ = std::fs::remove_file(temporary);
    Ok(result?)
}

fn resources(bytes: i64, capacity: &Resources, unified: bool) -> Resources {
    // Default to one compute job on this node. Model-size-derived memory is
    // a conservative starting estimate, not a claim of a measured peak.
    let footprint = bytes.saturating_add(1048575) / 1048576;
    let memory = footprint.saturating_mul(2).saturating_add(1024);
    Resources {
        cpu_millicores: capacity.cpu_millicores,
        memory_mib: memory,
        gpu_slots: capacity.gpu_slots,
        gpu_memory_mib: if unified || capacity.gpu_slots == 0 {
            0
        } else {
            memory
        },
    }
}

pub(super) struct NodeBudget {
    pub capacity: Resources,
    pub baseline: Resources,
    pub reserve: Resources,
    pub report: Value,
}

pub(super) async fn node_budget(a: &App, node: &str) -> anyhow::Result<NodeBudget> {
    let row = sqlx::query(
        "SELECT capacities,resident_baseline,safety_reserve,report,management_enabled FROM vrs_nodes WHERE id=$1",
    )
    .bind(node)
    .fetch_one(&a.db)
    .await?;
    ensure!(
        row.get::<bool, _>("management_enabled"),
        "local node disabled"
    );
    Ok(NodeBudget {
        capacity: serde_json::from_value(row.get("capacities"))?,
        baseline: serde_json::from_value(row.get("resident_baseline"))?,
        reserve: serde_json::from_value(row.get("safety_reserve"))?,
        report: row.get("report"),
    })
}

// The automatic estimate is the restore target, so calibration and the
// periodic registration share one calculation.
pub(super) fn default_cost(
    capacity: &Resources,
    baseline: &Resources,
    reserve: &Resources,
    report: &Value,
    bytes: i64,
) -> anyhow::Result<Resources> {
    let gpu = report["gpus"].as_array().and_then(|gpus| gpus.first());
    ensure!(
        report_fresh(report) && report["observed_at"].is_string(),
        "resource sample not ready"
    );
    // Do not publish a CPU-only estimate while initial GPU discovery is pending.
    ensure!(
        capacity.gpu_slots == 0 || gpu.is_some(),
        "GPU sample not ready"
    );
    let unified = gpu.is_some_and(|gpu| gpu["memory_type"] == "unified");
    let mut cost = resources(bytes, capacity, unified);
    cost.cpu_millicores = cost.cpu_millicores.saturating_sub(baseline.cpu_millicores);
    ensure!(
        cost.cpu_millicores > 0 && cost.fits(baseline, capacity),
        "model exceeds local node budget"
    );
    let physical_cpu = report["cpu_logical_count"]
        .as_i64()
        .context("CPU sample unavailable")?
        .saturating_mul(1000);
    cost.cpu_millicores = cost.cpu_millicores.min(
        physical_cpu
            .saturating_sub(reserve.safety_floor(report).cpu_millicores)
            .saturating_sub(baseline.cpu_millicores),
    );
    ensure!(cost.cpu_millicores > 0, "local compute budget unavailable");
    Ok(cost)
}

pub(super) fn automatic_id(
    model: &str,
    node: &str,
    cost: &Resources,
    origin: &str,
    bytes: i64,
) -> anyhow::Result<String> {
    let fingerprint = json!([model, node, cost, origin, "automatic-v2", bytes]);
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&fingerprint)?));
    Ok(format!("builtin-ollama-auto-{}", &digest[..24]))
}

async fn register(
    a: &App,
    directory: &Path,
    role: &str,
    name: &str,
    bytes: i64,
) -> anyhow::Result<()> {
    ensure!(
        bytes > 0 && !name.is_empty() && name.len() <= 512,
        "invalid installed model"
    );
    let _guard = a.voice_config.management_lock.lock().await;
    let current = a.voice_config.snapshot();
    let node = current
        .local_agent
        .as_ref()
        .and_then(|local| local["node_id"].as_str())
        .context("local node not ready")?;
    let budget = node_budget(a, node).await?;
    let cost = default_cost(
        &budget.capacity,
        &budget.baseline,
        &budget.reserve,
        &budget.report,
        bytes,
    )?;
    let model = format!("v-scheduler/ollama-{role}/{name}");
    let path = binding_path(directory, &model);
    let origin = origin(role);
    let executor_id = automatic_id(&model, node, &cost, &origin, bytes)?;
    if path.exists() {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file()
                && metadata.permissions().mode() & 0o077 == 0
                && metadata.len() <= 262144,
            "invalid private binding"
        );
        let old: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let old_id = old["models"][&model]["executor_id"].as_str().unwrap_or("");
        // An explicit administrator adjustment always wins over automatic defaults.
        if !old_id.starts_with("builtin-ollama-auto-") || old_id == executor_id {
            return Ok(());
        }
    }
    let definition = json!({"id":executor_id,"contract_version":"1","task_type":"model.infer","node_id":node,"app_id":OWNER,
        "resources":cost,"upstream_urls":[origin],"placements":[],"retry_mode":"none","max_attempts":1});
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let identity = Uuid::new_v4().to_string();
    let mut tx = a.db.begin().await?;
    sqlx::query("INSERT INTO vrs_executor_settings(id,node_id,definition,state) VALUES($1,$2,$3,'enabled') ON CONFLICT(id) DO NOTHING")
        .bind(&executor_id).bind(node).bind(&definition).execute(&mut *tx).await?;
    let enabled: bool = sqlx::query_scalar(
        "SELECT state='enabled' AND definition=$2 FROM vrs_executor_settings WHERE id=$1",
    )
    .bind(&executor_id)
    .bind(&definition)
    .fetch_one(&mut *tx)
    .await?;
    ensure!(enabled, "automatic executor was disabled or modified");
    sqlx::query("INSERT INTO vrs_service_identities(id,app_id,token_hash,permit_executors) VALUES($1,$2,$3,$4)")
        .bind(&identity).bind(OWNER).bind(format!("{:x}",Sha256::digest(token.as_bytes()))).bind(json!([executor_id])).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO vrs_management_events(actor,action,resource) VALUES($1,'ollama.auto.register',$2)")
        .bind(OWNER).bind(&model).execute(&mut *tx).await?;
    tx.commit().await?;
    management::restore(&a.db, &a.voice_config).await?;
    // Publish only after the controller recognizes this executor and identity.
    save(
        &path,
        &json!({"url":"http://v-scheduler:38120/api/v1","token":token,"require_model_policy":true,
        "models":{model:{"mode":"permit","executor_id":executor_id,"completion":"synchronous","upstream_url":origin,
            "max_input_bytes":33554432,"native_model_size_bytes":bytes}}}),
    )?;
    Ok(())
}

pub(super) fn start(shared: SharedApp) {
    if std::env::var_os("VOS_APP_VERSION").is_none() {
        return;
    }
    tokio::spawn(async move {
        let directory = directory();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Ollama discovery client");
        loop {
            for role in ["qa", "embedding", "rerank"] {
                let response = client
                    .get(format!("http://v-scheduler-ollama-{role}:11434/api/tags"))
                    .send()
                    .await;
                if let Ok(response) = response {
                    if let Ok(tags) = response.error_for_status().map(|r| r.json::<Value>()) {
                        if let Ok(tags) = tags.await {
                            for model in tags["models"].as_array().into_iter().flatten().take(256) {
                                if let (Some(name), Some(size)) =
                                    (model["name"].as_str(), model["size"].as_i64())
                                {
                                    let a = App::from_ref(&shared);
                                    if register(&a, &directory, role, name, size).await.is_err() {
                                        eprintln!(
                                            "built-in Ollama automatic registration pending ({role})"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                let a = App::from_ref(&shared);
                if recover_stopped(&a, &client, role).await.is_err() {
                    // Do not log underlying HTTP errors or private payloads.
                    eprintln!("built-in Ollama stop verification pending ({role})");
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

fn stopped_receipt(value: &Value, model: &str, nonce: &str) -> bool {
    value["protocol_version"] == 1
        && value["model"] == model
        && value["nonce"] == nonce
        && value["stopped"] == true
        && value["native_exit_barrier"] == true
}

// Only credit memory freshly observed in this app's fixed native backend.
// The permit keeps its full reservation; any unmaterialized part and every
// other commitment still need free memory in addition to the safety floor.
pub(super) async fn resident_report(upstream: &str, model: &str, cost: &Resources, report: &Value) -> Value {
    let Some(role) = ["qa", "embedding", "rerank"].into_iter()
        .find(|role| upstream == origin(role)) else { return report.clone() };
    let result = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none()).build()?;
        let response = client.get(format!("http://v-scheduler-ollama-{role}:11434/api/ps")).send().await?;
        super::app_bindings::bounded_json(response).await
    }.await;
    pressure_report(report, cost, model, &result.unwrap_or(Value::Null))
}

fn pressure_report(report: &Value, cost: &Resources, model: &str, loaded: &Value) -> Value {
    let mut adjusted = report.clone();
    let Some(entry) = loaded["models"].as_array().and_then(|rows| rows.iter()
        .find(|row| row["name"] == model || row["model"] == model)) else { return adjusted };
    let (Some(size), Some(vram)) = (entry["size"].as_u64(), entry["size_vram"].as_u64()) else { return adjusted };
    if vram > size { return adjusted; }
    let unified = report["gpus"].as_array()
        .is_some_and(|rows| rows.iter().any(|gpu| gpu["memory_type"] == "unified"));
    let mib = |bytes: u64| (bytes / 1048576).min(i64::MAX as u64) as i64;
    let host_credit = if unified {
        mib(size).min(cost.memory_mib.saturating_add(cost.gpu_memory_mib))
    } else {
        mib(size - vram).min(cost.memory_mib)
    }.max(0);
    if let Some(available) = report["memory_available_mib"].as_i64().filter(|n| *n >= 0) {
        adjusted["memory_available_mib"] = json!(available.saturating_add(host_credit));
    }
    if !unified && report["gpus"][0]["memory_type"] == "dedicated" {
        if let Some(free) = report["gpus"][0]["free_mib"].as_i64().filter(|n| *n >= 0) {
            adjusted["gpus"][0]["free_mib"] = json!(free.saturating_add(mib(vram).min(cost.gpu_memory_mib).max(0)));
        }
    }
    adjusted
}

// Only this app's fixed native backends can supply this recovery proof.
// An expired lease, empty /api/ps or elapsed timeout alone never releases work.
async fn recover_stopped(a: &App, client: &reqwest::Client, role: &str) -> anyhow::Result<()> {
    let current = a.voice_config.snapshot();
    let Some(node) = current
        .local_agent
        .as_ref()
        .and_then(|v| v["node_id"].as_str())
    else {
        return Ok(());
    };
    let origin = format!("http://v-scheduler-ollama-{role}:11535/v1");
    let candidates = sqlx::query("SELECT id FROM vrs_permits WHERE app=$1 AND node=$2 AND state='suspect' AND parent_attempt IS NULL AND metadata->>'upstream_url'=$3 ORDER BY created_at LIMIT 8")
        .bind(OWNER).bind(node).bind(&origin).fetch_all(&a.db).await?;
    for candidate in candidates {
        let id: String = candidate.get("id");
        let mut tx = a.db.begin().await?;
        sqlx::query("SELECT id FROM vrs_nodes WHERE id=$1 FOR UPDATE")
            .bind(node)
            .fetch_one(&mut *tx)
            .await?;
        let Some(row) = sqlx::query("SELECT executor,metadata FROM vrs_permits WHERE id=$1 AND app=$2 AND node=$3 AND state='suspect' AND parent_attempt IS NULL FOR UPDATE")
            .bind(&id).bind(OWNER).bind(node).fetch_optional(&mut *tx).await? else { continue };
        let metadata: Value = row.get("metadata");
        let executor: String = row.get("executor");
        let Some(model) = metadata["model_id"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 512)
        else {
            continue;
        };
        if !(executor.starts_with("builtin-ollama-auto-")
            || executor.starts_with("builtin-ollama-calibrated-"))
            || metadata["upstream_url"] != origin {
            continue;
        }
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM vrs_permits WHERE node=$1 AND state='active' AND COALESCE(metadata->>'resident_parked','false')<>'true')",
        )
        .bind(node)
        .fetch_one(&mut *tx)
        .await?;
        if active {
            continue;
        }
        let nonce = Uuid::new_v4().simple().to_string();
        let response = client
            .post(format!(
                "http://v-scheduler-ollama-{role}:11434/api/scheduler/confirm-stopped"
            ))
            .json(&json!({"model":model,"nonce":nonce}))
            .send()
            .await?;
        let receipt = super::app_bindings::bounded_json(response).await?;
        if !stopped_receipt(&receipt, model, &nonce) {
            continue;
        }
        sqlx::query("UPDATE vrs_permits SET state='released',evidence='builtin_ollama_native_stop_confirmed',released_at=COALESCE(released_at,now()) WHERE id=$1 AND app=$2 AND state='suspect'")
            .bind(&id).bind(OWNER).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resident_memory_credit_keeps_incremental_budget_and_safety_floor() {
        let cost = Resources {memory_mib:6144, cpu_millicores:1000, ..Default::default()};
        let report = json!({"memory_total_mib":16384,"memory_available_mib":5589,"cpu_logical_count":8,"cpu_used_percent":10,"gpus":[]});
        let loaded = json!({"models":[{"name":"qwen3.5:2b","size":3136290816_u64,"size_vram":0}]});
        let adjusted = pressure_report(&report, &cost, "qwen3.5:2b", &loaded);
        assert!(!Resources::default().fits_guarded(&cost, &report, &Resources::default()));
        assert!(Resources::default().fits_guarded(&cost, &adjusted, &Resources::default()));
        // Only the observed 2991 MiB, not the entire 6144 MiB reservation,
        // is credited. Runtime growth still needs free RAM alongside 2 GiB.
        let mut low = report.clone();
        low["memory_available_mib"] = json!(6144 - 2991 + 2048 - 1);
        assert!(!Resources::default().fits_guarded(&cost, &pressure_report(&low, &cost, "qwen3.5:2b", &loaded), &Resources::default()));
        let mut other = cost.clone(); other.memory_mib += 1024;
        assert!(!Resources::default().fits_guarded(&other, &adjusted, &Resources::default()));
        for evidence in [Value::Null, json!({"models":[]}), json!({"models":[{"name":"other","size":3136290816_u64,"size_vram":0}]}), json!({"models":[{"name":"qwen3.5:2b","size":1,"size_vram":2}]})] {
            assert_eq!(pressure_report(&report, &cost, "qwen3.5:2b", &evidence), report);
        }
        let huge = json!({"models":[{"name":"qwen3.5:2b","size":u64::MAX,"size_vram":0}]});
        low["memory_available_mib"] = json!(2047);
        assert!(!Resources::default().fits_guarded(&cost, &pressure_report(&low, &cost, "qwen3.5:2b", &huge), &Resources::default()));
    }

    #[test]
    fn resident_gpu_credit_uses_the_correct_memory_pool() {
        let cost = Resources {memory_mib:1024, gpu_slots:1, gpu_memory_mib:4096, ..Default::default()};
        let loaded = json!({"models":[{"model":"chat","size":5368709120_u64,"size_vram":4294967296_u64}]});
        let mut report = json!({"memory_total_mib":16384,"memory_available_mib":2048,"gpus":[{"memory_type":"dedicated","total_mib":8192,"free_mib":409}]});
        let adjusted = pressure_report(&report, &cost, "chat", &loaded);
        assert!(Resources::default().fits_guarded(&cost, &adjusted, &Resources::default()));
        report["gpus"][0]["free_mib"] = json!(408);
        assert!(!Resources::default().fits_guarded(&cost, &pressure_report(&report, &cost, "chat", &loaded), &Resources::default()));
        report["gpus"][0]["memory_type"] = json!("unified");
        report["gpus"][0]["free_mib"] = Value::Null;
        let adjusted = pressure_report(&report, &cost, "chat", &loaded);
        assert_eq!(adjusted["memory_available_mib"], 7168);
        assert!(Resources::default().fits_guarded(&cost, &adjusted, &Resources::default()));
        report["memory_available_mib"] = json!(2047);
        assert!(!Resources::default().fits_guarded(&cost, &pressure_report(&report, &cost, "chat", &loaded), &Resources::default()));
    }

    #[test]
    fn recovery_requires_matching_native_exit_receipt() {
        let good = json!({"protocol_version":1,"model":"qwen3.5:2b","nonce":"challenge","stopped":true,"native_exit_barrier":true});
        assert!(stopped_receipt(&good, "qwen3.5:2b", "challenge"));
        for key in [
            "protocol_version",
            "model",
            "nonce",
            "stopped",
            "native_exit_barrier",
        ] {
            let mut bad = good.clone();
            bad.as_object_mut().unwrap().remove(key);
            assert!(!stopped_receipt(&bad, "qwen3.5:2b", "challenge"));
        }
        assert!(!stopped_receipt(&good, "other", "challenge"));
        assert!(!stopped_receipt(&good, "qwen3.5:2b", "stale"));
    }
    #[test]
    fn automatic_memory_uses_one_unified_pool_and_compute_is_exclusive() {
        let capacity = Resources {
            cpu_millicores: 7000,
            memory_mib: 8192,
            gpu_slots: 1,
            gpu_memory_mib: 8192,
        };
        let unified = resources(1024 * 1048576, &capacity, true);
        assert_eq!(unified.memory_mib, 3072);
        assert_eq!(unified.gpu_memory_mib, 0);
        assert_eq!(unified.cpu_millicores, 7000);
        assert_eq!(
            resources(1024 * 1048576, &capacity, false).gpu_memory_mib,
            3072
        );
        assert!(!resources(8192 * 1048576, &capacity, true).fits(&Resources::default(), &capacity));
    }
}

// Fixed owned native gateways expose the number of in-flight compute slots.
// A completed client request alone is not proof that CPU/GPU compute is idle.
pub(super) async fn native_idle(upstream: &str) -> bool {
    let Some(role) = ["qa", "embedding", "rerank"].into_iter().find(|r| upstream == origin(r)) else { return false; };
    let result = async {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).redirect(reqwest::redirect::Policy::none()).build()?;
        let response = client.get(format!("http://v-scheduler-ollama-{role}:11535/metrics")).send().await?;
        super::app_bindings::bounded_json(response).await
    }.await;
    result.is_ok_and(|v| v["active_slots"].as_u64() == Some(0))
}

// Materialized memory is absent from host free RAM already. Credit only fresh
// native evidence, bounded by each parked permit's still-retained memory budget.
pub(super) async fn idle_report(tx: &mut Transaction<'_, Postgres>, node: &str, report: &Value, exclude: Option<&str>) -> Api<Value> {
    let rows = sqlx::query("SELECT id,resources,metadata FROM vrs_permits WHERE node=$1 AND app=$2 AND state='active' AND metadata->>'resident_parked'='true'")
        .bind(node).bind(OWNER).fetch_all(&mut **tx).await.map_err(unavailable)?;
    let mut adjusted = report.clone();
    for row in rows {
        if exclude.is_some_and(|id| id == row.get::<String, _>("id")) { continue; }
        let m: Value = row.get("metadata");
        let resources: Resources = serde_json::from_value(row.get("resources")).map_err(|_| invalid())?;
        adjusted = resident_report(m["upstream_url"].as_str().unwrap_or(""), m["model_id"].as_str().unwrap_or(""), &resources, &adjusted).await;
    }
    Ok(adjusted)
}

// The idle memory reservation is not zero: keep the observed native footprint,
// rounded up, in its actual memory pool. Runtime workspace is reacquired before
// the next computation; host pressure and safety reserves remain authoritative.
pub(super) async fn parked_resources(upstream: &str, model: &str, cost: &Resources, report: &Value) -> Option<Resources> {
    let role = ["qa", "embedding", "rerank"].into_iter().find(|r| upstream == origin(r))?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).redirect(reqwest::redirect::Policy::none()).build().ok()?;
    let response = client.get(format!("http://v-scheduler-ollama-{role}:11434/api/ps")).send().await.ok()?;
    let loaded = super::app_bindings::bounded_json(response).await.ok()?;
    idle_cost(cost, report, model, &loaded)
}
fn idle_cost(cost: &Resources, report: &Value, model: &str, loaded: &Value) -> Option<Resources> {
    let row = loaded["models"].as_array()?.iter().find(|r| r["name"] == model || r["model"] == model)?;
    let size = row["size"].as_u64()?; let vram = row["size_vram"].as_u64()?;
    if size == 0 || vram > size { return None; }
    let mib = |n: u64| i64::try_from(n.checked_add(1048575)? / 1048576).ok();
    let unified = report["gpus"].as_array().is_some_and(|g| g.iter().any(|v| v["memory_type"] == "unified"));
    let idle = Resources {cpu_millicores:0, gpu_slots:0, memory_mib:mib(if unified {size} else {size-vram})?, gpu_memory_mib:if unified {0} else {mib(vram)?}};
    idle.fits(&Resources::default(), cost).then_some(idle)
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    #[test]
    fn idle_memory_is_rounded_bounded_and_uses_one_unified_pool() {
        let cost=Resources{cpu_millicores:4000,memory_mib:4096,gpu_slots:1,gpu_memory_mib:0};
        let report=json!({"gpus":[{"memory_type":"unified"}]});
        let loaded=json!({"models":[{"name":"test","size":1073741825_u64,"size_vram":1073741825_u64}]});
        let idle=idle_cost(&cost,&report,"test",&loaded).unwrap();
        assert_eq!(idle.memory_mib,1025);assert_eq!(idle.cpu_millicores,0);assert_eq!(idle.gpu_slots,0);assert_eq!(idle.gpu_memory_mib,0);
        assert!(idle_cost(&cost,&report,"missing",&loaded).is_none());
        assert!(idle_cost(&cost,&report,"test",&json!({"models":[{"name":"test","size":u64::MAX,"size_vram":0}]})).is_none());
        assert!(idle_cost(&cost,&report,"test",&json!({"models":[{"name":"test","size":1,"size_vram":2}]})).is_none());
        let dedicated=Resources{gpu_memory_mib:4096,..cost.clone()};
        let split=idle_cost(&dedicated,&json!({"gpus":[{"memory_type":"dedicated"}]}),"test",&loaded).unwrap();
        assert_eq!(split.memory_mib,0);assert_eq!(split.gpu_memory_mib,1025);
    }
}
