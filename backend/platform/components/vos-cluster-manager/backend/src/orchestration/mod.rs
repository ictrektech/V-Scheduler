//! 编排模块：编译期纯函数集合（可独立测试）

pub mod etcd;
pub mod postgres_ha;
pub mod cluster;
pub mod haproxy;
pub mod keepalived;
pub mod temporal;

pub use etcd::{
    build_etcd_env_vars, client_url, etcd_compose_name, peer_url,
    validate_compose_project_name, EtcdServerTls, EtcdTopology,
};
pub use postgres_ha::{
    build_patroni_env_vars, gen_credential, pg_compose_name, PatroniTopology, PG_PORT,
    PATRONI_REST_PORT,
};
pub use haproxy::{
    build_haproxy_env_vars, haproxy_compose_name, parse_ip_list,
    render_haproxy_cfg, HaproxyTopology, GATEWAY_CHECK_PATH, GATEWAY_CHECK_PORT,
    NFS_FRONT_PORT, PG_BACKEND_PORT, PG_CHECK_PORT, PG_FRONT_PORT, RGW_FRONT_PORT, SMB_FRONT_PORT,
    STATS_PORT, TEMPORAL_BACKEND_PORT, TEMPORAL_FRONT_PORT,
};
pub use keepalived::{
    build_keepalived_env_vars, keepalived_compose_name, KeepalivedTopology, COMPOSE_PREFIX as KEEPALIVED_COMPOSE_PREFIX,
    DEFAULT_VIP_PREFIX, DEFAULT_VRRP_ID, HEALTH_URL_BASE as KEEPALIVED_HEALTH_URL_BASE,
};
pub use temporal::{
    DEFAULT_NAMESPACES, FRONTEND_PORT as TEMPORAL_FRONTEND_PORT, UI_PORT as TEMPORAL_UI_PORT,
    build_temporal_env_vars, temporal_compose_name, temporal_name_to_ip, TemporalTopology,
};