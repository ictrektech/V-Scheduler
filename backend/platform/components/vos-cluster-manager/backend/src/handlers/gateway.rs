//! 存储网关 VIP 管理 API（SMB 445 / NFS 2049）
//!
//! 职责（cephfs-share-gateway spec）：
//! - 维护「哪些节点是网关」（管理面表 `vcm_gateway_backend`，不写 etcd schema）
//! - 排空 / 恢复单个网关（drain / undrain）
//! - 为 keepalived 提供**健康门控端点**：只有「已登记 + 未排空 + 本机服务在监听」的
//!   节点返回 200，其余 503 → `vrrp_script` 降低优先级并触发 VIP 漂移
//!
//! VIP 由 keepalived 直接持有并指向 samba/nfsd；SMB active/backup 需要**凭据一致**
//! （平台用户是每节点本地库），该限制在 `docs/gateway-vip.md` 显式声明。

use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::db;
use crate::entity::vcm_gateway_backend::{service as svc, state as gstate};
use crate::error::{codes, ResultBody, VcmError};
use crate::orchestration::{NFS_FRONT_PORT, RGW_FRONT_PORT, SMB_FRONT_PORT};
use crate::state::AppState;

/// 网关服务端口（与本机服务监听端口一致；keepalived 健康检查使用）
pub fn service_port(service: &str) -> Option<u16> {
    match service {
        svc::SMB => Some(SMB_FRONT_PORT),
        svc::NFS => Some(NFS_FRONT_PORT),
        svc::RGW => Some(RGW_FRONT_PORT),
        _ => None,
    }
}

/// 本机 IP（用于 keepalived 健康检查自识别）
///
/// 优先 `VOS_CLUSTER_MANAGER_NODE_IP`（部署显式指定，最可靠）；否则用 UDP connect
/// 让内核选路由源地址（不发送任何报文）。
pub fn local_ip() -> Option<std::net::IpAddr> {
    if let Ok(v) = std::env::var("VOS_CLUSTER_MANAGER_NODE_IP")
        && let Ok(ip) = v.trim().parse()
    {
        return Some(ip);
    }
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

/// TCP 探活本机服务端口（keepalived 网关健康门控）
async fn local_service_listening(port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

fn db_of(state: &AppState) -> Result<&sea_orm::DatabaseConnection, VcmError> {
    state.db.as_ref().ok_or_else(|| {
        VcmError::new(
            codes::GATEWAY_FAILED,
            "管理面状态库不可用（未注入 VOS_POSTGRES_*）：网关集合无法维护。请确认平台已注入管理面数据库",
        )
    })
}

fn parse_service(v: Option<&Value>) -> Result<String, VcmError> {
    let s = v
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "service 必填（smb | nfs | rgw）"))?;
    if service_port(&s).is_none() {
        return Err(VcmError::new(
            codes::PARAM_INVALID,
            format!("service 非法: {s}（允许 smb | nfs | rgw）"),
        ));
    }
    Ok(s)
}

fn parse_ips(v: Option<&Value>) -> Result<Vec<String>, VcmError> {
    let arr = v
        .and_then(|v| v.as_array())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "node_ips 必填且为 IP 数组"))?;
    let mut out: Vec<String> = Vec::new();
    for item in arr {
        let ip = item
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
            .ok_or_else(|| {
                VcmError::new(codes::PARAM_INVALID, "node_ips 含非法 IP（形如 10.100.10.21）")
            })?;
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    Ok(out)
}

/// GET /api/gateway/backends —— 网关集合（含排空状态）
pub async fn gateway_backends_get(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let Some(db) = state.db.as_ref() else {
        return Ok(Json(ResultBody::ok(
            json!({"smb": [], "nfs": [], "db_ok": false}),
            version,
        )));
    };
    let rows = db::list_gateway_backends(db).await?;
    let of = |s: &str| -> Vec<Value> {
        rows.iter()
            .filter(|r| r.service == s)
            .map(|r| {
                json!({
                    "node_ip": r.node_ip,
                    "state": r.state,
                    "reason": r.reason,
                    "updated_at": r.updated_at.to_rfc3339(),
                })
            })
            .collect()
    };
    Ok(Json(ResultBody::ok(
        json!({
            "smb": of(svc::SMB),
            "nfs": of(svc::NFS),
            "rgw": of(svc::RGW),
            "smb_front_port": SMB_FRONT_PORT,
            "nfs_front_port": NFS_FRONT_PORT,
            "rgw_front_port": RGW_FRONT_PORT,
            "db_ok": true,
        }),
        version,
    )))
}

/// POST /api/gateway/backends —— 全量设置某服务的网关集合
///
/// body: `{ "service": "smb", "node_ips": ["10.100.10.21", ...] }`
/// 语义：替换集合（保留既有节点的排空状态；新节点默认 active）。
pub async fn gateway_backends_set(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let service = parse_service(body.get("service"))?;
    let node_ips = parse_ips(body.get("node_ips"))?;
    let db = db_of(&state)?;
    db::replace_gateway_backends(db, &service, &node_ips).await?;

    if let Some(conn) = state.db.as_ref() {
        let _ = db::insert_operation_log(
            conn,
            "gateway_backends_set",
            &service,
            Some(json!({"node_ips": node_ips})),
        )
        .await;
    }

    let unknown_nodes = match state.etcd_cluster_client.get_all_node_records().await {
        Ok(records) => node_ips
            .iter()
            .filter(|ip| !records.iter().any(|r| &r.metadata.node_ip == *ip))
            .cloned()
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };

    let rows = db::list_gateway_backends(db).await?;
    Ok(Json(ResultBody::ok(
        json!({
            "service": service,
            "node_ips": node_ips,
            "unknown_nodes": unknown_nodes,
            "backends": rows.iter().filter(|r| r.service == service).map(|r| json!({
                "node_ip": r.node_ip, "state": r.state, "reason": r.reason
            })).collect::<Vec<_>>(),
        }),
        version,
    )))
}

/// POST /api/gateway/drain —— 排空 / 恢复单个网关
///
/// body: `{ "service": "smb", "node_ip": "...", "drain": true, "reason": "...", "force": false }`
/// 恢复（`drain=false`）默认要求本机服务端口在监听，避免「挂载未恢复就回切」；
/// 目标不是本机或明确 `force=true` 时跳过该探测（审计记录 force）。
pub async fn gateway_drain(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let service = parse_service(body.get("service"))?;
    let node_ip = body
        .get("node_ip")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "node_ip 必填且为合法 IP"))?;
    let drain = body.get("drain").and_then(|v| v.as_bool()).unwrap_or(true);
    let force = body.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let db = db_of(&state)?;

    if !drain && !force {
        let is_local = local_ip().map(|ip| ip.to_string()) == Some(node_ip.clone());
        if is_local {
            let port = service_port(&service).unwrap_or_default();
            if !local_service_listening(port).await {
                return Err(VcmError::new(
                    codes::GATEWAY_FAILED,
                    format!(
                        "拒绝恢复 {node_ip} 的 {service} 网关：本机 {port} 端口未监听（挂载/共享可能尚未就绪）。确认可回切后使用 force=true"
                    ),
                ));
            }
        }
    }

    let found = db::set_gateway_drain(db, &service, &node_ip, drain, reason.as_deref()).await?;
    if !found {
        return Err(VcmError::new(
            codes::NOT_FOUND,
            format!("{node_ip} 不是已登记的 {service} 网关（请先设置后端集合）"),
        ));
    }

    let _ = db::insert_operation_log(
        db,
        if drain { "gateway_drain" } else { "gateway_undrain" },
        &format!("{service}/{node_ip}"),
        Some(json!({"reason": reason, "force": force})),
    )
    .await;

    Ok(Json(ResultBody::ok(
        json!({
            "service": service,
            "node_ip": node_ip,
            "state": if drain { gstate::DRAINED } else { gstate::ACTIVE },
            "forced": force,
        }),
        version,
    )))
}

/// GET /api/gateway/status —— 网关健康视图（前端 / 运维）
pub async fn gateway_status(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let local = local_ip().map(|ip| ip.to_string());
    let Some(db) = state.db.as_ref() else {
        return Ok(Json(ResultBody::ok(
            json!({"smb": [], "nfs": [], "local_node_ip": local, "db_ok": false}),
            version,
        )));
    };
    let rows = db::list_gateway_backends(db).await?;
    let mut smb = Vec::new();
    let mut nfs = Vec::new();
    let mut rgw = Vec::new();
    for row in &rows {
        let port = service_port(&row.service).unwrap_or_default();
        let reachable = if Some(row.node_ip.clone()) == local {
            Some(local_service_listening(port).await)
        } else {
            None
        };
        let item = json!({
            "node_ip": row.node_ip,
            "state": row.state,
            "reason": row.reason,
            "service_reachable": reachable,
            "updated_at": row.updated_at.to_rfc3339(),
        });
        match row.service.as_str() {
            svc::SMB => smb.push(item),
            svc::RGW => rgw.push(item),
            _ => nfs.push(item),
        }
    }
    Ok(Json(ResultBody::ok(
        json!({
            "smb": smb,
            "nfs": nfs,
            "rgw": rgw,
            "local_node_ip": local,
            "smb_front_port": SMB_FRONT_PORT,
            "nfs_front_port": NFS_FRONT_PORT,
            "rgw_front_port": RGW_FRONT_PORT,
            "db_ok": true,
        }),
        version,
    )))
}

/// keepalived 健康检查查询参数
#[derive(Debug, Deserialize)]
pub struct GatewayHealthQuery {
    pub service: Option<String>,
    pub node_ip: Option<String>,
}

/// GET /api/gateway/health —— keepalived 健康门控（**看 HTTP 状态码，不看 envelope**）
///
/// 200 = 该节点可作为该服务的网关（已登记 + 未排空 + 本机端口在监听）
/// 503 = 不可路由（未登记 / 已排空 / 服务未监听 / 管理面不可用）
pub async fn gateway_health(
    State(state): State<AppState>,
    Query(q): Query<GatewayHealthQuery>,
) -> Response {
    let service = q.service.as_deref().unwrap_or("").trim().to_ascii_lowercase();
    let Some(port) = service_port(&service) else {
        return health_response(StatusCode::SERVICE_UNAVAILABLE, "unknown service", None, None);
    };
    let Some(db) = state.db.as_ref() else {
        return health_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "management database unavailable",
            None,
            None,
        );
    };
    let node_ip = q
        .node_ip
        .map(|s| s.trim().to_string())
        .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
        .or_else(|| local_ip().map(|ip| ip.to_string()));
    let Some(node_ip) = node_ip else {
        return health_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "cannot determine local node ip",
            None,
            None,
        );
    };

    let rows = match db::list_gateway_backends(db).await {
        Ok(v) => v,
        Err(_) => {
            return health_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "gateway backend query failed",
                Some(&node_ip),
                Some(&service),
            );
        }
    };
    let Some(row) = rows
        .iter()
        .find(|r| r.service == service && r.node_ip == node_ip)
    else {
        return health_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "not a registered gateway",
            Some(&node_ip),
            Some(&service),
        );
    };
    if row.state == gstate::DRAINED {
        return health_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway drained",
            Some(&node_ip),
            Some(&service),
        );
    }
    if !local_service_listening(port).await {
        return health_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "service port not listening",
            Some(&node_ip),
            Some(&service),
        );
    }
    health_response(StatusCode::OK, "healthy", Some(&node_ip), Some(&service))
}

fn health_response(
    status: StatusCode,
    reason: &str,
    node_ip: Option<&str>,
    service: Option<&str>,
) -> Response {
    let body = json!({
        "status": if status == StatusCode::OK { "ok" } else { "unavailable" },
        "reason": reason,
        "node_ip": node_ip,
        "service": service,
    });
    (status, Json(body)).into_response()
}

/// 网关后端 IP 解析（管理面 active 集合）
pub(crate) async fn active_backends(state: &AppState, service: &str) -> Vec<String> {
    let Some(db) = state.db.as_ref() else {
        return Vec::new();
    };
    db::active_gateway_ips(db, service).await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_port_mapping() {
        assert_eq!(service_port("smb"), Some(445));
        assert_eq!(service_port("nfs"), Some(2049));
        assert_eq!(service_port("ftp"), None);
    }

    #[test]
    fn parse_service_rejects_unknown() {
        assert!(parse_service(Some(&json!("smb"))).is_ok());
        assert!(parse_service(Some(&json!("SMB"))).is_ok());
        assert_eq!(
            parse_service(Some(&json!("ftp"))).unwrap_err().code,
            codes::PARAM_INVALID
        );
        assert_eq!(parse_service(None).unwrap_err().code, codes::PARAM_INVALID);
    }

    #[test]
    fn parse_ips_dedups_and_validates() {
        assert_eq!(
            parse_ips(Some(&json!(["10.0.0.1", " 10.0.0.1 ", "10.0.0.2"]))).unwrap(),
            vec!["10.0.0.1".to_string(), "10.0.0.2".to_string()]
        );
        assert_eq!(
            parse_ips(Some(&json!(["nope"]))).unwrap_err().code,
            codes::PARAM_INVALID
        );
        assert_eq!(parse_ips(None).unwrap_err().code, codes::PARAM_INVALID);
    }
}