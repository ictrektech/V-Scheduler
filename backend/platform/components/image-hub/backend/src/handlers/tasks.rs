//! 异步任务查询与停止 handlers（拉取 / 同步任务）

use axum::{
    Json,
    extract::{Path, State},
};
use serde::Serialize;

use crate::error::{ImageHubError, ResultBody};
use crate::jobs::Job;
use crate::state::AppState;

/// 任务列表响应
#[derive(Serialize)]
pub struct TaskListData {
    pub count: usize,
    pub tasks: Vec<Job>,
}

/// 清理结果
#[derive(Serialize)]
pub struct RemoveResult {
    pub removed: String,
}

/// 停止请求结果
#[derive(Serialize)]
pub struct CancelResult {
    pub id: String,
    /// 停止请求后的任务状态（通常仍为 `running`，随后由执行体落 `cancelled`）
    pub status: String,
    pub cancel_requested: bool,
}

/// `GET /api/tasks` —— 任务列表（创建时间倒序）
pub async fn list_tasks(State(state): State<AppState>) -> Json<ResultBody<TaskListData>> {
    let tasks = state.jobs.list();
    Json(ResultBody::ok(
        TaskListData {
            count: tasks.len(),
            tasks,
        },
        state.version,
    ))
}

/// `GET /api/tasks/{id}` —— 任务详情
pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<Job>>, ImageHubError> {
    let job = state.jobs.get(&id)?;
    Ok(Json(ResultBody::ok(job, state.version)))
}

/// `POST /api/tasks/{id}/cancel` —— 请求停止运行中的任务（幂等）
///
/// 边界：任务不存在 → 20808；已终态 → 20801；已请求过 → 幂等 `code=0`。
pub async fn cancel_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<CancelResult>>, ImageHubError> {
    let job = state.jobs.cancel(&id)?;
    Ok(Json(ResultBody::ok(
        CancelResult {
            id: job.id,
            status: job.status.as_str().to_string(),
            cancel_requested: job.cancel_requested,
        },
        state.version,
    )))
}

/// `DELETE /api/tasks/{id}` —— 清理终态任务记录
pub async fn remove_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<RemoveResult>>, ImageHubError> {
    state.jobs.remove(&id)?;
    Ok(Json(ResultBody::ok(
        RemoveResult { removed: id },
        state.version,
    )))
}