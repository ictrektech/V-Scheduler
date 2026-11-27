//! 运行配置级模型卡片脚本 override。
//!
//! 约束（见 `openspec/changes/modelinfer-local-node-and-card-file-overrides/design.md`）：
//! - 仅允许覆盖卡片根下 `scripts/` 中已存在的 UTF-8 普通文件；
//! - 不修改卡片包 / 原始目录；override 独立存表、按运行配置隔离；
//! - 部署分发模型包后按相对路径覆盖到节点。

use std::path::{Path, PathBuf};

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};

use crate::entities::{deployment, deployment_file_override};
use crate::error::{ModelinferError, codes};
use crate::plaza;
use crate::time;

pub const MAX_OVERRIDE_CONTENT_BYTES: usize = 1024 * 1024;
pub const MAX_OVERRIDE_FILES: usize = 64;
const MAX_SCRIPT_FILES: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub struct ScriptFileView {
    pub path: String,
    pub size_bytes: u64,
    /// 已有 override 时非空
    pub override_id: String,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileOverrideList {
    pub files: Vec<ScriptFileView>,
    pub overridden: usize,
    pub max_content_bytes: usize,
    pub max_files: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileOverrideDetail {
    pub id: String,
    pub deployment_id: String,
    pub path: String,
    pub content: String,
    pub original_content: String,
    pub size_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileOverrideCreate {
    pub path: String,
    /// 缺省 = 复制原始文件；UI 保存编辑内容时可直接提交
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileOverrideUpdate {
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CardFileQuery {
    pub path: String,
}

#[derive(Debug, Clone)]
struct ScriptFile {
    path: String,
    size_bytes: u64,
}

fn db_err(e: sea_orm::DbErr) -> ModelinferError {
    ModelinferError::new(codes::DB_FAILED, format!("数据库操作失败: {e}"))
}

fn invalid(msg: impl Into<String>) -> ModelinferError {
    ModelinferError::new(codes::FILE_OVERRIDE_INVALID, msg)
}

/// 归一 / 校验脚本相对路径
pub fn normalize_script_path(raw: &str) -> Result<String, ModelinferError> {
    let raw = raw.trim().replace('\\', "/");
    if raw.is_empty() {
        return Err(invalid("override 路径不能为空"));
    }
    if raw.starts_with('/') || raw.len() > 512 {
        return Err(invalid("override 路径必须是卡片内相对路径且不超过 512 字符"));
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(invalid(format!("override 路径含非法段: {raw}")));
        }
        parts.push(part);
    }
    if parts.first().copied() != Some("scripts") || parts.len() < 2 {
        return Err(invalid("仅允许覆盖卡片 scripts/ 下的文件"));
    }
    Ok(parts.join("/"))
}

fn script_root(card_root: &Path) -> PathBuf {
    card_root.join("scripts")
}

fn collect_scripts(root: &Path, prefix: &str, out: &mut Vec<ScriptFile>) -> Result<(), ModelinferError> {
    if out.len() >= MAX_SCRIPT_FILES {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_SCRIPT_FILES {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if meta.is_dir() {
            collect_scripts(&path, &rel, out)?;
        } else if meta.is_file() {
            out.push(ScriptFile {
                path: format!("scripts/{rel}"),
                size_bytes: meta.len(),
            });
        }
    }
    Ok(())
}

fn list_scripts(card_root: &Path) -> Result<Vec<ScriptFile>, ModelinferError> {
    let root = script_root(card_root);
    let meta = std::fs::symlink_metadata(&root).map_err(|_| {
        ModelinferError::new(
            codes::CARD_FILE_NOT_FOUND,
            format!("模型卡片不存在 scripts 目录: {}", root.display()),
        )
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(codes::CARD_FILE_NOT_FOUND)
            .map_err(|e| ModelinferError::new(e, "模型卡片 scripts 不是普通目录"));
    }
    let mut out = Vec::new();
    collect_scripts(&root, "", &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn resolve_script(card_root: &Path, raw: &str) -> Result<(String, PathBuf), ModelinferError> {
    let rel = normalize_script_path(raw)?;
    let path = card_root.join(&rel);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| {
        ModelinferError::new(codes::CARD_FILE_NOT_FOUND, format!("卡片文件不存在: {rel}"))
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(codes::CARD_FILE_NOT_FOUND).map_err(|e| {
            ModelinferError::new(e, format!("卡片文件不是普通文件: {rel}"))
        });
    }
    Ok((rel, path))
}

fn read_original(card_root: &Path, path: &str) -> Result<(String, PathBuf), ModelinferError> {
    let (rel, abs) = resolve_script(card_root, path)?;
    let bytes = std::fs::read(&abs).map_err(|e| {
        ModelinferError::new(codes::IO_FAILED, format!("读取卡片原始文件失败 {rel}: {e}"))
    })?;
    let content = String::from_utf8(bytes).map_err(|_| {
        invalid(format!("仅支持 UTF-8 文本脚本，文件不是有效 UTF-8: {rel}"))
    })?;
    Ok((content, PathBuf::from(rel)))
}

async fn deployment_card_root(db: &DatabaseConnection, deployment_id: &str) -> Result<PathBuf, ModelinferError> {
    let d = deployment::Entity::find_by_id(deployment_id.to_string())
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ModelinferError::new(codes::NOT_FOUND, format!("运行配置不存在: {deployment_id}")))?;
    let card = plaza::get_model(db, &d.plaza_model_id).await?;
    Ok(PathBuf::from(card.path))
}

fn validate_content(content: &str) -> Result<i64, ModelinferError> {
    let bytes = content.len();
    if bytes > MAX_OVERRIDE_CONTENT_BYTES {
        return Err(invalid(format!(
            "override 内容过大（{bytes} 字节，上限 {MAX_OVERRIDE_CONTENT_BYTES}）"
        )));
    }
    Ok(bytes as i64)
}

pub async fn list(
    db: &DatabaseConnection,
    deployment_id: &str,
) -> Result<FileOverrideList, ModelinferError> {
    let card_root = deployment_card_root(db, deployment_id).await?;
    let scripts = list_scripts(&card_root)?;
    let rows = deployment_file_override::Entity::find()
        .filter(deployment_file_override::Column::DeploymentId.eq(deployment_id))
        .order_by_asc(deployment_file_override::Column::Path)
        .all(db)
        .await
        .map_err(db_err)?;
    let mut files = Vec::with_capacity(scripts.len());
    for s in scripts {
        let hit = rows.iter().find(|r| r.path == s.path);
        files.push(ScriptFileView {
            path: s.path,
            size_bytes: hit
                .map(|r| r.size_bytes.max(0) as u64)
                .unwrap_or(s.size_bytes),
            override_id: hit.map(|r| r.id.clone()).unwrap_or_default(),
            updated_at: hit.map(|r| r.updated_at.clone()),
        });
    }
    Ok(FileOverrideList {
        overridden: rows.len(),
        files,
        max_content_bytes: MAX_OVERRIDE_CONTENT_BYTES,
        max_files: MAX_OVERRIDE_FILES,
    })
}

pub async fn get_original(
    db: &DatabaseConnection,
    deployment_id: &str,
    path: &str,
) -> Result<String, ModelinferError> {
    let card_root = deployment_card_root(db, deployment_id).await?;
    Ok(read_original(&card_root, path)?.0)
}

pub async fn create(
    db: &DatabaseConnection,
    deployment_id: &str,
    req: FileOverrideCreate,
) -> Result<FileOverrideDetail, ModelinferError> {
    let card_root = deployment_card_root(db, deployment_id).await?;
    let (original, rel_path) = read_original(&card_root, &req.path)?;
    let rel = rel_path.display().to_string();
    let existing = deployment_file_override::Entity::find()
        .filter(deployment_file_override::Column::DeploymentId.eq(deployment_id))
        .filter(deployment_file_override::Column::Path.eq(&rel))
        .one(db)
        .await
        .map_err(db_err)?;
    if existing.is_some() {
        return Err(invalid(format!("该脚本已存在 override: {rel}")));
    }
    let count = deployment_file_override::Entity::find()
        .filter(deployment_file_override::Column::DeploymentId.eq(deployment_id))
        .count(db)
        .await
        .map_err(db_err)?;
    if count as usize >= MAX_OVERRIDE_FILES {
        return Err(invalid(format!("单运行配置最多 {MAX_OVERRIDE_FILES} 个文件 override")));
    }
    let content = req.content.unwrap_or(original);
    let size_bytes = validate_content(&content)?;
    let now = time::now_iso();
    let am = deployment_file_override::ActiveModel {
        id: Set(ulid::Ulid::new().to_string()),
        deployment_id: Set(deployment_id.to_string()),
        path: Set(rel),
        content: Set(content),
        size_bytes: Set(size_bytes),
        created_at: Set(now.clone()),
        updated_at: Set(now),
    };
    let saved = am.insert(db).await.map_err(db_err)?;
    detail_from_model(&card_root, saved).await
}

async fn get_row(
    db: &DatabaseConnection,
    deployment_id: &str,
    override_id: &str,
) -> Result<deployment_file_override::Model, ModelinferError> {
    deployment_file_override::Entity::find_by_id(override_id.to_string())
        .one(db)
        .await
        .map_err(db_err)?
        .filter(|r| r.deployment_id == deployment_id)
        .ok_or_else(|| ModelinferError::new(codes::NOT_FOUND, format!("文件 override 不存在: {override_id}")))
}

async fn detail_from_model(
    card_root: &Path,
    row: deployment_file_override::Model,
) -> Result<FileOverrideDetail, ModelinferError> {
    let original_content = read_original(card_root, &row.path)?.0;
    Ok(FileOverrideDetail {
        id: row.id,
        deployment_id: row.deployment_id,
        path: row.path,
        content: row.content,
        original_content,
        size_bytes: row.size_bytes,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

pub async fn get(
    db: &DatabaseConnection,
    deployment_id: &str,
    override_id: &str,
) -> Result<FileOverrideDetail, ModelinferError> {
    let card_root = deployment_card_root(db, deployment_id).await?;
    let row = get_row(db, deployment_id, override_id).await?;
    detail_from_model(&card_root, row).await
}

pub async fn update(
    db: &DatabaseConnection,
    deployment_id: &str,
    override_id: &str,
    req: FileOverrideUpdate,
) -> Result<FileOverrideDetail, ModelinferError> {
    let card_root = deployment_card_root(db, deployment_id).await?;
    let row = get_row(db, deployment_id, override_id).await?;
    let size_bytes = validate_content(&req.content)?;
    let mut am: deployment_file_override::ActiveModel = row.into();
    am.content = Set(req.content);
    am.size_bytes = Set(size_bytes);
    am.updated_at = Set(time::now_iso());
    let saved = am.update(db).await.map_err(db_err)?;
    detail_from_model(&card_root, saved).await
}

pub async fn delete(
    db: &DatabaseConnection,
    deployment_id: &str,
    override_id: &str,
) -> Result<(), ModelinferError> {
    let row = get_row(db, deployment_id, override_id).await?;
    deployment_file_override::Entity::delete_by_id(row.id)
        .exec(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// 部署分发阶段读取全部 override（相对路径 + 文本内容）
pub async fn for_deployment(
    db: &DatabaseConnection,
    deployment_id: &str,
) -> Result<Vec<(String, String)>, ModelinferError> {
    let rows = deployment_file_override::Entity::find()
        .filter(deployment_file_override::Column::DeploymentId.eq(deployment_id))
        .order_by_asc(deployment_file_override::Column::Path)
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows.into_iter().map(|r| (r.path, r.content)).collect())
}

pub async fn delete_for_deployment(
    db: &DatabaseConnection,
    deployment_id: &str,
) -> Result<u64, ModelinferError> {
    let res = deployment_file_override::Entity::delete_many()
        .filter(deployment_file_override::Column::DeploymentId.eq(deployment_id))
        .exec(db)
        .await
        .map_err(db_err)?;
    Ok(res.rows_affected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::entities::plaza_model;

    fn make_card(root: &Path) {
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("manifest.yaml"), "id: a.b\nversion: 1.0.0\n").unwrap();
        std::fs::write(root.join("scripts/start.sh"), "#!/bin/sh\necho original\n").unwrap();
        std::fs::write(root.join("scripts/pre-start.sh"), "echo pre\n").unwrap();
    }

    async fn seed(db: &DatabaseConnection, card_root: &Path) {
        let now = time::now_iso();
        plaza_model::ActiveModel {
            id: Set("a.b@1.0.0".into()),
            model_id: Set("a.b".into()),
            model: Set(String::new()),
            name: Set("a".into()),
            version: Set("1.0.0".into()),
            source: Set("local".into()),
            source_id: Set(String::new()),
            path: Set(card_root.display().to_string()),
            package_path: Set(String::new()),
            package_sha256: Set(String::new()),
            manifest_json: Set("{}".into()),
            state: Set("available".into()),
            size_bytes: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now.clone()),
        }
        .insert(db)
        .await
        .unwrap();
        deployment::ActiveModel {
            id: Set("d1".into()),
            name: Set("d1".into()),
            plaza_model_id: Set("a.b@1.0.0".into()),
            card_id: Set("a.b".into()),
            weight_id: Set(String::new()),
            hub_source: Set("hf".into()),
            hub_rel_path: Set(String::new()),
            weight_override_source: Set(String::new()),
            weight_override_id: Set(String::new()),
            weight_override_rel_path: Set(String::new()),
            images_override_json: Set("[]".into()),
            extra_env_json: Set("{}".into()),
            node_ids_json: Set("[]".into()),
            head_node_id: Set(String::new()),
            configs_json: Set("{}".into()),
            serve_model_name: Set(String::new()),
            serve_api_key: Set(String::new()),
            serve_port: Set(8000),
            serve_url_prefix: Set("/v1".into()),
            vrouter_state: Set("unregistered".into()),
            vrouter_upstream_id: Set(String::new()),
            vrouter_model: Set(String::new()),
            vrouter_registered_at: Set(None),
            vrouter_last_error: Set(String::new()),
            local_proxy_enabled: Set(false),
            local_proxy_port: Set(0),
            local_proxy_active_port: Set(0),
            local_proxy_last_error: Set(String::new()),
            inference_engine: Set(String::new()),
            inference_engine_version: Set(String::new()),
            inference_engine_source: Set(String::new()),
            metrics_state: Set("unregistered".into()),
            metrics_service_id: Set(String::new()),
            metrics_registered_at: Set(None),
            metrics_last_error: Set(String::new()),
            auto_restart_policy: Set("never".into()),
            auto_restart_fail_count: Set(10),
            auto_restart_enabled: Set(false),
            auto_restart_fail_secs: Set(300),
            auto_restart_consecutive_failures: Set(0),
            auto_restart_ignore_before: Set(String::new()),
            auto_restart_phase: Set(String::new()),
            auto_restart_failed_since: Set(String::new()),
            auto_restart_next_at: Set(String::new()),
            auto_restart_attempts: Set(0),
            auto_restart_last_at: Set(String::new()),
            auto_restart_last_error: Set(String::new()),
            state: Set("stopped".into()),
            desired_state: Set("stopped".into()),
            health_status: Set("unknown".into()),
            last_error: Set(String::new()),
            last_started_at: Set(None),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
    }

    #[test]
    fn path_rules() {
        assert_eq!(normalize_script_path("scripts/start.sh").unwrap(), "scripts/start.sh");
        assert_eq!(normalize_script_path("scripts/a/b.sh").unwrap(), "scripts/a/b.sh");
        for bad in ["manifest.yaml", "scripts/../manifest.yaml", "/scripts/a.sh", "scripts", "configs/a"] {
            assert_eq!(normalize_script_path(bad).unwrap_err().code, codes::FILE_OVERRIDE_INVALID, "{bad}");
        }
    }

    #[tokio::test]
    async fn create_update_delete_roundtrip_and_original_stays() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("card");
        make_card(&root);
        seed(&db, &root).await;

        let list = list(&db, "d1").await.unwrap();
        assert_eq!(list.files.len(), 2);
        assert!(list.files.iter().all(|f| f.override_id.is_empty()));

        let created = create(&db, "d1", FileOverrideCreate {
            path: "scripts/start.sh".into(),
            content: None,
        })
        .await
        .unwrap();
        assert!(created.content.contains("original"));
        assert_eq!(std::fs::read_to_string(root.join("scripts/start.sh")).unwrap(), "#!/bin/sh\necho original\n");

        let updated = update(&db, "d1", &created.id, FileOverrideUpdate {
            content: "#!/bin/sh\necho override\n".into(),
        })
        .await
        .unwrap();
        assert!(updated.content.contains("override"));
        let items = for_deployment(&db, "d1").await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].1.contains("override"));

        delete(&db, "d1", &created.id).await.unwrap();
        assert!(for_deployment(&db, "d1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_manifest_and_missing_script() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("card");
        make_card(&root);
        seed(&db, &root).await;

        let err = create(&db, "d1", FileOverrideCreate {
            path: "manifest.yaml".into(),
            content: None,
        }).await.unwrap_err();
        assert_eq!(err.code, codes::FILE_OVERRIDE_INVALID);

        let err = create(&db, "d1", FileOverrideCreate {
            path: "scripts/nope.sh".into(),
            content: None,
        }).await.unwrap_err();
        assert_eq!(err.code, codes::CARD_FILE_NOT_FOUND);
    }
}
