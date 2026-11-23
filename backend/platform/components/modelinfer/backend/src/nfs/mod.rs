//! NFS 解析与节点挂载编排
//!
//! 解析：应用 path 配置（model hub）→ 所属存储空间 → NFS 导出路径 → NFS 服务端 IP。
//! 挂载：经 SSH 在目标节点幂等挂载 NFS 导出，校验 `mountpoint`，失败回滚。
//!
//! **协议版本约束**：所有挂载参数均**不显式指定 NFS 版本**（无 `vers=` / `nfsvers=`），
//! 由节点内核 NFS 客户端与服务端协商；硬编码版本会在「服务端不支持该版本」的现场失败
//! （如 E1001 的 NFS 服务端仅支持 v3）。设计见
//! `openspec/changes/modelinfer-nfs-version-negotiation/design.md` 与
//! `specs/docs/design/modelinfer/index.md`（节点级 fscache 挂载参数）。

use serde::{Deserialize, Serialize};

use crate::error::{ModelinferError, codes};
use crate::platform::{NfsShareInfo, PlatformClient, VolumeInfo};
use crate::ssh::{SshExec, SshTarget};

/// 应用 id（硬编码：本应用的 manifest.id）
pub const APP_ID: &str = "com.ictrek.modelinfer";
/// model hub path 配置名（configs.yml）
pub const MODEL_HUB_CONFIG: &str = "MODELINFER_MODEL_HUB_PATH";

/// NFS 解析结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NfsResolveResult {
    pub config_name: String,
    /// 配置解析后的宿主机绝对路径（display_path）
    pub host_path: String,
    pub volume_uuid: String,
    pub volume_name: String,
    /// NFS 服务端 IP
    pub nfs_server: String,
    /// NFS 导出路径（存储空间挂载点）
    pub nfs_export_path: String,
    /// host_path 相对导出根的子路径（如 `model_hub`；可为空）
    pub sub_path: String,
    pub read_only: bool,
    pub mounted: bool,
    /// 组装出的挂载源 `<server>:<export>`
    pub mount_source: String,
}

impl NfsResolveResult {
    /// 空结果：表示本次部署**不使用** model-hub NFS（如 `node-local` 权重来源），
    /// 挂载 / 卸载编排应据此跳过。
    pub fn not_mounted() -> Self {
        Self {
            config_name: String::new(),
            host_path: String::new(),
            volume_uuid: String::new(),
            volume_name: String::new(),
            nfs_server: String::new(),
            nfs_export_path: String::new(),
            sub_path: String::new(),
            read_only: false,
            mounted: false,
            mount_source: String::new(),
        }
    }
}

/// 解析参数（便于纯函数测试）
pub struct ResolveInput<'a> {
    pub config_name: &'a str,
    pub fallback_hub_path: &'a str,
    pub nfs_server_override: &'a str,
    pub node_hosts: &'a [String],
}

/// 从 configs_json 提取 host_path 与 public_directory_uuid（纯函数）
pub fn extract_config_source(configs: &serde_json::Value, config_name: &str) -> Result<(String, Option<String>), ModelinferError> {
    let raw = configs.get(config_name);
    match raw {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Ok((s.trim().to_string(), None)),
        Some(serde_json::Value::Object(map)) => {
            let display = map
                .get("display_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let pd = map
                .get("public_directory_uuid")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let value = map.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let path = if !display.is_empty() { display } else { value };
            Ok((path, pd))
        }
        _ => Err(ModelinferError::new(
            codes::NFS_RESOLVE_FAILED,
            format!("未找到 model hub 路径配置 {config_name}"),
        )),
    }
}

/// 前缀匹配存储空间：返回 mount_point 为其前缀且最长者（纯函数）
pub fn match_volume<'a>(host_path: &str, volumes: &'a [VolumeInfo]) -> Option<&'a VolumeInfo> {
    volumes
        .iter()
        .filter(|v| !v.mount_point.is_empty())
        .filter(|v| host_path == v.mount_point || host_path.starts_with(&format!("{}/", v.mount_point)))
        .max_by_key(|v| v.mount_point.len())
}

/// 选择 NFS 服务端 IP：优先 override；否则过滤 loopback / link-local / docker 网桥，
/// 优先与节点同 /24 网段的地址（纯函数）
pub fn pick_nfs_server(override_ip: &str, candidates: &[String], node_hosts: &[String]) -> Option<String> {
    if !override_ip.trim().is_empty() {
        return Some(override_ip.trim().to_string());
    }
    let usable: Vec<&String> = candidates.iter().filter(|ip| is_usable_ip(ip)).collect();
    // 优先与节点同 /24
    for ip in &usable {
        if let Some(prefix) = ip.rsplit_once('.').map(|(p, _)| p)
            && node_hosts.iter().any(|h| h.starts_with(&format!("{prefix}.")))
        {
            return Some((*ip).clone());
        }
    }
    usable.first().map(|ip| (*ip).clone())
}

/// 是否为可用于与节点通信的本机 IP（排除 loopback / link-local / IPv6 / docker 网桥）
pub fn is_usable_ip(ip: &str) -> bool {
    if ip.starts_with("127.") || ip.starts_with("169.254.") || ip.contains(':') {
        return false;
    }
    // 排除 docker 默认网桥网段
    if ip.starts_with("172.17.") || ip.starts_with("172.18.") || ip.starts_with("172.19.") {
        return false;
    }
    true
}

/// 计算子路径（host_path 相对导出根，纯函数）
pub fn sub_path_of(host_path: &str, export_path: &str) -> String {
    if host_path == export_path {
        return String::new();
    }
    host_path
        .strip_prefix(export_path)
        .unwrap_or("")
        .trim_start_matches('/')
        .to_string()
}

/// 解析 model hub 存储空间的 NFS 信息
pub async fn resolve_nfs(
    platform: &dyn PlatformClient,
    input: &ResolveInput<'_>,
) -> Result<NfsResolveResult, ModelinferError> {
    let app_id = platform.app_id();
    let config_name = match (app_id, input.config_name) {
        ("com.ictrek.v-scheduler", MODEL_HUB_CONFIG) => "MODEL_HUB_SHARED_MODELS_PATH",
        (_, name) => name,
    };
    let configs = platform.app_configs_json(app_id).await?;
    let (host_path, public_dir_uuid) = match extract_config_source(&configs, config_name) {
        Ok(v) => v,
        Err(e) => {
            // 平台配置缺失时兜底到环境变量/启动参数
            if input.fallback_hub_path.trim().is_empty() {
                return Err(e);
            }
            (input.fallback_hub_path.trim().to_string(), None)
        }
    };
    if host_path.trim().is_empty() {
        return Err(ModelinferError::new(codes::NFS_RESOLVE_FAILED, "model hub 路径为空"));
    }

    let volumes = platform.list_volumes().await?;
    let shares = platform.list_nfs_shares().await?;

    // 定位存储空间
    let volume: VolumeInfo = if let Some(pd_uuid) = public_dir_uuid {
        let dirs = platform.list_public_directories().await?;
        let dir = dirs
            .into_iter()
            .find(|d| d.uuid == pd_uuid)
            .ok_or_else(|| ModelinferError::new(codes::NFS_RESOLVE_FAILED, format!("公共目录不存在: {pd_uuid}")))?;
        volumes
            .iter()
            .find(|v| v.uuid == dir.volume_uuid)
            .cloned()
            .ok_or_else(|| {
                ModelinferError::new(codes::NFS_RESOLVE_FAILED, format!("公共目录所属存储空间不存在: {}", dir.volume_uuid))
            })?
    } else {
        match_volume(&host_path, &volumes)
            .cloned()
            .ok_or_else(|| {
                ModelinferError::new(
                    codes::NFS_RESOLVE_FAILED,
                    format!("路径 {host_path} 不属于任何已挂载存储空间"),
                )
            })?
    };

    let share: NfsShareInfo = shares
        .into_iter()
        .find(|s| s.volume_uuid == volume.uuid)
        .ok_or_else(|| {
            ModelinferError::new(
                codes::NFS_SHARE_UNAVAILABLE,
                format!(
                    "权重目录 {host_path} 所在的存储空间「{}」（{}）未开启 NFS 共享。请在平台「存储空间」中为该存储空间开启 NFS 共享后重试。",
                    volume.name, volume.uuid,
                ),
            )
        })?;
    if !share.mounted {
        return Err(ModelinferError::new(
            codes::NFS_SHARE_UNAVAILABLE,
            format!(
                "权重目录 {host_path} 所在的存储空间「{}」（{}）尚未挂载，NFS 共享不可用。请检查存储空间挂载状态及 NFS 共享后重试。",
                volume.name, volume.uuid,
            ),
        ));
    }

    let export_path = if share.mount_point.is_empty() {
        volume.mount_point.clone()
    } else {
        share.mount_point.clone()
    };

    let device = platform.device_info().await?;
    let server = pick_nfs_server(input.nfs_server_override, &device.ip_addr, input.node_hosts)
        .ok_or_else(|| ModelinferError::new(codes::NFS_RESOLVE_FAILED, "无法推断 NFS 服务端 IP"))?;

    Ok(NfsResolveResult {
        config_name: config_name.to_string(),
        host_path: host_path.clone(),
        volume_uuid: volume.uuid.clone(),
        volume_name: volume.name.clone(),
        nfs_server: server.clone(),
        nfs_export_path: export_path.clone(),
        sub_path: sub_path_of(&host_path, &export_path),
        read_only: share.read_only,
        mounted: share.mounted,
        mount_source: format!("{server}:{export_path}"),
    })
}

/// 单节点挂载结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMountResult {
    pub node_id: String,
    pub name: String,
    pub host: String,
    pub mount_dir: String,
    pub ok: bool,
    pub message: String,
}

/// 普通挂载参数（未启用 fscache）。
///
/// **不指定 NFS 协议版本**：版本由节点内核 NFS 客户端与服务端协商（`mount -t nfs` 未给
/// `vers` 时由 `mount.nfs` 按服务端能力探测 4.2 → 4.1 → 4.0 → 3）。硬编码版本会在
/// 「服务端不支持该版本」的现场直接挂载失败（如 E1001 的 NFS 服务端仅支持 v3）。
pub const MOUNT_OPTS_PLAIN: &str = "_netdev,soft,timeo=30";
/// fscache 挂载参数（对齐 `docs/dsv41-4x-gb10/scripts/nfs_fscache_setup.sh` 实机经验）。
///
/// 同样**不指定协议版本**：`fsc` 是内核 `cachefiles` 前端缓存开关，与 NFSv3 / v4.x 均无关；
/// 其余 `tcp` / `hard` / `timeo` / `retrans` / `rsize` / `wsize` / `nconnect` 为实机验证过的
/// 吞吐与可靠性参数，也与版本无关。
pub const MOUNT_OPTS_FSCACHE: &str =
    "tcp,fsc,hard,timeo=600,retrans=2,rsize=1048576,wsize=1048576,nconnect=8";

/// 构建挂载参数（纯函数）：按节点 `fscache` 开关选择参数集；只读追加 `,ro`。
///
/// - `fscache = false`：`_netdev,soft,timeo=30`（默认）；
/// - `fscache = true`：`tcp,fsc,hard,timeo=600,retrans=2,rsize=1048576,wsize=1048576,
///   nconnect=8`（节点侧 `cachefilesd` 由用户负责）。
///
/// 两套参数**都不含 `vers` / `nfsvers`**：NFS 协议版本由客户端与服务端协商决定，
/// 详见 `openspec/changes/modelinfer-nfs-version-negotiation/design.md`（D-NV-01~05）。
pub fn build_mount_options(read_only: bool, fscache: bool) -> String {
    let base = if fscache { MOUNT_OPTS_FSCACHE } else { MOUNT_OPTS_PLAIN };
    if read_only {
        format!("{base},ro")
    } else {
        base.to_string()
    }
}

/// 构建幂等挂载命令（纯函数）
///
/// 挂载目录若残留**失效的 NFS 挂载点**（NFS 服务端重启 / 网络抖动后 `stale file handle`），
/// 直接 `mkdir -p` 会失败并被 `set -e` 中止，导致整个 `mount-nfs` 阶段失败（实机踩坑）。
/// 因此在创建目录失败时先 `umount -l` 惰性卸载失效挂载、再重建目录后挂载；对**正常的
/// 已挂载目录**不产生影响（`mkdir -p` 对已存在目录返回 0，不会进入恢复分支）。
pub fn build_mount_cmd(mount_dir: &str, source: &str, read_only: bool, fscache: bool) -> String {
    let opts = build_mount_options(read_only, fscache);
    format!(
        "set -e; \
         if ! mkdir -p '{dir}' 2>/dev/null; then \
           echo STALE_MOUNT_RECOVER; umount -l '{dir}' 2>/dev/null || true; mkdir -p '{dir}'; \
         fi; \
         if mountpoint -q '{dir}'; then echo ALREADY_MOUNTED; \
         else mount -t nfs -o {opts} '{source}' '{dir}'; fi; \
         mountpoint -q '{dir}' && echo MOUNT_OK",
        dir = mount_dir,
        opts = opts,
        source = source
    )
}

/// 构建卸载命令（纯函数）
pub fn build_umount_cmd(mount_dir: &str) -> String {
    format!(
        "if mountpoint -q '{dir}'; then umount -l '{dir}' && echo UMOUNT_OK; else echo NOT_MOUNTED; fi",
        dir = mount_dir
    )
}

/// 构建状态查询命令（纯函数）；附带 `OPTIONS` 便于核验 `fsc` 是否生效
pub fn build_status_cmd(mount_dir: &str) -> String {
    format!(
        "if mountpoint -q '{dir}'; then findmnt -n -o SOURCE,FSTYPE,TARGET,OPTIONS '{dir}'; else echo NOT_MOUNTED; fi",
        dir = mount_dir
    )
}

/// 节点挂载目标信息
pub struct MountNode {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// 认证方式（`password` / `key`）
    pub auth_method: String,
    pub password: String,
    /// SSH 私钥（`auth_method=key` 时）
    pub ssh_key: String,
    pub sudo_password: String,
    /// 是否启用 fscache（按节点选择挂载参数；节点侧 cachefilesd 由用户负责）
    pub fscache: bool,
}

/// 在节点上执行 NFS 挂载；任一下降则回滚已挂载节点
pub async fn mount_on_nodes(
    ssh: &dyn SshExec,
    resolved: &NfsResolveResult,
    nodes: &[MountNode],
    mount_dir: &str,
) -> Result<Vec<NodeMountResult>, ModelinferError> {
    mount_on_nodes_protected(ssh, resolved, nodes, mount_dir, &[]).await
}

/// 在节点上执行 NFS 挂载；回滚时跳过 `protected_node_ids` 中的共享节点
///
/// 多个运行配置可共享节点：若本次挂载失败需要回滚，而某节点仍被其他活跃运行配置使用，
/// MUST NOT 卸载该节点上的 NFS，否则会破坏其他配置正在使用的权重目录。
pub async fn mount_on_nodes_protected(
    ssh: &dyn SshExec,
    resolved: &NfsResolveResult,
    nodes: &[MountNode],
    mount_dir: &str,
    protected_node_ids: &[String],
) -> Result<Vec<NodeMountResult>, ModelinferError> {
    if nodes.is_empty() {
        return Err(ModelinferError::new(codes::PARAM_INVALID, "没有可挂载的节点"));
    }
    let mut results = Vec::new();
    let mut mounted_hosts: Vec<MountNode> = Vec::new();

    for node in nodes {
        // 按节点构建命令：fscache 开关是节点级属性，同一批节点可各自不同
        let cmd = build_mount_cmd(mount_dir, &resolved.mount_source, resolved.read_only, node.fscache);
        let target = SshTarget::from_credentials(
            node.host.clone(),
            node.port,
            node.user.clone(),
            &node.auth_method,
            &node.password,
            &node.ssh_key,
        ).with_sudo_password(&node.sudo_password);
        match ssh.run(&target, &cmd).await {
            Ok(out) if out.success() => {
                let already = out.stdout.contains("ALREADY_MOUNTED");
                results.push(NodeMountResult {
                    node_id: node.id.clone(),
                    name: node.name.clone(),
                    host: node.host.clone(),
                    mount_dir: mount_dir.to_string(),
                    ok: true,
                    message: out.stdout.trim().to_string(),
                });
                // 仅回滚本次真正新建的挂载（跳过原本已挂载的节点）
                if !already {
                    mounted_hosts.push(MountNode {
                        id: node.id.clone(),
                        name: node.name.clone(),
                        host: node.host.clone(),
                        port: node.port,
                        user: node.user.clone(),
                        auth_method: node.auth_method.clone(),
                        password: node.password.clone(),
                        ssh_key: node.ssh_key.clone(),
                        sudo_password: node.sudo_password.clone(),
                        fscache: node.fscache,
                    });
                }
            }
            Ok(out) => {
                let detail = format!(
                    "挂载失败(exit={}): {}{}",
                    out.exit_code,
                    out.stdout.trim(),
                    out.stderr.trim()
                );
                rollback(ssh, &mounted_hosts, mount_dir, protected_node_ids).await;
                results.push(NodeMountResult {
                    node_id: node.id.clone(),
                    name: node.name.clone(),
                    host: node.host.clone(),
                    mount_dir: mount_dir.to_string(),
                    ok: false,
                    message: detail,
                });
                return Err(rollback_error(results));
            }
            Err(e) => {
                rollback(ssh, &mounted_hosts, mount_dir, protected_node_ids).await;
                results.push(NodeMountResult {
                    node_id: node.id.clone(),
                    name: node.name.clone(),
                    host: node.host.clone(),
                    mount_dir: mount_dir.to_string(),
                    ok: false,
                    message: e.msg.clone(),
                });
                return Err(rollback_error(results));
            }
        }
    }
    Ok(results)
}

fn rollback_error(results: Vec<NodeMountResult>) -> ModelinferError {
    let detail = results
        .iter()
        .map(|r| format!("{}:{}", r.host, r.message))
        .collect::<Vec<_>>()
        .join("; ");
    ModelinferError::new(codes::NFS_MOUNT_FAILED, format!("NFS 挂载失败并已回滚: {detail}"))
}

async fn rollback(
    ssh: &dyn SshExec,
    nodes: &[MountNode],
    mount_dir: &str,
    protected_node_ids: &[String],
) {
    let cmd = build_umount_cmd(mount_dir);
    for node in nodes {
        if protected_node_ids.iter().any(|id| id == &node.id) {
            continue;
        }
        let target = SshTarget::from_credentials(
            node.host.clone(),
            node.port,
            node.user.clone(),
            &node.auth_method,
            &node.password,
            &node.ssh_key,
        ).with_sudo_password(&node.sudo_password);
        let _ = ssh.run(&target, &cmd).await;
    }
}

/// 卸载
pub async fn umount_on_nodes(
    ssh: &dyn SshExec,
    nodes: &[MountNode],
    mount_dir: &str,
) -> Result<Vec<NodeMountResult>, ModelinferError> {
    let cmd = build_umount_cmd(mount_dir);
    let mut results = Vec::new();
    for node in nodes {
        let target = SshTarget::from_credentials(
            node.host.clone(),
            node.port,
            node.user.clone(),
            &node.auth_method,
            &node.password,
            &node.ssh_key,
        ).with_sudo_password(&node.sudo_password);
        let (ok, message) = match ssh.run(&target, &cmd).await {
            Ok(out) => (out.success(), out.stdout.trim().to_string()),
            Err(e) => (false, e.msg.clone()),
        };
        results.push(NodeMountResult {
            node_id: node.id.clone(),
            name: node.name.clone(),
            host: node.host.clone(),
            mount_dir: mount_dir.to_string(),
            ok,
            message,
        });
    }
    Ok(results)
}

/// 查询挂载状态
pub async fn status_on_nodes(
    ssh: &dyn SshExec,
    nodes: &[MountNode],
    mount_dir: &str,
) -> Result<Vec<NodeMountResult>, ModelinferError> {
    let cmd = build_status_cmd(mount_dir);
    let mut results = Vec::new();
    for node in nodes {
        let target = SshTarget::from_credentials(
            node.host.clone(),
            node.port,
            node.user.clone(),
            &node.auth_method,
            &node.password,
            &node.ssh_key,
        ).with_sudo_password(&node.sudo_password);
        let (ok, message) = match ssh.run(&target, &cmd).await {
            Ok(out) => {
                let mounted = out.success() && !out.stdout.contains("NOT_MOUNTED");
                (mounted, out.stdout.trim().to_string())
            }
            Err(e) => (false, e.msg.clone()),
        };
        results.push(NodeMountResult {
            node_id: node.id.clone(),
            name: node.name.clone(),
            host: node.host.clone(),
            mount_dir: mount_dir.to_string(),
            ok,
            message,
        });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::MockPlatformClient;

    fn vol(uuid: &str, mp: &str) -> VolumeInfo {
        VolumeInfo { uuid: uuid.into(), name: uuid.into(), mount_point: mp.into(), state: "Running".into() }
    }
    fn share(volume: &str, mp: &str) -> NfsShareInfo {
        NfsShareInfo {
            uuid: format!("s-{volume}"),
            volume_uuid: volume.into(),
            name: volume.into(),
            mount_point: mp.into(),
            read_only: false,
            state: "Running".into(),
            mounted: true,
        }
    }

    #[test]
    fn extract_config_source_variants() {
        let cfg = serde_json::json!({
            "MODELINFER_MODEL_HUB_PATH": {
                "source_type": "host_path",
                "value": "/share/vol/model_hub",
                "display_path": "/share/vol/model_hub"
            }
        });
        let (path, pd) = extract_config_source(&cfg, MODEL_HUB_CONFIG).unwrap();
        assert_eq!(path, "/share/vol/model_hub");
        assert!(pd.is_none());

        let cfg2 = serde_json::json!({ "MODELINFER_MODEL_HUB_PATH": "/data/models" });
        let (path2, _) = extract_config_source(&cfg2, MODEL_HUB_CONFIG).unwrap();
        assert_eq!(path2, "/data/models");
    }

    #[test]
    fn match_volume_longest_prefix() {
        let vols = vec![vol("v1", "/share/v1"), vol("v2", "/share/v1/projects")];
        let v = match_volume("/share/v1/projects/models", &vols).unwrap();
        assert_eq!(v.uuid, "v2");
        assert!(match_volume("/data/x", &vols).is_none());
    }

    #[test]
    fn pick_nfs_server_prefers_node_subnet_and_skips_docker() {
        let candidates = vec![
            "172.17.0.1".to_string(),
            "127.0.0.1".to_string(),
            "10.100.10.24".to_string(),
            "192.168.1.5".to_string(),
        ];
        let nodes = vec!["10.100.10.21".to_string()];
        assert_eq!(pick_nfs_server("", &candidates, &nodes).unwrap(), "10.100.10.24");
        assert_eq!(pick_nfs_server("10.0.0.9", &candidates, &nodes).unwrap(), "10.0.0.9");
    }

    #[test]
    fn sub_path_computation() {
        assert_eq!(sub_path_of("/share/v1/model_hub", "/share/v1"), "model_hub");
        assert_eq!(sub_path_of("/share/v1", "/share/v1"), "");
    }

    #[tokio::test]
    async fn resolve_nfs_happy_path() {
        let mut p = MockPlatformClient::new();
        p.app_configs = serde_json::json!({
            "MODELINFER_MODEL_HUB_PATH": { "source_type":"host_path", "value":"/share/v1/model_hub", "display_path":"/share/v1/model_hub" }
        });
        p.volumes = vec![vol("v1", "/share/v1")];
        p.nfs_shares = vec![share("v1", "/share/v1")];
        p.device = crate::platform::DeviceInfo { ip_addr: vec!["172.17.0.1".into(), "10.100.10.24".into()] };
        let input = ResolveInput {
            config_name: MODEL_HUB_CONFIG,
            fallback_hub_path: "",
            nfs_server_override: "",
            node_hosts: &["10.100.10.21".to_string()],
        };
        let r = resolve_nfs(&p, &input).await.unwrap();
        assert_eq!(r.nfs_server, "10.100.10.24");
        assert_eq!(r.nfs_export_path, "/share/v1");
        assert_eq!(r.sub_path, "model_hub");
        assert_eq!(r.mount_source, "10.100.10.24:/share/v1");
    }

    #[tokio::test]
    async fn resolve_nfs_unmapped_path_fails() {
        let mut p = MockPlatformClient::new();
        p.app_configs = serde_json::json!({ "MODELINFER_MODEL_HUB_PATH": "/data/x" });
        p.volumes = vec![vol("v1", "/share/v1")];
        p.nfs_shares = vec![share("v1", "/share/v1")];
        p.device = crate::platform::DeviceInfo { ip_addr: vec!["10.0.0.5".into()] };
        let input = ResolveInput {
            config_name: MODEL_HUB_CONFIG,
            fallback_hub_path: "",
            nfs_server_override: "",
            node_hosts: &[],
        };
        let err = resolve_nfs(&p, &input).await.unwrap_err();
        assert_eq!(err.code, codes::NFS_RESOLVE_FAILED);
    }

    #[tokio::test]
    async fn resolve_nfs_share_preflight_errors_and_read_only_success() {
        let input = ResolveInput {
            config_name: MODEL_HUB_CONFIG,
            fallback_hub_path: "/share/v1/model_hub",
            nfs_server_override: "10.0.0.9",
            node_hosts: &[],
        };
        let mut p = MockPlatformClient::new();
        let mut volume = vol("v1", "/share/v1");
        volume.name = "模型盘".into();
        p.volumes = vec![volume];
        // 其他卷已经共享也不能满足目标权重卷的要求；配置缺失时仍检查 fallback 路径。
        p.nfs_shares = vec![share("other", "/share/other")];
        let err = resolve_nfs(&p, &input).await.unwrap_err();
        assert_eq!(err.code, codes::NFS_SHARE_UNAVAILABLE);
        for detail in ["模型盘", "v1", "/share/v1/model_hub", "未开启 NFS 共享", "开启 NFS 共享后重试"] {
            assert!(err.msg.contains(detail), "{}", err.msg);
        }
        assert!(!p.calls.lock().unwrap().iter().any(|c| c == "device_info"));

        let mut target = share("v1", "/share/v1");
        target.mounted = false;
        p.nfs_shares.push(target);
        let err = resolve_nfs(&p, &input).await.unwrap_err();
        assert_eq!(err.code, codes::NFS_SHARE_UNAVAILABLE);
        assert!(err.msg.contains("尚未挂载"));
        assert!(err.msg.contains("模型盘"));

        p.nfs_shares[1].mounted = true;
        p.nfs_shares[1].read_only = true;
        let resolved = resolve_nfs(&p, &input).await.unwrap();
        assert_eq!(resolved.volume_uuid, "v1");
        assert!(resolved.read_only);

        // 预检后关闭共享，异步准备再次解析时仍会拒绝。
        p.nfs_shares.clear();
        assert_eq!(resolve_nfs(&p, &input).await.unwrap_err().code, codes::NFS_SHARE_UNAVAILABLE);

        p.fail = true;
        assert_eq!(resolve_nfs(&p, &input).await.unwrap_err().code, codes::PLATFORM_CALL_FAILED);
    }

    #[tokio::test]
    async fn resolve_nfs_checks_public_directory_owner_share() {
        let mut p = MockPlatformClient::new();
        p.app_configs = serde_json::json!({
            "MODELINFER_MODEL_HUB_PATH": {
                "display_path": "/public/models",
                "public_directory_uuid": "pd1"
            }
        });
        p.volumes = vec![vol("v1", "/share/v1"), vol("other", "/public")];
        p.public_dirs = vec![crate::platform::PublicDirectoryInfo {
            uuid: "pd1".into(),
            volume_uuid: "v1".into(),
            name: "models".into(),
            mount_point: None,
        }];
        p.nfs_shares = vec![share("other", "/public")];
        let input = ResolveInput {
            config_name: MODEL_HUB_CONFIG,
            fallback_hub_path: "",
            nfs_server_override: "10.0.0.9",
            node_hosts: &[],
        };
        let err = resolve_nfs(&p, &input).await.unwrap_err();
        assert_eq!(err.code, codes::NFS_SHARE_UNAVAILABLE);
        assert!(err.msg.contains("（v1）"));
        assert!(err.msg.contains("/public/models"));
        assert!(p.calls.lock().unwrap().iter().any(|c| c == "list_public_directories"));
    }

    #[tokio::test]
    async fn mount_success_path() {
        use crate::ssh::MockSshExec;
        let ssh = MockSshExec::new();
        let nodes = vec![mount_node("1", "n1", "10.0.0.1")];
        let r = mount_on_nodes(&ssh, &resolved_sample(), &nodes, "/mnt/model_hub").await.unwrap();
        assert!(r[0].ok);
        let calls = ssh.recorded_calls();
        assert!(calls.iter().any(|c| c.contains("mount -t nfs")));
    }

    #[tokio::test]
    async fn mount_rolls_back_and_errors_on_host_failure() {
        use crate::ssh::MockSshExec;
        // 10.0.0.2 上命令失败：10.0.0.1 已挂载后应被 umount 回滚
        let ssh = MockSshExec::new().with_failing_host("10.0.0.2");
        let nodes = vec![
            mount_node("1", "n1", "10.0.0.1"),
            mount_node("2", "n2", "10.0.0.2"),
        ];
        let err = mount_on_nodes(&ssh, &resolved_sample(), &nodes, "/mnt/model_hub")
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::NFS_MOUNT_FAILED);
        let calls = ssh.recorded_calls();
        // 回滚应出现在 10.0.0.1 上（用 UMOUNT_OK 区分卸载命令与挂载命令内的 stale 恢复片段）
        assert!(calls.iter().any(|c| c.starts_with("10.0.0.1:") && c.contains("UMOUNT_OK")));
    }

    #[tokio::test]
    async fn mount_rollback_skips_shared_nodes() {
        use crate::ssh::MockSshExec;
        // 10.0.0.2 挂载失败：节点 1 虽为本次新建挂载，但仍被其他活跃配置共享 → 不得回滚卸载
        let ssh = MockSshExec::new().with_failing_host("10.0.0.2");
        let nodes = vec![
            mount_node("1", "n1", "10.0.0.1"),
            mount_node("2", "n2", "10.0.0.2"),
        ];
        let err = mount_on_nodes_protected(
            &ssh,
            &resolved_sample(),
            &nodes,
            "/mnt/model_hub",
            &["1".to_string()],
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::NFS_MOUNT_FAILED);
        let calls = ssh.recorded_calls();
        assert!(
            !calls.iter().any(|c| c.starts_with("10.0.0.1:") && c.contains("UMOUNT_OK")),
            "共享节点不应被回滚卸载: {calls:?}"
        );
    }

    fn mount_node(id: &str, name: &str, host: &str) -> MountNode {
        MountNode {
            id: id.into(),
            name: name.into(),
            host: host.into(),
            port: 22,
            user: "root".into(),
            auth_method: "password".into(),
            password: "x".into(),
            ssh_key: String::new(),
            sudo_password: String::new(),
            fscache: false,
        }
    }

    #[test]
    fn mount_options_plain_has_no_protocol_version() {
        // 普通参数：不指定 NFS 协议版本，由客户端与服务端协商
        assert_eq!(MOUNT_OPTS_PLAIN, "_netdev,soft,timeo=30");
        assert_eq!(build_mount_options(false, false), MOUNT_OPTS_PLAIN);
        assert_eq!(build_mount_options(true, false), format!("{MOUNT_OPTS_PLAIN},ro"));
        assert!(!build_mount_options(false, false).contains("fsc"));
    }

    #[test]
    fn mount_options_fscache_matches_dsv41_experience() {
        assert_eq!(build_mount_options(false, true), MOUNT_OPTS_FSCACHE);
        assert_eq!(
            build_mount_options(true, true),
            format!("{MOUNT_OPTS_FSCACHE},ro")
        );
        let opts = build_mount_options(true, true);
        for expected in [
            "tcp",
            "fsc",
            "hard",
            "timeo=600",
            "retrans=2",
            "rsize=1048576",
            "wsize=1048576",
            "nconnect=8",
            "ro",
        ] {
            assert!(opts.contains(expected), "缺少 {expected}: {opts}");
        }
    }

    #[test]
    fn mount_options_never_pin_protocol_version() {
        // 需求：无论是否启用 fscache、是否只读，都不得显式传入 vers
        let options = vec![
            MOUNT_OPTS_PLAIN.to_string(),
            MOUNT_OPTS_FSCACHE.to_string(),
            build_mount_options(false, false),
            build_mount_options(true, false),
            build_mount_options(false, true),
            build_mount_options(true, true),
        ];
        for opts in options {
            assert!(!opts.contains("vers="), "挂载参数不应固定协议版本: {opts}");
            assert!(!opts.contains("nfsvers="), "挂载参数不应固定协议版本: {opts}");
        }
    }

    #[test]
    fn mount_cmd_never_pins_protocol_version() {
        for fscache in [false, true] {
            for read_only in [false, true] {
                let cmd = build_mount_cmd(
                    "/mnt/model_hub",
                    "10.0.0.9:/share/v1",
                    read_only,
                    fscache,
                );
                assert!(
                    !cmd.contains("vers="),
                    "挂载命令不应固定协议版本（fscache={fscache}, read_only={read_only}）: {cmd}"
                );
                assert!(cmd.contains("mount -t nfs -o "), "{cmd}");
            }
        }
    }

    #[test]
    fn mount_cmd_recovers_stale_mountpoint() {
        let cmd = build_mount_cmd("/mnt/model_hub", "10.0.0.9:/share/v1", false, false);
        // 失效挂载点的恢复路径：创建目录失败 → 惰性卸载 → 重建目录
        assert!(cmd.contains("if ! mkdir -p '/mnt/model_hub'"), "{cmd}");
        assert!(cmd.contains("STALE_MOUNT_RECOVER"), "{cmd}");
        assert!(cmd.contains("umount -l '/mnt/model_hub'"), "{cmd}");
        // 卸载必须先于重建目录（先摘除失效挂载点，再 mkdir）
        let umount_at = cmd.find("umount -l").unwrap();
        let recover_mkdir_at = cmd[umount_at..].find("mkdir -p").unwrap() + umount_at;
        assert!(umount_at < recover_mkdir_at, "应先 umount 再 mkdir: {cmd}");
        // 正常挂载路径保持幂等语义
        assert!(cmd.contains("ALREADY_MOUNTED"), "{cmd}");
        assert!(cmd.contains("mount -t nfs -o "), "{cmd}");
        assert!(cmd.contains("MOUNT_OK"), "{cmd}");
    }

    #[tokio::test]
    async fn mount_uses_per_node_options() {
        use crate::ssh::MockSshExec;
        let ssh = MockSshExec::new();
        let mut plain = mount_node("1", "n1", "10.0.0.1");
        plain.fscache = false;
        let mut cached = mount_node("2", "n2", "10.0.0.2");
        cached.fscache = true;
        let nodes = vec![plain, cached];
        mount_on_nodes(&ssh, &resolved_sample(), &nodes, "/mnt/model_hub")
            .await
            .unwrap();
        let calls = ssh.recorded_calls();
        let plain_cmd = calls.iter().find(|c| c.starts_with("10.0.0.1:")).unwrap();
        let cached_cmd = calls.iter().find(|c| c.starts_with("10.0.0.2:")).unwrap();
        assert!(plain_cmd.contains(MOUNT_OPTS_PLAIN), "普通节点应使用普通参数: {plain_cmd}");
        assert!(!plain_cmd.contains("fsc"), "普通节点不应含 fsc: {plain_cmd}");
        assert!(cached_cmd.contains(MOUNT_OPTS_FSCACHE), "fscache 节点应使用 fscache 参数: {cached_cmd}");
        // 两条实际下发的命令都不得固定 NFS 协议版本
        for cmd in [plain_cmd, cached_cmd] {
            assert!(!cmd.contains("vers="), "挂载命令不应固定协议版本: {cmd}");
            assert!(!cmd.contains("nfsvers="), "挂载命令不应固定协议版本: {cmd}");
        }
    }

    fn resolved_sample() -> NfsResolveResult {
        NfsResolveResult {
            config_name: MODEL_HUB_CONFIG.into(),
            host_path: "/share/v1/model_hub".into(),
            volume_uuid: "v1".into(),
            volume_name: "v1".into(),
            nfs_server: "10.0.0.9".into(),
            nfs_export_path: "/share/v1".into(),
            sub_path: "model_hub".into(),
            read_only: false,
            mounted: true,
            mount_source: "10.0.0.9:/share/v1".into(),
        }
    }
}
