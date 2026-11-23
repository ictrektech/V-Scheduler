//! 本节点入群（self-join）：`POST /api/cluster/join`
//!
//! # 定位
//!
//! 已安装本应用、但**不属于任何集群**的节点，录入目标集群任一**管理节点 IP + 管理员
//! 密码**即可入群，成为该集群的管理节点（运行 etcd + postgres-ha）。
//!
//! # 编排归属：本端点只做「引导」，编排仍在 seed
//!
//! 入群编排**不在本节点重新实现** member_add / promote / pg join——那样会与
//! `nodes.rs` 的扩容链路产生第二套算法，且入群节点此刻还没有 etcd，无法做 raft 变更。
//! 正确链路：
//!
//! 1. 本节点探活 seed（GET `/api/health`）；
//! 2. 本节点调 seed 的 `POST /api/nodes`（`join_infra=true, role=manager`），由 seed
//!    复用既有 learner → 部署 etcd(existing) → promote → pg join 流程；
//! 3. 本节点**只等待**本地 etcd 出现 `/vos/cluster/info`（`LazyEtcdClusterClient` 会
//!    在 etcd 子项目就绪后自愈重连）。
//!
//! # 语义区分
//!
//! - 本端点（`/api/cluster/join`）= **本节点入群**：调用方节点无集群，seed 节点有集群；
//! - `POST /api/nodes`（`join_infra=true`）= **扩容**：在本集群已创建的节点上追加成员。
//!
//! # 安全边界
//!
//! - 前置：内网互信（与 `remote/` 跨节点通道一致，无独立鉴权）；seed 与入群节点须安装
//!   同版本本应用；
//! - **不接受任意 URL**：`seed_ip` 只按 `IpAddr` 解析，阻止把本端点当作任意地址的
//!   SSRF 跳板（协议固定 http、端口固定 `remote_app_port`）；
//! - 管理员密码仅随请求转发给 seed，不在本节点落盘、不写日志、不回显。
//!
//! 错误码（不新增）：20501 参数｜20507 本节点已在集群｜20509 seed 不可达｜
//! 20511 seed 编排失败 / 本机确认超时。

use std::net::{IpAddr, UdpSocket};
use std::time::Duration;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::codes;
use crate::error::{ResultBody, VcmError};
use crate::etcd::dto::{self, ClusterInfo};
use crate::state::AppState;

/// 本机 etcd 出现集群信息的确认轮询（生产 20 × 3s；单测注入毫秒级）
#[derive(Debug, Clone, Copy)]
struct ConfirmTimings {
    attempts: u32,
    interval: Duration,
}

impl Default for ConfirmTimings {
    fn default() -> Self {
        Self {
            attempts: 20,
            interval: Duration::from_secs(3),
        }
    }
}

fn parse_ip_field(raw: &str, field: &str, hint: &str) -> Result<String, VcmError> {
    let ip = raw.trim();
    ip.parse::<IpAddr>()
        .map(|_| ip.to_string())
        .map_err(|_| VcmError::new(codes::PARAM_INVALID, format!("{field} 必须为合法 IP 地址{hint}")))
}

/// 本机对外 IP 探测：UDP `connect` 只选路由、不发包，`local_addr()` 即到达 seed 的出口地址
fn detect_local_ip(seed_ip: &str) -> Result<String, VcmError> {
    let seed: IpAddr = seed_ip
        .parse()
        .map_err(|_| VcmError::new(codes::PARAM_INVALID, "seed_ip 必须为合法 IP 地址"))?;
    let bind_addr = if seed.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let port = crate::remote::default_remote_app_port();
    UdpSocket::bind(bind_addr)
        .and_then(|s| {
            s.connect((seed, port))?;
            s.local_addr()
        })
        .map(|addr| addr.ip().to_string())
        .map_err(|e| {
            VcmError::new(
                codes::PARAM_INVALID,
                format!("无法自动探测本机 IP（{e}），请显式传入 node_ip"),
            )
        })
}

/// 本节点入群：校验 → 探测本机 IP → 探活 seed → 请 seed 编排 → 等本地 etcd 可见集群
pub async fn cluster_join(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let data = cluster_join_with(&state, &body, ConfirmTimings::default()).await?;
    Ok(Json(ResultBody::ok(data, state.version)))
}

async fn cluster_join_with(
    state: &AppState,
    body: &Value,
    timings: ConfirmTimings,
) -> Result<Value, VcmError> {
    // ---- 参数校验（20501）----
    let seed_ip = body
        .get("seed_ip")
        .and_then(|v| v.as_str())
        .map(|s| parse_ip_field(s, "seed_ip", "（目标集群任一管理节点 IP）"))
        .transpose()?
        .ok_or_else(|| {
            VcmError::new(
                codes::PARAM_INVALID,
                "seed_ip 必填（目标集群任一管理节点 IP）",
            )
        })?;

    let admin_password = body
        .get("admin_password")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty() && s.len() >= 6)
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "admin_password 必填且长度 ≥6"))?;

    let display_name = body
        .get("display_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().count() <= 64);

    let node_ip_override = body
        .get("node_ip")
        .and_then(|v| v.as_str())
        .map(|s| parse_ip_field(s, "node_ip", "（本机 IP 覆盖，可留空自动探测）"))
        .transpose()?;

    // ---- 1) 已有集群守卫：本节点已入群 → 20507（本地 etcd 不存在视为全新节点）----
    match state.etcd_cluster_client.get_cluster_info().await {
        Ok(Some(_)) => {
            return Err(VcmError::new(
                codes::STATE_NOT_SUPPORTED,
                "本节点已属于集群，不能重复加入",
            ))
        }
        Ok(None) => {}
        Err(e) => {
            tracing::info!(
                err = %e.msg,
                "本节点入群: 本机 etcd 不可用（视为尚未入群，继续）"
            );
        }
    }

    // ---- 2) 本机 IP（未显式传入时按到 seed 的出口地址探测）----
    let node_ip = match node_ip_override {
        Some(ip) => ip,
        None => detect_local_ip(&seed_ip)?,
    };

    // ---- 3) 探活 seed（未安装本应用 / 端口不可达 → 20509）----
    state.remote_client.probe(&seed_ip).await.map_err(|e| {
        VcmError::new(
            codes::REMOTE_NODE_FAILED,
            format!(
                "无法连接集群管理节点 {seed_ip}（{}）；请确认目标节点已安装本应用且 19906 可达",
                e.msg
            ),
        )
    })?;

    // ---- 4) 请 seed 执行加入编排（复用其 /api/nodes，不在本节点重实现）----
    let seed_body = json!({
        "node_ip": node_ip,
        "admin_password": admin_password,
        "role": dto::role::MANAGER,
        "display_name": display_name.unwrap_or_else(|| node_ip.clone()),
        "join_infra": true,
    });
    let seed_out = state
        .remote_client
        .add_node_join(&seed_ip, seed_body)
        .await
        .map_err(|e| {
            VcmError::new(
                codes::ORCHESTRATION_FAILED,
                format!("seed 管理节点 {seed_ip} 编排失败：{}", e.msg),
            )
        })?;

    // ---- 5) 本机确认：等本地 etcd 看到集群状态（Lazy client 自愈重连）----
    let cluster = confirm_local_cluster(state, &timings).await?;

    tracing::info!(
        node_ip = %node_ip,
        seed_ip = %seed_ip,
        cluster_id = %cluster.cluster_id,
        "本节点入群完成"
    );

    Ok(json!({
        "cluster": cluster,
        "node_ip": node_ip,
        "member": seed_out.get("node").cloned().unwrap_or(Value::Null),
    }))
}

/// 轮询本地 etcd 直到出现 `/vos/cluster/info`；超时 → 20511（目标集群侧人工核对）
async fn confirm_local_cluster(
    state: &AppState,
    timings: &ConfirmTimings,
) -> Result<ClusterInfo, VcmError> {
    for attempt in 0..timings.attempts {
        if let Ok(Some(info)) = state.etcd_cluster_client.get_cluster_info().await {
            return Ok(info);
        }
        if attempt + 1 < timings.attempts {
            tokio::time::sleep(timings.interval).await;
        }
    }
    Err(VcmError::new(
        codes::ORCHESTRATION_FAILED,
        "seed 侧编排可能未完成：本机 etcd 尚未出现集群状态，请在目标集群检查节点列表",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::etcd::{EtcdClusterClient, MockEtcdClusterClient};
    use crate::projects::MockProjectsClient;
    use crate::remote::{MockRemoteNodeClient, RemoteNodeClient};
    use std::sync::{Arc, Mutex};

    const SEED_IP: &str = "10.0.0.1";
    const SELF_IP: &str = "10.0.0.9";

    fn fast_timings() -> ConfirmTimings {
        ConfirmTimings {
            attempts: 2,
            interval: Duration::from_millis(1),
        }
    }

    fn cluster_info() -> ClusterInfo {
        ClusterInfo {
            cluster_id: "cluster-abc".into(),
            cluster_name: "demo".into(),
            status: dto::status::CREATED.into(),
            created_at: "2026-10-30T00:00:00Z".into(),
        }
    }

    type JoinTestState = (
        AppState,
        Arc<MockEtcdClusterClient>,
        Arc<MockRemoteNodeClient>,
        Arc<Mutex<Vec<String>>>,
    );

    /// 构造入群单测 AppState：etcd / remote Mock 共享同一调用序日志，seed 预置可探活
    fn join_test_state() -> JoinTestState {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let etcd = Arc::new(MockEtcdClusterClient::with_event_log(log.clone()));
        let remote = Arc::new(MockRemoteNodeClient::with_event_log(log.clone()));
        remote.preload(&[SEED_IP]);
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            etcd.clone() as Arc<dyn EtcdClusterClient>,
            Arc::new(MockProjectsClient::new()),
            None,
            remote.clone() as Arc<dyn RemoteNodeClient>,
            crate::remote::default_remote_app_port(),
            None,
        );
        (state, etcd, remote, log)
    }

    fn request_body() -> Value {
        json!({
            "seed_ip": SEED_IP,
            "admin_password": "secret123",
            "node_ip": SELF_IP,
            "display_name": "node-9",
        })
    }

    /// 已有集群信息 → 拒绝重复加入（20507），且不发起任何跨节点调用
    #[tokio::test]
    async fn cluster_join_rejects_when_already_in_cluster() {
        let (state, etcd, remote, _log) = join_test_state();
        etcd.put_cluster_info(&cluster_info()).await.unwrap();

        let err = cluster_join_with(&state, &request_body(), fast_timings())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::STATE_NOT_SUPPORTED);
        assert!(err.msg.contains("已属于集群"), "msg={}", err.msg);
        assert!(
            remote.events().is_empty(),
            "已在集群时不应触达 seed: {:?}",
            remote.events()
        );
    }

    /// 成功路径：探活 seed → 请 seed 编排 → 本机 etcd 可见集群信息
    #[tokio::test]
    async fn cluster_join_happy_path() {
        let (state, etcd, remote, log) = join_test_state();
        let info = cluster_info();
        let seeded = info.clone();
        // Mock 无法访问 etcd 客户端：用钩子模拟 seed 编排完成后本机 etcd 出现集群状态
        remote.set_add_node_join_hook(Arc::new(move || etcd.seed_cluster_info(&seeded)));

        let out = cluster_join_with(&state, &request_body(), fast_timings())
            .await
            .expect("入群应成功");

        assert_eq!(out["node_ip"], SELF_IP);
        assert_eq!(out["cluster"]["cluster_id"], info.cluster_id);
        assert_eq!(out["member"]["node_ip"], SELF_IP, "seed 返回的成员对象应透传");

        let ev = log.lock().unwrap().clone();
        let probe = ev.iter().position(|e| e == "remote:probe").expect("应探活 seed");
        let join = ev
            .iter()
            .position(|e| e == "remote:add_node_join")
            .expect("应请求 seed 编排");
        assert!(probe < join, "必须先探活再请求编排: {ev:?}");

        let local = state
            .etcd_cluster_client
            .get_cluster_info()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(local.cluster_name, "demo");
    }

    /// seed 编排失败 → 20511，消息保留 seed 的错误信息
    #[tokio::test]
    async fn cluster_join_seed_fails_maps_to_20511() {
        let (state, _etcd, remote, _log) = join_test_state();
        remote.set_fail_op("add_node_join", true);

        let err = cluster_join_with(&state, &request_body(), fast_timings())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::ORCHESTRATION_FAILED);
        assert!(
            err.msg.contains("mock 持续故障注入"),
            "应保留 seed 错误信息: {}",
            err.msg
        );
    }

    /// seed 不可达 → 20509（提示确认已安装本应用且端口可达）
    #[tokio::test]
    async fn cluster_join_seed_unreachable_maps_to_20509() {
        let (state, _etcd, remote, _log) = join_test_state();
        remote.set_unreachable(&[SEED_IP]);

        let err = cluster_join_with(&state, &request_body(), fast_timings())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::REMOTE_NODE_FAILED);
        assert!(err.msg.contains("19906 可达"), "msg={}", err.msg);
    }

    /// seed 成功但本机 etcd 始终没有集群信息 → 20511（编排未完成提示）
    #[tokio::test]
    async fn cluster_join_when_local_cluster_never_appears_returns_20511() {
        let (state, _etcd, _remote, _log) = join_test_state();

        let err = cluster_join_with(&state, &request_body(), fast_timings())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::ORCHESTRATION_FAILED);
        assert!(err.msg.contains("本机 etcd 尚未出现集群状态"), "msg={}", err.msg);
    }

    /// 参数校验：非法 IP / 密码过短 → 20501
    #[tokio::test]
    async fn cluster_join_validates_input() {
        let (state, _etcd, remote, _log) = join_test_state();

        for body in [
            json!({"seed_ip": "not-an-ip", "admin_password": "secret123"}),
            json!({"admin_password": "secret123"}),
            json!({"seed_ip": SEED_IP, "admin_password": "12345"}),
            json!({"seed_ip": SEED_IP, "admin_password": "secret123", "node_ip": "bad"}),
        ] {
            let err = cluster_join_with(&state, &body, fast_timings())
                .await
                .unwrap_err();
            assert_eq!(err.code, codes::PARAM_INVALID, "body={body}");
        }
        assert!(
            remote.events().is_empty(),
            "参数校验失败不应触达 seed: {:?}",
            remote.events()
        );
    }

    /// 显式 node_ip 原样使用（跳过自动探测，兼容多网卡）
    #[tokio::test]
    async fn cluster_join_uses_explicit_node_ip() {
        let (state, etcd, remote, _log) = join_test_state();
        let info = cluster_info();
        let seeded = info.clone();
        remote.set_add_node_join_hook(Arc::new(move || etcd.seed_cluster_info(&seeded)));

        let out = cluster_join_with(&state, &request_body(), fast_timings())
            .await
            .unwrap();
        assert_eq!(out["node_ip"], SELF_IP);
        assert_eq!(out["member"]["role"], dto::role::MANAGER);
    }
}