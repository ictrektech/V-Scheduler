//! 模型广场 HTTP 客户端抽象（可 Mock，便于单测）

use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use crate::error::{ModelinferError, codes};
use crate::plaza::model_index::join_url;

/// 模型广场客户端抽象
#[async_trait]
pub trait PlazaClient: Send + Sync {
    /// 拉取 `<base_url>/index.yml` 文本
    async fn fetch_index(&self, base_url: &str) -> Result<String, ModelinferError>;
    /// 拉取任意 URL 的文本（如 manifest.yaml）
    async fn fetch_text(&self, url: &str) -> Result<String, ModelinferError>;
    /// 下载 URL 到本地文件（流式写入；原子改名由调用方保证）
    async fn download(&self, url: &str, local_path: &Path) -> Result<(), ModelinferError>;
}

/// 生产实现（reqwest + rustls）
pub struct RealPlazaClient {
    client: reqwest::Client,
}

impl RealPlazaClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(3600))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for RealPlazaClient {
    fn default() -> Self {
        Self::new()
    }
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String, ModelinferError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| ModelinferError::new(codes::PLAZA_FETCH_FAILED, format!("请求 {url} 失败: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ModelinferError::new(
            codes::PLAZA_FETCH_FAILED,
            format!("请求 {url} 返回 HTTP {status}"),
        ));
    }
    resp.text()
        .await
        .map_err(|e| ModelinferError::new(codes::PLAZA_FETCH_FAILED, format!("读取 {url} 响应失败: {e}")))
}

#[async_trait]
impl PlazaClient for RealPlazaClient {
    async fn fetch_index(&self, base_url: &str) -> Result<String, ModelinferError> {
        get_text(&self.client, &join_url(base_url, "index.yml")).await
    }

    async fn fetch_text(&self, url: &str) -> Result<String, ModelinferError> {
        get_text(&self.client, url).await
    }

    async fn download(&self, url: &str, local_path: &Path) -> Result<(), ModelinferError> {
        let mut resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ModelinferError::new(codes::PLAZA_DOWNLOAD_FAILED, format!("下载 {url} 失败: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ModelinferError::new(
                codes::PLAZA_DOWNLOAD_FAILED,
                format!("下载 {url} 返回 HTTP {status}"),
            ));
        }
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ModelinferError::new(codes::IO_FAILED, format!("创建目录失败: {e}"))
            })?;
        }
        let mut file = tokio::fs::File::create(local_path)
            .await
            .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建文件失败: {e}")))?;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| ModelinferError::new(codes::PLAZA_DOWNLOAD_FAILED, format!("读取响应流失败: {e}")))?
        {
            file.write_all(&chunk)
                .await
                .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("写文件失败: {e}")))?;
        }
        file.flush()
            .await
            .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("刷新文件失败: {e}")))?;
        Ok(())
    }
}

/// 测试替身：内存注入
#[derive(Default)]
pub struct MockPlazaClient {
    pub index_yaml: String,
    pub manifest_yaml: String,
    pub package_bytes: Vec<u8>,
    pub fail: bool,
    pub calls: Mutex<Vec<String>>,
}

impl MockPlazaClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_index(mut self, yaml: impl Into<String>) -> Self {
        self.index_yaml = yaml.into();
        self
    }

    pub fn with_manifest(mut self, yaml: impl Into<String>) -> Self {
        self.manifest_yaml = yaml.into();
        self
    }

    pub fn with_package(mut self, bytes: Vec<u8>) -> Self {
        self.package_bytes = bytes;
        self
    }

    pub fn with_failure(mut self) -> Self {
        self.fail = true;
        self
    }

    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn guard(&self, what: &str) -> Result<(), ModelinferError> {
        self.calls.lock().unwrap().push(what.to_string());
        if self.fail {
            return Err(ModelinferError::new(codes::PLAZA_FETCH_FAILED, "mock: plaza 不可用"));
        }
        Ok(())
    }
}

#[async_trait]
impl PlazaClient for MockPlazaClient {
    async fn fetch_index(&self, base_url: &str) -> Result<String, ModelinferError> {
        self.guard(&format!("fetch_index:{base_url}"))?;
        Ok(self.index_yaml.clone())
    }

    async fn fetch_text(&self, url: &str) -> Result<String, ModelinferError> {
        self.guard(&format!("fetch_text:{url}"))?;
        Ok(self.manifest_yaml.clone())
    }

    async fn download(&self, url: &str, local_path: &Path) -> Result<(), ModelinferError> {
        self.guard(&format!("download:{url}"))?;
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建目录失败: {e}")))?;
        }
        tokio::fs::write(local_path, &self.package_bytes)
            .await
            .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("写文件失败: {e}")))?;
        Ok(())
    }
}