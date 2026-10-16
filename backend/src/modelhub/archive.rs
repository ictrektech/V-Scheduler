use super::{Hub, storage::*};
use anyhow::{Result, bail, ensure};
use axum::extract::Multipart;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncWriteExt;

pub async fn upload(hub: Arc<Hub>, multipart: &mut Multipart) -> Result<Value> {
    let token = format!("import-{}", uuid::Uuid::new_v4());
    let staging = hub.store.root.join("shared/tmp").join(&token);
    fs::create_dir_all(&staging)?;
    let outcome = async {
        let upload = staging.join("archive");
        let extracted = staging.join("content");
        let mut values = json!({});
        let mut filename = String::new();
        let mut supplied = false;
        while let Some(mut field) = multipart.next_field().await? {
            let name = field.name().unwrap_or("").to_owned();
            if name == "archive" {
                ensure!(!supplied, "only one archive is accepted");
                supplied = true;
                filename = field.file_name().unwrap_or("model.zip").to_owned();
                let mut file = tokio::fs::File::create(&upload).await?;
                while let Some(chunk) = field.chunk().await? { file.write_all(&chunk).await?; }
                file.sync_all().await?;
            } else {
                let data = field.bytes().await?;
                ensure!(data.len() < 65536, "oversized archive metadata");
                values[&name] = json!(std::str::from_utf8(&data)?);
            }
        }
        ensure!(supplied, "archive is required");
        let source = values["source"].as_str().unwrap_or("");
        ensure!(matches!(source, "hf" | "ms"), "archive import supports HF and MS");
        let mut stem = filename.as_str();
        for suffix in [".tar.gz", ".tar.bz2", ".tar.xz", ".tgz", ".tbz2", ".txz", ".zip", ".tar"] {
            if stem.to_ascii_lowercase().ends_with(suffix) { stem = &stem[..stem.len()-suffix.len()]; break; }
        }
        let remote_repo = values["remote_repo"].as_str().filter(|s| !s.trim().is_empty());
        let supplied_id = values["model_id"].as_str().filter(|s| !s.trim().is_empty());
        let input = supplied_id.or(remote_repo).unwrap_or(stem).trim().trim_matches('/');
        let input = input.strip_prefix("hf://").or_else(|| input.strip_prefix("ms://")).unwrap_or(input);
        safe_relative(input)?;
        if let Some(repo) = remote_repo {
            let repo = repository(source,repo.trim().trim_matches('/'))?;
            ensure!(repo == input, "model_id must match remote_repo to support updates");
        }
        let id = format!("{source}://{input}");
        let root = hub.store.model_root(source, &id)?;
        let _guard = hub.lock_model(&id).await;
        let old = hub.store.model(&id).ok();
        ensure!(old.as_ref().is_none_or(|m| m["status"] != "pulling" && m["status"] != "deleting"), "model is downloading or deleting");
        ensure!(old.is_none() || values["replace_existing"] == "true", "model already exists; enable replace explicitly");
        ensure!(!hub.active.lock().await.values().any(|(model,_)| model == &id), "model has an active download");
        for task in hub.store.tasks()? { ensure!(task["model_id"] != id || terminal(&task), "model has an active task"); }
        let mut version = values["version"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        if remote_repo.is_some() {
            if let Ok(metadata) = super::download::remote(&hub,source,&id).await { version = metadata["version"].as_str().map(str::to_owned).or(version); }
        }
        let file = upload.clone(); let to = extracted.clone(); let archive_name = filename.clone();
        tokio::task::spawn_blocking(move || extract(&file, &to, &archive_name)).await??;
        let mut content = extracted;
        loop {
            let entries: Vec<_> = fs::read_dir(&content)?.collect::<std::result::Result<_,_>>()?;
            if entries.len() == 1 && entries[0].path().is_dir() && entries[0].file_name() != "snapshots" { content = entries[0].path(); } else { break; }
        }
        if content.join("snapshots").is_dir() { content = hub.store.access(&json!({"path":content}))?; }
        let mut files = vec![]; collect(&content,&content,&mut files)?;
        ensure!(!files.is_empty(), "archive is empty");
        ensure!(!files.iter().any(|p| p.components().any(|part| part.as_os_str() == "snapshots")), "snapshots must be at the model root");
        let entries: Vec<_> = fs::read_dir(&content)?.collect::<std::result::Result<_,_>>()?;
        ensure!(entries.iter().any(|e|e.path().is_file()) || entries.iter().filter(|e|e.path().is_dir()).count() <= 1, "archive must contain one model root");
        ensure!(files.iter().any(|p| looks_like_model(p)), "archive has no model files");
        let revision = format!("imported-{}", uuid::Uuid::new_v4());
        let hash_root=content.clone();
        let hash=tokio::task::spawn_blocking(move||directory_sha256(&hash_root)).await??;
        fs::create_dir_all(root.join("snapshots"))?;
        let snapshot = root.join("snapshots").join(&revision);
        fs::rename(&content, &snapshot)?;
        let model = json!({"model_id":id,"source":source,"name":values["name"].as_str().filter(|s|!s.is_empty()).unwrap_or(input),"path":root,"created_at":old.as_ref().and_then(|v|v["created_at"].as_str()).unwrap_or(&now()),"status":"ready","size":size(&snapshot),"version":version,"files":null,"sha256":hash});
        publish(&root,&revision)?;
        hub.store.put_model(model.clone())?;
        Ok(model)
    }.await;
    let _ = fs::remove_dir_all(staging);
    outcome
}
fn looks_like_model(path: &Path) -> bool {
    [
        "config.json",
        "generation_config.json",
        "model_index.json",
        "tokenizer.json",
        "tokenizer.model",
        "tokenizer_config.json",
        "preprocessor_config.json",
        "sentence_bert_config.json",
        "modules.json",
    ]
    .iter()
    .any(|name| path.file_name().is_some_and(|f| f == *name))
        || path.extension().is_some_and(|x| {
            ["gguf", "safetensors", "bin", "pt", "pth", "onnx", "ckpt"]
                .contains(&x.to_string_lossy().to_ascii_lowercase().as_str())
        })
}
fn collect(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        ensure!(
            !entry.file_type()?.is_symlink(),
            "archive links are not accepted"
        );
        if entry.path().is_dir() {
            collect(root, &entry.path(), files)?;
        } else {
            files.push(entry.path().strip_prefix(root)?.to_owned());
        }
    }
    Ok(())
}
fn target(base: &Path, name: &Path) -> Result<PathBuf> {
    let name = name
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid archive path"))?;
    Ok(base.join(safe_relative(name.trim_end_matches('/'))?))
}
fn extract(path: &Path, directory: &Path, filename: &str) -> Result<()> {
    let filename = filename.to_ascii_lowercase();
    fs::create_dir_all(directory)?;
    if filename.to_ascii_lowercase().ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(fs::File::open(path)?)?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            ensure!(
                entry
                    .unix_mode()
                    .is_none_or(|mode| mode & 0o170000 != 0o120000),
                "archive links are not accepted"
            );
            let out = target(directory, Path::new(entry.name()))?;
            if entry.is_dir() {
                fs::create_dir_all(out)?;
            } else {
                fs::create_dir_all(out.parent().unwrap())?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(out)?;
                std::io::copy(&mut entry, &mut file)?;
            }
        }
    } else {
        let input = fs::File::open(path)?;
        let reader: Box<dyn std::io::Read> =
            if filename.ends_with(".gz") || filename.ends_with(".tgz") {
                Box::new(flate2::read::GzDecoder::new(input))
            } else if filename.ends_with(".bz2")
                || filename.ends_with(".tbz2")
                || filename.ends_with(".xz")
                || filename.ends_with(".txz")
            {
                // Native decompression only; archive paths/types are still validated by Rust below.
                let expanded = path.with_extension("expanded.tar");
                let output = fs::File::create(&expanded)?;
                let utility = if filename.ends_with(".bz2") || filename.ends_with(".tbz2") {
                    "bzip2"
                } else {
                    "xz"
                };
                let status = std::process::Command::new(utility)
                    .arg("-dc")
                    .arg(path)
                    .stdout(std::process::Stdio::from(output))
                    .stderr(std::process::Stdio::null())
                    .status()?;
                ensure!(status.success(), "archive decompression failed");
                Box::new(fs::File::open(expanded)?)
            } else if filename.ends_with(".tar") {
                Box::new(input)
            } else {
                bail!("supported archives: zip, tar, tar.gz, tar.bz2, tar.xz, tgz, tbz2, txz");
            };
        let mut archive = tar::Archive::new(reader);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let kind = entry.header().entry_type();
            ensure!(
                kind.is_file() || kind.is_dir(),
                "archive links and special files are not accepted"
            );
            let raw = entry.path()?.into_owned();
            let clean = raw.strip_prefix(".").unwrap_or(&raw);
            if clean.as_os_str().is_empty() {
                continue;
            }
            let out = target(directory, clean)?;
            if kind.is_dir() {
                fs::create_dir_all(out)?;
            } else {
                fs::create_dir_all(out.parent().unwrap())?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(out)?;
                std::io::copy(&mut entry, &mut file)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_targets_stay_inside_storage() {
        let root = Path::new("/data/staging");
        for path in ["../secret", "/etc/passwd", "a/../../b", "weights\\escape"] {
            assert!(target(root, Path::new(path)).is_err());
        }
        assert_eq!(
            target(root, Path::new("weights/model.gguf")).unwrap(),
            root.join("weights/model.gguf")
        );
    }
    #[test]
    fn tar_symlinks_are_rejected_before_extraction() {
        let root = std::env::temp_dir().join(format!("vs-archive-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("unsafe.tar");
        let mut builder = tar::Builder::new(fs::File::create(&archive).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_mode(0o777);
        header.set_link_name("/etc/passwd").unwrap();
        header.set_cksum();
        builder
            .append_data(&mut header, "weights.gguf", std::io::empty())
            .unwrap();
        builder.finish().unwrap();
        drop(builder);
        assert!(extract(&archive, &root.join("out"), "unsafe.tar").is_err());
        assert!(!root.join("out/weights.gguf").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
