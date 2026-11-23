//! Demo workflow / activity（temporalio-sdk 0.4.0，VMS 同款 struct + workflow_methods 模式）
//!
//! 用于验证本应用 worker ↔ 平台 temporal 通道：Client start `echo_workflow` →
//! worker 调度 `echo_activity` → 返回原始输入（Phase 4 编排工作流挂载同型结构）。

use std::sync::Arc;

use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::{ActivityOptions, WorkflowContext, WorkflowContextView, WorkflowResult};

/// Echo activities 集合（0.4.0 以 `#[activities]` struct 提供 ActivityImplementer）
#[derive(Clone)]
pub struct EchoActivities;

/// 构造 activities 实例（worker 注册用）
pub fn echo_activities() -> EchoActivities {
    EchoActivities
}

#[activities]
impl EchoActivities {
    /// 回显 activity
    #[activity]
    pub async fn echo_activity(
        self: Arc<Self>,
        _ctx: ActivityContext,
        input: String,
    ) -> Result<String, ActivityError> {
        Ok(input)
    }
}

/// Echo workflow：输入回显（通道连通验证）
#[workflow]
pub struct EchoWorkflow;

#[workflow_methods]
impl EchoWorkflow {
    /// 初始化（0.4.0 workflow 需 state/init）
    #[init]
    fn new(_ctx: &WorkflowContextView) -> Self {
        Self
    }

    /// 运行：start_activity 调 echo_activity（start_to_close 60s）
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        input: String,
    ) -> WorkflowResult<String> {
        let echoed = ctx
            .start_activity(
                EchoActivities::echo_activity,
                input,
                ActivityOptions::start_to_close_timeout(std::time::Duration::from_secs(60)),
            )
            .await?;
        Ok(echoed)
    }
}