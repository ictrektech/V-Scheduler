//! 节点本机 etcd 客户端凭据文件的读写（0600）
//!
//! 每身份一份 JSON：`<credential-root>/clients/<identity>.json`，内容含端点、
//! 账号口令或 mTLS 证书路径——属于敏感文件，权限固定 0600、目录 0700。
//!
//! 只在**本机**落盘；跨节点分发由编排方调用目标节点的 credential sink 端点完成
//! （见 `handlers::etcd_auth`），不经过 `GET /api/nodes` 等任何公开读接口。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{codes, VcmError};

use super::pki::{ensure_private_dir, write_secret_file};
use super::security::{
    CredentialLayout, EtcdAuthMode, EtcdConnectSpec, normalize_endpoints, validate_identity,
};

/// etcd auth/TLS 完全关闭时写入凭据文件的传输字段
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialFile {
    /// 身份名（= etcd 用户名；同时是文件名）
    pub identity: String,
    /// etcd 用户名（mTLS-only 模式同样存在，用于 etcd 侧授权映射）
    pub username: String,
    /// 口令；`None` = 纯证书认证（不落明文口令）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// 归一化端点（含 scheme）
    pub endpoints: Vec<String>,
    /// 认证模式（`user` / `mtls` / `mtls_user`）
    pub auth_mode: String,
    /// CA 证书路径
    pub ca_cert: String,
    /// 客户端证书路径（mTLS）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,
    /// 客户端私钥路径（mTLS）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    /// 该身份可访问的 key 前缀（展示 / 文档用）
    #[serde(default)]
    pub key_prefixes: Vec<String>,
    /// 凭据代次（每次轮换 +1；消费方可据此判断是否需要重载）
    #[serde(default = "default_generation")]
    pub generation: u32,
    /// 签发时间（RFC3339）
    pub issued_at: String,
}

fn default_generation() -> u32 {
    1
}

impl CredentialFile {
    /// 转成 etcd 客户端连接参数
    pub fn connect_spec(&self) -> Result<EtcdConnectSpec, VcmError> {
        let mode = EtcdAuthMode::parse(&self.auth_mode).ok_or_else(|| {
            VcmError::new(
                codes::ETCD_AUTH_FAILED,
                format!("凭据文件 auth_mode 非法: {}", self.auth_mode),
            )
        })?;
        Ok(EtcdConnectSpec {
            endpoints: self.endpoints.clone(),
            mode,
            ca_cert: Some(PathBuf::from(&self.ca_cert)),
            client_cert: self.client_cert.clone().map(PathBuf::from),
            client_key: self.client_key.clone().map(PathBuf::from),
            username: if mode.uses_password() || !self.username.is_empty() {
                Some(self.username.clone())
            } else {
                None
            },
            password: self.password.clone(),
        })
    }
}

/// 凭据文件存储（绑定一个凭据根目录）
#[derive(Debug, Clone)]
pub struct CredentialStore {
    layout: CredentialLayout,
}

impl CredentialStore {
    /// 以布局构造
    pub fn new(layout: CredentialLayout) -> Self {
        Self { layout }
    }

    /// 布局（读取 PKI 路径用）
    pub fn layout(&self) -> &CredentialLayout {
        &self.layout
    }

    /// 写入凭据（0600，覆盖即轮换）
    ///
    /// 写入前做身份名校验（防目录穿越）；同时更新代次与签发时间。
    pub fn write(&self, mut cred: CredentialFile) -> Result<PathBuf, VcmError> {
        validate_identity(&cred.identity).map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
        ensure_private_dir(&self.layout.clients_dir())?;
        cred.endpoints = normalize_endpoints(&cred.endpoints, cred.auth_mode != "disabled");
        let path = self.layout.identity_file(&cred.identity);
        let json = serde_json::to_string_pretty(&cred)
            .map_err(|e| VcmError::new(codes::IO_FAILED, format!("序列化凭据失败: {e}")))?;
        write_secret_file(&path, &json)?;
        tracing::info!(
            identity = %cred.identity,
            generation = cred.generation,
            path = %path.display(),
            "etcd 客户端凭据已落盘（0600）"
        );
        Ok(path)
    }

    /// 读取凭据（不存在 → None）
    pub fn read(&self, identity: &str) -> Result<Option<CredentialFile>, VcmError> {
        validate_identity(identity).map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
        let path = self.layout.identity_file(identity);
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            VcmError::new(
                codes::IO_FAILED,
                format!("读取凭据 {} 失败: {e}", path.display()),
            )
        })?;
        let cred = serde_json::from_str(&raw).map_err(|e| {
            VcmError::new(
                codes::ETCD_AUTH_FAILED,
                format!("凭据文件 {} 解析失败: {e}", path.display()),
            )
        })?;
        Ok(Some(cred))
    }

    /// 是否存在
    pub fn exists(&self, identity: &str) -> bool {
        self.layout.identity_file(identity).exists()
    }

    /// 删除凭据（吊销后清理；返回是否实际删除）
    pub fn remove(&self, identity: &str) -> Result<bool, VcmError> {
        validate_identity(identity).map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
        let mut removed = false;
        for path in [
            self.layout.identity_file(identity),
            self.layout.identity_cert(identity),
            self.layout.identity_key(identity),
        ] {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| {
                    VcmError::new(
                        codes::IO_FAILED,
                        format!("删除 {} 失败: {e}", path.display()),
                    )
                })?;
                removed = true;
            }
        }
        Ok(removed)
    }

    /// 列出本机已落盘身份（按名排序；不含任何密钥内容）
    pub fn list(&self) -> Result<Vec<String>, VcmError> {
        let dir = self.layout.clients_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| {
            VcmError::new(codes::IO_FAILED, format!("读取 {} 失败: {e}", dir.display()))
        })? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && validate_identity(stem).is_ok()
            {
                out.push(stem.to_string());
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, CredentialStore) {
        let tmp = TempDir::new().unwrap();
        let layout = CredentialLayout::new(tmp.path().join("etcd"));
        (tmp, CredentialStore::new(layout))
    }

    fn sample(identity: &str, mode: &str) -> CredentialFile {
        CredentialFile {
            identity: identity.to_string(),
            username: identity.to_string(),
            password: Some("p@ss".to_string()),
            endpoints: vec!["10.100.10.21:2379".to_string()],
            auth_mode: mode.to_string(),
            ca_cert: "/etc/vos/etcd/pki/ca.crt".to_string(),
            client_cert: None,
            client_key: None,
            key_prefixes: vec!["/vos/cluster/".to_string()],
            generation: 1,
            issued_at: "2026-11-02T00:00:00+08:00".to_string(),
        }
    }

    #[test]
    fn write_read_roundtrip_and_normalize_endpoints() {
        let (_tmp, store) = store();
        let path = store.write(sample("vcm", "user")).unwrap();
        assert_eq!(
            path,
            store.layout().identity_file("vcm"),
            "凭据路径必须落在 <root>/clients/<identity>.json"
        );
        let cred = store.read("vcm").unwrap().unwrap();
        assert_eq!(cred.identity, "vcm");
        assert_eq!(cred.password.as_deref(), Some("p@ss"));
        assert_eq!(
            cred.endpoints,
            vec!["https://10.100.10.21:2379".to_string()],
            "user 模式端点必须补 https scheme"
        );
        assert!(store.exists("vcm"));
        assert_eq!(store.list().unwrap(), vec!["vcm".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn credential_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, store) = store();
        let path = store.write(sample("vos-ceph", "user")).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "凭据文件必须 0600");
    }

    #[test]
    fn remove_deletes_credential_and_material() {
        let (_tmp, store) = store();
        store.write(sample("agent-10-0-0-1", "mtls")).unwrap();
        std::fs::write(store.layout().identity_cert("agent-10-0-0-1"), "crt").unwrap();
        std::fs::write(store.layout().identity_key("agent-10-0-0-1"), "key").unwrap();
        assert!(store.remove("agent-10-0-0-1").unwrap());
        assert!(!store.exists("agent-10-0-0-1"));
        assert!(!store.layout().identity_key("agent-10-0-0-1").exists());
        assert!(!store.remove("agent-10-0-0-1").unwrap(), "重复删除幂等");
        assert!(store.read("agent-10-0-0-1").unwrap().is_none());
    }

    #[test]
    fn rejects_path_traversal_identity() {
        let (_tmp, store) = store();
        assert!(store.write(sample("../../etc/passwd", "user")).is_err());
        assert!(store.read("../x").is_err());
    }

    #[test]
    fn connect_spec_from_credential() {
        let (_tmp, store) = store();
        store.write(sample("vcm", "user")).unwrap();
        let spec = store.read("vcm").unwrap().unwrap().connect_spec().unwrap();
        assert_eq!(spec.mode, EtcdAuthMode::User);
        assert_eq!(spec.username.as_deref(), Some("vcm"));
        assert_eq!(spec.password.as_deref(), Some("p@ss"));
        assert!(spec.ca_cert.is_some());
    }

    #[test]
    fn list_ignores_non_credential_files() {
        let (_tmp, store) = store();
        store.write(sample("vcm", "user")).unwrap();
        std::fs::write(store.layout().clients_dir().join("notes.txt"), "x").unwrap();
        std::fs::write(store.layout().clients_dir().join(".hidden.json"), "{}").unwrap();
        assert_eq!(store.list().unwrap(), vec!["vcm".to_string()]);
    }
}