//! 平台 API 客户端：以应用上下文 token 调用 VOS 平台只读 API。
//!
//! - token：`${VOS_APP_CONTEXT_PATH}/access-token`（平台安装时写入的永久 admin JWT）
//! - 基地址：`https://${VOS_HOST_GW_IP}:${VOS_API_GW_PORT_EXTERNAL}`（缺省 `https://172.17.0.1:1180`）
//! - 仅消费既有只读端点，不修改平台代码。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ModelinferError, codes};

/// 存储空间（NAS 卷）摘要
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub uuid: String,
    pub name: String,
    /// 宿主机挂载点（`mount.target`），NFS 导出根
    pub mount_point: String,
    pub state: String,
}

/// NFS 导出摘要
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NfsShareInfo {
    pub uuid: String,
    pub volume_uuid: String,
    pub name: String,
    /// NFS 导出路径
    pub mount_point: String,
    pub read_only: bool,
    pub state: String,
    pub mounted: bool,
}

/// 公共目录摘要
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicDirectoryInfo {
    pub uuid: String,
    pub volume_uuid: String,
    pub name: String,
    pub mount_point: Option<String>,
}

/// 设备信息摘要
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub ip_addr: Vec<String>,
}

/// 已安装应用摘要（`GET /v1000/app/list`）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub app_id: String,
    /// 生命周期状态（`installed` / `upgrading` / `not_installed` …）
    pub state: String,
}

/// 平台只读 API 抽象
#[async_trait]
pub trait PlatformClient: Send + Sync {
    /// `GET /v1000/app/detail` → `configs_json`
    async fn app_configs_json(&self, app_id: &str) -> Result<Value, ModelinferError>;
    /// `GET /v1000/app/list` → 已安装应用摘要
    async fn list_apps(&self) -> Result<Vec<AppInfo>, ModelinferError>;
    async fn list_volumes(&self) -> Result<Vec<VolumeInfo>, ModelinferError>;
    async fn list_nfs_shares(&self) -> Result<Vec<NfsShareInfo>, ModelinferError>;
    async fn list_public_directories(&self) -> Result<Vec<PublicDirectoryInfo>, ModelinferError>;
    async fn device_info(&self) -> Result<DeviceInfo, ModelinferError>;
}

/// 读取应用上下文 token 路径
///
/// 平台注入的 `VOS_APP_CONTEXT_PATH` 是**宿主机路径**（供 compose 挂载），
/// 容器内固定映射到 `/var/lib/vos-context`。因此容器内 token 路径取
/// `MODELINFER_CONTEXT_DIR`（默认 `/var/lib/vos-context`）/access-token；
/// 原生运行时可显式设置 `MODELINFER_CONTEXT_DIR` 指向实际 context 目录。
pub fn context_token_path(env: &std::collections::HashMap<String, String>) -> Option<PathBuf> {
    let dir = env
        .get("MODELINFER_CONTEXT_DIR")
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "/var/lib/vos-context".to_string());
    Some(Path::new(&dir).join("access-token"))
}

/// 从文件读取 token（去除首尾空白）
pub fn read_token(path: &Path) -> Result<String, ModelinferError> {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|e| {
            ModelinferError::new(
                codes::PLATFORM_CALL_FAILED,
                format!("读取应用上下文 token 失败 {}: {e}", path.display()),
            )
        })
}

/// 从环境变量推导平台 API 基地址
pub fn platform_base_from_env(env: &std::collections::HashMap<String, String>) -> String {
    let host = env
        .get("VOS_HOST_GW_IP")
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "172.17.0.1".to_string());
    let port = env
        .get("VOS_API_GW_PORT_EXTERNAL")
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "1180".to_string());
    format!("https://{host}:{port}")
}

/// 生产实现（reqwest + rustls，忽略自签证书）
pub struct RealPlatformClient {
    base_url: String,
    token_path: Option<PathBuf>,
    client: reqwest::Client,
    _app_id: String,
}

impl RealPlatformClient {
    pub fn new(base_url: impl Into<String>, token_path: Option<PathBuf>, app_id: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token_path,
            client,
            _app_id: app_id.into(),
        }
    }

    fn token(&self) -> Result<String, ModelinferError> {
        match &self.token_path {
            Some(p) => read_token(p),
            None => Err(ModelinferError::new(
                codes::PLATFORM_CALL_FAILED,
                "应用上下文 token 路径缺失（VOS_APP_CONTEXT_PATH 未注入）",
            )),
        }
    }

    async fn get_json(&self, path: &str) -> Result<Value, ModelinferError> {
        let token = self.token()?;
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| ModelinferError::new(codes::PLATFORM_CALL_FAILED, format!("请求 {url} 失败: {e}")))?;
        parse_result(resp, &url).await
    }

    async fn post_json(&self, path: &str, body: Value) -> Result<Value, ModelinferError> {
        let token = self.token()?;
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| ModelinferError::new(codes::PLATFORM_CALL_FAILED, format!("请求 {url} 失败: {e}")))?;
        parse_result(resp, &url).await
    }
}

/// 解析 `ResultBody`，校验 `code == 0` 并返回 `data`
async fn parse_result(resp: reqwest::Response, url: &str) -> Result<Value, ModelinferError> {
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| ModelinferError::new(codes::PLATFORM_CALL_FAILED, format!("解析 {url} 响应失败: {e}")))?;
    let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = body.get("msg").and_then(Value::as_str).unwrap_or("");
        return Err(ModelinferError::new(
            codes::PLATFORM_CALL_FAILED,
            format!("平台接口 {url} 返回 code={code} ({status}): {msg}"),
        ));
    }
    Ok(body.get("data").cloned().unwrap_or(Value::Null))
}

#[async_trait]
impl PlatformClient for RealPlatformClient {
    async fn app_configs_json(&self, app_id: &str) -> Result<Value, ModelinferError> {
        let data = self
            .get_json(&format!("/v1000/app/detail?app_id={app_id}"))
            .await?;
        Ok(data.get("configs_json").cloned().unwrap_or(Value::Null))
    }

    async fn list_apps(&self) -> Result<Vec<AppInfo>, ModelinferError> {
        let data = self.get_json("/v1000/app/list").await?;
        // 兼容 `data: [...]` 与 `data: { data: [...] }` 两种形态
        let items = data
            .as_array()
            .cloned()
            .or_else(|| data.get("data").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        Ok(items.iter().filter_map(app_info_from_value).collect())
    }

    async fn list_volumes(&self) -> Result<Vec<VolumeInfo>, ModelinferError> {
        let data = self
            .post_json("/v1000/nas/volumes/list", serde_json::json!({}))
            .await?;
        let items = data
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(items.iter().filter_map(volume_from_value).collect())
    }

    async fn list_nfs_shares(&self) -> Result<Vec<NfsShareInfo>, ModelinferError> {
        let data = self
            .post_json("/v1000/nas/nfs/shares/list", serde_json::json!({}))
            .await?;
        let items = data
            .get("shares")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(items.iter().filter_map(nfs_share_from_value).collect())
    }

    async fn list_public_directories(&self) -> Result<Vec<PublicDirectoryInfo>, ModelinferError> {
        let data = self
            .post_json(
                "/v1000/nas/public-directories/list",
                serde_json::json!({ "page_num": 1, "page_size": 200 }),
            )
            .await?;
        let items = data
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(items.iter().filter_map(public_dir_from_value).collect())
    }

    async fn device_info(&self) -> Result<DeviceInfo, ModelinferError> {
        let data = self.get_json("/v1000/nas/device").await?;
        let ip_addr = data
            .get("ip_addr")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        Ok(DeviceInfo { ip_addr })
    }
}

// ==== Value → DTO 解析（纯函数，便于单测）====

/// 解析已安装应用条目（`data[]`）
///
/// `GET /v1000/app/list` 返回 `data: [AppSummaryDto]`；`data` 可能被包装为 `{data: [...]}`，
/// 两种形态都兼容。
pub fn app_info_from_value(v: &Value) -> Option<AppInfo> {
    let app_id = v.get("app_id").and_then(Value::as_str)?.to_string();
    let state = v
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    Some(AppInfo { app_id, state })
}

pub fn volume_from_value(v: &Value) -> Option<VolumeInfo> {
    let uuid = v.get("uuid").and_then(Value::as_str)?.to_string();
    let name = v
        .get("nick_name")
        .and_then(Value::as_str)
        .or_else(|| v.get("name").and_then(Value::as_str))
        .unwrap_or(&uuid)
        .to_string();
    // mount.target 优先，其次 mount_point，再次 local_path
    let mount_point = v
        .get("mount")
        .and_then(|m| m.get("target"))
        .and_then(Value::as_str)
        .or_else(|| v.get("mount_point").and_then(Value::as_str))
        .or_else(|| v.get("local_path").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let state = v
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    Some(VolumeInfo { uuid, name, mount_point, state })
}

pub fn nfs_share_from_value(v: &Value) -> Option<NfsShareInfo> {
    Some(NfsShareInfo {
        uuid: v.get("uuid").and_then(Value::as_str)?.to_string(),
        volume_uuid: v.get("volume_uuid").and_then(Value::as_str)?.to_string(),
        name: v.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
        mount_point: v.get("mount_point").and_then(Value::as_str).unwrap_or("").to_string(),
        read_only: v.get("read_only").and_then(Value::as_bool).unwrap_or(false),
        state: v.get("state").and_then(Value::as_str).unwrap_or("unknown").to_string(),
        mounted: v.get("mounted").and_then(Value::as_bool).unwrap_or(false),
    })
}

pub fn public_dir_from_value(v: &Value) -> Option<PublicDirectoryInfo> {
    Some(PublicDirectoryInfo {
        uuid: v.get("uuid").and_then(Value::as_str)?.to_string(),
        volume_uuid: v.get("volume_uuid").and_then(Value::as_str).unwrap_or("").to_string(),
        name: v.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
        mount_point: v.get("mount_point").and_then(Value::as_str).map(str::to_string),
    })
}

/// 测试替身
#[derive(Default)]
pub struct MockPlatformClient {
    pub app_configs: Value,
    /// 按 app_id 区分的应用配置（优先于 `app_configs`）
    pub app_configs_by_id: std::collections::HashMap<String, Value>,
    pub apps: Vec<AppInfo>,
    pub volumes: Vec<VolumeInfo>,
    pub nfs_shares: Vec<NfsShareInfo>,
    pub public_dirs: Vec<PublicDirectoryInfo>,
    pub device: DeviceInfo,
    pub fail: bool,
    pub calls: Mutex<Vec<String>>,
}

impl MockPlatformClient {
    pub fn new() -> Self {
        Self {
            app_configs: Value::Null,
            app_configs_by_id: std::collections::HashMap::new(),
            apps: Vec::new(),
            volumes: Vec::new(),
            nfs_shares: Vec::new(),
            public_dirs: Vec::new(),
            device: DeviceInfo { ip_addr: Vec::new() },
            fail: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn guard(&self, name: &str) -> Result<(), ModelinferError> {
        self.calls.lock().unwrap().push(name.to_string());
        if self.fail {
            return Err(ModelinferError::new(codes::PLATFORM_CALL_FAILED, "mock 平台调用失败"));
        }
        Ok(())
    }
}

#[async_trait]
impl PlatformClient for MockPlatformClient {
    async fn app_configs_json(&self, app_id: &str) -> Result<Value, ModelinferError> {
        self.guard("app_configs_json")?;
        Ok(self
            .app_configs_by_id
            .get(app_id)
            .cloned()
            .unwrap_or_else(|| self.app_configs.clone()))
    }
    async fn list_apps(&self) -> Result<Vec<AppInfo>, ModelinferError> {
        self.guard("list_apps")?;
        Ok(self.apps.clone())
    }
    async fn list_volumes(&self) -> Result<Vec<VolumeInfo>, ModelinferError> {
        self.guard("list_volumes")?;
        Ok(self.volumes.clone())
    }
    async fn list_nfs_shares(&self) -> Result<Vec<NfsShareInfo>, ModelinferError> {
        self.guard("list_nfs_shares")?;
        Ok(self.nfs_shares.clone())
    }
    async fn list_public_directories(&self) -> Result<Vec<PublicDirectoryInfo>, ModelinferError> {
        self.guard("list_public_directories")?;
        Ok(self.public_dirs.clone())
    }
    async fn device_info(&self) -> Result<DeviceInfo, ModelinferError> {
        self.guard("device_info")?;
        Ok(self.device.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_volume_prefers_mount_target() {
        let v = serde_json::json!({
            "uuid": "vol1",
            "nick_name": "demo",
            "state": "Running",
            "mount": { "target": "/share/vol1" }
        });
        let parsed = volume_from_value(&v).unwrap();
        assert_eq!(parsed.mount_point, "/share/vol1");
        assert_eq!(parsed.name, "demo");
    }

    #[test]
    fn parse_nfs_share() {
        let v = serde_json::json!({
            "uuid": "s1", "volume_uuid": "vol1", "name": "demo",
            "mount_point": "/share/vol1", "read_only": false,
            "state": "Running", "mounted": true
        });
        let parsed = nfs_share_from_value(&v).unwrap();
        assert!(parsed.mounted);
        assert_eq!(parsed.mount_point, "/share/vol1");
    }

    #[test]
    fn platform_base_from_env_defaults() {
        let env = std::collections::HashMap::new();
        assert_eq!(platform_base_from_env(&env), "https://172.17.0.1:1180");
    }
}
