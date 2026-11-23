//! 节点监控组件（node-exporter / 厂商 GPU exporter）部署、consul 注册与 reconcile。
//!
//! 设计见 `openspec/changes/modelinfer-node-gpu-monitoring/design.md`：
//! - 监控组件按 GPU 厂商适配（`applicable_components`），每组件独立部署 / 注册 / 状态；
//! - 镜像仅来自节点本地或 image-hub（**绝不** `docker pull`）；
//! - 部署成功注册到平台 consul，由平台 prometheus 经 `consul_sd` 采集（job 即服务名）；
//! - reconcile 只对齐注册信息（补注册 / 清残留）；
//! - 核心里程碑为纯函数，SSH / consul / prometheus 经 trait 注入，单测无需真实环境。

pub mod consul;
pub mod prometheus;
pub mod store;
pub mod workflows;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::entities::node;
use crate::error::{ModelinferError, codes};
use crate::ssh::{SshExec, SshTarget};

/// 默认 node-exporter 镜像（节点无外网，缺失时经 image-hub 分发）
pub const DEFAULT_NODE_EXPORTER_IMAGE: &str = "docker.io/prom/node-exporter:v1.12.0";
/// 默认 NVIDIA GPU exporter 镜像（节点无外网，缺失时经 image-hub 分发）
pub const DEFAULT_NVIDIA_GPU_EXPORTER_IMAGE: &str = "docker.io/utkuozdemir/nvidia_gpu_exporter:1.15.1";
/// node-exporter 容器名
pub const NODE_EXPORTER_CONTAINER: &str = "modelinfer-node-exporter";
/// nvidia-gpu-exporter 容器名
pub const NVIDIA_GPU_EXPORTER_CONTAINER: &str = "modelinfer-nvidia-gpu-exporter";
/// 默认 node-exporter 监听端口
///
/// 刻意避开上游标准端口 `9100`：算力节点常已运行自带的 node-exporter / 监控栈，
/// 组件以 `--net=host` 绑定端口时会发生冲突导致部署失败。节点级可在节点
/// 「高级配置」中覆盖。
pub const DEFAULT_NODE_EXPORTER_PORT: i32 = 19100;
/// 默认 nvidia-gpu-exporter 监听端口
///
/// 同样避开上游默认端口 `9835`，节点级可覆盖。
pub const DEFAULT_NVIDIA_GPU_EXPORTER_PORT: i32 = 19835;
/// 默认 reconcile 周期（秒）
pub const DEFAULT_RECONCILE_SECS: u64 = 60;
/// 监控部署步骤 SSH 命令超时（秒）
const MONITOR_CMD_TIMEOUT_SECS: u64 = 300;
/// 指标内容校验的抓取超时（秒，见 D-MI-MH-02）
const CONTENT_CHECK_TIMEOUT_SECS: u64 = 6;

/// 组件指标内容标记（`/metrics` 中必须出现的子串）
///
/// - 系统组件：存在 `node_*` 指标（node-exporter 指标前缀）；
/// - GPU 组件：`nvidia_smi_command_exit_code 0`（最近一次 `nvidia-smi` 成功，见 D-MI-MH-02）。
pub fn content_marker(kind: ComponentKind) -> &'static str {
    match kind {
        ComponentKind::NodeExporter => "^node_",
        ComponentKind::NvidiaGpuExporter => "nvidia_smi_command_exit_code 0",
    }
}

/// 监控组件种类（组件注册表）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComponentKind {
    /// 系统指标（node-exporter）
    NodeExporter,
    /// NVIDIA GPU 指标（nvidia_gpu_exporter）
    NvidiaGpuExporter,
}

/// 组件种类字面值常量（consul Meta / 状态表 `component`）
pub mod component {
    pub const NODE_EXPORTER: &str = "node-exporter";
    pub const NVIDIA_GPU_EXPORTER: &str = "nvidia-gpu-exporter";
}

impl ComponentKind {
    /// 组件标识字面值（consul Meta / 状态表 `component`）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NodeExporter => component::NODE_EXPORTER,
            Self::NvidiaGpuExporter => component::NVIDIA_GPU_EXPORTER,
        }
    }

    /// 从字面值解析（未知返回 None）
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            component::NODE_EXPORTER => Some(Self::NodeExporter),
            component::NVIDIA_GPU_EXPORTER => Some(Self::NvidiaGpuExporter),
            _ => None,
        }
    }

    /// consul 服务名（= prometheus job）
    pub fn service_name(&self) -> &'static str {
        match self {
            Self::NodeExporter => "modelinfer-node-exporter",
            Self::NvidiaGpuExporter => "modelinfer-nvidia-gpu-exporter",
        }
    }

    /// consul 服务 ID 前缀
    pub fn id_prefix(&self) -> &'static str {
        match self {
            Self::NodeExporter => "modelinfer-node-exporter-",
            Self::NvidiaGpuExporter => "modelinfer-nvidia-gpu-exporter-",
        }
    }

    /// 容器名
    pub fn container(&self) -> &'static str {
        match self {
            Self::NodeExporter => NODE_EXPORTER_CONTAINER,
            Self::NvidiaGpuExporter => NVIDIA_GPU_EXPORTER_CONTAINER,
        }
    }

    /// 指标分类（指标组 `category`）
    pub fn category(&self) -> &'static str {
        match self {
            Self::NodeExporter => "system",
            Self::NvidiaGpuExporter => "gpu",
        }
    }

    /// 是否需要 GPU 容器运行时
    pub fn gpu(&self) -> bool {
        matches!(self, Self::NvidiaGpuExporter)
    }
}

/// 由节点 `gpu_vendor` 派生适用的监控组件集合（纯函数）
///
/// 恒含系统组件 node-exporter；`nvidia` 时追加 GPU 组件 nvidia-gpu-exporter。
/// 其它厂商（amd / intel）本期不追加，预留扩展点。
pub fn applicable_components(gpu_vendor: &str) -> Vec<ComponentKind> {
    let mut v = vec![ComponentKind::NodeExporter];
    if gpu_vendor.trim().eq_ignore_ascii_case("nvidia") {
        v.push(ComponentKind::NvidiaGpuExporter);
    }
    v
}

/// 依据 `docker info` 的 runtimes JSON 选择 GPU 运行参数（纯函数）
///
/// 含 `nvidia` runtime → `--runtime=nvidia`；否则回退 `--gpus all`。
pub fn gpu_runtime_flag(runtimes: &str) -> &'static str {
    if runtimes.to_ascii_lowercase().contains("nvidia") {
        "--runtime=nvidia"
    } else {
        "--gpus all"
    }
}

/// 监控集成配置（consul / prometheus / 镜像 / reconcile 周期）
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub consul_url: String,
    pub prometheus_url: String,
    /// node-exporter 镜像
    pub node_exporter_image: String,
    /// nvidia-gpu-exporter 镜像
    pub nvidia_gpu_exporter_image: String,
    pub reconcile_secs: u64,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            consul_url: "http://172.17.0.1:8500".to_string(),
            prometheus_url: "http://172.17.0.1:9090".to_string(),
            node_exporter_image: DEFAULT_NODE_EXPORTER_IMAGE.to_string(),
            nvidia_gpu_exporter_image: DEFAULT_NVIDIA_GPU_EXPORTER_IMAGE.to_string(),
            reconcile_secs: DEFAULT_RECONCILE_SECS,
        }
    }
}

impl MonitorConfig {
    pub fn consul(&self) -> consul::ConsulConfig {
        consul::ConsulConfig::new(self.consul_url.clone())
    }
    pub fn prometheus(&self) -> prometheus::PrometheusConfig {
        prometheus::PrometheusConfig::new(self.prometheus_url.clone())
    }
}

/// 监控组件在节点上的工作目录
pub fn monitor_workdir(node_id: &str) -> String {
    format!("/opt/modelinfer/monitor/{node_id}")
}

/// 监控组件（容器规格 + consul 服务名 + 指标分类）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorComponent {
    /// 组件种类
    pub kind: ComponentKind,
    /// consul 服务名（= prometheus job）
    pub name: String,
    /// 容器名
    pub container: String,
    /// 镜像
    pub image: String,
    /// 监听端口
    pub port: u16,
    /// 指标分类（`system` / `gpu`）
    pub category: String,
    /// 是否需要 GPU 容器运行时
    pub gpu: bool,
}

impl MonitorComponent {
    /// 按组件种类构造（镜像 / 端口由调用方给出）
    pub fn of(kind: ComponentKind, image: &str, port: u16) -> Self {
        Self {
            kind,
            name: kind.service_name().to_string(),
            container: kind.container().to_string(),
            image: image.to_string(),
            port,
            category: kind.category().to_string(),
            gpu: kind.gpu(),
        }
    }

    /// node-exporter 组件
    pub fn node_exporter(image: &str, port: u16) -> Self {
        Self::of(ComponentKind::NodeExporter, image, port)
    }

    /// nvidia-gpu-exporter 组件
    pub fn nvidia_gpu_exporter(image: &str, port: u16) -> Self {
        Self::of(ComponentKind::NvidiaGpuExporter, image, port)
    }

    /// 移除旧容器（忽略不存在）
    pub fn remove_cmd(&self) -> String {
        format!("docker rm -f {} >/dev/null 2>&1 || true", crate::deploy::script::sh_quote(&self.container))
    }

    /// 启动容器
    ///
    /// - 系统组件：host 网络 / host pid / 宿主 rootfs 只读挂载；**禁用 cpufreq 采集器**
    ///   （GB10 Grace CPU 上 `cpuinfo_cur_freq` 读取阻塞会使抓取挂起、耗尽 node-exporter
    ///   并发上限后返回 503，见 D-MI-MH-01）；
    /// - GPU 组件：host 网络 + GPU 运行时（`gpu_flag`）+ NVIDIA 可见性环境变量。
    pub fn run_cmd(&self, gpu_flag: &str) -> String {
        let c = crate::deploy::script::sh_quote(&self.container);
        let img = crate::deploy::script::sh_quote(&self.image);
        if self.gpu {
            format!(
                "docker run -d --name {c} --restart=unless-stopped --net=host --pid=host {gpu_flag} \
                 -e NVIDIA_VISIBLE_DEVICES=all -e NVIDIA_DRIVER_CAPABILITIES=all \
                 {img} --web.listen-address=:{} --collect.compute-apps",
                self.port
            )
        } else {
            format!(
                "docker run -d --name {c} --restart=unless-stopped --net=host --pid=host \
                 -v /:/host:ro,rslave {img} --path.rootfs=/host --no-collector.cpufreq --web.listen-address=:{}",
                self.port
            )
        }
    }

    /// 容器是否 running
    pub fn running_cmd(&self) -> String {
        format!(
            "docker inspect -f '{{{{.State.Running}}}}' {} 2>/dev/null",
            crate::deploy::script::sh_quote(&self.container)
        )
    }

    /// 本机端口可达性检查（bash /dev/tcp）
    pub fn port_check_cmd(&self) -> String {
        format!("timeout 3 bash -c \"echo > /dev/tcp/127.0.0.1/{}\" 2>/dev/null", self.port)
    }

    /// 指标内容校验命令：抓取 `/metrics` 并要求 HTTP 200 + 关键标记（见 D-MI-MH-02）
    ///
    /// `curl -fsS` 在 HTTP ≥ 400（含 503）或超时（`--max-time`）时非零退出且不输出正文，
    /// 管道进入 `grep -q` 无匹配 → 整条命令失败；命中标记时 `grep` 提前退出返回 0。
    pub fn content_check_cmd(&self) -> String {
        let marker = crate::deploy::script::sh_quote(content_marker(self.kind));
        format!(
            "curl -fsS --max-time {} http://127.0.0.1:{}/metrics | grep -qm1 -e {}",
            CONTENT_CHECK_TIMEOUT_SECS, self.port, marker
        )
    }
}

/// 由节点台账 + 组件配置构造监控组件（纯函数）
pub fn component_for(node: &node::Model, kind: ComponentKind, cfg: &MonitorConfig) -> MonitorComponent {
    match kind {
        ComponentKind::NodeExporter => MonitorComponent::node_exporter(
            &cfg.node_exporter_image,
            u16::try_from(node.node_exporter_port.max(1)).unwrap_or(DEFAULT_NODE_EXPORTER_PORT as u16),
        ),
        ComponentKind::NvidiaGpuExporter => MonitorComponent::nvidia_gpu_exporter(
            &cfg.nvidia_gpu_exporter_image,
            u16::try_from(node.nvidia_gpu_exporter_port.max(1)).unwrap_or(DEFAULT_NVIDIA_GPU_EXPORTER_PORT as u16),
        ),
    }
}

/// consul 服务定义
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsulService {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub tags: Vec<String>,
    pub meta: BTreeMap<String, String>,
}

/// consul 中已注册服务的摘要（reconcile 用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredService {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
}

fn service_meta(node_id: &str, node_name: &str, vendor: &str, category: &str) -> BTreeMap<String, String> {
    let mut meta = BTreeMap::new();
    meta.insert("node_id".to_string(), node_id.to_string());
    meta.insert("node_name".to_string(), node_name.to_string());
    if !vendor.is_empty() {
        meta.insert("vendor".to_string(), vendor.to_string());
    }
    meta.insert("category".to_string(), category.to_string());
    meta
}

/// 由节点台账 + 组件构造 consul 服务定义（纯函数）
pub fn service_spec(m: &node::Model, comp: &MonitorComponent) -> ConsulService {
    ConsulService {
        id: format!("{}{}", comp.kind.id_prefix(), m.id),
        name: comp.name.clone(),
        address: m.host.clone(),
        port: comp.port,
        tags: vec!["modelinfer".to_string(), comp.kind.as_str().to_string()],
        meta: service_meta(&m.id, &m.name, &m.gpu_vendor, &comp.category),
    }
}

/// reconcile 期望项（节点摘要，纯函数入参）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileNode {
    pub id: String,
    pub name: String,
    pub host: String,
    pub vendor: String,
    pub node_exporter_port: u16,
    pub nvidia_gpu_exporter_port: u16,
    /// 期望保持注册的组件（仅「已部署 / 部署中」，由 `reconcile_once` 结合监控状态填充）
    pub components: Vec<ComponentKind>,
}

impl From<&node::Model> for ReconcileNode {
    fn from(m: &node::Model) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            host: m.host.clone(),
            vendor: m.gpu_vendor.clone(),
            node_exporter_port: u16::try_from(m.node_exporter_port.max(1))
                .unwrap_or(DEFAULT_NODE_EXPORTER_PORT as u16),
            nvidia_gpu_exporter_port: u16::try_from(m.nvidia_gpu_exporter_port.max(1))
                .unwrap_or(DEFAULT_NVIDIA_GPU_EXPORTER_PORT as u16),
            components: Vec::new(),
        }
    }
}

impl ReconcileNode {
    /// 该节点期望的 consul 服务定义（每个「已部署 / 部署中」组件一个）
    pub fn expected_services(&self) -> Vec<ConsulService> {
        self.components
            .iter()
            .map(|kind| {
                let port = match kind {
                    ComponentKind::NodeExporter => self.node_exporter_port,
                    ComponentKind::NvidiaGpuExporter => self.nvidia_gpu_exporter_port,
                };
                ConsulService {
                    id: format!("{}{}", kind.id_prefix(), self.id),
                    name: kind.service_name().to_string(),
                    address: self.host.clone(),
                    port,
                    tags: vec!["modelinfer".to_string(), kind.as_str().to_string()],
                    meta: service_meta(&self.id, &self.name, &self.vendor, kind.category()),
                }
            })
            .collect()
    }
}

/// reconcile 差异计划
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcilePlan {
    /// 需补注册 / 覆盖的服务
    pub register: Vec<ConsulService>,
    /// 需注销的 consul 服务 ID（残留）
    pub deregister: Vec<String>,
}

/// 是否为本应用注册的监控服务 ID
fn is_our_service_id(sid: &str) -> bool {
    sid.starts_with(ComponentKind::NodeExporter.id_prefix())
        || sid.starts_with(ComponentKind::NvidiaGpuExporter.id_prefix())
}

/// 执行一次 reconcile：读取节点台账 + consul 服务，按差异计划补注册 / 清残留。
///
/// 返回处理条数。consul 不可达 → `20735`（由调用方决定是否告警）。
pub async fn reconcile_once(
    db: &sea_orm::DatabaseConnection,
    consul: &dyn consul::ConsulClient,
) -> Result<usize, ModelinferError> {
    let models = crate::nodes::load_models(db, None).await?;
    let monitors = store::list(db).await?;
    let mut nodes: Vec<ReconcileNode> = models.iter().map(ReconcileNode::from).collect();
    // 仅「已部署 / 部署中」的组件期望保持注册（避免未部署组件产生幽灵注册）
    for n in nodes.iter_mut() {
        n.components = monitors
            .iter()
            .filter(|m| {
                m.node_id == n.id
                    && matches!(m.state.as_str(), store::state::DEPLOYED | store::state::DEPLOYING)
            })
            .filter_map(|m| ComponentKind::parse(&m.component))
            .collect();
    }
    let actual = consul.list_services().await?;
    let plan = reconcile_plan(&nodes, &actual);
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

/// 启动进程级 reconcile 调度器（只对齐 consul 注册信息，不重启容器）
pub fn spawn_reconcile_scheduler(
    db: sea_orm::DatabaseConnection,
    consul: std::sync::Arc<dyn consul::ConsulClient>,
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
                    tracing::info!(target: "modelinfer_monitor", changed = n, "节点监控注册 reconcile 完成")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    target: "modelinfer_monitor",
                    code = e.code,
                    error = %e.msg,
                    "节点监控注册 reconcile 失败（忽略，保持既有注册）"
                ),
            }
        }
    });
}

/// 执行一次监控组件健康自愈（见 D-MI-MH-03）
///
/// 仅检查状态为 `deployed` 的组件（`deploying` / `failed` 跳过，避免与在途工作流冲突）：
/// 运行 `verify_component`（内容校验）；不健康时**重建容器**（`docker rm -f` + `docker run`，
/// 复用镜像）+ 重注册 consul + 记录 `auto-repair` 步骤。成功保持 `deployed`，失败置 `failed`。
///
/// SSH 连接失败（节点离线）视为不可判定，跳过该节点且 MUST NOT 误报 / 误重建。
/// 返回本次重建的组件数。
pub async fn health_once(
    db: &sea_orm::DatabaseConnection,
    ssh: &dyn SshExec,
    consul: &dyn consul::ConsulClient,
    cfg: &MonitorConfig,
) -> Result<usize, ModelinferError> {
    let models = crate::nodes::load_models(db, None).await?;
    let monitors = store::list(db).await?;
    health_apply(db, ssh, consul, cfg, &models, &monitors).await
}

/// 健康自愈核心（入参为已加载的节点台账与监控记录，便于单测注入）
async fn health_apply(
    db: &sea_orm::DatabaseConnection,
    ssh: &dyn SshExec,
    consul: &dyn consul::ConsulClient,
    cfg: &MonitorConfig,
    models: &[node::Model],
    monitors: &[crate::entities::node_monitor::Model],
) -> Result<usize, ModelinferError> {
    let mut repaired = 0usize;
    for m in monitors {
        if m.state != store::state::DEPLOYED {
            continue;
        }
        let Some(kind) = ComponentKind::parse(&m.component) else {
            continue;
        };
        let Some(node) = models.iter().find(|n| n.id == m.node_id) else {
            continue;
        };
        let comp = component_for(node, kind, cfg);
        let target = crate::nodes::ssh_target_of(node);
        match verify_component(ssh, &target, &comp).await {
            Ok(()) => continue,
            Err(e) if e.code == codes::SSH_CONNECT_FAILED => {
                tracing::debug!(
                    target: "modelinfer_monitor",
                    node = %node.id,
                    component = kind.as_str(),
                    "节点 SSH 不可达，跳过监控健康自愈"
                );
                continue;
            }
            Err(e) => tracing::warn!(
                target: "modelinfer_monitor",
                node = %node.id,
                component = kind.as_str(),
                code = e.code,
                error = %e.msg,
                "监控组件指标不可用，尝试重建容器"
            ),
        }
        let gpu_flag = if comp.gpu { detect_gpu_flag(ssh, &target).await } else { String::new() };
        match deploy_component(ssh, &target, &comp, &gpu_flag).await {
            Ok(()) => {
                let svc = service_spec(node, &comp);
                let detail = match consul.register(&svc).await {
                    Ok(()) => "已重建容器并重注册 consul".to_string(),
                    Err(e) => format!("已重建容器，consul 重注册失败: {}", e.msg),
                };
                let step = store::MonitorStep::new("auto-repair", "自动修复").done(true, detail);
                let _ = store::append_step(db, &node.id, kind.as_str(), step).await;
                repaired += 1;
            }
            Err(e) => {
                let _ = store::finish(
                    db,
                    &node.id,
                    kind.as_str(),
                    store::state::FAILED,
                    &format!("健康自愈重建失败: {}", e.msg),
                )
                .await;
            }
        }
    }
    Ok(repaired)
}

/// 启动进程级监控健康自愈调度器（周期 = reconcile 周期，见 D-MI-MH-03）
pub fn spawn_health_scheduler(
    db: sea_orm::DatabaseConnection,
    ssh: std::sync::Arc<dyn SshExec>,
    consul: std::sync::Arc<dyn consul::ConsulClient>,
    cfg: MonitorConfig,
    interval_secs: u64,
) {
    let interval = std::time::Duration::from_secs(interval_secs.max(10));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match health_once(&db, ssh.as_ref(), consul.as_ref(), &cfg).await {
                Ok(n) if n > 0 => tracing::info!(
                    target: "modelinfer_monitor",
                    repaired = n,
                    "节点监控健康自愈完成"
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    target: "modelinfer_monitor",
                    code = e.code,
                    error = %e.msg,
                    "节点监控健康自愈失败（忽略，等待下一周期）"
                ),
            }
        }
    });
}

/// 生成 reconcile 差异计划（纯函数，见 design D-MI-NGM-06）
///
/// - 期望集合：每节点所有适用组件 → 对应 service id；缺失或 address/port 不一致 → register；
/// - consul 中本应用组件前缀匹配但不在期望集合 → deregister。
pub fn reconcile_plan(
    nodes: &[ReconcileNode],
    actual: &BTreeMap<String, RegisteredService>,
) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();
    let mut expected_ids: Vec<String> = Vec::new();
    for n in nodes {
        for svc in n.expected_services() {
            expected_ids.push(svc.id.clone());
            match actual.get(&svc.id) {
                Some(reg) if reg.address == svc.address && reg.port == svc.port => {}
                _ => plan.register.push(svc),
            }
        }
    }
    for (sid, _) in actual.iter() {
        if is_our_service_id(sid) && !expected_ids.contains(sid) {
            plan.deregister.push(sid.clone());
        }
    }
    plan
}

/// 读取节点 `docker info` 并选择 GPU 运行参数（GPU 组件部署前调用）
pub async fn detect_gpu_flag(ssh: &dyn SshExec, target: &SshTarget) -> String {
    let out = ssh
        .run_with_timeout(
            target,
            "docker info --format '{{json .Runtimes}}' 2>/dev/null",
            std::time::Duration::from_secs(MONITOR_CMD_TIMEOUT_SECS),
        )
        .await;
    match out {
        Ok(o) => gpu_runtime_flag(&o.stdout).to_string(),
        Err(_) => gpu_runtime_flag("").to_string(),
    }
}

/// 在节点上启动监控组件容器并校验（running + 端口可达）
///
/// `gpu_flag` 仅 GPU 组件使用（系统组件忽略）。GPU 组件启动失败 → `20739`。
pub async fn deploy_component(
    ssh: &dyn SshExec,
    target: &SshTarget,
    comp: &MonitorComponent,
    gpu_flag: &str,
) -> Result<(), ModelinferError> {
    let _ = ssh
        .run_with_timeout(target, &comp.remove_cmd(), std::time::Duration::from_secs(MONITOR_CMD_TIMEOUT_SECS))
        .await;
    let run = ssh
        .run_with_timeout(
            target,
            &comp.run_cmd(gpu_flag),
            std::time::Duration::from_secs(MONITOR_CMD_TIMEOUT_SECS),
        )
        .await?;
    if !run.success() {
        let code = if comp.gpu { codes::GPU_RUNTIME_UNAVAILABLE } else { codes::MONITOR_DEPLOY_FAILED };
        return Err(ModelinferError::new(
            code,
            format!(
                "启动监控组件 {} 容器失败(exit={}): {}{}",
                comp.kind.as_str(),
                run.exit_code,
                run.stdout.trim(),
                run.stderr.trim()
            ),
        ));
    }
    let running = ssh
        .run_with_timeout(
            target,
            &comp.running_cmd(),
            std::time::Duration::from_secs(MONITOR_CMD_TIMEOUT_SECS),
        )
        .await?;
    if running.stdout.trim() != "true" {
        let code = if comp.gpu { codes::GPU_RUNTIME_UNAVAILABLE } else { codes::MONITOR_DEPLOY_FAILED };
        return Err(ModelinferError::new(
            code,
            format!("监控组件 {} 容器未处于运行状态: {}", comp.kind.as_str(), running.stdout.trim()),
        ));
    }
    verify_component(ssh, target, comp).await
}

/// 校验监控组件可用（端口可达 + **指标内容可用**，见 D-MI-MH-02）
///
/// 先做本机端口前置检查（快速），再做内容校验：抓取 `/metrics` 要求 HTTP 200 且包含
/// `content_marker(kind)`。内容校验覆盖“端口在、指标坏”（cpufreq 挂起导致 503、GPU 设备失效
/// 导致 nvidia-smi 失败等）。失败统一复用 `20734`。
pub async fn verify_component(
    ssh: &dyn SshExec,
    target: &SshTarget,
    comp: &MonitorComponent,
) -> Result<(), ModelinferError> {
    let check = ssh
        .run_with_timeout(
            target,
            &comp.port_check_cmd(),
            std::time::Duration::from_secs(MONITOR_CMD_TIMEOUT_SECS),
        )
        .await?;
    if !check.success() {
        return Err(ModelinferError::new(
            codes::MONITOR_DEPLOY_FAILED,
            format!("监控组件 {} 端口 {} 在节点本机不可达", comp.kind.as_str(), comp.port),
        ));
    }
    // 内容校验（HTTP 200 + 关键标记）
    let content = ssh
        .run_with_timeout(
            target,
            &comp.content_check_cmd(),
            std::time::Duration::from_secs(CONTENT_CHECK_TIMEOUT_SECS + 6),
        )
        .await?;
    if !content.success() {
        return Err(ModelinferError::new(
            codes::MONITOR_DEPLOY_FAILED,
            format!(
                "监控组件 {} 指标不可用（需 HTTP 200 且包含 `{}`）",
                comp.kind.as_str(),
                content_marker(comp.kind)
            ),
        ));
    }
    Ok(())
}

/// 确保监控组件镜像在节点就绪（节点本地 → image-hub 存在性 → `image-hub.sh` 拉取 → 复检）。
///
/// 返回是否执行了拉取。**绝不**直接 `docker pull`（算力节点无外网）：
/// - 节点本地已有 → `Ok(false)`（零副作用）；
/// - 节点与 image-hub 均无 → `20737`（中止）；
/// - 拉取失败 / 拉取后仍缺失 → `20738`。
#[allow(clippy::too_many_arguments)] // 镜像分发编排的显式依赖（SSH/registry/源地址/配置/目标/目录/镜像）
pub async fn ensure_component_image(
    ssh: &dyn SshExec,
    hub: &dyn crate::images::ImageHubClient,
    srcip: &dyn crate::images::SourceIpResolver,
    cfg: &crate::images::ImageHubConfig,
    candidates: &[String],
    target: &SshTarget,
    workdir: &str,
    image: &str,
) -> Result<bool, ModelinferError> {
    let (repository, tag) = crate::images::normalize_image_ref(image)?;
    let full = format!("{repository}:{tag}");
    // 1. 节点本地
    if crate::images::image_present(ssh, target, &full).await? {
        return Ok(false);
    }
    // 2. image-hub 存在性
    if !hub.manifest_exists(&repository, &tag).await? {
        return Err(ModelinferError::new(
            codes::MONITOR_IMAGE_UNAVAILABLE,
            format!("监控镜像在节点与 image-hub 上均不存在，已中止（不会直接 docker pull）: {full}"),
        ));
    }
    // 3. 经 image-hub.sh 拉取
    let host = crate::images::resolve_hub_host(ssh, srcip, cfg, candidates, &target.host, target).await?;
    let addr = cfg.addr(&host);
    let script = format!("{workdir}/.modelinfer/image-hub.sh");
    crate::images::prepare_workdir(ssh, target, workdir).await?;
    crate::images::upload_script(ssh, target, &script, crate::images::IMAGE_HUB_SH).await?;
    let cmd = format!(
        "bash {} --ih-addr {} pull {}",
        crate::deploy::script::sh_quote(&script),
        crate::deploy::script::sh_quote(&addr),
        crate::deploy::script::sh_quote(&full)
    );
    let out = ssh
        .run_with_timeout(
            target,
            &cmd,
            std::time::Duration::from_secs(crate::images::IMAGE_PULL_TIMEOUT_SECS),
        )
        .await?;
    if !out.success() {
        return Err(ModelinferError::new(
            codes::MONITOR_PULL_FAILED,
            format!(
                "经 image-hub({addr}) 拉取监控镜像 {full} 失败(exit={}): {}{}",
                out.exit_code,
                out.stdout.trim(),
                out.stderr.trim()
            ),
        ));
    }
    if !crate::images::image_present(ssh, target, &full).await? {
        return Err(ModelinferError::new(
            codes::MONITOR_PULL_FAILED,
            format!("经 image-hub 拉取后仍缺少监控镜像 {full}"),
        ));
    }
    Ok(true)
}

/// 停止并移除监控组件容器（best-effort，失败仅返回错误供调用方告警）
pub async fn teardown_component(
    ssh: &dyn SshExec,
    target: &SshTarget,
    container: &str,
) -> Result<(), ModelinferError> {
    let cmd = format!(
        "docker rm -f {} >/dev/null 2>&1 || true",
        crate::deploy::script::sh_quote(container)
    );
    let out = ssh
        .run_with_timeout(target, &cmd, std::time::Duration::from_secs(MONITOR_CMD_TIMEOUT_SECS))
        .await?;
    if !out.success() {
        return Err(ModelinferError::new(
            codes::MONITOR_DEPLOY_FAILED,
            format!("停止监控组件容器失败: {}", out.stderr.trim()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg_node(id: &str, host: &str, sys_port: u16, gpu_port: u16, vendor: &str, components: &[ComponentKind]) -> ReconcileNode {
        ReconcileNode {
            id: id.into(),
            name: id.into(),
            host: host.into(),
            vendor: vendor.into(),
            node_exporter_port: sys_port,
            nvidia_gpu_exporter_port: gpu_port,
            components: components.to_vec(),
        }
    }

    fn svc(id: &str, name: &str, addr: &str, port: u16) -> RegisteredService {
        RegisteredService { id: id.into(), name: name.into(), address: addr.into(), port }
    }

    #[test]
    fn applicable_by_vendor() {
        assert_eq!(applicable_components("nvidia"), vec![ComponentKind::NodeExporter, ComponentKind::NvidiaGpuExporter]);
        assert_eq!(applicable_components("NVIDIA"), vec![ComponentKind::NodeExporter, ComponentKind::NvidiaGpuExporter]);
        assert_eq!(applicable_components("amd"), vec![ComponentKind::NodeExporter]);
        assert_eq!(applicable_components(""), vec![ComponentKind::NodeExporter]);
    }

    #[test]
    fn component_spec_commands() {
        let c = MonitorComponent::node_exporter(DEFAULT_NODE_EXPORTER_IMAGE, 9100);
        assert_eq!(c.category, "system");
        assert!(!c.gpu);
        assert!(c.remove_cmd().contains("docker rm -f"));
        let run = c.run_cmd("--runtime=nvidia");
        assert!(run.contains("--net=host"));
        assert!(run.contains("--pid=host"));
        assert!(run.contains("-v /:/host:ro,rslave"));
        assert!(run.contains("--path.rootfs=/host"));
        assert!(run.contains("--no-collector.cpufreq"), "系统组件必须禁用 cpufreq 采集器: {run}");
        assert!(run.contains("--web.listen-address=:9100"));
        assert!(c.running_cmd().contains(".State.Running"));
        assert!(c.port_check_cmd().contains("/dev/tcp/127.0.0.1/9100"));
        // 内容校验命令：HTTP 200 + 标记
        let cc = c.content_check_cmd();
        assert!(cc.contains("curl -fsS"), "{cc}");
        assert!(cc.contains("http://127.0.0.1:9100/metrics"), "{cc}");
        assert!(cc.contains("^node_"), "{cc}");

        let g = MonitorComponent::nvidia_gpu_exporter(DEFAULT_NVIDIA_GPU_EXPORTER_IMAGE, 9835);
        assert_eq!(g.category, "gpu");
        assert!(g.gpu);
        assert_eq!(g.name, "modelinfer-nvidia-gpu-exporter");
        assert_eq!(g.container, "modelinfer-nvidia-gpu-exporter");
        let run = g.run_cmd("--runtime=nvidia");
        assert!(run.contains("--net=host"));
        assert!(run.contains("--pid=host"));
        assert!(run.contains("--runtime=nvidia"));
        assert!(run.contains("NVIDIA_VISIBLE_DEVICES=all"));
        assert!(run.contains("NVIDIA_DRIVER_CAPABILITIES=all"));
        assert!(run.contains("--collect.compute-apps"));
        assert!(run.contains("--web.listen-address=:9835"));
        assert!(!run.contains("--path.rootfs"), "GPU 组件不应挂宿主 rootfs");
        assert!(!run.contains("--no-collector.cpufreq"), "GPU 组件不涉及 node-exporter 采集器");
        // gpus 回退
        assert!(g.run_cmd("--gpus all").contains("--gpus all"));
        // GPU 内容标记为 nvidia-smi 成功
        assert!(g.content_check_cmd().contains("nvidia_smi_command_exit_code 0"));
    }

    #[test]
    fn gpu_runtime_flag_detection() {
        assert_eq!(gpu_runtime_flag(r#"{"nvidia":{"path":"nvidia-container-runtime"},"runc":{}}"#), "--runtime=nvidia");
        assert_eq!(gpu_runtime_flag(r#"{"runc":{}}"#), "--gpus all");
        assert_eq!(gpu_runtime_flag(""), "--gpus all");
    }

    #[test]
    fn reconcile_plan_multi_component() {
        use ComponentKind::*;
        let nodes = vec![
            reg_node("a", "10.0.0.1", 9100, 9835, "nvidia", &[NodeExporter, NvidiaGpuExporter]),
            reg_node("b", "10.0.0.2", 9100, 9835, "", &[NodeExporter]),
        ];
        let mut actual = BTreeMap::new();
        // a 系统已注册；a GPU 缺失 → 补注册
        actual.insert(
            format!("{}a", NodeExporter.id_prefix()),
            svc(&format!("{}a", NodeExporter.id_prefix()), "modelinfer-node-exporter", "10.0.0.1", 9100),
        );
        // b 系统注册的端口不对 → 需覆盖
        actual.insert(
            format!("{}b", NodeExporter.id_prefix()),
            svc(&format!("{}b", NodeExporter.id_prefix()), "modelinfer-node-exporter", "10.0.0.2", 9999),
        );
        // 残留：节点 c 的 GPU 服务，c 已不存在
        actual.insert(
            format!("{}c", NvidiaGpuExporter.id_prefix()),
            svc(&format!("{}c", NvidiaGpuExporter.id_prefix()), "modelinfer-nvidia-gpu-exporter", "10.0.0.3", 9835),
        );
        // 非本应用服务不应被注销
        actual.insert("other".into(), svc("other", "modelinfer-node-exporter", "1.2.3.4", 9100));

        let plan = reconcile_plan(&nodes, &actual);
        assert_eq!(plan.register.len(), 2, "a GPU 补注册 + b 系统覆盖");
        assert!(plan.register.iter().any(|s| s.id == format!("{}a", NvidiaGpuExporter.id_prefix())));
        assert!(plan.register.iter().any(|s| s.id == format!("{}b", NodeExporter.id_prefix())));
        assert_eq!(plan.deregister, vec![format!("{}c", NvidiaGpuExporter.id_prefix())]);
    }

    #[test]
    fn reconcile_plan_empty_and_all_missing() {
        let plan = reconcile_plan(&[], &BTreeMap::new());
        assert!(plan.register.is_empty() && plan.deregister.is_empty());

        // 已部署组件全部缺失 → 每组件一条；未部署组件不产生注册
        let plan = reconcile_plan(
            &[reg_node("a", "10.0.0.1", 9100, 9835, "nvidia", &[ComponentKind::NodeExporter, ComponentKind::NvidiaGpuExporter])],
            &BTreeMap::new(),
        );
        assert_eq!(plan.register.len(), 2);
        let plan = reconcile_plan(&[reg_node("a", "10.0.0.1", 9100, 9835, "nvidia", &[])], &BTreeMap::new());
        assert_eq!(plan.register.len(), 0, "未部署组件不应产生幽灵注册");
    }

    #[test]
    fn service_spec_uses_component() {
        let m = node::Model {
            profile_override: None,
            detected_profile: None,
            id: "n1".into(),
            name: "node-1".into(),
            host: "10.0.0.1".into(),
            ssh_port: 22,
            ssh_user: "root".into(),
            auth_method: "password".into(),
            ssh_password: String::new(),
            ssh_key: String::new(),
            sudo_password: String::new(),
            hostname: "h".into(),
            arch: "aarch64".into(),
            kernel_version: "5.10".into(),
            gateway: "10.0.0.254".into(),
            gpu_vendor: "nvidia".into(),
            gpu_model: "Orin".into(),
            gpu_info_json: "[]".into(),
            node_exporter_port: 9100,
            nvidia_gpu_exporter_port: 9835,
            socket_ifname: "eth0".into(),
            ib_hca: String::new(),
            interfaces_json: "[]".into(),
            ib_devices_json: "[]".into(),
            fscache: false,
            status: "online".into(),
            last_checked_at: None,
            created_at: "t".into(),
            updated_at: "t".into(),
        };
        let s = service_spec(&m, &MonitorComponent::node_exporter("img", 9100));
        assert_eq!(s.id, "modelinfer-node-exporter-n1");
        assert_eq!(s.name, "modelinfer-node-exporter");
        assert_eq!(s.port, 9100);
        assert_eq!(s.meta.get("category").map(String::as_str), Some("system"));

        let g = service_spec(&m, &MonitorComponent::nvidia_gpu_exporter("img", 9835));
        assert_eq!(g.id, "modelinfer-nvidia-gpu-exporter-n1");
        assert_eq!(g.name, "modelinfer-nvidia-gpu-exporter");
        assert_eq!(g.port, 9835);
        assert_eq!(g.meta.get("category").map(String::as_str), Some("gpu"));
        assert_eq!(g.meta.get("vendor").map(String::as_str), Some("nvidia"));
    }

    #[tokio::test]
    async fn ensure_image_skips_when_present_locally() {
        use crate::images::{ImageHubConfig, MockImageHubClient, MockSourceIpResolver};
        let ssh = crate::ssh::MockSshExec::new().with_rule("docker image inspect", 0, "");
        let hub = MockImageHubClient::new();
        let target = SshTarget::new("10.0.0.1", 22, "root", "x");
        let pulled = ensure_component_image(
            &ssh,
            &hub,
            &MockSourceIpResolver::new(),
            &ImageHubConfig::default(),
            &[],
            &target,
            "/opt/modelinfer/monitor",
            DEFAULT_NVIDIA_GPU_EXPORTER_IMAGE,
        )
        .await
        .unwrap();
        assert!(!pulled);
        assert!(hub.recorded_calls().is_empty(), "本地已有镜像不应查询 hub");
    }

    #[tokio::test]
    async fn ensure_image_unavailable_is_20737_and_never_pulls() {
        use crate::images::{ImageHubConfig, MockImageHubClient, MockSourceIpResolver};
        let ssh = crate::ssh::MockSshExec::new().with_failing_host("10.0.0.1");
        let hub = MockImageHubClient::new();
        let target = SshTarget::new("10.0.0.1", 22, "root", "x");
        let err = ensure_component_image(
            &ssh,
            &hub,
            &MockSourceIpResolver::new(),
            &ImageHubConfig::default(),
            &[],
            &target,
            "/opt/modelinfer/monitor",
            DEFAULT_NVIDIA_GPU_EXPORTER_IMAGE,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::MONITOR_IMAGE_UNAVAILABLE);
        assert!(!ssh.recorded_calls().iter().any(|c| c.contains("image-hub.sh")), "不应尝试 pull");
        assert_eq!(hub.recorded_calls(), vec!["docker.io/utkuozdemir/nvidia_gpu_exporter:1.15.1".to_string()]);
    }

    /// 健康自愈测试用节点
    fn health_node() -> node::Model {
        node::Model {
            profile_override: None,
            detected_profile: None,
            id: "n1".into(),
            name: "node-1".into(),
            host: "10.0.0.1".into(),
            ssh_port: 22,
            ssh_user: "root".into(),
            auth_method: "password".into(),
            ssh_password: "x".into(),
            ssh_key: String::new(),
            sudo_password: String::new(),
            hostname: "h".into(),
            arch: "aarch64".into(),
            kernel_version: "5.10".into(),
            gateway: "10.0.0.254".into(),
            gpu_vendor: "nvidia".into(),
            gpu_model: "GB10".into(),
            gpu_info_json: "[]".into(),
            node_exporter_port: 9100,
            nvidia_gpu_exporter_port: 9835,
            socket_ifname: "eth0".into(),
            ib_hca: String::new(),
            interfaces_json: "[]".into(),
            ib_devices_json: "[]".into(),
            fscache: false,
            status: "online".into(),
            last_checked_at: None,
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    #[tokio::test]
    async fn health_apply_skips_healthy_component() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let node = health_node();
        store::begin(&db, &node.id, component::NODE_EXPORTER, "wf", "img:1", "system", "svc", "modelinfer-node-exporter")
            .await
            .unwrap();
        store::finish(&db, &node.id, component::NODE_EXPORTER, store::state::DEPLOYED, "").await.unwrap();
        let monitors = store::list(&db).await.unwrap();
        // 端口可达 + 内容校验通过
        let ssh = crate::ssh::MockSshExec::new()
            .with_rule("/dev/tcp", 0, "")
            .with_rule("metrics", 0, "");
        let consul = consul::MockConsulClient::new();
        let repaired = health_apply(&db, &ssh, &consul, &MonitorConfig::default(), &[node], &monitors)
            .await
            .unwrap();
        assert_eq!(repaired, 0);
        assert!(!ssh.recorded_calls().iter().any(|c| c.contains("docker rm -f")), "健康组件不应重建");
    }

    #[tokio::test]
    async fn health_apply_rebuilds_unhealthy_component() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let node = health_node();
        store::begin(&db, &node.id, component::NODE_EXPORTER, "wf", "img:1", "system", "svc", "modelinfer-node-exporter")
            .await
            .unwrap();
        store::finish(&db, &node.id, component::NODE_EXPORTER, store::state::DEPLOYED, "").await.unwrap();
        let monitors = store::list(&db).await.unwrap();
        // 端口可达但内容校验失败（模拟 cpufreq 挂起 → 503）
        let ssh = crate::ssh::MockSshExec::new()
            .with_rule("/dev/tcp", 0, "")
            .with_rule("metrics", 1, "")
            .with_rule(".State.Running", 0, "true");
        let consul = consul::MockConsulClient::new();
        let _ = health_apply(&db, &ssh, &consul, &MonitorConfig::default(), std::slice::from_ref(&node), &monitors)
            .await
            .unwrap();
        let calls = ssh.recorded_calls();
        assert!(calls.iter().any(|c| c.contains("docker rm -f")), "应重建容器: {calls:?}");
        assert!(
            calls
                .iter()
                .any(|c| c.contains("docker run -d --name") && c.contains("modelinfer-node-exporter")),
            "应以新参数重跑容器: {calls:?}"
        );
        // mock 无法真正恢复指标 → 重建后校验仍失败 → 置 failed
        assert_eq!(
            store::view_of(&db, &node.id, component::NODE_EXPORTER).await.unwrap().state,
            store::state::FAILED
        );
    }

    #[tokio::test]
    async fn health_apply_skips_offline_node() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let node = health_node();
        store::begin(&db, &node.id, component::NODE_EXPORTER, "wf", "img:1", "system", "svc", "modelinfer-node-exporter")
            .await
            .unwrap();
        store::finish(&db, &node.id, component::NODE_EXPORTER, store::state::DEPLOYED, "").await.unwrap();
        let monitors = store::list(&db).await.unwrap();
        let ssh = crate::ssh::MockSshExec::new().with_connect_failure();
        let consul = consul::MockConsulClient::new();
        let repaired = health_apply(&db, &ssh, &consul, &MonitorConfig::default(), &[node], &monitors)
            .await
            .unwrap();
        assert_eq!(repaired, 0);
        assert!(!ssh.recorded_calls().iter().any(|c| c.contains("docker rm -f")), "离线节点不应重建");
    }
}