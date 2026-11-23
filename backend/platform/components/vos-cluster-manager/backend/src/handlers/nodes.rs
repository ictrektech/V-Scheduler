//! 节点 API：录入 / 列表 / 详情 / 删除
//!
//! 契约见 `openspec/changes/cluster-registration-nodes/specs/cluster-registration/spec.md`：
//! - 参数校验 20501（IP / 密码 / role）｜IP 重复 20506｜不存在 20505｜etcd 故障 20504
//! - **凭据不回显**：所有响应仅返回 NodeRecordView（无 admin_password）
//! - Phase 4 扩展（D-CR-05）：`join_infra=true`（缺省）录入同时联动等 etcd member_add +
//!   目标节点 etcd(existing)/pg(join) 部署；`DELETE ?remove_infra=true` 联动移除子项目 +
//!   member_remove（失败反向移除，幂等可重试）
//! - 角色语义（cluster-role-and-create-entry）：join_infra 仅支持管理节点
//!   （role=manager，运行 etcd + postgres-ha）；工作节点（role=worker）不部署基础设施

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::Utc;
use serde_json::{json, Value};

use crate::db;
use crate::error::codes;
use crate::error::{ResultBody, VcmError};
use crate::etcd::dto::{self, NodeCredentials, NodeMetadata, NodeRecord, NodeRecordView};
use crate::handlers::cluster::default_api_endpoint;
use crate::state::AppState;

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn validate_ip(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

/// join 编排时序参数（生产默认值；单测注入毫秒级，避免 60s 级等待）
#[derive(Debug, Clone, Copy)]
struct JoinTimings {
    /// 目标节点 etcd 部署重试（unhealthy 竞态窗口）
    etcd_deploy_attempts: u32,
    etcd_deploy_interval: std::time::Duration,
    /// 本机 etcd 可达性探活（不要求 quorum / 成员数语义）
    ping_attempts: u32,
    ping_interval: std::time::Duration,
    /// 目标节点 etcd DCS 就绪轮询
    dcs_attempts: u32,
    dcs_interval: std::time::Duration,
    /// learner → 投票成员 promote 重试（需已追平 leader）
    promote_attempts: u32,
    promote_interval: std::time::Duration,
    /// promote 后投票成员数校验轮询
    member_count_attempts: u32,
    member_count_interval: std::time::Duration,
    /// 目标节点 pg 部署重试（DCS 未就绪窗口）
    pg_attempts: u32,
    pg_interval: std::time::Duration,
}

impl Default for JoinTimings {
    fn default() -> Self {
        Self {
            etcd_deploy_attempts: 3,
            etcd_deploy_interval: std::time::Duration::from_secs(60),
            ping_attempts: 10,
            ping_interval: std::time::Duration::from_secs(1),
            dcs_attempts: 12,
            dcs_interval: std::time::Duration::from_secs(5),
            promote_attempts: 6,
            promote_interval: std::time::Duration::from_secs(5),
            member_count_attempts: 10,
            member_count_interval: std::time::Duration::from_secs(3),
            pg_attempts: 10,
            pg_interval: std::time::Duration::from_secs(6),
        }
    }
}

/// 已加入 raft 的 join 计划（member 加入后各阶段共享的不可变上下文）
struct JoinPlan {
    node_ip: String,
    member_id: u64,
    all_ips: Vec<String>,
    initial_members: Vec<Value>,
    expect_members: u64,
}

/// join 各阶段进度（失败信息与补偿判定用）
#[derive(Default)]
struct JoinProgress {
    etcd_ok: bool,
    pg_ok: bool,
}

/// 节点等 etcd/pg 加入编排（join_infra 联动；仅管理节点。失败反向移除已部署子项目）
///
/// # quorum 不变式（2026-10-30 实机事故）
///
/// **投票成员数增加前，新成员必须已经在服**；否则单节点集群会永久失 quorum：
/// 单节点集群直接 `member_add`（投票）→ quorum 1→2 而新节点 etcd 尚未部署 →
/// raft `NO LEADER`；此时 `member_remove` 同样需要 quorum → 幽灵成员永远无法清理，
/// patroni 亦因 DCS 失联把 postgres 降为 replica。故 join 固定采用：
///
/// 1. `member_add_as_learner`（learner 不投票，quorum 不变）
/// 2. 目标节点 etcd `initial_state=existing` 部署 + DCS 就绪
/// 3. `member_promote`（目标 etcd 已追平 leader 才允许）→ 此时才增加投票成员数
///
/// 失败补偿顺序同样是安全关键（见 `join_rollback`）：**先** `member_remove`
/// （目标 etcd 可能仍在服，quorum 完整可提交），再删目标容器。
///
/// 其他健壮性（实机验证 2026-09-06）：成员加入幂等（peer 已存在则跳过）；
/// 部署允许 unhealthy 竞态重试（≤3 次 × 60s）。
///
/// 调用方保证：本函数仅对管理节点（role=manager）调用；`existing_ips` 只含
/// 集群中既有管理节点（worker 不在 raft 成员表内）。
async fn join_infra(
    state: &AppState,
    node_ip: &str,
    existing_ips: &[String],
) -> Result<(), VcmError> {
    join_infra_with(state, node_ip, existing_ips, JoinTimings::default()).await
}

/// `join_infra` 的可配时序实现（单测直接调用，注入毫秒级 timings）
async fn join_infra_with(
    state: &AppState,
    node_ip: &str,
    existing_ips: &[String],
    timings: JoinTimings,
) -> Result<(), VcmError> {
    // 1) 探活（目标节点须安装运行本应用）
    state
        .remote_client
        .probe(node_ip)
        .await
        .map_err(|e| VcmError::new(codes::REMOTE_NODE_FAILED, e.msg))?;

    // 全部 raft 成员（既有管理节点 + 新管理节点；worker 不入 raft——调用方已过滤）
    let all_ips: Vec<String> = existing_ips
        .iter()
        .cloned()
        .chain(std::iter::once(node_ip.to_string()))
        .collect();
    let initial_members: Vec<Value> = all_ips
        .iter()
        .enumerate()
        .map(|(i, ip)| json!({"name": format!("etcd{i}"), "node_ip": ip}))
        .collect();
    let peer_url = crate::orchestration::etcd::peer_url(node_ip);

    // 2) raft 成员加入：**learner**（不投票、不改变 quorum）；peer 已存在则跳过（幂等）
    let member_id = match state
        .etcd_cluster_client
        .find_member_id_by_peer_url(&peer_url)
        .await
    {
        Ok(Some(id)) => {
            tracing::info!(
                node_ip,
                member_id = id,
                "join: raft 成员已存在（幂等重入），跳过 learner 加入"
            );
            id
        }
        _ => {
            let (id, _) = state
                .etcd_cluster_client
                .member_add_as_learner(&peer_url)
                .await
                .map_err(|e| {
                    VcmError::new(
                        codes::ETCD_FAILED,
                        format!("etcd member_add_as_learner 失败: {}", e.msg),
                    )
                })?;
            tracing::info!(
                node_ip,
                member_id = id,
                "join: etcd learner 成员已加入（不参与投票，quorum 安全）"
            );
            id
        }
    };

    let plan = JoinPlan {
        node_ip: node_ip.to_string(),
        member_id,
        all_ips,
        initial_members,
        expect_members: existing_ips.len() as u64 + 1,
    };
    let mut progress = JoinProgress::default();

    // 3-6) 部署 + promote；member 已加入后**任何**失败都必须补偿（含 member_remove）
    if let Err(e) = join_deploy(state, &plan, &timings, &mut progress).await {
        tracing::error!(
            node_ip,
            member_id = plan.member_id,
            err = %e.msg,
            "join: 编排失败，开始补偿（先 member_remove 再删容器，避免失 quorum）"
        );
        join_rollback(state, &plan).await;
        return Err(VcmError::new(
            codes::ORCHESTRATION_FAILED,
            format!(
                "节点 {} 加入编排失败（etcd_ok={} pg_ok={}）：{}；已反向清理可重试",
                node_ip, progress.etcd_ok, progress.pg_ok, e.msg
            ),
        ));
    }

    tracing::info!(
        node_ip,
        raft_members = plan.expect_members,
        "节点等 etcd/pg 加入编排完成（join_infra）"
    );
    Ok(())
}

/// member 已加入后的部署阶段：ping → etcd(existing) → DCS 就绪 → promote → pg(join)
async fn join_deploy(
    state: &AppState,
    plan: &JoinPlan,
    timings: &JoinTimings,
    progress: &mut JoinProgress,
) -> Result<(), VcmError> {
    // 3) 仅确认本机 etcd 可达（serializable 探活，**不要求成员数 / quorum 语义**；
    //    旧实现用 member_count 校验，正是单节点集群加入时的永久超时点）
    let mut reachable = false;
    for attempt in 0..timings.ping_attempts {
        if state.etcd_cluster_client.ping().await {
            reachable = true;
            break;
        }
        tracing::warn!(attempt, "join: 本机 etcd 尚不可达，轮询");
        tokio::time::sleep(timings.ping_interval).await;
    }
    if !reachable {
        return Err(VcmError::new(
            codes::ETCD_FAILED,
            "本机 etcd 不可达（join 前置探活失败）",
        ));
    }

    // 4) 目标节点 etcd 部署（existing，全量成员含本节点；unhealthy 竞态重试；
    //    20506=已部署 → 交由 DCS 就绪轮询判定真实可用）
    let etcd_body = json!({
        "node_ip": plan.node_ip,
        "node_name": format!("etcd{}", plan.all_ips.len().saturating_sub(1)),
        "initial_state": "existing",
        "initial_members": plan.initial_members,
    });
    let mut etcd_deployed = false;
    for attempt in 0..timings.etcd_deploy_attempts {
        match state
            .remote_client
            .deploy_etcd_sync(&plan.node_ip, etcd_body.clone())
            .await
        {
            Ok(_) => {
                etcd_deployed = true;
                break;
            }
            Err(e) if e.code == codes::EXISTS || e.msg.contains("已部署") => {
                // 目标节点已有 etcd 部署（幂等边界：首次请求超时但对端已提交；
                // remote 转发的错误码被泛化为 REMOTE_NODE_FAILED，按 msg 判定）
                tracing::warn!(
                    attempt,
                    node_ip = %plan.node_ip,
                    "join: 目标节点 etcd 已部署（等待 DCS 就绪）"
                );
                etcd_deployed = true;
                break;
            }
            Err(e) => {
                tracing::warn!(
                    attempt,
                    node_ip = %plan.node_ip,
                    err = %e.msg,
                    "join: 远程 etcd 部署失败，等待 raft 稳定后重试"
                );
                tokio::time::sleep(timings.etcd_deploy_interval).await;
            }
        }
    }
    if !etcd_deployed {
        return Err(VcmError::new(
            codes::REMOTE_NODE_FAILED,
            format!(
                "目标节点 {} etcd 部署失败（{} 次重试后）",
                plan.node_ip, timings.etcd_deploy_attempts
            ),
        ));
    }

    // 4.1) 等待目标节点 etcd DCS 真实就绪（2379 探活；未就绪不得进入 promote/pg）
    let mut dcs_ready = false;
    for attempt in 0..timings.dcs_attempts {
        match state.remote_client.etcd_dcs_ready(&plan.node_ip).await {
            Ok(true) => {
                tracing::info!(attempt, node_ip = %plan.node_ip, "join: 目标节点 etcd DCS 已就绪");
                dcs_ready = true;
                break;
            }
            _ => {
                tracing::warn!(attempt, node_ip = %plan.node_ip, "join: 目标节点 etcd DCS 尚未就绪，轮询");
                tokio::time::sleep(timings.dcs_interval).await;
            }
        }
    }
    if !dcs_ready {
        return Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("目标节点 {} etcd DCS 未在等待窗口内就绪", plan.node_ip),
        ));
    }

    // 4.2) learner → 投票成员（目标 etcd 已启动并开始复制；promote 要求 learner
    //      已追平 leader，未追平按 6×5s 重试）。**此刻之前投票成员数始终未变**，
    //      故目标节点未在服时 quorum 不会被抬高（本次事故的根因修复点）。
    crate::etcd::promote_learner_with_retry(
        &*state.etcd_cluster_client,
        plan.member_id,
        timings.promote_attempts,
        timings.promote_interval,
    )
    .await?;
    tracing::info!(
        node_ip = %plan.node_ip,
        member_id = plan.member_id,
        "join: learner 已提升为投票成员"
    );

    // 4.3) 校验投票成员数达到预期（raft 成员表全量可见）
    let mut count_ok = false;
    for attempt in 0..timings.member_count_attempts {
        if state
            .etcd_cluster_client
            .member_count()
            .await
            .map(|n| n >= plan.expect_members)
            .unwrap_or(false)
        {
            count_ok = true;
            break;
        }
        tracing::warn!(
            attempt,
            expected = plan.expect_members,
            "join: raft 成员数未达预期，轮询"
        );
        tokio::time::sleep(timings.member_count_interval).await;
    }
    if !count_ok {
        return Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("raft 成员数未达预期（{}）", plan.expect_members),
        ));
    }
    progress.etcd_ok = true;

    // 5) 目标节点 pg 部署（join，bootstrap=false）
    //    20507(等 etcd DCS 未就绪→重试等待) / 20506(已部署→成功) 语义与 etcd 相同
    let pg_body = json!({
        "node_ip": plan.node_ip,
        "node_name": format!("pg-node-{}", plan.all_ips.len().saturating_sub(1)),
        "bootstrap": false,
        "etcd_hosts": plan.all_ips.iter().map(|ip| format!("{ip}:2379")).collect::<Vec<_>>().join(","),
    });
    for attempt in 0..timings.pg_attempts {
        match state
            .remote_client
            .deploy_pg_sync(&plan.node_ip, pg_body.clone())
            .await
        {
            Ok(_) => {
                progress.pg_ok = true;
                break;
            }
            Err(e) if e.code == codes::EXISTS || e.msg.contains("已部署") => {
                tracing::warn!(
                    attempt,
                    node_ip = %plan.node_ip,
                    "join: 目标节点 postgres-ha 已部署（视为成功）"
                );
                progress.pg_ok = true;
                break;
            }
            Err(e) => {
                tracing::warn!(
                    attempt,
                    node_ip = %plan.node_ip,
                    err = %e.msg,
                    "join: 远程 pg 部署失败，重试（etcd DCS 未就绪窗口）"
                );
                tokio::time::sleep(timings.pg_interval).await;
            }
        }
    }
    if !progress.pg_ok {
        return Err(VcmError::new(
            codes::REMOTE_NODE_FAILED,
            format!(
                "目标节点 {} postgres-ha 部署失败（{} 次重试后）",
                plan.node_ip, timings.pg_attempts
            ),
        ));
    }

    Ok(())
}

/// join 失败补偿：**顺序是关键**。
///
/// 必须先 `member_remove`（目标 etcd 可能仍在服 → quorum 完整 → 移除可提交），
/// 再删目标容器；反之先删容器会让单节点集群永久失 quorum（2026-10-30 事故：
/// `member_remove` 需要 quorum → 永远无法提交，幽灵成员残留）。
async fn join_rollback(state: &AppState, plan: &JoinPlan) {
    // (i) raft 成员移除（幂等：已不存在 / 已移除 → 仅日志）
    match state
        .etcd_cluster_client
        .member_remove(plan.member_id)
        .await
    {
        Ok(()) => tracing::info!(
            node_ip = %plan.node_ip,
            member_id = plan.member_id,
            "join 补偿: raft 成员已移除（quorum 安全）"
        ),
        Err(e) => tracing::warn!(
            node_ip = %plan.node_ip,
            member_id = plan.member_id,
            err = %e.msg,
            "join 补偿: raft 成员移除失败（已不存在或 etcd 不可达；若 quorum 已丢失需按 troubleshooting 手工恢复）"
        ),
    }

    // (ii) 目标节点 pg 子项目（best-effort；未部署时 list 返回空/失败均忽略）
    if let Ok(names) = state
        .remote_client
        .list_instances(&plan.node_ip, "pg")
        .await
    {
        for name in names {
            let _ = state.remote_client.remove_pg(&plan.node_ip, &name).await;
        }
    }

    // (iii) 目标节点 etcd 子项目
    if let Ok(names) = state
        .remote_client
        .list_instances(&plan.node_ip, "etcd")
        .await
    {
        for name in names {
            let _ = state.remote_client.remove_etcd(&plan.node_ip, &name).await;
        }
    }

    // (iv) 清理目标节点残留数据目录（失败实例 Restarting + 污染数据目录会永久
    //      阻塞后续 join；reset_data 删除 cluster-*-node-* 目录）
    match state.remote_client.reset_infra_data(&plan.node_ip).await {
        Ok(_) => tracing::info!(node_ip = %plan.node_ip, "join 补偿: 目标节点残留数据目录已清理"),
        Err(e) => {
            tracing::warn!(node_ip = %plan.node_ip, err = %e.msg, "join 补偿: 残留数据清理失败（best-effort）")
        }
    }
}

/// 录入节点（IP + 管理员密码持久化到 etcd；凭据不回显）
pub async fn node_add(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;

    // ---- 参数校验（20501）----
    let node_ip = body
        .get("node_ip")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| validate_ip(s))
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "node_ip 必填且为合法 IP 地址"))?;

    let admin_password = body
        .get("admin_password")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty() && s.len() >= 6)
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "admin_password 必填且长度 ≥6"))?;

    let role = body
        .get("role")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| s == dto::role::MANAGER || s == dto::role::WORKER)
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "role 必须为 manager（管理节点）或 worker（工作节点）"))?;

    let display_name = body
        .get("display_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().count() <= 64);

    let api_endpoint = body
        .get("api_endpoint")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_api_endpoint(&node_ip));

    // ---- 集群是否已初始化（未初始化 → 20507）----
    if state.etcd_cluster_client.get_cluster_info().await?.is_none() {
        return Err(VcmError::new(
            codes::STATE_NOT_SUPPORTED,
            "集群尚未初始化，请先调用 POST /api/cluster/init",
        ));
    }

    // ---- 幂等：node_ip 已存在 → 20506（重复请求优先于 join_infra 语义校验）----
    let records = state.etcd_cluster_client.get_all_node_records().await?;
    if records.iter().any(|r| r.metadata.node_ip == node_ip) {
        return Err(VcmError::new(codes::EXISTS, format!("节点 {node_ip} 已存在")));
    }

    // ---- join_infra 角色限制（D3）：仅管理节点（运行 etcd/pg）可联动部署基础设施；
    // 工作节点不加入 etcd raft / postgres-ha ----
    let join_infra_flag = body.get("join_infra").and_then(|v| v.as_bool()).unwrap_or(true);
    if join_infra_flag && role != dto::role::MANAGER {
        return Err(VcmError::new(
            codes::PARAM_INVALID,
            "工作节点（role=worker）不运行 etcd / postgres-ha，join_infra 仅支持管理节点（role=manager）",
        ));
    }

    // ---- join_infra 联动（Phase 4 D-CR-05；缺省 true；仅 manager 可联动；
    // worker 已在上面 20501 拦截）----
    if join_infra_flag {
        // 集群须已创建（etcd raft + pg 运行中）才能加入基础设施
        let info = state.etcd_cluster_client.get_cluster_info().await?;
        match info {
            Some(i) if i.status == dto::status::CREATED => {}
            _ => {
                return Err(VcmError::new(
                    codes::STATE_NOT_SUPPORTED,
                    "join_infra 要求集群已创建（status=created，etcd raft + postgres-ha 运行中）；请先 POST /api/cluster/create",
                ));
            }
        }
        let existing_ips: Vec<String> = records
            .iter()
            // raft 成员表只含管理节点（worker 不入 raft；旧值 master 归一化计入）
            .filter(|r| dto::role::normalize_legacy(&r.metadata.role) == dto::role::MANAGER)
            .map(|r| r.metadata.node_ip.clone())
            .collect();
        join_infra(&state, &node_ip, &existing_ips).await?;
    }

    // ---- 写 etcd ----
    let member_id = uuid::Uuid::new_v4().to_string();
    let now = now_rfc3339();
    let record = NodeRecord {
        metadata: NodeMetadata {
            member_id: member_id.clone(),
            display_name: display_name.unwrap_or_else(|| node_ip.clone()),
            role,
            node_ip,
            api_endpoint,
            joined_at: now.clone(),
        },
        credentials: NodeCredentials {
            admin_password,
            updated_at: now,
            pg_superuser_password: None,
            pg_replication_password: None,
        },
    };

    state
        .etcd_cluster_client
        .put_node_record(&member_id, &record)
        .await?;

    // ---- 管理面审计（db 失败不阻断）----
    if let Some(db) = &state.db {
        let _ = db::insert_operation_log(
            db,
            "node_add",
            &member_id,
            Some(json!({"node_ip": record.metadata.node_ip, "role": record.metadata.role})),
        )
        .await;
    }

    tracing::info!(
        member_id = %member_id,
        node_ip = %record.metadata.node_ip,
        "节点录入完成"
    );

    let view: NodeRecordView = (&record).into();
    Ok(Json(ResultBody::ok(json!({"node": view}), version)))
}

async fn use_registered_hostnames(state: &AppState, views: &mut [NodeRecordView]) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    if crate::projects::APP_ID != "com.ictrek.v-scheduler" || views.is_empty() {
        return;
    }
    let Some(db) = &state.db else { return };
    let Ok(rows) = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT ON (host) host,hostname FROM public.modelinfer_nodes
         WHERE hostname <> '' ORDER BY host,updated_at DESC",
        ))
        .await
    else {
        return;
    };
    for row in rows {
        let (Ok(host), Ok(hostname)) = (
            row.try_get::<String>("", "host"),
            row.try_get::<String>("", "hostname"),
        ) else {
            continue;
        };
        for view in views.iter_mut().filter(|view| view.node_ip == host) {
            view.hostname = Some(hostname.clone());
        }
    }
}

/// 节点列表（脱敏——无 admin_password）
pub async fn node_list(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let records = state.etcd_cluster_client.get_all_node_records().await?;
    let mut views: Vec<NodeRecordView> = records.iter().map(NodeRecordView::from).collect();
    use_registered_hostnames(&state, &mut views).await;
    Ok(Json(ResultBody::ok(json!({"nodes": views}), version)))
}

/// 节点详情（脱敏）
pub async fn node_get(
    State(state): State<AppState>,
    Path(member_id): Path<String>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    match state.etcd_cluster_client.get_node_record(&member_id).await? {
        Some(rec) => {
            let view: NodeRecordView = (&rec).into();
            Ok(Json(ResultBody::ok(json!({"node": view}), version)))
        }
        None => Err(VcmError::new(codes::NOT_FOUND, format!("节点 {member_id} 不存在"))),
    }
}

/// 删除节点（成员元数据；Phase 4 D-CR-05：`?remove_infra=true`（缺省）联动移除
/// 目标节点 etcd/pg 子项目 + etcd member remove，幂等可重试）
pub async fn node_delete(
    State(state): State<AppState>,
    Path(member_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;
    let remove_infra = params
        .get("remove_infra")
        .map(|s| s != "false")
        .unwrap_or(true);

    // 存在性检查（20505）
    let record = match state.etcd_cluster_client.get_node_record(&member_id).await? {
        Some(rec) => rec,
        None => {
            return Err(VcmError::new(codes::NOT_FOUND, format!("节点 {member_id} 不存在")));
        }
    };
    let node_ip = record.metadata.node_ip.clone();

    if remove_infra {
        // 1) **raft 成员必须先摘除**（顺序是安全关键）：此刻目标节点 etcd 可能仍在服，
        //    集群 quorum 完整，member_remove 才能提交。若先删目标 etcd 子项目，会立刻
        //    打掉 quorum（单节点 / 偶数成员尤甚），此后 member_remove 永远无法提交，
        //    留下幽灵成员——与 2026-10-30 join 事故同一失败模式（见 troubleshooting T-35）。
        let peer_url = crate::orchestration::etcd::peer_url(&node_ip);
        match state
            .etcd_cluster_client
            .find_member_id_by_peer_url(&peer_url)
            .await
        {
            Ok(Some(id)) => {
                state
                    .etcd_cluster_client
                    .member_remove(id)
                    .await
                    .map_err(|e| {
                        VcmError::new(
                            codes::ETCD_FAILED,
                            format!(
                                "raft 成员移除失败（member_remove）：{}；已中止，未删除子项目与节点记录（先删 etcd 会永久失 quorum，可重试）",
                                e.msg
                            ),
                        )
                    })?;
                tracing::info!(node_ip, member_id = id, "成员移除：etcd raft 成员已移除（quorum 安全）");
            }
            // worker 不在 raft 成员表内；或此前已手工移除 → 幂等跳过
            Ok(None) => tracing::warn!(
                node_ip,
                "成员移除：etcd raft 中未找到该成员（worker 或已移除，跳过）"
            ),
            // 查询失败时无法确认成员是否仍在 raft：继续删容器可能永久失 quorum → 必须先中止
            Err(e) => {
                return Err(VcmError::new(
                    codes::ETCD_FAILED,
                    format!(
                        "raft 成员查询失败：{}；已中止（先删 etcd 容器可能永久失 quorum，可重试）",
                        e.msg
                    ),
                ));
            }
        }

        // 2) 目标节点子项目移除（成员已摘除，集群按新成员数恢复 quorum，可安全检查）
        //    实例清单驱动；EIO 时容器兜底已删 + SQL 软删由平台侧兜底。
        //    先 pg 后 etcd：避免 etcd 先消失导致 patroni DCS 抖动。
        if let Ok(names) = state.remote_client.list_instances(&node_ip, "pg").await {
            for name in names {
                match state.remote_client.remove_pg(&node_ip, &name).await {
                    Ok(_) => tracing::info!(node_ip, compose = %name, "成员移除：pg 子项目已清除"),
                    Err(e) => tracing::warn!(node_ip, compose = %name, err = %e.msg, "成员移除：pg 子项目清除失败（幂等可重试）"),
                }
            }
        }
        if let Ok(names) = state.remote_client.list_instances(&node_ip, "etcd").await {
            for name in names {
                match state.remote_client.remove_etcd(&node_ip, &name).await {
                    Ok(_) => tracing::info!(node_ip, compose = %name, "成员移除：etcd 子项目已清除"),
                    Err(e) => tracing::warn!(node_ip, compose = %name, err = %e.msg, "成员移除：etcd 子项目清除失败（幂等可重试）"),
                }
            }
        }
    }

    state
        .etcd_cluster_client
        .delete_node_record(&member_id)
        .await?;

    if let Some(db) = &state.db {
        let _ = db::insert_operation_log(db, "node_remove", &member_id, None).await;
    }

    Ok(Json(ResultBody::ok(
        json!({"deleted": true, "member_id": member_id}),
        version,
    )))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::etcd::{EtcdClusterClient, MockEtcdClusterClient};
    use crate::projects::MockProjectsClient;
    use crate::remote::MockRemoteNodeClient;
    use std::sync::{Arc, Mutex};

    const TARGET_IP: &str = "10.0.0.5";
    const TARGET_PEER: &str = "http://10.0.0.5:2380";

    fn fast_timings() -> JoinTimings {
        JoinTimings {
            etcd_deploy_attempts: 2,
            etcd_deploy_interval: std::time::Duration::from_millis(1),
            ping_attempts: 2,
            ping_interval: std::time::Duration::from_millis(1),
            dcs_attempts: 2,
            dcs_interval: std::time::Duration::from_millis(1),
            promote_attempts: 2,
            promote_interval: std::time::Duration::from_millis(1),
            member_count_attempts: 3,
            member_count_interval: std::time::Duration::from_millis(1),
            pg_attempts: 2,
            pg_interval: std::time::Duration::from_millis(1),
        }
    }

    /// 构造 join 单测 AppState：etcd / remote Mock 共享同一调用序日志
    fn join_test_state(
        log: &Arc<Mutex<Vec<String>>>,
    ) -> (
        AppState,
        Arc<MockEtcdClusterClient>,
        Arc<MockRemoteNodeClient>,
    ) {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        let etcd = Arc::new(MockEtcdClusterClient::with_event_log(log.clone()));
        let remote = Arc::new(MockRemoteNodeClient::with_event_log(log.clone()));
        remote.preload(&[TARGET_IP]);
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            etcd.clone() as Arc<dyn crate::etcd::EtcdClusterClient>,
            Arc::new(MockProjectsClient::new()),
            None,
            remote.clone() as Arc<dyn crate::remote::RemoteNodeClient>,
            crate::remote::default_remote_app_port(),
            None,
        );
        (state, etcd, remote)
    }

    fn events(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        log.lock().unwrap().clone()
    }

    fn pos(events: &[String], needle: &str) -> usize {
        events
            .iter()
            .position(|e| e == needle)
            .unwrap_or_else(|| panic!("调用序缺失事件 {needle}（实际 {events:?}）"))
    }

    /// T1：单节点集群 join 成功路径——先 learner 加入、部署目标 etcd、再 promote
    #[tokio::test]
    async fn join_single_node_adds_learner_then_promotes() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (state, etcd, _remote) = join_test_state(&log);

        let out = join_infra_with(&state, TARGET_IP, &["10.0.0.1".into()], fast_timings()).await;
        assert!(out.is_ok(), "单节点 join 应成功: {out:?}");

        let ev = events(&log);
        let add = pos(&ev, "etcd:member_add_as_learner");
        let deploy = pos(&ev, "remote:deploy_etcd_sync");
        let promote = pos(&ev, "etcd:member_promote");
        assert!(add < deploy, "必须先以 learner 加入再部署目标 etcd: {ev:?}");
        assert!(deploy < promote, "目标 etcd 部署就绪后才 promote: {ev:?}");
        assert!(
            !ev.iter().any(|e| e == "etcd:member_add"),
            "join 不得使用投票 member_add（会立刻抬高 quorum）: {ev:?}"
        );

        assert_eq!(etcd.member_count().await.unwrap(), 2, "最终成员数应为 2");
        let id = etcd
            .find_member_id_by_peer_url(TARGET_PEER)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            etcd.member_is_learner(id).await.unwrap(),
            Some(false),
            "promote 后新成员应为投票成员"
        );
    }

    /// T2：远程 etcd 部署失败——回滚恢复成员表（无幽灵成员），且 member_remove 先于容器 remove
    #[tokio::test]
    async fn join_etcd_deploy_failure_rolls_back_member_before_containers() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (state, etcd, remote) = join_test_state(&log);
        remote.set_fail_op("deploy_etcd", true);

        let err = join_infra_with(&state, TARGET_IP, &["10.0.0.1".into()], fast_timings())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::ORCHESTRATION_FAILED, "join 失败应 20511");

        assert!(
            etcd.find_member_id_by_peer_url(TARGET_PEER)
                .await
                .unwrap()
                .is_none(),
            "回滚后不得留下幽灵 raft 成员"
        );
        assert_eq!(
            etcd.member_count().await.unwrap(),
            1,
            "成员表应恢复为单节点"
        );

        let ev = events(&log);
        let member_remove = pos(&ev, "etcd:member_remove");
        assert!(
            member_remove < pos(&ev, "remote:remove_etcd"),
            "member_remove 必须先于容器 remove（否则 quorum 丢失无法提交）: {ev:?}"
        );
        assert!(
            pos(&ev, "remote:remove_etcd") < pos(&ev, "remote:reset_infra_data"),
            "残留数据清理应最后执行: {ev:?}"
        );
    }

    /// T3：幂等重入——peer 已是投票成员 → 不再 add、不再 promote，流程继续
    #[tokio::test]
    async fn join_is_idempotent_when_peer_already_voting() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (state, etcd, remote) = join_test_state(&log);
        let (id, _) = etcd.member_add(TARGET_PEER).await.unwrap();

        let out = join_infra_with(&state, TARGET_IP, &["10.0.0.1".into()], fast_timings()).await;
        assert!(out.is_ok(), "幂等重入应继续完成: {out:?}");

        let ev = events(&log);
        assert!(
            !ev.iter().any(|e| e == "etcd:member_add_as_learner"),
            "peer 已存在时不应重复加入: {ev:?}"
        );
        assert!(
            !ev.iter().any(|e| e == "etcd:member_promote"),
            "已是投票成员时不应 promote: {ev:?}"
        );
        assert_eq!(etcd.member_is_learner(id).await.unwrap(), Some(false));
        assert!(!remote.events().is_empty(), "部署阶段仍应执行");
    }

    /// T4：promote 失败（learner 未追平）→ 回滚，成员表恢复原状
    #[tokio::test]
    async fn join_promote_failure_rolls_back_member() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (state, etcd, _remote) = join_test_state(&log);
        etcd.set_promote_failing(true);

        let err = join_infra_with(&state, TARGET_IP, &["10.0.0.1".into()], fast_timings())
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            codes::ORCHESTRATION_FAILED,
            "promote 失败应 20511"
        );

        assert!(
            etcd.find_member_id_by_peer_url(TARGET_PEER)
                .await
                .unwrap()
                .is_none(),
            "promote 失败回滚后不得残留成员"
        );
        assert_eq!(etcd.member_count().await.unwrap(), 1);

        let ev = events(&log);
        assert!(
            ev.iter().any(|e| e == "etcd:member_promote"),
            "应尝试 promote: {ev:?}"
        );
        assert!(
            pos(&ev, "etcd:member_remove") < pos(&ev, "remote:remove_etcd"),
            "回滚仍须先 member_remove 再删容器: {ev:?}"
        );
    }

    /// 成员移除：必须先摘 raft 成员再删目标容器（否则 quorum 丢失后 member_remove 无法提交）
    #[tokio::test]
    async fn node_delete_removes_raft_member_before_containers() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (state, etcd, remote) = join_test_state(&log);

        let record = NodeRecord {
            metadata: NodeMetadata {
                member_id: "m-1".into(),
                display_name: "node-5".into(),
                role: crate::etcd::dto::role::MANAGER.into(),
                node_ip: TARGET_IP.into(),
                api_endpoint: format!(
                    "http://{TARGET_IP}:19906/app/com.ictrek.vos-cluster-manager/"
                ),
                joined_at: "2026-10-30T00:00:00+00:00".into(),
            },
            credentials: NodeCredentials {
                admin_password: "pw123456".into(),
                pg_superuser_password: None,
                pg_replication_password: None,
                updated_at: "2026-10-30T00:00:00+00:00".into(),
            },
        };
        etcd.put_node_record("m-1", &record).await.unwrap();
        etcd.member_add(TARGET_PEER).await.unwrap();
        remote.preload_instances(TARGET_IP, "etcd", &["cluster-etcd-node-0"]);
        remote.preload_instances(TARGET_IP, "pg", &["cluster-pg-node-0"]);

        let out = node_delete(
            State(state),
            Path("m-1".to_string()),
            Query(std::collections::HashMap::new()),
        )
        .await
        .expect("node_delete 应成功");
        assert_eq!(out.0.code, 0);

        let ev = events(&log);
        let remove_member = pos(&ev, "etcd:member_remove");
        assert!(
            remove_member < pos(&ev, "remote:remove_pg"),
            "必须先摘 raft 成员再删 pg 容器（否则 quorum 丢失无法提交）: {ev:?}"
        );
        assert!(
            remove_member < pos(&ev, "remote:remove_etcd"),
            "必须先摘 raft 成员再删 etcd 容器: {ev:?}"
        );
        assert_eq!(etcd.member_count().await.unwrap(), 1, "成员表应回落为单节点");
        assert!(
            etcd.get_node_record("m-1").await.unwrap().is_none(),
            "节点记录应删除"
        );
    }

    /// 成员移除：member_remove 失败时不得继续删容器（否则永久失 quorum），也不删节点记录
    #[tokio::test]
    async fn node_delete_aborts_when_member_remove_fails() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (state, etcd, remote) = join_test_state(&log);

        let record = NodeRecord {
            metadata: NodeMetadata {
                member_id: "m-2".into(),
                display_name: "node-5".into(),
                role: crate::etcd::dto::role::MANAGER.into(),
                node_ip: TARGET_IP.into(),
                api_endpoint: format!(
                    "http://{TARGET_IP}:19906/app/com.ictrek.vos-cluster-manager/"
                ),
                joined_at: "2026-10-30T00:00:00+00:00".into(),
            },
            credentials: NodeCredentials {
                admin_password: "pw123456".into(),
                pg_superuser_password: None,
                pg_replication_password: None,
                updated_at: "2026-10-30T00:00:00+00:00".into(),
            },
        };
        etcd.put_node_record("m-2", &record).await.unwrap();
        etcd.member_add(TARGET_PEER).await.unwrap();
        remote.preload_instances(TARGET_IP, "etcd", &["cluster-etcd-node-0"]);

        let id = etcd
            .find_member_id_by_peer_url(TARGET_PEER)
            .await
            .unwrap()
            .unwrap();
        etcd.set_member_remove_failing(true);

        let err = node_delete(
            State(state),
            Path("m-2".to_string()),
            Query(std::collections::HashMap::new()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::ETCD_FAILED, "成员移除失败应 20504");
        assert!(
            etcd.find_member_id_by_peer_url(TARGET_PEER).await.unwrap() == Some(id),
            "成员未被摘除（失败即中止）"
        );
        let ev = events(&log);
        assert!(
            !ev.iter().any(|e| e == "remote:remove_etcd"),
            "member_remove 失败后不得删容器: {ev:?}"
        );
        assert!(
            etcd.get_node_record("m-2").await.unwrap().is_some(),
            "失败时不得删除节点记录（保证可重试）"
        );
    }
}
