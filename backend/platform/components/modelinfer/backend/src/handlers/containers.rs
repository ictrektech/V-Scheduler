//! 容器日志流式查看 handlers（SSE）

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use serde::Deserialize;

use crate::deploy::script::sh_quote;
use crate::error::{ModelinferError, codes};
use crate::observability::store;
use crate::state::AppState;

/// 默认日志尾部行数
pub const DEFAULT_TAIL: u32 = 30;
/// 尾部行数上限
pub const MAX_TAIL: u32 = 1000;
/// 会话读超时（流式跟随用于会话构造，不限制整体时长）
const STREAM_IDLE_TIMEOUT_SECS: u64 = 3600;
/// SSE keep-alive 间隔
const SSE_KEEPALIVE_SECS: u64 = 15;

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    pub tail: Option<u32>,
}

/// 归一 `tail`：默认 30，clamp 到 1..=1000
pub fn normalize_tail(raw: Option<u32>) -> u32 {
    match raw {
        Some(v) if v >= 1 => v.min(MAX_TAIL),
        _ => DEFAULT_TAIL,
    }
}

/// 构造容器日志跟随命令（容器名经单引号转义）
pub fn container_log_cmd(name: &str, tail: u32) -> String {
    format!("docker logs -f --tail {} {}", normalize_tail(Some(tail)), sh_quote(name))
}

/// `GET /api/deployments/{id}/nodes/{node_id}/containers/{name}/stream?tail=30`
///
/// 校验归属后以 SSE 流式推送 `docker logs -f --tail <tail> <name>` 输出。
pub async fn stream_container_logs(
    State(state): State<Arc<AppState>>,
    Path((deployment_id, node_id, name)): Path<(String, String, String)>,
    Query(q): Query<StreamQuery>,
) -> Result<Response, ModelinferError> {
    // 运行配置存在
    crate::deployments::get_deployment(&state.db, &deployment_id).await?;
    // 容器必须属于该运行配置该节点（避免越权查看节点上其它容器）
    let owned = store::list_containers(&state.db, &deployment_id)
        .await?
        .into_iter()
        .any(|c| c.node_id == node_id && c.name == name);
    if !owned {
        return Err(ModelinferError::new(
            codes::NOT_FOUND,
            format!("容器不存在或不属于该运行配置/节点: {name}"),
        ));
    }
    // 节点凭据（仅在后端内存使用）
    let node = crate::nodes::get_node(&state.db, &node_id).await?;
    let target = crate::nodes::ssh_target_of(&node);

    let tail = normalize_tail(q.tail);
    let cmd = container_log_cmd(&name, tail);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let ssh = state.ssh.clone();
    let host = node.host.clone();
    let ctn = name.clone();
    tokio::spawn(async move {
        if let Err(e) = ssh
            .stream(&target, &cmd, tx, Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS))
            .await
        {
            tracing::warn!(
                target: "modelinfer_deploy",
                host = %host,
                container = %ctn,
                error = %e.msg,
                "容器日志流式执行结束（异常）"
            );
        }
    });

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|chunk| {
            let event = Event::default().data(String::from_utf8_lossy(&chunk));
            (Ok::<Event, Infallible>(event), rx)
        })
    });
    let sse = Sse::new(stream).keep_alive(
        KeepAlive::new().interval(Duration::from_secs(SSE_KEEPALIVE_SECS)),
    );
    Ok(sse.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_normalization() {
        assert_eq!(normalize_tail(None), 30);
        assert_eq!(normalize_tail(Some(0)), 30);
        assert_eq!(normalize_tail(Some(5)), 5);
        assert_eq!(normalize_tail(Some(99999)), MAX_TAIL);
    }

    #[test]
    fn command_quotes_container_name() {
        assert_eq!(container_log_cmd("glm53", 30), "docker logs -f --tail 30 'glm53'");
        // 注入尝试被单引号转义
        let cmd = container_log_cmd("x'; rm -rf /", 30);
        assert!(cmd.contains("'x'\\''; rm -rf /'"));
    }
}