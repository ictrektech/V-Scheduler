//! 运行配置推理指标：推理引擎类型模型 / 卡片 `global.inference_engine` 解析 / 端口探测推断 /
//! 推理指标向平台 consul 的注册 / 注销 / reconcile。
//!
//! 设计见 `openspec/changes/modelinfer-sglang-metrics-and-history/design.md`（D-MI-SGH-01~08），
//! 长文档 SSOT 见 `specs/docs/design/modelinfer/inference-metrics.md`：
//! - 引擎来源优先级：脚本回传（`script`）> manifest 声明（`manifest`）> 端口探测（`probe`）> `unknown`；
//! - 受支持引擎：`vllm`（服务名 `modelinfer-vllm`）/ `sglang`（服务名 `modelinfer-sglang`）；
//!   prometheus job 即 consul 服务名（平台 `consul_sd` relabel），抓取路径固定 `/metrics`；
//! - 探测 / 注册 / 注销均为 best-effort，失败由后台 reconcile 兜底；
//! - 核心逻辑为纯函数，SSH / consul 经 trait 注入，单测无需真实环境。
//!
//! 指标目录与 PromQL 见 [`metrics`]；注册字段持久化见 [`store`]。
//!
//! # 示例
//!
//! ```no_run
//! use modelinfer::inference::{InferenceDeployment, metrics_service_id};
//!
//! let dep = InferenceDeployment {
//!     id: "01ABC".into(),
//!     name: "sglang-demo".into(),
//!     host: "10.0.0.8".into(),
//!     serve_port: 30000,
//!     model: "deepseek".into(),
//!     card_id: "com.ictrek.demo".into(),
//!     engine: "sglang".into(),
//!     engine_version: String::new(),
//!     active: true,
//! };
//! let svc = dep.service_spec();
//! assert_eq!(svc.name, "modelinfer-sglang");
//! assert_eq!(svc.id, metrics_service_id(&dep.engine, &dep.id));
//! ```

pub mod metrics;
pub mod store;

use std::collections::BTreeMap;

use serde_json::Value;

use crate::monitor::consul::ConsulClient;
use crate::monitor::{ConsulService, ReconcilePlan, RegisteredService};

/// vLLM consul 服务名（= prometheus job）
pub const VLLM_SERVICE_NAME: &str = "modelinfer-vllm";
/// vLLM consul 服务 ID 前缀
pub const VLLM_SERVICE_ID_PREFIX: &str = "modelinfer-vllm-";
/// SGLang consul 服务名（= prometheus job）
pub const SGLANG_SERVICE_NAME: &str = "modelinfer-sglang";
/// SGLang consul 服务 ID 前缀
pub const SGLANG_SERVICE_ID_PREFIX: &str = "modelinfer-sglang-";
/// 探测输出中的指标分界标记
pub const PROBE_MARKER: &str = "---MODELINFER-METRICS---";
/// 探测输出中的 SGLang 服务信息分界标记（`GET /get_server_info`）
pub const SERVER_INFO_MARKER: &str = "---MODELINFER-SGLANG-INFO---";
/// 探测命令 SSH 超时（秒）
pub const PROBE_TIMEOUT_SECS: u64 = 20;

/// 引擎来源字面值
pub mod source {
    /// 卡片脚本 `global` 回传
    pub const SCRIPT: &str = "script";
    /// 卡片 manifest `inference-engine` 声明
    pub const MANIFEST: &str = "manifest";
    /// 端口探测推断
    pub const PROBE: &str = "probe";
    /// 无法判定
    pub const UNKNOWN: &str = "unknown";
}

/// 指标注册状态字面值
pub mod metrics_state {
    /// 未注册
    pub const UNREGISTERED: &str = "unregistered";
    /// 已注册
    pub const REGISTERED: &str = "registered";
    /// 注册失败
    pub const ERROR: &str = "error";
    /// 引擎不受支持（不采集）
    pub const UNSUPPORTED: &str = "unsupported";
}

/// 推理引擎类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceEngine {
    Vllm,
    Sglang,
    Comfyui,
    Unknown,
}

impl InferenceEngine {
    /// 字面值（持久化 / API）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Vllm => "vllm",
            Self::Sglang => "sglang",
            Self::Comfyui => "comfyui",
            Self::Unknown => "unknown",
        }
    }

    /// 从字面值解析（大小写 / 空白归一化；未知 → Unknown）
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "vllm" => Self::Vllm,
            "sglang" => Self::Sglang,
            "comfyui" => Self::Comfyui,
            _ => Self::Unknown,
        }
    }
}

/// 引擎对应的 consul 服务名（= prometheus `job`）
///
/// 不受支持引擎返回空串（调用方据此不注册）。
pub fn service_name(engine: InferenceEngine) -> &'static str {
    match engine {
        InferenceEngine::Vllm => VLLM_SERVICE_NAME,
        InferenceEngine::Sglang => SGLANG_SERVICE_NAME,
        _ => "",
    }
}

/// 引擎对应的 consul 服务 ID 前缀（不受支持引擎返回空串）
pub fn service_id_prefix(engine: InferenceEngine) -> &'static str {
    match engine {
        InferenceEngine::Vllm => VLLM_SERVICE_ID_PREFIX,
        InferenceEngine::Sglang => SGLANG_SERVICE_ID_PREFIX,
        _ => "",
    }
}

/// 是否对该引擎支持指标采集（`vllm` / `sglang`；`comfyui` / `unknown` 不支持）
pub fn metrics_supported(engine: InferenceEngine) -> bool {
    matches!(engine, InferenceEngine::Vllm | InferenceEngine::Sglang)
}

/// 由字面值判断是否支持指标采集（便捷）
pub fn engine_metrics_supported(engine_str: &str) -> bool {
    metrics_supported(InferenceEngine::parse(engine_str))
}

/// 引擎判定结果（类型 + 版本 + 来源）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInfo {
    pub engine: InferenceEngine,
    pub version: String,
    /// `script` / `probe` / `unknown`
    pub source: String,
}

impl EngineInfo {
    pub fn new(engine: InferenceEngine, version: impl Into<String>, source: impl Into<String>) -> Self {
        Self { engine, version: version.into(), source: source.into() }
    }

    /// 无法判定
    pub fn unknown() -> Self {
        Self::new(InferenceEngine::Unknown, "", source::UNKNOWN)
    }

    /// 该结果是否受指标采集支持
    pub fn supported(&self) -> bool {
        metrics_supported(self.engine)
    }
}

/// 解析单个 `inference_engine` 值（字符串 / 对象）
fn parse_engine_value(v: &Value) -> Option<EngineInfo> {
    match v {
        // 字符串形态：`"inference_engine": "vllm"`
        Value::String(s) if !s.trim().is_empty() => Some(EngineInfo::new(
            InferenceEngine::parse(s),
            "",
            source::SCRIPT,
        )),
        // 对象形态：`{ "type": "vllm", "version": "0.9.3" }`（`type` 别名 `engine`）
        Value::Object(_) => {
            let type_str = v
                .get("type")
                .or_else(|| v.get("engine"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())?;
            let version = v.get("version").and_then(Value::as_str).unwrap_or("").trim().to_string();
            Some(EngineInfo::new(InferenceEngine::parse(type_str), version, source::SCRIPT))
        }
        _ => None,
    }
}

/// 从卡片 `start` 阶段回传的阶段全局对象解析推理引擎类型（纯函数）
///
/// 优先 `global.inference_engine`，其次顶层别名 `global.engine`；缺失 / 非法 → `None`
/// （调用方据此回落到端口探测）。
pub fn engine_from_global(global: &Value) -> Option<EngineInfo> {
    let obj = global.as_object()?;
    obj.get("inference_engine")
        .and_then(parse_engine_value)
        .or_else(|| obj.get("engine").and_then(parse_engine_value))
}

/// 从卡片 manifest 顶层 `inference-engine` 解析声明引擎（纯函数）
///
/// - 已识别取值（`vllm` / `sglang` / `comfyui`）→ `Some{ source = manifest }`；
/// - 缺省 / 空 / 未识别取值 → `None`（调用方据此回落到端口探测）。
pub fn engine_from_manifest(manifest: &Value) -> Option<EngineInfo> {
    let raw = manifest.get("inference-engine").and_then(Value::as_str).map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return None;
    }
    match InferenceEngine::parse(raw) {
        InferenceEngine::Unknown => None,
        engine => Some(EngineInfo::new(engine, "", source::MANIFEST)),
    }
}

/// 生成 head 节点上的引擎探测命令（`/version` + `/get_server_info` + `/metrics`）
///
/// 三段输出以标记分隔（见 [`parse_probe_output`]）：
/// 1. `GET /version`（vLLM 版本）；
/// 2. [`SERVER_INFO_MARKER`] 之后的 `GET /get_server_info`（SGLang 版本 / 模型信息）；
/// 3. [`PROBE_MARKER`] 之后的 `/metrics` 前缀样本（含 `vllm:` 或 `sglang:` 行）。
pub fn probe_command(port: u16) -> String {
    format!(
        "( curl -fsS --max-time 5 http://127.0.0.1:{port}/version 2>/dev/null || true; \
         printf '\\n{si}\n'; \
         curl -fsS --max-time 5 http://127.0.0.1:{port}/get_server_info 2>/dev/null || true; \
         printf '\\n{m}\n'; \
         curl -fsS --max-time 5 http://127.0.0.1:{port}/metrics 2>/dev/null | grep -m1 -E '^(vllm|sglang)[:_]' || true )",
        port = port,
        si = SERVER_INFO_MARKER,
        m = PROBE_MARKER
    )
}

/// 探测命令输出（三段：vLLM 版本体 / SGLang 服务信息体 / 指标样本）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeOutput {
    /// `GET /version` 返回的 JSON 对象（vLLM）
    pub version: Option<Value>,
    /// `GET /get_server_info` 返回的 JSON 对象（SGLang）
    pub server_info: Option<Value>,
    /// `/metrics` 前缀样本（可能为空）
    pub metrics_sample: String,
}

/// 解析探测命令输出（纯函数；缺段 / 非法 JSON 均视为缺失，不报错）
pub fn parse_probe_output(stdout: &str) -> ProbeOutput {
    // 先按 metrics 标记切分，再在头部按 server_info 标记切分
    let (head, metrics_sample) = match stdout.split_once(PROBE_MARKER) {
        Some((a, b)) => (a, b.trim().to_string()),
        None => (stdout, String::new()),
    };
    let (version_part, server_info_part) = match head.split_once(SERVER_INFO_MARKER) {
        Some((a, b)) => (a, Some(b)),
        None => (head, None),
    };
    let parse_obj = |s: &str| {
        serde_json::from_str::<Value>(s.trim())
            .ok()
            .filter(Value::is_object)
    };
    ProbeOutput {
        version: parse_obj(version_part),
        server_info: server_info_part.and_then(parse_obj),
        metrics_sample,
    }
}

/// 由探测结果推断引擎（纯函数）
///
/// 判定顺序（metrics 前缀最可靠，优先）：
/// 1. 指标样本含 `sglang:` → SGLang；
/// 2. 指标样本含 `vllm:` / `vllm_` → vLLM；
/// 3. `/get_server_info` 返回含 `version` 字符串的 JSON 对象 → SGLang；
/// 4. `/version` 返回含 `version` 字符串的 JSON 对象 → vLLM；
/// 5. 均不满足 → `unknown`。
///
/// 版本：SGLang 取 `/get_server_info.version`，vLLM 取 `/version.version`。
pub fn infer_engine_from_probe(probe: &ProbeOutput) -> EngineInfo {
    let pick_version = |v: Option<&Value>| {
        v.and_then(|v| v.get("version"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    if probe.metrics_sample.contains("sglang:") {
        return EngineInfo::new(
            InferenceEngine::Sglang,
            pick_version(probe.server_info.as_ref()),
            source::PROBE,
        );
    }
    if probe.metrics_sample.contains("vllm:") || probe.metrics_sample.contains("vllm_") {
        return EngineInfo::new(
            InferenceEngine::Vllm,
            pick_version(probe.version.as_ref()),
            source::PROBE,
        );
    }
    if probe.server_info.as_ref().and_then(|v| v.get("version")).and_then(Value::as_str).is_some() {
        return EngineInfo::new(
            InferenceEngine::Sglang,
            pick_version(probe.server_info.as_ref()),
            source::PROBE,
        );
    }
    if probe.version.as_ref().and_then(|v| v.get("version")).and_then(Value::as_str).is_some() {
        return EngineInfo::new(
            InferenceEngine::Vllm,
            pick_version(probe.version.as_ref()),
            source::PROBE,
        );
    }
    EngineInfo::unknown()
}

/// 运行配置的 consul 服务 ID（按引擎前缀）
///
/// 引擎不受支持 / 为空 → 空串（调用方据此不注册）。
pub fn metrics_service_id(engine: &str, deployment_id: &str) -> String {
    let prefix = service_id_prefix(InferenceEngine::parse(engine));
    if prefix.is_empty() {
        return String::new();
    }
    format!("{prefix}{deployment_id}")
}

/// 是否为本应用注册的推理指标服务 ID（任一引擎前缀）
pub fn is_inference_service_id(sid: &str) -> bool {
    sid.starts_with(VLLM_SERVICE_ID_PREFIX) || sid.starts_with(SGLANG_SERVICE_ID_PREFIX)
}

/// 受支持引擎的服务 ID 前缀集合（用于注销兜底）
pub fn all_service_id_prefixes() -> [&'static str; 2] {
    [VLLM_SERVICE_ID_PREFIX, SGLANG_SERVICE_ID_PREFIX]
}

/// reconcile 期望项（运行配置摘要，纯函数入参）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceDeployment {
    pub id: String,
    pub name: String,
    /// head 节点 IP
    pub host: String,
    pub serve_port: u16,
    /// 对外模型名
    pub model: String,
    pub card_id: String,
    /// 引擎字面值（`vllm` / `unknown` / 空）
    pub engine: String,
    pub engine_version: String,
    /// 是否处于可访问状态（`state ∈ {running, degraded}`）
    pub active: bool,
}

impl InferenceDeployment {
    /// 是否期望注册（受支持引擎且活跃）
    pub fn expected(&self) -> bool {
        self.active && engine_metrics_supported(&self.engine)
    }

    /// 该运行配置的 consul 服务定义（服务名 / ID / tags 按引擎取值）
    ///
    /// 引擎不受支持时 `name` 为空串、`id` 为空串（调用方 MUST NOT 注册）。
    pub fn service_spec(&self) -> ConsulService {
        let engine = InferenceEngine::parse(&self.engine);
        let mut meta = BTreeMap::new();
        meta.insert("deployment_id".to_string(), self.id.clone());
        meta.insert("deployment_name".to_string(), self.name.clone());
        meta.insert("card_id".to_string(), self.card_id.clone());
        meta.insert("model".to_string(), self.model.clone());
        meta.insert("engine".to_string(), self.engine.clone());
        if !self.engine_version.is_empty() {
            meta.insert("engine_version".to_string(), self.engine_version.clone());
        }
        meta.insert("category".to_string(), "inference".to_string());
        ConsulService {
            id: metrics_service_id(&self.engine, &self.id),
            name: service_name(engine).to_string(),
            address: self.host.clone(),
            port: self.serve_port,
            tags: vec!["modelinfer".to_string(), engine.as_str().to_string(), "inference".to_string()],
            meta,
        }
    }
}

/// 生成 reconcile 差异计划（纯函数）
///
/// - 期望集合：每个「受支持且活跃」的运行配置 → 对应 service id；缺失或 address/port 不一致 →
///   register；
/// - consul 中本应用推理指标前缀匹配但不在期望集合 → deregister（残留清理）。
pub fn reconcile_plan(
    deps: &[InferenceDeployment],
    actual: &BTreeMap<String, RegisteredService>,
) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();
    let mut expected_ids: Vec<String> = Vec::new();
    for d in deps.iter().filter(|d| d.expected()) {
        let svc = d.service_spec();
        expected_ids.push(svc.id.clone());
        match actual.get(&svc.id) {
            Some(reg) if reg.address == svc.address && reg.port == svc.port => {}
            _ => plan.register.push(svc),
        }
    }
    for sid in actual.keys() {
        if is_inference_service_id(sid) && !expected_ids.contains(sid) {
            plan.deregister.push(sid.clone());
        }
    }
    plan
}

/// 由部署记录 + head 节点 IP 构造 reconcile 期望项（纯函数）
pub fn deployment_of(m: &crate::entities::deployment::Model, head_host: &str) -> InferenceDeployment {
    InferenceDeployment {
        id: m.id.clone(),
        name: m.name.clone(),
        host: head_host.to_string(),
        serve_port: u16::try_from(m.serve_port.max(1)).unwrap_or(8000),
        model: m.serve_model_name.clone(),
        card_id: m.card_id.clone(),
        engine: m.inference_engine.clone(),
        engine_version: m.inference_engine_version.clone(),
        active: matches!(m.state.as_str(), "running" | "degraded"),
    }
}

/// 启动进程级 reconcile 调度器（只对齐推理指标 consul 注册，不重启任何容器）
///
/// 使用与节点监控相同的周期配置（`monitor_reconcile_secs`）；consul 不可达仅告警。
pub fn spawn_reconcile_scheduler(
    db: sea_orm::DatabaseConnection,
    consul: std::sync::Arc<dyn ConsulClient>,
    interval_secs: u64,
) {
    let interval = std::time::Duration::from_secs(interval_secs.max(10));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match reconcile_once(&db, consul.as_ref()).await {
                Ok(n) if n > 0 => {
                    tracing::info!(target: "modelinfer_inference", changed = n, "推理指标注册 reconcile 完成")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    target: "modelinfer_inference",
                    code = e.code,
                    error = %e.msg,
                    "推理指标注册 reconcile 失败（忽略，保持既有注册）"
                ),
            }
        }
    });
}

/// 执行一次 reconcile：读取运行配置 + consul 服务，按差异计划补注册 / 清残留。
///
/// 返回处理条数。consul 不可达 → `20735`（由调用方决定是否告警）。
pub async fn reconcile_once(
    db: &sea_orm::DatabaseConnection,
    consul: &dyn ConsulClient,
) -> Result<usize, crate::error::ModelinferError> {
    use sea_orm::EntityTrait;

    let rows = crate::entities::deployment::Entity::find()
        .all(db)
        .await
        .map_err(crate::nodes::db_err)?;
    // 仅在需要时才查询 head 节点 host
    let mut deps: Vec<InferenceDeployment> = Vec::new();
    for m in &rows {
        if !matches!(m.state.as_str(), "running" | "degraded") {
            // 非活跃部署也纳入（用于清理残留期望判断时忽略），但不产生期望注册
            deps.push(deployment_of(m, ""));
            continue;
        }
        let host = if m.head_node_id.trim().is_empty() {
            String::new()
        } else {
            crate::nodes::get_node(db, &m.head_node_id).await.map(|n| n.host).unwrap_or_default()
        };
        deps.push(deployment_of(m, &host));
    }
    let actual = consul.list_services().await?;
    let plan = reconcile_plan(&deps, &actual);
    let mut changed = 0usize;
    for svc in &plan.register {
        consul.register(svc).await?;
        changed += 1;
    }
    for id in &plan.deregister {
        consul.deregister(id).await?;
        changed += 1;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reg(id: &str, addr: &str, port: u16) -> RegisteredService {
        RegisteredService { id: id.to_string(), name: VLLM_SERVICE_NAME.to_string(), address: addr.to_string(), port }
    }

    #[test]
    fn parse_engine_normalizes() {
        assert_eq!(InferenceEngine::parse("vLLM"), InferenceEngine::Vllm);
        assert_eq!(InferenceEngine::parse(" vllm "), InferenceEngine::Vllm);
        assert_eq!(InferenceEngine::parse("sglang"), InferenceEngine::Sglang);
        assert_eq!(InferenceEngine::parse("ComfyUI"), InferenceEngine::Comfyui);
        assert_eq!(InferenceEngine::parse(""), InferenceEngine::Unknown);
        assert_eq!(InferenceEngine::Vllm.as_str(), "vllm");
        assert_eq!(InferenceEngine::Sglang.as_str(), "sglang");
        assert_eq!(InferenceEngine::Comfyui.as_str(), "comfyui");
        assert!(metrics_supported(InferenceEngine::Vllm));
        assert!(metrics_supported(InferenceEngine::Sglang));
        assert!(!metrics_supported(InferenceEngine::Unknown));
        assert!(!metrics_supported(InferenceEngine::Comfyui));
        // 服务标识按引擎取值
        assert_eq!(service_name(InferenceEngine::Vllm), "modelinfer-vllm");
        assert_eq!(service_name(InferenceEngine::Sglang), "modelinfer-sglang");
        assert_eq!(service_name(InferenceEngine::Comfyui), "");
        assert_eq!(service_id_prefix(InferenceEngine::Sglang), "modelinfer-sglang-");
    }

    #[test]
    fn engine_from_manifest_matrix() {
        let e = engine_from_manifest(&json!({"inference-engine": "comfyui"})).unwrap();
        assert_eq!(e.engine, InferenceEngine::Comfyui);
        assert_eq!(e.source, source::MANIFEST);
        assert_eq!(engine_from_manifest(&json!({"inference-engine": "vllm"})).unwrap().engine, InferenceEngine::Vllm);
        assert_eq!(engine_from_manifest(&json!({"inference-engine": "sglang"})).unwrap().engine, InferenceEngine::Sglang);
        // 缺省 / 空 / 未识别 / 非字符串 → None
        assert!(engine_from_manifest(&json!({})).is_none());
        assert!(engine_from_manifest(&json!({"inference-engine": ""})).is_none());
        assert!(engine_from_manifest(&json!({"inference-engine": "future-engine"})).is_none());
        assert!(engine_from_manifest(&json!({"inference-engine": 3})).is_none());
        // 旧 camelCase 键不被识别（无兼容别名）
        assert!(engine_from_manifest(&json!({"inferenceEngine": "comfyui"})).is_none());
    }

    #[test]
    fn engine_from_global_matrix() {
        // 字符串
        let g = json!({"inference_engine": "vllm"});
        let e = engine_from_global(&g).unwrap();
        assert_eq!(e.engine, InferenceEngine::Vllm);
        assert_eq!(e.source, source::SCRIPT);
        assert_eq!(e.version, "");
        // 对象
        let g = json!({"inference_engine": {"type": "vllm", "version": "0.9.3"}});
        let e = engine_from_global(&g).unwrap();
        assert_eq!(e.engine, InferenceEngine::Vllm);
        assert_eq!(e.version, "0.9.3");
        // 对象别名 engine
        let g = json!({"inference_engine": {"engine": "vllm"}});
        assert_eq!(engine_from_global(&g).unwrap().engine, InferenceEngine::Vllm);
        // 顶层别名
        let g = json!({"engine": "vllm"});
        assert_eq!(engine_from_global(&g).unwrap().engine, InferenceEngine::Vllm);
        // 缺失 / 非法
        assert!(engine_from_global(&json!({})).is_none());
        assert!(engine_from_global(&json!({"inference_engine": 123})).is_none());
        assert!(engine_from_global(&json!({"inference_engine": ""})).is_none());
        assert!(engine_from_global(&json!({"inference_engine": {"version": "1"}})).is_none());
        assert!(engine_from_global(&json!({"inference_engine": null})).is_none());
    }

    #[test]
    fn probe_command_and_parse() {
        let cmd = probe_command(8000);
        assert!(cmd.contains("127.0.0.1:8000/version"));
        assert!(cmd.contains("127.0.0.1:8000/get_server_info"));
        assert!(cmd.contains("127.0.0.1:8000/metrics"));
        assert!(cmd.contains(PROBE_MARKER));
        assert!(cmd.contains(SERVER_INFO_MARKER));
        assert!(cmd.contains("^(vllm|sglang)"));

        // 三段完整
        let out = format!(
            "{{\"version\":\"0.9.3\"}}\n{si}\n{{\"version\":\"0.5.2\",\"model_path\":\"/m\"}}\n{m}\nsglang:num_running_reqs 1.0\n",
            si = SERVER_INFO_MARKER,
            m = PROBE_MARKER
        );
        let p = parse_probe_output(&out);
        assert_eq!(p.version.as_ref().unwrap()["version"], "0.9.3");
        assert_eq!(p.server_info.as_ref().unwrap()["version"], "0.5.2");
        assert!(p.metrics_sample.contains("sglang:"));

        // 缺 server_info 段（旧格式兼容）
        let out = format!("{{\"version\":\"0.9.3\"}}\n{PROBE_MARKER}\nvllm:num_requests_running 0.0\n");
        let p = parse_probe_output(&out);
        assert_eq!(p.version.as_ref().unwrap()["version"], "0.9.3");
        assert!(p.server_info.is_none());
        assert!(p.metrics_sample.contains("vllm:"));

        // 缺 metrics 段
        let p = parse_probe_output("{\"version\":\"1\"}");
        assert!(p.version.is_some());
        assert!(p.server_info.is_none());
        assert!(p.metrics_sample.is_empty());
        // 非 JSON / 空输出
        let p = parse_probe_output("oops");
        assert!(p.version.is_none() && p.server_info.is_none() && p.metrics_sample.is_empty());
        let p = parse_probe_output("");
        assert_eq!(p, ProbeOutput::default());
    }

    fn probe(version: Option<Value>, server_info: Option<Value>, metrics: &str) -> ProbeOutput {
        ProbeOutput {
            version,
            server_info,
            metrics_sample: metrics.to_string(),
        }
    }

    #[test]
    fn infer_engine_matrix() {
        // vLLM 指标前缀命中，版本取 /version
        let e = infer_engine_from_probe(&probe(
            Some(json!({"version": "0.29.1"})),
            None,
            "vllm:kv_cache_usage_perc 0.1",
        ));
        assert_eq!(e.engine, InferenceEngine::Vllm);
        assert_eq!(e.version, "0.29.1");
        assert_eq!(e.source, source::PROBE);
        // SGLang 指标前缀命中，版本取 /get_server_info
        let e = infer_engine_from_probe(&probe(
            None,
            Some(json!({"version": "0.5.2"})),
            "sglang:num_running_reqs 1.0",
        ));
        assert_eq!(e.engine, InferenceEngine::Sglang);
        assert_eq!(e.version, "0.5.2");
        assert_eq!(e.source, source::PROBE);
        // 指标前缀优先于 JSON 形态（两者都有 version 时以 metrics 判定）
        let e = infer_engine_from_probe(&probe(
            Some(json!({"version": "9.9"})),
            Some(json!({"version": "0.5.2"})),
            "sglang:num_queue_reqs 0",
        ));
        assert_eq!(e.engine, InferenceEngine::Sglang);
        // 无指标样本 → /get_server_info 判定 SGLang
        let e = infer_engine_from_probe(&probe(None, Some(json!({"version": "0.5.0"})), ""));
        assert_eq!(e.engine, InferenceEngine::Sglang);
        // 无指标样本 → /version 判定 vLLM
        let e = infer_engine_from_probe(&probe(Some(json!({"version": "0.9.3"})), None, ""));
        assert_eq!(e.engine, InferenceEngine::Vllm);
        // 都不命中
        let e = infer_engine_from_probe(&probe(None, None, "some:other 1"));
        assert_eq!(e.engine, InferenceEngine::Unknown);
        assert_eq!(e.source, source::UNKNOWN);
        let e = infer_engine_from_probe(&probe(Some(json!({"hi": "there"})), None, ""));
        assert_eq!(e.engine, InferenceEngine::Unknown);
        // /get_server_info 无 version → 不判定
        let e = infer_engine_from_probe(&probe(None, Some(json!({"model_path": "/m"})), ""));
        assert_eq!(e.engine, InferenceEngine::Unknown);
    }

    #[test]
    fn service_spec_fields() {
        let d = InferenceDeployment {
            id: "01ABC".into(),
            name: "qwen25".into(),
            host: "192.168.8.100".into(),
            serve_port: 8000,
            model: "qwen".into(),
            card_id: "com.ictrek.qwen".into(),
            engine: "vllm".into(),
            engine_version: "0.9.3".into(),
            active: true,
        };
        let svc = d.service_spec();
        assert_eq!(svc.id, "modelinfer-vllm-01ABC");
        assert_eq!(svc.name, "modelinfer-vllm");
        assert_eq!(svc.address, "192.168.8.100");
        assert_eq!(svc.port, 8000);
        assert!(svc.tags.contains(&"inference".to_string()));
        assert_eq!(svc.meta.get("deployment_id").unwrap(), "01ABC");
        assert_eq!(svc.meta.get("category").unwrap(), "inference");
        assert_eq!(svc.meta.get("engine_version").unwrap(), "0.9.3");
        // Check URL 由 consul register_body 生成 http://<address>:<port>/metrics
        let body = crate::monitor::consul::register_body(&svc);
        assert_eq!(body["Check"]["HTTP"], "http://192.168.8.100:8000/metrics");

        // SGLang：独立服务名 / ID 前缀 / tags
        let mut sg = d.clone();
        sg.engine = "sglang".into();
        sg.serve_port = 30000;
        let svc = sg.service_spec();
        assert_eq!(svc.id, "modelinfer-sglang-01ABC");
        assert_eq!(svc.name, "modelinfer-sglang");
        assert!(svc.tags.contains(&"sglang".to_string()));
        let body = crate::monitor::consul::register_body(&svc);
        assert_eq!(body["Check"]["HTTP"], "http://192.168.8.100:30000/metrics");

        // 不受支持引擎：空服务名 / 空 ID（调用方不注册）
        let mut unsupported = d.clone();
        unsupported.engine = "comfyui".into();
        let svc = unsupported.service_spec();
        assert!(svc.id.is_empty() && svc.name.is_empty());
    }

    #[test]
    fn service_id_helpers() {
        assert_eq!(metrics_service_id("vllm", "d1"), "modelinfer-vllm-d1");
        assert_eq!(metrics_service_id("sglang", "d1"), "modelinfer-sglang-d1");
        assert_eq!(metrics_service_id("unknown", "d1"), "");
        assert_eq!(metrics_service_id("", "d1"), "");
        assert!(is_inference_service_id("modelinfer-vllm-d1"));
        assert!(is_inference_service_id("modelinfer-sglang-d1"));
        assert!(!is_inference_service_id("modelinfer-node-exporter-n1"));
        assert_eq!(all_service_id_prefixes().len(), 2);
    }

    fn dep(id: &str, state_engine: &str, active: bool, host: &str, port: u16) -> InferenceDeployment {
        InferenceDeployment {
            id: id.into(),
            name: id.into(),
            host: host.into(),
            serve_port: port,
            model: "m".into(),
            card_id: "c".into(),
            engine: state_engine.into(),
            engine_version: String::new(),
            active,
        }
    }

    #[test]
    fn reconcile_plan_registers_missing_and_clears_orphans() {
        let mut actual = BTreeMap::new();
        actual.insert("modelinfer-vllm-A".to_string(), reg("modelinfer-vllm-A", "10.0.0.1", 8000)); // 一致 → 不动
        actual.insert("modelinfer-vllm-B".to_string(), reg("modelinfer-vllm-B", "10.0.0.9", 8000)); // 端口/地址变化 → 覆盖
        actual.insert("modelinfer-vllm-ORPHAN".to_string(), reg("modelinfer-vllm-ORPHAN", "10.0.0.2", 8000)); // 残留
        actual.insert("modelinfer-node-exporter-N1".to_string(), reg("modelinfer-node-exporter-N1", "10.0.0.3", 9100)); // 非本域

        // SGLang 残留（跨引擎前缀也必须清理）
        actual.insert(
            "modelinfer-sglang-ORPHAN".to_string(),
            reg("modelinfer-sglang-ORPHAN", "10.0.0.7", 30000),
        );

        let deps = vec![
            dep("A", "vllm", true, "10.0.0.1", 8000),
            dep("B", "vllm", true, "10.0.0.9", 8010),
            dep("C", "vllm", true, "10.0.0.4", 8000),   // 缺失 → 注册
            dep("D", "unknown", true, "10.0.0.5", 8000), // 非受支持 → 不期望
            dep("E", "vllm", false, "10.0.0.6", 8000),   // 非活跃 → 不期望
            dep("S1", "sglang", true, "10.0.0.8", 30000), // SGLang 缺失 → 注册
            dep("S2", "sglang", false, "10.0.0.8", 30001), // 非活跃 → 不期望
        ];
        let plan = reconcile_plan(&deps, &actual);
        let reg_ids: Vec<String> = plan.register.iter().map(|s| s.id.clone()).collect();
        assert!(reg_ids.contains(&"modelinfer-vllm-B".to_string()));
        assert!(reg_ids.contains(&"modelinfer-vllm-C".to_string()));
        assert!(reg_ids.contains(&"modelinfer-sglang-S1".to_string()));
        assert!(!reg_ids.contains(&"modelinfer-vllm-A".to_string()));
        assert!(!reg_ids.contains(&"modelinfer-vllm-D".to_string()));
        assert!(!reg_ids.contains(&"modelinfer-vllm-E".to_string()));
        assert!(!reg_ids.contains(&"modelinfer-sglang-S2".to_string()));
        let mut dereg = plan.deregister.clone();
        dereg.sort();
        assert_eq!(
            dereg,
            vec![
                "modelinfer-sglang-ORPHAN".to_string(),
                "modelinfer-vllm-ORPHAN".to_string()
            ]
        );
        // SGLang 期望项服务名正确
        let sg = plan
            .register
            .iter()
            .find(|s| s.id == "modelinfer-sglang-S1")
            .expect("缺少 SGLang 注册项");
        assert_eq!(sg.name, "modelinfer-sglang");
    }
}