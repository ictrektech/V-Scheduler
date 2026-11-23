//! ComfyUI 实例权重管理：从 model-hub 已下载权重挑选文件，按类型以**相对软链接**加入实例
//! 权重目录，并落库台账（`modelinfer_comfyui_weights`）。
//!
//! 目录布局（见 `specs/docs/design/modelinfer/comfyui.md`）：
//!
//! ```text
//! <model-hub-root>/comfyui/
//! ├── input/                       # 多实例共享输入
//! ├── output/                      # 多实例共享输出
//! └── <instance_id>/models/<type>/<name> -> ../../../../export/<src>/<org>/<repo>/current/<file>
//! ```
//!
//! 核心逻辑为纯函数 / 直接文件系统操作，便于单测；不依赖节点 NFS 挂载或部署运行状态。

use std::path::{Path, PathBuf};

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};

use crate::entities::comfyui_weight;
use crate::error::{ModelinferError, codes};
use crate::model_hub;
use crate::time;

/// ComfyUI 预设权重类型目录（挂进容器 `/root/ComfyUI/models` 与 extra_model_paths 的常见类型）
pub const PRESET_TYPES: [&str; 24] = [
    "checkpoints",
    "diffusion_models",
    "unet",
    "text_encoders",
    "clip",
    "clip_vision",
    "vae",
    "vae_approx",
    "loras",
    "controlnet",
    "embeddings",
    "upscale_models",
    "style_models",
    "hypernetworks",
    "photomaker",
    "gligen",
    "ipadapter",
    "model_patches",
    "diffusers",
    "configs",
    "audio_encoders",
    "safety_checker",
    "text_encoder",
    "llava",
];

/// 预设类型清单
pub fn preset_types() -> Vec<&'static str> {
    PRESET_TYPES.to_vec()
}

fn invalid(msg: impl Into<String>) -> ModelinferError {
    ModelinferError::new(codes::COMFYUI_WEIGHT_INVALID, msg)
}

/// 校验目标类型（允许相对子目录 `a/b`；拒绝空 / 绝对 / `..` / `.` / 空段）
pub fn sanitize_target_type(raw: &str) -> Result<String, ModelinferError> {
    let t = raw.trim().trim_matches('/');
    if t.is_empty() {
        return Err(invalid("目标类型不能为空"));
    }
    if raw.trim().starts_with('/') {
        return Err(invalid("目标类型不能是绝对路径"));
    }
    if t.split('/').any(|seg| seg.is_empty() || seg == ".." || seg == ".") {
        return Err(invalid(format!("目标类型含非法路径段: {raw}")));
    }
    Ok(t.to_string())
}

/// 校验链接名（单段文件名；拒绝空 / `/` / `..` / 隐藏文件）
pub fn sanitize_link_name(raw: &str) -> Result<String, ModelinferError> {
    let n = raw.trim();
    if n.is_empty() {
        return Err(invalid("链接名不能为空"));
    }
    if n.contains('/') || n == "." || n == ".." || n.starts_with('.') {
        return Err(invalid(format!("链接名非法: {raw}")));
    }
    Ok(n.to_string())
}

/// 校验 model-hub 变体内文件相对路径（拒绝空 / 绝对 / `..` / 空段）
pub fn sanitize_file_path(raw: &str) -> Result<String, ModelinferError> {
    let p = raw.trim().trim_matches('/');
    if p.is_empty() {
        return Err(invalid("权重文件路径不能为空"));
    }
    if raw.trim().starts_with('/') {
        return Err(invalid("权重文件路径不能是绝对路径"));
    }
    if p.split('/').any(|seg| seg.is_empty() || seg == ".." || seg == ".") {
        return Err(invalid(format!("权重文件路径含非法路径段: {raw}")));
    }
    Ok(p.to_string())
}

/// 校验来源
pub fn sanitize_source(raw: &str) -> Result<String, ModelinferError> {
    let s = raw.trim().to_ascii_lowercase();
    if !model_hub::SOURCES.contains(&s.as_str()) {
        return Err(invalid(format!("来源只能是 hf 或 ms: {raw}")));
    }
    Ok(s)
}

/// 计算软链接的相对目标（纯函数）
///
/// 从实例类型目录（`<root>/comfyui/<instance>/models/<type>/`）往上看 4 层到达 root。
pub fn rel_link_target(source: &str, weight_id: &str, file_path: &str) -> String {
    let weight_id = weight_id.trim().trim_matches('/');
    format!("../../../../export/{source}/{weight_id}/current/{file_path}")
}

/// 实例目录（`<root>/comfyui/<instance>`）
pub fn instance_dir(hub_dir: &Path, instance_id: &str) -> PathBuf {
    hub_dir.join("comfyui").join(instance_id)
}

/// 实例权重目录（`<root>/comfyui/<instance>/models`）
pub fn instance_models_dir(hub_dir: &Path, instance_id: &str) -> PathBuf {
    instance_dir(hub_dir, instance_id).join("models")
}

/// 实例内软链接绝对路径
pub fn link_abs_path(
    hub_dir: &Path,
    instance_id: &str,
    target_type: &str,
    link_name: &str,
) -> PathBuf {
    instance_models_dir(hub_dir, instance_id)
        .join(target_type)
        .join(link_name)
}

/// 请求体：添加实例权重
#[derive(Debug, Clone, Deserialize)]
pub struct AddWeightRequest {
    pub source: String,
    pub weight_id: String,
    pub file_path: String,
    pub target_type: String,
    /// 链接名（留空取源文件基名）
    #[serde(default)]
    pub link_name: String,
}

/// 实例权重视图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightView {
    pub id: String,
    pub deployment_id: String,
    pub source: String,
    pub weight_id: String,
    pub file_path: String,
    pub target_type: String,
    pub link_name: String,
    pub link_rel: String,
    pub target_rel: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<comfyui_weight::Model> for WeightView {
    fn from(m: comfyui_weight::Model) -> Self {
        Self {
            id: m.id,
            deployment_id: m.deployment_id,
            source: m.source,
            weight_id: m.weight_id,
            file_path: m.file_path,
            target_type: m.target_type,
            link_name: m.link_name,
            link_rel: m.link_rel,
            target_rel: m.target_rel,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

fn db_err(e: sea_orm::DbErr) -> ModelinferError {
    ModelinferError::new(codes::DB_FAILED, format!("数据库操作失败: {e}"))
}

/// 列举实例权重台账（按创建时间升序）
pub async fn list_weights(
    db: &DatabaseConnection,
    deployment_id: &str,
) -> Result<Vec<WeightView>, ModelinferError> {
    let rows = comfyui_weight::Entity::find()
        .filter(comfyui_weight::Column::DeploymentId.eq(deployment_id))
        .order_by_asc(comfyui_weight::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows.into_iter().map(WeightView::from).collect())
}

/// 添加实例权重：校验 → 建相对软链 → 落库
pub async fn add_weight(
    db: &DatabaseConnection,
    hub_dir: &Path,
    deployment_id: &str,
    req: &AddWeightRequest,
) -> Result<WeightView, ModelinferError> {
    let source = sanitize_source(&req.source)?;
    let target_type = sanitize_target_type(&req.target_type)?;
    let file_path = sanitize_file_path(&req.file_path)?;
    let weight_id = req.weight_id.trim().trim_matches('/').to_string();
    if weight_id.is_empty() {
        return Err(invalid("权重 id 不能为空"));
    }
    let default_name = file_path.rsplit('/').next().unwrap_or("").to_string();
    let link_name = sanitize_link_name(if req.link_name.trim().is_empty() {
        &default_name
    } else {
        req.link_name.as_str()
    })?;

    // 源文件必须存在（变体根 / current）
    let root = model_hub::variant_root(hub_dir, &source, &weight_id).ok_or_else(|| {
        ModelinferError::new(
            codes::MODEL_HUB_FILE_NOT_FOUND,
            format!("model-hub 中不存在模型变体: {source}:{weight_id}"),
        )
    })?;
    let src_abs = root.join(&file_path);
    if std::fs::metadata(&src_abs).is_err() {
        return Err(ModelinferError::new(
            codes::MODEL_HUB_FILE_NOT_FOUND,
            format!("model-hub 中不存在权重文件: {source}:{weight_id}/{file_path}"),
        ));
    }

    let link_rel = format!("{target_type}/{link_name}");
    let link_abs = link_abs_path(hub_dir, deployment_id, &target_type, &link_name);

    // 幂等保护：同名链接已存在（文件系统或台账）即拒绝
    if std::fs::symlink_metadata(&link_abs).is_ok() {
        return Err(ModelinferError::new(
            codes::COMFYUI_WEIGHT_EXISTS,
            format!("目标链接已存在: {link_rel}"),
        ));
    }
    let existing = comfyui_weight::Entity::find()
        .filter(comfyui_weight::Column::DeploymentId.eq(deployment_id))
        .filter(comfyui_weight::Column::LinkRel.eq(link_rel.as_str()))
        .one(db)
        .await
        .map_err(db_err)?;
    if existing.is_some() {
        return Err(ModelinferError::new(
            codes::COMFYUI_WEIGHT_EXISTS,
            format!("实例权重台账中已存在同名链接: {link_rel}"),
        ));
    }

    // 建目录 + 相对软链
    let parent = link_abs.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&parent).map_err(|e| {
        ModelinferError::new(codes::COMFYUI_WEIGHT_IO_FAILED, format!("创建类型目录失败: {e}"))
    })?;
    let target_rel = rel_link_target(&source, &weight_id, &file_path);
    std::os::unix::fs::symlink(&target_rel, &link_abs).map_err(|e| {
        ModelinferError::new(
            codes::COMFYUI_WEIGHT_IO_FAILED,
            format!("创建软链接失败（{link_rel} -> {target_rel}）: {e}"),
        )
    })?;

    let now = time::now_iso();
    let am = comfyui_weight::ActiveModel {
        id: Set(ulid::Ulid::new().to_string()),
        deployment_id: Set(deployment_id.to_string()),
        source: Set(source),
        weight_id: Set(weight_id),
        file_path: Set(file_path),
        target_type: Set(target_type),
        link_name: Set(link_name),
        link_rel: Set(link_rel),
        target_rel: Set(target_rel),
        created_at: Set(now.clone()),
        updated_at: Set(now),
    };
    let inserted = am.insert(db).await.map_err(db_err)?;
    Ok(WeightView::from(inserted))
}

/// 删除实例权重：移除软链（幂等）→ 删除台账记录
pub async fn remove_weight(
    db: &DatabaseConnection,
    hub_dir: &Path,
    deployment_id: &str,
    record_id: &str,
) -> Result<(), ModelinferError> {
    let row = comfyui_weight::Entity::find_by_id(record_id.to_string())
        .filter(comfyui_weight::Column::DeploymentId.eq(deployment_id))
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| {
            ModelinferError::new(codes::COMFYUI_WEIGHT_NOT_FOUND, format!("权重记录不存在: {record_id}"))
        })?;
    let link_abs = instance_models_dir(hub_dir, deployment_id).join(&row.link_rel);
    if std::fs::symlink_metadata(&link_abs).is_ok() {
        std::fs::remove_file(&link_abs).map_err(|e| {
            ModelinferError::new(codes::COMFYUI_WEIGHT_IO_FAILED, format!("删除软链接失败: {e}"))
        })?;
    }
    comfyui_weight::Entity::delete_by_id(record_id.to_string())
        .exec(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rules() {
        assert_eq!(sanitize_target_type("checkpoints").unwrap(), "checkpoints");
        assert_eq!(sanitize_target_type("my/sub").unwrap(), "my/sub");
        for bad in ["", "/abs", "../x", "a/../b", "a//b", "./a"] {
            assert_eq!(
                sanitize_target_type(bad).unwrap_err().code,
                codes::COMFYUI_WEIGHT_INVALID,
                "type {bad}"
            );
        }
        assert_eq!(sanitize_link_name("a.safetensors").unwrap(), "a.safetensors");
        for bad in ["", "a/b", "..", ".hidden"] {
            assert_eq!(sanitize_link_name(bad).unwrap_err().code, codes::COMFYUI_WEIGHT_INVALID);
        }
        assert_eq!(sanitize_file_path("checkpoints/a.safetensors").unwrap(), "checkpoints/a.safetensors");
        for bad in ["", "/a", "../a", "a//b"] {
            assert_eq!(sanitize_file_path(bad).unwrap_err().code, codes::COMFYUI_WEIGHT_INVALID);
        }
        assert_eq!(sanitize_source("HF").unwrap(), "hf");
        assert!(sanitize_source("xx").is_err());
    }

    #[test]
    fn rel_target_depth() {
        assert_eq!(
            rel_link_target("hf", "org/repo", "checkpoints/a.safetensors"),
            "../../../../export/hf/org/repo/current/checkpoints/a.safetensors"
        );
    }

    #[test]
    fn path_helpers() {
        let root = Path::new("/modelhub");
        assert_eq!(instance_models_dir(root, "INST"), Path::new("/modelhub/comfyui/INST/models"));
        assert_eq!(
            link_abs_path(root, "INST", "loras", "a.safetensors"),
            Path::new("/modelhub/comfyui/INST/models/loras/a.safetensors")
        );
    }

    #[tokio::test]
    async fn add_list_remove_roundtrip() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let hub = tmp.path();
        // 造一个变体
        let snap = hub.join("export/hf/org/repo/snapshots/rev/checkpoints");
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(snap.join("a.safetensors"), b"12345").unwrap();
        std::os::unix::fs::symlink("snapshots/rev", hub.join("export/hf/org/repo/current")).unwrap();

        let req = AddWeightRequest {
            source: "hf".into(),
            weight_id: "org/repo".into(),
            file_path: "checkpoints/a.safetensors".into(),
            target_type: "checkpoints".into(),
            link_name: String::new(),
        };
        let v = add_weight(&db, hub, "INST", &req).await.unwrap();
        assert_eq!(v.link_rel, "checkpoints/a.safetensors");
        assert_eq!(v.target_rel, "../../../../export/hf/org/repo/current/checkpoints/a.safetensors");
        // 文件系统上确有软链，且可解析
        let link = link_abs_path(hub, "INST", "checkpoints", "a.safetensors");
        assert!(std::fs::metadata(&link).is_ok());
        assert_eq!(std::fs::read(&link).unwrap(), b"12345");

        // 重复 → 20741
        assert_eq!(add_weight(&db, hub, "INST", &req).await.unwrap_err().code, codes::COMFYUI_WEIGHT_EXISTS);

        // 源不存在 → 20743
        let mut bad = req.clone();
        bad.file_path = "checkpoints/nope.safetensors".into();
        assert_eq!(add_weight(&db, hub, "INST", &bad).await.unwrap_err().code, codes::MODEL_HUB_FILE_NOT_FOUND);

        // 列举
        let list = list_weights(&db, "INST").await.unwrap();
        assert_eq!(list.len(), 1);

        // 删除
        remove_weight(&db, hub, "INST", &v.id).await.unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert!(list_weights(&db, "INST").await.unwrap().is_empty());
        // 再删 → 20742
        assert_eq!(remove_weight(&db, hub, "INST", &v.id).await.unwrap_err().code, codes::COMFYUI_WEIGHT_NOT_FOUND);
    }

    #[tokio::test]
    async fn add_invalid_params() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let req = AddWeightRequest {
            source: "hf".into(),
            weight_id: "org/repo".into(),
            file_path: "a.safetensors".into(),
            target_type: "../evil".into(),
            link_name: String::new(),
        };
        assert_eq!(
            add_weight(&db, tmp.path(), "INST", &req).await.unwrap_err().code,
            codes::COMFYUI_WEIGHT_INVALID
        );
    }
}