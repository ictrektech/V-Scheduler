//! postgres-ha（patroni）子项目动态编排 API（/api/infra/pg/*，Phase 3）
//!
//! 镜像 `/api/infra/etcd/*` 模式（ProjectsClient + 平台 projects 原语），扩展：
//! - **DCS 前置**：etcd 子项目未就绪（2379 不可达 / member_count<1）→ 20507
//! - **凭据**：超级用户 / 复制用户密码由 `gen_credential()` 生成，部署时经 env_vars 注入，
//!   部署成功后回写 etcd NodeCredentials（`pg_superuser_password` / `pg_replication_password`）；
//!   所有响应不回显明文
//! - 端口固定：5433（postgres）/ 8008（patroni REST）

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::codes;
use crate::error::{ResultBody, VcmError};
use crate::orchestration::{
    build_patroni_env_vars, gen_credential, pg_compose_name, validate_compose_project_name,
    PatroniTopology,
};
use crate::projects::ProjectDeployBo;
use crate::state::AppState;

/// 本应用 app_id（平台 projects API 需要）
use crate::projects::APP_ID;
/// 默认 scope（集群内所有 patroni 实例共享）
const DEFAULT_SCOPE: &str = "vos-cluster";
/// 默认 DCS 端点（本机 etcd 子项目）
const DEFAULT_ETCD_HOSTS: &str = "127.0.0.1:2379";

/// 从现有实例推导下一个可用 index（与 infra_etcd 同规则）
fn next_pg_index(existing: &[crate::projects::ProjectInstanceVo]) -> u32 {
    let mut max: Option<u32> = None;
    for inst in existing {
        let name = &inst.compose_project_name;
        if let Some(rest) = name.strip_prefix("cluster-pg-node-")
            && let Ok(n) = rest.parse::<u32>()
        {
            max = Some(max.map_or(n, |m| m.max(n)));
        }
    }
    max.map_or(0, |m| m + 1)
}

/// DCS 前置校验：etcd 子项目必须就绪（patroni 依赖 etcd DCS，D-PG-05）
async fn ensure_etcd_dcs_ready(state: &AppState) -> Result<(), VcmError> {
    if !state.etcd_cluster_client.ping().await {
        return Err(VcmError::new(
            codes::STATE_NOT_SUPPORTED,
            "etcd DCS 未就绪：本机 etcd（2379）不可达。请先部署 etcd 子项目（POST /api/infra/etcd/deploy）",
        ));
    }
    let n = state.etcd_cluster_client.member_count().await?;
    if n < 1 {
        return Err(VcmError::new(
            codes::STATE_NOT_SUPPORTED,
            "etcd DCS 未就绪：raft 成员数为 0。请先部署 etcd 子项目",
        ));
    }
    Ok(())
}

/// 幂等检查：本应用已有**活跃** postgres-ha 实例（deploying/deployed）
fn check_no_active_pg(instances: &[crate::projects::ProjectInstanceVo]) -> Result<(), VcmError> {
    if instances.iter().any(|i| {
        i.compose_project_name.starts_with("cluster-pg-node-")
            && (i.state == "deployed" || i.state == "deploying")
    }) {
        return Err(VcmError::new(
            codes::EXISTS,
            "本节点已部署 postgres-ha 子项目（幂等防护；如需重建请先 remove）",
        ));
    }
    Ok(())
}

/// GET /api/infra/pg/status —— 实例 + patroni REST 领导权 + pg 连通聚合（业务态）
pub async fn pg_status(State(state): State<AppState>) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let dcs_ready = state.etcd_cluster_client.ping().await;

    // patroni REST 探测（本节点 8008 /leader；200 即 leader 就绪）
    let patroni_leader: Option<Value> = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(client) => match client
            .get(format!("http://127.0.0.1:{}/leader", crate::orchestration::PATRONI_REST_PORT))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                resp.json::<serde_json::Value>().await.ok()
            }
            _ => None,
        },
        Err(_) => None,
    };

    // pg 连通（5433 tcp 探活）
    let pg_tcp_ok = tokio::net::TcpStream::connect((
        "127.0.0.1",
        crate::orchestration::PG_PORT,
    ))
    .await
    .is_ok();
    let pg_ready = patroni_leader.is_some() && pg_tcp_ok;

    // 实例详情
    let instances = state.projects_client.instances().await?;
    let deployed: Vec<_> = instances
        .iter()
        .filter(|i| i.compose_project_name.starts_with("cluster-pg-node-"))
        .filter(|i| i.state == "deployed")
        .map(|i| i.compose_project_name.clone())
        .collect();
    let mut instance_details = Vec::new();
    for name in &deployed {
        if let Some(d) = state.projects_client.status(name).await? {
            instance_details.push(d);
        }
    }

    Ok(Json(ResultBody::ok(
        json!({
            "postgres_ha_ok": pg_ready,
            "patroni_leader": patroni_leader,
            "pg_tcp_ok": pg_tcp_ok,
            "dcs_ready": dcs_ready,
            "instances": instance_details,
            "db_ok": state.db_ok(),
        }),
        version,
    )))
}

/// GET /api/infra/pg/instances —— 本应用 postgres-ha 子项目实例列表
pub async fn pg_instances(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let instances = state.projects_client.instances().await?;
    let pg: Vec<_> = instances
        .into_iter()
        .filter(|i| i.compose_project_name.starts_with("cluster-pg-node-"))
        .collect();
    Ok(Json(ResultBody::ok(json!({"instances": pg}), version)))
}

/// 构造部署 Bo：解析入参 + 生成凭据 → PatroniTopology → env_vars + compose_project_name
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

    let scope = body
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SCOPE.to_string());

    let is_bootstrap = body
        .get("bootstrap")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // etcd_hosts：缺省本机 etcd 子项目；多节点逗号分隔（"ip:port"）
    let etcd_hosts: Vec<String> = body
        .get("etcd_hosts")
        .and_then(|v| v.as_str())
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| vec![DEFAULT_ETCD_HOSTS.to_string()]);

    // ---- 实例数推导 index 与 node_name 缺省 ----
    let instances = state.projects_client.instances().await?;
    let index = next_pg_index(&instances);
    let node_name = node_name.unwrap_or_else(|| format!("pg-node-{index}"));

    // ---- 凭据（Phase 4 D-CR-03 修正：bootstrap 生成并写 etcd，join 读取复用；
//      否则各节点独立随机密码 → replicator 认证失败无法 basebackup）----
    let (superuser_password, replication_password) = if is_bootstrap {
        let su = gen_credential();
        let re = gen_credential();
        let cred = crate::etcd::dto::PgCredentials {
            superuser_username: "postgres".to_string(),
            superuser_password: su.clone(),
            replication_username: "replicator".to_string(),
            replication_password: re.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        // 写 etcd（bootstrap 前置条件：etcd DCS 须可用；失败即阻断部署，避免
        // join 节点读取不到导致整集群凭据不一致）
        state
            .etcd_cluster_client
            .put_cluster_pg_credentials(&cred)
            .await
            .map_err(|e| {
                VcmError::new(
                    codes::ETCD_FAILED,
                    format!("bootstrap 写 pg 凭据到 etcd 失败（join 节点将无法复用）: {}", e.msg),
                )
            })?;
        (su, re)
    } else {
        // join：优先复用 bootstrap 写入的集群凭据；缺失（独立部署场景 / 曾手工
        // 部署）回退独立生成并告警（此场景 join 可能失败，由上层重试/重建兜底）
        match state.etcd_cluster_client.get_cluster_pg_credentials().await {
            Ok(Some(c)) => (c.superuser_password.clone(), c.replication_password.clone()),
            Ok(None) => {
                tracing::warn!(node_ip = %node_ip, "join 未读取到集群 pg 凭据（etcd 无记录），回退独立生成——replicator 密码将与 bootstrap 不一致，basebackup 可能失败");
                (gen_credential(), gen_credential())
            }
            Err(e) => {
                tracing::warn!(err = %e.msg, node_ip = %node_ip, "join 读取集群 pg 凭据失败，回退独立生成");
                (gen_credential(), gen_credential())
            }
        }
    };

    let topo = PatroniTopology {
        node_name: node_name.clone(),
        host_ip: node_ip.clone(),
        scope,
        etcd_hosts,
        is_bootstrap,
        superuser_username: "postgres".to_string(),
        replication_username: "replicator".to_string(),
        superuser_password: superuser_password.clone(),
        replication_password: replication_password.clone(),
    };
    let env_vars = build_patroni_env_vars(&topo);

    let compose_project_name = pg_compose_name(index);
    if !validate_compose_project_name(&compose_project_name) {
        return Err(VcmError::new(
            codes::PARAM_INVALID,
            format!("compose_project_name 非法: {compose_project_name}"),
        ));
    }

    // 部署成功后回写 etcd credentials（失败仅告警——凭据已随 env_vars 注入实例，
    // etcd 回写用于 failover 重建时复用；集群未录入该节点时跳过）
    match state
        .etcd_cluster_client
        .update_node_pg_credentials(&node_ip, superuser_password, replication_password)
        .await
    {
        Ok(true) => tracing::info!(node_ip = %node_ip, "postgres-ha 凭据已写入 etcd NodeCredentials"),
        Ok(false) => tracing::warn!(
            node_ip = %node_ip,
            "未找到节点记录，pg 凭据未回写 etcd（仅存在于实例 env_vars）"
        ),
        Err(e) => tracing::warn!(err = %e, "pg 凭据回写 etcd 失败（不阻断部署）"),
    }

    Ok(ProjectDeployBo {
        app_id: APP_ID.to_string(),
        project_name: "postgres-ha".to_string(),
        compose_project_name,
        env_vars,
        health_timeout_secs: Some(300),
    })
}

/// POST /api/infra/pg/deploy_async —— 异步部署（返回平台 task_id）
pub async fn pg_deploy_async(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    ensure_etcd_dcs_ready(&state).await?;
    let instances = state.projects_client.instances().await?;
    check_no_active_pg(&instances)?;
    let bo = build_deploy_bo(&state, &body).await?;
    let task_id = state.projects_client.deploy_async(&bo).await?;
    Ok(Json(ResultBody::ok(
        json!({"task_id": task_id, "compose_project_name": bo.compose_project_name}),
        version,
    )))
}

/// POST /api/infra/pg/deploy —— 同步部署（轮询平台 workflow 至终态）
pub async fn pg_deploy(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    ensure_etcd_dcs_ready(&state).await?;
    let instances = state.projects_client.instances().await?;
    check_no_active_pg(&instances)?;
    let bo = build_deploy_bo(&state, &body).await?;
    let instance = state.projects_client.deploy_sync(&bo).await?;
    Ok(Json(ResultBody::ok(json!({"instance": instance}), version)))
}

/// 提取 compose_project_name 参数
fn extract_name(body: &Value) -> Result<String, VcmError> {
    body.get("compose_project_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "compose_project_name 必填"))
}

/// POST /api/infra/pg/remove_async —— 异步移除（返回平台 task_id）
pub async fn pg_remove_async(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let compose_project_name = extract_name(&body)?;
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

/// POST /api/infra/pg/remove —— 同步移除
pub async fn pg_remove(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let compose_project_name = extract_name(&body)?;
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
    Ok(Json(ResultBody::ok(
        json!({"removed": true, "compose_project_name": compose_project_name}),
        version,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_index_logic() {
        let mk = |name: &str| crate::projects::ProjectInstanceVo {
            id: "x".into(),
            app_id: APP_ID.into(),
            project_name: "postgres-ha".into(),
            compose_project_name: name.into(),
            state: "deployed".into(),
            deployed_at: None,
            removed_at: None,
            create_time: "t".into(),
            updated_at: "t".into(),
        };
        assert_eq!(next_pg_index(&[]), 0);
        assert_eq!(
            next_pg_index(&[mk("cluster-pg-node-0"), mk("cluster-pg-node-2")]),
            3
        );
        assert_eq!(next_pg_index(&[mk("cluster-pg-node-0")]), 1);
    }

    /// D-CR-03：bootstrap 写入集群 pg 凭据 → join 复用同一密码（replicator 一致性）
    #[tokio::test]
    async fn bootstrap_credentials_shared_with_join() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let etcd = std::sync::Arc::new(crate::etcd::MockEtcdClusterClient::new());
        let state = crate::state::AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            etcd.clone() as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            None,
        );

        // bootstrap（21）：生成密码并写入 etcd
        let body_boot = serde_json::json!({"node_ip": "10.100.10.21", "bootstrap": true});
        let bo_boot = build_deploy_bo(&state, &body_boot).await.unwrap();
        let su_boot = bo_boot
            .env_vars
            .iter()
            .find(|(k, _)| k.as_str() == "PATRONI_SUPERUSER_PASSWORD")
            .map(|(_, v)| v.clone())
            .unwrap();

        // join（22）：从 etcd 读到同一密码
        let body_join = serde_json::json!({"node_ip": "10.100.10.22", "bootstrap": false, "etcd_hosts": "10.100.10.21:2379"});
        let bo_join = build_deploy_bo(&state, &body_join).await.unwrap();
        let su_join = bo_join
            .env_vars
            .iter()
            .find(|(k, _)| k.as_str() == "PATRONI_SUPERUSER_PASSWORD")
            .map(|(_, v)| v.clone())
            .unwrap();

        assert_eq!(su_boot, su_join, "bootstrap 与 join 必须复用同一 pg 凭据");
        // join 的 instance 名（Mock projects 库为空 → index 从 0 推导）
        assert_eq!(bo_join.compose_project_name, "cluster-pg-node-0");
    }
}