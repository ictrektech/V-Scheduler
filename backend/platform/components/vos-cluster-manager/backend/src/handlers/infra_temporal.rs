//! 集群 temporal 子项目动态编排 API（/api/infra/temporal/*，二期 S2）
//!
//! 镜像 `/api/infra/haproxy/*` 模式，关键差异（design D-TE-03/04）：
//! - **仅 manager 节点部署**：目标 manager 有本地 etcd，`build_deploy_bo` **自读**
//!   集群 pg 凭据（`/vos/cluster/pg_credentials`）→ 密码不出本机（`at` 转发体不含凭据）；
//! - **前置（20507）**：etcd 可达 + pg 凭据存在 + **本机 haproxy 15432 可达**
//!   （temporal DB 经 haproxy→leader，S1 数据面缺失时拒绝）；
//! - **deploy/remove 后自动刷新全集群 haproxy**（D-TE-04）：temporal_backends =
//!   跨 manager 扫描已部署实例反解 IP，逐节点 `at` 转发 haproxy deploy（best-effort，
//!   失败仅 warn——下次 refresh_all/deploy 可救）；
//! - 所有响应不回显凭据（密码仅存在于实例 env_vars）。

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::codes;
use crate::error::{ResultBody, VcmError};
use crate::orchestration::temporal::{
    DEFAULT_NAMESPACES, FRONTEND_PORT, UI_PORT, TemporalTopology, build_temporal_env_vars,
    temporal_compose_name,
};
use crate::orchestration::validate_compose_project_name;
use crate::projects::ProjectDeployBo;
use crate::state::AppState;

use super::infra_haproxy;

use crate::projects::APP_ID;
/// temporal 实例 compose 名前缀
const TEMPORAL_NAME_PREFIX: &str = "cluster-temporal-";

/// 跨节点扫描已部署 temporal 的 manager IP（refresh haproxy 与 nodes 聚合共用）。
///
/// 依赖调用方 etcd 可达（manager 本地视角）；逐 manager remote list_instances 匹配
/// `cluster-temporal-<ip-slug>`。任何节点查询失败按未部署处理（保守）。
pub(crate) async fn deployed_temporal_backend_ips(state: &AppState) -> Vec<String> {
    let managers = match infra_haproxy::manager_backend_ips(state).await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for ip in managers {
        let want = temporal_compose_name(&ip);
        if let (Some(w), Ok(names)) = (want, state.remote_client.list_instances(&ip, "temporal").await)
            && names.iter().any(|n| n == &w)
        {
            out.push(ip);
        }
    }
    out
}

/// 刷新全集群 haproxy（D-TE-04）：对 etcd 全节点（managers+workers）经 `at` 转发
/// deploy（pg_backends + temporal_backends **显式注入**——worker 无 etcd 依赖注入；
/// manager 亦走同一路径统一逻辑，loopback 通道 S1 已验证）。返回逐节点结果摘要。
///
/// 失败仅 warn（不阻断 temporal deploy/remove）；运维兜底 `POST /api/infra/haproxy/refresh_all`。
pub(crate) async fn refresh_cluster_haproxy(state: &AppState) -> Vec<Value> {
    let records = match state.etcd_cluster_client.get_all_node_records().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(err = %e.msg, "refresh_cluster_haproxy：etcd 节点记录不可读，跳过刷新");
            return Vec::new();
        }
    };
    let Ok(pg_backends) = infra_haproxy::manager_backend_ips(state).await else {
        tracing::warn!("refresh_cluster_haproxy：manager 推导失败，跳过刷新");
        return Vec::new();
    };
    let temporal_backends = deployed_temporal_backend_ips(state).await;
    let mut results = Vec::new();
    for rec in records {
        let ip = rec.metadata.node_ip.clone();
        let body = json!({
            "node_ip": ip,
            "pg_backends": pg_backends.join(","),
            "temporal_backends": temporal_backends.join(","),
        });
        match state.remote_client.deploy_haproxy_sync(&ip, body).await {
            Ok(_) => results.push(json!({"node_ip": ip, "ok": true})),
            Err(e) => {
                tracing::warn!(node_ip = %ip, err = %e.msg, "refresh_cluster_haproxy：节点刷新失败（不阻断）");
                results.push(json!({"node_ip": ip, "ok": false, "err": e.msg}));
            }
        }
    }
    results
}

/// 提取 compose_project_name 参数
fn extract_name(body: &Value) -> Result<String, VcmError> {
    body.get("compose_project_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "compose_project_name 必填"))
}

/// 提取可选 `at`（目标节点 IP，转发执行——同 haproxy 通道）
fn extract_at(body: &Value) -> Option<String> {
    body.get("at")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
}

fn is_active(inst: &crate::projects::ProjectInstanceVo) -> bool {
    inst.state == "deployed" || inst.state == "deploying"
}

/// 构造部署 Bo（目标 manager 本地执行）：node_ip（20501）→ etcd ping + pg 凭据 +
/// 本机 15432 可达（20507 前置链）→ env_vars + compose_project_name。
///
/// `pub(crate)`：create workflow activity 复用（防漂移，同 infra_pg/haproxy）。
pub(crate) async fn build_deploy_bo(
    state: &AppState,
    body: &Value,
) -> Result<ProjectDeployBo, VcmError> {
    let node_ip = body
        .get("node_ip")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "node_ip 必填且为合法 IP 地址"))?;

    // ---- 前置链（D-TE-03）----
    if !state.etcd_cluster_client.ping().await {
        return Err(VcmError::new(
            codes::STATE_NOT_SUPPORTED,
            "etcd 未就绪：temporal 仅可部署在 manager 节点（需本地 etcd 读集群凭据）",
        ));
    }
    let cred = state
        .etcd_cluster_client
        .get_cluster_pg_credentials()
        .await?
        .ok_or_else(|| {
            VcmError::new(
                codes::STATE_NOT_SUPPORTED,
                "集群 pg 凭据缺失（/vos/cluster/pg_credentials）：请先部署 postgres-ha",
            )
        })?;

    // 本机 haproxy pg 前端可达（temporal DB 经 127.0.0.1:15432 → leader）
    let pg_front_port: u16 = body
        .get("pg_front_port")
        .and_then(|v| v.as_u64())
        .and_then(|v| u16::try_from(v).ok())
        .unwrap_or(crate::orchestration::PG_FRONT_PORT);
    if tokio::net::TcpStream::connect(("127.0.0.1", pg_front_port))
        .await
        .is_err()
    {
        return Err(VcmError::new(
            codes::STATE_NOT_SUPPORTED,
            format!(
                "本机 haproxy pg 入口（127.0.0.1:{pg_front_port}）不可达：请先部署本节点 haproxy 子项目"
            ),
        ));
    }

    let namespaces = body
        .get("namespaces")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_NAMESPACES.to_string());

    let topo = TemporalTopology {
        db_host: "127.0.0.1".to_string(),
        db_port: pg_front_port,
        db_user: cred.superuser_username.clone(),
        db_password: cred.superuser_password.clone(),
        namespaces,
    };
    let env_vars = build_temporal_env_vars(&topo);

    let compose_project_name = temporal_compose_name(&node_ip).ok_or_else(|| {
        VcmError::new(codes::PARAM_INVALID, format!("node_ip 无法生成实例名: {node_ip}"))
    })?;
    if !validate_compose_project_name(&compose_project_name) {
        return Err(VcmError::new(
            codes::PARAM_INVALID,
            format!("compose_project_name 非法: {compose_project_name}"),
        ));
    }

    Ok(ProjectDeployBo {
        app_id: APP_ID.to_string(),
        project_name: "temporal".to_string(),
        compose_project_name,
        env_vars,
        // 自举窗口：等 pg ≤120s + schema + server 起 + healthcheck start_period
        health_timeout_secs: Some(600),
    })
}

/// GET /api/infra/temporal/status —— 本机视角聚合（实例 + 端口探测，业务态）
pub async fn temporal_status(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let mut data = temporal_status_data(&state).await;
    if let Some(obj) = data.as_object_mut() {
        obj.insert("db_ok".to_string(), json!(state.db_ok()));
    }
    Ok(Json(ResultBody::ok(data, version)))
}

/// 本机 temporal 状态聚合（status 端点与 cluster 视图共用；探测失败仅降级）
pub(crate) async fn temporal_status_data(state: &AppState) -> Value {
    let instances = match state.projects_client.instances().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(err = %e.msg, "temporal status：projects 实例列表失败（降级空清单）");
            Vec::new()
        }
    };
    let temporal: Vec<_> = instances
        .iter()
        .filter(|i| i.compose_project_name.starts_with(TEMPORAL_NAME_PREFIX))
        .collect();
    let mut instance_details = Vec::new();
    for inst in &temporal {
        if inst.state == "deployed" {
            match state.projects_client.status(&inst.compose_project_name).await {
                Ok(Some(d)) => instance_details.push(d),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        err = %e.msg,
                        name = %inst.compose_project_name,
                        "temporal status：实例详情失败（降级跳过）"
                    );
                }
            }
        }
    }
    let local_frontend_reachable =
        tokio::net::TcpStream::connect(("127.0.0.1", FRONTEND_PORT)).await.is_ok();
    let front_via_haproxy_reachable =
        tokio::net::TcpStream::connect(("127.0.0.1", crate::orchestration::TEMPORAL_FRONT_PORT))
            .await
            .is_ok();
    let db_front_reachable =
        tokio::net::TcpStream::connect(("127.0.0.1", crate::orchestration::PG_FRONT_PORT))
            .await
            .is_ok();
    let deployed_count = instance_details.len();
    let temporal_ok = deployed_count > 0 && local_frontend_reachable;

    json!({
        "temporal_ok": temporal_ok,
        "frontend_port": FRONTEND_PORT,
        "haproxy_frontend_port": crate::orchestration::TEMPORAL_FRONT_PORT,
        "ui_port": UI_PORT,
        "deployed_count": deployed_count,
        "instances": instance_details,
        "local_frontend_reachable": local_frontend_reachable,
        "front_via_haproxy_reachable": front_via_haproxy_reachable,
        "db_front_reachable": db_front_reachable,
    })
}

/// GET /api/infra/temporal/nodes —— 跨节点 temporal 部署态（逐 manager 查实例）
pub async fn temporal_nodes(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let records = match state.etcd_cluster_client.get_all_node_records().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(err = %e.msg, "temporal nodes：etcd 记录失败（返回空清单）");
            Vec::new()
        }
    };
    let mut nodes = Vec::new();
    for r in records {
        let ip = r.metadata.node_ip.clone();
        let slug = temporal_compose_name(&ip);
        let present = match (&slug, state.remote_client.list_instances(&ip, "temporal").await) {
            (Some(name), Ok(names)) => names.iter().any(|n| n == name),
            _ => false,
        };
        nodes.push(json!({
            "node_ip": ip,
            "compose_project_name": slug,
            "temporal_present": present,
            // temporal 仅允许 manager 部署（worker 恒 false 并标注不可部署）
            "deployable": crate::etcd::dto::role::normalize_legacy(&r.metadata.role)
                == crate::etcd::dto::role::MANAGER,
        }));
    }
    Ok(Json(ResultBody::ok(
        json!({"nodes": nodes, "frontend_port": FRONTEND_PORT}),
        version,
    )))
}

/// POST /api/infra/temporal/deploy —— 同步部署（upsert；成功后刷新全集群 haproxy）
pub async fn temporal_deploy(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;

    // at 转发：目标 manager 自读 etcd 凭据（密码不经编排方转发体，D-TE-03）
    if let Some(at) = extract_at(&body) {
        let fwd = state
            .remote_client
            .deploy_temporal_sync(&at, json!({"node_ip": at}))
            .await?;
        return Ok(Json(ResultBody::ok(fwd, version)));
    }

    let bo = build_deploy_bo(&state, &body).await?;
    let instances = state.projects_client.instances().await?;
    let existing_active = instances
        .iter()
        .any(|i| i.compose_project_name == bo.compose_project_name && is_active(i));
    let refreshed = existing_active;
    if existing_active {
        tracing::info!(name = %bo.compose_project_name, "temporal 实例已活跃 → upsert 刷新");
        state.projects_client.remove_sync(&bo.compose_project_name).await?;
    }
    let instance = state.projects_client.deploy_sync(&bo).await?;

    // 刷新全集群 haproxy temporal 前端（best-effort，D-TE-04）
    let refresh = refresh_cluster_haproxy(&state).await;

    Ok(Json(ResultBody::ok(
        json!({"instance": instance, "refreshed": refreshed, "haproxy_refresh": refresh}),
        version,
    )))
}

/// POST /api/infra/temporal/remove —— 同步移除（成功后刷新全集群 haproxy 收缩 backends）
pub async fn temporal_remove(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let compose_project_name = extract_name(&body)?;
    if let Some(at) = extract_at(&body) {
        let fwd = state
            .remote_client
            .remove_temporal_sync(&at, &compose_project_name)
            .await?;
        return Ok(Json(ResultBody::ok(fwd, version)));
    }
    let instances = state.projects_client.instances().await?;
    if !instances
        .iter()
        .any(|i| i.compose_project_name == compose_project_name)
    {
        return Err(VcmError::new(
            codes::NOT_FOUND,
            format!("实例 {compose_project_name} 不存在"),
        ));
    }
    state
        .projects_client
        .remove_sync(&compose_project_name)
        .await?;
    let refresh = refresh_cluster_haproxy(&state).await;
    Ok(Json(ResultBody::ok(
        json!({"removed": true, "compose_project_name": compose_project_name, "haproxy_refresh": refresh}),
        version,
    )))
}

/// GET /api/infra/temporal/instances —— 本节点 temporal 子项目实例列表
pub async fn temporal_instances(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let instances = state.projects_client.instances().await?;
    let temporal: Vec<_> = instances
        .into_iter()
        .filter(|i| i.compose_project_name.starts_with(TEMPORAL_NAME_PREFIX))
        .collect();
    Ok(Json(ResultBody::ok(json!({"instances": temporal}), version)))
}

/// POST /api/infra/temporal/deploy_async —— 异步部署（deploy 通道同 haproxy；刷新延后到 refresh_all）
pub async fn temporal_deploy_async(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    if let Some(at) = extract_at(&body) {
        let fwd = state
            .remote_client
            .deploy_temporal_sync(&at, json!({"node_ip": at}))
            .await?;
        return Ok(Json(ResultBody::ok(fwd, version)));
    }
    let bo = build_deploy_bo(&state, &body).await?;
    let instances = state.projects_client.instances().await?;
    let existing_active = instances
        .iter()
        .any(|i| i.compose_project_name == bo.compose_project_name && is_active(i));
    if existing_active {
        state.projects_client.remove_sync(&bo.compose_project_name).await?;
    }
    let task_id = state.projects_client.deploy_async(&bo).await?;
    // 异步通道：实例行 reserve 即时存在（deploying），刷新纳入 backends
    let refresh = refresh_cluster_haproxy(&state).await;
    Ok(Json(ResultBody::ok(
        json!({"task_id": task_id, "compose_project_name": bo.compose_project_name, "haproxy_refresh": refresh}),
        version,
    )))
}

/// POST /api/infra/temporal/remove_async —— 异步移除
pub async fn temporal_remove_async(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let compose_project_name = extract_name(&body)?;
    if let Some(at) = extract_at(&body) {
        let fwd = state
            .remote_client
            .remove_temporal_sync(&at, &compose_project_name)
            .await?;
        return Ok(Json(ResultBody::ok(fwd, version)));
    }
    let instances = state.projects_client.instances().await?;
    if !instances
        .iter()
        .any(|i| i.compose_project_name == compose_project_name)
    {
        return Err(VcmError::new(
            codes::NOT_FOUND,
            format!("实例 {compose_project_name} 不存在"),
        ));
    }
    let task_id = state
        .projects_client
        .remove_async(&compose_project_name)
        .await?;
    Ok(Json(ResultBody::ok(
        json!({"task_id": task_id, "compose_project_name": compose_project_name}),
        version,
    )))
}
