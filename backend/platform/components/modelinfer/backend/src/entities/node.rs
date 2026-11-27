//! 节点实体：`modelinfer_nodes`

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_nodes")]
pub struct Model {
    /// 节点 ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 节点名称
    pub name: String,
    pub profile_override: Option<String>,
    pub detected_profile: Option<String>,
    /// 主机地址（IP / hostname），全局唯一
    #[sea_orm(unique)]
    pub host: String,
    /// 访问模式：`ssh`（默认）/ `local`（本机 nsenter 免密执行）
    #[sea_orm(default_value = "ssh")]
    pub access_mode: String,
    /// SSH 端口，默认 22（local 模式不使用）
    #[sea_orm(default_value = 22)]
    pub ssh_port: i32,
    /// SSH 用户（root 或具有 sudo 权限的普通用户）
    #[sea_orm(default_value = "root")]
    pub ssh_user: String,
    /// SSH 认证方式：`password` / `key`
    #[sea_orm(default_value = "password")]
    pub auth_method: String,
    /// SSH 密码（明文存储，安全强化列入后续阶段；`auth_method=key` 时为空）
    #[sea_orm(default_value = "")]
    pub ssh_password: String,
    /// SSH 私钥内容（OpenSSH / PEM；`auth_method=password` 时为空；只入不出）
    #[sea_orm(default_value = "")]
    pub ssh_key: String,
    /// sudo 密码（只入不出；空值复用 SSH 密码或使用免密 sudo）
    #[sea_orm(default_value = "")]
    pub sudo_password: String,
    /// 节点主机名（扫描得到，可手动覆盖）
    #[sea_orm(default_value = "")]
    pub hostname: String,
    /// CPU 架构（`uname -m`）
    #[sea_orm(default_value = "")]
    pub arch: String,
    /// 内核版本（`uname -r`）
    #[sea_orm(default_value = "")]
    pub kernel_version: String,
    /// 默认网关 IP
    #[sea_orm(default_value = "")]
    pub gateway: String,
    /// GPU 厂商（`nvidia` / `amd` / `intel`；未识别为空）
    #[sea_orm(default_value = "")]
    pub gpu_vendor: String,
    /// GPU 型号汇总（单卡 name / 多卡 `<name> x N`）
    #[sea_orm(default_value = "")]
    pub gpu_model: String,
    /// 全部 GPU 设备（JSON 数组：[{index,name,memory_total_mib,pci_bus_id}]）
    #[sea_orm(default_value = "[]")]
    pub gpu_info_json: String,
    /// node-exporter 监听端口（节点高级配置；部署监控组件时使用，默认 19100）
    #[sea_orm(default_value = 19100)]
    pub node_exporter_port: i32,
    /// nvidia-gpu-exporter 监听端口（节点高级配置；部署 GPU 监控组件时使用，默认 19835）
    #[sea_orm(default_value = 19835)]
    pub nvidia_gpu_exporter_port: i32,
    /// 控制面网卡名（SOCKET_IFNAME 来源）
    #[sea_orm(default_value = "")]
    pub socket_ifname: String,
    /// IB 设备名（可为空）
    #[sea_orm(default_value = "")]
    pub ib_hca: String,
    /// 全部 IPv4 网卡：[{"name":"enp1s0","ips":["10.100.10.21"]}]
    #[sea_orm(default_value = "[]")]
    pub interfaces_json: String,
    /// 全部 IB 设备名（JSON 数组）
    #[sea_orm(default_value = "[]")]
    pub ib_devices_json: String,
    /// 挂载 model-hub NFS 时是否启用 fscache（节点侧 cachefilesd 由用户负责）
    #[sea_orm(default_value = false)]
    pub fscache: bool,
    /// Whether app backends may mount the Model Hub share on this node.
    #[sea_orm(default_value = true)]
    pub app_model_hub_nfs: bool,
    /// 连接状态：unknown / online / offline
    #[sea_orm(default_value = "unknown")]
    pub status: String,
    /// 最近一次探活时间（CST ISO-8601）
    pub last_checked_at: Option<String>,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
    /// 更新时间（CST ISO-8601）
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
