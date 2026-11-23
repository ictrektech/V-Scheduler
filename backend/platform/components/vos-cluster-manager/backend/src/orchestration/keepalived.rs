//! keepalived 编排纯函数（可独立测试，无副作用）
//!
//! SMB/NFS VIP 从 HAProxy 数据面移出后，由网关节点上的 keepalived 直接持有 VIP：
//! - 同一 VIP 在 `gateway_nodes` 之间 active/backup 漂移；
//! - `vrrp_script` 调用本机 VCM `/api/gateway/health?service=smb|nfs`，drain/端口未监听
//!   时优先降低，VIP 自动切走；
//! - 实际配置渲染在 keepalived 镜像内完成（需要 `iproute2` 自动识别网卡），
//!   本模块只负责稳定的 compose 名与容器 env 契约。

use std::collections::HashMap;

/// 默认 VRRP router id（0-255；同一 L2 内不同 VIP 组不能重复）
pub const DEFAULT_VRRP_ID: u8 = 51;
/// 默认 VIP 前缀（与节点管理网一致，通常 /24）
pub const DEFAULT_VIP_PREFIX: u8 = 24;
/// keepalived 检查脚本访问的本机 VCM 基址
pub const HEALTH_URL_BASE: &str =
    "http://127.0.0.1:19906/app/com.ictrek.vos-cluster-manager";
/// keepalived project 实例名前缀
pub const COMPOSE_PREFIX: &str = "cluster-keepalived-";

/// keepalived 拓扑（编排方按网关集合生成）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepalivedTopology {
    /// 本节点管理 IP（同时作为 `unicast_src_ip`）
    pub node_ip: String,
    /// 浮动 VIP
    pub vip: String,
    /// VIP 前缀长度
    pub vip_prefix: u8,
    /// 其余 VRRP 对端（不含本节点）
    pub peers: Vec<String>,
    /// VRRP 优先级（越大越优先）
    pub priority: u8,
    /// VRRP router id
    pub vrrp_id: u8,
    /// 显式网卡名；空字符串表示由镜像内 `iproute2` 自动识别
    pub interface: String,
    /// 本节点启用的网关服务（`smb` / `nfs`）
    pub services: Vec<String>,
    /// 可选 RGW VIP（与文件网关 VIP 独立；None = 不渲染 RGW 实例）
    pub rgw_vip: Option<String>,
    /// RGW VIP 前缀长度
    pub rgw_vip_prefix: u8,
    /// RGW VRRP 对端（空则回退文件网关 peers）
    pub rgw_peers: Vec<String>,
    /// RGW VRRP 优先级
    pub rgw_priority: u8,
    /// RGW VRRP ID（必须与文件网关不同）
    pub rgw_vrrp_id: u8,
    /// RGW 健康检查 URL（默认本机 HAProxy RGW listener）
    pub rgw_health_url: String,
}

fn ip_slug(ip: &str) -> Option<String> {
    let trimmed = ip.trim();
    if trimmed.is_empty()
        || !trimmed.chars().all(|c| c.is_ascii_digit() || c == '.')
        || trimmed.split('.').count() != 4
    {
        return None;
    }
    Some(trimmed.replace('.', "-"))
}

/// 生成 keepalived 子项目 compose_project_name：`cluster-keepalived-<ip-slug>`
pub fn keepalived_compose_name(node_ip: &str) -> Option<String> {
    ip_slug(node_ip).map(|slug| format!("{COMPOSE_PREFIX}{slug}"))
}

fn join(values: &[String]) -> String {
    values.iter().map(|s| s.trim()).collect::<Vec<_>>().join(",")
}

/// 构造 keepalived 容器 env（compose 全文本插值；镜像内 entrypoint 渲染配置）
pub fn build_keepalived_env_vars(topo: &KeepalivedTopology) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("KEEPALIVED_NODE_IP".into(), topo.node_ip.trim().into());
    env.insert("KEEPALIVED_VIP".into(), topo.vip.trim().into());
    env.insert(
        "KEEPALIVED_VIP_PREFIX".into(),
        topo.vip_prefix.to_string(),
    );
    env.insert("KEEPALIVED_PEERS".into(), join(&topo.peers));
    env.insert("KEEPALIVED_PRIORITY".into(), topo.priority.to_string());
    env.insert("KEEPALIVED_VRRP_ID".into(), topo.vrrp_id.to_string());
    env.insert(
        "KEEPALIVED_INTERFACE".into(),
        topo.interface.trim().into(),
    );
    env.insert("KEEPALIVED_SERVICES".into(), join(&topo.services));
    env.insert("KEEPALIVED_HEALTH_URL_BASE".into(), HEALTH_URL_BASE.into());
    if let Some(vip) = topo.rgw_vip.as_deref().filter(|v| !v.trim().is_empty()) {
        env.insert("KEEPALIVED_RGW_VIP".into(), vip.trim().into());
        env.insert("KEEPALIVED_RGW_VIP_PREFIX".into(), topo.rgw_vip_prefix.to_string());
        env.insert(
            "KEEPALIVED_RGW_PEERS".into(),
            join(if topo.rgw_peers.is_empty() { &topo.peers } else { &topo.rgw_peers }),
        );
        env.insert("KEEPALIVED_RGW_PRIORITY".into(), topo.rgw_priority.to_string());
        env.insert("KEEPALIVED_RGW_VRRP_ID".into(), topo.rgw_vrrp_id.to_string());
        env.insert("KEEPALIVED_RGW_HEALTH_URL".into(), topo.rgw_health_url.trim().into());
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topo() -> KeepalivedTopology {
        KeepalivedTopology {
            node_ip: "192.168.100.231".into(),
            vip: "192.168.100.240".into(),
            vip_prefix: 24,
            peers: vec!["192.168.100.232".into()],
            priority: 100,
            vrrp_id: 51,
            interface: "ens18".into(),
            services: vec!["smb".into()],
            rgw_vip: None,
            rgw_vip_prefix: 24,
            rgw_peers: Vec::new(),
            rgw_priority: 100,
            rgw_vrrp_id: 52,
            rgw_health_url: "http://127.0.0.1:7480/".into(),
        }
    }

    #[test]
    fn compose_name_shape() {
        assert_eq!(
            keepalived_compose_name("192.168.100.231").as_deref(),
            Some("cluster-keepalived-192-168-100-231")
        );
        assert!(keepalived_compose_name("not-an-ip").is_none());
    }

    #[test]
    fn env_contract() {
        let env = build_keepalived_env_vars(&topo());
        assert_eq!(env["KEEPALIVED_NODE_IP"], "192.168.100.231");
        assert_eq!(env["KEEPALIVED_VIP"], "192.168.100.240");
        assert_eq!(env["KEEPALIVED_VIP_PREFIX"], "24");
        assert_eq!(env["KEEPALIVED_PEERS"], "192.168.100.232");
        assert_eq!(env["KEEPALIVED_PRIORITY"], "100");
        assert_eq!(env["KEEPALIVED_VRRP_ID"], "51");
        assert_eq!(env["KEEPALIVED_SERVICES"], "smb");
    }

    #[test]
    fn rgw_env_is_independent_from_file_vip() {
        let mut t = topo();
        t.rgw_vip = Some("192.168.100.241".into());
        t.rgw_vrrp_id = 52;
        t.rgw_peers = vec!["192.168.100.232".into()];
        let env = build_keepalived_env_vars(&t);
        assert_eq!(env["KEEPALIVED_VIP"], "192.168.100.240");
        assert_eq!(env["KEEPALIVED_VRRP_ID"], "51");
        assert_eq!(env["KEEPALIVED_RGW_VIP"], "192.168.100.241");
        assert_eq!(env["KEEPALIVED_RGW_VRRP_ID"], "52");
        assert_eq!(env["KEEPALIVED_RGW_PEERS"], "192.168.100.232");
    }

    #[test]
    fn empty_peers_and_interface_are_safe() {
        let mut t = topo();
        t.peers.clear();
        t.interface.clear();
        let env = build_keepalived_env_vars(&t);
        assert_eq!(env["KEEPALIVED_PEERS"], "");
        assert_eq!(env["KEEPALIVED_INTERFACE"], "");
    }
}