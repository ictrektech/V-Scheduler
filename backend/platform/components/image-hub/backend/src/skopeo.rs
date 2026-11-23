//! skopeo 编排域：远程镜像拉取/同步的子进程封装与纯函数参数构造。
//!
//! - 抽象 `SkopeoRunner`：生产 `RealSkopeoRunner`（tokio::process），单测 `MockSkopeoRunner`
//! - 纯函数：`parse_image_ref` / `normalize_image_ref` / `multi_arch` / `build_copy_args` /
//!   `build_login_args` / `build_proxy_env`（全部可单测，不依赖真实 skopeo）
//! - 语义（承接 D-IH-03）：推入本地 registry 的仓库名 = **上游完整镜像名**（含 registry
//!   host 与 namespace），tag 不变；例如 `docker.io/library/ubuntu:22.04` →
//!   仓库名 `docker.io/library/ubuntu`
//! - 临时文件：全局 `--tmpdir <data-dir>/tmp`，避免写入容器可写层（docker 分区）

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use crate::error::{ImageHubError, codes};
use crate::settings::{ProxyConfig, UpstreamCredential, normalize_host, validate_registry_host};

/// skopeo 进程逐行日志发送端（接收端不可用时发送失败被忽略）
pub type LogSender = UnboundedSender<String>;

// ==================== 取消令牌 ====================

/// 协作式取消令牌（D-IH-39）：包装 `tokio::sync::watch` 接收端。
///
/// - `CancelToken::never()`：永不取消（单测 / 无可取消通道的场景）
/// - `is_cancelled()`：同步查询；`cancelled()`：异步等待（发送端 drop 亦视为取消）
#[derive(Clone)]
pub struct CancelToken {
    rx: Option<watch::Receiver<bool>>,
}

impl CancelToken {
    pub fn new(rx: watch::Receiver<bool>) -> Self {
        Self { rx: Some(rx) }
    }

    /// 永不取消的令牌
    pub fn never() -> Self {
        Self { rx: None }
    }

    /// 是否已请求取消
    pub fn is_cancelled(&self) -> bool {
        self.rx.as_ref().is_some_and(|rx| *rx.borrow())
    }

    /// 等待取消信号（`never()` 时永不返回）
    pub async fn cancelled(&mut self) {
        match self.rx.as_mut() {
            None => std::future::pending::<()>().await,
            Some(rx) => {
                if *rx.borrow() {
                    return;
                }
                // 发送端被 drop（任务管理器退出）也视为取消，避免永久等待
                let _ = rx.changed().await;
            }
        }
    }
}

/// 取消导致的错误（`20810`，仅内部使用）
pub fn cancelled_error() -> ImageHubError {
    ImageHubError::new(codes::TASK_CANCELLED, "任务已被用户取消")
}

/// 注入 skopeo 的代理环境变量键（大小写同时注入，兼容不同库读取）
const PROXY_ENV_KEYS: [&str; 6] = [
    "http_proxy",
    "HTTP_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "no_proxy",
    "NO_PROXY",
];

/// 始终清除的代理变量（避免继承宿主/容器环境造成意外代理）
const ALWAYS_CLEAR_PROXY_KEYS: [&str; 2] = ["all_proxy", "ALL_PROXY"];

/// 默认上游 registry（缺省 registry 时按 docker 规则补全）
pub const DEFAULT_REGISTRY: &str = "docker.io";

/// 默认单次 skopeo 执行总超时（秒）：防上游连接挂死导致任务永不结束
pub const DEFAULT_SKOPEO_TIMEOUT_SECS: u64 = 3600;

/// 单次上游传输超时（分钟）——默认值（与既有兜底一致）
pub const DEFAULT_TRANSFER_TIMEOUT_MINUTES: u64 = 60;
/// 单次上游传输超时下限（分钟）
pub const MIN_TRANSFER_TIMEOUT_MINUTES: u64 = 1;
/// 单次上游传输超时：`0` 表示**无限制**（不设超时；D-IH-44）
pub const UNLIMITED_TRANSFER_TIMEOUT_MINUTES: u64 = 0;

/// 传输失败重试次数——默认值（D-IH-45）
pub const DEFAULT_RETRY_TIMES: u64 = 2;
/// 传输失败重试次数下限
pub const MIN_RETRY_TIMES: u64 = 1;
/// 传输失败重试次数上限
pub const MAX_RETRY_TIMES: u64 = 5;
/// 重试退避起始时长（秒）
pub const RETRY_BACKOFF_BASE_SECS: u64 = 5;
/// 重试退避上限（秒）
pub const RETRY_BACKOFF_MAX_SECS: u64 = 60;

/// skopeo login 超时（秒）：登录为尽力而为，不宜久等
pub const LOGIN_TIMEOUT_SECS: u64 = 30;

/// 校验并归一单次传输超时（分钟）：
/// - 缺省 → 默认 60 分钟
/// - `0` → `Duration::ZERO` = **无限制**（执行器不设超时，D-IH-44）
/// - `>= 1` → 该分钟数（**无上限**）
///
/// 校验为纯函数；`u64` 入参自身已排除负数（协议层为整数分钟，非整数由反序列化拒绝）。
pub fn validate_transfer_timeout(minutes: Option<u64>) -> Result<Duration, ImageHubError> {
    let value = minutes.unwrap_or(DEFAULT_TRANSFER_TIMEOUT_MINUTES);
    if value == UNLIMITED_TRANSFER_TIMEOUT_MINUTES {
        return Ok(Duration::ZERO);
    }
    if value < MIN_TRANSFER_TIMEOUT_MINUTES {
        return Err(ImageHubError::new(
            codes::PARAM_INVALID,
            format!("超时时间非法（{value} 分钟）：须为 0（无限制）或不小于 {MIN_TRANSFER_TIMEOUT_MINUTES} 分钟的整数"),
        ));
    }
    Ok(Duration::from_secs(value * 60))
}

/// 校验并归一传输失败重试次数：缺省 → 默认 2；越界 → `20801`（D-IH-45）
pub fn validate_retry_times(times: Option<u64>) -> Result<u64, ImageHubError> {
    let value = times.unwrap_or(DEFAULT_RETRY_TIMES);
    if !(MIN_RETRY_TIMES..=MAX_RETRY_TIMES).contains(&value) {
        return Err(ImageHubError::new(
            codes::PARAM_INVALID,
            format!(
                "重试次数非法（{value} 次）：允许范围 {MIN_RETRY_TIMES}~{MAX_RETRY_TIMES} 次"
            ),
        ));
    }
    Ok(value)
}

/// 第 `failure_index` 次失败后的退避时长（纯函数）：
/// `min(BASE * 2^(index-1), MAX)` → 生产取值 5 / 10 / 20 / 40 / 60 秒
pub fn retry_backoff(failure_index: u64) -> Duration {
    retry_backoff_delay(Duration::from_secs(RETRY_BACKOFF_BASE_SECS), failure_index)
}

/// 退避时长（可注入基准时长，便于单测缩短等待）：
/// `min(base * 2^(index-1), RETRY_BACKOFF_MAX_SECS)`
pub fn retry_backoff_delay(base: Duration, failure_index: u64) -> Duration {
    let index = failure_index.max(1);
    let shift = (index - 1).min(16); // 防溢出
    let factor = 1u32 << shift;
    let base = base.min(Duration::from_secs(RETRY_BACKOFF_MAX_SECS));
    let delay = base.saturating_mul(factor);
    delay.min(Duration::from_secs(RETRY_BACKOFF_MAX_SECS))
}

/// 把超时时长格式化为可读文案：
/// - `ZERO` → 「无限制」（`timeout_minutes = 0`）
/// - `>= 1 小时` 且整小时 → 「N 小时」
/// - 整分钟 → 「N 分钟」；其余 → 「N 秒」
pub fn format_timeout(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs == 0 {
        return "无限制".to_string();
    }
    if secs >= 3600 && secs.is_multiple_of(3600) {
        return format!("{} 小时", secs / 3600);
    }
    if secs >= 60 && secs.is_multiple_of(60) {
        return format!("{} 分钟", secs / 60);
    }
    format!("{secs} 秒")
}

// ==================== 进程输出与抽象 ====================

/// skopeo 子进程执行结果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkopeoOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl SkopeoOutput {
    /// 合并输出（stdout + stderr）
    pub fn combined(&self) -> String {
        let mut s = self.stdout.clone();
        if !self.stderr.is_empty() {
            if !s.is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&self.stderr);
        }
        s
    }

    /// 最近 n 行（用于错误信息 / 日志摘要）
    pub fn tail_lines(&self, n: usize) -> Vec<String> {
        let all: Vec<String> = self
            .combined()
            .lines()
            .map(|l| l.trim_end().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        let start = all.len().saturating_sub(n);
        all[start..].to_vec()
    }
}

/// 一次 skopeo 子命令调用（argv 不含 skopeo 二进制本身）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkopeoCommand {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    /// 需要从继承环境移除的变量（如禁用代理时的 *_proxy）
    pub clear_env: Vec<String>,
    /// 通过 stdin 传入的数据（如 login 密码，避免进入 argv / /proc）
    pub stdin: Option<String>,
    /// 单次执行总超时；`None` 使用执行器默认值，`Some(0)` 表示不限时
    pub timeout: Option<Duration>,
}

/// skopeo 执行抽象（便于单测注入 Mock）
#[async_trait]
pub trait SkopeoRunner: Send + Sync {
    /// 执行一次 skopeo 调用；输出逐行经 `log` 发送，最终连同退出状态返回。
    ///
    /// `cancel` 被触发时 MUST **终止子进程**并返回 `20810 TASK_CANCELLED`（D-IH-39）。
    async fn run(
        &self,
        cmd: SkopeoCommand,
        log: LogSender,
        cancel: CancelToken,
    ) -> Result<SkopeoOutput, ImageHubError>;
}

/// 生产实现：tokio::process 执行真实 skopeo
pub struct RealSkopeoRunner {
    bin: String,
    default_timeout: Duration,
}

impl RealSkopeoRunner {
    pub fn new(bin: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            default_timeout: Duration::from_secs(DEFAULT_SKOPEO_TIMEOUT_SECS),
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    pub fn bin(&self) -> &str {
        &self.bin
    }
}

/// 逐行读取 reader 并转发到 log，返回累计内容
async fn drain_lines<R>(reader: R, log: LogSender) -> String
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    let mut buf = String::new();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let _ = log.send(line.clone());
                buf.push_str(&line);
                buf.push('\n');
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    buf
}

#[async_trait]
impl SkopeoRunner for RealSkopeoRunner {
    async fn run(
        &self,
        cmd: SkopeoCommand,
        log: LogSender,
        mut cancel: CancelToken,
    ) -> Result<SkopeoOutput, ImageHubError> {
        let mut command = tokio::process::Command::new(&self.bin);
        command.args(&cmd.argv);
        for key in &cmd.clear_env {
            command.env_remove(key);
        }
        for (key, value) in &cmd.env {
            command.env(key, value);
        }
        command
            .stdin(if cmd.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|e| {
            ImageHubError::new(
                codes::SKOPEO_FAILED,
                format!("启动 skopeo ({}) 失败: {e}", self.bin),
            )
        })?;

        if let Some(input) = cmd.stdin.as_deref()
            && let Some(mut stdin) = child.stdin.take()
        {
            let _ = stdin.write_all(input.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }

        let out_task = child
            .stdout
            .take()
            .map(|s| tokio::spawn(drain_lines(s, log.clone())));
        let err_task = child
            .stderr
            .take()
            .map(|s| tokio::spawn(drain_lines(s, log.clone())));

        let limit = cmd.timeout.unwrap_or(self.default_timeout);

        // 结果分三种：正常结束 / 超时 / 被取消（None 表示取消）
        let waited: Option<Result<std::process::ExitStatus, ImageHubError>> = if limit.is_zero() {
            tokio::select! {
                r = child.wait() => Some(r.map_err(|e| {
                    ImageHubError::new(codes::SKOPEO_FAILED, format!("等待 skopeo 结束失败: {e}"))
                })),
                _ = cancel.cancelled() => None,
            }
        } else {
            tokio::select! {
                r = tokio::time::timeout(limit, child.wait()) => match r {
                    Ok(Ok(status)) => Some(Ok(status)),
                    Ok(Err(e)) => Some(Err(ImageHubError::new(
                        codes::SKOPEO_FAILED,
                        format!("等待 skopeo 结束失败: {e}"),
                    ))),
                    Err(_) => Some(Err(ImageHubError::new(
                        codes::SKOPEO_FAILED,
                        format!("skopeo 执行超时（{}）", format_timeout(limit)),
                    ))),
                },
                _ = cancel.cancelled() => None,
            }
        };

        let status = match waited {
            None => {
                // 取消：立即终止子进程并回收，避免留下持续占用网络/磁盘的孤儿进程
                let _ = child.start_kill();
                let _ = child.wait().await;
                if let Some(h) = out_task {
                    let _ = h.await;
                }
                if let Some(h) = err_task {
                    let _ = h.await;
                }
                let _ = log.send("任务已被用户取消，已终止 skopeo".to_string());
                return Err(cancelled_error());
            }
            Some(Ok(status)) => status,
            Some(Err(e)) => {
                if e.msg.contains("超时") {
                    // 超时：终止子进程并报错（防上游连接挂死 / 超大镜像超时导致任务永不结束）
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    let _ = log.send(format!(
                        "skopeo 执行超时（{}），已终止；如为超大镜像请在发起任务时增大超时时间",
                        format_timeout(limit)
                    ));
                }
                return Err(e);
            }
        };

        let stdout = match out_task {
            Some(h) => h.await.unwrap_or_default(),
            None => String::new(),
        };
        let stderr = match err_task {
            Some(h) => h.await.unwrap_or_default(),
            None => String::new(),
        };

        Ok(SkopeoOutput {
            success: status.success(),
            stdout,
            stderr,
        })
    }
}

// ==================== 测试替身 ====================

/// 记录一次 mock 调用
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MockCall {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub clear_env: Vec<String>,
    pub stdin: Option<String>,
    /// 单次执行超时（`None` 表示使用执行器默认值）
    pub timeout: Option<Duration>,
}

/// 内存 mock：记录调用序列，可注入成功/失败与逐行输出
#[derive(Default)]
pub struct MockSkopeoRunner {
    calls: std::sync::Mutex<Vec<MockCall>>,
    /// true 时所有调用返回失败
    pub fail: bool,
    /// 每次调用前发送的日志行
    pub log_lines: Vec<String>,
    /// `Some(ms)`：模拟耗时传输（等待期间响应取消），用于取消单测
    pub delay_ms: Option<u64>,
    /// 前 N 次调用失败、之后成功（用于重试单测）
    pub fail_times: u64,
    /// 被取消的次数（单测断言用）
    cancelled_calls: std::sync::atomic::AtomicU64,
}

impl MockSkopeoRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Default::default()
        }
    }

    /// 模拟耗时传输（等待期间可被取消）
    pub fn with_delay(delay_ms: u64) -> Self {
        Self {
            delay_ms: Some(delay_ms),
            ..Default::default()
        }
    }

    /// 前 `times` 次调用失败，之后成功（重试单测用）
    pub fn failing_then_ok(times: u64) -> Self {
        Self {
            fail_times: times,
            ..Default::default()
        }
    }

    /// 调用次数
    pub fn call_count(&self) -> u64 {
        self.calls.lock().unwrap().len() as u64
    }

    pub fn invocations(&self) -> Vec<MockCall> {
        self.calls.lock().unwrap().clone()
    }

    /// 提取所有调用中的 argv（便于断言子命令顺序）
    pub fn argv_list(&self) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.argv.clone())
            .collect()
    }

    /// 被取消的调用次数
    pub fn cancelled_count(&self) -> u64 {
        self.cancelled_calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl SkopeoRunner for MockSkopeoRunner {
    async fn run(
        &self,
        cmd: SkopeoCommand,
        log: LogSender,
        mut cancel: CancelToken,
    ) -> Result<SkopeoOutput, ImageHubError> {
        for line in &self.log_lines {
            let _ = log.send(line.clone());
        }
        let call_index = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(MockCall {
                argv: cmd.argv.clone(),
                env: cmd.env.clone(),
                clear_env: cmd.clear_env.clone(),
                stdin: cmd.stdin.clone(),
                timeout: cmd.timeout,
            });
            calls.len() as u64
        };
        if let Some(delay_ms) = self.delay_ms {
            // 模拟耗时传输：期间响应取消（用于取消单测）
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                _ = cancel.cancelled() => {
                    self.cancelled_calls
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _ = log.send("任务已被用户取消，已终止 skopeo".to_string());
                    return Err(cancelled_error());
                }
            }
        }
        if self.fail || call_index <= self.fail_times {
            Ok(SkopeoOutput {
                success: false,
                stdout: String::new(),
                stderr: "mock skopeo failure".to_string(),
            })
        } else {
            Ok(SkopeoOutput {
                success: true,
                stdout: "mock ok".to_string(),
                stderr: String::new(),
            })
        }
    }
}

// ==================== 镜像引用解析（纯函数） ====================

/// 归一后的镜像引用（registry / repository / tag）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    /// registry host（可含端口），如 `docker.io`、`127.0.0.1:5000`
    pub registry: String,
    /// namespace + 名称，如 `library/ubuntu`、`foo/bar`
    pub repository: String,
    pub tag: String,
}

impl ImageRef {
    /// 上游完整引用（`registry/repository:tag`）
    pub fn reference(&self) -> String {
        format!("{}/{}:{}", self.registry, self.repository, self.tag)
    }

    /// 上游完整仓库名（`registry/repository`）——即本地 registry 中的仓库名
    pub fn upstream_repository(&self) -> String {
        format!("{}/{}", self.registry, self.repository)
    }

    /// 本地 registry 中的目标引用（`<dest_host>/<上游完整仓库名>:<tag>`）
    pub fn dest_ref(&self, dest_host: &str) -> String {
        format!("{dest_host}/{}:{}", self.upstream_repository(), self.tag)
    }
}

/// 判定首段是否为 registry host（含 `.` / `:` 或为 `localhost`）
pub fn looks_like_registry_host(segment: &str) -> bool {
    segment == "localhost" || segment.contains('.') || segment.contains(':')
}

/// 校验镜像仓库名（小写字母数字与 `._-/`，无 `..`/空段/首尾斜杠）
pub fn validate_image_repository(repo: &str) -> Result<(), ImageHubError> {
    let ok = !repo.is_empty()
        && !repo.starts_with('/')
        && !repo.ends_with('/')
        && !repo.contains("..")
        && repo.split('/').all(|s| !s.is_empty())
        && repo.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-' | '/')
        });
    if ok {
        Ok(())
    } else {
        Err(ImageHubError::new(
            codes::IMAGE_REF_INVALID,
            format!("非法镜像仓库名: {repo}"),
        ))
    }
}

/// 校验标签（字母数字与 `._-`，长度 1..=128）
pub fn validate_image_tag(tag: &str) -> Result<(), ImageHubError> {
    let ok = !tag.is_empty()
        && tag.len() <= 128
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(ImageHubError::new(
            codes::IMAGE_REF_INVALID,
            format!("非法镜像标签: {tag}"),
        ))
    }
}

/// 已知操作系统段（用于区分 `os/arch` 完整平台名与旧式 `arch/variant` 写法）
const KNOWN_OS: [&str; 5] = ["linux", "windows", "darwin", "freebsd", "wasip1"];

/// 判定单个平台段（`os` / `arch`）字符集是否合法：非空且仅含小写字母 / 数字 / `_`
fn is_platform_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// 归一化单个平台 token 为 docker 标准完整平台名 `os/arch`：
/// - `amd64`（单段纯架构名）→ `linux/amd64`（补默认 `os = linux`，向后兼容）；
/// - `linux/amd64`（`os/arch`）→ 原样保留；
/// - `arm/v7`（旧式 `arch/variant`，首段非已知 os）→ `linux/arm`；
/// - `linux/arm/v7`（`os/arch/variant`）→ `linux/arm`（variant 忽略——skopeo `--multi-arch`
///   平台列表仅接受 `os/arch` 两段，且平台过滤按 `os` + `architecture` 匹配、忽略 variant）。
///
/// 空段 / 段数大于 3 / `os` 或 `arch` 含非法字符 → `20809`。
pub fn normalize_arch_token(raw: &str) -> Result<String, ImageHubError> {
    let invalid = || ImageHubError::new(codes::IMAGE_REF_INVALID, format!("非法架构规格: {raw}"));
    let parts: Vec<&str> = raw.trim().split('/').map(str::trim).collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        return Err(invalid());
    }
    let (os, arch) = match parts.as_slice() {
        [arch] => ("linux", *arch),
        [os, arch] if KNOWN_OS.contains(os) => (*os, *arch),
        [arch, _variant] => ("linux", *arch),
        [os, arch, _variant] => (*os, *arch),
        _ => return Err(invalid()),
    };
    if !is_platform_segment(os) || !is_platform_segment(arch) {
        return Err(invalid());
    }
    Ok(format!("{os}/{arch}"))
}

/// 解析逗号分隔的平台列表 → 归一化后的完整平台名（`os/arch`，去重、保持顺序）。
///
/// 空白列表 / 任一空 token / 非法字符 → `20809`。
pub fn parse_arch_list(raw: &str) -> Result<Vec<String>, ImageHubError> {
    let mut archs: Vec<String> = Vec::new();
    for part in raw.split(',') {
        let arch = normalize_arch_token(part)?;
        if !archs.iter().any(|existing| existing == &arch) {
            archs.push(arch);
        }
    }
    if archs.is_empty() {
        return Err(ImageHubError::new(
            codes::IMAGE_REF_INVALID,
            format!("非法架构规格: {raw}"),
        ));
    }
    Ok(archs)
}

/// 解析并归一镜像引用：
/// - 支持 `[registry/]repository[:tag]`；tag 缺省 `latest`
/// - 首段含 `.`/`:` 或为 `localhost` → registry host；否则补 `docker.io`
/// - 无 namespace 的单段名 → 补 `library/`（如 `ubuntu` → `docker.io/library/ubuntu`）
/// - `@digest` 引用暂不支持 → 20809
pub fn parse_image_ref(input: &str) -> Result<ImageRef, ImageHubError> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(ImageHubError::new(
            codes::IMAGE_REF_INVALID,
            "镜像名不能为空",
        ));
    }
    if raw.contains('@') {
        return Err(ImageHubError::new(
            codes::IMAGE_REF_INVALID,
            format!("暂不支持 digest 引用（请使用 tag）: {input}"),
        ));
    }
    if raw.contains(char::is_whitespace) {
        return Err(ImageHubError::new(
            codes::IMAGE_REF_INVALID,
            format!("镜像名不能包含空白字符: {input}"),
        ));
    }

    // 拆 tag：最后一个 `:` 之后且不含 `/` 者视为 tag
    let (name_part, tag) = match raw.rfind(':') {
        Some(i) if !raw[i + 1..].contains('/') => (&raw[..i], &raw[i + 1..]),
        _ => (raw, "latest"),
    };
    validate_image_tag(tag)?;

    let (registry, repository) = match name_part.split_once('/') {
        Some((first, rest)) => {
            if first.is_empty() || rest.is_empty() {
                return Err(ImageHubError::new(
                    codes::IMAGE_REF_INVALID,
                    format!("非法镜像名（存在空路径段）: {input}"),
                ));
            }
            if looks_like_registry_host(first) {
                (validate_registry_host(first)?, rest.to_string())
            } else {
                (DEFAULT_REGISTRY.to_string(), name_part.to_string())
            }
        }
        None => {
            if name_part.is_empty() {
                return Err(ImageHubError::new(
                    codes::IMAGE_REF_INVALID,
                    format!("非法镜像名: {input}"),
                ));
            }
            (DEFAULT_REGISTRY.to_string(), format!("library/{name_part}"))
        }
    };

    let registry = normalize_host(&registry);
    validate_image_repository(&repository)?;
    Ok(ImageRef {
        registry,
        repository,
        tag: tag.to_string(),
    })
}

/// 拆分「上游完整仓库名」`registry/repository` 为 `(registry, repository)`。
///
/// 用于同步：本地 registry 中仓库名即上游完整镜像名（如 `docker.m.daocloud.io/library/alpine`
/// 或 `library/registry`），据此还原上游 registry 与仓库路径。
pub fn split_registry_repository(repo: &str) -> Result<(String, String), ImageHubError> {
    let raw = repo.trim();
    if raw.is_empty() {
        return Err(ImageHubError::new(
            codes::IMAGE_REF_INVALID,
            "仓库名不能为空",
        ));
    }
    match raw.split_once('/') {
        Some((first, rest)) if !first.is_empty() && !rest.is_empty() => {
            if looks_like_registry_host(first) {
                Ok((validate_registry_host(first)?, rest.to_string()))
            } else {
                validate_image_repository(raw)?;
                Ok((DEFAULT_REGISTRY.to_string(), raw.to_string()))
            }
        }
        _ => Err(ImageHubError::new(
            codes::IMAGE_REF_INVALID,
            format!("非法仓库名: {repo}"),
        )),
    }
}

// ==================== 架构选择（纯函数） ====================

/// 架构选择模式
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchMode {
    /// 全部架构（manifest list 全量）
    All,
    /// 本地 host 架构
    Host,
    /// 指定平台列表（docker 标准完整平台名，如 `linux/amd64` / `linux/arm64`；至少一个）
    Specific(Vec<String>),
}

impl ArchMode {
    /// 从请求字段解析：
    /// - None / 空 / `all` → `All`
    /// - `host` / `system` → `Host`
    /// - 逗号分隔平台列表（如 `linux/amd64,linux/arm64`；单值即单元素）→ `Specific`
    ///   （每个 token 经 `parse_arch_list` 归一化为 `os/arch` 完整平台名，含 `/variant` 者忽略 variant）
    pub fn from_request(raw: Option<&str>) -> Result<Self, ImageHubError> {
        let value = raw.map(str::trim).unwrap_or("");
        match value {
            "" | "all" => Ok(ArchMode::All),
            "host" | "system" => Ok(ArchMode::Host),
            other => Ok(ArchMode::Specific(parse_arch_list(other)?)),
        }
    }

    /// skopeo `--multi-arch` 取值（`all` / `system` / 逗号分隔完整平台名列表）
    pub fn multi_arch(&self) -> String {
        match self {
            ArchMode::All => "all".to_string(),
            ArchMode::Host => "system".to_string(),
            // 值已是 `os/arch` 完整平台名，直接拼接
            ArchMode::Specific(archs) => archs.join(","),
        }
    }

    /// 请求侧字符串表示（用于日志 / 任务摘要，如 `linux/amd64,linux/arm64`）
    pub fn as_request_str(&self) -> String {
        match self {
            ArchMode::All => "all".to_string(),
            ArchMode::Host => "host".to_string(),
            ArchMode::Specific(archs) => archs.join(","),
        }
    }
}

/// 便捷函数：架构模式 → `--multi-arch` 取值
pub fn multi_arch_arg(mode: &ArchMode) -> String {
    mode.multi_arch()
}

// ==================== 参数构造（纯函数） ====================

/// 判定 host 是否为回环地址（含端口形式，如 `127.0.0.1:19913`）
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim();
    let host_only = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        h.split(':').next().unwrap_or(h)
    };
    host_only == "localhost" || host_only == "::1" || host_only.starts_with("127.")
}

/// copy 计划（源 / 目标 URL 均已带传输前缀，如 `docker://` / `docker-archive:`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyPlan {
    /// 完整源 URL（含传输前缀，如 `docker://host/repo:tag` 或 `docker-archive:/path:ref`）
    pub source_url: String,
    /// 源 registry host（用于 `--src-tls-verify` 判定；归档等非 registry 源为空串）
    pub source_registry: String,
    /// 完整目标 URL（`docker://<本地 registry host>/<repo>:<tag>`）
    pub dest_url: String,
    pub dest_host: String,
    pub arch: ArchMode,
    pub insecure: bool,
    pub tmpdir: String,
    pub authfile: Option<String>,
}

/// 构造 `skopeo copy` 参数（不含二进制名）
pub fn build_copy_args(plan: &CopyPlan) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "--tmpdir".into(),
        plan.tmpdir.clone(),
        "copy".into(),
        "--multi-arch".into(),
        plan.arch.multi_arch(),
    ];
    // 指定平台时清理「未复制平台」的稀疏条目，使落地 index 仅含所选平台（D-IH-80）；
    // `all` / `host` 不适用（全量 / 单镜像）。
    if matches!(plan.arch, ArchMode::Specific(_)) {
        argv.push("--strip-removed-platforms".into());
    }
    // 归档等非 registry 源不适用源 TLS 参数
    if !plan.source_registry.is_empty()
        && (plan.insecure || is_loopback_host(&plan.source_registry))
    {
        argv.push("--src-tls-verify=false".into());
    }
    if is_loopback_host(&plan.dest_host) {
        argv.push("--dest-tls-verify=false".into());
    }
    if let Some(authfile) = &plan.authfile {
        argv.push("--authfile".into());
        argv.push(authfile.clone());
    }
    argv.push("--retry-times".into());
    argv.push("3".into());
    argv.push("--retry-delay".into());
    argv.push("5s".into());
    argv.push(plan.source_url.clone());
    argv.push(plan.dest_url.clone());
    argv
}

/// login 计划
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPlan {
    pub host: String,
    pub username: String,
    pub tmpdir: String,
    pub authfile: String,
    pub insecure: bool,
}

/// 构造 `skopeo login` 参数（密码经 stdin，不进 argv）
pub fn build_login_args(plan: &LoginPlan) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "--tmpdir".into(),
        plan.tmpdir.clone(),
        "login".into(),
        "--authfile".into(),
        plan.authfile.clone(),
        "-u".into(),
        plan.username.clone(),
        "--password-stdin".into(),
        plan.host.clone(),
    ];
    if plan.insecure {
        argv.push("--tls-verify=false".into());
    }
    argv
}

/// 合并 `no_proxy`：保留用户项，追加本地回环（保证本地 registry 直连）
pub fn merge_no_proxy(raw: &str, registry_host: &str) -> String {
    let mut parts: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    for extra in ["127.0.0.1", "localhost"] {
        if !parts.iter().any(|p| p == extra) {
            parts.push(extra.to_string());
        }
    }
    let host = registry_host.trim();
    if !host.is_empty() {
        let host_only = host.split(':').next().unwrap_or(host).to_string();
        if !parts.contains(&host_only) {
            parts.push(host_only);
        }
    }
    parts.join(",")
}

/// 构造代理环境变量与需清除项：
/// - 启用：注入大小写 `http_proxy`/`https_proxy`/`no_proxy`（`no_proxy` 追加本地回环）
/// - 禁用：不注入，并清除继承的代理变量（避免宿主/容器意外代理）
pub fn build_proxy_env(proxy: &ProxyConfig, registry_host: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut clear: Vec<String> = ALWAYS_CLEAR_PROXY_KEYS
        .iter()
        .map(|s| s.to_string())
        .collect();

    if !proxy.enabled {
        clear.extend(PROXY_ENV_KEYS.iter().map(|s| s.to_string()));
        return (Vec::new(), clear);
    }

    let mut env: Vec<(String, String)> = Vec::new();
    let http = proxy.http_proxy.trim();
    if !http.is_empty() {
        env.push(("http_proxy".to_string(), http.to_string()));
        env.push(("HTTP_PROXY".to_string(), http.to_string()));
    }
    let https = proxy.https_proxy.trim();
    if !https.is_empty() {
        env.push(("https_proxy".to_string(), https.to_string()));
        env.push(("HTTPS_PROXY".to_string(), https.to_string()));
    }
    let no_proxy = merge_no_proxy(&proxy.no_proxy, registry_host);
    if !no_proxy.is_empty() {
        env.push(("no_proxy".to_string(), no_proxy.clone()));
        env.push(("NO_PROXY".to_string(), no_proxy));
    }
    (env, clear)
}

// ==================== 编排 ====================

/// 复制来源（D-IH-52）：上游 registry / 本地 registry / 归档文件
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    /// 上游 registry（可选镜像源覆盖：仅改变实际拉取来源，D-IH-32）
    Registry { registry_override: Option<String> },
    /// 本地 registry 内的引用（克隆 / 改名；源与目标同在本地 registry）
    Local { repository: String, tag: String },
    /// docker-archive 归档（`inner` 为归档内原始引用，如 `docker.io/library/alpine:3.19`）
    DockerArchive { path: PathBuf, inner: Option<String> },
    /// OCI 归档
    OciArchive { path: PathBuf, inner: Option<String> },
}

/// 传输计划：复制一个具体 tag（`image` 恒为**目标身份**，dest 由本地 registry host 派生）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlan {
    /// 目标引用身份（仓库名保留策略 / 克隆改名目标名）
    pub image: ImageRef,
    pub arch: ArchMode,
    pub insecure: bool,
    /// 复制来源（D-IH-52）
    pub source: SourceKind,
    /// 单次上游传输超时（D-IH-44）；`None` 使用执行器默认值（60 分钟），
    /// `Some(Duration::ZERO)` 表示无限制
    pub timeout: Option<Duration>,
    /// 单次传输失败后的重试次数（D-IH-45）：最多尝试 `1 + retry_times` 次
    pub retry_times: u64,
}

impl TransferPlan {
    /// 上游 registry 变体的镜像源覆盖（其余变体为 `None`）
    pub fn registry_override(&self) -> Option<&str> {
        match &self.source {
            SourceKind::Registry { registry_override } => registry_override.as_deref(),
            _ => None,
        }
    }

    /// 生效源 registry（用于凭据查找与 TLS 判定；归档源为空串）
    pub fn source_registry(&self, ctx: &TransferContext) -> String {
        match &self.source {
            SourceKind::Registry { registry_override } => crate::mirror::effective_source_registry(
                &self.image,
                registry_override.as_deref(),
            ),
            SourceKind::Local { .. } => ctx.registry_host.clone(),
            SourceKind::DockerArchive { .. } | SourceKind::OciArchive { .. } => String::new(),
        }
    }

    /// 是否需要对源执行 `skopeo login`（仅上游 registry 源）
    pub fn needs_login(&self) -> bool {
        matches!(self.source, SourceKind::Registry { .. })
    }

    /// 源引用（展示 / 日志用，不含 `docker://` 前缀）
    pub fn source_display(&self, ctx: &TransferContext) -> String {
        match &self.source {
            SourceKind::Registry { registry_override } => crate::mirror::effective_source_ref(
                &self.image,
                registry_override.as_deref(),
            ),
            SourceKind::Local { repository, tag } => {
                format!("{}/{repository}:{tag}", ctx.registry_host)
            }
            SourceKind::DockerArchive { path, inner } => {
                archive_ref("docker-archive", path, inner.as_deref())
            }
            SourceKind::OciArchive { path, inner } => {
                archive_ref("oci-archive", path, inner.as_deref())
            }
        }
    }

    /// skopeo 源 URL（含传输前缀）
    pub fn source_url(&self, ctx: &TransferContext) -> String {
        match &self.source {
            SourceKind::Registry { .. } | SourceKind::Local { .. } => {
                format!("docker://{}", self.source_display(ctx))
            }
            SourceKind::DockerArchive { .. } | SourceKind::OciArchive { .. } => {
                self.source_display(ctx)
            }
        }
    }

    /// skopeo 目标 URL（恒为本地 registry）
    pub fn dest_url(&self, ctx: &TransferContext) -> String {
        format!("docker://{}", self.image.dest_ref(&ctx.registry_host))
    }
}

/// 归档传输前缀（`docker-archive:` / `oci-archive:`）+ `[:<inner>]`
pub fn archive_ref(transport: &str, path: &Path, inner: Option<&str>) -> String {
    match inner.map(str::trim).filter(|s| !s.is_empty()) {
        Some(inner) => format!("{transport}:{}:{inner}", path.display()),
        None => format!("{transport}:{}", path.display()),
    }
}

/// 传输上下文（本地 registry 目标、数据目录、代理与上游凭据）
#[derive(Debug, Clone)]
pub struct TransferContext {
    /// 本地 registry 目标 host（含端口），如 `127.0.0.1:19913`
    pub registry_host: String,
    pub data_dir: PathBuf,
    pub proxy: ProxyConfig,
    pub credential: Option<UpstreamCredential>,
    /// 重试退避的基准时长（生产 = `RETRY_BACKOFF_BASE_SECS`；单测可注入极小值）
    pub retry_backoff_base: Duration,
}

impl TransferContext {
    pub fn tmpdir(&self) -> PathBuf {
        self.data_dir.join("tmp")
    }

    pub fn authfile(&self) -> PathBuf {
        self.data_dir.join("auth.json")
    }
}

/// 执行一次传输（命中凭据先 login 尽力而为 → 注入代理/临时目录 → copy）。
///
/// - `cancel` 被触发时提前返回 `20810 TASK_CANCELLED`，并终止子进程（D-IH-39）
/// - `plan.timeout` 为单次 copy 的超时（D-IH-43）；login 始终使用独立短超时
///
/// 该函数不依赖 spawn，可被单测直接 await。
pub async fn perform_transfer(
    runner: &dyn SkopeoRunner,
    plan: &TransferPlan,
    ctx: &TransferContext,
    log: LogSender,
    cancel: &CancelToken,
) -> Result<(), ImageHubError> {
    if cancel.is_cancelled() {
        return Err(cancelled_error());
    }
    let tmpdir = ctx.tmpdir();
    let authfile = ctx.authfile();
    let authfile_arg = if authfile.exists() {
        Some(authfile.to_string_lossy().to_string())
    } else {
        None
    };

    // 公共环境：代理（按设置）+ TMPDIR（临时文件落应用数据目录）
    let (mut base_env, clear_env) = build_proxy_env(&ctx.proxy, &ctx.registry_host);
    base_env.push((
        "TMPDIR".to_string(),
        tmpdir.to_string_lossy().to_string(),
    ));

    // 1) 命中上游凭据且源为上游 registry → 先 login（尽力而为，失败不中止；独立短超时）
    if plan.needs_login()
        && let Some(cred) = &ctx.credential
    {
        let login_args = build_login_args(&LoginPlan {
            host: cred.host.clone(),
            username: cred.username.clone(),
            tmpdir: tmpdir.to_string_lossy().to_string(),
            authfile: authfile.to_string_lossy().to_string(),
            insecure: plan.insecure,
        });
        let output = runner
            .run(
                SkopeoCommand {
                    argv: login_args,
                    env: base_env.clone(),
                    clear_env: clear_env.clone(),
                    stdin: Some(format!("{}\n", cred.password)),
                    timeout: Some(Duration::from_secs(LOGIN_TIMEOUT_SECS)),
                },
                log.clone(),
                cancel.clone(),
            )
            .await;
        match output {
            Ok(out) if out.success => {
                let _ = log.send(format!("已登录上游 registry {}", cred.host));
            }
            Ok(out) => {
                let _ = log.send(format!(
                    "警告：skopeo login {} 失败（继续尝试匿名拉取）: {}",
                    cred.host,
                    out.tail_lines(3).join(" | ")
                ));
            }
            Err(e) if e.code == codes::TASK_CANCELLED => return Err(e),
            Err(e) => {
                let _ = log.send(format!(
                    "警告：skopeo login {} 执行异常（继续尝试匿名拉取）: {}",
                    cred.host, e.msg
                ));
            }
        }
    }

    if cancel.is_cancelled() {
        return Err(cancelled_error());
    }

    // 2) copy：失败后退避重试（D-IH-45）
    let source_ref = plan.source_display(ctx);
    let source_url = plan.source_url(ctx);
    let source_registry = plan.source_registry(ctx);
    let dest_url = plan.dest_url(ctx);
    let dest_ref = plan.image.dest_ref(&ctx.registry_host);
    let max_attempts = plan.retry_times.saturating_add(1);
    // 单次传输超时按任务配置（D-IH-44）：`Some(ZERO)` = 无限制
    let copy_timeout = Some(
        plan.timeout
            .unwrap_or(Duration::from_secs(DEFAULT_SKOPEO_TIMEOUT_SECS)),
    );

    let mut attempt: u64 = 0;
    loop {
        attempt += 1;
        if cancel.is_cancelled() {
            return Err(cancelled_error());
        }
        if max_attempts > 1 {
            // 稳定格式：供 `progress.rs` 解析为「尝试」事件
            let _ = log.send(format!("第 {attempt}/{max_attempts} 次尝试"));
        }

        let args = build_copy_args(&CopyPlan {
            source_url: source_url.clone(),
            source_registry: source_registry.clone(),
            dest_url: dest_url.clone(),
            dest_host: ctx.registry_host.clone(),
            arch: plan.arch.clone(),
            insecure: plan.insecure,
            tmpdir: tmpdir.to_string_lossy().to_string(),
            authfile: authfile_arg.clone(),
        });
        let outcome = runner
            .run(
                SkopeoCommand {
                    argv: args,
                    env: base_env.clone(),
                    clear_env: clear_env.clone(),
                    stdin: None,
                    timeout: copy_timeout,
                },
                log.clone(),
                cancel.clone(),
            )
            .await;

        let failure: Option<ImageHubError> = match outcome {
            // 用户取消 MUST NOT 重试
            Err(e) if e.code == codes::TASK_CANCELLED => return Err(e),
            // 执行器错误（超时 / 无法启动）：同样计入一次失败并进入重试判定
            Err(e) => Some(e),
            Ok(out) if out.success => {
                let _ = log.send(format!("已推送 {source_ref} → {dest_ref}"));
                return Ok(());
            }
            Ok(out) => Some(ImageHubError::new(
                codes::SKOPEO_FAILED,
                format!(
                    "skopeo copy {} 失败: {}",
                    source_ref,
                    out.tail_lines(5).join(" | ")
                ),
            )),
        };

        let err = failure.expect("非成功分支必有错误");
        if attempt >= max_attempts {
            return Err(err);
        }

        // 退避等待（可被「停止」立即打断）
        let delay = retry_backoff_delay(ctx.retry_backoff_base, attempt);
        let _ = log.send(format!(
            "第 {attempt}/{max_attempts} 次尝试失败，{} 秒后重试：{}",
            delay.as_secs(),
            err.msg
        ));
        let mut cancel_wait = cancel.clone();
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = cancel_wait.cancelled() => return Err(cancelled_error()),
        }
    }
}

// ==================== 归档导入（D-IH-53 ~ D-IH-57） ====================

/// OCI 布局 `index.json` 中承载原始名称的注解键
pub const OCI_REF_NAME_ANNOTATION: &str = "org.opencontainers.image.ref.name";
/// OCI 布局标识文件（与 docker-archive 的 `manifest.json` 区分）
pub const OCI_LAYOUT_FILE: &str = "oci-layout";

/// gzip 魔数
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
/// xz 魔数
const XZ_MAGIC: [u8; 6] = [0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00];
/// `list-tags` 识别超时（秒）：避免超大归档长时间无反馈
const LIST_TAGS_TIMEOUT_SECS: u64 = 300;

/// 归档类型（由**内容**判定，与文件后缀无关）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveKind {
    DockerArchive,
    OciArchive,
}

impl ArchiveKind {
    /// skopeo 传输前缀
    pub fn transport(&self) -> &'static str {
        match self {
            ArchiveKind::DockerArchive => "docker-archive",
            ArchiveKind::OciArchive => "oci-archive",
        }
    }

    /// 对外展示名
    pub fn as_str(&self) -> &'static str {
        self.transport()
    }
}

/// 解析 `skopeo list-tags` 输出（`{"Tags":[...]}`；null / 非法 → 空）
pub fn parse_list_tags(stdout: &str) -> Vec<String> {
    serde_json::from_str::<Value>(stdout.trim())
        .ok()
        .and_then(|v| {
            v.get("Tags").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// 从 OCI 布局 `index.json` 解析原始名称（`manifests[].annotations["...ref.name"]`）：
/// 去空白、保序去重；无法解析 → 空
pub fn parse_oci_index_names(json: &str) -> Vec<String> {
    let Ok(body) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    if let Some(manifests) = body.get("manifests").and_then(Value::as_array) {
        for m in manifests {
            let name = m
                .get("annotations")
                .and_then(|a| a.get(OCI_REF_NAME_ANNOTATION))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(name) = name
                && !out.iter().any(|x| x == name)
            {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// 读取归档起始魔数并复位文件游标（用于识别压缩格式）
fn sniff_magic(file: &mut std::fs::File) -> std::io::Result<Vec<u8>> {
    let mut buf = [0u8; 6];
    let n = file.read(&mut buf)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(buf[..n].to_vec())
}

/// 读取归档内 `index.json` 并解析原始名称：
/// - `Ok(None)`：归档内无 `index.json`（非 OCI 布局）
/// - `Ok(Some(names))`：存在 `index.json`（可能无 ref.name 注解 → 空列表）
///
/// 按魔数识别 gzip / xz（其余按未压缩 tar）；命中即停，不读完整包。
pub fn read_oci_index(path: &Path) -> Result<Option<Vec<String>>, ImageHubError> {
    let io_err = |e: std::io::Error| {
        ImageHubError::new(
            codes::IO_FAILED,
            format!("读取归档失败 {}: {e}", path.display()),
        )
    };
    let mut file = std::fs::File::open(path).map_err(io_err)?;
    let magic = sniff_magic(&mut file).map_err(io_err)?;
    let reader: Box<dyn Read> = if magic.starts_with(&GZIP_MAGIC) {
        Box::new(flate2::read::GzDecoder::new(file))
    } else if magic.starts_with(&XZ_MAGIC) {
        Box::new(xz2::read::XzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut archive = tar::Archive::new(reader);
    // 非 tar / 内容损坏：视为「非 OCI 布局」（由 `detect_archive` 给出统一的「无法识别」提示）
    let Ok(entries) = archive.entries() else {
        return Ok(None);
    };
    for entry in entries {
        let Ok(mut entry) = entry else {
            return Ok(None);
        };
        let is_index = entry
            .path()
            .map(|p| p == Path::new("index.json"))
            .unwrap_or(false);
        if !is_index {
            continue;
        }
        let mut buf = String::new();
        if entry.read_to_string(&mut buf).is_err() {
            return Ok(None);
        }
        return Ok(Some(parse_oci_index_names(&buf)));
    }
    Ok(None)
}

/// 归档识别结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveDetection {
    pub kind: ArchiveKind,
    /// 归档内原始名称（docker-archive 来自 `list-tags`；OCI 来自 `index.json`；可能为空）
    pub names: Vec<String>,
}

/// 识别归档类型并解析原始名称（D-IH-54）：
/// 1. `skopeo list-tags docker-archive:<path>` 成功 → docker-archive；
/// 2. 否则读取 `index.json`（OCI 布局）→ oci-archive；
/// 3. 均不成立 → `20801`（无法识别）。
pub async fn detect_archive(
    runner: &dyn SkopeoRunner,
    path: &Path,
    tmpdir: &Path,
    log: LogSender,
    cancel: &CancelToken,
    timeout: Option<Duration>,
) -> Result<ArchiveDetection, ImageHubError> {
    let argv = vec![
        "--tmpdir".to_string(),
        tmpdir.to_string_lossy().to_string(),
        "list-tags".to_string(),
        archive_ref("docker-archive", path, None),
    ];
    let out = runner
        .run(
            SkopeoCommand {
                argv,
                env: Vec::new(),
                clear_env: Vec::new(),
                stdin: None,
                timeout: Some(timeout.unwrap_or(Duration::from_secs(LIST_TAGS_TIMEOUT_SECS))),
            },
            log.clone(),
            cancel.clone(),
        )
        .await?;
    if out.success {
        let names = parse_list_tags(&out.stdout);
        let _ = log.send(format!(
            "识别为 docker-archive 归档（{} 个原始名称）",
            names.len()
        ));
        return Ok(ArchiveDetection {
            kind: ArchiveKind::DockerArchive,
            names,
        });
    }
    if let Some(names) = read_oci_index(path)? {
        let _ = log.send(format!("识别为 OCI 归档（{} 个原始名称）", names.len()));
        return Ok(ArchiveDetection {
            kind: ArchiveKind::OciArchive,
            names,
        });
    }
    Err(ImageHubError::new(
        codes::PARAM_INVALID,
        "无法识别的镜像归档（既非 docker-archive 也非 OCI 归档），请确认文件为 docker save / skopeo copy 产出的镜像归档",
    ))
}

/// 导入目标（落地引用 + 归档内原始引用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportTarget {
    /// 落地引用（按镜像引用规则归一）
    pub image: ImageRef,
    /// 归档内原始引用（`None` = 使用归档默认引用，整包导入）
    pub inner: Option<String>,
}

/// 从名称中取出显式 tag（最后一个 `:` 之后且不含 `/`）
pub fn explicit_tag(name: &str) -> Option<&str> {
    let name = name.trim();
    match name.rfind(':') {
        Some(i) if !name[i + 1..].contains('/') && !name[i + 1..].is_empty() => {
            Some(&name[i + 1..])
        }
        _ => None,
    }
}

/// 推算导入目标（纯函数，D-IH-57）：
/// - 提供 `repository`：目标名 = `repository`（若自身含 tag 则用之，否则用 `tag`；`tag` 也未提供时，
///   归档仅一个原始名称则沿用其 tag，否则 `latest`）；`inner = None`（归档默认引用）
/// - 未提供 `repository`：对每个原始名称逐个导入并**保留原始名称**（`inner = 原始名称`）；
///   无原始名称 → `20801`
pub fn plan_import_targets(
    repository: Option<&str>,
    tag: Option<&str>,
    archive_names: &[String],
) -> Result<Vec<ImportTarget>, ImageHubError> {
    let repo = repository.map(str::trim).filter(|s| !s.is_empty());
    match repo {
        Some(repo) => {
            let explicit = tag.map(str::trim).filter(|s| !s.is_empty());
            let candidate = if let Some(tag) = explicit {
                format!("{repo}:{tag}")
            } else if explicit_tag(repo).is_some() {
                repo.to_string()
            } else {
                let fallback = if archive_names.len() == 1 {
                    explicit_tag(&archive_names[0]).unwrap_or("latest").to_string()
                } else {
                    "latest".to_string()
                };
                format!("{repo}:{fallback}")
            };
            let image = parse_image_ref(&candidate)?;
            Ok(vec![ImportTarget { image, inner: None }])
        }
        None => {
            if archive_names.is_empty() {
                return Err(ImageHubError::new(
                    codes::PARAM_INVALID,
                    "无法从归档解析原始镜像名称，请填写目标镜像名（repository 与可选 tag）",
                ));
            }
            archive_names
                .iter()
                .map(|name| {
                    let image = parse_image_ref(name)?;
                    Ok(ImportTarget {
                        image,
                        inner: Some(name.trim().to_string()),
                    })
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_of(source: &str, insecure: bool, arch: ArchMode) -> TransferPlan {
        TransferPlan {
            image: parse_image_ref(source).unwrap(),
            arch,
            insecure,
            source: SourceKind::Registry {
                registry_override: None,
            },
            timeout: None,
            retry_times: 0,
        }
    }

    fn plan_with_mirror(source: &str, mirror: &str, arch: ArchMode) -> TransferPlan {
        TransferPlan {
            image: parse_image_ref(source).unwrap(),
            arch,
            insecure: true,
            source: SourceKind::Registry {
                registry_override: Some(mirror.to_string()),
            },
            timeout: None,
            retry_times: 0,
        }
    }

    /// 无需取消能力的令牌（既有用例沿用）
    fn no_cancel() -> CancelToken {
        CancelToken::never()
    }

    fn ctx(tmp: &std::path::Path, cred: Option<UpstreamCredential>) -> TransferContext {
        TransferContext {
            registry_host: "127.0.0.1:19913".to_string(),
            data_dir: tmp.to_path_buf(),
            proxy: ProxyConfig::default(),
            credential: cred,
            // 单测用极小退避基准，避免重试用例真实等待 5s+
            retry_backoff_base: Duration::from_millis(1),
        }
    }

    #[test]
    fn parse_ref_explicit_registry_and_tag() {
        let r = parse_image_ref("docker.m.daocloud.io/library/alpine:3.20").unwrap();
        assert_eq!(r.registry, "docker.m.daocloud.io");
        assert_eq!(r.repository, "library/alpine");
        assert_eq!(r.tag, "3.20");
        assert_eq!(r.upstream_repository(), "docker.m.daocloud.io/library/alpine");
    }

    #[test]
    fn parse_ref_default_tag_and_registry_normalization() {
        let r = parse_image_ref("docker.io/library/ubuntu").unwrap();
        assert_eq!(r.tag, "latest");
        assert_eq!(r.registry, "docker.io");

        // 单段名 → docker.io/library/
        let r = parse_image_ref("ubuntu:22.04").unwrap();
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "library/ubuntu");
        assert_eq!(r.upstream_repository(), "docker.io/library/ubuntu");

        // 无 registry 但有 namespace
        let r = parse_image_ref("ictrek/image-hub:1.0").unwrap();
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "ictrek/image-hub");
    }

    #[test]
    fn parse_ref_host_with_port() {
        let r = parse_image_ref("127.0.0.1:5000/foo/bar:1.0").unwrap();
        assert_eq!(r.registry, "127.0.0.1:5000");
        assert_eq!(r.repository, "foo/bar");
        assert_eq!(r.tag, "1.0");
    }

    #[test]
    fn parse_ref_rejects_invalid() {
        assert_eq!(
            parse_image_ref("docker.io/library/ubuntu@sha256:abc")
                .unwrap_err()
                .code,
            codes::IMAGE_REF_INVALID
        );
        assert_eq!(parse_image_ref("").unwrap_err().code, codes::IMAGE_REF_INVALID);
        assert_eq!(
            parse_image_ref("Bad Image!!").unwrap_err().code,
            codes::IMAGE_REF_INVALID
        );
        assert_eq!(
            parse_image_ref("foo//bar").unwrap_err().code,
            codes::IMAGE_REF_INVALID
        );
    }

    #[test]
    fn split_registry_repository_variants() {
        let (reg, repo) =
            split_registry_repository("docker.m.daocloud.io/library/alpine").unwrap();
        assert_eq!(reg, "docker.m.daocloud.io");
        assert_eq!(repo, "library/alpine");

        let (reg, repo) = split_registry_repository("library/registry").unwrap();
        assert_eq!(reg, "docker.io");
        assert_eq!(repo, "library/registry");

        let (reg, repo) = split_registry_repository("127.0.0.1:5000/foo/bar").unwrap();
        assert_eq!(reg, "127.0.0.1:5000");
        assert_eq!(repo, "foo/bar");

        assert!(split_registry_repository("").is_err());
        assert!(split_registry_repository("foo").is_err());
    }

    #[test]
    fn arch_mode_mapping() {
        assert_eq!(ArchMode::from_request(None).unwrap(), ArchMode::All);
        assert_eq!(ArchMode::from_request(Some("all")).unwrap(), ArchMode::All);
        assert_eq!(ArchMode::from_request(Some("  ")).unwrap(), ArchMode::All);
        assert_eq!(ArchMode::from_request(Some("host")).unwrap(), ArchMode::Host);
        assert_eq!(ArchMode::from_request(Some("system")).unwrap(), ArchMode::Host);
        // 完整平台名原样保留
        assert_eq!(
            ArchMode::from_request(Some("linux/amd64")).unwrap(),
            ArchMode::Specific(vec!["linux/amd64".to_string()])
        );
        assert_eq!(
            ArchMode::from_request(Some("windows/amd64")).unwrap(),
            ArchMode::Specific(vec!["windows/amd64".to_string()])
        );
        // 裸架构名补默认 os（向后兼容）
        assert_eq!(
            ArchMode::from_request(Some("arm64")).unwrap(),
            ArchMode::Specific(vec!["linux/arm64".to_string()])
        );
        // 多平台（去空白 / 去重 / 保持顺序；裸名与完整名归一一致）
        let multi = ArchMode::from_request(Some(" linux/amd64 , linux/arm64,amd64 ")).unwrap();
        assert_eq!(
            multi,
            ArchMode::Specific(vec!["linux/amd64".to_string(), "linux/arm64".to_string()])
        );
        assert_eq!(multi.multi_arch(), "linux/amd64,linux/arm64");
        assert_eq!(multi.as_request_str(), "linux/amd64,linux/arm64");
        // 旧式 arch/variant → 归一为 linux/arm（skopeo 平台列表不支持 variant）
        assert_eq!(
            ArchMode::from_request(Some("arm/v7")).unwrap(),
            ArchMode::Specific(vec!["linux/arm".to_string()])
        );
        // os/arch/variant → 忽略 variant
        assert_eq!(
            ArchMode::from_request(Some("linux/arm/v7"))
                .unwrap()
                .multi_arch(),
            "linux/arm"
        );
        assert_eq!(
            ArchMode::from_request(Some("arm64/v8")).unwrap().multi_arch(),
            "linux/arm64"
        );
        assert_eq!(multi_arch_arg(&ArchMode::All), "all");
        assert_eq!(multi_arch_arg(&ArchMode::Host), "system");
        assert_eq!(
            multi_arch_arg(&ArchMode::Specific(vec!["linux/arm64".to_string()])),
            "linux/arm64"
        );
    }

    #[test]
    fn arch_mode_rejects_invalid_tokens() {
        for bad in [
            "Bad Arch",
            "amd64,",
            ",",
            "amd64,,arm64",
            "-amd64",
            "AMD64",
            "/amd64",
            "linux/",
            "linux//amd64",
            "a/b/c/d",
        ] {
            let err = ArchMode::from_request(Some(bad)).unwrap_err();
            assert_eq!(err.code, codes::IMAGE_REF_INVALID, "input={bad}");
        }
    }

    #[test]
    fn copy_args_multi_arch_selection_strips_removed_platforms() {
        let plan = CopyPlan {
            source_url: "docker://registry.example.com/foo/bar:1.0".into(),
            source_registry: "registry.example.com".into(),
            dest_url: "docker://127.0.0.1:19913/registry.example.com/foo/bar:1.0".into(),
            dest_host: "127.0.0.1:19913".into(),
            arch: ArchMode::Specific(vec!["linux/amd64".to_string(), "linux/arm64".to_string()]),
            insecure: false,
            tmpdir: "/tmp/x".into(),
            authfile: None,
        };
        let argv = build_copy_args(&plan);
        let joined = argv.join(" ");
        assert!(
            joined.contains("--multi-arch linux/amd64,linux/arm64"),
            "{joined}"
        );
        assert!(joined.contains("--strip-removed-platforms"), "{joined}");
    }

    #[test]
    fn copy_args_all_and_host_do_not_strip_platforms() {
        for arch in [ArchMode::All, ArchMode::Host] {
            let plan = CopyPlan {
                source_url: "docker://registry.example.com/foo/bar:1.0".into(),
                source_registry: "registry.example.com".into(),
                dest_url: "docker://127.0.0.1:19913/registry.example.com/foo/bar:1.0".into(),
                dest_host: "127.0.0.1:19913".into(),
                arch,
                insecure: false,
                tmpdir: "/tmp/x".into(),
                authfile: None,
            };
            let joined = build_copy_args(&plan).join(" ");
            assert!(!joined.contains("--strip-removed-platforms"), "{joined}");
        }
    }

    #[test]
    fn copy_args_preserve_upstream_name_and_flags() {
        let plan = CopyPlan {
            source_url: "docker://docker.io/library/ubuntu:22.04".into(),
            source_registry: "docker.io".into(),
            dest_url: "docker://127.0.0.1:19913/docker.io/library/ubuntu:22.04".into(),
            dest_host: "127.0.0.1:19913".into(),
            arch: ArchMode::All,
            insecure: true,
            tmpdir: "/var/lib/image-hub/tmp".into(),
            authfile: None,
        };
        let argv = build_copy_args(&plan);
        let joined = argv.join(" ");
        assert!(joined.contains("--tmpdir /var/lib/image-hub/tmp"));
        assert!(joined.contains("copy --multi-arch all"));
        assert!(joined.contains("--src-tls-verify=false"));
        assert!(joined.contains("--dest-tls-verify=false"));
        assert!(joined.contains("--retry-times 3"));
        assert!(joined.ends_with(
            "docker://docker.io/library/ubuntu:22.04 docker://127.0.0.1:19913/docker.io/library/ubuntu:22.04"
        ));
        assert!(!joined.contains("--authfile"));
    }

    #[test]
    fn copy_args_secure_source_and_authfile() {
        let plan = CopyPlan {
            source_url: "docker://registry.example.com/foo/bar:1.0".into(),
            source_registry: "registry.example.com".into(),
            dest_url: "docker://127.0.0.1:19913/registry.example.com/foo/bar:1.0".into(),
            dest_host: "127.0.0.1:19913".into(),
            arch: ArchMode::Specific(vec!["linux/amd64".to_string()]),
            insecure: false,
            tmpdir: "/tmp/x".into(),
            authfile: Some("/var/lib/image-hub/auth.json".into()),
        };
        let joined = build_copy_args(&plan).join(" ");
        assert!(joined.contains("--multi-arch linux/amd64"));
        assert!(joined.contains("--strip-removed-platforms"));
        assert!(!joined.contains("--src-tls-verify=false"));
        assert!(joined.contains("--dest-tls-verify=false"));
        assert!(joined.contains("--authfile /var/lib/image-hub/auth.json"));
    }

    #[test]
    fn copy_args_archive_source_has_no_src_tls_flag() {
        let plan = CopyPlan {
            source_url: "docker-archive:/tmp/a.tar".into(),
            // 归档源无 registry → 不注入源 TLS 参数
            source_registry: String::new(),
            dest_url: "docker://127.0.0.1:19913/docker.io/library/alpine:3.20".into(),
            dest_host: "127.0.0.1:19913".into(),
            arch: ArchMode::All,
            insecure: true,
            tmpdir: "/tmp/x".into(),
            authfile: None,
        };
        let joined = build_copy_args(&plan).join(" ");
        assert!(!joined.contains("--src-tls-verify"), "{joined}");
        assert!(joined.contains("--dest-tls-verify=false"), "{joined}");
        assert!(joined.ends_with("docker-archive:/tmp/a.tar docker://127.0.0.1:19913/docker.io/library/alpine:3.20"), "{joined}");
    }

    #[test]
    fn login_args_password_via_stdin() {
        let argv = build_login_args(&LoginPlan {
            host: "docker.m.daocloud.io".into(),
            username: "user".into(),
            tmpdir: "/tmp/x".into(),
            authfile: "/var/lib/image-hub/auth.json".into(),
            insecure: true,
        });
        let joined = argv.join(" ");
        assert!(joined.starts_with("--tmpdir /tmp/x login"));
        assert!(joined.contains("-u user"));
        assert!(joined.contains("--password-stdin docker.m.daocloud.io"));
        assert!(joined.contains("--tls-verify=false"));
        // 密码绝不进入 argv
        assert!(!joined.contains("password="));
    }

    #[test]
    fn proxy_env_enabled_and_disabled() {
        let enabled = ProxyConfig {
            enabled: true,
            http_proxy: "http://proxy:3128".into(),
            https_proxy: "http://proxy:3128".into(),
            no_proxy: "example.com".into(),
        };
        let (env, clear) = build_proxy_env(&enabled, "127.0.0.1:19913");
        let map: std::collections::HashMap<_, _> = env.iter().cloned().collect();
        assert_eq!(map.get("http_proxy").unwrap(), "http://proxy:3128");
        assert_eq!(map.get("HTTP_PROXY").unwrap(), "http://proxy:3128");
        assert_eq!(map.get("https_proxy").unwrap(), "http://proxy:3128");
        let no_proxy = map.get("no_proxy").unwrap().clone();
        assert!(no_proxy.contains("example.com"));
        assert!(no_proxy.contains("127.0.0.1"));
        assert!(no_proxy.contains("localhost"));
        assert!(clear.contains(&"all_proxy".to_string()));

        let disabled = ProxyConfig::default();
        let (env, clear) = build_proxy_env(&disabled, "127.0.0.1:19913");
        assert!(env.is_empty());
        for key in PROXY_ENV_KEYS {
            assert!(clear.contains(&key.to_string()), "{key} 应被清除");
        }
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("127.0.0.1:19913"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(!is_loopback_host("docker.io"));
        assert!(!is_loopback_host("10.0.0.5:5000"));
    }

    #[tokio::test]
    async fn perform_transfer_login_then_copy_order() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let credential = Some(UpstreamCredential {
            host: "docker.m.daocloud.io".into(),
            username: "u".into(),
            password: "p".into(),
        });
        let context = ctx(tmp.path(), credential);
        let plan = plan_of("docker.m.daocloud.io/library/alpine:3.20", true, ArchMode::All);

        perform_transfer(&runner, &plan, &context, tx, &no_cancel()).await.unwrap();

        let calls = runner.invocations();
        assert_eq!(calls.len(), 2, "应先 login 再 copy");
        assert_eq!(calls[0].argv[2], "login");
        assert_eq!(calls[0].stdin.as_deref(), Some("p\n"));
        assert_eq!(calls[1].argv[2], "copy");
        // 临时目录指向数据目录
        assert!(calls[1].argv[1].ends_with("/tmp"));
        // 目标引用保留上游完整名
        assert!(calls[1]
            .argv
            .last()
            .unwrap()
            .contains("127.0.0.1:19913/docker.m.daocloud.io/library/alpine:3.20"));

        let mut logs = Vec::new();
        while let Ok(l) = rx.try_recv() {
            logs.push(l);
        }
        assert!(logs.iter().any(|l| l.contains("已登录上游 registry")));
    }

    #[tokio::test]
    async fn perform_transfer_login_failure_does_not_abort() {
        let tmp = tempfile::TempDir::new().unwrap();
        // login 失败 + copy 成功：mock 无法区分，用失败 mock 验证错误映射即可
        let runner = MockSkopeoRunner::failing();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let credential = Some(UpstreamCredential {
            host: "docker.m.daocloud.io".into(),
            username: "u".into(),
            password: "p".into(),
        });
        let context = ctx(tmp.path(), credential);
        let plan = plan_of("docker.m.daocloud.io/library/alpine:3.20", true, ArchMode::All);

        let err = perform_transfer(&runner, &plan, &context, tx, &no_cancel())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::SKOPEO_FAILED);
        // login 失败仍继续执行 copy（两次调用均有）
        assert_eq!(runner.invocations().len(), 2);
    }

    #[tokio::test]
    async fn perform_transfer_without_credential_only_copies() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let context = ctx(tmp.path(), None);
        let plan = plan_of("docker.io/library/ubuntu:22.04", false, ArchMode::Host);

        perform_transfer(&runner, &plan, &context, tx, &no_cancel()).await.unwrap();
        let calls = runner.invocations();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].argv[2], "copy");
        assert!(calls[0].argv.join(" ").contains("--multi-arch system"));
        // TMPDIR 指向数据目录下 tmp/（临时文件不落容器可写层）
        let tmpdir = calls[0]
            .env
            .iter()
            .find(|(k, _)| k == "TMPDIR")
            .map(|(_, v)| v.clone())
            .expect("应注入 TMPDIR");
        assert!(tmpdir.ends_with("/tmp"), "{tmpdir}");
    }

    #[tokio::test]
    async fn perform_transfer_with_mirror_keeps_upstream_dest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let context = ctx(tmp.path(), None);
        // 输入 ubuntu:22.04（归一 docker.io/library/ubuntu），镜像源 docker.m.daocloud.io
        let plan = plan_with_mirror("ubuntu:22.04", "docker.m.daocloud.io", ArchMode::All);

        perform_transfer(&runner, &plan, &context, tx, &no_cancel()).await.unwrap();

        let calls = runner.invocations();
        assert_eq!(calls.len(), 1);
        let joined = calls[0].argv.join(" ");
        // 实际拉取源使用镜像源
        assert!(
            joined.contains("docker://docker.m.daocloud.io/library/ubuntu:22.04"),
            "{joined}"
        );
        // 目标引用恒为上游完整镜像名（镜像源主机名绝不进入落地名）
        assert!(
            joined.contains("docker://127.0.0.1:19913/docker.io/library/ubuntu:22.04"),
            "{joined}"
        );
        assert!(!joined.contains("19913/docker.m.daocloud.io"), "{joined}");
    }

    #[tokio::test]
    async fn perform_transfer_without_mirror_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let context = ctx(tmp.path(), None);
        let plan = plan_of("docker.io/library/ubuntu:22.04", false, ArchMode::Host);
        perform_transfer(&runner, &plan, &context, tx, &no_cancel()).await.unwrap();
        let joined = runner.invocations()[0].argv.join(" ");
        assert!(joined.ends_with(
            "docker://docker.io/library/ubuntu:22.04 docker://127.0.0.1:19913/docker.io/library/ubuntu:22.04"
        ), "{joined}");
    }

    #[test]
    fn transfer_plan_source_accessors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = ctx(tmp.path(), None);
        let plan = plan_with_mirror("ghcr.io/foo/bar:1.0", "ghcr.m.daocloud.io", ArchMode::All);
        assert_eq!(plan.source_registry(&context), "ghcr.m.daocloud.io");
        assert_eq!(plan.source_display(&context), "ghcr.m.daocloud.io/foo/bar:1.0");
        assert_eq!(
            plan.source_url(&context),
            "docker://ghcr.m.daocloud.io/foo/bar:1.0"
        );
        assert!(plan.needs_login());
        assert_eq!(
            plan.dest_url(&context),
            "docker://127.0.0.1:19913/ghcr.io/foo/bar:1.0"
        );

        let plain = plan_of("ghcr.io/foo/bar:1.0", true, ArchMode::All);
        assert_eq!(plain.source_registry(&context), "ghcr.io");
        assert_eq!(plain.source_display(&context), "ghcr.io/foo/bar:1.0");

        // 本地源（克隆/改名）：源 = 本地 registry 引用，无需 login
        let local = TransferPlan {
            image: parse_image_ref("local/alpine:stable").unwrap(),
            arch: ArchMode::All,
            insecure: true,
            source: SourceKind::Local {
                repository: "docker.io/library/alpine".into(),
                tag: "3.20".into(),
            },
            timeout: None,
            retry_times: 0,
        };
        assert!(!local.needs_login());
        assert_eq!(local.source_registry(&context), "127.0.0.1:19913");
        assert_eq!(
            local.source_url(&context),
            "docker://127.0.0.1:19913/docker.io/library/alpine:3.20"
        );
        assert_eq!(
            local.dest_url(&context),
            "docker://127.0.0.1:19913/docker.io/local/alpine:stable"
        );

        // 归档源：无 registry、无 login
        let archive = TransferPlan {
            image: parse_image_ref("docker.io/library/alpine:3.20").unwrap(),
            arch: ArchMode::All,
            insecure: true,
            source: SourceKind::DockerArchive {
                path: PathBuf::from("/data/tmp/a.tar"),
                inner: Some("docker.io/library/alpine:3.20".into()),
            },
            timeout: None,
            retry_times: 0,
        };
        assert!(!archive.needs_login());
        assert_eq!(archive.source_registry(&context), "");
        assert_eq!(
            archive.source_url(&context),
            "docker-archive:/data/tmp/a.tar:docker.io/library/alpine:3.20"
        );

        let oci = TransferPlan {
            source: SourceKind::OciArchive {
                path: PathBuf::from("/data/tmp/b.tar.xz"),
                inner: None,
            },
            ..archive.clone()
        };
        assert_eq!(oci.source_url(&context), "oci-archive:/data/tmp/b.tar.xz");
    }

    #[test]
    fn archive_ref_builds_transport_prefixed_path() {
        assert_eq!(
            archive_ref("docker-archive", Path::new("/tmp/a.tar"), None),
            "docker-archive:/tmp/a.tar"
        );
        assert_eq!(
            archive_ref("oci-archive", Path::new("/tmp/a.tar"), Some(" alpine:3.20 ")),
            "oci-archive:/tmp/a.tar:alpine:3.20"
        );
        assert_eq!(
            archive_ref("oci-archive", Path::new("/tmp/a.tar"), Some("  ")),
            "oci-archive:/tmp/a.tar"
        );
    }

    #[tokio::test]
    async fn login_command_has_short_timeout() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let credential = Some(UpstreamCredential {
            host: "docker.m.daocloud.io".into(),
            username: "u".into(),
            password: "p".into(),
        });
        let context = ctx(tmp.path(), credential);
        let plan = plan_of("docker.m.daocloud.io/library/alpine:3.20", true, ArchMode::All);
        perform_transfer(&runner, &plan, &context, tx, &no_cancel()).await.unwrap();
        // 仅验证 login 先于 copy（超时由执行器内部使用）
        assert_eq!(runner.invocations()[0].argv[2], "login");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_runner_cancel_kills_process() {
        // 以 /bin/sleep 冒充 skopeo：验证取消会终止子进程并返回 20810
        let runner = RealSkopeoRunner::new("/bin/sleep").with_timeout(Duration::from_secs(30));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let started = std::time::Instant::now();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel_tx.send(true).unwrap();
        });
        let err = runner
            .run(
                SkopeoCommand {
                    argv: vec!["30".into()],
                    ..Default::default()
                },
                tx,
                CancelToken::new(cancel_rx),
            )
            .await
            .unwrap_err();
        let _ = handle.await;
        assert_eq!(err.code, codes::TASK_CANCELLED);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn transfer_timeout_validation_and_format() {
        // 缺省 → 60 分钟
        assert_eq!(
            validate_transfer_timeout(None).unwrap(),
            Duration::from_secs(60 * 60)
        );
        assert_eq!(
            validate_transfer_timeout(Some(480)).unwrap(),
            Duration::from_secs(480 * 60)
        );
        assert!(validate_transfer_timeout(Some(1)).is_ok());
        assert!(validate_transfer_timeout(Some(1440)).is_ok());
        // D-IH-44：无上限（远超原 1440 分钟上限亦可）
        assert!(validate_transfer_timeout(Some(4320)).is_ok());
        assert_eq!(
            validate_transfer_timeout(Some(4320)).unwrap(),
            Duration::from_secs(4320 * 60)
        );
        // D-IH-44：0 = 无限制
        assert!(validate_transfer_timeout(Some(0)).unwrap().is_zero());
        // 仅 0 为特例（无限制），不存在上界；故无「越界」用例

        assert_eq!(format_timeout(Duration::from_secs(3600)), "1 小时");
        assert_eq!(format_timeout(Duration::from_secs(90)), "90 秒");
        assert_eq!(format_timeout(Duration::from_secs(480 * 60)), "8 小时");
        assert_eq!(format_timeout(Duration::from_secs(4320 * 60)), "72 小时");
        assert_eq!(format_timeout(Duration::ZERO), "无限制");
    }

    #[test]
    fn no_timeout_upper_bound_anymore() {
        // 原 1440 分钟上限已移除（D-IH-44），极大值仍合法
        assert!(validate_transfer_timeout(Some(100_000)).is_ok());
    }

    #[test]
    fn retry_times_validation_and_backoff_sequence() {
        // 缺省 = 2 次
        assert_eq!(validate_retry_times(None).unwrap(), DEFAULT_RETRY_TIMES);
        for v in 1..=MAX_RETRY_TIMES {
            assert_eq!(validate_retry_times(Some(v)).unwrap(), v);
        }
        assert_eq!(
            validate_retry_times(Some(0)).unwrap_err().code,
            codes::PARAM_INVALID
        );
        assert_eq!(
            validate_retry_times(Some(6)).unwrap_err().code,
            codes::PARAM_INVALID
        );

        // 退避序列：5 / 10 / 20 / 40 / 60 / 60（封顶）
        let secs: Vec<u64> = (1..=6).map(|i| retry_backoff(i).as_secs()).collect();
        assert_eq!(secs, vec![5, 10, 20, 40, 60, 60]);
        // 注入极小基准时仍受 60s 上限约束
        assert_eq!(
            retry_backoff_delay(Duration::from_millis(1), 3),
            Duration::from_millis(4)
        );
    }

    #[tokio::test]
    async fn perform_transfer_retries_until_success_and_logs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::failing_then_ok(1);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let context = ctx(tmp.path(), None);
        let mut plan = plan_of("docker.io/library/ubuntu:22.04", true, ArchMode::All);
        plan.retry_times = 2;

        perform_transfer(&runner, &plan, &context, tx, &no_cancel())
            .await
            .unwrap();

        assert_eq!(runner.call_count(), 2, "第 1 次失败后应重试 1 次");
        let logs: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(logs.iter().any(|l| l == "第 1/3 次尝试"), "日志: {logs:?}");
        assert!(
            logs.iter()
                .any(|l| l.starts_with("第 1/3 次尝试失败，") && l.contains("秒后重试")),
            "日志: {logs:?}"
        );
        assert!(logs.iter().any(|l| l.contains("已推送")));
    }

    #[tokio::test]
    async fn perform_transfer_returns_error_after_exhausting_retries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::failing_then_ok(u64::MAX); // 永不成功
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let context = ctx(tmp.path(), None);
        let mut plan = plan_of("docker.io/library/ubuntu:22.04", true, ArchMode::All);
        plan.retry_times = 3;

        let err = perform_transfer(&runner, &plan, &context, tx, &no_cancel())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::SKOPEO_FAILED);
        assert_eq!(runner.call_count(), 4, "1 次初始 + 3 次重试");
    }

    #[tokio::test]
    async fn cancel_during_retry_backoff_takes_effect_immediately() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut context = ctx(tmp.path(), None);
        context.retry_backoff_base = Duration::from_secs(30); // 远长于断言等待
        let mut plan = plan_of("docker.io/library/ubuntu:22.04", true, ArchMode::All);
        plan.retry_times = 5;

        let (cancel_tx, cancel_rx) = watch::channel(false);
        let cancel = CancelToken::new(cancel_rx);
        let handle = tokio::spawn(async move {
            let runner = MockSkopeoRunner::failing_then_ok(u64::MAX); // 永不成功
            let res = perform_transfer(&runner, &plan, &context, tx, &cancel).await;
            (res, runner.call_count())
        });

        // 等第 1 次失败并进入退避（日志出现「秒后重试」即已进入退避等待）
        let mut entered_backoff = false;
        for _ in 0..200 {
            if let Ok(line) = rx.try_recv()
                && line.contains("秒后重试")
            {
                entered_backoff = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(entered_backoff, "应进入退避等待");
        cancel_tx.send(true).unwrap();

        let (err, calls) = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("退避应可被取消立即打断")
            .unwrap();
        assert_eq!(err.unwrap_err().code, codes::TASK_CANCELLED);
        assert_eq!(calls, 1, "取消后不应再发起重试");
    }

    #[tokio::test]
    async fn perform_transfer_passes_timeout_to_copy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let context = ctx(tmp.path(), None);
        let mut plan = plan_of("docker.io/library/ubuntu:22.04", true, ArchMode::All);
        plan.timeout = validate_transfer_timeout(Some(480)).ok();

        perform_transfer(&runner, &plan, &context, tx, &no_cancel())
            .await
            .unwrap();
        let call = &runner.invocations()[0];
        assert_eq!(call.timeout, Some(Duration::from_secs(480 * 60)));
        assert_eq!(call.argv[2], "copy");
    }

    #[tokio::test]
    async fn perform_transfer_short_circuits_when_already_cancelled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let context = ctx(tmp.path(), None);
        let plan = plan_of("docker.io/library/ubuntu:22.04", true, ArchMode::All);

        let (cancel_tx, cancel_rx) = watch::channel(false);
        cancel_tx.send(true).unwrap();
        let cancel = CancelToken::new(cancel_rx);

        let err = perform_transfer(&runner, &plan, &context, tx, &cancel)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::TASK_CANCELLED);
        assert!(runner.invocations().is_empty(), "取消后不应再启动 skopeo");
    }

    #[tokio::test]
    async fn perform_transfer_cancel_during_copy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::with_delay(10_000);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let context = ctx(tmp.path(), None);
        let plan = plan_of("docker.io/library/ubuntu:22.04", true, ArchMode::All);

        let (cancel_tx, cancel_rx) = watch::channel(false);
        let cancel = CancelToken::new(cancel_rx);
        let handle = tokio::spawn({
            let tx = tx.clone();
            async move {
                // 略等：确保 copy 已启动
                tokio::time::sleep(Duration::from_millis(20)).await;
                cancel_tx.send(true).unwrap();
                tx
            }
        });

        let started = std::time::Instant::now();
        let err = perform_transfer(&runner, &plan, &context, tx, &cancel)
            .await
            .unwrap_err();
        let _ = handle.await;
        assert_eq!(err.code, codes::TASK_CANCELLED);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "取消应立即返回，而非等待 delay 结束"
        );
        assert_eq!(runner.cancelled_count(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_runner_times_out_and_kills_process() {
        // 以 /bin/sleep 冒充 skopeo：验证超时会终止子进程并返回 20807
        let runner = RealSkopeoRunner::new("/bin/sleep").with_timeout(Duration::from_millis(300));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let started = std::time::Instant::now();
        let err = runner
            .run(
                SkopeoCommand {
                    argv: vec!["30".into()],
                    ..Default::default()
                },
                tx,
                no_cancel(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::SKOPEO_FAILED);
        assert!(err.msg.contains("超时"), "{}", err.msg);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    // ==================== 本地复制 / 归档导入 ====================

    #[tokio::test]
    async fn local_source_copy_skips_login_and_uses_local_registry_source() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // 即使命中凭据，本地源也不应 login
        let credential = Some(UpstreamCredential {
            host: "127.0.0.1:19913".into(),
            username: "u".into(),
            password: "p".into(),
        });
        let context = ctx(tmp.path(), credential);
        let plan = TransferPlan {
            image: parse_image_ref("local/alpine:stable").unwrap(),
            arch: ArchMode::All,
            insecure: true,
            source: SourceKind::Local {
                repository: "docker.io/library/alpine".into(),
                tag: "3.20".into(),
            },
            timeout: None,
            retry_times: 0,
        };
        perform_transfer(&runner, &plan, &context, tx, &no_cancel())
            .await
            .unwrap();

        let calls = runner.invocations();
        assert_eq!(calls.len(), 1, "本地源不应执行 login");
        assert_eq!(calls[0].argv[2], "copy");
        let joined = calls[0].argv.join(" ");
        assert!(joined.contains("--multi-arch all"), "{joined}");
        assert!(joined.contains("--src-tls-verify=false"), "{joined}");
        assert!(joined.ends_with(
            "docker://127.0.0.1:19913/docker.io/library/alpine:3.20 docker://127.0.0.1:19913/docker.io/local/alpine:stable"
        ), "{joined}");
    }

    #[tokio::test]
    async fn archive_source_copy_uses_transport_url_and_no_login() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = MockSkopeoRunner::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let credential = Some(UpstreamCredential {
            host: "127.0.0.1:19913".into(),
            username: "u".into(),
            password: "p".into(),
        });
        let context = ctx(tmp.path(), credential);
        let plan = TransferPlan {
            image: parse_image_ref("docker.io/library/alpine:3.20").unwrap(),
            arch: ArchMode::All,
            insecure: true,
            source: SourceKind::OciArchive {
                path: PathBuf::from("/data/tmp/x.tar.xz"),
                inner: None,
            },
            timeout: None,
            retry_times: 0,
        };
        perform_transfer(&runner, &plan, &context, tx, &no_cancel())
            .await
            .unwrap();

        let calls = runner.invocations();
        assert_eq!(calls.len(), 1, "归档源不应执行 login");
        let joined = calls[0].argv.join(" ");
        assert!(!joined.contains("--src-tls-verify"), "{joined}");
        assert!(joined.contains("--dest-tls-verify=false"), "{joined}");
        assert!(joined.contains("oci-archive:/data/tmp/x.tar.xz"), "{joined}");
        assert!(joined.ends_with(
            "oci-archive:/data/tmp/x.tar.xz docker://127.0.0.1:19913/docker.io/library/alpine:3.20"
        ), "{joined}");
    }

    #[test]
    fn parse_list_tags_and_oci_index_names() {
        assert_eq!(
            parse_list_tags(r#"{"Tags":["docker.io/library/alpine:3.20"]}"#),
            vec!["docker.io/library/alpine:3.20"]
        );
        assert!(parse_list_tags(r#"{"Tags":null}"#).is_empty());
        assert!(parse_list_tags("not json").is_empty());

        let index = serde_json::json!({
            "schemaVersion": 2,
            "manifests": [
                {"annotations": {"org.opencontainers.image.ref.name": "docker.io/library/alpine:3.20"}},
                // 无注解条目不产生名称
                {"platform": {"os": "linux", "architecture": "amd64"}},
                {"annotations": {"org.opencontainers.image.ref.name": "  docker.io/library/alpine:3.21  "}},
                // 重复名称去重
                {"annotations": {"org.opencontainers.image.ref.name": "docker.io/library/alpine:3.20"}},
                {"annotations": {"org.opencontainers.image.ref.name": "   "}}
            ]
        });
        assert_eq!(
            parse_oci_index_names(&index.to_string()),
            vec![
                "docker.io/library/alpine:3.20",
                "docker.io/library/alpine:3.21"
            ]
        );
        assert!(parse_oci_index_names("{}").is_empty());
        assert!(parse_oci_index_names("not json").is_empty());
    }

    /// 生成一个含 `index.json`（ref.name 注解）的 OCI 归档（可选 gzip / xz 压缩）
    fn write_oci_archive(path: &Path, names: &[&str], compression: Option<&str>) {
        let manifests: Vec<serde_json::Value> = names
            .iter()
            .map(|n| {
                serde_json::json!({
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                    "size": 123,
                    "annotations": { OCI_REF_NAME_ANNOTATION: n }
                })
            })
            .collect();
        let index = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": manifests
        });
        let payload = serde_json::to_vec(&index).unwrap();

        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "index.json", payload.as_slice())
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let out = match compression {
            Some("gz") => {
                use flate2::write::GzEncoder;
                use std::io::Write;
                let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::default());
                enc.write_all(&tar_bytes).unwrap();
                enc.finish().unwrap()
            }
            Some("xz") => {
                use std::io::Write;
                let mut enc = xz2::write::XzEncoder::new(Vec::new(), 6);
                enc.write_all(&tar_bytes).unwrap();
                enc.finish().unwrap()
            }
            _ => tar_bytes,
        };
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn read_oci_index_handles_tar_tar_gz_and_tar_xz() {
        let tmp = tempfile::TempDir::new().unwrap();
        for (file, compression) in [
            ("a.tar", None),
            ("b.tar.gz", Some("gz")),
            ("c.tar.xz", Some("xz")),
            // 后缀无关：无扩展名同样可读
            ("image", None),
            ("image.gz", Some("gz")),
        ] {
            let path = tmp.path().join(file);
            write_oci_archive(&path, &["docker.io/library/alpine:3.20"], compression);
            let names = read_oci_index(&path).unwrap().expect("应识别出 index.json");
            assert_eq!(names, vec!["docker.io/library/alpine:3.20"], "{file}");
        }

        // 非 OCI 布局（无 index.json）→ None
        let plain = tmp.path().join("plain.tar");
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, "manifest.json", b"{}"[..].as_ref()).unwrap();
        std::fs::write(&plain, builder.into_inner().unwrap()).unwrap();
        assert!(read_oci_index(&plain).unwrap().is_none());
    }

    #[tokio::test]
    async fn detect_archive_recognizes_docker_oci_and_rejects_unknown() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        // list-tags 成功 → docker-archive（Mock 输出无 Tags → 名称为空）
        let ok_runner = MockSkopeoRunner::new();
        let detection = detect_archive(
            &ok_runner,
            &tmp.path().join("docker.tar"),
            tmp.path(),
            tx.clone(),
            &no_cancel(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(detection.kind, ArchiveKind::DockerArchive);
        assert!(detection.names.is_empty());
        let argv = ok_runner.argv_list();
        assert_eq!(argv[0][2], "list-tags");
        assert!(argv[0][3].starts_with("docker-archive:"), "{:?}", argv[0]);

        // list-tags 失败 + 存在 index.json → oci-archive
        let oci_path = tmp.path().join("oci.tar.xz");
        write_oci_archive(&oci_path, &["docker.io/library/alpine:3.20"], Some("xz"));
        let detection = detect_archive(
            &MockSkopeoRunner::failing(),
            &oci_path,
            tmp.path(),
            tx.clone(),
            &no_cancel(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(detection.kind, ArchiveKind::OciArchive);
        assert_eq!(detection.names, vec!["docker.io/library/alpine:3.20"]);

        // 都识别不了 → 20801
        let junk = tmp.path().join("junk.bin");
        std::fs::write(&junk, b"not an archive").unwrap();
        let err = detect_archive(
            &MockSkopeoRunner::failing(),
            &junk,
            tmp.path(),
            tx,
            &no_cancel(),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::PARAM_INVALID);
    }

    #[test]
    fn plan_import_targets_rules() {
        // 提供 repository + tag → 单一目标，使用归档默认引用
        let targets = plan_import_targets(Some("docker.io/library/alpine"), Some("stable"), &[]).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].image.repository, "library/alpine");
        assert_eq!(targets[0].image.tag, "stable");
        assert_eq!(targets[0].inner, None);
        assert_eq!(
            targets[0].image.dest_ref("127.0.0.1:19913"),
            "127.0.0.1:19913/docker.io/library/alpine:stable"
        );

        // 提供 repository、未提供 tag：归档仅一个名称 → 沿用其 tag
        let names = vec!["docker.io/library/alpine:3.20".to_string()];
        let targets = plan_import_targets(Some("local/alpine"), None, &names).unwrap();
        assert_eq!(targets[0].image.tag, "3.20");
        assert_eq!(targets[0].image.repository, "local/alpine");

        // 提供 repository、未提供 tag、归档多个名称 → latest
        let names = vec![
            "docker.io/library/alpine:3.19".to_string(),
            "docker.io/library/alpine:3.20".to_string(),
        ];
        let targets = plan_import_targets(Some("local/alpine"), None, &names).unwrap();
        assert_eq!(targets[0].image.tag, "latest");

        // repository 自带 tag → 使用之
        let targets = plan_import_targets(Some("local/alpine:v1"), None, &names).unwrap();
        assert_eq!(targets[0].image.repository, "local/alpine");
        assert_eq!(targets[0].image.tag, "v1");

        // 未提供 repository → 逐个保留原始名称（inner = 原始名称）
        let targets = plan_import_targets(None, None, &names).unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].image.repository, "library/alpine");
        assert_eq!(targets[0].image.tag, "3.19");
        assert_eq!(targets[0].inner.as_deref(), Some("docker.io/library/alpine:3.19"));
        assert_eq!(targets[1].image.tag, "3.20");

        // 未提供 repository 且无原始名称 → 20801
        let err = plan_import_targets(None, None, &[]).unwrap_err();
        assert_eq!(err.code, codes::PARAM_INVALID);

        // 单段名归一（缺省 docker.io + library）
        let targets = plan_import_targets(Some("alpine-copy"), None, &[]).unwrap();
        assert_eq!(targets[0].image.upstream_repository(), "docker.io/library/alpine-copy");

        // 非法目标名 → 20809
        let err = plan_import_targets(Some("Bad Name"), None, &[]).unwrap_err();
        assert_eq!(err.code, codes::IMAGE_REF_INVALID);
    }
}