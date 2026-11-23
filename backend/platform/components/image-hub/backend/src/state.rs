//! 应用共享状态

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::registry_host_from_url;
use crate::gc::{MockRegistryGcRunner, RealRegistryGcRunner, RegistryGcRunner};
use crate::jobs::JobManager;
use crate::platform::{
    PlatformClient, RealPlatformClient, context_token_path, platform_base_from_env,
};
use crate::registry::{RealRegistryClient, RegistryClient};
use crate::settings::{FileSettingsStore, MemorySettingsStore, SettingsStore};
use crate::skopeo::{MockSkopeoRunner, RealSkopeoRunner, SkopeoRunner};

/// 应用共享状态（依赖全部 trait 注入，便于单测）
#[derive(Clone)]
pub struct AppState {
    /// 数据目录（本地管理状态根；镜像数据在 registry 容器卷，不在此列）
    pub data_dir: PathBuf,
    /// 前端 dist 目录（运行时托管，禁止编译期嵌入）
    pub frontend_dir: PathBuf,
    /// 归一化后的 url_base（形如 "/app/com.ictrek.image-hub" 或 ""）
    pub url_base: String,
    /// registry 数据面基地址（管理面经此访问 v2 API）
    pub registry_url: String,
    /// registry 数据面 host（含端口，skopeo 目标 `docker://<host>/...`）
    pub registry_host: String,
    /// registry 对宿主暴露的端口（前端拼地址用）
    pub registry_port: u16,
    /// registry 数据面客户端（Real=reqwest；单测 Mock）
    pub registry: Arc<dyn RegistryClient>,
    /// 平台只读 API 客户端（取设备网卡地址列表）
    pub platform: Arc<dyn PlatformClient>,
    /// 管理面设置存储（上游认证 + 代理）
    pub settings: Arc<dyn SettingsStore>,
    /// skopeo 执行器（拉取/同步编排）
    pub skopeo: Arc<dyn SkopeoRunner>,
    /// registry 垃圾回收执行器（内置 registry 二进制，D-IH-59）
    pub gc: Arc<dyn RegistryGcRunner>,
    /// 异步任务管理器（拉取/同步/克隆/改名/导入/GC 任务）
    pub jobs: Arc<JobManager>,
    /// 组件版本（health/响应透出；cargo pkg version）
    pub version: &'static str,
}

impl AppState {
    /// 生产构造：Real registry / 平台客户端 / skopeo + GC 执行器 + 文件设置存储 + 任务管理器
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        data_dir: PathBuf,
        frontend_dir: PathBuf,
        url_base: String,
        registry_url: String,
        registry_port: u16,
        skopeo_bin: String,
        registry_bin: String,
        registry_storage_dir: PathBuf,
    ) -> Self {
        let env: std::collections::HashMap<String, String> = std::env::vars().collect();
        let base_url = platform_base_from_env(&env);
        let token_path = context_token_path(&env);
        let registry_host = registry_host_from_url(&registry_url);
        let settings: Arc<dyn SettingsStore> =
            Arc::new(FileSettingsStore::new(data_dir.join("settings.json")));
        let skopeo: Arc<dyn SkopeoRunner> = Arc::new(RealSkopeoRunner::new(skopeo_bin));
        let gc: Arc<dyn RegistryGcRunner> =
            Arc::new(RealRegistryGcRunner::new(registry_bin, registry_storage_dir));
        let registry: Arc<dyn RegistryClient> = Arc::new(RealRegistryClient::new(registry_url.clone()));
        let jobs = Arc::new(JobManager::new(
            Arc::clone(&skopeo),
            Arc::clone(&gc),
            Arc::clone(&settings),
            Arc::clone(&registry),
            registry_host.clone(),
            data_dir.clone(),
        ));
        Self {
            data_dir,
            frontend_dir,
            url_base,
            registry,
            platform: Arc::new(RealPlatformClient::new(base_url, token_path)),
            registry_url,
            registry_host,
            registry_port,
            settings,
            skopeo,
            gc,
            jobs,
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// 测试装配：注入 Mock registry / platform + 内存设置 + Mock skopeo + Mock GC
    #[must_use]
    pub fn for_test(
        frontend_dir: PathBuf,
        registry: Arc<dyn RegistryClient>,
        platform: Arc<dyn PlatformClient>,
    ) -> Self {
        Self::for_test_full(
            frontend_dir,
            registry,
            platform,
            Arc::new(MemorySettingsStore::new()),
            Arc::new(MockSkopeoRunner::new()),
            Arc::new(MockRegistryGcRunner::new()),
        )
    }

    /// 测试装配（可注入自定义设置存储 / skopeo 执行器；GC 用默认 Mock）
    #[must_use]
    pub fn for_test_with(
        frontend_dir: PathBuf,
        registry: Arc<dyn RegistryClient>,
        platform: Arc<dyn PlatformClient>,
        settings: Arc<dyn SettingsStore>,
        skopeo: Arc<dyn SkopeoRunner>,
    ) -> Self {
        Self::for_test_full(
            frontend_dir,
            registry,
            platform,
            settings,
            skopeo,
            Arc::new(MockRegistryGcRunner::new()),
        )
    }

    /// 测试装配（可注入 GC 执行器；skopeo 用默认 Mock）
    #[must_use]
    pub fn for_test_with_gc(
        frontend_dir: PathBuf,
        registry: Arc<dyn RegistryClient>,
        platform: Arc<dyn PlatformClient>,
        settings: Arc<dyn SettingsStore>,
        gc: Arc<dyn RegistryGcRunner>,
    ) -> Self {
        Self::for_test_full(
            frontend_dir,
            registry,
            platform,
            settings,
            Arc::new(MockSkopeoRunner::new()),
            gc,
        )
    }

    /// 测试装配（全量注入）
    #[must_use]
    pub fn for_test_full(
        frontend_dir: PathBuf,
        registry: Arc<dyn RegistryClient>,
        platform: Arc<dyn PlatformClient>,
        settings: Arc<dyn SettingsStore>,
        skopeo: Arc<dyn SkopeoRunner>,
        gc: Arc<dyn RegistryGcRunner>,
    ) -> Self {
        let data_dir = unique_test_data_dir();
        let _ = std::fs::create_dir_all(&data_dir);
        let registry_url = "http://127.0.0.1:19913".to_string();
        let registry_host = registry_host_from_url(&registry_url);
        let jobs = Arc::new(JobManager::new(
            Arc::clone(&skopeo),
            Arc::clone(&gc),
            Arc::clone(&settings),
            Arc::clone(&registry),
            registry_host.clone(),
            data_dir.clone(),
        ));
        Self {
            data_dir,
            frontend_dir,
            url_base: String::new(),
            registry_url,
            registry_host,
            registry_port: 19913,
            registry,
            platform,
            settings,
            skopeo,
            gc,
            jobs,
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// 覆盖 url_base（子路径托管测试）
    #[must_use]
    pub fn with_url_base(mut self, url_base: impl Into<String>) -> Self {
        self.url_base = url_base.into();
        self
    }
}

/// 单测用唯一数据目录（避免并行测试共享 jobs.json）
fn unique_test_data_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "image-hub-test-{}-{millis}-{seq}",
        std::process::id()
    ))
}