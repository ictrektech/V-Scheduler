//! 部署编排：Temporal 工作流 + Activities 的共享类型与运行上下文
//!
//! 设计要点：**Workflow 只持有 `deployment_id`**，所有上下文（节点凭据、模型路径、生命周期
//! 脚本）在各 Activity 内实时加载，避免将 SSH 密码等敏感信息写入 Temporal history。

pub mod activities;
pub mod autorestart;
pub mod control;
pub mod health;
pub mod runtime;
pub mod script;
pub mod temporal_query;
pub mod workflows;

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::deployments;
use crate::error::ModelinferError;
use crate::images::ImageHubConfig;
use crate::model_hub;
use crate::nfs::{self, NfsResolveResult, ResolveInput};
use crate::nodes;
use crate::observability::SettingsDefaults;
use crate::platform::PlatformClient;
use crate::plaza;
use crate::ssh::SshExec;
use crate::vrouter::{VrouterClient, VrouterConfig};

/// 启动阶段顺序
///
/// `wait-ready` 位于 `start` 之后：`start` 只负责启动（非阻塞、立即返回），
/// `wait-ready` 负责等待服务就绪；卡片未声明该阶段时视为已就绪。
pub const STAGES_START: [&str; 5] = ["init-env", "fetch-data", "pre-start", "start", "wait-ready"];
/// 停止阶段顺序
pub const STAGES_STOP: [&str; 4] = ["pre-stop", "stop", "cleanup-data", "teardown-env"];
/// 健康巡检阶段
pub const HEALTH_STAGE: &str = "health-probe";
/// 全量阶段（用于分发脚本）
pub const ALL_STAGES: [&str; 10] = [
    "init-env",
    "fetch-data",
    "pre-start",
    "start",
    "wait-ready",
    "health-probe",
    "pre-stop",
    "stop",
    "cleanup-data",
    "teardown-env",
];

/// 默认服务端口
pub const DEFAULT_SERVE_PORT: u16 = 8000;
/// 生命周期阶段默认超时（秒）
pub const DEFAULT_STAGE_TIMEOUT_SECS: u64 = 1800;
/// 生命周期阶段最大超时（秒）
pub const MAX_STAGE_TIMEOUT_SECS: u64 = 86400;
/// 默认阶段节点顺序
pub const DEFAULT_STAGE_ORDER: &str = "head-first";

/// 解析阶段节点顺序（非法值回落 `head-first`，防御式，永不 panic）
pub fn normalize_stage_order(raw: &str) -> String {
    let v = raw.trim();
    if crate::deployments::validate::LIFECYCLE_ORDERS.contains(&v) {
        v.to_string()
    } else {
        DEFAULT_STAGE_ORDER.to_string()
    }
}

/// 解析阶段超时（非法回落默认，clamp 到上限）
pub fn normalize_stage_timeout(raw: Option<&serde_json::Value>) -> u64 {
    let secs = match raw {
        Some(serde_json::Value::Number(n)) => n.as_u64(),
        Some(serde_json::Value::String(s)) => s.trim().parse::<u64>().ok(),
        _ => None,
    };
    match secs {
        Some(v) if v >= 1 => v.min(MAX_STAGE_TIMEOUT_SECS),
        _ => DEFAULT_STAGE_TIMEOUT_SECS,
    }
}

/// 按顺序模式重排节点（首元素为 head）
///
/// - `head-first`（缺省）：head、其余按配置顺序；
/// - `worker-first`：非 head 按配置顺序（rank 升序），head 最后；
/// - `worker-first-reverse`：非 head 逆序（rank 降序），head 最后。
pub fn node_order_for<'a>(nodes: &[&'a DeployNode], order: &str) -> Vec<&'a DeployNode> {
    let mut out: Vec<&'a DeployNode> = Vec::with_capacity(nodes.len());
    let Some((head, workers)) = nodes.split_first() else {
        return out;
    };
    match order {
        "worker-first" => {
            out.extend(workers.iter().copied());
            out.push(*head);
        }
        "worker-first-reverse" => {
            out.extend(workers.iter().rev().copied());
            out.push(*head);
        }
        _ => out.extend(nodes.iter().copied()),
    }
    out
}

/// 供 Activities 使用的外部依赖
#[derive(Clone)]
pub struct DeployDeps {
    pub db: DatabaseConnection,
    pub ssh: Arc<dyn SshExec>,
    pub platform: Arc<dyn PlatformClient>,
    pub hub_dir: String,
    pub model_hub_path: String,
    pub mount_dir: String,
    pub nfs_server: String,
    /// image-hub registry 配置（启动前缺失镜像分发）
    pub image_hub: ImageHubConfig,
    /// 运行可观测性保留策略默认值（run 历史裁剪）
    pub settings_defaults: SettingsDefaults,
    /// 统一网关 HTTP 客户端（启动后注册 / 停止注销）
    pub vrouter_client: Arc<dyn VrouterClient>,
    /// 统一网关集成配置
    pub vrouter_config: VrouterConfig,
    /// 本地反代注册表（启动后监听 / 停止关闭 / 巡检 reconcile）
    pub local_proxy: crate::localproxy::ProxyRegistry,
    /// 节点监控集成配置（consul / prometheus / node-exporter 镜像 / reconcile 周期）
    pub monitor: crate::monitor::MonitorConfig,
}

/// 参与部署的节点（含凭据，仅在活动内部使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployNode {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// 认证方式（`password` / `key`）
    pub auth_method: String,
    pub password: String,
    /// SSH 私钥（`auth_method=key` 时）
    pub ssh_key: String,
    #[serde(default)]
    pub sudo_password: String,
    pub hostname: String,
    pub socket_ifname: String,
    pub ib_hca: String,
    /// CPU 架构（`amd64` / `arm64` 等；用于按架构过滤生效镜像）
    #[serde(default)]
    pub arch: String,
    /// 挂载 model-hub NFS 时是否启用 fscache（节点级，随节点台账）
    pub fscache: bool,
    pub role: String,
}

impl DeployNode {
    pub fn from_model(n: &crate::entities::node::Model, role: &str) -> Self {
        Self {
            id: n.id.clone(),
            name: n.name.clone(),
            host: n.host.clone(),
            port: n.ssh_port as u16,
            user: n.ssh_user.clone(),
            auth_method: n.auth_method.clone(),
            password: n.ssh_password.clone(),
            ssh_key: n.ssh_key.clone(),
            sudo_password: n.sudo_password.clone(),
            hostname: n.hostname.clone(),
            socket_ifname: n.socket_ifname.clone(),
            ib_hca: n.ib_hca.clone(),
            arch: n.arch.clone(),
            fscache: n.fscache,
            role: role.into(),
        }
    }
}

/// 部署运行上下文
#[derive(Debug, Clone)]
pub struct DeployRuntime {
    pub deployment_id: String,
    pub name: String,
    pub nodes: Vec<DeployNode>,
    pub head_ip: String,
    pub nodes_map: String,
    pub card_id: String,
    /// 生效权重 id（运行配置覆盖优先，否则卡片 `manifest.model`）
    pub weight_id: String,
    /// 卡片 `manifest.model` 声明的权重 id（供卡片判断是否被覆盖）
    pub card_weight_id: String,
    /// 是否覆盖权重
    pub weight_overridden: bool,
    /// 生效权重的来源（hf / ms）
    pub weight_source: String,
    pub model_path: String,
    /// 节点侧 model-hub 根路径（NFS 挂载目录 + 存储空间子路径），供 ComfyUI 等定位共享目录
    pub model_hub_root: String,
    pub serve_port: u16,
    pub serve_api_key: String,
    pub serve_model_name: String,
    pub serve_url_prefix: String,
    pub configs: Map<String, Value>,
    /// 额外环境变量（在卡片 `configs` 之后注入）
    pub extra_env: Map<String, Value>,
    pub lifecycle: Map<String, Value>,
    /// 阶段节点顺序声明（阶段 → 顺序字面值）
    pub lifecycle_order: Map<String, Value>,
    /// 阶段超时声明（阶段 → 秒）
    pub lifecycle_timeout: Map<String, Value>,
    pub workdir: String,
    pub mount_dir: String,
    pub nfs: NfsResolveResult,
    pub package_path: String,
    /// 生效镜像（运行配置覆盖优先，否则卡片 `manifest.images`）
    ///
    /// 启动前预检 / 分发（`crate::images::ensure_images`）与生命周期环境变量注入
    /// （`MODELINFER_IMAGES*`）MUST 使用同一份列表。
    pub manifest_images: Vec<Value>,
    /// 生效镜像来源：`card` / `override`
    pub images_source: String,
}

impl DeployRuntime {
    pub fn node(&self, id: &str) -> Option<&DeployNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn head(&self) -> Option<&DeployNode> {
        self.nodes.first()
    }
    /// head 优先，其余节点保持顺序
    pub fn ordered_nodes(&self) -> Vec<&DeployNode> {
        let mut v: Vec<&DeployNode> = Vec::with_capacity(self.nodes.len());
        if let Some(h) = self.head() {
            v.push(h);
        }
        for n in self.nodes.iter().skip(1) {
            v.push(n);
        }
        v
    }
    /// 阶段节点顺序（缺省 `head-first`）
    pub fn stage_order(&self, stage: &str) -> String {
        let raw = self
            .lifecycle_order
            .get(stage)
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_STAGE_ORDER);
        normalize_stage_order(raw)
    }
    /// 阶段超时（秒）
    pub fn stage_timeout_secs(&self, stage: &str) -> u64 {
        normalize_stage_timeout(self.lifecycle_timeout.get(stage))
    }
    /// 某阶段实际执行的节点顺序
    pub fn ordered_stage_nodes(&self, stage: &str) -> Vec<&DeployNode> {
        let order = self.stage_order(stage);
        let refs: Vec<&DeployNode> = self.nodes.iter().collect();
        node_order_for(&refs, &order)
    }
    /// 节点序号（head=0，与部署配置节点顺序一致）
    pub fn rank_of(&self, node_id: &str) -> usize {
        self.nodes.iter().position(|n| n.id == node_id).unwrap_or(0)
    }
    /// 生成节点上的阶段脚本路径
    pub fn stage_script(&self, stage: &str) -> String {
        format!("{}/.modelinfer/stage_{}.sh", self.workdir, stage)
    }
    pub fn env_script(&self) -> String {
        format!("{}/.modelinfer/env.sh", self.workdir)
    }
    pub fn remote_package(&self) -> String {
        format!("{}/model.tar.gz", self.workdir)
    }
}

/// 部署计划（不含敏感信息，进入 workflow history）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployPlan {
    pub deployment_id: String,
    /// 启动运行 ID（由 `prepare` activity 创建，供后续阶段记录日志 / 数据）
    #[serde(default)]
    pub run_id: String,
    pub node_ids: Vec<String>,
    pub head_id: String,
    pub stages_start: Vec<String>,
    pub stages_stop: Vec<String>,
    pub has_health_probe: bool,
    /// 各阶段节点顺序（`head-first` / `worker-first` / `worker-first-reverse`）
    #[serde(default)]
    pub stage_order: Map<String, Value>,
    /// 各阶段超时（秒）
    #[serde(default)]
    pub stage_timeouts: Map<String, Value>,
}

impl DeployPlan {
    /// 某阶段节点顺序（缺省 `head-first`）
    pub fn stage_order_of(&self, stage: &str) -> String {
        normalize_stage_order(
            self.stage_order.get(stage).and_then(Value::as_str).unwrap_or(DEFAULT_STAGE_ORDER),
        )
    }
    /// 某阶段超时（秒）
    pub fn stage_timeout_of(&self, stage: &str) -> u64 {
        normalize_stage_timeout(self.stage_timeouts.get(stage))
    }
}

/// 加载运行时上下文（DB + 平台 NFS 解析 + model-hub 变体）
pub async fn load_runtime(
    deps: &DeployDeps,
    deployment_id: &str,
) -> Result<DeployRuntime, ModelinferError> {
    let d = deployments::get_deployment(&deps.db, deployment_id).await?;
    let card = plaza::get_model(&deps.db, &d.plaza_model_id).await?;
    let node_ids: Vec<String> = serde_json::from_str(&d.node_ids_json).unwrap_or_default();
    let models = nodes::load_models(&deps.db, Some(&node_ids)).await?;
    if models.is_empty() {
        return Err(ModelinferError::new(
            crate::error::codes::PARAM_INVALID,
            "部署配置未关联任何节点",
        ));
    }
    let mut dep_nodes = Vec::with_capacity(models.len());
    for (idx, n) in models.iter().enumerate() {
        dep_nodes.push(DeployNode::from_model(n, if idx == 0 { "head" } else { "worker" }));
    }
    let configs: Map<String, Value> = serde_json::from_str(&d.configs_json).unwrap_or_default();
    let extra_env: Map<String, Value> = serde_json::from_str(&d.extra_env_json).unwrap_or_default();
    let manifest: Value = serde_json::from_str(&card.manifest_json).unwrap_or(Value::Null);
    // 生效镜像：运行配置覆盖（非空，整体替换）优先，否则卡片 `manifest.images`
    let images_override: Vec<Value> =
        serde_json::from_str(&d.images_override_json).unwrap_or_default();
    let card_images = crate::images::manifest_images(&manifest).unwrap_or_default();
    let manifest_images = crate::images::effective_images(&card_images, &images_override);
    let images_source = crate::images::images_source(&images_override).to_string();
    let lifecycle = manifest
        .get("lifecycle")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let lifecycle_order = manifest
        .get("lifecycle-order")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let lifecycle_timeout = manifest
        .get("lifecycle-timeout")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    // 权重 MUST 来自 model-hub：解析存储空间 → NFS 挂载 → 定位权重目录
    let hosts: Vec<String> = dep_nodes.iter().map(|n| n.host.clone()).collect();
    let nfs = nfs::resolve_nfs(
        deps.platform.as_ref(),
        &ResolveInput {
            config_name: nfs::MODEL_HUB_CONFIG,
            fallback_hub_path: &deps.model_hub_path,
            nfs_server_override: &deps.nfs_server,
            node_hosts: &hosts,
        },
    )
    .await?;

    // 生效权重：覆盖优先（启动时重新解析，防御 model-hub 目录被删除）
    let card_weight_id = card.model.trim().to_string();
    let over_source = d.weight_override_source.trim();
    let over_id = d.weight_override_id.trim();
    let weight_overridden = !over_id.is_empty();
    let (effective_weight_id, effective_weight_source, effective_rel_path) = if weight_overridden {
        let variants = model_hub::find_variants(std::path::Path::new(&deps.hub_dir), over_id);
        let variant = variants.iter().find(|v| v.source == over_source).ok_or_else(|| {
            ModelinferError::new(
                crate::error::codes::MODEL_NOT_FOUND,
                format!("权重覆盖在 model-hub 中不存在: {over_source}:{over_id}"),
            )
        })?;
        (
            variant.model_id.clone(),
            variant.source.clone(),
            variant.rel_path.clone(),
        )
    } else {
        (
            d.weight_id.clone(),
            d.hub_source.clone(),
            d.hub_rel_path.clone(),
        )
    };
    let model_path = if effective_rel_path.trim().is_empty() {
        // 无权重卡片（如 ComfyUI）：不注入权重路径，仅暴露 model-hub 根供共享目录使用
        String::new()
    } else {
        model_hub::node_model_path(&deps.mount_dir, &nfs.sub_path, &effective_rel_path)
    };
    let model_hub_root = model_hub::node_model_path(&deps.mount_dir, &nfs.sub_path, "");

    let head_ip = dep_nodes.first().map(|n| n.host.clone()).unwrap_or_default();
    let nodes_map = dep_nodes
        .iter()
        .map(|n| format!("{}:{}", if n.hostname.is_empty() { &n.host } else { &n.hostname }, n.host))
        .collect::<Vec<_>>()
        .join(" ");
    // 标准接入参数：来自运行配置的持久化字段（留空按推断规则回落，保证跨阶段稳定）
    let serve_model_name = if d.serve_model_name.trim().is_empty() {
        crate::access::default_model_name(&effective_weight_id, &d.card_id)
    } else {
        d.serve_model_name.clone()
    };
    let serve_port = crate::access::normalize_serve_port(Some(d.serve_port.max(0) as u64));
    let serve_url_prefix = crate::access::normalize_url_prefix(&d.serve_url_prefix);
    let serve_api_key = d.serve_api_key.clone();

    Ok(DeployRuntime {
        deployment_id: d.id.clone(),
        name: d.name.clone(),
        nodes: dep_nodes,
        head_ip,
        nodes_map,
        card_id: d.card_id.clone(),
        weight_id: effective_weight_id,
        card_weight_id,
        weight_overridden,
        weight_source: effective_weight_source,
        model_path,
        model_hub_root,
        serve_port,
        serve_api_key,
        serve_model_name,
        serve_url_prefix,
        configs,
        extra_env,
        lifecycle,
        lifecycle_order,
        lifecycle_timeout,
        workdir: deployment_workdir(&d.id),
        mount_dir: deps.mount_dir.clone(),
        nfs,
        package_path: card.package_path,
        manifest_images,
        images_source,
    })
}

/// 节点工作目录
pub fn deployment_workdir(id: &str) -> String {
    format!("/opt/modelinfer/deployments/{id}")
}

/// 从生命周期定义中提取存在的阶段（保持固定顺序）
pub fn present_stages(rt: &DeployRuntime, order: &[&str]) -> Vec<String> {
    order
        .iter()
        .filter(|s| rt.lifecycle.contains_key(**s))
        .map(|s| s.to_string())
        .collect()
}

/// 由 runtime 构造部署计划
pub fn plan_of(rt: &DeployRuntime, run_id: &str) -> DeployPlan {
    // 仅为实际声明的阶段记录顺序 / 超时，避免 plan 冗余
    let declared: Vec<String> = ALL_STAGES
        .iter()
        .filter(|s| rt.lifecycle.contains_key(**s))
        .map(|s| s.to_string())
        .collect();
    let mut stage_order = Map::new();
    let mut stage_timeouts = Map::new();
    for stage in declared {
        let order = rt.stage_order(&stage);
        if order != DEFAULT_STAGE_ORDER
            || rt.lifecycle_order.contains_key(stage.as_str())
        {
            stage_order.insert(stage.clone(), Value::String(order));
        }
        let secs = rt.stage_timeout_secs(&stage);
        if secs != DEFAULT_STAGE_TIMEOUT_SECS || rt.lifecycle_timeout.contains_key(stage.as_str()) {
            stage_timeouts.insert(stage.clone(), Value::Number(secs.into()));
        }
    }
    DeployPlan {
        deployment_id: rt.deployment_id.clone(),
        run_id: run_id.to_string(),
        node_ids: rt.nodes.iter().map(|n| n.id.clone()).collect(),
        head_id: rt.nodes.first().map(|n| n.id.clone()).unwrap_or_default(),
        stages_start: present_stages(rt, &STAGES_START),
        stages_stop: present_stages(rt, &STAGES_STOP),
        has_health_probe: rt.lifecycle.contains_key(HEALTH_STAGE),
        stage_order,
        stage_timeouts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> DeployNode {
        DeployNode {
            id: id.into(),
            name: id.into(),
            host: format!("10.0.0.{}", id),
            port: 22,
            user: "root".into(),
            auth_method: "password".into(),
            password: "x".into(),
            ssh_key: String::new(),
            sudo_password: String::new(),
            hostname: id.into(),
            socket_ifname: "eth0".into(),
            ib_hca: String::new(),
            arch: "amd64".into(),
            fscache: false,
            role: "worker".into(),
        }
    }

    fn runtime(nodes: Vec<DeployNode>, lifecycle: Map<String, Value>) -> DeployRuntime {
        DeployRuntime {
            deployment_id: "d".into(),
            name: "n".into(),
            nodes,
            head_ip: String::new(),
            nodes_map: String::new(),
            card_id: String::new(),
            weight_id: String::new(),
            card_weight_id: String::new(),
            weight_overridden: false,
            weight_source: "hf".into(),
            model_path: String::new(),
            model_hub_root: String::new(),
            serve_port: 8000,
            serve_api_key: String::new(),
            serve_model_name: String::new(),
            serve_url_prefix: "/v1".into(),
            configs: Map::new(),
            extra_env: Map::new(),
            lifecycle,
            lifecycle_order: Map::new(),
            lifecycle_timeout: Map::new(),
            workdir: "/opt/modelinfer/deployments/d".into(),
            mount_dir: "/mnt/model_hub".into(),
            nfs: NfsResolveResult::not_mounted(),
            package_path: String::new(),
            manifest_images: Vec::new(),
            images_source: "card".into(),
        }
    }

    #[test]
    fn workdir_and_stage_paths() {
        assert_eq!(deployment_workdir("abc"), "/opt/modelinfer/deployments/abc");
    }

    #[test]
    fn present_stages_filters_and_orders() {
        let mut lifecycle = Map::new();
        lifecycle.insert("start".into(), Value::String("x".into()));
        lifecycle.insert("health-probe".into(), Value::String("x".into()));
        lifecycle.insert("stop".into(), Value::String("x".into()));
        let rt = runtime(vec![], lifecycle);
        assert_eq!(present_stages(&rt, &STAGES_START), vec!["start"]);
        assert_eq!(present_stages(&rt, &STAGES_STOP), vec!["stop"]);
        assert!(plan_of(&rt, "run-1").has_health_probe);
    }

    #[test]
    fn wait_ready_stage_in_start_sequence() {
        let mut lifecycle = Map::new();
        lifecycle.insert("start".into(), Value::String("x".into()));
        lifecycle.insert("wait-ready".into(), Value::String("y".into()));
        let rt = runtime(vec![node("0")], lifecycle);
        // wait-ready 位于 start 之后
        assert_eq!(present_stages(&rt, &STAGES_START), vec!["start", "wait-ready"]);
        assert!(ALL_STAGES.contains(&"wait-ready"));
        // 分发脚本路径可生成
        assert!(rt.stage_script("wait-ready").contains("stage_wait-ready.sh"));
        assert_eq!(plan_of(&rt, "run-1").stages_start, vec!["start", "wait-ready"]);
    }

    #[test]
    fn node_order_modes() {
        let a = node("0");
        let b = node("1");
        let c = node("2");
        let refs = vec![&a, &b, &c];
        let ids = |v: Vec<&DeployNode>| v.iter().map(|n| n.id.clone()).collect::<Vec<_>>();

        assert_eq!(ids(node_order_for(&refs, "head-first")), vec!["0", "1", "2"]);
        assert_eq!(ids(node_order_for(&refs, "worker-first")), vec!["1", "2", "0"]);
        assert_eq!(ids(node_order_for(&refs, "worker-first-reverse")), vec!["2", "1", "0"]);
        // 未知值回落 head-first
        assert_eq!(ids(node_order_for(&refs, "whatever")), vec!["0", "1", "2"]);
        // 单节点
        let only = vec![&a];
        assert_eq!(ids(node_order_for(&only, "worker-first-reverse")), vec!["0"]);
        // 空
        assert!(node_order_for(&[], "worker-first").is_empty());
    }

    #[test]
    fn stage_order_and_timeout_normalize() {
        assert_eq!(normalize_stage_order("worker-first-reverse"), "worker-first-reverse");
        assert_eq!(normalize_stage_order("bogus"), DEFAULT_STAGE_ORDER);
        assert_eq!(normalize_stage_timeout(None), DEFAULT_STAGE_TIMEOUT_SECS);
        assert_eq!(normalize_stage_timeout(Some(&Value::Number(0.into()))), DEFAULT_STAGE_TIMEOUT_SECS);
        assert_eq!(normalize_stage_timeout(Some(&Value::String("-5".into()))), DEFAULT_STAGE_TIMEOUT_SECS);
        assert_eq!(normalize_stage_timeout(Some(&Value::Number(3600.into()))), 3600);
        assert_eq!(
            normalize_stage_timeout(Some(&Value::Number(999_999.into()))),
            MAX_STAGE_TIMEOUT_SECS
        );
    }

    #[test]
    fn plan_carries_order_timeout_and_ranks() {
        let mut lifecycle = Map::new();
        lifecycle.insert("start".into(), Value::String("x".into()));
        let mut rt = runtime(vec![node("0"), node("1"), node("2")], lifecycle);
        rt.lifecycle_order.insert("start".into(), Value::String("worker-first-reverse".into()));
        rt.lifecycle_timeout.insert("start".into(), Value::Number(3600.into()));

        assert_eq!(rt.stage_order("start"), "worker-first-reverse");
        assert_eq!(rt.stage_timeout_secs("start"), 3600);
        let order: Vec<String> = rt.ordered_stage_nodes("start").iter().map(|n| n.id.clone()).collect();
        assert_eq!(order, vec!["2", "1", "0"]);
        assert_eq!(rt.rank_of("0"), 0);
        assert_eq!(rt.rank_of("2"), 2);
        // 未声明阶段回落默认
        assert_eq!(rt.stage_order("stop"), DEFAULT_STAGE_ORDER);
        assert_eq!(rt.stage_timeout_secs("stop"), DEFAULT_STAGE_TIMEOUT_SECS);

        let plan = plan_of(&rt, "run-1");
        assert_eq!(plan.stage_order_of("start"), "worker-first-reverse");
        assert_eq!(plan.stage_timeout_of("start"), 3600);
        assert_eq!(plan.stage_order_of("stop"), DEFAULT_STAGE_ORDER);
        assert_eq!(plan.stage_timeout_of("stop"), DEFAULT_STAGE_TIMEOUT_SECS);
    }
}