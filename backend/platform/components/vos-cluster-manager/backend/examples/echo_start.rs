//! EchoWorkflow 端到端验证器（Stage 6.4 实机使用；后续 Phase 4 编排验证可复用）
//!
//! 用法：`cargo run --example echo_start -- http://<宿主IP>:7233`
//! - 连接目标 temporal（worker 需已在 vos-cluster-manager-tasks 队列运行）
//! - start `echo_workflow`（EchoWorkflow）→ activity echo → 打印回显结果

use temporalio_client::WorkflowStartOptions;
use vos_cluster_manager::temporal::{connect, demo::EchoWorkflow, TASK_QUEUE};

#[tokio::main]
async fn main() -> Result<(), String> {
    let server = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:7233".to_string());
    let input = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "hello-vcm-stage64".to_string());

    let client = connect(&server).await?;
    let wid = format!(
        "echo-vcm-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis()
    );

    let handle = client
        .start_workflow(
            EchoWorkflow,
            input.clone(),
            WorkflowStartOptions::new(TASK_QUEUE.to_string(), wid.clone()).build(),
        )
        .await
        .map_err(|e| format!("start_workflow 失败: {e}"))?;

    let result: String = handle
        .get_result(temporalio_client::WorkflowGetResultOptions::default())
        .await
        .map_err(|e| format!("get_result 失败: {e}"))?;

    println!("[echo_start] workflow_id={wid} 输入={input} 回显={result}");
    if result == input {
        println!("[echo_start] ✅ EchoWorkflow 端到端成功");
        Ok(())
    } else {
        Err(format!("回显不符: 期望 {input} 实得 {result}"))
    }
}