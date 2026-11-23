//! modelinfer 二进制入口：装配状态并启动 HTTP 服务

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;

use modelinfer::platform::{platform_base_from_env, context_token_path, RealPlatformClient};
use modelinfer::nfs;
use modelinfer::deploy::{DeployDeps, activities::DeployActivities, runtime};
use modelinfer::plaza::{self, RealPlazaClient};
use modelinfer::ssh::RealSshExec;
use modelinfer::vrouter::{RealVrouterClient, VrouterClient};
use modelinfer::{AppState, Config, build_router, db, logging};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    logging::init_logging(&config.log_level);

    std::fs::create_dir_all(&config.data_dir)
        .map_err(|e| anyhow::anyhow!("创建数据目录 {} 失败: {e}", config.data_dir.display()))?;
    std::fs::create_dir_all(config.plaza_dir())
        .map_err(|e| anyhow::anyhow!("创建模型广场目录失败: {e}"))?;

    let url_base = config.normalized_url_base();

    // ---- 数据库连接 + schema 同步（仅平台 postgres；无本地 sqlite 回退）----
    let database_url = config
        .resolved_database_url()
        .map_err(|e| anyhow::anyhow!("数据库配置非法（{}）: {}", e.code, e.msg))?;
    let conn = db::connect(&database_url)
        .await
        .map_err(|e| anyhow::anyhow!("数据库初始化失败: {}", e.msg))?;

    // ---- 平台 API 客户端 ----
    let env_map: std::collections::HashMap<String, String> = std::env::vars().collect();
    let token_path = context_token_path(&env_map);
    match &token_path {
        Some(p) if p.exists() => tracing::info!(token = %p.display(), "平台 access-token 就绪"),
        Some(p) => tracing::warn!(
            token = %p.display(),
            "平台 access-token 不存在（MODELINFER_CONTEXT_DIR 未正确挂载？），平台 API 相关功能不可用"
        ),
        None => tracing::warn!("平台 access-token 路径未解析"),
    }
    let base_url = if config.platform_api.trim().is_empty() {
        platform_base_from_env(&env_map)
    } else {
        config.platform_api.trim().to_string()
    };
    let platform = Arc::new(RealPlatformClient::new(base_url.clone(), token_path.clone(), nfs::APP_ID));

    // ---- 模型广场：种子默认来源 ----
    if let Err(e) = plaza::seed_default_source(&conn, &config.plaza_server).await {
        tracing::warn!(error = %e.msg, "模型广场默认来源种子失败");
    }

    // ---- SSH 执行器（供节点管理与部署编排共用）----
    let ssh: Arc<dyn modelinfer::ssh::SshExec> = Arc::new(RealSshExec::new());

    // ---- 统一网关集成（启动后自动注册 / 周期续约 / 停止注销）----
    let vrouter_config = config.vrouter_config();
    let vrouter_client: Arc<dyn VrouterClient> = Arc::new(RealVrouterClient::new());

    // ---- 本地反代注册表（本机 TCP → head vLLM）----
    let local_proxy = modelinfer::localproxy::ProxyRegistry::new();

    // ---- 部署依赖（供 Temporal activities 与进程级健康巡检共用）----
    let deploy_deps = DeployDeps {
        db: conn.clone(),
        ssh: ssh.clone(),
        platform: platform.clone(),
        hub_dir: config.model_hub_dir.clone(),
        model_hub_path: config.model_hub_path.clone(),
        mount_dir: config.default_mount_dir.clone(),
        nfs_server: config.nfs_server.clone(),
        image_hub: config.image_hub(),
        settings_defaults: config.observability_defaults(),
        vrouter_client: vrouter_client.clone(),
        vrouter_config: vrouter_config.clone(),
        local_proxy: local_proxy.clone(),
        monitor: config.monitor(),
    };

    // ---- 统一网关注册续约调度器（进程级，非 Temporal）----
    modelinfer::vrouter::spawn_scheduler(
        conn.clone(),
        platform.clone(),
        vrouter_client.clone(),
        vrouter_config.clone(),
        config.health_interval_secs,
    );

    // ---- Temporal：连接 + 内嵌 Worker（失败不阻塞 HTTP）----
    let temporal_address = config.resolved_temporal_address();
    let mut temporal_client = None;
    let mut _worker_handle = runtime::WorkerHandle::noop();
    if config.temporal_enabled {
        match runtime::connect(&temporal_address, &config.temporal_namespace).await {
            Ok(client) => {
                let activities = DeployActivities::new(deploy_deps.clone());
                match runtime::spawn_worker(client.clone(), &config.task_queue, activities) {
                    Ok(h) => {
                        _worker_handle = h;
                        tracing::info!(task_queue = %config.task_queue, "Temporal Worker 已启动");
                    }
                    Err(e) => tracing::error!(error = %e.msg, "Temporal Worker 启动失败"),
                }
                temporal_client = Some(client);
            }
            Err(e) => tracing::warn!(
                address = %temporal_address,
                error = %e.msg,
                "Temporal 不可用，部署启动/停止接口将返回错误"
            ),
        }
    } else {
        tracing::warn!("Temporal 已禁用（VOS_MODELINFER_TEMPORAL_ENABLED=false）");
    }

    // ---- 进程级健康巡检 + 自动重启调度器（同一周期：先巡检、后自动重启决策）----
    // 自动重启需要 Temporal 客户端下发停止 / 启动工作流；不可用时只记录判定结果。
    let lifecycle_control: Option<std::sync::Arc<dyn modelinfer::deploy::control::Control>> =
        temporal_client.clone().map(|client| {
            std::sync::Arc::new(modelinfer::deploy::control::TemporalControl::new(
                client,
                config.task_queue.clone(),
            )) as std::sync::Arc<dyn modelinfer::deploy::control::Control>
        });
    modelinfer::deploy::health::spawn_scheduler(
        deploy_deps.clone(),
        config.health_interval_secs,
        modelinfer::deploy::health::AutoRestartOptions {
            control: lifecycle_control,
            config: modelinfer::deploy::autorestart::AutoRestartConfig {
                backoff: modelinfer::deploy::autorestart::BackoffPolicy {
                    base_secs: config.auto_restart_backoff_secs as i64,
                    max_secs: config.auto_restart_backoff_max_secs as i64,
                },
            },
        },
    );

    // ---- 节点监控：consul 注册 reconcile 调度器（进程级，只对齐注册信息）----
    let monitor_config = config.monitor();
    let consul_client: Arc<dyn modelinfer::monitor::consul::ConsulClient> =
        Arc::new(modelinfer::monitor::consul::RealConsulClient::new(monitor_config.consul()));
    modelinfer::monitor::spawn_reconcile_scheduler(
        conn.clone(),
        consul_client.clone(),
        monitor_config.reconcile_secs,
    );
    // ---- 运行配置推理指标：consul 注册 reconcile 调度器（同一周期配置）----
    modelinfer::inference::spawn_reconcile_scheduler(
        conn.clone(),
        consul_client.clone(),
        monitor_config.reconcile_secs,
    );
    // ---- 节点监控：健康自愈调度器（内容校验失败 → 重建容器 + 重注册）----
    modelinfer::monitor::spawn_health_scheduler(
        conn.clone(),
        ssh.clone(),
        consul_client.clone(),
        monitor_config.clone(),
        monitor_config.reconcile_secs,
    );

    let state = AppState {
        db: conn,
        ssh,
        image_hub: config.image_hub(),
        platform,
        plaza: Arc::new(RealPlazaClient::new()),
        plaza_card_cache: crate::plaza::new_card_cache(),
        frontend_dir: config.frontend_dir.clone(),
        url_base: url_base.clone(),
        token_path,
        model_hub_path: config.model_hub_path.clone(),
        model_hub_dir: config.model_hub_dir.clone(),
        default_mount_dir: config.default_mount_dir.clone(),
        nfs_server: config.nfs_server.clone(),
        plaza_server: config.plaza_server.clone(),
        plaza_dir: config.plaza_dir(),
        temporal: temporal_client,
        temporal_task_queue: config.task_queue.clone(),
        health_interval_secs: config.health_interval_secs,
        settings_defaults: config.observability_defaults(),
        vrouter_client,
        vrouter_config,
        local_proxy,
        monitor: monitor_config,
        prometheus: None,
        version: env!("CARGO_PKG_VERSION"),
    };

    let app = build_router(state);
    let addr: SocketAddr = ([0, 0, 0, 0], config.port).into();
    tracing::info!(
        %addr,
        url_base = %if url_base.is_empty() { "/" } else { &url_base },
        platform_api = %base_url,
        // 脱敏：不回显数据库密码
        database = %modelinfer::config::mask_database_url(&database_url),
        "modelinfer 启动"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
