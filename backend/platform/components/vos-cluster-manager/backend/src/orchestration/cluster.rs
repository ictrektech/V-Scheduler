//! 集群创建编排纯函数（Phase 4）
//!
//! 接收用户录入的多节点列表，推导 etcd raft 成员表 / postgres-ha bootstrap+join 序列 /
//! saga 补偿清单——全部无副作用，可独立测试（D-CR-02 拓扑 + D-CR-04 补偿序）。


use serde::{Deserialize, Serialize};

use super::postgres_ha::{PG_PORT, PATRONI_REST_PORT};
use super::etcd::peer_url;

/// 用户录入的节点（与 Phase 1 NodeMetadata 对齐；temporal workflow 输入序列化）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInput {
    pub node_ip: String,
    pub display_name: String,
    /// 节点管理员密码（仅编排期授权；明文持久化到 etcd credentials，内网可信）
    pub admin_password: String,
    /// manager / worker（cluster-role-and-create-entry：管理节点运行 etcd+ha postgres；
    /// 工作节点不运行基础设施、可运行集群化负载）
    pub role: String,
}

/// 创建请求（POST /api/cluster/create 入参）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateClusterInput {
    pub cluster_name: String,
    pub nodes: Vec<NodeInput>,
}

/// 校验输入（20501 语义由 handler 层映射；纯函数返回可读错误）
pub fn validate_create_input(input: &CreateClusterInput) -> Result<(), String> {
    if input.cluster_name.trim().is_empty() {
        return Err("cluster_name 必填".into());
    }
    if input.nodes.is_empty() {
        return Err("nodes 至少 1 个节点".into());
    }
    for n in &input.nodes {
        if n.node_ip.parse::<std::net::IpAddr>().is_err() {
            return Err(format!("节点 IP 非法: {}", n.node_ip));
        }
        if n.admin_password.len() < 6 {
            return Err(format!("节点 {} 管理员密码过短（≥6）", n.node_ip));
        }
        if n.node_ip.is_empty() || n.display_name.trim().is_empty() {
            return Err(format!("节点 {} display_name 必填", n.node_ip));
        }
        if n.role != crate::etcd::dto::role::MANAGER && n.role != crate::etcd::dto::role::WORKER {
            return Err(format!("节点 {} role 必须为 manager 或 worker", n.node_ip));
        }
    }
    // 管理节点约束（D2）：基础设施拓扑 = manager 子集；发起节点（nodes[0]）须为
    // manager——stage1 本机 etcd bootstrap / stage3 本机 pg bootstrap 依赖本机部署
    if !input.nodes.iter().any(|n| n.role == crate::etcd::dto::role::MANAGER) {
        return Err("nodes 至少包含 1 个管理节点（role=manager，运行 etcd + 高可用 postgres）".into());
    }
    if input.nodes[0].role != crate::etcd::dto::role::MANAGER {
        return Err("发起节点（nodes[0]）必须为管理节点（role=manager）——编排将自举 etcd 与高可用 postgres".into());
    }
    Ok(())
}

// =========================================================================
// 角色子集（D2：基础设施拓扑 = manager 子集；worker 仅登记元数据）
// =========================================================================

/// 是否管理节点（运行 etcd + ha postgres，维护集群高可用状态）
pub fn is_manager(n: &NodeInput) -> bool {
    n.role == crate::etcd::dto::role::MANAGER
}

/// 管理节点子集（保持录入顺序；内部序号重新从 0 编号——raft 成员名 / pg 节点名
/// / compose 名 index 均以该子集顺序为准）
pub fn manager_nodes(nodes: &[NodeInput]) -> Vec<&NodeInput> {
    nodes.iter().filter(|n| is_manager(n)).collect()
}

/// 工作节点子集（保持录入顺序；不部署 etcd / postgres-ha）
pub fn worker_nodes(nodes: &[NodeInput]) -> Vec<&NodeInput> {
    nodes.iter().filter(|n| !is_manager(n)).collect()
}

/// etcd raft 成员表：`[(etcd 名, peer_url)]`（etcd0..etcdM-1，M=manager 子集数；
/// 顺序与 manager 子集一致）
pub fn etcd_member_table(nodes: &[NodeInput]) -> Vec<(String, String)> {
    manager_nodes(nodes)
        .iter()
        .enumerate()
        .map(|(i, n)| (format!("etcd{i}"), peer_url(&n.node_ip)))
        .collect()
}

/// etcd 子项目名（每管理节点本地部署用 compose_project_name；index 与 raft 名对齐）
pub fn etcd_names_for(nodes: &[NodeInput]) -> Vec<String> {
    (0..manager_nodes(nodes).len())
        .map(|i| crate::orchestration::etcd::etcd_compose_name(i as u32))
        .collect()
}

/// postgres-ha 节点名（pg-node-<i>；与 manager 子集顺序一致）
pub fn pg_names_for(nodes: &[NodeInput]) -> Vec<String> {
    (0..manager_nodes(nodes).len())
        .map(|i| format!("pg-node-{i}"))
        .collect()
}

/// 目标节点 pg 部署需要的 DCS 端点列表（等 orchestration etcd_hosts；真机为全量
/// manager 节点的 etcd 成员，worker 节点不提供 DCS）
pub fn etcd_dcs_hosts(nodes: &[NodeInput]) -> Vec<String> {
    manager_nodes(nodes)
        .iter()
        .map(|n| format!("{}:2379", n.node_ip))
        .collect()
}

/// 端口常量（导出供 handler/测试使用）
pub fn pg_port() -> u16 {
    PG_PORT
}
pub fn patroni_rest_port() -> u16 {
    PATRONI_REST_PORT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(ip: &str) -> NodeInput {
        NodeInput {
            node_ip: ip.into(),
            display_name: format!("n-{}", ip),
            admin_password: "ictrek123".into(),
            role: crate::etcd::dto::role::MANAGER.into(),
        }
    }

    /// 工作节点辅助（不部署基础设施）
    fn worker(ip: &str) -> NodeInput {
        NodeInput {
            node_ip: ip.into(),
            display_name: format!("w-{}", ip),
            admin_password: "ictrek123".into(),
            role: crate::etcd::dto::role::WORKER.into(),
        }
    }

    fn input3() -> CreateClusterInput {
        CreateClusterInput {
            cluster_name: "c3".into(),
            nodes: vec![node("10.0.0.1"), node("10.0.0.2"), node("10.0.0.3")],
        }
    }

    #[test]
    fn validate_ok_and_err() {
        assert!(validate_create_input(&input3()).is_ok());
        let bad = CreateClusterInput {
            cluster_name: " ".into(),
            nodes: vec![node("10.0.0.1")],
        };
        assert!(validate_create_input(&bad).is_err());
        let bad2 = CreateClusterInput {
            cluster_name: "x".into(),
            nodes: vec![],
        };
        assert!(validate_create_input(&bad2).is_err());
        let mut bad3 = input3();
        bad3.nodes[0].admin_password = "123".into();
        assert!(validate_create_input(&bad3).is_err());
        let mut bad4 = input3();
        bad4.nodes[1].node_ip = "nope".into();
        assert!(validate_create_input(&bad4).is_err());
    }

    #[test]
    fn validate_role_constraints() {
        // 全 worker（无 manager）→ 拒绝
        let all_worker = CreateClusterInput {
            cluster_name: "c".into(),
            nodes: vec![worker("10.0.0.1"), worker("10.0.0.2")],
        };
        assert!(validate_create_input(&all_worker).is_err());
        // 首节点 worker（即使后续有 manager）→ 拒绝（发起节点须为管理节点）
        let first_worker = CreateClusterInput {
            cluster_name: "c".into(),
            nodes: vec![worker("10.0.0.1"), node("10.0.0.2")],
        };
        assert!(validate_create_input(&first_worker).is_err());
        // 非法角色枚举 → 拒绝
        let mut bad_role = input3();
        bad_role.nodes[1].role = "master".into(); // 旧值不再接受
        assert!(validate_create_input(&bad_role).is_err());
        // 混合（首 manager + worker + manager）→ 通过
        let mixed = CreateClusterInput {
            cluster_name: "c".into(),
            nodes: vec![node("10.0.0.1"), worker("10.0.0.2"), node("10.0.0.3")],
        };
        assert!(validate_create_input(&mixed).is_ok());
    }

    #[test]
    fn member_table_three_nodes() {
        let t = etcd_member_table(&input3().nodes);
        assert_eq!(
            t,
            vec![
                ("etcd0".to_string(), "http://10.0.0.1:2380".to_string()),
                ("etcd1".to_string(), "http://10.0.0.2:2380".to_string()),
                ("etcd2".to_string(), "http://10.0.0.3:2380".to_string()),
            ]
        );
    }

    #[test]
    fn member_table_single_node() {
        let input = CreateClusterInput {
            cluster_name: "c1".into(),
            nodes: vec![node("10.0.0.1")],
        };
        assert_eq!(
            etcd_member_table(&input.nodes),
            vec![("etcd0".to_string(), "http://10.0.0.1:2380".to_string())]
        );
    }

    #[test]
    fn compose_names_and_dcs_hosts() {
        let n = input3().nodes;
        assert_eq!(
            etcd_names_for(&n),
            vec!["cluster-etcd-node-0", "cluster-etcd-node-1", "cluster-etcd-node-2"]
        );
        assert_eq!(
            pg_names_for(&n),
            vec!["pg-node-0", "pg-node-1", "pg-node-2"]
        );
        assert_eq!(
            etcd_dcs_hosts(&n),
            vec!["10.0.0.1:2379", "10.0.0.2:2379", "10.0.0.3:2379"]
        );
        // 复用既有编排常量（compose_name 形状）
        assert!(crate::orchestration::validate_compose_project_name("cluster-etcd-node-0"));
    }

    #[test]
    fn mixed_roles_topology_uses_manager_subset() {
        // 2 manager（21/22）+ 1 worker（23）：raft 成员表 / compose 名 / pg 名 /
        // DCS 端点全部只含 manager 子集，且 manager 内序号从 0 连续编号
        let mixed = CreateClusterInput {
            cluster_name: "c".into(),
            nodes: vec![node("10.0.0.21"), node("10.0.0.22"), worker("10.0.0.23")],
        }
        .nodes;
        assert_eq!(
            etcd_member_table(&mixed),
            vec![
                ("etcd0".to_string(), "http://10.0.0.21:2380".to_string()),
                ("etcd1".to_string(), "http://10.0.0.22:2380".to_string()),
            ],
            "raft 成员表只含 manager 子集"
        );
        assert_eq!(
            etcd_names_for(&mixed),
            vec!["cluster-etcd-node-0", "cluster-etcd-node-1"]
        );
        assert_eq!(
            pg_names_for(&mixed),
            vec!["pg-node-0", "pg-node-1"]
        );
        assert_eq!(
            etcd_dcs_hosts(&mixed),
            vec!["10.0.0.21:2379", "10.0.0.22:2379"]
        );
        // 子集拆分
        assert_eq!(manager_nodes(&mixed).len(), 2);
        assert_eq!(worker_nodes(&mixed).len(), 1);
        assert_eq!(worker_nodes(&mixed)[0].node_ip, "10.0.0.23");
        assert!(is_manager(&mixed[0]));
        assert!(!is_manager(&mixed[2]));
    }

    #[test]
    fn port_constants() {
        assert_eq!(pg_port(), 5433);
        assert_eq!(patroni_rest_port(), 8008);
    }
}