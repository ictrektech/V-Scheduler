//! 运行可观测性 HTTP handlers（只读）

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use crate::error::{ModelinferError, ResultBody};
use crate::observability::store::{
    self, ContainerView, GlobalDataView, NodeDataView, RunLogView, RunStepView, RunView,
};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct RunQuery {
    pub kind: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    pub stage: Option<String>,
    pub node_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DataQuery {
    pub stage: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RunListVo {
    pub data: Vec<RunView>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct RunDetailVo {
    pub run: RunView,
    /// 运行步骤（编排活动 / 阶段）
    pub steps: Vec<RunStepView>,
    pub node_data: Vec<NodeDataView>,
    pub global_data: Vec<GlobalDataView>,
}

#[derive(Debug, Serialize)]
pub struct StepListVo {
    pub data: Vec<RunStepView>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct LogListVo {
    pub data: Vec<RunLogView>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct DataVo {
    pub node_data: Vec<NodeDataView>,
    pub global_data: Vec<GlobalDataView>,
}

#[derive(Debug, Serialize)]
pub struct ContainerListVo {
    pub data: Vec<ContainerView>,
    pub total: usize,
}

/// `GET /api/deployments/{id}/runs`
pub async fn list_runs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<RunQuery>,
) -> Result<Json<ResultBody<RunListVo>>, ModelinferError> {
    // 校验运行配置存在
    crate::deployments::get_deployment(&state.db, &id).await?;
    let data = store::list_runs(&state.db, &id, q.kind.as_deref(), q.limit).await?;
    let total = data.len();
    Ok(Json(ResultBody::ok(RunListVo { data, total }, state.version)))
}

/// `GET /api/deployments/{id}/runs/{run_id}`
pub async fn get_run(
    State(state): State<Arc<AppState>>,
    Path((id, run_id)): Path<(String, String)>,
) -> Result<Json<ResultBody<RunDetailVo>>, ModelinferError> {
    let run = store::get_run(&state.db, &id, &run_id).await?;
    let steps = store::list_steps(&state.db, &id, &run_id).await?;
    let node_data = store::list_node_data(&state.db, &id, &run_id, None).await?;
    let global_data = store::list_global_data(&state.db, &id, &run_id).await?;
    Ok(Json(ResultBody::ok(
        RunDetailVo { run, steps, node_data, global_data },
        state.version,
    )))
}

/// `GET /api/deployments/{id}/runs/{run_id}/steps`
pub async fn list_steps(
    State(state): State<Arc<AppState>>,
    Path((id, run_id)): Path<(String, String)>,
) -> Result<Json<ResultBody<StepListVo>>, ModelinferError> {
    store::get_run(&state.db, &id, &run_id).await?;
    let data = store::list_steps(&state.db, &id, &run_id).await?;
    let total = data.len();
    Ok(Json(ResultBody::ok(StepListVo { data, total }, state.version)))
}

/// `GET /api/deployments/{id}/runs/{run_id}/temporal`
///
/// 按需查询 Temporal 工作流执行描述（**不落库**）；无工作流 / Temporal 不可用返回 20721。
pub async fn temporal(
    State(state): State<Arc<AppState>>,
    Path((id, run_id)): Path<(String, String)>,
) -> Result<Json<ResultBody<crate::deploy::temporal_query::TemporalWorkflowView>>, ModelinferError>
{
    let workflow_id = store::run_workflow_id(&state.db, &id, &run_id)
        .await?
        .ok_or_else(|| {
            ModelinferError::new(
                crate::error::codes::TEMPORAL_FAILED,
                "该运行不由 Temporal 编排（无工作流）",
            )
        })?;
    let Some(client) = state.temporal.as_ref() else {
        return Err(ModelinferError::new(
            crate::error::codes::TEMPORAL_FAILED,
            "Temporal 未启用或不可用，无法查询活动",
        ));
    };
    let view = crate::deploy::temporal_query::describe_workflow(client, &workflow_id).await?;
    Ok(Json(ResultBody::ok(view, state.version)))
}

/// `GET /api/deployments/{id}/runs/{run_id}/logs`
pub async fn list_logs(
    State(state): State<Arc<AppState>>,
    Path((id, run_id)): Path<(String, String)>,
    Query(q): Query<LogQuery>,
) -> Result<Json<ResultBody<LogListVo>>, ModelinferError> {
    store::get_run(&state.db, &id, &run_id).await?;
    let data = store::list_run_logs(
        &state.db,
        &id,
        &run_id,
        q.stage.as_deref(),
        q.node_id.as_deref(),
    )
    .await?;
    let total = data.len();
    Ok(Json(ResultBody::ok(LogListVo { data, total }, state.version)))
}

/// `GET /api/deployments/{id}/runs/{run_id}/data`
pub async fn list_data(
    State(state): State<Arc<AppState>>,
    Path((id, run_id)): Path<(String, String)>,
    Query(q): Query<DataQuery>,
) -> Result<Json<ResultBody<DataVo>>, ModelinferError> {
    store::get_run(&state.db, &id, &run_id).await?;
    let node_data = store::list_node_data(&state.db, &id, &run_id, q.stage.as_deref()).await?;
    let global_data = store::list_global_data(&state.db, &id, &run_id).await?;
    Ok(Json(ResultBody::ok(DataVo { node_data, global_data }, state.version)))
}

/// `GET /api/deployments/{id}/containers`
pub async fn list_containers(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<ContainerListVo>>, ModelinferError> {
    crate::deployments::get_deployment(&state.db, &id).await?;
    let data = store::list_containers(&state.db, &id).await?;
    let total = data.len();
    Ok(Json(ResultBody::ok(ContainerListVo { data, total }, state.version)))
}