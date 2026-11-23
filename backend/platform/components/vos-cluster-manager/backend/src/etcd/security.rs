//! etcd 认证/TLS 前置的纯函数层（D9）
//!
//! 该模块只做**决策**（无 IO / 无网络），便于单测覆盖：
//! - 角色与 key 前缀映射：[`base_roles`] / [`agent_role`]（最小权限，前缀级）
//! - 身份命名与校验：[`agent_identity`] / [`validate_identity`]
//! - 认证模式与连接参数校验：[`EtcdAuthMode`] / [`validate_connect_spec`]
//! - 凭据布局（目录 + 文件名规范）：[`CredentialLayout`]（落盘细节见 `credential_store`）
//!
//! 设计文档：`openspec/changes/cephfs-multinode-storage-provider/design.md` D9，
//! 运维手册：`components/vos-cluster-manager/docs/etcd-auth-tls.md`。

use std::path::{Path, PathBuf};

/// `vos-ceph` 应用身份角色（`/vos-ceph/*` 读写）
pub const ROLE_VOS_CEPH: &str = "vos-ceph";
/// 每节点 mount agent 共用角色（`/vos-ceph/*` 只读 + 空间状态子树写）
pub const ROLE_VOS_CEPH_AGENT: &str = "vos-ceph-agent";
/// VCM 自身身份角色（`/vos/cluster/*` 读写）
pub const ROLE_VCM: &str = "vcm";
/// 只读消费者角色（`/vos/cluster/*` 只读）
pub const ROLE_VCM_READONLY: &str = "vcm-readonly";
/// etcd 内建超级用户角色（启用 auth 前必须存在并授予 root 用户）
pub const ROLE_ROOT: &str = "root";

/// `/vos-ceph/*` 前缀
pub const PREFIX_VOS_CEPH: &str = "/vos-ceph/";
/// `/vos-ceph/spaces/*` 前缀（agent 需要写挂载状态）
pub const PREFIX_VOS_CEPH_SPACES: &str = "/vos-ceph/spaces/";
/// `/vos/cluster/*` 前缀
pub const PREFIX_CLUSTER: &str = "/vos/cluster/";

/// VCM 自身默认身份名（凭据文件名 `<identity>.json`）
pub const DEFAULT_VCM_IDENTITY: &str = "vcm";
/// 凭据根目录默认值（宿主机约定路径，见 `docs/etcd-auth-tls.md`）
pub const DEFAULT_CREDENTIAL_ROOT: &str = "/etc/vos/etcd";

/// key 权限类型（映射 etcd `PermissionType`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermKind {
    Read,
    Write,
    ReadWrite,
}

impl PermKind {
    /// etcd `PermissionType` 文本（审计 / 状态接口展示用）
    pub fn as_str(&self) -> &'static str {
        match self {
            PermKind::Read => "read",
            PermKind::Write => "write",
            PermKind::ReadWrite => "readwrite",
        }
    }
}

/// 一条前缀级权限
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPermission {
    /// key 前缀（以 `/` 结尾，配合 `with_prefix()`）
    pub key: &'static str,
    pub kind: PermKind,
}

/// 角色定义（角色名 + 期望权限集合）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleSpec {
    pub name: String,
    pub permissions: Vec<KeyPermission>,
}

impl RoleSpec {
    /// 构造角色定义
    pub fn new(name: impl Into<String>, permissions: Vec<KeyPermission>) -> Self {
        Self {
            name: name.into(),
            permissions,
        }
    }
}

/// 集群基础角色（不含每节点 agent 角色——agent 角色按集群固定一份，用户按节点一份）
pub fn base_roles() -> Vec<RoleSpec> {
    vec![
        RoleSpec::new(
            ROLE_VOS_CEPH,
            vec![KeyPermission {
                key: PREFIX_VOS_CEPH,
                kind: PermKind::ReadWrite,
            }],
        ),
        RoleSpec::new(
            ROLE_VCM,
            vec![KeyPermission {
                key: PREFIX_CLUSTER,
                kind: PermKind::ReadWrite,
            }],
        ),
        RoleSpec::new(
            ROLE_VCM_READONLY,
            vec![KeyPermission {
                key: PREFIX_CLUSTER,
                kind: PermKind::Read,
            }],
        ),
    ]
}

/// mount agent 共用角色：读 `/vos-ceph/*` + 写 `/vos-ceph/spaces/*`（挂载状态回写）
///
/// 说明：v1 的挂载状态键位于 `/vos-ceph/spaces/<space_id>/mounts/<node_id>`，
/// 不存在「每节点独立前缀」，因此无法把写权限收敛到「仅自己的状态键」。该限制在
/// `docs/etcd-auth-tls.md` 中显式声明；按节点吊销仍通过删除该节点的 agent 用户实现。
pub fn agent_role() -> RoleSpec {
    RoleSpec::new(
        ROLE_VOS_CEPH_AGENT,
        vec![
            KeyPermission {
                key: PREFIX_VOS_CEPH,
                kind: PermKind::Read,
            },
            KeyPermission {
                key: PREFIX_VOS_CEPH_SPACES,
                kind: PermKind::ReadWrite,
            },
        ],
    )
}

/// 需要确保存在的全部角色（基础角色 + 基础 agent 角色）
pub fn all_roles() -> Vec<RoleSpec> {
    let mut roles = base_roles();
    roles.push(agent_role());
    roles
}

/// IP → 身份片段：`10.100.10.21` → `10-100-10-21`
pub fn ip_slug(ip: &str) -> Option<String> {
    let trimmed = ip.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.parse::<std::net::IpAddr>().is_err() {
        return None;
    }
    Some(trimmed.replace(['.', ':'], "-"))
}

/// 节点 agent 身份名：`agent-<ip-slug>`
pub fn agent_identity(node_ip: &str) -> Option<String> {
    ip_slug(node_ip).map(|s| format!("agent-{s}"))
}

/// 身份名合法性（etcd 用户名 / 凭据文件名共用）：`[A-Za-z0-9._-]{1,64}`
pub fn validate_identity(identity: &str) -> Result<(), String> {
    let ok = !identity.is_empty()
        && identity.len() <= 64
        && identity
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !identity.starts_with('.');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "身份名非法: {identity:?}（允许 [A-Za-z0-9._-]，长度 1~64，且不得以 '.' 开头）"
        ))
    }
}

/// 一个待创建的客户端身份（用户 + 绑定角色 + 可见前缀）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySpec {
    /// 身份名（etcd 用户名 + 凭据文件名）
    pub identity: String,
    /// 绑定的角色（按序授予）
    pub roles: Vec<String>,
    /// 该身份可访问的 key 前缀（仅用于状态展示 / 文档，不参与授权）
    pub key_prefixes: Vec<String>,
}

impl IdentitySpec {
    /// 构造身份定义
    pub fn new(identity: impl Into<String>, roles: Vec<String>, key_prefixes: Vec<String>) -> Self {
        Self {
            identity: identity.into(),
            roles,
            key_prefixes,
        }
    }
}

/// 按节点清单推导默认身份集合：`vcm` / `vcm-readonly` / `vos-ceph` / 每节点 `agent-<slug>`
///
/// 节点 IP 非法时跳过（调用方负责参数校验）。
pub fn default_identities(node_ips: &[String]) -> Vec<IdentitySpec> {
    let mut out = vec![
        IdentitySpec::new(
            DEFAULT_VCM_IDENTITY,
            vec![ROLE_VCM.to_string()],
            vec![PREFIX_CLUSTER.to_string()],
        ),
        IdentitySpec::new(
            "vcm-readonly",
            vec![ROLE_VCM_READONLY.to_string()],
            vec![PREFIX_CLUSTER.to_string()],
        ),
        IdentitySpec::new(
            ROLE_VOS_CEPH,
            vec![ROLE_VOS_CEPH.to_string()],
            vec![PREFIX_VOS_CEPH.to_string()],
        ),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for ip in node_ips {
        if let Some(id) = agent_identity(ip)
            && seen.insert(id.clone())
        {
            out.push(IdentitySpec::new(
                id,
                vec![ROLE_VOS_CEPH_AGENT.to_string()],
                vec![PREFIX_VOS_CEPH.to_string(), PREFIX_VOS_CEPH_SPACES.to_string()],
            ));
        }
    }
    out
}

// ============================================================================
// 认证模式与连接参数
// ============================================================================

/// etcd 客户端认证模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EtcdAuthMode {
    /// 明文 + 匿名（当前线上行为；**仅为兼容窗口保留**）
    #[default]
    Disabled,
    /// 自动：存在本机凭据文件则按凭据连接，否则保持 `Disabled`（迁移窗口便利项）
    Auto,
    /// TLS（服务端校验 CA）+ 用户名密码
    User,
    /// TLS + 客户端证书（mTLS；etcd 以证书 CN 作为用户身份）
    MutualTls,
    /// TLS + 客户端证书 + 用户名密码（两者同时校验）
    MutualTlsUser,
}

impl EtcdAuthMode {
    /// 解析配置字符串（CLI / env）
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "disabled" | "off" | "plaintext" => Some(EtcdAuthMode::Disabled),
            "auto" => Some(EtcdAuthMode::Auto),
            "user" | "password" => Some(EtcdAuthMode::User),
            "mtls" | "mutual-tls" | "mutual_tls" => Some(EtcdAuthMode::MutualTls),
            "mtls_user" | "mtls-user" | "mutual-tls-user" => Some(EtcdAuthMode::MutualTlsUser),
            _ => None,
        }
    }

    /// 规范化字符串（状态接口 / 凭据文件回显用）
    pub fn as_str(&self) -> &'static str {
        match self {
            EtcdAuthMode::Disabled => "disabled",
            EtcdAuthMode::Auto => "auto",
            EtcdAuthMode::User => "user",
            EtcdAuthMode::MutualTls => "mtls",
            EtcdAuthMode::MutualTlsUser => "mtls_user",
        }
    }

    /// 是否要求 TLS 传输（除 `Disabled` / `Auto` 外的显式模式一律要求）
    pub fn requires_tls(&self) -> bool {
        matches!(
            self,
            EtcdAuthMode::User | EtcdAuthMode::MutualTls | EtcdAuthMode::MutualTlsUser
        )
    }

    /// 是否使用客户端证书
    pub fn uses_client_cert(&self) -> bool {
        matches!(self, EtcdAuthMode::MutualTls | EtcdAuthMode::MutualTlsUser)
    }

    /// 是否使用用户名密码
    pub fn uses_password(&self) -> bool {
        matches!(self, EtcdAuthMode::User | EtcdAuthMode::MutualTlsUser)
    }

    /// 是否匿名（禁用认证）
    pub fn is_anonymous(&self) -> bool {
        matches!(self, EtcdAuthMode::Disabled)
    }
}

/// etcd 客户端连接参数（含**敏感字段**：`Debug` 已脱敏，禁止直接打印）
#[derive(Clone, PartialEq, Eq)]
pub struct EtcdConnectSpec {
    /// 已归一化端点（含 scheme；TLS 模式为 `https://`）
    pub endpoints: Vec<String>,
    pub mode: EtcdAuthMode,
    pub ca_cert: Option<PathBuf>,
    pub client_cert: Option<PathBuf>,
    pub client_key: Option<PathBuf>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for EtcdConnectSpec {
    /// 手工实现：密码永不进入日志 / panic 信息
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EtcdConnectSpec")
            .field("endpoints", &self.endpoints)
            .field("mode", &self.mode)
            .field("ca_cert", &self.ca_cert)
            .field("client_cert", &self.client_cert)
            .field("client_key", &self.client_key)
            .field("username", &self.username)
            .field(
                "password",
                &self.password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl EtcdConnectSpec {
    /// 匿名连接（无语义认证；仅兼容窗口 / 首次 bootstrap 使用）
    pub fn anonymous(endpoints: &[String]) -> Self {
        Self {
            endpoints: endpoints
                .iter()
                .map(|e| normalize_endpoint(e, false))
                .collect(),
            mode: EtcdAuthMode::Disabled,
            ca_cert: None,
            client_cert: None,
            client_key: None,
            username: None,
            password: None,
        }
    }
}

/// 端点归一化：缺 scheme 时按 `tls` 补 `https://` / `http://`；去尾部 `/`
pub fn normalize_endpoint(raw: &str, tls: bool) -> String {
    let s = raw.trim().trim_end_matches('/');
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else if tls {
        format!("https://{s}")
    } else {
        format!("http://{s}")
    }
}

/// 批量归一化 + 去重（保持输入顺序）
pub fn normalize_endpoints(raw: &[String], tls: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in raw {
        let n = normalize_endpoint(e, tls);
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// 校验连接参数（启动期 / bootstrap 前的硬门禁）
///
/// 覆盖 spec「Legacy configuration is rejected」：显式要求认证/TLS 时，明文端点或
/// 缺失凭据必须**立即失败并给出可操作错误**，不得静默降级。
pub fn validate_connect_spec(spec: &EtcdConnectSpec) -> Result<(), String> {
    if spec.endpoints.is_empty() {
        return Err("etcd 端点列表为空".into());
    }
    let mut schemes = std::collections::BTreeSet::new();
    for e in &spec.endpoints {
        let Some(scheme) = e.split_once("://").map(|(s, _)| s) else {
            return Err(format!("etcd 端点缺少 scheme（应为 http:// 或 https://）: {e}"));
        };
        if scheme != "http" && scheme != "https" {
            return Err(format!("etcd 端点 scheme 非法: {e}"));
        }
        let host_port = e.split_once("://").map(|(_, r)| r).unwrap_or("");
        let (host, port) = host_port.rsplit_once(':').ok_or_else(|| {
            format!("etcd 端点缺少端口（形如 https://10.0.0.1:2379）: {e}")
        })?;
        if host.is_empty() || port.parse::<u16>().is_err() {
            return Err(format!("etcd 端点格式非法: {e}"));
        }
        schemes.insert(scheme.to_string());
    }
    if schemes.len() > 1 {
        return Err(format!(
            "etcd 端点不得混用 http/https: {}",
            spec.endpoints.join(",")
        ));
    }

    let all_https = schemes.contains("https");
    match spec.mode {
        EtcdAuthMode::Disabled => {
            if all_https {
                return Err(
                    "etcd 认证模式为 disabled 但端点已是 https://：请设置 --etcd-auth-mode user|mtls|mtls_user"
                        .into(),
                );
            }
        }
        EtcdAuthMode::Auto => {}
        mode => {
            if !all_https {
                return Err(format!(
                    "etcd 认证模式 {} 要求 TLS 端点，但配置为明文 {}；请将 --etcd-endpoints 改为 https://<ip>:2379 并部署 etcd TLS",
                    mode.as_str(),
                    spec.endpoints.join(",")
                ));
            }
        }
    }

    if spec.mode.requires_tls() {
        match &spec.ca_cert {
            None => {
                return Err(format!(
                    "etcd 认证模式 {} 需要 CA 证书路径（--etcd-ca-cert 或凭据文件 ca_cert）",
                    spec.mode.as_str()
                ));
            }
            Some(p) if p.as_os_str().is_empty() => {
                return Err("etcd CA 证书路径为空".into());
            }
            Some(_) => {}
        }
    }
    if spec.mode.uses_client_cert() {
        if spec.client_cert.is_none() || spec.client_key.is_none() {
            return Err(format!(
                "etcd 认证模式 {} 需要客户端证书与私钥（--etcd-client-cert / --etcd-client-key）",
                spec.mode.as_str()
            ));
        }
    }
    if spec.mode.uses_password() {
        let user_ok = spec.username.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
        let pass_ok = spec.password.as_deref().map(|s| !s.is_empty()).unwrap_or(false);
        if !user_ok || !pass_ok {
            return Err(format!(
                "etcd 认证模式 {} 需要用户名与密码（--etcd-username / --etcd-password-file，或本机凭据文件）",
                spec.mode.as_str()
            ));
        }
    }
    Ok(())
}

/// 校验文件存在且非空（启动期使用；纯函数只做路径校验，IO 由调用方完成）
pub fn path_is_set(p: Option<&Path>) -> bool {
    p.map(|p| !p.as_os_str().is_empty()).unwrap_or(false)
}

// ============================================================================
// 凭据布局
// ============================================================================

/// 凭据 / PKI 目录布局（宿主机 `/etc/vos/etcd`）
///
/// ```text
/// /etc/vos/etcd/
/// ├── pki/                     # 集群内部 CA + etcd 服务端证书（0700 目录）
/// │   ├── ca.crt               # 0644（公开）
/// │   ├── ca.key               # 0600（仅签发节点持有）
/// │   ├── server.crt           # 0644
/// │   ├── server.key           # 0600
/// │   └── root.password        # 0600（etcd root 用户口令）
/// └── clients/                 # 每身份一份（0700 目录）
///     ├── vcm.json             # 0600（含口令 / 客户端私钥路径）
///     ├── vcm.crt / vcm.key    # 0644 / 0600（mTLS 模式）
///     └── agent-10-100-10-21.json
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialLayout {
    root: PathBuf,
}

impl CredentialLayout {
    /// 以根目录构造布局
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 默认布局（`/etc/vos/etcd`）
    pub fn default_root() -> Self {
        Self::new(DEFAULT_CREDENTIAL_ROOT)
    }

    /// 根目录
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// PKI 目录（`<root>/pki`）
    pub fn pki_dir(&self) -> PathBuf {
        self.root.join("pki")
    }

    /// 客户端凭据目录（`<root>/clients`）
    pub fn clients_dir(&self) -> PathBuf {
        self.root.join("clients")
    }

    /// 集群内部 CA 证书
    pub fn ca_cert(&self) -> PathBuf {
        self.pki_dir().join("ca.crt")
    }

    /// 集群内部 CA 私钥
    pub fn ca_key(&self) -> PathBuf {
        self.pki_dir().join("ca.key")
    }

    /// etcd 服务端证书
    pub fn server_cert(&self) -> PathBuf {
        self.pki_dir().join("server.crt")
    }

    /// etcd 服务端私钥
    pub fn server_key(&self) -> PathBuf {
        self.pki_dir().join("server.key")
    }

    /// etcd root 用户口令文件
    pub fn root_password_file(&self) -> PathBuf {
        self.pki_dir().join("root.password")
    }

    /// 身份凭据文件（`<root>/clients/<identity>.json`）
    pub fn identity_file(&self, identity: &str) -> PathBuf {
        self.clients_dir().join(format!("{identity}.json"))
    }

    /// 身份客户端证书（mTLS）
    pub fn identity_cert(&self, identity: &str) -> PathBuf {
        self.clients_dir().join(format!("{identity}.crt"))
    }

    /// 身份客户端私钥（mTLS）
    pub fn identity_key(&self, identity: &str) -> PathBuf {
        self.clients_dir().join(format!("{identity}.key"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_prefixes_are_least_privilege() {
        let roles = base_roles();
        let by_name = |n: &str| roles.iter().find(|r| r.name == n).cloned().unwrap();

        let ceph = by_name(ROLE_VOS_CEPH);
        assert_eq!(
            ceph.permissions,
            vec![KeyPermission {
                key: PREFIX_VOS_CEPH,
                kind: PermKind::ReadWrite
            }]
        );
        // ceph 身份不得拥有任何 /vos/cluster 权限（spec: Ceph client cannot write cluster keys）
        assert!(
            ceph.permissions
                .iter()
                .all(|p| !p.key.starts_with(PREFIX_CLUSTER)),
            "vos-ceph 角色不得包含 /vos/cluster 前缀"
        );

        let vcm = by_name(ROLE_VCM);
        assert_eq!(
            vcm.permissions,
            vec![KeyPermission {
                key: PREFIX_CLUSTER,
                kind: PermKind::ReadWrite
            }]
        );

        let ro = by_name(ROLE_VCM_READONLY);
        assert_eq!(ro.permissions[0].kind, PermKind::Read);
        assert_eq!(ro.permissions[0].key, PREFIX_CLUSTER);
    }

    #[test]
    fn agent_role_reads_ceph_and_writes_space_status_only() {
        let role = agent_role();
        assert_eq!(role.name, ROLE_VOS_CEPH_AGENT);
        assert!(role.permissions.contains(&KeyPermission {
            key: PREFIX_VOS_CEPH,
            kind: PermKind::Read
        }));
        assert!(role.permissions.contains(&KeyPermission {
            key: PREFIX_VOS_CEPH_SPACES,
            kind: PermKind::ReadWrite
        }));
        assert!(
            role.permissions
                .iter()
                .all(|p| p.kind != PermKind::ReadWrite || p.key == PREFIX_VOS_CEPH_SPACES),
            "agent 的写权限必须收敛到 /vos-ceph/spaces/"
        );
        assert!(
            role.permissions
                .iter()
                .all(|p| !p.key.starts_with(PREFIX_CLUSTER))
        );
    }

    #[test]
    fn identity_naming_and_validation() {
        assert_eq!(agent_identity("10.100.10.21").as_deref(), Some("agent-10-100-10-21"));
        assert_eq!(agent_identity(" not-an-ip").as_deref(), None);
        assert!(validate_identity("agent-10-100-10-21").is_ok());
        assert!(validate_identity("vcm.readonly_1").is_ok());
        assert!(validate_identity("").is_err());
        assert!(validate_identity("../escape").is_err());
        assert!(validate_identity(".hidden").is_err());
        assert!(validate_identity("a".repeat(65).as_str()).is_err());
    }

    #[test]
    fn default_identities_cover_all_roles() {
        let ids = default_identities(&[
            "10.100.10.21".into(),
            "10.100.10.22".into(),
            "10.100.10.21".into(), // 重复 IP 只产生一个 agent
        ]);
        let names: Vec<&str> = ids.iter().map(|i| i.identity.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "vcm",
                "vcm-readonly",
                "vos-ceph",
                "agent-10-100-10-21",
                "agent-10-100-10-22"
            ]
        );
        assert_eq!(ids[0].roles, vec![ROLE_VCM.to_string()]);
        assert_eq!(ids[1].roles, vec![ROLE_VCM_READONLY.to_string()]);
        assert_eq!(ids[2].roles, vec![ROLE_VOS_CEPH.to_string()]);
    }

    #[test]
    fn auth_mode_parse_aliases() {
        assert_eq!(EtcdAuthMode::parse("disabled"), Some(EtcdAuthMode::Disabled));
        assert_eq!(EtcdAuthMode::parse("plaintext"), Some(EtcdAuthMode::Disabled));
        assert_eq!(EtcdAuthMode::parse("auto"), Some(EtcdAuthMode::Auto));
        assert_eq!(EtcdAuthMode::parse("password"), Some(EtcdAuthMode::User));
        assert_eq!(EtcdAuthMode::parse("mTLS"), Some(EtcdAuthMode::MutualTls));
        assert_eq!(EtcdAuthMode::parse("mtls-user"), Some(EtcdAuthMode::MutualTlsUser));
        assert_eq!(EtcdAuthMode::parse("bogus"), None);
        assert!(EtcdAuthMode::Disabled.is_anonymous());
        assert!(EtcdAuthMode::User.requires_tls());
        assert!(EtcdAuthMode::MutualTls.uses_client_cert());
        assert!(EtcdAuthMode::MutualTlsUser.uses_password());
    }

    #[test]
    fn endpoint_normalization() {
        assert_eq!(normalize_endpoint("10.0.0.1:2379", false), "http://10.0.0.1:2379");
        assert_eq!(normalize_endpoint("10.0.0.1:2379", true), "https://10.0.0.1:2379");
        assert_eq!(normalize_endpoint("https://10.0.0.1:2379/", true), "https://10.0.0.1:2379");
        assert_eq!(
            normalize_endpoints(&[" 10.0.0.1:2379 ".into(), "http://10.0.0.1:2379".into()], false),
            vec!["http://10.0.0.1:2379"]
        );
    }

    fn spec(mode: EtcdAuthMode, endpoints: &[&str]) -> EtcdConnectSpec {
        EtcdConnectSpec {
            endpoints: endpoints.iter().map(|s| s.to_string()).collect(),
            mode,
            ca_cert: None,
            client_cert: None,
            client_key: None,
            username: None,
            password: None,
        }
    }

    #[test]
    fn validate_rejects_plaintext_endpoint_when_auth_enabled() {
        let mut s = spec(EtcdAuthMode::User, &["http://10.0.0.1:2379"]);
        s.ca_cert = Some("/etc/vos/etcd/pki/ca.crt".into());
        s.username = Some("vcm".into());
        s.password = Some("pw".into());
        let err = validate_connect_spec(&s).unwrap_err();
        assert!(err.contains("要求 TLS 端点"), "错误需可操作: {err}");
    }

    #[test]
    fn validate_rejects_anonymous_credentials_gaps() {
        // TLS 模式缺 CA
        let s = spec(EtcdAuthMode::User, &["https://10.0.0.1:2379"]);
        assert!(validate_connect_spec(&s).unwrap_err().contains("CA 证书"));

        // user 模式缺密码
        let mut s = spec(EtcdAuthMode::User, &["https://10.0.0.1:2379"]);
        s.ca_cert = Some("/ca".into());
        s.username = Some("vcm".into());
        assert!(validate_connect_spec(&s).unwrap_err().contains("用户名与密码"));

        // mTLS 模式缺客户端证书
        let mut s = spec(EtcdAuthMode::MutualTls, &["https://10.0.0.1:2379"]);
        s.ca_cert = Some("/ca".into());
        assert!(validate_connect_spec(&s).unwrap_err().contains("客户端证书"));
    }

    #[test]
    fn validate_accepts_complete_credentials() {
        let mut s = spec(EtcdAuthMode::MutualTlsUser, &["https://10.0.0.1:2379"]);
        s.ca_cert = Some("/ca".into());
        s.client_cert = Some("/c.crt".into());
        s.client_key = Some("/c.key".into());
        s.username = Some("vcm".into());
        s.password = Some("pw".into());
        validate_connect_spec(&s).unwrap();

        // disabled 且 http 合法（当前线上行为）
        validate_connect_spec(&spec(EtcdAuthMode::Disabled, &["http://10.0.0.1:2379"])).unwrap();
        // disabled 但 https → 提示改用显式模式
        assert!(
            validate_connect_spec(&spec(EtcdAuthMode::Disabled, &["https://10.0.0.1:2379"]))
                .unwrap_err()
                .contains("--etcd-auth-mode")
        );
    }

    #[test]
    fn validate_rejects_mixed_schemes_and_bad_endpoints() {
        assert!(
            validate_connect_spec(&spec(
                EtcdAuthMode::Disabled,
                &["http://10.0.0.1:2379", "https://10.0.0.2:2379"]
            ))
            .unwrap_err()
            .contains("混用")
        );
        assert!(validate_connect_spec(&spec(EtcdAuthMode::Disabled, &["10.0.0.1"])).is_err());
        assert!(validate_connect_spec(&spec(EtcdAuthMode::Disabled, &[])).is_err());
    }

    #[test]
    fn connect_spec_debug_redacts_password() {
        let mut s = spec(EtcdAuthMode::User, &["https://10.0.0.1:2379"]);
        s.ca_cert = Some("/ca".into());
        s.username = Some("vcm".into());
        s.password = Some("super-secret".into());
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("super-secret"), "Debug 输出不得含密码: {dbg}");
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn credential_layout_paths() {
        let layout = CredentialLayout::new("/etc/vos/etcd");
        assert_eq!(layout.pki_dir(), PathBuf::from("/etc/vos/etcd/pki"));
        assert_eq!(layout.ca_cert(), PathBuf::from("/etc/vos/etcd/pki/ca.crt"));
        assert_eq!(
            layout.identity_file("vcm"),
            PathBuf::from("/etc/vos/etcd/clients/vcm.json")
        );
        assert_eq!(
            layout.identity_key("agent-10-0-0-1"),
            PathBuf::from("/etc/vos/etcd/clients/agent-10-0-0-1.key")
        );
        assert_eq!(
            CredentialLayout::default_root().root(),
            Path::new(DEFAULT_CREDENTIAL_ROOT)
        );
    }
}