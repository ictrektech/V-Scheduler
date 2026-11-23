//! 健康检查

use axum::{Json, extract::State};
use serde::Serialize;
use std::sync::Arc;

use crate::error::ResultBody;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct HealthVo {
    pub status: &'static str,
    pub version: &'static str,
    pub platform_token_present: bool,
    pub db_ready: bool,
    pub timestamp: String,
}

/// `GET <url-base>/api/health`
pub async fn health(State(state): State<Arc<AppState>>) -> Json<ResultBody<HealthVo>> {
    // 轻量探测 DB 可用性
    let db_ready = state.db.ping().await.is_ok();
    let vo = HealthVo {
        status: "ok",
        version: state.version,
        platform_token_present: state.platform_token_present(),
        db_ready,
        timestamp: crate::time::now_iso(),
    };
    Json(ResultBody::ok(vo, state.version))
}
