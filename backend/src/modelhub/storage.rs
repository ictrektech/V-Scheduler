use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::fd::AsRawFd,
    path::{Component, Path, PathBuf},
};

// Python filelock on Linux uses flock on exactly these files. Never replace the lock inode.
pub struct FileLock(File);
impl FileLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let start = std::time::Instant::now();
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Self(file));
            }
            ensure!(start.elapsed().as_secs() < 30, "shared registry is busy");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}
impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        for dir in [
            "registry",
            "shared/tasks",
            "shared/tmp",
            "shared/manifests",
            "shared/locks",
            "export/hf",
            "export/ms",
            "ollama/models",
        ] {
            fs::create_dir_all(root.join(dir))?;
        }
        Ok(Self { root })
    }
    pub fn registry<T>(
        &self,
        name: &str,
        change: impl FnOnce(&mut Value) -> Result<T>,
    ) -> Result<T> {
        let _lock = FileLock::acquire(&self.root.join(format!("shared/locks/{name}.json.lock")))?;
        let path = self.root.join(format!("registry/{name}.json"));
        let mut data = read(&path)?.unwrap_or(json!({}));
        ensure!(
            data.is_object(),
            "invalid {name} registry; preserved without changes"
        );
        let original = data.clone();
        let result = change(&mut data)?;
        if data != original {
            atomic_json(&path, &data)?;
        }
        Ok(result)
    }
    pub fn entries(&self, name: &str) -> Result<Value> {
        self.registry(name, |data| Ok(data.clone()))
    }
    pub fn model(&self, id: &str) -> Result<Value> {
        let aliases = self.entries("aliases")?;
        let id = aliases[id].as_str().unwrap_or(id);
        let mut value = self.entries("models")?[id].clone();
        ensure!(!value.is_null(), "model not found");
        value["model_id"] = json!(id);
        Ok(value)
    }
    pub fn put_model(&self, model: Value) -> Result<()> {
        let id = model["model_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing model id"))?
            .to_owned();
        self.registry("models", |data| {
            data[&id] = model;
            Ok(())
        })
    }
    pub fn task_path(&self, id: &str) -> Result<PathBuf> {
        safe_relative(id)?;
        ensure!(!id.contains('/'), "invalid task id");
        Ok(self.root.join("shared/tasks").join(format!("{id}.json")))
    }
    pub fn task(&self, id: &str) -> Result<Value> {
        read(&self.task_path(id)?)?.ok_or_else(|| anyhow::anyhow!("task not found"))
    }
    pub fn put_task(&self, task: &Value) -> Result<()> {
        atomic_json(
            &self.task_path(task["task_id"].as_str().unwrap_or(""))?,
            task,
        )
    }
    pub fn tasks(&self) -> Result<Vec<Value>> {
        let mut items = vec![];
        for entry in fs::read_dir(self.root.join("shared/tasks"))? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|x| x == "json") {
                if let Ok(Some(mut value)) = read(&entry.path()) {
                    value["_mtime"] = json!(
                        entry
                            .metadata()?
                            .modified()?
                            .duration_since(std::time::UNIX_EPOCH)?
                            .as_secs_f64()
                    );
                    items.push(value);
                }
            }
        }
        items.sort_by(|a, b| {
            b["_mtime"]
                .as_f64()
                .partial_cmp(&a["_mtime"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(items)
    }
    pub fn model_root(&self, source: &str, id: &str) -> Result<PathBuf> {
        if source == "ollama" {
            return Ok(self.root.join("ollama/models"));
        }
        ensure!(matches!(source, "hf" | "ms"), "unknown model source");
        let prefix = format!("{source}://");
        let relative = safe_relative(id.strip_prefix(&prefix).unwrap_or(id))?;
        Ok(self.root.join("export").join(source).join(relative))
    }
    pub fn access(&self, model: &Value) -> Result<PathBuf> {
        let raw = PathBuf::from(
            model["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing model path"))?,
        );
        // Old VOS mounts use /data too. Never rewrite stored paths during adoption.
        let root = raw.canonicalize()?;
        ensure!(
            root.starts_with(self.root.canonicalize()?),
            "model path is outside shared storage"
        );
        let current = root.join("current");
        if current.exists() {
            let target = current.canonicalize()?;
            ensure!(target.starts_with(&root), "unsafe current link");
            return Ok(target);
        }
        let snapshots = root.join("snapshots");
        if snapshots.is_dir() {
            let mut dirs: Vec<_> = fs::read_dir(snapshots)?
                .filter_map(|x| x.ok())
                .filter(|x| x.path().is_dir())
                .collect();
            dirs.sort_by_key(|x| x.metadata().and_then(|m| m.modified()).ok());
            if let Some(dir) = dirs.last() {
                let target = dir.path().canonicalize()?;
                ensure!(target.starts_with(&root), "unsafe snapshot link");
                return Ok(target);
            }
        }
        Ok(root)
    }
    pub fn local_files(&self, model: &Value) -> Result<Vec<Value>> {
        ensure!(
            model["source"] != "ollama",
            "file selection is available for HF/MS repositories"
        );
        let managed = PathBuf::from(model["path"].as_str().unwrap_or("")).canonicalize()?;
        let access = self.access(model)?;
        let mut result = vec![];
        collect_files(&access, &access, &managed, &mut result)?;
        result.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        Ok(result)
    }
}
pub fn read(path: &Path) -> Result<Option<Value>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
pub fn safe_relative(value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty()
            && !value.contains(['\\', '*', '?', '[', ']'])
            && !value.chars().any(char::is_control),
        "invalid relative file path"
    );
    let path = Path::new(value);
    ensure!(
        path.components().all(|p| matches!(p, Component::Normal(_))),
        "invalid relative file path"
    );
    Ok(path.to_owned())
}
pub fn repository<'a>(source: &str, id: &'a str) -> Result<&'a str> {
    ensure!(matches!(source, "hf" | "ms"), "unknown repository source");
    let prefix = format!("{source}://");
    let repo = id.strip_prefix(&prefix).unwrap_or(id);
    ensure!(
        repo.split('/').count() == 2
            && !repo
                .chars()
                .any(|c| c.is_whitespace() || ":?\\*[]#%".contains(c)),
        "repository must be org/repo"
    );
    safe_relative(repo)?;
    Ok(repo)
}
pub fn normalize_id(source: &str, id: &str) -> Result<String> {
    if source == "ollama" {
        let name = id.trim().strip_prefix("ollama://").unwrap_or(id.trim());
        ensure!(
            !name.is_empty() && !name.contains("://") && !name.chars().any(char::is_whitespace),
            "invalid Ollama model"
        );
        return Ok(format!("ollama://{name}"));
    }
    Ok(format!("{source}://{}", repository(source, id.trim())?))
}
pub fn public_task(mut task: Value) -> Value {
    if let Some(obj) = task.as_object_mut() {
        obj.retain(|key, _| !key.starts_with('_'));
    }
    task
}
pub fn terminal(task: &Value) -> bool {
    matches!(task["phase"].as_str(), Some("READY" | "FAILED"))
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn collect_files(base: &Path, dir: &Path, managed: &Path, items: &mut Vec<Value>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if [".cache", ".git", ".msc", ".mdl", ".mv"].contains(&name.as_str())
            || [".incomplete", ".tmp", ".lock"]
                .iter()
                .any(|s| name.ends_with(s))
        {
            continue;
        }
        let target = match path.canonicalize() {
            Ok(p) if p.starts_with(managed) => p,
            _ => continue,
        };
        if path.is_dir() {
            if !entry.file_type()?.is_symlink() {
                collect_files(base, &path, managed, items)?;
            }
        } else if target.is_file() {
            items.push(json!({"path":path.strip_prefix(base)?.to_string_lossy(),"size":fs::metadata(target)?.len(),"downloaded":true}));
        }
    }
    Ok(())
}
pub fn copy_files(source: &Path, target: &Path, managed: &Path) -> Result<()> {
    let mut files = vec![];
    collect_files(source, source, managed, &mut files)?;
    for value in files {
        let relative = safe_relative(value["path"].as_str().unwrap())?;
        let to = target.join(&relative);
        fs::create_dir_all(to.parent().unwrap())?;
        if fs::hard_link(source.join(&relative), &to).is_err() {
            fs::copy(source.join(&relative), to)?;
        }
    }
    Ok(())
}
pub fn size(path: &Path) -> u64 {
    if path.is_file() {
        return path.metadata().map(|x| x.len()).unwrap_or(0);
    }
    fs::read_dir(path)
        .map(|r| {
            r.filter_map(|x| x.ok())
                .filter(|x| !x.file_type().is_ok_and(|t| t.is_symlink()))
                .map(|x| size(&x.path()))
                .sum()
        })
        .unwrap_or(0)
}
pub fn publish(root: &Path, revision: &str) -> Result<()> {
    safe_relative(revision)?;
    ensure!(!revision.contains('/'), "invalid revision");
    let temp = root.join(format!(".current-{}", uuid::Uuid::new_v4()));
    std::os::unix::fs::symlink(format!("snapshots/{revision}"), &temp)?;
    let current = root.join("current");
    if current.exists() && !current.is_symlink() {
        bail!("current is not a symlink; preserving existing directory");
    }
    fs::rename(temp, current)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("vs-modelhub-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn selected_paths_reject_traversal_and_globs() {
        for path in [
            "",
            "../secret",
            "/etc/passwd",
            "a/../../b",
            "weights\\x",
            "*.gguf",
            "model?.bin",
            "[ab].bin",
            "a\nfile",
        ] {
            assert!(safe_relative(path).is_err(), "accepted {path:?}");
        }
        assert_eq!(
            safe_relative("weights/model-Q4_K_M.gguf").unwrap(),
            PathBuf::from("weights/model-Q4_K_M.gguf")
        );
        for id in ["../repo", "org/../repo", "org/repo?token=x", "org//repo"] {
            assert!(repository("hf", id).is_err());
        }
    }
    #[test]
    fn adopts_original_registry_without_rewriting_weights_or_metadata() {
        let temp = Fixture::new();
        let store = Store::new(temp.0.clone()).unwrap();
        let root = temp.0.join("export/hf/org/repo");
        let snapshot = root.join("snapshots/old");
        fs::create_dir_all(&snapshot).unwrap();
        fs::write(snapshot.join("weights.gguf"), b"existing-weights").unwrap();
        publish(&root, "old").unwrap();
        let original = json!({"hf://org/repo":{"name":"Original name","source":"hf","status":"ready","path":root,"version":"old","size":16,"files":["weights.gguf"],"extra_legacy_field":"keep"}});
        atomic_json(&temp.0.join("registry/models.json"), &original).unwrap();
        let bytes = fs::read(temp.0.join("registry/models.json")).unwrap();
        let model = store.model("hf://org/repo").unwrap();
        assert_eq!(
            store.local_files(&model).unwrap()[0]["path"],
            "weights.gguf"
        );
        assert_eq!(
            fs::read(temp.0.join("registry/models.json")).unwrap(),
            bytes
        );
        assert_eq!(store.entries("models").unwrap(), original);
        store
            .registry("aliases", |v| {
                v["original"] = json!("hf://org/repo");
                Ok(())
            })
            .unwrap();
        assert_eq!(store.model("original").unwrap()["name"], "Original name");
        assert_eq!(
            fs::read(snapshot.join("weights.gguf")).unwrap(),
            b"existing-weights"
        );
    }
    #[test]
    fn local_file_listing_allows_managed_blob_links_but_excludes_escape_and_partials() {
        let temp = Fixture::new();
        let store = Store::new(temp.0.clone()).unwrap();
        let root = temp.0.join("export/hf/org/repo");
        let snapshot = root.join("snapshots/old");
        fs::create_dir_all(&snapshot).unwrap();
        fs::create_dir_all(root.join("blobs")).unwrap();
        fs::write(root.join("blobs/abc"), b"weight").unwrap();
        fs::write(snapshot.join("partial.incomplete"), b"unfinished").unwrap();
        fs::create_dir_all(snapshot.join(".cache")).unwrap();
        fs::write(snapshot.join(".cache/hidden"), b"cache").unwrap();
        std::os::unix::fs::symlink("../../blobs/abc", snapshot.join("weights.gguf")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", snapshot.join("escape")).unwrap();
        publish(&root, "old").unwrap();
        let model = json!({"source":"hf","path":root});
        let files = store.local_files(&model).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["size"], 6);
    }
    #[test]
    fn corrupt_registry_is_preserved_and_rejected() {
        let temp = Fixture::new();
        let store = Store::new(temp.0.clone()).unwrap();
        let path = temp.0.join("registry/models.json");
        fs::write(&path, b"broken JSON").unwrap();
        assert!(store.entries("models").is_err());
        assert_eq!(fs::read(path).unwrap(), b"broken JSON");
    }
    #[test]
    fn publishing_keeps_old_snapshot_and_requires_relative_target() {
        let temp = Fixture::new();
        let root = temp.0.join("repo");
        fs::create_dir_all(root.join("snapshots/old")).unwrap();
        fs::create_dir_all(root.join("snapshots/new")).unwrap();
        publish(&root, "old").unwrap();
        publish(&root, "new").unwrap();
        assert_eq!(
            fs::read_link(root.join("current")).unwrap(),
            PathBuf::from("snapshots/new")
        );
        assert!(root.join("snapshots/old").is_dir());
        assert!(publish(&root, "../outside").is_err());
    }
}

pub fn directory_sha256(root: &Path) -> Result<Option<String>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut files = vec![];
    let managed = root.canonicalize()?;
    collect_files(root, root, &managed, &mut files)?;
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    if files.is_empty() {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    for value in files {
        let mut file = File::open(root.join(value["path"].as_str().unwrap()))?;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
    }
    Ok(Some(format!("sha256:{:x}", hash.finalize())))
}
