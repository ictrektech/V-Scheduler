//! etcd 集群状态客户端抽象（EtcdClusterClient trait + Real / Mock / Unavailable）
//!
//! - `RealEtcdClusterClient`：包装 `etcd-client` 0.19（与平台 P1 同系），单次操作 3s 超时
//! - `MockEtcdClusterClient`：内存 HashMap（测试与 Phase 4 编排 handler 复用），支持故障注入
//! - `UnavailableEtcdClusterClient`：etcd 端点未配置 / 启动连接失败时的降级实现
//!   （保证主容器始终可启动，`/api/health` 反映 etcd_ok=false）

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use etcd_client::{Compare, CompareOp, GetOptions, Txn, TxnOp};

use crate::error::codes;
use crate::error::VcmError;

use super::dto::{ClusterInfo, InfraStatus, NodeRecord, PgCredentials, VrrpAllocation};
use super::keys::{KEY_VRRP_VIPS_PREFIX, cluster_info_key, infra_status_key, node_key, vrrp_id_key, vrrp_vip_key};
use super::security::{EtcdConnectSpec, normalize_endpoints};

/// 单次 etcd 操作超时
const OP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(3000);

/// unhealthy cluster 判定：etcd 在 quorum 未达成 / raft 未稳定时的可重试错误
fn is_unhealthy_err(e: &VcmError) -> bool {
    let m = e.msg.to_lowercase();
    m.contains("unhealthy cluster") || m.contains("unhealthy") || m.contains("not leader")
}

/// raft 集群拓扑操作超时（member_add/member_remove/member_list；需要 leader 确认 +
/// quorum 复制，且可能在 etcd 子项目刚部署 leader 尚未就绪时调用，放宽到 30s）
const CLUSTER_OP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// 将 etcd 操作错误映射为 20504 统一错误
pub fn map_etcd_err(context: impl Into<String>, e: impl std::fmt::Display) -> VcmError {
    VcmError::new(codes::ETCD_FAILED, format!("{}: {e}", context.into()))
}

/// 由连接参数构造 `ConnectOptions`（TLS / 客户端证书 / 账号口令）
///
/// 采用 rustls（etcd-client `tls` feature → tonic/tls-ring），不依赖系统 OpenSSL。
/// 任何证书读取失败都映射为 20515（ETCD_AUTH_FAILED），提示可操作的排查路径。
pub fn build_connect_options(spec: &EtcdConnectSpec) -> Result<etcd_client::ConnectOptions, VcmError> {
    let mut opts = etcd_client::ConnectOptions::new()
        .with_connect_timeout(std::time::Duration::from_secs(5))
        .with_keep_alive(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(10),
        );

    if spec.mode.requires_tls() {
        let ca = spec.ca_cert.as_ref().ok_or_else(|| {
            VcmError::new(codes::ETCD_AUTH_FAILED, "缺少 CA 证书路径（TLS 模式必填）")
        })?;
        let ca_pem = std::fs::read(ca).map_err(|e| {
            VcmError::new(
                codes::ETCD_AUTH_FAILED,
                format!("读取 etcd CA 证书 {} 失败: {e}", ca.display()),
            )
        })?;
        let mut tls = etcd_client::TlsOptions::new()
            .ca_certificate(etcd_client::Certificate::from_pem(ca_pem));
        if let (Some(cert), Some(key)) = (&spec.client_cert, &spec.client_key) {
            let cert_pem = std::fs::read(cert).map_err(|e| {
                VcmError::new(
                    codes::ETCD_AUTH_FAILED,
                    format!("读取 etcd 客户端证书 {} 失败: {e}", cert.display()),
                )
            })?;
            let key_pem = std::fs::read(key).map_err(|e| {
                VcmError::new(
                    codes::ETCD_AUTH_FAILED,
                    format!("读取 etcd 客户端私钥 {} 失败: {e}", key.display()),
                )
            })?;
            tls = tls.identity(etcd_client::Identity::from_pem(cert_pem, key_pem));
        }
        opts = opts.with_tls(tls);
    }

    if let (Some(user), Some(password)) = (&spec.username, &spec.password) {
        opts = opts.with_user(user.clone(), password.clone());
    }
    Ok(opts)
}

/// etcd 业务客户端 trait（依赖注入，保证核心逻辑可测性）
#[async_trait]
pub trait EtcdClusterClient: Send + Sync {
    /// 读集群信息（`/vos/cluster/info`）
    async fn get_cluster_info(&self) -> Result<Option<ClusterInfo>, VcmError>;
    /// 写集群信息
    async fn put_cluster_info(&self, info: &ClusterInfo) -> Result<(), VcmError>;
    /// 删除集群信息（init 写入 node 失败时的补偿回滚，保持幂等可重试）
    async fn delete_cluster_info(&self) -> Result<(), VcmError>;
    /// 读单个节点记录
    async fn get_node_record(&self, member_id: &str) -> Result<Option<NodeRecord>, VcmError>;
    /// 读全部节点记录（prefix 扫描 `/vos/cluster/nodes/`）
    async fn get_all_node_records(&self) -> Result<Vec<NodeRecord>, VcmError>;
    /// 写节点记录
    async fn put_node_record(&self, member_id: &str, rec: &NodeRecord) -> Result<(), VcmError>;
    /// 删除节点记录
    async fn delete_node_record(&self, member_id: &str) -> Result<(), VcmError>;
    /// 写基础设施就绪位
    async fn put_infra_status(&self, st: &InfraStatus) -> Result<(), VcmError>;
    /// 读基础设施就绪位
    async fn get_infra_status(&self) -> Result<Option<InfraStatus>, VcmError>;
    /// 更新指定 node_ip 节点记录的 postgres-ha 凭据（Phase 3；未找到节点 → Ok(false) 不报错）
    async fn update_node_pg_credentials(
        &self,
        node_ip: &str,
        pg_superuser_password: String,
        pg_replication_password: String,
    ) -> Result<bool, VcmError>;
    /// 读集群级 postgres-ha 凭据（Phase 4；bootstrap 写入 / join 复用）
    async fn get_cluster_pg_credentials(&self) -> Result<Option<PgCredentials>, VcmError>;
    /// 写集群级 postgres-ha 凭据（Phase 4；bootstrap 部署时写入）
    async fn put_cluster_pg_credentials(&self, cred: &PgCredentials) -> Result<(), VcmError>;
    /// 连通性探测（/health 与 status 聚合用；不可达返回 false，不抛错）
    async fn ping(&self) -> bool;
    /// 本机 etcd 是否在服务（**串行读**；learner 阶段同样可用）。
    ///
    /// 与 `ping`（线性一致读）区分：etcd 的 learner 不参与投票，会直接拒绝线性一致
    /// 读/写（`etcdserver: rpc not supported for learner`）。join 流程中目标节点在
    /// promote 之前一直是 learner，若用线性一致读判"etcd 就绪"会永远为 false →
    /// promote 永不执行 → 死锁（2026-10-30 实机）。故就绪门禁必须用串行读。
    async fn ping_local(&self) -> bool;
    /// raft 成员数（Phase 2+ 实例聚合；不可达 → Err）
    async fn member_count(&self) -> Result<u64, VcmError>;
    /// 添加集群成员（Phase 4 编排；gRPC cluster API）→ `(member_id, name)`
    ///
    /// ⚠️ 该成员**立即拥有投票权**：单节点 / 偶数成员集群在目标节点 etcd 尚未在服时
    /// 调用会立刻抬高 quorum → 永久失 quorum（2026-10-30 实机事故）。成员加入编排
    /// 必须改用 `member_add_as_learner`（learner 不投票），待目标 etcd 就绪后再 promote。
    async fn member_add(&self, peer_url: &str) -> Result<(u64, String), VcmError>;
    /// 以 **learner**（raft 非投票成员）身份添加集群成员 → `(member_id, name)`。
    ///
    /// learner 只复制日志、**不参与投票、不影响 quorum**；因此可以在目标节点 etcd
    /// 尚未部署 / 尚未启动时安全地把它计入成员表——这是 join 不破坏 quorum 的关键。
    /// 待目标节点 etcd 启动并追平 leader 后，再用 `member_promote` 提升为投票成员。
    async fn member_add_as_learner(&self, peer_url: &str) -> Result<(u64, String), VcmError>;
    /// 把 learner 提升为 raft **投票成员**（member_promote）。
    ///
    /// etcd 要求 learner 已追平 leader（复制进度足够）才允许提升，否则返回
    /// "can not promote ... not enough progress" / unhealthy 类错误；调用方须在目标
    /// etcd 部署并就绪后按有界重试调用。已是投票成员时调用会失败（调用方用
    /// `member_is_learner` 先判定，保证幂等）。
    async fn member_promote(&self, member_id: u64) -> Result<(), VcmError>;
    /// 查询成员当前是否为 learner（raft 非投票成员）。
    ///
    /// 返回 `Ok(None)` = 成员不存在（join 幂等重入 / 回滚后判定用）；
    /// `Ok(Some(true))` = 仍是 learner（需要 promote）；`Ok(Some(false))` = 已是投票成员。
    async fn member_is_learner(&self, member_id: u64) -> Result<Option<bool>, VcmError>;
    /// 移除集群成员（Phase 4；幂等：已不在集群的成员返回 Err 由调用方容忍）
    async fn member_remove(&self, member_id: u64) -> Result<(), VcmError>;
    /// 按 peer URL 查找成员 id（Phase 4 成员移除；未找到 → Ok(None)）
    async fn find_member_id_by_peer_url(&self, peer_url: &str) -> Result<Option<u64>, VcmError>;
    /// 读取全局 VRRP 占用视图（etcd 权威）
    async fn list_vrrp_allocations(&self) -> Result<Vec<VrrpAllocation>, VcmError> {
        Err(VcmError::new(codes::ETCD_FAILED, "etcd 客户端未实现全局 VRRP 读取"))
    }
    /// 原子 claim 一个 VRRP 分配（同时写 VIP→ID 与 ID→VIP key）
    async fn claim_vrrp_allocation(&self, _alloc: &VrrpAllocation) -> Result<bool, VcmError> {
        Err(VcmError::new(codes::ETCD_FAILED, "etcd 客户端未实现全局 VRRP 分配"))
    }
    /// 原子释放一个 VRRP 分配（owner/id 必须匹配）
    async fn release_vrrp_allocation(
        &self,
        _vip: &str,
        _vrrp_id: u8,
        _owner: &str,
    ) -> Result<bool, VcmError> {
        Err(VcmError::new(codes::ETCD_FAILED, "etcd 客户端未实现全局 VRRP 释放"))
    }
}

/// learner → 投票成员提升（有界重试；幂等）。
///
/// etcd 只有在 learner 已追平 leader 后才允许 promote（否则 "can not promote ...
/// not enough progress"）。目标 etcd 刚部署时复制可能尚未追平，故按
/// `attempts` × `interval` 重试；成员**已是投票成员**时直接返回成功（activity /
/// handler 重试的幂等路径）。
pub async fn promote_learner_with_retry(
    client: &dyn EtcdClusterClient,
    member_id: u64,
    attempts: u32,
    interval: std::time::Duration,
) -> Result<(), VcmError> {
    let mut last_err: Option<VcmError> = None;
    for attempt in 0..attempts {
        match client.member_is_learner(member_id).await {
            // 已是投票成员（上一次 try 已 promote / 重入）→ 无需再提升
            Ok(Some(false)) => return Ok(()),
            Ok(Some(true)) => match client.member_promote(member_id).await {
                Ok(()) => {
                    tracing::info!(member_id, attempt, "learner 已提升为 raft 投票成员");
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        member_id,
                        attempt,
                        err = %e.msg,
                        "learner 尚未追平 leader / raft 未稳定，promote 重试"
                    );
                    last_err = Some(e);
                }
            },
            Ok(None) => {
                return Err(VcmError::new(
                    codes::NOT_FOUND,
                    format!("raft 成员 {member_id} 不存在（无法 promote）"),
                ));
            }
            Err(e) => {
                tracing::warn!(member_id, attempt, err = %e.msg, "查询 learner 状态失败，重试");
                last_err = Some(e);
            }
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(interval).await;
        }
    }
    // 重试耗尽后再确认一次（最后一次 promote 可能已生效）
    if let Ok(Some(false)) = client.member_is_learner(member_id).await {
        return Ok(());
    }
    Err(last_err.unwrap_or_else(|| {
        VcmError::new(
            codes::ETCD_FAILED,
            format!("learner {member_id} 提升为投票成员失败（{attempts} 次重试后仍未追平）"),
        )
    }))
}

// ============================================================================
// Real 实现：包装 etcd_client::Client
// ============================================================================

/// 真实 etcd 客户端（内部连接池并发安全，无需外部锁）
pub struct RealEtcdClusterClient {
    client: Arc<etcd_client::Client>,
}

impl RealEtcdClusterClient {
    /// 连接 etcd 集群。
    ///
    /// `endpoints` 形如 `["http://127.0.0.1:2379"]`。连接失败返回 Err（调用方决定降级
    /// 为 `UnavailableEtcdClusterClient`，保证主容器可启动）。
    pub async fn new(endpoints: &[String]) -> Result<Self, VcmError> {
        Self::new_with_spec(&EtcdConnectSpec::anonymous(endpoints)).await
    }

    /// 按认证/TLS 参数连接（D9：mTLS / 账号口令）
    pub async fn new_with_spec(spec: &EtcdConnectSpec) -> Result<Self, VcmError> {
        if spec.endpoints.is_empty() {
            return Err(VcmError::new(codes::ETCD_FAILED, "etcd 端点列表为空"));
        }
        let endpoints = normalize_endpoints(&spec.endpoints, spec.mode.requires_tls());
        let addrs: Vec<&str> = endpoints.iter().map(|s| s.as_str()).collect();
        let options = build_connect_options(spec)?;
        let client = etcd_client::Client::connect(&addrs, Some(options))
            .await
            .map_err(|e| map_etcd_err("etcd 连接失败", e))?;
        tracing::info!(
            endpoints = %endpoints.join(","),
            auth_mode = spec.mode.as_str(),
            user = spec.username.as_deref().unwrap_or("<none>"),
            "etcd client connected"
        );
        Ok(Self {
            client: Arc::new(client),
        })
    }

    /// 带超时的 kv get
    async fn kv_get(&self, key: &str) -> Result<Option<String>, VcmError> {
        let resp = tokio::time::timeout(OP_TIMEOUT, self.client.kv_client().get(key, None))
            .await
            .map_err(|_| VcmError::new(codes::ETCD_FAILED, format!("etcd get {key} 超时")))?
            .map_err(|e| map_etcd_err(format!("etcd get {key}"), e))?;
        Ok(resp
            .kvs()
            .first()
            .map(|kv| String::from_utf8_lossy(kv.value()).to_string()))
    }

    /// 带超时的 kv put
    async fn kv_put(&self, key: &str, value: &str) -> Result<(), VcmError> {
        tokio::time::timeout(OP_TIMEOUT, self.client.kv_client().put(key, value, None))
            .await
            .map_err(|_| VcmError::new(codes::ETCD_FAILED, format!("etcd put {key} 超时")))?
            .map_err(|e| map_etcd_err(format!("etcd put {key}"), e))
            .map(|_| ())
    }

    /// 带超时的 kv delete
    async fn kv_delete(&self, key: &str) -> Result<(), VcmError> {
        tokio::time::timeout(OP_TIMEOUT, self.client.kv_client().delete(key, None))
            .await
            .map_err(|_| VcmError::new(codes::ETCD_FAILED, format!("etcd delete {key} 超时")))?
            .map_err(|e| map_etcd_err(format!("etcd delete {key}"), e))
            .map(|_| ())
    }
}

#[async_trait]
impl EtcdClusterClient for RealEtcdClusterClient {
    async fn get_cluster_info(&self) -> Result<Option<ClusterInfo>, VcmError> {
        let raw = self.kv_get(cluster_info_key()).await?;
        match raw {
            None => Ok(None),
            Some(s) => serde_json::from_str(&s)
                .map(Some)
                .map_err(|e| map_etcd_err("解析 /vos/cluster/info", e)),
        }
    }

    async fn put_cluster_info(&self, info: &ClusterInfo) -> Result<(), VcmError> {
        let s = serde_json::to_string(info).map_err(|e| map_etcd_err("序列化 ClusterInfo", e))?;
        self.kv_put(cluster_info_key(), &s).await
    }

    async fn delete_cluster_info(&self) -> Result<(), VcmError> {
        self.kv_delete(cluster_info_key()).await
    }

    async fn get_node_record(&self, member_id: &str) -> Result<Option<NodeRecord>, VcmError> {
        let key = node_key(member_id);
        let raw = self.kv_get(&key).await?;
        match raw {
            None => Ok(None),
            Some(s) => serde_json::from_str(&s)
                .map(Some)
                .map_err(|e| map_etcd_err(format!("解析 {key}"), e)),
        }
    }

    async fn get_all_node_records(&self) -> Result<Vec<NodeRecord>, VcmError> {
        use etcd_client::GetOptions;
        let opts = GetOptions::new().with_prefix();
        let resp = tokio::time::timeout(
            OP_TIMEOUT,
            self.client
                .kv_client()
                .get(super::keys::KEY_NODE_PREFIX, Some(opts)),
        )
        .await
        .map_err(|_| VcmError::new(codes::ETCD_FAILED, "etcd prefix get 超时"))?
        .map_err(|e| map_etcd_err("etcd prefix get /vos/cluster/nodes/", e))?;

        let mut out = Vec::new();
        for kv in resp.kvs() {
            let s = String::from_utf8_lossy(kv.value());
            match serde_json::from_str::<NodeRecord>(&s) {
                Ok(rec) => out.push(rec),
                Err(e) => {
                    tracing::warn!(
                        key = %String::from_utf8_lossy(kv.key()),
                        "跳过解析失败的节点记录: {e}"
                    );
                }
            }
        }
        Ok(out)
    }

    async fn put_node_record(&self, member_id: &str, rec: &NodeRecord) -> Result<(), VcmError> {
        let s = serde_json::to_string(rec).map_err(|e| map_etcd_err("序列化 NodeRecord", e))?;
        self.kv_put(&node_key(member_id), &s).await
    }

    async fn delete_node_record(&self, member_id: &str) -> Result<(), VcmError> {
        self.kv_delete(&node_key(member_id)).await
    }

    async fn put_infra_status(&self, st: &InfraStatus) -> Result<(), VcmError> {
        let s = serde_json::to_string(st).map_err(|e| map_etcd_err("序列化 InfraStatus", e))?;
        self.kv_put(infra_status_key(), &s).await
    }

    async fn get_infra_status(&self) -> Result<Option<InfraStatus>, VcmError> {
        let raw = self.kv_get(infra_status_key()).await?;
        match raw {
            None => Ok(None),
            Some(s) => serde_json::from_str(&s)
                .map(Some)
                .map_err(|e| map_etcd_err("解析 /vos/cluster/infra_status", e)),
        }
    }

    async fn update_node_pg_credentials(
        &self,
        node_ip: &str,
        pg_superuser_password: String,
        pg_replication_password: String,
    ) -> Result<bool, VcmError> {
        let recs = self.get_all_node_records().await?;
        let Some(mut rec) = recs.into_iter().find(|r| r.metadata.node_ip == node_ip) else {
            return Ok(false);
        };
        rec.credentials.pg_superuser_password = Some(pg_superuser_password);
        rec.credentials.pg_replication_password = Some(pg_replication_password);
        rec.credentials.updated_at = chrono::Utc::now().to_rfc3339();
        self.put_node_record(&rec.metadata.member_id, &rec).await?;
        Ok(true)
    }

    async fn get_cluster_pg_credentials(&self) -> Result<Option<PgCredentials>, VcmError> {
        let raw = self.kv_get(super::keys::pg_credentials_key()).await?;
        match raw {
            None => Ok(None),
            Some(s) => serde_json::from_str(&s)
                .map(Some)
                .map_err(|e| map_etcd_err("解析 /vos/cluster/pg_credentials", e)),
        }
    }

    async fn put_cluster_pg_credentials(&self, cred: &PgCredentials) -> Result<(), VcmError> {
        let s = serde_json::to_string(cred).map_err(|e| map_etcd_err("序列化 PgCredentials", e))?;
        self.kv_put(super::keys::pg_credentials_key(), &s).await
    }

    async fn ping(&self) -> bool {
        // etcd-client 无独立 ping；用 kv get 一个不存在的 key 验证连接可用
        self.kv_get("/vos/cluster/.ping").await.is_ok()
    }

    async fn ping_local(&self) -> bool {
        probe_client_serializable(&self.client).await
    }

async fn member_count(&self) -> Result<u64, VcmError> {
        let resp = tokio::time::timeout(
            OP_TIMEOUT,
            self.client.cluster_client().member_list(),
        )
        .await
        .map_err(|_| VcmError::new(codes::ETCD_FAILED, "etcd member_list 超时"))?
        .map_err(|e| map_etcd_err("etcd member_list", e))?;
        Ok(resp.members().len() as u64)
    }

    async fn member_add(&self, peer_url: &str) -> Result<(u64, String), VcmError> {
        let peer_url_clone = peer_url.to_string();
        // unhealthy cluster（刚加入成员 raft 未稳定 / quorum 未达成）时重试：
        // kv probe 是 serializable 读（不依赖 quorum），等待逻辑无法发现 unhealthy，
        // 故业务层对 unhealthy/unavailable 错误重试最多 5 次 × 3s
        let mut last_err: Option<VcmError> = None;
        for attempt in 0..5 {
            let peer = peer_url_clone.clone();
            let out = tokio::time::timeout(CLUSTER_OP_TIMEOUT, async move {
                let mut cc = self.client.cluster_client();
                let resp = cc
                    .member_add(vec![peer.clone()], None)
                    .await
                    .map_err(|e| map_etcd_err("member_add", e))?;
                let m = resp.member().ok_or_else(|| {
                    VcmError::new(codes::ETCD_FAILED, "member_add 响应缺 member")
                })?;
                Ok::<(u64, String), VcmError>((m.id(), m.name().to_string()))
            })
            .await;
            match out {
                Ok(Ok(v)) => {
                    tracing::info!(peer_url = %peer_url, member_id = v.0, "etcd member_add 成功");
                    return Ok(v);
                }
                Ok(Err(e)) if is_unhealthy_err(&e) => {
                    tracing::warn!(
                        attempt,
                        err = %e.msg,
                        "etcd member_add 遇 unhealthy cluster，3s 后重试"
                    );
                    last_err = Some(e);
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    last_err = Some(VcmError::new(
                        codes::ETCD_FAILED,
                        "etcd member_add 超时",
                    ));
                    tracing::warn!(attempt, "etcd member_add 超时，3s 后重试");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        Err(last_err.unwrap_or_else(|| VcmError::new(codes::ETCD_FAILED, "etcd member_add 失败")))
    }

    async fn member_add_as_learner(&self, peer_url: &str) -> Result<(u64, String), VcmError> {
        // learner 不参与投票 → 加入不会抬高 quorum，因此即便 raft 正在重配也可安全重试
        // （错误同样按 unhealthy 5 次 × 3s 重试）。
        let mut last_err: Option<VcmError> = None;
        for attempt in 0..5 {
            let peer = peer_url.to_string();
            let out = tokio::time::timeout(CLUSTER_OP_TIMEOUT, async move {
                let mut cc = self.client.cluster_client();
                let resp = cc
                    .member_add(
                        vec![peer],
                        Some(etcd_client::MemberAddOptions::new().with_is_learner()),
                    )
                    .await
                    .map_err(|e| map_etcd_err("member_add(learner)", e))?;
                let m = resp.member().ok_or_else(|| {
                    VcmError::new(codes::ETCD_FAILED, "member_add(learner) 响应缺 member")
                })?;
                Ok::<(u64, String), VcmError>((m.id(), m.name().to_string()))
            })
            .await;
            match out {
                Ok(Ok(v)) => {
                    tracing::info!(
                        peer_url = %peer_url,
                        member_id = v.0,
                        "etcd member_add(learner) 成功（不参与投票，quorum 安全）"
                    );
                    return Ok(v);
                }
                Ok(Err(e)) if is_unhealthy_err(&e) => {
                    tracing::warn!(
                        attempt,
                        err = %e.msg,
                        "etcd member_add(learner) 遇 unhealthy cluster，3s 后重试"
                    );
                    last_err = Some(e);
                }
                Ok(Err(e)) => {
                    // add 报错 / leader 切换时请求可能已提交：复查成员表，已存在即按成功处理
                    if let Ok(Some(id)) = self.find_member_id_by_peer_url(peer_url).await {
                        tracing::warn!(
                            peer_url = %peer_url,
                            member_id = id,
                            err = %e.msg,
                            "member_add(learner) 报错但成员已存在，按成功处理"
                        );
                        return Ok((id, String::new()));
                    }
                    return Err(e);
                }
                Err(_) => {
                    last_err = Some(VcmError::new(
                        codes::ETCD_FAILED,
                        "etcd member_add(learner) 超时",
                    ));
                    tracing::warn!(attempt, "etcd member_add(learner) 超时，3s 后重试");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        // 最后一次超时可能已提交（leader 尚未返回响应）→ 复查成员表
        if let Ok(Some(id)) = self.find_member_id_by_peer_url(peer_url).await {
            tracing::warn!(
                peer_url = %peer_url,
                member_id = id,
                "member_add(learner) 重试耗尽但成员已存在，按成功处理"
            );
            return Ok((id, String::new()));
        }
        Err(last_err
            .unwrap_or_else(|| VcmError::new(codes::ETCD_FAILED, "etcd member_add(learner) 失败")))
    }

    async fn member_promote(&self, member_id: u64) -> Result<(), VcmError> {
        tokio::time::timeout(CLUSTER_OP_TIMEOUT, async move {
            self.client
                .cluster_client()
                .member_promote(member_id)
                .await
                .map_err(|e| map_etcd_err("member_promote", e))?;
            Ok(())
        })
        .await
        .map_err(|_| VcmError::new(codes::ETCD_FAILED, "etcd member_promote 超时"))?
    }

    async fn member_is_learner(&self, member_id: u64) -> Result<Option<bool>, VcmError> {
        let out = tokio::time::timeout(CLUSTER_OP_TIMEOUT, async move {
            let resp = self
                .client
                .cluster_client()
                .member_list()
                .await
                .map_err(|e| map_etcd_err("member_list", e))?;
            Ok::<Option<bool>, VcmError>(
                resp.members()
                    .iter()
                    .find(|m| m.id() == member_id)
                    .map(|m| m.is_learner()),
            )
        })
        .await
        .map_err(|_| VcmError::new(codes::ETCD_FAILED, "etcd member_list 超时"))??;
        Ok(out)
    }

    async fn member_remove(&self, member_id: u64) -> Result<(), VcmError> {
        tokio::time::timeout(CLUSTER_OP_TIMEOUT, async move {
            self.client
                .cluster_client()
                .member_remove(member_id)
                .await
                .map_err(|e| map_etcd_err("member_remove", e))?;
            Ok(())
        })
        .await
        .map_err(|_| VcmError::new(codes::ETCD_FAILED, "etcd member_remove 超时"))?
    }

    async fn find_member_id_by_peer_url(&self, peer_url: &str) -> Result<Option<u64>, VcmError> {
        let target = peer_url.to_string();
        let out = tokio::time::timeout(CLUSTER_OP_TIMEOUT, async move {
            let resp = self.client.cluster_client().member_list().await.map_err(|e| {
                map_etcd_err("member_list", e)
            })?;
            Ok::<Option<u64>, VcmError>(
                resp.members()
                    .iter()
                    .find(|m| m.peer_urls().contains(&target))
                    .map(|m| m.id()),
            )
        })
        .await
        .map_err(|_| VcmError::new(codes::ETCD_FAILED, "etcd member_list 超时"))??;
        Ok(out)
    }

    async fn list_vrrp_allocations(&self) -> Result<Vec<VrrpAllocation>, VcmError> {
        let resp = tokio::time::timeout(
            OP_TIMEOUT,
            self.client
                .kv_client()
                .get(KEY_VRRP_VIPS_PREFIX, Some(GetOptions::new().with_prefix())),
        )
        .await
        .map_err(|_| VcmError::new(codes::ETCD_FAILED, "etcd 读取全局 VRRP 超时"))?
        .map_err(|e| map_etcd_err("etcd 读取全局 VRRP", e))?;
        let mut out = Vec::new();
        for kv in resp.kvs() {
            let text = String::from_utf8_lossy(kv.value());
            if let Ok(alloc) = serde_json::from_str::<VrrpAllocation>(&text) {
                out.push(alloc);
            }
        }
        Ok(out)
    }

    async fn claim_vrrp_allocation(&self, alloc: &VrrpAllocation) -> Result<bool, VcmError> {
        let vip_key = vrrp_vip_key(&alloc.vip);
        let id_key = vrrp_id_key(alloc.vrrp_id);
        let vip_value = serde_json::to_string(alloc)
            .map_err(|e| VcmError::new(codes::ETCD_FAILED, format!("序列化 VRRP 分配失败: {e}")))?;
        let id_value = vip_value.clone();
        let txn = Txn::new()
            .when(vec![
                Compare::create_revision(vip_key.clone(), CompareOp::Equal, 0),
                Compare::create_revision(id_key.clone(), CompareOp::Equal, 0),
            ])
            .and_then(vec![
                TxnOp::put(vip_key, vip_value, None),
                TxnOp::put(id_key, id_value, None),
            ]);
        let resp = tokio::time::timeout(OP_TIMEOUT, self.client.kv_client().txn(txn))
            .await
            .map_err(|_| VcmError::new(codes::ETCD_FAILED, "etcd 全局 VRRP claim 超时"))?
            .map_err(|e| map_etcd_err("etcd 全局 VRRP claim", e))?;
        Ok(resp.succeeded())
    }

    async fn release_vrrp_allocation(
        &self,
        vip: &str,
        vrrp_id: u8,
        owner: &str,
    ) -> Result<bool, VcmError> {
        let vip_key = vrrp_vip_key(vip);
        let id_key = vrrp_id_key(vrrp_id);
        let Some(raw) = self.kv_get(&vip_key).await? else {
            return Ok(false);
        };
        let current: VrrpAllocation = serde_json::from_str(&raw)
            .map_err(|e| VcmError::new(codes::ETCD_FAILED, format!("解析 VRRP 分配失败: {e}")))?;
        if current.vrrp_id != vrrp_id || current.owner != owner {
            return Err(VcmError::new(
                codes::PARAM_INVALID,
                format!("VRRP 释放 owner/id 不匹配: vip={vip}"),
            ));
        }
        let txn = Txn::new()
            .when(vec![Compare::value(vip_key.clone(), CompareOp::Equal, raw)])
            .and_then(vec![
                TxnOp::delete(vip_key, None),
                TxnOp::delete(id_key, None),
            ]);
        let resp = tokio::time::timeout(OP_TIMEOUT, self.client.kv_client().txn(txn))
            .await
            .map_err(|_| VcmError::new(codes::ETCD_FAILED, "etcd 全局 VRRP release 超时"))?
            .map_err(|e| map_etcd_err("etcd 全局 VRRP release", e))?;
        Ok(resp.succeeded())
    }
}

// ============================================================================
// Mock 实现：内存 HashMap（可测性 / Phase 4 编排 handler 复用）
// ============================================================================

/// 内存 Mock etcd 客户端（`set_fail_next()` 注入下一次 kv 操作失败）
pub struct MockEtcdClusterClient {
    kv: Arc<StdMutex<HashMap<String, String>>>,
    /// raft 成员表（member_add/remove/member_count 用；Phase 4）
    members: Arc<StdMutex<Vec<(u64, String, String)>>>,
    /// raft learner 子集（member_add_as_learner / member_promote / member_is_learner）
    learners: Arc<StdMutex<HashSet<u64>>>,
    /// 调用序记录（测试断言 learner → deploy → promote / member_remove 先于容器 remove）
    events: Arc<StdMutex<Vec<String>>>,
    next_member_id: Arc<AtomicU64>,
    fail_next: AtomicBool,
    /// promote 持续失败（模拟 learner 尚未追平 leader）
    promote_failing: AtomicBool,
    /// member_remove 持续失败（模拟 raft 变更无法提交）
    member_remove_failing: AtomicBool,
    /// ping 返回 false（模拟 etcd 不可达）
    unavailable: AtomicBool,
}

impl MockEtcdClusterClient {
    pub fn new() -> Self {
        Self::with_event_log(Arc::new(StdMutex::new(Vec::new())))
    }

    /// 构造并绑定**共享**事件日志（与 `MockRemoteNodeClient::with_event_log` 共用同
    /// 一个 Arc，即可断言跨客户端的调用顺序，如 member_remove 先于容器 remove）
    pub fn with_event_log(events: Arc<StdMutex<Vec<String>>>) -> Self {
        Self {
            kv: Arc::new(StdMutex::new(HashMap::new())),
            members: Arc::new(StdMutex::new(Vec::new())),
            learners: Arc::new(StdMutex::new(HashSet::new())),
            events,
            next_member_id: Arc::new(AtomicU64::new(1)),
            fail_next: AtomicBool::new(false),
            promote_failing: AtomicBool::new(false),
            member_remove_failing: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
        }
    }

    /// 注入故障：下一次 kv 操作返回 ETCD_FAILED（不影响 ping）
    pub fn set_fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    /// 让 `member_promote` 持续失败（模拟 learner 未追平 leader → promote 不可用）
    pub fn set_promote_failing(&self, fail: bool) {
        self.promote_failing.store(fail, Ordering::SeqCst);
    }

    /// 注入 `member_remove` 持续失败（模拟 raft 成员变更无法提交）
    pub fn set_member_remove_failing(&self, fail: bool) {
        self.member_remove_failing.store(fail, Ordering::SeqCst);
    }

    /// 模拟 etcd 整体不可达（ping=false + 所有操作失败）
    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }

    /// 调用序记录（测试断言编排顺序）
    pub fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn record(&self, event: &str) {
        self.events.lock().unwrap().push(event.to_string());
    }

    /// 检查并消费故障标志
    fn check_fail(&self) -> Result<(), VcmError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(VcmError::new(codes::ETCD_FAILED, "etcd 模拟不可达"));
        }
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(VcmError::new(codes::ETCD_FAILED, "etcd 模拟故障注入"));
        }
        Ok(())
    }

    /// 测试辅助：直接查看底层数据（key → json）
    pub fn raw(&self) -> HashMap<String, String> {
        self.kv.lock().unwrap().clone()
    }

    /// 测试辅助：**同步**写入 `/vos/cluster/info`（供跨客户端联动场景——如 Mock seed
    /// 编排完成后本机 etcd 立即可见集群状态；异步 `put_cluster_info` 无法在同步钩子内调用）
    pub fn seed_cluster_info(&self, info: &ClusterInfo) {
        let Ok(s) = serde_json::to_string(info) else {
            return;
        };
        self.kv
            .lock()
            .unwrap()
            .insert(cluster_info_key().to_string(), s);
    }
}

impl Default for MockEtcdClusterClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EtcdClusterClient for MockEtcdClusterClient {
    async fn get_cluster_info(&self) -> Result<Option<ClusterInfo>, VcmError> {
        self.check_fail()?;
        let map = self.kv.lock().unwrap();
        Ok(map
            .get(cluster_info_key())
            .and_then(|s| serde_json::from_str::<ClusterInfo>(s).ok()))
    }

    async fn put_cluster_info(&self, info: &ClusterInfo) -> Result<(), VcmError> {
        self.check_fail()?;
        let s = serde_json::to_string(info).map_err(|e| map_etcd_err("序列化", e))?;
        self.kv.lock().unwrap().insert(cluster_info_key().to_string(), s);
        Ok(())
    }

    async fn delete_cluster_info(&self) -> Result<(), VcmError> {
        self.check_fail()?;
        self.kv.lock().unwrap().remove(cluster_info_key());
        Ok(())
    }

    async fn get_node_record(&self, member_id: &str) -> Result<Option<NodeRecord>, VcmError> {
        self.check_fail()?;
        let map = self.kv.lock().unwrap();
        Ok(map
            .get(&node_key(member_id))
            .and_then(|s| serde_json::from_str::<NodeRecord>(s).ok()))
    }

    async fn get_all_node_records(&self) -> Result<Vec<NodeRecord>, VcmError> {
        self.check_fail()?;
        let map = self.kv.lock().unwrap();
        let mut out = Vec::new();
        for (k, v) in map.iter() {
            if k.starts_with(super::keys::KEY_NODE_PREFIX)
                && let Ok(rec) = serde_json::from_str::<NodeRecord>(v)
            {
                out.push(rec);
            }
        }
        Ok(out)
    }

    async fn put_node_record(&self, member_id: &str, rec: &NodeRecord) -> Result<(), VcmError> {
        self.check_fail()?;
        let s = serde_json::to_string(rec).map_err(|e| map_etcd_err("序列化", e))?;
        self.kv.lock().unwrap().insert(node_key(member_id), s);
        Ok(())
    }

    async fn delete_node_record(&self, member_id: &str) -> Result<(), VcmError> {
        self.check_fail()?;
        self.kv.lock().unwrap().remove(&node_key(member_id));
        Ok(())
    }

    async fn put_infra_status(&self, st: &InfraStatus) -> Result<(), VcmError> {
        self.check_fail()?;
        let s = serde_json::to_string(st).map_err(|e| map_etcd_err("序列化", e))?;
        self.kv.lock().unwrap().insert(infra_status_key().to_string(), s);
        Ok(())
    }

    async fn get_infra_status(&self) -> Result<Option<InfraStatus>, VcmError> {
        self.check_fail()?;
        let map = self.kv.lock().unwrap();
        Ok(map
            .get(infra_status_key())
            .and_then(|s| serde_json::from_str::<InfraStatus>(s).ok()))
    }

    async fn update_node_pg_credentials(
        &self,
        node_ip: &str,
        pg_superuser_password: String,
        pg_replication_password: String,
    ) -> Result<bool, VcmError> {
        self.check_fail()?;
        let mut map = self.kv.lock().unwrap();
        let mut found = false;
        let mut updates: Vec<(String, NodeRecord)> = Vec::new();
        for (k, v) in map.iter() {
            if k.starts_with(super::keys::KEY_NODE_PREFIX)
                && let Ok(mut rec) = serde_json::from_str::<NodeRecord>(v)
                && rec.metadata.node_ip == node_ip
            {
                rec.credentials.pg_superuser_password = Some(pg_superuser_password.clone());
                rec.credentials.pg_replication_password = Some(pg_replication_password.clone());
                rec.credentials.updated_at = chrono::Utc::now().to_rfc3339();
                updates.push((k.clone(), rec));
                found = true;
                break;
            }
        }
        for (k, rec) in updates {
            let s = serde_json::to_string(&rec).map_err(|e| map_etcd_err("序列化", e))?;
            map.insert(k, s);
        }
        Ok(found)
    }

    async fn get_cluster_pg_credentials(&self) -> Result<Option<PgCredentials>, VcmError> {
        self.check_fail()?;
        let map = self.kv.lock().unwrap();
        Ok(map
            .get(super::keys::pg_credentials_key())
            .and_then(|s| serde_json::from_str::<PgCredentials>(s).ok()))
    }

    async fn put_cluster_pg_credentials(&self, cred: &PgCredentials) -> Result<(), VcmError> {
        self.check_fail()?;
        let s = serde_json::to_string(cred).map_err(|e| map_etcd_err("序列化", e))?;
        self.kv
            .lock()
            .unwrap()
            .insert(super::keys::pg_credentials_key().to_string(), s);
        Ok(())
    }

    async fn ping(&self) -> bool {
        !self.unavailable.load(Ordering::SeqCst)
    }

    async fn ping_local(&self) -> bool {
        !self.unavailable.load(Ordering::SeqCst)
    }

    async fn member_count(&self) -> Result<u64, VcmError> {
        self.check_fail()?;
        Ok(self.members.lock().unwrap().len() as u64 + 1) // +1 虚拟 bootstrap 节点
    }

    async fn member_add(&self, peer_url: &str) -> Result<(u64, String), VcmError> {
        self.check_fail()?;
        self.record("etcd:member_add");
        let id = self.next_member_id.fetch_add(1, Ordering::SeqCst);
        let name = format!("member-{id}");
        self.members
            .lock()
            .unwrap()
            .push((id, name.clone(), peer_url.to_string()));
        Ok((id, name))
    }

    async fn member_add_as_learner(&self, peer_url: &str) -> Result<(u64, String), VcmError> {
        self.check_fail()?;
        self.record("etcd:member_add_as_learner");
        let id = self.next_member_id.fetch_add(1, Ordering::SeqCst);
        let name = format!("member-{id}");
        self.members
            .lock()
            .unwrap()
            .push((id, name.clone(), peer_url.to_string()));
        self.learners.lock().unwrap().insert(id);
        Ok((id, name))
    }

    async fn member_promote(&self, member_id: u64) -> Result<(), VcmError> {
        self.check_fail()?;
        self.record("etcd:member_promote");
        if self.promote_failing.load(Ordering::SeqCst) {
            return Err(VcmError::new(
                codes::ETCD_FAILED,
                "模拟 learner 尚未追平 leader（can not promote）",
            ));
        }
        let known = self
            .members
            .lock()
            .unwrap()
            .iter()
            .any(|(id, _, _)| *id == member_id);
        if !known {
            return Err(VcmError::new(
                codes::NOT_FOUND,
                format!("member_promote: 成员 {member_id} 不存在"),
            ));
        }
        self.learners.lock().unwrap().remove(&member_id);
        Ok(())
    }

    async fn member_is_learner(&self, member_id: u64) -> Result<Option<bool>, VcmError> {
        self.check_fail()?;
        let known = self
            .members
            .lock()
            .unwrap()
            .iter()
            .any(|(id, _, _)| *id == member_id);
        if !known {
            return Ok(None);
        }
        Ok(Some(self.learners.lock().unwrap().contains(&member_id)))
    }

    async fn member_remove(&self, member_id: u64) -> Result<(), VcmError> {
        self.check_fail()?;
        self.record("etcd:member_remove");
        if self.member_remove_failing.load(Ordering::SeqCst) {
            return Err(VcmError::new(codes::ETCD_FAILED, "mock: member_remove 持续失败"));
        }
        self.members
            .lock()
            .unwrap()
            .retain(|(id, _, _)| *id != member_id);
        self.learners.lock().unwrap().remove(&member_id);
        Ok(())
    }

    async fn find_member_id_by_peer_url(&self, peer_url: &str) -> Result<Option<u64>, VcmError> {
        self.check_fail()?;
        let m = self.members.lock().unwrap();
        Ok(m.iter().find(|(_, _, p)| p == peer_url).map(|(id, _, _)| *id))
    }

    async fn list_vrrp_allocations(&self) -> Result<Vec<VrrpAllocation>, VcmError> {
        self.check_fail()?;
        let map = self.kv.lock().unwrap();
        let mut out = Vec::new();
        for (k, v) in map.iter() {
            if k.starts_with(KEY_VRRP_VIPS_PREFIX)
                && let Ok(alloc) = serde_json::from_str::<VrrpAllocation>(v)
            {
                out.push(alloc);
            }
        }
        Ok(out)
    }

    async fn claim_vrrp_allocation(&self, alloc: &VrrpAllocation) -> Result<bool, VcmError> {
        self.check_fail()?;
        let mut map = self.kv.lock().unwrap();
        let vip_key = vrrp_vip_key(&alloc.vip);
        let id_key = vrrp_id_key(alloc.vrrp_id);
        if map.contains_key(&vip_key) || map.contains_key(&id_key) {
            return Ok(false);
        }
        let value = serde_json::to_string(alloc)
            .map_err(|e| map_etcd_err("序列化 VRRP 分配", e))?;
        map.insert(vip_key, value.clone());
        map.insert(id_key, value);
        Ok(true)
    }

    async fn release_vrrp_allocation(
        &self,
        vip: &str,
        vrrp_id: u8,
        owner: &str,
    ) -> Result<bool, VcmError> {
        self.check_fail()?;
        let mut map = self.kv.lock().unwrap();
        let vip_key = vrrp_vip_key(vip);
        let Some(raw) = map.get(&vip_key).cloned() else {
            return Ok(false);
        };
        let current: VrrpAllocation = serde_json::from_str(&raw)
            .map_err(|e| map_etcd_err("解析 VRRP 分配", e))?;
        if current.vrrp_id != vrrp_id || current.owner != owner {
            return Err(VcmError::new(
                codes::PARAM_INVALID,
                format!("VRRP 释放 owner/id 不匹配: vip={vip}"),
            ));
        }
        map.remove(&vip_key);
        map.remove(&vrrp_id_key(vrrp_id));
        Ok(true)
    }
}

// ============================================================================
// Unavailable：etcd 端点未配置 / 启动连接失败时的降级实现
// ============================================================================

/// etcd 不可用降级实现：主容器保证可启动，所有操作返回 20504，ping=false
pub struct UnavailableEtcdClusterClient {
    reason: String,
}

impl UnavailableEtcdClusterClient {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl EtcdClusterClient for UnavailableEtcdClusterClient {
    async fn get_cluster_info(&self) -> Result<Option<ClusterInfo>, VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn put_cluster_info(&self, _: &ClusterInfo) -> Result<(), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn delete_cluster_info(&self) -> Result<(), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn get_node_record(&self, _: &str) -> Result<Option<NodeRecord>, VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn get_all_node_records(&self) -> Result<Vec<NodeRecord>, VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn put_node_record(&self, _: &str, _: &NodeRecord) -> Result<(), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn delete_node_record(&self, _: &str) -> Result<(), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn put_infra_status(&self, _: &InfraStatus) -> Result<(), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn get_infra_status(&self) -> Result<Option<InfraStatus>, VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn update_node_pg_credentials(
        &self,
        _: &str,
        _: String,
        _: String,
    ) -> Result<bool, VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }

    async fn get_cluster_pg_credentials(&self) -> Result<Option<PgCredentials>, VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }

    async fn put_cluster_pg_credentials(&self, _: &PgCredentials) -> Result<(), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
    async fn ping(&self) -> bool {
        false
    }

    async fn ping_local(&self) -> bool {
        false
    }

    async fn member_count(&self) -> Result<u64, VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }

    async fn member_add(&self, _: &str) -> Result<(u64, String), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }

    async fn member_add_as_learner(&self, _: &str) -> Result<(u64, String), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }

    async fn member_promote(&self, _: u64) -> Result<(), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }

    async fn member_is_learner(&self, _: u64) -> Result<Option<bool>, VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }

    async fn member_remove(&self, _: u64) -> Result<(), VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }

    async fn find_member_id_by_peer_url(&self, _: &str) -> Result<Option<u64>, VcmError> {
        Err(VcmError::new(
            codes::ETCD_FAILED,
            format!("etcd 不可用: {}", self.reason),
        ))
    }
}

// ============================================================================
// Lazy：启动时 etcd 未就绪（编排自举前）的惰性重连实现
// ============================================================================

/// 惰性连接 etcd 客户端：构造时不连接（启动必成功），每次操作前如有必要
/// 尝试连接（etcd-client 内部断线自动重连）。用于"创建编排自举 etcd 子项目后
/// 立即写入集群状态"的场景（workflow finalize / create 前置幂等检查）。
///
/// 语义与 `UnavailableEtcdClusterClient` 的差异：后者一次性降级（容器重启才恢复），
/// 前者可随 etcd 子项目部署自动恢复（首个操作失败仅说明 etcd 尚未就绪，20504）。
pub struct LazyEtcdClusterClient {
    spec: EtcdConnectSpec,
    endpoints: Vec<String>,
    inner: std::sync::Mutex<Option<Arc<etcd_client::Client>>>,
}

impl LazyEtcdClusterClient {
    pub fn new(endpoints: Vec<String>) -> Self {
        Self::new_with_spec(EtcdConnectSpec::anonymous(&endpoints))
    }

    /// 按认证/TLS 参数惰性连接（凭据在 etcd 就绪前可能尚未生成，故不在构造期读取证书文件）
    pub fn new_with_spec(spec: EtcdConnectSpec) -> Self {
        Self {
            endpoints: spec.endpoints.clone(),
            spec,
            inner: std::sync::Mutex::new(None),
        }
    }

    /// 同步取缓存 client（std::sync 锁守卫不外泄，保证 future Send）
    fn cached(&self) -> Option<Arc<etcd_client::Client>> {
        self.inner.lock().unwrap().clone()
    }

    /// 同步写缓存（None 清除）
    fn set_cached(&self, c: Option<Arc<etcd_client::Client>>) {
        *self.inner.lock().unwrap() = c;
    }

    /// 返回可用 client：缓存命中先真实探活（etcd-client connect 为懒连接，缓存可能
    /// 持有"从未真正连接 / 连接已失效"的 channel），失败则重建；无缓存则惰性连接。
    async fn ensure(&self) -> Result<Arc<etcd_client::Client>, VcmError> {
        if let Some(c) = self.cached() {
            if probe_client(&c).await {
                return Ok(c);
            }
            // 缓存 client 不可用（如创建编排 handler 阶段 etcd 未部署时缓存的
            // 懒连接 channel 未真正建立）→ 丢弃重建（自愈）
            tracing::warn!("etcd 缓存 client 探活失败，重建连接");
            self.set_cached(None);
        }
        let endpoints = normalize_endpoints(&self.endpoints, self.spec.mode.requires_tls());
        let addrs: Vec<&str> = endpoints.iter().map(|s| s.as_str()).collect();
        if addrs.is_empty() {
            return Err(VcmError::new(codes::ETCD_FAILED, "etcd 端点列表为空"));
        }
        let options = build_connect_options(&self.spec)?;
        let client = etcd_client::Client::connect(&addrs, Some(options))
            .await
            .map_err(|e| map_etcd_err("etcd 惰性连接失败（请先部署 etcd 子项目）", e))?;
        let client = Arc::new(client);
        // 连接后仍须真实探活通过才缓存（懒连接 connect 总返回 Ok）
        if !probe_client(&client).await {
            return Err(VcmError::new(
                codes::ETCD_FAILED,
                "etcd 探活失败（连接已建立但不可用，请确认 etcd 子项目已就绪）",
            ));
        }
        self.set_cached(Some(client.clone()));
        tracing::info!(endpoints = %self.endpoints.join(","), "etcd lazy client connected");
        Ok(client)
    }

    /// 真实探活：一次 kv get（etcd-client connect 为懒连接，须发请求验证可达性）
    async fn kv_probe(&self) -> Result<(), VcmError> {
        let client = self.ensure().await?;
        if probe_client(&client).await {
            Ok(())
        } else {
            Err(VcmError::new(codes::ETCD_FAILED, "etcd probe 失败"))
        }
    }
}

/// 对具体 client 做一次真实 kv get 探活（毫秒级）
async fn probe_client(client: &etcd_client::Client) -> bool {
    use etcd_client::GetOptions;
    tokio::time::timeout(
        std::time::Duration::from_millis(1500),
        client
            .kv_client()
            .get("/vos/cluster/.probe", Some(GetOptions::new().with_limit(1))),
    )
    .await
    .map(|resp| resp.is_ok())
    .unwrap_or(false)
}

/// **串行读**探活（learner 可用）。
///
/// etcd learner 不参与投票，会拒绝线性一致读（`rpc not supported for learner`），
/// 因此"本机 etcd 已就绪"这类判断必须用 `with_serializable()` 的本地读。
async fn probe_client_serializable(client: &etcd_client::Client) -> bool {
    use etcd_client::GetOptions;
    tokio::time::timeout(
        std::time::Duration::from_millis(1500),
        client.kv_client().get(
            "/vos/cluster/.probe",
            Some(GetOptions::new().with_limit(1).with_serializable()),
        ),
    )
    .await
    .map(|resp| resp.is_ok())
    .unwrap_or(false)
}

#[async_trait]
impl EtcdClusterClient for LazyEtcdClusterClient {
    async fn ping(&self) -> bool {
        // 真实探活（etcd-client connect 为懒连接，需一次 kv 操作验证可达性）
        self.kv_probe().await.is_ok()
    }

    async fn ping_local(&self) -> bool {
        // 刻意不走 ensure()：其探活是线性一致读，learner 会返回
        // "rpc not supported for learner" → join 就绪门禁永远 false
        let endpoints = normalize_endpoints(&self.endpoints, self.spec.mode.requires_tls());
        let addrs: Vec<&str> = endpoints.iter().map(|s| s.as_str()).collect();
        if addrs.is_empty() {
            return false;
        }
        let options = match build_connect_options(&self.spec) {
            Ok(o) => o,
            Err(_) => return false,
        };
        let Ok(client) = etcd_client::Client::connect(&addrs, Some(options)).await else {
            return false;
        };
        probe_client_serializable(&client).await
    }

    async fn get_cluster_info(&self) -> Result<Option<ClusterInfo>, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }.get_cluster_info().await
    }

    async fn put_cluster_info(&self, info: &ClusterInfo) -> Result<(), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }.put_cluster_info(info).await
    }

    async fn delete_cluster_info(&self) -> Result<(), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }.delete_cluster_info().await
    }

    async fn get_node_record(&self, member_id: &str) -> Result<Option<NodeRecord>, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .get_node_record(member_id)
            .await
    }

    async fn get_all_node_records(&self) -> Result<Vec<NodeRecord>, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .get_all_node_records()
            .await
    }

    async fn put_node_record(&self, member_id: &str, rec: &NodeRecord) -> Result<(), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .put_node_record(member_id, rec)
            .await
    }

    async fn delete_node_record(&self, member_id: &str) -> Result<(), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .delete_node_record(member_id)
            .await
    }

    async fn put_infra_status(&self, st: &InfraStatus) -> Result<(), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .put_infra_status(st)
            .await
    }

    async fn get_infra_status(&self) -> Result<Option<InfraStatus>, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .get_infra_status()
            .await
    }

    async fn update_node_pg_credentials(
        &self,
        member_id: &str,
        superuser: String,
        replication: String,
    ) -> Result<bool, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .update_node_pg_credentials(member_id, superuser, replication)
            .await
    }

    async fn get_cluster_pg_credentials(&self) -> Result<Option<PgCredentials>, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .get_cluster_pg_credentials()
            .await
    }

    async fn put_cluster_pg_credentials(&self, cred: &PgCredentials) -> Result<(), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .put_cluster_pg_credentials(cred)
            .await
    }

    async fn member_count(&self) -> Result<u64, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }.member_count().await
    }

    async fn member_add(&self, peer_url: &str) -> Result<(u64, String), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .member_add(peer_url)
            .await
    }

    async fn member_add_as_learner(&self, peer_url: &str) -> Result<(u64, String), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .member_add_as_learner(peer_url)
            .await
    }

    async fn member_promote(&self, member_id: u64) -> Result<(), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .member_promote(member_id)
            .await
    }

    async fn member_is_learner(&self, member_id: u64) -> Result<Option<bool>, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .member_is_learner(member_id)
            .await
    }

    async fn member_remove(&self, member_id: u64) -> Result<(), VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .member_remove(member_id)
            .await
    }

    async fn find_member_id_by_peer_url(&self, peer_url: &str) -> Result<Option<u64>, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .find_member_id_by_peer_url(peer_url)
            .await
    }

    async fn list_vrrp_allocations(&self) -> Result<Vec<VrrpAllocation>, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .list_vrrp_allocations()
            .await
    }

    async fn claim_vrrp_allocation(&self, alloc: &VrrpAllocation) -> Result<bool, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .claim_vrrp_allocation(alloc)
            .await
    }

    async fn release_vrrp_allocation(
        &self,
        vip: &str,
        vrrp_id: u8,
        owner: &str,
    ) -> Result<bool, VcmError> {
        let c = self.ensure().await?;
        RealEtcdClusterClient { client: c }
            .release_vrrp_allocation(vip, vrrp_id, owner)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::etcd::dto::{role, status, NodeCredentials, NodeMetadata};

    fn sample_record(id: &str, ip: &str) -> NodeRecord {
        NodeRecord {
            metadata: NodeMetadata {
                member_id: id.into(),
                display_name: format!("n-{id}"),
                role: role::WORKER.into(),
                node_ip: ip.into(),
                api_endpoint: format!("http://{ip}:19906/app/"),
                joined_at: "2026-09-06T08:00:00+08:00".into(),
            },
            credentials: NodeCredentials {
                admin_password: "pw".into(),
                updated_at: "2026-09-06T08:00:00+08:00".into(),
                pg_superuser_password: None,
                pg_replication_password: None,
            },
        }
    }

    #[tokio::test]
    async fn mock_cluster_info_write_read() {
        let c = MockEtcdClusterClient::new();
        let info = ClusterInfo {
            cluster_id: "c1".into(),
            cluster_name: "集群".into(),
            status: status::INITIALIZED.into(),
            created_at: "2026-09-06T08:00:00+08:00".into(),
        };
        assert!(c.get_cluster_info().await.unwrap().is_none());
        c.put_cluster_info(&info).await.unwrap();
        assert_eq!(c.get_cluster_info().await.unwrap(), Some(info));
    }

    #[tokio::test]
    async fn mock_node_crud_and_prefix() {
        let c = MockEtcdClusterClient::new();
        let a = sample_record("a", "10.0.0.1");
        let b = sample_record("b", "10.0.0.2");
        c.put_node_record("a", &a).await.unwrap();
        c.put_node_record("b", &b).await.unwrap();

        let all = c.get_all_node_records().await.unwrap();
        assert_eq!(all.len(), 2);

        assert_eq!(c.get_node_record("a").await.unwrap(), Some(a));
        c.delete_node_record("a").await.unwrap();
        assert!(c.get_node_record("a").await.unwrap().is_none());
        assert_eq!(c.get_all_node_records().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn mock_fault_injection_fails_operations() {
        let c = MockEtcdClusterClient::new();
        c.set_fail_next();
        let info = ClusterInfo {
            cluster_id: "x".into(),
            cluster_name: "x".into(),
            status: "initialized".into(),
            created_at: "t".into(),
        };
        let err = c.put_cluster_info(&info).await.unwrap_err();
        assert_eq!(err.code, codes::ETCD_FAILED);

        // 故障消费后恢复
        c.put_cluster_info(&info).await.unwrap();

        // 不可达：读也失败 + ping=false
        c.set_unavailable(true);
        assert!(!c.ping().await);
        assert!(c.get_cluster_info().await.is_err());
    }

    #[tokio::test]
    async fn unavailable_client_never_succeeds() {
        let c = UnavailableEtcdClusterClient::new("未配置端点");
        assert!(!c.ping().await);
        let err = c.get_cluster_info().await.unwrap_err();
        assert_eq!(err.code, codes::ETCD_FAILED);
    }

    #[tokio::test]
    async fn lazy_client_unreachable_returns_etcd_failed() {
        // 惰性连接：未部署 etcd 时操作返回 20504 而非 panic；ping=false
        let c = LazyEtcdClusterClient::new(vec!["127.0.0.1:1".to_string()]); // 不可达端点
        assert!(!c.ping().await);
        let err = c.get_cluster_info().await.unwrap_err();
        assert_eq!(err.code, codes::ETCD_FAILED);
        // 持久可重试（不固化失败）
        let err2 = c.get_node_record("m1").await.unwrap_err();
        assert_eq!(err2.code, codes::ETCD_FAILED);

        // 空端点同样 20504
        let c2 = LazyEtcdClusterClient::new(vec![]);
        let err3 = c2.member_count().await.unwrap_err();
        assert_eq!(err3.code, codes::ETCD_FAILED);
    }

    #[tokio::test]
    async fn mock_learner_lifecycle_add_promote_remove() {
        let c = MockEtcdClusterClient::new();
        let (id, _) = c
            .member_add_as_learner("http://10.0.0.9:2380")
            .await
            .unwrap();
        assert_eq!(
            c.member_is_learner(id).await.unwrap(),
            Some(true),
            "learner 加入后应标记 learner"
        );
        assert_eq!(c.member_count().await.unwrap(), 2);

        c.member_promote(id).await.unwrap();
        assert_eq!(
            c.member_is_learner(id).await.unwrap(),
            Some(false),
            "promote 后应为投票成员"
        );

        assert_eq!(
            c.member_promote(9999).await.unwrap_err().code,
            codes::NOT_FOUND
        );

        c.member_remove(id).await.unwrap();
        assert_eq!(
            c.member_is_learner(id).await.unwrap(),
            None,
            "移除后成员不存在"
        );
        assert_eq!(c.member_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn mock_promote_failure_keeps_learner() {
        let c = MockEtcdClusterClient::new();
        let (id, _) = c
            .member_add_as_learner("http://10.0.0.9:2380")
            .await
            .unwrap();
        c.set_promote_failing(true);
        assert_eq!(
            c.member_promote(id).await.unwrap_err().code,
            codes::ETCD_FAILED
        );
        assert_eq!(
            c.member_is_learner(id).await.unwrap(),
            Some(true),
            "promote 失败仍为 learner"
        );

        assert!(
            promote_learner_with_retry(&c, id, 2, std::time::Duration::from_millis(1))
                .await
                .is_err()
        );
        c.set_promote_failing(false);
        promote_learner_with_retry(&c, id, 2, std::time::Duration::from_millis(1))
            .await
            .unwrap();
        assert_eq!(c.member_is_learner(id).await.unwrap(), Some(false));
    }

    #[test]
    fn mock_raw_exposes_etcd_layout() {
        let c = MockEtcdClusterClient::new();
        let info = ClusterInfo {
            cluster_id: "c".into(),
            cluster_name: "n".into(),
            status: "initialized".into(),
            created_at: "t".into(),
        };
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            c.put_cluster_info(&info).await.unwrap();
        });
        // 数据必须以 /vos/cluster/info 落盘（真实 etcd 同 key 布局）
        assert!(c.raw().contains_key("/vos/cluster/info"));
    }
}