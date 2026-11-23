//! 节点监控 HTTP handlers：按组件的异步部署触发 / 状态查询 / prometheus 指标查询

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use crate::deploy::runtime::start_workflow;
use crate::error::{ModelinferError, ResultBody, codes};
use crate::monitor::{self, ComponentKind, prometheus, store};
use crate::nodes;
use crate::state::AppState;

/// 节点监控状态响应（按组件）
#[derive(Debug, Clone, Serialize)]
pub struct NodeMonitorsVo {
    pub node_id: String,
    pub components: Vec<store::MonitorView>,
}

/// `POST /api/nodes/{id}/monitor/deploy`：按适用组件逐一下发监控部署工作流（失败可重试）
pub async fn deploy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<NodeMonitorsVo>>, ModelinferError> {
    let node = nodes::get_node(&state.db, &id).await?;
    let kinds = monitor::applicable_components(&node.gpu_vendor);
    let Some(client) = state.temporal.as_ref() else {
        let msg = "Temporal 未启用或不可用，无法部署节点监控组件";
        for kind in &kinds {
            let comp = monitor::component_for(&node, *kind, &state.monitor);
            let _ = store::mark_failed(&state.db, &id, kind.as_str(), &comp.image, &comp.category, msg).await;
        }
        return Err(ModelinferError::new(codes::TEMPORAL_FAILED, msg));
    };
    let mut last_err: Option<ModelinferError> = None;
    for kind in &kinds {
        let comp = monitor::component_for(&node, *kind, &state.monitor);
        let wid = format!("modelinfer-monitor-{}-{}-{}", id, kind.as_str(), ulid::Ulid::new());
        let input = serde_json::json!({ "node_id": node.id, "component": kind.as_str() });
        if let Err(e) = start_workflow(
            client,
            "NodeMonitorDeployWorkflow",
            &wid,
            &state.temporal_task_queue,
            &input,
        )
        .await
        {
            tracing::warn!(target: "modelinfer_monitor", component = kind.as_str(), error = %e.msg, "监控部署工作流下发失败");
            let _ = store::mark_failed(&state.db, &id, kind.as_str(), &comp.image, &comp.category, &e.msg).await;
            last_err = Some(e);
        }
    }
    // 仅当所有组件均下发失败时上抛错误；部分失败在状态中体现，前端可见并重试
    let components = store::views_of(&state.db, &id).await?;
    if let Some(e) = last_err
        && components.iter().all(|v| v.state == store::state::FAILED)
    {
        return Err(e);
    }
    let view = NodeMonitorsVo { node_id: id.clone(), components };
    Ok(Json(ResultBody::ok(view, state.version)))
}

/// `GET /api/nodes/{id}/monitor`：各组件监控状态与最近一次部署步骤
pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<NodeMonitorsVo>>, ModelinferError> {
    let node = nodes::get_node(&state.db, &id).await?;
    let mut components = store::views_of(&state.db, &id).await?;
    // 补齐尚未有任何记录的适用组件（空态 pending），保证前端按组件稳定渲染
    for kind in monitor::applicable_components(&node.gpu_vendor) {
        if !components.iter().any(|c| c.component == kind.as_str()) {
            let mut v = store::view_of(&state.db, &id, kind.as_str()).await?;
            let comp = monitor::component_for(&node, kind, &state.monitor);
            v.image = comp.image.clone();
            v.service_id = monitor::service_spec(&node, &comp).id;
            v.service_name = monitor::service_spec(&node, &comp).name;
            components.push(v);
        }
    }
    let view = NodeMonitorsVo { node_id: id, components };
    Ok(Json(ResultBody::ok(view, state.version)))
}

/// `GET /api/nodes/{id}/metrics` 查询参数
#[derive(Debug, Default, Deserialize)]
pub struct MetricsQuery {
    /// 预设区间：15m / 1h / 6h / 24h / 7d（缺省 1h）
    #[serde(default)]
    pub range: Option<String>,
    /// 自定义区间起点（unix 秒）
    #[serde(default)]
    pub start: Option<i64>,
    /// 自定义区间终点（unix 秒）
    #[serde(default)]
    pub end: Option<i64>,
    /// 步长（秒；缺省按区间推导）
    #[serde(default)]
    pub step: Option<u64>,
}

/// 单条时间序列（按标签区分，如磁盘挂载点 / GPU）
#[derive(Debug, Clone, Serialize)]
pub struct SeriesVo {
    pub labels: std::collections::BTreeMap<String, String>,
    pub points: Vec<(f64, f64)>,
}

/// 一个指标组（可含多条序列）
#[derive(Debug, Clone, Serialize)]
pub struct MetricGroupVo {
    pub key: String,
    pub name: String,
    pub unit: String,
    /// prometheus job（= consul 服务名）
    pub source: String,
    /// 指标分类（`system` / `gpu`）
    pub category: String,
    pub series: Vec<SeriesVo>,
}

/// 指标查询响应
#[derive(Debug, Clone, Serialize)]
pub struct MetricsVo {
    pub node_id: String,
    /// 系统指标 prometheus 实例标签（`<IP>:<node_exporter_port>`）
    pub instance: String,
    /// GPU 指标 prometheus 实例标签（`<IP>:<nvidia_gpu_exporter_port>`；仅 NVIDIA 节点）
    pub gpu_instance: Option<String>,
    pub range: String,
    pub start: i64,
    pub end: i64,
    pub step: u64,
    pub groups: Vec<MetricGroupVo>,
}

/// `GET /api/nodes/{id}/metrics`：代理 prometheus `query_range`，返回预置指标目录的时间序列。
///
/// 指标组按 `source` / `category` 分类（系统 + 适用厂商 GPU）；无数据（未部署 / 尚未采集 /
/// 该设备不支持该指标）返回空序列，MUST NOT 报错。
pub async fn metrics(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<MetricsQuery>,
) -> Result<Json<ResultBody<MetricsVo>>, ModelinferError> {
    let node = nodes::get_node(&state.db, &id).await?;
    let instance = format!("{}:{}", node.host, node.node_exporter_port);
    let kinds = monitor::applicable_components(&node.gpu_vendor);
    let gpu = kinds.contains(&ComponentKind::NvidiaGpuExporter);
    let gpu_instance = if gpu {
        Some(format!("{}:{}", node.host, node.nvidia_gpu_exporter_port))
    } else {
        None
    };

    let now = chrono::Utc::now().timestamp();
    let (range, start, end, step) = prometheus::resolve_range(
        q.range.as_deref().unwrap_or("1h"),
        q.start,
        q.end,
        q.step,
        now,
    )?;

    let mut queries = prometheus::system_metric_catalog(&instance);
    if let Some(gi) = &gpu_instance {
        queries.extend(prometheus::nvidia_gpu_metric_catalog(gi));
    }

    // 并发查询 prometheus（避免逐条串行）
    let client = prometheus::RealPrometheusClient::new(state.monitor.prometheus());
    let results = futures::future::join_all(queries.iter().map(|m| {
        prometheus::PrometheusClient::query_range(&client, &m.query, start, end, step)
    }))
    .await;

    let mut groups = Vec::with_capacity(queries.len());
    for (m, res) in queries.iter().zip(results) {
        let series = res?;
        groups.push(MetricGroupVo {
            key: m.key.clone(),
            name: m.name.clone(),
            unit: m.unit.clone(),
            source: m.source.clone(),
            category: m.category.clone(),
            series: series
                .into_iter()
                .map(|s| SeriesVo {
                    labels: s.labels,
                    points: s.points,
                })
                .collect(),
        });
    }

    Ok(Json(ResultBody::ok(
        MetricsVo {
            node_id: id,
            instance,
            gpu_instance,
            range,
            start,
            end,
            step,
            groups,
        },
        state.version,
    )))
}

/// 由节点创建流程调用：best-effort 触发全部适用组件的监控部署
pub async fn trigger_deploy(state: &AppState, node_id: &str) {
    let node = match nodes::get_node(&state.db, node_id).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(target: "modelinfer_monitor", node = %node_id, error = %e.msg, "加载节点失败，跳过监控部署");
            return;
        }
    };
    let kinds = monitor::applicable_components(&node.gpu_vendor);
    for kind in &kinds {
        let comp = monitor::component_for(&node, *kind, &state.monitor);
        match state.temporal.as_ref() {
            Some(client) => {
                let wid = format!("modelinfer-monitor-{}-{}-{}", node_id, kind.as_str(), ulid::Ulid::new());
                let input = serde_json::json!({ "node_id": node_id, "component": kind.as_str() });
                if let Err(e) = start_workflow(
                    client,
                    "NodeMonitorDeployWorkflow",
                    &wid,
                    &state.temporal_task_queue,
                    &input,
                )
                .await
                {
                    tracing::warn!(
                        target: "modelinfer_monitor",
                        node = %node_id,
                        component = kind.as_str(),
                        error = %e.msg,
                        "自动触发监控部署失败"
                    );
                    let _ = store::mark_failed(&state.db, node_id, kind.as_str(), &comp.image, &comp.category, &e.msg).await;
                }
            }
            None => {
                let _ = store::mark_failed(
                    &state.db,
                    node_id,
                    kind.as_str(),
                    &comp.image,
                    &comp.category,
                    "Temporal 未启用，无法部署监控组件",
                )
                .await;
            }
        }
    }
}

/// 由节点删除流程调用：best-effort 注销各组件的 consul 服务并停止容器（不阻塞删除）
pub async fn cleanup_on_delete(state: &AppState, node: &crate::entities::node::Model) {
    let consul = monitor::consul::RealConsulClient::new(state.monitor.consul());
    let target = nodes::ssh_target_of(node);
    for kind in monitor::applicable_components(&node.gpu_vendor) {
        let comp = monitor::component_for(node, kind, &state.monitor);
        let svc = monitor::service_spec(node, &comp);
        if let Err(e) = monitor::consul::ConsulClient::deregister(&consul, &svc.id).await {
            tracing::warn!(target: "modelinfer_monitor", component = kind.as_str(), error = %e.msg, "consul 注销失败（reconcile 兜底）");
        }
        if let Err(e) = monitor::teardown_component(state.ssh.as_ref(), &target, &comp.container).await {
            tracing::warn!(target: "modelinfer_monitor", component = kind.as_str(), error = %e.msg, "停止监控组件容器失败（忽略）");
        }
    }
    let _ = store::remove(&state.db, &node.id).await;
}