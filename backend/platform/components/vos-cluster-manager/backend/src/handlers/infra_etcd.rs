//! etcd 子项目动态编排 API（/api/infra/etcd/*，Phase 2）
//!
//! 经平台 projects API 动态启停 etcd 子项目（D-VCM-03——主容器不跑基础设施进程）。
//! 编排纯函数见 `orchestration/`；平台契约见 `projects/`。

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::codes;
use crate::error::{ResultBody, VcmError};
use crate::orchestration::{
    build_etcd_env_vars, etcd_compose_name, peer_url, validate_compose_project_name, EtcdServerTls,
    EtcdTopology,
};
use crate::projects::ProjectDeployBo;
use crate::state::AppState;

/// 本应用 app_id（平台 projects API 需要）
use crate::projects::APP_ID;

/// 从现有实例推导下一个可用 index（移除后可复用新编号；活跃实例名唯一）
fn next_etcd_index(existing: &[crate::projects::ProjectInstanceVo]) -> u32 {
    let mut max: Option<u32> = None;
    for inst in existing {
        let name = &inst.compose_project_name;
        if let Some(rest) = name.strip_prefix("cluster-etcd-node-")
            && let Ok(n) = rest.parse::<u32>()
        {
            max = Some(max.map_or(n, |m| m.max(n)));
        }
    }
    max.map_or(0, |m| m + 1)
}

/// GET /api/infra/etcd/status —— 本节点 etcd 实例 + 连通性聚合（业务态）
pub async fn etcd_status(State(state): State<AppState>) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    // 用**串行读**判"本机 etcd 已就绪"：join 期间目标节点在 promote 前是 learner，
    // 线性一致读会被 etcd 拒绝（rpc not supported for learner）→ 永远判未就绪。
    let etcd_ok = state.etcd_cluster_client.ping_local().await;
    let member_count = state.etcd_cluster_client.member_count().await;

    // 实例状态（找已部署的 etcd 实例）
    let instances = state.projects_client.instances().await?;
    let deployed: Vec<_> = instances
        .iter()
        .filter(|i| i.project_name == "etcd")
        .filter(|i| i.state == "deployed")
        .collect();
    let compose_names: Vec<String> = deployed
        .iter()
        .map(|i| i.compose_project_name.clone())
        .collect();

    let mut instance_details = Vec::new();
    for name in &compose_names {
        if let Some(d) = state.projects_client.status(name).await? {
            instance_details.push(d);
        }
    }

    Ok(Json(ResultBody::ok(
        json!({
            "etcd_ok": etcd_ok,
            "member_count": member_count.ok(),
            "instances": instance_details,
            "db_ok": state.db_ok(),
        }),
        version,
    )))
}

/// GET /api/infra/etcd/instances —— 本应用 etcd 子项目实例列表
pub async fn etcd_instances(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let instances = state.projects_client.instances().await?;
    let etcd: Vec<_> = instances
        .into_iter()
        .filter(|i| i.project_name == "etcd")
        .collect();
    Ok(Json(ResultBody::ok(json!({"instances": etcd}), version)))
}

/// 幂等检查：本节点已有**活跃** etcd 实例（deploying/deployed）→ 重复部署防护
fn check_no_active_etcd(instances: &[crate::projects::ProjectInstanceVo]) -> Result<(), VcmError> {
    if instances.iter().any(|i| {
        i.project_name == "etcd" && (i.state == "deployed" || i.state == "deploying")
    }) {
        return Err(VcmError::new(
            codes::EXISTS,
            "本节点已部署 etcd 子项目（幂等防护；如需重建请先 remove）",
        ));
    }
    Ok(())
}

/// POST /api/infra/etcd/deploy_async —— 异步部署本节点 etcd（返回平台 task_id）
pub async fn etcd_deploy_async(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let instances = state.projects_client.instances().await?;
    check_no_active_etcd(&instances)?;
    let bo = build_deploy_bo(&state, &body).await?;
    let task_id = state.projects_client.deploy_async(&bo).await?;
    Ok(Json(ResultBody::ok(
        json!({"task_id": task_id, "compose_project_name": bo.compose_project_name}),
        version,
    )))
}

/// POST /api/infra/etcd/deploy —— 同步部署本节点 etcd（轮询平台 workflow 至终态）
pub async fn etcd_deploy(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let instances = state.projects_client.instances().await?;
    check_no_active_etcd(&instances)?;
    let bo = build_deploy_bo(&state, &body).await?;
    let instance = state.projects_client.deploy_sync(&bo).await?;
    Ok(Json(ResultBody::ok(
        json!({"instance": instance}),
        version,
    )))
}

/// POST /api/infra/etcd/remove_async —— 异步移除（返回平台 task_id）
pub async fn etcd_remove_async(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let compose_project_name = extract_name(&body)?;
    // 存在性：不在本应用实例列表 → 20505
    let instances = state.projects_client.instances().await?;
    if !instances.iter().any(|i| i.compose_project_name == compose_project_name) {
        return Err(VcmError::new(
            codes::NOT_FOUND,
            format!("实例 {compose_project_name} 不存在"),
        ));
    }
    let task_id = state.projects_client.remove_async(&compose_project_name).await?;
    Ok(Json(ResultBody::ok(
        json!({"task_id": task_id, "compose_project_name": compose_project_name}),
        version,
    )))
}

/// POST /api/infra/etcd/remove —— 同步移除
pub async fn etcd_remove(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let compose_project_name = extract_name(&body)?;
    let instances = state.projects_client.instances().await?;
    if !instances.iter().any(|i| i.compose_project_name == compose_project_name) {
        return Err(VcmError::new(
            codes::NOT_FOUND,
            format!("实例 {compose_project_name} 不存在"),
        ));
    }
    state
        .projects_client
        .remove_sync(&compose_project_name)
        .await?;
    Ok(Json(ResultBody::ok(
        json!({"removed": true, "compose_project_name": compose_project_name}),
        version,
    )))
}

/// 构造部署 Bo：解析入参 + 拓扑 → compose_project_name + env_vars
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

    let node_name = body
        .get("node_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // initial_state（new / existing）
    let initial_state = body
        .get("initial_state")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| s == "new" || s == "existing")
        .unwrap_or_else(|| "new".to_string());

    // initial_members：[(name, ip)]（多节点 raft 初始成员；成员地址一律归一化为 peer URL）
    let mut initial_members: Vec<(String, String)> = Vec::new();
    if let Some(members) = body.get("initial_members").and_then(|v| v.as_array()) {
        for m in members {
            let name = m.get("name").and_then(|v| v.as_str());
            let ip = m.get("node_ip").and_then(|v| v.as_str());
            match (name, ip) {
                (Some(n), Some(ip)) if ip.parse::<std::net::IpAddr>().is_ok() => {
                    initial_members.push((n.trim().to_string(), peer_url(ip.trim())));
                }
                _ => {
                    return Err(VcmError::new(
                        codes::PARAM_INVALID,
                        "initial_members 每项需 name(String) + node_ip(合法 IP)",
                    ));
                }
            }
        }
    }

    // 实例数推导 index 与 node_name 缺省
    let instances = state.projects_client.instances().await?;
    let index = next_etcd_index(&instances);
    let node_name = node_name.unwrap_or_else(|| format!("etcd{index}"));

    // 单节点缺省：initial_members 未提供时 = 自身（peer URL，见 orchestration::peer_url）
    if initial_members.is_empty() {
        initial_members = vec![(node_name.clone(), peer_url(&node_ip))];
    }

    let tls = state.etcd_auth.mode.requires_tls().then(|| EtcdServerTls {
        pki_dir: state
            .etcd_auth
            .layout
            .pki_dir()
            .display()
            .to_string(),
        client_cert_auth: true,
    });

    let topo = EtcdTopology {
        node_name: node_name.clone(),
        client_host: node_ip.clone(),
        peer_host: node_ip.clone(),
        initial_members,
        initial_state,
        tls,
    };
    let env_vars = build_etcd_env_vars(&topo);

    let compose_project_name = etcd_compose_name(index);
    if !validate_compose_project_name(&compose_project_name) {
        return Err(VcmError::new(
            codes::PARAM_INVALID,
            format!("compose_project_name 非法: {compose_project_name}"),
        ));
    }

    Ok(ProjectDeployBo {
        app_id: APP_ID.to_string(),
        project_name: "etcd".to_string(),
        compose_project_name,
        env_vars,
        health_timeout_secs: Some(300),
    })
}

/// 提取 compose_project_name 参数
fn extract_name(body: &Value) -> Result<String, VcmError> {
    body.get("compose_project_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "compose_project_name 必填"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_index_logic() {
        let mk = |name: &str, state: &str| crate::projects::ProjectInstanceVo {
            id: "x".into(),
            app_id: APP_ID.into(),
            project_name: "etcd".into(),
            compose_project_name: name.into(),
            state: state.into(),
            deployed_at: None,
            removed_at: None,
            create_time: "t".into(),
            updated_at: "t".into(),
        };
        // 空 → 0
        assert_eq!(next_etcd_index(&[]), 0);
        // 已有 0 / 2（1 被移除过）→ 3
        assert_eq!(
            next_etcd_index(&[mk("cluster-etcd-node-0", "deployed"), mk("cluster-etcd-node-2", "removed")]),
            3
        );
        // removed 实例不占名（新 index 复用无冲突）
        assert_eq!(next_etcd_index(&[mk("cluster-etcd-node-0", "removed")]), 1);
    }
}