use tracing_subscriber::EnvFilter;

/// 初始化 tracing-subscriber：结构化日志，支持 RUST_LOG（优先） / --log-level
pub fn init_logging(log_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{log_level},sqlx=warn,sea_orm=warn")));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
}
