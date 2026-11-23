//! 集群创建编排 Workflow + Activity 集（Phase 4，D-CR-03 / D-CR-04）
//!
//! Worker 运行于编排发起方（master）应用容器；ClusterActivities 持有 AppState 依赖
//! （本机 ProjectsClient / 跨节点 RemoteNodeClient / etcd EtcdClusterClient member API）。
//! Workflow 编排 5 stage：本机 etcd new → 逐成员 learner 加入 + 目标节点 etcd existing
//! （部署后 promote 为投票成员）→ 本机 pg bootstrap → 目标节点 pg join → finalize
//! （写集群信息 + infra_status）；任一 stage 失败 → 反向 saga 补偿（best-effort remove
//! 已部署子项目），workflow Failed。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::{ActivityOptions, WorkflowContext, WorkflowResult, WorkflowTermination};

use crate::error::codes;
use crate::error::VcmError;
use crate::etcd::dto::{NodeCredentials, NodeMetadata, NodeRecord};
use crate::etcd::EtcdClusterClient;
use crate::orchestration::cluster::NodeInput;
use crate::state::AppState;

/// 编排 Activity 超时（同步部署最长 health 窗口 300s + 裕量）
const ACTIVITY_TIMEOUT_SECS: u64 = 600;

// =========================================================================
// Workflow 输入 / Activity 请求-响应契约
// =========================================================================

/// 集群创建编排输入（handler 构造；nodes 顺序 = raft 成员顺序，node[0] 为发起方）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateClusterWorkflowInput {
    pub cluster_id: String,
    pub cluster_name: String,
    pub nodes: Vec<NodeInput>,
}

/// etcd 节点部署请求（new / existing；非 bootstrap 节点先 member_add）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcdDeployNodeReq {
    pub node_ip: String,
    pub node_name: String, // etcd0 / etcd1 ...
    pub is_bootstrap: bool,
    /// 初始成员 `(name, node_ip)`（全部节点；handler 内归一化为 peer URL）
    pub initial_members: Vec<(String, String)>,
    /// 目标节点应用地址（None=本机 ProjectsClient；Some=RemoteNodeClient）
    pub remote: Option<String>,
}

/// postgres-ha 节点部署请求（bootstrap / join）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PgDeployNodeReq {
    pub node_ip: String,
    pub node_name: String, // pg-node-0 ...
    pub is_bootstrap: bool,
    pub etcd_dcs_hosts: Vec<String>,
    pub remote: Option<String>,
}

/// haproxy 节点部署请求（二期 S1/S2；全节点部署，backends 由 workflow 显式注入
/// ——worker 无本地 etcd，走 D-HA-08 拓扑注入路径）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaproxyDeployNodeReq {
    pub node_ip: String,
    /// postgres HA 后端（manager IP 清单，注入目标节点 haproxy env）
    pub pg_backends: Vec<String>,
    /// 目标节点应用地址（None=本机；Some=RemoteNodeClient at 转发）
    pub remote: Option<String>,
}

/// 集群 temporal 节点部署请求（二期 S2；仅首 manager，DB 经本机 haproxy 15432）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalDeployNodeReq {
    pub node_ip: String,
    pub remote: Option<String>,
}

/// saga 补偿步骤（反向序由 workflow 构造）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SagaCleanupReq {
    pub steps: Vec<SagaStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SagaStep {
    /// etcd | pg | haproxy | temporal
    pub kind: String,
    pub node_ip: String,
    pub remote: Option<String>,
    pub compose_project_name: String,
}

/// finalize：写 etcd 集群信息 + 基础设施就绪位 + 节点记录（含凭据持久化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizeClusterReq {
    pub cluster_id: String,
    pub cluster_name: String,
    /// 由创建入参传入的节点拓扑（IP + 管理员密码；须持久化到 etcd credentials）
    pub nodes: Vec<NodeInput>,
}

/// 由创建入参构建节点记录（含凭据——IP + 管理员密码持久化到 etcd，内网可信；
/// display_name 缺省回退 node_ip；api_endpoint 沿用 `.default_api_endpoint` 推导）
pub(crate) fn build_node_record(n: &NodeInput) -> NodeRecord {
    let now = chrono::Utc::now().to_rfc3339();
    NodeRecord {
        metadata: NodeMetadata {
            member_id: uuid::Uuid::new_v4().to_string(),
            display_name: if n.display_name.is_empty() {
                n.node_ip.clone()
            } else {
                n.display_name.clone()
            },
            role: n.role.clone(),
            node_ip: n.node_ip.clone(),
            api_endpoint: crate::handlers::cluster::default_api_endpoint(&n.node_ip),
            joined_at: now.clone(),
        },
        credentials: NodeCredentials {
            admin_password: n.admin_password.clone(),
            updated_at: now,
            pg_superuser_password: None,
            pg_replication_password: None,
        },
    }
}

/// 编排最终输出（Completed 时）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterCreateWorkflowOutput {
    pub status: String, // created
    pub cluster_id: String,
}

// =========================================================================
// Activity 集（worker 注册时注入 AppState 实例；workflow 内引用实现器路径）
// =========================================================================

#[derive(Clone)]
pub struct ClusterActivities {
    pub state: AppState,
}

fn act_err(ctx: &str, e: VcmError) -> ActivityError {
    ActivityError::application(temporalio_common::error::ApplicationFailure::non_retryable(
        anyhow::anyhow!("{ctx}: {}", e.msg),
    ))
}

/// 等待 etcd DCS **全量**就绪（pg 部署前置；≤60s）。
///
/// 成员以 learner 顺序加入并在 `etcd_deploy_node` 末尾 promote，故到达本等待时
/// 预期全部成员均为投票成员；等待规则 = ping 可达 + member_count ≥ 预期成员数
/// （= DCS 端点数 = 全量 manager）。纯异步函数便于单测（注入 Mock）。
async fn wait_etcd_dcs_ready(client: &dyn EtcdClusterClient, expected_members: usize) -> bool {
    wait_etcd_dcs_ready_with(
        client,
        expected_members,
        20,
        std::time::Duration::from_secs(3),
    )
    .await
}

/// `wait_etcd_dcs_ready` 的可配参数版本（attempts/interval 供单测加速）
async fn wait_etcd_dcs_ready_with(
    client: &dyn EtcdClusterClient,
    expected_members: usize,
    attempts: u32,
    interval: std::time::Duration,
) -> bool {
    if expected_members == 0 {
        return true;
    }
    for _ in 0..attempts {
        if client.ping().await
            && client
                .member_count()
                .await
                .map(|n| n >= expected_members as u64)
                .unwrap_or(false)
        {
            return true;
        }
        tokio::time::sleep(interval).await;
    }
    false
}

#[activities]
impl ClusterActivities {
    /// 部署 etcd 节点：非 bootstrap 先以 **learner** 身份加入 raft（不投票、不改
    /// quorum），再本机（ProjectsClient）或目标节点（RemoteNodeClient）同步部署至
    /// 终态，最后把 learner 提升为投票成员；返回实际 compose_project_name（供 saga 追踪）。
    #[activity]
    pub async fn etcd_deploy_node(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: EtcdDeployNodeReq,
    ) -> Result<String, ActivityError> {
        // 1) raft 成员加入（非 bootstrap）：**learner**（raft 非投票成员）。
        //    旧实现直接 member_add 投票成员：目标节点 etcd 尚未启动时 quorum 立即
        //    抬高（单节点 1→2、偶数成员同理），整个集群永久失 quorum 且
        //    member_remove 也无法提交（2026-10-30 实机事故）。learner 不参与投票，
        //    因此可以在目标节点部署前安全计入成员表。
        let mut learner_member_id: Option<u64> = None;
        if !req.is_bootstrap {
            // 等待编排基础设施 etcd 就绪（首节点 bootstrap 刚部署，Lazy client
            // 可能尚连不上；最多等 30s，避免 tonic 在 etcd 未接受端口前挂起）
            let mut ready = false;
            for _ in 0..10 {
                if self.state.etcd_cluster_client.ping().await {
                    ready = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
            }
            if !ready {
                return Err(act_err(
                    "等待 etcd 就绪超时（编排基础设施等 DCS 不可达）",
                    VcmError::new(codes::ETCD_FAILED, "etcd 就绪等待超时（30s）"),
                ));
            }
            let peer_url = crate::orchestration::etcd::peer_url(&req.node_ip);
            let id = match self
                .state
                .etcd_cluster_client
                .find_member_id_by_peer_url(&peer_url)
                .await
            {
                Ok(Some(id)) => {
                    tracing::info!(
                        node_ip = %req.node_ip,
                        member_id = id,
                        "etcd 成员已存在（activity 重试幂等），跳过 learner 加入"
                    );
                    id
                }
                _ => {
                    let (id, _) = self
                        .state
                        .etcd_cluster_client
                        .member_add_as_learner(&peer_url)
                        .await
                        .map_err(|e| {
                            tracing::error!(
                                node_ip = %req.node_ip,
                                peer_url = %peer_url,
                                err = %e.msg,
                                "etcd member_add_as_learner 失败（编排）"
                            );
                            act_err(&format!("etcd learner 加入 {}", req.node_ip), e)
                        })?;
                    tracing::info!(
                        node_ip = %req.node_ip,
                        member_id = id,
                        "etcd learner 成员已加入（不参与投票，quorum 安全）"
                    );
                    id
                }
            };
            learner_member_id = Some(id);
        }

        let body = serde_json::json!({
            "node_ip": req.node_ip,
            "node_name": req.node_name,
            "initial_state": if req.is_bootstrap { "new" } else { "existing" },
            "initial_members": req.initial_members.iter().map(|(n, ip)| {
                serde_json::json!({"name": n, "node_ip": ip})
            }).collect::<Vec<_>>(),
        });

        // 2) 部署（本机 handler builder + deploy_sync / 目标节点同步 deploy）
        let name = if let Some(remote_ip) = &req.remote {
            let out = self
                .state
                .remote_client
                .deploy_etcd_sync(remote_ip, body)
                .await
                .map_err(|e| act_err(&format!("远程 etcd deploy {remote_ip}"), e))?;
            let name = out["instance"]["compose_project_name"]
                .as_str()
                .ok_or_else(|| {
                    ActivityError::application(
                        temporalio_common::error::ApplicationFailure::new(anyhow::anyhow!(
                            "远程 etcd deploy {remote_ip} 响应缺 instance.compose_project_name: {out}"
                        )),
                    )
                })?
                .to_string();
            tracing::info!(node_ip = %req.node_ip, compose = %name, "远程 etcd 部署完成");
            name
        } else {
            // 本机：复用 handlers::infra_etcd 的 Bo 构造（防逻辑漂移）
            let bo = crate::handlers::infra_etcd::build_deploy_bo(&self.state, &body)
                .await
                .map_err(|e| act_err("本地 etcd deploy Bo 构造", e))?;
            let instance = self
                .state
                .projects_client
                .deploy_sync(&bo)
                .await
                .map_err(|e| act_err("本地 etcd deploy_sync", e))?;
            tracing::info!(compose = %instance.compose_project_name, "本地 etcd 部署完成");
            instance.compose_project_name
        };

        // 3) 目标 etcd 已部署并就绪 → learner 提升为投票成员。
        //    必须在部署成功后 promote（etcd 要求 learner 已追平 leader）；有界重试，
        //    保证 pg stage 开始时全部成员都是投票成员（wait_etcd_dcs_ready 的前提）。
        if let Some(id) = learner_member_id {
            crate::etcd::promote_learner_with_retry(
                &*self.state.etcd_cluster_client,
                id,
                6,
                std::time::Duration::from_secs(5),
            )
            .await
            .map_err(|e| act_err(&format!("etcd learner promote {}", req.node_ip), e))?;
            tracing::info!(
                node_ip = %req.node_ip,
                member_id = id,
                "etcd learner 已提升为投票成员（编排）"
            );
        }

        Ok(name)
    }

    /// 部署 postgres-ha 节点（bootstrap / join；DCS 端点列表经 req 注入）
    #[activity]
    pub async fn pg_deploy_node(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: PgDeployNodeReq,
    ) -> Result<String, ActivityError> {
        // 前置：等待 etcd DCS **全量**就绪（偶数成员 raft quorum 缺口：member_add
        // 后新成员完成 join 前发起方 etcd 无 quorum，任何读写阻塞；等待 = ping 可达
        // + member_count ≥ DCS 端点数 = 全量 manager，见 wait_etcd_dcs_ready 注释）。
        let expected_members = req.etcd_dcs_hosts.len();
        if !wait_etcd_dcs_ready(&*self.state.etcd_cluster_client, expected_members).await {
            return Err(act_err(
                "等待 etcd DCS 全量就绪超时（pg 部署前置）",
                VcmError::new(
                    codes::ETCD_FAILED,
                    format!("等待 etcd DCS 就绪超时（预期 {expected_members} 成员）"),
                ),
            ));
        }

        let body = serde_json::json!({
            "node_ip": req.node_ip,
            "node_name": req.node_name,
            "bootstrap": req.is_bootstrap,
            "etcd_hosts": req.etcd_dcs_hosts.join(","),
        });

        if let Some(remote_ip) = &req.remote {
            let out = self
                .state
                .remote_client
                .deploy_pg_sync(remote_ip, body)
                .await
                .map_err(|e| act_err(&format!("远程 pg deploy {remote_ip}"), e))?;
            let name = out["instance"]["compose_project_name"]
                .as_str()
                .ok_or_else(|| {
                    ActivityError::application(
                        temporalio_common::error::ApplicationFailure::new(anyhow::anyhow!(
                            "远程 pg deploy {remote_ip} 响应缺 instance.compose_project_name: {out}"
                        )),
                    )
                })?
                .to_string();
            tracing::info!(node_ip = %req.node_ip, compose = %name, "远程 pg 部署完成");
            Ok(name)
        } else {
            let bo = crate::handlers::infra_pg::build_deploy_bo(&self.state, &body)
                .await
                .map_err(|e| act_err("本地 pg deploy Bo 构造", e))?;
            let instance = self
                .state
                .projects_client
                .deploy_sync(&bo)
                .await
                .map_err(|e| act_err("本地 pg deploy_sync", e))?;
            tracing::info!(compose = %instance.compose_project_name, "本地 pg 部署完成");
            Ok(instance.compose_project_name)
        }
    }

    /// haproxy 节点部署（二期 S1/S2，全节点含 worker）：backends 显式注入
    /// （D-HA-08 拓扑注入——worker 无 etcd 不自推导）；本机走 infra_haproxy 的
    /// build_deploy_bo（防漂移），远程 at 转发体直接带 pg_backends。返回 compose 名。
    #[activity]
    pub async fn haproxy_deploy_node(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: HaproxyDeployNodeReq,
    ) -> Result<String, ActivityError> {
        let joined = req.pg_backends.join(",");
        if let Some(remote_ip) = req.remote.clone() {
            let body = serde_json::json!({
                "node_ip": req.node_ip,
                "pg_backends": joined,
            });
            let out = self
                .state
                .remote_client
                .deploy_haproxy_sync(&remote_ip, body)
                .await
                .map_err(|e| act_err("远程 haproxy deploy", e))?;
            let name = out
                .get("instance")
                .and_then(|i| i.get("compose_project_name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ActivityError::application(
                        temporalio_common::error::ApplicationFailure::new(anyhow::anyhow!(
                            "远程 haproxy deploy {remote_ip} 响应缺 instance.compose_project_name: {out}"
                        )),
                    )
                })?
                .to_string();
            tracing::info!(node_ip = %req.node_ip, compose = %name, "远程 haproxy 部署完成");
            Ok(name)
        } else {
            let body = serde_json::json!({
                "node_ip": req.node_ip,
                "pg_backends": joined,
            });
            let bo = crate::handlers::infra_haproxy::build_deploy_bo(&self.state, &body)
                .await
                .map_err(|e| act_err("本地 haproxy deploy Bo 构造", e))?;
            let instance = self
                .state
                .projects_client
                .deploy_sync(&bo)
                .await
                .map_err(|e| act_err("本地 haproxy deploy_sync", e))?;
            tracing::info!(compose = %instance.compose_project_name, "本地 haproxy 部署完成");
            Ok(instance.compose_project_name)
        }
    }

    /// 集群 temporal 部署（二期 S2，仅首 manager 单实例 D-TE-02）。
    /// 本机路径：等 haproxy 15432 bind 窗口（≤30s）→ infra_temporal::build_deploy_bo
    /// （自读本机 etcd 凭据，密码不出本机）→ projects deploy_sync；
    /// 远程路径（备用）：at 转发（目标 manager 同走自举凭据）。
    #[activity]
    pub async fn temporal_deploy_node(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: TemporalDeployNodeReq,
    ) -> Result<String, ActivityError> {
        if let Some(remote_ip) = req.remote.clone() {
            let out = self
                .state
                .remote_client
                .deploy_temporal_sync(&remote_ip, serde_json::json!({"node_ip": req.node_ip}))
                .await
                .map_err(|e| act_err("远程 temporal deploy", e))?;
            let name = out
                .get("instance")
                .and_then(|i| i.get("compose_project_name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ActivityError::application(temporalio_common::error::ApplicationFailure::new(
                        anyhow::anyhow!("远程 temporal deploy 响应缺 instance 名: {out}"),
                    ))
                })?
                .to_string();
            return Ok(name);
        }
        // 本机路径：吸收 haproxy 绑定窗口
        let mut ready = false;
        for _ in 0..15 {
            if tokio::net::TcpStream::connect(("127.0.0.1", crate::orchestration::PG_FRONT_PORT))
                .await
                .is_ok()
            {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        if !ready {
            return Err(act_err(
                "temporal 前置",
                VcmError::new(
                    crate::error::codes::STATE_NOT_SUPPORTED,
                    "本机 haproxy 15432 30s 内不可达",
                ),
            ));
        }
        let bo = crate::handlers::infra_temporal::build_deploy_bo(
            &self.state,
            &serde_json::json!({"node_ip": req.node_ip}),
        )
        .await
        .map_err(|e| act_err("本地 temporal deploy Bo 构造", e))?;
        let instance = self
            .state
            .projects_client
            .deploy_sync(&bo)
            .await
            .map_err(|e| act_err("本地 temporal deploy_sync", e))?;
        tracing::info!(compose = %instance.compose_project_name, "集群 temporal 部署完成");
        Ok(instance.compose_project_name)
    }

    /// 集群 haproxy 全节点刷新（二期 S2 D-TE-04：temporal 部署后激活 7133 监听）。
    /// 复用 handler 的 refresh_cluster_haproxy（workflow 本机直连 temporal 部署时
    /// 不经过 deploy handler，须显式补偿联动）。best-effort：失败仅记 warn，
    /// 返回失败节点数供观测（refresh_all 端点兜底）。
    #[activity]
    pub async fn haproxy_refresh_cluster(
        self: Arc<Self>,
        _ctx: ActivityContext,
        _req: (),
    ) -> Result<u32, ActivityError> {
        let results = crate::handlers::infra_temporal::refresh_cluster_haproxy(&self.state).await;
        let failed = results.iter().filter(|r| r.get("ok").and_then(|v| v.as_bool()) != Some(true)).count();
        if failed > 0 {
            tracing::warn!(failed = failed, total = results.len(), "temporal 后置 haproxy 刷新部分失败（refresh_all 可兜底）");
        } else {
            tracing::info!(total = results.len(), "temporal 后置 haproxy 刷新完成（7133 激活）");
        }
        Ok(failed as u32)
    }

    /// finalize：写 etcd 集群信息 + infra_status（即集群创建完成的权威落点）
    #[activity]
    pub async fn finalize_cluster(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: FinalizeClusterReq,
    ) -> Result<(), ActivityError> {
        use crate::etcd::dto::{ClusterInfo, InfraStatus, status};
        let info = ClusterInfo {
            cluster_id: req.cluster_id.clone(),
            cluster_name: req.cluster_name.clone(),
            status: status::CREATED.into(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.state
            .etcd_cluster_client
            .put_cluster_info(&info)
            .await
            .map_err(|e| act_err("写 ClusterInfo", e))?;
        let st = InfraStatus {
            etcd_ok: true,
            postgres_ha_ok: true,
            // 二期 S1/S2：haproxy（全节点）与 temporal（首 manager 单实例）stage
            // 均先于 finalize 成功——就绪位置 true
            haproxy_ok: true,
            temporal_ok: true,
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        self.state
            .etcd_cluster_client
            .put_infra_status(&st)
            .await
            .map_err(|e| act_err("写 infra_status", e))?;

        // ---- 节点记录持久化（创建入参的 IP + 管理员密码落到 etcd credentials）----
        // 幂等：node_ip 已存在则跳过（重试续跑场景不重复写）。
        let existing_ips: std::collections::HashSet<String> = self
            .state
            .etcd_cluster_client
            .get_all_node_records()
            .await
            .map(|recs| recs.into_iter().map(|r| r.metadata.node_ip).collect())
            .unwrap_or_default();
        for n in &req.nodes {
            if existing_ips.contains(&n.node_ip) {
                continue;
            }
            let record = build_node_record(n);
            let member_id = record.metadata.member_id.clone();
            self.state
                .etcd_cluster_client
                .put_node_record(&member_id, &record)
                .await
                .map_err(|e| act_err(&format!("写节点记录 {}", n.node_ip), e))?;
            tracing::info!(node_ip = %n.node_ip, member_id = %member_id, "集群创建 finalize：节点记录已持久化");
        }

        tracing::info!(cluster_id = %req.cluster_id, "集群创建 finalize 完成");
        Ok(())
    }

    /// saga 补偿：反向 remove 已部署子项目（本地同步等终态 / 远程 best-effort 异步 + 日志）
    #[activity]
    pub async fn saga_cleanup(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: SagaCleanupReq,
    ) -> Result<(), ActivityError> {
        for step in req.steps.iter().rev() {
            // etcd 步骤：先移除 raft 成员（etcd 集群尚存活时；防止"幽灵成员"
            // 累积耗尽 quorum——仅 member_add 未 join 的成员会导致后续所有
            // 成员变更永久等待 quorum）；best-effort，失败仅日志
            if step.kind == "etcd" {
                let peer = crate::orchestration::etcd::peer_url(&step.node_ip);
                match self.state.etcd_cluster_client.find_member_id_by_peer_url(&peer).await {
                    Ok(Some(id)) => match self.state.etcd_cluster_client.member_remove(id).await {
                        Ok(()) => tracing::info!(
                            member_id = id,
                            node_ip = %step.node_ip,
                            "saga: raft 成员已移除"
                        ),
                        Err(e) => tracing::warn!(
                            err = %e.msg,
                            node_ip = %step.node_ip,
                            "saga: raft 成员移除失败（best-effort）"
                        ),
                    },
                    Ok(None) => {}
                    Err(e) => tracing::warn!(
                        err = %e.msg,
                        node_ip = %step.node_ip,
                        "saga: raft 成员查询失败（best-effort）"
                    ),
                }
            }
            let r: Result<(), String> = match (&step.kind[..], &step.remote) {
                ("etcd", None) => self
                    .state
                    .projects_client
                    .remove_sync(&step.compose_project_name)
                    .await
                    .map_err(|e| e.msg),
                ("pg", None) => self
                    .state
                    .projects_client
                    .remove_sync(&step.compose_project_name)
                    .await
                    .map_err(|e| e.msg),
                ("etcd", Some(ip)) => self
                    .state
                    .remote_client
                    .remove_etcd(ip, &step.compose_project_name)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.msg),
                ("pg", Some(ip)) => self
                    .state
                    .remote_client
                    .remove_pg(ip, &step.compose_project_name)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.msg),
                ("haproxy", None) => self
                    .state
                    .projects_client
                    .remove_sync(&step.compose_project_name)
                    .await
                    .map_err(|e| e.msg),
                ("haproxy", Some(ip)) => self
                    .state
                    .remote_client
                    .remove_haproxy_sync(ip, &step.compose_project_name)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.msg),
                ("temporal", None) => self
                    .state
                    .projects_client
                    .remove_sync(&step.compose_project_name)
                    .await
                    .map_err(|e| e.msg),
                ("temporal", Some(ip)) => self
                    .state
                    .remote_client
                    .remove_temporal_sync(ip, &step.compose_project_name)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.msg),
                (k, _) => Err(format!("未知 saga 步骤 kind={k}")),
            };
            match r {
                Ok(()) => tracing::info!(compose = %step.compose_project_name, "saga 补偿移除成功"),
                Err(msg) => tracing::warn!(
                    compose = %step.compose_project_name,
                    error = %msg,
                    "saga 补偿移除失败（容器已兜底清理则无害；残留实例行可 SQL 软删）"
                ),
            }
        }
        Ok(())
    }
}

// =========================================================================
// ClusterCreateWorkflow
// =========================================================================

#[workflow]
#[derive(Default)]
pub struct ClusterCreateWorkflow;

#[workflow_methods]
impl ClusterCreateWorkflow {
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        input: CreateClusterWorkflowInput,
    ) -> WorkflowResult<ClusterCreateWorkflowOutput> {
        let nodes = &input.nodes;
        // 基础设施拓扑 = 管理节点子集（D2：etcd raft / postgres-ha 仅部署于
        // manager；worker 节点跳过基础设施、finalize 阶段登记元数据）。
        // 调用方保证 nodes[0] 必为 manager（orchestration::validate_create_input）。
        let managers = crate::orchestration::cluster::manager_nodes(nodes);
        let total = nodes.len();
        let m = managers.len();
        let mut deployed: Vec<SagaStep> = Vec::new();
        let opts = ActivityOptions::start_to_close_timeout(std::time::Duration::from_secs(
            ACTIVITY_TIMEOUT_SECS,
        ));
        let short = ActivityOptions::start_to_close_timeout(std::time::Duration::from_secs(120));

        let initial_members: Vec<(String, String)> = managers
            .iter()
            .enumerate()
            .map(|(i, node)| (format!("etcd{i}"), node.node_ip.clone()))
            .collect();

        // ---- Stage 1：本机（首管理节点）etcd new ----
        // bootstrap 的 initial-cluster **只含自己**（new 模式自举；传未来成员会让
        // etcd 因 quorum 未达成而 /health 与读写全部卡住——3.5 实测：3 成员配置
        // 仅 1 节点在线时 probe 永久等待）。后续成员经 member_add 逐个 join。
        let stage = "etcd-new";
        let name = match ctx
            .start_activity(
                ClusterActivities::etcd_deploy_node,
                EtcdDeployNodeReq {
                    node_ip: managers[0].node_ip.clone(),
                    node_name: "etcd0".into(),
                    is_bootstrap: true,
                    initial_members: initial_members[..=0].to_vec(),
                    remote: None,
                },
                opts.clone(),
            )
            .await
            {
                Err(e) => {
                    return Err(on_stage_error(ctx, stage, &deployed, e).await);
                }
                Ok(n) => n,
            };
        deployed.push(SagaStep {
            kind: "etcd".into(),
            node_ip: managers[0].node_ip.clone(),
            remote: None,
            compose_project_name: name,
        });

        // ---- Stage 2：其余管理节点 etcd existing（member_add 在 activity 内）----
        // 注意：existing join 的 initial-cluster 必须只含**当前时点**已存在的 raft 成员
        // （member 0..=i：前段已 member_add 的 + 本节点自己）。若传全量未来成员，
        // etcd existing 启动时校验 "member count is unequal"（后续节点尚未 member_add）。
        for (i, node) in managers.iter().enumerate().skip(1) {
            let stage = format!("etcd-join-{i}");
            let joined_members: Vec<(String, String)> = initial_members[..=i].to_vec();
            let name = match ctx
                .start_activity(
                    ClusterActivities::etcd_deploy_node,
                    EtcdDeployNodeReq {
                        node_ip: node.node_ip.clone(),
                        node_name: format!("etcd{i}"),
                        is_bootstrap: false,
                        initial_members: joined_members,
                        remote: Some(node.node_ip.clone()),
                    },
                    opts.clone(),
                )
                .await
                {
                    Err(e) => {
                        return Err(on_stage_error(ctx, &stage, &deployed, e).await);
                    }
                    Ok(n) => n,
                };
            deployed.push(SagaStep {
                kind: "etcd".into(),
                node_ip: node.node_ip.clone(),
                remote: Some(node.node_ip.clone()),
                compose_project_name: name,
            });
        }

        // ---- Stage 3：本机（首管理节点）pg bootstrap ----
        // 前置：等 etcd DCS 全量成员就绪（偶数成员 raft quorum 缺口——Stage2
        // member_add 后新成员 join 完成前，本机 etcd 无 quorum 会短暂不可读写，
        // pg bootstrap 的 DCS 前置检查将 20507 失败）。等待由 pg_deploy_node
        // activity 内部 wait_etcd_dcs_ready 保证（bootstrap/join 统一）。
        let dcs_hosts = crate::orchestration::cluster::etcd_dcs_hosts(nodes);
        let stage = "pg-bootstrap";
        let name = match ctx
            .start_activity(
                ClusterActivities::pg_deploy_node,
                PgDeployNodeReq {
                    node_ip: managers[0].node_ip.clone(),
                    node_name: "pg-node-0".into(),
                    is_bootstrap: true,
                    etcd_dcs_hosts: dcs_hosts.clone(),
                    remote: None,
                },
                opts.clone(),
            )
            .await
            {
                Err(e) => {
                    return Err(on_stage_error(ctx, stage, &deployed, e).await);
                }
                Ok(n) => n,
            };
        deployed.push(SagaStep {
            kind: "pg".into(),
            node_ip: managers[0].node_ip.clone(),
            remote: None,
            compose_project_name: name,
        });

        // ---- Stage 4：其余管理节点 pg join ----
        for (i, node) in managers.iter().enumerate().skip(1) {
            let stage = format!("pg-join-{i}");
            let name = match ctx
                .start_activity(
                    ClusterActivities::pg_deploy_node,
                    PgDeployNodeReq {
                        node_ip: node.node_ip.clone(),
                        node_name: format!("pg-node-{i}"),
                        is_bootstrap: false,
                        etcd_dcs_hosts: dcs_hosts.clone(),
                        remote: Some(node.node_ip.clone()),
                    },
                    opts.clone(),
                )
                .await
                {
                    Err(e) => {
                        return Err(on_stage_error(ctx, &stage, &deployed, e).await);
                    }
                    Ok(n) => n,
                };
            deployed.push(SagaStep {
                kind: "pg".into(),
                node_ip: node.node_ip.clone(),
                remote: Some(node.node_ip.clone()),
                compose_project_name: name,
            });
        }

        // ---- Stage 5：haproxy 部署到全节点（managers 本机/远程 + workers 远程）----
        // pg_backends = manager 子集 IP（D-HA-08：worker 无 etcd，显式注入拓扑）；
        // temporal_backends 此刻恒空（temporal 尚未部署，Stage 6 成功后由
        // temporal_deploy 的 refresh_cluster_haproxy 联动纳入 7133）。
        // 本机（首 manager，remote=None）直连；其余节点经 remote 转发。
        let pg_backends: Vec<String> = managers.iter().map(|n| n.node_ip.clone()).collect();
        for node in nodes {
            let is_local = node.node_ip == managers[0].node_ip;
            let remote = if is_local { None } else { Some(node.node_ip.clone()) };
            let stage = format!("haproxy-{}", node.node_ip);
            let name = match ctx
                .start_activity(
                    ClusterActivities::haproxy_deploy_node,
                    HaproxyDeployNodeReq {
                        node_ip: node.node_ip.clone(),
                        pg_backends: pg_backends.clone(),
                        remote: remote.clone(),
                    },
                    opts.clone(),
                )
                .await
                {
                    Err(e) => {
                        return Err(on_stage_error(ctx, &stage, &deployed, e).await);
                    }
                    Ok(n) => n,
                };
            deployed.push(SagaStep {
                kind: "haproxy".into(),
                node_ip: node.node_ip.clone(),
                remote,
                compose_project_name: name,
            });
        }

        // ---- Stage 6：集群 temporal 部署（仅首 manager 单实例，D-TE-02）----
        // 前置本机 haproxy 15432（Stage 5 已部首 manager 本机）；DB 经本机 haproxy
        // → leader；凭据目标侧自读本机 etcd（密码不出本机）。
        let stage = "temporal";
        let name = match ctx
            .start_activity(
                ClusterActivities::temporal_deploy_node,
                TemporalDeployNodeReq {
                    node_ip: managers[0].node_ip.clone(),
                    remote: None,
                },
                opts.clone(),
            )
            .await
        {
            Err(e) => {
                return Err(on_stage_error(ctx, stage, &deployed, e).await);
            }
            Ok(n) => n,
        };
        deployed.push(SagaStep {
            kind: "temporal".into(),
            node_ip: managers[0].node_ip.clone(),
            remote: None,
            compose_project_name: name,
        });

        // ---- Stage 7：finalize（写 ClusterInfo + infra_status；全部节点登记）----
        let stage = "finalize";
        if let Err(e) = ctx
            .start_activity(
                ClusterActivities::finalize_cluster,
                FinalizeClusterReq {
                    cluster_id: input.cluster_id.clone(),
                    cluster_name: input.cluster_name.clone(),
                    nodes: input.nodes.clone(),
                },
                short.clone(),
            )
            .await
        {
            return Err(on_stage_error(ctx, stage, &deployed, e).await);
        }

        // temporal 部署经 activity 直连 build_deploy_bo，未过 handler 的 refresh 联动；
        // 显式补一次全集群 haproxy 刷新（激活 7133 temporal 监听，D-TE-04）。
        // 必须后置 finalize 之后：NodeRecord 于 finalize 才写入 etcd，refresh 依赖
        // 节点记录推导拓扑（v41 实机教训：前置执行 results 空集 → no-op）。
        // best-effort：失败仅观测（refresh_all 端点兜底），不阻断 create。
        let _ = ctx
            .start_activity(ClusterActivities::haproxy_refresh_cluster, (), opts.clone())
            .await;

        tracing::info!(total, managers = m, cluster_id = %input.cluster_id, "集群创建编排完成");
        Ok(ClusterCreateWorkflowOutput {
            status: "created".into(),
            cluster_id: input.cluster_id,
        })
    }
}

/// 失败路径：saga 补偿（best-effort）后转为 WorkflowTermination::Failed
async fn on_stage_error(
    ctx: &mut WorkflowContext<ClusterCreateWorkflow>,
    stage: &str,
    deployed: &[SagaStep],
    err: impl std::fmt::Display,
) -> WorkflowTermination {
    tracing::error!(stage, error = %err, "编排 stage 失败，触发 saga 补偿");
    let cleanup = ctx
        .start_activity(
            ClusterActivities::saga_cleanup,
            SagaCleanupReq {
                steps: deployed.to_vec(),
            },
            ActivityOptions::start_to_close_timeout(std::time::Duration::from_secs(300)),
        )
        .await;
    match cleanup {
        Ok(()) => tracing::warn!(stage, "编排失败，saga 补偿完成（反向 remove 已部署子项目）"),
        Err(e) => tracing::error!(stage, error = %e, "编排失败且 saga 补偿异常"),
    }
    WorkflowTermination::from(anyhow::anyhow!("{stage} 阶段失败，已 saga 补偿: {err}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::etcd::MockEtcdClusterClient;

    #[test]
    fn build_node_record_maps_fields_and_credentials() {
        let input = NodeInput {
            node_ip: "10.100.10.21".into(),
            display_name: String::new(), // 缺省 → 回退 node_ip
            admin_password: "pw123456".into(),
            role: crate::etcd::dto::role::MANAGER.into(),
        };
        let rec = build_node_record(&input);
        assert_eq!(rec.metadata.node_ip, "10.100.10.21");
        assert_eq!(rec.metadata.display_name, "10.100.10.21", "display_name 缺失应回退 node_ip");
        assert_eq!(rec.metadata.role, crate::etcd::dto::role::MANAGER);
        assert!(!rec.metadata.joined_at.is_empty());
        assert_eq!(rec.credentials.admin_password, "pw123456", "管理员密码应持久化到 credentials");
        assert_eq!(rec.credentials.pg_superuser_password, None);
        assert!(rec.metadata.api_endpoint.contains("10.100.10.21"), "api_endpoint 按 default 推导");
        assert!(!rec.metadata.member_id.is_empty());
    }

    #[test]
    fn build_node_record_keeps_display_name_and_worker_role() {
        let input = NodeInput {
            node_ip: "10.100.10.22".into(),
            display_name: "node-b".into(),
            admin_password: "pw123456".into(),
            role: crate::etcd::dto::role::WORKER.into(),
        };
        let rec = build_node_record(&input);
        assert_eq!(rec.metadata.display_name, "node-b");
        assert_eq!(rec.metadata.role, crate::etcd::dto::role::WORKER);
    }

    #[test]
    fn build_node_record_worker_keeps_role_for_finalize() {
        // finalize 以全部录入节点登记 NodeRecord：worker 角色必须原样保留
        let input = NodeInput {
            node_ip: "10.100.10.23".into(),
            display_name: "worker-23".into(),
            admin_password: "pw123456".into(),
            role: crate::etcd::dto::role::WORKER.into(),
        };
        let rec = build_node_record(&input);
        assert_eq!(rec.metadata.role, crate::etcd::dto::role::WORKER);
    }

    #[test]
    fn manager_subset_ordering_matches_topology() {
        // 混合角色：2 manager（21/22）+ 1 worker（23）—— manager 子集按录入顺序
        // 抽取且内部序号连续（与 orchestration 纯函数语义一致）
        let nodes = vec![
            NodeInput { node_ip: "10.0.0.21".into(), display_name: "m1".into(), admin_password: "pw123456".into(), role: crate::etcd::dto::role::MANAGER.into() },
            NodeInput { node_ip: "10.0.0.22".into(), display_name: "m2".into(), admin_password: "pw123456".into(), role: crate::etcd::dto::role::WORKER.into() },
            NodeInput { node_ip: "10.0.0.23".into(), display_name: "m3".into(), admin_password: "pw123456".into(), role: crate::etcd::dto::role::MANAGER.into() },
        ];
        let managers = crate::orchestration::cluster::manager_nodes(&nodes);
        assert_eq!(managers.len(), 2, "worker 不入 manager 子集");
        assert_eq!(managers[0].node_ip, "10.0.0.21");
        assert_eq!(managers[1].node_ip, "10.0.0.23");
        // workflow 使用的 initial_members 形状（与 orchestration::etcd_member_table 一致）
        let members: Vec<(String, String)> = managers
            .iter()
            .enumerate()
            .map(|(i, n)| (format!("etcd{i}"), crate::orchestration::etcd::peer_url(&n.node_ip)))
            .collect();
        assert_eq!(
            members,
            vec![
                ("etcd0".to_string(), "http://10.0.0.21:2380".to_string()),
                ("etcd1".to_string(), "http://10.0.0.23:2380".to_string()),
            ]
        );
    }

    /// DCS 全量就绪等待：member_count 达标后立即返回（分批补齐成员模拟 join 推进）
    #[tokio::test]
    async fn wait_dcs_ready_succeeds_once_members_join() {
        let mock = MockEtcdClusterClient::new();
        // 初始 member_count=1（虚拟 bootstrap + 0 成员）< 2 → 未就绪
        assert!(
            !wait_etcd_dcs_ready_with(&mock, 2, 2, std::time::Duration::from_millis(5)).await,
            "成员数不足时应继续等待"
        );
        // member_add 补齐（1 个真实成员 → member_count=2）→ 立即就绪
        let _ = mock.member_add("http://10.0.0.22:2380").await.unwrap();
        assert!(
            wait_etcd_dcs_ready_with(&mock, 2, 2, std::time::Duration::from_millis(5)).await,
            "成员数达标后应就绪"
        );
    }

    /// DCS 全量就绪等待：expected=0（无边端点）视为立即就绪；etcd 不可达则超时失败
    #[tokio::test]
    async fn wait_dcs_ready_edge_cases() {
        let mock = MockEtcdClusterClient::new();
        assert!(
            wait_etcd_dcs_ready_with(&mock, 0, 1, std::time::Duration::from_millis(1)).await,
            "expected=0 应视为就绪"
        );
        mock.set_unavailable(true);
        assert!(
            !wait_etcd_dcs_ready_with(&mock, 2, 2, std::time::Duration::from_millis(5)).await,
            "etcd 不可达时应等到超时返回 false"
        );
    }
}
