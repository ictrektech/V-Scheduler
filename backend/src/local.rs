//! The installed app owns its local collector. No separate deployment or user token.
use anyhow::Context;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn write_private(path: &Path, value: &Value) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_extension("pending");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

pub async fn prepare(path: &Path, listen: &str) -> anyhow::Result<Option<PathBuf>> {
    let enabled = match std::env::var("SCHEDULER_EMBEDDED_AGENT").as_deref() {
        Ok("false") => false,
        Ok("true") => true,
        _ => std::env::var_os("VOS_APP_VERSION").is_some() && crate::identity::base().is_some(),
    };
    if !enabled {
        return Ok(None);
    }
    prepare_enabled(path, listen).await.map(Some)
}
async fn prepare_enabled(path: &Path, listen: &str) -> anyhow::Result<PathBuf> {
    let mut config: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let report = crate::agent::probe(&mut None).await;
    let base = crate::identity::base();
    let id = config["local_agent"]["node_id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            config["nodes"].as_array()?.iter().find(|n| {
                base.as_ref().is_some_and(|b| {
                    n["vos"]["url"]
                        .as_str()
                        .is_some_and(|u| u.trim_end_matches('/') == b.trim_end_matches('/'))
                })
            })?["id"]
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "local".to_owned());
    let previously_automatic = config["local_agent"]["automatic_capacity"] == true;
    let nodes = config["nodes"].as_array_mut().context("missing nodes")?;
    let created = !nodes.iter().any(|node| node["id"] == id);
    if created {
        let resources = capacity(&report).context("local host resources unavailable")?;
        let mut node = json!({"id":id,"capacities":resources});
        if let Some(url) = base {
            node["vos"] = json!({"url":url});
        }
        nodes.push(node);
    } else if previously_automatic {
        if let Some(resources) = capacity(&report) {
            nodes.iter_mut().find(|node| node["id"] == id).unwrap()["capacities"] =
                json!(resources);
        }
    }
    let principals = config["principals"]
        .as_array_mut()
        .context("missing principals")?;
    let token = if let Some(principal) = principals
        .iter()
        .find(|p| p["role"] == "worker" && p["node_id"] == id)
    {
        principal["token"]
            .as_str()
            .context("invalid local worker")?
            .to_owned()
    } else {
        let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        principals
            .push(json!({"token":token,"role":"worker","app_id":"v-scheduler.local","node_id":id}));
        token
    };
    let adapters = config["local_agent"]["executors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    config["local_agent"] = json!({"node_id":id,"executors":adapters,"automatic_capacity":created||previously_automatic});
    write_private(path, &config)?;
    let directory = path.parent().context("missing config directory")?;
    let agent_path = directory.join("local-agent.json");
    let port = listen
        .rsplit(':')
        .next()
        .context("invalid controller address")?;
    write_private(
        &agent_path,
        &json!({"controller":format!("http://127.0.0.1:{port}/api/v1"),"node_id":id,"token":token,"journal":directory.join("local-agent-journal.json"),"executors":adapters}),
    )?;
    Ok(agent_path)
}

pub fn capacity(report: &Value) -> Option<crate::model::Resources> {
    let cores = report["cpu_logical_count"].as_i64().filter(|v| *v > 0)?;
    let memory = report["memory_total_mib"].as_i64().filter(|v| *v > 0)?;
    let gpu = report["gpus"].as_array().and_then(|g| g.first());
    let ram_budget = memory.saturating_sub(2048.max(memory / 10)).max(0);
    let gpu_memory = gpu
        .map(|g| {
            if g["memory_type"] == "unified" {
                ram_budget
            } else {
                let total = g["total_mib"].as_i64().unwrap_or(0);
                total.saturating_sub(256.max(total / 20)).max(0)
            }
        })
        .unwrap_or(0);
    Some(crate::model::Resources {
        cpu_millicores: cores.saturating_mul(1000).saturating_sub(if cores == 1 {
            500
        } else {
            1000
        }),
        memory_mib: ram_budget,
        gpu_slots: i64::from(gpu.is_some()),
        gpu_memory_mib: gpu_memory,
    })
}

pub fn start(path: Option<PathBuf>) {
    if let Some(path) = path {
        tokio::spawn(async move {
            loop {
                if crate::agent::run(path.clone()).await.is_err() {
                    eprintln!("local resource collector restarting");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn empty_installation_starts_with_local_resources_and_keeps_identity() {
        let directory =
            std::env::temp_dir().join(format!("scheduler-local-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("controller.json");
        write_private(&path,&json!({"principals":[{"token":"unit-test-administrator-not-a-secret","role":"admin","app_id":"admin"}],"nodes":[],"executors":[]})).unwrap();
        let agent_path = prepare_enabled(&path, "0.0.0.0:38120").await.unwrap();
        let first: Value = serde_json::from_slice(&std::fs::read(&agent_path).unwrap()).unwrap();
        prepare_enabled(&path, "0.0.0.0:38120").await.unwrap();
        let second: Value = serde_json::from_slice(&std::fs::read(&agent_path).unwrap()).unwrap();
        let config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(first["token"], second["token"]);
        assert_eq!(config["nodes"].as_array().unwrap().len(), 1);
        assert!(
            config["nodes"][0]["capacities"]["cpu_millicores"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(
            config["nodes"][0]["capacities"]["memory_mib"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert_eq!(first["controller"], "http://127.0.0.1:38120/api/v1");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
