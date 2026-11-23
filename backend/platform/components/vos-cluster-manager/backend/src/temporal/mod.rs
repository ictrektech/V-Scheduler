//! 平台 temporal 接入（D-VCM-08：节点本地异步编排接平台 temporal）
//!
//! - 连接：`VOS_HOST_GW_IP:VOS_TEMPORAL_PORT`（namespace default，VMS 先例）
//! - Worker task queue：`vos-cluster-manager-tasks`
//! - Phase 2：demo workflow（EchoWorkflow）验证"本应用 worker ↔ 平台 temporal"通道；
//!   Phase 4 编排工作流（create cluster / join node saga）挂载到同一 worker

pub mod cluster_create;
pub mod demo;
pub mod worker;

/// 本应用 temporal task queue（worker 注册 + client start 用）
pub const TASK_QUEUE: &str = match option_env!("VOS_CLUSTER_MANAGER_OWNER_APP") {
    Some(_) => "v-scheduler-cluster-manager-tasks",
    None => "vos-cluster-manager-tasks",
};
/// 默认 namespace
pub const NAMESPACE: &str = "default";

/// 归一化 temporal 地址：无 scheme 前缀自动补 `http://`（VMS 同款，防 `172.17.0.1:7233` 解析失败）
pub fn normalize_temporal_url(address: &str) -> String {
    if address.starts_with("http://") || address.starts_with("https://") {
        address.to_string()
    } else {
        format!("http://{address}")
    }
}

/// 连接平台 temporal（`<VOS_HOST_GW_IP>:<VOS_TEMPORAL_PORT>`，namespace default），
/// 返回 Arc<Client>（供 start workflow / worker 复用）
pub async fn connect(address: &str) -> Result<std::sync::Arc<temporalio_client::Client>, String> {
    let url = url::Url::parse(&normalize_temporal_url(address))
        .map_err(|e| format!("temporal 地址解析失败 {address}: {e}"))?;
    let conn = temporalio_client::Connection::connect(
        temporalio_client::ConnectionOptions::new(url).build(),
    )
    .await
    .map_err(|e| format!("temporal 连接失败（{address}）: {e}"))?;
    let client = temporalio_client::Client::new(
        conn,
        temporalio_client::ClientOptions::new(NAMESPACE).build(),
    )
    .map_err(|e| format!("Client 构造失败: {e}"))?;
    Ok(std::sync::Arc::new(client))
}
