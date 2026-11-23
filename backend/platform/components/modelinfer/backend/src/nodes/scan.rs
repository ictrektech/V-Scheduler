//! 节点硬件扫描：hostname / CPU 架构 / 内核 / 网关 / 网卡 / IB / GPU
//!
//! 解析逻辑为纯函数，便于单测；SSH 交互经 [`SshExec`] 注入。
//!
//! GPU 探测按 `nvidia → amd → intel` 顺序，**取第一个命中的厂商**（一个节点至多一个厂商），
//! 并采集该厂商的**全部** GPU。探测命令失败 / 无输出 MUST 视为「无 GPU」，不影响节点可添加。

use serde::{Deserialize, Serialize};

use crate::error::{ModelinferError, codes};
use crate::ssh::{SshExec, SshTarget};

/// 单个 IPv4 网卡
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceInfo {
    pub name: String,
    pub ips: Vec<String>,
}

/// 单张 GPU（同一厂商内逐卡记录）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuDevice {
    /// 设备序号（厂商工具给出的 index，从 0 起）
    pub index: u32,
    /// 型号名称
    pub name: String,
    /// 显存（MiB；未知为 0）
    pub memory_total_mib: u64,
    /// PCI 总线地址（未知为空串）
    pub pci_bus_id: String,
}

/// GPU 汇总（一个节点至多一个厂商）
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GpuInfo {
    /// 厂商：`nvidia` / `amd` / `intel`；未识别为空串
    pub vendor: String,
    /// 驱动版本（未知为空串）
    pub driver_version: String,
    /// 全部 GPU（同厂商）
    pub devices: Vec<GpuDevice>,
}

impl GpuInfo {
    /// 未识别到 GPU
    pub fn none() -> Self {
        Self::default()
    }

    /// 型号汇总：单卡取 name；多卡取 `<name> x N`
    pub fn model_summary(&self) -> String {
        summarize_gpu_model(&self.devices)
    }
}

/// 扫描结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeScanResult {
    pub hostname: String,
    /// CPU 架构（`uname -m`，如 `aarch64` / `x86_64`）
    pub arch: String,
    /// 内核版本（`uname -r`）
    pub kernel_version: String,
    /// 默认网关（无则空串）
    pub gateway: String,
    pub interfaces: Vec<InterfaceInfo>,
    pub default_socket_ifname: Option<String>,
    pub ib_devices: Vec<String>,
    pub suggested_ib_hca: Option<String>,
    /// GPU 汇总（可能为空）
    #[serde(default)]
    pub gpu: GpuInfo,
}

/// 解析 `ip -o -4 addr show` 输出
pub fn parse_ip_addr(output: &str) -> Vec<InterfaceInfo> {
    let mut out: Vec<InterfaceInfo> = Vec::new();
    for line in output.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 4 {
            continue;
        }
        // 形如：`2: enp1s0    inet 10.100.10.21/24 ...`
        let name = tokens[1].trim_end_matches(':').to_string();
        if name == "lo" {
            continue;
        }
        let Some(pos) = tokens.iter().position(|t| *t == "inet") else {
            continue;
        };
        let Some(addr) = tokens.get(pos + 1) else {
            continue;
        };
        let ip = addr.split('/').next().unwrap_or("").to_string();
        if ip.is_empty() {
            continue;
        }
        match out.iter_mut().find(|i| i.name == name) {
            Some(existing) => existing.ips.push(ip),
            None => out.push(InterfaceInfo { name, ips: vec![ip] }),
        }
    }
    out
}

/// 解析 `ls /sys/class/infiniband` 输出
pub fn parse_ib_devices(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// 解析「netdev → ibdev」映射输出（每行 `<netdev> <ibdev>`）
///
/// 采集命令遍历 `/sys/class/infiniband/<ibdev>/device/net/<netdev>`，输出形如：
///
/// ```text
/// enP2p1s0f1np1 roceP2p1s0f1
/// ```
///
/// 用于把控制面网卡（承载节点注册 IP 的网卡）与其 RoCE IB 设备关联起来。多行时保留
/// 出现顺序；非法行（字段数不对）忽略。
pub fn parse_ib_netdev_map(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let netdev = it.next()?;
            let ibdev = it.next()?;
            if netdev.is_empty() || ibdev.is_empty() {
                return None;
            }
            Some((netdev.to_string(), ibdev.to_string()))
        })
        .collect()
}

/// 选择包含目标 IP 的网卡（无匹配返回 None）
pub fn pick_socket_ifname(ip: &str, interfaces: &[InterfaceInfo]) -> Option<String> {
    let ip = ip.trim();
    interfaces
        .iter()
        .find(|i| i.ips.iter().any(|x| x == ip))
        .map(|i| i.name.clone())
}

/// 推断建议的 IB HCA：优先返回与控制面网卡（承载节点注册 IP 的网卡）关联的 IB 设备，
/// 无法关联时回退到首个 IB 设备。
///
/// 背景：`/sys/class/infiniband` 的首项在 DGX Spark / GB10 上可能是**未启用**的
/// `rocep1s0f0`，而 10.10.11 织物实际使用 `roceP2p1s0f1`（netdev `enP2p1s0f1np1`）。
/// 该值会被注入 `NCCL_IB_HCA`，取错设备会破坏多节点通信。
pub fn pick_ib_hca(
    socket_ifname: Option<&str>,
    netdev_map: &[(String, String)],
    ib_devices: &[String],
) -> Option<String> {
    if ib_devices.is_empty() {
        return None;
    }
    if let Some(iface) = socket_ifname
        && let Some((_, ibdev)) = netdev_map.iter().find(|(netdev, _)| netdev == iface)
    {
        return Some(ibdev.clone());
    }
    ib_devices.first().cloned()
}

/// 从数字 / 带单位的字段中提取整数（`8192` / `8192 MiB` / `8192.5`）
fn parse_u64_field(raw: &str) -> u64 {
    let s = raw.trim();
    let mut num = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() || (ch == '.' && !num.contains('.')) {
            num.push(ch);
        } else if !num.is_empty() {
            break;
        }
    }
    num.parse::<f64>().map(|v| v as u64).unwrap_or(0)
}

/// 解析 `nvidia-smi --query-gpu=index,name,memory.total,driver_version --format=csv,noheader,nounits`
/// 输出（兼容带第 5 列 `pci_bus_id` 的输出）。
///
/// 返回 `(driver_version, devices)`；非法行忽略，空输出 → 空设备。
/// 注意：Jetson（Orin 等）的 `nvidia-smi` **不支持** `pci_bus_id` 查询字段，故默认不查该列，
/// 解析时 `pci_bus_id` 允许缺失（为空串）。
pub fn parse_nvidia_smi_csv(output: &str) -> (String, Vec<GpuDevice>) {
    let mut driver = String::new();
    let mut devices = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() < 4 {
            continue;
        }
        let index = parse_u64_field(fields[0]) as u32;
        let name = fields[1].to_string();
        let memory_total_mib = parse_u64_field(fields[2]);
        let drv = fields[3].to_string();
        // 第 5 列为可选 PCI 地址（Jetson 无）
        let pci = fields.get(4).map(|s| s.to_string()).unwrap_or_default();
        if driver.is_empty() && !drv.is_empty() {
            driver = drv;
        }
        devices.push(GpuDevice {
            index,
            name,
            memory_total_mib,
            pci_bus_id: pci,
        });
    }
    (driver, devices)
}

/// 解析 `nvidia-smi -L` 输出（Jetson 等不支持 `--query-gpu` 时的回退）。
///
/// 形如 `GPU 0: Orin (nvgpu) (UUID: 9f16fab1-...)` → `GpuDevice { index, name: "Orin (nvgpu)" }`。
pub fn parse_nvidia_list(output: &str) -> Vec<GpuDevice> {
    let mut devices = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("GPU ") else {
            continue;
        };
        let Some((idx_raw, tail)) = rest.split_once(':') else {
            continue;
        };
        let index = parse_u64_field(idx_raw) as u32;
        // 去掉尾部 " (UUID: ...)"
        let name = match tail.trim().rfind(" (UUID:") {
            Some(pos) => tail.trim()[..pos].trim(),
            None => tail.trim(),
        };
        if name.is_empty() {
            continue;
        }
        devices.push(GpuDevice {
            index,
            name: name.to_string(),
            memory_total_mib: 0,
            pci_bus_id: String::new(),
        });
    }
    devices
}

/// 解析 `rocm-smi --showproductname`（best-effort）：提取 `Card series` / `Card model` 行。
pub fn parse_rocm_smi(output: &str) -> Vec<GpuDevice> {
    let mut devices: Vec<GpuDevice> = Vec::new();
    let mut idx = 0u32;
    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        if !(lower.contains("card series") || lower.contains("card model")) {
            continue;
        }
        let Some(value) = line.rsplit_once(':').map(|(_, v)| v.trim()) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        devices.push(GpuDevice {
            index: idx,
            name: value.to_string(),
            memory_total_mib: 0,
            pci_bus_id: String::new(),
        });
        idx += 1;
    }
    devices
}

/// 解析 `xpu-smi discovery`（best-effort）：提取 `Device Name` 行。
pub fn parse_xpu_smi(output: &str) -> Vec<GpuDevice> {
    let mut devices: Vec<GpuDevice> = Vec::new();
    let mut idx = 0u32;
    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if !lower.starts_with("device name") {
            continue;
        }
        let Some(value) = trimmed.split_once(':').map(|(_, v)| v.trim()) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        devices.push(GpuDevice {
            index: idx,
            name: value.to_string(),
            memory_total_mib: 0,
            pci_bus_id: String::new(),
        });
        idx += 1;
    }
    devices
}

/// GPU 型号汇总：单卡取 name；多卡取 `<name> x N`
pub fn summarize_gpu_model(devices: &[GpuDevice]) -> String {
    match devices.first() {
        None => String::new(),
        Some(first) if devices.len() == 1 => first.name.clone(),
        Some(first) => format!("{} x {}", first.name, devices.len()),
    }
}

/// 归一 CPU 架构（`aarch64` / `x86_64` 原样返回；其余小写）
pub fn normalize_arch(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

/// 经 SSH 扫描节点
pub async fn scan_node(ssh: &dyn SshExec, target: &SshTarget) -> Result<NodeScanResult, ModelinferError> {
    let hostname = run_trimmed(ssh, target, "hostname").await?;
    let arch = normalize_arch(&run_trimmed(ssh, target, "uname -m").await?);
    let kernel_version = run_trimmed(ssh, target, "uname -r").await?;
    let ip_out = ssh.run(target, "ip -o -4 addr show").await?;
    if ip_out.exit_code != 0 {
        return Err(ModelinferError::new(
            codes::NODE_SCAN_FAILED,
            format!("ip 命令失败: {}", ip_out.stderr.trim()),
        ));
    }
    let interfaces = parse_ip_addr(&ip_out.stdout);
    // 无 IB 时 `ls` 返回非零，视为空
    let ib_out = ssh.run(target, "ls /sys/class/infiniband 2>/dev/null || true").await?;
    let ib_devices = parse_ib_devices(&ib_out.stdout);
    // netdev → ibdev 映射（依据 `/sys/class/infiniband/<dev>/device/net/<netdev>`）
    // 该命令失败不应影响扫描主体，缺省为空映射（回退首设备）
    let netdev_map = match ssh.run(target, IB_NETDEV_MAP_CMD).await {
        Ok(o) => parse_ib_netdev_map(&o.stdout),
        Err(_) => Vec::new(),
    };
    // 默认网关（best-effort；失败为空）
    let gateway = match ssh.run(target, DEFAULT_GATEWAY_CMD).await {
        Ok(o) => crate::images::parse_default_gateway(&o.stdout).unwrap_or_default(),
        Err(_) => String::new(),
    };
    // GPU（best-effort；探测失败视为无 GPU）
    let gpu = detect_gpu(ssh, target).await;

    let default_socket_ifname = pick_socket_ifname(&target.host, &interfaces);
    let suggested_ib_hca = pick_ib_hca(default_socket_ifname.as_deref(), &netdev_map, &ib_devices);
    Ok(NodeScanResult {
        hostname,
        arch,
        kernel_version,
        gateway,
        interfaces,
        default_socket_ifname,
        ib_devices,
        suggested_ib_hca,
        gpu,
    })
}

/// 默认路由采集命令（JSON 优先，回退文本）
const DEFAULT_GATEWAY_CMD: &str = "ip -j route show default 2>/dev/null || ip route show default";

/// NVIDIA GPU 查询命令（默认不查 `pci_bus_id`——Jetson 不支持该字段；未安装 / 不支持 → 回退 `-L`）
const NVIDIA_QUERY_CMD: &str =
    "nvidia-smi --query-gpu=index,name,memory.total,driver_version --format=csv,noheader,nounits 2>/dev/null";
/// NVIDIA GPU 列表回退命令（`--query-gpu` 不可用时）
const NVIDIA_LIST_CMD: &str = "nvidia-smi -L 2>/dev/null";
/// AMD GPU 查询命令（best-effort）
const AMD_QUERY_CMD: &str = "rocm-smi --showproductname 2>/dev/null";
/// Intel GPU 查询命令（best-effort）
const INTEL_QUERY_CMD: &str = "xpu-smi discovery 2>/dev/null";

/// GPU 探测：`nvidia → amd → intel`，取第一个命中的厂商（同厂商全部设备）。
///
/// 探测命令失败 / 无输出 → 继续下一个厂商；全部未命中 → [`GpuInfo::none`]。
pub async fn detect_gpu(ssh: &dyn SshExec, target: &SshTarget) -> GpuInfo {
    if let Some(gpu) = try_vendor(ssh, target, NVIDIA_QUERY_CMD, "nvidia", |out| {
        let (driver, devices) = parse_nvidia_smi_csv(out);
        (driver, devices)
    })
    .await
    {
        return gpu;
    }
    // 回退：Jetson 等不支持 `--query-gpu` 时用 `nvidia-smi -L`（PCI / 显存 / 驱动缺失）
    if let Some(gpu) = try_vendor(ssh, target, NVIDIA_LIST_CMD, "nvidia", |out| {
        (String::new(), parse_nvidia_list(out))
    })
    .await
    {
        return gpu;
    }
    if let Some(gpu) = try_vendor(ssh, target, AMD_QUERY_CMD, "amd", |out| {
        (String::new(), parse_rocm_smi(out))
    })
    .await
    {
        return gpu;
    }
    if let Some(gpu) = try_vendor(ssh, target, INTEL_QUERY_CMD, "intel", |out| {
        (String::new(), parse_xpu_smi(out))
    })
    .await
    {
        return gpu;
    }
    GpuInfo::none()
}

/// 尝试单个厂商：命令成功且解析出设备才算命中
async fn try_vendor(
    ssh: &dyn SshExec,
    target: &SshTarget,
    cmd: &str,
    vendor: &str,
    parse: impl Fn(&str) -> (String, Vec<GpuDevice>),
) -> Option<GpuInfo> {
    let out = ssh.run(target, cmd).await.ok()?;
    if out.exit_code != 0 || out.stdout.trim().is_empty() {
        return None;
    }
    let (driver_version, devices) = parse(&out.stdout);
    if devices.is_empty() {
        return None;
    }
    Some(GpuInfo {
        vendor: vendor.to_string(),
        driver_version,
        devices,
    })
}

/// 采集 `/sys/class/infiniband/<ibdev>/device/net/<netdev>` 的映射（纯 shell，无外部依赖）
const IB_NETDEV_MAP_CMD: &str = "for d in /sys/class/infiniband/*; do dev=${d##*/}; \
for n in \"$d\"/device/net/*; do [ -e \"$n\" ] && printf '%s %s\\n' \"${n##*/}\" \"$dev\"; done; done";

async fn run_trimmed(
    ssh: &dyn SshExec,
    target: &SshTarget,
    cmd: &str,
) -> Result<String, ModelinferError> {
    let out = ssh.run(target, cmd).await?;
    if out.exit_code != 0 {
        return Err(ModelinferError::new(
            codes::NODE_SCAN_FAILED,
            format!("`{cmd}` 失败: {}", out.stderr.trim()),
        ));
    }
    Ok(out.stdout.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::MockSshExec;

    const IP_OUT: &str = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever\n2: enp1s0    inet 10.100.10.21/24 brd 10.100.10.255 scope global enp1s0\n3: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0\n";

    #[test]
    fn parse_and_pick() {
        let ifaces = parse_ip_addr(IP_OUT);
        assert_eq!(ifaces.len(), 2);
        assert_eq!(pick_socket_ifname("10.100.10.21", &ifaces).as_deref(), Some("enp1s0"));
        assert_eq!(pick_socket_ifname("10.9.9.9", &ifaces), None);
    }

    #[test]
    fn parse_ib() {
        assert_eq!(parse_ib_devices("mlx5_0\nmlx5_1\n"), vec!["mlx5_0", "mlx5_1"]);
        assert!(parse_ib_devices("").is_empty());
    }

    #[test]
    fn parse_ib_netdev_map_ok() {
        let m = parse_ib_netdev_map("enP2p1s0f1np1 roceP2p1s0f1\n");
        assert_eq!(m, vec![("enP2p1s0f1np1".to_string(), "roceP2p1s0f1".to_string())]);
        assert!(parse_ib_netdev_map("").is_empty());
        // 非法行（字段数不足）忽略
        assert!(parse_ib_netdev_map("bogus\n").is_empty());
    }

    #[test]
    fn pick_ib_hca_prefers_socket_mapping() {
        // GB10 实况：首设备是 DOWN 的 rocep1s0f0，控制面网卡关联 roceP2p1s0f1
        let devices = vec![
            "rocep1s0f0".to_string(),
            "rocep1s0f1".to_string(),
            "roceP2p1s0f0".to_string(),
            "roceP2p1s0f1".to_string(),
        ];
        let map = vec![
            ("enP2p1s0f1np1".to_string(), "roceP2p1s0f1".to_string()),
            ("enP2p1s0f0np0".to_string(), "roceP2p1s0f0".to_string()),
        ];
        // 命中控制面网卡 → 关联设备（而非首设备）
        assert_eq!(
            pick_ib_hca(Some("enP2p1s0f1np1"), &map, &devices).as_deref(),
            Some("roceP2p1s0f1")
        );
        // 未命中映射 → 回退首设备
        assert_eq!(
            pick_ib_hca(Some("enp1s0"), &map, &devices).as_deref(),
            Some("rocep1s0f0")
        );
        // 无控制面网卡 → 回退首设备
        assert_eq!(pick_ib_hca(None, &map, &devices).as_deref(), Some("rocep1s0f0"));
        // 无 IB 设备 → None
        assert_eq!(pick_ib_hca(Some("enP2p1s0f1np1"), &map, &[]), None);
    }

    #[test]
    fn normalize_arch_variants() {
        assert_eq!(normalize_arch("aarch64"), "aarch64");
        assert_eq!(normalize_arch("x86_64\n"), "x86_64");
        assert_eq!(normalize_arch("  AMD64 "), "amd64");
    }

    #[test]
    fn parse_nvidia_smi_single_and_multi() {
        // 单卡（Orin，显存无单位）
        let (drv, devs) = parse_nvidia_smi_csv("0, Orin, 7620, 540.4.0, 0000:01:00.0\n");
        assert_eq!(drv, "540.4.0");
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].name, "Orin");
        assert_eq!(devs[0].memory_total_mib, 7620);
        assert_eq!(devs[0].pci_bus_id, "0000:01:00.0");

        // 多卡 + 带单位 + 空行
        let out = "0, NVIDIA H100 80GB HBM3, 81559 MiB, 550.54.14, 0000:01:00.0\n\
                   \n\
                   1, NVIDIA H100 80GB HBM3, 81559 MiB, 550.54.14, 0000:02:00.0\n";
        let (drv, devs) = parse_nvidia_smi_csv(out);
        assert_eq!(drv, "550.54.14");
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[1].index, 1);
        assert_eq!(summarize_gpu_model(&devs), "NVIDIA H100 80GB HBM3 x 2");

        // 空输出
        let (drv, devs) = parse_nvidia_smi_csv("");
        assert!(drv.is_empty());
        assert!(devs.is_empty());
        assert_eq!(summarize_gpu_model(&devs), "");

        // 4 列输出（无 PCI，Jetson 实况；显存 [N/A] → 0）
        let (drv, devs) = parse_nvidia_smi_csv("0, Orin (nvgpu), [N/A], 540.4.0\n");
        assert_eq!(drv, "540.4.0");
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].name, "Orin (nvgpu)");
        assert_eq!(devs[0].memory_total_mib, 0);
        assert!(devs[0].pci_bus_id.is_empty());
    }

    #[test]
    fn parse_nvidia_list_fallback() {
        let out = "GPU 0: Orin (nvgpu) (UUID: 9f16fab1-7bbd-53ab-87d5-d7d7bf6c697c)\n";
        let devs = parse_nvidia_list(out);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].index, 0);
        assert_eq!(devs[0].name, "Orin (nvgpu)");
        assert!(parse_nvidia_list("").is_empty());
        assert!(parse_nvidia_list("no gpus\n").is_empty());
    }

    #[test]
    fn parse_rocm_and_xpu_best_effort() {
        let rocm = "======================= ROCm System Management Interface =======================\n\
                    GPU[0]\t\t: Card series: \t\tRadeon Instinct MI250X\n\
                    GPU[0]\t\t: Card model: \t\t0x0c34\n";
        let devs = parse_rocm_smi(rocm);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].name, "Radeon Instinct MI250X");

        let xpu = "Device 0:\n    Device Name: Intel(R) Data Center GPU Max 1550\n";
        let devs = parse_xpu_smi(xpu);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].name, "Intel(R) Data Center GPU Max 1550");

        assert!(parse_rocm_smi("nothing here").is_empty());
        assert!(parse_xpu_smi("").is_empty());
    }

    #[tokio::test]
    async fn scan_with_mock() {
        let ssh = MockSshExec::new()
            .with_rule("hostname", 0, "node-21\n")
            .with_rule("uname -m", 0, "x86_64\n")
            .with_rule("uname -r", 0, "5.15.0-generic\n")
            .with_rule("ip -o -4", 0, IP_OUT)
            .with_rule("infiniband", 0, "")
            .with_rule("route show default", 0, "default via 10.100.10.1 dev enp1s0\n")
            .with_rule("nvidia-smi", 1, "");
        let target = SshTarget::new("10.100.10.21", 22, "root", "x");
        let r = scan_node(&ssh, &target).await.unwrap();
        assert_eq!(r.hostname, "node-21");
        assert_eq!(r.arch, "x86_64");
        assert_eq!(r.kernel_version, "5.15.0-generic");
        assert_eq!(r.gateway, "10.100.10.1");
        assert_eq!(r.default_socket_ifname.as_deref(), Some("enp1s0"));
        assert!(r.ib_devices.is_empty());
        assert_eq!(r.gpu, GpuInfo::none());
    }

    #[tokio::test]
    async fn scan_detects_nvidia_and_ignores_others() {
        let ssh = MockSshExec::new()
            .with_rule("hostname", 0, "E1001NX\n")
            .with_rule("uname -m", 0, "aarch64\n")
            .with_rule("uname -r", 0, "5.10.104-tegra\n")
            .with_rule("ip -o -4", 0, "2: enP8p1s0    inet 192.168.8.100/24 scope global enP8p1s0\n")
            .with_rule("infiniband", 0, "")
            .with_rule("route show default", 0, "default via 192.168.8.254 dev enP8p1s0\n")
            .with_rule("nvidia-smi", 0, "0, Orin (nvgpu), [N/A], 540.4.0\n")
            // AMD 命令即便有输出也不应被采用（首个命中即返回）
            .with_rule("rocm-smi", 0, "Card series: Radeon\n");
        let target = SshTarget::new("192.168.8.100", 22, "root", "x");
        let r = scan_node(&ssh, &target).await.unwrap();
        assert_eq!(r.arch, "aarch64");
        assert_eq!(r.gateway, "192.168.8.254");
        assert_eq!(r.gpu.vendor, "nvidia");
        assert_eq!(r.gpu.devices.len(), 1);
        assert_eq!(r.gpu.model_summary(), "Orin (nvgpu)");
    }

    #[tokio::test]
    async fn scan_with_mock_maps_ib_hca_to_socket_ifname() {
        let ip_out = "2: enP2p1s0f1np1    inet 10.10.11.154/24 brd 10.10.11.255 scope global enP2p1s0f1np1\n";
        let map_out = "enP2p1s0f0np0 roceP2p1s0f0\nenP2p1s0f1np1 roceP2p1s0f1\n";
        let ssh = MockSshExec::new()
            .with_rule("hostname", 0, "spark-90f4\n")
            .with_rule("uname -m", 0, "aarch64\n")
            .with_rule("uname -r", 0, "6.8.0\n")
            .with_rule("ip -o -4", 0, ip_out)
            .with_rule("ls /sys/class/infiniband", 0, "rocep1s0f0\nroceP2p1s0f1\n")
            .with_rule("device/net", 0, map_out)
            .with_rule("nvidia-smi", 1, "");
        let target = SshTarget::new("10.10.11.154", 22, "root", "x");
        let r = scan_node(&ssh, &target).await.unwrap();
        assert_eq!(r.default_socket_ifname.as_deref(), Some("enP2p1s0f1np1"));
        assert_eq!(r.suggested_ib_hca.as_deref(), Some("roceP2p1s0f1"));
    }

    /// GPU 探测命令失败 / 无输出不得使扫描失败
    #[tokio::test]
    async fn scan_succeeds_without_gpu_tools() {
        let ssh = MockSshExec::new()
            .with_rule("hostname", 0, "cpu-only\n")
            .with_rule("uname -m", 0, "x86_64\n")
            .with_rule("uname -r", 0, "6.1.0\n")
            .with_rule("ip -o -4", 0, "")
            .with_rule("infiniband", 0, "")
            .with_rule("nvidia-smi", 127, "")
            .with_rule("rocm-smi", 127, "")
            .with_rule("xpu-smi", 127, "");
        let target = SshTarget::new("10.0.0.9", 22, "root", "x");
        let r = scan_node(&ssh, &target).await.unwrap();
        assert_eq!(r.gpu, GpuInfo::none());
        assert!(r.gpu.model_summary().is_empty());
    }
}