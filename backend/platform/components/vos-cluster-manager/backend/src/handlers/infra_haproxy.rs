//! haproxy 子项目动态编排 API（/api/infra/haproxy/*，二期 S1 cluster-haproxy-lb）
//!
//! 镜像 `/api/infra/pg/*` 模式（ProjectsClient + 平台 projects 原语），差异点：
//! - **每节点一个实例**：compose_project_name = `cluster-haproxy-<ip-slug>`（D-HA-02），
//!   与 etcd/pg 的序号命名不同——haproxy 是节点本地服务，IP slug 自描述且跨节点防重名
//! - **deploy = upsert 刷新**（D-HA-04）：同实例活跃时先 remove 再 deploy（拓扑变更后
//!   重渲染配置；leader 漂移本身由运行时 8008 http-check 收敛，无需刷新）
//! - **前置**（D-HA-03）：etcd 可达且存在 ≥1 个 manager 节点记录（20507）——backends
//!   自动从 etcd NodeMetadata 推导，不强制 postgres_ha_ok（haproxy 可先于 pg 全量就绪）
//! - **状态聚合**：实例清单 + 本机 15432 TCP 探测 + managers 遍历 8008 /leader 找 leader；
//!   探测失败仅降级 false，不报错
//! - 所有响应不回显任何凭据（backends 仅节点 IP）

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::codes;
use crate::error::{ResultBody, VcmError};
use crate::etcd::dto;
use crate::orchestration::{
    NFS_FRONT_PORT, PG_CHECK_PORT, PG_FRONT_PORT, RGW_FRONT_PORT, SMB_FRONT_PORT, STATS_PORT,
    build_haproxy_env_vars, haproxy_compose_name, parse_ip_list, render_haproxy_cfg,
    validate_compose_project_name, HaproxyTopology,
};
use crate::projects::ProjectDeployBo;
use crate::state::AppState;

/// 本应用 app_id（平台 projects API 需要）
use crate::projects::APP_ID;
/// haproxy 实例 compose 名前缀（过滤 / 幂等判定共用）
const HAPROXY_NAME_PREFIX: &str = "cluster-haproxy-";

/// 列出已注册 manager 节点 IP（backends 推导，D-HA-03）；按 IP 排序保证跨调用稳定
pub(crate) async fn manager_backend_ips(state: &AppState) -> Result<Vec<String>, VcmError> {
    let records = state.etcd_cluster_client.get_all_node_records().await?;
    let mut ips: Vec<String> = records
        .iter()
        .filter(|r| dto::role::normalize_legacy(&r.metadata.role) == dto::role::MANAGER)
        .map(|r| r.metadata.node_ip.clone())
        .collect();
    ips.sort();
    ips.dedup();
    Ok(ips)
}

/// 前置校验：etcd 可达（20507）+ ≥1 manager 节点记录（20507）→ 返回 backends
async fn ensure_haproxy_prereq(state: &AppState) -> Result<Vec<String>, VcmError> {
    if !state.etcd_cluster_client.ping().await {
        return Err(VcmError::new(
            codes::STATE_NOT_SUPPORTED,
            "etcd 未就绪：集群状态不可读。请先部署 etcd 子项目（POST /api/infra/etcd/deploy）",
        ));
    }
    let backends = manager_backend_ips(state).await?;
    if backends.is_empty() {
        return Err(VcmError::new(
            codes::STATE_NOT_SUPPORTED,
            "集群无 manager 节点记录：postgres HA 后端无从推导。请先初始化集群并录入 manager",
        ));
    }
    Ok(backends)
}

/// 是否活跃实例（deploying/deployed）
fn is_active(inst: &crate::projects::ProjectInstanceVo) -> bool {
    inst.state == "deployed" || inst.state == "deploying"
}

/// 解析显式后端清单（逗号分隔字符串或字符串数组）；空 → None。
///
/// 编排方（manager，具备 etcd 读能力）推导拓扑后经 `at` 转发注入；目标节点（可能是
/// 无本地 etcd 的 worker）直接采用，不再本地重推导——与 etcd/pg 远程部署同构
/// （编排方计算拓扑 → 目标节点仅执行子项目部署）。
fn parse_backends_list(v: Option<&Value>) -> Option<Vec<String>> {
    let raw: Vec<String> = match v? {
        Value::String(s) => s.split(',').map(|x| x.trim().to_string()).collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
            .collect(),
        _ => return None,
    };
    let list: Vec<String> = raw.into_iter().filter(|s| !s.is_empty()).collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

/// 构造部署 Bo：解析 node_ip（20501）→ backends（显式注入优先，否则 etcd 推导
/// 含 20507 前置）→ HaproxyTopology → env_vars + compose_project_name。
///
/// 网关 VIP 地址（SMB/NFS 必须绑定独立 VIP，避免与节点本地 445/2049 冲突）
fn haproxy_vip() -> Option<String> {
    std::env::var("VOS_CLUSTER_MANAGER_HAPROXY_VIP")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn require_nonlocal_bind() -> Result<(), VcmError> {
    let value = std::fs::read_to_string("/proc/sys/net/ipv4/ip_nonlocal_bind")
        .unwrap_or_default();
    if value.trim() == "1" {
        Ok(())
    } else {
        Err(VcmError::new(
            codes::STATE_NOT_SUPPORTED,
            "RGW HA 需要 HAProxy 节点启用 net.ipv4.ip_nonlocal_bind=1（当前未启用）",
        ))
    }
}

fn haproxy_rgw_vip() -> Option<String> {
    std::env::var("VOS_CLUSTER_MANAGER_HAPROXY_RGW_VIP")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `pub(crate)`：S2 cluster-create workflow activity 复用（防逻辑漂移，同 infra_pg）。
pub(crate) async fn build_deploy_bo(
    state: &AppState,
    body: &Value,
) -> Result<ProjectDeployBo, VcmError> {
    // ---- 参数校验（20501）----
    let node_ip = body
        .get("node_ip")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "node_ip 必填且为合法 IP 地址"))?;

    // ---- backends：显式注入优先（编排方经 at 转发），否则 etcd 本地推导（20507 前置）----
    let pg_backends = match parse_backends_list(body.get("pg_backends")) {
        Some(v) => v,
        None => ensure_haproxy_prereq(state).await?,
    };

    // temporal 后端仅由请求体显式注入（S2 refresh_cluster_haproxy / 编排方注入）；
    // 缺省空 → compose 不渲染 temporal listener（保持 build_deploy_bo 纯函数、松耦合）
    let temporal_backends = parse_backends_list(body.get("temporal_backends")).unwrap_or_default();

    // 网关 VIP 后端：显式注入优先（编排方），否则取管理面 active 集合（排空的不进入）。
    // 注意：**显式空数组表示“该服务不启用”**，不能回退到管理面 active 集合，
    // 否则关闭 NFS/SMB 后仍会渲染对应 listener（例如 NFS:2049 与宿主机 nfsd 冲突）。
    let smb_backends = match body.get("smb_backends") {
        Some(v) => parse_ip_list(Some(v)).unwrap_or_default(),
        None => super::gateway::active_backends(state, "smb").await,
    };
    let nfs_backends = match body.get("nfs_backends") {
        Some(v) => parse_ip_list(Some(v)).unwrap_or_default(),
        None => super::gateway::active_backends(state, "nfs").await,
    };

    let vip = body
        .get("vip")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| v.parse::<std::net::IpAddr>().is_ok())
        .or_else(haproxy_vip);

    let rgw_backends = match body.get("rgw_backends") {
        Some(v) => parse_ip_list(Some(v)).unwrap_or_default(),
        None => super::gateway::active_backends(state, "rgw").await,
    };
    let rgw_vip = body
        .get("rgw_vip")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| v.parse::<std::net::IpAddr>().is_ok())
        .or_else(haproxy_rgw_vip);

    if rgw_vip.is_some() && !rgw_backends.is_empty() {
        require_nonlocal_bind()?;
    }

    let topo = HaproxyTopology {
        pg_backends,
        temporal_backends,
        smb_backends,
        nfs_backends,
        vip,
        rgw_backends,
        rgw_vip,
    };
    let env_vars = build_haproxy_env_vars(&topo);

    let compose_project_name = haproxy_compose_name(&node_ip).ok_or_else(|| {
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
        project_name: "haproxy".to_string(),
        compose_project_name,
        env_vars,
        // haproxy 无慢启动依赖（渲染 + 校验秒级）；窗口取小值快速失败
        health_timeout_secs: Some(120),
    })
}

/// GET /api/infra/haproxy/status —— 实例 + 本机入口探测 + leader 视图聚合（业务态）
pub async fn haproxy_status(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let mut data = haproxy_status_data(&state, true).await?;
    if let Some(obj) = data.as_object_mut() {
        obj.insert("db_ok".to_string(), json!(state.db_ok()));
    }
    Ok(Json(ResultBody::ok(data, version)))
}

/// 聚合视图（`/api/infra/haproxy/status` 与 `cluster/status` 共用，D-HA-05）
///
/// - `probe_leader`：是否遍历 managers 8008 探测 patroni leader（cluster 聚合跳过以控时延）
/// - 全部探测失败仅降级为 false/null，不报错；projects_client 故障 → 空实例清单
pub(crate) async fn haproxy_status_data(
    state: &AppState,
    probe_leader: bool,
) -> Result<Value, VcmError> {
    // backends 视图（etcd 不可达 → 空，不报错）
    let pg_backends = manager_backend_ips(state).await.unwrap_or_default();

    // 本机统一入口 TCP 探测（主容器 host 网络，127.0.0.1:15432 即本机 haproxy）
    let local_pg_front_reachable = tokio::net::TcpStream::connect(("127.0.0.1", PG_FRONT_PORT))
        .await
        .is_ok();

    // patroni leader 视图：遍历 managers 的 8008 /leader，首个 200 即 leader IP
    let mut patroni_leader: Option<String> = None;
    if probe_leader
        && !pg_backends.is_empty()
        && let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
    {
        for ip in &pg_backends {
            let url = format!("http://{ip}:{PG_CHECK_PORT}/leader");
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    patroni_leader = Some(ip.clone());
                    break;
                }
                _ => continue,
            }
        }
    }

    // 本应用 haproxy 实例清单（deployed 取详情；projects 故障 → 空清单降级）
    let instances = match state.projects_client.instances().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(err = %e.msg, "haproxy status：projects 实例列表获取失败（降级空清单）");
            Vec::new()
        }
    };
    let haproxy: Vec<_> = instances
        .iter()
        .filter(|i| i.compose_project_name.starts_with(HAPROXY_NAME_PREFIX))
        .collect();
    let mut instance_details = Vec::new();
    for inst in &haproxy {
        if inst.state == "deployed" {
            match state.projects_client.status(&inst.compose_project_name).await {
                Ok(Some(d)) => instance_details.push(d),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        err = %e.msg,
                        name = %inst.compose_project_name,
                        "haproxy status：实例详情获取失败（降级跳过）"
                    );
                }
            }
        }
    }
    let deployed_count = instance_details.len();
    let haproxy_ok = deployed_count > 0 && local_pg_front_reachable;

    let smb_backends = super::gateway::active_backends(state, "smb").await;
    let nfs_backends = super::gateway::active_backends(state, "nfs").await;
    let rgw_backends = super::gateway::active_backends(state, "rgw").await;
    let drained = match state.db.as_ref() {
        Some(db) => crate::db::list_gateway_backends(db)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.state == crate::entity::vcm_gateway_backend::state::DRAINED)
            .map(|r| json!({"service": r.service, "node_ip": r.node_ip, "reason": r.reason}))
            .collect::<Vec<_>>(),
        None => Vec::new(),
    };

    Ok(json!({
        "haproxy_ok": haproxy_ok,
        "front_port": PG_FRONT_PORT,
        "stats_port": STATS_PORT,
        "pg_backends": pg_backends,
        "deployed_count": deployed_count,
        "instances": instance_details,
        "local_pg_front_reachable": local_pg_front_reachable,
        "patroni_leader": patroni_leader,
        "gateway_vip": {
            "smb": {"front_port": SMB_FRONT_PORT, "backends": smb_backends},
            "nfs": {"front_port": NFS_FRONT_PORT, "backends": nfs_backends},
            "rgw": {"front_port": RGW_FRONT_PORT, "vip": haproxy_rgw_vip(), "backends": rgw_backends},
            "drained": drained,
        },
    }))
}

/// GET /api/infra/haproxy/instances —— 本应用 haproxy 子项目实例列表
pub async fn haproxy_instances(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let instances = state.projects_client.instances().await?;
    let haproxy: Vec<_> = instances
        .into_iter()
        .filter(|i| i.compose_project_name.starts_with(HAPROXY_NAME_PREFIX))
        .collect();
    Ok(Json(ResultBody::ok(json!({"instances": haproxy}), version)))
}

/// GET /api/infra/haproxy/nodes —— 跨节点入口部署态（节点管理页「统一入口」列）
///
/// 遍历 etcd 全节点记录，对每个 `node_ip` 经 remote_client.list_instances(ip,"haproxy")
/// 查目标节点本应用是否已有对应 `cluster-haproxy-<ip-slug>` 实例（目标节点用其本地
/// projects 视角回答）。目标不可达 / 查询失败 → present=false（保守显示「未部署」）。
pub async fn haproxy_nodes(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let records = match state.etcd_cluster_client.get_all_node_records().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(err = %e.msg, "haproxy nodes：etcd 节点记录获取失败（返回空清单）");
            Vec::new()
        }
    };
    let mut nodes = Vec::new();
    for r in records {
        let ip = r.metadata.node_ip.clone();
        let slug = haproxy_compose_name(&ip);
        let present = match (&slug, state.remote_client.list_instances(&ip, "haproxy").await) {
            (Some(name), Ok(names)) => names.iter().any(|n| n == name),
            _ => false,
        };
        nodes.push(json!({
            "node_ip": ip,
            "compose_project_name": slug,
            "haproxy_present": present,
        }));
    }
    Ok(Json(ResultBody::ok(
        json!({"nodes": nodes, "front_port": PG_FRONT_PORT}),
        version,
    )))
}

/// 提取可选 `at`（目标节点 IP）：存在则本 handler 作为编排方转发到目标节点应用
/// （目标节点用**本地** access-token 调**本地**平台 API，token 不出节点；一期
/// remote 互信通道复用）。返回 None = 部署/移除在本节点。
fn extract_at(body: &Value) -> Option<String> {
    body.get("at")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
}

/// POST /api/infra/haproxy/refresh_all —— 按 etcd + 实例现态重刷全集群 haproxy
/// （运维兜底 / 验证 G3；委托 infra_temporal::refresh_cluster_haproxy，D-TE-04）
pub async fn haproxy_refresh_all(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    if !state.etcd_cluster_client.ping().await {
        return Err(VcmError::new(
            codes::STATE_NOT_SUPPORTED,
            "etcd 未就绪：无法推导全集群拓扑",
        ));
    }
    let results = super::infra_temporal::refresh_cluster_haproxy(&state).await;
    Ok(Json(ResultBody::ok(
        json!({"refreshed": results, "count": results.len()}),
        version,
    )))
}

/// POST /api/infra/haproxy/deploy —— 同步部署（upsert 刷新语义，D-HA-04；
/// 带 `at` = 转发到目标节点执行）
pub async fn haproxy_deploy(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;

    // 远程转发通道（前端节点页跨节点部署入口）：编排方推导 backends 后随体注入，
    // 目标节点（可能是无本地 etcd 的 worker）直接采用不再本地重推导
    if let Some(at) = extract_at(&body) {
        let backends = match parse_backends_list(body.get("pg_backends")) {
            Some(v) => v,
            None => ensure_haproxy_prereq(&state).await?,
        };
        let fwd = state
            .remote_client
            .deploy_haproxy_sync(
                &at,
                json!({"node_ip": at, "pg_backends": backends.join(",")}),
            )
            .await?;
        return Ok(Json(ResultBody::ok(fwd, version)));
    }

    let bo = build_deploy_bo(&state, &body).await?;

    // upsert：同实例活跃 → 先 remove 再 deploy（拓扑变更后的配置刷新通道；
    // 窗口 ~10s 本机入口拒连，见 design Risks）
    let instances = state.projects_client.instances().await?;
    let existing_active = instances
        .iter()
        .any(|i| i.compose_project_name == bo.compose_project_name && is_active(i));
    let refreshed = existing_active;
    if existing_active {
        tracing::info!(
            compose_project_name = %bo.compose_project_name,
            "haproxy 实例已活跃 → upsert 刷新（remove + deploy）"
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

/// POST /api/infra/haproxy/deploy_async —— 异步部署（返回平台 task_id；upsert 语义同上；
/// 带 `at` 转发目标节点——目标侧降级为同步通道后返回其 instance 视图）
pub async fn haproxy_deploy_async(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;

    if let Some(at) = extract_at(&body) {
        let backends = match parse_backends_list(body.get("pg_backends")) {
            Some(v) => v,
            None => ensure_haproxy_prereq(&state).await?,
        };
        let fwd = state
            .remote_client
            .deploy_haproxy_sync(
                &at,
                json!({"node_ip": at, "pg_backends": backends.join(",")}),
            )
            .await?;
        return Ok(Json(ResultBody::ok(fwd, version)));
    }
    let bo = build_deploy_bo(&state, &body).await?;

    let instances = state.projects_client.instances().await?;
    let existing_active = instances
        .iter()
        .any(|i| i.compose_project_name == bo.compose_project_name && is_active(i));
    if existing_active {
        // 异步通道保留同步 remove（秒级操作），deploy 转异步
        state
            .projects_client
            .remove_sync(&bo.compose_project_name)
            .await?;
    }

    let task_id = state.projects_client.deploy_async(&bo).await?;
    Ok(Json(ResultBody::ok(
        json!({"task_id": task_id, "compose_project_name": bo.compose_project_name}),
        version,
    )))
}

/// 提取 compose_project_name 参数
fn extract_name(body: &Value) -> Result<String, VcmError> {
    body.get("compose_project_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "compose_project_name 必填"))
}

/// haproxy 实例存在性检查（remove 前置）：不存在 → 20505
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
            format!("实例 {compose_project_name} 不存在"),
        ));
    }
    Ok(())
}

/// POST /api/infra/haproxy/remove —— 同步移除（带 `at` = 转发目标节点）
pub async fn haproxy_remove(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let compose_project_name = extract_name(&body)?;
    if let Some(at) = extract_at(&body) {
        let fwd = state
            .remote_client
            .remove_haproxy_sync(&at, &compose_project_name)
            .await?;
        return Ok(Json(ResultBody::ok(fwd, version)));
    }
    ensure_instance_exists(&state, &compose_project_name).await?;
    state
        .projects_client
        .remove_sync(&compose_project_name)
        .await?;
    Ok(Json(ResultBody::ok(
        json!({"removed": true, "compose_project_name": compose_project_name}),
        version,
    )))
}

/// POST /api/infra/haproxy/remove_async —— 异步移除
pub async fn haproxy_remove_async(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let compose_project_name = extract_name(&body)?;
    ensure_instance_exists(&state, &compose_project_name).await?;
    let task_id = state
        .projects_client
        .remove_async(&compose_project_name)
        .await?;
    Ok(Json(ResultBody::ok(
        json!({"task_id": task_id, "compose_project_name": compose_project_name}),
        version,
    )))
}

/// GET /api/infra/haproxy/config_preview —— 渲染预览（运维核对 / 与 compose 模板比对）
///
/// 后端集合来源：管理面 active 网关 + etcd manager（pg）+ 本机 temporal 实例推导，
/// 与 `deploy` 的 env 完全同源，便于在改配置前先看结果。
pub async fn haproxy_config_preview(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let pg_backends = manager_backend_ips(&state).await.unwrap_or_default();
    let smb_backends = super::gateway::active_backends(&state, "smb").await;
    let nfs_backends = super::gateway::active_backends(&state, "nfs").await;
    let rgw_backends = super::gateway::active_backends(&state, "rgw").await;
    let topo = HaproxyTopology {
        pg_backends,
        temporal_backends: Vec::new(),
        smb_backends,
        nfs_backends,
        vip: haproxy_vip(),
        rgw_backends,
        rgw_vip: haproxy_rgw_vip(),
    };
    let cfg = render_haproxy_cfg(&topo);
    Ok(Json(ResultBody::ok(
        json!({
            "config": cfg,
            "env": build_haproxy_env_vars(&topo),
            "note": "运行时以 projects/haproxy/docker-compose.yml 内联脚本为准；本预览用于改配置前核对",
        }),
        version,
    )))
}
