//! 节点管理 HTTP handlers

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use crate::deployments;
use crate::error::{ModelinferError, ResultBody};
use crate::monitor::store as monitor_store;
use crate::nodes::{self, NodeCreate, NodeFilter, NodeSort, NodeStats, NodeUpdate, NodeView};
use crate::paging::{DEFAULT_PAGE_SIZE, PageMeta, PageQuery, paginate};
use crate::state::AppState;

#[derive(Serialize)]
pub struct ListVo {
    /// 当前页节点
    data: Vec<NodeView>,
    /// 分页元数据（`total` 为**筛选后**条数）
    #[serde(flatten)]
    page: PageMeta,
    /// 未筛选统计（页头统计条）
    stats: NodeStats,
}

/// `GET /api/nodes` 查询参数（分页 + 筛选 + 排序）
#[derive(Debug, Default, Deserialize)]
pub struct NodesQuery {
    #[serde(default)]
    page_num: Option<u64>,
    #[serde(default)]
    page_size: Option<u64>,
    /// 关键词（名称 / 主机 / 主机名，大小写不敏感）
    #[serde(default)]
    keyword: Option<String>,
    /// 状态（`all` 表示不限）
    #[serde(default)]
    status: Option<String>,
    /// 排序：`name` / `recent`
    #[serde(default)]
    sort: Option<String>,
}

/// `GET /api/nodes`
///
/// 支持服务端分页与搜索 / 筛选 / 排序；`total` 为筛选后条数，`stats` 为未筛选统计。
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<NodesQuery>,
) -> Result<Json<ResultBody<ListVo>>, ModelinferError> {
    let mut all = nodes::list_nodes(&state.db).await?;
    let occupied = deployments::occupied_nodes(&state.db, "").await?;
    nodes::apply_occupancy(&mut all, &occupied);
    let monitors = monitor_store::summaries(&state.db).await.unwrap_or_default();
    nodes::apply_monitor(&mut all, &monitors);
    let stats = nodes::node_stats(&all);
    let filtered = nodes::filter_nodes(
        all,
        &NodeFilter {
            keyword: q.keyword.clone().unwrap_or_default(),
            status: q.status.clone().unwrap_or_else(|| "all".into()),
            sort: NodeSort::parse(q.sort.as_deref().unwrap_or("name")),
        },
    );
    let params = PageQuery { page_num: q.page_num, page_size: q.page_size }.resolve(DEFAULT_PAGE_SIZE);
    let (data, page) = paginate(filtered, params);
    Ok(Json(ResultBody::ok(ListVo { data, page, stats }, state.version)))
}

/// `POST /api/nodes`
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NodeCreate>,
) -> Result<Json<ResultBody<NodeView>>, ModelinferError> {
    let node = nodes::create_node(&state.db, state.ssh.as_ref(), req).await?;
    // best-effort 触发节点监控部署（失败不影响创建结果）
    crate::handlers::monitor::trigger_deploy(&state, &node.id).await;
    Ok(Json(ResultBody::ok(node, state.version)))
}

/// `GET /api/nodes/{id}`
pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<NodeView>>, ModelinferError> {
    let node = nodes::get_node(&state.db, &id).await?;
    let mut view = NodeView::from(node);
    let occupied = deployments::occupied_nodes(&state.db, "").await?;
    nodes::apply_occupancy(std::slice::from_mut(&mut view), &occupied);
    let monitors = monitor_store::summaries(&state.db).await.unwrap_or_default();
    nodes::apply_monitor(std::slice::from_mut(&mut view), &monitors);
    Ok(Json(ResultBody::ok(view, state.version)))
}

/// `PUT /api/nodes/{id}`
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<NodeUpdate>,
) -> Result<Json<ResultBody<NodeView>>, ModelinferError> {
    let node = nodes::update_node(&state.db, state.ssh.as_ref(), &id, req).await?;
    Ok(Json(ResultBody::ok(node, state.version)))
}

/// `DELETE /api/nodes/{id}`
///
/// 引用完整性：节点被**任一**运行配置引用时拒绝删除（`20712`），避免产生悬挂节点引用
/// （见 `openspec/changes/modelinfer-dangling-node-reference/`）。
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<serde_json::Value>>, ModelinferError> {
    let node = nodes::get_node(&state.db, &id).await?;
    // 引用检查必须在存在性校验之后：不存在的节点仍返回 20702（幂等语义）
    let refs = deployments::deployments_referencing_node(&state.db, &id).await?;
    if !refs.is_empty() {
        let names: Vec<&str> = refs.iter().take(3).map(|(_, name)| name.as_str()).collect();
        let suffix = if refs.len() > 3 {
            format!(" 等 {} 个", refs.len())
        } else {
            String::new()
        };
        return Err(ModelinferError::new(
            crate::error::codes::NODE_IN_USE,
            format!("节点被运行配置引用，禁止删除: {}{suffix}", names.join("、")),
        ));
    }
    // 删除前 best-effort 清理监控资源（consul 注销 + 停止容器）；失败由 reconcile 兜底
    crate::handlers::monitor::cleanup_on_delete(&state, &node).await;
    nodes::delete_node(&state.db, &id).await?;
    Ok(Json(ResultBody::ok(serde_json::json!({ "deleted": id }), state.version)))
}

/// `POST /api/nodes/{id}/test`
pub async fn test(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<NodeView>>, ModelinferError> {
    let node = nodes::test_node(&state.db, state.ssh.as_ref(), &id).await?;
    Ok(Json(ResultBody::ok(node, state.version)))
}

/// `POST /api/nodes/probe`：连接目标设备并扫描硬件（不落库）
pub async fn probe(
    State(state): State<Arc<AppState>>,
    Json(req): Json<nodes::NodeProbeReq>,
) -> Result<Json<ResultBody<nodes::scan::NodeScanResult>>, ModelinferError> {
    let scan = nodes::probe_node(state.ssh.as_ref(), &req).await?;
    Ok(Json(ResultBody::ok(scan, state.version)))
}

/// `POST /api/nodes/{id}/rescan`：重新扫描并更新硬件字段
pub async fn rescan(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<NodeView>>, ModelinferError> {
    let node = nodes::rescan_node(&state.db, state.ssh.as_ref(), &id).await?;
    Ok(Json(ResultBody::ok(node, state.version)))
}
