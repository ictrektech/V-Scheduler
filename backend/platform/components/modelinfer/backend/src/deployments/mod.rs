//! 部署配置领域逻辑：CRUD + 校验（模型变体 / 节点 hostname 唯一 / 参数）

pub mod validate;
pub mod preflight;

use std::path::Path;

use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, QueryOrder, Set};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::access;
use crate::entities::deployment;
use crate::error::{ModelinferError, codes};
use crate::model_hub;
use crate::nodes;
use crate::plaza;
use crate::time;
use validate::{NodeBrief, check_unique_hostnames, config_schema, validate_config_values};

/// 运行配置视图中的节点占位状态：节点已被删除（历史悬挂引用，见
/// `openspec/changes/modelinfer-dangling-node-reference/`）
pub const NODE_STATUS_MISSING: &str = "missing";

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeploymentCreate {
    pub name: String,
    pub plaza_model_id: String,
    #[serde(default = "default_source")]
    pub hub_source: String,
    /// 节点 id 有序列表（首元素为 head）
    pub node_ids: Vec<String>,
    #[serde(default)]
    pub configs: Map<String, Value>,
    /// 权重覆盖（`None` / 空 `model_id` = 使用卡片默认权重）
    #[serde(default)]
    pub weight_override: Option<WeightOverrideInput>,
    /// 镜像覆盖（`None` / 空数组 = 使用卡片 `manifest.images`）
    #[serde(default)]
    pub images_override: Option<Vec<Value>>,
    /// 额外环境变量（在卡片 `configs` 之后注入）
    #[serde(default)]
    pub extra_env: Option<Map<String, Value>>,
    /// 标准接入参数（留空 / 缺省 = 自动推断）
    #[serde(default)]
    pub serve_model_name: Option<String>,
    #[serde(default)]
    pub serve_api_key: Option<String>,
    #[serde(default)]
    pub serve_port: Option<i64>,
    #[serde(default)]
    pub serve_url_prefix: Option<String>,
    /// 本地反代：是否开启（默认 false）
    #[serde(default)]
    pub local_proxy_enabled: bool,
    /// 本地反代：期望端口（缺省 / 0 = 自动分配）
    #[serde(default)]
    pub local_proxy_port: Option<i64>,
    /// 探活失败自动重启开关（缺省 false）
    #[serde(default)]
    pub auto_restart_enabled: bool,
    /// 探活连续失败阈值（秒；缺省 300）
    #[serde(default)]
    pub auto_restart_fail_secs: Option<i64>,
}

/// 权重覆盖输入（`source` ∈ `hf` / `ms`；`model_id` 为 `<org>/<repo>`）
#[derive(Debug, Clone, Deserialize, Default, serde::Serialize, PartialEq, Eq)]
pub struct WeightOverrideInput {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub model_id: String,
}

/// 显式 null 转为清空指令；字段缺省由 serde(default) 保持 None，表示沿用。
fn deserialize_weight_override_update<'de, D>(
    deserializer: D,
) -> Result<Option<WeightOverrideInput>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(
        Option::<WeightOverrideInput>::deserialize(deserializer)?.unwrap_or_default(),
    ))
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeploymentUpdate {
    pub name: Option<String>,
    pub plaza_model_id: Option<String>,
    pub hub_source: Option<String>,
    pub node_ids: Option<Vec<String>>,
    pub configs: Option<Map<String, Value>>,
    /// 权重覆盖（字段缺省 = 沿用现值；显式 null / 空 model_id = 清除覆盖）
    #[serde(default, deserialize_with = "deserialize_weight_override_update")]
    pub weight_override: Option<WeightOverrideInput>,
    /// 镜像覆盖（`None` = 沿用现值；`Some([])` = 清除覆盖）
    #[serde(default)]
    pub images_override: Option<Vec<Value>>,
    /// 额外环境变量（`None` = 沿用现值；`Some({})` = 清空）
    #[serde(default)]
    pub extra_env: Option<Map<String, Value>>,
    /// 标准接入参数（None = 沿用现值）
    #[serde(default)]
    pub serve_model_name: Option<String>,
    #[serde(default)]
    pub serve_api_key: Option<String>,
    #[serde(default)]
    pub serve_port: Option<i64>,
    #[serde(default)]
    pub serve_url_prefix: Option<String>,
    /// 本地反代：是否开启（None = 沿用现值）
    #[serde(default)]
    pub local_proxy_enabled: Option<bool>,
    /// 本地反代：期望端口（None = 沿用现值）
    #[serde(default)]
    pub local_proxy_port: Option<i64>,
    /// 自动重启开关（None = 沿用现值）
    #[serde(default)]
    pub auto_restart_enabled: Option<bool>,
    /// 自动重启连续失败阈值（秒；None = 沿用现值）
    #[serde(default)]
    pub auto_restart_fail_secs: Option<i64>,
}

fn default_source() -> String {
    "hf".to_string()
}

/// 部署是否处于「活跃」状态（节点承载观测口径）
///
/// 活跃 = `starting | running | stopping | degraded`：
/// - `starting`：启动意图已置位，用于收窄同一配置的并发重复启动；
/// - `stopping`：停止清理在节点上执行期间仍计入节点承载；
/// - `stopped` / `failed`：不计入节点承载。
///
/// 注意：活跃状态仅用于统计 / 展示，**不再作为节点级启动互斥条件**。
pub fn is_active_state(state: &str) -> bool {
    matches!(state, "starting" | "running" | "stopping" | "degraded")
}

/// 活跃运行配置对节点的承载项（节点 → 运行配置；用于展示与统计）
///
/// 同一节点可对应多个运行配置；列表不再按节点去重。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OccupiedNode {
    /// 节点 id
    pub node_id: String,
    /// 使用该节点的运行配置 id
    pub deployment_id: String,
    /// 使用该节点的运行配置名称
    pub deployment_name: String,
}

/// 节点摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentNodeView {
    pub id: String,
    pub name: String,
    pub host: String,
    pub hostname: String,
    pub socket_ifname: String,
    pub ib_hca: String,
    pub status: String,
    pub role: String,
}

/// 部署配置视图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentView {
    pub id: String,
    pub name: String,
    pub plaza_model_id: String,
    pub card_id: String,
    pub card_name: String,
    pub weight_id: String,
    /// 家族标识（后端 SSOT 推断，未命中为 `generic`）
    pub family: String,
    /// 卡片 manifest 声明的推理引擎类型（`inference-engine`；缺省为空串）
    pub engine: String,
    pub hub_source: String,
    pub hub_rel_path: String,
    pub node_ids: Vec<String>,
    pub head_node_id: String,
    pub nodes: Vec<DeploymentNodeView>,
    /// 已删除 / 不存在的节点 id（按 `node_ids` 顺序去重；空数组表示全部节点可解析）
    ///
    /// 派生字段：`node_ids` 仍保留持久化原值，不因悬挂引用被静默改写。
    pub missing_node_ids: Vec<String>,
    pub configs: Map<String, Value>,
    /// 权重覆盖（`None` = 未覆盖）
    pub weight_override: Option<WeightOverrideInput>,
    /// 是否覆盖权重
    pub weight_overridden: bool,
    /// 生效权重 id（覆盖优先，否则卡片默认）
    pub effective_weight_id: String,
    /// 生效权重来源（hf / ms）
    pub effective_hub_source: String,
    /// 生效权重的 model-hub 相对路径
    pub effective_hub_rel_path: String,
    /// 镜像覆盖条目（空 = 使用卡片声明）
    pub images_override: Vec<Value>,
    /// 生效镜像条目（覆盖优先，否则卡片声明）
    pub effective_images: Vec<Value>,
    /// 生效镜像来源：`card` / `override`
    pub images_source: String,
    /// 额外环境变量（在卡片 `configs` 之后注入）
    pub extra_env: Map<String, Value>,
    /// 标准接入参数（API Key 不回显，见 `/api/deployments/{id}/access`）
    pub serve_model_name: String,
    pub serve_port: u16,
    pub serve_url_prefix: String,
    /// 统一网关注册状态
    pub vrouter_state: String,
    pub vrouter_upstream_id: String,
    pub vrouter_model: String,
    pub vrouter_registered_at: Option<String>,
    pub vrouter_last_error: String,
    /// 本地反代：开关 / 期望端口（0 = 自动）/ 实际监听端口（0 = 未运行）/ 最近错误
    pub local_proxy_enabled: bool,
    pub local_proxy_port: u16,
    pub local_proxy_active_port: u16,
    pub local_proxy_last_error: String,
    /// 推理引擎类型（vllm / unknown / 空 = 未探测）与来源
    pub inference_engine: String,
    pub inference_engine_version: String,
    pub inference_engine_source: String,
    /// 推理指标注册状态：unregistered / registered / error / unsupported
    pub metrics_state: String,
    pub metrics_service_id: String,
    pub metrics_registered_at: Option<String>,
    pub metrics_last_error: String,
    /// 探活失败自动重启：开关 / 连续失败阈值（秒）
    pub auto_restart_enabled: bool,
    pub auto_restart_fail_secs: i32,
    /// 自动重启运行态：相位（""/stopping/starting）/ 失败起点 / backoff 截止 / 尝试次数 /
    /// 最近动作时间 / 最近错误（见 `specs/docs/design/modelinfer/auto-restart.md`）
    pub auto_restart_phase: String,
    pub auto_restart_failed_since: String,
    pub auto_restart_next_at: String,
    pub auto_restart_attempts: i32,
    pub auto_restart_last_at: String,
    pub auto_restart_last_error: String,
    /// 卡片是否声明 `lifecycle.health-probe`（未声明时自动重启不会生效）
    pub health_probe_declared: bool,
    pub state: String,
    pub desired_state: String,
    pub health_status: String,
    pub last_error: String,
    pub last_started_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn db_err(e: sea_orm::DbErr) -> ModelinferError {
    ModelinferError::new(codes::DB_FAILED, format!("数据库操作失败: {e}"))
}

fn parse_json_map(s: &str) -> Map<String, Value> {
    serde_json::from_str(s).unwrap_or_default()
}

fn parse_json_array(s: &str) -> Vec<Value> {
    serde_json::from_str(s).unwrap_or_default()
}

/// 组装视图（需要节点与卡片信息）
async fn to_view(db: &DatabaseConnection, m: deployment::Model) -> Result<DeploymentView, ModelinferError> {
    let node_ids: Vec<String> = serde_json::from_str(&m.node_ids_json).unwrap_or_default();
    let mut nodes_view = Vec::new();
    let mut missing_node_ids: Vec<String> = Vec::new();
    for (idx, nid) in node_ids.iter().enumerate() {
        let role = if idx == 0 { "head" } else { "worker" };
        match nodes::find_node(db, nid).await? {
            Some(n) => nodes_view.push(DeploymentNodeView {
                id: n.id,
                name: n.name,
                host: n.host,
                hostname: n.hostname,
                socket_ifname: n.socket_ifname,
                ib_hca: n.ib_hca,
                status: n.status,
                role: role.into(),
            }),
            None => {
                // 节点已被删除：返回占位项而不是让整个列表 / 详情失败（D-MI-DNR-01/02）
                if !missing_node_ids.iter().any(|id| id == nid) {
                    missing_node_ids.push(nid.clone());
                }
                nodes_view.push(DeploymentNodeView {
                    id: nid.clone(),
                    name: String::new(),
                    host: String::new(),
                    hostname: String::new(),
                    socket_ifname: String::new(),
                    ib_hca: String::new(),
                    status: NODE_STATUS_MISSING.to_string(),
                    role: role.into(),
                });
            }
        }
    }
    let card = plaza::get_model(db, &m.plaza_model_id).await.ok();
    let card_name = card
        .as_ref()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| m.card_id.clone());
    // 卡片 manifest（家族 / 引擎 / 默认镜像）
    let card_manifest: Value = card
        .as_ref()
        .and_then(|c| serde_json::from_str::<Value>(&c.manifest_json).ok())
        .unwrap_or(Value::Null);
    // 家族推断：优先用卡片的 manifest tags，其次是权重 id / 卡片 id / 展示名
    let card_tags: Vec<String> = card_manifest
        .get("tags")
        .and_then(Value::as_array)
        .cloned()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    // 卡片 manifest 声明的推理引擎类型（缺省空串）
    let engine = card_manifest
        .get("inference-engine")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_string)
        .unwrap_or_default();
    // 卡片是否声明 health-probe（自动重启的唯一健康判据）
    let health_probe_declared = card_manifest
        .get("lifecycle")
        .and_then(Value::as_object)
        .map(|l| l.contains_key(crate::deploy::HEALTH_STAGE))
        .unwrap_or(false);
    let family = crate::model_family::infer(
        &card_tags,
        Some(&m.weight_id),
        Some(&m.card_id),
        Some(&card_name),
    )
    .to_string();

    // 权重 / 镜像覆盖与生效值（覆盖优先；见 runtime-overrides.md）
    let weight_override = if m.weight_override_id.trim().is_empty() {
        None
    } else {
        Some(WeightOverrideInput {
            source: m.weight_override_source.clone(),
            model_id: m.weight_override_id.clone(),
        })
    };
    let weight_overridden = weight_override.is_some();
    let (effective_weight_id, effective_hub_source, effective_hub_rel_path) = match &weight_override {
        Some(o) => (
            o.model_id.clone(),
            o.source.clone(),
            m.weight_override_rel_path.clone(),
        ),
        None => (
            m.weight_id.clone(),
            m.hub_source.clone(),
            m.hub_rel_path.clone(),
        ),
    };
    let images_override = parse_json_array(&m.images_override_json);
    let card_images = crate::images::manifest_images(&card_manifest).unwrap_or_default();
    let effective_images = crate::images::effective_images(&card_images, &images_override);
    let images_source = crate::images::images_source(&images_override).to_string();

    Ok(DeploymentView {
        id: m.id,
        name: m.name,
        plaza_model_id: m.plaza_model_id,
        card_id: m.card_id,
        card_name,
        weight_id: m.weight_id,
        family,
        engine,
        hub_source: m.hub_source,
        hub_rel_path: m.hub_rel_path,
        weight_override,
        weight_overridden,
        effective_weight_id,
        effective_hub_source,
        effective_hub_rel_path,
        images_override,
        effective_images,
        images_source,
        extra_env: parse_json_map(&m.extra_env_json),
        node_ids,
        head_node_id: m.head_node_id,
        nodes: nodes_view,
        missing_node_ids,
        configs: parse_json_map(&m.configs_json),
        serve_model_name: m.serve_model_name,
        serve_port: access::normalize_serve_port(Some(m.serve_port.max(0) as u64)),
        serve_url_prefix: access::normalize_url_prefix(&m.serve_url_prefix),
        vrouter_state: m.vrouter_state,
        vrouter_upstream_id: m.vrouter_upstream_id,
        vrouter_model: m.vrouter_model,
        vrouter_registered_at: m.vrouter_registered_at,
        vrouter_last_error: m.vrouter_last_error,
        local_proxy_enabled: m.local_proxy_enabled,
        local_proxy_port: crate::localproxy::normalize_proxy_port(m.local_proxy_port).max(0) as u16,
        local_proxy_active_port: m.local_proxy_active_port.max(0) as u16,
        local_proxy_last_error: m.local_proxy_last_error,
        inference_engine: m.inference_engine,
        inference_engine_version: m.inference_engine_version,
        inference_engine_source: m.inference_engine_source,
        metrics_state: m.metrics_state,
        metrics_service_id: m.metrics_service_id,
        metrics_registered_at: m.metrics_registered_at,
        metrics_last_error: m.metrics_last_error,
        auto_restart_enabled: m.auto_restart_enabled,
        auto_restart_fail_secs: m.auto_restart_fail_secs,
        auto_restart_phase: m.auto_restart_phase,
        auto_restart_failed_since: m.auto_restart_failed_since,
        auto_restart_next_at: m.auto_restart_next_at,
        auto_restart_attempts: m.auto_restart_attempts,
        auto_restart_last_at: m.auto_restart_last_at,
        auto_restart_last_error: m.auto_restart_last_error,
        health_probe_declared,
        state: m.state,
        desired_state: m.desired_state,
        health_status: m.health_status,
        last_error: m.last_error,
        last_started_at: m.last_started_at,
        created_at: m.created_at,
        updated_at: m.updated_at,
    })
}

/// 校验并解析（模型卡片 → 变体 → 节点）
pub struct ResolvedDeployment {
    pub card_id: String,
    /// 卡片默认权重 id（`manifest.model`；覆盖时保持不变）
    pub weight_id: String,
    /// 卡片默认权重的变体来源（用户选择；覆盖时保留，未选择可为空）
    pub hub_source: String,
    /// 卡片默认权重的 model-hub 相对路径（覆盖时尽力解析，可为空）
    pub hub_rel_path: String,
    /// 权重覆盖来源（空 = 未覆盖）
    pub weight_override_source: String,
    /// 权重覆盖 id（空 = 未覆盖）
    pub weight_override_id: String,
    /// 权重覆盖的 model-hub 相对路径（空 = 未覆盖）
    pub weight_override_rel_path: String,
    pub node_ids: Vec<String>,
    pub configs_json: String,
    /// 镜像覆盖条目（JSON 数组；`[]` = 使用卡片声明）
    pub images_override_json: String,
    /// 额外环境变量（JSON 对象）
    pub extra_env_json: String,
    /// 标准接入参数（已推断 / 归一化）
    pub serve_model_name: String,
    pub serve_api_key: String,
    pub serve_port: i32,
    pub serve_url_prefix: String,
}

/// 覆盖输入（创建 / 更新解析用；已按更新语义与现值合并）
#[derive(Debug, Clone, Default)]
pub struct OverrideInput {
    pub weight_source: String,
    pub weight_model_id: String,
    pub images: Vec<Value>,
    pub extra_env: Map<String, Value>,
}

/// 标准接入参数的请求输入（`None` = 自动推断 / 沿用）
#[derive(Debug, Clone, Default)]
pub struct ServeInput {
    pub model_name: Option<String>,
    pub api_key: Option<String>,
    pub port: Option<i64>,
    pub url_prefix: Option<String>,
}

impl ServeInput {
    /// 从创建 / 更新请求构造
    pub fn from_parts(
        model_name: Option<String>,
        api_key: Option<String>,
        port: Option<i64>,
        url_prefix: Option<String>,
    ) -> Self {
        Self {
            model_name,
            api_key,
            port,
            url_prefix,
        }
    }

    /// 更新时对未提供的字段沿用现值（`keep` = 当前运行配置）
    pub fn merge_with_current(&self, current: &deployment::Model) -> Self {
        Self {
            model_name: self
                .model_name
                .clone()
                .or_else(|| Some(current.serve_model_name.clone())),
            api_key: self.api_key.clone().or_else(|| Some(current.serve_api_key.clone())),
            port: self.port.or(Some(current.serve_port as i64)),
            url_prefix: self
                .url_prefix
                .clone()
                .or_else(|| Some(current.serve_url_prefix.clone())),
        }
    }
}

/// 解析标准接入参数（留空 / 非法 → 自动推断）
///
/// - `model_name`：非空 → 采用；否则取卡片权重末段（回落卡片 id）；
/// - `api_key`：`Some(非空)` → 采用；`Some("")` → 显式不启用鉴权；`None` → 生成 ULID；
/// - `port`：合法 `1..=65535` → 采用；否则默认 `8000`；
/// - `url_prefix`：非空 → 归一化；否则默认 `/v1`。
pub fn resolve_serve(
    input: &ServeInput,
    weight_id: &str,
    card_id: &str,
) -> (String, String, i32, String) {
    let model_name = input
        .model_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| access::default_model_name(weight_id, card_id));
    let api_key = match input.api_key.as_deref() {
        Some(v) => v.to_string(),
        None => ulid::Ulid::new().to_string(),
    };
    let port = access::normalize_serve_port(input.port.map(|p| p.max(0) as u64)) as i32;
    let url_prefix = input
        .url_prefix
        .as_deref()
        .map(access::normalize_url_prefix)
        .unwrap_or_else(|| access::DEFAULT_URL_PREFIX.to_string());
    (model_name, api_key, port, url_prefix)
}

#[allow(clippy::too_many_arguments)]
pub async fn resolve_config(
    db: &DatabaseConnection,
    hub_dir: &Path,
    name: &str,
    plaza_model_id: &str,
    hub_source: &str,
    node_ids: &[String],
    configs: &Map<String, Value>,
    serve: &ServeInput,
    over: &OverrideInput,
) -> Result<ResolvedDeployment, ModelinferError> {
    if name.trim().is_empty() {
        return Err(ModelinferError::new(codes::PARAM_INVALID, "配置名称不能为空"));
    }
    if node_ids.is_empty() {
        return Err(ModelinferError::new(codes::PARAM_INVALID, "请至少选择一个节点"));
    }

    // 模型卡片（`model` 可选：无权重卡片跳过变体解析）
    let card = plaza::get_model(db, plaza_model_id).await?;

    let manifest: Value = serde_json::from_str(&card.manifest_json).unwrap_or(Value::Null);
    validate::check_lifecycle_order(&manifest)?;
    // 模型卡片 caps 拓扑约束：`caps.max-nodes`（0 = 不限制）
    validate::check_caps_node_limit(&manifest, node_ids.len())?;

    // 参数校验
    let schema = config_schema(&manifest);
    let configs_json = validate_config_values(&schema, configs)?;

    // 覆盖校验（权重 / 镜像 / 额外环境变量）
    let card_config_names: Vec<String> = schema
        .iter()
        .filter_map(|s| s.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    let weight_over = validate::validate_weight_override(&over.weight_source, &over.weight_model_id)?;
    let images_override = validate::validate_images_override(&over.images)?;
    let extra_env = validate::validate_extra_env(&over.extra_env, &card_config_names)?;
    let weight_overridden = weight_over.is_some();

    // 生效权重：覆盖优先，否则卡片 `manifest.model` + 用户选择的变体来源。
    let card_model = card.model.trim();
    let (eff_id, eff_source) = validate::effective_weight(
        card_model,
        hub_source,
        weight_over.as_ref().map(|(s, m)| (s.as_str(), m.as_str())),
    );

    // 权重变体：生效权重 MUST 命中 model-hub（`export/<hf|ms>/<weight>/current`）；
    // 无权重卡片且未覆盖时跳过，weight_id / hub_source / hub_rel_path 均为空。
    let (weight_id, hub_source_out, hub_rel_path, override_rel_path) = if eff_id.is_empty() {
        (String::new(), String::new(), String::new(), String::new())
    } else {
        if !matches!(eff_source.as_str(), "hf" | "ms") {
            return Err(ModelinferError::new(codes::PARAM_INVALID, "hub_source 只能是 hf 或 ms"));
        }
        let variants = model_hub::find_variants(hub_dir, &eff_id);
        let variant = variants
            .iter()
            .find(|v| v.source == eff_source)
            .ok_or_else(|| {
                if weight_overridden {
                    ModelinferError::new(
                        codes::MODEL_NOT_FOUND,
                        format!("权重覆盖在 model-hub 中不存在: {eff_source}:{eff_id}"),
                    )
                } else {
                    ModelinferError::new(
                        codes::MODEL_NOT_FOUND,
                        format!("model-hub 中无匹配模型变体: {eff_source}:{eff_id}"),
                    )
                }
            })?;
        if weight_overridden {
            // 覆盖时卡片默认权重**不要求**存在：尽力解析其相对路径供展示，缺失留空。
            let card_src = hub_source.trim();
            let card_rel = if card_model.is_empty() || !matches!(card_src, "hf" | "ms") {
                String::new()
            } else {
                model_hub::find_variants(hub_dir, card_model)
                    .iter()
                    .find(|v| v.source == card_src)
                    .map(|v| v.rel_path.clone())
                    .unwrap_or_default()
            };
            (card_model.to_string(), card_src.to_string(), card_rel, variant.rel_path.clone())
        } else {
            (eff_id.clone(), variant.source.clone(), variant.rel_path.clone(), String::new())
        }
    };

    // 节点
    let models = nodes::load_models(db, Some(node_ids)).await?;
    let briefs: Vec<NodeBrief> = models
        .iter()
        .map(|n| NodeBrief {
            id: n.id.clone(),
            name: n.name.clone(),
            host: n.host.clone(),
            hostname: n.hostname.clone(),
        })
        .collect();
    check_unique_hostnames(&briefs)?;

    // 标准接入参数（留空 / 非法 → 自动推断）
    let (serve_model_name, serve_api_key, serve_port, serve_url_prefix) =
        resolve_serve(serve, &card.model, &card.model_id);

    let (weight_override_source, weight_override_id) = weight_over.clone().unwrap_or_default();

    Ok(ResolvedDeployment {
        card_id: card.model_id.clone(),
        weight_id,
        hub_source: hub_source_out,
        hub_rel_path,
        weight_override_source,
        weight_override_id,
        weight_override_rel_path: override_rel_path,
        node_ids: node_ids.to_vec(),
        configs_json: serde_json::to_string(&configs_json).unwrap_or_else(|_| "{}".into()),
        images_override_json: serde_json::to_string(&images_override).unwrap_or_else(|_| "[]".into()),
        extra_env_json: serde_json::to_string(&extra_env).unwrap_or_else(|_| "{}".into()),
        serve_model_name,
        serve_api_key,
        serve_port,
        serve_url_prefix,
    })
}

pub async fn list_deployments(db: &DatabaseConnection) -> Result<Vec<DeploymentView>, ModelinferError> {
    let rows = deployment::Entity::find()
        .order_by_asc(deployment::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(to_view(db, r).await?);
    }
    Ok(out)
}

// ============ 列表筛选 / 排序 / 统计（纯函数，供分页前端参数使用） ============

/// 运行配置排序方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentSort {
    /// 名称（Unicode 码点序；不做拼音排序，见 change design §6）
    Name,
    /// 最近更新（`last_started_at` 优先，其次 `updated_at`，降序）
    Recent,
    /// 状态（字典序）
    State,
}

impl DeploymentSort {
    /// 解析排序取值，未知值回落 `Name`
    pub fn parse(value: &str) -> Self {
        match value {
            "recent" => Self::Recent,
            "state" => Self::State,
            _ => Self::Name,
        }
    }
}

/// 运行配置筛选参数
#[derive(Debug, Clone)]
pub struct DeploymentFilter {
    /// 关键词（大小写不敏感），匹配 名称 / 卡片名 / 权重 id
    pub keyword: String,
    /// 状态；`all` / 空表示不限
    pub state: String,
    /// 模型卡片 id；`all` / 空表示不限
    pub card_id: String,
    pub sort: DeploymentSort,
}

impl Default for DeploymentFilter {
    fn default() -> Self {
        Self {
            keyword: String::new(),
            state: "all".into(),
            card_id: "all".into(),
            sort: DeploymentSort::Name,
        }
    }
}

/// 运行配置的可搜索文本（与前端治理前字段集合一致）
pub fn deployment_search_text(d: &DeploymentView) -> String {
    [
        d.name.as_str(),
        d.card_name.as_str(),
        d.weight_id.as_str(),
        d.effective_weight_id.as_str(),
    ]
    .join(" ")
    .to_lowercase()
}

/// 按关键词 / 状态 / 卡片筛选并排序运行配置
pub fn filter_deployments(
    rows: Vec<DeploymentView>,
    f: &DeploymentFilter,
) -> Vec<DeploymentView> {
    let kw = f.keyword.trim().to_lowercase();
    let mut list: Vec<DeploymentView> = rows
        .into_iter()
        .filter(|d| {
            if f.state != "all" && !f.state.is_empty() && d.state != f.state {
                return false;
            }
            if f.card_id != "all" && !f.card_id.is_empty() && d.card_id != f.card_id {
                return false;
            }
            if kw.is_empty() {
                return true;
            }
            deployment_search_text(d).contains(&kw)
        })
        .collect();

    list.sort_by(|a, b| match f.sort {
        DeploymentSort::Recent => {
            let at = a.last_started_at.as_deref().unwrap_or(&a.updated_at);
            let bt = b.last_started_at.as_deref().unwrap_or(&b.updated_at);
            bt.cmp(at)
        }
        DeploymentSort::State => a.state.cmp(&b.state),
        DeploymentSort::Name => a.name.cmp(&b.name),
    });
    list
}

/// 运行配置的未筛选统计（页头统计条）
#[derive(Debug, Clone, Serialize)]
pub struct DeploymentStats {
    pub total: usize,
    /// `starting | running | degraded`（与页面既有口径一致）
    pub running: usize,
    pub stopped: usize,
    pub failed: usize,
}

/// 计算运行配置的未筛选统计
pub fn deployment_stats(rows: &[DeploymentView]) -> DeploymentStats {
    let count = |states: &[&str]| rows.iter().filter(|d| states.contains(&d.state.as_str())).count();
    DeploymentStats {
        total: rows.len(),
        running: count(&["starting", "running", "degraded"]),
        stopped: count(&["stopped"]),
        failed: count(&["failed"]),
    }
}

pub async fn get_deployment(
    db: &DatabaseConnection,
    id: &str,
) -> Result<deployment::Model, ModelinferError> {
    deployment::Entity::find_by_id(id.to_string())
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ModelinferError::new(codes::NOT_FOUND, format!("部署配置不存在: {id}")))
}

pub async fn get_view(db: &DatabaseConnection, id: &str) -> Result<DeploymentView, ModelinferError> {
    to_view(db, get_deployment(db, id).await?).await
}

pub async fn create_deployment(
    db: &DatabaseConnection,
    hub_dir: &Path,
    req: DeploymentCreate,
) -> Result<DeploymentView, ModelinferError> {
    let serve = ServeInput::from_parts(
        req.serve_model_name.clone(),
        req.serve_api_key.clone(),
        req.serve_port,
        req.serve_url_prefix.clone(),
    );
    let over = OverrideInput {
        weight_source: req
            .weight_override
            .as_ref()
            .map(|w| w.source.clone())
            .unwrap_or_default(),
        weight_model_id: req
            .weight_override
            .as_ref()
            .map(|w| w.model_id.clone())
            .unwrap_or_default(),
        images: req.images_override.clone().unwrap_or_default(),
        extra_env: req.extra_env.clone().unwrap_or_default(),
    };
    let r = resolve_config(
        db,
        hub_dir,
        &req.name,
        &req.plaza_model_id,
        &req.hub_source,
        &req.node_ids,
        &req.configs,
        &serve,
        &over,
    )
    .await?;
    let now = time::now_iso();
    let local_proxy_port = crate::localproxy::normalize_proxy_port(
        req.local_proxy_port.unwrap_or(0).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
    );
    // 自动重启配置（阈值合法性在落库前校验，非法返回 20748）
    let auto_restart_fail_secs = validate::normalize_auto_restart_fail_secs(
        req.auto_restart_fail_secs
            .unwrap_or(validate::DEFAULT_AUTO_RESTART_FAIL_SECS as i64),
    )?;
    let am = deployment::ActiveModel {
        id: Set(ulid::Ulid::new().to_string()),
        name: Set(req.name.trim().to_string()),
        plaza_model_id: Set(req.plaza_model_id),
        card_id: Set(r.card_id),
        weight_id: Set(r.weight_id),
        hub_source: Set(r.hub_source),
        hub_rel_path: Set(r.hub_rel_path),
        weight_override_source: Set(r.weight_override_source),
        weight_override_id: Set(r.weight_override_id),
        weight_override_rel_path: Set(r.weight_override_rel_path),
        images_override_json: Set(r.images_override_json),
        extra_env_json: Set(r.extra_env_json),
        head_node_id: Set(r.node_ids.first().cloned().unwrap_or_default()),
        node_ids_json: Set(serde_json::to_string(&r.node_ids).unwrap_or_else(|_| "[]".into())),
        configs_json: Set(r.configs_json),
        serve_model_name: Set(r.serve_model_name),
        serve_api_key: Set(r.serve_api_key),
        serve_port: Set(r.serve_port),
        serve_url_prefix: Set(r.serve_url_prefix),
        vrouter_state: Set("unregistered".to_string()),
        vrouter_upstream_id: Set(String::new()),
        vrouter_model: Set(String::new()),
        vrouter_registered_at: Set(None),
        vrouter_last_error: Set(String::new()),
        local_proxy_enabled: Set(req.local_proxy_enabled),
        local_proxy_port: Set(local_proxy_port),
        local_proxy_active_port: Set(0),
        local_proxy_last_error: Set(String::new()),
        inference_engine: Set(String::new()),
        inference_engine_version: Set(String::new()),
        inference_engine_source: Set(String::new()),
        metrics_state: Set("unregistered".to_string()),
        metrics_service_id: Set(String::new()),
        metrics_registered_at: Set(None),
        metrics_last_error: Set(String::new()),
        auto_restart_enabled: Set(req.auto_restart_enabled),
        auto_restart_fail_secs: Set(auto_restart_fail_secs),
        auto_restart_phase: Set(String::new()),
        auto_restart_failed_since: Set(String::new()),
        auto_restart_next_at: Set(String::new()),
        auto_restart_attempts: Set(0),
        auto_restart_last_at: Set(String::new()),
        auto_restart_last_error: Set(String::new()),
        state: Set("stopped".to_string()),
        desired_state: Set("stopped".to_string()),
        health_status: Set("unknown".to_string()),
        last_error: Set(String::new()),
        last_started_at: Set(None),
        created_at: Set(now.clone()),
        updated_at: Set(now),
    };
    let inserted = am.insert(db).await.map_err(db_err)?;
    to_view(db, inserted).await
}

pub async fn update_deployment(
    db: &DatabaseConnection,
    hub_dir: &Path,
    id: &str,
    req: DeploymentUpdate,
) -> Result<DeploymentView, ModelinferError> {
    let current = get_deployment(db, id).await?;
    if !validate::is_editable(&current.state, &current.desired_state) {
        return Err(ModelinferError::new(
            codes::DEPLOYMENT_STATE_INVALID,
            format!(
                "部署配置当前状态为 {}（期望 {}），仅未启动状态可编辑",
                current.state, current.desired_state
            ),
        ));
    }
    let name = req.name.clone().unwrap_or_else(|| current.name.clone());
    let plaza_model_id = req
        .plaza_model_id
        .clone()
        .unwrap_or_else(|| current.plaza_model_id.clone());
    let hub_source = req.hub_source.clone().unwrap_or_else(|| current.hub_source.clone());
    let node_ids: Vec<String> = match &req.node_ids {
        Some(v) => v.clone(),
        None => serde_json::from_str(&current.node_ids_json).unwrap_or_default(),
    };
    let configs = req
        .configs
        .clone()
        .unwrap_or_else(|| parse_json_map(&current.configs_json));
    // 标准接入参数：请求未提供时沿用现值
    let serve = ServeInput::from_parts(
        req.serve_model_name.clone(),
        req.serve_api_key.clone(),
        req.serve_port,
        req.serve_url_prefix.clone(),
    )
    .merge_with_current(&current);

    // 覆盖：None = 沿用现值；空值 = 清除（由 resolve_config 统一归一 / 校验）
    let over = OverrideInput {
        weight_source: match &req.weight_override {
            Some(w) => w.source.clone(),
            None => current.weight_override_source.clone(),
        },
        weight_model_id: match &req.weight_override {
            Some(w) => w.model_id.clone(),
            None => current.weight_override_id.clone(),
        },
        images: match &req.images_override {
            Some(v) => v.clone(),
            None => parse_json_array(&current.images_override_json),
        },
        extra_env: match &req.extra_env {
            Some(m) => m.clone(),
            None => parse_json_map(&current.extra_env_json),
        },
    };

    let r = resolve_config(
        db,
        hub_dir,
        &name,
        &plaza_model_id,
        &hub_source,
        &node_ids,
        &configs,
        &serve,
        &over,
    )
    .await?;

    // 自动重启：None = 沿用现值；阈值非法返回 20748
    let auto_restart_enabled = req.auto_restart_enabled.unwrap_or(current.auto_restart_enabled);
    let auto_restart_fail_secs = match req.auto_restart_fail_secs {
        Some(v) => validate::normalize_auto_restart_fail_secs(v)?,
        None => current.auto_restart_fail_secs,
    };
    // 关闭开关或阈值变更 → 清零运行态（避免沿用旧计时 / 旧 backoff 造成误触发）
    let auto_restart_reset = !auto_restart_enabled
        || auto_restart_fail_secs != current.auto_restart_fail_secs
        || !current.auto_restart_enabled;

    let mut am: deployment::ActiveModel = current.into();
    am.name = Set(name.trim().to_string());
    am.plaza_model_id = Set(plaza_model_id);
    am.card_id = Set(r.card_id);
    am.weight_id = Set(r.weight_id);
    am.hub_source = Set(r.hub_source);
    am.hub_rel_path = Set(r.hub_rel_path);
    am.weight_override_source = Set(r.weight_override_source);
    am.weight_override_id = Set(r.weight_override_id);
    am.weight_override_rel_path = Set(r.weight_override_rel_path);
    am.images_override_json = Set(r.images_override_json);
    am.extra_env_json = Set(r.extra_env_json);
    am.head_node_id = Set(r.node_ids.first().cloned().unwrap_or_default());
    am.node_ids_json = Set(serde_json::to_string(&r.node_ids).unwrap_or_else(|_| "[]".into()));
    am.configs_json = Set(r.configs_json);
    am.serve_model_name = Set(r.serve_model_name);
    am.serve_api_key = Set(r.serve_api_key);
    am.serve_port = Set(r.serve_port);
    am.serve_url_prefix = Set(r.serve_url_prefix);
    // 本地反代：仅在请求提供时更新（运行态由反代生命周期回写）
    if let Some(enabled) = req.local_proxy_enabled {
        am.local_proxy_enabled = Set(enabled);
    }
    if let Some(port) = req.local_proxy_port {
        am.local_proxy_port = Set(crate::localproxy::normalize_proxy_port(
            port.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        ));
    }
    am.auto_restart_enabled = Set(auto_restart_enabled);
    am.auto_restart_fail_secs = Set(auto_restart_fail_secs);
    if auto_restart_reset {
        am.auto_restart_phase = Set(String::new());
        am.auto_restart_failed_since = Set(String::new());
        am.auto_restart_next_at = Set(String::new());
        am.auto_restart_attempts = Set(0);
        am.auto_restart_last_at = Set(String::new());
        am.auto_restart_last_error = Set(String::new());
    }
    am.updated_at = Set(time::now_iso());
    let updated = am.update(db).await.map_err(db_err)?;
    to_view(db, updated).await
}

pub async fn delete_deployment(db: &DatabaseConnection, id: &str) -> Result<(), ModelinferError> {
    let current = get_deployment(db, id).await?;
    if current.state != "stopped" {
        return Err(ModelinferError::new(
            codes::DEPLOYMENT_STATE_INVALID,
            format!("部署配置当前状态为 {}，仅停止状态可删除", current.state),
        ));
    }
    deployment::Entity::delete_by_id(id.to_string())
        .exec(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// 更新本地反代配置（运行中也可操作；运行态由反代生命周期回写）
pub async fn update_local_proxy(
    db: &DatabaseConnection,
    id: &str,
    enabled: bool,
    port: Option<i64>,
) -> Result<deployment::Model, ModelinferError> {
    let current = get_deployment(db, id).await?;
    let mut am: deployment::ActiveModel = current.into();
    am.local_proxy_enabled = Set(enabled);
    if let Some(p) = port {
        am.local_proxy_port = Set(crate::localproxy::normalize_proxy_port(
            p.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        ));
    }
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(db_err)
}

/// 列出引用该节点的运行配置（`id`, `name`），按创建时间顺序。
///
/// 用于节点删除保护（`20712`，见 `openspec/changes/modelinfer-dangling-node-reference/`）：
/// 任一运行配置的 `node_ids_json` 包含该节点即视为引用，**与运行状态无关**；
/// `node_ids_json` 解析失败按空集合处理（防御式，永不 panic）。
pub async fn deployments_referencing_node(
    db: &DatabaseConnection,
    node_id: &str,
) -> Result<Vec<(String, String)>, ModelinferError> {
    if node_id.trim().is_empty() {
        return Ok(Vec::new());
    }
    let rows = deployment::Entity::find()
        .order_by_asc(deployment::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .filter(|d| {
            serde_json::from_str::<Vec<String>>(&d.node_ids_json)
                .unwrap_or_default()
                .iter()
                .any(|id| id == node_id)
        })
        .map(|d| (d.id, d.name))
        .collect())
}

/// 列出活跃运行配置对节点的承载明细（`exclude_id` 为需要排除的运行配置 id，如「自身」）
///
/// 系由活跃运行配置持久化的节点集合（`node_ids_json`）实时派生，不落库。
/// **同一节点可被多个运行配置使用**：每个 (节点, 运行配置) 组合返回一项，不按节点去重；
/// 同一运行配置内重复的节点 id 去重。按运行配置创建时间稳定排序。
/// `node_ids_json` 解析失败按空集合处理（防御式，永不 panic）。
pub async fn occupied_nodes(
    db: &DatabaseConnection,
    exclude_id: &str,
) -> Result<Vec<OccupiedNode>, ModelinferError> {
    let rows = deployment::Entity::find()
        .order_by_asc(deployment::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)?;
    let mut out: Vec<OccupiedNode> = Vec::new();
    for d in rows {
        if d.id == exclude_id || !is_active_state(&d.state) {
            continue;
        }
        let ids: Vec<String> = serde_json::from_str(&d.node_ids_json).unwrap_or_default();
        for node_id in ids {
            // 仅去重同一运行配置内的重复节点；不同运行配置可共享同一节点。
            if out
                .iter()
                .any(|o| o.node_id == node_id && o.deployment_id == d.id)
            {
                continue;
            }
            out.push(OccupiedNode {
                node_id,
                deployment_id: d.id.clone(),
                deployment_name: d.name.clone(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn seed_card(db: &DatabaseConnection) -> String {
        // 通过 plaza 领域插入一行已登记模型（直接构造 ActiveModel 更简单）
        let manifest = serde_json::json!({
            "id": "com.ictrek.modelinfer.test-mock",
            "version": "1.0.0",
            "model": "Qwen/Qwen3-0.6B",
            "configs": [{"name":"A","type":"integer","default":1}]
        });
        let now = time::now_iso();
        let am = crate::entities::plaza_model::ActiveModel {
            id: Set("com.ictrek.modelinfer.test-mock@1.0.0".into()),
            model_id: Set("com.ictrek.modelinfer.test-mock".into()),
            model: Set("Qwen/Qwen3-0.6B".into()),
            name: Set("mock".into()),
            version: Set("1.0.0".into()),
            source: Set("plaza".into()),
            source_id: Set(String::new()),
            path: Set("/tmp/x".into()),
            package_path: Set("/tmp/x/model.tar.gz".into()),
            package_sha256: Set(String::new()),
            manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
            state: Set("available".into()),
            size_bytes: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        };
        am.insert(db).await.unwrap();
        "com.ictrek.modelinfer.test-mock@1.0.0".to_string()
    }

    async fn seed_node(db: &DatabaseConnection, host: &str, hostname: &str) -> String {
        nodes::create_node(
            db,
            &crate::ssh::MockSshExec::new()
                .with_rule("hostname", 0, hostname),
            crate::nodes::NodeCreate {
                name: host.to_string(),
                host: host.to_string(),
                ssh_port: 22,
                ssh_user: "root".into(),
                auth_method: "password".into(),
                ssh_password: "x".into(),
                ssh_key: String::new(),
                sudo_password: String::new(),
                hostname: Some(hostname.to_string()),
                socket_ifname: Some("enp1s0".into()),
                ib_hca: Some(String::new()),
                fscache: false,
                node_exporter_port: crate::nodes::DEFAULT_NODE_EXPORTER_PORT,
                nvidia_gpu_exporter_port: crate::nodes::DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn create_validate_and_delete() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let n2 = seed_node(&db, "10.0.0.2", "h2").await;

        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/Qwen/Qwen3-0.6B/current")).unwrap();

        let mut configs = Map::new();
        configs.insert("A".into(), serde_json::json!(7));
        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d1".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1.clone(), n2.clone()],
                configs,
                            ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(created.state, "stopped");
        assert_eq!(created.head_node_id, n1);
        assert_eq!(created.nodes[0].role, "head");
        assert_eq!(created.weight_id, "Qwen/Qwen3-0.6B");
        assert_eq!(created.hub_rel_path, "export/hf/Qwen/Qwen3-0.6B/current");
        assert!(created.missing_node_ids.is_empty());

        // 删除（停止态）
        delete_deployment(&db, &created.id).await.unwrap();
        assert!(list_deployments(&db).await.unwrap().is_empty());
    }

    /// 历史悬空引用（节点已删除）不得让运行配置列表 / 详情整体失败
    /// （见 openspec/changes/modelinfer-dangling-node-reference/）
    #[tokio::test]
    async fn view_degrades_for_deleted_node() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let n2 = seed_node(&db, "10.0.0.2", "h2").await;

        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/Qwen/Qwen3-0.6B/current")).unwrap();

        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "stale".into(),
                plaza_model_id: card,
                hub_source: "hf".into(),
                node_ids: vec![n1.clone(), n2.clone()],
                configs: Map::new(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // 模拟历史删除（HTTP handler 已加删除保护，此处直接走领域删除）
        nodes::delete_node(&db, &n1).await.unwrap();

        let list = list_deployments(&db).await.unwrap();
        assert_eq!(list.len(), 1);
        let v = &list[0];
        assert_eq!(v.id, created.id);
        // 持久化原值不被静默改写
        assert_eq!(v.node_ids, vec![n1.clone(), n2.clone()]);
        assert_eq!(v.missing_node_ids, vec![n1.clone()]);
        assert_eq!(v.nodes.len(), 2);
        assert_eq!(v.nodes[0].id, n1);
        assert_eq!(v.nodes[0].status, NODE_STATUS_MISSING);
        assert_eq!(v.nodes[0].name, "");
        assert_eq!(v.nodes[0].host, "");
        assert_eq!(v.nodes[0].role, "head");
        assert_eq!(v.nodes[1].status, "online");

        // 详情同样可读
        let detail = get_view(&db, &created.id).await.unwrap();
        assert_eq!(detail.missing_node_ids, vec![n1]);

        // 节点删除保护的数据源：任一引用（含 stopped）都会命中
        let refs = deployments_referencing_node(&db, &v.node_ids[1]).await.unwrap();
        assert_eq!(refs, vec![(created.id.clone(), "stale".to_string())]);
        assert!(deployments_referencing_node(&db, "not-exist").await.unwrap().is_empty());
    }

    async fn seed_comfyui_card(db: &DatabaseConnection) -> String {
        use sea_orm::{ActiveModelTrait, Set};
        let manifest = serde_json::json!({
            "id": "com.ictrek.comfyui-gb10",
            "version": "1.0.0",
            "inference-engine": "comfyui",
            "images": ["nvidia/cuda:13.1.1-devel-ubuntu24.04"],
            "lifecycle": {"start": "./scripts/start.sh"},
        });
        let now = time::now_iso();
        crate::entities::plaza_model::ActiveModel {
            id: Set("com.ictrek.comfyui-gb10@1.0.0".into()),
            model_id: Set("com.ictrek.comfyui-gb10".into()),
            model: Set(String::new()),
            name: Set("comfyui".into()),
            version: Set("1.0.0".into()),
            source: Set("local".into()),
            source_id: Set(String::new()),
            path: Set("/tmp/c".into()),
            package_path: Set("/tmp/c/model.tar.gz".into()),
            package_sha256: Set(String::new()),
            manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
            state: Set("available".into()),
            size_bytes: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
        "com.ictrek.comfyui-gb10@1.0.0".to_string()
    }

    #[tokio::test]
    async fn weightless_card_creates_without_variant() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_comfyui_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        // 空 hub：无任何变体
        let tmp = tempfile::TempDir::new().unwrap();

        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "cf".into(),
                plaza_model_id: card,
                hub_source: String::new(),
                node_ids: vec![n1],
                configs: Map::new(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(created.weight_id, "");
        assert_eq!(created.hub_rel_path, "");
        assert_eq!(created.hub_source, "");
        // 视图暴露卡片声明的引擎
        assert_eq!(created.engine, "comfyui");
    }

    #[tokio::test]
    async fn duplicate_hostname_rejected_and_missing_variant() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "same").await;
        let n2 = seed_node(&db, "10.0.0.2", "same").await;

        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/Qwen/Qwen3-0.6B/current")).unwrap();

        let err = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1, n2],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::DEPLOYMENT_CONFLICT);
    }

    #[tokio::test]
    async fn missing_variant_rejected() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap(); // 空目录，无变体
        let err = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d".into(),
                plaza_model_id: card,
                hub_source: "ms".into(),
                node_ids: vec![n1],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::MODEL_NOT_FOUND);
    }

    /// 以给定 manifest JSON 登记一张卡片（用于 node-local / 顺序校验用例）
    async fn seed_card_json(
        db: &DatabaseConnection,
        id: &str,
        manifest: serde_json::Value,
    ) -> String {
        let now = time::now_iso();
        let model = manifest
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("org/w")
            .to_string();
        let am = crate::entities::plaza_model::ActiveModel {
            id: Set(format!("{id}@1.0.0")),
            model_id: Set(id.to_string()),
            model: Set(model),
            name: Set(id.to_string()),
            version: Set("1.0.0".into()),
            source: Set("plaza".into()),
            source_id: Set(String::new()),
            path: Set("/tmp/x".into()),
            package_path: Set("/tmp/x/model.tar.gz".into()),
            package_sha256: Set(String::new()),
            manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
            state: Set("available".into()),
            size_bytes: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        };
        am.insert(db).await.unwrap();
        format!("{id}@1.0.0")
    }

    #[tokio::test]
    async fn invalid_lifecycle_order_rejected() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card_json(
            &db,
            "com.ictrek.bad-order",
            serde_json::json!({
                "id": "com.ictrek.bad-order",
                "version": "1.0.0",
                "model": "org/w",
                "lifecycle-order": {"start": "random"},
            }),
        )
        .await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        let err = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d".into(),
                plaza_model_id: card,
                hub_source: "hf".into(),
                node_ids: vec![n1],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::LIFECYCLE_ORDER_INVALID);
    }

    /// 在临时 hub 目录中造出 `export/hf/org/w/current` 变体
    fn seed_variant(hub: &std::path::Path) {
        std::fs::create_dir_all(hub.join("export/hf/org/w/current")).unwrap();
    }

    #[tokio::test]
    async fn caps_max_nodes_limits_node_selection() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card_json(
            &db,
            "com.ictrek.single-node",
            serde_json::json!({
                "id": "com.ictrek.single-node",
                "version": "1.0.0",
                "model": "org/w",
                "caps": {"max-nodes": 1},
            }),
        )
        .await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let n2 = seed_node(&db, "10.0.0.2", "h2").await;
        let tmp = tempfile::TempDir::new().unwrap();
        seed_variant(tmp.path());

        // 超过上限 → 20727
        let err = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1.clone(), n2.clone()],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::CAPS_LIMIT_EXCEEDED);

        // 恰好 1 个节点 → 成功
        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d".into(),
                plaza_model_id: card,
                hub_source: "hf".into(),
                node_ids: vec![n1.clone()],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(created.node_ids.len(), 1);
        assert_eq!(created.head_node_id, n1);

        // 编辑为 2 个节点 → 20727，且原配置节点保持不变
        let err = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate {
                node_ids: Some(vec![n1.clone(), n2.clone()]),
                ..empty_update()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::CAPS_LIMIT_EXCEEDED);
        let after = get_view(&db, &created.id).await.unwrap();
        assert_eq!(after.node_ids, vec![n1.clone()]);
    }

    #[tokio::test]
    async fn cards_without_caps_are_unlimited() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card_json(
            &db,
            "com.ictrek.multi-node",
            serde_json::json!({
                "id": "com.ictrek.multi-node",
                "version": "1.0.0",
                "model": "org/w",
                "caps": {"max-nodes": 0},
            }),
        )
        .await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let n2 = seed_node(&db, "10.0.0.2", "h2").await;
        let n3 = seed_node(&db, "10.0.0.3", "h3").await;
        let tmp = tempfile::TempDir::new().unwrap();
        seed_variant(tmp.path());

        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "mp".into(),
                plaza_model_id: card,
                hub_source: "hf".into(),
                node_ids: vec![n1, n2, n3],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(created.node_ids.len(), 3);
        assert_eq!(created.nodes[1].role, "worker");
    }

    #[tokio::test]
    async fn invalid_caps_max_nodes_rejected() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card_json(
            &db,
            "com.ictrek.bad-caps",
            serde_json::json!({
                "id": "com.ictrek.bad-caps",
                "version": "1.0.0",
                "model": "org/w",
                "caps": {"max-nodes": -1},
            }),
        )
        .await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        seed_variant(tmp.path());
        let err = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d".into(),
                plaza_model_id: card,
                hub_source: "hf".into(),
                node_ids: vec![n1],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::MANIFEST_INVALID);
    }

    fn empty_update() -> DeploymentUpdate {
        DeploymentUpdate::default()
    }

    #[tokio::test]
    async fn update_changes_nodes_and_configs() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let n2 = seed_node(&db, "10.0.0.2", "h2").await;
        let n3 = seed_node(&db, "10.0.0.3", "h3").await;

        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/Qwen/Qwen3-0.6B/current")).unwrap();

        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d1".into(),
                plaza_model_id: card,
                hub_source: "hf".into(),
                node_ids: vec![n1],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap();

        // 仅改节点
        let updated = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate {
                node_ids: Some(vec![n2.clone(), n3.clone()]),
                ..empty_update()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.head_node_id, n2);
        assert_eq!(updated.node_ids, vec![n2.clone(), n3.clone()]);
        assert_eq!(updated.nodes[0].role, "head");

        // 仅改名称与参数（节点保持 n2/n3）
        let mut configs = Map::new();
        configs.insert("A".into(), serde_json::json!(9));
        let updated = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate {
                name: Some("d1-renamed".into()),
                configs: Some(configs),
                ..empty_update()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.name, "d1-renamed");
        assert_eq!(updated.configs.get("A"), Some(&serde_json::json!(9)));
        assert_eq!(updated.head_node_id, n2);
    }

    #[tokio::test]
    async fn update_rejected_when_active() {
        use crate::deploy::activities::{MarkStateReq, apply_state};

        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/Qwen/Qwen3-0.6B/current")).unwrap();

        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d1".into(),
                plaza_model_id: card,
                hub_source: "hf".into(),
                node_ids: vec![n1],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap();

        apply_state(
            &db,
            &MarkStateReq {
                deployment_id: created.id.clone(),
                state: Some("running".into()),
                desired_state: Some("running".into()),
                health_status: None,
                last_error: None,
            },
        )
        .await
        .unwrap();

        let err = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate {
                name: Some("x".into()),
                ..empty_update()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::DEPLOYMENT_STATE_INVALID);
        // 名称未被修改
        assert_eq!(get_deployment(&db, &created.id).await.unwrap().name, "d1");
    }

    #[tokio::test]
    async fn serve_params_inferred_persisted_and_kept_on_update() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await; // model = Qwen/Qwen3-0.6B
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/Qwen/Qwen3-0.6B/current")).unwrap();

        // 留空 → 自动推断并持久化
        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1.clone()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(created.serve_model_name, "Qwen3-0.6B");
        assert_eq!(created.serve_port, 8000);
        assert_eq!(created.serve_url_prefix, "/v1");
        let raw = get_deployment(&db, &created.id).await.unwrap();
        assert!(!raw.serve_api_key.is_empty(), "API Key 应被生成并持久化");

        // 重复加载取值稳定（不再每次随机）
        let again = load_runtime_serve(&db, &created.id).await;
        assert_eq!(again.0, raw.serve_api_key, "API Key 跨加载稳定");
        assert_eq!(again.1, "Qwen3-0.6B");

        // 更新：显式提供端口 / 前缀 / 模型名；API Key 未提供 → 沿用
        let updated = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate {
                serve_model_name: Some("custom-name".into()),
                serve_port: Some(9000),
                serve_url_prefix: Some("openai/v1".into()),
                ..empty_update()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.serve_model_name, "custom-name");
        assert_eq!(updated.serve_port, 9000);
        assert_eq!(updated.serve_url_prefix, "/openai/v1");
        let raw2 = get_deployment(&db, &created.id).await.unwrap();
        assert_eq!(raw2.serve_api_key, raw.serve_api_key, "更新未提供 API Key 时沿用现值");

        // 显式清空 API Key → 不启用鉴权
        let updated = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate {
                serve_api_key: Some(String::new()),
                ..empty_update()
            },
        )
        .await
        .unwrap();
        assert!(updated.serve_model_name.eq("custom-name"));
        assert!(get_deployment(&db, &created.id).await.unwrap().serve_api_key.is_empty());
    }

    /// 便捷：读取运行配置的 (api_key, model_name)
    async fn load_runtime_serve(db: &DatabaseConnection, id: &str) -> (String, String) {
        let m = get_deployment(db, id).await.unwrap();
        (m.serve_api_key, m.serve_model_name)
    }

    #[tokio::test]
    async fn local_proxy_config_persisted_and_updated() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/Qwen/Qwen3-0.6B/current")).unwrap();

        // 默认：关闭 + 自动端口
        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1.clone()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(!created.local_proxy_enabled);
        assert_eq!(created.local_proxy_port, 0);
        assert_eq!(created.local_proxy_active_port, 0);

        // 创建时开启 + 指定端口
        let created2 = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "d2".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1.clone()],
                local_proxy_enabled: true,
                local_proxy_port: Some(31050),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(created2.local_proxy_enabled);
        assert_eq!(created2.local_proxy_port, 31050);

        // 在线更新：开启 + 自动端口
        let updated = update_local_proxy(&db, &created.id, true, Some(0)).await.unwrap();
        assert!(updated.local_proxy_enabled);
        assert_eq!(updated.local_proxy_port, 0);

        // 非法端口归一化为自动
        let updated = update_local_proxy(&db, &created.id, true, Some(-5)).await.unwrap();
        assert_eq!(updated.local_proxy_port, 0);

        // 关闭（端口沿用）
        let updated = update_local_proxy(&db, &created.id, false, None).await.unwrap();
        assert!(!updated.local_proxy_enabled);
    }

    #[test]
    fn active_state_predicate() {
        for s in ["starting", "running", "stopping", "degraded"] {
            assert!(is_active_state(s), "{s} 应为活跃");
        }
        for s in ["stopped", "failed", ""] {
            assert!(!is_active_state(s), "{s} 应为非活跃");
        }
    }

    async fn set_state(db: &DatabaseConnection, id: &str, state: &str) {
        let m = get_deployment(db, id).await.unwrap();
        let mut am: deployment::ActiveModel = m.into();
        am.state = Set(state.to_string());
        am.update(db).await.unwrap();
    }

    #[tokio::test]
    async fn occupied_nodes_track_active_deployments() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let n2 = seed_node(&db, "10.0.0.2", "h2").await;
        let n3 = seed_node(&db, "10.0.0.3", "h3").await;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/Qwen/Qwen3-0.6B/current")).unwrap();
        let a = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "A".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1.clone(), n2.clone()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let b = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "B".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1.clone()],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // 全 stopped → 无承载项
        assert!(occupied_nodes(&db, "").await.unwrap().is_empty());

        // A running → 承载 n1/n2；B stopped → 不计入
        set_state(&db, &a.id, "running").await;
        let occ = occupied_nodes(&db, "").await.unwrap();
        assert_eq!(occ.len(), 2);
        assert!(occ.iter().all(|o| o.deployment_id == a.id && o.deployment_name == "A"));
        assert!(occupied_nodes(&db, &a.id).await.unwrap().is_empty(), "排除自身后无承载项");

        // B 也 running 且共享 n1 → n1 返回 A、B 两项，n2 返回 A 一项
        set_state(&db, &b.id, "running").await;
        let occ = occupied_nodes(&db, "").await.unwrap();
        assert_eq!(occ.len(), 3, "同一节点不被去重: {occ:?}");
        let n1_occ: Vec<_> = occ.iter().filter(|o| o.node_id == n1).collect();
        assert_eq!(n1_occ.len(), 2);
        assert!(n1_occ.iter().any(|o| o.deployment_id == a.id));
        assert!(n1_occ.iter().any(|o| o.deployment_id == b.id));
        assert_eq!(
            occupied_nodes(&db, &a.id).await.unwrap(),
            vec![OccupiedNode {
                node_id: n1.clone(),
                deployment_id: b.id.clone(),
                deployment_name: "B".into(),
            }]
        );

        // stopping 仍计入；stopped 释放
        set_state(&db, &a.id, "stopping").await;
        assert_eq!(occupied_nodes(&db, "").await.unwrap().len(), 3);
        set_state(&db, &a.id, "stopped").await;
        assert_eq!(occupied_nodes(&db, "").await.unwrap().len(), 1);
        set_state(&db, &b.id, "stopped").await;
        assert!(occupied_nodes(&db, "").await.unwrap().is_empty());

        // 同一运行配置内重复节点 id 只返回一项（防御异常历史数据）
        let m = get_deployment(&db, &b.id).await.unwrap();
        let mut am: deployment::ActiveModel = m.into();
        am.node_ids_json = Set(serde_json::to_string(&vec![n3.clone(), n3.clone()]).unwrap());
        am.state = Set("running".into());
        am.update(&db).await.unwrap();
        let occ = occupied_nodes(&db, "").await.unwrap();
        assert_eq!(occ.len(), 1);
        assert_eq!(occ[0].node_id, n3);
        assert_eq!(occ[0].deployment_id, b.id);
    }

    fn seed_hub_variant(hub: &Path, source: &str, model_id: &str) {
        std::fs::create_dir_all(hub.join("export").join(source).join(model_id).join("current")).unwrap();
    }

    fn weight_override(source: &str, model_id: &str) -> Option<WeightOverrideInput> {
        Some(WeightOverrideInput {
            source: source.into(),
            model_id: model_id.into(),
        })
    }

    #[tokio::test]
    async fn weight_override_effective_and_clear() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        seed_hub_variant(tmp.path(), "hf", "Qwen/Qwen3-0.6B");
        seed_hub_variant(tmp.path(), "ms", "Qwen/Qwen3-8B");

        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "ov".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1.clone()],
                weight_override: weight_override("ms", "Qwen/Qwen3-8B"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // 卡片默认列保持卡片声明
        assert_eq!(created.weight_id, "Qwen/Qwen3-0.6B");
        assert_eq!(created.hub_source, "hf");
        // 生效值来自覆盖
        assert!(created.weight_overridden);
        assert_eq!(created.effective_weight_id, "Qwen/Qwen3-8B");
        assert_eq!(created.effective_hub_source, "ms");
        assert_eq!(
            created.effective_hub_rel_path,
            "export/ms/Qwen/Qwen3-8B/current"
        );
        // 搜索文本包含生效权重
        assert!(deployment_search_text(&created).contains("qwen/qwen3-8b"));

        // 未提供覆盖字段 → 沿用（部分更新）
        let updated = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate { name: Some("ov2".into()), ..Default::default() },
        )
        .await
        .unwrap();
        assert!(updated.weight_overridden);
        assert_eq!(updated.effective_weight_id, "Qwen/Qwen3-8B");

        // 显式清空 → 回到卡片默认
        let cleared = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate {
                weight_override: weight_override("", ""),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(!cleared.weight_overridden);
        assert_eq!(cleared.effective_weight_id, "Qwen/Qwen3-0.6B");
        assert_eq!(cleared.effective_hub_source, "hf");
    }

    #[tokio::test]
    async fn weight_override_does_not_require_card_default_variant() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        // 卡片默认权重未下载，仅覆盖权重存在
        seed_hub_variant(tmp.path(), "hf", "org/only-override");

        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "only".into(),
                plaza_model_id: card.clone(),
                hub_source: String::new(),
                node_ids: vec![n1],
                weight_override: weight_override("hf", "org/only-override"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(created.effective_weight_id, "org/only-override");
        assert_eq!(created.hub_rel_path, "", "卡片默认未下载时相对路径留空");
    }

    #[tokio::test]
    async fn weight_override_validation_and_missing_variant() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        seed_hub_variant(tmp.path(), "hf", "Qwen/Qwen3-0.6B");
        seed_hub_variant(tmp.path(), "ms", "org/present");

        // 不成对
        let err = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "bad".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1.clone()],
                weight_override: weight_override("ms", ""),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::WEIGHT_OVERRIDE_INVALID);

        // 未下载
        let err = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "bad2".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1],
                weight_override: weight_override("hf", "org/missing"),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::MODEL_NOT_FOUND);
        assert!(err.msg.contains("权重覆盖"));
    }

    #[tokio::test]
    async fn images_override_and_extra_env_persisted() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        seed_hub_variant(tmp.path(), "hf", "Qwen/Qwen3-0.6B");

        let mut extra = Map::new();
        extra.insert("DEBUG_MODE".into(), serde_json::json!(1));
        let created = create_deployment(
            &db,
            tmp.path(),
            DeploymentCreate {
                name: "img".into(),
                plaza_model_id: card.clone(),
                hub_source: "hf".into(),
                node_ids: vec![n1],
                images_override: Some(vec![
                    serde_json::json!("repo/main:1"),
                    serde_json::json!({"name": "repo/arm-only:2", "arch": "arm64"}),
                ]),
                extra_env: Some(extra),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(created.images_source, "override");
        assert_eq!(created.images_override.len(), 2);
        assert_eq!(created.effective_images.len(), 2);
        assert_eq!(created.extra_env.get("DEBUG_MODE"), Some(&serde_json::json!("1")));

        // 部分更新沿用；清空镜像覆盖回到卡片声明（卡片未声明 images → 空）
        let updated = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate { name: Some("img2".into()), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(updated.images_source, "override");

        let cleared = update_deployment(
            &db,
            tmp.path(),
            &created.id,
            DeploymentUpdate {
                images_override: Some(Vec::new()),
                extra_env: Some(Map::new()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(cleared.images_source, "card");
        assert!(cleared.images_override.is_empty());
        assert!(cleared.extra_env.is_empty());
    }

    #[tokio::test]
    async fn images_and_extra_env_validation() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let card = seed_card(&db).await;
        let n1 = seed_node(&db, "10.0.0.1", "h1").await;
        let tmp = tempfile::TempDir::new().unwrap();
        seed_hub_variant(tmp.path(), "hf", "Qwen/Qwen3-0.6B");

        let base = |images: Option<Vec<Value>>, extra: Option<Map<String, Value>>| DeploymentCreate {
            name: "x".into(),
            plaza_model_id: card.clone(),
            hub_source: "hf".into(),
            node_ids: vec![n1.clone()],
            images_override: images,
            extra_env: extra,
            ..Default::default()
        };

        // 非法镜像
        let err = create_deployment(
            &db,
            tmp.path(),
            base(Some(vec![serde_json::json!("repo/x@sha256:abcd")]), None),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::IMAGE_OVERRIDE_INVALID);

        // 保留前缀
        let mut extra = Map::new();
        extra.insert("MODELINFER_SERVE_PORT".into(), serde_json::json!("9000"));
        let err = create_deployment(&db, tmp.path(), base(None, Some(extra)))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::EXTRA_ENV_INVALID);

        // 与卡片 configs 同名
        let mut extra = Map::new();
        extra.insert("A".into(), serde_json::json!(2));
        let err = create_deployment(&db, tmp.path(), base(None, Some(extra)))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::EXTRA_ENV_INVALID);
    }
}
