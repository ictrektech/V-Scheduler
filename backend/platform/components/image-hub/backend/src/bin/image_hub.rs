use std::net::SocketAddr;

use clap::Parser;
use image_hub::{AppState, Config, build_router, logging, scheduler::GcScheduler};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    logging::init_logging(&config.log_level);

    // 确保数据目录存在（本地管理状态根）
    std::fs::create_dir_all(&config.data_dir)
        .map_err(|e| anyhow::anyhow!("创建数据目录 {} 失败: {e}", config.data_dir.display()))?;

    // 确保 skopeo 临时目录存在于应用数据目录（避免写入容器可写层 / docker 分区）
    let tmp_dir = config.data_dir.join("tmp");
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| anyhow::anyhow!("创建临时目录 {} 失败: {e}", tmp_dir.display()))?;

    let url_base = config.normalized_url_base();

    let state = AppState::new(
        config.data_dir.clone(),
        config.frontend_dir.clone(),
        url_base.clone(),
        config.registry_url.clone(),
        config.registry_port,
        config.skopeo_bin.clone(),
        config.registry_bin.clone(),
        config.registry_storage_dir.clone(),
    );

    // registry 垃圾回收自动调度（默认关闭；启用后按间隔触发，D-IH-62）
    let scheduler = GcScheduler::new(state.clone());
    tokio::spawn(scheduler.run());

    let app = build_router(state);

    let bind_address = std::env::var("VOS_IMAGE_HUB_BIND_ADDRESS")
        .unwrap_or_else(|_| "0.0.0.0".to_owned())
        .parse::<std::net::IpAddr>()?;
    let addr = SocketAddr::new(bind_address, config.port);
    tracing::info!(
        %addr,
        url_base = %if url_base.is_empty() { "/" } else { &url_base },
        registry_url = %config.registry_url,
        "image-hub 启动（管理面；镜像数据面为官方 registry 19913）"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}