//! 应用启动配置（clap derive + env，优先级 CLI 参数 > 环境变量 > 默认值）

use std::{net::IpAddr, path::PathBuf};

use clap::Parser;

use crate::etcd::credential_store::CredentialStore;
use crate::etcd::security::{
    CredentialLayout, DEFAULT_VCM_IDENTITY, EtcdAuthMode, EtcdConnectSpec, normalize_endpoints,
    validate_connect_spec,
};

/// VOS 集群管理服务后端
#[derive(Debug, Clone, Parser)]
#[command(name = "vos-cluster-manager", version, about)]
pub struct Config {
    /// HTTP 监听端口
    #[arg(long, env = "VOS_CLUSTER_MANAGER_PORT", default_value_t = 18093)]
    pub port: u16,

    #[arg(long, env = "VOS_CLUSTER_MANAGER_BIND_ADDRESS", default_value = "0.0.0.0")]
    pub bind_address: IpAddr,

    /// 前端 dist 目录（构建产物，运行时托管）
    #[arg(long, env = "VOS_CLUSTER_MANAGER_FRONTEND_DIR")]
    pub frontend_dir: PathBuf,

    /// 数据目录（管理面本地状态持久化根，如 SPA 之外的管理状态库）
    #[arg(long, env = "VOS_CLUSTER_MANAGER_DATA_DIR")]
    pub data_dir: PathBuf,

    /// （可选）URL 前缀，使整个组件可托管在任意子路径下（VOS iframe 场景必填）
    #[arg(long, env = "VOS_CLUSTER_MANAGER_URL_BASE", default_value = "/")]
    pub url_base: String,

    /// 日志级别：error / warn / info / debug / trace
    #[arg(long, env = "VOS_CLUSTER_MANAGER_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// etcd 端点（逗号分隔；TLS 模式需 `https://<ip>:2379`）
    #[arg(
        long,
        env = "VOS_CLUSTER_MANAGER_ETCD_ENDPOINTS",
        default_value = "127.0.0.1:2379",
        value_delimiter = ','
    )]
    pub etcd_endpoints: Vec<String>,

    /// etcd 认证模式：disabled | auto | user | mtls | mtls_user
    ///
    /// 默认 `disabled` = 保持既有明文行为；启用认证后必须改为显式模式
    /// （`auto` 在本机存在凭据文件时自动按凭据连接）。
    #[arg(
        long,
        env = "VOS_CLUSTER_MANAGER_ETCD_AUTH_MODE",
        default_value = "disabled"
    )]
    pub etcd_auth_mode: String,

    /// etcd CA 证书路径（TLS 模式必填；缺省回退 `<credential-dir>/pki/ca.crt`）
    #[arg(long, env = "VOS_CLUSTER_MANAGER_ETCD_CA_CERT")]
    pub etcd_ca_cert: Option<PathBuf>,

    /// etcd 客户端证书路径（mTLS）
    #[arg(long, env = "VOS_CLUSTER_MANAGER_ETCD_CLIENT_CERT")]
    pub etcd_client_cert: Option<PathBuf>,

    /// etcd 客户端私钥路径（mTLS）
    #[arg(long, env = "VOS_CLUSTER_MANAGER_ETCD_CLIENT_KEY")]
    pub etcd_client_key: Option<PathBuf>,

    /// etcd 用户名（user / mtls_user 模式）
    #[arg(long, env = "VOS_CLUSTER_MANAGER_ETCD_USERNAME")]
    pub etcd_username: Option<String>,

    /// etcd 口令文件路径（单行；0600）
    #[arg(long, env = "VOS_CLUSTER_MANAGER_ETCD_PASSWORD_FILE")]
    pub etcd_password_file: Option<PathBuf>,

    /// 凭据根目录（默认 `/etc/vos/etcd`；内含 pki/ 与 clients/）
    #[arg(
        long,
        env = "VOS_CLUSTER_MANAGER_ETCD_CREDENTIAL_DIR",
        default_value = "/etc/vos/etcd"
    )]
    pub etcd_credential_dir: PathBuf,

    /// 本机消费的凭据身份名（默认 `vcm` → `<credential-dir>/clients/vcm.json`）
    #[arg(
        long,
        env = "VOS_CLUSTER_MANAGER_ETCD_IDENTITY",
        default_value = DEFAULT_VCM_IDENTITY
    )]
    pub etcd_identity: String,
}

impl Config {
    /// 归一化 url_base：保证以 `/` 开头、结尾无 `/`；空 / "/" / 纯斜杠归一为空字符串（根挂载）
    pub fn normalized_url_base(&self) -> String {
        let s = self.url_base.trim().trim_matches('/');
        if s.is_empty() {
            return String::new();
        }
        format!("/{s}")
    }

    /// 凭据根目录布局
    pub fn credential_layout(&self) -> CredentialLayout {
        CredentialLayout::new(self.etcd_credential_dir.clone())
    }

    /// 本机凭据存储
    pub fn credential_store(&self) -> CredentialStore {
        CredentialStore::new(self.credential_layout())
    }

    /// 解析 etcd 认证模式（非法值 → 可操作错误）
    pub fn etcd_auth_mode(&self) -> Result<EtcdAuthMode, String> {
        EtcdAuthMode::parse(&self.etcd_auth_mode).ok_or_else(|| {
            format!(
                "VOS_CLUSTER_MANAGER_ETCD_AUTH_MODE 非法: {:?}（允许 disabled / auto / user / mtls / mtls_user）",
                self.etcd_auth_mode
            )
        })
    }

    /// 构造 etcd 客户端连接参数（启动期硬门禁：显式认证模式下明文端点/缺凭据直接失败）
    ///
    /// 凭据来源优先级：显式参数 > 本机凭据文件（`<credential-dir>/clients/<identity>.json`）。
    pub fn etcd_connect_spec(&self) -> Result<EtcdConnectSpec, String> {
        let mode = self.etcd_auth_mode()?;
        let layout = self.credential_layout();
        let stored = self
            .credential_store()
            .read(&self.etcd_identity)
            .ok()
            .flatten();

        let effective_mode = match mode {
            EtcdAuthMode::Auto => match &stored {
                Some(cred) => EtcdAuthMode::parse(&cred.auth_mode).unwrap_or(EtcdAuthMode::User),
                None => EtcdAuthMode::Disabled,
            },
            other => other,
        };

        let mut spec = EtcdConnectSpec {
            endpoints: normalize_endpoints(&self.etcd_endpoints, effective_mode.requires_tls()),
            mode: effective_mode,
            ca_cert: self
                .etcd_ca_cert
                .clone()
                .or_else(|| effective_mode.requires_tls().then(|| layout.ca_cert())),
            client_cert: self.etcd_client_cert.clone(),
            client_key: self.etcd_client_key.clone(),
            username: self.etcd_username.clone(),
            password: None,
        };

        if let Some(p) = self.read_password_file()? {
            spec.password = Some(p);
        }

        if let Some(cred) = &stored {
            if spec.username.is_none() {
                spec.username = Some(cred.username.clone());
            }
            if spec.password.is_none() {
                spec.password = cred.password.clone();
            }
            if spec.client_cert.is_none() {
                spec.client_cert = cred.client_cert.clone().map(PathBuf::from);
            }
            if spec.client_key.is_none() {
                spec.client_key = cred.client_key.clone().map(PathBuf::from);
            }
            if spec.mode.requires_tls() && spec.ca_cert.is_none() {
                spec.ca_cert = Some(PathBuf::from(&cred.ca_cert));
            }
            if !cred.endpoints.is_empty() {
                spec.endpoints =
                    normalize_endpoints(&cred.endpoints, spec.mode.requires_tls());
            }
        }

        validate_connect_spec(&spec)?;
        Ok(spec)
    }

    fn read_password_file(&self) -> Result<Option<String>, String> {
        let Some(path) = &self.etcd_password_file else {
            return Ok(None);
        };
        let raw = std::fs::read_to_string(path).map_err(|e| {
            format!("读取 etcd 口令文件 {} 失败: {e}", path.display())
        })?;
        let p = raw.trim().to_string();
        if p.is_empty() {
            return Err(format!("etcd 口令文件 {} 为空", path.display()));
        }
        Ok(Some(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(url_base: &str) -> Config {
        Config {
            port: 18093,
            bind_address: "0.0.0.0".parse().unwrap(),
            frontend_dir: PathBuf::from("/tmp/dist"),
            data_dir: PathBuf::from("/tmp/data"),
            url_base: url_base.to_string(),
            log_level: "info".to_string(),
            etcd_endpoints: vec!["127.0.0.1:2379".to_string()],
            etcd_auth_mode: "disabled".to_string(),
            etcd_ca_cert: None,
            etcd_client_cert: None,
            etcd_client_key: None,
            etcd_username: None,
            etcd_password_file: None,
            etcd_credential_dir: PathBuf::from("/tmp/vcm-etcd"),
            etcd_identity: DEFAULT_VCM_IDENTITY.to_string(),
        }
    }

    #[test]
    fn normalized_url_base_variants() {
        assert_eq!(base("/").normalized_url_base(), "");
        assert_eq!(base("").normalized_url_base(), "");
        assert_eq!(base("//").normalized_url_base(), "");
        assert_eq!(
            base("/app/com.ictrek.vos-cluster-manager/").normalized_url_base(),
            "/app/com.ictrek.vos-cluster-manager"
        );
        assert_eq!(
            base("app/com.ictrek.vos-cluster-manager").normalized_url_base(),
            "/app/com.ictrek.vos-cluster-manager"
        );
        assert_eq!(base("/a/b/c").normalized_url_base(), "/a/b/c");
    }

    #[test]
    fn default_cli_parses() {
        let c = Config::try_parse_from([
            "vos-cluster-manager",
            "--frontend-dir",
            "/d",
            "--data-dir",
            "/x",
        ])
        .expect("前端目录与数据目录必填，其余取默认");
        assert_eq!(c.port, 18093);
        assert_eq!(c.url_base, "/");
        assert_eq!(c.log_level, "info");
        assert!(c.etcd_endpoints.contains(&"127.0.0.1:2379".to_string()));
        assert_eq!(c.etcd_auth_mode, "disabled");
        assert_eq!(c.etcd_credential_dir, PathBuf::from("/etc/vos/etcd"));
        assert_eq!(c.etcd_identity, "vcm");
    }

    #[test]
    fn default_mode_keeps_legacy_plaintext_behaviour() {
        let spec = base("/").etcd_connect_spec().unwrap();
        assert_eq!(spec.mode, EtcdAuthMode::Disabled);
        assert_eq!(spec.endpoints, vec!["http://127.0.0.1:2379".to_string()]);
        assert!(spec.username.is_none());
    }

    #[test]
    fn explicit_auth_mode_rejects_plaintext_endpoints_at_startup() {
        let mut c = base("/");
        c.etcd_auth_mode = "user".into();
        c.etcd_username = Some("vcm".into());
        c.etcd_password_file = None;
        let err = c.etcd_connect_spec().unwrap_err();
        assert!(err.contains("要求 TLS 端点"), "必须在启动期失败: {err}");
    }

    #[test]
    fn invalid_auth_mode_is_rejected() {
        let mut c = base("/");
        c.etcd_auth_mode = "nonsense".into();
        assert!(c.etcd_auth_mode().unwrap_err().contains("非法"));
    }

    #[test]
    fn user_mode_reads_password_file_and_tls_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pw = tmp.path().join("pw");
        std::fs::write(&pw, "s3cret\n").unwrap();
        let mut c = base("/");
        c.etcd_auth_mode = "user".into();
        c.etcd_endpoints = vec!["https://10.0.0.1:2379".into()];
        c.etcd_username = Some("vcm".into());
        c.etcd_password_file = Some(pw);
        c.etcd_credential_dir = tmp.path().join("etcd");
        let spec = c.etcd_connect_spec().unwrap();
        assert_eq!(spec.mode, EtcdAuthMode::User);
        assert_eq!(spec.username.as_deref(), Some("vcm"));
        assert_eq!(spec.password.as_deref(), Some("s3cret"));
        assert_eq!(spec.ca_cert, Some(c.credential_layout().ca_cert()));
    }

    #[test]
    fn auto_mode_uses_local_credential_when_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut c = base("/");
        c.etcd_credential_dir = tmp.path().join("etcd");
        c.etcd_auth_mode = "auto".into();
        // 无凭据文件 → 退回 disabled（保持旧行为）
        assert_eq!(c.etcd_connect_spec().unwrap().mode, EtcdAuthMode::Disabled);

        let store = c.credential_store();
        store
            .write(crate::etcd::CredentialFile {
                identity: "vcm".into(),
                username: "vcm".into(),
                password: Some("pw".into()),
                endpoints: vec!["https://10.0.0.1:2379".into()],
                auth_mode: "user".into(),
                ca_cert: c.credential_layout().ca_cert().display().to_string(),
                client_cert: None,
                client_key: None,
                key_prefixes: vec!["/vos/cluster/".into()],
                generation: 1,
                issued_at: "2026-11-02T00:00:00+08:00".into(),
            })
            .unwrap();
        let spec = c.etcd_connect_spec().unwrap();
        assert_eq!(spec.mode, EtcdAuthMode::User);
        assert_eq!(spec.username.as_deref(), Some("vcm"));
        assert_eq!(spec.endpoints, vec!["https://10.0.0.1:2379".to_string()]);
    }

    #[test]
    fn missing_password_in_user_mode_fails_with_actionable_message() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut c = base("/");
        c.etcd_credential_dir = tmp.path().join("etcd");
        c.etcd_auth_mode = "user".into();
        c.etcd_endpoints = vec!["https://10.0.0.1:2379".into()];
        let err = c.etcd_connect_spec().unwrap_err();
        assert!(err.contains("用户名与密码"), "{err}");
    }
}
