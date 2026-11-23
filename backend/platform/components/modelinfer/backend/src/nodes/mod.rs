//! 节点管理领域逻辑：CRUD + SSH 探活（密码 / 私钥）+ 硬件与 GPU 扫描 + 凭据脱敏
//!
//! SSH 用户可为 root 或 sudo 用户；认证方式 password / key；凭据只入不出。

pub mod scan;

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};

use crate::entities::node;
use crate::error::{ModelinferError, codes};
use crate::ssh::{SshAuth, SshExec, SshTarget};
use crate::time;
use scan::{GpuDevice, InterfaceInfo, NodeScanResult, scan_node};

/// node-exporter 默认端口（与监控域单一来源，避开上游标准端口 9100）
pub const DEFAULT_NODE_EXPORTER_PORT: i32 = crate::monitor::DEFAULT_NODE_EXPORTER_PORT;
/// nvidia-gpu-exporter 默认端口
pub const DEFAULT_NVIDIA_GPU_EXPORTER_PORT: i32 = crate::monitor::DEFAULT_NVIDIA_GPU_EXPORTER_PORT;

/// 创建节点请求
#[derive(Debug, Clone, Deserialize)]
pub struct NodeCreate {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub ssh_port: i32,
    /// SSH 用户（默认 root；普通用户需要 sudo root 权限）
    #[serde(default = "default_user")]
    pub ssh_user: String,
    /// 认证方式：`password`（缺省）/ `key`
    #[serde(default)]
    pub auth_method: String,
    #[serde(default)]
    pub ssh_password: String,
    /// SSH 私钥内容（`auth_method=key` 时必填）
    #[serde(default)]
    pub ssh_key: String,
    #[serde(default)]
    pub sudo_password: String,
    /// 节点主机名（扫描得到，可手动覆盖）
    #[serde(default)]
    pub hostname: Option<String>,
    /// 控制面网卡名
    #[serde(default)]
    pub socket_ifname: Option<String>,
    /// IB 设备名（可空）
    #[serde(default)]
    pub ib_hca: Option<String>,
    /// 挂载 model-hub NFS 时是否启用 fscache（默认 false）
    #[serde(default)]
    pub fscache: bool,
    /// node-exporter 监听端口（高级配置；默认 19100）
    #[serde(default = "default_node_exporter_port")]
    pub node_exporter_port: i32,
    /// nvidia-gpu-exporter 监听端口（高级配置；默认 19835）
    #[serde(default = "default_nvidia_gpu_exporter_port")]
    pub nvidia_gpu_exporter_port: i32,
}

/// 更新节点请求（字段可选；`ssh_password` / `ssh_key` 为空表示不改）
#[derive(Debug, Clone, Deserialize)]
pub struct NodeUpdate {
    pub name: Option<String>,
    pub host: Option<String>,
    pub ssh_port: Option<i32>,
    pub ssh_user: Option<String>,
    /// 认证方式（`password` / `key`；切换为 `key` 时须有私钥）
    pub auth_method: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_key: Option<String>,
    /// 省略保留，空字符串清除；空值回退 SSH 密码或免密 sudo。
    pub sudo_password: Option<String>,
    pub hostname: Option<String>,
    pub socket_ifname: Option<String>,
    pub ib_hca: Option<String>,
    /// 挂载 model-hub NFS 时是否启用 fscache（未提供表示不改）
    pub fscache: Option<bool>,
    /// node-exporter 端口（未提供表示不改）
    pub node_exporter_port: Option<i32>,
    /// nvidia-gpu-exporter 端口（未提供表示不改）
    pub nvidia_gpu_exporter_port: Option<i32>,
}

/// 节点视图（凭据脱敏）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeView {
    pub id: String,
    pub name: String,
    pub host: String,
    pub ssh_port: i32,
    pub ssh_user: String,
    /// 认证方式：`password` / `key`
    pub auth_method: String,
    pub has_password: bool,
    pub has_key: bool,
    #[serde(default)]
    pub has_sudo_password: bool,
    pub hostname: String,
    /// CPU 架构
    pub arch: String,
    /// 内核版本
    pub kernel_version: String,
    /// 默认网关
    pub gateway: String,
    /// GPU 厂商（`nvidia` / `amd` / `intel`；空 = 未识别）
    pub gpu_vendor: String,
    /// GPU 型号汇总
    pub gpu_model: String,
    /// 全部 GPU 设备
    pub gpu_devices: Vec<GpuDevice>,
    pub socket_ifname: String,
    pub ib_hca: String,
    pub interfaces: Vec<InterfaceInfo>,
    pub ib_devices: Vec<String>,
    pub fscache: bool,
    /// node-exporter 监听端口
    pub node_exporter_port: i32,
    /// nvidia-gpu-exporter 监听端口
    pub nvidia_gpu_exporter_port: i32,
    pub status: String,
    /// 是否被至少一个活跃运行配置使用（由活跃运行配置的节点集合实时派生）
    #[serde(default)]
    pub busy: bool,
    /// 首个使用该节点的运行配置 id（空闲时为空串；兼容旧客户端）
    #[serde(default)]
    pub busy_deployment_id: String,
    /// 首个使用该节点的运行配置名称（空闲时为空串；兼容旧客户端）
    #[serde(default)]
    pub busy_deployment_name: String,
    /// 使用该节点的全部运行配置 id（按运行配置创建时间稳定排序）
    #[serde(default)]
    pub busy_deployment_ids: Vec<String>,
    /// 使用该节点的全部运行配置名称（与 `busy_deployment_ids` 同序）
    #[serde(default)]
    pub busy_deployment_names: Vec<String>,
    pub last_checked_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// 节点监控状态摘要（由监控表实时派生；未部署为空 / pending）
    #[serde(default)]
    pub monitor_state: String,
    #[serde(default)]
    pub monitor_last_error: String,
    #[serde(default)]
    pub monitor_deployed_at: Option<String>,
}

impl From<node::Model> for NodeView {
    fn from(m: node::Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            host: m.host,
            ssh_port: m.ssh_port,
            ssh_user: m.ssh_user,
            auth_method: m.auth_method,
            has_password: !m.ssh_password.is_empty(),
            has_key: !m.ssh_key.trim().is_empty(),
            has_sudo_password: !m.sudo_password.is_empty(),
            hostname: m.hostname,
            arch: m.arch,
            kernel_version: m.kernel_version,
            gateway: m.gateway,
            gpu_vendor: m.gpu_vendor,
            gpu_model: m.gpu_model,
            gpu_devices: serde_json::from_str(&m.gpu_info_json).unwrap_or_default(),
            socket_ifname: m.socket_ifname,
            ib_hca: m.ib_hca,
            interfaces: serde_json::from_str(&m.interfaces_json).unwrap_or_default(),
            ib_devices: serde_json::from_str(&m.ib_devices_json).unwrap_or_default(),
            fscache: m.fscache,
            node_exporter_port: m.node_exporter_port,
            nvidia_gpu_exporter_port: m.nvidia_gpu_exporter_port,
            status: m.status,
            busy: false,
            busy_deployment_id: String::new(),
            busy_deployment_name: String::new(),
            busy_deployment_ids: Vec::new(),
            busy_deployment_names: Vec::new(),
            last_checked_at: m.last_checked_at,
            created_at: m.created_at,
            updated_at: m.updated_at,
            monitor_state: String::new(),
            monitor_last_error: String::new(),
            monitor_deployed_at: None,
        }
    }
}

/// 按监控状态富化节点视图（就地）
pub fn apply_monitor(
    views: &mut [NodeView],
    summaries: &std::collections::HashMap<String, crate::monitor::store::MonitorSummary>,
) {
    for v in views.iter_mut() {
        if let Some(s) = summaries.get(&v.id) {
            v.monitor_state = s.monitor_state.clone();
            v.monitor_last_error = s.monitor_last_error.clone();
            v.monitor_deployed_at = s.monitor_deployed_at.clone();
        }
    }
}

/// 按活跃运行配置承载关系富化节点视图（就地；无承载项则保持空闲）
///
/// 同一节点可被多个运行配置使用：多值字段收集全部归属，单值字段取首项以兼容旧客户端。
pub fn apply_occupancy(views: &mut [NodeView], occupied: &[crate::deployments::OccupiedNode]) {
    for v in views.iter_mut() {
        let mut ids: Vec<String> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for o in occupied.iter().filter(|o| o.node_id == v.id) {
            if ids.iter().any(|id| id == &o.deployment_id) {
                continue;
            }
            ids.push(o.deployment_id.clone());
            names.push(o.deployment_name.clone());
        }
        if ids.is_empty() {
            continue;
        }
        v.busy = true;
        v.busy_deployment_id = ids[0].clone();
        v.busy_deployment_name = names[0].clone();
        v.busy_deployment_ids = ids;
        v.busy_deployment_names = names;
    }
}

fn default_port() -> i32 {
    22
}
fn default_user() -> String {
    "root".to_string()
}
fn default_node_exporter_port() -> i32 {
    DEFAULT_NODE_EXPORTER_PORT
}
fn default_nvidia_gpu_exporter_port() -> i32 {
    DEFAULT_NVIDIA_GPU_EXPORTER_PORT
}

/// 节点记录的 SSH 认证方式（密码 / 私钥）
pub fn ssh_auth_of(m: &node::Model) -> SshAuth {
    SshAuth::from_parts(&m.auth_method, &m.ssh_password, &m.ssh_key)
}

/// 由节点记录构造 SSH 目标
pub fn ssh_target_of(m: &node::Model) -> SshTarget {
    SshTarget::with_auth(
        m.host.clone(),
        m.ssh_port as u16,
        m.ssh_user.clone(),
        ssh_auth_of(m),
    )
    .with_sudo_password(&m.sudo_password)
}

fn validate_port(port: i32) -> Result<(), ModelinferError> {
    if !(1..=65535).contains(&port) {
        return Err(ModelinferError::new(codes::PARAM_INVALID, "SSH 端口必须在 1-65535 之间"));
    }
    Ok(())
}

/// 校验并归一认证方式（空 → `password`；非法取值拒绝）
pub fn normalize_auth_method(raw: &str) -> Result<String, ModelinferError> {
    match raw.trim() {
        "" | "password" => Ok("password".to_string()),
        "key" => Ok("key".to_string()),
        other => Err(ModelinferError::new(
            codes::NODE_AUTH_INVALID,
            format!("SSH 认证方式非法（仅支持 password / key）: {other}"),
        )),
    }
}

/// 校验凭据与认证方式匹配
pub fn validate_auth(auth_method: &str, password: &str, key: &str) -> Result<(), ModelinferError> {
    match auth_method {
        "key" if key.trim().is_empty() => Err(ModelinferError::new(
            codes::NODE_AUTH_INVALID,
            "认证方式为 key 时必须提供 SSH 私钥",
        )),
        "password" if password.is_empty() => Err(ModelinferError::new(
            codes::NODE_AUTH_INVALID,
            "认证方式为 password 时必须提供 SSH 密码",
        )),
        "key" | "password" => Ok(()),
        other => Err(ModelinferError::new(
            codes::NODE_AUTH_INVALID,
            format!("SSH 认证方式非法: {other}"),
        )),
    }
}

/// 空用户名兼容旧客户端的 root 默认值；拒绝空白和控制字符。
pub fn normalize_ssh_user(user: &str) -> Result<String, ModelinferError> {
    let u = user.trim();
    if u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ModelinferError::new(
            codes::NODE_AUTH_INVALID,
            "SSH 用户名不能包含空白或控制字符",
        ));
    }
    Ok(if u.is_empty() { "root".to_string() } else { u.to_string() })
}

fn validate_sudo_password(user: &str, auth_method: &str, password: &str, sudo_password: &str) -> Result<(), ModelinferError> {
    let effective = if !sudo_password.is_empty() { sudo_password } else if auth_method == "password" { password } else { "" };
    if user != "root" && effective.contains(['\r', '\n', '\0']) {
        return Err(ModelinferError::new(codes::NODE_AUTH_INVALID, "sudo 密码不能包含换行或空字符"));
    }
    Ok(())
}

/// 校验监控组件端口（node-exporter / nvidia-gpu-exporter 共用）
pub fn validate_monitor_port(port: i32) -> Result<(), ModelinferError> {
    if !(1024..=65535).contains(&port) {
        return Err(ModelinferError::new(
            codes::PARAM_INVALID,
            "监控组件端口必须在 1024-65535 之间",
        ));
    }
    Ok(())
}

pub async fn list_nodes(db: &DatabaseConnection) -> Result<Vec<NodeView>, ModelinferError> {
    let rows = node::Entity::find()
        .order_by_asc(node::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows.into_iter().map(NodeView::from).collect())
}

// ============ 列表筛选 / 排序 / 统计（纯函数，供分页前端参数使用） ============

/// 节点排序方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeSort {
    /// 名称（Unicode 码点序；不做拼音排序，见 change design §6）
    Name,
    /// 最近探活（降序）
    Recent,
}

impl NodeSort {
    /// 解析排序取值，未知值回落 `Name`
    pub fn parse(value: &str) -> Self {
        match value {
            "recent" => Self::Recent,
            _ => Self::Name,
        }
    }
}

/// 节点筛选参数
#[derive(Debug, Clone)]
pub struct NodeFilter {
    /// 关键词（大小写不敏感），匹配 名称 / 主机 / 主机名
    pub keyword: String,
    /// 状态（`online` / `offline`）；`all` / 空表示不限
    pub status: String,
    pub sort: NodeSort,
}

impl Default for NodeFilter {
    fn default() -> Self {
        Self { keyword: String::new(), status: "all".into(), sort: NodeSort::Name }
    }
}

/// 节点的可搜索文本（与前端治理前字段集合一致）
pub fn node_search_text(n: &NodeView) -> String {
    [n.name.as_str(), n.host.as_str(), n.hostname.as_str(), n.gpu_model.as_str()]
        .join(" ")
        .to_lowercase()
}

/// 按关键词 / 状态筛选并排序节点
pub fn filter_nodes(rows: Vec<NodeView>, f: &NodeFilter) -> Vec<NodeView> {
    let kw = f.keyword.trim().to_lowercase();
    let mut list: Vec<NodeView> = rows
        .into_iter()
        .filter(|n| {
            if f.status != "all" && !f.status.is_empty() && n.status != f.status {
                return false;
            }
            if kw.is_empty() {
                return true;
            }
            node_search_text(n).contains(&kw)
        })
        .collect();

    list.sort_by(|a, b| match f.sort {
        NodeSort::Recent => {
            let at = a.last_checked_at.as_deref().unwrap_or("");
            let bt = b.last_checked_at.as_deref().unwrap_or("");
            bt.cmp(at)
        }
        NodeSort::Name => a.name.cmp(&b.name),
    });
    list
}

/// 节点的未筛选统计（页头统计条）
#[derive(Debug, Clone, Serialize)]
pub struct NodeStats {
    pub total: usize,
    pub online: usize,
    pub offline: usize,
    /// 启用 fscache 的节点数
    pub fscache: usize,
    /// 被活跃运行配置占用的节点数
    pub busy: usize,
}

/// 计算节点的未筛选统计
pub fn node_stats(rows: &[NodeView]) -> NodeStats {
    NodeStats {
        total: rows.len(),
        online: rows.iter().filter(|n| n.status == "online").count(),
        offline: rows.iter().filter(|n| n.status == "offline").count(),
        fscache: rows.iter().filter(|n| n.fscache).count(),
        busy: rows.iter().filter(|n| n.busy).count(),
    }
}

/// 按 id 查询节点；**不存在返回 `None`**（不报 20702）。
///
/// 用于「历史数据可能引用已删除节点」的读路径降级（运行配置视图 / 接入配置 / 推理指标），
/// 见 `openspec/changes/modelinfer-dangling-node-reference/`。需要强制存在性的校验路径仍应使用
/// [`get_node`]。
pub async fn find_node(
    db: &DatabaseConnection,
    id: &str,
) -> Result<Option<node::Model>, ModelinferError> {
    node::Entity::find_by_id(id.to_string())
        .one(db)
        .await
        .map_err(db_err)
}

pub async fn get_node(db: &DatabaseConnection, id: &str) -> Result<node::Model, ModelinferError> {
    find_node(db, id)
        .await?
        .ok_or_else(|| ModelinferError::new(codes::NOT_FOUND, format!("节点不存在: {id}")))
}

/// 加载节点原始模型（含 password，供 SSH 编排使用）；`ids` 为 None 时返回全部
pub async fn load_models(
    db: &DatabaseConnection,
    ids: Option<&[String]>,
) -> Result<Vec<node::Model>, ModelinferError> {
    match ids {
        None => node::Entity::find()
            .order_by_asc(node::Column::CreatedAt)
            .all(db)
            .await
            .map_err(db_err),
        Some(list) => {
            let mut out = Vec::with_capacity(list.len());
            for id in list {
                out.push(get_node(db, id).await?);
            }
            Ok(out)
        }
    }
}

/// 创建节点：先 SSH 探活 + 扫描（服务端权威），成功后落库
pub async fn create_node(
    db: &DatabaseConnection,
    ssh: &dyn SshExec,
    req: NodeCreate,
) -> Result<NodeView, ModelinferError> {
    if req.name.trim().is_empty() || req.host.trim().is_empty() {
        return Err(ModelinferError::new(codes::PARAM_INVALID, "节点名称与主机地址不能为空"));
    }
    let ssh_user = normalize_ssh_user(&req.ssh_user)?;
    let auth_method = normalize_auth_method(&req.auth_method)?;
    validate_auth(&auth_method, &req.ssh_password, &req.ssh_key)?;
    validate_sudo_password(&ssh_user, &auth_method, &req.ssh_password, &req.sudo_password)?;
    validate_port(req.ssh_port)?;
    validate_monitor_port(req.node_exporter_port)?;
    validate_monitor_port(req.nvidia_gpu_exporter_port)?;

    let host = req.host.trim().to_string();
    let existing = node::Entity::find()
        .filter(node::Column::Host.eq(host.clone()))
        .one(db)
        .await
        .map_err(db_err)?;
    if existing.is_some() {
        return Err(ModelinferError::new(codes::EXISTS, format!("主机已存在: {host}")));
    }

    let target = SshTarget::with_auth(
        host.clone(),
        req.ssh_port as u16,
        ssh_user.clone(),
        SshAuth::from_parts(&auth_method, &req.ssh_password, &req.ssh_key),
    ).with_sudo_password(&req.sudo_password);
    ssh.test(&target).await?;
    // 连接并扫描（同时完成探活）；扫描失败即视为节点不可用
    let scan = scan_node(ssh, &target).await?;

    let hostname = req
        .hostname
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(scan.hostname);
    let socket_ifname = req
        .socket_ifname
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or(scan.default_socket_ifname)
        .unwrap_or_default();
    let ib_hca = req
        .ib_hca
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or(scan.suggested_ib_hca)
        .unwrap_or_default();

    let now = time::now_iso();
    let am = node::ActiveModel {
        id: Set(ulid::Ulid::new().to_string()),
        name: Set(req.name.trim().to_string()),
        host: Set(host),
        ssh_port: Set(req.ssh_port),
        ssh_user: Set(ssh_user.clone()),
        ssh_password: Set(if auth_method == "password" { req.ssh_password } else { String::new() }),
        ssh_key: Set(if auth_method == "key" { req.ssh_key } else { String::new() }),
        auth_method: Set(auth_method),
        sudo_password: Set(if ssh_user == "root" { String::new() } else { req.sudo_password }),
        hostname: Set(hostname.trim().to_string()),
        arch: Set(scan.arch.clone()),
        kernel_version: Set(scan.kernel_version.clone()),
        gateway: Set(scan.gateway.clone()),
        gpu_vendor: Set(scan.gpu.vendor.clone()),
        gpu_model: Set(scan.gpu.model_summary()),
        gpu_info_json: Set(
            serde_json::to_string(&scan.gpu.devices).unwrap_or_else(|_| "[]".into()),
        ),
        node_exporter_port: Set(req.node_exporter_port),
        nvidia_gpu_exporter_port: Set(req.nvidia_gpu_exporter_port),
        socket_ifname: Set(socket_ifname.trim().to_string()),
        ib_hca: Set(ib_hca.trim().to_string()),
        interfaces_json: Set(serde_json::to_string(&scan.interfaces).unwrap_or_else(|_| "[]".into())),
        ib_devices_json: Set(serde_json::to_string(&scan.ib_devices).unwrap_or_else(|_| "[]".into())),
        fscache: Set(req.fscache),
        status: Set("online".to_string()),
        last_checked_at: Set(Some(now.clone())),
        created_at: Set(now.clone()),
        updated_at: Set(now),
    };
    let inserted = am.insert(db).await.map_err(|e| {
        if is_unique_violation(&e) {
            ModelinferError::new(codes::EXISTS, "主机已存在")
        } else {
            db_err(e)
        }
    })?;
    Ok(NodeView::from(inserted))
}

pub async fn update_node(
    db: &DatabaseConnection,
    ssh: &dyn SshExec,
    id: &str,
    req: NodeUpdate,
) -> Result<NodeView, ModelinferError> {
    let current = get_node(db, id).await?;
    let mut am: node::ActiveModel = current.clone().into();
    if let Some(name) = req.name {
        if name.trim().is_empty() {
            return Err(ModelinferError::new(codes::PARAM_INVALID, "节点名称不能为空"));
        }
        am.name = Set(name.trim().to_string());
    }
    if let Some(host) = req.host {
        if host.trim().is_empty() {
            return Err(ModelinferError::new(codes::PARAM_INVALID, "主机地址不能为空"));
        }
        am.host = Set(host.trim().to_string());
    }
    if let Some(port) = req.ssh_port {
        validate_port(port)?;
        am.ssh_port = Set(port);
    }
    if let Some(user) = req.ssh_user {
        am.ssh_user = Set(normalize_ssh_user(&user)?);
    }
    // 认证方式 / 凭据：合并「当前值 + 请求覆盖」后整体校验
    let auth_method = match req.auth_method.as_deref() {
        Some(m) => normalize_auth_method(m)?,
        None => current.auth_method.clone(),
    };
    let password = req
        .ssh_password
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| current.ssh_password.clone());
    let key = req
        .ssh_key
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| current.ssh_key.clone());
    validate_auth(&auth_method, &password, &key)?;
    // 切换认证方式时清除不再使用的凭据（减少凭据足迹）
    let (password, key) = if auth_method == "key" {
        (String::new(), key)
    } else {
        (password, String::new())
    };
    am.auth_method = Set(auth_method);
    am.ssh_password = Set(password);
    am.ssh_key = Set(key);
    let user = am.ssh_user.as_ref();
    let sudo_password = if user == "root" {
        String::new()
    } else {
        req.sudo_password.unwrap_or_else(|| current.sudo_password.clone())
    };
    validate_sudo_password(user, am.auth_method.as_ref(), am.ssh_password.as_ref(), &sudo_password)?;
    am.sudo_password = Set(sudo_password);
    // 对合并后的有效配置做检查；仅修改显示名称等配置时无需远程连接。
    if am.host.as_ref() != &current.host || am.ssh_port.as_ref() != &current.ssh_port
        || am.ssh_user.as_ref() != &current.ssh_user || am.auth_method.as_ref() != &current.auth_method
        || am.ssh_password.as_ref() != &current.ssh_password || am.ssh_key.as_ref() != &current.ssh_key
        || am.sudo_password.as_ref() != &current.sudo_password {
        let target = SshTarget::from_credentials(
            am.host.as_ref().clone(), *am.ssh_port.as_ref() as u16, am.ssh_user.as_ref().clone(),
            am.auth_method.as_ref(), am.ssh_password.as_ref(), am.ssh_key.as_ref(),
        ).with_sudo_password(am.sudo_password.as_ref());
        ssh.test(&target).await?;
    }
    if let Some(h) = req.hostname {
        am.hostname = Set(h.trim().to_string());
    }
    if let Some(si) = req.socket_ifname {
        am.socket_ifname = Set(si.trim().to_string());
    }
    if let Some(ib) = req.ib_hca {
        am.ib_hca = Set(ib.trim().to_string());
    }
    if let Some(fscache) = req.fscache {
        am.fscache = Set(fscache);
    }
    if let Some(port) = req.node_exporter_port {
        validate_monitor_port(port)?;
        am.node_exporter_port = Set(port);
    }
    if let Some(port) = req.nvidia_gpu_exporter_port {
        validate_monitor_port(port)?;
        am.nvidia_gpu_exporter_port = Set(port);
    }
    am.updated_at = Set(time::now_iso());
    let updated = am.update(db).await.map_err(db_err)?;
    Ok(NodeView::from(updated))
}

/// 探测请求（仅连接信息，无需名称）
#[derive(Debug, Clone, Deserialize)]
pub struct NodeProbeReq {
    pub host: String,
    #[serde(default = "default_port")]
    pub ssh_port: i32,
    #[serde(default = "default_user")]
    pub ssh_user: String,
    /// 认证方式：`password`（缺省）/ `key`
    #[serde(default)]
    pub auth_method: String,
    #[serde(default)]
    pub ssh_password: String,
    #[serde(default)]
    pub ssh_key: String,
    #[serde(default)]
    pub sudo_password: String,
}

/// 扫描节点硬件（不落库）
pub async fn probe_node(
    ssh: &dyn SshExec,
    req: &NodeProbeReq,
) -> Result<NodeScanResult, ModelinferError> {
    if req.host.trim().is_empty() {
        return Err(ModelinferError::new(codes::PARAM_INVALID, "主机地址不能为空"));
    }
    let ssh_user = normalize_ssh_user(&req.ssh_user)?;
    let auth_method = normalize_auth_method(&req.auth_method)?;
    validate_auth(&auth_method, &req.ssh_password, &req.ssh_key)?;
    validate_sudo_password(&ssh_user, &auth_method, &req.ssh_password, &req.sudo_password)?;
    validate_port(req.ssh_port)?;
    let target = SshTarget::with_auth(
        req.host.trim().to_string(),
        req.ssh_port as u16,
        ssh_user,
        SshAuth::from_parts(&auth_method, &req.ssh_password, &req.ssh_key),
    ).with_sudo_password(&req.sudo_password);
    ssh.test(&target).await?;
    scan_node(ssh, &target).await
}

/// 重新扫描已有节点并更新硬件字段（不覆盖用户已选的 socket / ib）
pub async fn rescan_node(
    db: &DatabaseConnection,
    ssh: &dyn SshExec,
    id: &str,
) -> Result<NodeView, ModelinferError> {
    let current = get_node(db, id).await?;
    let target = ssh_target_of(&current);
    ssh.test(&target).await?;
    let scan = scan_node(ssh, &target).await?;
    let had_socket = !current.socket_ifname.trim().is_empty();
    let had_ib = !current.ib_hca.trim().is_empty();
    let mut am: node::ActiveModel = current.into();
    am.hostname = Set(scan.hostname);
    am.arch = Set(scan.arch);
    am.kernel_version = Set(scan.kernel_version);
    am.gateway = Set(scan.gateway);
    let gpu = scan.gpu;
    let gpu_model = gpu.model_summary();
    let gpu_info = serde_json::to_string(&gpu.devices).unwrap_or_else(|_| "[]".into());
    am.gpu_vendor = Set(gpu.vendor);
    am.gpu_model = Set(gpu_model);
    am.gpu_info_json = Set(gpu_info);
    am.interfaces_json = Set(serde_json::to_string(&scan.interfaces).unwrap_or_else(|_| "[]".into()));
    am.ib_devices_json = Set(serde_json::to_string(&scan.ib_devices).unwrap_or_else(|_| "[]".into()));
    if !had_socket && let Some(si) = scan.default_socket_ifname {
        am.socket_ifname = Set(si);
    }
    if !had_ib && let Some(ib) = scan.suggested_ib_hca {
        am.ib_hca = Set(ib);
    }
    am.status = Set("online".to_string());
    am.last_checked_at = Set(Some(time::now_iso()));
    am.updated_at = Set(time::now_iso());
    let updated = am.update(db).await.map_err(db_err)?;
    Ok(NodeView::from(updated))
}

pub async fn delete_node(db: &DatabaseConnection, id: &str) -> Result<(), ModelinferError> {
    let result = node::Entity::delete_by_id(id.to_string())
        .exec(db)
        .await
        .map_err(db_err)?;
    if result.rows_affected == 0 {
        return Err(ModelinferError::new(codes::NOT_FOUND, format!("节点不存在: {id}")));
    }
    Ok(())
}

/// 探活：更新 status / last_checked_at
pub async fn test_node(
    db: &DatabaseConnection,
    ssh: &dyn SshExec,
    id: &str,
) -> Result<NodeView, ModelinferError> {
    let current = get_node(db, id).await?;
    let target = ssh_target_of(&current);
    let now = time::now_iso();
    let (status, result) = match ssh.test(&target).await {
        Ok(()) => ("online", Ok(())),
        Err(e) => ("offline", Err(e)),
    };
    let mut am: node::ActiveModel = current.into();
    am.status = Set(status.to_string());
    am.last_checked_at = Set(Some(now));
    am.updated_at = Set(time::now_iso());
    let updated = am.update(db).await.map_err(db_err)?;
    match result {
        Ok(()) => Ok(NodeView::from(updated)),
        Err(e) => Err(e),
    }
}

pub(crate) fn db_err(e: sea_orm::DbErr) -> ModelinferError {
    ModelinferError::new(codes::DB_FAILED, format!("数据库操作失败: {e}"))
}

fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("unique") || s.contains("constraint")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::ssh::MockSshExec;

    fn sample(host: &str) -> NodeCreate {
        NodeCreate {
            name: format!("n-{host}"),
            host: host.to_string(),
            ssh_port: 22,
            ssh_user: "root".into(),
            auth_method: "password".into(),
            ssh_password: "ictrek".into(),
            ssh_key: String::new(),
            sudo_password: String::new(),
            hostname: Some(format!("h-{host}")),
            socket_ifname: Some("enp1s0".into()),
            ib_hca: Some(String::new()),
            fscache: false,
            node_exporter_port: DEFAULT_NODE_EXPORTER_PORT,
            nvidia_gpu_exporter_port: DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
        }
    }

    /// 默认监听端口必须避开上游标准端口（9100 / 9835），避免与节点自带监控栈冲突
    #[test]
    fn monitor_port_defaults_avoid_upstream_ports() {
        assert_eq!(DEFAULT_NODE_EXPORTER_PORT, 19100);
        assert_eq!(DEFAULT_NVIDIA_GPU_EXPORTER_PORT, 19835);
    }

    /// 请求未指定端口时 serde 默认值即新的非标准端口
    #[test]
    fn create_req_defaults_use_nonstandard_ports() {
        let req: NodeCreate =
            serde_json::from_str(r#"{"name":"n","host":"10.0.0.1","ssh_password":"ictrek"}"#).unwrap();
        assert_eq!(req.node_exporter_port, 19100);
        assert_eq!(req.nvidia_gpu_exporter_port, 19835);
    }

    #[tokio::test]
    async fn create_list_and_delete_roundtrip() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        let created = create_node(&db, &ssh, sample("10.0.0.1")).await.unwrap();
        assert_eq!(created.status, "online");
        assert!(created.has_password);
        assert!(!created.has_key);
        assert_eq!(created.auth_method, "password");
        assert_eq!(created.node_exporter_port, DEFAULT_NODE_EXPORTER_PORT);

        let list = list_nodes(&db).await.unwrap();
        assert_eq!(list.len(), 1);

        delete_node(&db, &created.id).await.unwrap();
        assert!(list_nodes(&db).await.unwrap().is_empty());
    }

    #[test]
    fn apply_occupancy_collects_all_active_deployments() {
        let mut views = vec![NodeView {
            id: "n1".into(),
            name: "node-1".into(),
            host: "10.0.0.1".into(),
            ssh_port: 22,
            ssh_user: "root".into(),
            auth_method: "password".into(),
            has_password: true,
            has_key: false,
            has_sudo_password: false,
            hostname: "h1".into(),
            arch: "amd64".into(),
            kernel_version: String::new(),
            gateway: String::new(),
            gpu_vendor: String::new(),
            gpu_model: String::new(),
            gpu_devices: Vec::new(),
            socket_ifname: String::new(),
            ib_hca: String::new(),
            interfaces: Vec::new(),
            ib_devices: Vec::new(),
            fscache: false,
            node_exporter_port: DEFAULT_NODE_EXPORTER_PORT,
            nvidia_gpu_exporter_port: DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
            status: "online".into(),
            busy: false,
            busy_deployment_id: String::new(),
            busy_deployment_name: String::new(),
            busy_deployment_ids: Vec::new(),
            busy_deployment_names: Vec::new(),
            last_checked_at: None,
            created_at: String::new(),
            updated_at: String::new(),
            monitor_state: String::new(),
            monitor_last_error: String::new(),
            monitor_deployed_at: None,
        }];
        let occ = vec![
            crate::deployments::OccupiedNode {
                node_id: "n1".into(),
                deployment_id: "A".into(),
                deployment_name: "alpha".into(),
            },
            crate::deployments::OccupiedNode {
                node_id: "n2".into(),
                deployment_id: "X".into(),
                deployment_name: "other".into(),
            },
            crate::deployments::OccupiedNode {
                node_id: "n1".into(),
                deployment_id: "B".into(),
                deployment_name: "beta".into(),
            },
        ];
        apply_occupancy(&mut views, &occ);
        let v = &views[0];
        assert!(v.busy);
        assert_eq!(v.busy_deployment_id, "A");
        assert_eq!(v.busy_deployment_name, "alpha");
        assert_eq!(v.busy_deployment_ids, vec!["A", "B"]);
        assert_eq!(v.busy_deployment_names, vec!["alpha", "beta"]);
    }

    #[tokio::test]
    async fn find_node_optional_lookup() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let created = create_node(&db, &MockSshExec::new(), sample("10.0.0.3")).await.unwrap();

        let found = find_node(&db, &created.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().host, "10.0.0.3");

        // 读路径降级：未命中返回 None
        assert!(find_node(&db, "not-exist").await.unwrap().is_none());
        // 强制存在路径仍返回 20702
        let err = get_node(&db, "not-exist").await.unwrap_err();
        assert_eq!(err.code, codes::NOT_FOUND);
    }

    #[tokio::test]
    async fn create_with_private_key() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        let mut req = sample("10.0.0.21");
        req.auth_method = "key".into();
        req.ssh_password = String::new();
        req.ssh_key = "-----BEGIN OPENSSH PRIVATE KEY-----\nxxx\n-----END OPENSSH PRIVATE KEY-----".into();
        let created = create_node(&db, &ssh, req).await.unwrap();
        assert_eq!(created.auth_method, "key");
        assert!(created.has_key);
        assert!(!created.has_password);
        // Mock 记录到使用了私钥认证（每次 SSH 调用均为 key，且不含凭据明文）
        let auth = ssh.recorded_auth();
        assert!(!auth.is_empty() && auth.iter().all(|a| a == "key"), "auth={auth:?}");
    }

    #[tokio::test]
    async fn sudo_users_support_password_and_key_without_exposing_credentials() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        for (host, method) in [("10.0.0.31", "password"), ("10.0.0.32", "key")] {
            let ssh = MockSshExec::new();
            let mut req = sample(host);
            req.ssh_user = " admin ".into();
            req.auth_method = method.into();
            req.ssh_key = "PRIVATE_SECRET".into();
            req.sudo_password = "SUDO_SECRET".into();
            let node = create_node(&db, &ssh, req).await.unwrap();
            assert_eq!(node.ssh_user, "admin");
            assert!(node.has_sudo_password);
            let json = serde_json::to_string(&node).unwrap();
            assert!(!json.contains("SECRET"));
            assert!(!json.contains("\"sudo_password\""));
            let model = get_node(&db, &node.id).await.unwrap();
            let target = ssh_target_of(&model);
            assert_eq!(target.user, "admin");
            assert_eq!(target.sudo_password, "SUDO_SECRET");
            assert_eq!(target.auth.kind(), method);
            assert!(ssh.recorded_calls()[0].ends_with(crate::ssh::ROOT_CHECK));
        }
    }

    #[tokio::test]
    async fn sudo_denial_stops_probe_and_create_before_scan_or_insert() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new().with_rule(crate::ssh::ROOT_CHECK, 1, "");
        let mut req = sample("10.0.0.33");
        req.ssh_user = "admin".into();
        assert_eq!(create_node(&db, &ssh, req).await.unwrap_err().code, codes::NODE_SUDO_FAILED);
        let probe: NodeProbeReq = serde_json::from_value(serde_json::json!({
            "host": "10.0.0.33", "ssh_user": "admin", "ssh_password": "login"
        })).unwrap();
        assert_eq!(probe_node(&ssh, &probe).await.unwrap_err().code, codes::NODE_SUDO_FAILED);
        assert!(list_nodes(&db).await.unwrap().is_empty());
        assert_eq!(ssh.recorded_calls().len(), 2);
        assert!(ssh.recorded_calls().iter().all(|c| c.ends_with(crate::ssh::ROOT_CHECK)));
    }

    #[tokio::test]
    async fn sudo_update_checks_new_credentials_and_preserves_old_values_on_failure() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        let mut req = sample("10.0.0.34");
        req.ssh_user = "admin".into();
        req.sudo_password = "old-sudo".into();
        let created = create_node(&db, &ssh, req).await.unwrap();
        let denied = MockSshExec::new().with_rule(crate::ssh::ROOT_CHECK, 1, "");
        let update = serde_json::from_value(serde_json::json!({"sudo_password": "wrong"})).unwrap();
        assert_eq!(update_node(&db, &denied, &created.id, update).await.unwrap_err().code, codes::NODE_SUDO_FAILED);
        assert_eq!(get_node(&db, &created.id).await.unwrap().sudo_password, "old-sudo");
        let rename = serde_json::from_value(serde_json::json!({"name": "renamed"})).unwrap();
        update_node(&db, &denied, &created.id, rename).await.unwrap();
        assert_eq!(denied.recorded_calls().len(), 1, "仅改名称不连接远端");
        let clear = serde_json::from_value(serde_json::json!({"sudo_password": ""})).unwrap();
        let updated = update_node(&db, &ssh, &created.id, clear).await.unwrap();
        assert!(!updated.has_sudo_password);
    }

    #[tokio::test]
    async fn auth_validation_rejects_mismatch() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        // key 但无私钥
        let mut req = sample("10.0.0.22");
        req.auth_method = "key".into();
        req.ssh_key = String::new();
        assert_eq!(create_node(&db, &ssh, req).await.unwrap_err().code, codes::NODE_AUTH_INVALID);
        // 非法认证方式
        let mut req = sample("10.0.0.23");
        req.auth_method = "kerberos".into();
        assert_eq!(create_node(&db, &ssh, req).await.unwrap_err().code, codes::NODE_AUTH_INVALID);
        // 用户名非法
        let mut req = sample("10.0.0.24");
        req.ssh_user = "bad user".into();
        assert_eq!(create_node(&db, &ssh, req).await.unwrap_err().code, codes::NODE_AUTH_INVALID);
        assert!(list_nodes(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fscache_defaults_false_and_can_be_enabled() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        // 缺省：不启用
        let plain = create_node(&db, &ssh, sample("10.0.0.11")).await.unwrap();
        assert!(!plain.fscache);
        // 显式启用
        let mut req = sample("10.0.0.12");
        req.fscache = true;
        let enabled = create_node(&db, &ssh, req).await.unwrap();
        assert!(enabled.fscache);
        // 回读一致
        let listed = list_nodes(&db).await.unwrap();
        assert_eq!(listed.iter().filter(|n| n.fscache).count(), 1);
    }

    #[tokio::test]
    async fn update_toggles_fscache_and_monitor_port() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        let mut req = sample("10.0.0.13");
        req.fscache = true;
        let created = create_node(&db, &ssh, req).await.unwrap();
        assert!(created.fscache);

        // 未提供 → 不变
        let untouched = update_node(
            &db,
            &ssh,
            &created.id,
            NodeUpdate {
                name: None,
                host: None,
                ssh_port: None,
                ssh_user: None,
                auth_method: None,
                ssh_password: None,
                ssh_key: None, sudo_password: None,
                hostname: None,
                socket_ifname: None,
                ib_hca: None,
                fscache: None,
                node_exporter_port: None,
                nvidia_gpu_exporter_port: None,
            },
        )
        .await
        .unwrap();
        assert!(untouched.fscache);
        assert_eq!(untouched.node_exporter_port, DEFAULT_NODE_EXPORTER_PORT);

        // 关闭 fscache + 改端口
        let off = update_node(
            &db,
            &ssh,
            &created.id,
            NodeUpdate {
                name: None,
                host: None,
                ssh_port: None,
                ssh_user: None,
                auth_method: None,
                ssh_password: None,
                ssh_key: None, sudo_password: None,
                hostname: None,
                socket_ifname: None,
                ib_hca: None,
                fscache: Some(false),
                node_exporter_port: Some(9110),
                nvidia_gpu_exporter_port: None,
            },
        )
        .await
        .unwrap();
        assert!(!off.fscache);
        assert_eq!(off.node_exporter_port, 9110);
    }

    #[tokio::test]
    async fn update_switch_auth_method_requires_credentials() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        let created = create_node(&db, &ssh, sample("10.0.0.14")).await.unwrap();
        // 切到 key 但未提供私钥 → 拒绝
        let err = update_node(
            &db,
            &ssh,
            &created.id,
            NodeUpdate {
                name: None, host: None, ssh_port: None, ssh_user: None,
                auth_method: Some("key".into()), ssh_password: None, ssh_key: None, sudo_password: None,
                hostname: None, socket_ifname: None, ib_hca: None,
                fscache: None, node_exporter_port: None, nvidia_gpu_exporter_port: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::NODE_AUTH_INVALID);
        // 提供私钥后成功，密码清空
        let updated = update_node(
            &db,
            &ssh,
            &created.id,
            NodeUpdate {
                name: None, host: None, ssh_port: None, ssh_user: None,
                auth_method: Some("key".into()), ssh_password: None,
                ssh_key: Some("KEYDATA".into()), sudo_password: None,
                hostname: None, socket_ifname: None, ib_hca: None,
                fscache: None, node_exporter_port: None, nvidia_gpu_exporter_port: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.auth_method, "key");
        assert!(updated.has_key);
        assert!(!updated.has_password, "切换到 key 后旧密码应被清除");
    }

    #[tokio::test]
    async fn monitor_port_bounds() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        let mut req = sample("10.0.0.15");
        req.node_exporter_port = 80;
        assert_eq!(create_node(&db, &ssh, req).await.unwrap_err().code, codes::PARAM_INVALID);
    }

    #[tokio::test]
    async fn duplicate_host_rejected() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        create_node(&db, &ssh, sample("10.0.0.2")).await.unwrap();
        let err = create_node(&db, &ssh, sample("10.0.0.2")).await.unwrap_err();
        assert_eq!(err.code, codes::EXISTS);
    }

    #[tokio::test]
    async fn ssh_failure_keeps_node_absent() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new().with_connect_failure();
        let err = create_node(&db, &ssh, sample("10.0.0.3")).await.unwrap_err();
        assert_eq!(err.code, codes::SSH_CONNECT_FAILED);
        assert!(list_nodes(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_port_rejected() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new();
        let mut req = sample("10.0.0.4");
        req.ssh_port = 70000;
        let err = create_node(&db, &ssh, req).await.unwrap_err();
        assert_eq!(err.code, codes::PARAM_INVALID);
    }

    #[tokio::test]
    async fn probe_and_rescan_with_mock_ssh() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let ssh = MockSshExec::new()
            .with_rule("hostname", 0, "node-21\n")
            .with_rule("uname -m", 0, "x86_64\n")
            .with_rule("uname -r", 0, "5.15.0\n")
            .with_rule("ip -o -4", 0, "2: enp1s0    inet 10.100.10.21/24 scope global enp1s0\n")
            .with_rule("infiniband", 0, "")
            .with_rule("route show default", 0, "default via 10.100.10.1 dev enp1s0\n")
            .with_rule("nvidia-smi", 0, "0, NVIDIA L40, 46068, 550.1, 0000:01:00.0\n");

        // probe
        let scan = probe_node(
            &ssh,
            &NodeProbeReq {
                host: "10.100.10.21".into(),
                ssh_port: 22,
                ssh_user: "root".into(),
                auth_method: "password".into(),
                ssh_password: "ictrek".into(),
                ssh_key: String::new(),
                sudo_password: String::new(),
            },
        )
        .await
        .unwrap();
        assert_eq!(scan.hostname, "node-21");
        assert_eq!(scan.default_socket_ifname.as_deref(), Some("enp1s0"));
        assert_eq!(scan.gpu.vendor, "nvidia");

        // create + rescan
        let created = create_node(&db, &ssh, sample("10.100.10.21")).await.unwrap();
        assert_eq!(created.hostname, "h-10.100.10.21");
        assert_eq!(created.arch, "x86_64");
        assert_eq!(created.gpu_model, "NVIDIA L40");
        let rescanned = rescan_node(&db, &ssh, &created.id).await.unwrap();
        assert_eq!(rescanned.hostname, "node-21");
        assert_eq!(rescanned.socket_ifname, "enp1s0");
        assert_eq!(rescanned.gateway, "10.100.10.1");
        assert!(rescanned.ib_devices.is_empty());
    }
}
