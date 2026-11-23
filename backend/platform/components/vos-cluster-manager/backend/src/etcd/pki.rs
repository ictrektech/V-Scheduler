//! etcd TLS 材料签发（集群内部 CA + 服务端证书 + 每身份客户端证书）
//!
//! 采用纯 Rust `rcgen`（ECDSA P-256）：runtime 容器只装 `ca-certificates`，没有
//! `openssl` 可执行文件，因此不能依赖外部 CLI 签发。
//!
//! 落盘权限（与 `docs/etcd-auth-tls.md` 一致）：
//! - 目录 0700；私钥 0600；证书 / CA 0644
//! - CA 私钥只在执行 bootstrap 的节点存在，**不随凭据分发**
//!
//! 幂等：CA 已存在则复用（不重签，避免已分发证书失效）；服务端证书在缺失或
//! `force` 时重签；客户端证书同理。

use std::path::Path;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};

use crate::error::{codes, VcmError};

use super::security::{CredentialLayout, validate_identity};

/// 服务端证书固定 SAN（本机回环 + 节点 IP 由调用方补充）
const LOCAL_SANS: [&str; 3] = ["127.0.0.1", "::1", "localhost"];

/// PKI 落盘结果（不含任何私钥内容）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkiReport {
    pub ca_created: bool,
    pub server_created: bool,
    pub ca_cert: String,
    pub server_cert: String,
    pub server_key: String,
    /// 服务端证书 SAN（运维核对用）
    pub server_sans: Vec<String>,
}

fn io_err(ctx: &str, e: impl std::fmt::Display) -> VcmError {
    VcmError::new(codes::IO_FAILED, format!("{ctx}: {e}"))
}

fn gen_err(ctx: &str, e: impl std::fmt::Display) -> VcmError {
    VcmError::new(codes::ETCD_AUTH_FAILED, format!("{ctx}: {e}"))
}

/// 创建目录（0700，幂等）
pub fn ensure_private_dir(path: &Path) -> Result<(), VcmError> {
    std::fs::create_dir_all(path).map_err(|e| io_err(&format!("创建目录 {}", path.display()), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| io_err(&format!("设置目录权限 {}", path.display()), e))?;
    }
    Ok(())
}

/// 写私密文件（0600；父目录自动以 0700 创建）
pub fn write_secret_file(path: &Path, contents: &str) -> Result<(), VcmError> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    std::fs::write(path, contents).map_err(|e| io_err(&format!("写入 {}", path.display()), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| io_err(&format!("设置文件权限 {}", path.display()), e))?;
    }
    Ok(())
}

/// 写公开文件（0644；证书 / CA 可被同机其他应用只读挂载）
pub fn write_public_file(path: &Path, contents: &str) -> Result<(), VcmError> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    std::fs::write(path, contents).map_err(|e| io_err(&format!("写入 {}", path.display()), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
            .map_err(|e| io_err(&format!("设置文件权限 {}", path.display()), e))?;
    }
    Ok(())
}

/// CA 材料（内存）
struct CaMaterial {
    params: CertificateParams,
    key: KeyPair,
}

fn load_or_create_ca(layout: &CredentialLayout) -> Result<(CaMaterial, bool), VcmError> {
    let cert_path = layout.ca_cert();
    let key_path = layout.ca_key();
    if cert_path.exists() && key_path.exists() {
        std::fs::read_to_string(&cert_path).map_err(|e| io_err("读取 CA 证书", e))?;
        let key_pem = std::fs::read_to_string(&key_path).map_err(|e| io_err("读取 CA 私钥", e))?;
        let key = KeyPair::from_pem(&key_pem)
            .map_err(|e| gen_err("解析 CA 私钥失败（文件可能损坏）", e))?;
        let mut params = CertificateParams::new(Vec::<String>::new())
            .map_err(|e| gen_err("构造 CA 参数", e))?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(DnType::CommonName, "vos-etcd-ca");
        return Ok((CaMaterial { params, key }, false));
    }

    let mut params =
        CertificateParams::new(Vec::<String>::new()).map_err(|e| gen_err("构造 CA 参数", e))?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(DnType::CommonName, "vos-etcd-ca");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "VOS Cluster");
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let key = KeyPair::generate().map_err(|e| gen_err("生成 CA 私钥", e))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| gen_err("自签 CA 证书", e))?;
    let cert_pem = cert.pem();
    let key_pem = key.serialize_pem();

    write_public_file(&cert_path, &cert_pem)?;
    write_secret_file(&key_path, &key_pem)?;
    tracing::info!(ca = %cert_path.display(), "etcd 集群内部 CA 已生成");

    Ok((CaMaterial { params, key }, true))
}

/// 服务端证书的 SAN 集合（节点 IP + 回环）
pub fn server_sans(node_ips: &[String]) -> Vec<String> {
    let mut sans: Vec<String> = LOCAL_SANS.iter().map(|s| s.to_string()).collect();
    for ip in node_ips {
        let ip = ip.trim();
        if !ip.is_empty() && !sans.iter().any(|s| s == ip) {
            sans.push(ip.to_string());
        }
    }
    sans
}

/// 确保证书与私钥对（CA、服务端、指定身份客户端）
///
/// - `node_ips`：该 etcd **成员节点**的 IP（写入服务端证书 SAN；同一份服务端证书
///   可分发到全部成员，便于统一重签）
/// - `identities`：需要客户端证书的身份（仅 mTLS 模式需要）
/// - `force`：强制重签服务端 / 客户端证书（CA 不重签）
pub fn ensure_pki(
    layout: &CredentialLayout,
    node_ips: &[String],
    identities: &[String],
    force: bool,
) -> Result<PkiReport, VcmError> {
    ensure_private_dir(layout.root())?;
    ensure_private_dir(&layout.pki_dir())?;
    ensure_private_dir(&layout.clients_dir())?;

    let (ca, ca_created) = load_or_create_ca(layout)?;
    let sans = server_sans(node_ips);

    let server_exists = layout.server_cert().exists() && layout.server_key().exists();
    let server_created = !server_exists || force;
    if server_created {
        let mut params =
            CertificateParams::new(sans.clone()).map_err(|e| gen_err("构造服务端证书参数", e))?;
        params.distinguished_name.push(DnType::CommonName, "etcd-server");
        // peer TLS 复用同一证书，故同时具备 ServerAuth / ClientAuth
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let key = KeyPair::generate().map_err(|e| gen_err("生成服务端私钥", e))?;
        let issuer = Issuer::from_params(&ca.params, &ca.key);
        let cert = params
            .signed_by(&key, &issuer)
            .map_err(|e| gen_err("签发服务端证书", e))?;
        write_public_file(&layout.server_cert(), &cert.pem())?;
        write_secret_file(&layout.server_key(), &key.serialize_pem())?;
        tracing::info!(
            cert = %layout.server_cert().display(),
            sans = %sans.join(","),
            "etcd 服务端证书已签发"
        );
    }

    for identity in identities {
        ensure_client_cert(layout, &ca, identity, force)?;
    }

    Ok(PkiReport {
        ca_created,
        server_created,
        ca_cert: layout.ca_cert().display().to_string(),
        server_cert: layout.server_cert().display().to_string(),
        server_key: layout.server_key().display().to_string(),
        server_sans: sans,
    })
}

fn ensure_client_cert(
    layout: &CredentialLayout,
    ca: &CaMaterial,
    identity: &str,
    force: bool,
) -> Result<bool, VcmError> {
    validate_identity(identity)
        .map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
    let cert_path = layout.identity_cert(identity);
    let key_path = layout.identity_key(identity);
    if cert_path.exists() && key_path.exists() && !force {
        return Ok(false);
    }
    let mut params = CertificateParams::new(Vec::<String>::new())
        .map_err(|e| gen_err("构造客户端证书参数", e))?;
    params
        .distinguished_name
        .push(DnType::CommonName, identity.to_string());
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let key = KeyPair::generate().map_err(|e| gen_err("生成客户端私钥", e))?;
    let issuer = Issuer::from_params(&ca.params, &ca.key);
    let cert = params
        .signed_by(&key, &issuer)
        .map_err(|e| gen_err("签发客户端证书", e))?;
    write_public_file(&cert_path, &cert.pem())?;
    write_secret_file(&key_path, &key.serialize_pem())?;
    tracing::info!(identity, cert = %cert_path.display(), "etcd 客户端证书已签发");
    Ok(true)
}

/// 读取 CA 证书 PEM（客户端连接校验用）
pub fn read_ca_pem(layout: &CredentialLayout) -> Result<String, VcmError> {
    std::fs::read_to_string(layout.ca_cert())
        .map_err(|e| io_err(&format!("读取 CA 证书 {} 失败（请先执行 pki generate）", layout.ca_cert().display()), e))
}

/// 读取 root 口令（不存在返回 None）
pub fn read_root_password(layout: &CredentialLayout) -> Result<Option<String>, VcmError> {
    let path = layout.root_password_file();
    if !path.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&path).map_err(|e| io_err("读取 root 口令文件", e))?;
    let s = s.trim().to_string();
    Ok(if s.is_empty() { None } else { Some(s) })
}

/// 写入 root 口令（0600；幂等：内容相同则不动）
pub fn write_root_password(layout: &CredentialLayout, password: &str) -> Result<(), VcmError> {
    write_secret_file(&layout.root_password_file(), password)
}

/// CA 私钥 PEM 是否与证书配套（供状态接口做基本健全性检查：仅判断存在性）
pub fn pki_present(layout: &CredentialLayout) -> bool {
    layout.ca_cert().exists() && layout.server_cert().exists() && layout.server_key().exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn layout() -> (TempDir, CredentialLayout) {
        let tmp = TempDir::new().unwrap();
        let layout = CredentialLayout::new(tmp.path().join("etcd"));
        (tmp, layout)
    }

    #[test]
    fn ensure_pki_creates_ca_and_server_cert_idempotently() {
        let (_tmp, layout) = layout();
        let ips = vec!["10.100.10.21".to_string(), "10.100.10.22".to_string()];
        let r1 = ensure_pki(&layout, &ips, &[], false).unwrap();
        assert!(r1.ca_created);
        assert!(r1.server_created);
        assert!(r1.server_sans.contains(&"10.100.10.21".to_string()));
        assert!(r1.server_sans.contains(&"127.0.0.1".to_string()));

        let ca_pem = std::fs::read_to_string(layout.ca_cert()).unwrap();
        assert!(ca_pem.contains("BEGIN CERTIFICATE"));
        let srv_pem = std::fs::read_to_string(layout.server_cert()).unwrap();
        assert!(srv_pem.contains("BEGIN CERTIFICATE"));
        let srv_key = std::fs::read_to_string(layout.server_key()).unwrap();
        assert!(srv_key.contains("PRIVATE KEY"));

        // 二次调用：复用，不重签
        let r2 = ensure_pki(&layout, &ips, &[], false).unwrap();
        assert!(!r2.ca_created);
        assert!(!r2.server_created);
        assert_eq!(
            std::fs::read_to_string(layout.ca_cert()).unwrap(),
            ca_pem,
            "CA 不得被重签（已分发证书会失效）"
        );
        assert_eq!(std::fs::read_to_string(layout.server_cert()).unwrap(), srv_pem);

        // force：服务端重签，CA 保持不变
        let r3 = ensure_pki(&layout, &ips, &[], true).unwrap();
        assert!(r3.server_created);
        assert!(!r3.ca_created);
        assert_ne!(std::fs::read_to_string(layout.server_cert()).unwrap(), srv_pem);
        assert_eq!(std::fs::read_to_string(layout.ca_cert()).unwrap(), ca_pem);
    }

    #[test]
    fn client_certs_are_issued_per_identity() {
        let (_tmp, layout) = layout();
        let ids = vec!["vcm".to_string(), "agent-10-100-10-21".to_string()];
        ensure_pki(&layout, &["10.100.10.21".to_string()], &ids, false).unwrap();
        for id in &ids {
            assert!(layout.identity_cert(id).exists(), "{id} 证书缺失");
            assert!(layout.identity_key(id).exists(), "{id} 私钥缺失");
            let pem = std::fs::read_to_string(layout.identity_cert(id)).unwrap();
            assert!(pem.contains("BEGIN CERTIFICATE"));
        }
        assert!(ensure_pki(&layout, &[], &ids, false).is_ok());
        assert!(
            ensure_pki(&layout, &[], &["../bad".to_string()], false).is_err(),
            "非法身份名必须被拒绝（防目录穿越）"
        );
    }

    #[cfg(unix)]
    #[test]
    fn secret_files_are_0600_and_certs_0644() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, layout) = layout();
        ensure_pki(
            &layout,
            &["10.0.0.1".to_string()],
            &["vcm".to_string()],
            false,
        )
        .unwrap();
        let mode = |p: &std::path::Path| {
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode(&layout.ca_key()), 0o600);
        assert_eq!(mode(&layout.server_key()), 0o600);
        assert_eq!(mode(&layout.identity_key("vcm")), 0o600);
        assert_eq!(mode(&layout.ca_cert()), 0o644);
        assert_eq!(mode(&layout.server_cert()), 0o644);
        assert_eq!(mode(&layout.identity_cert("vcm")), 0o644);
        assert_eq!(mode(&layout.clients_dir()), 0o700);
    }

    #[test]
    fn root_password_roundtrip() {
        let (_tmp, layout) = layout();
        assert!(read_root_password(&layout).unwrap().is_none());
        write_root_password(&layout, "s3cret-root").unwrap();
        assert_eq!(
            read_root_password(&layout).unwrap().as_deref(),
            Some("s3cret-root")
        );
    }
}