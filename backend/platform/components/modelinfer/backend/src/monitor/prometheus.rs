//! prometheus 客户端抽象与节点指标目录
//!
//! 后端代理 prometheus `query_range`，前端（echarts）按预置指标目录渲染。
//! prometheus 不可达 → `20736`。

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ModelinferError, codes};

/// prometheus 连接配置
#[derive(Debug, Clone)]
pub struct PrometheusConfig {
    pub url: String,
}

impl PrometheusConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into().trim_end_matches('/').to_string(),
        }
    }
}

impl Default for PrometheusConfig {
    fn default() -> Self {
        Self::new("http://172.17.0.1:9090")
    }
}

/// 单条时间序列
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromSeries {
    /// 标签（`instance` / `device` / `mountpoint` 等）
    pub labels: BTreeMap<String, String>,
    /// 采样点 `(unix 秒, 值)`
    pub points: Vec<(f64, f64)>,
}

/// prometheus 客户端
#[async_trait]
pub trait PrometheusClient: Send + Sync {
    async fn query_range(
        &self,
        query: &str,
        start: i64,
        end: i64,
        step: u64,
    ) -> Result<Vec<PromSeries>, ModelinferError>;
}

/// 生产实现（reqwest `GET /api/v1/query_range`）
pub struct RealPrometheusClient {
    cfg: PrometheusConfig,
    client: reqwest::Client,
}

impl RealPrometheusClient {
    pub fn new(cfg: PrometheusConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self { cfg, client }
    }
}

/// 解析 `query_range` 响应（纯函数）
pub fn parse_query_range(v: &Value) -> Vec<PromSeries> {
    let mut out = Vec::new();
    let Some(result) = v.get("data").and_then(|d| d.get("result")).and_then(Value::as_array) else {
        return out;
    };
    for series in result {
        let mut labels = BTreeMap::new();
        if let Some(m) = series.get("metric").and_then(Value::as_object) {
            for (k, val) in m {
                if let Some(s) = val.as_str() {
                    labels.insert(k.clone(), s.to_string());
                }
            }
        }
        let mut points = Vec::new();
        if let Some(values) = series.get("values").and_then(Value::as_array) {
            for pair in values {
                let Some(arr) = pair.as_array() else { continue };
                if arr.len() < 2 {
                    continue;
                }
                let ts = arr[0].as_f64().unwrap_or_default();
                let val = match &arr[1] {
                    Value::String(s) => s.parse::<f64>().unwrap_or(f64::NAN),
                    other => other.as_f64().unwrap_or(f64::NAN),
                };
                if ts.is_finite() {
                    points.push((ts, val));
                }
            }
        }
        out.push(PromSeries { labels, points });
    }
    out
}

#[async_trait]
impl PrometheusClient for RealPrometheusClient {
    async fn query_range(
        &self,
        query: &str,
        start: i64,
        end: i64,
        step: u64,
    ) -> Result<Vec<PromSeries>, ModelinferError> {
        let url = format!("{}/api/v1/query_range", self.cfg.url);
        let resp = self
            .client
            .get(&url)
            .query(&[
                ("query", query.to_string()),
                ("start", start.to_string()),
                ("end", end.to_string()),
                ("step", step.to_string()),
            ])
            .send()
            .await
            .map_err(|e| {
                ModelinferError::new(codes::PROMETHEUS_UNAVAILABLE, format!("prometheus 不可达 {url}: {e}"))
            })?;
        let body: Value = resp.json().await.map_err(|e| {
            ModelinferError::new(codes::PROMETHEUS_UNAVAILABLE, format!("解析 prometheus 响应失败: {e}"))
        })?;
        if body.get("status").and_then(Value::as_str) != Some("success") {
            return Err(ModelinferError::new(
                codes::PROMETHEUS_UNAVAILABLE,
                format!("prometheus 查询失败: {}", body.get("error").and_then(Value::as_str).unwrap_or("")),
            ));
        }
        Ok(parse_query_range(&body))
    }
}

/// 测试替身
#[derive(Default)]
pub struct MockPrometheusClient {
    /// 按 query 子串预设序列
    pub series: Vec<(String, Vec<PromSeries>)>,
    pub fail: bool,
    pub calls: Mutex<Vec<String>>,
}

impl MockPrometheusClient {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_series(self, substr: &str, s: Vec<PromSeries>) -> Self {
        let mut me = self;
        me.series.push((substr.to_string(), s));
        me
    }
    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }
    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl PrometheusClient for MockPrometheusClient {
    async fn query_range(
        &self,
        query: &str,
        _start: i64,
        _end: i64,
        _step: u64,
    ) -> Result<Vec<PromSeries>, ModelinferError> {
        self.calls.lock().unwrap().push(query.to_string());
        if self.fail {
            return Err(ModelinferError::new(codes::PROMETHEUS_UNAVAILABLE, "mock prometheus 不可达"));
        }
        for (sub, s) in &self.series {
            if query.contains(sub.as_str()) {
                return Ok(s.clone());
            }
        }
        Ok(Vec::new())
    }
}

// ============ 节点指标目录与时间区间（纯函数） ============

/// 单条预置指标查询
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricQuery {
    pub key: String,
    pub name: String,
    /// 展示单位（`%` / `B/s` / `B` / `s` / `W` / `Hz` / `°C` / 空）
    pub unit: String,
    /// prometheus job（= consul 服务名；用于分组与展示）
    pub source: String,
    /// 指标分类（`system` / `gpu`）
    pub category: String,
    pub query: String,
}

/// 转义 PromQL 字符串字面量中的反斜杠与双引号
pub(crate) fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 预置系统指标目录（node-exporter 基础指标；`instance` = `<节点IP>:<node_exporter_port>`）
pub fn system_metric_catalog(instance: &str) -> Vec<MetricQuery> {
    let i = esc(instance);
    let fs_filter = r#"fstype!~"tmpfs|overlay|squashfs|ramfs|devtmpfs""#;
    let net_filter = r#"device!~"lo|docker.*|veth.*|br-.*""#;
    let src = crate::monitor::ComponentKind::NodeExporter.service_name();
    let q = |key: &str, name: &str, unit: &str, query: String| MetricQuery {
        key: key.to_string(),
        name: name.to_string(),
        unit: unit.to_string(),
        source: src.to_string(),
        category: "system".to_string(),
        query,
    };
    vec![
        // CPU：按 instance 平均（跨核 + 历史重复目标折叠）
        q(
            "cpu_usage",
            "CPU 使用率",
            "%",
            format!(r#"100 - (avg by (instance) (rate(node_cpu_seconds_total{{mode="idle",instance="{i}"}}[5m])) * 100)"#),
        ),
        // 内存：按 instance 取最大（折叠历史重复目标）
        q(
            "mem_usage",
            "内存使用率",
            "%",
            format!(r#"max by (instance) ((1 - node_memory_MemAvailable_bytes{{instance="{i}"}} / node_memory_MemTotal_bytes{{instance="{i}"}}) * 100)"#),
        ),
        q("load1", "系统负载（1m）", "", format!(r#"max by (instance) (node_load1{{instance="{i}"}})"#)),
        q("load5", "系统负载（5m）", "", format!(r#"max by (instance) (node_load5{{instance="{i}"}})"#)),
        q("load15", "系统负载（15m）", "", format!(r#"max by (instance) (node_load15{{instance="{i}"}})"#)),
        // 磁盘使用率：按挂载点取最大
        q(
            "disk_usage",
            "磁盘使用率",
            "%",
            format!(r#"max by (mountpoint) ((1 - node_filesystem_avail_bytes{{instance="{i}",{fs_filter}}} / node_filesystem_size_bytes{{instance="{i}",{fs_filter}}}) * 100)"#),
        ),
        // 网络：先按 (instance, device) 折叠重复目标，再按 instance 求和
        q(
            "net_rx",
            "网络接收速率",
            "B/s",
            format!(r#"sum by (instance) (max by (instance, device) (rate(node_network_receive_bytes_total{{instance="{i}",{net_filter}}}[5m])))"#),
        ),
        q(
            "net_tx",
            "网络发送速率",
            "B/s",
            format!(r#"sum by (instance) (max by (instance, device) (rate(node_network_transmit_bytes_total{{instance="{i}",{net_filter}}}[5m])))"#),
        ),
        // 磁盘 IO：先按 (instance, device) 折叠重复目标，再按 instance 求和
        q(
            "disk_read",
            "磁盘读取速率",
            "B/s",
            format!(r#"sum by (instance) (max by (instance, device) (rate(node_disk_read_bytes_total{{instance="{i}"}}[5m])))"#),
        ),
        q(
            "disk_write",
            "磁盘写入速率",
            "B/s",
            format!(r#"sum by (instance) (max by (instance, device) (rate(node_disk_written_bytes_total{{instance="{i}"}}[5m])))"#),
        ),
        // 文件系统可用空间：按挂载点取最大
        q(
            "fs_avail",
            "文件系统可用空间",
            "B",
            format!(r#"max by (mountpoint) (node_filesystem_avail_bytes{{instance="{i}",{fs_filter}}})"#),
        ),
        // 运行时长：按 instance 取最大
        q(
            "uptime",
            "运行时长",
            "s",
            format!(r#"max by (instance) (node_time_seconds{{instance="{i}"}} - node_boot_time_seconds{{instance="{i}"}})"#),
        ),
    ]
}

/// 预置 NVIDIA GPU 指标目录（nvidia_gpu_exporter；`instance` = `<节点IP>:<nvidia_gpu_exporter_port>`）
///
/// per-GPU 指标 `max by (uuid)`，并以 `* on(uuid) group_left(index) nvidia_smi_gpu_info` 关联
/// GPU `index` 标签（前端渲染为 `GPU 0` 等）；缺 `gpu_info` 时退化为按 `uuid` 命名。
/// 受限设备（如 Jetson Orin）不支持的指标会返回空序列，前端显示空态，MUST NOT 报错。
pub fn nvidia_gpu_metric_catalog(instance: &str) -> Vec<MetricQuery> {
    let i = esc(instance);
    let src = crate::monitor::ComponentKind::NvidiaGpuExporter.service_name();
    let info = format!(r#"max by (uuid, index) (nvidia_smi_gpu_info{{instance="{i}"}})"#);
    // per-GPU 指标 + index join（`base` 应已 `max by (uuid)`）
    let gpu = |key: &str, name: &str, unit: &str, base: String| MetricQuery {
        key: key.to_string(),
        name: name.to_string(),
        unit: unit.to_string(),
        source: src.to_string(),
        category: "gpu".to_string(),
        query: format!("({base}) * on(uuid) group_left(index) {info}"),
    };
    // 不依赖 index 的全局指标（健康 / 计数）
    let plain = |key: &str, name: &str, unit: &str, query: String| MetricQuery {
        key: key.to_string(),
        name: name.to_string(),
        unit: unit.to_string(),
        source: src.to_string(),
        category: "gpu".to_string(),
        query,
    };
    vec![
        gpu(
            "gpu_util",
            "GPU 使用率",
            "%",
            format!(r#"max by (uuid) (nvidia_smi_utilization_gpu_ratio{{instance="{i}"}}) * 100"#),
        ),
        gpu(
            "gpu_mem_used",
            "显存已用",
            "B",
            format!(r#"max by (uuid) (nvidia_smi_memory_used_bytes{{instance="{i}"}})"#),
        ),
        gpu(
            "gpu_mem_total",
            "显存总量",
            "B",
            format!(r#"max by (uuid) (nvidia_smi_memory_total_bytes{{instance="{i}"}})"#),
        ),
        gpu(
            "gpu_mem_pct",
            "显存使用率",
            "%",
            format!(
                r#"max by (uuid) ((nvidia_smi_memory_used_bytes{{instance="{i}"}} / nvidia_smi_memory_total_bytes{{instance="{i}"}}) * 100)"#
            ),
        ),
        // 进程占用显存：统一内存设备（GB10 等）下 memory_used 为 N/A，可用 compute_apps 替代
        gpu(
            "gpu_proc_mem",
            "GPU 显存（进程占用）",
            "B",
            format!(
                r#"sum by (uuid) (max by (uuid, pid) (nvidia_smi_compute_app_used_memory_bytes{{instance="{i}"}}))"#
            ),
        ),
        gpu(
            "gpu_temp",
            "GPU 温度",
            "°C",
            format!(r#"max by (uuid) (nvidia_smi_temperature_gpu{{instance="{i}"}})"#),
        ),
        gpu(
            "gpu_temp_mem",
            "显存温度",
            "°C",
            format!(r#"max by (uuid) (nvidia_smi_temperature_memory{{instance="{i}"}})"#),
        ),
        gpu(
            "gpu_power",
            "功耗",
            "W",
            format!(r#"max by (uuid) (nvidia_smi_power_draw_watts{{instance="{i}"}})"#),
        ),
        gpu(
            "gpu_power_limit",
            "功耗上限",
            "W",
            format!(r#"max by (uuid) (nvidia_smi_power_limit_watts{{instance="{i}"}})"#),
        ),
        gpu(
            "gpu_clocks_graphics",
            "图形时钟",
            "Hz",
            format!(r#"max by (uuid) (nvidia_smi_clocks_current_graphics_clock_hz{{instance="{i}"}})"#),
        ),
        gpu(
            "gpu_clocks_sm",
            "SM 时钟",
            "Hz",
            format!(r#"max by (uuid) (nvidia_smi_clocks_current_sm_clock_hz{{instance="{i}"}})"#),
        ),
        gpu(
            "gpu_clocks_memory",
            "显存时钟",
            "Hz",
            format!(r#"max by (uuid) (nvidia_smi_clocks_current_memory_clock_hz{{instance="{i}"}})"#),
        ),
        gpu(
            "gpu_procs",
            "计算进程数",
            "",
            format!(r#"max by (uuid) (nvidia_smi_compute_apps{{instance="{i}"}})"#),
        ),
        plain(
            "gpu_collect_success",
            "采集成功",
            "",
            format!(r#"max(nvidia_smi_last_collect_success{{instance="{i}"}})"#),
        ),
        plain(
            "gpu_command_exit_code",
            "nvidia-smi 退出码",
            "",
            format!(r#"max(nvidia_smi_command_exit_code{{instance="{i}"}})"#),
        ),
    ]
}

/// 支持的预设时间区间（秒）
pub fn range_seconds(range: &str) -> Option<u64> {
    match range.trim() {
        "15m" => Some(15 * 60),
        "1h" => Some(3600),
        "6h" => Some(6 * 3600),
        "24h" => Some(24 * 3600),
        "7d" => Some(7 * 86400),
        _ => None,
    }
}

/// 默认步长（秒）：区间 / 300，下限 15s
pub fn default_step_secs(duration_secs: u64) -> u64 {
    (duration_secs / 300).max(15)
}

/// 由查询步长推导 `rate()` 的滑动窗口（秒）：`(step × 2).clamp(30, 300)`（纯函数）。
///
/// - 下限 30s：prometheus `scrape_interval` 通常 15s，窗口需至少含 2 个样本，且最接近 vLLM
///   日志的瞬时吞吐语义；
/// - 上限 300s：长区间（24h/7d）步长很大，窗口过短会在采样点之间漏掉活动、曲线锯齿化。
///
/// 仅用于吞吐 / 速率类 counter（直方图分位与比例需保留更长窗口以保证样本量）。
pub fn rate_window_secs(step: u64) -> u64 {
    step.saturating_mul(2).clamp(30, 300)
}

/// 归一查询区间（纯函数）：`(range 字面值或 "custom", start, end, step)`
///
/// - 预设区间：以 `now` 为终点；
/// - 自定义：`start` / `end`（unix 秒）均需提供，且 `start < end`。
pub fn resolve_range(
    range: &str,
    start: Option<i64>,
    end: Option<i64>,
    step: Option<u64>,
    now: i64,
) -> Result<(String, i64, i64, u64), ModelinferError> {
    if let Some(dur) = range_seconds(range) {
        let end = now;
        let start = now - dur as i64;
        let step = step.filter(|s| *s >= 1).unwrap_or_else(|| default_step_secs(dur));
        return Ok((range.trim().to_string(), start, end, step));
    }
    match (start, end) {
        (Some(s), Some(e)) if e > s => {
            let dur = (e - s) as u64;
            let step = step.filter(|v| *v >= 1).unwrap_or_else(|| default_step_secs(dur));
            Ok(("custom".to_string(), s, e, step))
        }
        _ => Err(ModelinferError::new(
            codes::PARAM_INVALID,
            "时间区间非法：请提供 range=15m|1h|6h|24h|7d 或 start/end（unix 秒，start<end）",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_query_range_matrix() {
        let v = serde_json::json!({
            "status": "success",
            "data": {
                "resultType": "matrix",
                "result": [{
                    "metric": {"instance": "10.0.0.1:9100", "mountpoint": "/"},
                    "values": [[1700000000.0, "12.5"], [1700000015.0, "13.0"]]
                }]
            }
        });
        let s = parse_query_range(&v);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].labels.get("instance").unwrap(), "10.0.0.1:9100");
        assert_eq!(s[0].points.len(), 2);
        assert!((s[0].points[1].1 - 13.0).abs() < 1e-9);
        // 非成功 / 空结果
        assert!(parse_query_range(&serde_json::json!({"data": {"result": []}})).is_empty());
    }

    #[test]
    fn catalog_contains_instance_and_keys() {
        let c = system_metric_catalog("10.0.0.1:9100");
        let keys: Vec<&str> = c.iter().map(|m| m.key.as_str()).collect();
        for k in ["cpu_usage", "mem_usage", "load1", "disk_usage", "net_rx", "net_tx", "disk_read", "disk_write", "fs_avail", "uptime"] {
            assert!(keys.contains(&k), "缺少指标 {k}");
        }
        assert!(c.iter().all(|m| m.query.contains("10.0.0.1:9100")));
        assert!(c.iter().all(|m| m.source == "modelinfer-node-exporter" && m.category == "system"));
        // 非聚合指标按 instance / mountpoint 折叠（防历史重复目标造成重复序列）
        let bykey = |k: &str| c.iter().find(|m| m.key == k).unwrap().query.clone();
        assert!(bykey("load1").contains("max by (instance)"));
        assert!(bykey("mem_usage").contains("max by (instance)"));
        assert!(bykey("disk_usage").contains("max by (mountpoint)"));
        assert!(bykey("fs_avail").contains("max by (mountpoint)"));
        assert!(bykey("uptime").contains("max by (instance)"));
        // 网络 / 磁盘 IO 先按 (instance, device) 折叠再求和
        assert!(bykey("net_rx").contains("max by (instance, device)"));
        assert!(bykey("disk_write").contains("max by (instance, device)"));
    }

    #[test]
    fn nvidia_catalog_keys_source_and_join() {
        let c = nvidia_gpu_metric_catalog("10.0.0.1:9835");
        let keys: Vec<&str> = c.iter().map(|m| m.key.as_str()).collect();
        for k in [
            "gpu_util",
            "gpu_mem_used",
            "gpu_mem_total",
            "gpu_mem_pct",
            "gpu_proc_mem",
            "gpu_temp",
            "gpu_temp_mem",
            "gpu_power",
            "gpu_power_limit",
            "gpu_clocks_graphics",
            "gpu_clocks_sm",
            "gpu_clocks_memory",
            "gpu_procs",
            "gpu_collect_success",
            "gpu_command_exit_code",
        ] {
            assert!(keys.contains(&k), "缺少 GPU 指标 {k}");
        }
        assert!(c.iter().all(|m| m.source == "modelinfer-nvidia-gpu-exporter" && m.category == "gpu"));
        assert!(c.iter().all(|m| m.query.contains("10.0.0.1:9835")));
        // per-GPU 指标带 index join；进程显存按 pid 折叠求和
        let bykey = |k: &str| c.iter().find(|m| m.key == k).unwrap().query.clone();
        assert!(bykey("gpu_util").contains("group_left(index)"));
        assert!(bykey("gpu_util").contains("max by (uuid)"));
        assert!(bykey("gpu_proc_mem").contains("max by (uuid, pid)"));
        assert!(bykey("gpu_proc_mem").contains("sum by (uuid)"));
        // 健康指标不带 join
        assert!(!bykey("gpu_collect_success").contains("group_left"));
    }

    #[test]
    fn resolve_range_preset_and_custom() {
        let now = 1_700_000_000;
        let (label, s, e, step) = resolve_range("1h", None, None, None, now).unwrap();
        assert_eq!(label, "1h");
        assert_eq!(e - s, 3600);
        assert_eq!(step, 15, "3600/300=12 → 下限 15");
        let (_, s7, e7, step7) = resolve_range("7d", None, None, None, now).unwrap();
        assert_eq!(e7 - s7, 7 * 86400);
        assert_eq!(step7, 7 * 86400 / 300);

        let (label, s, e, _) = resolve_range("custom", Some(100), Some(1000), Some(30), now).unwrap();
        assert_eq!((label.as_str(), s, e), ("custom", 100, 1000));

        assert!(resolve_range("bogus", None, None, None, now).is_err());
        assert!(resolve_range("custom", Some(1000), Some(100), None, now).is_err());
    }

    #[test]
    fn rate_window_matrix() {
        // 15m/1h（step=15）→ 30s 下限（与 vLLM 日志瞬时吞吐一致）
        assert_eq!(rate_window_secs(15), 30);
        // 6h（step=72）→ 144s
        assert_eq!(rate_window_secs(72), 144);
        // 24h/7d（step>=288）→ 300s 上限
        assert_eq!(rate_window_secs(288), 300);
        assert_eq!(rate_window_secs(2016), 300);
        // 极端小步长仍取下限
        assert_eq!(rate_window_secs(1), 30);
    }

    #[tokio::test]
    async fn mock_series_and_failure() {
        let s = PromSeries {
            labels: BTreeMap::new(),
            points: vec![(1.0, 2.0)],
        };
        let c = MockPrometheusClient::new().with_series("cpu_usage", vec![s]);
        let got = c.query_range("...cpu_usage...", 0, 1, 15).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(c.query_range("other", 0, 1, 15).await.unwrap().len(), 0);

        let f = MockPrometheusClient::failing();
        assert_eq!(f.query_range("x", 0, 1, 15).await.unwrap_err().code, codes::PROMETHEUS_UNAVAILABLE);
    }
}