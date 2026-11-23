//! etcd 集群状态 DTO（D-VCM-02：/vos/cluster/* 为集群状态唯一权威信源）
//!
//! key 布局承接旧方案（vos/docs/cluster-management.md §2.4）：
//! - `/vos/cluster/info`            ClusterInfo（集群基本信息）
//! - `/vos/cluster/nodes/<id>`      NodeRecord（NodeMetadata + NodeCredentials 合并，明文内网可信）
//! - `/vos/cluster/pg_credentials`  PgCredentials（postgres-ha 超级用户 / 复制用户密码，bootstrap 写入）
//! - `/vos/cluster/infra_status`    InfraStatus（etcd / postgres-ha 就绪位）

use serde::{Deserialize, Serialize};

/// 集群状态常量
pub mod status {
    /// 已注册（集群信息 + 本节点自我注册完成，Phase 1 达成）
    pub const INITIALIZED: &str = "initialized";
    /// 编排完成（etcd raft + postgres-ha 子项目全部就绪，Phase 4 写入）
    pub const CREATED: &str = "created";
}

/// postgres-ha 集群凭据（`/vos/cluster/pg_credentials`，Phase 4 内网可信）
///
/// bootstrap 节点生成并写入，join 节点读取复用——保证 replicator 密码一致
/// （否则 join 节点 basebackup 时 "password authentication failed for user
/// replicator"）；明文存 etcd 见 D-VCM-02 风险声明，后续加密/轮换。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PgCredentials {
    pub superuser_username: String,
    pub superuser_password: String,
    pub replication_username: String,
    pub replication_password: String,
    /// RFC3339 写入时间（join 侧校验新鲜度预留）
    pub created_at: String,
}

/// 节点角色常量（cluster-role-and-create-entry：master/node → manager/worker）
pub mod role {
    /// 管理节点（运行 etcd + ha postgres：基于 raft 与 postgres 数据同步维护集群
    /// 高可用状态；本身是特殊 worker，其上也可运行其他集群化工作负载）
    pub const MANAGER: &str = "manager";
    /// 工作节点（不运行 etcd 与 ha postgres，可运行其他集群化工作负载）
    pub const WORKER: &str = "worker";

    /// 旧值（一期 master/node）归一化为新枚举；未知值原样返回。
    /// 仅用于**视图层展示**（NodeRecordView），不重写 etcd 上的持久化值。
    pub fn normalize_legacy(role: &str) -> String {
        match role {
            "master" => MANAGER.to_string(),
            "node" => WORKER.to_string(),
            other => other.to_string(),
        }
    }
}

/// 集群基本信息（`/vos/cluster/info`）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClusterInfo {
    /// 集群 UUID（POST /api/cluster/init 生成）
    pub cluster_id: String,
    pub cluster_name: String,
    /// [`status::INITIALIZED`] / [`status::CREATED`]
    pub status: String,
    /// RFC3339（Asia/Shanghai 约定，边界转换见 roadmap §8）
    pub created_at: String,
}

/// 节点业务元数据（对外展示，无敏感信息）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeMetadata {
    /// 逻辑成员 ID（UUID；Phase 2 起编排方再关联 raft member id）
    pub member_id: String,
    /// 显示名称（用户可读标签）
    pub display_name: String,
    /// [`role::MANAGER`] / [`role::WORKER`]（旧值 master/node 读取时视图归一化）
    pub role: String,
    /// 节点 IP（集群成员互连地址）
    pub node_ip: String,
    /// 节点应用 API 端点（如 `http://<ip>:19906/app/com.ictrek.vos-cluster-manager/`）
    pub api_endpoint: String,
    /// RFC3339
    pub joined_at: String,
}

/// 节点凭据（明文持久化——仅内网可信；日志 / 响应不回显；后续 etcd RBAC /
/// 信封加密 / 短凭据轮换见 roadmap §后续安全强化）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeCredentials {
    /// 节点管理员密码（仅用于节点授权 / 编排期 SSH/HTTP 凭据校验）
    pub admin_password: String,
    /// postgres-ha 超级用户密码（Phase 3 编排写入；集群未部署 pg 前为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pg_superuser_password: Option<String>,
    /// postgres-ha 复制用户密码（Phase 3 编排写入）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pg_replication_password: Option<String>,
    /// RFC3339
    pub updated_at: String,
}

/// 节点记录（元数据 + 凭据合并，存同一 key 保证原子读写）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeRecord {
    pub metadata: NodeMetadata,
    pub credentials: NodeCredentials,
}

/// 基础设施就绪位（`/vos/cluster/infra_status`；一期仅骨架，Phase 2/3 写入真实值）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InfraStatus {
    /// etcd raft 集群就绪（Phase 2 写入）
    pub etcd_ok: bool,
    /// postgres-ha（patroni）就绪（Phase 3 写入）
    pub postgres_ha_ok: bool,
    /// 各节点 haproxy 统一入口就绪（二期 S1；create finalize 写入）。
    /// `#[serde(default)]`：etcd 已持久化的旧 JSON 无此字段 → 反序列化为 false 兼容（RC-D5）
    #[serde(default)]
    pub haproxy_ok: bool,
    /// 集群 temporal 就绪（二期 S2；create finalize 写入，serde default 兼容同上）
    #[serde(default)]
    pub temporal_ok: bool,
    /// RFC3339
    pub updated_at: String,
}

/// 节点脱敏视图（API 响应用——**不含 admin_password**，凭据不回显）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeRecordView {
    pub member_id: String,
    pub display_name: String,
    pub role: String,
    pub node_ip: String,
    pub api_endpoint: String,
    pub joined_at: String,
}

impl From<&NodeRecord> for NodeRecordView {
    fn from(rec: &NodeRecord) -> Self {
        let m = &rec.metadata;
        Self {
            member_id: m.member_id.clone(),
            display_name: m.display_name.clone(),
            // 旧值（master/node）视图归一化为 manager/worker（D1）
            role: role::normalize_legacy(&m.role),
            node_ip: m.node_ip.clone(),
            api_endpoint: m.api_endpoint.clone(),
            joined_at: m.joined_at.clone(),
        }
    }
}


/// 集群全局 VRRP 分配记录（etcd 权威视图；本地 DB 仅缓存/审计）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VrrpAllocation {
    pub vip: String,
    pub vrrp_id: u8,
    pub owner: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> NodeRecord {
        NodeRecord {
            metadata: NodeMetadata {
                member_id: "n1".into(),
                display_name: "节点A".into(),
                role: role::MANAGER.into(),
                node_ip: "10.0.0.1".into(),
                api_endpoint: "http://10.0.0.1:19906/app/".into(),
                joined_at: "2026-09-06T08:00:00+08:00".into(),
            },
            credentials: NodeCredentials {
                admin_password: "S3cret!".into(),
                updated_at: "2026-09-06T08:00:00+08:00".into(),
                pg_superuser_password: None,
                pg_replication_password: None,
            },
        }
    }

    #[test]
    fn cluster_info_serde_roundtrip() {
        let info = ClusterInfo {
            cluster_id: "c1".into(),
            cluster_name: "测试集群".into(),
            status: status::INITIALIZED.into(),
            created_at: "2026-09-06T08:00:00+08:00".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: ClusterInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn node_record_serde_roundtrip_keeps_credentials() {
        // etcd 上保存的 NodeRecord 必须完整含凭据（JSON 字段往返一致）
        let rec = sample_record();
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("admin_password"));
        let back: NodeRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
    }

    #[test]
    fn view_strips_admin_password() {
        let rec = sample_record();
        let view: NodeRecordView = (&rec).into();
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("admin_password"), "视图不得含密码字段: {json}");
        assert!(!json.contains("S3cret"), "视图不得含密码值: {json}");
        assert_eq!(view.node_ip, "10.0.0.1");
        assert_eq!(view.role, role::MANAGER);
    }

    #[test]
    fn normalize_legacy_maps_old_roles() {
        assert_eq!(role::normalize_legacy("master"), role::MANAGER);
        assert_eq!(role::normalize_legacy("node"), role::WORKER);
        // 新枚举原样；未知值原样
        assert_eq!(role::normalize_legacy("manager"), role::MANAGER);
        assert_eq!(role::normalize_legacy("worker"), role::WORKER);
        assert_eq!(role::normalize_legacy("weird"), "weird");
    }

    #[test]
    fn view_normalizes_legacy_role() {
        // 一期遗留 master/node 在视图层归一化为 manager/worker（不重写 etcd）
        let legacy_master = NodeRecord { metadata: NodeMetadata { role: "master".into(), ..sample_record().metadata }, credentials: sample_record().credentials };
        let view_m: NodeRecordView = (&legacy_master).into();
        assert_eq!(view_m.role, role::MANAGER);
        let legacy_node = NodeRecord { metadata: NodeMetadata { role: "node".into(), ..sample_record().metadata }, credentials: sample_record().credentials };
        let view_n: NodeRecordView = (&legacy_node).into();
        assert_eq!(view_n.role, role::WORKER);
    }
}