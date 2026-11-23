use axum::{Json, extract::State};
use chrono::Utc;
use serde::Serialize;

use crate::{error::ResultBody, state::AppState};

#[derive(Serialize)]
pub struct HealthData {
    pub status: String,
    pub version: String,
    /// registry 地址是否已配置（`--registry-url` 非空）
    pub registry_configured: bool,
    /// registry `/v2/` 是否可达（不可达不阻断 code=0）
    pub registry_reachable: bool,
    pub timestamp: String,
}

/// `GET /api/health` —— 容器健康检查与前端探测
pub async fn health(State(state): State<AppState>) -> Json<ResultBody<HealthData>> {
    let registry_reachable = state.registry.ping().await;
    let data = HealthData {
        status: "ok".to_string(),
        version: state.version.to_string(),
        registry_configured: !state.registry_url.trim().is_empty(),
        registry_reachable,
        timestamp: Utc::now().to_rfc3339(),
    };
    Json(ResultBody::ok(data, state.version))
}