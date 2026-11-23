//! etcd key 拼接纯函数（可独立单测，无副作用）
//!
//! 布局承接旧方案（`vos/docs/cluster-management.md` §2.4）：
//! - `/vos/cluster/info`          集群基本信息
//! - `/vos/cluster/nodes/<id>`    节点记录（NodeMetadata + NodeCredentials）
//! - `/vos/cluster/pg_credentials` postgres-ha 集群凭据（bootstrap 写入 / join 读取）
//! - `/vos/cluster/infra_status`  基础设施就绪位

/// 集群信息 key
pub const KEY_CLUSTER_INFO: &str = "/vos/cluster/info";
/// 节点记录 key 前缀（其后接 member_id）
pub const KEY_NODE_PREFIX: &str = "/vos/cluster/nodes/";
/// postgres-ha 集群凭据 key
pub const KEY_PG_CREDENTIALS: &str = "/vos/cluster/pg_credentials";
/// 基础设施就绪位 key
pub const KEY_INFRA_STATUS: &str = "/vos/cluster/infra_status";

/// 集群信息 key
pub fn cluster_info_key() -> &'static str {
    KEY_CLUSTER_INFO
}

/// 节点记录 key：`/vos/cluster/nodes/<member_id>`
pub fn node_key(member_id: &str) -> String {
    format!("{KEY_NODE_PREFIX}{member_id}")
}

/// 基础设施就绪位 key
pub fn infra_status_key() -> &'static str {
    KEY_INFRA_STATUS
}

/// postgres-ha 集群凭据 key
pub fn pg_credentials_key() -> &'static str {
    KEY_PG_CREDENTIALS
}

/// 全局 VRRP VIP → ID key 前缀
pub const KEY_VRRP_VIPS_PREFIX: &str = "/vos/cluster/vrrp/vips/";
/// 全局 VRRP ID → VIP key 前缀
pub const KEY_VRRP_IDS_PREFIX: &str = "/vos/cluster/vrrp/ids/";

/// VIP → VRRP 记录 key
pub fn vrrp_vip_key(vip: &str) -> String {
    format!("{KEY_VRRP_VIPS_PREFIX}{vip}")
}

/// VRRP ID → VIP 记录 key
pub fn vrrp_id_key(id: u8) -> String {
    format!("{KEY_VRRP_IDS_PREFIX}{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_follow_layout() {
        assert_eq!(cluster_info_key(), "/vos/cluster/info");
        assert_eq!(node_key("abc-123"), "/vos/cluster/nodes/abc-123");
        assert_eq!(infra_status_key(), "/vos/cluster/infra_status");
        assert_eq!(pg_credentials_key(), "/vos/cluster/pg_credentials");
        // 前缀语义：同前缀批量扫描（get_all_node_records 用）
        assert!(node_key("x").starts_with(KEY_NODE_PREFIX));
    }

    #[test]
    fn node_key_roundtrip_prefix() {
        for id in ["1", "uuid-42", "member-a"] {
            let k = node_key(id);
            assert!(k.strip_prefix(KEY_NODE_PREFIX).unwrap() == id);
        }
    }
}