//! 统一网关集成：检测、模型服务注册（public）、周期续约、停止注销。
//!
//! V-Scheduler（`com.ictrek.v-scheduler`）是 V-Router（`com.ictrek.v-router`）的继任者，
//! 内置同一套 Rust 网关协议（`/healthz`、`/api/v1/registrations` 等）。本模块：
//! - 检测优先选择 V-Scheduler，兼容回退旧 V-Router：平台 `GET /v1000/app/list` +
//!   `GET /healthz` 探活；
//! - 地址 / 凭据：从命中的应用配置读取 `V_ROUTER_HOST_PORT`，凭据使用应用上下文 token；
//! - 注册默认携带 `scheduler.mode=direct`（V-Scheduler 直接放行：不申请 permit、不排队、
//!   不占调度预算），modelinfer 自行负责实例运行与容量；
//! - 注册 / 续约 / 注销全部 **best-effort**，不阻断部署；
//! - 网络与平台依赖经 trait 注入，核心（请求构造 / 状态判定）为纯函数。
//!
//! 设计见 `openspec/changes/modelinfer-vscheduler-migration/design.md` 与
//! `specs/docs/design/modelinfer/access-and-vrouter.md`。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entities::deployment;
use crate::error::{ModelinferError, codes};
use crate::platform::PlatformClient;
use crate::time;

/// V-Scheduler 统一应用 id（模型仓库 + 统一网关 + 调度；V-Router 的继任者）
pub const VSCHEDULER_APP_ID: &str = "com.ictrek.v-scheduler";
/// 旧 V-Router 应用 id（兼容期保留）
pub const VROUTER_APP_ID: &str = "com.ictrek.v-router";
/// 网关应用候选（优先级从高到低）
pub const GATEWAY_APP_IDS: [&str; 2] = [VSCHEDULER_APP_ID, VROUTER_APP_ID];
/// 默认宿主端口（V-Scheduler / 旧 V-Router 的 `V_ROUTER_HOST_PORT` 默认值）
pub const DEFAULT_VROUTER_PORT: u16 = 22888;
/// 默认 docker 网关 IP（`VOS_HOST_GW_IP`，容器 / host 均可经它访问宿主发布端口）
pub const DEFAULT_HOST_GATEWAY_IP: &str = "172.17.0.1";
/// 默认注册 TTL（秒）
pub const DEFAULT_TTL_SECS: u64 = 90;
/// 默认注册可见范围
pub const VISIBILITY_PUBLIC: &str = "public";
/// 上游 ID 前缀（统一网关生成 `registered-<service_id>`）
pub const UPSTREAM_ID_PREFIX: &str = "registered-";
/// 统一网关模型 ID 前缀（主动注册来源）
pub const MODEL_ID_PREFIX: &str = "registered/";
/// 认证失败标记（自动重试抑制：`vrouter_last_error` 以该前缀开头时不再自动重试）
pub const AUTH_ERROR_PREFIX: &str = "认证失败";

/// 注册状态
pub mod state {
    pub const DISABLED: &str = "disabled";
    pub const UNREGISTERED: &str = "unregistered";
    pub const REGISTERED: &str = "registered";
    pub const ERROR: &str = "error";
}

/// 统一网关集成配置
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VrouterConfig {
    /// 注册 TTL（秒，`30..=3600`）
    pub ttl_secs: u64,
    /// 平台应用上下文 access-token 路径（注册凭据；`VOS_APP_CONTEXT_PATH/access-token`）
    pub token_path: Option<std::path::PathBuf>,
    /// docker 网关 IP（`VOS_HOST_GW_IP`，默认 `172.17.0.1`）；用于拼接统一网关地址
    pub host_gateway_ip: String,
}

impl Default for VrouterConfig {
    fn default() -> Self {
        Self {
            ttl_secs: DEFAULT_TTL_SECS,
            token_path: None,
            host_gateway_ip: DEFAULT_HOST_GATEWAY_IP.to_string(),
        }
    }
}

impl VrouterConfig {
    /// 归一化 TTL（`30..=3600`，否则默认）
    pub fn normalized_ttl(&self) -> u64 {
        if (30..=3600).contains(&self.ttl_secs) {
            self.ttl_secs
        } else {
            DEFAULT_TTL_SECS
        }
    }
}

/// 注册目标（地址 + 凭据）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VrouterTarget {
    pub base_url: String,
    /// 平台应用上下文 access-token（Bearer）
    pub token: String,
}

/// 统一网关调度策略（注册请求 `scheduler` 字段）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerPolicy {
    /// 调度模式：`direct` = 直接放行（跳过调度准入 / permit / 排队 / 预算）
    pub mode: String,
}

impl SchedulerPolicy {
    /// 直接放行策略（默认）：注册上游的请求由统一网关直接转发到模型服务
    pub fn direct() -> Self {
        Self {
            mode: "direct".to_string(),
        }
    }
}

/// 注册请求（单模型声明）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationRequest {
    pub service_id: String,
    pub name: String,
    pub visibility: String,
    pub base_url: String,
    /// 模型服务自身的 API Key（统一网关调用上游时使用；空表示不鉴权）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    pub protocol: String,
    pub model: String,
    pub model_kind: String,
    pub endpoints: Vec<String>,
    pub capabilities: Vec<String>,
    /// 调度策略（默认 `direct`：直接放行，不参与调度准入）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<SchedulerPolicy>,
    pub ttl_seconds: u64,
}

/// 由部署 id 派生稳定 `service_id`（`modelinfer-<deployment_id>`）
pub fn service_id_of(deployment_id: &str) -> String {
    format!("modelinfer-{deployment_id}")
}

/// 由 `service_id` 派生上游 ID（与统一网关生成规则一致）
pub fn upstream_id_of(service_id: &str) -> String {
    format!("{UPSTREAM_ID_PREFIX}{service_id}")
}

/// 由模型名派生统一网关模型 ID（主动注册来源）
pub fn model_id_of(model: &str) -> String {
    format!("{MODEL_ID_PREFIX}{model}")
}

/// 构造单模型注册请求（纯函数）
///
/// `base_url` 为从统一网关容器可达的模型服务地址（无尾 `/`）。
/// 注册默认携带 `scheduler.mode=direct`：V-Scheduler 直接放行该上游，不申请 permit、
/// 不排队、不占调度预算（modelinfer 自行负责实例运行与容量）。
pub fn build_registration(
    deployment_id: &str,
    name: &str,
    base_url: &str,
    model: &str,
    upstream_api_key: &str,
    ttl_secs: u64,
) -> RegistrationRequest {
    RegistrationRequest {
        service_id: service_id_of(deployment_id),
        name: if name.trim().is_empty() {
            service_id_of(deployment_id)
        } else {
            name.trim().to_string()
        },
        visibility: VISIBILITY_PUBLIC.to_string(),
        base_url: base_url.trim_end_matches('/').to_string(),
        api_key: upstream_api_key.trim().to_string(),
        protocol: "chat_completions".to_string(),
        model: model.trim().to_string(),
        model_kind: "chat".to_string(),
        endpoints: vec![
            "chat_completions".into(),
            "responses".into(),
            "messages".into(),
        ],
        capabilities: vec!["chat".into(), "completion".into()],
        scheduler: Some(SchedulerPolicy::direct()),
        ttl_seconds: ttl_secs,
    }
}

/// 续约结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatOutcome {
    /// 续约成功
    Ok { heartbeat_after_secs: u64 },
    /// 注册已失效（404 / 403）：需重新注册
    NotFound,
    /// 凭据无效（401）：停止自动重试
    Unauthorized,
}

/// 注册结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationOutcome {
    pub upstream_id: String,
    pub model_id: String,
    pub heartbeat_after_secs: u64,
}

/// 统一网关 HTTP 客户端抽象（便于单测注入）
#[async_trait]
pub trait VrouterClient: Send + Sync {
    /// `GET /healthz` 探活（2xx → true）
    async fn healthz(&self, base_url: &str) -> Result<bool, ModelinferError>;
    /// `POST /api/v1/registrations`
    async fn register(
        &self,
        target: &VrouterTarget,
        req: &RegistrationRequest,
    ) -> Result<RegistrationOutcome, ModelinferError>;
    /// `POST /api/v1/registrations/{id}/heartbeat`
    async fn heartbeat(
        &self,
        target: &VrouterTarget,
        upstream_id: &str,
        ttl_secs: u64,
    ) -> Result<HeartbeatOutcome, ModelinferError>;
    /// `DELETE /api/v1/registrations/{id}`（404 视为成功）
    async fn deregister(&self, target: &VrouterTarget, upstream_id: &str) -> Result<(), ModelinferError>;
}

fn http_err(msg: String) -> ModelinferError {
    ModelinferError::new(codes::VROUTER_UNAVAILABLE, msg)
}

/// 生产实现（reqwest）
pub struct RealVrouterClient {
    client: reqwest::Client,
}

impl Default for RealVrouterClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RealVrouterClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self { client }
    }

    fn bearer(&self, req: reqwest::RequestBuilder, target: &VrouterTarget) -> reqwest::RequestBuilder {
        if target.token.trim().is_empty() {
            req
        } else {
            req.bearer_auth(target.token.trim())
        }
    }
}

#[async_trait]
impl VrouterClient for RealVrouterClient {
    async fn healthz(&self, base_url: &str) -> Result<bool, ModelinferError> {
        let url = format!("{}/healthz", base_url.trim_end_matches('/'));
        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(e) => Err(http_err(format!("探测统一网关 {url} 失败: {e}"))),
        }
    }

    async fn register(
        &self,
        target: &VrouterTarget,
        req: &RegistrationRequest,
    ) -> Result<RegistrationOutcome, ModelinferError> {
        let url = format!("{}/api/v1/registrations", target.base_url.trim_end_matches('/'));
        let resp = self
            .bearer(self.client.post(&url).json(req), target)
            .send()
            .await
            .map_err(|e| http_err(format!("注册统一网关失败（{url}）: {e}")))?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(http_err(format!(
                "注册统一网关返回 {}: {}",
                status,
                error_message(&body)
            )));
        }
        let upstream = body.get("upstream").cloned().unwrap_or(Value::Null);
        let computed_upstream = upstream_id_of(&req.service_id);
        let upstream_id = upstream
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(computed_upstream);
        let model_id = upstream
            .get("model_ids")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find_map(Value::as_str))
            .map(|raw| {
                // 统一网关返回原始模型 id；主动注册来源的对外模型 id 为 `registered/<model>`
                if raw.starts_with(MODEL_ID_PREFIX) {
                    raw.to_string()
                } else {
                    model_id_of(raw)
                }
            })
            .unwrap_or_else(|| model_id_of(&req.model));
        Ok(RegistrationOutcome {
            upstream_id,
            model_id,
            heartbeat_after_secs: heartbeat_after_secs(&body, req.ttl_seconds),
        })
    }

    async fn heartbeat(
        &self,
        target: &VrouterTarget,
        upstream_id: &str,
        ttl_secs: u64,
    ) -> Result<HeartbeatOutcome, ModelinferError> {
        let url = format!(
            "{}/api/v1/registrations/{upstream_id}/heartbeat",
            target.base_url.trim_end_matches('/')
        );
        let resp = self
            .bearer(
                self.client
                    .post(&url)
                    .json(&serde_json::json!({ "ttl_seconds": ttl_secs })),
                target,
            )
            .send()
            .await
            .map_err(|e| http_err(format!("续约统一网关失败（{url}）: {e}")))?;
        match resp.status().as_u16() {
            200..=299 => {
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                Ok(HeartbeatOutcome::Ok {
                    heartbeat_after_secs: heartbeat_after_secs(&body, ttl_secs),
                })
            }
            401 | 403 => Ok(HeartbeatOutcome::Unauthorized),
            404 => Ok(HeartbeatOutcome::NotFound),
            s => Err(http_err(format!("续约统一网关返回 {s}"))),
        }
    }

    async fn deregister(&self, target: &VrouterTarget, upstream_id: &str) -> Result<(), ModelinferError> {
        let url = format!(
            "{}/api/v1/registrations/{upstream_id}",
            target.base_url.trim_end_matches('/')
        );
        let resp = self
            .bearer(self.client.delete(&url), target)
            .send()
            .await
            .map_err(|e| http_err(format!("注销统一网关失败（{url}）: {e}")))?;
        let status = resp.status();
        // 404：注册已过期 / 不存在，视为成功（幂等注销）
        if status.is_success() || status.as_u16() == 404 || status.as_u16() == 409 {
            return Ok(());
        }
        Err(http_err(format!("注销统一网关返回 {status}")))
    }
}

fn error_message(body: &Value) -> String {
    body.get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .or_else(|| body.get("msg").and_then(Value::as_str))
        .unwrap_or("(no message)")
        .to_string()
}

fn heartbeat_after_secs(body: &Value, ttl: u64) -> u64 {
    body.get("heartbeat_after_seconds")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .unwrap_or(ttl / 3)
}

/// Mock 客户端（单测）：记录调用、可注入行为
#[derive(Default)]
pub struct MockVrouterClient {
    inner: std::sync::Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    healthz_ok: bool,
    register_result: Option<Result<RegistrationOutcome, String>>,
    heartbeat_results: std::collections::VecDeque<HeartbeatOutcome>,
    deregister_error: Option<String>,
    calls: Vec<String>,
}

impl MockVrouterClient {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_healthz(self, ok: bool) -> Self {
        self.inner.lock().unwrap().healthz_ok = ok;
        self
    }
    pub fn with_register_ok(self, upstream_id: &str, model_id: &str) -> Self {
        self.inner.lock().unwrap().register_result = Some(Ok(RegistrationOutcome {
            upstream_id: upstream_id.into(),
            model_id: model_id.into(),
            heartbeat_after_secs: 30,
        }));
        self
    }
    pub fn with_register_err(self, msg: &str) -> Self {
        self.inner.lock().unwrap().register_result = Some(Err(msg.into()));
        self
    }
    pub fn with_heartbeat(self, outcomes: Vec<HeartbeatOutcome>) -> Self {
        self.inner.lock().unwrap().heartbeat_results = outcomes.into();
        self
    }
    pub fn with_deregister_err(self, msg: &str) -> Self {
        self.inner.lock().unwrap().deregister_error = Some(msg.into());
        self
    }
    pub fn recorded_calls(&self) -> Vec<String> {
        self.inner.lock().unwrap().calls.clone()
    }
}

#[async_trait]
impl VrouterClient for MockVrouterClient {
    async fn healthz(&self, base_url: &str) -> Result<bool, ModelinferError> {
        let mut s = self.inner.lock().unwrap();
        s.calls.push(format!("healthz {base_url}"));
        Ok(s.healthz_ok)
    }

    async fn register(
        &self,
        target: &VrouterTarget,
        req: &RegistrationRequest,
    ) -> Result<RegistrationOutcome, ModelinferError> {
        let mut s = self.inner.lock().unwrap();
        s.calls.push(format!(
            "register {} {} {} {}",
            target.base_url,
            req.service_id,
            req.model,
            req.scheduler.as_ref().map(|p| p.mode.as_str()).unwrap_or("-")
        ));
        match s.register_result.clone() {
            Some(Ok(o)) => Ok(o),
            Some(Err(m)) => Err(http_err(m)),
            None => Ok(RegistrationOutcome {
                upstream_id: upstream_id_of(&req.service_id),
                model_id: model_id_of(&req.model),
                heartbeat_after_secs: 30,
            }),
        }
    }

    async fn heartbeat(
        &self,
        target: &VrouterTarget,
        upstream_id: &str,
        _ttl_secs: u64,
    ) -> Result<HeartbeatOutcome, ModelinferError> {
        let mut s = self.inner.lock().unwrap();
        s.calls.push(format!("heartbeat {} {upstream_id}", target.base_url));
        Ok(s.heartbeat_results
            .pop_front()
            .unwrap_or(HeartbeatOutcome::Ok {
                heartbeat_after_secs: 30,
            }))
    }

    async fn deregister(&self, target: &VrouterTarget, upstream_id: &str) -> Result<(), ModelinferError> {
        let mut s = self.inner.lock().unwrap();
        s.calls.push(format!("deregister {} {upstream_id}", target.base_url));
        match s.deregister_error.clone() {
            Some(m) => Err(http_err(m)),
            None => Ok(()),
        }
    }
}

/// 统一网关检测 / 运行状态
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VrouterStatus {
    pub installed: bool,
    pub reachable: bool,
    pub credential_present: bool,
    pub registered_count: u64,
    pub message: String,
}

/// 检测结果 + 注册目标
#[derive(Debug, Clone)]
pub struct VrouterResolution {
    pub status: VrouterStatus,
    pub target: VrouterTarget,
}

/// 从统一网关应用配置（`configs_json`）读取字符串配置项
pub fn config_str(configs: &Value, key: &str) -> Option<String> {
    configs
        .get(key)
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 解析统一网关地址（应用配置端口 + docker 网关 IP）
///
/// 不使用 `127.0.0.1`：modelinfer 经 docker 网关 IP（`VOS_HOST_GW_IP`，默认 `172.17.0.1`）
/// 访问统一网关暴露到宿主机的端口，兼容 host / 容器两种网络视角。
pub fn resolve_url(config: &VrouterConfig, vrouter_configs: &Value) -> String {
    let port = config_str(vrouter_configs, "V_ROUTER_HOST_PORT")
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(DEFAULT_VROUTER_PORT);
    let host = config.host_gateway_ip.trim();
    let host = if host.is_empty() {
        DEFAULT_HOST_GATEWAY_IP
    } else {
        host
    };
    format!("http://{host}:{port}")
}

/// 解析注册凭据：读取平台应用上下文 access-token（`VOS_APP_CONTEXT_PATH/access-token`）
pub fn resolve_token(config: &VrouterConfig) -> String {
    match &config.token_path {
        Some(p) => std::fs::read_to_string(p)
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        None => String::new(),
    }
}

/// 从已安装应用列表中选择统一网关：优先 V-Scheduler，回退旧 V-Router。
///
/// 仅 `state = installed` 的应用视为可用；升级中 / 其他状态在下一轮检测恢复。
pub fn gateway_app_id(apps: &[crate::platform::AppInfo]) -> Option<&'static str> {
    GATEWAY_APP_IDS
        .into_iter()
        .find(|app_id| apps.iter().any(|a| a.app_id == *app_id && a.state == "installed"))
}

/// 统一网关显示名（用于状态说明 / 日志）
fn gateway_display_name(app_id: &str) -> &'static str {
    if app_id == VSCHEDULER_APP_ID {
        "V-Scheduler 统一网关"
    } else {
        "V-Router（兼容）"
    }
}

/// 平台已安装应用列表中是否存在统一网关（V-Scheduler 或旧 V-Router，`state = installed`）
pub fn has_vrouter_app(apps: &[crate::platform::AppInfo]) -> bool {
    gateway_app_id(apps).is_some()
}

/// 解析注册目标与可用性（检测 + 地址 + 凭据 + 探活）
pub async fn resolve_target_and_status(
    platform: &dyn PlatformClient,
    client: &dyn VrouterClient,
    config: &VrouterConfig,
) -> VrouterResolution {
    let token = resolve_token(config);
    let mut installed = false;
    let mut url = String::new();
    let mut reachable = false;
    let mut gateway_name = gateway_display_name(VSCHEDULER_APP_ID);

    match platform.list_apps().await {
        Ok(apps) => {
            if let Some(app_id) = gateway_app_id(&apps) {
                installed = true;
                gateway_name = gateway_display_name(app_id);
                let configs = platform
                    .app_configs_json(app_id)
                    .await
                    .unwrap_or(Value::Null);
                url = resolve_url(config, &configs);
                reachable = client.healthz(&url).await.unwrap_or(false);
            }
        }
        Err(_) => {
            // 平台列表不可用：按「可能已安装」处理，依次探活 V-Scheduler / 旧 V-Router，
            // 选择第一个可达网关；均不可达时保留第一个候选地址用于状态说明。
            installed = true;
            for app_id in GATEWAY_APP_IDS {
                let configs = platform
                    .app_configs_json(app_id)
                    .await
                    .unwrap_or(Value::Null);
                let candidate = resolve_url(config, &configs);
                if client.healthz(&candidate).await.unwrap_or(false) {
                    url = candidate;
                    reachable = true;
                    gateway_name = gateway_display_name(app_id);
                    break;
                }
                if url.is_empty() {
                    url = candidate;
                    gateway_name = gateway_display_name(app_id);
                }
            }
        }
    }

    let credential_present = !token.trim().is_empty();
    let message = if !installed {
        "未检测到已安装的 V-Scheduler 统一网关（或兼容的旧 V-Router）".to_string()
    } else if !reachable {
        format!("{gateway_name} 不可达：{url}")
    } else if !credential_present {
        "应用上下文 access-token 缺失（上下文目录未挂载），无法注册".to_string()
    } else {
        String::new()
    };
    VrouterResolution {
        status: VrouterStatus {
            installed,
            reachable,
            credential_present,
            registered_count: 0,
            message,
        },
        target: VrouterTarget {
            base_url: url,
            token,
        },
    }
}

/// 便捷：仅取状态（供只读状态查询）
pub async fn resolve_status(
    platform: &dyn PlatformClient,
    client: &dyn VrouterClient,
    config: &VrouterConfig,
) -> VrouterStatus {
    resolve_target_and_status(platform, client, config).await.status
}

fn db_err(e: sea_orm::DbErr) -> ModelinferError {
    ModelinferError::new(codes::DB_FAILED, format!("数据库操作失败: {e}"))
}

async fn set_state(
    db: &DatabaseConnection,
    current: deployment::Model,
    state: &str,
    upstream_id: Option<String>,
    model: Option<String>,
    registered_at: Option<String>,
    error: Option<String>,
) -> Result<deployment::Model, ModelinferError> {
    let mut am: deployment::ActiveModel = current.into();
    am.vrouter_state = Set(state.to_string());
    if let Some(v) = upstream_id {
        am.vrouter_upstream_id = Set(v);
    }
    if let Some(v) = model {
        am.vrouter_model = Set(v);
    }
    if registered_at.is_some() {
        am.vrouter_registered_at = Set(registered_at);
    }
    if let Some(e) = error {
        am.vrouter_last_error = Set(e);
    }
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(db_err)
}

/// 解析某运行配置的注册目标与状态（附加已注册计数）
async fn deployment_resolution(
    platform: &dyn PlatformClient,
    client: &dyn VrouterClient,
    config: &VrouterConfig,
    d: &deployment::Model,
) -> VrouterResolution {
    let mut r = resolve_target_and_status(platform, client, config).await;
    r.status.registered_count = if d.vrouter_state == state::REGISTERED { 1 } else { 0 };
    r
}

/// 尽力为某运行配置注册 / 续约统一网关。
///
/// 已注册时优先心跳续约；仅对 `state = running` 的运行配置生效；所有失败均写状态与错误，
/// 不返回硬错误（除数据库错误外）。返回更新后的运行配置。
pub async fn ensure_registered(
    db: &DatabaseConnection,
    platform: &dyn PlatformClient,
    client: &dyn VrouterClient,
    config: &VrouterConfig,
    deployment_id: &str,
) -> Result<deployment::Model, ModelinferError> {
    ensure_registered_with(db, platform, client, config, deployment_id, false).await
}

/// 强制完整注册（跳过心跳续约）：用于启动 / 手动注册，确保最新调度策略
/// （默认 `scheduler.mode=direct`）对已有注册生效。
pub async fn ensure_registered_force(
    db: &DatabaseConnection,
    platform: &dyn PlatformClient,
    client: &dyn VrouterClient,
    config: &VrouterConfig,
    deployment_id: &str,
) -> Result<deployment::Model, ModelinferError> {
    ensure_registered_with(db, platform, client, config, deployment_id, true).await
}

async fn ensure_registered_with(
    db: &DatabaseConnection,
    platform: &dyn PlatformClient,
    client: &dyn VrouterClient,
    config: &VrouterConfig,
    deployment_id: &str,
    force_register: bool,
) -> Result<deployment::Model, ModelinferError> {
    let d = crate::deployments::get_deployment(db, deployment_id).await?;
    if d.state != "running" || d.desired_state == "stopping" {
        return Ok(d);
    }
    // 认证失败抑制：上次为 401 时不再自动重试（手动注册可清除）
    if d.vrouter_state == state::ERROR && d.vrouter_last_error.starts_with(AUTH_ERROR_PREFIX) {
        return Ok(d);
    }
    let res = deployment_resolution(platform, client, config, &d).await;
    let status = &res.status;
    if !status.installed {
        return set_state(db, d, state::DISABLED, None, None, None, Some(status.message.clone())).await;
    }
    if !status.reachable || !status.credential_present {
        return set_state(db, d, state::ERROR, None, None, None, Some(status.message.clone())).await;
    }

    let ttl = config.normalized_ttl();
    let target = &res.target;

    // 已注册 → 心跳续约（强制注册时跳过，直接重新提交完整注册以应用最新策略）
    if !force_register && d.vrouter_state == state::REGISTERED && !d.vrouter_upstream_id.is_empty()
    {
        match client.heartbeat(target, &d.vrouter_upstream_id, ttl).await {
            Ok(HeartbeatOutcome::Ok { .. }) => {
                return set_state(
                    db,
                    d,
                    state::REGISTERED,
                    None,
                    None,
                    Some(time::now_iso()),
                    Some(String::new()),
                )
                .await;
            }
            Ok(HeartbeatOutcome::Unauthorized) => {
                return set_state(
                    db,
                    d,
                    state::ERROR,
                    None,
                    None,
                    None,
                    Some(format!("{AUTH_ERROR_PREFIX}：统一网关拒绝续约凭据")),
                )
                .await;
            }
            Ok(HeartbeatOutcome::NotFound) => {
                // 租约过期 / 网关重启 → 落到完整注册
            }
            Err(e) => {
                return set_state(db, d, state::ERROR, None, None, None, Some(e.msg)).await;
            }
        }
    }

    // 完整注册
    let (base_url, model, upstream_key) = match registration_endpoint(db, &d).await {
        Ok(v) => v,
        Err(e) => return set_state(db, d, state::ERROR, None, None, None, Some(e.msg)).await,
    };
    let req = build_registration(&d.id, &d.name, &base_url, &model, &upstream_key, ttl);
    match client.register(target, &req).await {
        Ok(outcome) => {
            set_state(
                db,
                d,
                state::REGISTERED,
                Some(outcome.upstream_id),
                Some(outcome.model_id),
                Some(time::now_iso()),
                Some(String::new()),
            )
            .await
        }
        Err(e) => set_state(db, d, state::ERROR, None, None, None, Some(e.msg)).await,
    }
}

/// 计算注册 `base_url`、服务模型名与上游 API Key（head 节点 + 端口 + 前缀 + 运行配置键）
async fn registration_endpoint(
    db: &DatabaseConnection,
    d: &deployment::Model,
) -> Result<(String, String, String), ModelinferError> {
    let node_ids: Vec<String> = serde_json::from_str(&d.node_ids_json).unwrap_or_default();
    let head = match node_ids.first() {
        Some(id) => crate::nodes::get_node(db, id).await?,
        None => return Err(ModelinferError::new(codes::PARAM_INVALID, "运行配置未关联节点")),
    };
    let port = crate::access::normalize_serve_port(Some(d.serve_port.max(0) as u64));
    let prefix = crate::access::normalize_url_prefix(&d.serve_url_prefix);
    let model = if d.serve_model_name.trim().is_empty() {
        crate::access::default_model_name(&d.weight_id, &d.card_id)
    } else {
        d.serve_model_name.clone()
    };
    Ok((
        format!("http://{}:{port}{prefix}", head.host),
        model,
        d.serve_api_key.clone(),
    ))
}

/// 尽力注销某运行配置的统一网关注册（停止清理时）。
///
/// 无论成功与否均将运行配置归位为 `unregistered`（租约会自然过期）。
pub async fn deregister(
    db: &DatabaseConnection,
    platform: &dyn PlatformClient,
    client: &dyn VrouterClient,
    config: &VrouterConfig,
    deployment_id: &str,
) -> Result<Option<String>, ModelinferError> {
    let d = crate::deployments::get_deployment(db, deployment_id).await?;
    let upstream_id = d.vrouter_upstream_id.clone();
    if upstream_id.is_empty() {
        clear_registered_at(db, d, state::UNREGISTERED, String::new()).await?;
        return Ok(None);
    }
    let mut warning = None;
    let res = deployment_resolution(platform, client, config, &d).await;
    let status = &res.status;
    if status.installed
        && status.reachable
        && status.credential_present
        && let Err(e) = client.deregister(&res.target, &upstream_id).await
    {
        warning = Some(e.msg);
    }
    clear_registered_at(db, d, state::UNREGISTERED, warning.clone().unwrap_or_default()).await?;
    Ok(warning)
}

/// 归位注销状态并**清除**最近注册时间 / 上游 ID / 模型 ID
async fn clear_registered_at(
    db: &DatabaseConnection,
    current: deployment::Model,
    new_state: &str,
    last_error: String,
) -> Result<(), ModelinferError> {
    let mut am: deployment::ActiveModel = current.into();
    am.vrouter_state = Set(new_state.to_string());
    am.vrouter_upstream_id = Set(String::new());
    am.vrouter_model = Set(String::new());
    am.vrouter_registered_at = Set(None);
    am.vrouter_last_error = Set(last_error);
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(db_err)?;
    Ok(())
}

/// 周期续约 / 注册调度（进程级，非 Temporal）
pub fn spawn_scheduler(
    db: DatabaseConnection,
    platform: Arc<dyn PlatformClient>,
    client: Arc<dyn VrouterClient>,
    config: VrouterConfig,
    interval_secs: u64,
) {
    let interval = interval_secs.max(15);
    tokio::spawn(async move {
        tracing::info!(
            target: "modelinfer_vrouter",
            interval,
            "统一网关注册续约调度器启动（进程级）"
        );
        loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            if let Err(e) = renew_active(&db, platform.as_ref(), client.as_ref(), &config).await {
                tracing::warn!(target: "modelinfer_vrouter", error = %e.msg, "统一网关续约调度失败");
            }
        }
    });
}

/// 遍历运行中的运行配置，注册 / 续约
pub async fn renew_active(
    db: &DatabaseConnection,
    platform: &dyn PlatformClient,
    client: &dyn VrouterClient,
    config: &VrouterConfig,
) -> Result<(), ModelinferError> {
    let rows = deployment::Entity::find()
        .all(db)
        .await
        .map_err(db_err)?;
    for d in rows {
        if d.state != "running" || d.desired_state == "stopping" {
            continue;
        }
        match ensure_registered(db, platform, client, config, &d.id).await {
            Ok(updated) => tracing::debug!(
                target: "modelinfer_vrouter",
                deployment = %d.id,
                state = %updated.vrouter_state,
                "统一网关注册状态刷新"
            ),
            Err(e) => tracing::warn!(
                target: "modelinfer_vrouter",
                deployment = %d.id,
                error = %e.msg,
                "统一网关注册 / 续约失败"
            ),
        }
    }
    Ok(())
}

/// 统计已注册的运行配置数（供状态查询）
pub async fn registered_count(db: &DatabaseConnection) -> Result<u64, ModelinferError> {
    let rows = deployment::Entity::find()
        .filter(deployment::Column::VrouterState.eq(state::REGISTERED))
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn service_and_model_ids() {
        assert_eq!(service_id_of("abc"), "modelinfer-abc");
        assert_eq!(upstream_id_of("modelinfer-abc"), "registered-modelinfer-abc");
        assert_eq!(model_id_of("Qwen3.8-27B"), "registered/Qwen3.8-27B");
    }

    #[test]
    fn registration_request_shape() {
        let r = build_registration("d1", "cfg", "http://10.0.0.1:8000/v1/", "m1", "svc-key", 90);
        assert_eq!(r.service_id, "modelinfer-d1");
        assert_eq!(r.visibility, "public");
        assert_eq!(r.base_url, "http://10.0.0.1:8000/v1");
        assert_eq!(r.api_key, "svc-key");
        assert_eq!(r.model, "m1");
        assert_eq!(r.protocol, "chat_completions");
        assert_eq!(r.ttl_seconds, 90);
        assert!(r.endpoints.contains(&"chat_completions".to_string()));
        // 默认直接放行：注册请求携带 scheduler.mode=direct
        assert_eq!(r.scheduler, Some(SchedulerPolicy::direct()));
        let body = serde_json::to_value(&r).unwrap();
        assert_eq!(body["scheduler"], json!({"mode": "direct"}));
        // 空名称回落 service_id
        let r2 = build_registration("d1", "  ", "u", "m", "", 30);
        assert_eq!(r2.name, "modelinfer-d1");
        assert!(r2.api_key.is_empty());
    }

    #[test]
    fn ttl_normalization() {
        assert_eq!(
            VrouterConfig { ttl_secs: 10, ..Default::default() }.normalized_ttl(),
            90
        );
        assert_eq!(
            VrouterConfig { ttl_secs: 3600, ..Default::default() }.normalized_ttl(),
            3600
        );
    }

    #[test]
    fn url_and_token_resolution() {
        let cfg = VrouterConfig::default();
        // 默认 docker 网关 IP，不再使用 127.0.0.1
        assert_eq!(resolve_url(&cfg, &json!({})), "http://172.17.0.1:22888");
        assert_eq!(
            resolve_url(&cfg, &json!({"V_ROUTER_HOST_PORT": 9999})),
            "http://172.17.0.1:9999"
        );
        let cfg2 = VrouterConfig {
            host_gateway_ip: "10.0.0.1".into(),
            ..Default::default()
        };
        assert_eq!(
            resolve_url(&cfg2, &json!({"V_ROUTER_HOST_PORT": 9999})),
            "http://10.0.0.1:9999"
        );
        // 无 token 路径 → 空凭据
        assert_eq!(resolve_token(&cfg), "");
        // 有 token 文件 → 读取并去首尾空白
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("access-token");
        std::fs::write(&path, "  app-token-abc\n").unwrap();
        let cfg3 = VrouterConfig {
            token_path: Some(path),
            ..Default::default()
        };
        assert_eq!(resolve_token(&cfg3), "app-token-abc");
    }

    #[test]
    fn app_detection() {
        use crate::platform::AppInfo;
        let vscheduler = AppInfo { app_id: VSCHEDULER_APP_ID.into(), state: "installed".into() };
        let vrouter = AppInfo { app_id: VROUTER_APP_ID.into(), state: "installed".into() };
        let upgrading = AppInfo { app_id: VROUTER_APP_ID.into(), state: "upgrading".into() };
        assert_eq!(
            gateway_app_id(std::slice::from_ref(&vscheduler)),
            Some(VSCHEDULER_APP_ID)
        );
        assert_eq!(
            gateway_app_id(std::slice::from_ref(&vrouter)),
            Some(VROUTER_APP_ID)
        );
        // 两者共存：优先 V-Scheduler
        assert_eq!(
            gateway_app_id(&[vrouter.clone(), vscheduler.clone()]),
            Some(VSCHEDULER_APP_ID)
        );
        assert!(has_vrouter_app(&[vscheduler]));
        assert!(has_vrouter_app(&[vrouter]));
        assert!(!has_vrouter_app(&[upgrading]));
        assert!(!has_vrouter_app(&[]));
    }

    #[test]
    fn resolve_url_reads_selected_gateway_config() {
        let cfg = VrouterConfig::default();
        assert_eq!(
            resolve_url(&cfg, &json!({"V_ROUTER_HOST_PORT": 22888})),
            "http://172.17.0.1:22888"
        );
        assert_eq!(
            resolve_url(&cfg, &json!({"V_ROUTER_HOST_PORT": "29999"})),
            "http://172.17.0.1:29999"
        );
    }

    #[tokio::test]
    async fn prefers_vscheduler_config_when_both_installed() {
        use crate::platform::{AppInfo, MockPlatformClient};
        let mut p = MockPlatformClient::new();
        p.apps = vec![
            AppInfo { app_id: VROUTER_APP_ID.into(), state: "installed".into() },
            AppInfo { app_id: VSCHEDULER_APP_ID.into(), state: "installed".into() },
        ];
        p.app_configs_by_id.insert(VROUTER_APP_ID.into(), json!({"V_ROUTER_HOST_PORT": 28888}));
        p.app_configs_by_id.insert(
            VSCHEDULER_APP_ID.into(),
            json!({"V_ROUTER_HOST_PORT": 22888}),
        );
        let client = MockVrouterClient::new().with_healthz(true);
        let cfg = VrouterConfig::default();
        let r = resolve_target_and_status(&p, &client, &cfg).await;
        assert_eq!(r.status, VrouterStatus {
            installed: true,
            reachable: true,
            credential_present: false,
            registered_count: 0,
            message: "应用上下文 access-token 缺失（上下文目录未挂载），无法注册".into(),
        });
        assert_eq!(r.target.base_url, "http://172.17.0.1:22888");
        assert!(client
            .recorded_calls()
            .iter()
            .any(|c| c == "healthz http://172.17.0.1:22888"));
    }

    #[tokio::test]
    async fn list_apps_failure_probes_vscheduler_first() {
        use crate::platform::MockPlatformClient;
        let mut p = MockPlatformClient::new();
        p.fail = true; // 平台列表 / 应用配置都不可用
        let client = MockVrouterClient::new().with_healthz(true);
        let cfg = VrouterConfig::default();
        let r = resolve_target_and_status(&p, &client, &cfg).await;
        assert!(r.status.installed && r.status.reachable);
        assert_eq!(r.target.base_url, "http://172.17.0.1:22888");
        assert!(client
            .recorded_calls()
            .iter()
            .any(|c| c == "healthz http://172.17.0.1:22888"));
    }

    #[tokio::test]
    async fn falls_back_to_legacy_vrouter_when_only_it_is_installed() {
        use crate::platform::{AppInfo, MockPlatformClient};
        let mut p = MockPlatformClient::new();
        p.apps = vec![AppInfo { app_id: VROUTER_APP_ID.into(), state: "installed".into() }];
        p.app_configs_by_id.insert(VROUTER_APP_ID.into(), json!({"V_ROUTER_HOST_PORT": 28888}));
        let client = MockVrouterClient::new().with_healthz(true);
        let cfg = VrouterConfig::default();
        let r = resolve_target_and_status(&p, &client, &cfg).await;
        assert!(r.status.installed && r.status.reachable);
        assert_eq!(r.target.base_url, "http://172.17.0.1:28888");
    }

    // ---- 编排 / 状态机（Mock 客户端 + 内存库）----

    fn platform_installed() -> crate::platform::MockPlatformClient {
        use crate::platform::{AppInfo, MockPlatformClient};
        let mut p = MockPlatformClient::new();
        p.apps = vec![AppInfo { app_id: VSCHEDULER_APP_ID.into(), state: "installed".into() }];
        p.app_configs_by_id.insert(
            VSCHEDULER_APP_ID.into(),
            json!({"V_ROUTER_HOST_PORT": 22888}),
        );
        p
    }

    /// 构造带应用上下文 token 的配置（返回 TempDir 以保持 token 文件存活）
    fn cfg_with_token() -> (tempfile::TempDir, VrouterConfig) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("access-token");
        std::fs::write(&path, "app-context-token").unwrap();
        (
            dir,
            VrouterConfig {
                token_path: Some(path),
                ..Default::default()
            },
        )
    }

    async fn seed_running(db: &DatabaseConnection) -> String {
        use sea_orm::{ActiveModelTrait, Set};
        use crate::deploy::activities::{MarkStateReq, apply_state};
        use crate::nodes::{self, NodeCreate};
        use crate::deployments::{self, DeploymentCreate};
        use crate::ssh::MockSshExec;

        let now = crate::time::now_iso();
        let manifest = json!({
            "id": "com.ictrek.vr-test", "version": "1.0.0",
            "model": "Qwen/Qwen3.8-27B",
            "lifecycle": {"start": "./s.sh"},
        });
        crate::entities::plaza_model::ActiveModel {
            id: Set("com.ictrek.vr-test@1.0.0".into()),
            model_id: Set("com.ictrek.vr-test".into()),
            model: Set("Qwen/Qwen3.8-27B".into()),
            name: Set("vr".into()),
            version: Set("1.0.0".into()),
            source: Set("plaza".into()),
            source_id: Set(String::new()),
            path: Set("/tmp/x".into()),
            package_path: Set("/tmp/x/model.tar.gz".into()),
            package_sha256: Set(String::new()),
            manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
            state: Set("available".into()),
            size_bytes: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();

        let node = nodes::create_node(
            db,
            &MockSshExec::new().with_rule("hostname", 0, "h1"),
            NodeCreate {
                name: "n1".into(),
                host: "10.10.11.154".into(),
                ssh_port: 22,
                ssh_user: "root".into(),
                auth_method: "password".into(),
                ssh_password: "x".into(),
                ssh_key: String::new(),
                sudo_password: String::new(),
                hostname: Some("h1".into()),
                socket_ifname: Some("eth0".into()),
                ib_hca: Some(String::new()),
                fscache: false,
                node_exporter_port: crate::nodes::DEFAULT_NODE_EXPORTER_PORT,
                nvidia_gpu_exporter_port: crate::nodes::DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
            },
        )
        .await
        .unwrap()
        .id;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/Qwen/Qwen3.8-27B/current")).unwrap();
        let d = deployments::create_deployment(
            db,
            tmp.path(),
            DeploymentCreate {
                name: "vr".into(),
                plaza_model_id: "com.ictrek.vr-test@1.0.0".into(),
                hub_source: "hf".into(),
                node_ids: vec![node],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        apply_state(
            db,
            &MarkStateReq {
                deployment_id: d.id.clone(),
                state: Some("running".into()),
                desired_state: Some("running".into()),
                health_status: None,
                last_error: None,
            },
        )
        .await
        .unwrap();
        // 保持 TempDir 存活（hub_dir 仅创建期使用）
        let _ = Box::leak(Box::new(tmp));
        d.id
    }

    #[tokio::test]
    async fn registers_when_running_public() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_running(&db).await;
        let client = MockVrouterClient::new().with_healthz(true);
        let p = platform_installed();
        let (_td, cfg) = cfg_with_token();
        let d = ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(d.vrouter_state, state::REGISTERED);
        assert_eq!(d.vrouter_upstream_id, format!("registered-modelinfer-{id}"));
        assert_eq!(d.vrouter_model, "registered/Qwen3.8-27B");
        assert!(d.vrouter_registered_at.is_some());
        let calls = client.recorded_calls();
        assert!(calls.iter().any(|c| c.contains("register") && c.contains("Qwen3.8-27B")));
        assert!(
            calls.iter().any(|c| c.starts_with("register") && c.ends_with("direct")),
            "注册请求应默认携带 scheduler.mode=direct：{calls:?}"
        );
    }

    #[tokio::test]
    async fn force_registration_reapplies_direct_policy() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_running(&db).await;
        let client = MockVrouterClient::new().with_healthz(true);
        let p = platform_installed();
        let (_td, cfg) = cfg_with_token();
        // 首次注册 + 常规（心跳）调度：不再提交完整注册
        ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        let after_first = client
            .recorded_calls()
            .iter()
            .filter(|c| c.starts_with("register"))
            .count();
        ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(
            client.recorded_calls().iter().filter(|c| c.starts_with("register")).count(),
            after_first,
            "已注册时常规调度应走心跳续约"
        );
        // 强制注册：再次提交完整注册，确保已有注册应用 direct 策略
        let d = ensure_registered_force(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(d.vrouter_state, state::REGISTERED);
        let calls = client.recorded_calls();
        assert!(
            calls.iter().filter(|c| c.starts_with("register")).count() > after_first,
            "强制注册应重新提交完整注册：{calls:?}"
        );
        assert!(
            calls
                .iter()
                .filter(|c| c.starts_with("register"))
                .all(|c| c.ends_with("direct")),
            "每次完整注册都应携带 scheduler.mode=direct：{calls:?}"
        );
    }

    #[tokio::test]
    async fn heartbeat_404_triggers_reregister() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_running(&db).await;
        let client = MockVrouterClient::new()
            .with_healthz(true)
            .with_heartbeat(vec![HeartbeatOutcome::NotFound]);
        let p = platform_installed();
        let (_td, cfg) = cfg_with_token();
        // 先注册
        ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(
            crate::deployments::get_deployment(&db, &id).await.unwrap().vrouter_state,
            state::REGISTERED
        );
        // 心跳 404 → 重新注册
        let d = ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(d.vrouter_state, state::REGISTERED);
        assert!(client.recorded_calls().iter().filter(|c| c.starts_with("register")).count() >= 2);
    }

    #[tokio::test]
    async fn unauthorized_marks_error_and_suppresses_retry() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_running(&db).await;
        let client = MockVrouterClient::new()
            .with_healthz(true)
            .with_heartbeat(vec![HeartbeatOutcome::Unauthorized]);
        let p = platform_installed();
        let (_td, cfg) = cfg_with_token();
        ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap(); // 注册成功
        let d = ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(d.vrouter_state, state::ERROR);
        assert!(d.vrouter_last_error.starts_with(AUTH_ERROR_PREFIX));
        // 再次调度不再尝试（抑制）
        let before = client.recorded_calls().len();
        ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(client.recorded_calls().len(), before, "认证失败后应抑制自动重试");
    }

    #[tokio::test]
    async fn not_installed_sets_disabled() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_running(&db).await;
        let client = MockVrouterClient::new().with_healthz(true);
        let p = crate::platform::MockPlatformClient::new(); // 无统一网关（V-Scheduler / V-Router）
        let cfg = VrouterConfig::default();
        let d = ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(d.vrouter_state, state::DISABLED);
        assert!(client.recorded_calls().is_empty(), "未安装不应发起注册");
    }

    #[tokio::test]
    async fn register_error_sets_error_without_panic() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_running(&db).await;
        let client = MockVrouterClient::new()
            .with_healthz(true)
            .with_register_err("boom");
        let p = platform_installed();
        let (_td, cfg) = cfg_with_token();
        let d = ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(d.vrouter_state, state::ERROR);
        assert!(d.vrouter_last_error.contains("boom"));
    }

    #[tokio::test]
    async fn deregister_clears_state() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_running(&db).await;
        let client = MockVrouterClient::new().with_healthz(true);
        let p = platform_installed();
        let (_td, cfg) = cfg_with_token();
        ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        let warning = deregister(&db, &p, &client, &cfg, &id).await.unwrap();
        assert!(warning.is_none());
        let d = crate::deployments::get_deployment(&db, &id).await.unwrap();
        assert_eq!(d.vrouter_state, state::UNREGISTERED);
        assert!(d.vrouter_upstream_id.is_empty());
        assert!(d.vrouter_model.is_empty());
        assert!(d.vrouter_registered_at.is_none(), "注销应清除最近注册时间");
        assert!(client.recorded_calls().iter().any(|c| c.starts_with("deregister")));
    }

    #[tokio::test]
    async fn no_credential_sets_error() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_running(&db).await;
        let client = MockVrouterClient::new().with_healthz(true);
        let p = platform_installed();
        // 未提供 token 路径（应用上下文未挂载）→ 视为无凭据
        let cfg = VrouterConfig::default();
        let d = ensure_registered(&db, &p, &client, &cfg, &id).await.unwrap();
        assert_eq!(d.vrouter_state, state::ERROR);
        assert!(d.vrouter_last_error.contains("access-token"));
    }
}