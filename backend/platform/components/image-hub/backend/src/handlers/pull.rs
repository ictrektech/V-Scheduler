//! 远程拉取 / 同步 handlers（skopeo 编排入口）
//!
//! - 请求体可选 `mirror`（镜像源）：仅作为**实际拉取来源**，不影响本地落地仓库名（D-IH-32）
//! - 请求体可选 `timeout_minutes`（单次上游传输超时，缺省 60 分钟，`0` = 无限制；D-IH-44）
//! - 请求体可选 `retry_times`（传输失败重试次数，缺省 2，范围 1~5；D-IH-45）

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::error::{ImageHubError, ResultBody, codes};
use crate::mirror::normalize_mirror;
use crate::skopeo::{
    ArchMode, parse_image_ref, split_registry_repository, validate_image_tag,
    validate_retry_times, validate_transfer_timeout,
};
use crate::state::AppState;

/// `POST /api/registry/pull` 请求体
#[derive(Deserialize)]
pub struct PullRequest {
    /// 完整上游镜像名，如 `docker.m.daocloud.io/library/alpine:3.20`
    pub image: String,
    /// 架构选择：`all`（缺省）/ `host` / 逗号分隔完整平台列表（docker 标准完整平台名，如
    /// `linux/amd64,linux/arm64`；裸架构名 `amd64` 与旧式 `arm/v7` 仍被归一兼容）
    #[serde(default)]
    pub arch: Option<String>,
    /// 是否对上游禁用 TLS 校验（缺省 true）
    #[serde(default = "default_insecure")]
    pub insecure: bool,
    /// 可选镜像源主机名（缺省/空白 = 不使用镜像源；支持手动输入任意合法主机名）
    #[serde(default)]
    pub mirror: Option<String>,
    /// 单次上游传输超时（分钟，缺省 60；`0` = 无限制；D-IH-44）
    #[serde(default)]
    pub timeout_minutes: Option<u64>,
    /// 传输失败重试次数（缺省 2，范围 1~5；D-IH-45）
    #[serde(default)]
    pub retry_times: Option<u64>,
}

/// `POST /api/registry/sync` 请求体
#[derive(Deserialize)]
pub struct SyncRequest {
    /// 本地 registry 中的仓库名（即上游完整镜像名），如 `docker.m.daocloud.io/library/alpine`
    pub repository: String,
    /// 指定 tag；缺省同步该仓库全部 tag
    #[serde(default)]
    pub tag: Option<String>,
    /// 架构选择：`all`（缺省）/ `host` / 逗号分隔完整平台列表（如 `linux/amd64,linux/arm64`）
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default = "default_insecure")]
    pub insecure: bool,
    /// 可选镜像源主机名（语义同拉取：仅改变实际拉取来源）
    #[serde(default)]
    pub mirror: Option<String>,
    /// 单次上游传输超时（分钟，缺省 60；`0` = 无限制）——同步时**每个标签各自计时**
    #[serde(default)]
    pub timeout_minutes: Option<u64>,
    /// 传输失败重试次数（缺省 2，范围 1~5）——同步时**每个标签各自计数**
    #[serde(default)]
    pub retry_times: Option<u64>,
}

fn default_insecure() -> bool {
    true
}

/// 任务已创建响应
#[derive(Serialize)]
pub struct TaskStarted {
    pub task_id: String,
}

/// `POST /api/registry/pull` —— 从上游拉取镜像并推入本地 registry（保留上游完整镜像名）
pub async fn pull_image(
    State(state): State<AppState>,
    Json(req): Json<PullRequest>,
) -> Result<Json<ResultBody<TaskStarted>>, ImageHubError> {
    let image = parse_image_ref(&req.image)?;
    let arch = ArchMode::from_request(req.arch.as_deref())?;
    let mirror = normalize_mirror(req.mirror.as_deref())?;
    let timeout = validate_transfer_timeout(req.timeout_minutes)?;
    let retry_times = validate_retry_times(req.retry_times)?;
    let task_id =
        state
            .jobs
            .start_pull(image, arch, req.insecure, mirror, timeout, retry_times);
    Ok(Json(ResultBody::ok(
        TaskStarted { task_id },
        state.version,
    )))
}

/// `POST /api/registry/sync` —— 对本地已有镜像重新从上游拉取（更新）
pub async fn sync_image(
    State(state): State<AppState>,
    Json(req): Json<SyncRequest>,
) -> Result<Json<ResultBody<TaskStarted>>, ImageHubError> {
    let full_repository = req.repository.trim().to_string();
    let (registry, repository) = split_registry_repository(&full_repository)?;
    let arch = ArchMode::from_request(req.arch.as_deref())?;
    let mirror = normalize_mirror(req.mirror.as_deref())?;

    let tags: Vec<String> = match req
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(tag) => {
            validate_image_tag(tag)?;
            vec![tag.to_string()]
        }
        None => {
            let tags = state.registry.list_tags(&full_repository).await?;
            if tags.is_empty() {
                return Err(ImageHubError::new(
                    codes::MANIFEST_NOT_FOUND,
                    format!("仓库 {full_repository} 无标签可同步"),
                ));
            }
            tags
        }
    };

    let timeout = validate_transfer_timeout(req.timeout_minutes)?;
    let retry_times = validate_retry_times(req.retry_times)?;
    let task_id = state.jobs.start_sync(
        registry,
        repository,
        tags,
        arch,
        req.insecure,
        mirror,
        timeout,
        retry_times,
    );
    Ok(Json(ResultBody::ok(
        TaskStarted { task_id },
        state.version,
    )))
}