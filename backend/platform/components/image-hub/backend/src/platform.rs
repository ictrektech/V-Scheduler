//! 平台 API 客户端：以应用上下文 token 调用 VOS 平台只读 API。
//!
//! - token：`${VOS_APP_CONTEXT_PATH}/access-token`（平台安装时写入的永久 admin JWT）
//! - 基地址：`https://${VOS_HOST_GW_IP}:${VOS_API_GW_PORT_EXTERNAL}`（缺省 `https://172.17.0.1:1180`）
//! - 仅消费既有只读端点 `GET /v1000/nas/device`（`ip_addr` 网卡地址列表），不改平台代码

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ImageHubError, codes};

/// 设备信息摘要
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub ip_addr: Vec<String>,
}

/// 平台只读 API 抽象
#[async_trait]
pub trait PlatformClient: Send + Sync {
    async fn device_info(&self) -> Result<DeviceInfo, ImageHubError>;
}

/// 读取应用上下文 token 路径
///
/// 平台注入的 `VOS_APP_CONTEXT_PATH` 是宿主机路径（供 compose 挂载），容器内固定映射到
/// `/var/lib/vos-context`。容器内 token 路径取 `IMAGE_HUB_CONTEXT_DIR`（默认
/// `/var/lib/vos-context`）/access-token；原生运行可显式设置指向实际 context 目录。
pub fn context_token_path(env: &HashMap<String, String>) -> Option<PathBuf> {
    let dir = env
        .get("IMAGE_HUB_CONTEXT_DIR")
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "/var/lib/vos-context".to_string());
    Some(Path::new(&dir).join("access-token"))
}

/// 从文件读取 token（去除首尾空白）
pub fn read_token(path: &Path) -> Result<String, ImageHubError> {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|e| {
            ImageHubError::new(
                codes::PLATFORM_CALL_FAILED,
                format!("读取应用上下文 token 失败 {}: {e}", path.display()),
            )
        })
}

/// 从环境变量推导平台 API 基地址
pub fn platform_base_from_env(env: &HashMap<String, String>) -> String {
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

/// 从 `GET /v1000/nas/device` 响应 data 中解析 `ip_addr`
pub fn device_from_value(data: &Value) -> DeviceInfo {
    let ip_addr = data
        .get("ip_addr")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    DeviceInfo { ip_addr }
}

/// 生产实现（reqwest + rustls，忽略自签证书）
pub struct RealPlatformClient {
    base_url: String,
    token_path: Option<PathBuf>,
    client: reqwest::Client,
}

impl RealPlatformClient {
    pub fn new(base_url: impl Into<String>, token_path: Option<PathBuf>) -> Self {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token_path,
            client,
        }
    }

    fn token(&self) -> Result<String, ImageHubError> {
        match &self.token_path {
            Some(p) => read_token(p),
            None => Err(ImageHubError::new(
                codes::PLATFORM_CALL_FAILED,
                "应用上下文 token 路径缺失（VOS_APP_CONTEXT_PATH 未注入）",
            )),
        }
    }
}

#[async_trait]
impl PlatformClient for RealPlatformClient {
    async fn device_info(&self) -> Result<DeviceInfo, ImageHubError> {
        let token = self.token()?;
        let url = format!("{}/v1000/nas/device", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| {
                ImageHubError::new(codes::PLATFORM_CALL_FAILED, format!("请求 {url} 失败: {e}"))
            })?;
        let body: Value = resp.json().await.map_err(|e| {
            ImageHubError::new(codes::PLATFORM_CALL_FAILED, format!("解析 {url} 响应失败: {e}"))
        })?;
        let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            let msg = body.get("msg").and_then(Value::as_str).unwrap_or("");
            return Err(ImageHubError::new(
                codes::PLATFORM_CALL_FAILED,
                format!("平台接口 {url} 返回 code={code}: {msg}"),
            ));
        }
        Ok(device_from_value(
            body.get("data").unwrap_or(&Value::Null),
        ))
    }
}

/// 测试替身
#[derive(Default)]
pub struct MockPlatformClient {
    pub device: DeviceInfo,
    pub fail: bool,
}

impl MockPlatformClient {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PlatformClient for MockPlatformClient {
    async fn device_info(&self) -> Result<DeviceInfo, ImageHubError> {
        if self.fail {
            return Err(ImageHubError::new(codes::PLATFORM_CALL_FAILED, "mock 平台调用失败"));
        }
        Ok(self.device.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_device_ip_addr() {
        let data = json!({"hostname": "h", "ip_addr": ["10.0.0.1", "172.17.0.1"]});
        assert_eq!(
            device_from_value(&data).ip_addr,
            vec!["10.0.0.1", "172.17.0.1"]
        );
        assert!(device_from_value(&json!({})).ip_addr.is_empty());
    }

    #[test]
    fn platform_base_from_env_defaults() {
        let env = HashMap::new();
        assert_eq!(platform_base_from_env(&env), "https://172.17.0.1:1180");
    }

    #[test]
    fn context_token_path_default() {
        let env = HashMap::new();
        let p = context_token_path(&env).unwrap();
        assert!(p.ends_with("var/lib/vos-context/access-token"));
    }
}