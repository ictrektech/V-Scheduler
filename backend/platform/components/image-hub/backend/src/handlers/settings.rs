//! 管理面设置 handlers（上游 registry 认证 + 代理）
//!
//! 密码**只写不读**：响应仅返回 `has_password`，绝不回显 password。

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::error::{ImageHubError, ResultBody};
use crate::settings::{
    ProxyConfig, RegistryView, UpstreamCredential, masked_views, remove_registry, upsert_registry,
    validate_proxy_url,
};
use crate::state::AppState;

/// 设置查询响应
#[derive(Serialize)]
pub struct SettingsData {
    pub registries: Vec<RegistryView>,
    pub proxy: ProxyConfig,
}

/// 新增/更新上游凭据请求
#[derive(Deserialize)]
pub struct UpsertRegistryRequest {
    pub host: String,
    pub username: String,
    /// 为空 / 缺省时保留原密码
    #[serde(default)]
    pub password: Option<String>,
}

/// 删除上游凭据请求
#[derive(Deserialize)]
pub struct DeleteRegistryRequest {
    pub host: String,
}

/// 代理更新请求（字段缺省表示保留原值）
#[derive(Deserialize)]
pub struct ProxyUpdateRequest {
    pub enabled: bool,
    #[serde(default)]
    pub http_proxy: Option<String>,
    #[serde(default)]
    pub https_proxy: Option<String>,
    #[serde(default)]
    pub no_proxy: Option<String>,
}

/// 凭据列表响应
#[derive(Serialize)]
pub struct RegistryListData {
    pub registries: Vec<RegistryView>,
}

/// `GET /api/settings`
pub async fn get_settings(State(state): State<AppState>) -> Result<Json<ResultBody<SettingsData>>, ImageHubError> {
    let settings = state.settings.load()?;
    Ok(Json(ResultBody::ok(
        SettingsData {
            registries: masked_views(&settings),
            proxy: settings.proxy,
        },
        state.version,
    )))
}

/// `PUT /api/settings/registries` —— 新增/更新上游 registry 凭据
pub async fn upsert_registry_credential(
    State(state): State<AppState>,
    Json(req): Json<UpsertRegistryRequest>,
) -> Result<Json<ResultBody<RegistryListData>>, ImageHubError> {
    let mut settings = state.settings.load()?;
    upsert_registry(
        &mut settings,
        UpstreamCredential {
            host: req.host,
            username: req.username,
            password: req.password.unwrap_or_default(),
        },
    )?;
    state.settings.save(&settings)?;
    Ok(Json(ResultBody::ok(
        RegistryListData {
            registries: masked_views(&settings),
        },
        state.version,
    )))
}

/// `DELETE /api/settings/registries` —— 按 host 删除上游凭据
pub async fn delete_registry_credential(
    State(state): State<AppState>,
    Json(req): Json<DeleteRegistryRequest>,
) -> Result<Json<ResultBody<RegistryListData>>, ImageHubError> {
    let mut settings = state.settings.load()?;
    remove_registry(&mut settings, &req.host);
    state.settings.save(&settings)?;
    Ok(Json(ResultBody::ok(
        RegistryListData {
            registries: masked_views(&settings),
        },
        state.version,
    )))
}

/// `PUT /api/settings/proxy` —— 更新代理配置
pub async fn update_proxy(
    State(state): State<AppState>,
    Json(req): Json<ProxyUpdateRequest>,
) -> Result<Json<ResultBody<ProxyConfig>>, ImageHubError> {
    let mut settings = state.settings.load()?;
    let proxy = &mut settings.proxy;

    if let Some(v) = req.http_proxy.as_deref() {
        validate_proxy_url(v)?;
        proxy.http_proxy = v.trim().to_string();
    }
    if let Some(v) = req.https_proxy.as_deref() {
        validate_proxy_url(v)?;
        proxy.https_proxy = v.trim().to_string();
    }
    if let Some(v) = req.no_proxy.as_deref() {
        proxy.no_proxy = v.trim().to_string();
    }
    proxy.enabled = req.enabled;

    let proxy = proxy.clone();
    state.settings.save(&settings)?;
    Ok(Json(ResultBody::ok(proxy, state.version)))
}