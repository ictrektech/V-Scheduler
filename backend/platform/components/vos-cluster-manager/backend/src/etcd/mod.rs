//! etcd 集群状态客户端（集群状态唯一权威信源，D-VCM-02）
//!
//! - `dto`：ClusterInfo / NodeRecord / InfraStatus 等 DTO（含脱敏视图）
//! - `keys`：key 拼接纯函数
//! - `client`：EtcdClusterClient trait（Real / Mock / Unavailable / Lazy）
//! - `security`：认证/TLS 决策纯函数（角色前缀映射 / 连接参数校验 / 凭据布局）
//! - `pki`：集群内部 CA 与证书签发
//! - `credential_store`：本机凭据文件（0600）读写
//! - `auth_admin`：etcd auth 管理（角色 / 用户 / auth 开关）抽象与实现
//! - `provisioning`：引导 / 轮换 / 吊销编排（create → verify → enable 顺序约束）

pub mod auth_admin;
pub mod client;
pub mod credential_store;
pub mod dto;
pub mod keys;
pub mod pki;
pub mod provisioning;
pub mod security;

pub use auth_admin::{
    EtcdAuthAdmin, EtcdAuthConnector, MockAuthState, MockEtcdAuthConnector, RealEtcdAuthAdmin,
    RealEtcdAuthConnector,
};
pub use client::{
    EtcdClusterClient, LazyEtcdClusterClient, MockEtcdClusterClient, RealEtcdClusterClient,
    UnavailableEtcdClusterClient, promote_learner_with_retry,
};
pub use credential_store::{CredentialFile, CredentialStore};
pub use dto::VrrpAllocation;
pub use security::{
    CredentialLayout, EtcdAuthMode, EtcdConnectSpec, IdentitySpec, PermKind, RoleSpec,
    agent_identity, default_identities, validate_connect_spec,
};
