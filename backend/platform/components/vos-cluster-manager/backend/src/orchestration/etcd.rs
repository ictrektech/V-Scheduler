//! 编排纯函数（可独立测试，无副作用）
//!
//! - `etcd_compose_name` / `validate_compose_project_name`：compose_project_name 生成与校验
//!   （对齐 app-projects-design.md §5.4：`^[a-z0-9][-a-z0-9_.]{1,62}[a-z0-9]$`，禁 `vos-app-` 前缀）
//! - `EtcdTopology` / `build_etcd_env_vars`：etcd raft 启动参数（映射 projects/etcd
//!   docker-compose.yml 的 `${ETCD_*}` 占位）

use std::collections::HashMap;

/// 校验 compose_project_name（平台规则 §5.4）
///
/// 说明：平台校验为 `^[a-z0-9][-a-z0-9_.]{1,62}[a-z0-9]$`（长度 3~64）且禁止
/// `vos-app-` 前缀。Rust regex: 开头小写字母/数字，中间 `[-a-z0-9_.]{1,62}`，
/// 结尾小写字母/数字（长度 3~64）。
pub fn validate_compose_project_name(name: &str) -> bool {
    if name.starts_with("vos-app-") {
        return false;
    }
    let re = regex::Regex::new(r"^[a-z0-9][-a-z0-9_.]{1,62}[a-z0-9]$").unwrap();
    re.is_match(name)
}

/// 生成 etcd 子项目 compose_project_name：`cluster-etcd-node-<index>`
pub fn etcd_compose_name(index: u32) -> String {
    format!("cluster-etcd-node-{index}")
}

/// etcd 服务端 TLS 配置（客户端证书校验；服务端证书由 vcm-etcd-auth 签发）
#[derive(Debug, Clone, PartialEq)]
pub struct EtcdServerTls {
    /// 宿主 PKI 目录（挂载进容器的 `/etc/vos/etcd/pki`）
    pub pki_dir: String,
    /// 是否要求客户端证书（v1 开启 TLS 时固定 true）
    pub client_cert_auth: bool,
}

/// etcd 节点拓扑（单节点 / 多节点 raft）
#[derive(Debug, Clone, PartialEq)]
pub struct EtcdTopology {
    /// 本节点 etcd 名称（etcd0 / etcd1 / ...）
    pub node_name: String,
    /// 本节点 client 通告地址（内网 IP）
    pub client_host: String,
    /// 本节点 peer 通告地址（内网 IP）
    pub peer_host: String,
    /// 初始成员 `(name, peer_url)`（单节点 = 自身；多节点 = 全量）
    pub initial_members: Vec<(String, String)>,
    /// `new`（集群初始化）| `existing`（加入运行中集群）
    pub initial_state: String,
    /// 服务端 TLS（None = 明文，保持既有行为）
    pub tls: Option<EtcdServerTls>,
}

impl EtcdTopology {
    /// 单节点拓扑（集群初始化首节点）
    pub fn single(node_name: &str, host: &str) -> Self {
        Self {
            node_name: node_name.to_string(),
            client_host: host.to_string(),
            peer_host: host.to_string(),
            initial_members: vec![(node_name.to_string(), peer_url(host))],
            initial_state: "new".to_string(),
            tls: None,
        }
    }
}

/// peer url：`http://<host>:2380`
pub fn peer_url(host: &str) -> String {
    format!("http://{host}:2380")
}

/// client url：`http://<host>:2379`
pub fn client_url(host: &str) -> String {
    format!("http://{host}:2379")
}

/// 构造 etcd 启动 env_vars（映射 projects/etcd compose 的 `${ETCD_*}`）
pub fn build_etcd_env_vars(topo: &EtcdTopology) -> HashMap<String, String> {
    let initial_cluster = topo
        .initial_members
        .iter()
        .map(|(n, p)| format!("{n}={p}"))
        .collect::<Vec<_>>()
        .join(",");

    let mut env = HashMap::new();
    env.insert("ETCD_NAME".to_string(), topo.node_name.clone());
    let advertise_client = match topo.tls.as_ref() {
        Some(_) => client_url(&topo.client_host).replace("http://", "https://"),
        None => client_url(&topo.client_host),
    };
    env.insert("ETCD_ADVERTISE_CLIENT".to_string(), advertise_client.clone());
    env.insert("ETCD_ADVERTISE_CLIENT_URLS".to_string(), advertise_client);
    env.insert("ETCD_ADVERTISE_PEER".to_string(), peer_url(&topo.peer_host));
    env.insert("ETCD_INITIAL_CLUSTER".to_string(), initial_cluster);
    env.insert("ETCD_INITIAL_CLUSTER_STATE".to_string(), topo.initial_state.clone());

    match topo.tls.as_ref() {
        Some(tls) => {
            let pki = tls.pki_dir.trim_end_matches('/');
            env.insert(
                "ETCD_LISTEN_CLIENT_URLS".to_string(),
                "https://0.0.0.0:2379".to_string(),
            );
            env.insert(
                "ETCD_CLIENT_CERT_AUTH".to_string(),
                if tls.client_cert_auth { "true" } else { "false" }.to_string(),
            );
            env.insert("ETCD_TRUSTED_CA_FILE".to_string(), format!("{pki}/ca.crt"));
            env.insert("ETCD_CERT_FILE".to_string(), format!("{pki}/server.crt"));
            env.insert("ETCD_KEY_FILE".to_string(), format!("{pki}/server.key"));
            env.insert("ETCD_CERT_HOST_DIR".to_string(), tls.pki_dir.clone());
        }
        None => {
            env.insert(
                "ETCD_LISTEN_CLIENT_URLS".to_string(),
                "http://0.0.0.0:2379".to_string(),
            );
            env.insert("ETCD_CLIENT_CERT_AUTH".to_string(), "false".to_string());
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_name_validate() {
        // 合法
        assert!(validate_compose_project_name("cluster-etcd-node-0"));
        assert!(validate_compose_project_name("cluster-etcd-node-12"));
        // 非法：保留前缀
        assert!(!validate_compose_project_name("vos-app-cluster"));
        // 非法：大写 / 非法字符 / 边界长度
        assert!(!validate_compose_project_name("Cluster-etcd"));
        assert!(!validate_compose_project_name("cluster etcd"));
        assert!(!validate_compose_project_name("ab")); // 长度 <3
        assert!(!validate_compose_project_name(""));   // 空
        // 生成值合法
        assert!(validate_compose_project_name(&etcd_compose_name(0)));
        assert!(validate_compose_project_name(&etcd_compose_name(99)));
    }

    #[test]
    fn etcd_compose_name_shape() {
        assert_eq!(etcd_compose_name(0), "cluster-etcd-node-0");
        assert_eq!(etcd_compose_name(3), "cluster-etcd-node-3");
    }

    #[test]
    fn single_node_env_vars() {
        let topo = EtcdTopology::single("etcd0", "10.0.0.1");
        let env = build_etcd_env_vars(&topo);
        assert_eq!(env["ETCD_NAME"], "etcd0");
        assert_eq!(env["ETCD_ADVERTISE_CLIENT"], "http://10.0.0.1:2379");
        assert_eq!(env["ETCD_ADVERTISE_PEER"], "http://10.0.0.1:2380");
        assert_eq!(env["ETCD_INITIAL_CLUSTER"], "etcd0=http://10.0.0.1:2380");
        assert_eq!(env["ETCD_INITIAL_CLUSTER_STATE"], "new");
    }

    #[test]
    fn multi_node_env_vars() {
        let topo = EtcdTopology {
            node_name: "etcd1".into(),
            client_host: "10.0.0.2".into(),
            peer_host: "10.0.0.2".into(),
            initial_members: vec![
                ("etcd0".into(), peer_url("10.0.0.1")),
                ("etcd1".into(), peer_url("10.0.0.2")),
                ("etcd2".into(), peer_url("10.0.0.3")),
            ],
            initial_state: "existing".into(),
            tls: None,
        };
        let env = build_etcd_env_vars(&topo);
        assert_eq!(env["ETCD_NAME"], "etcd1");
        assert_eq!(
            env["ETCD_INITIAL_CLUSTER"],
            "etcd0=http://10.0.0.1:2380,etcd1=http://10.0.0.2:2380,etcd2=http://10.0.0.3:2380"
        );
        assert_eq!(env["ETCD_INITIAL_CLUSTER_STATE"], "existing");
    }
}