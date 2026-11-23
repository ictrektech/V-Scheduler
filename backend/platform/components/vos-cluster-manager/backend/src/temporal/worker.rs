//! Temporal worker 启动（独立线程 + current_thread runtime + LocalSet，VMS 模式）
//!
//! 关键约束：temporalio-sdk 0.4 `Worker` 为 `!Send`，必须在**独立线程**内构造并运行
//! （CoreRuntime / Worker::new / worker.run() 全部在本线程完成，VMS
//! `scheduler/worker.rs` design D4 同款）。`spawn_echo_worker` 立即返回线程句柄，
//! worker 在后台线程长驻（不阻塞调用方）。

use std::sync::Arc;

use temporalio_client::{Client, ClientOptions, Connection, ConnectionOptions};
use temporalio_common::telemetry::TelemetryOptions;
use temporalio_sdk_core::{CoreRuntime, RuntimeOptions};
use temporalio_sdk::{Worker, WorkerOptions};

use super::cluster_create::{ClusterActivities, ClusterCreateWorkflow};
use super::demo::{echo_activities, EchoWorkflow};
use crate::state::AppState;

/// 启动 temporal worker（长驻后台线程；返回 JoinHandle，调用方不等待）。
///
/// 注册 Workflow：EchoWorkflow（演示）+ ClusterCreateWorkflow（Phase 4 编排）；
/// Activities：EchoActivities（演示）+ ClusterActivities（持有 AppState 依赖）。
pub fn spawn_worker(
    address: String,
    task_queue: String,
    state: AppState,
) -> Result<std::thread::JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name("vcm-temporal-worker".to_owned())
        .spawn(move || {
            // 步骤 1：独立线程 + current_thread runtime + enter 上下文
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("构造 current_thread runtime 失败");
            let _guard = rt.enter();

            // 步骤 2：连接平台 temporal（namespace default）+ Client（阻塞等待，通常 <1s）
            let client: Result<Arc<temporalio_client::Client>, String> = rt.block_on(async move {
                let url = url::Url::parse(&crate::temporal::normalize_temporal_url(&address))
                    .map_err(|e| format!("temporal 地址解析失败 {address}: {e}"))?;
                let conn = Connection::connect(ConnectionOptions::new(url).build())
                    .await
                    .map_err(|e| format!("temporal 连接失败（{address}）: {e}"))?;
                tracing::info!(address = %address, "temporal 已连接");
                let client = Client::new(conn, ClientOptions::new("default").build())
                    .map_err(|e| format!("Client 构造失败: {e}"))?;
                Ok::<_, String>(Arc::new(client))
            });

            let client = match client {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(err = %e, "temporal worker 启动失败（连接/Client）");
                    return;
                }
            };

            // 步骤 3：CoreRuntime（new_assume_tokio 复用当前 runtime）+ Worker::new（!Send 本线程）
            let result = (|| -> Result<(), String> {
                let runtime_options = RuntimeOptions::builder()
                    .telemetry_options(TelemetryOptions::builder().build())
                    .build()
                    .map_err(|e| format!("构造 RuntimeOptions 失败: {e}"))?;
                let core_runtime = CoreRuntime::new_assume_tokio(runtime_options)
                    .map_err(|e| format!("构造 CoreRuntime 失败: {e}"))?;

                let worker_options = WorkerOptions::new(&task_queue)
                    .register_workflow::<EchoWorkflow>()
                    .register_activities(echo_activities())
                    .register_workflow::<ClusterCreateWorkflow>()
                    .register_activities(ClusterActivities { state: state.clone() })
                    .build();
                let mut worker = Worker::new(&core_runtime, client.as_ref().clone(), worker_options)
                    .map_err(|e| format!("Worker 构造失败: {e}"))?;

                tracing::info!(task_queue = %task_queue, "temporal worker 启动（EchoWorkflow 演示 + ClusterCreateWorkflow 编排）");

                // 步骤 4：LocalSet + block_on 驱动 worker.run()
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move { worker.run().await })
                    .map_err(|e| format!("worker run 失败: {e}"))
            })();

            match result {
                Ok(()) => tracing::info!("temporal worker 正常退出"),
                Err(e) => tracing::error!(err = %e, "temporal worker 终止"),
            }
        })
        .map_err(|e| format!("spawn worker 线程失败: {e}"))
}