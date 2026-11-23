//! 归档导入 handler（D-IH-53 ~ D-IH-55 / D-IH-57）
//!
//! `POST /api/registry/import`（`multipart/form-data`）：
//! - 必填 `file`：镜像归档（docker-archive / OCI 归档；后缀不限，压缩由内容识别）
//! - 可选 `repository` / `tag`：目标镜像名；缺省时保留归档内原始名称
//! - 可选 `timeout_minutes` / `retry_times`：单次传输超时与失败重试（语义同拉取/同步）
//!
//! 上传内容**逐 chunk 流式写入** `<data-dir>/tmp/import-*.bin`（不整包入内存）；
//! 归档识别 / 原始名称解析在**任务内**完成；任务终态清理临时文件。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::{
    Json,
    extract::{Multipart, State},
};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::error::{ImageHubError, ResultBody, codes};
use crate::jobs::ImportSpec;
use crate::skopeo::{parse_image_ref, validate_retry_times, validate_transfer_timeout};
use crate::state::AppState;

/// 任务已创建响应
#[derive(Serialize)]
pub struct ImportStarted {
    pub task_id: String,
}

/// 上传临时文件路径序号（同毫秒内多请求不冲突）
static IMPORT_SEQ: AtomicU64 = AtomicU64::new(0);

/// 生成上传临时文件路径
fn import_temp_path(tmpdir: &std::path::Path) -> PathBuf {
    let seq = IMPORT_SEQ.fetch_add(1, Ordering::SeqCst);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    tmpdir.join(format!("import-{millis}-{seq}.bin"))
}

/// 删除文件（不存在视为成功；失败仅告警）
fn remove_file_quiet(path: &std::path::Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(path = %path.display(), err = %e, "删除导入临时文件失败"),
    }
}

/// 上传文件守护：出错提前返回时自动清理；成功创建任务后 `keep`
struct TempGuard {
    path: PathBuf,
    keep: bool,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if !self.keep {
            remove_file_quiet(&self.path);
        }
    }
}

/// `POST /api/registry/import`
pub async fn import_archive(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ResultBody<ImportStarted>>, ImageHubError> {
    let tmpdir = state.data_dir.join("tmp");
    tokio::fs::create_dir_all(&tmpdir).await.map_err(|e| {
        ImageHubError::new(codes::IO_FAILED, format!("创建临时目录失败: {e}"))
    })?;
    let path = import_temp_path(&tmpdir);
    let mut guard = TempGuard {
        path: path.clone(),
        keep: false,
    };

    let mut original_name: Option<String> = None;
    let mut repository: Option<String> = None;
    let mut tag: Option<String> = None;
    let mut timeout_minutes: Option<u64> = None;
    let mut retry_times: Option<u64> = None;
    let mut saved = false;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ImageHubError::new(codes::PARAM_INVALID, format!("解析上传请求失败: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                original_name = field
                    .file_name()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let mut file = tokio::fs::File::create(&path).await.map_err(|e| {
                    ImageHubError::new(
                        codes::IO_FAILED,
                        format!("写入临时文件失败 {}: {e}", path.display()),
                    )
                })?;
                while let Some(chunk) = field.chunk().await.map_err(|e| {
                    ImageHubError::new(codes::PARAM_INVALID, format!("读取上传内容失败: {e}"))
                })? {
                    file.write_all(&chunk).await.map_err(|e| {
                        ImageHubError::new(codes::IO_FAILED, format!("写入临时文件失败: {e}"))
                    })?;
                }
                file.flush().await.map_err(|e| {
                    ImageHubError::new(codes::IO_FAILED, format!("写入临时文件失败: {e}"))
                })?;
                saved = true;
            }
            other => {
                let text = field.text().await.map_err(|e| {
                    ImageHubError::new(codes::PARAM_INVALID, format!("读取字段 {other} 失败: {e}"))
                })?;
                let value = text.trim().to_string();
                let value = if value.is_empty() { None } else { Some(value) };
                match other {
                    "repository" => repository = value,
                    "tag" => tag = value,
                    "timeout_minutes" => {
                        if let Some(v) = value {
                            timeout_minutes = Some(v.parse::<u64>().map_err(|_| {
                                ImageHubError::new(
                                    codes::PARAM_INVALID,
                                    format!("timeout_minutes 非法: {v}"),
                                )
                            })?);
                        }
                    }
                    "retry_times" => {
                        if let Some(v) = value {
                            retry_times = Some(v.parse::<u64>().map_err(|_| {
                                ImageHubError::new(
                                    codes::PARAM_INVALID,
                                    format!("retry_times 非法: {v}"),
                                )
                            })?);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if !saved {
        return Err(ImageHubError::new(
            codes::PARAM_INVALID,
            "缺少上传文件字段 file",
        ));
    }

    // 目标名合法性在创建任务前校验（引用格式）；归档原始名称的解析在任务内进行
    if let Some(repo) = repository.as_deref() {
        let probe = match tag.as_deref() {
            Some(tag) => format!("{repo}:{tag}"),
            None => repo.to_string(),
        };
        parse_image_ref(&probe)?;
    } else if let Some(tag) = tag.as_deref() {
        // 仅提供 tag 而无仓库名 → 无意义
        let _ = tag;
        return Err(ImageHubError::new(
            codes::PARAM_INVALID,
            "提供 tag 时必须同时提供 repository",
        ));
    }

    let timeout = validate_transfer_timeout(timeout_minutes)?;
    let retry_times = validate_retry_times(retry_times)?;

    let task_id = state.jobs.start_import(ImportSpec {
        path: path.clone(),
        original_name,
        requested_repository: repository,
        requested_tag: tag,
        transfer_timeout: timeout,
        retry_times,
    });
    // 任务接管临时文件（终态清理）
    guard.keep = true;
    Ok(Json(ResultBody::ok(
        ImportStarted { task_id },
        state.version,
    )))
}