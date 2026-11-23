//! etcd 认证管理（角色 / 用户 / auth 开关）的抽象与实现
//!
//! - [`EtcdAuthAdmin`]：幂等角色/用户操作（Real 走 etcd gRPC Auth API，Mock 为内存实现）
//! - [`EtcdAuthConnector`]：连接工厂。**bootstrap 必须先用匿名连接创建角色/用户/root，
//!   再 `auth_enable`**——etcd 在认证关闭时拒绝带账号的连接（`Authenticate` 报
//!   "authentication is not enabled"），因此不能"先带凭据连再启用"。
//!
//! 安全约定：任何返回/日志都不包含口令明文；错误信息只带身份名与前缀。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::error::{codes, VcmError};

use super::security::{EtcdAuthMode, EtcdConnectSpec, PermKind, RoleSpec};

/// 单条已授予权限（etcd 侧读回视图）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleGrant {
    pub key: String,
    pub kind: PermKind,
    pub prefix: bool,
}

/// 角色当前状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleState {
    pub name: String,
    pub grants: Vec<RoleGrant>,
}

/// 用户当前状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserState {
    pub name: String,
    pub roles: Vec<String>,
}

/// etcd auth 管理接口（幂等）
#[async_trait]
pub trait EtcdAuthAdmin: Send + Sync {
    /// 匿名访问是否已被拒绝（= auth 已启用）
    ///
    /// etcd-client 0.19 无 `AuthStatus` 高层 API，故用"匿名探测"语义表达：
    /// 匿名 kv 读被拒绝（或 TLS 握手因缺客户端证书失败）→ `true`。
    async fn auth_enabled(&self) -> Result<bool, VcmError>;
    /// 读取角色（不存在 → None）
    async fn role_get(&self, name: &str) -> Result<Option<RoleState>, VcmError>;
    /// 确保角色存在且包含期望权限（幂等、只增不减）
    async fn role_upsert(&self, role: &RoleSpec) -> Result<(), VcmError>;
    /// 读取用户（不存在 → None）
    async fn user_get(&self, name: &str) -> Result<Option<UserState>, VcmError>;
    /// 确保用户存在（不存在则创建并授予角色；`password = None` = 仅证书身份）
    async fn user_upsert(
        &self,
        name: &str,
        password: Option<&str>,
        roles: &[String],
    ) -> Result<(), VcmError>;
    /// 修改口令（轮换）
    async fn user_set_password(&self, name: &str, password: &str) -> Result<(), VcmError>;
    /// 删除用户（吊销；不存在视为成功）
    async fn user_delete(&self, name: &str) -> Result<(), VcmError>;
    /// 用户清单
    async fn user_list(&self) -> Result<Vec<String>, VcmError>;
    /// 启用认证（幂等：已启用视为成功）
    async fn auth_enable(&self) -> Result<(), VcmError>;
    /// 关闭认证（紧急回滚；需要可用的 root 凭据）
    async fn auth_disable(&self) -> Result<(), VcmError>;
}

/// 连接工厂（bootstrap 需要"匿名 → 启用 → 带凭据"两阶段）
#[async_trait]
pub trait EtcdAuthConnector: Send + Sync {
    /// 匿名连接（仅用于 auth 关闭时的首次 bootstrap / 状态探测）
    async fn connect_anonymous(&self) -> Result<Arc<dyn EtcdAuthAdmin>, VcmError>;
    /// 带凭据连接
    async fn connect(&self, spec: &EtcdConnectSpec) -> Result<Arc<dyn EtcdAuthAdmin>, VcmError>;
}

fn map_auth_err(ctx: &str, e: impl std::fmt::Display) -> VcmError {
    VcmError::new(codes::ETCD_AUTH_FAILED, format!("{ctx}: {e}"))
}

fn err_text(e: &etcd_client::Error) -> String {
    e.to_string()
}

/// etcd 错误语义判定：是否"已存在"（幂等路径）
pub fn is_already_exists(msg: &str) -> bool {
    msg.contains("already exists")
}

/// etcd 错误语义判定：认证未启用
pub fn is_auth_not_enabled(msg: &str) -> bool {
    msg.contains("authentication is not enabled")
}

/// etcd 错误语义判定：未认证 / 无权限 / 证书缺失
pub fn is_unauthenticated(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("user name is empty")
        || m.contains("invalid auth token")
        || m.contains("permission denied")
        || m.contains("authentication failed")
        || m.contains("user name not found")
        || m.contains("certificate")
        || m.contains("tls")
}

// ============================================================================
// Real
// ============================================================================

/// 真实实现：包装 etcd gRPC Auth API（`Client` 的 auth 方法需要 `&mut`，故内部加锁）
pub struct RealEtcdAuthAdmin {
    client: tokio::sync::Mutex<etcd_client::Client>,
    /// 是否带凭据（匿名连接下启用 auth 后会失去写权限）
    authenticated: bool,
}

impl RealEtcdAuthAdmin {
    async fn connect_with(
        endpoints: &[String],
        options: Option<etcd_client::ConnectOptions>,
        authenticated: bool,
    ) -> Result<Self, VcmError> {
        let addrs: Vec<&str> = endpoints.iter().map(|s| s.as_str()).collect();
        let client = etcd_client::Client::connect(&addrs, options)
            .await
            .map_err(|e| {
                VcmError::new(
                    codes::ETCD_AUTH_FAILED,
                    format!(
                        "etcd 连接失败（认证模式 / 凭据可能不匹配）: {}",
                        err_text(&e)
                    ),
                )
            })?;
        Ok(Self {
            client: tokio::sync::Mutex::new(client),
            authenticated,
        })
    }

    fn grant_to_permission(g: &RoleGrant) -> etcd_client::Permission {
        let p = match g.kind {
            PermKind::Read => etcd_client::Permission::read(g.key.clone()),
            PermKind::Write => etcd_client::Permission::write(g.key.clone()),
            PermKind::ReadWrite => etcd_client::Permission::read_write(g.key.clone()),
        };
        if g.prefix { p.with_prefix() } else { p }
    }

    fn permission_to_grant(p: &etcd_client::Permission) -> RoleGrant {
        let kind = match p.get_type() {
            0 => PermKind::Read,
            1 => PermKind::Write,
            _ => PermKind::ReadWrite,
        };
        RoleGrant {
            key: p.key_str().unwrap_or_default().to_string(),
            kind,
            prefix: p.is_prefix(),
        }
    }
}

#[async_trait]
impl EtcdAuthAdmin for RealEtcdAuthAdmin {
    async fn auth_enabled(&self) -> Result<bool, VcmError> {
        // 已认证连接：能通过身份校验即说明 auth 已启用
        if self.authenticated {
            return Ok(true);
        }
        use etcd_client::GetOptions;
        let c = self.client.lock().await;
        let resp = c
            .kv_client()
            .get(
                "/vos/cluster/.authprobe",
                Some(GetOptions::new().with_limit(1)),
            )
            .await;
        match resp {
            Ok(_) => Ok(false),
            Err(e) => {
                let msg = err_text(&e);
                if is_auth_not_enabled(&msg) {
                    Ok(false)
                } else if is_unauthenticated(&msg) {
                    Ok(true)
                } else {
                    Err(map_auth_err("探测 etcd auth 状态失败", msg))
                }
            }
        }
    }

    async fn role_get(&self, name: &str) -> Result<Option<RoleState>, VcmError> {
        let mut c = self.client.lock().await;
        match c.role_get(name).await {
            Ok(resp) => Ok(Some(RoleState {
                name: name.to_string(),
                grants: resp.permissions().iter().map(Self::permission_to_grant).collect(),
            })),
            Err(e) => {
                let msg = err_text(&e);
                if msg.contains("role name does not exist") || msg.contains("not found") {
                    Ok(None)
                } else {
                    Err(map_auth_err(&format!("读取角色 {name}"), msg))
                }
            }
        }
    }

    async fn role_upsert(&self, role: &RoleSpec) -> Result<(), VcmError> {
        {
            let mut c = self.client.lock().await;
            if let Err(e) = c.role_add(role.name.clone()).await {
                let msg = err_text(&e);
                if !is_already_exists(&msg) {
                    return Err(map_auth_err(&format!("创建角色 {}", role.name), msg));
                }
            }
        }
        let existing = self.role_get(&role.name).await?.unwrap_or(RoleState {
            name: role.name.clone(),
            grants: Vec::new(),
        });
        for perm in &role.permissions {
            let desired = RoleGrant {
                key: perm.key.to_string(),
                kind: perm.kind,
                prefix: true,
            };
            if existing.grants.iter().any(|g| {
                g.key == desired.key && g.kind == desired.kind && g.prefix == desired.prefix
            }) {
                continue;
            }
            let mut c = self.client.lock().await;
            c.role_grant_permission(role.name.clone(), Self::grant_to_permission(&desired))
                .await
                .map_err(|e| {
                    map_auth_err(
                        &format!("授予角色 {} 权限 {}", role.name, perm.key),
                        err_text(&e),
                    )
                })?;
        }
        Ok(())
    }

    async fn user_get(&self, name: &str) -> Result<Option<UserState>, VcmError> {
        let mut c = self.client.lock().await;
        match c.user_get(name).await {
            Ok(resp) => Ok(Some(UserState {
                name: name.to_string(),
                roles: resp.roles().to_vec(),
            })),
            Err(e) => {
                let msg = err_text(&e);
                if msg.contains("user name not found") || msg.contains("user does not exist") {
                    Ok(None)
                } else {
                    Err(map_auth_err(&format!("读取用户 {name}"), msg))
                }
            }
        }
    }

    async fn user_upsert(
        &self,
        name: &str,
        password: Option<&str>,
        roles: &[String],
    ) -> Result<(), VcmError> {
        let existing = self.user_get(name).await?;
        if existing.is_none() {
            let mut c = self.client.lock().await;
            if let Err(e) = c.user_add(name, password.unwrap_or(""), None).await {
                let msg = err_text(&e);
                if !is_already_exists(&msg) {
                    return Err(map_auth_err(&format!("创建用户 {name}"), msg));
                }
            }
        }
        for role in roles {
            let mut c = self.client.lock().await;
            if let Err(e) = c.user_grant_role(name, role).await {
                let msg = err_text(&e);
                // 已授予该角色时 etcd 返回 "role is already granted to user"
                if !msg.contains("already granted") && !is_already_exists(&msg) {
                    return Err(map_auth_err(
                        &format!("为用户 {name} 授予角色 {role}"),
                        msg,
                    ));
                }
            }
        }
        Ok(())
    }

    async fn user_set_password(&self, name: &str, password: &str) -> Result<(), VcmError> {
        let mut c = self.client.lock().await;
        c.user_change_password(name, password)
            .await
            .map_err(|e| map_auth_err(&format!("修改用户 {name} 口令"), err_text(&e)))?;
        Ok(())
    }

    async fn user_delete(&self, name: &str) -> Result<(), VcmError> {
        let mut c = self.client.lock().await;
        match c.user_delete(name).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = err_text(&e);
                if msg.contains("user name not found") {
                    Ok(())
                } else {
                    Err(map_auth_err(&format!("删除用户 {name}"), msg))
                }
            }
        }
    }

    async fn user_list(&self) -> Result<Vec<String>, VcmError> {
        let mut c = self.client.lock().await;
        let resp = c
            .user_list()
            .await
            .map_err(|e| map_auth_err("列出用户", err_text(&e)))?;
        Ok(resp.users().to_vec())
    }

    async fn auth_enable(&self) -> Result<(), VcmError> {
        let mut c = self.client.lock().await;
        match c.auth_enable().await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = err_text(&e);
                if msg.contains("already enabled") {
                    Ok(())
                } else {
                    Err(map_auth_err("启用 etcd 认证", msg))
                }
            }
        }
    }

    async fn auth_disable(&self) -> Result<(), VcmError> {
        let mut c = self.client.lock().await;
        match c.auth_disable().await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = err_text(&e);
                if msg.contains("already disabled") || is_auth_not_enabled(&msg) {
                    Ok(())
                } else {
                    Err(map_auth_err("关闭 etcd 认证", msg))
                }
            }
        }
    }
}

/// 真实连接工厂（持有端点 + TLS 材料，按调用方给的凭据建立连接）
pub struct RealEtcdAuthConnector {
    endpoints: Vec<String>,
    ca_cert: Option<std::path::PathBuf>,
}

impl RealEtcdAuthConnector {
    /// 构造（`ca_cert` 为 None 表示明文阶段）
    pub fn new(endpoints: Vec<String>, ca_cert: Option<std::path::PathBuf>) -> Self {
        Self {
            endpoints,
            ca_cert,
        }
    }

    fn tls_config(&self) -> Result<etcd_client::TlsOptions, VcmError> {
        let ca = self
            .ca_cert
            .as_ref()
            .ok_or_else(|| VcmError::new(codes::ETCD_AUTH_FAILED, "缺少 CA 证书路径，无法建立 TLS 连接"))?;
        let pem = std::fs::read(ca).map_err(|e| {
            VcmError::new(
                codes::IO_FAILED,
                format!("读取 CA 证书 {} 失败: {e}", ca.display()),
            )
        })?;
        Ok(etcd_client::TlsOptions::new().ca_certificate(etcd_client::Certificate::from_pem(pem)))
    }
}

#[async_trait]
impl EtcdAuthConnector for RealEtcdAuthConnector {
    async fn connect_anonymous(&self) -> Result<Arc<dyn EtcdAuthAdmin>, VcmError> {
        // TLS 由端点 scheme 决定（迁移窗口内可能仍是 http + auth 未启用）
        let https = self.endpoints.iter().any(|e| e.starts_with("https://"));
        let opts = if https {
            let mut o = etcd_client::ConnectOptions::new().with_tls(self.tls_config()?);
            o = o.with_connect_timeout(std::time::Duration::from_secs(5));
            Some(o)
        } else {
            None
        };
        Ok(Arc::new(
            RealEtcdAuthAdmin::connect_with(&self.endpoints, opts, false).await?,
        ))
    }

    async fn connect(&self, spec: &EtcdConnectSpec) -> Result<Arc<dyn EtcdAuthAdmin>, VcmError> {
        let mut opts = etcd_client::ConnectOptions::new();
        if spec.mode.requires_tls() {
            let mut tls = self.tls_config()?;
            if let (Some(cert), Some(key)) = (&spec.client_cert, &spec.client_key) {
                let cert_pem = std::fs::read(cert).map_err(|e| {
                    VcmError::new(
                        codes::IO_FAILED,
                        format!("读取客户端证书 {} 失败: {e}", cert.display()),
                    )
                })?;
                let key_pem = std::fs::read(key).map_err(|e| {
                    VcmError::new(
                        codes::IO_FAILED,
                        format!("读取客户端私钥 {} 失败: {e}", key.display()),
                    )
                })?;
                tls = tls.identity(etcd_client::Identity::from_pem(cert_pem, key_pem));
            }
            opts = opts.with_tls(tls);
        }
        if let (Some(u), Some(p)) = (&spec.username, &spec.password) {
            opts = opts.with_user(u.clone(), p.clone());
        }
        opts = opts.with_connect_timeout(std::time::Duration::from_secs(5));
        Ok(Arc::new(
            RealEtcdAuthAdmin::connect_with(&self.endpoints, Some(opts), true).await?,
        ))
    }
}

// ============================================================================
// Mock（单测 / 无 etcd 环境）
// ============================================================================

/// Mock 共享状态：角色 / 用户 / auth 开关 / 调用序
#[derive(Default)]
pub struct MockAuthState {
    enabled: AtomicBool,
    roles: Mutex<BTreeMap<String, Vec<RoleGrant>>>,
    users: Mutex<BTreeMap<String, (Option<String>, Vec<String>)>>,
    events: Mutex<Vec<String>>,
    fail_next: AtomicBool,
}

impl MockAuthState {
    /// 新状态
    pub fn new() -> Self {
        Self::default()
    }

    /// auth 是否已启用
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// 事件序列（断言"先建角色/用户再 enable"）
    pub fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    /// 已创建用户（名 → 是否设置了口令，不含口令本体）
    pub fn users(&self) -> BTreeMap<String, (bool, Vec<String>)> {
        self.users
            .lock()
            .unwrap()
            .iter()
            .map(|(k, (pw, roles))| (k.clone(), (pw.is_some(), roles.clone())))
            .collect()
    }

    /// 角色权限
    pub fn roles(&self) -> BTreeMap<String, Vec<RoleGrant>> {
        self.roles.lock().unwrap().clone()
    }

    /// 注入一次故障
    pub fn fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    fn record(&self, ev: impl Into<String>) {
        self.events.lock().unwrap().push(ev.into());
    }

    fn take_fail(&self) -> Result<(), VcmError> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(VcmError::new(codes::ETCD_AUTH_FAILED, "mock: 注入故障"));
        }
        Ok(())
    }
}

struct MockAdmin {
    state: Arc<MockAuthState>,
    privileged: bool,
}

impl MockAdmin {
    fn guard(&self) -> Result<(), VcmError> {
        if self.state.is_enabled() && !self.privileged {
            return Err(VcmError::new(
                codes::ETCD_AUTH_FAILED,
                "etcdserver: user name is empty（匿名访问已被拒绝）",
            ));
        }
        self.state.take_fail()
    }
}

#[async_trait]
impl EtcdAuthAdmin for MockAdmin {
    async fn auth_enabled(&self) -> Result<bool, VcmError> {
        Ok(self.state.is_enabled())
    }

    async fn role_get(&self, name: &str) -> Result<Option<RoleState>, VcmError> {
        self.guard()?;
        Ok(self.state.roles.lock().unwrap().get(name).map(|g| RoleState {
            name: name.to_string(),
            grants: g.clone(),
        }))
    }

    async fn role_upsert(&self, role: &RoleSpec) -> Result<(), VcmError> {
        self.guard()?;
        let mut roles = self.state.roles.lock().unwrap();
        let entry = roles.entry(role.name.clone()).or_default();
        for p in &role.permissions {
            let grant = RoleGrant {
                key: p.key.to_string(),
                kind: p.kind,
                prefix: true,
            };
            if !entry.contains(&grant) {
                entry.push(grant);
            }
        }
        drop(roles);
        self.state.record(format!("role_upsert:{}", role.name));
        Ok(())
    }

    async fn user_get(&self, name: &str) -> Result<Option<UserState>, VcmError> {
        self.guard()?;
        Ok(self
            .state
            .users
            .lock()
            .unwrap()
            .get(name)
            .map(|(_, roles)| UserState {
                name: name.to_string(),
                roles: roles.clone(),
            }))
    }

    async fn user_upsert(
        &self,
        name: &str,
        password: Option<&str>,
        roles: &[String],
    ) -> Result<(), VcmError> {
        self.guard()?;
        let mut users = self.state.users.lock().unwrap();
        let entry = users
            .entry(name.to_string())
            .or_insert_with(|| (None, Vec::new()));
        if let Some(p) = password {
            entry.0 = Some(p.to_string());
        }
        for r in roles {
            if !entry.1.contains(r) {
                entry.1.push(r.clone());
            }
        }
        drop(users);
        self.state.record(format!("user_upsert:{name}"));
        Ok(())
    }

    async fn user_set_password(&self, name: &str, password: &str) -> Result<(), VcmError> {
        self.guard()?;
        let mut users = self.state.users.lock().unwrap();
        let entry = users
            .get_mut(name)
            .ok_or_else(|| VcmError::new(codes::NOT_FOUND, format!("用户 {name} 不存在")))?;
        entry.0 = Some(password.to_string());
        drop(users);
        self.state.record(format!("user_rotate:{name}"));
        Ok(())
    }

    async fn user_delete(&self, name: &str) -> Result<(), VcmError> {
        self.guard()?;
        self.state.users.lock().unwrap().remove(name);
        self.state.record(format!("user_delete:{name}"));
        Ok(())
    }

    async fn user_list(&self) -> Result<Vec<String>, VcmError> {
        self.guard()?;
        Ok(self.state.users.lock().unwrap().keys().cloned().collect())
    }

    async fn auth_enable(&self) -> Result<(), VcmError> {
        self.guard()?;
        self.state.enabled.store(true, Ordering::SeqCst);
        self.state.record("auth_enable");
        Ok(())
    }

    async fn auth_disable(&self) -> Result<(), VcmError> {
        self.state.enabled.store(false, Ordering::SeqCst);
        self.state.record("auth_disable");
        Ok(())
    }
}

/// Mock 连接工厂（与 `MockAuthState` 共享状态，便于断言两阶段连接语义）
pub struct MockEtcdAuthConnector {
    state: Arc<MockAuthState>,
}

impl MockEtcdAuthConnector {
    /// 新建共享状态与工厂
    pub fn new() -> (Arc<MockAuthState>, Self) {
        let state = Arc::new(MockAuthState::new());
        (
            state.clone(),
            Self {
                state,
            },
        )
    }
}

impl Default for MockEtcdAuthConnector {
    fn default() -> Self {
        Self::new().1
    }
}

#[async_trait]
impl EtcdAuthConnector for MockEtcdAuthConnector {
    async fn connect_anonymous(&self) -> Result<Arc<dyn EtcdAuthAdmin>, VcmError> {
        Ok(Arc::new(MockAdmin {
            state: self.state.clone(),
            privileged: false,
        }))
    }

    async fn connect(&self, spec: &EtcdConnectSpec) -> Result<Arc<dyn EtcdAuthAdmin>, VcmError> {
        if !self.state.is_enabled() {
            return Err(VcmError::new(
                codes::ETCD_AUTH_FAILED,
                "etcdserver: authentication is not enabled",
            ));
        }
        let user = spec.username.clone().unwrap_or_default();
        let password = spec.password.clone().unwrap_or_default();
        let users = self.state.users.lock().unwrap();
        match users.get(&user) {
            Some((Some(pw), _)) if *pw == password => {}
            Some((None, _)) if spec.mode.uses_client_cert() => {}
            _ => {
                return Err(VcmError::new(
                    codes::ETCD_AUTH_FAILED,
                    format!("etcdserver: authentication failed（用户 {user}）"),
                ));
            }
        }
        Ok(Arc::new(MockAdmin {
            state: self.state.clone(),
            privileged: true,
        }))
    }
}

/// 把模式转成凭据文件字符串（避免调用方重复手写）
pub fn auth_mode_str(mode: EtcdAuthMode) -> &'static str {
    mode.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::etcd::security::{
        PermKind, RoleSpec, base_roles, EtcdAuthMode, KeyPermission, PREFIX_CLUSTER,
    };

    fn user_spec(user: &str) -> EtcdConnectSpec {
        EtcdConnectSpec {
            endpoints: vec!["https://10.0.0.1:2379".into()],
            mode: EtcdAuthMode::User,
            ca_cert: Some("/ca".into()),
            client_cert: None,
            client_key: None,
            username: Some(user.into()),
            password: Some("pw".into()),
        }
    }

    #[tokio::test]
    async fn mock_requires_enable_before_authenticated_connect() {
        let (state, conn) = MockEtcdAuthConnector::new();
        let anon = conn.connect_anonymous().await.unwrap();
        assert!(!anon.auth_enabled().await.unwrap());

        // auth 未启用时带凭据连接失败（与真实 etcd 语义一致）
        assert!(conn.connect(&user_spec("root")).await.is_err());

        anon.role_upsert(&RoleSpec::new(
            "vcm",
            vec![KeyPermission {
                key: PREFIX_CLUSTER,
                kind: PermKind::ReadWrite,
            }],
        ))
        .await
        .unwrap();
        anon.user_upsert("root", Some("rootpw"), &["root".to_string()])
            .await
            .unwrap();
        anon.auth_enable().await.unwrap();
        assert!(state.is_enabled());

        // 启用后匿名连接被拒绝
        let anon2 = conn.connect_anonymous().await.unwrap();
        assert!(anon2.role_get("vcm").await.is_err());
        // 正确凭据可连接
        let admin = conn.connect(&user_spec("root")).await.unwrap();
        assert!(admin.user_get("root").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn role_upsert_is_idempotent_and_keeps_prefix_semantics() {
        let (_state, conn) = MockEtcdAuthConnector::new();
        let admin = conn.connect_anonymous().await.unwrap();
        for _ in 0..2 {
            for role in base_roles() {
                admin.role_upsert(&role).await.unwrap();
            }
        }
        let ceph = admin.role_get("vos-ceph").await.unwrap().unwrap();
        assert_eq!(ceph.grants.len(), 1, "重复 upsert 不得产生重复权限");
        assert_eq!(ceph.grants[0].key, "/vos-ceph/");
        assert!(ceph.grants[0].prefix);
        assert_eq!(ceph.grants[0].kind, PermKind::ReadWrite);

        let ro = admin.role_get("vcm-readonly").await.unwrap().unwrap();
        assert_eq!(ro.grants[0].kind, PermKind::Read);
    }

    #[test]
    fn error_classification_helpers() {
        assert!(is_already_exists("etcdserver: role name already exists"));
        assert!(is_auth_not_enabled("etcdserver: authentication is not enabled"));
        assert!(is_unauthenticated("etcdserver: user name is empty"));
        assert!(is_unauthenticated("etcdserver: permission denied"));
        assert!(!is_unauthenticated("context deadline exceeded"));
    }
}