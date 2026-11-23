//! 运行可观测性落库与查询

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::Serialize;
use serde_json::Value;

use crate::entities::{
    deployment_container, run, run_global_data, run_log, run_node_data, run_step, setting,
};
use crate::error::{ModelinferError, codes};
use crate::observability::directive::{
    ContainerSpec, aggregate, extract_containers, extract_directives,
};
use crate::ssh::SshOutput;
use crate::time;

/// 单条脚本日志存储上限（字节）
pub const MAX_LOG_BYTES: usize = 256 * 1024;
/// 设置键：健康巡检保留次数
pub const KEY_HEALTH_KEEP: &str = "health_history_keep";
/// 设置键：启动 / 停止保留次数
pub const KEY_RUN_KEEP: &str = "run_history_keep";
/// 健康巡检保留次数上限
pub const MAX_HEALTH_KEEP: u32 = 100;
/// 启动 / 停止保留次数上限
pub const MAX_RUN_KEEP: u32 = 200;

/// 运行类型
pub mod kind {
    pub const START: &str = "start";
    pub const STOP: &str = "stop";
    pub const HEALTH: &str = "health";
    pub const ALL: [&str; 3] = [START, STOP, HEALTH];
}

/// 运行状态
pub mod state {
    pub const RUNNING: &str = "running";
    pub const SUCCEEDED: &str = "succeeded";
    pub const FAILED: &str = "failed";
}

/// 运行步骤状态
pub mod step_state {
    pub const RUNNING: &str = "running";
    pub const SUCCEEDED: &str = "succeeded";
    pub const FAILED: &str = "failed";
    pub const SKIPPED: &str = "skipped";
}

/// 步骤编排来源
pub mod orchestrator {
    pub const TEMPORAL: &str = "temporal";
    pub const SCHEDULER: &str = "scheduler";
}

/// 保留策略设置
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Settings {
    pub health_history_keep: u32,
    pub run_history_keep: u32,
}

/// 保留策略默认值（来自启动配置）
#[derive(Debug, Clone, Copy)]
pub struct SettingsDefaults {
    pub health_history_keep: u32,
    pub run_history_keep: u32,
}

impl Default for SettingsDefaults {
    fn default() -> Self {
        Self {
            health_history_keep: 10,
            run_history_keep: 20,
        }
    }
}

/// 运行视图
#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub id: String,
    pub deployment_id: String,
    pub kind: String,
    pub state: String,
    pub trigger: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub summary: String,
    pub error: String,
    /// 是否存在 Temporal 工作流（决定前端能否按需查询 Temporal 活动）
    pub has_temporal: bool,
    /// 当前正在执行的步骤（仅 running 步骤；无则 null）
    pub active_step: Option<StepBrief>,
}

impl RunView {
    /// 构造视图；`active_step` 由调用方（列表批量填充）决定
    pub fn new(m: run::Model, active_step: Option<StepBrief>) -> Self {
        let has_temporal = !m.workflow_id.is_empty();
        Self {
            id: m.id,
            deployment_id: m.deployment_id,
            kind: m.kind,
            state: m.state,
            trigger: m.trigger,
            started_at: m.started_at,
            finished_at: m.finished_at,
            summary: m.summary,
            error: m.error,
            has_temporal,
            active_step,
        }
    }
}

impl From<run::Model> for RunView {
    fn from(m: run::Model) -> Self {
        Self::new(m, None)
    }
}

/// 脚本日志视图
#[derive(Debug, Clone, Serialize)]
pub struct RunLogView {
    pub id: String,
    pub run_id: String,
    pub kind: String,
    pub stage: String,
    pub node_id: String,
    pub node_name: String,
    pub node_host: String,
    pub attempt: i32,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: i64,
    pub stderr_bytes: i64,
    pub truncated: bool,
    pub started_at: String,
    pub finished_at: String,
}

impl From<run_log::Model> for RunLogView {
    fn from(m: run_log::Model) -> Self {
        Self {
            id: m.id,
            run_id: m.run_id,
            kind: m.kind,
            stage: m.stage,
            node_id: m.node_id,
            node_name: m.node_name,
            node_host: m.node_host,
            attempt: m.attempt,
            exit_code: m.exit_code,
            stdout: m.stdout,
            stderr: m.stderr,
            stdout_bytes: m.stdout_bytes,
            stderr_bytes: m.stderr_bytes,
            truncated: m.truncated,
            started_at: m.started_at,
            finished_at: m.finished_at,
        }
    }
}

/// 节点结构化数据视图
#[derive(Debug, Clone, Serialize)]
pub struct NodeDataView {
    pub id: String,
    pub stage: String,
    pub node_id: String,
    pub node_name: String,
    pub node_host: String,
    pub data: Value,
    pub created_at: String,
}

/// 全局结构化数据视图
#[derive(Debug, Clone, Serialize)]
pub struct GlobalDataView {
    pub id: String,
    pub stage: String,
    pub data: Value,
    pub node_count: i32,
    pub created_at: String,
}

/// 容器视图
#[derive(Debug, Clone, Serialize)]
pub struct ContainerView {
    pub id: String,
    pub deployment_id: String,
    pub node_id: String,
    pub node_name: String,
    pub node_host: String,
    pub name: String,
    pub image: String,
    pub run_id: String,
    pub stage: String,
    pub updated_at: String,
}

/// 运行步骤视图
#[derive(Debug, Clone, Serialize)]
pub struct RunStepView {
    pub id: String,
    pub run_id: String,
    pub deployment_id: String,
    pub kind: String,
    pub step: String,
    pub stage: String,
    pub title: String,
    pub state: String,
    pub orchestrator: String,
    pub node_id: String,
    pub node_name: String,
    pub node_host: String,
    pub seq: i64,
    pub detail: Value,
    pub error: String,
    pub started_at: String,
    pub finished_at: Option<String>,
}

impl From<run_step::Model> for RunStepView {
    fn from(m: run_step::Model) -> Self {
        Self {
            id: m.id,
            run_id: m.run_id,
            deployment_id: m.deployment_id,
            kind: m.kind,
            step: m.step,
            stage: m.stage,
            title: m.title,
            state: m.state,
            orchestrator: m.orchestrator,
            node_id: m.node_id,
            node_name: m.node_name,
            node_host: m.node_host,
            seq: m.seq,
            detail: serde_json::from_str(&m.detail_json).unwrap_or(Value::Null),
            error: m.error,
            started_at: m.started_at,
            finished_at: m.finished_at,
        }
    }
}

/// 当前步骤摘要（运行列表 `active_step`）
#[derive(Debug, Clone, Serialize)]
pub struct StepBrief {
    pub step: String,
    pub stage: String,
    pub title: String,
    pub node_name: String,
    pub started_at: String,
}

/// 创建一个运行步骤所需信息
pub struct StepStartReq<'a> {
    pub run_id: &'a str,
    pub deployment_id: &'a str,
    pub kind: &'a str,
    pub step: &'a str,
    pub stage: &'a str,
    pub title: &'a str,
    pub orchestrator: &'a str,
    pub node_id: &'a str,
    pub node_name: &'a str,
    pub node_host: &'a str,
}

/// 记录脚本执行输出所需信息
pub struct RecordScriptReq<'a> {
    pub run_id: &'a str,
    pub deployment_id: &'a str,
    pub kind: &'a str,
    pub stage: &'a str,
    pub node_id: &'a str,
    pub node_name: &'a str,
    pub node_host: &'a str,
    pub attempt: i32,
    pub output: &'a SshOutput,
    pub started_at: &'a str,
    pub finished_at: &'a str,
}

fn db_err(e: sea_orm::DbErr) -> ModelinferError {
    ModelinferError::new(codes::DB_FAILED, format!("数据库操作失败: {e}"))
}

/// UTF-8 安全截断
pub fn truncate_utf8(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// 创建 run，返回 run_id
///
/// `workflow_id` 仅作 Temporal 按需查询句柄（健康巡检等无工作流的运行为空）。
pub async fn create_run(
    db: &DatabaseConnection,
    deployment_id: &str,
    kind: &str,
    trigger: &str,
    summary: &str,
    workflow_id: &str,
) -> Result<String, ModelinferError> {
    let now = time::now_iso();
    let id = ulid::Ulid::new().to_string();
    let am = run::ActiveModel {
        id: Set(id.clone()),
        deployment_id: Set(deployment_id.to_string()),
        kind: Set(kind.to_string()),
        state: Set(state::RUNNING.to_string()),
        trigger: Set(trigger.to_string()),
        workflow_id: Set(workflow_id.to_string()),
        started_at: Set(now.clone()),
        finished_at: Set(None),
        summary: Set(summary.to_string()),
        error: Set(String::new()),
        created_at: Set(now),
    };
    am.insert(db).await.map_err(db_err)?;
    Ok(id)
}

/// 收尾 run（幂等：不存在时忽略）
pub async fn finish_run(
    db: &DatabaseConnection,
    run_id: &str,
    run_state: &str,
    summary: &str,
    error: &str,
) -> Result<(), ModelinferError> {
    let Some(m) = run::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await
        .map_err(db_err)?
    else {
        return Ok(());
    };
    let mut am: run::ActiveModel = m.into();
    am.state = Set(run_state.to_string());
    am.summary = Set(summary.to_string());
    am.error = Set(error.to_string());
    am.finished_at = Set(Some(time::now_iso()));
    am.update(db).await.map_err(db_err)?;
    Ok(())
}

/// 创建一个运行步骤（`state=running`，`seq = 该 run 现有最大 seq + 1`），返回步骤 ID。
pub async fn start_step(
    db: &DatabaseConnection,
    req: &StepStartReq<'_>,
) -> Result<String, ModelinferError> {
    let now = time::now_iso();
    let max_seq = run_step::Entity::find()
        .filter(run_step::Column::RunId.eq(req.run_id))
        .order_by_desc(run_step::Column::Seq)
        .one(db)
        .await
        .map_err(db_err)?
        .map(|m| m.seq)
        .unwrap_or(0);
    let id = ulid::Ulid::new().to_string();
    let am = run_step::ActiveModel {
        id: Set(id.clone()),
        run_id: Set(req.run_id.to_string()),
        deployment_id: Set(req.deployment_id.to_string()),
        kind: Set(req.kind.to_string()),
        step: Set(req.step.to_string()),
        stage: Set(req.stage.to_string()),
        title: Set(req.title.to_string()),
        state: Set(step_state::RUNNING.to_string()),
        orchestrator: Set(req.orchestrator.to_string()),
        node_id: Set(req.node_id.to_string()),
        node_name: Set(req.node_name.to_string()),
        node_host: Set(req.node_host.to_string()),
        seq: Set(max_seq + 1),
        detail_json: Set(String::new()),
        error: Set(String::new()),
        started_at: Set(now.clone()),
        finished_at: Set(None),
        created_at: Set(now),
    };
    am.insert(db).await.map_err(db_err)?;
    Ok(id)
}

/// 收尾一个运行步骤（幂等：不存在时忽略）。`detail` 为可选结果 / 输入摘要。
pub async fn finish_step(
    db: &DatabaseConnection,
    step_id: &str,
    step_st: &str,
    detail: Option<&Value>,
    error: &str,
) -> Result<(), ModelinferError> {
    let Some(m) = run_step::Entity::find_by_id(step_id.to_string())
        .one(db)
        .await
        .map_err(db_err)?
    else {
        return Ok(());
    };
    let mut am: run_step::ActiveModel = m.into();
    am.state = Set(step_st.to_string());
    am.finished_at = Set(Some(time::now_iso()));
    if let Some(d) = detail {
        am.detail_json = Set(d.to_string());
    }
    am.error = Set(error.to_string());
    am.update(db).await.map_err(db_err)?;
    Ok(())
}

/// 某 run 的全部步骤（按 seq / 起止升序）
pub async fn list_steps(
    db: &DatabaseConnection,
    deployment_id: &str,
    run_id: &str,
) -> Result<Vec<RunStepView>, ModelinferError> {
    let rows = run_step::Entity::find()
        .filter(run_step::Column::RunId.eq(run_id))
        .filter(run_step::Column::DeploymentId.eq(deployment_id))
        .order_by_asc(run_step::Column::Seq)
        .order_by_asc(run_step::Column::StartedAt)
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows.into_iter().map(RunStepView::from).collect())
}

/// 批量获取若干 run 的当前 `running` 步骤（供运行列表 `active_step`，避免 N+1）。
pub async fn active_steps_of(
    db: &DatabaseConnection,
    deployment_id: &str,
    run_ids: &[String],
) -> Result<std::collections::HashMap<String, StepBrief>, ModelinferError> {
    use std::collections::HashMap;
    let mut out = HashMap::new();
    if run_ids.is_empty() {
        return Ok(out);
    }
    let rows = run_step::Entity::find()
        .filter(run_step::Column::DeploymentId.eq(deployment_id))
        .filter(run_step::Column::State.eq(step_state::RUNNING))
        .filter(run_step::Column::RunId.is_in(run_ids.iter().cloned()))
        .order_by_asc(run_step::Column::Seq)
        .all(db)
        .await
        .map_err(db_err)?;
    for m in rows {
        // 每个 run 取第一条（seq 最小）的 running 步骤
        out.entry(m.run_id.clone()).or_insert(StepBrief {
            step: m.step,
            stage: m.stage,
            title: m.title,
            node_name: m.node_name,
            started_at: m.started_at,
        });
    }
    Ok(out)
}

/// 将某 run 仍处于 `running` 的步骤置为失败（用于 run 被中断的收尾）
pub async fn fail_running_steps(
    db: &DatabaseConnection,
    run_id: &str,
    error: &str,
) -> Result<u64, ModelinferError> {
    let rows = run_step::Entity::find()
        .filter(run_step::Column::RunId.eq(run_id))
        .filter(run_step::Column::State.eq(step_state::RUNNING))
        .all(db)
        .await
        .map_err(db_err)?;
    let now = time::now_iso();
    let mut n = 0u64;
    for m in rows {
        let mut am: run_step::ActiveModel = m.into();
        am.state = Set(step_state::FAILED.to_string());
        am.error = Set(error.to_string());
        am.finished_at = Set(Some(now.clone()));
        am.update(db).await.map_err(db_err)?;
        n += 1;
    }
    Ok(n)
}

/// 记录一次脚本执行：写日志 + 解析结构化数据（节点对象 / 容器）+ 返回全局贡献
pub async fn record_script_output(
    db: &DatabaseConnection,
    req: &RecordScriptReq<'_>,
) -> Result<Value, ModelinferError> {
    let (stdout, out_truncated) = truncate_utf8(&req.output.stdout, MAX_LOG_BYTES);
    let (stderr, err_truncated) = truncate_utf8(&req.output.stderr, MAX_LOG_BYTES);
    let now = time::now_iso();
    let log = run_log::ActiveModel {
        id: Set(ulid::Ulid::new().to_string()),
        run_id: Set(req.run_id.to_string()),
        deployment_id: Set(req.deployment_id.to_string()),
        kind: Set(req.kind.to_string()),
        stage: Set(req.stage.to_string()),
        node_id: Set(req.node_id.to_string()),
        node_name: Set(req.node_name.to_string()),
        node_host: Set(req.node_host.to_string()),
        attempt: Set(req.attempt),
        exit_code: Set(req.output.exit_code),
        stdout: Set(stdout),
        stderr: Set(stderr),
        stdout_bytes: Set(req.output.stdout.len() as i64),
        stderr_bytes: Set(req.output.stderr.len() as i64),
        truncated: Set(out_truncated || err_truncated),
        started_at: Set(req.started_at.to_string()),
        finished_at: Set(req.finished_at.to_string()),
        created_at: Set(now),
    };
    log.insert(db).await.map_err(db_err)?;

    let directives = extract_directives(&req.output.stdout);
    let agg = aggregate(&directives);
    let node_empty = agg.node.as_object().map(|m| m.is_empty()).unwrap_or(true);
    if !node_empty {
        let data = run_node_data::ActiveModel {
            id: Set(ulid::Ulid::new().to_string()),
            run_id: Set(req.run_id.to_string()),
            deployment_id: Set(req.deployment_id.to_string()),
            kind: Set(req.kind.to_string()),
            stage: Set(req.stage.to_string()),
            node_id: Set(req.node_id.to_string()),
            node_name: Set(req.node_name.to_string()),
            node_host: Set(req.node_host.to_string()),
            data_json: Set(agg.node.to_string()),
            created_at: Set(time::now_iso()),
        };
        data.insert(db).await.map_err(db_err)?;
    }
    let containers = extract_containers(&agg.node);
    if !containers.is_empty() {
        upsert_containers(
            db,
            req.deployment_id,
            req.run_id,
            req.stage,
            req.node_id,
            req.node_name,
            req.node_host,
            &containers,
        )
        .await?;
    }
    Ok(agg.global_patch)
}

/// 写入阶段全局对象（无贡献时不写）
pub async fn finalize_stage_global(
    db: &DatabaseConnection,
    run_id: &str,
    deployment_id: &str,
    kind: &str,
    stage: &str,
    global: &Value,
    node_count: i32,
) -> Result<(), ModelinferError> {
    let empty = global.as_object().map(|m| m.is_empty()).unwrap_or(true);
    if empty {
        return Ok(());
    }
    let am = run_global_data::ActiveModel {
        id: Set(ulid::Ulid::new().to_string()),
        run_id: Set(run_id.to_string()),
        deployment_id: Set(deployment_id.to_string()),
        kind: Set(kind.to_string()),
        stage: Set(stage.to_string()),
        global_json: Set(global.to_string()),
        node_count: Set(node_count),
        created_at: Set(time::now_iso()),
    };
    am.insert(db).await.map_err(db_err)?;
    Ok(())
}

/// 容器清单 upsert（应用级唯一：运行配置 + 节点 + 名称）
#[allow(clippy::too_many_arguments)]
pub async fn upsert_containers(
    db: &DatabaseConnection,
    deployment_id: &str,
    run_id: &str,
    stage: &str,
    node_id: &str,
    node_name: &str,
    node_host: &str,
    containers: &[ContainerSpec],
) -> Result<(), ModelinferError> {
    let now = time::now_iso();
    for c in containers {
        let existing = deployment_container::Entity::find()
            .filter(deployment_container::Column::DeploymentId.eq(deployment_id))
            .filter(deployment_container::Column::NodeId.eq(node_id))
            .filter(deployment_container::Column::Name.eq(c.name.as_str()))
            .one(db)
            .await
            .map_err(db_err)?;
        match existing {
            Some(m) => {
                let mut am: deployment_container::ActiveModel = m.into();
                am.node_name = Set(node_name.to_string());
                am.node_host = Set(node_host.to_string());
                am.image = Set(c.image.clone());
                am.run_id = Set(run_id.to_string());
                am.stage = Set(stage.to_string());
                am.updated_at = Set(now.clone());
                am.update(db).await.map_err(db_err)?;
            }
            None => {
                let am = deployment_container::ActiveModel {
                    id: Set(ulid::Ulid::new().to_string()),
                    deployment_id: Set(deployment_id.to_string()),
                    node_id: Set(node_id.to_string()),
                    node_name: Set(node_name.to_string()),
                    node_host: Set(node_host.to_string()),
                    name: Set(c.name.clone()),
                    image: Set(c.image.clone()),
                    run_id: Set(run_id.to_string()),
                    stage: Set(stage.to_string()),
                    created_at: Set(now.clone()),
                    updated_at: Set(now.clone()),
                };
                am.insert(db).await.map_err(db_err)?;
            }
        }
    }
    Ok(())
}

/// 清除某运行配置的全部容器清单（停止清理后）
pub async fn clear_containers(
    db: &DatabaseConnection,
    deployment_id: &str,
) -> Result<(), ModelinferError> {
    deployment_container::Entity::delete_many()
        .filter(deployment_container::Column::DeploymentId.eq(deployment_id))
        .exec(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// 运行列表（按开始时间倒序）
pub async fn list_runs(
    db: &DatabaseConnection,
    deployment_id: &str,
    kind: Option<&str>,
    limit: Option<u64>,
) -> Result<Vec<RunView>, ModelinferError> {
    let mut q = run::Entity::find()
        .filter(run::Column::DeploymentId.eq(deployment_id))
        .order_by_desc(run::Column::StartedAt);
    if let Some(k) = kind {
        q = q.filter(run::Column::Kind.eq(k));
    }
    if let Some(l) = limit {
        q = q.limit(l);
    }
    let rows = q.all(db).await.map_err(db_err)?;
    let run_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let active = active_steps_of(db, deployment_id, &run_ids).await?;
    Ok(rows
        .into_iter()
        .map(|m| {
            let a = active.get(&m.id).cloned();
            RunView::new(m, a)
        })
        .collect())
}

/// 运行详情（校验归属）
pub async fn get_run(
    db: &DatabaseConnection,
    deployment_id: &str,
    run_id: &str,
) -> Result<RunView, ModelinferError> {
    let m = run::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await
        .map_err(db_err)?;
    match m {
        Some(m) if m.deployment_id == deployment_id => Ok(RunView::from(m)),
        _ => Err(ModelinferError::new(codes::NOT_FOUND, "运行记录不存在")),
    }
}

/// 某 run 的 Temporal 工作流 id（校验归属；无则 None）
pub async fn run_workflow_id(
    db: &DatabaseConnection,
    deployment_id: &str,
    run_id: &str,
) -> Result<Option<String>, ModelinferError> {
    let m = run::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await
        .map_err(db_err)?;
    match m {
        Some(m) if m.deployment_id == deployment_id => {
            Ok((!m.workflow_id.trim().is_empty()).then_some(m.workflow_id))
        }
        _ => Err(ModelinferError::new(codes::NOT_FOUND, "运行记录不存在")),
    }
}

/// 脚本日志（按执行时间升序）
pub async fn list_run_logs(
    db: &DatabaseConnection,
    deployment_id: &str,
    run_id: &str,
    stage: Option<&str>,
    node_id: Option<&str>,
) -> Result<Vec<RunLogView>, ModelinferError> {
    let mut q = run_log::Entity::find()
        .filter(run_log::Column::RunId.eq(run_id))
        .filter(run_log::Column::DeploymentId.eq(deployment_id))
        .order_by_asc(run_log::Column::StartedAt)
        .order_by_asc(run_log::Column::Attempt);
    if let Some(s) = stage {
        q = q.filter(run_log::Column::Stage.eq(s));
    }
    if let Some(n) = node_id {
        q = q.filter(run_log::Column::NodeId.eq(n));
    }
    let rows = q.all(db).await.map_err(db_err)?;
    Ok(rows.into_iter().map(RunLogView::from).collect())
}

/// 节点结构化数据
pub async fn list_node_data(
    db: &DatabaseConnection,
    deployment_id: &str,
    run_id: &str,
    stage: Option<&str>,
) -> Result<Vec<NodeDataView>, ModelinferError> {
    let mut q = run_node_data::Entity::find()
        .filter(run_node_data::Column::RunId.eq(run_id))
        .filter(run_node_data::Column::DeploymentId.eq(deployment_id))
        .order_by_asc(run_node_data::Column::CreatedAt);
    if let Some(s) = stage {
        q = q.filter(run_node_data::Column::Stage.eq(s));
    }
    let rows = q.all(db).await.map_err(db_err)?;
    Ok(rows
        .into_iter()
        .map(|m| NodeDataView {
            id: m.id,
            stage: m.stage,
            node_id: m.node_id,
            node_name: m.node_name,
            node_host: m.node_host,
            data: serde_json::from_str(&m.data_json).unwrap_or(Value::Null),
            created_at: m.created_at,
        })
        .collect())
}

/// 全局结构化数据
pub async fn list_global_data(
    db: &DatabaseConnection,
    deployment_id: &str,
    run_id: &str,
) -> Result<Vec<GlobalDataView>, ModelinferError> {
    let rows = run_global_data::Entity::find()
        .filter(run_global_data::Column::RunId.eq(run_id))
        .filter(run_global_data::Column::DeploymentId.eq(deployment_id))
        .order_by_asc(run_global_data::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .map(|m| GlobalDataView {
            id: m.id,
            stage: m.stage,
            data: serde_json::from_str(&m.global_json).unwrap_or(Value::Null),
            node_count: m.node_count,
            created_at: m.created_at,
        })
        .collect())
}

/// 最近一次成功启动 run 的 `start` 阶段全局对象中的 `serve` 子对象（接入配置脚本回传）
///
/// 按启动时间倒序遍历成功（`state=succeeded`）的 `kind=start` run，返回首个含对象型
/// `serve` 的阶段全局数据；无则返回 `None`。
pub async fn latest_serve_global(
    db: &DatabaseConnection,
    deployment_id: &str,
) -> Result<Option<Value>, ModelinferError> {
    let runs = run::Entity::find()
        .filter(run::Column::DeploymentId.eq(deployment_id))
        .filter(run::Column::Kind.eq(kind::START))
        .filter(run::Column::State.eq(state::SUCCEEDED))
        .order_by_desc(run::Column::StartedAt)
        .all(db)
        .await
        .map_err(db_err)?;
    for r in runs {
        let row = run_global_data::Entity::find()
            .filter(run_global_data::Column::RunId.eq(r.id.as_str()))
            .filter(run_global_data::Column::Stage.eq("start"))
            .one(db)
            .await
            .map_err(db_err)?;
        if let Some(m) = row {
            let v: Value = serde_json::from_str(&m.global_json).unwrap_or(Value::Null);
            if let Some(serve) = v.get("serve").filter(|s| s.is_object()) {
                return Ok(Some(serve.clone()));
            }
        }
    }
    Ok(None)
}

/// 读取某 run 指定阶段的阶段全局对象（跨节点合并；无则 `None`）
pub async fn global_of_stage(
    db: &DatabaseConnection,
    deployment_id: &str,
    run_id: &str,
    stage: &str,
) -> Result<Option<Value>, ModelinferError> {
    let row = run_global_data::Entity::find()
        .filter(run_global_data::Column::RunId.eq(run_id))
        .filter(run_global_data::Column::DeploymentId.eq(deployment_id))
        .filter(run_global_data::Column::Stage.eq(stage))
        .one(db)
        .await
        .map_err(db_err)?;
    Ok(row.map(|m| serde_json::from_str(&m.global_json).unwrap_or(Value::Null)))
}

/// 容器清单（按节点、名称排序）
pub async fn list_containers(
    db: &DatabaseConnection,
    deployment_id: &str,
) -> Result<Vec<ContainerView>, ModelinferError> {
    let rows = deployment_container::Entity::find()
        .filter(deployment_container::Column::DeploymentId.eq(deployment_id))
        .order_by_asc(deployment_container::Column::NodeName)
        .order_by_asc(deployment_container::Column::Name)
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .map(|m| ContainerView {
            id: m.id,
            deployment_id: m.deployment_id,
            node_id: m.node_id,
            node_name: m.node_name,
            node_host: m.node_host,
            name: m.name,
            image: m.image,
            run_id: m.run_id,
            stage: m.stage,
            updated_at: m.updated_at,
        })
        .collect())
}

/// 按保留策略裁剪某运行配置的历史 run（连同日志 / 结构化数据）
pub async fn prune_runs(
    db: &DatabaseConnection,
    deployment_id: &str,
    settings: Settings,
) -> Result<u64, ModelinferError> {
    let mut removed = 0u64;
    for (kind, keep) in [
        (kind::HEALTH, settings.health_history_keep as u64),
        (kind::START, settings.run_history_keep as u64),
        (kind::STOP, settings.run_history_keep as u64),
    ] {
        let rows = run::Entity::find()
            .filter(run::Column::DeploymentId.eq(deployment_id))
            .filter(run::Column::Kind.eq(kind))
            .order_by_desc(run::Column::StartedAt)
            .all(db)
            .await
            .map_err(db_err)?;
        let excess: Vec<String> = rows.into_iter().skip(keep as usize).map(|r| r.id).collect();
        for run_id in &excess {
            run_log::Entity::delete_many()
                .filter(run_log::Column::RunId.eq(run_id.as_str()))
                .exec(db)
                .await
                .map_err(db_err)?;
            run_node_data::Entity::delete_many()
                .filter(run_node_data::Column::RunId.eq(run_id.as_str()))
                .exec(db)
                .await
                .map_err(db_err)?;
            run_global_data::Entity::delete_many()
                .filter(run_global_data::Column::RunId.eq(run_id.as_str()))
                .exec(db)
                .await
                .map_err(db_err)?;
            run_step::Entity::delete_many()
                .filter(run_step::Column::RunId.eq(run_id.as_str()))
                .exec(db)
                .await
                .map_err(db_err)?;
            run::Entity::delete_by_id(run_id.clone())
                .exec(db)
                .await
                .map_err(db_err)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// 将其它仍处于 `running` 的 run 标记为失败（例如被停止清理中断的启动 run）
pub async fn fail_stale_runs(
    db: &DatabaseConnection,
    deployment_id: &str,
    except_run_id: &str,
    summary: &str,
) -> Result<u64, ModelinferError> {
    let rows = run::Entity::find()
        .filter(run::Column::DeploymentId.eq(deployment_id))
        .filter(run::Column::State.eq(state::RUNNING))
        .all(db)
        .await
        .map_err(db_err)?;
    let mut n = 0u64;
    for m in rows {
        if m.id == except_run_id {
            continue;
        }
        finish_run(db, &m.id, state::FAILED, summary, summary).await?;
        if let Err(e) = fail_running_steps(db, &m.id, summary).await {
            tracing::warn!(target: "modelinfer_observability", error = %e.msg, "收尾中断 run 的悬挂步骤失败（忽略）");
        }
        n += 1;
    }
    Ok(n)
}

/// 归一保留数（0 / 非法 → 默认；超上限 → 上限）
pub fn normalize_keep(raw: u32, default: u32, max: u32) -> u32 {
    if raw == 0 { default.min(max).max(1) } else { raw.min(max).max(1) }
}

/// 读取设置（DB 值优先，回退默认）
pub async fn get_settings(
    db: &DatabaseConnection,
    defaults: SettingsDefaults,
) -> Result<Settings, ModelinferError> {
    let rows = setting::Entity::find().all(db).await.map_err(db_err)?;
    let find = |key: &str| {
        rows.iter()
            .find(|r| r.key == key)
            .and_then(|r| r.value.trim().parse::<u32>().ok())
    };
    Ok(Settings {
        health_history_keep: normalize_keep(
            find(KEY_HEALTH_KEEP).unwrap_or(defaults.health_history_keep),
            defaults.health_history_keep,
            MAX_HEALTH_KEEP,
        ),
        run_history_keep: normalize_keep(
            find(KEY_RUN_KEEP).unwrap_or(defaults.run_history_keep),
            defaults.run_history_keep,
            MAX_RUN_KEEP,
        ),
    })
}

/// 写入设置（校验范围）
pub async fn put_settings(db: &DatabaseConnection, s: Settings) -> Result<Settings, ModelinferError> {
    if s.health_history_keep < 1 || s.health_history_keep > MAX_HEALTH_KEEP {
        return Err(ModelinferError::new(
            codes::PARAM_INVALID,
            format!("健康巡检保留次数必须在 1..={MAX_HEALTH_KEEP} 之间"),
        ));
    }
    if s.run_history_keep < 1 || s.run_history_keep > MAX_RUN_KEEP {
        return Err(ModelinferError::new(
            codes::PARAM_INVALID,
            format!("运行记录保留次数必须在 1..={MAX_RUN_KEEP} 之间"),
        ));
    }
    for (key, value) in [
        (KEY_HEALTH_KEEP, s.health_history_keep),
        (KEY_RUN_KEEP, s.run_history_keep),
    ] {
        upsert_setting(db, key, &value.to_string()).await?;
    }
    Ok(s)
}

async fn upsert_setting(
    db: &DatabaseConnection,
    key: &str,
    value: &str,
) -> Result<(), ModelinferError> {
    let existing = setting::Entity::find_by_id(key.to_string())
        .one(db)
        .await
        .map_err(db_err)?;
    match existing {
        Some(m) => {
            let mut am: setting::ActiveModel = m.into();
            am.value = Set(value.to_string());
            am.updated_at = Set(time::now_iso());
            am.update(db).await.map_err(db_err)?;
        }
        None => {
            let am = setting::ActiveModel {
                key: Set(key.to_string()),
                value: Set(value.to_string()),
                updated_at: Set(time::now_iso()),
            };
            am.insert(db).await.map_err(db_err)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ssh_out(exit_code: i32, stdout: &str) -> SshOutput {
        SshOutput {
            exit_code,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    async fn db() -> DatabaseConnection {
        crate::db::connect("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn run_lifecycle_and_logs() {
        let db = db().await;
        let run_id = create_run(&db, "d1", kind::START, "manual", "准备", "").await.unwrap();
        let out = ssh_out(0, "modelinfer:rank=1\nmodelinfer:{\"global\":{\"m\":\"x\"}}\n");
        let global = record_script_output(
            &db,
            &RecordScriptReq {
                run_id: &run_id,
                deployment_id: "d1",
                kind: kind::START,
                stage: "start",
                node_id: "n1",
                node_name: "n1",
                node_host: "10.0.0.1",
                attempt: 1,
                output: &out,
                started_at: "2026-01-01T00:00:00+08:00",
                finished_at: "2026-01-01T00:00:01+08:00",
            },
        )
        .await
        .unwrap();
        assert_eq!(global, json!({"m":"x"}));
        finalize_stage_global(&db, &run_id, "d1", kind::START, "start", &global, 1)
            .await
            .unwrap();
        finish_run(&db, &run_id, state::SUCCEEDED, "完成", "").await.unwrap();

        let runs = list_runs(&db, "d1", None, None).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, state::SUCCEEDED);

        let logs = list_run_logs(&db, "d1", &run_id, None, None).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].exit_code, 0);
        let node = list_node_data(&db, "d1", &run_id, None).await.unwrap();
        assert_eq!(node.len(), 1);
        assert_eq!(node[0].data, json!({"rank":"1"}));
        let g = list_global_data(&db, "d1", &run_id).await.unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].data, json!({"m":"x"}));
    }

    #[tokio::test]
    async fn run_ownership_enforced() {
        let db = db().await;
        let run_id = create_run(&db, "d1", kind::START, "manual", "", "").await.unwrap();
        assert!(get_run(&db, "d1", &run_id).await.is_ok());
        let err = get_run(&db, "d2", &run_id).await.unwrap_err();
        assert_eq!(err.code, codes::NOT_FOUND);
    }

    #[tokio::test]
    async fn truncates_large_output() {
        let db = db().await;
        let run_id = create_run(&db, "d1", kind::HEALTH, "scheduler", "", "").await.unwrap();
        let big = "x".repeat(MAX_LOG_BYTES + 10);
        let out = ssh_out(0, &big);
        record_script_output(
            &db,
            &RecordScriptReq {
                run_id: &run_id,
                deployment_id: "d1",
                kind: kind::HEALTH,
                stage: "health-probe",
                node_id: "n1",
                node_name: "n1",
                node_host: "h",
                attempt: 1,
                output: &out,
                started_at: "t",
                finished_at: "t",
            },
        )
        .await
        .unwrap();
        let logs = list_run_logs(&db, "d1", &run_id, None, None).await.unwrap();
        assert!(logs[0].truncated);
        assert_eq!(logs[0].stdout_bytes, (MAX_LOG_BYTES + 10) as i64);
        assert!(logs[0].stdout.len() <= MAX_LOG_BYTES);
    }

    #[tokio::test]
    async fn containers_upsert_and_clear() {
        let db = db().await;
        let run_id = create_run(&db, "d1", kind::START, "manual", "", "").await.unwrap();
        let out = ssh_out(0, r#"modelinfer:{"containers":[{"name":"a","image":"i"}]}"#);
        record_script_output(
            &db,
            &RecordScriptReq {
                run_id: &run_id,
                deployment_id: "d1",
                kind: kind::START,
                stage: "start",
                node_id: "n1",
                node_name: "n1",
                node_host: "h",
                attempt: 1,
                output: &out,
                started_at: "t",
                finished_at: "t",
            },
        )
        .await
        .unwrap();
        assert_eq!(list_containers(&db, "d1").await.unwrap().len(), 1);
        // 再次上报同名（更新而非重复）
        record_script_output(
            &db,
            &RecordScriptReq {
                run_id: &run_id,
                deployment_id: "d1",
                kind: kind::START,
                stage: "start",
                node_id: "n1",
                node_name: "n1",
                node_host: "h",
                attempt: 1,
                output: &out,
                started_at: "t",
                finished_at: "t",
            },
        )
        .await
        .unwrap();
        assert_eq!(list_containers(&db, "d1").await.unwrap().len(), 1);
        clear_containers(&db, "d1").await.unwrap();
        assert!(list_containers(&db, "d1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn prune_keeps_latest_n_and_cascades() {
        let db = db().await;
        let mut ids = Vec::new();
        for i in 0..5 {
            let run_id = create_run(&db, "d1", kind::HEALTH, "scheduler", "", "").await.unwrap();
            // started_at 使用递增时间确保顺序
            let m = run::Entity::find_by_id(run_id.clone()).one(&db).await.unwrap().unwrap();
            let mut am: run::ActiveModel = m.into();
            am.started_at = Set(format!("2026-01-01T00:00:0{i}+08:00"));
            am.update(&db).await.unwrap();
            let out = ssh_out(0, "modelinfer:x=1\n");
            record_script_output(
                &db,
                &RecordScriptReq {
                    run_id: &run_id,
                    deployment_id: "d1",
                    kind: kind::HEALTH,
                    stage: "health-probe",
                    node_id: "n1",
                    node_name: "n1",
                    node_host: "h",
                    attempt: 1,
                    output: &out,
                    started_at: "t",
                    finished_at: "t",
                },
            )
            .await
            .unwrap();
            // 一并创建一条运行步骤，验证裁剪级联
            start_step(
                &db,
                &StepStartReq {
                    run_id: &run_id,
                    deployment_id: "d1",
                    kind: kind::HEALTH,
                    step: "stage:health-probe",
                    stage: "health-probe",
                    title: "执行阶段 health-probe",
                    orchestrator: orchestrator::SCHEDULER,
                    node_id: "",
                    node_name: "",
                    node_host: "",
                },
            )
            .await
            .unwrap();
            ids.push(run_id);
        }
        let settings = Settings { health_history_keep: 2, run_history_keep: 20 };
        let removed = prune_runs(&db, "d1", settings).await.unwrap();
        assert_eq!(removed, 3);
        let runs = list_runs(&db, "d1", Some(kind::HEALTH), None).await.unwrap();
        assert_eq!(runs.len(), 2);
        // 最早 3 个的日志被级联删除
        for old in &ids[..3] {
            assert!(list_run_logs(&db, "d1", old, None, None).await.unwrap().is_empty());
            assert!(list_steps(&db, "d1", old).await.unwrap().is_empty());
        }
        // 保留的 2 个步骤仍在
        assert_eq!(list_steps(&db, "d1", &ids[4]).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn steps_lifecycle_seq_and_active() {
        let db = db().await;
        let run_id = create_run(&db, "d1", kind::START, "manual", "启动", "modelinfer-deploy-d1")
            .await
            .unwrap();
        let s1 = start_step(
            &db,
            &StepStartReq {
                run_id: &run_id,
                deployment_id: "d1",
                kind: kind::START,
                step: "prepare",
                stage: "",
                title: "准备部署",
                orchestrator: orchestrator::TEMPORAL,
                node_id: "",
                node_name: "",
                node_host: "",
            },
        )
        .await
        .unwrap();
        let _s2 = start_step(
            &db,
            &StepStartReq {
                run_id: &run_id,
                deployment_id: "d1",
                kind: kind::START,
                step: "check-node",
                stage: "",
                title: "节点连通性检查",
                orchestrator: orchestrator::TEMPORAL,
                node_id: "n1",
                node_name: "n1",
                node_host: "10.0.0.1",
            },
        )
        .await
        .unwrap();
        finish_step(&db, &s1, step_state::SUCCEEDED, Some(&json!({"nodes": 2})), "")
            .await
            .unwrap();

        let steps = list_steps(&db, "d1", &run_id).await.unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].seq, 1);
        assert_eq!(steps[1].seq, 2);
        assert_eq!(steps[0].state, step_state::SUCCEEDED);
        assert_eq!(steps[0].detail, json!({"nodes": 2}));
        assert!(steps[0].finished_at.is_some());
        assert_eq!(steps[1].state, step_state::RUNNING);
        assert_eq!(steps[1].node_id, "n1");
        assert!(steps[1].finished_at.is_none());

        let runs = list_runs(&db, "d1", None, None).await.unwrap();
        assert!(runs[0].has_temporal);
        let active = runs[0].active_step.as_ref().expect("当前步骤");
        assert_eq!(active.step, "check-node");
    }

    #[tokio::test]
    async fn fail_stale_runs_fails_hanging_steps() {
        let db = db().await;
        let interrupted = create_run(&db, "d1", kind::START, "manual", "启动", "")
            .await
            .unwrap();
        let keep = create_run(&db, "d1", kind::STOP, "auto", "停止", "")
            .await
            .unwrap();
        let sid = start_step(
            &db,
            &StepStartReq {
                run_id: &interrupted,
                deployment_id: "d1",
                kind: kind::START,
                step: "stage:start",
                stage: "start",
                title: "执行阶段 start",
                orchestrator: orchestrator::TEMPORAL,
                node_id: "",
                node_name: "",
                node_host: "",
            },
        )
        .await
        .unwrap();
        assert_eq!(sid.len(), 26);

        fail_stale_runs(&db, "d1", &keep, "被停止清理中断").await.unwrap();
        let steps = list_steps(&db, "d1", &interrupted).await.unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].state, step_state::FAILED);
        assert_eq!(steps[0].error, "被停止清理中断");
        assert!(steps[0].finished_at.is_some());
    }

    #[tokio::test]
    async fn settings_read_write_and_fallback() {
        let db = db().await;
        let defaults = SettingsDefaults { health_history_keep: 10, run_history_keep: 20 };
        assert_eq!(get_settings(&db, defaults).await.unwrap().health_history_keep, 10);
        let saved = put_settings(
            &db,
            Settings { health_history_keep: 5, run_history_keep: 30 },
        )
        .await
        .unwrap();
        assert_eq!(saved.health_history_keep, 5);
        let got = get_settings(&db, defaults).await.unwrap();
        assert_eq!(got.health_history_keep, 5);
        assert_eq!(got.run_history_keep, 30);
        // 非法值拒绝
        let err = put_settings(&db, Settings { health_history_keep: 0, run_history_keep: 20 })
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::PARAM_INVALID);
    }

    #[test]
    fn normalize_keep_bounds() {
        assert_eq!(normalize_keep(0, 10, 100), 10);
        assert_eq!(normalize_keep(5, 10, 100), 5);
        assert_eq!(normalize_keep(999, 10, 100), 100);
    }

    #[test]
    fn truncate_boundaries() {
        assert_eq!(truncate_utf8("abc", 5), ("abc".to_string(), false));
        let (s, t) = truncate_utf8("中文测试", 5);
        assert!(t);
        assert!(s.len() <= 5);
        assert!(s.starts_with('中'));
    }
}