//! postgres-ha（patroni）编排纯函数（可独立测试，无副作用）
//!
//! - `pg_compose_name`：子项目 compose_project_name 生成（`cluster-pg-node-<index>`）
//! - `PatroniTopology` / `build_patroni_env_vars`：patroni 启动参数（映射
//!   projects/postgres-ha docker-compose.yml 的 `${PATRONI_*}` 占位）
//! - `gen_credential`：超级用户 / 复制用户密码生成（安全字符集，≥16 位）
//! - 端口常量：PG_PORT=5433（postgres，避让单机 5432）/ PATRONI_REST_PORT=8008

use std::collections::HashMap;

/// postgres 端口（避让单机 5432）
pub const PG_PORT: u16 = 5433;
/// patroni REST API 端口（/leader 200 = leader）
pub const PATRONI_REST_PORT: u16 = 8008;

/// 生成 postgres-ha 子项目 compose_project_name：`cluster-pg-node-<index>`
pub fn pg_compose_name(index: u32) -> String {
    format!("cluster-pg-node-{index}")
}

/// 生成凭据（超级用户 / 复制用户密码）：uuid 短码转安全字符集，≥16 位
///
/// 使用 rust uuid v4 的 hex（去掉 `-`），取前 24 位（96 bit 熵），仅含 [a-f0-9]，
/// 无 `-`/空格等 YAML 特殊字符（patroni.yml 渲染为 quoted string 亦安全）。
pub fn gen_credential() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    raw.chars().take(24).collect()
}

/// patroni 节点拓扑（单节点 bootstrap / 多节点 join）
#[derive(Debug, Clone, PartialEq)]
pub struct PatroniTopology {
    /// 本节点 patroni 名称（pg-node-<i>，与 compose_project_name 尾号一致）
    pub node_name: String,
    /// 本节点通告地址（内网 IP）
    pub host_ip: String,
    /// scope（集群内所有 patroni 实例同一 scope，DCS 下共享集群命名空间）
    pub scope: String,
    /// etcd DCS 端点（`host:port` 列表；逗号连接后注入 PATRONI_ETCD_HOSTS）
    pub etcd_hosts: Vec<String>,
    /// 首节点 bootstrap（true=写 DCS 初始化；后续节点 false=joind）
    pub is_bootstrap: bool,
    /// postgres 超级用户
    pub superuser_username: String,
    /// 复制用户
    pub replication_username: String,
    /// 超级用户密码（编排方 gen_credential 生成）
    pub superuser_password: String,
    /// 复制用户密码
    pub replication_password: String,
}

/// 构造 patroni 启动 env_vars（映射 projects/postgres-ha compose 的 `${PATRONI_*}`）
pub fn build_patroni_env_vars(topo: &PatroniTopology) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("PATRONI_SCOPE".to_string(), topo.scope.clone());
    env.insert("PATRONI_NAME".to_string(), topo.node_name.clone());
    env.insert("PATRONI_HOST_IP".to_string(), topo.host_ip.clone());
    env.insert(
        "PATRONI_ETCD_HOSTS".to_string(),
        topo.etcd_hosts.join(","),
    );
    env.insert(
        "PATRONI_BOOTSTRAP".to_string(),
        if topo.is_bootstrap { "true" } else { "false" }.to_string(),
    );
    env.insert(
        "PATRONI_SUPERUSER_USERNAME".to_string(),
        topo.superuser_username.clone(),
    );
    env.insert(
        "PATRONI_SUPERUSER_PASSWORD".to_string(),
        topo.superuser_password.clone(),
    );
    env.insert(
        "PATRONI_REPLICATION_USERNAME".to_string(),
        topo.replication_username.clone(),
    );
    env.insert(
        "PATRONI_REPLICATION_PASSWORD".to_string(),
        topo.replication_password.clone(),
    );
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topo(node: &str, ip: &str, bootstrap: bool) -> PatroniTopology {
        PatroniTopology {
            node_name: node.into(),
            host_ip: ip.into(),
            scope: "vos-cluster".into(),
            etcd_hosts: vec!["10.0.0.1:2379".into()],
            is_bootstrap: bootstrap,
            superuser_username: "postgres".into(),
            replication_username: "replicator".into(),
            superuser_password: gen_credential(),
            replication_password: gen_credential(),
        }
    }

    #[test]
    fn compose_name_shape() {
        assert_eq!(pg_compose_name(0), "cluster-pg-node-0");
        assert_eq!(pg_compose_name(3), "cluster-pg-node-3");
        // 生成值通过平台 compose_project_name 校验
        assert!(crate::orchestration::validate_compose_project_name(&pg_compose_name(0)));
        assert!(crate::orchestration::validate_compose_project_name(&pg_compose_name(99)));
    }

    #[test]
    fn bootstrap_env_vars() {
        let t = topo("pg-node-0", "10.0.0.1", true);
        let env = build_patroni_env_vars(&t);
        assert_eq!(env["PATRONI_NAME"], "pg-node-0");
        assert_eq!(env["PATRONI_SCOPE"], "vos-cluster");
        assert_eq!(env["PATRONI_HOST_IP"], "10.0.0.1");
        assert_eq!(env["PATRONI_ETCD_HOSTS"], "10.0.0.1:2379");
        assert_eq!(env["PATRONI_BOOTSTRAP"], "true");
        assert_eq!(env["PATRONI_SUPERUSER_USERNAME"], "postgres");
        assert_eq!(env["PATRONI_REPLICATION_USERNAME"], "replicator");
        assert!(!env["PATRONI_SUPERUSER_PASSWORD"].is_empty());
        assert!(!env["PATRONI_REPLICATION_PASSWORD"].is_empty());
    }

    #[test]
    fn join_env_vars_multi_dcs() {
        let mut t = topo("pg-node-1", "10.0.0.2", false);
        t.etcd_hosts = vec!["10.0.0.1:2379".into(), "10.0.0.2:2379".into()];
        let env = build_patroni_env_vars(&t);
        assert_eq!(env["PATRONI_BOOTSTRAP"], "false");
        assert_eq!(env["PATRONI_ETCD_HOSTS"], "10.0.0.1:2379,10.0.0.2:2379");
    }

    #[test]
    fn credentials_unique_and_safe() {
        let a = gen_credential();
        let b = gen_credential();
        assert_ne!(a, b);
        assert!(a.len() >= 16 && b.len() >= 16);
        for ch in a.chars().chain(b.chars()) {
            assert!(ch.is_ascii_alphanumeric(), "非法字符 {ch}");
        }
    }

    #[test]
    fn port_constants_distinct_from_single() {
        assert_eq!(PG_PORT, 5433);
        assert_eq!(PATRONI_REST_PORT, 8008);
        assert_ne!(PG_PORT, 5432, "必须避让平台单机 postgres");
    }
}