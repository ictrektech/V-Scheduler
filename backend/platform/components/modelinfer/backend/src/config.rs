//! 应用启动配置（clap derive + env，优先级 CLI 参数 > 环境变量 > 默认值）

use std::{net::IpAddr, path::PathBuf};

use clap::Parser;

use crate::error::{ModelinferError, codes};

/// modelinfer 服务后端（模型下载管理与集群调度）
#[derive(Debug, Clone, Parser)]
#[command(name = "modelinfer", version, about)]
pub struct Config {
    /// HTTP 监听端口（VOS 应用 compose 显式传 19908；host 网络）
    #[arg(long, env = "VOS_MODELINFER_PORT", default_value_t = 19908)]
    pub port: u16,

    /// HTTP 监听地址。嵌入 V-Scheduler 时仅监听回环接口。
    #[arg(long, env = "VOS_MODELINFER_BIND_ADDRESS", default_value = "0.0.0.0")]
    pub bind_address: IpAddr,

    /// 提供应用上下文凭据及模型仓库配置的 VOS App。
    #[arg(long, env = "MODELINFER_PLATFORM_APP_ID", default_value = crate::nfs::APP_ID)]
    pub platform_app_id: String,

    /// 前端 dist 目录（构建产物，运行时托管）
    #[arg(long, env = "VOS_MODELINFER_FRONTEND_DIR")]
    pub frontend_dir: PathBuf,

    /// 数据目录（模型广场本地卡片目录根；数据库不落于此，见 --database-url）
    #[arg(long, env = "VOS_MODELINFER_DATA_DIR")]
    pub data_dir: PathBuf,

    /// 数据库连接串；MUST 为 postgres（缺省由 `VOS_POSTGRES_*` 推导，无本地 sqlite 回退）
    #[arg(long, env = "VOS_MODELINFER_DATABASE_URL", default_value = "")]
    pub database_url: String,

    /// Temporal 服务地址；缺省由 `VOS_HOST_GW_IP:VOS_TEMPORAL_PORT` 推导
    #[arg(long, env = "VOS_MODELINFER_TEMPORAL_ADDRESS", default_value = "")]
    pub temporal_address: String,

    /// Temporal namespace
    #[arg(long, env = "VOS_MODELINFER_TEMPORAL_NAMESPACE", default_value = "default")]
    pub temporal_namespace: String,

    /// Temporal task queue
    #[arg(long, env = "VOS_MODELINFER_TASK_QUEUE", default_value = "modelinfer-tasks")]
    pub task_queue: String,

    /// 是否启用 Temporal 编排（本地开发可关闭）
    #[arg(long, env = "VOS_MODELINFER_TEMPORAL_ENABLED", default_value_t = true)]
    pub temporal_enabled: bool,

    /// 服务启动时要求 Temporal Worker 可用。
    #[arg(long, default_value_t = false)]
    pub require_temporal_worker: bool,

    /// 健康巡检周期（秒）
    #[arg(long, env = "VOS_MODELINFER_HEALTH_INTERVAL_SECS", default_value_t = 30)]
    pub health_interval_secs: u64,

    /// 自动重启 backoff 基准（秒；第 n 次失败等待 min(base*2^(n-1), max)）
    #[arg(long, env = "MODELINFER_AUTO_RESTART_BACKOFF_SECS", default_value_t = 15)]
    pub auto_restart_backoff_secs: u64,

    /// 自动重启 backoff 上限（秒）
    #[arg(long, env = "MODELINFER_AUTO_RESTART_BACKOFF_MAX_SECS", default_value_t = 300)]
    pub auto_restart_backoff_max_secs: u64,

    /// 健康巡检运行记录保留次数（默认 10；0 / 非法 → 默认）
    #[arg(long, env = "MODELINFER_HEALTH_HISTORY_KEEP", default_value_t = 10)]
    pub health_history_keep: u32,

    /// 启动 / 停止运行记录保留次数（默认 20；0 / 非法 → 默认）
    #[arg(long, env = "MODELINFER_RUN_HISTORY_KEEP", default_value_t = 20)]
    pub run_history_keep: u32,

    /// （可选）URL 前缀，使整个组件可托管在任意子路径下（VOS iframe 场景必填）
    #[arg(long, env = "VOS_MODELINFER_URL_BASE", default_value = "/")]
    pub url_base: String,

    /// 日志级别：error / warn / info / debug / trace
    #[arg(long, env = "VOS_MODELINFER_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// model hub 共享目录（平台 path 配置解析后的宿主机绝对路径；
    /// 用于在平台 API 不可用时兜底，见 NFS 解析）
    #[arg(long, env = "MODELINFER_MODEL_HUB_PATH", default_value = "")]
    pub model_hub_path: String,

    /// 节点默认 NFS 挂载点
    #[arg(
        long,
        env = "MODELINFER_DEFAULT_MOUNT_DIR",
        default_value = "/mnt/model_hub"
    )]
    pub default_mount_dir: String,

    /// NFS 服务端 IP 覆盖（空 = 自动从平台 device info 推断）
    #[arg(long, env = "MODELINFER_NFS_SERVER", default_value = "")]
    pub nfs_server: String,

    /// image-hub registry 地址 host 覆盖（空 = 按节点自动解析：
    /// 本机路由源地址同网段 → 平台设备同网段 → 节点默认网关 → 首个可用本机 IP）
    #[arg(long, env = "MODELINFER_IMAGE_HUB_SERVER", default_value = "")]
    pub image_hub_server: String,

    /// 可选的管理进程 registry 探测地址；不会传给执行节点。
    #[arg(long, env = "MODELINFER_IMAGE_HUB_PROBE_HOST", default_value = "")]
    pub image_hub_probe_host: String,

    /// image-hub registry 端口
    #[arg(long, env = "MODELINFER_IMAGE_HUB_PORT", default_value_t = crate::images::DEFAULT_IMAGE_HUB_PORT)]
    pub image_hub_port: u16,

    /// 镜像分发节点级并行度（0 = 不限制 / 全部节点并行；用于多节点大镜像首次启动提速）
    #[arg(
        long,
        env = "MODELINFER_IMAGE_HUB_CONCURRENCY",
        default_value_t = crate::images::DEFAULT_IMAGE_DISTRIBUTION_CONCURRENCY
    )]
    pub image_hub_concurrency: usize,

    /// 平台 API 基地址覆盖（空 = 由 VOS_HOST_GW_IP / VOS_API_GW_PORT_EXTERNAL 推导）
    #[arg(long, env = "MODELINFER_PLATFORM_API", default_value = "")]
    pub platform_api: String,

    /// 模型广场服务端基地址（第一阶段仅展示，后续用于下载）
    #[arg(long, env = "MODELINFER_PLAZA_SERVER", default_value = "")]
    pub plaza_server: String,

    /// V-Scheduler 模型仓库共享目录在容器内的挂载点（MODELINFER_MODEL_HUB_PATH 挂到此处）
    #[arg(long, env = "MODELINFER_MODEL_HUB_DIR", default_value = "/modelhub")]
    pub model_hub_dir: String,

    /// 统一网关（V-Scheduler / 旧 V-Router）注册租约 TTL（秒，30..=3600；内部参数，非应用配置项）
    #[arg(long, env = "MODELINFER_VROUTER_TTL_SECS", default_value_t = 90)]
    pub vrouter_ttl_secs: u64,

    /// consul 地址（节点监控服务注册；缺省由 `VOS_HOST_GW_IP:8500` 推导）
    #[arg(long, env = "MODELINFER_CONSUL_URL", default_value = "")]
    pub consul_url: String,

    /// prometheus 地址（节点指标查询；缺省由 `VOS_HOST_GW_IP:9090` 推导）
    #[arg(long, env = "MODELINFER_PROMETHEUS_URL", default_value = "")]
    pub prometheus_url: String,

    /// node-exporter 镜像（节点监控组件；缺失时经 image-hub 分发，绝不直接 docker pull）
    #[arg(
        long,
        env = "MODELINFER_NODE_EXPORTER_IMAGE",
        default_value = crate::monitor::DEFAULT_NODE_EXPORTER_IMAGE
    )]
    pub node_exporter_image: String,

    /// nvidia-gpu-exporter 镜像（NVIDIA 节点 GPU 监控组件；缺失时经 image-hub 分发）
    #[arg(
        long,
        env = "MODELINFER_NVIDIA_GPU_EXPORTER_IMAGE",
        default_value = crate::monitor::DEFAULT_NVIDIA_GPU_EXPORTER_IMAGE
    )]
    pub nvidia_gpu_exporter_image: String,

    /// 节点监控注册 reconcile 周期（秒）
    #[arg(long, env = "MODELINFER_MONITOR_RECONCILE_SECS", default_value_t = crate::monitor::DEFAULT_RECONCILE_SECS)]
    pub monitor_reconcile_secs: u64,
}

impl Config {
    /// 归一化 url_base：保证以 `/` 开头、结尾无 `/`；空 / "/" / 纯斜杠归一为空字符串（根挂载）
    pub fn normalized_url_base(&self) -> String {
        normalize_url_base(&self.url_base)
    }

    /// 解析数据库连接串（**仅支持 postgres，无本地 sqlite 回退**）：
    ///
    /// 1. 显式 `--database-url` / `VOS_MODELINFER_DATABASE_URL`（MUST 为 postgres）；
    /// 2. 平台 postgres（由 `VOS_POSTGRES_*` 推导）；
    /// 3. 均不可用 → 20726（错误信息说明需注入的环境变量）。
    pub fn resolved_database_url(&self) -> Result<String, ModelinferError> {
        let explicit = self.database_url.trim();
        if !explicit.is_empty() {
            if !is_postgres_url(explicit) {
                return Err(ModelinferError::new(
                    codes::DATABASE_CONFIG_INVALID,
                    format!(
                        "数据库连接串必须是 postgres（当前: {}）：本应用不再回退本地 SQLite",
                        mask_database_url(explicit)
                    ),
                ));
            }
            return Ok(explicit.to_string());
        }
        postgres_url_from_env(std::env::vars().collect()).ok_or_else(|| {
            ModelinferError::new(
                codes::DATABASE_CONFIG_INVALID,
                "未配置数据库连接：请注入平台 postgres 环境变量（VOS_POSTGRES_USER / \
                 VOS_POSTGRES_PASSWORD / VOS_POSTGRES_DB / VOS_POSTGRES_PORT / VOS_HOST_GW_IP），\
                 或显式传入 --database-url postgres://...（本应用不再回退本地 SQLite）",
            )
        })
    }

    /// 解析 Temporal 地址（`http://host:port`）
    pub fn resolved_temporal_address(&self) -> String {
        let explicit = self.temporal_address.trim();
        if !explicit.is_empty() {
            return explicit.to_string();
        }
        temporal_address_from_env(std::env::vars().collect())
    }

    /// 模型广场本地存储根目录
    pub fn plaza_dir(&self) -> PathBuf {
        self.data_dir.join("plaza")
    }

    /// image-hub registry 配置（启动前缺失镜像分发）
    pub fn image_hub(&self) -> crate::images::ImageHubConfig {
        let mut hub = crate::images::ImageHubConfig::new(self.image_hub_server.clone(), self.image_hub_port)
            .with_concurrency(self.image_hub_concurrency);
        hub.probe_host = self.image_hub_probe_host.clone();
        hub
    }

    /// 统一网关集成配置（自动发现地址 + 应用上下文 token）
    pub fn vrouter_config(&self) -> crate::vrouter::VrouterConfig {
        let env: std::collections::HashMap<String, String> = std::env::vars().collect();
        let token_path = crate::platform::context_token_path(&env);
        let host_gateway_ip = env
            .get("VOS_HOST_GW_IP")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| crate::vrouter::DEFAULT_HOST_GATEWAY_IP.to_string());
        crate::vrouter::VrouterConfig {
            ttl_secs: self.vrouter_ttl_secs,
            token_path,
            host_gateway_ip,
        }
    }

    /// 节点监控集成配置（consul / prometheus / 镜像 / reconcile 周期）
    ///
    /// 未显式配置时以 `VOS_HOST_GW_IP` 推导（consul 8500 / prometheus 9090）。
    pub fn monitor(&self) -> crate::monitor::MonitorConfig {
        let env: std::collections::HashMap<String, String> = std::env::vars().collect();
        let gw = env
            .get("VOS_HOST_GW_IP")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "172.17.0.1".to_string());
        let consul_url = if self.consul_url.trim().is_empty() {
            format!("http://{gw}:8500")
        } else {
            self.consul_url.trim().to_string()
        };
        let prometheus_url = if self.prometheus_url.trim().is_empty() {
            format!("http://{gw}:9090")
        } else {
            self.prometheus_url.trim().to_string()
        };
        crate::monitor::MonitorConfig {
            consul_url,
            prometheus_url,
            node_exporter_image: self.node_exporter_image.trim().to_string(),
            nvidia_gpu_exporter_image: self.nvidia_gpu_exporter_image.trim().to_string(),
            reconcile_secs: self.monitor_reconcile_secs.max(10),
        }
    }

    /// 运行可观测性保留策略默认值
    pub fn observability_defaults(&self) -> crate::observability::SettingsDefaults {
        crate::observability::SettingsDefaults {
            health_history_keep: crate::observability::store::normalize_keep(
                self.health_history_keep,
                10,
                crate::observability::store::MAX_HEALTH_KEEP,
            ),
            run_history_keep: crate::observability::store::normalize_keep(
                self.run_history_keep,
                20,
                crate::observability::store::MAX_RUN_KEEP,
            ),
        }
    }
}

/// 纯函数：从 `VOS_POSTGRES_*` 环境变量推导 postgres 连接串（缺关键字段返回 None）
pub fn postgres_url_from_env(env: std::collections::HashMap<String, String>) -> Option<String> {
    let user = env.get("VOS_POSTGRES_USER").filter(|s| !s.trim().is_empty())?;
    let pwd = env
        .get("VOS_POSTGRES_PASSWORD")
        .filter(|s| !s.trim().is_empty())?;
    let db = env
        .get("VOS_POSTGRES_DB")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("postgres");
    let host = env
        .get("VOS_HOST_GW_IP")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("172.17.0.1");
    let port = env
        .get("VOS_POSTGRES_PORT")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("5432");
    Some(format!("postgres://{user}:{pwd}@{host}:{port}/{db}"))
}

/// 纯函数：是否为 postgres 连接串（`postgres://` / `postgresql://`）
pub fn is_postgres_url(url: &str) -> bool {
    let u = url.trim();
    u.starts_with("postgres://") || u.starts_with("postgresql://")
}

/// 纯函数：连接串脱敏（密码替换为 `***`），用于日志输出
///
/// - `postgres://u:s3cr3t@h:5432/db` → `postgres://u:***@h:5432/db`
/// - 无密码段（`postgres://u@h:5432/db`）→ 原样返回
/// - 无法解析（无 `://`）→ `***`（不泄漏原文）
pub fn mask_database_url(raw: &str) -> String {
    let Some(pos) = raw.find("://") else {
        return "***".to_string();
    };
    let (scheme, rest) = raw.split_at(pos + 3);
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let masked = match authority.rsplit_once('@') {
        Some((userinfo, host)) => {
            if userinfo.contains(':') {
                let user = userinfo.split(':').next().unwrap_or("");
                format!("{user}:***@{host}")
            } else {
                format!("{userinfo}@{host}")
            }
        }
        None => authority.to_string(),
    };
    format!("{scheme}{masked}{tail}")
}

/// 纯函数：从平台 env 推导 Temporal 地址
pub fn temporal_address_from_env(env: std::collections::HashMap<String, String>) -> String {
    let host = env
        .get("VOS_HOST_GW_IP")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("172.17.0.1");
    let port = env
        .get("VOS_TEMPORAL_PORT")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("7233");
    format!("http://{host}:{port}")
}

/// 纯函数：url_base 归一化
pub fn normalize_url_base(raw: &str) -> String {
    let s = raw.trim().trim_matches('/');
    if s.is_empty() {
        return String::new();
    }
    format!("/{s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_url_base_variants() {
        assert_eq!(normalize_url_base("/"), "");
        assert_eq!(normalize_url_base(""), "");
        assert_eq!(normalize_url_base("/app/com.ictrek.modelinfer/"), "/app/com.ictrek.modelinfer");
        assert_eq!(normalize_url_base("app/com.ictrek.modelinfer"), "/app/com.ictrek.modelinfer");
    }

    #[test]
    fn resolved_database_url_requires_postgres() {
        use clap::Parser as _;
        // 显式 postgres → 原样返回
        let cfg = Config::parse_from([
            "modelinfer",
            "--frontend-dir",
            "/tmp",
            "--data-dir",
            "/var/lib/modelinfer",
            "--database-url",
            "postgres://u:p@h:5432/db",
        ]);
        assert_eq!(
            cfg.resolved_database_url().unwrap(),
            "postgres://u:p@h:5432/db"
        );

        // 显式 sqlite → 20726
        let cfg = Config::parse_from([
            "modelinfer",
            "--frontend-dir",
            "/tmp",
            "--data-dir",
            "/var/lib/modelinfer",
            "--database-url",
            "sqlite:///tmp/modelinfer.db",
        ]);
        let err = cfg.resolved_database_url().unwrap_err();
        assert_eq!(err.code, codes::DATABASE_CONFIG_INVALID);
        assert!(err.msg.contains("不再回退本地 SQLite"));

        // 无显式连接串 → 由 VOS_POSTGRES_* 推导（测试环境已设置时）或 20726
        let cfg = Config::parse_from(["modelinfer", "--frontend-dir", "/tmp", "--data-dir", "/tmp"]);
        match cfg.resolved_database_url() {
            Ok(url) => assert!(is_postgres_url(&url), "推导结果必须是 postgres: {url}"),
            Err(e) => {
                assert_eq!(e.code, codes::DATABASE_CONFIG_INVALID);
                assert!(e.msg.contains("VOS_POSTGRES_USER"));
                assert!(e.msg.contains("不再回退本地 SQLite"));
            }
        }
    }

    #[test]
    fn db_path_arg_removed() {
        use clap::Parser as _;
        let res = Config::try_parse_from([
            "modelinfer",
            "--frontend-dir",
            "/tmp",
            "--data-dir",
            "/tmp",
            "--db-path",
            "/tmp/x.db",
        ]);
        assert!(res.is_err(), "--db-path 应已移除");
    }

    #[test]
    fn plaza_dir_under_data_dir() {
        use clap::Parser as _;
        let cfg = Config::parse_from([
            "modelinfer",
            "--frontend-dir",
            "/tmp",
            "--data-dir",
            "/var/lib/modelinfer",
        ]);
        assert_eq!(cfg.plaza_dir(), PathBuf::from("/var/lib/modelinfer/plaza"));
    }

    #[test]
    fn mask_database_url_variants() {
        assert_eq!(
            mask_database_url("postgres://u:s3cr3t@h:5432/db"),
            "postgres://u:***@h:5432/db"
        );
        // 无密码段原样返回
        assert_eq!(
            mask_database_url("postgres://u@h:5432/db"),
            "postgres://u@h:5432/db"
        );
        // 密码含特殊字符（含 @ 与 :）仍只保留用户名
        assert_eq!(
            mask_database_url("postgres://u:p@ss:w@h/db"),
            "postgres://u:***@h/db"
        );
        // 无法解析 → 不泄漏原文
        assert_eq!(mask_database_url("not-a-url"), "***");
    }

    #[test]
    fn is_postgres_url_variants() {
        assert!(is_postgres_url("postgres://u:p@h/db"));
        assert!(is_postgres_url("postgresql://u:p@h/db"));
        assert!(!is_postgres_url("sqlite:///tmp/x.db"));
        assert!(!is_postgres_url("mysql://u:p@h/db"));
    }

    #[test]
    fn postgres_url_from_env_variants() {
        let mut env = std::collections::HashMap::new();
        assert!(postgres_url_from_env(env.clone()).is_none());
        env.insert("VOS_POSTGRES_USER".into(), "postgres".into());
        env.insert("VOS_POSTGRES_PASSWORD".into(), "Pp123456".into());
        env.insert("VOS_HOST_GW_IP".into(), "172.17.0.1".into());
        env.insert("VOS_POSTGRES_PORT".into(), "5432".into());
        env.insert("VOS_POSTGRES_DB".into(), "postgres".into());
        assert_eq!(
            postgres_url_from_env(env).unwrap(),
            "postgres://postgres:Pp123456@172.17.0.1:5432/postgres"
        );
    }

    #[test]
    fn temporal_address_from_env_default() {
        let env = std::collections::HashMap::new();
        assert_eq!(temporal_address_from_env(env), "http://172.17.0.1:7233");
    }

    #[test]
    fn observability_defaults_normalized() {
        use clap::Parser as _;
        let cfg = Config::parse_from(["modelinfer", "--frontend-dir", "/tmp", "--data-dir", "/tmp"]);
        let d = cfg.observability_defaults();
        assert_eq!(d.health_history_keep, 10);
        assert_eq!(d.run_history_keep, 20);
        let cfg = Config::parse_from([
            "modelinfer",
            "--frontend-dir",
            "/tmp",
            "--data-dir",
            "/tmp",
            "--health-history-keep",
            "0",
            "--run-history-keep",
            "9999",
        ]);
        let d = cfg.observability_defaults();
        assert_eq!(d.health_history_keep, 10, "0 → 默认");
        assert_eq!(d.run_history_keep, 200, "超上限 → 上限");
    }

    #[test]
    fn image_hub_config_defaults_and_override() {
        use clap::Parser as _;
        let cfg = Config::parse_from(["modelinfer", "--frontend-dir", "/tmp", "--data-dir", "/tmp"]);
        let hub = cfg.image_hub();
        assert_eq!(hub.server, "");
        assert_eq!(hub.port, 19913);
        assert_eq!(hub.concurrency, 4, "默认并行度");
        assert_eq!(hub.addr("10.10.11.90"), "10.10.11.90:19913");
        assert_eq!(hub.registry_base(), "http://127.0.0.1:19913");

        let cfg = Config::parse_from([
            "modelinfer", "--frontend-dir", "/tmp", "--data-dir", "/tmp",
            "--image-hub-probe-host", "172.17.0.1",
        ]);
        let hub = cfg.image_hub();
        assert_eq!(hub.registry_base(), "http://172.17.0.1:19913");
        assert!(hub.server.is_empty(), "SSH 节点继续按各自网络选择地址");

        let cfg = Config::parse_from([
            "modelinfer",
            "--frontend-dir",
            "/tmp",
            "--data-dir",
            "/tmp",
            "--image-hub-server",
            "hub.local",
            "--image-hub-port",
            "5000",
            "--image-hub-concurrency",
            "8",
        ]);
        let hub = cfg.image_hub();
        assert_eq!(hub.server, "hub.local");
        assert_eq!(hub.port, 5000);
        assert_eq!(hub.concurrency, 8);
        assert_eq!(hub.registry_base(), "http://hub.local:5000");

        // 0 = 不限制
        let cfg = Config::parse_from([
            "modelinfer",
            "--frontend-dir",
            "/tmp",
            "--data-dir",
            "/tmp",
            "--image-hub-concurrency",
            "0",
        ]);
        assert_eq!(cfg.image_hub().concurrency, 0);
    }

    #[test]
    fn monitor_config_defaults_and_override() {
        use clap::Parser as _;
        let cfg = Config::parse_from(["modelinfer", "--frontend-dir", "/tmp", "--data-dir", "/tmp"]);
        let m = cfg.monitor();
        assert!(m.consul_url.starts_with("http://"));
        assert!(m.consul_url.ends_with(":8500"));
        assert!(m.prometheus_url.ends_with(":9090"));
        assert_eq!(m.node_exporter_image, "docker.io/prom/node-exporter:v1.12.0");
        assert_eq!(m.nvidia_gpu_exporter_image, "docker.io/utkuozdemir/nvidia_gpu_exporter:1.15.1");
        assert_eq!(m.reconcile_secs, 60);

        let cfg = Config::parse_from([
            "modelinfer",
            "--frontend-dir",
            "/tmp",
            "--data-dir",
            "/tmp",
            "--consul-url",
            "http://consul.local:8501",
            "--prometheus-url",
            "http://prom.local:9091",
            "--node-exporter-image",
            "reg.example/ne:1",
            "--nvidia-gpu-exporter-image",
            "reg.example/ngx:2",
            "--monitor-reconcile-secs",
            "120",
        ]);
        let m = cfg.monitor();
        assert_eq!(m.consul_url, "http://consul.local:8501");
        assert_eq!(m.prometheus_url, "http://prom.local:9091");
        assert_eq!(m.node_exporter_image, "reg.example/ne:1");
        assert_eq!(m.nvidia_gpu_exporter_image, "reg.example/ngx:2");
        assert_eq!(m.reconcile_secs, 120);
        // 过低周期被夹紧
        let cfg = Config::parse_from(["modelinfer", "--frontend-dir", "/tmp", "--data-dir", "/tmp", "--monitor-reconcile-secs", "1"]);
        assert_eq!(cfg.monitor().reconcile_secs, 10);
    }
}
