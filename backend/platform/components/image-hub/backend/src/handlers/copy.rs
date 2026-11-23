//! 克隆 / 改名 handlers（本地 registry 内复制，D-IH-50 ~ D-IH-52 / D-IH-56 / D-IH-58）
//!
//! - `POST /api/registry/clone`：复制到新名称（源不变）
//! - `POST /api/registry/rename`：复制到新名称后删除源引用（按 tag 摘标签 / 删除源仓库）
//!
//! 单 tag 与整仓两种粒度：`tag` 缺省 = 整仓复制（`target_repository` 必填、`target_tag` 不适用）。
//! 目标占用 / 源不存在 / 名称非法在**创建任务前**校验，避免产生无效任务。

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::error::{ImageHubError, ResultBody, codes};
use crate::jobs::{CleanupAction, LocalCopyTarget};
use crate::registry::{validate_repository, validate_tag};
use crate::skopeo::{parse_image_ref, validate_retry_times, validate_transfer_timeout};
use crate::state::AppState;

/// `POST /api/registry/clone` / `rename` 请求体
#[derive(Deserialize)]
pub struct CopyRequest {
    /// 源仓库名（本地 registry 中的仓库名）
    pub repository: String,
    /// 源 tag；缺省 = 整仓复制
    #[serde(default)]
    pub tag: Option<String>,
    /// 目标仓库名；缺省 = 与源相同（仅单 tag 允许）
    #[serde(default)]
    pub target_repository: Option<String>,
    /// 目标 tag；缺省 = 与源 tag 相同
    #[serde(default)]
    pub target_tag: Option<String>,
    /// 单次传输超时（分钟，缺省 60；`0` = 无限制）
    #[serde(default)]
    pub timeout_minutes: Option<u64>,
    /// 传输失败重试次数（缺省 2，范围 1~5）
    #[serde(default)]
    pub retry_times: Option<u64>,
}

/// 任务已创建响应
#[derive(Serialize)]
pub struct CopyStarted {
    pub task_id: String,
}

/// `POST /api/registry/clone`
pub async fn clone_image(
    State(state): State<AppState>,
    Json(req): Json<CopyRequest>,
) -> Result<Json<ResultBody<CopyStarted>>, ImageHubError> {
    copy_or_rename(state, req, false).await
}

/// `POST /api/registry/rename`
pub async fn rename_image(
    State(state): State<AppState>,
    Json(req): Json<CopyRequest>,
) -> Result<Json<ResultBody<CopyStarted>>, ImageHubError> {
    copy_or_rename(state, req, true).await
}

/// 克隆 / 改名公共流程（全部校验通过后才创建任务）
async fn copy_or_rename(
    state: AppState,
    req: CopyRequest,
    rename: bool,
) -> Result<Json<ResultBody<CopyStarted>>, ImageHubError> {
    let source_repository = req.repository.trim().to_string();
    validate_repository(&source_repository)?;
    let source_tag = req
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let target_repository = req
        .target_repository
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let target_tag = req
        .target_tag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let timeout = validate_transfer_timeout(req.timeout_minutes)?;
    let retry_times = validate_retry_times(req.retry_times)?;

    let (targets, cleanup) = build_targets(
        &state,
        &source_repository,
        source_tag.as_deref(),
        target_repository,
        target_tag,
        rename,
    )
    .await?;

    let task_id = state.jobs.start_local_copy(
        rename,
        &source_repository,
        targets,
        cleanup,
        timeout,
        retry_times,
    );
    Ok(Json(ResultBody::ok(
        CopyStarted { task_id },
        state.version,
    )))
}

/// 校验源 / 目标并构造复制目标列表（+ 改名清理动作）
async fn build_targets(
    state: &AppState,
    source_repository: &str,
    source_tag: Option<&str>,
    target_repository: Option<&str>,
    target_tag: Option<&str>,
    rename: bool,
) -> Result<(Vec<LocalCopyTarget>, Option<CleanupAction>), ImageHubError> {
    match source_tag {
        Some(tag) => {
            validate_tag(tag)?;
            // 源必须存在（不存在 → 20803）
            state.registry.describe_manifest(source_repository, tag).await?;

            let target_repo_input = target_repository.unwrap_or(source_repository);
            let target_tag_input = target_tag.unwrap_or(tag);
            validate_tag(target_tag_input)?;
            if target_repo_input == source_repository && target_tag_input == tag {
                return Err(ImageHubError::new(
                    codes::PARAM_INVALID,
                    "目标与源相同，无需克隆/改名",
                ));
            }
            let image = parse_image_ref(&format!("{target_repo_input}:{target_tag_input}"))?;
            let target_repo_norm = image.upstream_repository();
            // 目标占用 → 20801（不覆盖）
            if state
                .registry
                .list_tags(&target_repo_norm)
                .await?
                .iter()
                .any(|t| t == target_tag_input)
            {
                return Err(ImageHubError::new(
                    codes::PARAM_INVALID,
                    format!("目标已存在：{target_repo_norm}:{target_tag_input}"),
                ));
            }
            let cleanup = rename.then(|| CleanupAction::Untag {
                repository: source_repository.to_string(),
                tag: tag.to_string(),
            });
            Ok((
                vec![LocalCopyTarget {
                    source_tag: tag.to_string(),
                    target: image,
                }],
                cleanup,
            ))
        }
        None => {
            // 整仓复制
            let target_repo_input = target_repository.ok_or_else(|| {
                ImageHubError::new(
                    codes::PARAM_INVALID,
                    "整仓克隆/改名必须提供 target_repository",
                )
            })?;
            if target_tag.is_some() {
                return Err(ImageHubError::new(
                    codes::PARAM_INVALID,
                    "整仓克隆/改名不支持 target_tag（各标签保持原名）",
                ));
            }
            if target_repo_input == source_repository {
                return Err(ImageHubError::new(
                    codes::PARAM_INVALID,
                    "目标仓库与源仓库相同，无需克隆/改名",
                ));
            }
            let source_tags = state.registry.list_tags(source_repository).await?;
            if source_tags.is_empty() {
                return Err(ImageHubError::new(
                    codes::MANIFEST_NOT_FOUND,
                    format!("仓库 {source_repository} 不存在或无标签"),
                ));
            }
            // 目标仓库必须不存在任何标签（不覆盖）
            let target_repo_norm = parse_image_ref(target_repo_input)?.upstream_repository();
            let existing = state.registry.list_tags(&target_repo_norm).await?;
            if !existing.is_empty() {
                return Err(ImageHubError::new(
                    codes::PARAM_INVALID,
                    format!("目标仓库已存在（{} 个标签）：{target_repo_norm}", existing.len()),
                ));
            }
            let targets: Vec<LocalCopyTarget> = source_tags
                .iter()
                .map(|tag| {
                    Ok(LocalCopyTarget {
                        source_tag: tag.clone(),
                        target: parse_image_ref(&format!("{target_repo_input}:{tag}"))?,
                    })
                })
                .collect::<Result<_, ImageHubError>>()?;
            let cleanup = rename.then(|| CleanupAction::DeleteRepository {
                repository: source_repository.to_string(),
            });
            Ok((targets, cleanup))
        }
    }
}