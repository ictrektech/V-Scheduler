//! 部署编排 Workflow（启动 / 停止相互独立）
//!
//! - `DeploymentStartWorkflow`：启动全流程，置 `running` 后**正常结束**（有限步骤）；
//! - `DeploymentStopWorkflow`：独立停止清理（停止阶段 + 卸载 NFS + 置 `stopped`）。
//!
//! 健康巡检不再作为 workflow 执行，而由进程级调度器 [`crate::deploy::health`] 周期执行，
//! 因此两个工作流都是有限步骤、history 天然有界，不会因长运行触碰 Temporal 历史上限。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{ActivityOptions, WorkflowContext, WorkflowResult, WorkflowTermination};

use crate::deploy::activities::{
    DeployActivities, DeploymentIdReq, DeploymentStartReq, DeploymentStopReq, FinishRunReq,
    MarkStateReq, NodeReq, StageReq,
};

/// 启动 / 停止工作流输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentStateOutput {
    pub state: String,
}

fn opts(secs: u64) -> ActivityOptions {
    ActivityOptions::start_to_close_timeout(Duration::from_secs(secs))
}

/// 标记失败并返回终止错误
async fn mark_failed<T>(
    ctx: &mut WorkflowContext<T>,
    deployment_id: &str,
    _stage: &str,
    detail: &str,
) {
    let _ = ctx
        .start_activity(
            DeployActivities::mark_state,
            MarkStateReq {
                deployment_id: deployment_id.to_string(),
                state: Some("failed".into()),
                desired_state: Some("stopped".into()),
                health_status: None,
                last_error: Some(detail.to_string()),
            },
            opts(30),
        )
        .await;
}

fn termination(stage: &str, detail: &str) -> WorkflowTermination {
    WorkflowTermination::from(anyhow::anyhow!("[DeploymentStartWorkflow][{stage}] {detail}"))
}

/// 收尾启动 run（记录终态并触发保留策略裁剪）
async fn finish_run<T>(
    ctx: &mut WorkflowContext<T>,
    deployment_id: &str,
    run_id: &str,
    state: &str,
    summary: &str,
    error: &str,
) {
    let _ = ctx
        .start_activity(
            DeployActivities::finish_run,
            FinishRunReq {
                deployment_id: deployment_id.to_string(),
                run_id: run_id.to_string(),
                state: state.to_string(),
                summary: summary.to_string(),
                error: error.to_string(),
            },
            opts(30),
        )
        .await;
}

/// 失败：标记部署失败、收尾 run、返回终止错误
async fn fail<T>(
    ctx: &mut WorkflowContext<T>,
    deployment_id: &str,
    run_id: &str,
    stage: &str,
    detail: &str,
) -> WorkflowTermination {
    mark_failed(ctx, deployment_id, stage, detail).await;
    if !run_id.is_empty() {
        finish_run(ctx, deployment_id, run_id, "failed", detail, detail).await;
    }
    termination(stage, detail)
}

/// 部署启动工作流
#[workflow]
#[derive(Default)]
pub struct DeploymentStartWorkflow;

#[workflow_methods]
impl DeploymentStartWorkflow {
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        input: DeploymentStartReq,
    ) -> WorkflowResult<DeploymentStateOutput> {
        let did = input.deployment_id.clone();
        let trigger = input.trigger.clone();

        // 1. 准备
        let plan = match ctx
            .start_activity(DeployActivities::prepare, DeploymentStartReq { deployment_id: did.clone(), trigger: trigger.clone(), run_id: String::new() }, opts(120))
            .await
        {
            Ok(p) => p,
            Err(e) => {
                let detail = format!("准备阶段失败: {e}");
                mark_failed(ctx, &did, "prepare", &detail).await;
                return Err(termination("prepare", &detail));
            }
        };

        // 2. 节点连通性检查
        for node_id in &plan.node_ids {
            let r = ctx
                .start_activity(
                    DeployActivities::check_node,
                    NodeReq {
                        deployment_id: did.clone(),
                        node_id: node_id.clone(),
                        run_id: plan.run_id.clone(),
                    },
                    opts(60),
                )
                .await;
            if let Err(e) = r {
                let detail = format!("节点 {node_id} 连通性检查失败: {e}");
                return Err(fail(ctx, &did, &plan.run_id, "check-node", &detail).await);
            }
        }

        // 3. 镜像预检与分发（挂载 NFS / 分发 / 启动之前；失败即中止且无节点侧副作用）
        if let Err(e) = ctx
            .start_activity(
                DeployActivities::ensure_images,
                DeploymentIdReq { deployment_id: did.clone(), run_id: plan.run_id.clone() },
                opts(crate::images::IMAGE_ENSURE_ACTIVITY_TIMEOUT_SECS),
            )
            .await
        {
            let detail = format!("镜像预检/分发失败: {e}");
            return Err(fail(ctx, &did, &plan.run_id, "ensure-images", &detail).await);
        }

        // 4. 挂载 NFS
        if let Err(e) = ctx
            .start_activity(DeployActivities::mount_nfs, DeploymentIdReq { deployment_id: did.clone(), run_id: plan.run_id.clone() }, opts(300))
            .await
        {
            let detail = format!("NFS 挂载失败: {e}");
            return Err(fail(ctx, &did, &plan.run_id, "mount-nfs", &detail).await);
        }

        // 5. 分发模型包与脚本
        if let Err(e) = ctx
            .start_activity(DeployActivities::distribute_model, DeploymentIdReq { deployment_id: did.clone(), run_id: plan.run_id.clone() }, opts(1800))
            .await
        {
            let detail = format!("模型分发失败: {e}");
            mark_failed(ctx, &did, "distribute", &detail).await;
            // 分发失败也尝试清理已挂载的 NFS
            let _ = ctx
                .start_activity(DeployActivities::cleanup, DeploymentIdReq { deployment_id: did.clone(), run_id: String::new() }, opts(600))
                .await;
            finish_run(ctx, &did, &plan.run_id, "failed", &detail, &detail).await;
            return Err(termination("distribute", &detail));
        }

        // 6. 启动阶段顺序执行（按卡片声明的阶段超时；缺省 1800s）
        for stage in &plan.stages_start {
            let stage_timeout = plan.stage_timeout_of(stage);
            if let Err(e) = ctx
                .start_activity(
                    DeployActivities::run_stage,
                    StageReq {
                        deployment_id: did.clone(),
                        stage: stage.clone(),
                        run_id: plan.run_id.clone(),
                    },
                    opts(stage_timeout),
                )
                .await
            {
                let detail = format!("阶段 {stage} 执行失败: {e}");
                mark_failed(ctx, &did, stage, &detail).await;
                let _ = ctx
                    .start_activity(DeployActivities::cleanup, DeploymentIdReq { deployment_id: did.clone(), run_id: String::new() }, opts(600))
                    .await;
                finish_run(ctx, &did, &plan.run_id, "failed", &detail, &detail).await;
                return Err(termination(stage, &detail));
            }
        }

        // 7. 置运行中（健康巡检由进程级调度器接管）
        let _ = ctx
            .start_activity(
                DeployActivities::mark_state,
                MarkStateReq {
                    deployment_id: did.clone(),
                    state: Some("running".into()),
                    desired_state: Some("running".into()),
                    health_status: Some("healthy".into()),
                    last_error: Some(String::new()),
                },
                opts(30),
            )
            .await;

        // 8. 推理引擎探测（best-effort：脚本回传 → 端口探测 → unknown；失败不影响启动）
        let _ = ctx
            .start_activity(
                DeployActivities::detect_inference_engine,
                DeploymentIdReq { deployment_id: did.clone(), run_id: plan.run_id.clone() },
                opts(60),
            )
            .await;

        // 9. 推理指标注册（best-effort：受支持引擎 → consul；失败不影响启动）
        let _ = ctx
            .start_activity(
                DeployActivities::register_inference_metrics,
                DeploymentIdReq { deployment_id: did.clone(), run_id: plan.run_id.clone() },
                opts(60),
            )
            .await;

        // 10. 尽力注册到统一网关（best-effort：失败不影响启动结果）
        let _ = ctx
            .start_activity(
                DeployActivities::register_vrouter,
                DeploymentIdReq { deployment_id: did.clone(), run_id: String::new() },
                opts(60),
            )
            .await;

        finish_run(ctx, &did, &plan.run_id, "succeeded", "部署已进入运行中", "").await;

        Ok(DeploymentStateOutput { state: "running".into() })
    }
}

/// 部署停止工作流（独立于启动；用于运行中 / 启动中 / 失败态清理）
///
/// `force = true` 表示用户显式触发的「强制停止」：即使部署已处于 `stopped` / `stopping`
/// （正常路径下幂等无操作）也重新执行清理，用于清除残留容器 / 卡死的异常状态。
#[workflow]
#[derive(Default)]
pub struct DeploymentStopWorkflow;

#[workflow_methods]
impl DeploymentStopWorkflow {
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        input: DeploymentStopReq,
    ) -> WorkflowResult<DeploymentStateOutput> {
        // 置停止过渡态（阻止重复清理与并发启动；最终由 cleanup 置 stopped）
        let _ = ctx
            .start_activity(
                DeployActivities::mark_state,
                MarkStateReq {
                    deployment_id: input.deployment_id.clone(),
                    state: Some("stopping".into()),
                    desired_state: Some("stopping".into()),
                    health_status: None,
                    last_error: None,
                },
                opts(30),
            )
            .await;

        if let Err(e) = ctx
            .start_activity(
                DeployActivities::cleanup,
                DeploymentIdReq {
                    deployment_id: input.deployment_id.clone(),
                    run_id: String::new(),
                },
                opts(900),
            )
            .await
        {
            return Err(WorkflowTermination::from(anyhow::anyhow!(
                "[DeploymentStopWorkflow] {e}"
            )));
        }
        Ok(DeploymentStateOutput { state: "stopped".into() })
    }
}