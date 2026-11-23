use axum::{Json, extract::State};
use serde::Serialize;

use crate::error::ResultBody;
use crate::state::AppState;

/// 网卡地址列表响应
#[derive(Serialize)]
pub struct NetworkAddresses {
    pub ip_addr: Vec<String>,
}

/// `GET /api/network/addresses` —— 平台设备网卡地址列表（失败降级为空数组，code 仍 0）
///
/// 前端据此渲染 registry 地址下拉；列表为空时前端兜底 `window.location.hostname`。
pub async fn addresses(State(state): State<AppState>) -> Json<ResultBody<NetworkAddresses>> {
    let ip_addr = match state.platform.device_info().await {
        Ok(info) => info.ip_addr,
        Err(e) => {
            tracing::warn!(err = %e.msg, "获取平台网卡地址失败，降级为空列表");
            Vec::new()
        }
    };
    Json(ResultBody::ok(NetworkAddresses { ip_addr }, state.version))
}