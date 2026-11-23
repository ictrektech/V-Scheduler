//! 应用启动配置（clap derive + env，优先级 CLI 参数 > 环境变量 > 默认值）

use std::path::PathBuf;

use clap::Parser;

/// VOS 离线镜像中转（image-hub）管理服务后端
#[derive(Debug, Clone, Parser)]
#[command(name = "image-hub", version, about)]
pub struct Config {
    /// HTTP 监听端口（VOS 应用部署 compose 显式传 19914；host 网络）
    #[arg(long, env = "VOS_IMAGE_HUB_PORT", default_value_t = 19914)]
    pub port: u16,

    /// 前端 dist 目录（构建产物，运行时托管）
    #[arg(long, env = "VOS_IMAGE_HUB_FRONTEND_DIR")]
    pub frontend_dir: PathBuf,

    /// 数据目录（本地管理状态根；镜像数据在 registry 容器卷，不在此列）
    #[arg(long, env = "VOS_IMAGE_HUB_DATA_DIR")]
    pub data_dir: PathBuf,

    /// （可选）URL 前缀，使整个组件可托管在任意子路径下（VOS iframe 场景必填）
    #[arg(long, env = "VOS_IMAGE_HUB_URL_BASE", default_value = "/")]
    pub url_base: String,

    /// 日志级别：error / warn / info / debug / trace
    #[arg(long, env = "VOS_IMAGE_HUB_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// registry 数据面基地址（host 网络下与本服务同机；管理面经此访问 v2 API）
    #[arg(
        long,
        env = "VOS_IMAGE_HUB_REGISTRY_URL",
        default_value = "http://127.0.0.1:19913"
    )]
    pub registry_url: String,

    /// registry 对宿主暴露的端口（前端据此拼 `<ip>:<port>` 展示地址与生成命令）
    #[arg(long, env = "VOS_IMAGE_HUB_REGISTRY_PORT", default_value_t = 19913)]
    pub registry_port: u16,

    /// skopeo 可执行文件路径（镜像内为 /usr/local/bin/skopeo，通常直接写 skopeo）
    #[arg(long, env = "VOS_IMAGE_HUB_SKOPEO_BIN", default_value = "skopeo")]
    pub skopeo_bin: String,

    /// 内置官方 registry 二进制路径（用于垃圾回收；镜像内为 /usr/local/bin/registry）
    #[arg(
        long,
        env = "VOS_IMAGE_HUB_REGISTRY_BIN",
        default_value = crate::gc::DEFAULT_REGISTRY_BIN
    )]
    pub registry_bin: String,

    /// registry 存储目录（管理容器内挂载点；GC 直接操作该目录）
    #[arg(
        long,
        env = "VOS_IMAGE_HUB_REGISTRY_STORAGE_DIR",
        default_value = crate::gc::DEFAULT_REGISTRY_STORAGE_DIR
    )]
    pub registry_storage_dir: std::path::PathBuf,
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

    /// registry 目标 host（含端口），用于拼 skopeo 目标 `docker://<host>/...`
    pub fn registry_host(&self) -> String {
        registry_host_from_url(&self.registry_url)
    }
}

/// 从 registry URL 提取 host（含端口）：去 scheme 与尾斜杠
pub fn registry_host_from_url(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_url_base_variants() {
        assert_eq!(url_base_of("/"), "");
        assert_eq!(url_base_of(""), "");
        assert_eq!(url_base_of("/app/com.ictrek.image-hub/"), "/app/com.ictrek.image-hub");
        assert_eq!(url_base_of("app/com.ictrek.image-hub"), "/app/com.ictrek.image-hub");
    }

    fn url_base_of(s: &str) -> String {
        Config {
            port: 19914,
            frontend_dir: PathBuf::from("/tmp"),
            data_dir: PathBuf::from("/tmp"),
            url_base: s.to_string(),
            log_level: "info".into(),
            registry_url: "http://127.0.0.1:19913".into(),
            registry_port: 19913,
            skopeo_bin: "skopeo".into(),
            registry_bin: crate::gc::DEFAULT_REGISTRY_BIN.into(),
            registry_storage_dir: PathBuf::from(crate::gc::DEFAULT_REGISTRY_STORAGE_DIR),
        }
        .normalized_url_base()
    }

    #[test]
    fn registry_host_extraction() {
        assert_eq!(registry_host_from_url("http://127.0.0.1:19913"), "127.0.0.1:19913");
        assert_eq!(registry_host_from_url("https://reg.example.com/"), "reg.example.com");
        assert_eq!(registry_host_from_url("  reg:5000  "), "reg:5000");
    }
}