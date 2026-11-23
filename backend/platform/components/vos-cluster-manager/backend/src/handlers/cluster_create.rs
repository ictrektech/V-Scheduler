//! 集群创建编排 API（/api/cluster/create · /api/cluster/create/status，Phase 4）
//!
//! `POST /api/cluster/create`：校验 + 全节点探活 → 启动 ClusterCreateWorkflow（平台
//! temporal，workflow_id=`cluster-create-<cluster_id>`，冲突 TerminateExisting 兜底）→
//! 返回 task_id（即 workflow_id）与 cluster_id。
//! `POST /api/cluster/create/status`：describe workflow → Running/Completed/Failed + stage。

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::codes;
use crate::error::{ResultBody, VcmError};
use crate::orchestration::cluster::{CreateClusterInput, NodeInput};
use crate::state::AppState;

/// POST /api/cluster/create —— 多节点一键创建（异步编排）
pub async fn cluster_create(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let input = parse_input(&body)?;
    let name = input.cluster_name.clone();

    // 并发防重：同一 cluster_name 创建中 → 拒绝（进程内 in-flight 集合；
    // 双发 create 会启动两个 workflow 竞争部署等基础设施并互相 saga 拆除；
    // 守卫仅在同步块内存活，避免 std::sync::MutexGuard 跨 await 非 Send）
    let acquired = {
        let mut inflight = state.create_inflight.lock().unwrap();
        if inflight.contains(&name) {
            false
        } else {
            inflight.push(name.clone());
            true
        }
    };
    if !acquired {
        return Err(VcmError::new(
            codes::EXISTS,
            format!("集群 {name} 创建中，请勿重复发起（create/status 跟踪进度）"),
        ));
    }

    let result = create_inner(&state, input).await;

    {
        let mut inflight = state.create_inflight.lock().unwrap();
        inflight.retain(|n| *n != name);
    }
    result
}

/// POST /api/cluster/create 实际执行（start workflow；供 create 外层并发防重包裹）
async fn create_inner(
    state: &AppState,
    input: CreateClusterInput,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;

    // 前置：集群信息已存在 → 拒绝重复创建（幂等防护）。
    // etcd 未就绪（编排自举前）→ 视为无集群，继续（workflow 内部先部署 etcd）
    let existing = match state.etcd_cluster_client.get_cluster_info().await {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(err = %e.msg, "etcd 未就绪，按无集群处理（创建编排将自举 etcd）");
            None
        }
    };
    if existing.is_some() {
        return Err(VcmError::new(
            codes::EXISTS,
            "集群已创建（etcd /vos/cluster/info 存在）；如需重建请先清理集群状态",
        ));
    }

    // 前置：逐节点探活（目标节点须安装并运行本应用同版本）
    let mut unreachable = Vec::new();
    for n in &input.nodes {
        if state.remote_client.probe(&n.node_ip).await.is_err() {
            unreachable.push(n.node_ip.clone());
        }
    }
    if !unreachable.is_empty() {
        return Err(VcmError::new(
            codes::REMOTE_NODE_FAILED,
            format!("以下节点不可达（未安装/未运行本应用或端口不通），创建中止: {}", unreachable.join(", ")),
        ));
    }

    // 生成 cluster_id 与输入
    let cluster_id = uuid::Uuid::new_v4().to_string();
    let wf_input = crate::temporal::cluster_create::CreateClusterWorkflowInput {
        cluster_id: cluster_id.clone(),
        cluster_name: input.cluster_name,
        nodes: input.nodes,
    };

    // 连接平台 temporal + 启动编排 workflow
    let task_id = start_create_workflow(state, &cluster_id, wf_input).await?;

    Ok(Json(ResultBody::ok(
        json!({"cluster_id": cluster_id, "task_id": task_id}),
        version,
    )))
}

/// POST /api/cluster/create/status —— 编排状态查询
pub async fn cluster_create_status(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let task_id = body
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "task_id 必填"))?;

    let address = state
        .temporal_address
        .clone()
        .ok_or_else(|| VcmError::new(codes::ORCHESTRATION_FAILED, "temporal 未配置，无法查询编排状态"))?;
    let client = crate::temporal::connect(&address)
        .await
        .map_err(|e| VcmError::new(codes::ORCHESTRATION_FAILED, e))?;

    let handle = client.get_workflow_handle::<crate::temporal::cluster_create::ClusterCreateWorkflow>(&task_id);
    let desc = handle
        .describe(
            temporalio_client::WorkflowDescribeOptions::default(),
        )
        .await
        .map_err(|e| {
            VcmError::new(
                codes::ORCHESTRATION_FAILED,
                format!("编排 workflow 查询失败（{task_id}）: {e}"),
            )
        })?;
    let status_str = match desc.status() {
        temporalio_common::protos::temporal::api::enums::v1::WorkflowExecutionStatus::Running => "Running",
        temporalio_common::protos::temporal::api::enums::v1::WorkflowExecutionStatus::Completed => "Completed",
        temporalio_common::protos::temporal::api::enums::v1::WorkflowExecutionStatus::Failed => "Failed",
        temporalio_common::protos::temporal::api::enums::v1::WorkflowExecutionStatus::Canceled => "Cancelled",
        other => other.as_str_name(),
    };

    Ok(Json(ResultBody::ok(
        json!({"task_id": task_id, "status": status_str}),
        version,
    )))
}

/// 解析并校验 create 入参（20501；远端探活/集群已存在校验在 handler 内）
fn parse_input(body: &Value) -> Result<CreateClusterInput, VcmError> {
    let cluster_name = body
        .get("cluster_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "cluster_name 必填"))?;

    let mut nodes = Vec::new();
    if let Some(arr) = body.get("nodes").and_then(|v| v.as_array()) {
        for m in arr {
            let node_ip = m
                .get("node_ip")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
                .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "nodes[].node_ip 必填且为合法 IP"))?;
            let display_name = m
                .get("display_name")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| node_ip.clone());
            let admin_password = m
                .get("admin_password")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| s.len() >= 6)
                .ok_or_else(|| {
                    VcmError::new(codes::PARAM_INVALID, "nodes[].admin_password 必填且长度 ≥6")
                })?;
            let role = m
                .get("role")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| s == "manager" || s == "worker")
                .unwrap_or_else(|| "worker".to_string());
            nodes.push(NodeInput {
                node_ip,
                display_name,
                admin_password,
                role,
            });
        }
    }
    if nodes.is_empty() {
        return Err(VcmError::new(codes::PARAM_INVALID, "nodes 至少 1 个节点"));
    }
    let input = CreateClusterInput { cluster_name, nodes };
    // 统一兜底：role 枚举 / ≥1 管理节点 / 首节点须为管理节点（单点语义，防两处漂移）
    crate::orchestration::cluster::validate_create_input(&input)
        .map_err(|msg| VcmError::new(codes::PARAM_INVALID, msg))?;
    Ok(input)
}

/// 启动编排 workflow（temporal connect + start；连接失败 → 20511）
async fn start_create_workflow(
    state: &AppState,
    cluster_id: &str,
    input: crate::temporal::cluster_create::CreateClusterWorkflowInput,
) -> Result<String, VcmError> {
    let address = state
        .temporal_address
        .clone()
        .ok_or_else(|| VcmError::new(codes::ORCHESTRATION_FAILED, "temporal 未配置，无法启动编排"))?;
    let client = crate::temporal::connect(&address)
        .await
        .map_err(|e| VcmError::new(codes::ORCHESTRATION_FAILED, e))?;

    let workflow_id = format!("cluster-create-{cluster_id}");
    let handle = client
        .start_workflow(
            crate::temporal::cluster_create::ClusterCreateWorkflow,
            input,
            temporalio_client::WorkflowStartOptions::new(
                crate::temporal::TASK_QUEUE.to_string(),
                workflow_id.clone(),
            )
            .id_conflict_policy(
                temporalio_common::protos::temporal::api::enums::v1::WorkflowIdConflictPolicy::TerminateExisting,
            )
            .build(),
        )
        .await
        .map_err(|e| {
            VcmError::new(
                codes::ORCHESTRATION_FAILED,
                format!("编排 workflow 启动失败: {e}"),
            )
        })?;
    let _ = handle;
    tracing::info!(cluster_id, workflow_id, "集群创建编排已启动");
    Ok(workflow_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use crate::etcd::EtcdClusterClient;
    use crate::etcd::MockEtcdClusterClient;
    use tower::util::ServiceExt;

    fn code_of(v: &Value) -> i32 {
        v["code"].as_i64().unwrap_or(-1) as i32
    }

    async fn post_json(app: &Router, uri: &str, body: Value) -> Value {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn valid_body() -> Value {
        json!({
            "cluster_name": "t",
            "nodes": [{"node_ip": "10.0.0.1", "admin_password": "pw123456", "display_name": "a", "role": "manager"}]
        })
    }

    /// 首节点为 worker（发起节点非法）→ 20501
    fn first_worker_body() -> Value {
        json!({
            "cluster_name": "t",
            "nodes": [
                {"node_ip": "10.0.0.1", "admin_password": "pw123456", "display_name": "a", "role": "worker"},
                {"node_ip": "10.0.0.2", "admin_password": "pw123456", "display_name": "b", "role": "manager"},
            ]
        })
    }

    #[tokio::test]
    async fn create_invalid_inputs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let state = AppState::new(tmp.path().join("data"), frontend, "/vcm".into(), vec![]);
        let app = crate::router::build_router(state);

        for bad in [
            json!({"nodes": [{"node_ip": "10.0.0.1", "admin_password": "pw123456"}]}), // 缺 cluster_name
            json!({"cluster_name": "t", "nodes": []}),                                 // 空 nodes
            json!({"cluster_name": "t", "nodes": [{"node_ip": "bad", "admin_password": "pw123456"}]}), // 非法 IP
            json!({"cluster_name": "t", "nodes": [{"node_ip": "10.0.0.1", "admin_password": "123"}]}), // 密码过短
            first_worker_body(),                                                                       // 首节点非 manager
            json!({"cluster_name": "t", "nodes": [
                {"node_ip": "10.0.0.1", "admin_password": "pw123456", "role": "worker"},
                {"node_ip": "10.0.0.2", "admin_password": "pw123456", "role": "worker"},
            ]}),                                                                                       // 全 worker 无 manager
            json!({"cluster_name": "t", "nodes": [{"node_ip": "10.0.0.1", "admin_password": "pw123456", "role": "master"}]}), // 旧值 role 拒绝
        ] {
            let v = post_json(&app, "/vcm/api/cluster/create", bad).await;
            assert_eq!(code_of(&v), 20501, "{v}");
        }
    }

    #[tokio::test]
    async fn create_probe_fail_rejects() {
        // cluster info 不存在（空 etcd mock），remote mock 未预置节点 → 探活失败
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();

        let remote = std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new());
        // 不 preload → probe("10.0.0.1") 失败
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            std::sync::Arc::new(MockEtcdClusterClient::new())
                as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            Some("127.0.0.1:7233".into()),
            remote as std::sync::Arc<dyn crate::remote::RemoteNodeClient>,
            crate::remote::default_remote_app_port(),
            None,
        );
        let app = crate::router::build_router(state);
        let v = post_json(&app, "/vcm/api/cluster/create", valid_body()).await;
        assert_eq!(code_of(&v), 20509, "探活失败应 20509: {v}");
    }

    #[tokio::test]
    async fn create_etcd_unavailable_tolerated_as_no_cluster() {
        // etcd 不可用（编排自举前）：create 不应被 20504 阻断——视为无集群继续，
        // 走到 temporal 启动（未配置 → ORCHESTRATION_FAILED 20509，证明越过 etcd 前置）
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();

        let etcd = std::sync::Arc::new(MockEtcdClusterClient::new());
        etcd.set_unavailable(true);
        let remote = std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new());
        remote.preload(&["10.0.0.1"]);
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            etcd as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            None, // temporal 未配置 → 编排启动失败（20509 而非 20504）
            remote as std::sync::Arc<dyn crate::remote::RemoteNodeClient>,
            crate::remote::default_remote_app_port(),
            None,
        );
        let app = crate::router::build_router(state);
        let v = post_json(&app, "/vcm/api/cluster/create", valid_body()).await;
        assert_eq!(code_of(&v), 20511, "etcd 不可用应容忍并走到 temporal 启动失败: {v}");
    }

    #[tokio::test]
    async fn create_cluster_already_exists_rejects() {
        // etcd mock 预置 cluster info → EXISTS
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();

        let etcd = std::sync::Arc::new(MockEtcdClusterClient::new());
        let remote = std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new());
        remote.preload(&["10.0.0.1"]);
        let info = crate::etcd::dto::ClusterInfo {
            cluster_id: "c1".into(),
            cluster_name: "已有".into(),
            status: crate::etcd::dto::status::CREATED.into(),
            created_at: "t".into(),
        };
        let _ = etcd.put_cluster_info(&info).await;
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            etcd as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            Some("127.0.0.1:7233".into()),
            remote as std::sync::Arc<dyn crate::remote::RemoteNodeClient>,
            crate::remote::default_remote_app_port(),
            None,
        );
        let app = crate::router::build_router(state);
        let v = post_json(&app, "/vcm/api/cluster/create", valid_body()).await;
        assert_eq!(code_of(&v), 20506, "集群已存在应 20506: {v}");
    }

    /// 并发防重：同一 cluster_name 创建中 → 第二个请求 20506（进程内 in-flight）
    #[tokio::test]
    async fn create_duplicate_inflight_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            std::sync::Arc::new(MockEtcdClusterClient::new())
                as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            None,
        );

        // 直接操纵 in-flight 集合模拟「同名单次 create 已在进行」
        let name = "重复集群".to_string();
        state.create_inflight.lock().unwrap().push(name.clone());

        let app = crate::router::build_router(state);
        let v = post_json(
            &app,
            "/vcm/api/cluster/create",
            json!({"cluster_name": name, "nodes": [{"node_ip": "10.0.0.1", "admin_password": "pw123456", "role": "manager"}]}),
        )
        .await;
        assert_eq!(code_of(&v), 20506, "并发重复 create 应 20506: {v}");
    }
}