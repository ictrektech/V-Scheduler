//! 运行配置推理指标 HTTP handler：代理 prometheus `query_range`，返回 vLLM / SGLang 关键指标组
//!
//! 设计见 `specs/docs/design/modelinfer/inference-metrics.md`（D-MI-SGH-05）：
//! - **始终返回历史数据**：只要引擎受支持且 head 节点可解析，就查询并返回该区间内已存在的数据，
//!   不因运行配置未运行 / prometheus 目标下线而清空（历史排障优先）；
//! - `active` 仅作提示位（运行态 ∧ `up{job=<引擎>,instance}==1`）；
//! - `last_sample` 为结果中最近采样时刻，前端用于「展示历史指标（最近采样 …）」。

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Serialize;

use crate::deployments;
use crate::error::{ModelinferError, ResultBody};
use crate::handlers::monitor::{MetricGroupVo, MetricsQuery, SeriesVo};
use crate::inference;
use crate::monitor::prometheus::{self, PrometheusClient};
use crate::state::AppState;

/// 运行配置推理指标响应
#[derive(Debug, Clone, Serialize)]
pub struct DeploymentMetricsVo {
    pub deployment_id: String,
    /// 引擎字面值（vllm / sglang / comfyui / unknown / 空）
    pub engine: String,
    pub engine_version: String,
    /// 该引擎是否支持指标采集
    pub supported: bool,
    /// 当前是否处于采集状态（运行态且 prometheus 目标在线）；**仅提示位**，为 false 时仍返回历史数据
    pub active: bool,
    /// 本次结果中最近采样时间（unix 秒）；无数据时为 null
    pub last_sample: Option<i64>,
    /// prometheus 实例标签（`<head IP>:<serve_port>`；不受支持 / head 节点缺失时为空）
    pub instance: Option<String>,
    pub range: String,
    pub start: i64,
    pub end: i64,
    pub step: u64,
    pub groups: Vec<MetricGroupVo>,
}

/// `GET /api/deployments/{id}/metrics`：代理 prometheus `query_range`，返回引擎指标目录的时间序列。
///
/// - 引擎不受支持 / 未识别 → 返回空指标组（**不报错**），由前端提示；
/// - prometheus 不可达 → `20736`；
/// - **历史语义**：受支持引擎且 head 节点可解析时，MUST 无条件查询并返回区间内已存在的数据
///   （停止后仍可回看），`active` 仅表示当前是否在采。
pub async fn metrics(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<MetricsQuery>,
) -> Result<Json<ResultBody<DeploymentMetricsVo>>, ModelinferError> {
    let d = deployments::get_deployment(&state.db, &id).await?;
    let engine = d.inference_engine.clone();
    let engine_version = d.inference_engine_version.clone();
    let supported = inference::engine_metrics_supported(&engine);

    let now = chrono::Utc::now().timestamp();
    let (range, start, end, step) = prometheus::resolve_range(
        q.range.as_deref().unwrap_or("1h"),
        q.start,
        q.end,
        q.step,
        now,
    )?;

    let mut groups: Vec<MetricGroupVo> = Vec::new();
    let mut instance: Option<String> = None;
    let mut active = false;
    // head 节点可能已被删除（历史悬挂引用）：降级为空指标组，不返回 20702
    // （见 openspec/changes/modelinfer-dangling-node-reference/）
    let head_node = if supported && !d.head_node_id.trim().is_empty() {
        crate::nodes::find_node(&state.db, &d.head_node_id).await?
    } else {
        None
    };
    if let Some(node) = head_node {
        let port = u16::try_from(d.serve_port.max(1)).unwrap_or(8000);
        let inst = format!("{}:{}", node.host, port);
        // prometheus 客户端：单测注入 Mock，生产按监控配置构造（见 AppState.prometheus）
        let client: Arc<dyn PrometheusClient> = match state.prometheus.clone() {
            Some(c) => c,
            None => Arc::new(prometheus::RealPrometheusClient::new(state.monitor.prometheus())),
        };
        // 活跃判定仅用于提示（覆盖进程已退出 / consul 已注销），不影响历史数据返回
        active = deployments::is_active_state(&d.state)
            && target_up(client.as_ref(), &engine, &inst, end).await?;
        // rate 窗口随查询步长自适应，避免短窗口稀释瞬时吞吐（见 design D-MI-IM-F1 / D-MI-SGH-04）
        let rate_window = prometheus::rate_window_secs(step);
        let queries = inference::metrics::catalog_for(&engine, &inst, rate_window);
        // 并发查询 prometheus（避免逐条串行）
        let results = futures::future::join_all(
            queries
                .iter()
                .map(|m| PrometheusClient::query_range(client.as_ref(), &m.query, start, end, step)),
        )
        .await;
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
                    .map(|s| SeriesVo { labels: s.labels, points: s.points })
                    .collect(),
            });
        }
        instance = Some(inst);
    }
    let last_sample = last_sample_of(&groups);

    Ok(Json(ResultBody::ok(
        DeploymentMetricsVo {
            deployment_id: id,
            engine,
            engine_version,
            supported,
            active,
            last_sample,
            instance,
            range,
            start,
            end,
            step,
            groups,
        },
        state.version,
    )))
}

/// 结果中最近采样时间（unix 秒，纯函数）：所有指标组 / 序列采样点的最大值；无数据 → `None`。
fn last_sample_of(groups: &[MetricGroupVo]) -> Option<i64> {
    let ts = groups
        .iter()
        .flat_map(|g| g.series.iter())
        .flat_map(|s| s.points.iter())
        .map(|(t, _)| *t)
        .filter(|t| t.is_finite())
        .fold(f64::NEG_INFINITY, f64::max);
    if ts.is_finite() && ts > f64::NEG_INFINITY {
        Some(ts as i64)
    } else {
        None
    }
}

/// 判断 prometheus 推理目标**当前**是否在线：`max(up{job="<引擎 job>",instance})` 在 `now`
/// 单点求值是否为 1（仅用于 `active` 提示位）。
///
/// 关键：必须在**当前时刻**求值，而非取最近一段区间的末值——目标被 consul SD 移除 / 进程退出后
/// prometheus 写入 stale marker，`up` 在 `now` 立即为空，而取区间末值仍会命中移除前的 `up=1`。
/// 查询失败（prometheus 不可达）向上抛 `20736`；`up` 为空（目标已移除 / 尚未采集）视为不活跃。
async fn target_up(
    client: &dyn PrometheusClient,
    engine: &str,
    instance: &str,
    now: i64,
) -> Result<bool, ModelinferError> {
    let job = inference::service_name(inference::InferenceEngine::parse(engine));
    if job.is_empty() {
        return Ok(false);
    }
    let query = format!(
        "max(up{{job=\"{}\",instance=\"{}\"}})",
        prometheus::esc(job),
        prometheus::esc(instance),
    );
    // start == end == now：单点（instant）求值，避免把已失效目标的历史 `up=1` 当作在线
    let series = PrometheusClient::query_range(client, &query, now, now, 15).await?;
    // 取末个有限值；缺失 / 非 1 → 不活跃
    for s in series {
        for (_, v) in s.points.iter().rev() {
            if v.is_finite() {
                return Ok(*v == 1.0);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::prometheus::{MockPrometheusClient, PromSeries};
    use std::collections::BTreeMap;

    fn series(points: Vec<(f64, f64)>) -> PromSeries {
        PromSeries { labels: BTreeMap::new(), points }
    }

    #[tokio::test]
    async fn target_up_matrix() {
        // up=1 → 活跃
        let c = MockPrometheusClient::new().with_series("up{", vec![series(vec![(1.0, 1.0), (2.0, 1.0)])]);
        assert!(target_up(&c, "vllm", "192.168.8.100:8000", 10).await.unwrap());
        // up=0 → 不活跃
        let c = MockPrometheusClient::new().with_series("up{", vec![series(vec![(1.0, 0.0)])]);
        assert!(!target_up(&c, "vllm", "192.168.8.100:8000", 10).await.unwrap());
        // 目标已移除 / 空结果 → 不活跃
        let c = MockPrometheusClient::new();
        assert!(!target_up(&c, "vllm", "192.168.8.100:8000", 10).await.unwrap());
        // 末点非有限值时回溯取前一个有限值
        let c = MockPrometheusClient::new().with_series("up{", vec![series(vec![(1.0, 1.0), (2.0, f64::NAN)])]);
        assert!(target_up(&c, "vllm", "192.168.8.100:8000", 10).await.unwrap());
        // prometheus 不可达 → 上抛 20736
        let c = MockPrometheusClient::failing();
        assert_eq!(
            target_up(&c, "vllm", "192.168.8.100:8000", 10).await.unwrap_err().code,
            crate::error::codes::PROMETHEUS_UNAVAILABLE
        );
        // 非受支持引擎不查询（不产生 prometheus 调用）
        let c = MockPrometheusClient::new();
        assert!(!target_up(&c, "comfyui", "192.168.8.100:8188", 10).await.unwrap());
        assert!(c.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn target_up_uses_engine_job() {
        let c = MockPrometheusClient::new().with_series("up{", vec![series(vec![(1.0, 1.0)])]);
        assert!(target_up(&c, "sglang", "10.0.0.1:30000", 10).await.unwrap());
        let calls = c.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains(r#"job="modelinfer-sglang""#));
        assert!(calls[0].contains(r#"instance="10.0.0.1:30000""#));
    }

    #[test]
    fn last_sample_matrix() {
        fn group(points: Vec<(f64, f64)>) -> MetricGroupVo {
            MetricGroupVo {
                key: "k".into(),
                name: "n".into(),
                unit: "".into(),
                source: "s".into(),
                category: "inference".into(),
                series: vec![SeriesVo { labels: BTreeMap::new(), points }],
            }
        }
        assert_eq!(last_sample_of(&[]), None);
        assert_eq!(last_sample_of(&[group(vec![])]), None);
        assert_eq!(last_sample_of(&[group(vec![(100.0, 1.0), (200.0, 2.0)])]), Some(200));
        // 多组取最大；非有限值忽略
        assert_eq!(
            last_sample_of(&[
                group(vec![(100.0, 1.0), (f64::NAN, 9.0)]),
                group(vec![(150.0, 1.0), (300.0, 0.5)]),
            ]),
            Some(300)
        );
    }
}