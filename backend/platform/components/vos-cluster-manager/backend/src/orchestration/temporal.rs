//! 集群 temporal 编排纯函数（可独立测试，无副作用）
//!
//! 二期 S2 cluster-temporal-service：temporal 作为应用 projects 子项目
//! （project_name="temporal"），管理节点部署，DB 经本节点 haproxy 15432 连集群
//! postgres HA（旧方案 D3 语义）。
//!
//! - [`temporal_compose_name`]：`cluster-temporal-<ip-slug>`（每 manager 至多一个，
//!   同 haproxy 节点本地服务命名 D-TE-05）
//! - [`TemporalTopology`] / [`build_temporal_env_vars`]：映射 projects/temporal
//!   compose 的 `${TEMPORAL_DB_*}` / `${TEMPORAL_NAMESPACES}` 占位（D-TE-03：
//!   目标 manager 自读 etcd 凭据，密码不出本机）
//! - 端口常量：FRONTEND_PORT=7333（+100 避让平台 7233）/ UI_PORT=8133；
//!   haproxy 前端 7133 在 orchestration/haproxy.rs

use std::collections::HashMap;

/// temporal frontend gRPC（haproxy 7133 的后端目标；避让平台 7233）
pub const FRONTEND_PORT: u16 = 7333;
/// temporal Web UI（host 网络直连观测；平台 UI 在 18080 无冲突）
pub const UI_PORT: u16 = 8133;
/// temporal 默认命名空间清单
pub const DEFAULT_NAMESPACES: &str = "default";

/// IP → compose 名片段（与 haproxy 同规则；复用其内部实现避免两处漂移）
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

/// 生成 temporal 子项目 compose_project_name：`cluster-temporal-<ip-slug>`
///
/// 非法 IP 返回 None（调用方映射 PARAM_INVALID 20501）。
pub fn temporal_compose_name(node_ip: &str) -> Option<String> {
    ip_slug(node_ip).map(|slug| format!("cluster-temporal-{slug}"))
}

/// 从实例名反解节点 IP（`cluster-temporal-10-0-0-1` → `10.0.0.1`）；仅数字/连字符
/// 且四段还原，失败 None（refresh_cluster_haproxy 扫实例用）
pub fn temporal_name_to_ip(compose_project_name: &str) -> Option<String> {
    let slug = compose_project_name.strip_prefix("cluster-temporal-")?;
    let parts: Vec<&str> = slug.split('-').collect();
    if parts.len() != 4 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some(parts.join("."))
}

/// temporal 部署拓扑（凭据/端点由 handler 从 etcd 与本机探测推导，D-TE-03）
#[derive(Debug, Clone, PartialEq)]
pub struct TemporalTopology {
    /// pg 入口 host（本节点 haproxy，固定 127.0.0.1）
    pub db_host: String,
    /// pg 入口端口（haproxy PG_FRONT_PORT=15432）
    pub db_port: u16,
    /// postgres 超级用户（etcd pg_credentials）
    pub db_user: String,
    /// postgres 超级用户密码（etcd pg_credentials；仅本机 env，响应不回显）
    pub db_password: String,
    /// 逗号分隔命名空间清单
    pub namespaces: String,
}

/// 构造 temporal 实例 env_vars（映射 projects/temporal compose 的 `${TEMPORAL_DB_*}`）
pub fn build_temporal_env_vars(topo: &TemporalTopology) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("TEMPORAL_DB_HOST".to_string(), topo.db_host.clone());
    env.insert("TEMPORAL_DB_PORT".to_string(), topo.db_port.to_string());
    env.insert("TEMPORAL_DB_USER".to_string(), topo.db_user.clone());
    env.insert("TEMPORAL_DB_PASSWORD".to_string(), topo.db_password.clone());
    env.insert(
        "TEMPORAL_NAMESPACES".to_string(),
        if topo.namespaces.trim().is_empty() {
            DEFAULT_NAMESPACES.to_string()
        } else {
            topo.namespaces.clone()
        },
    );
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::etcd::validate_compose_project_name;

    #[test]
    fn compose_name_shape_and_valid() {
        assert_eq!(
            temporal_compose_name("10.100.10.21").as_deref(),
            Some("cluster-temporal-10-100-10-21")
        );
        assert!(validate_compose_project_name(
            &temporal_compose_name("10.100.10.21").unwrap()
        ));
        assert_eq!(temporal_compose_name("bad"), None);
    }

    #[test]
    fn name_to_ip_roundtrip() {
        assert_eq!(
            temporal_name_to_ip("cluster-temporal-10-100-10-21").as_deref(),
            Some("10.100.10.21")
        );
        assert_eq!(temporal_name_to_ip("cluster-temporal-bad"), None);
        assert_eq!(temporal_name_to_ip("cluster-temporal-1-2-3"), None);
        assert_eq!(
            temporal_name_to_ip("cluster-pg-node-0"),
            None,
            "非 temporal 前缀"
        );
        // roundtrip
        let ip = "192.168.1.100";
        assert_eq!(
            temporal_name_to_ip(&temporal_compose_name(ip).unwrap()).as_deref(),
            Some(ip)
        );
    }

    #[test]
    fn env_vars_shape() {
        let topo = TemporalTopology {
            db_host: "127.0.0.1".into(),
            db_port: 15432,
            db_user: "postgres".into(),
            db_password: "s3cret".into(),
            namespaces: "default,vos-ceph".into(),
        };
        let env = build_temporal_env_vars(&topo);
        assert_eq!(env["TEMPORAL_DB_HOST"], "127.0.0.1");
        assert_eq!(env["TEMPORAL_DB_PORT"], "15432");
        assert_eq!(env["TEMPORAL_DB_USER"], "postgres");
        assert_eq!(env["TEMPORAL_DB_PASSWORD"], "s3cret");
        assert_eq!(env["TEMPORAL_NAMESPACES"], "default,vos-ceph");
    }

    #[test]
    fn namespaces_default_when_empty() {
        let topo = TemporalTopology {
            db_host: "127.0.0.1".into(),
            db_port: 15432,
            db_user: "postgres".into(),
            db_password: "x".into(),
            namespaces: "  ".into(),
        };
        let env = build_temporal_env_vars(&topo);
        assert_eq!(env["TEMPORAL_NAMESPACES"], "default");
    }

    #[test]
    fn ports_avoid_platform() {
        // 平台单机 temporal：7233-7239 + UI 18080；本服务全部 +100 / 8133 避让
        assert_eq!(FRONTEND_PORT, 7333);
        assert_eq!(UI_PORT, 8133);
        assert_ne!(FRONTEND_PORT, 7233);
        assert_ne!(UI_PORT, 8080);
    }
}
