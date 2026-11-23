use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use vos_cluster_manager::etcd::security::normalize_endpoints;
use vos_cluster_manager::state::EtcdAuthState;
use vos_cluster_manager::{build_router, db, etcd, logging, projects, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    logging::init_logging(&config.log_level);

    // 确保数据目录存在（本地管理状态根）
    std::fs::create_dir_all(&config.data_dir)
        .map_err(|e| anyhow::anyhow!("创建数据目录 {} 失败: {e}", config.data_dir.display()))?;

    let url_base = config.normalized_url_base();

    // ---- etcd 连接参数（D9：认证/TLS 硬门禁）----
    // 显式认证模式下明文端点 / 缺凭据 → 启动失败并给出可操作错误（spec：禁止静默回退）
    let connect_spec = config.etcd_connect_spec().map_err(|e| {
        anyhow::anyhow!(
            "etcd 连接配置非法：{e}\n修复建议：检查 VOS_CLUSTER_MANAGER_ETCD_AUTH_MODE / \
             VOS_CLUSTER_MANAGER_ETCD_ENDPOINTS / VOS_CLUSTER_MANAGER_ETCD_CREDENTIAL_DIR \
             （详见 docs/etcd-auth-tls.md）"
        )
    })?;

    // ---- etcd 客户端装配（依赖注入）----
    // 端点未配置 → Unavailable；启动连接失败 → Lazy（惰性重连：创建编排自举 etcd
    // 子项目后自动恢复，无需容器重启；/api/health 反映 etcd_ok）
    let etcd_client: Arc<dyn etcd::EtcdClusterClient> = if config.etcd_endpoints.is_empty() {
        tracing::warn!("未配置 --etcd-endpoints，etcd 客户端降级为不可用（/api/health etcd_ok=false）");
        Arc::new(etcd::UnavailableEtcdClusterClient::new("未配置 --etcd-endpoints"))
    } else {
        match etcd::RealEtcdClusterClient::new_with_spec(&connect_spec).await {
            Ok(c) => Arc::new(c),
            Err(e) => {
                tracing::warn!(
                    err = %e.msg,
                    auth_mode = connect_spec.mode.as_str(),
                    "etcd 启动连接失败，启用惰性重连（创建编排自举 etcd 子项目后自动恢复）"
                );
                Arc::new(etcd::LazyEtcdClusterClient::new_with_spec(
                    connect_spec.clone(),
                ))
            }
        }
    };

    // ---- etcd 认证连接工厂（bootstrap / 状态 / 轮换 / 凭据分发）----
    let credential_layout = config.credential_layout();
    let auth_endpoints = normalize_endpoints(
        &config.etcd_endpoints,
        connect_spec.mode.requires_tls(),
    );
    let auth_connector: Arc<dyn etcd::EtcdAuthConnector> = Arc::new(
        etcd::RealEtcdAuthConnector::new(auth_endpoints, Some(credential_layout.ca_cert())),
    );
    let etcd_auth = EtcdAuthState {
        mode: connect_spec.mode,
        store: etcd::CredentialStore::new(credential_layout.clone()),
        layout: credential_layout,
        endpoints: config.etcd_endpoints.clone(),
        identity: config.etcd_identity.clone(),
        connector: Some(auth_connector),
    };

    // ---- 管理面状态库装配（平台 postgres，D-VCM-09）----
    // 平台未注入 VOS_POSTGRES_* → init_db 返回 Ok(None) 降级；注入但连接失败 → 启动失败
    let db_cfg = db::db_config_from_env(std::env::vars());
    let database_url = std::env::var("VOS_CLUSTER_MANAGER_DATABASE_URL").ok();
    let db = if let Some(url) = database_url.as_deref() {
        db::init_db_from_url(Some(url)).await
    } else {
        db::init_db(db_cfg.as_ref()).await
    }
        .map_err(|e| anyhow::anyhow!("管理面状态库初始化失败: {}", e.msg))?;

    // ---- 平台 projects 客户端装配（D-VCM-03/08，/v1000/app/projects/* 编排）----
    let env_map = std::env::vars().collect::<std::collections::HashMap<_, _>>();
    let gw_ip = env_map.get("VOS_HOST_GW_IP").cloned().unwrap_or_else(|| "172.17.0.1".into());
    let gw_http_port = env_map
        .get("VOS_API_GW_PORT_INTERNAL")
        .cloned()
        .unwrap_or_else(|| "1182".into());
    let base_url = format!("http://{gw_ip}:{gw_http_port}/v1000/app/projects/");
    let token_path = projects::access_token::access_token_path(&env_map);
    let projects_client: Arc<dyn projects::ProjectsClient> =
        match projects::RealProjectsClient::new(base_url, &token_path) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                tracing::warn!(err = %e.msg, "平台 projects 客户端不可用（access-token 缺失），编排接口将返回 20508");
                Arc::new(projects::UnavailableProjectsClient::new(e.msg))
            }
        };

    // ---- 平台 temporal 地址（VMS 先例；未注入 → None，worker 不启动）----
    let temporal_port = env_map
        .get("VOS_TEMPORAL_PORT")
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "7233".into());
    let temporal_address = Some(format!("{gw_ip}:{temporal_port}"));

    // ---- 跨节点执行客户端（Phase 4；目标节点须安装本应用，host 网络直连）----
    let remote_port = std::env::var("REMOTE_APP_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(19906);
    let remote_client: Arc<dyn vos_cluster_manager::remote::RemoteNodeClient> =
        if projects::APP_ID == "com.ictrek.v-scheduler" {
            Arc::new(vos_cluster_manager::remote::RealRemoteNodeClient::scheduler()
                .map_err(|error| anyhow::anyhow!("{}", error.msg))?)
        } else {
        match vos_cluster_manager::remote::RealRemoteNodeClient::new(
        url_base.clone(),
        remote_port,
    ) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            tracing::warn!(err = %e.msg, "跨节点执行客户端构造失败，改用 Mock（编排将不可用）");
            Arc::new(vos_cluster_manager::remote::MockRemoteNodeClient::new())
        }
        }
        };

    let state = vos_cluster_manager::AppState::with_runtime(
        config.data_dir,
        config.frontend_dir,
        url_base.clone(),
        config.etcd_endpoints,
        etcd_client,
        projects_client,
        temporal_address.clone(),
        remote_client,
        remote_port,
        db,
    )
    .with_etcd_auth(etcd_auth);

    let app = build_router(state.clone());

    let addr = SocketAddr::new(config.bind_address, config.port);
    tracing::info!(
        %addr,
        url_base = %if url_base.is_empty() { "/" } else { &url_base },
        db_ok = %(database_url.is_some() || db_cfg.is_some()),
        etcd_auth_mode = connect_spec.mode.as_str(),
        "vos-cluster-manager 启动"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // ---- 平台 temporal worker（D-VCM-08；未注入/连接失败降级，不阻断主服务）----
    if let Some(addr) = temporal_address {
        match vos_cluster_manager::temporal::worker::spawn_worker(
            addr.clone(),
            vos_cluster_manager::temporal::TASK_QUEUE.to_string(),
            state.clone(),
        ) {
            Ok(_handle) => tracing::info!(
                temporal = %addr,
                "temporal worker 已启动（EchoWorkflow 演示 + ClusterCreateWorkflow 编排）"
            ),
            Err(e) => tracing::warn!(err = %e, "temporal worker 启动失败（降级：编排动作仍可直接调平台 projects API）"),
        }
    } else {
        tracing::warn!("未注入 VOS_TEMPORAL_PORT，temporal worker 不启动");
    }

    axum::serve(listener, app).await?;
    Ok(())
}
