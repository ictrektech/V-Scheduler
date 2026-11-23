use tracing_subscriber::EnvFilter;

/// 初始化 tracing-subscriber：结构化日志，支持 RUST_LOG（优先） / --log-level
///
/// `sqlx` 的 query INFO 日志（SQL 摘要）在平台 postgres 场景刷屏（schema 探测大量
/// information_schema 查询），默认压到 warn；用户显式设置 RUST_LOG 时尊重用户配置。
pub fn init_logging(log_level: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(format!("{log_level},sqlx=warn")));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
}