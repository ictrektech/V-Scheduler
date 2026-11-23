//! 运行可观测性：run 记录、脚本日志、结构化数据协议与聚合、容器清单、保留策略
//!
//! - [`directive`]：`modelinfer:` 结构化输出协议解析与聚合（纯函数）
//! - [`store`]：落库与查询（run / 日志 / 节点数据 / 全局数据 / 容器 / 设置）
//!
//! 设计原则：一切解析「尽力而为」，非法数据不影响阶段成败；协议对新增字段 / 操作符保持
//! 向后兼容（未知即忽略）。详见 `openspec/changes/modelinfer-run-observability/design.md`。

pub mod directive;
pub mod step;
pub mod store;

use serde_json::Value;

use crate::error::ModelinferError;
pub use directive::{ContainerSpec, PREFIX, deep_merge, extract_containers};
pub use step::{is_stage_step, stage_of_step, stage_step_id, step_title};
pub use store::{
    ContainerView, GlobalDataView, NodeDataView, RecordScriptReq, RunLogView, RunStepView, RunView,
    Settings, SettingsDefaults, StepBrief, StepStartReq, kind, orchestrator, state, step_state,
};
use crate::ssh::SshOutput;

/// 阶段全局对象累加器（按节点执行顺序合并）
#[derive(Debug, Clone, Default)]
pub struct StageGlobal {
    /// 合并后的全局对象
    pub value: Value,
    /// 参与节点数
    pub node_count: i32,
}

impl StageGlobal {
    pub fn new() -> Self {
        Self {
            value: Value::Object(Default::default()),
            node_count: 0,
        }
    }

    /// 合并一个节点的全局贡献
    pub fn merge(&mut self, contribution: &Value) {
        self.node_count += 1;
        if contribution.as_object().map(|m| !m.is_empty()).unwrap_or(false) {
            deep_merge(&mut self.value, contribution);
        }
    }
}

/// 记录一次节点脚本执行并合并其全局贡献
pub async fn record_script(
    db: &sea_orm::DatabaseConnection,
    req: &RecordScriptReq<'_>,
    stage: &mut StageGlobal,
) -> Result<(), ModelinferError> {
    let contribution = store::record_script_output(db, req).await?;
    stage.merge(&contribution);
    Ok(())
}

/// 收尾阶段全局对象
pub async fn finalize_stage(
    db: &sea_orm::DatabaseConnection,
    run_id: &str,
    deployment_id: &str,
    kind: &str,
    stage: &str,
    global: &StageGlobal,
) -> Result<(), ModelinferError> {
    store::finalize_stage_global(
        db,
        run_id,
        deployment_id,
        kind,
        stage,
        &global.value,
        global.node_count,
    )
    .await
}

/// 读取保留策略设置
pub async fn settings(
    db: &sea_orm::DatabaseConnection,
    defaults: SettingsDefaults,
) -> Result<Settings, ModelinferError> {
    store::get_settings(db, defaults).await
}

/// 按保留策略裁剪某运行配置的历史 run
pub async fn prune(
    db: &sea_orm::DatabaseConnection,
    deployment_id: &str,
    defaults: SettingsDefaults,
) -> Result<u64, ModelinferError> {
    let s = settings(db, defaults).await?;
    store::prune_runs(db, deployment_id, s).await
}

/// 收尾 run 并按保留策略裁剪该运行配置的历史
pub async fn finish_and_prune(
    db: &sea_orm::DatabaseConnection,
    deployment_id: &str,
    defaults: SettingsDefaults,
    run_id: &str,
    run_state: &str,
    summary: &str,
    error: &str,
) -> Result<(), ModelinferError> {
    store::finish_run(db, run_id, run_state, summary, error).await?;
    if let Err(e) = prune(db, deployment_id, defaults).await {
        tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "裁剪运行历史失败（忽略）");
    }
    Ok(())
}

/// 记录脚本执行所需的最小节点信息
#[derive(Debug, Clone)]
pub struct NodeRef {
    pub id: String,
    pub name: String,
    pub host: String,
}

impl NodeRef {
    pub fn new(id: impl Into<String>, name: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            host: host.into(),
        }
    }
}

/// 便捷构造：根据执行上下文生成 `RecordScriptReq`
#[allow(clippy::too_many_arguments)]
pub fn script_req<'a>(
    run_id: &'a str,
    deployment_id: &'a str,
    kind: &'a str,
    stage: &'a str,
    node: &'a NodeRef,
    attempt: i32,
    output: &'a SshOutput,
    started_at: &'a str,
    finished_at: &'a str,
) -> RecordScriptReq<'a> {
    RecordScriptReq {
        run_id,
        deployment_id,
        kind,
        stage,
        node_id: &node.id,
        node_name: &node.name,
        node_host: &node.host,
        attempt,
        output,
        started_at,
        finished_at,
    }
}