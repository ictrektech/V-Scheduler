use super::{Hub, storage::*};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub fn sync(hub: &Hub) -> Result<Value> {
    reconcile(hub)?;
    let mut counts = [0usize; 3];
    for (index, source) in ["hf", "ms"].iter().enumerate() {
        let base = hub.store.root.join("export").join(source);
        scan_exports(hub, source, &base, &base, &mut counts[index])?;
    }
    // Legacy SDK cache layouts are adopted in place, with no copy or startup migration.
    let hf = hub.store.root.join("huggingface/hub");
    if let Ok(entries) = fs::read_dir(hf) {
        for entry in entries.flatten() {
            if let Some(name) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.strip_prefix("models--"))
            {
                let repo = name.replace("--", "/");
                if repository("hf", &repo).is_ok()
                    && register(hub, "hf", &format!("hf://{repo}"), &entry.path())?
                {
                    counts[0] += 1;
                }
            }
        }
    }
    let ms = hub.store.root.join("modelscope/hub");
    if let Ok(orgs) = fs::read_dir(ms) {
        for org in orgs.flatten().filter(|x| x.path().is_dir()) {
            if let Ok(repos) = fs::read_dir(org.path()) {
                for repo in repos.flatten().filter(|x| x.path().is_dir()) {
                    let id = format!(
                        "ms://{}/{}",
                        org.file_name().to_string_lossy(),
                        repo.file_name().to_string_lossy()
                    );
                    if register(hub, "ms", &id, &repo.path())? {
                        counts[1] += 1;
                    }
                }
            }
        }
    }
    let manifests = hub.store.root.join("ollama/models/manifests");
    if manifests.is_dir() {
        scan_manifests(hub, &manifests, &manifests, &mut counts[2])?;
    }
    Ok(
        json!({"synced":counts.iter().sum::<usize>(),"hf_added":counts[0],"ms_added":counts[1],"ollama_added":counts[2],"message":"Shared storage scan completed"}),
    )
}
fn register(hub: &Hub, source: &str, id: &str, path: &Path) -> Result<bool> {
    let existing = hub.store.model(id).ok();
    if let Some(model) = &existing {
        if model["status"] != "pulling" {
            return Ok(false);
        }
        if hub.store.tasks()?.iter().any(|t| {
            t["model_id"] == id
                && !terminal(t)
                && (chrono::Utc::now().timestamp() as f64 - t["_mtime"].as_f64().unwrap_or(0.))
                    < 120.
        }) {
            return Ok(false);
        }
    }
    if !path.is_dir() {
        return Ok(false);
    }
    let managed = path.canonicalize()?;
    if !managed.starts_with(hub.store.root.canonicalize()?) {
        return Ok(false);
    }
    let mut model = json!({"model_id":id,"source":source,"name":id.split("://").nth(1).unwrap_or(id),"path":path,"created_at":now(),"size":size(path),"files":null,"version":null,"sha256":null,"status":"ready"});
    let incomplete = has_marker(path)?;
    if incomplete {
        model["status"] = json!("pulling");
        if existing.is_some() {
            return Ok(false);
        }
    } else if let Ok(access) = hub.store.access(&model) {
        if access == managed && !path.join("snapshots").is_dir() && size(path) == 0 {
            return Ok(false);
        }
        if path.join("snapshots").is_dir() && !path.join("current").exists() {
            if let Some(revision) = access.file_name().and_then(|x| x.to_str()) {
                publish(path, revision)?;
            }
        }
    } else {
        return Ok(false);
    }
    hub.store.registry("models", |data| {
        if data[id].is_null() {
            data[id] = model;
            Ok(true)
        } else if data[id]["status"] == "pulling" && model["status"] == "ready" {
            let mut record = data[id].clone();
            record["status"] = json!("ready");
            record["size"] = model["size"].clone();
            data[id] = record;
            Ok(false)
        } else {
            Ok(false)
        }
    })
}
fn has_marker(path: &Path) -> Result<bool> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".incomplete") || name.ends_with(".tmp") {
            return Ok(true);
        }
        if entry.path().is_dir() && has_marker(&entry.path())? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn scan_manifests(hub: &Hub, base: &Path, dir: &Path, count: &mut usize) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            scan_manifests(hub, base, &path, count)?;
        } else {
            let relative = path
                .strip_prefix(base)?
                .components()
                .map(|p| p.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            if relative.len() < 3 {
                continue;
            }
            let bytes = fs::read(&path)?;
            let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let namespace = &relative[1..relative.len() - 1];
            let name = namespace.join("/");
            let name = name.strip_prefix("library/").unwrap_or(&name);
            let tag = relative.last().unwrap();
            let id = format!("ollama://{name}:{tag}");
            if hub.store.model(&id).is_ok() {
                continue;
            }
            let size = manifest["layers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| v["size"].as_u64().unwrap_or(0))
                .sum::<u64>();
            hub.store.put_model(json!({"model_id":id,"name":format!("{name}:{tag}"),"source":"ollama","path":hub.store.root.join("ollama/models"),"status":"ready","size":size,"version":format!("sha256:{:x}",Sha256::digest(&bytes)),"created_at":now(),"files":null,"sha256":null}))?;
            *count += 1;
        }
    }
    Ok(())
}

fn reconcile(hub: &Hub) -> Result<()> {
    let active = match hub.active.try_lock() {
        Ok(active) => active
            .values()
            .map(|(id, _)| id.clone())
            .collect::<std::collections::HashSet<_>>(),
        Err(_) => return Ok(()),
    };
    for mut task in hub.store.tasks()? {
        if !matches!(task["phase"].as_str(), Some("PENDING" | "DOWNLOADING"))
            || active.contains(task["model_id"].as_str().unwrap_or(""))
        {
            continue;
        }
        let id = task["model_id"].as_str().unwrap_or("");
        if let Ok(mut model) = hub.store.model(id) {
            if model["status"] == "ready" {
                task["phase"] = json!("READY");
                task["progress"] = json!(1.);
                task["speed"] = Value::Null;
                task["eta"] = Value::Null;
                hub.store.put_task(&task)?;
                continue;
            }
            if chrono::Utc::now().timestamp() as f64 - task["_mtime"].as_f64().unwrap_or(0.)
                > (super::download::number("MODEL_TASK_STALE_MINUTES", 10) * 60).max(60) as f64
            {
                task["phase"] = json!("FAILED");
                task["error_msg"] = json!("Download stopped refreshing; retry to continue.");
                hub.store.put_task(&task)?;
                if model["status"] == "pulling" {
                    model["status"] = json!("failed");
                    hub.store.put_model(model)?;
                }
            }
        }
    }
    Ok(())
}

fn scan_exports(
    hub: &Hub,
    source: &str,
    base: &Path,
    directory: &Path,
    count: &mut usize,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if path.join("snapshots").is_dir() || path.join("current").exists() {
            let relative = path.strip_prefix(base)?.to_string_lossy();
            if register(hub, source, &format!("{source}://{relative}"), &path)? {
                *count += 1;
            }
        } else {
            scan_exports(hub, source, base, &path, count)?;
        }
    }
    Ok(())
}
