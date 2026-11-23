//! 启动前镜像预检与 image-hub 分发
//!
//! 依据卡片 `manifest.images` 逐节点检查镜像是否已就绪；缺失时确认 image-hub registry 上存在，
//! 再经内置 `image-hub.sh` 在对应节点拉取；hub 亦不具备时中止启动。
//!
//! 设计见 `openspec/changes/modelinfer-image-distribution/design.md`（D-MI-ID-01~09）：
//! - 只管理 `manifest.images`，忽略 `configs` 中的镜像类参数；
//! - image-hub 地址按节点解析（显式覆盖 → 本机路由源地址同网段 → 平台设备同网段 → 节点默认网关
//!   → 首个可用本机 IP）；
//! - 本机路由源地址用标准库 UDP `connect` 探测（等价 `ip -j route get <dst>` 的 prefsrc，
//!   不引入 `iproute2` 依赖）；
//! - hub 存在性走 registry v2 只读 `HEAD /v2/<repo>/manifests/<tag>`。

use std::collections::BTreeMap;
use std::net::{IpAddr, UdpSocket};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::deploy::script::sh_quote;
use crate::deploy::{DeployNode, DeployRuntime};
use crate::error::{ModelinferError, codes};
use crate::nfs::is_usable_ip;
use crate::ssh::{SshExec, SshTarget};

/// 内置 `image-hub.sh`（与 `components/image-hub/frontend/public/image-hub.sh` 逐字节一致，
/// 经 `scripts/sync-image-hub.sh` 同步；单测校验关键标记，漂移可被发现）
pub const IMAGE_HUB_SH: &str = include_str!("../../assets/image-hub.sh");

/// 单节点单次镜像拉取命令超时（秒）：大镜像可达数十 GB，按小时起步（MB-BD-011）
pub const IMAGE_PULL_TIMEOUT_SECS: u64 = 3600;
/// `ensure-images` activity 的 start_to_close 超时（秒）：覆盖多节点大镜像并行拉取的上界
pub const IMAGE_ENSURE_ACTIVITY_TIMEOUT_SECS: u64 = 7200;
/// 节点镜像存在性与架构探测等短命令超时（秒）
const IMAGE_PROBE_TIMEOUT_SECS: u64 = 120;
/// 默认 registry 端口
pub const DEFAULT_IMAGE_HUB_PORT: u16 = 19913;
/// 镜像分发默认节点级并行度（0 = 不限制 / 全部节点并行）
pub const DEFAULT_IMAGE_DISTRIBUTION_CONCURRENCY: usize = 4;

/// image-hub 配置
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageHubConfig {
    /// registry 地址 host 覆盖（空 = 按节点自动解析）
    pub server: String,
    /// 管理进程访问宿主机 registry 的内部地址，节点拉取仍自动选择设备地址。
    pub probe_host: String,
    /// registry 端口
    pub port: u16,
    /// 镜像分发节点级并行度（0 = 不限制 / 全部节点并行）
    pub concurrency: usize,
}

impl Default for ImageHubConfig {
    fn default() -> Self {
        Self {
            server: String::new(),
            probe_host: String::new(),
            port: DEFAULT_IMAGE_HUB_PORT,
            concurrency: DEFAULT_IMAGE_DISTRIBUTION_CONCURRENCY,
        }
    }
}

impl ImageHubConfig {
    pub fn new(server: impl Into<String>, port: u16) -> Self {
        Self {
            server: server.into(),
            probe_host: String::new(),
            port: if port == 0 { DEFAULT_IMAGE_HUB_PORT } else { port },
            concurrency: DEFAULT_IMAGE_DISTRIBUTION_CONCURRENCY,
        }
    }

    /// 覆盖节点级并行度（0 = 不限制）
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }

    /// 组装 `host:port`
    pub fn addr(&self, host: &str) -> String {
        format!("{}:{}", host.trim(), self.port)
    }

    /// hub 存在性检查使用的基地址（同宿主默认 `127.0.0.1`）
    pub fn registry_base(&self) -> String {
        let host = [&self.server, &self.probe_host].into_iter()
            .map(|host| host.trim()).find(|host| !host.is_empty()).unwrap_or("127.0.0.1");
        format!("http://{}:{}", host, self.port)
    }
}

/// 实际并行度（纯函数）：配置 `0` = 不限制（取节点数，至少 1）；否则 `max(1)` 且不超过节点数
pub fn effective_concurrency(configured: usize, total_nodes: usize) -> usize {
    let total = total_nodes.max(1);
    if configured == 0 {
        total
    } else {
        configured.max(1).min(total)
    }
}

/// 归一后的镜像引用
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    /// 卡片中的原始声明
    pub raw: String,
    /// 归一仓库名（含 registry host，如 `docker.io/library/alpine`）
    pub repository: String,
    /// tag（缺省 `latest`）
    pub tag: String,
    /// 声明的适用架构（空 = 通用）
    pub arch: String,
}

impl ImageRef {
    /// 上游完整引用 `<repository>:<tag>`
    pub fn full(&self) -> String {
        format!("{}:{}", self.repository, self.tag)
    }
}

/// 归一 CPU 架构（`aarch64`→`arm64`、`x86_64`→`amd64`；其余小写原样）
pub fn normalize_arch(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "aarch64" | "arm64" => "arm64".to_string(),
        "x86_64" | "amd64" | "x86-64" => "amd64".to_string(),
        other => other.to_string(),
    }
}

/// 归一镜像引用（与 `image-hub.sh#parse_image_ref` 同规则），返回 `(repository, tag)`
///
/// - 缺省 registry `docker.io`；单段名补 `library/`；tag 缺省 `latest`；
/// - 不支持 `@digest`。
pub fn normalize_image_ref(raw: &str) -> Result<(String, String), ModelinferError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(invalid_image(raw, "镜像引用为空"));
    }
    if s.contains('@') || s.chars().any(char::is_whitespace) {
        return Err(invalid_image(raw, "不支持 @digest 或含空白"));
    }

    // 解析 tag（`:tag` 且冒号后无 `/`）
    let (name_part, tag) = match s.rsplit_once(':') {
        Some((name, after)) if !after.contains('/') => (name.to_string(), after.to_string()),
        _ => (s.to_string(), "latest".to_string()),
    };
    if name_part.is_empty() || !tag.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        || tag.is_empty()
        || tag.len() > 128
    {
        return Err(invalid_image(raw, "镜像 tag 非法"));
    }

    // registry / repo
    let (registry, repo) = match name_part.split_once('/') {
        Some((first, rest)) => {
            if first.is_empty() || rest.is_empty() {
                return Err(invalid_image(raw, "镜像名路径非法"));
            }
            let first_lc = first.to_ascii_lowercase();
            if first.contains('.') || first.contains(':') || first_lc == "localhost" {
                (first_lc, rest.to_string())
            } else {
                ("docker.io".to_string(), name_part.clone())
            }
        }
        None => ("docker.io".to_string(), format!("library/{name_part}")),
    };

    if repo.starts_with('/')
        || repo.ends_with('/')
        || repo.contains("..")
        || repo.contains("//")
        || !repo
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return Err(invalid_image(raw, "镜像仓库名非法"));
    }

    Ok((format!("{registry}/{repo}"), tag))
}

fn invalid_image(raw: &str, why: &str) -> ModelinferError {
    ModelinferError::new(codes::MANIFEST_INVALID, format!("镜像引用非法（{why}）: {raw}"))
}

/// 从卡片 `manifest.images` 原始条目解析指定架构下的镜像列表（纯函数）
///
/// 成员为字符串（全架构通用）或对象 `{ name, arch? }`；`arch` 缺省表示通用，
/// 否则仅在归一后与目标架构一致时纳入。
pub fn images_for_arch(raw_images: &[Value], arch: &str) -> Result<Vec<ImageRef>, ModelinferError> {
    let target = normalize_arch(arch);
    let mut out = Vec::new();
    for item in raw_images {
        let (raw_name, item_arch) = match item {
            Value::String(s) => (s.clone(), String::new()),
            Value::Object(map) => {
                let name = map
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid_image("(object)", "对象成员缺少 name"))?
                    .to_string();
                let a = map
                    .get("arch")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                (name, a)
            }
            other => {
                return Err(ModelinferError::new(
                    codes::MANIFEST_INVALID,
                    format!("manifest.images 成员必须是字符串或对象: {other}"),
                ));
            }
        };
        if !item_arch.trim().is_empty() && normalize_arch(&item_arch) != target {
            continue;
        }
        let (repository, tag) = normalize_image_ref(&raw_name)?;
        out.push(ImageRef {
            raw: raw_name,
            repository,
            tag,
            arch: item_arch,
        });
    }
    Ok(out)
}

/// 取 `manifest.images` 原始数组（缺失 / 非法时返回错误）
pub fn manifest_images(manifest: &Value) -> Result<Vec<Value>, ModelinferError> {
    match manifest.get("images") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => Ok(items.clone()),
        Some(_) => Err(ModelinferError::new(
            codes::MANIFEST_INVALID,
            "manifest.images 必须是数组",
        )),
    }
}

/// 生效镜像（纯函数）：运行配置覆盖非空时**整体替换**卡片声明，否则使用卡片声明
///
/// 预检 / 分发（[`ensure_images`]）与生命周期环境变量注入 MUST 使用同一份生效镜像，
/// 保证「分发的镜像」与「卡片看到的镜像」严格一致。
/// 设计见 `specs/docs/design/modelinfer/runtime-overrides.md`。
pub fn effective_images(card_images: &[Value], override_images: &[Value]) -> Vec<Value> {
    if override_images.is_empty() {
        card_images.to_vec()
    } else {
        override_images.to_vec()
    }
}

/// 生效镜像的来源标识：`card` / `override`
pub fn images_source(override_images: &[Value]) -> &'static str {
    if override_images.is_empty() { "card" } else { "override" }
}

/// 两个 IPv4 是否同 /24（与 NFS 服务端选择一致）
pub fn same_subnet(a: &str, b: &str) -> bool {
    match (a.rsplit_once('.'), b.rsplit_once('.')) {
        (Some((pa, _)), Some((pb, _))) => !pa.is_empty() && pa == pb,
        _ => false,
    }
}

/// 本机路由源地址探测抽象（便于单测注入）
pub trait SourceIpResolver: Send + Sync {
    /// 返回本机到目标 IP 的路由源地址；无路由 / 不支持返回 `None`
    fn source_ip(&self, dst: &str) -> Option<String>;
}

/// 生产实现：标准库 UDP `connect` 探测内核为该目标选定的源地址
///
/// 等价于 `ip -j route get <dst>` 的 `prefsrc`（不发包、无网络 I/O）；不依赖 `iproute2`。
pub struct RealSourceIpResolver;

impl SourceIpResolver for RealSourceIpResolver {
    fn source_ip(&self, dst: &str) -> Option<String> {
        let addr: IpAddr = dst.trim().parse().ok()?;
        let bind = match addr {
            IpAddr::V4(_) => "0.0.0.0:0",
            IpAddr::V6(_) => "[::]:0",
        };
        let sock = UdpSocket::bind(bind).ok()?;
        sock.connect((addr, 1)).ok()?;
        match sock.local_addr().ok()?.ip() {
            IpAddr::V4(v) if !v.is_loopback() && !v.is_unspecified() => Some(v.to_string()),
            _ => None,
        }
    }
}

/// 测试替身：按目标 IP 预置源地址
#[derive(Default)]
pub struct MockSourceIpResolver {
    map: BTreeMap<String, String>,
}

impl MockSourceIpResolver {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_source(mut self, dst: &str, src: &str) -> Self {
        self.map.insert(dst.to_string(), src.to_string());
        self
    }
}

impl SourceIpResolver for MockSourceIpResolver {
    fn source_ip(&self, dst: &str) -> Option<String> {
        self.map.get(dst).cloned()
    }
}

/// 解析 `ip route show default` / `ip -j route show default` 输出中的网关
pub fn parse_default_gateway(output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.starts_with('[') {
        let v: Value = serde_json::from_str(trimmed).ok()?;
        return v
            .as_array()?
            .iter()
            .find_map(|e| e.get("gateway").and_then(Value::as_str))
            .map(str::to_string);
    }
    for line in trimmed.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if let Some(pos) = tokens.iter().position(|t| *t == "via")
            && let Some(gw) = tokens.get(pos + 1)
            && !gw.is_empty()
        {
            return Some((*gw).to_string());
        }
    }
    None
}

/// 地址解析输入
pub struct HubHostInput<'a> {
    /// 显式覆盖（`MODELINFER_IMAGE_HUB_SERVER`）
    pub server_override: &'a str,
    /// 本机到节点 IP 的路由源地址（可能为 None）
    pub local_source: Option<&'a str>,
    /// 节点台账 IP
    pub node_host: &'a str,
    /// 节点默认网关（可能为空）
    pub node_gateway: &'a str,
    /// 平台设备信息中的本机可用 IP 候选
    pub candidates: &'a [String],
}

/// 是否为「非空且可用」的本机 / 网关 IP
fn usable_host(ip: &str) -> bool {
    !ip.trim().is_empty() && is_usable_ip(ip.trim())
}

/// 按节点选择 image-hub registry 地址（host 部分，纯函数）
///
/// 优先级：显式覆盖 → 本机路由源地址同网段 → 平台设备 IP 同网段 → **本机路由源地址**
/// → 首个可用本机 IP → 节点默认网关。
///
/// 说明：hub 与 modelinfer 同机（host 网络 `0.0.0.0:19913`），故「本机地址」通常即为正确
/// 答案；本机路由源地址由内核按到该节点的路由选定，双向可达。节点默认网关仅作最后兜底
/// （网关自身通常并不监听 hub 端口）。
pub fn pick_hub_host(input: &HubHostInput<'_>) -> Option<String> {
    if !input.server_override.trim().is_empty() {
        return Some(input.server_override.trim().to_string());
    }
    if let Some(src) = input.local_source
        && usable_host(src)
        && same_subnet(src, input.node_host)
    {
        return Some(src.to_string());
    }
    if let Some(c) = input
        .candidates
        .iter()
        .find(|c| usable_host(c) && same_subnet(c, input.node_host))
    {
        return Some(c.clone());
    }
    if let Some(src) = input.local_source
        && usable_host(src)
    {
        return Some(src.to_string());
    }
    if let Some(c) = input.candidates.iter().find(|c| usable_host(c)) {
        return Some(c.clone());
    }
    if usable_host(input.node_gateway) {
        return Some(input.node_gateway.trim().to_string());
    }
    None
}

/// image-hub registry 只读客户端（存在性判定）
#[async_trait]
pub trait ImageHubClient: Send + Sync {
    /// `HEAD /v2/<repository>/manifests/<tag>`：`Ok(true)` 存在、`Ok(false)` 不存在
    async fn manifest_exists(&self, repository: &str, tag: &str) -> Result<bool, ModelinferError>;
}

/// 生产实现（reqwest，registry v2 只读 HEAD）
pub struct RealImageHubClient {
    base: String,
    client: reqwest::Client,
}

impl RealImageHubClient {
    pub fn new(base: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            client,
        }
    }

    pub fn from_config(cfg: &ImageHubConfig) -> Self {
        Self::new(cfg.registry_base())
    }
}

/// registry v2 manifest 的 Accept 头（兼容 docker / OCI 的 index 与 manifest）
const MANIFEST_ACCEPT: &str = "application/vnd.docker.distribution.manifest.v2+json, \
application/vnd.docker.distribution.manifest.list.v2+json, \
application/vnd.oci.image.manifest.v1+json, \
application/vnd.oci.image.index.v1+json";

#[async_trait]
impl ImageHubClient for RealImageHubClient {
    async fn manifest_exists(&self, repository: &str, tag: &str) -> Result<bool, ModelinferError> {
        let url = format!("{}/v2/{}/manifests/{}", self.base, repository, tag);
        let resp = self
            .client
            .head(&url)
            .header("Accept", MANIFEST_ACCEPT)
            .send()
            .await
            .map_err(|e| {
                ModelinferError::new(
                    codes::IMAGE_HUB_UNREACHABLE,
                    format!("image-hub registry 不可达 {url}: {e}"),
                )
            })?;
        match resp.status().as_u16() {
            200..=299 => Ok(true),
            404 => Ok(false),
            other => Err(ModelinferError::new(
                codes::IMAGE_HUB_UNREACHABLE,
                format!("image-hub registry 返回异常状态 {other}: {url}"),
            )),
        }
    }
}

/// 测试替身
#[derive(Default)]
pub struct MockImageHubClient {
    /// 存在的镜像（`repository:tag`）
    pub existing: Vec<String>,
    /// 模拟 registry 不可达
    pub fail: bool,
    pub calls: Mutex<Vec<String>>,
}

impl MockImageHubClient {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_existing(mut self, images: &[&str]) -> Self {
        self.existing = images.iter().map(|s| (*s).to_string()).collect();
        self
    }
    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }
    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ImageHubClient for MockImageHubClient {
    async fn manifest_exists(&self, repository: &str, tag: &str) -> Result<bool, ModelinferError> {
        self.calls.lock().unwrap().push(format!("{repository}:{tag}"));
        if self.fail {
            return Err(ModelinferError::new(
                codes::IMAGE_HUB_UNREACHABLE,
                "mock registry 不可达",
            ));
        }
        Ok(self.existing.iter().any(|e| e == &format!("{repository}:{tag}")))
    }
}

/// 单次拉取记录
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePullRecord {
    pub node: String,
    pub image: String,
    pub ok: bool,
    pub message: String,
}

/// 镜像预检 / 分发报告
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageEnsureReport {
    pub node_count: usize,
    pub image_count: usize,
    /// 预检时缺失镜像的节点数
    pub nodes_missing: usize,
    /// 实际执行的拉取记录
    pub pulls: Vec<ImagePullRecord>,
}

/// 只读资源检查结果；节点索引保持调用方顺序，供后续分发复用。
struct ImageAvailability {
    probed: Vec<(usize, usize, Vec<ImageRef>)>,
    unavailable: Vec<String>,
}

/// 只读检查：节点本地已存在或 image-hub 可分发即视为可用，不拉取或修改节点。
pub async fn unavailable_images(
    ssh: &dyn SshExec,
    hub: &dyn ImageHubClient,
    nodes: &[&DeployNode],
    manifest_images: &[Value],
    concurrency: usize,
) -> Result<Vec<String>, ModelinferError> {
    Ok(check_image_availability(ssh, hub, nodes, manifest_images, concurrency).await?.unavailable)
}

async fn check_image_availability(
    ssh: &dyn SshExec,
    hub: &dyn ImageHubClient,
    nodes: &[&DeployNode],
    manifest_images: &[Value],
    concurrency: usize,
) -> Result<ImageAvailability, ModelinferError> {
    if manifest_images.is_empty() {
        return Ok(ImageAvailability { probed: Vec::new(), unavailable: Vec::new() });
    }
    let concurrency = effective_concurrency(concurrency, nodes.len());
    // 说明：本函数被 Temporal `#[activities]` 宏包裹，宏生成的 `BoxFuture<'static, _>` 会对
    // 内部「返回 future 的闭包」施加过强的 HRTB 约束（"implementation of FnOnce is not
    // general enough"）。因此这里刻意**不使用闭包**，改为普通 `for` 循环构建 future 集合，
    // 再交给 `buffer_unordered` 并发执行。

    // 阶段 1：节点并行——探测架构 → 过滤镜像 → 检查存在性
    let mut probe_futs = Vec::with_capacity(nodes.len());
    for (idx, node) in nodes.iter().enumerate() {
        probe_futs.push(async move {
            let (count, missing) = probe_node(ssh, node, manifest_images).await?;
            Ok::<_, ModelinferError>((idx, count, missing))
        });
    }
    let mut probed: Vec<(usize, usize, Vec<ImageRef>)> = Vec::with_capacity(nodes.len());
    {
        let mut stream = stream::iter(probe_futs).buffer_unordered(concurrency);
        while let Some(res) = stream.next().await {
            probed.push(res?);
        }
    }
    probed.sort_by_key(|(idx, _, _)| *idx);
    // 阶段 2：去重缺失镜像 → 并行确认 image-hub 存在性；任一 hub 无 → 中止且不拉取
    let unique_missing = unique_missing_images(&probed);
    let mut hub_futs = Vec::with_capacity(unique_missing.len());
    for (idx, img) in unique_missing.iter().enumerate() {
        hub_futs.push(async move {
            let exists = hub.manifest_exists(&img.repository, &img.tag).await?;
            Ok::<_, ModelinferError>((idx, exists))
        });
    }
    let mut hub_checked: Vec<(usize, bool)> = Vec::with_capacity(unique_missing.len());
    {
        let mut stream = stream::iter(hub_futs).buffer_unordered(concurrency);
        while let Some(res) = stream.next().await {
            hub_checked.push(res?);
        }
    }
    hub_checked.sort_by_key(|(idx, _)| *idx);
    let mut hub_missing: Vec<String> = Vec::new();
    for (idx, exists) in &hub_checked {
        if !*exists {
            hub_missing.push(unique_missing[*idx].full());
        }
    }
    Ok(ImageAvailability { probed, unavailable: hub_missing })
}

/// 逐节点执行 `ensure_images`（**节点间并行**、节点内镜像串行）
///
/// 前置：`rt` 已加载（含 `manifest_images` / 节点 / workdir）。
///
/// 并行度取 `cfg.concurrency`（`0` = 不限制，见 [`effective_concurrency`]）；四个阶段
/// （预检 / hub 存在性判定 / 拉取 / 复检）均按节点或镜像并行，任一节点失败即取消其余在途任务。
/// `report.pulls` 按节点台账顺序（`ordered_nodes`）输出，与并行完成顺序无关。
pub async fn ensure_images(
    ssh: &dyn SshExec,
    hub: &dyn ImageHubClient,
    srcip: &dyn SourceIpResolver,
    rt: &DeployRuntime,
    cfg: &ImageHubConfig,
    local_candidates: &[String],
) -> Result<ImageEnsureReport, ModelinferError> {
    let mut report = ImageEnsureReport {
        node_count: rt.nodes.len(),
        ..Default::default()
    };
    if rt.manifest_images.is_empty() {
        return Ok(report);
    }

    let nodes = rt.ordered_nodes();
    let concurrency = effective_concurrency(cfg.concurrency, nodes.len());

    let availability = check_image_availability(
        ssh, hub, &nodes, &rt.manifest_images, concurrency,
    ).await?;
    let probed = availability.probed;
    let hub_missing = availability.unavailable;
    let (max_images, missing_nodes) = probe_stats(&probed);
    report.image_count = max_images;
    report.nodes_missing = missing_nodes;
    if !hub_missing.is_empty() {
        return Err(ModelinferError::new(
            codes::IMAGE_NOT_AVAILABLE,
            format!(
                "以下镜像在节点与 image-hub 上均不存在，已中止启动: {}",
                hub_missing.join(", ")
            ),
        ));
    }

    // 阶段 3：缺失节点并行拉取（节点内镜像逐个串行）
    let mut pull_futs = Vec::new();
    for (idx, _, missing) in probed.iter() {
        if missing.is_empty() {
            continue;
        }
        let node = nodes[*idx];
        pull_futs.push(async move {
            let records =
                pull_node(ssh, srcip, cfg, local_candidates, node, &rt.workdir, missing).await?;
            Ok::<_, ModelinferError>((*idx, records))
        });
    }
    let mut pulled: Vec<(usize, Vec<ImagePullRecord>)> = Vec::new();
    {
        let mut stream = stream::iter(pull_futs).buffer_unordered(concurrency);
        while let Some(res) = stream.next().await {
            pulled.push(res?);
        }
    }
    pulled.sort_by_key(|(idx, _)| *idx);
    for (_, records) in pulled {
        report.pulls.extend(records);
    }

    // 阶段 4：节点并行复检；任一仍缺失 → 失败
    let mut recheck_futs = Vec::new();
    for (idx, _, missing) in probed.iter() {
        if missing.is_empty() {
            continue;
        }
        let node = nodes[*idx];
        recheck_futs.push(async move {
            recheck_node(ssh, node, missing).await?;
            Ok::<_, ModelinferError>(*idx)
        });
    }
    {
        let mut stream = stream::iter(recheck_futs).buffer_unordered(concurrency);
        while let Some(res) = stream.next().await {
            res?;
        }
    }

    Ok(report)
}

/// 预检统计：`(各节点适用镜像数上限, 缺失节点数)`
fn probe_stats(probed: &[(usize, usize, Vec<ImageRef>)]) -> (usize, usize) {
    let mut max_images = 0usize;
    let mut missing_nodes = 0usize;
    for (_, count, missing) in probed {
        if *count > max_images {
            max_images = *count;
        }
        if !missing.is_empty() {
            missing_nodes += 1;
        }
    }
    (max_images, missing_nodes)
}

/// 去重缺失镜像（按 `repository:tag` 归一；保留首次出现顺序）
fn unique_missing_images(probed: &[(usize, usize, Vec<ImageRef>)]) -> Vec<ImageRef> {
    let mut out: Vec<ImageRef> = Vec::new();
    for (_, _, missing) in probed {
        for img in missing {
            let full = img.full();
            let mut exists = false;
            for seen in &out {
                if seen.full() == full {
                    exists = true;
                    break;
                }
            }
            if !exists {
                out.push(img.clone());
            }
        }
    }
    out
}

/// 单节点预检：架构探测 → 按架构过滤镜像 → 逐镜像检查本地存在性
///
/// 返回 `(该节点适用的镜像数, 缺失镜像列表)`；节点内镜像检查串行。
async fn probe_node(
    ssh: &dyn SshExec,
    node: &DeployNode,
    manifest_images: &[Value],
) -> Result<(usize, Vec<ImageRef>), ModelinferError> {
    let arch = node_arch(ssh, node).await?;
    let images = images_for_arch(manifest_images, &arch)?;
    let count = images.len();
    let mut missing = Vec::new();
    for img in &images {
        if !image_present(ssh, &target_of(node), &img.full()).await? {
            missing.push(img.clone());
        }
    }
    Ok((count, missing))
}

/// 单节点拉取：解析 hub 地址 → 建目录 / 上传脚本 → 逐镜像 `image-hub.sh pull`（节点内串行）
#[allow(clippy::too_many_arguments)] // 镜像分发编排的显式依赖（SSH/源地址/配置/候选/目标/目录/镜像）
async fn pull_node(
    ssh: &dyn SshExec,
    srcip: &dyn SourceIpResolver,
    cfg: &ImageHubConfig,
    local_candidates: &[String],
    node: &DeployNode,
    workdir: &str,
    missing: &[ImageRef],
) -> Result<Vec<ImagePullRecord>, ModelinferError> {
    let host =
        resolve_hub_host(ssh, srcip, cfg, local_candidates, &node.host, &target_of(node)).await?;
    let addr = cfg.addr(&host);
    let script = format!("{workdir}/.modelinfer/image-hub.sh");
    prepare_workdir(ssh, &target_of(node), workdir).await?;
    upload_script(ssh, &target_of(node), &script, IMAGE_HUB_SH).await?;

    let mut records = Vec::with_capacity(missing.len());
    for img in missing {
        let full = img.full();
        let cmd = format!(
            "bash {} --ih-addr {} pull {}",
            sh_quote(&script),
            sh_quote(&addr),
            sh_quote(&full)
        );
        let out = ssh
            .run_with_timeout(
                &target_of(node),
                &cmd,
                Duration::from_secs(IMAGE_PULL_TIMEOUT_SECS),
            )
            .await?;
        if !out.success() {
            return Err(ModelinferError::new(
                codes::IMAGE_PULL_FAILED,
                format!(
                    "节点 {} 经 image-hub({}) 拉取镜像 {} 失败(exit={}): {}{}",
                    node.host,
                    addr,
                    full,
                    out.exit_code,
                    out.stdout.trim(),
                    out.stderr.trim()
                ),
            ));
        }
        records.push(ImagePullRecord {
            node: node.host.clone(),
            image: full,
            ok: true,
            message: out.stdout.trim().to_string(),
        });
    }
    Ok(records)
}

/// 单节点复检：逐镜像确认本地存在性（节点内串行）
async fn recheck_node(
    ssh: &dyn SshExec,
    node: &DeployNode,
    missing: &[ImageRef],
) -> Result<(), ModelinferError> {
    for img in missing {
        if !image_present(ssh, &target_of(node), &img.full()).await? {
            return Err(ModelinferError::new(
                codes::IMAGE_PULL_FAILED,
                format!("节点 {} 拉取后仍缺少镜像 {}", node.host, img.full()),
            ));
        }
    }
    Ok(())
}

async fn node_arch(ssh: &dyn SshExec, node: &DeployNode) -> Result<String, ModelinferError> {
    let out = ssh
        .run_with_timeout(&target_of(node), "uname -m", Duration::from_secs(IMAGE_PROBE_TIMEOUT_SECS))
        .await?;
    if !out.success() {
        return Err(ModelinferError::new(
            codes::NODE_SCAN_FAILED,
            format!("节点 {} 获取架构失败: {}", node.host, out.stderr.trim()),
        ));
    }
    Ok(normalize_arch(&out.stdout))
}

/// 镜像是否存在于节点本地（`pub(crate)`：供监控镜像分发复用）
pub(crate) async fn image_present(
    ssh: &dyn SshExec,
    target: &SshTarget,
    image: &str,
) -> Result<bool, ModelinferError> {
    let cmd = format!("docker image inspect {} >/dev/null 2>&1", sh_quote(image));
    let out = ssh
        .run_with_timeout(target, &cmd, Duration::from_secs(IMAGE_PROBE_TIMEOUT_SECS))
        .await?;
    Ok(out.success())
}

/// 按节点解析 image-hub 地址（`pub(crate)`：供监控镜像分发复用）
pub(crate) async fn resolve_hub_host(
    ssh: &dyn SshExec,
    srcip: &dyn SourceIpResolver,
    cfg: &ImageHubConfig,
    candidates: &[String],
    node_host: &str,
    target: &SshTarget,
) -> Result<String, ModelinferError> {
    let local_source = srcip.source_ip(node_host);
    let mut gateway = String::new();
    // 仅在前两级都无法确定同网段地址时才需要节点网关，减少一次 SSH
    let needs_gateway = {
        let same_via_source = local_source
            .as_deref()
            .map(|s| usable_host(s) && same_subnet(s, node_host))
            .unwrap_or(false);
        let same_via_candidates = candidates
            .iter()
            .any(|c| usable_host(c) && same_subnet(c, node_host));
        cfg.server.trim().is_empty() && !same_via_source && !same_via_candidates
    };
    if needs_gateway {
        gateway = node_default_gateway(ssh, target).await.unwrap_or_default();
    }
    pick_hub_host(&HubHostInput {
        server_override: &cfg.server,
        local_source: local_source.as_deref(),
        node_host,
        node_gateway: &gateway,
        candidates,
    })
    .ok_or_else(|| {
        ModelinferError::new(
            codes::IMAGE_HUB_UNREACHABLE,
            format!("无法为节点 {node_host} 确定 image-hub 地址（路由源地址 / 网关 / 设备 IP 均不可用）"),
        )
    })
}

/// 读取节点默认网关（失败不报错，返回 None；`pub(crate)`：供监控镜像分发复用）
pub(crate) async fn node_default_gateway(ssh: &dyn SshExec, target: &SshTarget) -> Option<String> {
    let cmd = "ip -j route show default 2>/dev/null || ip route show default";
    let out = ssh
        .run_with_timeout(target, cmd, Duration::from_secs(IMAGE_PROBE_TIMEOUT_SECS))
        .await
        .ok()?;
    parse_default_gateway(&out.stdout)
}

/// 准备节点工作目录（`pub(crate)`：供监控镜像分发复用）
pub(crate) async fn prepare_workdir(
    ssh: &dyn SshExec,
    target: &SshTarget,
    workdir: &str,
) -> Result<(), ModelinferError> {
    let cmd = format!("mkdir -p {}/.modelinfer", sh_quote(workdir));
    let out = ssh
        .run_with_timeout(target, &cmd, Duration::from_secs(IMAGE_PROBE_TIMEOUT_SECS))
        .await?;
    if !out.success() {
        return Err(ModelinferError::new(
            codes::IMAGE_PULL_FAILED,
            format!("节点 {} 创建工作目录失败: {}", target.host, out.stderr.trim()),
        ));
    }
    Ok(())
}

/// 上传脚本到节点（`pub(crate)`：供监控镜像分发复用）
pub(crate) async fn upload_script(
    ssh: &dyn SshExec,
    target: &SshTarget,
    remote: &str,
    content: &str,
) -> Result<(), ModelinferError> {
    let dir = std::env::temp_dir().join("modelinfer-image-hub");
    std::fs::create_dir_all(&dir)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建临时目录失败: {e}")))?;
    let local = dir.join(format!("{}-image-hub.sh", ulid::Ulid::new()));
    std::fs::write(&local, content)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("写临时脚本失败: {e}")))?;
    let res = ssh.upload(target, &local, remote).await;
    let _ = std::fs::remove_file(&local);
    res
}

/// 节点 SSH 目标（`pub(crate)`：供监控镜像分发复用）
pub(crate) fn target_of(node: &DeployNode) -> SshTarget {
    SshTarget::from_credentials(
        node.host.clone(),
        node.port,
        node.user.clone(),
        &node.auth_method,
        &node.password,
        &node.ssh_key,
    ).with_sudo_password(&node.sudo_password)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// 模拟节点：维护各节点本地镜像集合，支持 `docker image inspect` 与 `image-hub.sh pull`
    /// （拉取即把镜像加入集合），用于验证「预检 → 拉取 → 复检」闭环。
    ///
    /// `pull_delay` 用于让并发拉取可观测：单次 pull 在改动镜像集合前 `sleep`，使
    /// `buffer_unordered` 的多个节点任务真正重叠；`max_active_pulls` 记录峰值并发。
    #[derive(Default)]
    struct FakeNodeSsh {
        images: Mutex<BTreeMap<String, BTreeSet<String>>>,
        calls: Mutex<Vec<String>>,
        uploads: Mutex<Vec<String>>,
        pull_calls: Mutex<Vec<(String, String, String)>>,
        gateway: Mutex<String>,
        arch: Mutex<String>,
        available: AtomicBool,
        pull_fail: AtomicBool,
        /// 按主机设置的 pull 延迟（毫秒）；`"*"` 表示全局默认
        pull_delay_ms: Mutex<BTreeMap<String, u64>>,
        active_pulls: AtomicUsize,
        max_active_pulls: AtomicUsize,
    }

    impl FakeNodeSsh {
        fn new() -> Self {
            Self {
                arch: Mutex::new("aarch64".into()),
                available: AtomicBool::new(true),
                ..Self::default()
            }
        }
        fn with_image(self, host: &str, image: &str) -> Self {
            self.images
                .lock()
                .unwrap()
                .entry(host.to_string())
                .or_default()
                .insert(image.to_string());
            self
        }
        fn with_gateway(self, gw: &str) -> Self {
            *self.gateway.lock().unwrap() = gw.to_string();
            self
        }
        fn with_arch(self, arch: &str) -> Self {
            *self.arch.lock().unwrap() = arch.to_string();
            self
        }
        fn failing_pull(self) -> Self {
            self.pull_fail.store(true, Ordering::SeqCst);
            self
        }
        /// 设置全局 pull 延迟（毫秒），使并发拉取可观测
        fn with_pull_delay(self, ms: u64) -> Self {
            self.pull_delay_ms.lock().unwrap().insert("*".to_string(), ms);
            self
        }
        /// 按主机设置 pull 延迟（毫秒），用于构造「完成顺序与台账顺序相反」的场景
        fn with_pull_delay_for(self, host: &str, ms: u64) -> Self {
            self.pull_delay_ms.lock().unwrap().insert(host.to_string(), ms);
            self
        }
        /// 峰值并发拉取数
        fn max_active_pulls(&self) -> usize {
            self.max_active_pulls.load(Ordering::SeqCst)
        }
        fn pull_calls(&self) -> Vec<(String, String, String)> {
            self.pull_calls.lock().unwrap().clone()
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    /// 提取命令中的单引号参数
    fn quoted(cmd: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = cmd;
        while let Some(start) = rest.find('\'') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('\'') else { break };
            out.push(after[..end].to_string());
            rest = &after[end + 1..];
        }
        out
    }

    #[async_trait]
    impl SshExec for FakeNodeSsh {
        async fn run(
            &self,
            target: &SshTarget,
            cmd: &str,
        ) -> Result<crate::ssh::SshOutput, ModelinferError> {
            if !self.available.load(Ordering::SeqCst) {
                return Err(ModelinferError::new(codes::SSH_CONNECT_FAILED, "mock 不可达"));
            }
            self.calls.lock().unwrap().push(format!("{}:{}", target.host, cmd));
            let ok = |stdout: &str| crate::ssh::SshOutput {
                exit_code: 0,
                stdout: stdout.into(),
                stderr: String::new(),
            };
            let fail = |stdout: &str| crate::ssh::SshOutput {
                exit_code: 1,
                stdout: stdout.into(),
                stderr: "mock fail".into(),
            };
            if cmd.starts_with("uname -m") {
                return Ok(ok(&format!("{}\n", self.arch.lock().unwrap())));
            }
            if cmd.starts_with("mkdir -p") {
                return Ok(ok(""));
            }
            if cmd.contains("docker image inspect") {
                let q = quoted(cmd);
                let image = q.first().cloned().unwrap_or_default();
                let present = self
                    .images
                    .lock()
                    .unwrap()
                    .get(&target.host)
                    .map(|s| s.contains(&image))
                    .unwrap_or(false);
                return Ok(if present { ok("") } else { fail("") });
            }
            if cmd.contains("--ih-addr") && cmd.contains(" pull ") {
                let q = quoted(cmd);
                // [script, addr, image]
                let addr = q.get(1).cloned().unwrap_or_default();
                let image = q.get(2).cloned().unwrap_or_default();
                self.pull_calls
                    .lock()
                    .unwrap()
                    .push((target.host.clone(), addr, image.clone()));
                // 并发可观测：进入 pull 即计入活跃数并记录峰值；sleep 让多个节点任务真正重叠
                let active = self.active_pulls.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_active_pulls.fetch_max(active, Ordering::SeqCst);
                let delay = {
                    let map = self.pull_delay_ms.lock().unwrap();
                    map.get(&target.host)
                        .or_else(|| map.get("*"))
                        .copied()
                        .unwrap_or(0)
                };
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                let failed = self.pull_fail.load(Ordering::SeqCst);
                if !failed {
                    self.images
                        .lock()
                        .unwrap()
                        .entry(target.host.clone())
                        .or_default()
                        .insert(image);
                }
                self.active_pulls.fetch_sub(1, Ordering::SeqCst);
                if failed {
                    return Ok(fail("拉取失败"));
                }
                return Ok(ok("完成"));
            }
            if cmd.contains("route show default") {
                let gw = self.gateway.lock().unwrap().clone();
                if gw.is_empty() {
                    return Ok(fail(""));
                }
                return Ok(ok(&format!("default via {gw} dev eth0 proto dhcp\n")));
            }
            Ok(ok(""))
        }

        async fn upload(
            &self,
            target: &SshTarget,
            _local: &std::path::Path,
            remote: &str,
        ) -> Result<(), ModelinferError> {
            if !self.available.load(Ordering::SeqCst) {
                return Err(ModelinferError::new(codes::SSH_CONNECT_FAILED, "mock 不可达"));
            }
            self.uploads
                .lock()
                .unwrap()
                .push(format!("{}:{}", target.host, remote));
            Ok(())
        }
    }

    fn node(id: &str, host: &str) -> DeployNode {
        DeployNode {
            id: id.into(),
            name: id.into(),
            host: host.into(),
            port: 22,
            user: "root".into(),
            auth_method: "password".into(),
            password: "x".into(),
            ssh_key: String::new(),
            sudo_password: String::new(),
            hostname: id.into(),
            socket_ifname: "eth0".into(),
            ib_hca: String::new(),
            arch: "amd64".into(),
            fscache: false,
            role: "head".into(),
        }
    }

    fn runtime(nodes: Vec<DeployNode>, images: Vec<Value>) -> DeployRuntime {
        DeployRuntime {
            deployment_id: "d".into(),
            name: "n".into(),
            nodes,
            head_ip: String::new(),
            nodes_map: String::new(),
            card_id: "c".into(),
            weight_id: "w".into(),
            card_weight_id: String::new(),
            weight_overridden: false,
            weight_source: "hf".into(),
            model_path: "/data/w".into(),
            model_hub_root: "/data".into(),
            serve_port: 8000,
            serve_api_key: "k".into(),
            serve_model_name: "w".into(),
            configs: Default::default(),
            extra_env: Default::default(),
            lifecycle: Default::default(),
            lifecycle_order: Default::default(),
            lifecycle_timeout: Default::default(),
            workdir: "/opt/modelinfer/deployments/d".into(),
            mount_dir: "/mnt/model_hub".into(),
            nfs: crate::nfs::NfsResolveResult::not_mounted(),
            package_path: String::new(),
            manifest_images: images,
            images_source: "card".into(),
            serve_url_prefix: "/v1".into(),
        }
    }

    #[test]
    fn normalize_image_ref_variants() {
        assert_eq!(
            normalize_image_ref("swr.cn-southwest-2.myhuaweicloud.com/ictrek/vllm-openai:glm-5.3-flash-gb10").unwrap(),
            (
                "swr.cn-southwest-2.myhuaweicloud.com/ictrek/vllm-openai".to_string(),
                "glm-5.3-flash-gb10".to_string()
            )
        );
        // 单段名补 library/，缺省 docker.io + latest
        assert_eq!(
            normalize_image_ref("alpine").unwrap(),
            ("docker.io/library/alpine".to_string(), "latest".to_string())
        );
        // 两段名无 registry → docker.io
        assert_eq!(
            normalize_image_ref("vllm/vllm-openai:nightly").unwrap(),
            ("docker.io/vllm/vllm-openai".to_string(), "nightly".to_string())
        );
        // 端口形式的 registry
        assert_eq!(
            normalize_image_ref("127.0.0.1:5000/foo/bar:1").unwrap(),
            ("127.0.0.1:5000/foo/bar".to_string(), "1".to_string())
        );
        // 非法
        assert!(normalize_image_ref("").is_err());
        assert!(normalize_image_ref("a@sha256:deadbeef").is_err());
        assert!(normalize_image_ref("Foo/Bar:tag").is_err());
    }

    #[test]
    fn images_for_arch_filters() {
        let raw = vec![
            json!("docker.io/library/alpine:3"),
            json!({"name": "reg.example/x:arm", "arch": "arm64"}),
            json!({"name": "reg.example/y:amd", "arch": "amd64"}),
        ];
        let arm = images_for_arch(&raw, "aarch64").unwrap();
        assert_eq!(arm.len(), 2);
        assert!(arm.iter().any(|i| i.full() == "docker.io/library/alpine:3"));
        assert!(arm.iter().any(|i| i.full() == "reg.example/x:arm"));

        let amd = images_for_arch(&raw, "x86_64").unwrap();
        assert_eq!(amd.len(), 2);
        assert!(amd.iter().any(|i| i.full() == "reg.example/y:amd"));
    }

    #[test]
    fn effective_images_and_source() {
        let card = vec![json!("a:1"), json!("b:2")];
        let over = vec![json!("c:3")];
        // 空覆盖 → 卡片声明
        assert_eq!(effective_images(&card, &[]), card);
        assert_eq!(images_source(&[]), "card");
        // 非空覆盖 → 整体替换（不合并）
        assert_eq!(effective_images(&card, &over), over);
        assert_eq!(images_source(&over), "override");
        // 空卡片 + 空覆盖
        assert!(effective_images(&[], &[]).is_empty());
    }

    #[test]
    fn manifest_images_parse() {
        assert!(manifest_images(&json!({})).unwrap().is_empty());
        assert_eq!(manifest_images(&json!({"images": ["a"]})).unwrap().len(), 1);
        assert!(manifest_images(&json!({"images": "a"})).is_err());
    }

    #[test]
    fn same_subnet_and_usable() {
        assert!(same_subnet("10.10.11.90", "10.10.11.154"));
        assert!(!same_subnet("10.10.11.90", "192.168.8.74"));
        assert!(!same_subnet("bad", "10.10.11.154"));
        assert!(!is_usable_ip("127.0.0.1"));
        assert!(!is_usable_ip("172.17.0.1"));
        assert!(is_usable_ip("10.10.11.90"));
    }

    #[test]
    fn parse_gateway_plain_and_json() {
        assert_eq!(
            parse_default_gateway("default via 10.10.11.90 dev br0 proto dhcp metric 100\n"),
            Some("10.10.11.90".to_string())
        );
        assert_eq!(
            parse_default_gateway(
                r#"[{"dst":"default","gateway":"192.168.8.254","dev":"wlP9s9","prefsrc":"192.168.8.74"}]"#
            ),
            Some("192.168.8.254".to_string())
        );
        assert_eq!(parse_default_gateway(""), None);
    }

    #[test]
    fn pick_hub_host_priority() {
        let candidates = vec!["172.17.0.1".to_string(), "10.100.10.24".to_string()];
        // override 最高
        assert_eq!(
            pick_hub_host(&HubHostInput {
                server_override: "hub.example:1",
                local_source: Some("10.10.11.90"),
                node_host: "10.10.11.154",
                node_gateway: "10.10.11.90",
                candidates: &candidates,
            }),
            Some("hub.example:1".to_string())
        );
        // 本机路由源地址同网段优先
        assert_eq!(
            pick_hub_host(&HubHostInput {
                server_override: "",
                local_source: Some("10.10.11.90"),
                node_host: "10.10.11.154",
                node_gateway: "192.168.8.254",
                candidates: &candidates,
            }),
            Some("10.10.11.90".to_string())
        );
        // 跨网段 → 本机路由源地址（hub 与 modelinfer 同机，通常即正确答案）
        assert_eq!(
            pick_hub_host(&HubHostInput {
                server_override: "",
                local_source: Some("10.10.11.90"),
                node_host: "192.168.8.74",
                node_gateway: "192.168.8.254",
                candidates: &candidates,
            }),
            Some("10.10.11.90".to_string())
        );
        // 跨网段且本机地址不可用 → 节点默认网关（最后兜底）
        assert_eq!(
            pick_hub_host(&HubHostInput {
                server_override: "",
                local_source: None,
                node_host: "192.168.8.74",
                node_gateway: "192.168.8.254",
                candidates: &[],
            }),
            Some("192.168.8.254".to_string())
        );
        // 平台设备 IP 同网段
        assert_eq!(
            pick_hub_host(&HubHostInput {
                server_override: "",
                local_source: None,
                node_host: "10.100.10.21",
                node_gateway: "",
                candidates: &candidates,
            }),
            Some("10.100.10.24".to_string())
        );
        // 兜底首个可用本机 IP（排除 docker 网桥）
        assert_eq!(
            pick_hub_host(&HubHostInput {
                server_override: "",
                local_source: None,
                node_host: "10.9.9.9",
                node_gateway: "",
                candidates: &candidates,
            }),
            Some("10.100.10.24".to_string())
        );
        assert_eq!(
            pick_hub_host(&HubHostInput {
                server_override: "",
                local_source: None,
                node_host: "10.9.9.9",
                node_gateway: "",
                candidates: &[],
            }),
            None
        );
    }

    #[test]
    fn embedded_script_markers() {
        assert!(IMAGE_HUB_SH.starts_with("#!/usr/bin/env bash"));
        for marker in ["--ih-addr", "pull", "push", "socat"] {
            assert!(IMAGE_HUB_SH.contains(marker), "内置脚本缺少标记 {marker}");
        }
    }

    /// 与 image-hub 组件源码逐字节一致（monorepo 内生效；源码不在时跳过）
    ///
    /// 漂移时执行 `components/modelinfer/scripts/sync-image-hub.sh` 同步。
    #[test]
    fn embedded_script_matches_image_hub_source_if_available() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../image-hub/frontend/public/image-hub.sh");
        if !p.exists() {
            return;
        }
        let src = std::fs::read_to_string(&p).expect("读取 image-hub 源码脚本");
        assert_eq!(
            src, IMAGE_HUB_SH,
            "内置 image-hub.sh 与 image-hub 组件源码不一致，请执行 scripts/sync-image-hub.sh"
        );
    }

    #[tokio::test]
    async fn preflight_collects_all_missing_images_without_pulling() {
        let nodes = [node("n1", "10.0.0.1"), node("n2", "10.0.0.2")];
        let ssh = FakeNodeSsh::new()
            .with_image("10.0.0.1", "docker.io/library/alpine:latest")
            .with_image("10.0.0.1", "reg.example/local:1")
            .with_image("10.0.0.2", "reg.example/local:1");
        let hub = MockImageHubClient::new().with_existing(&["reg.example/ready:1"]);
        let images = vec![
            json!("alpine"), json!("docker.io/library/alpine:latest"),
            json!("reg.example/local:1"), json!("reg.example/ready:1"),
            json!({"name": "reg.example/arm:1", "arch": "arm64"}),
            json!({"name": "reg.example/amd:1", "arch": "amd64"}),
        ];
        let missing = unavailable_images(&ssh, &hub, &[&nodes[0], &nodes[1]], &images, 2)
            .await.unwrap();
        assert_eq!(missing, ["reg.example/arm:1", "docker.io/library/alpine:latest"]);
        let mut calls = hub.recorded_calls();
        calls.sort();
        assert_eq!(calls, ["docker.io/library/alpine:latest", "reg.example/arm:1", "reg.example/ready:1"]);
        assert!(ssh.pull_calls().is_empty());
        assert!(ssh.uploads.lock().unwrap().is_empty());
        assert!(ssh.calls().iter().all(|c| c.contains("uname -m") || c.contains("docker image inspect")));
    }

    #[tokio::test]
    async fn preflight_accepts_local_images_without_requiring_hub() {
        let n = node("n", "10.0.0.1");
        let ssh = FakeNodeSsh::new().with_image("10.0.0.1", "reg.example/local:1");
        let hub = MockImageHubClient::failing();
        assert!(unavailable_images(&ssh, &hub, &[&n], &[json!("reg.example/local:1")], 1)
            .await.unwrap().is_empty());
        assert!(hub.recorded_calls().is_empty());
        let error = unavailable_images(&ssh, &hub, &[&n], &[json!("reg.example/other:1")], 1)
            .await.unwrap_err();
        assert_eq!(error.code, codes::IMAGE_HUB_UNREACHABLE);
    }

    #[tokio::test]
    async fn ensure_images_all_present_is_noop() {
        let rt = runtime(vec![node("n1", "10.10.11.154")], vec![json!("reg.example/x:1")]);
        let ssh = FakeNodeSsh::new().with_image("10.10.11.154", "reg.example/x:1");
        let hub = MockImageHubClient::new();
        let report = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new().with_source("10.10.11.154", "10.10.11.90"),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.nodes_missing, 0);
        assert!(report.pulls.is_empty());
        // 未触发 hub 查询与拉取
        assert!(hub.recorded_calls().is_empty());
        assert!(ssh.pull_calls().is_empty());
        assert!(!ssh.calls().iter().any(|c| c.contains("image-hub.sh")));
    }

    #[tokio::test]
    async fn ensure_images_pulls_only_missing_node() {
        let rt = runtime(
            vec![node("n1", "10.10.11.154"), node("n2", "10.10.11.155")],
            vec![json!("reg.example/x:1")],
        );
        let ssh = FakeNodeSsh::new().with_image("10.10.11.154", "reg.example/x:1");
        let hub = MockImageHubClient::new().with_existing(&["reg.example/x:1"]);
        let report = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new()
                .with_source("10.10.11.154", "10.10.11.90")
                .with_source("10.10.11.155", "10.10.11.90"),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.nodes_missing, 1);
        let pulls = ssh.pull_calls();
        assert_eq!(pulls.len(), 1);
        assert_eq!(pulls[0].0, "10.10.11.155");
        assert_eq!(pulls[0].1, "10.10.11.90:19913");
        assert_eq!(pulls[0].2, "reg.example/x:1");
        assert_eq!(report.pulls.len(), 1);
        assert!(report.pulls[0].ok);
        // 已存在的 n1 未被拉取
        assert!(!pulls.iter().any(|(h, _, _)| h == "10.10.11.154"));
    }

    #[tokio::test]
    async fn ensure_images_aborts_when_hub_missing() {
        let rt = runtime(vec![node("n1", "10.10.11.154")], vec![json!("reg.example/x:1")]);
        // 节点缺失（无预置镜像）
        let ssh = FakeNodeSsh::new();
        let hub = MockImageHubClient::new(); // 空 = 不存在
        let err = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new(),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::IMAGE_NOT_AVAILABLE);
        // 不发起任何拉取
        assert!(ssh.pull_calls().is_empty());
        assert_eq!(hub.recorded_calls(), vec!["reg.example/x:1".to_string()]);
    }

    #[tokio::test]
    async fn ensure_images_hub_unreachable() {
        let rt = runtime(vec![node("n1", "10.10.11.154")], vec![json!("reg.example/x:1")]);
        let ssh = FakeNodeSsh::new();
        let hub = MockImageHubClient::failing();
        let err = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new(),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::IMAGE_HUB_UNREACHABLE);
    }

    #[tokio::test]
    async fn ensure_images_pull_failure_is_20729() {
        let rt = runtime(vec![node("n1", "10.10.11.154")], vec![json!("reg.example/x:1")]);
        let ssh = FakeNodeSsh::new().failing_pull();
        let hub = MockImageHubClient::new().with_existing(&["reg.example/x:1"]);
        let err = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new().with_source("10.10.11.154", "10.10.11.90"),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::IMAGE_PULL_FAILED);
    }

    #[tokio::test]
    async fn ensure_images_uses_override_addr() {
        let rt = runtime(vec![node("n1", "10.10.11.154")], vec![json!("reg.example/x:1")]);
        let ssh = FakeNodeSsh::new();
        let hub = MockImageHubClient::new().with_existing(&["reg.example/x:1"]);
        let cfg = ImageHubConfig::new("hub.local", 5000);
        let report = ensure_images(&ssh, &hub, &MockSourceIpResolver::new(), &rt, &cfg, &[])
            .await
            .unwrap();
        assert_eq!(report.pulls.len(), 1);
        assert_eq!(ssh.pull_calls()[0].1, "hub.local:5000");
    }

    #[tokio::test]
    async fn ensure_images_cross_subnet_uses_local_source() {
        let rt = runtime(vec![node("n1", "192.168.8.74")], vec![json!("reg.example/x:1")]);
        let ssh = FakeNodeSsh::new().with_gateway("192.168.8.254");
        let hub = MockImageHubClient::new().with_existing(&["reg.example/x:1"]);
        let report = ensure_images(
            &ssh,
            &hub,
            // 本机路由源地址与节点不同网段：hub 与 modelinfer 同机 → 用本机地址
            &MockSourceIpResolver::new().with_source("192.168.8.74", "10.10.11.90"),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.pulls.len(), 1);
        assert_eq!(ssh.pull_calls()[0].1, "10.10.11.90:19913");
    }

    #[tokio::test]
    async fn ensure_images_falls_back_to_node_gateway_when_no_local_ip() {
        let rt = runtime(vec![node("n1", "192.168.8.74")], vec![json!("reg.example/x:1")]);
        let ssh = FakeNodeSsh::new().with_gateway("192.168.8.254");
        let hub = MockImageHubClient::new().with_existing(&["reg.example/x:1"]);
        let report = ensure_images(
            &ssh,
            &hub,
            // 本机无可用地址 → 节点默认网关兜底
            &MockSourceIpResolver::new(),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.pulls.len(), 1);
        assert_eq!(ssh.pull_calls()[0].1, "192.168.8.254:19913");
    }

    #[tokio::test]
    async fn ensure_images_skips_when_no_images_declared() {
        let rt = runtime(vec![node("n1", "10.10.11.154")], vec![]);
        let ssh = FakeNodeSsh::new();
        let hub = MockImageHubClient::new();
        let report = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new(),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.node_count, 1);
        assert!(ssh.calls().is_empty());
        assert!(hub.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn ensure_images_filters_by_node_arch() {
        // 卡片仅声明 arm64 镜像；节点为 x86_64 → 不适用 → 无需拉取
        let rt = runtime(
            vec![node("n1", "10.10.11.154")],
            vec![json!({"name": "reg/arm:1", "arch": "arm64"})],
        );
        let ssh = FakeNodeSsh::new().with_arch("x86_64");
        let hub = MockImageHubClient::new();
        let report = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new(),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.nodes_missing, 0);
        assert!(ssh.pull_calls().is_empty());
        assert!(hub.recorded_calls().is_empty());
    }

    #[test]
    fn effective_concurrency_branches() {
        assert_eq!(effective_concurrency(4, 8), 4);
        assert_eq!(effective_concurrency(0, 8), 8, "0 = 不限制 → 全部节点");
        assert_eq!(effective_concurrency(4, 2), 2, "不超过节点数");
        assert_eq!(effective_concurrency(1, 2), 1);
        assert_eq!(effective_concurrency(0, 0), 1, "无节点时至少 1");
    }

    /// 多节点缺失 → 节点间并行拉取（峰值并发 = min(节点数, 并发度)）
    #[tokio::test]
    async fn ensure_images_pulls_nodes_in_parallel() {
        let rt = runtime(
            vec![
                node("n1", "10.10.11.154"),
                node("n2", "10.10.11.155"),
                node("n3", "10.10.11.156"),
            ],
            vec![json!("reg.example/x:1")],
        );
        let ssh = FakeNodeSsh::new().with_pull_delay(20);
        let hub = MockImageHubClient::new().with_existing(&["reg.example/x:1"]);
        let report = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new()
                .with_source("10.10.11.154", "10.10.11.90")
                .with_source("10.10.11.155", "10.10.11.90")
                .with_source("10.10.11.156", "10.10.11.90"),
            &rt,
            &ImageHubConfig::default(), // 默认并发度 4，节点数 3
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.pulls.len(), 3);
        assert_eq!(report.nodes_missing, 3);
        assert_eq!(ssh.max_active_pulls(), 3, "三节点应并行拉取");
    }

    /// 并发度 = 1 → 退化为既有串行行为
    #[tokio::test]
    async fn ensure_images_concurrency_one_is_serial() {
        let rt = runtime(
            vec![
                node("n1", "10.10.11.154"),
                node("n2", "10.10.11.155"),
                node("n3", "10.10.11.156"),
            ],
            vec![json!("reg.example/x:1")],
        );
        let ssh = FakeNodeSsh::new().with_pull_delay(10);
        let hub = MockImageHubClient::new().with_existing(&["reg.example/x:1"]);
        let cfg = ImageHubConfig::default().with_concurrency(1);
        let report = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new()
                .with_source("10.10.11.154", "10.10.11.90")
                .with_source("10.10.11.155", "10.10.11.90")
                .with_source("10.10.11.156", "10.10.11.90"),
            &rt,
            &cfg,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.pulls.len(), 3);
        assert_eq!(ssh.max_active_pulls(), 1, "并发度 1 应退化为串行");
    }

    /// 单节点内多镜像必须串行（不与自身竞争 SSH / 磁盘 / 带宽）
    #[tokio::test]
    async fn ensure_images_serializes_images_within_node() {
        let rt = runtime(
            vec![node("n1", "10.10.11.154")],
            vec![
                json!("reg.example/a:1"),
                json!("reg.example/b:1"),
                json!("reg.example/c:1"),
            ],
        );
        let ssh = FakeNodeSsh::new().with_pull_delay(10);
        let hub = MockImageHubClient::new().with_existing(&[
            "reg.example/a:1",
            "reg.example/b:1",
            "reg.example/c:1",
        ]);
        let report = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new().with_source("10.10.11.154", "10.10.11.90"),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.pulls.len(), 3);
        assert_eq!(ssh.max_active_pulls(), 1, "单节点内镜像必须串行");
    }

    /// 并行完成顺序与台账顺序相反时，报告仍按台账顺序输出
    #[tokio::test]
    async fn ensure_images_report_preserves_node_order() {
        let rt = runtime(
            vec![node("n1", "10.10.11.154"), node("n2", "10.10.11.155")],
            vec![json!("reg.example/x:1")],
        );
        let ssh = FakeNodeSsh::new()
            .with_pull_delay_for("10.10.11.154", 25)
            .with_pull_delay_for("10.10.11.155", 2);
        let hub = MockImageHubClient::new().with_existing(&["reg.example/x:1"]);
        let report = ensure_images(
            &ssh,
            &hub,
            &MockSourceIpResolver::new()
                .with_source("10.10.11.154", "10.10.11.90")
                .with_source("10.10.11.155", "10.10.11.90"),
            &rt,
            &ImageHubConfig::default(),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(report.pulls.len(), 2);
        assert_eq!(report.pulls[0].node, "10.10.11.154", "报告按台账顺序");
        assert_eq!(report.pulls[1].node, "10.10.11.155");
    }
}
