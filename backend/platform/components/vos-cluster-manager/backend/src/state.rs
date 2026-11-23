//! 应用共享状态

use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::etcd::credential_store::CredentialStore;
use crate::etcd::security::{CredentialLayout, EtcdAuthMode};
use crate::etcd::{
    EtcdAuthConnector, EtcdClusterClient, UnavailableEtcdClusterClient,
};
use crate::projects::{MockProjectsClient, ProjectsClient};
use crate::remote::{MockRemoteNodeClient, RemoteNodeClient, default_remote_app_port};

/// etcd 认证/TLS 运行期上下文（bootstrap / 状态 / 轮换 / 凭据分发）
#[derive(Clone)]
pub struct EtcdAuthState {
    /// 客户端认证模式（`disabled` 表示兼容窗口）
    pub mode: EtcdAuthMode,
    /// 凭据 / PKI 布局（`/etc/vos/etcd`）
    pub layout: CredentialLayout,
    /// 集群 etcd 端点（未归一化，按模式补 scheme）
    pub endpoints: Vec<String>,
    /// 本机消费的身份名（默认 `vcm`）
    pub identity: String,
    /// 本机凭据存储
    pub store: CredentialStore,
    /// 连接工厂（None = 未装配，认证接口返回可操作错误）
    pub connector: Option<Arc<dyn EtcdAuthConnector>>,
}

impl EtcdAuthState {
    /// 以凭据根目录构造（未装配连接工厂）
    pub fn new(layout: CredentialLayout, endpoints: Vec<String>, identity: impl Into<String>) -> Self {
        Self {
            mode: EtcdAuthMode::Disabled,
            store: CredentialStore::new(layout.clone()),
            layout,
            endpoints,
            identity: identity.into(),
            connector: None,
        }
    }
}

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// 数据目录（管理面本地状态持久化根）
    pub data_dir: PathBuf,
    /// 前端 dist 目录（运行时托管，禁止编译期嵌入）
    pub frontend_dir: PathBuf,
    /// 归一化后的 url_base（形如 "/app/com.ictrek.vos-cluster-manager" 或 ""）
    pub url_base: String,
    /// etcd 端点列表（集群状态权威信源地址；health 的 etcd_configured 依据）
    pub etcd_endpoints: Vec<String>,
    /// etcd 业务客户端（Real / Unavailable 降级；依赖注入保证可测）
    pub etcd_cluster_client: Arc<dyn EtcdClusterClient>,
    /// 平台 projects API 客户端（Phase 2+ 子项目编排）
    pub projects_client: Arc<dyn ProjectsClient>,
    /// 平台 temporal 地址（`VOS_HOST_GW_IP:VOS_TEMPORAL_PORT`；未注入 None → worker 不启动）
    pub temporal_address: Option<String>,
    /// 跨节点执行客户端（Phase 4 编排用；目标节点须安装本应用）
    pub remote_client: Arc<dyn RemoteNodeClient>,
    /// 目标节点应用 HTTP 端口（host 网络直连；env REMOTE_APP_PORT 覆盖，缺省 19906）
    pub remote_app_port: u16,
/// 管理面状态库（平台 postgres；未注入 VOS_POSTGRES_* 时为 None 降级）
    pub db: Option<DatabaseConnection>,
    /// 集群创建并发防重（进程内 in-flight cluster_name 集合；双发 create 时
    /// 第二个请求被 20506 拒绝，避免两个 workflow 竞争部署互毁）
    pub create_inflight: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// etcd 认证/TLS 上下文（D9）
    pub etcd_auth: EtcdAuthState,
    /// 组件版本（health/响应透出；cargo pkg version）
    pub version: &'static str,
}

impl AppState {
    /// 骨架期构造（兼容测试与旧调用）：etcd/projects 不可用 + 无状态库
    pub fn new(
        data_dir: PathBuf,
        frontend_dir: PathBuf,
        url_base: String,
        etcd_endpoints: Vec<String>,
    ) -> Self {
        let auth_endpoints = etcd_endpoints.clone();
        Self {
            data_dir,
            frontend_dir,
            url_base,
            etcd_endpoints,
            etcd_cluster_client: Arc::new(UnavailableEtcdClusterClient::new(
                "AppState::new 骨架构造（未注入 etcd client）",
            )),
            projects_client: Arc::new(MockProjectsClient::new()),
            temporal_address: None,
            remote_client: Arc::new(MockRemoteNodeClient::new()),
            remote_app_port: default_remote_app_port(),
            db: None,
            create_inflight: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            etcd_auth: EtcdAuthState::new(
                CredentialLayout::default_root(),
                auth_endpoints,
                crate::etcd::security::DEFAULT_VCM_IDENTITY,
            ),
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// 装配 etcd 认证上下文（builder 风格，避免改动既有 `with_runtime` 调用点）
    pub fn with_etcd_auth(mut self, auth: EtcdAuthState) -> Self {
        self.etcd_auth = auth;
        self
    }

    /// 运行期构造：注入 etcd / projects 客户端、temporal 地址、跨节点客户端与管理面状态库
    #[allow(clippy::too_many_arguments)]
    pub fn with_runtime(
        data_dir: PathBuf,
        frontend_dir: PathBuf,
        url_base: String,
        etcd_endpoints: Vec<String>,
        etcd_cluster_client: Arc<dyn EtcdClusterClient>,
        projects_client: Arc<dyn ProjectsClient>,
        temporal_address: Option<String>,
        remote_client: Arc<dyn RemoteNodeClient>,
        remote_app_port: u16,
        db: Option<DatabaseConnection>,
    ) -> Self {
        let auth_endpoints = etcd_endpoints.clone();
        Self {
            data_dir,
            frontend_dir,
            url_base,
            etcd_endpoints,
            etcd_cluster_client,
            projects_client,
            temporal_address,
            remote_client,
            remote_app_port,
            db,
            create_inflight: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            etcd_auth: EtcdAuthState::new(
                CredentialLayout::default_root(),
                auth_endpoints,
                crate::etcd::security::DEFAULT_VCM_IDENTITY,
            ),
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// 本机凭据文件是否已就位（health / status 展示）
    pub fn etcd_credential_present(&self) -> bool {
        self.etcd_auth.store.exists(&self.etcd_auth.identity)
    }

    /// 管理面状态库是否可用（供 health / status 聚合）
    pub fn db_ok(&self) -> bool {
        self.db.is_some()
    }
}