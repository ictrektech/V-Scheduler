//! keepalived 子项目动态编排 API（/api/infra/keepalived/*）
//!
//! 职责：
//! - 按网关集合在每个网关节点部署/移除 keepalived project（每节点一个实例）；
//! - 容器以 host network + NET_ADMIN 运行，镜像内 entrypoint 渲染 VRRP 配置；
//! - `vrrp_script` 调用本机 VCM 网关健康端点，drain / 端口未监听时降低优先级，
//!   让 VIP 漂移到其他网关节点；
//! - status 读取 keepalived 写入 project data 目录的 `state` 文件，暴露当前
//!   `MASTER/BACKUP/FAULT/STOP`，用于判断 VIP owner。
//!
//! 该模块只做编排，不直接修改宿主网络；VIP 的加/删由 keepalived 容器完成。

use std::path::PathBuf;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::codes;
use crate::error::{ResultBody, VcmError};
use crate::orchestration::{
    build_keepalived_env_vars, keepalived_compose_name, KeepalivedTopology,
    KEEPALIVED_COMPOSE_PREFIX as COMPOSE_PREFIX,
    DEFAULT_VIP_PREFIX, DEFAULT_VRRP_ID,
};
use crate::projects::{ProjectDeployBo, ProjectInstanceVo};
use crate::state::AppState;

const APP_ID: &str = "com.ictrek.vos-cluster-manager";
const PROJECT_NAME: &str = "keepalived";
const PROJECT_DATA_ROOT: &str = "/var/lib/vos-projects";
const STATE_FILE: &str = "state";

fn invalid(msg: impl Into<String>) -> VcmError {
    VcmError::new(codes::PARAM_INVALID, msg)
}

fn parse_ip(v: Option<&Value>, field: &str) -> Result<String, VcmError> {
    v.and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
        .ok_or_else(|| invalid(format!("{field} 必填且为合法 IP 地址")))
}

fn parse_ip_list(v: Option<&Value>, field: &str) -> Result<Vec<String>, VcmError> {
    let Some(arr) = v.and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in arr {
        let ip = item
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
            .ok_or_else(|| invalid(format!("{field} 含非法 IP")))?;
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    Ok(out)
}

fn parse_u8(v: Option<&Value>, field: &str, default: u8) -> Result<u8, VcmError> {
    match v {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| invalid(format!("{field} 必须是 0-255 的整数"))),
    }
}

fn parse_services(v: Option<&Value>) -> Result<Vec<String>, VcmError> {
    let Some(arr) = v.and_then(|v| v.as_array()) else {
        return Err(invalid("services 必填且为数组（smb/nfs）"));
    };
    let mut out = Vec::new();
    for item in arr {
        let service = item
            .as_str()
            .map(|s| s.trim().to_ascii_lowercase())
            .ok_or_else(|| invalid("services 含非法项（只允许 smb / nfs）"))?;
        if service != "smb" && service != "nfs" {
            return Err(invalid(format!("services 非法项: {service}")));
        }
        if !out.contains(&service) {
            out.push(service);
        }
    }
    if out.is_empty() {
        return Err(invalid("services 不能为空"));
    }
    Ok(out)
}

fn parse_interface(v: Option<&Value>) -> Result<String, VcmError> {
    let Some(s) = v.and_then(|v| v.as_str()).map(str::trim) else {
        return Ok(String::new());
    };
    if s.is_empty() {
        return Ok(String::new());
    }
    if s.len() > 15 || !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(invalid("interface 非法（Linux 网卡名，≤15 字符）"));
    }
    Ok(s.to_string())
}

fn build_deploy_bo(body: &Value) -> Result<ProjectDeployBo, VcmError> {
    let node_ip = parse_ip(body.get("node_ip"), "node_ip")?;
    let vip = match body.get("vip").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => parse_ip(body.get("vip"), "vip")?,
        _ => String::new(),
    };
    let vip_prefix = parse_u8(body.get("vip_prefix"), "vip_prefix", DEFAULT_VIP_PREFIX)?;
    if !(1..=32).contains(&vip_prefix) {
        return Err(invalid("vip_prefix 必须在 1-32"));
    }
    let vrrp_id = parse_u8(body.get("vrrp_id"), "vrrp_id", DEFAULT_VRRP_ID)?;
    if vrrp_id == 0 {
        return Err(invalid("vrrp_id 必须在 1-255"));
    }
    let priority = parse_u8(body.get("priority"), "priority", 100)?;
    if !(1..=254).contains(&priority) {
        return Err(invalid("priority 必须在 1-254"));
    }
    let peers = parse_ip_list(body.get("peers"), "peers")?;
    if peers.iter().any(|p| p == &node_ip) {
        return Err(invalid("peers 不能包含本节点"));
    }
    let services = match body.get("services") {
        Some(v) => parse_services(Some(v))?,
        None => Vec::new(),
    };
    if !vip.is_empty() && services.is_empty() {
        return Err(invalid("启用文件网关 VIP 时 services 不能为空（smb / nfs）"));
    }
    let interface = parse_interface(body.get("interface"))?;

    let rgw_vip = match body.get("rgw_vip").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => parse_ip(body.get("rgw_vip"), "rgw_vip").ok(),
        _ => None,
    };
    let rgw_vip_prefix = parse_u8(body.get("rgw_vip_prefix"), "rgw_vip_prefix", DEFAULT_VIP_PREFIX)?;
    let rgw_priority = parse_u8(body.get("rgw_priority"), "rgw_priority", 100)?;
    let rgw_vrrp_id = parse_u8(body.get("rgw_vrrp_id"), "rgw_vrrp_id", 52)?;
    let mut rgw_peers = parse_ip_list(body.get("rgw_peers"), "rgw_peers")?;
    if rgw_peers.is_empty() {
        rgw_peers = peers.clone();
    }
    if let Some(ref rv) = rgw_vip
        && rv == &vip
    {
        return Err(invalid("rgw_vip 不能与文件网关 vip 相同"));
    }
    let rgw_health_url = body
        .get("rgw_health_url")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("http://127.0.0.1:7480/")
        .to_string();

    let topo = KeepalivedTopology {
        node_ip: node_ip.clone(),
        vip,
        vip_prefix,
        peers,
        priority,
        vrrp_id,
        interface,
        services,
        rgw_vip,
        rgw_vip_prefix,
        rgw_peers,
        rgw_priority,
        rgw_vrrp_id,
        rgw_health_url,
    };
    let compose_project_name = keepalived_compose_name(&node_ip)
        .ok_or_else(|| invalid(format!("node_ip 无法生成实例名: {node_ip}")))?;
    Ok(ProjectDeployBo {
        app_id: APP_ID.to_string(),
        project_name: PROJECT_NAME.to_string(),
        compose_project_name,
        env_vars: build_keepalived_env_vars(&topo),
        health_timeout_secs: Some(30),
    })
}

fn compose_name_from(body: &Value) -> Result<String, VcmError> {
    if let Some(name) = body
        .get("compose_project_name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(name.to_string());
    }
    let node_ip = parse_ip(body.get("node_ip"), "node_ip")?;
    keepalived_compose_name(&node_ip)
        .ok_or_else(|| invalid(format!("node_ip 无法生成实例名: {node_ip}")))
}

fn needs_refresh(inst: &ProjectInstanceVo) -> bool {
    // failed 实例也会占用 compose_project_name，必须先 remove 才能重新 deploy。
    inst.state != "removed"
}

async fn ensure_instance_exists(
    state: &AppState,
    compose_project_name: &str,
) -> Result<(), VcmError> {
    let instances = state.projects_client.instances().await?;
    if !instances
        .iter()
        .any(|i| i.compose_project_name == compose_project_name)
    {
        return Err(VcmError::new(
            codes::NOT_FOUND,
            format!("keepalived 实例不存在: {compose_project_name}"),
        ));
    }
    Ok(())
}

fn state_path(compose_project_name: &str) -> PathBuf {
    PathBuf::from(PROJECT_DATA_ROOT)
        .join(compose_project_name)
        .join(STATE_FILE)
}

async fn read_vrrp_state(compose_project_name: &str) -> Option<String> {
    read_vrrp_state_file(&state_path(compose_project_name)).await
}

async fn read_rgw_vrrp_state(compose_project_name: &str) -> Option<String> {
    let mut path = state_path(compose_project_name);
    path.set_file_name("state-VOS_RGW_GATEWAY");
    read_vrrp_state_file(&path).await
}

async fn read_vrrp_state_file(path: &std::path::Path) -> Option<String> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let state = text.trim().to_ascii_uppercase();
    if state.is_empty() {
        None
    } else {
        Some(state)
    }
}

/// POST /api/infra/keepalived/deploy —— 同步部署（同实例活跃则先 remove 再 deploy）
pub async fn keepalived_deploy(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let bo = build_deploy_bo(&body)?;

    let instances = state.projects_client.instances().await?;
    let existing = instances
        .iter()
        .find(|i| i.compose_project_name == bo.compose_project_name && needs_refresh(i));
    let refreshed = existing.is_some();
    if let Some(inst) = existing {
        tracing::info!(
            compose_project_name = %bo.compose_project_name,
            state = %inst.state,
            "keepalived 实例已存在 → upsert 刷新（remove + deploy）"
        );
        state
            .projects_client
            .remove_sync(&bo.compose_project_name)
            .await?;
    }

    let instance = state.projects_client.deploy_sync(&bo).await?;
    Ok(Json(ResultBody::ok(
        json!({"instance": instance, "refreshed": refreshed}),
        version,
    )))
}

/// POST /api/infra/keepalived/remove —— 同步移除（keepalived 收到 SIGTERM 后移除 VIP）
pub async fn keepalived_remove(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let compose_project_name = compose_name_from(&body)?;
    ensure_instance_exists(&state, &compose_project_name).await?;
    state
        .projects_client
        .remove_sync(&compose_project_name)
        .await?;
    let _ = tokio::fs::remove_file(state_path(&compose_project_name)).await;
    Ok(Json(ResultBody::ok(
        json!({"removed": true, "compose_project_name": compose_project_name}),
        version,
    )))
}

/// POST /api/infra/keepalived/vrrp-id —— 由 VCM 管理面数据库分配/复用 VRRP ID
///
/// body: `{ "vip": "...", "preferred": 51 }`；同一 VIP 幂等，preferred 被占用时选空闲 ID。
pub async fn keepalived_allocate_vrrp_id(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let vip = parse_ip(body.get("vip"), "vip")?;
    let preferred = parse_u8(body.get("preferred"), "preferred", 0)?;
    let alloc = crate::db::vrrp::allocate(
        state.etcd_cluster_client.as_ref(),
        state.db.as_ref(),
        &vip,
        (preferred != 0).then_some(preferred),
    )
    .await?;
    Ok(Json(ResultBody::ok(
        json!({
            "vip": alloc.vip,
            "vrrp_id": alloc.vrrp_id,
            "owner": alloc.owner,
            "created_at": alloc.updated_at,
            "updated_at": alloc.updated_at,
        }),
        version,
    )))
}

/// POST /api/infra/keepalived/vrrp-id/release —— 释放 VIP 的 VRRP ID
pub async fn keepalived_release_vrrp_id(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let vip = parse_ip(body.get("vip"), "vip")?;
    let existing = state
        .etcd_cluster_client
        .list_vrrp_allocations()
        .await?
        .into_iter()
        .find(|a| a.vip == vip);
    let Some(existing) = existing else {
        return Ok(Json(ResultBody::ok(
            json!({"vip": vip, "released": false}),
            version,
        )));
    };
    let vrrp_id = match body.get("vrrp_id").and_then(|v| v.as_u64()) {
        Some(v) => u8::try_from(v).map_err(|_| invalid("vrrp_id 必须在 1-255"))?,
        None => existing.vrrp_id,
    };
    let owner = body
        .get("owner")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(existing.owner.as_str());
    let released = crate::db::vrrp::release(
        state.etcd_cluster_client.as_ref(),
        state.db.as_ref(),
        &vip,
        vrrp_id,
        owner,
    )
    .await?;
    Ok(Json(ResultBody::ok(
        json!({"vip": vip, "vrrp_id": vrrp_id, "owner": owner, "released": released}),
        version,
    )))
}

/// GET /api/infra/keepalived/instances —— 本应用 keepalived 实例清单
pub async fn keepalived_instances(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let instances = state.projects_client.instances().await?;
    let keepalived: Vec<_> = instances
        .into_iter()
        .filter(|i| i.compose_project_name.starts_with(COMPOSE_PREFIX))
        .collect();
    Ok(Json(ResultBody::ok(
        json!({"instances": keepalived}),
        version,
    )))
}

/// GET /api/infra/keepalived/status —— 实例 + VRRP 状态（MASTER/BACKUP/FAULT/STOP）
pub async fn keepalived_status(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let instances = state.projects_client.instances().await?;
    let mut rows: Vec<Value> = Vec::new();
    let mut vip_owner: Option<String> = None;
    let mut rgw_vip_owner: Option<String> = None;
    for inst in instances
        .iter()
        .filter(|i| i.compose_project_name.starts_with(COMPOSE_PREFIX))
    {
        let vrrp_state = read_vrrp_state(&inst.compose_project_name).await;
        let vip_present = vrrp_state.as_deref() == Some("MASTER");
        if vip_present && vip_owner.is_none() {
            vip_owner = Some(inst.compose_project_name.clone());
        }
        let rgw_vrrp_state = read_rgw_vrrp_state(&inst.compose_project_name).await;
        let rgw_vip_present = rgw_vrrp_state.as_deref() == Some("MASTER");
        if rgw_vip_present && rgw_vip_owner.is_none() {
            rgw_vip_owner = Some(inst.compose_project_name.clone());
        }
        rows.push(json!({
            "compose_project_name": inst.compose_project_name,
            "state": inst.state,
            "vrrp_state": vrrp_state,
            "vip_present": vip_present,
            "rgw_vrrp_state": rgw_vrrp_state,
            "rgw_vip_present": rgw_vip_present,
            "deployed_at": inst.deployed_at,
        }));
    }
    Ok(Json(ResultBody::ok(
        json!({
            "instances": rows,
            "vip_owner": vip_owner,
            "rgw_vip_owner": rgw_vip_owner,
        }),
        version,
    )))
}