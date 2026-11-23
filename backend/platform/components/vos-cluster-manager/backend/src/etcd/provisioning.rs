//! etcd auth/TLS 引导编排（角色 → 用户 → 校验 → 启用认证）
//!
//! 顺序是硬约束（[`bootstrap`]）：
//! 1. 建立管理连接（auth 未启用用匿名连接；已启用用 root 凭据）
//! 2. 创建 / 补齐角色（前缀最小权限）
//! 3. 创建 root 用户 + 全部客户端身份用户（口令缺失则生成）
//! 4. **读回校验**（角色权限、root 角色绑定）——不通过则拒绝启用
//! 5. `auth_enable`（last，避免把自己锁在门外）
//! 6. 用新凭据重连校验，再落盘 0600 凭据文件
//!
//! 幂等：重复执行不改变已有口令（除非 `rotate`），不重复授予权限。

use std::sync::Arc;

use crate::error::{codes, VcmError};

use super::auth_admin::{EtcdAuthAdmin, EtcdAuthConnector};
use super::credential_store::{CredentialFile, CredentialStore};
use super::pki;
use super::security::{
    CredentialLayout, EtcdAuthMode, EtcdConnectSpec, IdentitySpec, ROLE_ROOT, RoleSpec,
    all_roles, normalize_endpoints, validate_identity,
};

/// 引导选项
#[derive(Debug, Clone)]
pub struct BootstrapOptions {
    /// 待创建 / 补齐的身份（含 root 之外的客户端身份）
    pub identities: Vec<IdentitySpec>,
    /// 是否启用认证（false = 只建角色/用户，保留明文窗口）
    pub enable_auth: bool,
    /// 是否强制轮换全部身份口令
    pub rotate: bool,
    /// 目标认证模式（决定是否生成 mTLS 客户端证书 / 是否写口令）
    pub auth_mode: EtcdAuthMode,
    /// etcd 端点（未归一化亦可）
    pub endpoints: Vec<String>,
    /// root 口令（None → 读 `<pki>/root.password`，仍无则生成）
    pub root_password: Option<String>,
}

impl BootstrapOptions {
    /// 构造
    pub fn new(endpoints: Vec<String>, auth_mode: EtcdAuthMode, identities: Vec<IdentitySpec>) -> Self {
        Self {
            identities,
            enable_auth: true,
            rotate: false,
            auth_mode,
            endpoints,
            root_password: None,
        }
    }
}

/// 身份结果（**不含口令**，可直接回给前端）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityOutcome {
    pub identity: String,
    pub username: String,
    pub roles: Vec<String>,
    pub key_prefixes: Vec<String>,
    /// 本次是否生成 / 轮换了口令
    pub password_rotated: bool,
    /// 凭据文件是否落盘
    pub credential_written: bool,
    /// 凭据代次
    pub generation: u32,
}

/// 引导结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapReport {
    /// 执行前 auth 是否已启用
    pub auth_was_enabled: bool,
    /// 执行后 auth 是否已启用
    pub auth_enabled: bool,
    /// 本次 `auth_enable` 是否由本流程触发
    pub auth_enabled_by_bootstrap: bool,
    /// root 口令是否本次生成（值不返回，落盘 `<pki>/root.password`）
    pub root_password_generated: bool,
    pub root_password_file: String,
    pub roles_ensured: Vec<String>,
    pub identities: Vec<IdentityOutcome>,
    /// 认证启用后的重连校验是否通过
    pub verified_after_enable: bool,
}

/// 生成高熵口令（两次 UUIDv4 拼接 = 244 bit；无额外随机源依赖）
pub fn generate_password() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn auth_err(msg: impl Into<String>) -> VcmError {
    VcmError::new(codes::ETCD_AUTH_FAILED, msg)
}

fn readonly_spec(endpoints: &[String], mode: EtcdAuthMode, ca: Option<std::path::PathBuf>) -> EtcdConnectSpec {
    EtcdConnectSpec {
        endpoints: normalize_endpoints(endpoints, mode.requires_tls()),
        mode,
        ca_cert: ca,
        client_cert: None,
        client_key: None,
        username: None,
        password: None,
    }
}

async fn connect_authenticated(
    connector: &dyn EtcdAuthConnector,
    endpoints: &[String],
    mode: EtcdAuthMode,
    ca: Option<std::path::PathBuf>,
    username: &str,
    password: &str,
) -> Result<Arc<dyn EtcdAuthAdmin>, VcmError> {
    let mut spec = readonly_spec(endpoints, mode, ca);
    spec.username = Some(username.to_string());
    spec.password = Some(password.to_string());
    connector.connect(&spec).await
}

/// 读回校验：角色权限齐全 + root 用户绑定 root 角色
async fn verify_pre_enable(
    admin: &dyn EtcdAuthAdmin,
    roles: &[RoleSpec],
) -> Result<(), VcmError> {
    for role in roles {
        let state = admin.role_get(&role.name).await?.ok_or_else(|| {
            auth_err(format!("启用认证前校验失败：角色 {} 不存在", role.name))
        })?;
        for perm in &role.permissions {
            let ok = state.grants.iter().any(|g| {
                g.key == perm.key && g.kind == perm.kind && g.prefix
            });
            if !ok {
                return Err(auth_err(format!(
                    "启用认证前校验失败：角色 {} 缺少 {} {} 权限",
                    role.name,
                    perm.kind.as_str(),
                    perm.key
                )));
            }
        }
    }
    let root = admin
        .user_get(ROLE_ROOT)
        .await?
        .ok_or_else(|| auth_err("启用认证前校验失败：root 用户不存在"))?;
    if !root.roles.iter().any(|r| r == ROLE_ROOT) {
        return Err(auth_err(
            "启用认证前校验失败：root 用户未绑定 root 角色（启用后将无法管理）",
        ));
    }
    Ok(())
}

async fn ensure_root(
    admin: &dyn EtcdAuthAdmin,
    password: &str,
    rotate: bool,
) -> Result<(), VcmError> {
    // etcd 的 root 角色名具有特殊语义（拥有全部权限）；预先建空角色避免 grant 报
    // "role name does not exist"，再绑定用户。
    admin
        .role_upsert(&RoleSpec::new(ROLE_ROOT, Vec::new()))
        .await?;
    let existing = admin.user_get(ROLE_ROOT).await?;
    match existing {
        None => {
            admin
                .user_upsert(ROLE_ROOT, Some(password), &[ROLE_ROOT.to_string()])
                .await?;
        }
        Some(_) => {
            if rotate {
                admin.user_set_password(ROLE_ROOT, password).await?;
            }
            admin
                .user_upsert(ROLE_ROOT, None, &[ROLE_ROOT.to_string()])
                .await?;
        }
    }
    Ok(())
}

/// 执行引导（幂等）
pub async fn bootstrap(
    connector: &dyn EtcdAuthConnector,
    store: &CredentialStore,
    opts: &BootstrapOptions,
) -> Result<BootstrapReport, VcmError> {
    let layout = store.layout().clone();
    let endpoints = normalize_endpoints(&opts.endpoints, opts.auth_mode.requires_tls());
    if endpoints.is_empty() {
        return Err(VcmError::new(codes::PARAM_INVALID, "etcd 端点列表为空"));
    }
    for id in &opts.identities {
        validate_identity(&id.identity).map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
    }

    let ca = if opts.auth_mode.requires_tls() {
        Some(layout.ca_cert())
    } else {
        None
    };
    let root_password = resolve_root_password(&layout, opts.root_password.as_deref())?;
    let root_password_generated = !layout.root_password_file().exists()
        && opts.root_password.is_none();

    // ---- 1. 管理连接 ----
    let (admin, auth_was_enabled) = connect_admin(
        connector,
        &endpoints,
        opts.auth_mode,
        ca.clone(),
        &root_password,
    )
    .await?;

    // ---- 2. 角色 ----
    let roles = all_roles();
    for role in &roles {
        admin.role_upsert(role).await?;
    }

    // ---- 3. 身份用户 ----
    let mut pending: Vec<(IdentitySpec, Option<String>, bool, u32)> = Vec::new();
    for spec in &opts.identities {
        let existing_cred = store.read(&spec.identity)?;
        let user_exists = admin.user_get(&spec.identity).await?.is_some();
        let need_password = opts.auth_mode.uses_password();
        let (password, rotated) = if !need_password {
            (None, false)
        } else if opts.rotate || !user_exists || existing_cred.as_ref().and_then(|c| c.password.clone()).is_none() {
            (Some(generate_password()), true)
        } else {
            (None, false)
        };

        let effective_password = password
            .clone()
            .or_else(|| existing_cred.as_ref().and_then(|c| c.password.clone()));

        admin
            .user_upsert(
                &spec.identity,
                effective_password.as_deref(),
                &spec.roles,
            )
            .await?;
        if let Some(p) = password.as_deref() {
            admin.user_set_password(&spec.identity, p).await?;
        }
        if opts.auth_mode.uses_client_cert() {
            let ids = vec![spec.identity.clone()];
            pki::ensure_pki(
                &layout,
                &endpoints_hosts(&endpoints),
                &ids,
                opts.rotate,
            )?;
        }
        let generation = existing_cred.map(|c| c.generation + 1).unwrap_or(1);
        pending.push((spec.clone(), effective_password, rotated, generation));
    }

    // ---- 4. root 用户（启用认证前必须就绪）----
    ensure_root(admin.as_ref(), &root_password, opts.rotate).await?;

    // ---- 5. 校验后启用认证 ----
    let mut enabled_by_bootstrap = false;
    if opts.enable_auth && !auth_was_enabled {
        if opts.auth_mode.is_anonymous() {
            return Err(auth_err(
                "拒绝在认证模式 disabled（匿名）下启用 etcd 认证：请先以 --etcd-auth-mode user|mtls|mtls_user 配置客户端凭据，否则启用后将全部客户端失联",
            ));
        }
        verify_pre_enable(admin.as_ref(), &roles).await?;
        admin.auth_enable().await?;
        enabled_by_bootstrap = true;
        tracing::info!("etcd 认证已启用（角色/用户已就绪并通过读回校验）");
    } else if !auth_was_enabled {
        tracing::warn!("etcd 认证未启用（按请求保留明文兼容窗口）；角色与用户已创建");
    }

    // ---- 6. 凭据落盘（启用后必须在能用新凭据连上的前提下写盘）----
    let mut verified = false;
    if opts.enable_auth && (auth_was_enabled || enabled_by_bootstrap) {
        match connect_authenticated(
            connector,
            &endpoints,
            opts.auth_mode,
            ca.clone(),
            ROLE_ROOT,
            &root_password,
        )
        .await
        {
            Ok(admin2) => {
                let probe = admin2.role_get("vcm").await?;
                verified = probe.is_some();
                if !verified {
                    return Err(auth_err(
                        "认证启用后校验失败：无法读回 vcm 角色（请检查 root 凭据与 auth 状态）",
                    ));
                }
            }
            Err(e) => {
                return Err(auth_err(format!(
                    "认证启用后无法用 root 凭据重连（{}）。如确认已锁死，请执行 `vcm-etcd-auth disable --yes` 回滚认证",
                    e.msg
                )));
            }
        }
    }

    let mut identities = Vec::new();
    for (spec, password, rotated, generation) in pending {
        let mut written = false;
        if let Some(pw) = password.clone() {
            let cred = CredentialFile {
                identity: spec.identity.clone(),
                username: spec.identity.clone(),
                password: Some(pw),
                endpoints: endpoints.clone(),
                auth_mode: opts.auth_mode.as_str().to_string(),
                ca_cert: layout.ca_cert().display().to_string(),
                client_cert: opts
                    .auth_mode
                    .uses_client_cert()
                    .then(|| layout.identity_cert(&spec.identity).display().to_string()),
                client_key: opts
                    .auth_mode
                    .uses_client_cert()
                    .then(|| layout.identity_key(&spec.identity).display().to_string()),
                key_prefixes: spec.key_prefixes.clone(),
                generation,
                issued_at: chrono::Utc::now().to_rfc3339(),
            };
            store.write(cred)?;
            written = true;
        }
        identities.push(IdentityOutcome {
            username: spec.identity.clone(),
            identity: spec.identity,
            roles: spec.roles,
            key_prefixes: spec.key_prefixes,
            password_rotated: rotated,
            credential_written: written,
            generation,
        });
    }

    Ok(BootstrapReport {
        auth_was_enabled,
        auth_enabled: auth_was_enabled || enabled_by_bootstrap,
        auth_enabled_by_bootstrap: enabled_by_bootstrap,
        root_password_generated,
        root_password_file: layout.root_password_file().display().to_string(),
        roles_ensured: all_roles().into_iter().map(|r| r.name).collect(),
        identities,
        verified_after_enable: verified,
    })
}

/// root 口令：显式 → 文件 → 生成并落盘（0600）
fn resolve_root_password(
    layout: &CredentialLayout,
    explicit: Option<&str>,
) -> Result<String, VcmError> {
    if let Some(p) = explicit
        && !p.is_empty()
    {
        pki::write_root_password(layout, p)?;
        return Ok(p.to_string());
    }
    if let Some(p) = pki::read_root_password(layout)? {
        return Ok(p);
    }
    let p = generate_password();
    pki::write_root_password(layout, &p)?;
    tracing::info!(
        path = %layout.root_password_file().display(),
        "已生成 etcd root 口令并落盘（0600；内容不写日志）"
    );
    Ok(p)
}

async fn connect_admin(
    connector: &dyn EtcdAuthConnector,
    endpoints: &[String],
    mode: EtcdAuthMode,
    ca: Option<std::path::PathBuf>,
    root_password: &str,
) -> Result<(Arc<dyn EtcdAuthAdmin>, bool), VcmError> {
    match connector.connect_anonymous().await {
        Ok(admin) => match admin.auth_enabled().await {
            Ok(false) => Ok((admin, false)),
            Ok(true) => {
                drop(admin);
                let admin = connect_authenticated(
                    connector,
                    endpoints,
                    mode,
                    ca,
                    ROLE_ROOT,
                    root_password,
                )
                .await
                .map_err(|e| {
                    auth_err(format!(
                        "etcd 认证已启用，但 root 凭据无法连接（{}）；请检查 {}",
                        e.msg,
                        "/etc/vos/etcd/pki/root.password"
                    ))
                })?;
                Ok((admin, true))
            }
            Err(e) => Err(e),
        },
        Err(e) => {
            // 匿名连接失败（典型：服务端已要求客户端证书）→ 视为已启用认证
            let admin =
                connect_authenticated(connector, endpoints, mode, ca, ROLE_ROOT, root_password)
                    .await?;
            tracing::debug!(err = %e.msg, "匿名连接失败，按已启用认证处理");
            Ok((admin, true))
        }
    }
}

fn endpoints_hosts(endpoints: &[String]) -> Vec<String> {
    endpoints
        .iter()
        .filter_map(|e| {
            let rest = e.split_once("://").map(|(_, r)| r).unwrap_or(e);
            let (host, _) = rest.rsplit_once(':')?;
            Some(host.to_string())
        })
        .collect()
}

/// 轮换指定身份口令（需要 root 管理凭据）
pub async fn rotate_identity(
    connector: &dyn EtcdAuthConnector,
    store: &CredentialStore,
    endpoints: &[String],
    mode: EtcdAuthMode,
    identity: &str,
) -> Result<IdentityOutcome, VcmError> {
    validate_identity(identity).map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
    let layout = store.layout().clone();
    let root_password = pki::read_root_password(&layout)?.ok_or_else(|| {
        auth_err(format!(
            "缺少 root 口令文件 {}，无法轮换身份凭据",
            layout.root_password_file().display()
        ))
    })?;
    let ca = mode.requires_tls().then(|| layout.ca_cert());
    let endpoints = normalize_endpoints(endpoints, mode.requires_tls());
    let admin = connect_authenticated(
        connector,
        &endpoints,
        mode,
        ca,
        ROLE_ROOT,
        &root_password,
    )
    .await?;

    let existing = store.read(identity)?.ok_or_else(|| {
        VcmError::new(
            codes::NOT_FOUND,
            format!("本机无身份 {identity} 的凭据文件（无法确认其角色）"),
        )
    })?;
    let roles = admin
        .user_get(identity)
        .await?
        .map(|u| u.roles)
        .unwrap_or_default();
    let password = if mode.uses_password() {
        Some(generate_password())
    } else {
        None
    };
    if let Some(p) = password.as_deref() {
        admin.user_set_password(identity, p).await?;
    }
    if mode.uses_client_cert() {
        pki::ensure_pki(
            &layout,
            &endpoints_hosts(&endpoints),
            &[identity.to_string()],
            true,
        )?;
    }
    let generation = existing.generation + 1;
    let cred = CredentialFile {
        identity: identity.to_string(),
        username: identity.to_string(),
        password,
        endpoints: endpoints.clone(),
        auth_mode: mode.as_str().to_string(),
        ca_cert: layout.ca_cert().display().to_string(),
        client_cert: mode
            .uses_client_cert()
            .then(|| layout.identity_cert(identity).display().to_string()),
        client_key: mode
            .uses_client_cert()
            .then(|| layout.identity_key(identity).display().to_string()),
        key_prefixes: existing.key_prefixes.clone(),
        generation,
        issued_at: chrono::Utc::now().to_rfc3339(),
    };
    store.write(cred)?;
    Ok(IdentityOutcome {
        identity: identity.to_string(),
        username: identity.to_string(),
        roles,
        key_prefixes: existing.key_prefixes,
        password_rotated: true,
        credential_written: true,
        generation,
    })
}

/// 吊销身份（删除 etcd 用户 + 本机凭据/证书）
pub async fn revoke_identity(
    connector: &dyn EtcdAuthConnector,
    store: &CredentialStore,
    endpoints: &[String],
    mode: EtcdAuthMode,
    identity: &str,
) -> Result<bool, VcmError> {
    validate_identity(identity).map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
    if identity == ROLE_ROOT {
        return Err(VcmError::new(
            codes::PARAM_INVALID,
            "禁止吊销 root 身份（会永久失去 etcd 管理能力）",
        ));
    }
    let layout = store.layout().clone();
    let root_password = pki::read_root_password(&layout)?.ok_or_else(|| {
        auth_err(format!(
            "缺少 root 口令文件 {}，无法吊销身份",
            layout.root_password_file().display()
        ))
    })?;
    let ca = mode.requires_tls().then(|| layout.ca_cert());
    let endpoints = normalize_endpoints(endpoints, mode.requires_tls());
    let admin = connect_authenticated(
        connector,
        &endpoints,
        mode,
        ca,
        ROLE_ROOT,
        &root_password,
    )
    .await?;
    let existed = admin.user_get(identity).await?.is_some();
    admin.user_delete(identity).await?;
    store.remove(identity)?;
    Ok(existed)
}

/// 状态探测（不改变任何状态；无密码输出）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub auth_enabled: bool,
    pub auth_probe_error: Option<String>,
    pub roles: Vec<RoleStateSummary>,
    pub users: Vec<String>,
    pub pki_present: bool,
    pub root_password_present: bool,
}

/// 角色摘要
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleStateSummary {
    pub name: String,
    pub present: bool,
    pub grants: Vec<String>,
}

/// 读取 etcd 侧认证状态（尽量降级：不可达时返回 `auth_probe_error`）
pub async fn probe_status(
    connector: &dyn EtcdAuthConnector,
    store: &CredentialStore,
    endpoints: &[String],
    mode: EtcdAuthMode,
) -> AuthStatus {
    let layout = store.layout().clone();
    let mut out = AuthStatus {
        auth_enabled: false,
        auth_probe_error: None,
        roles: Vec::new(),
        users: Vec::new(),
        pki_present: pki::pki_present(&layout),
        root_password_present: pki::read_root_password(&layout).ok().flatten().is_some(),
    };
    let admin = match connector.connect_anonymous().await {
        Ok(a) => match a.auth_enabled().await {
            Ok(true) => {
                let root_password = pki::read_root_password(&layout).ok().flatten();
                match root_password {
                    Some(pw) => connect_authenticated(
                        connector,
                        endpoints,
                        mode,
                        mode.requires_tls().then(|| layout.ca_cert()),
                        ROLE_ROOT,
                        &pw,
                    )
                    .await
                    .ok(),
                    None => None,
                }
            }
            Ok(false) => Some(a),
            Err(e) => {
                out.auth_probe_error = Some(e.msg);
                None
            }
        },
        Err(e) => {
            out.auth_probe_error = Some(e.msg);
            let root_password = pki::read_root_password(&layout).ok().flatten();
            match root_password {
                Some(pw) => connect_authenticated(
                    connector,
                    endpoints,
                    mode,
                    mode.requires_tls().then(|| layout.ca_cert()),
                    ROLE_ROOT,
                    &pw,
                )
                .await
                .ok(),
                None => None,
            }
        }
    };
    let Some(admin) = admin else { return out };
    out.auth_enabled = admin.auth_enabled().await.unwrap_or(false);
    for name in ["vos-ceph", "vos-ceph-agent", "vcm", "vcm-readonly"] {
        match admin.role_get(name).await {
            Ok(Some(r)) => out.roles.push(RoleStateSummary {
                name: r.name,
                present: true,
                grants: r
                    .grants
                    .iter()
                    .map(|g| format!("{}:{}{}", g.kind.as_str(), g.key, if g.prefix { "*" } else { "" }))
                    .collect(),
            }),
            Ok(None) => out.roles.push(RoleStateSummary {
                name: name.to_string(),
                present: false,
                grants: Vec::new(),
            }),
            Err(e) => out.auth_probe_error = Some(e.msg),
        }
    }
    out.users = admin.user_list().await.unwrap_or_default();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::etcd::PermKind;
    use crate::etcd::auth_admin::MockEtcdAuthConnector;
    use crate::etcd::security::{CredentialLayout, agent_identity, default_identities};
    use tempfile::TempDir;

    fn store() -> (TempDir, CredentialStore) {
        let tmp = TempDir::new().unwrap();
        let layout = CredentialLayout::new(tmp.path().join("etcd"));
        pki::ensure_pki(&layout, &["10.0.0.1".to_string()], &[], false).unwrap();
        (tmp, CredentialStore::new(layout))
    }

    fn opts(mode: EtcdAuthMode) -> BootstrapOptions {
        BootstrapOptions::new(
            vec!["10.0.0.1:2379".into()],
            mode,
            default_identities(&["10.0.0.1".to_string()]),
        )
    }

    #[tokio::test]
    async fn bootstrap_creates_roles_users_then_enables_auth() {
        let (_tmp, store) = store();
        let (state, conn) = MockEtcdAuthConnector::new();
        let report = bootstrap(&conn, &store, &opts(EtcdAuthMode::User))
            .await
            .unwrap();

        assert!(!report.auth_was_enabled);
        assert!(report.auth_enabled);
        assert!(report.auth_enabled_by_bootstrap);
        assert!(report.verified_after_enable);

        let events = state.events();
        let enable_pos = events.iter().position(|e| e == "auth_enable").unwrap();
        let last_user_pos = events
            .iter()
            .rposition(|e| e.starts_with("user_upsert:"))
            .unwrap();
        assert!(last_user_pos < enable_pos, "必须先建用户再启用认证: {events:?}");
        for role in ["vos-ceph", "vos-ceph-agent", "vcm", "vcm-readonly"] {
            assert!(
                events.iter().any(|e| e == &format!("role_upsert:{role}")),
                "缺少角色 {role}"
            );
        }

        // 用户与角色
        let users = state.users();
        for id in ["root", "vcm", "vcm-readonly", "vos-ceph", "agent-10-0-0-1"] {
            assert!(users.contains_key(id), "缺少用户 {id}");
            assert!(users[id].0, "用户 {id} 必须设置口令");
        }
        assert_eq!(users["agent-10-0-0-1"].1, vec!["vos-ceph-agent".to_string()]);
        assert_eq!(
            state.roles()["vos-ceph"],
            vec![crate::etcd::auth_admin::RoleGrant {
                key: "/vos-ceph/".into(),
                kind: PermKind::ReadWrite,
                prefix: true
            }]
        );

        // 凭据文件 0600 落盘且不含 root
        let ids = store.list().unwrap();
        assert!(ids.contains(&"vcm".to_string()));
        assert!(!ids.contains(&"root".to_string()));
        let cred = store.read("vcm").unwrap().unwrap();
        assert_eq!(cred.auth_mode, "user");
        assert_eq!(cred.generation, 1);
        assert!(cred.password.is_some());
        assert!(store.layout().root_password_file().exists());
    }

    #[tokio::test]
    async fn bootstrap_is_idempotent_and_keeps_passwords() {
        let (_tmp, store) = store();
        let (_state, conn) = MockEtcdAuthConnector::new();
        let r1 = bootstrap(&conn, &store, &opts(EtcdAuthMode::User))
            .await
            .unwrap();
        let pw1 = store.read("vcm").unwrap().unwrap().password;
        let r2 = bootstrap(&conn, &store, &opts(EtcdAuthMode::User))
            .await
            .unwrap();
        assert!(r2.auth_was_enabled, "第二次应识别到 auth 已启用");
        assert!(!r2.auth_enabled_by_bootstrap);
        assert!(r2.identities.iter().all(|i| !i.password_rotated));
        assert_eq!(store.read("vcm").unwrap().unwrap().password, pw1, "不得静默改口令");
        assert_eq!(r1.identities.len(), r2.identities.len());
    }

    #[tokio::test]
    async fn bootstrap_with_rotate_changes_all_passwords() {
        let (_tmp, store) = store();
        let (_state, conn) = MockEtcdAuthConnector::new();
        bootstrap(&conn, &store, &opts(EtcdAuthMode::User))
            .await
            .unwrap();
        let pw1 = store.read("vcm").unwrap().unwrap().password;
        let mut o = opts(EtcdAuthMode::User);
        o.rotate = true;
        let r = bootstrap(&conn, &store, &o).await.unwrap();
        assert!(r.identities.iter().all(|i| i.password_rotated));
        assert_ne!(store.read("vcm").unwrap().unwrap().password, pw1);
        assert_eq!(store.read("vcm").unwrap().unwrap().generation, 2);
    }

    #[tokio::test]
    async fn bootstrap_refuses_to_enable_auth_in_anonymous_mode() {
        let (_tmp, store) = store();
        let (state, conn) = MockEtcdAuthConnector::new();
        let err = bootstrap(&conn, &store, &opts(EtcdAuthMode::Disabled))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::ETCD_AUTH_FAILED);
        assert!(err.msg.contains("匿名"), "必须是可操作错误: {}", err.msg);
        assert!(!state.is_enabled(), "拒绝后不得启用认证");
    }

    #[tokio::test]
    async fn bootstrap_without_enable_keeps_plaintext_window() {
        let (_tmp, store) = store();
        let (state, conn) = MockEtcdAuthConnector::new();
        let mut o = opts(EtcdAuthMode::User);
        o.enable_auth = false;
        let r = bootstrap(&conn, &store, &o).await.unwrap();
        assert!(!r.auth_enabled);
        assert!(!state.is_enabled());
        assert!(store.read("vcm").unwrap().is_some(), "仍应写凭据以便切换");
    }

    #[tokio::test]
    async fn mtls_mode_issues_client_certs_and_omits_password_when_cert_only() {
        let (_tmp, store) = store();
        let (_state, conn) = MockEtcdAuthConnector::new();
        let mut o = opts(EtcdAuthMode::MutualTls);
        o.identities = vec![crate::etcd::security::IdentitySpec::new(
            "vcm",
            vec!["vcm".to_string()],
            vec!["/vos/cluster/".to_string()],
        )];
        let r = bootstrap(&conn, &store, &o).await.unwrap();
        assert!(store.layout().identity_cert("vcm").exists());
        let cred = store.read("vcm").unwrap().unwrap();
        assert_eq!(cred.auth_mode, "mtls");
        assert!(cred.password.is_none(), "纯证书模式不得落明文口令");
        assert_eq!(r.identities[0].identity, "vcm");
    }

    #[tokio::test]
    async fn rotate_and_revoke_identity() {
        let (_tmp, store) = store();
        let (state, conn) = MockEtcdAuthConnector::new();
        bootstrap(&conn, &store, &opts(EtcdAuthMode::User))
            .await
            .unwrap();
        let pw1 = store.read("vcm").unwrap().unwrap().password;

        let out = rotate_identity(
            &conn,
            &store,
            &["10.0.0.1:2379".into()],
            EtcdAuthMode::User,
            "vcm",
        )
        .await
        .unwrap();
        assert_eq!(out.generation, 2);
        assert_ne!(store.read("vcm").unwrap().unwrap().password, pw1);

        let existed = revoke_identity(
            &conn,
            &store,
            &["10.0.0.1:2379".into()],
            EtcdAuthMode::User,
            "agent-10-0-0-1",
        )
        .await
        .unwrap();
        assert!(existed);
        assert!(!state.users().contains_key("agent-10-0-0-1"));
        assert!(!store.exists("agent-10-0-0-1"));

        // root 不可吊销
        let err = revoke_identity(
            &conn,
            &store,
            &["10.0.0.1:2379".into()],
            EtcdAuthMode::User,
            "root",
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::PARAM_INVALID);
    }

    #[tokio::test]
    async fn probe_status_reports_roles_without_secrets() {
        let (_tmp, store) = store();
        let (_state, conn) = MockEtcdAuthConnector::new();
        bootstrap(&conn, &store, &opts(EtcdAuthMode::User))
            .await
            .unwrap();
        let st = probe_status(&conn, &store, &["10.0.0.1:2379".into()], EtcdAuthMode::User).await;
        assert!(st.auth_enabled);
        assert!(st.pki_present);
        assert!(st.root_password_present);
        assert_eq!(st.roles.len(), 4);
        assert!(st.roles.iter().all(|r| r.present));
        let text = format!("{st:?}");
        let pw = store.read("vcm").unwrap().unwrap().password.unwrap();
        assert!(!text.contains(&pw), "状态输出不得含口令");
    }

    #[test]
    fn generated_passwords_are_long_and_unique() {
        let a = generate_password();
        let b = generate_password();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn agent_identity_for_non_ip_is_none() {
        assert!(agent_identity("host-a").is_none());
    }
}