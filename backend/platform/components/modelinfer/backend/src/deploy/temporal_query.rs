//! Temporal 工作流按需查询（**不落库**）
//!
//! 对由 Temporal 编排的运行配置，按需查询工作流执行描述（状态 / 当前与待执行 activity /
//! 最近失败等）。查询结果仅用于即时展示，MUST NOT 写入数据库（见设计 D-MI-RTL-04）。

use serde::Serialize;
use temporalio_client::{Client, UntypedWorkflow};
use temporalio_common::protos::temporal::api::enums::v1::WorkflowExecutionStatus;

use crate::error::{ModelinferError, codes};
use crate::time;

/// Temporal 工作流执行描述（即时查询结果）
#[derive(Debug, Clone, Serialize)]
pub struct TemporalWorkflowView {
    pub workflow_id: String,
    pub run_id: String,
    pub workflow_type: String,
    pub status: String,
    pub task_queue: String,
    pub history_length: usize,
    pub start_time: Option<String>,
    pub close_time: Option<String>,
    pub pending_activities: Vec<TemporalActivityView>,
}

/// Temporal 活动（当前 / 待执行）即时信息
#[derive(Debug, Clone, Serialize)]
pub struct TemporalActivityView {
    pub activity_id: String,
    pub activity_type: String,
    pub state: String,
    pub attempt: i32,
    pub scheduled_time: Option<String>,
    pub last_failure: String,
}

/// 工作流状态枚举 → 稳定字符串
fn status_str(s: WorkflowExecutionStatus) -> &'static str {
    use WorkflowExecutionStatus::*;
    match s {
        Unspecified => "unspecified",
        Running => "running",
        Completed => "completed",
        Failed => "failed",
        Canceled => "canceled",
        Terminated => "terminated",
        ContinuedAsNew => "continued-as-new",
        TimedOut => "timed-out",
        Paused => "paused",
    }
}

/// 待执行活动状态枚举 → 稳定字符串
fn pending_state_str(v: i32) -> String {
    use temporalio_common::protos::temporal::api::enums::v1::PendingActivityState;
    PendingActivityState::try_from(v)
        .map(|s| format!("{s:?}").to_lowercase())
        .unwrap_or_else(|_| "unspecified".to_string())
}

/// 按需查询某工作流的执行描述（不落库）
pub async fn describe_workflow(
    client: &Client,
    workflow_id: &str,
) -> Result<TemporalWorkflowView, ModelinferError> {
    let handle = client.get_workflow_handle::<UntypedWorkflow>(workflow_id.to_string());
    let desc = handle.describe(Default::default()).await.map_err(|e| {
        ModelinferError::new(
            codes::TEMPORAL_FAILED,
            format!("查询 Temporal 工作流失败: {e}"),
        )
    })?;

    let pending_activities = desc
        .raw()
        .pending_activities
        .iter()
        .map(|a| TemporalActivityView {
            activity_id: a.activity_id.clone(),
            activity_type: a
                .activity_type
                .as_ref()
                .map(|t| t.name.clone())
                .unwrap_or_default(),
            state: pending_state_str(a.state),
            attempt: a.attempt,
            scheduled_time: a.scheduled_time.as_ref().map(|ts| {
                use std::time::{Duration, UNIX_EPOCH};
                let secs = u64::try_from(ts.seconds).unwrap_or(0);
                let nanos = u32::try_from(ts.nanos).unwrap_or(0);
                time::system_time_to_iso(UNIX_EPOCH + Duration::new(secs, nanos))
            }),
            last_failure: a
                .last_failure
                .as_ref()
                .map(|f| f.message.clone())
                .unwrap_or_default(),
        })
        .collect();

    Ok(TemporalWorkflowView {
        workflow_id: desc.id().to_string(),
        run_id: desc.run_id().to_string(),
        workflow_type: desc.workflow_type().to_string(),
        status: status_str(desc.status()).to_string(),
        task_queue: desc.task_queue().to_string(),
        history_length: desc.history_length(),
        start_time: desc.start_time().map(time::system_time_to_iso),
        close_time: desc.close_time().map(time::system_time_to_iso),
        pending_activities,
    })
}