//! 管理面设置存储：上游 registry 认证信息 + 代理配置。
//!
//! - 持久化：`<data-dir>/settings.json`（权限 0600，临时文件 + rename 原子替换）
//! - 抽象 `SettingsStore`：生产 `FileSettingsStore`，单测 `MemorySettingsStore`
//! - 密码**只写不读**：对外视图（`RegistryView`）仅含 `has_password`
//!
//! 安全边界：密码以明文存于设备本地数据目录（应用存储卷，root 可读），与「内网受信、
//! HTTP 无认证」定位一致；后续安全强化提案可引入加密存储。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{ImageHubError, codes};

/// 上游 registry 认证信息（password 明文，仅内部使用）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamCredential {
    pub host: String,
    pub username: String,
    #[serde(default)]
    pub password: String,
}

/// 代理配置（启用时注入 skopeo 进程环境变量）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub http_proxy: String,
    #[serde(default)]
    pub https_proxy: String,
    #[serde(default)]
    pub no_proxy: String,
}

/// 全部管理面设置
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub registries: Vec<UpstreamCredential>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// registry 垃圾回收（GC）配置（D-IH-59 ~ D-IH-66；旧设置文件缺该段时为默认值）
    #[serde(default)]
    pub gc: GcSettings,
}

/// registry 垃圾回收（GC）配置（D-IH-62）
///
/// - `enabled` 默认 **false**（默认不自动触发）
/// - `interval_hours` 默认 24，范围 1~720（由 `gc::validate_gc_interval` 校验）
/// - `delete_untagged` 默认 true（回收改名 / 摘标签留下的未打标签 manifest）
/// - `last_run_at` 最近一次 GC 完成时间（手动或自动均更新），作为自动调度基准
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_gc_interval_hours")]
    pub interval_hours: u64,
    #[serde(default = "default_true")]
    pub delete_untagged: bool,
    #[serde(default)]
    pub last_run_at: Option<String>,
}

fn default_gc_interval_hours() -> u64 {
    crate::gc::DEFAULT_GC_INTERVAL_HOURS
}

fn default_true() -> bool {
    true
}

impl Default for GcSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_hours: crate::gc::DEFAULT_GC_INTERVAL_HOURS,
            delete_untagged: true,
            last_run_at: None,
        }
    }
}

/// 上游 registry 对外视图（绝不含 password）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryView {
    pub host: String,
    pub username: String,
    pub has_password: bool,
}

/// 设置读写抽象
pub trait SettingsStore: Send + Sync {
    fn load(&self) -> Result<Settings, ImageHubError>;
    fn save(&self, settings: &Settings) -> Result<(), ImageHubError>;
}

// ==================== 纯函数 ====================

/// 归一 host：去空白 / 去尾斜杠 / 小写（便于匹配）
pub fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('/').to_lowercase()
}

/// 在设置中按 host 查找凭据（大小写无关）
pub fn find_credential<'a>(settings: &'a Settings, host: &str) -> Option<&'a UpstreamCredential> {
    let want = normalize_host(host);
    settings
        .registries
        .iter()
        .find(|c| normalize_host(&c.host) == want)
}

/// 校验 host：非空，仅允许 host[:port] 字符集，禁止 `/`
pub fn validate_registry_host(host: &str) -> Result<String, ImageHubError> {
    let h = normalize_host(host);
    if h.is_empty()
        || h.contains('/')
        || !h
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return Err(ImageHubError::new(
            codes::PARAM_INVALID,
            format!("非法上游 registry 地址: {host}"),
        ));
    }
    Ok(h)
}

/// 校验代理 URL（允许空；须带受支持的 scheme）
pub fn validate_proxy_url(value: &str) -> Result<(), ImageHubError> {
    let v = value.trim();
    if v.is_empty() {
        return Ok(());
    }
    const SCHEMES: [&str; 4] = ["http://", "https://", "socks5://", "socks5h://"];
    if SCHEMES.iter().any(|s| v.starts_with(s)) {
        Ok(())
    } else {
        Err(ImageHubError::new(
            codes::PARAM_INVALID,
            format!("非法代理地址（需含 http://、https:// 或 socks5:// 前缀）: {value}"),
        ))
    }
}

/// 新增/更新上游凭据（host 相同则覆盖；password 为空时保留原密码）
pub fn upsert_registry(
    settings: &mut Settings,
    cred: UpstreamCredential,
) -> Result<(), ImageHubError> {
    let host = validate_registry_host(&cred.host)?;
    let username = cred.username.trim().to_string();
    if username.is_empty() {
        return Err(ImageHubError::new(
            codes::PARAM_INVALID,
            "上游 registry 用户名不能为空",
        ));
    }
    match settings
        .registries
        .iter_mut()
        .find(|c| normalize_host(&c.host) == host)
    {
        Some(existing) => {
            existing.host = host;
            existing.username = username;
            if !cred.password.is_empty() {
                existing.password = cred.password;
            }
        }
        None => {
            settings.registries.push(UpstreamCredential {
                host,
                username,
                password: cred.password,
            });
        }
    }
    Ok(())
}

/// 按 host 删除上游凭据，返回是否删除
pub fn remove_registry(settings: &mut Settings, host: &str) -> bool {
    let want = normalize_host(host);
    let before = settings.registries.len();
    settings
        .registries
        .retain(|c| normalize_host(&c.host) != want);
    settings.registries.len() != before
}

/// 对外视图（密码替换为 has_password 布尔）
pub fn masked_views(settings: &Settings) -> Vec<RegistryView> {
    settings
        .registries
        .iter()
        .map(|c| RegistryView {
            host: c.host.clone(),
            username: c.username.clone(),
            has_password: !c.password.is_empty(),
        })
        .collect()
}

// ==================== 生产实现 ====================

/// 文件设置存储（JSON，0600，原子写）
pub struct FileSettingsStore {
    path: PathBuf,
}

impl FileSettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SettingsStore for FileSettingsStore {
    fn load(&self) -> Result<Settings, ImageHubError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                ImageHubError::new(
                    codes::IO_FAILED,
                    format!("解析设置文件 {} 失败: {e}", self.path.display()),
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(ImageHubError::new(
                codes::IO_FAILED,
                format!("读取设置文件 {} 失败: {e}", self.path.display()),
            )),
        }
    }

    fn save(&self, settings: &Settings) -> Result<(), ImageHubError> {
        write_json_atomic(&self.path, settings)
    }
}

/// 原子写 JSON 文件（父目录自动创建；权限 0600）
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), ImageHubError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            ImageHubError::new(
                codes::IO_FAILED,
                format!("创建目录 {} 失败: {e}", parent.display()),
            )
        })?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| {
        ImageHubError::new(codes::IO_FAILED, format!("序列化设置失败: {e}"))
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| {
        ImageHubError::new(codes::IO_FAILED, format!("写入 {} 失败: {e}", tmp.display()))
    })?;
    restrict_permissions(&tmp);
    std::fs::rename(&tmp, path).map_err(|e| {
        ImageHubError::new(
            codes::IO_FAILED,
            format!("替换 {} 失败: {e}", path.display()),
        )
    })?;
    Ok(())
}

/// 收紧文件权限至 0600（失败不致命：非 Unix 或受限环境）
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

// ==================== 测试替身 ====================

/// 内存设置存储（单测）
#[derive(Default)]
pub struct MemorySettingsStore {
    inner: Mutex<Settings>,
}

impl MemorySettingsStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(settings: Settings) -> Self {
        Self {
            inner: Mutex::new(settings),
        }
    }
}

impl SettingsStore for MemorySettingsStore {
    fn load(&self) -> Result<Settings, ImageHubError> {
        Ok(self.inner.lock().unwrap().clone())
    }

    fn save(&self, settings: &Settings) -> Result<(), ImageHubError> {
        *self.inner.lock().unwrap() = settings.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(host: &str, user: &str, pass: &str) -> UpstreamCredential {
        UpstreamCredential {
            host: host.into(),
            username: user.into(),
            password: pass.into(),
        }
    }

    #[test]
    fn upsert_and_find_and_remove() {
        let mut s = Settings::default();
        upsert_registry(&mut s, cred("Docker.M.Daocloud.IO", "u1", "p1")).unwrap();
        assert_eq!(s.registries.len(), 1);
        assert_eq!(s.registries[0].host, "docker.m.daocloud.io");

        // 覆盖：password 为空保留原值
        upsert_registry(&mut s, cred("docker.m.daocloud.io", "u2", "")).unwrap();
        assert_eq!(s.registries.len(), 1);
        assert_eq!(s.registries[0].username, "u2");
        assert_eq!(s.registries[0].password, "p1");

        // 覆盖：password 非空替换
        upsert_registry(&mut s, cred("docker.m.daocloud.io", "u2", "p2")).unwrap();
        assert_eq!(s.registries[0].password, "p2");

        assert!(find_credential(&s, "DOCKER.m.daocloud.io").is_some());
        assert!(find_credential(&s, "docker.io").is_none());

        assert!(remove_registry(&mut s, "docker.m.daocloud.io"));
        assert!(!remove_registry(&mut s, "docker.m.daocloud.io"));
        assert!(s.registries.is_empty());
    }

    #[test]
    fn validate_and_reject_bad_input() {
        assert!(validate_registry_host("docker.io").is_ok());
        assert!(validate_registry_host("127.0.0.1:5000").is_ok());
        assert!(validate_registry_host("").is_err());
        assert!(validate_registry_host("http://docker.io").is_err());

        let mut s = Settings::default();
        assert!(upsert_registry(&mut s, cred("docker.io", "", "p")).is_err());
    }

    #[test]
    fn masked_views_never_leak_password() {
        let s = Settings {
            registries: vec![cred("docker.io", "u", "secret")],
            proxy: ProxyConfig::default(),
            gc: GcSettings::default(),
        };
        let views = masked_views(&s);
        assert_eq!(views.len(), 1);
        assert!(views[0].has_password);
        let json = serde_json::to_string(&views).unwrap();
        assert!(!json.contains("secret"), "{json}");
    }

    #[test]
    fn file_store_roundtrip_and_missing_default() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");
        let store = FileSettingsStore::new(&path);

        // 不存在 → 默认
        assert_eq!(store.load().unwrap(), Settings::default());

        let mut s = Settings::default();
        upsert_registry(&mut s, cred("docker.io", "u", "p")).unwrap();
        s.proxy = ProxyConfig {
            enabled: true,
            http_proxy: "http://p:3128".into(),
            https_proxy: "http://p:3128".into(),
            no_proxy: "127.0.0.1".into(),
        };
        store.save(&s).unwrap();
        assert!(path.exists());
        assert_eq!(store.load().unwrap(), s);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "设置文件应为 0600");
        }
    }

    #[test]
    fn memory_store_roundtrip() {
        let store = MemorySettingsStore::new();
        let mut s = Settings::default();
        upsert_registry(&mut s, cred("docker.io", "u", "p")).unwrap();
        store.save(&s).unwrap();
        assert_eq!(store.load().unwrap(), s);
    }

    #[test]
    fn gc_settings_defaults_and_legacy_load() {
        // 默认：关闭 / 24 小时 / 删除无标签 / 无 last_run_at（D-IH-62）
        let gc = GcSettings::default();
        assert!(!gc.enabled);
        assert_eq!(gc.interval_hours, 24);
        assert!(gc.delete_untagged);
        assert_eq!(gc.last_run_at, None);

        // 旧设置文件缺 gc 段 → 默认值（向后兼容）
        let legacy: Settings = serde_json::from_str(r#"{"registries":[],"proxy":{}}"#).unwrap();
        assert_eq!(legacy.gc, GcSettings::default());

        // gc 段部分字段缺省 → 补默认
        let partial: Settings =
            serde_json::from_str(r#"{"gc":{"enabled":true}}"#).unwrap();
        assert!(partial.gc.enabled);
        assert_eq!(partial.gc.interval_hours, 24);
        assert!(partial.gc.delete_untagged);
    }

    #[test]
    fn gc_settings_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");
        let store = FileSettingsStore::new(&path);
        let s = Settings {
            gc: GcSettings {
                enabled: true,
                interval_hours: 6,
                delete_untagged: false,
                last_run_at: Some("2026-09-24T00:00:00Z".into()),
            },
            ..Default::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load().unwrap(), s);
    }
}