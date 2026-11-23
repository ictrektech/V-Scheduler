//! 部署配置 HTTP handlers

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use crate::deploy::autorestart;
use crate::deploy::control::{self, Control, TemporalControl};
use crate::deployments::{
    self, DeploymentCreate, DeploymentFilter, DeploymentSort, DeploymentStats, DeploymentUpdate,
    DeploymentView, OccupiedNode,
};
use crate::error::{ModelinferError, ResultBody, codes};
use crate::{nfs, nodes};
use crate::paging::{DEFAULT_PAGE_SIZE, PageMeta, PageQuery, paginate};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct DeploymentListVo {
    /// 当前页运行配置
    pub data: Vec<DeploymentView>,
    /// 分页元数据（`total` 为**筛选后**条数）
    #[serde(flatten)]
    pub page: PageMeta,
    /// 未筛选统计（页头统计条）
    pub stats: DeploymentStats,
    /// 活跃运行配置对节点的承载明细（节点 → 运行配置；同一节点可有多项）
    ///
    /// 仅用于展示 / 统计，不参与启动准入。
    pub occupied_nodes: Vec<OccupiedNode>,
}

/// `GET /api/deployments` 查询参数（分页 + 筛选 + 排序）
#[derive(Debug, Default, Deserialize)]
pub struct DeploymentsQuery {
    #[serde(default)]
    pub page_num: Option<u64>,
    #[serde(default)]
    pub page_size: Option<u64>,
    /// 关键词（名称 / 卡片名 / 权重，大小写不敏感）
    #[serde(default)]
    pub keyword: Option<String>,
    /// 状态（`all` 表示不限）
    #[serde(default)]
    pub state: Option<String>,
    /// 模型卡片 id（`all` 表示不限）
    #[serde(default)]
    pub card_id: Option<String>,
    /// 排序：`name` / `recent` / `state`
    #[serde(default)]
    pub sort: Option<String>,
}

/// `POST /api/deployments/{id}/stop` 查询参数
#[derive(Debug, Default, Deserialize)]
pub struct StopQuery {
    /// 强制停止：即便部署已处于 `stopped` / `stopping`（正常路径下幂等无操作），
    /// 也重新下发停止清理工作流，用于清除残留容器 / 卡死的异常状态。
    #[serde(default)]
    pub force: bool,
}

/// 是否应下发停止清理工作流（纯函数；实现见 [`control::stop_should_launch`]）
pub use control::stop_should_launch;

/// 构造 Temporal 生命周期控制入口（`None` = Temporal 不可用）
fn temporal_control(state: &AppState) -> Option<TemporalControl> {
    state
        .temporal
        .clone()
        .map(|client| TemporalControl::new(client, state.temporal_task_queue.clone()))
}

/// `GET /api/deployments`
///
/// 支持服务端分页与搜索 / 筛选 / 排序；`total` 为筛选后条数，`stats` 为未筛选统计。
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DeploymentsQuery>,
) -> Result<Json<ResultBody<DeploymentListVo>>, ModelinferError> {
    let all = deployments::list_deployments(&state.db).await?;
    let stats = deployments::deployment_stats(&all);
    let filtered = deployments::filter_deployments(
        all,
        &DeploymentFilter {
            keyword: q.keyword.clone().unwrap_or_default(),
            state: q.state.clone().unwrap_or_else(|| "all".into()),
            card_id: q.card_id.clone().unwrap_or_else(|| "all".into()),
            sort: DeploymentSort::parse(q.sort.as_deref().unwrap_or("name")),
        },
    );
    let params = PageQuery { page_num: q.page_num, page_size: q.page_size }.resolve(DEFAULT_PAGE_SIZE);
    let (data, page) = paginate(filtered, params);
    let occupied_nodes = deployments::occupied_nodes(&state.db, "").await?;
    Ok(Json(ResultBody::ok(DeploymentListVo { data, page, stats, occupied_nodes }, state.version)))
}

/// `POST /api/deployments`
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeploymentCreate>,
) -> Result<Json<ResultBody<DeploymentView>>, ModelinferError> {
    let d = deployments::create_deployment(&state.db, std::path::Path::new(&state.model_hub_dir), req).await?;
    Ok(Json(ResultBody::ok(d, state.version)))
}

/// `GET /api/deployments/{id}`
pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<DeploymentView>>, ModelinferError> {
    let d = deployments::get_view(&state.db, &id).await?;
    Ok(Json(ResultBody::ok(d, state.version)))
}

/// `PUT /api/deployments/{id}`
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<DeploymentUpdate>,
) -> Result<Json<ResultBody<DeploymentView>>, ModelinferError> {
    let d = deployments::update_deployment(&state.db, std::path::Path::new(&state.model_hub_dir), &id, req).await?;
    Ok(Json(ResultBody::ok(d, state.version)))
}

/// `DELETE /api/deployments/{id}`
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<serde_json::Value>>, ModelinferError> {
    // 删除前 best-effort 注销推理指标 consul 注册（失败不阻塞删除，由 reconcile 兜底）
    if let Err(e) =
        crate::deploy::activities::deregister_inference_metrics(&state.db, &state.monitor, &id).await
    {
        tracing::warn!(target: "modelinfer_inference", deployment = %id, error = %e.msg, "删除前推理指标注销失败（忽略）");
    }
    deployments::delete_deployment(&state.db, &id).await?;
    Ok(Json(ResultBody::ok(serde_json::json!({ "deleted": id }), state.version)))
}

/// `POST /api/deployments/{id}/start`
///
/// 节点可被多个运行配置共享：仅当**自身**已活跃时拒绝重复启动（`20719`），
/// 不再因所选节点被其他活跃运行配置使用而拒绝。实现见
/// [`crate::deploy::control::TemporalControl::start`]（与自动重启同源）。
///
/// 用户手动启动 MUST 同时取消自动重启循环（运行态清零）。
/// NFS / 权重 / 镜像预检失败时不更改任何运行态，也不下发工作流。
pub async fn start(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<DeploymentView>>, ModelinferError> {
    let d = deployments::get_deployment(&state.db, &id).await?;
    if deployments::is_active_state(&d.state) {
        return Err(ModelinferError::new(
            codes::DEPLOYMENT_CONFLICT,
            format!("部署 {} 已处于 {} 状态", d.name, d.state),
        ));
    }
    // 同步检查权重所在卷的共享，避免返回启动成功后才在异步 prepare 中失败。
    // 复用完整解析以保持公共目录 / 宿主路径 / NFS 地址推断与实际挂载一致。
    let node_ids: Vec<String> = serde_json::from_str(&d.node_ids_json).unwrap_or_default();
    let selected_nodes = nodes::load_models(&state.db, Some(&node_ids)).await?;
    let hosts: Vec<String> = selected_nodes.iter().map(|node| node.host.clone()).collect();
    nfs::resolve_nfs(
        state.platform.as_ref(),
        &nfs::ResolveInput {
            config_name: nfs::MODEL_HUB_CONFIG,
            fallback_hub_path: &state.model_hub_path,
            nfs_server_override: &state.nfs_server,
            node_hosts: &hosts,
        },
    )
    .await?;
    let card = crate::plaza::get_model(&state.db, &d.plaza_model_id).await?;
    let manifest = serde_json::from_str(&card.manifest_json).unwrap_or(serde_json::Value::Null);
    let hub = crate::images::RealImageHubClient::from_config(&state.image_hub);
    // 限定只读检查总时长，留在 HTTP 请求超时内，让用户收到明确的失败提示。
    tokio::time::timeout(
        std::time::Duration::from_secs(90),
        deployments::preflight::check(
            &d,
            &manifest,
            &selected_nodes,
            std::path::Path::new(&state.model_hub_dir),
            state.ssh.as_ref(),
            &hub,
            state.image_hub.concurrency,
        ),
    ).await.map_err(|_| ModelinferError::new(
        codes::START_PREFLIGHT_TIMEOUT,
        "启动前资源检查超时，请检查节点与 image-hub 连接后重试",
    ))??;
    let Some(ctl) = temporal_control(&state) else {
        return Err(ModelinferError::new(
            codes::TEMPORAL_FAILED,
            "Temporal 未启用或不可用，无法启动部署",
        ));
    };
    // 用户手动启动 = 取消自动重启循环（避免旧计时 / 旧 backoff 干扰人工操作）
    autorestart::reset(&state.db, &id).await?;
    ctl.start(&state.db, &id, control::TRIGGER_MANUAL).await?;
    Ok(Json(ResultBody::ok(deployments::get_view(&state.db, &id).await?, state.version)))
}


/// `POST /api/deployments/{id}/stop`
///
/// 启动与停止是相互独立的工作流：停止 MUST NOT 依赖启动工作流存活或在其中轮询控制信号。
/// 实现见 [`crate::deploy::control::TemporalControl::stop`]（与自动重启同源）。
///
/// 查询参数 `force=true` 时强制下发（跳过 `stopped` / `stopping` 的幂等守卫），
/// 用于清除残留容器 / 卡死的异常状态。
///
/// 用户手动停止 MUST 清除自动重启运行态（打断自动重启循环的唯一入口）。
pub async fn stop(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<StopQuery>,
) -> Result<Json<ResultBody<DeploymentView>>, ModelinferError> {
    let d = deployments::get_deployment(&state.db, &id).await?;
    // 已停止 / 已在停止清理中且未强制：幂等无操作（避免重复启动清理）
    if stop_should_launch(&d.state, q.force) {
        let Some(ctl) = temporal_control(&state) else {
            return Err(ModelinferError::new(
                codes::TEMPORAL_FAILED,
                "Temporal 未启用或不可用，无法停止部署",
            ));
        };
        // 用户手动停止 = 取消自动重启循环（打断自动重启的唯一入口）
        autorestart::reset(&state.db, &id).await?;
        ctl.stop(&id, q.force, "用户请求停止，终止在途启动工作流").await?;
    } else {
        // 幂等无操作时也清除循环：用户已明确要求停止
        autorestart::reset(&state.db, &id).await?;
    }
    Ok(Json(ResultBody::ok(deployments::get_view(&state.db, &id).await?, state.version)))
}


#[cfg(test)]
mod tests {
    use super::stop_should_launch;

    #[test]
    fn stop_normal_path_only_for_active_states() {
        // 正常停止：仅对未处于 stopped / stopping 的部署下发清理
        assert!(stop_should_launch("running", false));
        assert!(stop_should_launch("starting", false));
        assert!(stop_should_launch("degraded", false));
        assert!(stop_should_launch("failed", false));
        assert!(!stop_should_launch("stopped", false));
        assert!(!stop_should_launch("stopping", false));
    }

    #[test]
    fn force_stop_always_launches() {
        // 强制停止：任何状态都下发，用于清除异常状态
        for state in ["running", "starting", "degraded", "failed", "stopped", "stopping"] {
            assert!(stop_should_launch(state, true), "state={state} 强制停止应下发清理");
        }
    }
}
