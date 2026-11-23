//! 运行配置生命周期控制：启动 / 停止工作流下发的**唯一入口**
//!
//! 背景：手动启停（HTTP handler）与探活失败自动重启（进程级调度器）必须走**完全同源**的下发
//! 路径，否则自动重启很容易在无意中绕过状态置位顺序或在途启动终止逻辑。本模块把
//! 「下发启动 / 停止」抽成 [`Control`] trait：
//!
//! - [`TemporalControl`]：生产实现（Temporal 客户端 + task queue），handler 与
//!   [`crate::deploy::autorestart`] 共用；
//! - [`MockControl`]：单测实现（记录调用、可注入失败），使自动重启状态机可在无 Temporal、
//!   无真实节点的情况下测试。
//!
//! 设计 SSOT：`specs/docs/design/modelinfer/auto-restart.md`（执行路径见 §5）。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use temporalio_client::Client;

use crate::deploy::activities::{MarkStateReq, apply_state};
use crate::deploy::runtime::{start_workflow, terminate_workflow};
use crate::deployments;
use crate::error::{ModelinferError, codes};

/// 启动工作流类型名
pub const START_WORKFLOW_TYPE: &str = "DeploymentStartWorkflow";
/// 停止工作流类型名
pub const STOP_WORKFLOW_TYPE: &str = "DeploymentStopWorkflow";
/// 启动工作流 id 前缀（每个运行配置一个固定 id，重复启动复用）
pub const START_WORKFLOW_ID_PREFIX: &str = "modelinfer-deploy-";
/// 启动来源：用户手动
pub const TRIGGER_MANUAL: &str = "manual";
/// 启动来源：探活失败自动重启
pub const TRIGGER_AUTO_RESTART: &str = "auto-restart";

/// 生命周期控制抽象（启动 / 停止工作流下发）
#[async_trait]
pub trait Control: Send + Sync {
    /// 下发启动
    ///
    /// 节点可被多个运行配置共享：MUST NOT 因节点被其他活跃配置引用而拒绝。仅当自身已活跃
    /// 时返回 `20719`。MUST 先置 `starting` 意图、再启动 `DeploymentStartWorkflow`
    /// （`trigger` 进入运行记录）。
    async fn start(
        &self,
        db: &DatabaseConnection,
        deployment_id: &str,
        trigger: &str,
    ) -> Result<(), ModelinferError>;

    /// 下发停止
    ///
    /// `force = true` 时跳过 `stopped` / `stopping` 幂等守卫（清除异常残留）。
    /// MUST 先 best-effort 终止在途启动工作流，再启动独立的停止工作流。
    async fn stop(
        &self,
        deployment_id: &str,
        force: bool,
        reason: &str,
    ) -> Result<(), ModelinferError>;
}

/// 启动 / 停止工作流 id（供 handler 与调度器共用，避免字符串漂移）
pub fn start_workflow_id(deployment_id: &str) -> String {
    format!("{START_WORKFLOW_ID_PREFIX}{deployment_id}")
}

/// 是否应下发停止清理工作流（纯函数，便于单测）。
///
/// - 正常停止：仅对未处于 `stopped` / `stopping` 的部署生效（幂等无操作）。
/// - 强制停止（`force = true`）：无条件下发，用于清除异常状态。
pub fn stop_should_launch(state: &str, force: bool) -> bool {
    force || !matches!(state, "stopped" | "stopping")
}

/// Temporal 实现（生产）
pub struct TemporalControl {
    client: Arc<Client>,
    task_queue: String,
}

impl TemporalControl {
    /// 构造：`client` 为已连接的 Temporal 客户端，`task_queue` 为本组件 worker 的任务队列
    pub fn new(client: Arc<Client>, task_queue: impl Into<String>) -> Self {
        Self {
            client,
            task_queue: task_queue.into(),
        }
    }
}

#[async_trait]
impl Control for TemporalControl {
    async fn start(
        &self,
        db: &DatabaseConnection,
        deployment_id: &str,
        trigger: &str,
    ) -> Result<(), ModelinferError> {
        let d = deployments::get_deployment(db, deployment_id).await?;
        // 自身已活跃 → 拒绝重复启动
        if deployments::is_active_state(&d.state) {
            return Err(ModelinferError::new(
                codes::DEPLOYMENT_CONFLICT,
                format!("部署 {} 已处于 {} 状态", d.name, d.state),
            ));
        }
        let node_ids: Vec<String> = serde_json::from_str(&d.node_ids_json).unwrap_or_default();
        if node_ids.is_empty() {
            return Err(ModelinferError::new(
                codes::PARAM_INVALID,
                "运行配置未关联任何节点，无法启动",
            ));
        }
        // 先置启动意图：收窄同一运行配置的并发重复启动窗口。
        // 节点可被多个运行配置共享，因此这里不再复核 / 拒绝节点占用冲突。
        apply_state(
            db,
            &MarkStateReq {
                deployment_id: deployment_id.to_string(),
                state: Some("starting".into()),
                desired_state: Some("running".into()),
                health_status: Some("unknown".into()),
                last_error: Some(String::new()),
            },
        )
        .await?;
        let input = serde_json::json!({ "deployment_id": deployment_id, "trigger": trigger });
        start_workflow(
            &self.client,
            START_WORKFLOW_TYPE,
            &start_workflow_id(deployment_id),
            &self.task_queue,
            &input,
        )
        .await
    }

    async fn stop(
        &self,
        deployment_id: &str,
        force: bool,
        reason: &str,
    ) -> Result<(), ModelinferError> {
        // 终止在途启动工作流（若存在；不存在 / 已关闭不报错），使停止不依赖其存活
        let run_wid = start_workflow_id(deployment_id);
        if let Err(e) = terminate_workflow(&self.client, &run_wid, reason).await {
            tracing::warn!(
                target: "modelinfer_deploy",
                workflow_id = %run_wid,
                error = %e.msg,
                "终止启动工作流失败（继续停止清理）"
            );
        }
        let input = serde_json::json!({ "deployment_id": deployment_id, "force": force });
        let wid = format!("modelinfer-stop-{deployment_id}-{}", ulid::Ulid::new());
        start_workflow(
            &self.client,
            STOP_WORKFLOW_TYPE,
            &wid,
            &self.task_queue,
            &input,
        )
        .await
    }
}

/// 单次控制调用记录（Mock 用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlCall {
    /// 启动（部署 id / 触发来源）
    Start { deployment_id: String, trigger: String },
    /// 停止（部署 id / 是否强制 / 原因）
    Stop { deployment_id: String, force: bool, reason: String },
}

/// Mock 实现（单测：记录调用、可注入失败）
#[derive(Default)]
pub struct MockControl {
    calls: Mutex<Vec<ControlCall>>,
    fail_start: Mutex<Option<String>>,
    fail_stop: Mutex<Option<String>>,
}

impl MockControl {
    /// 新建空 Mock
    pub fn new() -> Self {
        Self::default()
    }

    /// 让启动调用失败（返回 `20721`，消息为给定文本）
    pub fn fail_start(&self, msg: impl Into<String>) -> &Self {
        *self.fail_start.lock().expect("mock lock") = Some(msg.into());
        self
    }

    /// 让停止调用失败（返回 `20721`，消息为给定文本）
    pub fn fail_stop(&self, msg: impl Into<String>) -> &Self {
        *self.fail_stop.lock().expect("mock lock") = Some(msg.into());
        self
    }

    /// 已记录的调用序列
    pub fn calls(&self) -> Vec<ControlCall> {
        self.calls.lock().expect("mock lock").clone()
    }

    /// 启动调用次数
    pub fn start_count(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| matches!(c, ControlCall::Start { .. }))
            .count()
    }

    /// 停止调用次数
    pub fn stop_count(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| matches!(c, ControlCall::Stop { .. }))
            .count()
    }

    /// 清空调用记录（保留失败注入）
    pub fn clear(&self) {
        self.calls.lock().expect("mock lock").clear();
    }
}

#[async_trait]
impl Control for MockControl {
    async fn start(
        &self,
        _db: &DatabaseConnection,
        deployment_id: &str,
        trigger: &str,
    ) -> Result<(), ModelinferError> {
        if let Some(msg) = self.fail_start.lock().expect("mock lock").clone() {
            return Err(ModelinferError::new(codes::TEMPORAL_FAILED, msg));
        }
        self.calls.lock().expect("mock lock").push(ControlCall::Start {
            deployment_id: deployment_id.to_string(),
            trigger: trigger.to_string(),
        });
        Ok(())
    }

    async fn stop(
        &self,
        deployment_id: &str,
        force: bool,
        reason: &str,
    ) -> Result<(), ModelinferError> {
        if let Some(msg) = self.fail_stop.lock().expect("mock lock").clone() {
            return Err(ModelinferError::new(codes::TEMPORAL_FAILED, msg));
        }
        self.calls.lock().expect("mock lock").push(ControlCall::Stop {
            deployment_id: deployment_id.to_string(),
            force,
            reason: reason.to_string(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_id_is_stable_per_deployment() {
        assert_eq!(start_workflow_id("01ABC"), "modelinfer-deploy-01ABC");
    }

    #[test]
    fn stop_normal_path_only_for_active_states() {
        // 正常停止：仅对未处于 stopped / stopping 的部署下发清理
        assert!(stop_should_launch("running", false));
        assert!(stop_should_launch("starting", false));
        assert!(stop_should_launch("degraded", false));
        assert!(stop_should_launch("failed", false));
        assert!(!stop_should_launch("stopped", false));
        assert!(!stop_should_launch("stopping", false));
    }

    #[test]
    fn force_stop_always_launches() {
        // 强制停止：任何状态都下发，用于清除异常状态
        for state in ["running", "starting", "degraded", "failed", "stopped", "stopping"] {
            assert!(stop_should_launch(state, true), "state={state} 强制停止应下发清理");
        }
    }

    #[tokio::test]
    async fn mock_records_calls_and_injects_failures() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let c = MockControl::new();
        c.start(&db, "d1", TRIGGER_MANUAL).await.unwrap();
        c.stop("d1", false, "test").await.unwrap();
        assert_eq!(c.start_count(), 1);
        assert_eq!(c.stop_count(), 1);
        assert_eq!(
            c.calls()[0],
            ControlCall::Start { deployment_id: "d1".into(), trigger: "manual".into() }
        );

        c.fail_start("boom");
        assert!(c.start(&db, "d2", TRIGGER_AUTO_RESTART).await.is_err());
        c.clear();
        assert!(c.calls().is_empty());
    }
}