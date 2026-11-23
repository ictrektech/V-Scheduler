//! 节点监控组件部署工作流（node-exporter）
//!
//! `NodeMonitorDeployWorkflow`：`prepare → ensure-image → deploy → register-consul → verify → finish`。
//! 输入仅 `node_id`（凭据在 activity 内实时加载，避免进入 workflow history）；任一步失败即置
//! `failed` 并记录错误，用户可重试。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{ActivityOptions, WorkflowContext, WorkflowResult, WorkflowTermination};

use crate::deploy::activities::{DeployActivities, MonitorFailReq, NodeMonitorReq};

/// 工作流输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorStateOutput {
    pub state: String,
}

fn opts(secs: u64) -> ActivityOptions {
    ActivityOptions::start_to_close_timeout(Duration::from_secs(secs))
}

fn termination(stage: &str, detail: &str) -> WorkflowTermination {
    WorkflowTermination::from(anyhow::anyhow!("[NodeMonitorDeployWorkflow][{stage}] {detail}"))
}

/// 标记失败（best-effort）并返回终止错误
async fn fail<T>(
    ctx: &mut WorkflowContext<T>,
    node_id: &str,
    component: &str,
    stage: &str,
    detail: &str,
) -> WorkflowTermination {
    let _ = ctx
        .start_activity(
            DeployActivities::monitor_fail,
            MonitorFailReq {
                node_id: node_id.to_string(),
                component: component.to_string(),
                stage: stage.to_string(),
                error: detail.to_string(),
            },
            opts(30),
        )
        .await;
    termination(stage, detail)
}

/// 节点监控组件部署工作流（单组件）
#[workflow]
#[derive(Default)]
pub struct NodeMonitorDeployWorkflow;

#[workflow_methods]
impl NodeMonitorDeployWorkflow {
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        input: NodeMonitorReq,
    ) -> WorkflowResult<MonitorStateOutput> {
        let nid = input.node_id.clone();
        let comp = input.component.clone();
        let req = || NodeMonitorReq { node_id: nid.clone(), component: comp.clone() };

        // 1. 准备（初始化监控记录，置 deploying）
        if let Err(e) = ctx
            .start_activity(DeployActivities::monitor_prepare, req(), opts(60))
            .await
        {
            return Err(fail(ctx, &nid, &comp, "prepare", &format!("准备失败: {e}")).await);
        }

        // 2. 镜像就绪（节点本地 → image-hub 分发；绝不 docker pull）；大镜像拉取按小时起步
        if let Err(e) = ctx
            .start_activity(
                DeployActivities::monitor_ensure_image,
                req(),
                opts(crate::images::IMAGE_ENSURE_ACTIVITY_TIMEOUT_SECS),
            )
            .await
        {
            return Err(fail(ctx, &nid, &comp, "ensure-image", &format!("镜像就绪失败: {e}")).await);
        }

        // 3. 启动容器并校验 running
        if let Err(e) = ctx
            .start_activity(DeployActivities::monitor_deploy, req(), opts(600))
            .await
        {
            return Err(fail(ctx, &nid, &comp, "deploy", &format!("容器启动失败: {e}")).await);
        }

        // 4. 注册 consul
        if let Err(e) = ctx
            .start_activity(DeployActivities::monitor_register_consul, req(), opts(60))
            .await
        {
            return Err(fail(ctx, &nid, &comp, "register-consul", &format!("consul 注册失败: {e}")).await);
        }

        // 5. 校验端口可达
        if let Err(e) = ctx
            .start_activity(DeployActivities::monitor_verify, req(), opts(120))
            .await
        {
            return Err(fail(ctx, &nid, &comp, "verify", &format!("校验失败: {e}")).await);
        }

        // 6. 收尾（置 deployed）
        if let Err(e) = ctx
            .start_activity(DeployActivities::monitor_finish, req(), opts(30))
            .await
        {
            return Err(fail(ctx, &nid, &comp, "finish", &format!("收尾失败: {e}")).await);
        }

        Ok(MonitorStateOutput { state: "deployed".into() })
    }
}