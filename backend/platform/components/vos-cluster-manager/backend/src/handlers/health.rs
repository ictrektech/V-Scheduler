use axum::Json;
use chrono::Utc;
use serde::Serialize;

use crate::{
    error::ResultBody,
    state::AppState,
};

#[derive(Serialize)]
pub struct HealthData {
    pub status: String,
    pub version: String,
    /// etcd 端点是否已配置
    pub etcd_configured: bool,
    /// etcd 实探测结果（Phase 1：连通性探测；不可达返回 false，不阻断 code=0）
    pub etcd_ok: bool,
    /// 管理面状态库可用性（平台 postgres；未注入 VOS_POSTGRES_* 或连接失败为 false）
    pub db_ok: bool,
    pub timestamp: String,
}

/// GET /api/health —— 容器健康检查与前端探测
pub async fn health(state: axum::extract::State<AppState>) -> Json<ResultBody<HealthData>> {
    let etcd_ok = state.etcd_cluster_client.ping().await;
    let data = HealthData {
        status: "ok".to_string(),
        version: state.version.to_string(),
        etcd_configured: !state.etcd_endpoints.is_empty(),
        etcd_ok,
        db_ok: state.db_ok(),
        timestamp: Utc::now().to_rfc3339(),
    };
    Json(ResultBody::ok(data, state.version))
}