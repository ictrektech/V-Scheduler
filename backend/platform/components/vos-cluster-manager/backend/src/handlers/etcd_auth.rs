//! etcd 认证/TLS 管理 API（`/api/infra/etcd/auth/*`）
//!
//! 覆盖 tasks 1.2 / 1.4 / 1.5 / 1.10：
//! - `GET  status`：认证开关 / 角色 / 用户 / 本机凭据 / PKI 健全性（无任何口令）
//! - `POST pki`：签发或续签集群内部 CA + 服务端证书 + 客户端证书
//! - `POST bootstrap`：幂等创建角色/用户 → 读回校验 → **最后**启用认证
//! - `POST rotate` / `POST revoke`：凭据轮换 / 吊销（可随之分发 / 清理）
//! - `POST distribute`：把本机凭据推送到 VCM 台账中的其他节点（0600 落盘）
//! - `POST credentials`：节点侧接收端点（凭据分发落点）
//!
//! 安全：所有响应只回路径 / 身份 / 角色，不回口令或私钥；接收端点只接受非 root 身份。

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::{codes, ResultBody, VcmError};
use crate::etcd::pki;
use crate::etcd::provisioning::{
    self, BootstrapOptions, IdentityOutcome, probe_status,
};
use crate::etcd::security::{
    DEFAULT_VCM_IDENTITY, EtcdAuthMode, IdentitySpec, agent_identity, default_identities,
    validate_identity,
};
use crate::state::AppState;

use super::gateway::local_ip;

const ROOT_IDENTITY: &str = "root";

fn connector_of(
    state: &AppState,
) -> Result<&std::sync::Arc<dyn crate::etcd::EtcdAuthConnector>, VcmError> {
    state.etcd_auth.connector.as_ref().ok_or_else(|| {
        VcmError::new(
            codes::ETCD_AUTH_FAILED,
            "etcd 认证连接器未装配（启动期 --etcd-auth-mode/凭据校验失败）。请检查启动日志与 /api/health",
        )
    })
}

fn mode_from_body_or_state(state: &AppState, body: &Value) -> Result<EtcdAuthMode, VcmError> {
    match body.get("auth_mode").and_then(|v| v.as_str()) {
        Some(s) => EtcdAuthMode::parse(s).ok_or_else(|| {
            VcmError::new(
                codes::PARAM_INVALID,
                format!("auth_mode 非法: {s}（disabled / auto / user / mtls / mtls_user）"),
            )
        }),
        None => Ok(state.etcd_auth.mode),
    }
}

fn node_ips_from_body(body: &Value) -> Result<Option<Vec<String>>, VcmError> {
    let Some(v) = body.get("node_ips") else {
        return Ok(None);
    };
    let arr = v
        .as_array()
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "node_ips 必须为 IP 数组"))?;
    let mut out: Vec<String> = Vec::new();
    for item in arr {
        let ip = item
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
            .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "node_ips 含非法 IP"))?;
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    Ok(Some(out))
}

/// 节点清单：请求体 > etcd 台账 > 本机 IP
async fn resolve_node_ips(state: &AppState, body: &Value) -> Result<Vec<String>, VcmError> {
    if let Some(ips) = node_ips_from_body(body)? {
        return Ok(ips);
    }
    if let Ok(records) = state.etcd_cluster_client.get_all_node_records().await
        && !records.is_empty()
    {
        let mut ips: Vec<String> = records.iter().map(|r| r.metadata.node_ip.clone()).collect();
        ips.sort();
        ips.dedup();
        return Ok(ips);
    }
    Ok(local_ip().map(|ip| vec![ip.to_string()]).unwrap_or_default())
}

fn identity_specs_from_body(
    body: &Value,
    node_ips: &[String],
) -> Result<Vec<IdentitySpec>, VcmError> {
    let defaults = default_identities(node_ips);
    let Some(v) = body.get("identities") else {
        return Ok(defaults);
    };
    let arr = v
        .as_array()
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "identities 必须为字符串数组"))?;
    let mut out = Vec::new();
    for item in arr {
        let name = item
            .as_str()
            .map(|s| s.trim().to_string())
            .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "identities 含非字符串项"))?;
        validate_identity(&name).map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
        let spec = defaults
            .iter()
            .find(|s| s.identity == name)
            .cloned()
            .or_else(|| {
                // 允许显式指定 `agent-<ip-slug>`（其节点未在 node_ips 中时）
                name.strip_prefix("agent-").map(|_| {
                    IdentitySpec::new(
                        name.clone(),
                        vec![crate::etcd::security::ROLE_VOS_CEPH_AGENT.to_string()],
                        vec![
                            crate::etcd::security::PREFIX_VOS_CEPH.to_string(),
                            crate::etcd::security::PREFIX_VOS_CEPH_SPACES.to_string(),
                        ],
                    )
                })
            })
            .ok_or_else(|| {
                VcmError::new(
                    codes::PARAM_INVALID,
                    format!(
                        "未知身份 {name}（可用：vcm / vcm-readonly / vos-ceph / agent-<ip-slug>）"
                    ),
                )
            })?;
        if !out.iter().any(|s: &IdentitySpec| s.identity == name) {
            out.push(spec);
        }
    }
    if out.is_empty() {
        return Err(VcmError::new(codes::PARAM_INVALID, "identities 不能为空"));
    }
    Ok(out)
}

fn outcome_json(o: &IdentityOutcome) -> Value {
    json!({
        "identity": o.identity,
        "username": o.username,
        "roles": o.roles,
        "key_prefixes": o.key_prefixes,
        "password_rotated": o.password_rotated,
        "credential_written": o.credential_written,
        "generation": o.generation,
    })
}

/// GET /api/infra/etcd/auth/status
pub async fn etcd_auth_status(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let layout = &state.etcd_auth.layout;
    let credentials = state.etcd_auth.store.list()?;
    let pki_json = json!({
        "present": pki::pki_present(layout),
        "ca_cert": layout.ca_cert().display().to_string(),
        "server_cert": layout.server_cert().display().to_string(),
        "root_password_present": pki::read_root_password(layout).ok().flatten().is_some(),
    });

    let etcd_json = match state.etcd_auth.connector.as_ref() {
        None => json!({
            "auth_enabled": null,
            "roles": [],
            "users": [],
            "error": "连接器未装配",
        }),
        Some(conn) => {
            let st = probe_status(
                conn.as_ref(),
                &state.etcd_auth.store,
                &state.etcd_auth.endpoints,
                state.etcd_auth.mode,
            )
            .await;
            json!({
                "auth_enabled": st.auth_enabled,
                "roles": st.roles.iter().map(|r| json!({
                    "name": r.name, "present": r.present, "grants": r.grants
                })).collect::<Vec<_>>(),
                "users": st.users,
                "error": st.auth_probe_error,
            })
        }
    };

    Ok(Json(ResultBody::ok(
        json!({
            "client": {
                "mode": state.etcd_auth.mode.as_str(),
                "identity": state.etcd_auth.identity,
                "credential_present": state.etcd_auth.store.exists(&state.etcd_auth.identity),
                "credential_path": layout.identity_file(&state.etcd_auth.identity).display().to_string(),
                "credential_dir": layout.root().display().to_string(),
                "endpoints": state.etcd_auth.endpoints,
            },
            "credentials": credentials,
            "pki": pki_json,
            "etcd": etcd_json,
        }),
        version,
    )))
}

/// POST /api/infra/etcd/auth/pki
///
/// body: `{ "node_ips": [...], "identities": ["vcm", ...], "force": false }`
pub async fn etcd_auth_pki(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let layout = state.etcd_auth.layout.clone();
    let node_ips = resolve_node_ips(&state, &body).await?;
    let identities: Vec<String> = match body.get("identities") {
        None => vec![state.etcd_auth.identity.clone()],
        Some(v) => v
            .as_array()
            .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "identities 必须为数组"))?
            .iter()
            .map(|i| {
                i.as_str()
                    .map(|s| s.trim().to_string())
                    .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "identities 含非字符串项"))
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    let force = body.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let report = pki::ensure_pki(&layout, &node_ips, &identities, force)?;
    Ok(Json(ResultBody::ok(
        json!({
            "ca_created": report.ca_created,
            "server_created": report.server_created,
            "ca_cert": report.ca_cert,
            "server_cert": report.server_cert,
            "server_key": report.server_key,
            "server_sans": report.server_sans,
            "client_identities": identities,
        }),
        version,
    )))
}

/// POST /api/infra/etcd/auth/bootstrap
///
/// body: `{ "node_ips"?, "identities"?, "auth_mode"?, "enable_auth"?: false, "rotate"?: false, "root_password"?, "distribute"?: false }`
///
/// **默认不启用认证**（`enable_auth` 必须显式 true）：避免在兼容窗口误触发全局切换。
pub async fn etcd_auth_bootstrap(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let connector = connector_of(&state)?.clone();
    let mode = mode_from_body_or_state(&state, &body)?;
    let node_ips = resolve_node_ips(&state, &body).await?;
    let identities = identity_specs_from_body(&body, &node_ips)?;
    let enable_auth = body
        .get("enable_auth")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let rotate = body.get("rotate").and_then(|v| v.as_bool()).unwrap_or(false);
    let distribute = body
        .get("distribute")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let generate_tls = body
        .get("generate_tls")
        .and_then(|v| v.as_bool())
        .unwrap_or(mode.requires_tls());

    let mut pki_json = Value::Null;
    if generate_tls {
        let ids: Vec<String> = identities.iter().map(|i| i.identity.clone()).collect();
        let report = pki::ensure_pki(&state.etcd_auth.layout, &node_ips, &ids, rotate)?;
        pki_json = json!({
            "ca_created": report.ca_created,
            "server_created": report.server_created,
            "server_sans": report.server_sans,
        });
    }

    let opts = BootstrapOptions {
        identities,
        enable_auth,
        rotate,
        auth_mode: mode,
        endpoints: state.etcd_auth.endpoints.clone(),
        root_password: body
            .get("root_password")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    };
    let report = provisioning::bootstrap(connector.as_ref(), &state.etcd_auth.store, &opts).await?;

    let mut warnings = Vec::new();
    if enable_auth && state.etcd_auth.mode.is_anonymous() {
        warnings.push(
            "本节点 VCM 仍以匿名模式运行：认证启用后它将无法读取 etcd。请立即把 --etcd-auth-mode 切到 user|mtls|mtls_user（或 auto）并重启本应用".to_string(),
        );
    }

    let mut distributed: Vec<Value> = Vec::new();
    if distribute {
        let names: Vec<String> = report.identities.iter().map(|i| i.identity.clone()).collect();
        distributed = distribute_credentials(&state, &names, &node_ips).await;
    }

    Ok(Json(ResultBody::ok(
        json!({
            "report": {
                "auth_was_enabled": report.auth_was_enabled,
                "auth_enabled": report.auth_enabled,
                "auth_enabled_by_bootstrap": report.auth_enabled_by_bootstrap,
                "verified_after_enable": report.verified_after_enable,
                "root_password_generated": report.root_password_generated,
                "root_password_file": report.root_password_file,
                "roles_ensured": report.roles_ensured,
                "identities": report.identities.iter().map(outcome_json).collect::<Vec<_>>(),
            },
            "pki": pki_json,
            "distributed": distributed,
            "warnings": warnings,
        }),
        version,
    )))
}

/// POST /api/infra/etcd/auth/rotate
///
/// body: `{ "identity": "vcm", "auth_mode"?, "distribute"?: true, "node_ips"? }`
pub async fn etcd_auth_rotate(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let connector = connector_of(&state)?.clone();
    let mode = mode_from_body_or_state(&state, &body)?;
    let identity = body
        .get("identity")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "identity 必填"))?;
    let out = provisioning::rotate_identity(
        connector.as_ref(),
        &state.etcd_auth.store,
        &state.etcd_auth.endpoints,
        mode,
        &identity,
    )
    .await?;

    let mut distributed = Vec::new();
    if body
        .get("distribute")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let node_ips = resolve_node_ips(&state, &body).await?;
        distributed = distribute_credentials(&state, &[identity.clone()], &node_ips).await;
    }

    Ok(Json(ResultBody::ok(
        json!({"identity": outcome_json(&out), "distributed": distributed}),
        version,
    )))
}

/// POST /api/infra/etcd/auth/revoke
///
/// body: `{ "identity": "agent-10-100-10-21", "auth_mode"?, "node_ips"? }`
/// 语义：删除 etcd 用户 + 删除本机凭据/证书；随后把该身份从其他节点清理
/// （分发一个空 `revoke` 指令）。
pub async fn etcd_auth_revoke(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let connector = connector_of(&state)?.clone();
    let mode = mode_from_body_or_state(&state, &body)?;
    let identity = body
        .get("identity")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "identity 必填"))?;
    if identity == ROOT_IDENTITY {
        return Err(VcmError::new(codes::PARAM_INVALID, "禁止吊销 root 身份"));
    }
    let existed = provisioning::revoke_identity(
        connector.as_ref(),
        &state.etcd_auth.store,
        &state.etcd_auth.endpoints,
        mode,
        &identity,
    )
    .await?;

    let mut cleaned = Vec::new();
    if body
        .get("distribute")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let node_ips = resolve_node_ips(&state, &body).await?;
        for ip in node_ips {
            if local_ip().map(|l| l.to_string()).as_deref() == Some(ip.as_str()) {
                continue;
            }
            let res = state
                .remote_client
                .push_etcd_credential(&ip, json!({"identity": identity, "revoke": true}))
                .await;
            cleaned.push(json!({
                "node_ip": ip,
                "ok": res.is_ok(),
                "error": res.err().map(|e| e.msg),
            }));
        }
    }

    Ok(Json(ResultBody::ok(
        json!({"identity": identity, "existed": existed, "cleaned": cleaned}),
        version,
    )))
}

/// POST /api/infra/etcd/auth/distribute
///
/// body: `{ "identities"?: [...], "node_ips"?: [...] }`
pub async fn etcd_auth_distribute(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let identities: Vec<String> = match body.get("identities") {
        Some(v) => v
            .as_array()
            .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "identities 必须为数组"))?
            .iter()
            .map(|i| {
                i.as_str()
                    .map(|s| s.trim().to_string())
                    .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "identities 含非字符串项"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => state.etcd_auth.store.list()?,
    };
    if identities.is_empty() {
        return Err(VcmError::new(
            codes::NOT_FOUND,
            "本机无任何 etcd 凭据可分发（请先执行 bootstrap）",
        ));
    }
    let node_ips = resolve_node_ips(&state, &body).await?;
    let results = distribute_credentials(&state, &identities, &node_ips).await;
    Ok(Json(ResultBody::ok(
        json!({"distributed": results, "count": results.len()}),
        version,
    )))
}

async fn distribute_credentials(
    state: &AppState,
    identities: &[String],
    node_ips: &[String],
) -> Vec<Value> {
    let local = local_ip().map(|ip| ip.to_string());
    let mut out = Vec::new();
    for identity in identities {
        let cred = match state.etcd_auth.store.read(identity) {
            Ok(Some(c)) => c,
            Ok(None) => {
                out.push(json!({
                    "identity": identity,
                    "node_ip": Value::Null,
                    "ok": false,
                    "error": "本机无该身份凭据文件",
                }));
                continue;
            }
            Err(e) => {
                out.push(json!({
                    "identity": identity,
                    "node_ip": Value::Null,
                    "ok": false,
                    "error": e.msg,
                }));
                continue;
            }
        };
        let payload = match serde_json::to_value(&cred) {
            Ok(v) => v,
            Err(e) => {
                out.push(json!({
                    "identity": identity,
                    "node_ip": Value::Null,
                    "ok": false,
                    "error": format!("序列化凭据失败: {e}"),
                }));
                continue;
            }
        };
        for ip in node_ips {
            if local.as_deref() == Some(ip.as_str()) {
                out.push(json!({
                    "identity": identity,
                    "node_ip": ip,
                    "ok": true,
                    "local": true,
                }));
                continue;
            }
            let res = state
                .remote_client
                .push_etcd_credential(ip, payload.clone())
                .await;
            out.push(json!({
                "identity": identity,
                "node_ip": ip,
                "ok": res.is_ok(),
                "error": res.err().map(|e| e.msg),
            }));
        }
    }
    out
}

/// POST /api/infra/etcd/auth/credentials —— 节点侧凭据落点（分发目标）
///
/// body：`CredentialFile` JSON，或 `{ "identity": "...", "revoke": true }`（清理）。
/// 拒绝 root 身份（节点只应持有自己的 scoped 凭据）。
pub async fn etcd_auth_credentials_sink(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let identity = body
        .get("identity")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "identity 必填"))?;
    validate_identity(&identity).map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
    if identity == ROOT_IDENTITY {
        return Err(VcmError::new(
            codes::PARAM_INVALID,
            "节点不得接收 root 凭据（分发仅限 scoped 身份）",
        ));
    }

    if body.get("revoke").and_then(|v| v.as_bool()).unwrap_or(false) {
        let removed = state.etcd_auth.store.remove(&identity)?;
        return Ok(Json(ResultBody::ok(
            json!({"identity": identity, "removed": removed}),
            version,
        )));
    }

    let cred: crate::etcd::CredentialFile = serde_json::from_value(body).map_err(|e| {
        VcmError::new(codes::PARAM_INVALID, format!("凭据结构非法: {e}"))
    })?;
    if cred.identity != identity {
        return Err(VcmError::new(
            codes::PARAM_INVALID,
            "identity 与凭据体不一致",
        ));
    }
    let path = state.etcd_auth.store.write(cred)?;
    Ok(Json(ResultBody::ok(
        json!({"identity": identity, "written": true, "path": path.display().to_string()}),
        version,
    )))
}

/// GET /api/infra/etcd/auth/credentials —— 本机已落盘身份（无密钥内容）
pub async fn etcd_auth_credentials_list(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let ids = state.etcd_auth.store.list()?;
    let dir = state.etcd_auth.layout.clients_dir().display().to_string();
    Ok(Json(ResultBody::ok(
        json!({"identities": ids, "credential_dir": dir, "local_identity": state.etcd_auth.identity,
               "local_credential_present": ids.iter().any(|i| i == &state.etcd_auth.identity)}),
        version,
    )))
}

/// 测试 / 运维辅助：本机默认身份名
pub fn default_identity() -> &'static str {
    DEFAULT_VCM_IDENTITY
}

/// agent 身份名（外部工具/脚本用）
pub fn agent_identity_for(ip: &str) -> Option<String> {
    agent_identity(ip)
}