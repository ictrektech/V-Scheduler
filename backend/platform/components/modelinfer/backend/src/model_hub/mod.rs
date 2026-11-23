//! model-hub 共享目录接入
//!
//! model-hub（`com.ictrek.model-hub`）把下载完成的模型按稳定目录结构导出：
//!
//! ```text
//! <hub_dir>/
//! ── export/
//!     ├── hf/<org>/<repo>/current -> snapshots/<revision>
//!     ── ms/<org>/<repo>/current -> snapshots/<revision>
//! ```
//!
//! 其他应用只读 `export/<hf|ms>/<org>/<repo>/current`（见 model-hub README）。
//! 本模块提供目录扫描与按权重 id 查找变体（纯 IO，便于用临时目录单测）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 一个可用的模型变体（已下载就绪）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelHubVariant {
    /// 来源：hf / ms
    pub source: String,
    /// 权重 id（`<org>/<repo>`）
    pub model_id: String,
    /// 相对 hub 根的路径（如 export/hf/Qwen/Qwen3-0.6B/current）
    pub rel_path: String,
    /// 绝对路径
    pub abs_path: String,
}

/// hub 目录概览
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHubInfo {
    pub dir: String,
    pub exists: bool,
    pub hf_count: usize,
    pub ms_count: usize,
    pub models: Vec<ModelHubVariant>,
}

/// 支持的来源目录名
pub const SOURCES: [&str; 2] = ["hf", "ms"];

/// 扫描单个来源目录（`export/<source>`）
pub fn scan_source(hub_dir: &Path, source: &str) -> Vec<ModelHubVariant> {
    let root = hub_dir.join("export").join(source);
    let mut out = Vec::new();
    let Ok(orgs) = std::fs::read_dir(&root) else {
        return out;
    };
    for org in orgs.flatten() {
        if !org.path().is_dir() {
            continue;
        }
        let org_name = org.file_name().to_string_lossy().to_string();
        let Ok(repos) = std::fs::read_dir(org.path()) else {
            continue;
        };
        for repo in repos.flatten() {
            if !repo.path().is_dir() {
                continue;
            }
            let repo_name = repo.file_name().to_string_lossy().to_string();
            let current = repo.path().join("current");
            if !current.exists() {
                continue;
            }
            let model_id = format!("{org_name}/{repo_name}");
            let rel_path = format!("export/{source}/{model_id}/current");
            out.push(ModelHubVariant {
                source: source.to_string(),
                model_id,
                rel_path,
                abs_path: current.display().to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.model_id.cmp(&b.model_id));
    out
}

/// 扫描全部来源
pub fn scan_models(hub_dir: &Path) -> Vec<ModelHubVariant> {
    let mut out = Vec::new();
    for s in SOURCES {
        out.extend(scan_source(hub_dir, s));
    }
    out
}

/// 按权重 id 查找匹配变体（hf / ms）
pub fn find_variants(hub_dir: &Path, weight_id: &str) -> Vec<ModelHubVariant> {
    let weight_id = weight_id.trim().trim_matches('/');
    if weight_id.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for s in SOURCES {
        let rel = format!("export/{s}/{weight_id}/current");
        let abs = hub_dir.join(&rel);
        if abs.exists() {
            out.push(ModelHubVariant {
                source: s.to_string(),
                model_id: weight_id.to_string(),
                rel_path: rel,
                abs_path: abs.display().to_string(),
            });
        }
    }
    out
}

/// 汇总 hub 信息
pub fn info(hub_dir: &Path) -> ModelHubInfo {
    let exists = hub_dir.exists();
    let models = if exists { scan_models(hub_dir) } else { Vec::new() };
    let hf_count = models.iter().filter(|m| m.source == "hf").count();
    let ms_count = models.iter().filter(|m| m.source == "ms").count();
    ModelHubInfo {
        dir: hub_dir.display().to_string(),
        exists,
        hf_count,
        ms_count,
        models,
    }
}

/// 节点上的模型目录：挂载目录 + 子路径 + 变体相对路径
pub fn node_model_path(mount_dir: &str, sub_path: &str, variant_rel: &str) -> String {
    let base = PathBuf::from(mount_dir);
    let mut p = base;
    for seg in sub_path.split('/').filter(|s| !s.is_empty()) {
        p = p.join(seg);
    }
    for seg in variant_rel.split('/').filter(|s| !s.is_empty()) {
        p = p.join(seg);
    }
    p.display().to_string()
}

/// 变体内一个文件的相对路径与大小（供 ComfyUI 权重挑选）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantFile {
    /// 相对变体根（`current`）的路径，如 `checkpoints/a.safetensors`
    pub rel_path: String,
    /// 文件大小（字节；无法解析时为 0）
    pub size_bytes: u64,
}

/// 单个变体最多返回的文件数（防御超大仓库）
pub const MAX_VARIANT_FILES: usize = 10_000;

/// 定位变体根（`export/<source>/<weight>/current`）；不存在返回 None
pub fn variant_root(hub_dir: &Path, source: &str, weight_id: &str) -> Option<PathBuf> {
    let weight_id = weight_id.trim().trim_matches('/');
    if weight_id.is_empty() || !SOURCES.contains(&source) {
        return None;
    }
    let p = hub_dir.join("export").join(source).join(weight_id).join("current");
    p.exists().then_some(p)
}

/// 列举变体根下的文件（含子目录；不跟随目录符号链接，避免环 / 越界）
///
/// 仅返回普通文件与文件符号链接；`rel_path` 相对变体根。
pub fn list_variant_files(
    hub_dir: &Path,
    source: &str,
    weight_id: &str,
) -> Result<Vec<VariantFile>, crate::error::ModelinferError> {
    let root = variant_root(hub_dir, source, weight_id).ok_or_else(|| {
        crate::error::ModelinferError::new(
            crate::error::codes::MODEL_HUB_FILE_NOT_FOUND,
            format!("model-hub 中不存在模型变体: {source}:{weight_id}"),
        )
    })?;
    let mut out = Vec::new();
    walk_files(&root, "", &mut out);
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// 递归遍历（只进入真实目录；跳过隐藏目录与未完成标记文件）
fn walk_files(dir: &Path, prefix: &str, out: &mut Vec<VariantFile>) {
    if out.len() >= MAX_VARIANT_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_VARIANT_FILES {
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.ends_with(".incomplete") || name.ends_with(".tmp") {
            continue;
        }
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        // 用 symlink_metadata 判断类型：目录符号链接不进入（避免越界 / 环）
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.is_dir() {
            walk_files(&entry.path(), &rel, out);
        } else {
            // 文件符号链接：跟随取大小（best-effort）
            let size = std::fs::metadata(entry.path()).map(|m| m.len()).unwrap_or(0);
            out.push(VariantFile { rel_path: rel, size_bytes: size });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p).unwrap();
    }

    #[test]
    fn scan_and_find() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        touch(&root.join("export/hf/Qwen/Qwen3-0.6B/current"));
        touch(&root.join("export/ms/Qwen/Qwen3-0.6B/current"));
        touch(&root.join("export/hf/other/repo/current"));
        // 未就绪（无 current）
        touch(&root.join("export/hf/Qwen/NotReady/snapshots/x"));

        let all = scan_models(root);
        assert_eq!(all.len(), 3);
        let variants = find_variants(root, "Qwen/Qwen3-0.6B");
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].source, "hf");
        assert!(find_variants(root, "Qwen/Nope").is_empty());

        let i = info(root);
        assert!(i.exists);
        assert_eq!(i.hf_count, 2);
        assert_eq!(i.ms_count, 1);
    }

    #[test]
    fn node_path_compose() {
        assert_eq!(
            node_model_path("/mnt/model_hub", "public/modelhub", "export/hf/Qwen/Qwen3-0.6B/current"),
            "/mnt/model_hub/public/modelhub/export/hf/Qwen/Qwen3-0.6B/current"
        );
    }

    #[test]
    fn list_variant_files_walks_files_and_ignores_hidden() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let snap = root.join("export/hf/org/repo/snapshots/rev");
        std::fs::create_dir_all(snap.join("checkpoints")).unwrap();
        std::fs::write(snap.join("checkpoints/a.safetensors"), b"abcde").unwrap();
        std::fs::write(snap.join("model_index.json"), b"{}").unwrap();
        std::fs::write(snap.join("model.safetensors.incomplete"), b"x").unwrap();
        std::fs::create_dir_all(snap.join(".git")).unwrap();
        std::fs::write(snap.join(".git/config"), b"x").unwrap();
        // current -> snapshots/rev
        std::os::unix::fs::symlink("snapshots/rev", root.join("export/hf/org/repo/current")).unwrap();

        let files = list_variant_files(root, "hf", "org/repo").unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert!(names.contains(&"checkpoints/a.safetensors"), "{names:?}");
        assert!(names.contains(&"model_index.json"), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("incomplete")), "{names:?}");
        assert!(!names.iter().any(|n| n.starts_with(".git")), "{names:?}");
        let a = files.iter().find(|f| f.rel_path == "checkpoints/a.safetensors").unwrap();
        assert_eq!(a.size_bytes, 5);
    }

    #[test]
    fn list_variant_files_missing_variant_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = list_variant_files(tmp.path(), "hf", "org/nope").unwrap_err();
        assert_eq!(err.code, crate::error::codes::MODEL_HUB_FILE_NOT_FOUND);
    }
}