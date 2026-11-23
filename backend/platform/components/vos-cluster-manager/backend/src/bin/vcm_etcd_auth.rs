//! `vcm-etcd-auth`：etcd 认证/TLS 迁移与凭据运维命令行
//!
//! 与 HTTP 端点共用同一套库函数（`etcd::provisioning` / `etcd::pki`），用于：
//! - `pki`：签发集群内部 CA / etcd 服务端证书 / 客户端证书
//! - `bootstrap`：幂等创建角色与用户（`--enable-auth` 才真正启用认证）
//! - `status`：查看认证开关 / 角色 / 用户 / 本机凭据（无口令）
//! - `rotate` / `revoke`：轮换 / 吊销单个身份
//! - `disable`：紧急回滚（关闭 etcd 认证；需 root 凭据可用）
//!
//! 典型用法（实机迁移，详见 `components/vos-cluster-manager/docs/etcd-auth-tls.md`）：
//! ```text
//! # 1) 生成 PKI（每节点各自签自己的服务端证书；CA 只在首个节点生成后分发）
//! vcm-etcd-auth pki --node-ips 10.100.10.21,10.100.10.22,10.100.10.23
//! # 2) 建角色/用户（不启用认证，留出兼容窗口）
//! vcm-etcd-auth bootstrap --auth-mode user --node-ips ... --no-enable-auth
//! # 3) 全节点切客户端配置并重启后，启用认证
//! vcm-etcd-auth bootstrap --auth-mode user --enable-auth
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use vos_cluster_manager::etcd::auth_admin::{EtcdAuthConnector, RealEtcdAuthConnector};
use vos_cluster_manager::etcd::credential_store::CredentialStore;
use vos_cluster_manager::etcd::pki;
use vos_cluster_manager::etcd::provisioning::{
    BootstrapOptions, bootstrap, probe_status, revoke_identity, rotate_identity,
};
use vos_cluster_manager::etcd::security::{
    CredentialLayout, DEFAULT_VCM_IDENTITY, EtcdAuthMode, default_identities,
    normalize_endpoints, validate_connect_spec, EtcdConnectSpec,
};

#[derive(Debug, Parser)]
#[command(
    name = "vcm-etcd-auth",
    version,
    about = "etcd 认证/TLS 迁移与凭据运维（vos-cluster-manager）"
)]
struct Cli {
    /// etcd 端点（逗号分隔；TLS 模式用 https://<ip>:2379）
    #[arg(long, env = "VOS_CLUSTER_MANAGER_ETCD_ENDPOINTS", value_delimiter = ',', default_value = "127.0.0.1:2379")]
    endpoints: Vec<String>,

    /// 凭据 / PKI 根目录
    #[arg(long, default_value = "/etc/vos/etcd")]
    credential_dir: PathBuf,

    /// 认证模式：disabled | user | mtls | mtls_user（`disable` 子命令忽略）
    #[arg(long, default_value = "user")]
    auth_mode: String,

    /// 可选 CA 证书覆盖（缺省 `<credential-dir>/pki/ca.crt`）
    #[arg(long)]
    ca_cert: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 生成 / 续签 CA、服务端证书与客户端证书
    Pki {
        /// 集群节点 IP（写入服务端证书 SAN）
        #[arg(long, value_delimiter = ',', default_value = "")]
        node_ips: Vec<String>,
        /// 需要客户端证书的身份
        #[arg(long, value_delimiter = ',', default_value = "")]
        identities: Vec<String>,
        /// 强制重签服务端 / 客户端证书（CA 不重签）
        #[arg(long)]
        force: bool,
    },
    /// 幂等创建角色与用户（默认不启用认证）
    Bootstrap {
        /// 集群节点 IP（推导每节点 agent 身份）
        #[arg(long, value_delimiter = ',', default_value = "")]
        node_ips: Vec<String>,
        /// 启用 etcd 认证（**最后一步**；完成后所有客户端必须使用凭据）
        #[arg(long)]
        enable_auth: bool,
        /// 强制轮换全部身份口令
        #[arg(long)]
        rotate: bool,
        /// root 口令（缺省读 `<pki>/root.password`，仍无则生成）
        #[arg(long, env = "VCM_ETCD_ROOT_PASSWORD")]
        root_password: Option<String>,
    },
    /// 查看认证状态（输出 JSON，无口令）
    Status,
    /// 轮换单个身份凭据
    Rotate {
        #[arg(long)]
        identity: String,
    },
    /// 吊销单个身份（删除 etcd 用户与本机凭据）
    Revoke {
        #[arg(long)]
        identity: String,
    },
    /// 紧急回滚：关闭 etcd 认证
    Disable {
        /// 确认执行（无此参数只做预检）
        #[arg(long)]
        yes: bool,
    },
}

fn fail(msg: impl Into<String>) -> ! {
    eprintln!("错误: {}", msg.into());
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let layout = CredentialLayout::new(cli.credential_dir.clone());
    let store = CredentialStore::new(layout.clone());
    let mode = EtcdAuthMode::parse(&cli.auth_mode).unwrap_or_else(|| {
        fail(format!(
            "--auth-mode 非法: {}（disabled / user / mtls / mtls_user）",
            cli.auth_mode
        ))
    });

    match cli.command {
        Command::Pki {
            node_ips,
            identities,
            force,
        } => {
            let ids = if identities.is_empty() {
                vec![DEFAULT_VCM_IDENTITY.to_string()]
            } else {
                identities
            };
            match pki::ensure_pki(&layout, &node_ips, &ids, force) {
                Ok(report) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ca_created": report.ca_created,
                            "server_created": report.server_created,
                            "ca_cert": report.ca_cert,
                            "server_cert": report.server_cert,
                            "server_sans": report.server_sans,
                            "client_identities": ids,
                        }))
                        .unwrap()
                    );
                }
                Err(e) => fail(e.msg),
            }
        }
        Command::Bootstrap {
            node_ips,
            enable_auth,
            rotate,
            root_password,
        } => {
            let ca = cli.ca_cert.clone().unwrap_or_else(|| layout.ca_cert());
            let endpoints = normalize_endpoints(&cli.endpoints, mode.requires_tls());
            let connector = RealEtcdAuthConnector::new(endpoints.clone(), Some(ca.clone()));
            let identities = default_identities(&node_ips);
            let opts = BootstrapOptions {
                identities,
                enable_auth,
                rotate,
                auth_mode: mode,
                endpoints: cli.endpoints.clone(),
                root_password,
            };
            match bootstrap(&connector, &store, &opts).await {
                Ok(report) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "auth_was_enabled": report.auth_was_enabled,
                            "auth_enabled": report.auth_enabled,
                            "auth_enabled_by_bootstrap": report.auth_enabled_by_bootstrap,
                            "verified_after_enable": report.verified_after_enable,
                            "root_password_generated": report.root_password_generated,
                            "root_password_file": report.root_password_file,
                            "roles_ensured": report.roles_ensured,
                            "identities": report.identities.iter().map(|i| serde_json::json!({
                                "identity": i.identity,
                                "roles": i.roles,
                                "password_rotated": i.password_rotated,
                                "credential_written": i.credential_written,
                                "generation": i.generation,
                            })).collect::<Vec<_>>(),
                        }))
                        .unwrap()
                    );
                }
                Err(e) => fail(e.msg),
            }
        }
        Command::Status => {
            let ca = cli.ca_cert.clone().unwrap_or_else(|| layout.ca_cert());
            let endpoints = normalize_endpoints(&cli.endpoints, mode.requires_tls());
            let connector = RealEtcdAuthConnector::new(endpoints, Some(ca));
            let st = probe_status(&connector, &store, &cli.endpoints, mode).await;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "auth_enabled": st.auth_enabled,
                    "auth_probe_error": st.auth_probe_error,
                    "pki_present": st.pki_present,
                    "root_password_present": st.root_password_present,
                    "roles": st.roles.iter().map(|r| serde_json::json!({
                        "name": r.name, "present": r.present, "grants": r.grants
                    })).collect::<Vec<_>>(),
                    "users": st.users,
                    "credentials": store.list().unwrap_or_default(),
                }))
                .unwrap()
            );
        }
        Command::Rotate { identity } => {
            let ca = cli.ca_cert.clone().unwrap_or_else(|| layout.ca_cert());
            let endpoints = normalize_endpoints(&cli.endpoints, mode.requires_tls());
            let connector = RealEtcdAuthConnector::new(endpoints, Some(ca));
            match rotate_identity(&connector, &store, &cli.endpoints, mode, &identity).await {
                Ok(o) => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "identity": o.identity, "generation": o.generation,
                        "password_rotated": o.password_rotated,
                    }))
                    .unwrap()
                ),
                Err(e) => fail(e.msg),
            }
        }
        Command::Revoke { identity } => {
            let ca = cli.ca_cert.clone().unwrap_or_else(|| layout.ca_cert());
            let endpoints = normalize_endpoints(&cli.endpoints, mode.requires_tls());
            let connector = RealEtcdAuthConnector::new(endpoints, Some(ca));
            match revoke_identity(&connector, &store, &cli.endpoints, mode, &identity).await {
                Ok(existed) => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "identity": identity, "existed": existed, "revoked": true,
                    }))
                    .unwrap()
                ),
                Err(e) => fail(e.msg),
            }
        }
        Command::Disable { yes } => {
            if !yes {
                println!(
                    "预检：将关闭 etcd 认证（auth disable）。确认执行请追加 --yes。\n\
                     回滚后所有客户端需恢复明文 / 匿名配置（VOS_CLUSTER_MANAGER_ETCD_AUTH_MODE=disabled）。"
                );
                return;
            }
            let ca = cli.ca_cert.clone().unwrap_or_else(|| layout.ca_cert());
            let endpoints = normalize_endpoints(&cli.endpoints, true);
            let connector = RealEtcdAuthConnector::new(endpoints, Some(ca));
            match disable_auth(&connector, &layout, &cli.endpoints).await {
                Ok(()) => println!("etcd 认证已关闭（请同步恢复各客户端配置）"),
                Err(e) => fail(e.msg),
            }
        }
    }
}

/// 关闭 etcd 认证（紧急回滚）
async fn disable_auth(
    connector: &dyn EtcdAuthConnector,
    layout: &CredentialLayout,
    endpoints: &[String],
) -> Result<(), vos_cluster_manager::error::VcmError> {
    use vos_cluster_manager::error::{codes, VcmError};

    let root_password = pki::read_root_password(layout)?.ok_or_else(|| {
        VcmError::new(
            codes::ETCD_AUTH_FAILED,
            format!(
                "缺少 root 口令文件 {}，无法关闭认证",
                layout.root_password_file().display()
            ),
        )
    })?;
    let spec = EtcdConnectSpec {
        endpoints: normalize_endpoints(endpoints, true),
        mode: EtcdAuthMode::User,
        ca_cert: Some(layout.ca_cert()),
        client_cert: None,
        client_key: None,
        username: Some("root".to_string()),
        password: Some(root_password),
    };
    validate_connect_spec(&spec).map_err(|e| VcmError::new(codes::PARAM_INVALID, e))?;
    let admin: Arc<dyn vos_cluster_manager::etcd::EtcdAuthAdmin> = connector.connect(&spec).await?;
    admin.auth_disable().await
}