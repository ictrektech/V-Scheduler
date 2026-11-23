//! 本地反代：在本机监听一个 TCP 端口，把连接双向转发到 head 节点的 vLLM 端口（L4）。
//!
//! 设计见 `openspec/changes/modelinfer-local-proxy/design.md`：
//! - 纯 tokio（`TcpListener` + `copy_bidirectional`），不引入外部组件；
//! - 未指定端口时从 `31000..=31100` 取第一个空闲端口（每次启动重新分配）；
//! - 进程级注册表 + `ensure` / `stop` / `reconcile_all`，随部署状态收敛；
//! - 配置（`enabled` / `port`）持久化在运行配置，运行态（`active_port` / `last_error`）回写。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use tokio::net::{TcpListener, TcpStream};

use crate::entities::deployment;
use crate::error::{ModelinferError, codes};
use crate::time;

/// 自动分配端口区间起点
pub const PORT_RANGE_START: u16 = 31000;
/// 自动分配端口区间终点
pub const PORT_RANGE_END: u16 = 31100;

fn db_err(e: sea_orm::DbErr) -> ModelinferError {
    ModelinferError::new(codes::DB_FAILED, format!("数据库操作失败: {e}"))
}

/// 归一化期望端口：`<= 0` 或超出 `u16` → `0`（自动分配）；否则原值
pub fn normalize_proxy_port(port: i32) -> i32 {
    if port <= 0 || port > u16::MAX as i32 {
        0
    } else {
        port
    }
}

/// 端口分配纯函数：在 `[start, end]` 内返回第一个满足 `probe` 的端口
pub fn pick_free_port<F: FnMut(u16) -> bool>(start: u16, end: u16, mut probe: F) -> Option<u16> {
    (start..=end).find(|p| probe(*p))
}

/// 单个运行配置的反代句柄
struct ProxyHandle {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

/// 进程级本地反代注册表（`deployment_id → 句柄`）
#[derive(Clone, Default)]
pub struct ProxyRegistry {
    inner: Arc<Mutex<HashMap<String, ProxyHandle>>>,
}

impl ProxyRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前监听端口（未运行为 `None`）
    pub fn active_port(&self, deployment_id: &str) -> Option<u16> {
        self.inner.lock().unwrap().get(deployment_id).map(|h| h.port)
    }

    /// 停止并移除某运行配置的监听（幂等）
    pub fn stop(&self, deployment_id: &str) {
        if let Some(h) = self.inner.lock().unwrap().remove(deployment_id) {
            h.task.abort();
        }
    }
}

/// 目标地址：head 节点 host + 有效服务端口（脚本 `global.serve.port` 优先，见 D-MI-CA-03）
fn proxy_target(head_host: &str, port: u16) -> String {
    format!("{head_host}:{port}")
}

/// 绑定监听：指定端口直接绑定；`0` 时在区间内取第一个空闲端口
async fn bind_preferred(preferred: i32) -> Result<(TcpListener, u16), String> {
    if preferred > 0 {
        let port = preferred as u16;
        match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(l) => Ok((l, port)),
            Err(e) => Err(format!("端口 {port} 绑定失败：{e}")),
        }
    } else {
        let chosen = pick_free_port(PORT_RANGE_START, PORT_RANGE_END, |p| {
            std::net::TcpListener::bind(("0.0.0.0", p)).is_ok()
        })
        .ok_or_else(|| format!("{PORT_RANGE_START}~{PORT_RANGE_END} 无空闲端口"))?;
        match TcpListener::bind(("0.0.0.0", chosen)).await {
            Ok(l) => Ok((l, chosen)),
            Err(e) => Err(format!("端口 {chosen} 绑定失败：{e}")),
        }
    }
}

/// 启动 accept loop 并注册句柄（同 id 已存在时先中止旧监听）
fn spawn_proxy(registry: &ProxyRegistry, deployment_id: &str, listener: TcpListener, port: u16, target: String) {
    let id = deployment_id.to_string();
    let id_for_log = id.clone();
    let target_for_log = target.clone();
    let task = tokio::spawn(async move {
        tracing::info!(
            target: "modelinfer_localproxy",
            deployment = %id_for_log,
            port,
            target = %target_for_log,
            "本地反代监听已启动"
        );
        loop {
            match listener.accept().await {
                Ok((mut inbound, peer)) => {
                    let target = target_for_log.clone();
                    tokio::spawn(async move {
                        match TcpStream::connect(&target).await {
                            Ok(mut outbound) => {
                                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                            }
                            Err(e) => tracing::warn!(
                                target: "modelinfer_localproxy",
                                %peer,
                                target = %target,
                                error = %e,
                                "本地反代连接上游失败"
                            ),
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(target: "modelinfer_localproxy", error = %e, "本地反代 accept 失败");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });
    let mut guard = registry.inner.lock().unwrap();
    if let Some(old) = guard.insert(id, ProxyHandle { port, task }) {
        old.task.abort();
    }
}

/// 回写运行态（`active_port` / `last_error`）
async fn set_proxy_db(
    db: &DatabaseConnection,
    current: deployment::Model,
    active_port: i32,
    error: String,
) -> Result<deployment::Model, ModelinferError> {
    let mut am: deployment::ActiveModel = current.into();
    am.local_proxy_active_port = Set(active_port);
    am.local_proxy_last_error = Set(error);
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(db_err)
}

/// 确保某运行配置的反代状态与配置一致（幂等）。
///
/// - 非 `running` / 关闭 / 正在停止 → 停止监听并归零运行态；
/// - 已监听且端口符合期望 → 跳过；
/// - 否则（重新）绑定并启动监听；失败仅写 `local_proxy_last_error`，不返回硬错误。
pub async fn ensure(
    db: &DatabaseConnection,
    registry: &ProxyRegistry,
    deployment_id: &str,
) -> Result<deployment::Model, ModelinferError> {
    let d = crate::deployments::get_deployment(db, deployment_id).await?;
    if d.state != "running" || d.desired_state == "stopping" || !d.local_proxy_enabled {
        registry.stop(deployment_id);
        if d.local_proxy_active_port != 0 || !d.local_proxy_last_error.is_empty() {
            return set_proxy_db(db, d, 0, String::new()).await;
        }
        return Ok(d);
    }

    // 已监听且端口符合期望 → 保持；必要时同步 DB
    if let Some(cur) = registry.active_port(deployment_id) {
        let want = normalize_proxy_port(d.local_proxy_port);
        if want == 0 || cur as i32 == want {
            if d.local_proxy_active_port != cur as i32 {
                return set_proxy_db(db, d, cur as i32, String::new()).await;
            }
            return Ok(d);
        }
        registry.stop(deployment_id);
    }

    // 解析 head 目标
    let node_ids: Vec<String> = serde_json::from_str(&d.node_ids_json).unwrap_or_default();
    let head = match node_ids.first() {
        // head 节点可能已被删除：只写 last_error，不返回硬错误
        // （见 openspec/changes/modelinfer-dangling-node-reference/）
        Some(nid) => match crate::nodes::find_node(db, nid).await? {
            Some(node) => node,
            None => {
                return set_proxy_db(db, d, 0, "运行配置关联的节点不存在".to_string()).await;
            }
        },
        None => return set_proxy_db(db, d, 0, "运行配置未关联节点".to_string()).await,
    };
    // 有效服务端口：脚本 global.serve.port 优先，否则运行配置 serve_port（含默认 8000）
    let default_port = crate::access::normalize_serve_port(Some(d.serve_port.max(0) as u64));
    let script_serve = crate::observability::store::latest_serve_global(db, deployment_id).await?;
    let port = crate::access::effective_serve_port(default_port, script_serve.as_ref());
    let target = proxy_target(&head.host, port);
    let preferred = normalize_proxy_port(d.local_proxy_port);
    match bind_preferred(preferred).await {
        Ok((listener, port)) => {
            spawn_proxy(registry, deployment_id, listener, port, target);
            set_proxy_db(db, d, port as i32, String::new()).await
        }
        Err(msg) => set_proxy_db(db, d, 0, msg).await,
    }
}

/// 停止某运行配置的本地反代（停止清理时调用）
pub async fn stop(
    db: &DatabaseConnection,
    registry: &ProxyRegistry,
    deployment_id: &str,
) -> Result<(), ModelinferError> {
    registry.stop(deployment_id);
    let d = crate::deployments::get_deployment(db, deployment_id).await?;
    if d.local_proxy_active_port != 0 || !d.local_proxy_last_error.is_empty() {
        set_proxy_db(db, d, 0, String::new()).await?;
    }
    Ok(())
}

/// 巡检 reconcile：遍历全部运行配置，使其反代状态与配置 / 部署状态一致。
pub async fn reconcile_all(
    db: &DatabaseConnection,
    registry: &ProxyRegistry,
) -> Result<(), ModelinferError> {
    let rows = deployment::Entity::find().all(db).await.map_err(db_err)?;
    for d in rows {
        if let Err(e) = ensure(db, registry, &d.id).await {
            tracing::warn!(
                target: "modelinfer_localproxy",
                deployment = %d.id,
                error = %e.msg,
                "本地反代 reconcile 失败"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn normalize_port_rules() {
        assert_eq!(normalize_proxy_port(0), 0);
        assert_eq!(normalize_proxy_port(-1), 0);
        assert_eq!(normalize_proxy_port(70000), 0);
        assert_eq!(normalize_proxy_port(31050), 31050);
        assert_eq!(normalize_proxy_port(65535), 65535);
    }

    #[test]
    fn pick_free_port_first_available() {
        // 31000 / 31001 被占，31002 空闲
        let taken = [31000u16, 31001];
        let got = pick_free_port(PORT_RANGE_START, PORT_RANGE_END, |p| !taken.contains(&p));
        assert_eq!(got, Some(31002));
    }

    #[test]
    fn pick_free_port_none_when_full() {
        let got = pick_free_port(31000, 31002, |_| false);
        assert_eq!(got, None);
    }

    #[test]
    fn proxy_target_uses_effective_port() {
        // 有效端口由调用方解析（脚本回传优先），此处只校验拼接
        assert_eq!(proxy_target("10.0.0.1", 8188), "10.0.0.1:8188");
        assert_eq!(proxy_target("10.0.0.1", 8000), "10.0.0.1:8000");
    }

    #[tokio::test]
    async fn proxy_forwards_tcp_bytes() {
        // 目标 echo server
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = target.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 64];
                    loop {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if s.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let registry = ProxyRegistry::new();
        spawn_proxy(
            &registry,
            "d1",
            listener,
            proxy_port,
            format!("127.0.0.1:{target_port}"),
        );
        assert_eq!(registry.active_port("d1"), Some(proxy_port));

        let mut conn = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        conn.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        registry.stop("d1");
        assert_eq!(registry.active_port("d1"), None);
    }
}