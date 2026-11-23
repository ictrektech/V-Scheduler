//! haproxy 编排纯函数（可独立测试，无副作用）
//!
//! 二期 S1 cluster-haproxy-lb：haproxy 作为应用 projects 子项目（project_name="haproxy"），
//! 每节点部署一个实例，提供本地统一入口 15432 → postgres HA（patroni leader 路由）。
//!
//! **SMB/NFS VIP 已迁移**：由网关节点上的 keepalived 直接持有 VIP 并指向 samba/nfsd，
//! 本模块不再为 445/2049 渲染 listener；`HaproxyTopology` 中 smb/nfs/vip 字段仅为
//! 旧 API 兼容保留（ceph 已不再传），运行时 compose 已移除对应块。
//!
//! - [`haproxy_compose_name`]：compose_project_name `cluster-haproxy-<ip-slug>`（D-HA-02）
//! - [`HaproxyTopology`] / [`build_haproxy_env_vars`]：单行逗号分隔拓扑 env（D-HA-01）
//! - [`render_haproxy_cfg`]：与 compose 内联脚本一致的 Rust 镜像渲染器（配置校验 / 预览 / 单测）

use std::collections::HashMap;

/// postgres HA 本地统一入口端口（haproxy frontend → managers:5433）
pub const PG_FRONT_PORT: u16 = 15432;
/// haproxy stats 观测页端口
pub const STATS_PORT: u16 = 18404;
/// 集群 temporal 本地统一入口端口（S2 预留；避让平台单机 temporal 7233）
pub const TEMPORAL_FRONT_PORT: u16 = 7133;
/// postgres HA 后端端口（patroni postgres，与 orchestration::postgres_ha::PG_PORT 一致）
pub const PG_BACKEND_PORT: u16 = 5433;
/// patroni REST 健康检查端口（leader `/leader` 200）
pub const PG_CHECK_PORT: u16 = 8008;
/// 集群 temporal 后端端口（S2：manager 上 temporal server frontend，避让平台 7233）
pub const TEMPORAL_BACKEND_PORT: u16 = 7333;
/// SMB 网关 VIP 端口（标准 445；host 网络需要 root 绑定）
pub const SMB_FRONT_PORT: u16 = 445;
/// NFS 网关 VIP 端口（标准 2049）
pub const NFS_FRONT_PORT: u16 = 2049;
/// 网关健康检查端口（本应用后端 19906）
pub const GATEWAY_CHECK_PORT: u16 = 19906;
/// 网关健康检查路径（保留给 gateway 健康端点；HAProxy 不再渲染 SMB/NFS listener）
pub const GATEWAY_CHECK_PATH: &str = "/app/com.ictrek.vos-cluster-manager/api/gateway/health";
/// RGW S3 前端/后端端口（节点 RGW 仍监听 nodeIP:7480）
pub const RGW_FRONT_PORT: u16 = 7480;

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

/// 生成 haproxy 子项目 compose_project_name：`cluster-haproxy-<ip-slug>`
///
/// 非法 IP 返回 None（调用方映射 PARAM_INVALID 20501）。
pub fn haproxy_compose_name(node_ip: &str) -> Option<String> {
    ip_slug(node_ip).map(|slug| format!("cluster-haproxy-{slug}"))
}

/// haproxy 部署拓扑（backends 由编排方从 etcd NodeMetadata / 管理面网关表推导）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HaproxyTopology {
    /// postgres HA 后端（manager 节点 IP 清单，按序渲染 server pg0/pg1/...）
    pub pg_backends: Vec<String>,
    /// 集群 temporal 后端（S2 前恒空；非空则渲染 listen temporal）
    pub temporal_backends: Vec<String>,
    /// SMB 网关后端（healthy gateway IP；空则不渲染 445 listener）
    pub smb_backends: Vec<String>,
    /// NFS 网关后端（空则不渲染 2049 listener）
    pub nfs_backends: Vec<String>,
    /// 网关 VIP 地址（SMB/NFS frontend 绑定该地址，避免与节点本地 445/2049 冲突）
    pub vip: Option<String>,
    /// RGW 后端（全部 RGW 节点 IP；HAProxy 后端连接 nodeIP:7480）
    pub rgw_backends: Vec<String>,
    /// RGW 独立 VIP（HAProxy RGW listener 绑定该地址）
    pub rgw_vip: Option<String>,
}

fn join_backends(list: &[String]) -> String {
    list.iter().map(|s| s.trim()).collect::<Vec<_>>().join(",")
}

/// 构造 haproxy 实例 env_vars（映射 projects/haproxy compose 的 `${HAPROXY_*}`）
///
/// 所有值为单行、无空格（D-HA-01：避免多行 .env 插值兼容风险）。
pub fn build_haproxy_env_vars(topo: &HaproxyTopology) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("HAPROXY_STATS_PORT".to_string(), STATS_PORT.to_string());
    env.insert("HAPROXY_PG_FRONT_PORT".to_string(), PG_FRONT_PORT.to_string());
    env.insert("HAPROXY_PG_BACKEND_PORT".to_string(), PG_BACKEND_PORT.to_string());
    env.insert("HAPROXY_PG_CHECK_PORT".to_string(), PG_CHECK_PORT.to_string());
    env.insert(
        "HAPROXY_PG_BACKENDS".to_string(),
        join_backends(&topo.pg_backends),
    );
    env.insert(
        "HAPROXY_TEMPORAL_FRONT_PORT".to_string(),
        TEMPORAL_FRONT_PORT.to_string(),
    );
    env.insert(
        "HAPROXY_TEMPORAL_BACKEND_PORT".to_string(),
        TEMPORAL_BACKEND_PORT.to_string(),
    );
    env.insert(
        "HAPROXY_TEMPORAL_BACKENDS".to_string(),
        join_backends(&topo.temporal_backends),
    );
    env.insert(
        "HAPROXY_SMB_FRONT_PORT".to_string(),
        SMB_FRONT_PORT.to_string(),
    );
    env.insert(
        "HAPROXY_SMB_BACKEND_PORT".to_string(),
        SMB_FRONT_PORT.to_string(),
    );
    env.insert(
        "HAPROXY_SMB_BACKENDS".to_string(),
        join_backends(&topo.smb_backends),
    );
    env.insert(
        "HAPROXY_NFS_FRONT_PORT".to_string(),
        NFS_FRONT_PORT.to_string(),
    );
    env.insert(
        "HAPROXY_NFS_BACKEND_PORT".to_string(),
        NFS_FRONT_PORT.to_string(),
    );
    env.insert(
        "HAPROXY_NFS_BACKENDS".to_string(),
        join_backends(&topo.nfs_backends),
    );
    env.insert(
        "HAPROXY_GATEWAY_CHECK_PORT".to_string(),
        GATEWAY_CHECK_PORT.to_string(),
    );
    env.insert(
        "HAPROXY_VIP".to_string(),
        topo.vip.clone().unwrap_or_default(),
    );
    env.insert(
        "HAPROXY_RGW_VIP".to_string(),
        topo.rgw_vip.clone().unwrap_or_default(),
    );
    env.insert(
        "HAPROXY_RGW_PORT".to_string(),
        RGW_FRONT_PORT.to_string(),
    );
    env.insert(
        "HAPROXY_RGW_BACKENDS".to_string(),
        join_backends(&topo.rgw_backends),
    );
    env
}

/// 渲染 haproxy 配置（与 `projects/haproxy/docker-compose.yml` 内联脚本一致）
///
/// 用途：`GET /api/infra/haproxy/config_preview` 运维核对 + 单测覆盖渲染语义。
/// **运行时以 compose 内联脚本为准**；单测做契约片段静态比对防止漂移。
pub fn render_haproxy_cfg(topo: &HaproxyTopology) -> String {
    let mut cfg = String::new();
    cfg.push_str("global\n");
    cfg.push_str("    maxconn 4096\n");
    cfg.push_str("    log stdout format raw local0 info\n\n");
    cfg.push_str("defaults\n");
    cfg.push_str("    log global\n");
    cfg.push_str("    mode tcp\n");
    cfg.push_str("    retries 2\n");
    cfg.push_str("    timeout connect 5s\n");
    cfg.push_str("    timeout client 1h\n");
    cfg.push_str("    timeout server 1h\n\n");
    cfg.push_str("listen stats\n");
    cfg.push_str(&format!("    bind *:{STATS_PORT}\n"));
    cfg.push_str("    mode http\n");
    cfg.push_str("    stats enable\n");
    cfg.push_str("    stats uri /\n");
    cfg.push_str("    stats refresh 10s\n");

    if !topo.pg_backends.is_empty() {
        cfg.push_str("\nlisten pg_ha\n");
        cfg.push_str(&format!("    bind *:{PG_FRONT_PORT}\n"));
        cfg.push_str("    balance roundrobin\n");
        cfg.push_str("    option httpchk\n");
        cfg.push_str("    http-check send meth GET uri /leader ver HTTP/1.1\n");
        cfg.push_str("    http-check expect status 200\n");
        for (i, ip) in topo.pg_backends.iter().enumerate() {
            cfg.push_str(&format!(
                "    server pg{i} {ip}:{PG_BACKEND_PORT} check port {PG_CHECK_PORT} inter 2s fall 3 rise 2\n"
            ));
        }
    }

    if !topo.temporal_backends.is_empty() {
        cfg.push_str("\nlisten temporal\n");
        cfg.push_str(&format!("    bind *:{TEMPORAL_FRONT_PORT}\n"));
        cfg.push_str("    balance roundrobin\n");
        for (i, ip) in topo.temporal_backends.iter().enumerate() {
            cfg.push_str(&format!(
                "    server t{i} {ip}:{TEMPORAL_BACKEND_PORT} check inter 2s fall 3 rise 2\n"
            ));
        }
    }

    if !topo.rgw_backends.is_empty()
        && let Some(vip) = topo.rgw_vip.as_deref().filter(|v| !v.trim().is_empty())
    {
        cfg.push_str("\nlisten rgw\n");
        cfg.push_str(&format!(
            "    bind 127.0.0.1:{RGW_FRONT_PORT}\n    bind {vip}:{RGW_FRONT_PORT}\n"
        ));
        cfg.push_str("    mode tcp\n    balance roundrobin\n    option tcp-check\n");
        for (i, ip) in topo.rgw_backends.iter().enumerate() {
            cfg.push_str(&format!(
                "    server rgw{i} {ip}:{RGW_FRONT_PORT} check inter 2s fall 3 rise 2\n"
            ));
        }
    }

    cfg
}

/// 解析逗号分隔 / 数组形式的 IP 清单（编排方经 `at` 转发或前端显式注入）
///
/// 过滤非法 IP 并去重（保持输入顺序）；空结果返回 None。
pub fn parse_ip_list(v: Option<&serde_json::Value>) -> Option<Vec<String>> {
    use serde_json::Value;
    let raw: Vec<String> = match v? {
        Value::String(s) => s.split(',').map(|x| x.trim().to_string()).collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
            .collect(),
        _ => return None,
    };
    let list: Vec<String> = raw
        .into_iter()
        .filter(|s| !s.is_empty())
        .fold(Vec::new(), |mut acc, ip| {
            if ip.parse::<std::net::IpAddr>().is_ok() && !acc.contains(&ip) {
                acc.push(ip);
            }
            acc
        });
    if list.is_empty() { None } else { Some(list) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::etcd::validate_compose_project_name;

    #[test]
    fn compose_name_shape_and_valid() {
        assert_eq!(
            haproxy_compose_name("10.100.10.21").as_deref(),
            Some("cluster-haproxy-10-100-10-21")
        );
        assert_eq!(
            haproxy_compose_name(" 192.168.1.5 ").as_deref(),
            Some("cluster-haproxy-192-168-1-5")
        );
        assert!(validate_compose_project_name(
            &haproxy_compose_name("10.100.10.21").unwrap()
        ));
    }

    #[test]
    fn compose_name_rejects_bad_input() {
        assert_eq!(haproxy_compose_name(""), None);
        assert_eq!(haproxy_compose_name("host-a"), None);
        assert_eq!(haproxy_compose_name("10.100.10"), None);
        assert_eq!(haproxy_compose_name("10.100.10.21.5"), None);
        assert_eq!(haproxy_compose_name("10.100.10.x"), None);
    }

    #[test]
    fn env_vars_pg_only_s1() {
        let topo = HaproxyTopology {
            pg_backends: vec!["10.0.0.1".into(), "10.0.0.2".into()],
            temporal_backends: vec![],
            ..Default::default()
        };
        let env = build_haproxy_env_vars(&topo);
        assert_eq!(env["HAPROXY_PG_BACKENDS"], "10.0.0.1,10.0.0.2");
        assert_eq!(env["HAPROXY_PG_FRONT_PORT"], "15432");
        assert_eq!(env["HAPROXY_PG_BACKEND_PORT"], "5433");
        assert_eq!(env["HAPROXY_PG_CHECK_PORT"], "8008");
        assert_eq!(env["HAPROXY_STATS_PORT"], "18404");
        assert_eq!(env["HAPROXY_TEMPORAL_BACKENDS"], "");
        assert_eq!(env["HAPROXY_SMB_BACKENDS"], "");
        assert_eq!(env["HAPROXY_NFS_BACKENDS"], "");
    }

    #[test]
    fn env_vars_with_temporal_backends_s2_contract() {
        let topo = HaproxyTopology {
            pg_backends: vec!["10.0.0.1".into()],
            temporal_backends: vec!["10.0.0.1".into(), "10.0.0.2".into()],
            ..Default::default()
        };
        let env = build_haproxy_env_vars(&topo);
        assert_eq!(env["HAPROXY_TEMPORAL_BACKENDS"], "10.0.0.1,10.0.0.2");
        assert_eq!(env["HAPROXY_TEMPORAL_FRONT_PORT"], "7133");
        assert_eq!(env["HAPROXY_TEMPORAL_BACKEND_PORT"], "7333");
    }

    #[test]
    fn env_vars_include_gateway_vip_ports_and_backends() {
        let topo = HaproxyTopology {
            pg_backends: vec!["10.0.0.1".into()],
            temporal_backends: vec![],
            smb_backends: vec!["10.0.0.1".into(), "10.0.0.2".into()],
            nfs_backends: vec!["10.0.0.2".into()],
            vip: Some("10.0.0.100".into()),
            rgw_backends: vec![],
            rgw_vip: None,
        };
        let env = build_haproxy_env_vars(&topo);
        assert_eq!(env["HAPROXY_VIP"], "10.0.0.100");
        assert_eq!(env["HAPROXY_SMB_FRONT_PORT"], "445");
        assert_eq!(env["HAPROXY_SMB_BACKEND_PORT"], "445");
        assert_eq!(env["HAPROXY_SMB_BACKENDS"], "10.0.0.1,10.0.0.2");
        assert_eq!(env["HAPROXY_NFS_FRONT_PORT"], "2049");
        assert_eq!(env["HAPROXY_NFS_BACKENDS"], "10.0.0.2");
        assert_eq!(env["HAPROXY_GATEWAY_CHECK_PORT"], "19906");
    }

    #[test]
    fn env_values_single_line_no_space() {
        let topo = HaproxyTopology {
            pg_backends: vec![" 10.0.0.1 ".into(), "10.0.0.2".into()],
            temporal_backends: vec![],
            smb_backends: vec![" 10.0.0.3 ".into()],
            nfs_backends: vec![],
            vip: Some("10.0.0.100".into()),
            rgw_backends: vec![],
            rgw_vip: None,
        };
        for (k, v) in build_haproxy_env_vars(&topo) {
            assert!(!v.contains(' '), "{k} 含空格: {v:?}");
            assert!(!v.contains('\n'), "{k} 含换行: {v:?}");
        }
    }

    #[test]
    fn ports_avoid_platform_conflicts() {
        assert_eq!(PG_FRONT_PORT, 15432);
        assert_eq!(TEMPORAL_FRONT_PORT, 7133);
        assert_eq!(TEMPORAL_BACKEND_PORT, 7333);
        assert_eq!(STATS_PORT, 18404);
        assert_eq!(SMB_FRONT_PORT, 445);
        assert_eq!(NFS_FRONT_PORT, 2049);
        assert_ne!(TEMPORAL_FRONT_PORT, 7233);
        assert_ne!(PG_FRONT_PORT, 5432);
        assert_ne!(GATEWAY_CHECK_PORT, PG_BACKEND_PORT);
    }

    #[test]
    fn render_pg_temporal_and_gateway_listeners() {
        let topo = HaproxyTopology {
            pg_backends: vec!["10.0.0.1".into(), "10.0.0.2".into()],
            temporal_backends: vec!["10.0.0.1".into()],
            smb_backends: vec!["10.0.0.1".into(), "10.0.0.2".into()],
            nfs_backends: vec!["10.0.0.2".into()],
            vip: Some("10.0.0.100".into()),
            rgw_backends: vec![],
            rgw_vip: None,
        };
        let cfg = render_haproxy_cfg(&topo);
        assert!(cfg.contains("listen stats"));
        assert!(cfg.contains("bind *:18404"));
        assert!(cfg.contains("listen pg_ha"));
        assert!(cfg.contains("bind *:15432"));
        assert!(cfg.contains("http-check send meth GET uri /leader"));
        assert!(cfg.contains("server pg0 10.0.0.1:5433 check port 8008"));
        assert!(cfg.contains("listen temporal"));
        assert!(cfg.contains("bind *:7133"));
        assert!(cfg.contains("server t0 10.0.0.1:7333"));

    }

    #[test]
    fn render_rgw_listener_uses_tcp_and_vip() {
        let topo = HaproxyTopology {
            rgw_backends: vec!["10.0.0.1".into(), "10.0.0.2".into()],
            rgw_vip: Some("10.0.0.101".into()),
            ..Default::default()
        };
        let cfg = render_haproxy_cfg(&topo);
        assert!(cfg.contains("listen rgw"), "{cfg}");
        assert!(cfg.contains("bind 127.0.0.1:7480"), "{cfg}");
        assert!(cfg.contains("bind 10.0.0.101:7480"), "{cfg}");
        assert!(cfg.contains("option tcp-check"), "{cfg}");
        assert!(cfg.contains("server rgw0 10.0.0.1:7480"), "{cfg}");
    }

    #[test]
    fn render_skips_rgw_without_vip_or_backends() {
        let cfg = render_haproxy_cfg(&HaproxyTopology {
            rgw_backends: vec!["10.0.0.1".into()],
            ..Default::default()
        });
        assert!(!cfg.contains("listen rgw"), "{cfg}");
    }

    #[test]
    fn render_skips_empty_listeners() {
        let cfg = render_haproxy_cfg(&HaproxyTopology::default());
        assert!(cfg.contains("listen stats"));
        assert!(!cfg.contains("listen pg_ha"));
        assert!(!cfg.contains("listen temporal"));
    }

    #[test]
    fn parse_ip_list_filters_and_dedups() {
        let v = serde_json::json!(["10.0.0.1", "10.0.0.1", "bad", "10.0.0.2"]);
        assert_eq!(
            parse_ip_list(Some(&v)).unwrap(),
            vec!["10.0.0.1".to_string(), "10.0.0.2".to_string()]
        );
        let v = serde_json::json!("10.0.0.1, 10.0.0.2 , bad");
        assert_eq!(
            parse_ip_list(Some(&v)).unwrap(),
            vec!["10.0.0.1".to_string(), "10.0.0.2".to_string()]
        );
        assert!(parse_ip_list(Some(&serde_json::json!([]))).is_none());
        assert!(parse_ip_list(None).is_none());
    }

    /// 与 compose 内联脚本保持同构：模板必须包含本渲染器产出的全部结构性片段
    #[test]
    fn compose_template_matches_renderer_contract() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let template = manifest.join(
            "../../../apps/com.ictrek.vos-cluster-manager/src/projects/haproxy/docker-compose.yml",
        );
        let Ok(text) = std::fs::read_to_string(&template) else {
            eprintln!("跳过：未找到 {}（容器构建上下文不含 apps/）", template.display());
            return;
        };
        for needle in [
            "listen pg_ha",
            "listen temporal",
            "listen rgw",
            "HAPROXY_RGW_BACKENDS",
            "HAPROXY_RGW_VIP",
            "HAPROXY_RGW_PORT",
            "option tcp-check",
            "check inter 2s fall 3 rise 2",
        ] {
            assert!(
                text.contains(needle),
                "compose 模板缺少契约片段 {needle:?}（与 render_haproxy_cfg 漂移）"
            );
        }
    }
}