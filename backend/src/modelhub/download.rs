use super::{Hub, storage::*};
use anyhow::{Result, bail, ensure};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
struct DownloadHttpError(u16);
impl std::fmt::Display for DownloadHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "download HTTP {}", self.0)
    }
}
impl std::error::Error for DownloadHttpError {}

// Never expose reqwest's authenticated redirect URL or arbitrary storage paths.
fn failure_code(error: &anyhow::Error) -> String {
    if let Some(status) = error.downcast_ref::<DownloadHttpError>() {
        return format!("HTTP_{}", status.0);
    }
    if let Some(request) = error.downcast_ref::<reqwest::Error>() {
        if let Some(status) = request.status() {
            return format!("HTTP_{}", status.as_u16());
        }
        if request.is_timeout() {
            return "TRANSFER_TIMEOUT".into();
        }
        if request.is_connect() {
            if error.chain().any(|cause| {
                let text = cause.to_string().to_lowercase();
                text.contains("certificate") || text.contains("unknownissuer")
            }) {
                return "TLS_CERTIFICATE_ERROR".into();
            }
            return "UPSTREAM_CONNECTION_FAILED".into();
        }
        return "UPSTREAM_TRANSFER_FAILED".into();
    }
    if error
        .downcast_ref::<tokio::time::error::Elapsed>()
        .is_some()
    {
        return "TRANSFER_IDLE_TIMEOUT".into();
    }
    if let Some(storage) = error.downcast_ref::<std::io::Error>() {
        return if storage.raw_os_error() == Some(libc::ENOSPC) {
            "STORAGE_FULL"
        } else if storage.kind() == std::io::ErrorKind::PermissionDenied {
            "STORAGE_PERMISSION_DENIED"
        } else {
            "STORAGE_IO_FAILED"
        }
        .into();
    }
    match error.to_string().as_str() {
        "file checksum mismatch" => "CHECKSUM_MISMATCH",
        "downloaded file has wrong size" => "FILE_SIZE_MISMATCH",
        "invalid range response" => "INVALID_RANGE_RESPONSE",
        "cancelled" => "CANCELLED",
        _ => "DOWNLOAD_FAILED",
    }
    .into()
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    #[test]
    fn download_failures_are_actionable_without_echoing_secrets() {
        assert_eq!(failure_code(&DownloadHttpError(403).into()), "HTTP_403");
        assert_eq!(
            failure_code(&anyhow::anyhow!("file checksum mismatch")),
            "CHECKSUM_MISMATCH"
        );
        let private = anyhow::anyhow!("https://example.invalid/object?signature=private-value");
        assert_eq!(failure_code(&private), "DOWNLOAD_FAILED");
        let storage =
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "/private/token-file");
        assert_eq!(failure_code(&storage.into()), "STORAGE_PERMISSION_DENIED");
    }
}

pub fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}
pub fn number(name: &str, default: u64) -> u64 {
    env(name, &default.to_string()).parse().unwrap_or(default)
}
pub fn hf_endpoint() -> String {
    if matches!(
        env("HF_MIRROR_ENABLED", "false").as_str(),
        "true" | "1" | "yes"
    ) {
        env("HF_ENDPOINT", "https://hf-mirror.com")
    } else {
        "https://huggingface.co".into()
    }
}
fn authorize(request: reqwest::RequestBuilder, source: &str) -> reqwest::RequestBuilder {
    let token = if source == "hf" {
        env("HF_TOKEN", &env("HUGGING_FACE_HUB_TOKEN", ""))
    } else {
        env("MODELSCOPE_API_TOKEN", "")
    };
    if token.is_empty() {
        request
    } else {
        request.bearer_auth(token)
    }
}
pub async fn remote(hub: &Hub, source: &str, id: &str) -> Result<Value> {
    let repo = repository(source, id)?;
    let url = if source == "hf" {
        format!(
            "{}/api/models/{repo}?blobs=true",
            hf_endpoint().trim_end_matches('/')
        )
    } else {
        format!(
            "{}/api/v1/models/{repo}/repo/files?Revision=master&Root=&Recursive=true",
            env("MODELSCOPE_ENDPOINT", "https://modelscope.cn").trim_end_matches('/')
        )
    };
    let response = authorize(hub.client.get(url), source).send().await?;
    ensure!(
        response.status().is_success(),
        "repository metadata request failed (HTTP {}); verify repository permission and endpoint",
        response.status()
    );
    let data: Value = response.json().await?;
    let (entries, version) = if source == "hf" {
        (
            data["siblings"].as_array(),
            data["sha"].as_str().unwrap_or("main"),
        )
    } else {
        (
            data["Data"]["Files"].as_array(),
            data["Data"]["Revision"].as_str().unwrap_or("master"),
        )
    };
    let entries =
        entries.ok_or_else(|| anyhow::anyhow!("repository did not return a file list"))?;
    let canonical_id = normalize_id(source, id)?;
    let local: BTreeSet<String> = hub
        .store
        .model(&canonical_id)
        .ok()
        .and_then(|m| hub.store.local_files(&m).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v["path"].as_str().map(str::to_owned))
        .collect();
    let mut files = vec![];
    for item in entries {
        if matches!(item["Type"].as_str(), Some("tree" | "directory")) {
            continue;
        }
        let Some(path) = item["rfilename"].as_str().or(item["Path"].as_str()) else {
            continue;
        };
        safe_relative(path)?;
        let size = item["size"]
            .as_u64()
            .or(item["Size"].as_u64())
            .or(item["lfs"]["size"].as_u64());
        let hash = item["lfs"]["sha256"].as_str().or(item["Sha256"].as_str());
        files
            .push(json!({"path":path,"size":size,"downloaded":local.contains(path),"sha256":hash}));
    }
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let revision = version.to_owned();
    let version = if source == "ms" {
        let url = format!(
            "{}/api/v1/models/{repo}/revisions",
            env("MODELSCOPE_ENDPOINT", "https://modelscope.cn").trim_end_matches('/')
        );
        let revisions = authorize(hub.client.get(url), source)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let branch = revisions["Data"]["RevisionMap"]["Branches"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["Revision"] == "master")
            .ok_or_else(|| anyhow::anyhow!("missing ModelScope master revision"))?;
        let timestamp = branch["CreatedAt"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("missing ModelScope revision timestamp"))?;
        format!("master@{timestamp}")
    } else {
        revision.clone()
    };
    Ok(
        json!({"model_id":normalize_id(source,id)?,"source":source,"total_size":files.iter().map(|x|x["size"].as_u64().unwrap_or(0)).sum::<u64>(),"files":files,"version":version,"revision":revision}),
    )
}
pub async fn create(hub: Arc<Hub>, mut request: Value) -> Result<Value> {
    let source = request["source"].as_str().unwrap_or("").to_owned();
    let id = normalize_id(&source, request["model_id"].as_str().unwrap_or(""))?;
    request["model_id"] = json!(id);
    let _guard = hub.lock_model_change(&id).await?;
    let mut files = BTreeSet::new();
    if let Some(items) = request["files"].as_array() {
        for file in items {
            let name = file
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("files must contain strings"))?
                .trim();
            files.insert(safe_relative(name)?.to_string_lossy().into_owned());
        }
    }
    ensure!(
        source != "ollama" || files.is_empty(),
        "Ollama does not support selected repository files"
    );
    let append = request["append"].as_bool().unwrap_or(false);
    ensure!(
        !append || (!files.is_empty() && source != "ollama"),
        "append requires HF/MS and a nonempty files list"
    );
    request["files"] = if files.is_empty() {
        Value::Null
    } else {
        json!(files)
    };
    let task_id = format!("task-{}", uuid::Uuid::new_v4());
    let active = hub.active.lock().await;
    ensure!(
        !active.values().any(|(model, _)| model == &id),
        "model already has an active task"
    );
    // Also honor recently refreshed task records created by other shared-directory users.
    let timestamp = chrono::Utc::now().timestamp() as f64;
    for task in hub.store.tasks()? {
        ensure!(
            task["model_id"] != id
                || terminal(&task)
                || timestamp - task["_mtime"].as_f64().unwrap_or(0.) > 120.,
            "another application is downloading this model"
        );
    }
    let previous = hub.store.model(&id).ok();
    ensure!(
        previous
            .as_ref()
            .is_none_or(|m| m["status"] != "pulling" && m["status"] != "deleting"),
        "model is currently downloading or deleting"
    );
    ensure!(
        !append || previous.as_ref().is_none_or(|m| m["status"] == "ready"),
        "append requires a ready model"
    );
    ensure!(
        request["force"] == true
            || append
            || !previous.as_ref().is_some_and(|m| m["status"] == "ready"),
        "model is already installed; use force to update"
    );
    drop(active);
    if source=="ollama" {crate::router::prepare_model_change(id.trim_start_matches("ollama://")).await?;}
    let model = previous.clone().unwrap_or(json!({"model_id":id,"source":source,"name":request["name"].as_str().unwrap_or(&id),"created_at":now(),"path":hub.store.model_root(&source,&id)?,"size":0,"version":null,"sha256":null,"files":null}));
    let mut task = json!({"task_id":task_id,"model_id":id,"progress":0.,"speed":null,"eta":null,"phase":"PENDING","error_msg":null,"files":request["files"],"append":append,"_request":request,"_previous":previous,"_owner":"v-scheduler"});
    // Snapshot the old record before publishing a pulling state so interruption can restore it.
    hub.store.put_task(&task)?;
    let mut pulling = model;
    pulling["status"] = json!("pulling");
    hub.store.put_model(pulling)?;
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let mut active=hub.active.lock().await;
    active.insert(task_id.clone(), (id.clone(), cancel_tx));
    drop(active);
    let result = json!({"task_id":task_id,"model_id":id});
    tokio::spawn(async move {
        task["phase"] = json!("DOWNLOADING");
        let _ = hub.store.put_task(&task);
        let transfer = async {
            if source == "ollama" {
                pull_ollama(&hub, &mut task).await
            } else {
                pull_repository(&hub, &mut task).await
            }
        };
        tokio::pin!(transfer);
        let outcome = tokio::select! { result = &mut transfer => result, _ = async { let mut rx=cancel_rx; while !*rx.borrow_and_update() { if rx.changed().await.is_err() { break; } } } => Err(anyhow::anyhow!("cancelled by user")) };
        drop(transfer);
        // Transfer borrow ends at the helper boundary below.
        finish_task(&hub, &task_id, outcome).await;
    });
    Ok(result)
}
async fn finish_task(hub: &Hub, id: &str, outcome: Result<()>) {
    // A visible terminal state must permit an immediate retry. Hold the same
    // admission lock through publishing that state and removing active ownership.
    let mut active = hub.active.lock().await;
    if let Ok(mut task) = hub.store.task(id) {
        match outcome {
            Ok(()) => {
                task["phase"] = json!("READY");
                task["progress"] = json!(1.);
                task["error_msg"] = Value::Null;
                task["error_code"] = Value::Null;
            }
            Err(error) => {
                // Do not persist upstream errors containing authenticated redirect URLs.
                task["phase"] = json!("FAILED");
                let code = failure_code(&error);
                task["error_code"] = json!(code);
                task["error_msg"] = json!(format!(
                    "Download failed ({code}); existing model preserved. Verify endpoint, permissions, storage and network, then retry."
                ));
                let mut model = task["_previous"].clone();
                if model.is_null() {
                    if let Ok(m) = hub.store.model(task["model_id"].as_str().unwrap_or("")) {
                        model = m;
                        model["status"] = json!("failed");
                    }
                }
                if !model.is_null() {
                    let _ = hub.store.put_model(model);
                }
            }
        }
        task["speed"] = Value::Null;
        task["eta"] = Value::Null;
        let _ = hub.store.put_task(&task);
    }
    active.remove(id);
}
async fn pull_repository(hub: &Hub, task: &mut Value) -> Result<()> {
    let id = task["model_id"].as_str().unwrap().to_owned();
    let source = task["_request"]["source"].as_str().unwrap().to_owned();
    let metadata = remote(hub, &source, &id).await?;
    let available: HashMap<String, Value> = metadata["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["path"].as_str().unwrap().to_owned(), v.clone()))
        .collect();
    let selected: Vec<String> = if let Some(files) = task["files"].as_array() {
        files
            .iter()
            .map(|x| x.as_str().unwrap().to_owned())
            .collect()
    } else {
        available.keys().cloned().collect()
    };
    ensure!(!selected.is_empty(), "repository is empty");
    ensure!(
        selected.iter().all(|f| available.contains_key(f)),
        "selected file is absent from repository"
    );
    let model_root = hub.store.model_root(&source, &id)?;
    let staging_id = task["_request"]["_resume"]
        .as_str()
        .unwrap_or(task["task_id"].as_str().unwrap());
    safe_relative(staging_id)?;
    ensure!(!staging_id.contains('/'), "invalid resume task");
    let stage = hub.store.root.join("shared/tmp").join(staging_id);
    fs::create_dir_all(&stage)?;
    let state_path = hub
        .store
        .root
        .join("shared/manifests")
        .join(format!("{staging_id}.json"));
    let mut download_state = read(&state_path)?
        .unwrap_or(json!({"version":metadata["version"],"model_id":id,"completed":[]}));
    ensure!(
        download_state["model_id"] == id,
        "resume data belongs to a different model"
    );
    if download_state["version"] != metadata["version"] {
        fs::remove_dir_all(&stage)?;
        fs::create_dir_all(&stage)?;
        download_state = json!({"version":metadata["version"],"model_id":id,"completed":[]});
    }
    atomic_json(&state_path, &download_state)?;

    let revision = format!(
        "{}-{}",
        metadata["version"]
            .as_str()
            .unwrap_or("download")
            .replace(['/', ':'], "_"),
        task["task_id"].as_str().unwrap()
    );
    // Reuse old unselected files for append. Old snapshots remain untouched on failed updates.
    if task["append"] == true
        && task["_previous"].is_object()
        && fs::read_dir(&stage)?.next().is_none()
    {
        let old = hub.store.access(&task["_previous"])?;
        copy_files(
            &old,
            &stage,
            &Path::new(task["_previous"]["path"].as_str().unwrap()).canonicalize()?,
        )?;
    }
    let total: u64 = selected
        .iter()
        .map(|x| available[x]["size"].as_u64().unwrap_or(0))
        .sum();
    let started = Instant::now();
    let mut received = 0u64;
    let mut completed = 0u64;
    for relative in &selected {
        let path = stage.join(safe_relative(relative)?);
        if download_state["completed"]
            .as_array()
            .is_some_and(|files| files.iter().any(|v| v == relative))
            && path.is_file()
            && available[relative]["size"]
                .as_u64()
                .is_none_or(|expected| fs::metadata(&path).is_ok_and(|m| m.len() == expected))
        {
            completed += fs::metadata(&path)?.len();
            continue;
        }

        fs::create_dir_all(path.parent().unwrap())?;
        // A seeded hard link must never be truncated; download into a separate partial inode.
        let partial = path.with_file_name(format!(
            "{}.incomplete",
            path.file_name().unwrap().to_string_lossy()
        ));
        let offset = fs::metadata(&partial).map(|x| x.len()).unwrap_or(0);
        let repo = repository(&source, &id)?;
        let revision_name =
            metadata["revision"]
                .as_str()
                .unwrap_or(if source == "hf" { "main" } else { "master" });
        let endpoint = if source == "hf" {
            hf_endpoint()
        } else {
            env("MODELSCOPE_ENDPOINT", "https://modelscope.cn")
        };
        let mut url = reqwest::Url::parse(&endpoint)?;
        if source == "hf" {
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("invalid endpoint"))?
                .extend(repo.split('/'))
                .extend(["resolve", revision_name])
                .extend(relative.split('/'));
        } else {
            url.set_path(&format!("/api/v1/models/{repo}/repo"));
            url.query_pairs_mut()
                .append_pair("Revision", revision_name)
                .append_pair("FilePath", relative);
        }
        let mut request = authorize(hub.client.get(url), &source);
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        let mut response = request.send().await?;
        if !response.status().is_success() {
            return Err(DownloadHttpError(response.status().as_u16()).into());
        }
        let append = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        if append {
            ensure!(
                response
                    .headers()
                    .get("content-range")
                    .and_then(|x| x.to_str().ok())
                    .is_some_and(|x| x.starts_with(&format!("bytes {offset}-"))),
                "invalid range response"
            );
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&partial)
            .await?;
        let mut last = Instant::now();
        let mut file_bytes = if append { offset } else { 0 };
        while let Some(chunk) = tokio::time::timeout(
            Duration::from_secs(number("OLLAMA_DOWNLOAD_TIMEOUT", 3600)),
            response.chunk(),
        )
        .await??
        {
            file.write_all(&chunk).await?;
            received += chunk.len() as u64;
            file_bytes += chunk.len() as u64;
            if last.elapsed().as_millis() >= 400 {
                let speed = received as f64 / started.elapsed().as_secs_f64().max(0.01);
                task["progress"] = json!(if total > 0 {
                    ((completed + file_bytes) as f64 / total as f64).min(0.99)
                } else {
                    0.01
                });
                task["speed"] = json!(speed);
                task["eta"] = if total > 0 {
                    json!(total.saturating_sub(completed + file_bytes) as f64 / speed.max(1.))
                } else {
                    Value::Null
                };
                hub.store.put_task(task)?;
                last = Instant::now();
            }
        }
        file.sync_all().await?;
        drop(file);
        if let Some(expected) = available[relative]["size"].as_u64() {
            ensure!(
                fs::metadata(&partial)?.len() == expected,
                "downloaded file has wrong size"
            );
        }
        if let Some(hash) = available[relative]["sha256"].as_str() {
            let path = partial.clone();
            let hash = hash.to_owned();
            tokio::task::spawn_blocking(move || -> Result<()> {
                use sha2::{Digest, Sha256};
                use std::io::Read;
                let mut file = fs::File::open(path)?;
                let mut digest = Sha256::new();
                let mut buffer = vec![0; 1024 * 1024];
                loop {
                    let n = file.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    digest.update(&buffer[..n]);
                }
                ensure!(
                    format!("{:x}", digest.finalize()) == hash,
                    "file checksum mismatch"
                );
                Ok(())
            })
            .await??;
        }
        fs::rename(partial, path)?;
        if !download_state["completed"].is_array() {
            download_state["completed"] = json!([]);
        }
        download_state["completed"]
            .as_array_mut()
            .unwrap()
            .push(json!(relative));
        atomic_json(&state_path, &download_state)?;

        completed += file_bytes;
    }
    task["phase"] = json!("VERIFYING");
    hub.store.put_task(task)?;
    let hash_root = stage.clone();
    let hash = tokio::task::spawn_blocking(move || directory_sha256(&hash_root)).await??;
    fs::create_dir_all(model_root.join("snapshots"))?;
    fs::rename(&stage, model_root.join("snapshots").join(&revision))?;
    publish(&model_root, &revision)?;
    let mut model = hub.store.model(&id)?;
    model["status"] = json!("ready");
    model["path"] = json!(model_root);
    model["version"] = metadata["version"].clone();
    model["sha256"] = json!(hash);
    model["size"] = json!(size(&model_root.join("snapshots").join(revision)));
    model["files"] = if task["append"] == true
        && task["_previous"].is_object()
        && task["_previous"]["files"].is_null()
    {
        Value::Null
    } else if task["append"] == true && task["_previous"].is_object() {
        let mut set: BTreeSet<String> = task["_previous"]["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        set.extend(selected);
        json!(set)
    } else {
        task["files"].clone()
    };
    hub.store.put_model(model)?;
    Ok(())
}
async fn pull_ollama(hub: &Hub, task: &mut Value) -> Result<()> {
    let name = task["model_id"]
        .as_str()
        .unwrap()
        .trim_start_matches("ollama://")
        .to_owned();
    let url = format!(
        "{}/api/pull",
        super::runtime::runtime("qa")?["base_url"].as_str().unwrap()
    );
    let max = number("OLLAMA_PULL_MAX_ATTEMPTS", 5).max(1);
    for attempt in 0..max {
        let result = ollama_attempt(hub, task, &url, &name).await;
        if result.is_ok() {
            let mut model = hub.store.model(task["model_id"].as_str().unwrap())?;
            model["status"] = json!("ready");
            let tags = hub
                .client
                .get(url.trim_end_matches("/api/pull").to_owned() + "/api/tags")
                .send()
                .await?
                .error_for_status()?
                .json::<Value>()
                .await?;
            if let Some(item) = tags["models"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|m| m["name"] == name || m["name"] == format!("{name}:latest"))
            {
                model["size"] = item["size"].clone();
                model["version"] = item["digest"].clone();
            }
            hub.store.put_model(model)?;
            return Ok(());
        }
        if attempt + 1 == max {
            return result;
        }
        let delay = number("OLLAMA_PULL_RETRY_DELAY_SECONDS", 15)
            .saturating_mul(1u64 << attempt.min(20))
            .min(number("OLLAMA_PULL_RETRY_MAX_DELAY_SECONDS", 300));
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
    bail!("pull failed")
}
async fn ollama_attempt(hub: &Hub, task: &mut Value, url: &str, name: &str) -> Result<()> {
    let response = hub
        .client
        .post(url)
        .json(&json!({"model":name,"stream":true}))
        .send()
        .await?
        .error_for_status()?;
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut layers = HashMap::<String, (u64, u64)>::new();
    let mut success = false;
    let start = Instant::now();
    let mut newly = 0u64;
    let mut baseline = HashMap::<String, u64>::new();
    let window = number("OLLAMA_PULL_SLOW_WINDOW_SECONDS", 120).max(1);
    let mut progress_at = Instant::now();
    let mut healthy = false;
    let mut slow_since = None;
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(window), stream.next()).await?
    {
        pending.extend_from_slice(&chunk?);
        while let Some(end) = pending.iter().position(|x| *x == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let value: Value = serde_json::from_slice(&line)?;
            ensure!(value["error"].is_null(), "Ollama rejected pull");
            if value["status"] == "success" {
                success = true;
            }
            if let Some(digest) = value["digest"].as_str() {
                let completed = value["completed"].as_u64().unwrap_or(0);
                let total = value["total"].as_u64().unwrap_or(0);
                let first = *baseline.entry(digest.into()).or_insert(completed);
                let previous = layers
                    .insert(digest.into(), (completed, total))
                    .map(|x| x.0)
                    .unwrap_or(first);
                if completed > previous {
                    newly += completed - previous;
                    progress_at = Instant::now();
                }
                let (done, total) = layers
                    .values()
                    .fold((0u64, 0u64), |(a, b), (c, t)| (a + c, b + t));
                let speed = newly as f64 / start.elapsed().as_secs_f64().max(0.01);
                let threshold = number("OLLAMA_PULL_MIN_SPEED_BYTES_PER_SECOND", 262144) as f64;
                if speed >= threshold {
                    healthy = true;
                    slow_since = None;
                } else if healthy {
                    let since = slow_since.get_or_insert_with(Instant::now);
                    ensure!(
                        since.elapsed().as_secs() < window,
                        "Ollama transfer stalled"
                    );
                }
                ensure!(
                    progress_at.elapsed().as_secs() < window,
                    "Ollama transfer stalled"
                );
                task["progress"] = json!(if total > 0 {
                    (done as f64 / total as f64).min(0.99)
                } else {
                    0.
                });
                task["speed"] = json!(speed);
                task["eta"] = if speed > 0. {
                    json!(total.saturating_sub(done) as f64 / speed)
                } else {
                    Value::Null
                };
                hub.store.put_task(task)?;
            }
        }
    }
    ensure!(success, "Ollama stream ended before success");
    Ok(())
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    #[tokio::test]
    async fn terminal_download_is_not_visible_before_active_bookkeeping_can_finish() {
        let root =
            std::env::temp_dir().join(format!("vs-download-finish-{}", uuid::Uuid::new_v4()));
        let hub = Arc::new(Hub {
            store: Store::new(root.clone()).unwrap(),
            client: reqwest::Client::new(),
            active: Default::default(),
            model_locks: Default::default(),
        });
        hub.store.put_task(&json!({"task_id":"owned-task","model_id":"hf://fixture/model","phase":"DOWNLOADING"})).unwrap();
        let (tx, _rx) = tokio::sync::watch::channel(false);
        hub.active
            .lock()
            .await
            .insert("owned-task".into(), ("hf://fixture/model".into(), tx));
        let guard = hub.active.lock().await;
        let (started, ready) = tokio::sync::oneshot::channel();
        let worker = hub.clone();
        let job = tokio::spawn(async move {
            started.send(()).unwrap();
            finish_task(&worker, "owned-task", Ok(())).await;
        });
        ready.await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(
            hub.store.task("owned-task").unwrap()["phase"],
            "DOWNLOADING",
            "terminal state must not race ahead of the active-task lock"
        );
        drop(guard);
        job.await.unwrap();
        assert_eq!(hub.store.task("owned-task").unwrap()["phase"], "READY");
        assert!(hub.active.lock().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
