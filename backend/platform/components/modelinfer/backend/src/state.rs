//! 应用共享状态（依赖以 trait 注入，便于单测）

use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::observability::SettingsDefaults;
use crate::platform::PlatformClient;
use crate::plaza::PlazaClient;
use crate::ssh::SshExec;
use crate::vrouter::{VrouterClient, VrouterConfig};

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// 数据库连接（生产仅 postgres；单测为内存 sqlite）
    pub db: DatabaseConnection,
    /// SSH 执行器
    pub ssh: Arc<dyn SshExec>,
    /// 启动前镜像检查与异步分发使用相同的 registry 配置
    pub image_hub: crate::images::ImageHubConfig,
    /// 平台 API 客户端
    pub platform: Arc<dyn PlatformClient>,
    /// 模型广场客户端
    pub plaza: Arc<dyn PlazaClient>,
    /// 广场卡片索引短 TTL 缓存（`source_id` → 卡片列表；分页后避免每页都拉上游）
    pub plaza_card_cache: crate::plaza::CardCache,
    /// 前端 dist 目录
    pub frontend_dir: PathBuf,
    /// 归一化后的 url_base（形如 "/app/com.ictrek.modelinfer" 或 ""）
    pub url_base: String,
    /// 应用上下文 token 路径（`VOS_APP_CONTEXT_PATH/access-token`）
    pub token_path: Option<PathBuf>,
    /// model hub 路径兜底（平台配置不可用时）
    pub model_hub_path: String,
    /// model-hub 共享目录在容器内的挂载点
    pub model_hub_dir: String,
    /// 节点默认挂载点
    pub default_mount_dir: String,
    /// NFS 服务端 IP 覆盖
    pub nfs_server: String,
    /// 模型广场服务端地址
    pub plaza_server: String,
    /// 模型广场本地存储根目录
    pub plaza_dir: PathBuf,
    /// Temporal 客户端（未启用 / 连接失败时为 None）
    pub temporal: Option<Arc<temporalio_client::Client>>,
    /// Temporal task queue
    pub temporal_task_queue: String,
    /// 健康巡检周期（秒）
    pub health_interval_secs: u64,
    /// 运行可观测性保留策略默认值
    pub settings_defaults: SettingsDefaults,
    /// 统一网关 HTTP 客户端（自动注册 / 续约 / 注销）
    pub vrouter_client: Arc<dyn VrouterClient>,
    /// 统一网关集成配置
    pub vrouter_config: VrouterConfig,
    /// 节点监控集成配置（consul / prometheus / node-exporter 镜像 / reconcile 周期）
    pub monitor: crate::monitor::MonitorConfig,
    /// prometheus 客户端覆盖（单测注入 Mock；`None` = 按 `monitor.prometheus()` 构造真实客户端）
    pub prometheus: Option<Arc<dyn crate::monitor::prometheus::PrometheusClient>>,
    /// 本地反代注册表（进程级）
    pub local_proxy: crate::localproxy::ProxyRegistry,
    /// 组件版本
    pub version: &'static str,
}

impl AppState {
    /// 平台 token 是否存在（health 用）
    pub fn platform_token_present(&self) -> bool {
        self.token_path.as_ref().map(|p| p.exists()).unwrap_or(false)
    }
}
