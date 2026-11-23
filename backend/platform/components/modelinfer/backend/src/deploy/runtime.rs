//! Temporal 客户端连接与内嵌 Worker 启动
//!
//! Worker 采用「独立 OS 线程 + current_thread runtime + LocalSet」模式
//! （对齐 `vos-platform-backend` 的实现约定：`Worker` 是 `!Send`，不能 `tokio::spawn`）。

use std::str::FromStr;
use std::sync::Arc;

use temporalio_client::{Client, ClientOptions, Connection, ConnectionOptions};
use temporalio_common::telemetry::TelemetryOptions;
use temporalio_sdk::{Worker, WorkerOptions};
use temporalio_sdk_core::{CoreRuntime, RuntimeOptions};
use tokio::sync::oneshot;
use tokio::task::LocalSet;
use url::Url;

use crate::deploy::activities::DeployActivities;
use crate::deploy::workflows::{DeploymentStartWorkflow, DeploymentStopWorkflow};
use crate::monitor::workflows::NodeMonitorDeployWorkflow;
use crate::error::{ModelinferError, codes};

/// 解析 Temporal 地址（兼容 `host:port` 与 `scheme://host:port`）
pub fn parse_address(address: &str) -> Result<Url, ModelinferError> {
    let normalized = if address.contains("://") {
        address.to_string()
    } else {
        format!("http://{address}")
    };
    Url::from_str(&normalized).map_err(|e| {
        ModelinferError::new(codes::TEMPORAL_FAILED, format!("Temporal 地址解析失败: {e}"))
    })
}

/// 连接 Temporal 并返回 Client
pub async fn connect(address: &str, namespace: &str) -> Result<Arc<Client>, ModelinferError> {
    let target = parse_address(address)?;
    let conn_opts = ConnectionOptions::new(target).identity("modelinfer".to_string()).build();
    let connection = Connection::connect(conn_opts)
        .await
        .map_err(|e| ModelinferError::new(codes::TEMPORAL_FAILED, format!("Temporal 连接失败: {e}")))?;
    let client = Client::new(connection, ClientOptions::new(namespace.to_string()).build())
        .map_err(|e| ModelinferError::new(codes::TEMPORAL_FAILED, format!("Temporal Client 构造失败: {e}")))?;
    Ok(Arc::new(client))
}

/// Worker 句柄
pub struct WorkerHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WorkerHandle {
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|thread| !thread.is_finished())
    }

    pub fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(th) = self.thread.take() {
            let _ = th.join();
        }
    }

    /// noop handle（Temporal 未启用时）
    pub fn noop() -> Self {
        Self { shutdown_tx: None, thread: None }
    }
}

/// 启动内嵌 Worker（后台线程，非阻塞）
pub async fn spawn_worker(
    client: Arc<Client>,
    task_queue: &str,
    activities: DeployActivities,
) -> Result<WorkerHandle, ModelinferError> {
    let task_queue = task_queue.to_string();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (ready_tx, ready_rx) = oneshot::channel::<()>();

    let thread = std::thread::Builder::new()
        .name("modelinfer-worker".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!(error = %e, "构造 worker runtime 失败");
                    return;
                }
            };
            let _guard = rt.enter();

            let runtime_options = match RuntimeOptions::builder()
                .telemetry_options(TelemetryOptions::builder().build())
                .build()
            {
                Ok(o) => o,
                Err(e) => {
                    tracing::error!(error = %e, "构造 Temporal RuntimeOptions 失败");
                    return;
                }
            };
            let core_runtime = match CoreRuntime::new_assume_tokio(runtime_options) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(error = %e, "构造 Temporal CoreRuntime 失败");
                    return;
                }
            };

            let worker_options = WorkerOptions::new(&task_queue)
                .register_workflow::<DeploymentStartWorkflow>()
                .register_workflow::<DeploymentStopWorkflow>()
                .register_workflow::<NodeMonitorDeployWorkflow>()
                .register_activities(activities)
                .build();

            let mut worker = match Worker::new(&core_runtime, (*client).clone(), worker_options) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!(error = %e, "构造 Temporal Worker 失败");
                    return;
                }
            };
            if ready_tx.send(()).is_err() {
                return;
            }
            tracing::info!(task_queue = %task_queue, "modelinfer Temporal Worker 启动");

            let local = LocalSet::new();
            let result: Result<(), String> = local.block_on(&rt, async move {
                tokio::select! {
                    r = worker.run() => {
                        if let Err(e) = r {
                            tracing::error!(error = %e, "Worker 异常退出");
                        }
                    }
                    _ = shutdown_rx => {
                        tracing::info!("收到 shutdown 信号，Worker 退出");
                        drop(worker);
                    }
                }
                Ok::<(), String>(())
            });
            if let Err(e) = result {
                tracing::error!(error = %e, "Worker LocalSet 任务异常");
            }
        })
        .map_err(|e| ModelinferError::new(codes::TEMPORAL_FAILED, format!("启动 Worker 线程失败: {e}")))?;

    if !matches!(tokio::time::timeout(std::time::Duration::from_secs(10), ready_rx).await, Ok(Ok(()))) {
        return Err(ModelinferError::new(codes::TEMPORAL_FAILED, "Temporal Worker 未就绪"));
    }

    Ok(WorkerHandle {
        shutdown_tx: Some(shutdown_tx),
        thread: Some(thread),
    })
}

/// 启动工作流（untyped start + JSON 输入）
pub async fn start_workflow(
    client: &Client,
    workflow_type: &str,
    workflow_id: &str,
    task_queue: &str,
    input: &serde_json::Value,
) -> Result<(), ModelinferError> {
    use temporalio_client::{UntypedWorkflow, WorkflowStartOptions};
    use temporalio_common::data_converters::{PayloadConverter, RawValue};

    let opts = WorkflowStartOptions::new(task_queue.to_string(), workflow_id.to_string()).build();
    let converter = PayloadConverter::serde_json();
    let raw = RawValue::from_value(input, &converter);
    client
        .start_workflow(UntypedWorkflow::new(workflow_type), raw, opts)
        .await
        .map(|_| ())
        .map_err(|e| ModelinferError::new(codes::TEMPORAL_FAILED, format!("启动工作流失败: {e}")))
}

/// 终止指定工作流（best-effort）。
///
/// 用于停止时打断在途的启动工作流：启动与停止相互独立，停止 MUST NOT 依赖启动工作流存活。
/// 工作流不存在或已关闭 MUST 视为成功（幂等终止）。
pub async fn terminate_workflow(
    client: &Client,
    workflow_id: &str,
    reason: &str,
) -> Result<(), ModelinferError> {
    use temporalio_client::errors::WorkflowInteractionError;
    use temporalio_client::{UntypedWorkflow, WorkflowTerminateOptions};

    let handle = client.get_workflow_handle::<UntypedWorkflow>(workflow_id.to_string());
    let opts = WorkflowTerminateOptions::builder().reason(reason.to_string()).build();
    match handle.terminate(opts).await {
        Ok(()) => Ok(()),
        // 工作流不存在 / 已关闭：视为成功（幂等终止）
        Err(WorkflowInteractionError::NotFound(_)) => Ok(()),
        Err(e) => Err(ModelinferError::new(
            codes::TEMPORAL_FAILED,
            format!("终止工作流失败: {e}"),
        )),
    }
}
