//! registry 垃圾回收（GC）handlers（D-IH-59 ~ D-IH-66）。
//!
//! - `GET /api/registry/gc`：GC 状态（自动回收配置 + 能力 + 存储占用 + 运行中任务）
//! - `PUT /api/registry/gc`：更新自动回收配置（默认不启用；启用即以当前时刻为计时基准）
//! - `POST /api/registry/gc/run`：手动触发（异步任务，支持预演 / 删除无标签 manifest）

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::error::{ImageHubError, ResultBody};
use crate::gc::{
    GcRequest, compute_next_run_at, dir_size_bytes, validate_gc_interval,
};
use crate::handlers::pull::TaskStarted;
use crate::jobs::now_rfc3339;
use crate::skopeo::validate_transfer_timeout;
use crate::state::AppState;

/// GC 状态 / 配置响应
#[derive(Serialize)]
pub struct GcStatusData {
    /// 是否启用自动回收（默认 false）
    pub enabled: bool,
    /// 自动回收间隔（小时）
    pub interval_hours: u64,
    /// 自动回收是否删除未打标签 manifest
    pub delete_untagged: bool,
    /// 最近一次 GC 完成时间（RFC3339；未运行过为 null）
    pub last_run_at: Option<String>,
    /// 下次自动回收预计时间（RFC3339；未启用或未运行过为 null）
    pub next_run_at: Option<String>,
    /// GC 能力是否可用（registry 二进制 + 存储目录）
    pub available: bool,
    /// 不可用原因（可用时为 null；中文调试文案）
    pub unavailable_reason: Option<String>,
    /// 不可用原因稳定码（`registry_binary_missing` / `storage_dir_missing`；未知为 null）
    pub unavailable_code: Option<String>,
    /// registry 存储目录（容器内路径）
    pub storage_dir: Option<String>,
    /// registry 存储占用字节数（统计失败为 null）
    pub storage_bytes: Option<u64>,
    /// 当前运行中的 GC 任务 id（无则 null）
    pub running_task_id: Option<String>,
}

/// 更新自动回收配置请求（`interval_hours` / `delete_untagged` 缺省时保留现值）
#[derive(Deserialize)]
pub struct GcConfigRequest {
    pub enabled: bool,
    #[serde(default)]
    pub interval_hours: Option<u64>,
    #[serde(default)]
    pub delete_untagged: Option<bool>,
}

/// 手动触发 GC 请求（字段均缺省：`delete_untagged` 取配置值、非预演、超时 60 分钟）
#[derive(Deserialize, Default)]
pub struct GcRunRequest {
    #[serde(default)]
    pub delete_untagged: Option<bool>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub timeout_minutes: Option<u64>,
}

/// 组装 GC 状态（`GET` 与 `PUT` 共用）
async fn build_status(state: &AppState) -> Result<GcStatusData, ImageHubError> {
    let settings = state.settings.load()?;
    let gc = settings.gc;
    let available = state.gc.available();
    let storage_dir = state.gc.storage_dir().map(|p| p.display().to_string());
    let storage_bytes = match state.gc.storage_dir() {
        Some(dir) if available => {
            let dir = dir.to_path_buf();
            tokio::task::spawn_blocking(move || dir_size_bytes(&dir))
                .await
                .ok()
                .flatten()
        }
        _ => None,
    };
    let running_task_id = state
        .jobs
        .list()
        .into_iter()
        .find(|j| j.kind == "gc" && !j.status.is_terminal())
        .map(|j| j.id);
    Ok(GcStatusData {
        enabled: gc.enabled,
        interval_hours: gc.interval_hours,
        delete_untagged: gc.delete_untagged,
        next_run_at: compute_next_run_at(gc.enabled, gc.last_run_at.as_deref(), gc.interval_hours),
        last_run_at: gc.last_run_at,
        available,
        unavailable_reason: state.gc.unavailable_reason(),
        unavailable_code: state.gc.unavailable_code().map(str::to_string),
        storage_dir,
        storage_bytes,
        running_task_id,
    })
}

/// `GET /api/registry/gc` —— GC 状态与配置
pub async fn gc_status(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<GcStatusData>>, ImageHubError> {
    let data = build_status(&state).await?;
    Ok(Json(ResultBody::ok(data, state.version)))
}

/// `PUT /api/registry/gc` —— 更新自动回收配置
pub async fn update_gc_config(
    State(state): State<AppState>,
    Json(req): Json<GcConfigRequest>,
) -> Result<Json<ResultBody<GcStatusData>>, ImageHubError> {
    let mut settings = state.settings.load()?;
    let was_enabled = settings.gc.enabled;
    if let Some(interval) = req.interval_hours {
        settings.gc.interval_hours = validate_gc_interval(interval)?;
    }
    if let Some(delete_untagged) = req.delete_untagged {
        settings.gc.delete_untagged = delete_untagged;
    }
    settings.gc.enabled = req.enabled;
    // 首次启用（false → true）以当前时刻为自动回收计时基准（D-IH-62：不立即触发）
    if req.enabled && !was_enabled {
        settings.gc.last_run_at = Some(now_rfc3339());
    }
    state.settings.save(&settings)?;
    let data = build_status(&state).await?;
    Ok(Json(ResultBody::ok(data, state.version)))
}

/// `POST /api/registry/gc/run` —— 手动触发 GC（异步任务）
pub async fn run_gc(
    State(state): State<AppState>,
    Json(req): Json<GcRunRequest>,
) -> Result<Json<ResultBody<TaskStarted>>, ImageHubError> {
    let delete_untagged = match req.delete_untagged {
        Some(v) => v,
        None => state.settings.load()?.gc.delete_untagged,
    };
    let timeout = validate_transfer_timeout(req.timeout_minutes)?;
    let task_id = state.jobs.start_gc(GcRequest {
        delete_untagged,
        dry_run: req.dry_run,
        timeout,
    })?;
    Ok(Json(ResultBody::ok(TaskStarted { task_id }, state.version)))
}