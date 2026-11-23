//! 标准接入配置与统一网关状态 HTTP handlers

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::access::{self, PlatformAccess};
use crate::deployments;
use crate::error::{ModelinferError, ResultBody};
use crate::observability::store as obs_store;
use crate::state::AppState;
use crate::vrouter;

/// 统一网关注册信息（接入配置面板）
#[derive(Debug, Clone, Serialize)]
pub struct VrouterInfoVo {
    pub state: String,
    pub upstream_id: String,
    pub model_id: String,
    pub registered_at: Option<String>,
    pub last_error: String,
}

/// 本地反代信息（TCP 端口映射到 head vLLM）
#[derive(Debug, Clone, Serialize)]
pub struct LocalProxyVo {
    pub enabled: bool,
    /// 期望端口（0 = 自动分配）
    pub port: u16,
    /// 实际监听端口（0 = 未运行）
    pub active_port: u16,
    /// 转发目标 `head_host:serve_port`
    pub target: String,
    pub last_error: String,
}

/// 运行配置的接入配置
#[derive(Debug, Clone, Serialize)]
pub struct DeploymentAccessVo {
    pub deployment_id: String,
    /// 是否处于可访问状态（运行中 / 健康）
    pub available: bool,
    pub deployment_state: String,
    pub host: String,
    pub port: u16,
    pub url_prefix: String,
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub openai_base_url: String,
    /// 浏览器访问地址（`http://<host>:<port>`，无 URL 前缀；host 为空时为空串）
    pub web_url: String,
    /// 各字段来源（`platform` / `script`）
    pub sources: Map<String, Value>,
    pub vrouter: VrouterInfoVo,
    pub local_proxy: LocalProxyVo,
}

/// `GET /api/deployments/{id}/access`
pub async fn get_access(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<DeploymentAccessVo>>, ModelinferError> {
    let vo = resolve_access_vo(&state, &id).await?;
    Ok(Json(ResultBody::ok(vo, state.version)))
}

/// 解析运行配置的接入配置（平台推断 + 脚本 `global.serve` 合并）
pub async fn resolve_access_vo(
    state: &AppState,
    id: &str,
) -> Result<DeploymentAccessVo, ModelinferError> {
    let d = deployments::get_deployment(&state.db, id).await?;
    let node_ids: Vec<String> = serde_json::from_str(&d.node_ids_json).unwrap_or_default();
    // head 节点可能已被删除（历史悬挂引用）：降级为空 host，不让接入配置整体失败
    // （见 openspec/changes/modelinfer-dangling-node-reference/）
    let host = match node_ids.first() {
        Some(nid) => crate::nodes::find_node(&state.db, nid)
            .await?
            .map(|n| n.host)
            .unwrap_or_default(),
        None => String::new(),
    };
    let model = if d.serve_model_name.trim().is_empty() {
        access::default_model_name(&d.weight_id, &d.card_id)
    } else {
        d.serve_model_name.clone()
    };
    let serve_port = access::normalize_serve_port(Some(d.serve_port.max(0) as u64));
    let head_host = host.clone();
    let defaults = PlatformAccess {
        host,
        port: serve_port,
        url_prefix: access::normalize_url_prefix(&d.serve_url_prefix),
        model,
        api_key: d.serve_api_key.clone(),
    };
    let script_serve = obs_store::latest_serve_global(&state.db, id).await?;
    let resolved = access::resolve_access(&defaults, script_serve.as_ref());
    // 本地反代转发目标 = head 节点 host + 有效服务端口（脚本回传优先，见 D-MI-CA-03）
    let proxy_target = format!("{head_host}:{}", resolved.port);
    let available = matches!(d.state.as_str(), "running" | "degraded");
    Ok(DeploymentAccessVo {
        deployment_id: d.id.clone(),
        available,
        deployment_state: d.state.clone(),
        host: resolved.host.clone(),
        port: resolved.port,
        url_prefix: resolved.url_prefix.clone(),
        model: resolved.model.clone(),
        api_key: resolved.api_key.clone(),
        base_url: resolved.base_url.clone(),
        openai_base_url: resolved.openai_base_url(),
        web_url: resolved.web_url(),
        sources: resolved.sources.clone(),
        vrouter: VrouterInfoVo {
            state: d.vrouter_state.clone(),
            upstream_id: d.vrouter_upstream_id.clone(),
            model_id: d.vrouter_model.clone(),
            registered_at: d.vrouter_registered_at.clone(),
            last_error: d.vrouter_last_error.clone(),
        },
        local_proxy: LocalProxyVo {
            enabled: d.local_proxy_enabled,
            port: crate::localproxy::normalize_proxy_port(d.local_proxy_port).max(0) as u16,
            active_port: d.local_proxy_active_port.max(0) as u16,
            target: proxy_target,
            last_error: d.local_proxy_last_error.clone(),
        },
    })
}

/// 统一网关检测状态（不暴露网关地址）
#[derive(Debug, Clone, Serialize)]
pub struct VrouterStatusVo {
    pub installed: bool,
    pub reachable: bool,
    pub credential_present: bool,
    pub registered_count: u64,
    pub message: String,
}

/// `GET /api/vrouter/status`
pub async fn vrouter_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ResultBody<VrouterStatusVo>>, ModelinferError> {
    let mut status = vrouter::resolve_status(
        state.platform.as_ref(),
        state.vrouter_client.as_ref(),
        &state.vrouter_config,
    )
    .await;
    status.registered_count = vrouter::registered_count(&state.db).await?;
    Ok(Json(ResultBody::ok(
        VrouterStatusVo {
            installed: status.installed,
            reachable: status.reachable,
            credential_present: status.credential_present,
            registered_count: status.registered_count,
            message: status.message,
        },
        state.version,
    )))
}

/// `POST /api/deployments/{id}/vrouter/register`
///
/// 立即尽力注册（best-effort）。若上次为认证失败，手动注册会先清除抑制标记再尝试。
pub async fn register_vrouter(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<DeploymentAccessVo>>, ModelinferError> {
    let d = deployments::get_deployment(&state.db, &id).await?;
    // 手动注册：清除认证失败抑制标记，允许重试
    if d.vrouter_state == vrouter::state::ERROR
        && d.vrouter_last_error.starts_with(vrouter::AUTH_ERROR_PREFIX)
    {
        let mut am: crate::entities::deployment::ActiveModel = d.into();
        am.vrouter_state = sea_orm::Set(vrouter::state::UNREGISTERED.to_string());
        am.vrouter_last_error = sea_orm::Set(String::new());
        am.updated_at = sea_orm::Set(crate::time::now_iso());
        use sea_orm::ActiveModelTrait;
        am.update(&state.db)
            .await
            .map_err(|e| ModelinferError::new(crate::error::codes::DB_FAILED, format!("更新注册状态失败: {e}")))?;
    }
    vrouter::ensure_registered_force(
        &state.db,
        state.platform.as_ref(),
        state.vrouter_client.as_ref(),
        &state.vrouter_config,
        &id,
    )
    .await?;
    let vo = resolve_access_vo(&state, &id).await?;
    Ok(Json(ResultBody::ok(vo, state.version)))
}

/// `PUT /api/deployments/{id}/local-proxy`
///
/// 在线开启 / 关闭本地反代并调整端口（运行中也可操作）；持久化后立即生效。
#[derive(Debug, Clone, Deserialize)]
pub struct LocalProxyUpdate {
    pub enabled: bool,
    /// 期望端口；缺省 / 0 = 自动分配
    #[serde(default)]
    pub port: Option<i64>,
}

pub async fn update_local_proxy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<LocalProxyUpdate>,
) -> Result<Json<ResultBody<DeploymentAccessVo>>, ModelinferError> {
    deployments::update_local_proxy(&state.db, &id, body.enabled, body.port).await?;
    if body.enabled {
        crate::localproxy::ensure(&state.db, &state.local_proxy, &id).await?;
    } else {
        crate::localproxy::stop(&state.db, &state.local_proxy, &id).await?;
    }
    let vo = resolve_access_vo(&state, &id).await?;
    Ok(Json(ResultBody::ok(vo, state.version)))
}