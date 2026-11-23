//! 跨节点执行客户端（Phase 4，D-CR-01）
//!
//! 编排方（master 应用）调用**目标节点本应用**的 infra 端点（GET /api/health 探活 /
//! POST /api/infra/{etcd,pg}/* 部署/移除），目标节点用**本地** access-token 调**本地**
//! 平台 projects——token 不出节点（roadmap §5 Q1 默认路径）。
//!
//! 安全边界：一期为内网互信 HTTP（目标节点须已安装同版本本应用）；跨节点强认证 / TLS
//! 列为后续安全强化（decisions.md / roadmap §后续）。

use crate::error::codes;
use crate::error::VcmError;

/// 目标节点应用 HTTP 端口（container host 网络直连；可经 env `REMOTE_APP_PORT` 覆盖）
pub fn default_remote_app_port() -> u16 {
    std::env::var("REMOTE_APP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(19906)
}

/// 跨节点执行客户端抽象（依赖注入，保证编排核心逻辑可测）
#[async_trait::async_trait]
pub trait RemoteNodeClient: Send + Sync {
    /// 目标节点应用探活：返回版本号（GET `/<url_base>/api/health`）
    async fn probe(&self, node_ip: &str) -> Result<String, VcmError>;
    /// 转发 etcd 部署（POST `/<url_base>/api/infra/etcd/deploy_async`）
    async fn deploy_etcd(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError>;
    /// 转发 etcd 部署（同步；POST `/<url_base>/api/infra/etcd/deploy`，目标节点内部轮询至终态）
    async fn deploy_etcd_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError>;
    /// 转发 postgres-ha 部署（POST `/<url_base>/api/infra/pg/deploy_async`）
    async fn deploy_pg(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError>;
    /// 转发 postgres-ha 部署（同步；POST `/<url_base>/api/infra/pg/deploy`）
    async fn deploy_pg_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError>;
    /// 转发 etcd 移除（POST `/<url_base>/api/infra/etcd/remove_async`）
    async fn remove_etcd(
        &self,
        node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError>;
    /// 转发 postgres-ha 移除（POST `/<url_base>/api/infra/pg/remove_async`）
    async fn remove_pg(
        &self,
        node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError>;
    /// 转发 haproxy 部署（同步；POST `/<url_base>/api/infra/haproxy/deploy`，二期 S1）
    async fn deploy_haproxy_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError>;
    /// 转发 haproxy 移除（同步；POST `/<url_base>/api/infra/haproxy/remove`，二期 S1）
    async fn remove_haproxy_sync(
        &self,
        node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError>;
    /// 转发 temporal 部署（同步；POST `/<url_base>/api/infra/temporal/deploy`，二期 S2）
    async fn deploy_temporal_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError>;
    /// 转发 temporal 移除（同步；POST `/<url_base>/api/infra/temporal/remove`，二期 S2）
    async fn remove_temporal_sync(
        &self,
        node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError>;
    /// 目标节点子项目实例清单（GET `/<url_base>/api/infra/{etcd|pg}/instances`；
    /// 返回 compose_project_name 列表）
    async fn list_instances(&self, node_ip: &str, kind: &str) -> Result<Vec<String>, VcmError>;
    /// 目标节点基础设施数据清理（POST `/<url_base>/api/infra/reset_data`；
    /// 删除该节点 cluster-etcd-*/cluster-pg-* 数据目录——成员编排失败残留兜底）
    async fn reset_infra_data(&self, node_ip: &str) -> Result<serde_json::Value, VcmError>;
    /// 目标节点 etcd DCS 就绪轮询（GET `/<url_base>/api/infra/etcd/status`，
    /// 解析 data.etcd_ok；join 时部署"已部署"须等待真实就绪而非虚假成功）
    async fn etcd_dcs_ready(&self, node_ip: &str) -> Result<bool, VcmError>;
    /// 向目标节点下发 etcd 客户端凭据（D9 凭据分发）
    ///
    /// 目标节点把凭据写入本机 `<credential-dir>/clients/<identity>.json`（0600）并回写
    /// 代次；凭据只在节点间一次性传递，不经公共读接口。
    async fn push_etcd_credential(
        &self,
        node_ip: &str,
        credential: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError>;
    /// 请 seed（另一个**已入群**的本应用实例）执行「加入节点」编排：
    /// POST `/<url_base>/api/nodes`，body 含 join_infra=true（同步等待，分钟级）
    ///
    /// 语义：本节点入群（self-join）的编排**不在本节点做**——由 seed 管理节点复用
    /// 既有扩容链路（learner 加入 → 部署目标 etcd(existing) → promote → pg join），
    /// 本节点只等待本地 etcd 出现 `/vos/cluster/info`。调用方据 Err 拼 20511。
    async fn add_node_join(
        &self,
        seed_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError>;
}

/// seed 侧 `/api/nodes`（join_infra 同步编排）超时：分钟级，绝不能沿用 30s 默认值
pub const ADD_NODE_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

fn map_remote_err(ctx: &str, e: impl std::fmt::Display) -> VcmError {
    VcmError::new(codes::REMOTE_NODE_FAILED, format!("{ctx}: {e}"))
}

// ============================================================================
// Real 实现：reqwest 直连目标节点应用
// ============================================================================

pub struct RealRemoteNodeClient {
    http: reqwest::Client,
    /// 长超时 client（仅 seed 侧 `/api/nodes` 同步编排用；默认 30s 会把分钟级编排打断）
    http_long: reqwest::Client,
    /// 目标节点 url_base（各节点应用一致：/app/com.ictrek.vos-cluster-manager）
    url_base: String,
    port: u16,
    peers: Option<std::collections::HashMap<String, SchedulerPeer>>,
}

#[derive(serde::Deserialize)]
struct SchedulerPeer {
    #[serde(deserialize_with = "deserialize_peer_origin")]
    origin: reqwest::Url,
    token: String,
}

fn deserialize_peer_origin<'de, D>(deserializer: D) -> Result<reqwest::Url, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    reqwest::Url::parse(&value).map_err(|_| serde::de::Error::custom("invalid peer origin"))
}

impl RealRemoteNodeClient {
    pub fn new(url_base: String, port: u16) -> Result<Self, VcmError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| map_remote_err("构造 HTTP client", e))?;
        let http_long = reqwest::Client::builder()
            .timeout(ADD_NODE_JOIN_TIMEOUT)
            .build()
            .map_err(|e| map_remote_err("构造长超时 HTTP client", e))?;
        Ok(Self {
            http,
            http_long,
            url_base,
            port,
            peers: None,
        })
    }

    pub fn scheduler() -> Result<Self, VcmError> {
        let path = std::path::Path::new("/var/lib/vos-cluster-manager/peers.json");
        let peers: std::collections::HashMap<String, SchedulerPeer> = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| map_remote_err("invalid cluster peer configuration", "expected a peer map"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(error) => return Err(map_remote_err("read cluster peer configuration", error.kind())),
        };
        if peers.iter().any(|(ip, peer)| ip.parse::<std::net::IpAddr>().is_err()
            || peer.origin.scheme() != "https"
            || peer.origin.host_str().is_none()
            || !peer.origin.username().is_empty()
            || peer.origin.password().is_some()
            || peer.origin.path() != "/"
            || peer.origin.query().is_some()
            || peer.origin.fragment().is_some()
            || peer.token.trim().len() < 32
            || peer.token.bytes().any(|byte| byte.is_ascii_whitespace()))
        {
            return Err(map_remote_err("invalid cluster peer configuration", "invalid IP, HTTPS origin or bearer token"));
        }
        let ca_path = std::path::Path::new("/var/lib/vos-cluster-manager/peer-ca.pem");
        let ca = match std::fs::read(ca_path) {
            Ok(bytes) => Some(reqwest::Certificate::from_pem(&bytes)
                .map_err(|_| map_remote_err("invalid cluster peer CA", "expected PEM certificate"))?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(map_remote_err("read cluster peer CA", error.kind())),
        };
        let client = |timeout| {
            let mut builder = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout);
            if let Some(ca) = ca.as_ref() {
                builder = builder.add_root_certificate(ca.clone());
            }
            builder.build().map_err(|e| map_remote_err("build cluster HTTPS client", e))
        };
        Ok(Self {
            http: client(std::time::Duration::from_secs(30))?,
            http_long: client(ADD_NODE_JOIN_TIMEOUT)?,
            url_base: String::new(),
            port: 0,
            peers: Some(peers),
        })
    }

    fn request(
        &self, client: &reqwest::Client, method: reqwest::Method, node_ip: &str, resource: &str,
    ) -> Result<reqwest::RequestBuilder, VcmError> {
        if let Some(peers) = &self.peers {
            let peer = peers.get(node_ip)
                .ok_or_else(|| map_remote_err("cluster peer is not configured", node_ip))?;
            let mut url = peer.origin.clone();
            url.set_path(&format!("/api/com.ictrek.v-scheduler/scheduler/api/v1/cluster/runtime/{resource}"));
            Ok(client.request(method, url).bearer_auth(&peer.token))
        } else {
            Ok(client.request(method, format!("{}/api/{resource}", self.base(node_ip))))
        }
    }

    fn base(&self, ip: &str) -> String {
        format!("http://{ip}:{}/{}", self.port, self.url_base.trim_start_matches('/'))
    }

    /// 解析目标节点 ResultBody：code==0 → data；否则 VcmError
    async fn parse_body(&self, resp: reqwest::Response, ctx: &str) -> Result<serde_json::Value, VcmError> {
        let _status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| map_remote_err(&format!("{ctx} 读响应"), e))?;
        let body: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| map_remote_err(&format!("{ctx} 响应非 JSON"), e))?;
        if body["code"].as_i64() != Some(0) {
            return Err(map_remote_err(
                ctx,
                format!(
                    "目标节点返回 code={} msg={}",
                    body["code"].as_i64().unwrap_or(-1),
                    body["msg"].as_str().unwrap_or("unknown")
                ),
            ));
        }
        Ok(body.get("data").cloned().unwrap_or(serde_json::Value::Null))
    }
}

#[async_trait::async_trait]
impl RemoteNodeClient for RealRemoteNodeClient {
    async fn probe(&self, node_ip: &str) -> Result<String, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::GET, node_ip, "health")?
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("探活 {node_ip}"), e))?;
        let body = self.parse_body(resp, "探活").await?;
        body.get("version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| map_remote_err(&format!("探活 {node_ip}"), "响应缺 version"))
    }

    async fn deploy_etcd(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/etcd/deploy_async")?
            .json(&body)
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("deploy etcd @{node_ip}"), e))?;
        self.parse_body(resp, &format!("deploy etcd @{node_ip}")).await
    }

    async fn deploy_etcd_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/etcd/deploy")?
            .json(&body)
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("deploy etcd(同步) @{node_ip}"), e))?;
        self.parse_body(resp, &format!("deploy etcd(同步) @{node_ip}"))
            .await
    }

    async fn deploy_pg(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/pg/deploy_async")?
            .json(&body)
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("deploy pg @{node_ip}"), e))?;
        self.parse_body(resp, &format!("deploy pg @{node_ip}")).await
    }

    async fn deploy_pg_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/pg/deploy")?
            .json(&body)
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("deploy pg(同步) @{node_ip}"), e))?;
        self.parse_body(resp, &format!("deploy pg(同步) @{node_ip}"))
            .await
    }

    async fn remove_etcd(
        &self,
        node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/etcd/remove_async")?
            .json(&serde_json::json!({"compose_project_name": compose_project_name}))
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("remove etcd @{node_ip}"), e))?;
        self.parse_body(resp, &format!("remove etcd @{node_ip}")).await
    }

    async fn remove_pg(
        &self,
        node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/pg/remove_async")?
            .json(&serde_json::json!({"compose_project_name": compose_project_name}))
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("remove pg @{node_ip}"), e))?;
        self.parse_body(resp, &format!("remove pg @{node_ip}")).await
    }

    async fn deploy_haproxy_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/haproxy/deploy")?
            .json(&body)
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("deploy haproxy @{node_ip}"), e))?;
        self.parse_body(resp, &format!("deploy haproxy @{node_ip}")).await
    }

    async fn remove_haproxy_sync(
        &self,
        node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/haproxy/remove")?
            .json(&serde_json::json!({"compose_project_name": compose_project_name}))
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("remove haproxy @{node_ip}"), e))?;
        self.parse_body(resp, &format!("remove haproxy @{node_ip}")).await
    }

    async fn deploy_temporal_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/temporal/deploy")?
            .json(&body)
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("deploy temporal @{node_ip}"), e))?;
        self.parse_body(resp, &format!("deploy temporal @{node_ip}")).await
    }

    async fn remove_temporal_sync(
        &self,
        node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/temporal/remove")?
            .json(&serde_json::json!({"compose_project_name": compose_project_name}))
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("remove temporal @{node_ip}"), e))?;
        self.parse_body(resp, &format!("remove temporal @{node_ip}")).await
    }

    async fn list_instances(&self, node_ip: &str, kind: &str) -> Result<Vec<String>, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::GET, node_ip, &format!("infra/{}/instances", kind))?
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("list {kind} @{node_ip}"), e))?;
        let out = self
            .parse_body(resp, &format!("list {kind} @{node_ip}"))
            .await?;
        let names: Vec<String> = out["instances"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["compose_project_name"].as_str().map(|s| s.to_string()))
            .collect();
        Ok(names)
    }

    async fn reset_infra_data(&self, node_ip: &str) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/reset_data")?
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("reset_data @{node_ip}"), e))?;
        self.parse_body(resp, &format!("reset_data @{node_ip}")).await
    }

    async fn push_etcd_credential(
        &self,
        node_ip: &str,
        credential: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::POST, node_ip, "infra/etcd/auth/credentials")?
            .json(&credential)
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("push etcd credential @{node_ip}"), e))?;
        self.parse_body(resp, &format!("push etcd credential @{node_ip}"))
            .await
    }

    async fn etcd_dcs_ready(&self, node_ip: &str) -> Result<bool, VcmError> {
        let resp = self
            .request(&self.http, reqwest::Method::GET, node_ip, "infra/etcd/status")?
            .send()
            .await
            .map_err(|e| map_remote_err(&format!("etcd status @{node_ip}"), e))?;
        let out = self
            .parse_body(resp, &format!("etcd status @{node_ip}"))
            .await?;
        Ok(out["etcd_ok"].as_bool().unwrap_or(false))
    }

    async fn add_node_join(
        &self,
        seed_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        let ctx = format!("加入节点编排 @seed {seed_ip}");
        let resp = self
            .request(&self.http_long, reqwest::Method::POST, seed_ip, "nodes")?
            .json(&body)
            .send()
            .await
            .map_err(|e| map_remote_err(&ctx, e))?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| map_remote_err(&format!("{ctx} 读响应"), e))?;
        let out: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| map_remote_err(&format!("{ctx} 响应非 JSON"), e))?;
        let code = out["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            // 保留 seed 的 code 与 msg：调用方据此拼 20511，界面可直接展示
            let msg = out["msg"].as_str().unwrap_or("unknown");
            return Err(map_remote_err(&ctx, format!("[{code}] {msg}")));
        }
        Ok(out.get("data").cloned().unwrap_or(serde_json::Value::Null))
    }
}

// ============================================================================
// Mock 实现：内存状态机 + 故障注入（编排逻辑单测用）
// ============================================================================

pub struct MockRemoteNodeClient {
    inner: std::sync::Mutex<MockState>,
    /// 调用序记录（测试断言编排顺序，可与 MockEtcdClusterClient 共享同一 Arc）
    events: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[derive(Default)]
struct MockState {
    probes: std::collections::HashMap<String, String>, // ip → version
    /// 预置实例清单：node_ip → (kind → [compose_project_name])（present 语义测试）
    instances: std::collections::HashMap<String, std::collections::HashMap<String, Vec<String>>>,
    fail_next: bool,
    /// 按操作名持续注入故障（deploy_etcd / deploy_pg / deploy_haproxy / deploy_temporal）
    fail_ops: std::collections::HashSet<String>,
    unreachable: std::collections::HashSet<String>,
    /// 已下发的 etcd 凭据：(node_ip, credential)（单测断言分发内容，不含真实口令）
    pushed_credentials: Vec<(String, serde_json::Value)>,
    /// `add_node_join` 成功返回前的一次性钩子（单测用于模拟 seed 编排完成后
    /// 本机 etcd 状态变化——Mock 无法直接访问 etcd 客户端）
    add_node_join_hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl Default for MockRemoteNodeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRemoteNodeClient {
    pub fn new() -> Self {
        Self::with_event_log(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
    }

    /// 构造并绑定**共享**事件日志（与 `MockEtcdClusterClient::with_event_log`
    /// 共用同一 Arc 可断言跨客户端调用顺序，如 member_remove 先于容器 remove）
    pub fn with_event_log(events: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Self {
        Self {
            inner: std::sync::Mutex::new(MockState::default()),
            events,
        }
    }

    /// 预置可探活节点（ip → version）
    pub fn preload(&self, ips: &[&str]) {
        let mut m = self.inner.lock().unwrap();
        for ip in ips {
            m.probes.insert((*ip).to_string(), "0.1.0".to_string());
        }
    }

    /// 单次调用注入故障（任一方法下一次调用返回错误）
    pub fn set_fail_next(&self) {
        self.inner.lock().unwrap().fail_next = true;
    }

    /// 按操作名**持续**注入故障（如 "deploy_etcd" 让每次 etcd 部署都失败）
    pub fn set_fail_op(&self, op: &str, fail: bool) {
        let mut m = self.inner.lock().unwrap();
        if fail {
            m.fail_ops.insert(op.to_string());
        } else {
            m.fail_ops.remove(op);
        }
    }

    /// 标记节点不可达（探活失败）
    pub fn set_unreachable(&self, ips: &[&str]) {
        let mut m = self.inner.lock().unwrap();
        for ip in ips {
            m.unreachable.insert((*ip).to_string());
        }
    }

    /// 预置某节点的实例清单（present 语义测试：temporal deploy 后 refresh 纳入 backends）
    pub fn preload_instances(&self, node_ip: &str, kind: &str, names: &[&str]) {
        let mut m = self.inner.lock().unwrap();
        m.instances
            .entry(node_ip.to_string())
            .or_default()
            .insert(
                kind.to_string(),
                names.iter().map(|s| s.to_string()).collect(),
            );
    }

    /// 预置 `add_node_join` 成功回调（单测模拟 seed 编排把集群状态写进本机 etcd）
    pub fn set_add_node_join_hook(&self, hook: std::sync::Arc<dyn Fn() + Send + Sync>) {
        self.inner.lock().unwrap().add_node_join_hook = Some(hook);
    }

    /// 调用序记录（测试断言编排顺序）
    pub fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    /// 已下发的 etcd 凭据（identity, node_ip）
    pub fn pushed_credentials(&self) -> Vec<(String, serde_json::Value)> {
        self.inner.lock().unwrap().pushed_credentials.clone()
    }

    fn record(&self, event: &str) {
        self.events.lock().unwrap().push(event.to_string());
    }

    fn check_fail(&self, op: &str, ctx: &str) -> Result<(), VcmError> {
        let mut m = self.inner.lock().unwrap();
        if m.fail_ops.contains(op) {
            return Err(map_remote_err(ctx, "mock 持续故障注入"));
        }
        if m.fail_next {
            m.fail_next = false;
            return Err(map_remote_err(ctx, "mock 注入故障"));
        }
        Ok(())
    }

    fn ok<T: serde::Serialize>(v: T) -> Result<serde_json::Value, VcmError> {
        serde_json::to_value(v).map_err(|e| map_remote_err("mock 序列化", e))
    }
}

#[async_trait::async_trait]
impl RemoteNodeClient for MockRemoteNodeClient {
    async fn probe(&self, node_ip: &str) -> Result<String, VcmError> {
        self.record("remote:probe");
        let m = self.inner.lock().unwrap();
        if m.unreachable.contains(node_ip) {
            return Err(map_remote_err(&format!("探活 {node_ip}"), "mock 不可达"));
        }
        m.probes
            .get(node_ip)
            .cloned()
            .ok_or_else(|| map_remote_err(&format!("探活 {node_ip}"), "mock 未预置节点"))
    }

    async fn deploy_etcd(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:deploy_etcd");
        self.check_fail("deploy_etcd", &format!("deploy etcd @{node_ip}"))?;
        Self::ok(serde_json::json!({
            "task_id": format!("mock-etcd-{node_ip}"),
            "compose_project_name": body.get("compose_project_name")
        }))
    }

    async fn deploy_etcd_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:deploy_etcd_sync");
        self.check_fail("deploy_etcd", &format!("deploy etcd(同步) @{node_ip}"))?;
        // 同步形态：返回含 instance（对齐目标节点 deploy 同步端点）
        Self::ok(serde_json::json!({
            "instance": {
                "id": format!("mock-{node_ip}"),
                "app_id": "com.ictrek.vos-cluster-manager",
                "project_name": "etcd",
                "compose_project_name": body.get("compose_project_name"),
                "state": "deployed"
            }
        }))
    }

    async fn deploy_pg(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:deploy_pg");
        self.check_fail("deploy_pg", &format!("deploy pg @{node_ip}"))?;
        Self::ok(serde_json::json!({
            "task_id": format!("mock-pg-{node_ip}"),
            "compose_project_name": body.get("compose_project_name")
        }))
    }

    async fn deploy_pg_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:deploy_pg_sync");
        self.check_fail("deploy_pg", &format!("deploy pg(同步) @{node_ip}"))?;
        Self::ok(serde_json::json!({
            "instance": {
                "id": format!("mock-{node_ip}"),
                "app_id": "com.ictrek.vos-cluster-manager",
                "project_name": "postgres-ha",
                "compose_project_name": body.get("compose_project_name"),
                "state": "deployed"
            }
        }))
    }

    async fn remove_etcd(
        &self,
        _node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:remove_etcd");
        Self::ok(serde_json::json!({
            "removed": true,
            "compose_project_name": compose_project_name
        }))
    }

    async fn remove_pg(
        &self,
        _node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:remove_pg");
        Self::ok(serde_json::json!({
            "removed": true,
            "compose_project_name": compose_project_name
        }))
    }

    async fn deploy_haproxy_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:deploy_haproxy_sync");
        self.check_fail(
            "deploy_haproxy",
            &format!("deploy haproxy(同步) @{node_ip}"),
        )?;
        Self::ok(serde_json::json!({
            "instance": {
                "id": format!("mock-{node_ip}"),
                "app_id": "com.ictrek.vos-cluster-manager",
                "project_name": "haproxy",
                "compose_project_name": body.get("compose_project_name")
                    .cloned()
                    .unwrap_or(serde_json::json!(format!("cluster-haproxy-{}", node_ip.replace('.', "-")))),
                "state": "deployed"
            },
            "refreshed": false
        }))
    }

    async fn remove_haproxy_sync(
        &self,
        _node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:remove_haproxy");
        Self::ok(serde_json::json!({
            "removed": true,
            "compose_project_name": compose_project_name
        }))
    }

    async fn deploy_temporal_sync(
        &self,
        node_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:deploy_temporal_sync");
        self.check_fail(
            "deploy_temporal",
            &format!("deploy temporal(同步) @{node_ip}"),
        )?;
        Self::ok(serde_json::json!({
            "instance": {
                "id": format!("mock-{node_ip}"),
                "app_id": "com.ictrek.vos-cluster-manager",
                "project_name": "temporal",
                "compose_project_name": body.get("compose_project_name")
                    .cloned()
                    .unwrap_or(serde_json::json!(format!("cluster-temporal-{}", node_ip.replace('.', "-")))),
                "state": "deployed"
            },
            "refreshed": false
        }))
    }

    async fn remove_temporal_sync(
        &self,
        _node_ip: &str,
        compose_project_name: &str,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:remove_temporal");
        Self::ok(serde_json::json!({
            "removed": true,
            "compose_project_name": compose_project_name
        }))
    }

    async fn list_instances(&self, node_ip: &str, kind: &str) -> Result<Vec<String>, VcmError> {
        // haproxy/temporal：优先预置清单（present 语义测试控制）；无预置时 haproxy
        // 返回匹配 slug（S1 测试语义）、temporal 返回不匹配名（默认未部署）
        let pre = {
            let m = self.inner.lock().unwrap();
            m.instances.get(node_ip).and_then(|byk| byk.get(kind)).cloned()
        };
        if let Some(v) = pre {
            return Ok(v);
        }
        if kind == "haproxy" {
            return Ok(vec![format!(
                "cluster-haproxy-{}",
                node_ip.replace('.', "-")
            )]);
        }
        Ok(vec![format!("cluster-{kind}-mock-0")])
    }

    async fn reset_infra_data(&self, _node_ip: &str) -> Result<serde_json::Value, VcmError> {
        self.record("remote:reset_infra_data");
        Self::ok(serde_json::json!({"reset": true, "removed_dirs": []}))
    }

    async fn push_etcd_credential(
        &self,
        node_ip: &str,
        credential: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        self.record(&format!("remote:push_etcd_credential:{node_ip}"));
        self.check_fail(
            "push_etcd_credential",
            &format!("push etcd credential @{node_ip}"),
        )?;
        self.inner
            .lock()
            .unwrap()
            .pushed_credentials
            .push((node_ip.to_string(), credential));
        Self::ok(serde_json::json!({"written": true, "node_ip": node_ip}))
    }

    async fn etcd_dcs_ready(&self, _node_ip: &str) -> Result<bool, VcmError> {
        Ok(true) // Mock 探活通过（单测走全流程；真实就绪判定由 Real 验证）
    }

    async fn add_node_join(
        &self,
        seed_ip: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, VcmError> {
        self.record("remote:add_node_join");
        self.check_fail("add_node_join", &format!("加入节点编排 @seed {seed_ip}"))?;
        let hook = self.inner.lock().unwrap().add_node_join_hook.clone();
        if let Some(h) = hook {
            h();
        }
        Self::ok(serde_json::json!({
            "node": {
                "member_id": "mock-member-seed",
                "display_name": body.get("display_name").cloned().unwrap_or(serde_json::Value::Null),
                "role": body.get("role").cloned().unwrap_or(serde_json::json!("manager")),
                "node_ip": body.get("node_ip").cloned().unwrap_or(serde_json::Value::Null)
            },
            "code": 0
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_probe_preload_and_unreachable() {
        let c = MockRemoteNodeClient::new();
        c.preload(&["10.0.0.1", "10.0.0.2"]);
        assert_eq!(c.probe("10.0.0.1").await.unwrap(), "0.1.0");
        c.set_unreachable(&["10.0.0.2"]);
        assert!(c.probe("10.0.0.2").await.is_err());
        assert!(c.probe("10.0.0.3").await.is_err(), "未预置节点探活失败");
    }

    #[tokio::test]
    async fn mock_deploy_forward_and_fail_next() {
        let c = MockRemoteNodeClient::new();
        let out = c
            .deploy_etcd("10.0.0.1", serde_json::json!({"node_ip": "10.0.0.1"}))
            .await
            .unwrap();
        assert_eq!(out["task_id"], "mock-etcd-10.0.0.1");

        c.set_fail_next();
        assert!(c.deploy_pg("10.0.0.1", serde_json::json!({})).await.is_err());
        // 单次注入消费后恢复正常
        assert!(c.deploy_pg("10.0.0.1", serde_json::json!({})).await.is_ok());
    }

    #[tokio::test]
    async fn mock_remove() {
        let c = MockRemoteNodeClient::new();
        let out = c.remove_etcd("10.0.0.1", "cluster-etcd-node-1").await.unwrap();
        assert_eq!(out["removed"], true);
        assert_eq!(out["compose_project_name"], "cluster-etcd-node-1");
    }
}
