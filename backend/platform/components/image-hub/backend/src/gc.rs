//! registry 垃圾回收（GC）域：官方 registry 二进制的执行封装与纯函数。
//!
//! 背景（见 D-IH-59 ~ D-IH-66）：
//! - registry 的 v2 API 只能删除 manifest 引用，镜像层（blob）与未打标签的 manifest revision
//!   必须由官方 `registry garbage-collect` 回收；
//! - 管理面镜像**内置**官方 registry 二进制（构建期从 `registry:3.1.1` 镜像复制），并在 compose
//!   中把 registry 存储目录挂载进管理容器；后端直接以该二进制执行 GC，**不使用 docker socket**，
//!   也不二次封装 registry 镜像；
//! - 官方镜像不含 `/etc/docker/registry/config.yml`，故 GC 的 config 由后端生成（仅含存储驱动）；
//! - `--delete-untagged` 默认开启：改名（按 tag 摘除）等操作留下的未打标签 manifest 只有加该标志
//!   才能回收；`--dry-run` 提供「预演」。
//!
//! 抽象 `RegistryGcRunner`：生产 `RealRegistryGcRunner`（tokio::process），
//! 单测 `MockRegistryGcRunner`。执行结果计数来自 GC 汇总行
//! `N blobs marked, X blobs and Y manifests eligible for deletion`。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::error::{ImageHubError, codes};
use crate::skopeo::{CancelToken, LogSender, cancelled_error};

/// 自动回收间隔默认值（小时）
pub const DEFAULT_GC_INTERVAL_HOURS: u64 = 24;
/// 自动回收间隔下限（小时）
pub const MIN_GC_INTERVAL_HOURS: u64 = 1;
/// 自动回收间隔上限（小时，= 30 天）
pub const MAX_GC_INTERVAL_HOURS: u64 = 720;
/// 数据目录内 GC 临时配置文件相对路径
pub const GC_CONFIG_RELATIVE_PATH: &str = "tmp/gc-config.yml";
/// 单次 GC 执行默认超时（分钟；`0` 表示无限制）
pub const DEFAULT_GC_TIMEOUT_MINUTES: u64 = 60;
/// 默认的镜像内 registry 二进制路径
pub const DEFAULT_REGISTRY_BIN: &str = "/usr/local/bin/registry";
/// 默认的 registry 存储目录（单容器内 registry 服务与 GC 共用）
pub const DEFAULT_REGISTRY_STORAGE_DIR: &str = "/var/lib/registry";

// ==================== 数据结构 ====================

/// 一次 GC 请求（来自 API 或自动调度器；参数已归一）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcRequest {
    /// 是否一并删除未打标签的 manifest（`--delete-untagged`）
    pub delete_untagged: bool,
    /// 预演（`--dry-run`，只统计不删除）
    pub dry_run: bool,
    /// 单次执行超时（`Duration::ZERO` = 无限制）
    pub timeout: Duration,
}

impl GcRequest {
    /// 执行规格（补上由执行器生成的 config 路径）
    pub fn to_spec(&self, config_path: impl Into<PathBuf>) -> GcSpec {
        GcSpec {
            config_path: config_path.into(),
            delete_untagged: self.delete_untagged,
            dry_run: self.dry_run,
            timeout: Some(self.timeout),
        }
    }
}

/// 传给执行器的 GC 规格
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcSpec {
    pub config_path: PathBuf,
    pub delete_untagged: bool,
    pub dry_run: bool,
    /// `None` 使用执行器默认超时；`Some(ZERO)` 表示无限制
    pub timeout: Option<Duration>,
}

/// GC 计数（来自汇总行）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcCounters {
    /// 被标记为「仍被引用」的 blob 数
    pub blobs_marked: u64,
    /// 可删除 / 已删除的 blob 数
    pub blobs_deleted: u64,
    /// 可删除 / 已删除的 manifest 数
    pub manifests_deleted: u64,
}

/// GC 任务结果摘要（`Job.gc`，`None` 兼容旧记录）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcSummary {
    /// 预演（未实际删除）
    pub dry_run: bool,
    /// 是否删除未打标签 manifest
    pub delete_untagged: bool,
    #[serde(flatten)]
    pub counters: GcCounters,
    /// 回收前存储占用（字节；统计失败为 None）
    pub storage_bytes_before: Option<u64>,
    /// 回收后存储占用（字节；统计失败为 None）
    pub storage_bytes_after: Option<u64>,
    /// 释放字节数（前后均可统计时为差值，否则 None）
    pub freed_bytes: Option<u64>,
}

impl GcSummary {
    /// 释放字节数（仅当前后均可统计）
    pub fn compute_freed(before: Option<u64>, after: Option<u64>) -> Option<u64> {
        match (before, after) {
            (Some(b), Some(a)) => Some(b.saturating_sub(a)),
            _ => None,
        }
    }
}

/// GC 子进程输出
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl GcOutput {
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
}

// ==================== 执行抽象 ====================

/// registry GC 执行抽象（便于单测注入 Mock）
#[async_trait]
pub trait RegistryGcRunner: Send + Sync {
    /// 执行一次 GC；输出逐行经 `log` 发送，最终连同退出状态返回。
    ///
    /// `cancel` 被触发时 MUST 终止子进程并返回 `20810 TASK_CANCELLED`。
    async fn run(
        &self,
        spec: &GcSpec,
        log: LogSender,
        cancel: CancelToken,
    ) -> Result<GcOutput, ImageHubError>;

    /// GC 能力是否可用（二进制可解析且存储目录存在）
    fn available(&self) -> bool;

    /// 能力不可用原因（可用时为 None）
    fn unavailable_reason(&self) -> Option<String>;

    /// 能力不可用原因的稳定码（供前端本地化；未知 / 可用时为 `None`）
    ///
    /// 已知值：`registry_binary_missing` / `storage_dir_missing`。
    fn unavailable_code(&self) -> Option<&'static str> {
        None
    }

    /// registry 存储目录（占用统计用；不可用时为 None）
    fn storage_dir(&self) -> Option<&Path>;

    /// 配置的二进制路径（展示用）
    fn bin(&self) -> &str;
}

/// 生产实现：tokio::process 执行内置的 registry 二进制
pub struct RealRegistryGcRunner {
    bin: String,
    storage_dir: PathBuf,
    default_timeout: Duration,
}

impl RealRegistryGcRunner {
    pub fn new(bin: impl Into<String>, storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            storage_dir: storage_dir.into(),
            // GC 可能耗时较久（大存储），默认 60 分钟，可按请求覆盖为无限制
            default_timeout: Duration::from_secs(60 * 60),
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// 解析可执行的二进制路径（绝对路径须存在；相对路径在 PATH 中查找）
    fn resolved_bin(&self) -> Option<PathBuf> {
        let path_env = std::env::var("PATH").unwrap_or_default();
        resolve_registry_bin(&self.bin, &path_env)
    }
}

/// 解析 registry 二进制路径（纯函数，便于单测）：
/// 含 `/` 视为路径并检查存在；否则在 `path_env`（`:` 分隔）中逐个查找可执行文件。
pub fn resolve_registry_bin(bin: &str, path_env: &str) -> Option<PathBuf> {
    let bin = bin.trim();
    if bin.is_empty() {
        return None;
    }
    let candidate = Path::new(bin);
    if bin.contains('/') {
        return if candidate.is_file() { Some(candidate.to_path_buf()) } else { None };
    }
    path_env
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|dir| Path::new(dir).join(bin))
        .find(|p| p.is_file())
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
impl RegistryGcRunner for RealRegistryGcRunner {
    async fn run(
        &self,
        spec: &GcSpec,
        log: LogSender,
        mut cancel: CancelToken,
    ) -> Result<GcOutput, ImageHubError> {
        let bin = self.resolved_bin().ok_or_else(|| {
            ImageHubError::new(
                codes::GC_UNAVAILABLE,
                format!("registry 二进制不可用: {}", self.bin),
            )
        })?;
        let mut command = tokio::process::Command::new(&bin);
        command.args(build_gc_args(spec));
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|e| {
            ImageHubError::new(
                codes::GC_FAILED,
                format!("启动 registry garbage-collect（{}）失败: {e}", bin.display()),
            )
        })?;

        let out_task = child.stdout.take().map(|s| tokio::spawn(drain_lines(s, log.clone())));
        let err_task = child.stderr.take().map(|s| tokio::spawn(drain_lines(s, log.clone())));

        let limit = spec.timeout.unwrap_or(self.default_timeout);

        let waited: Option<Result<std::process::ExitStatus, ImageHubError>> = if limit.is_zero() {
            tokio::select! {
                r = child.wait() => Some(r.map_err(|e| {
                    ImageHubError::new(codes::GC_FAILED, format!("等待 registry GC 结束失败: {e}"))
                })),
                _ = cancel.cancelled() => None,
            }
        } else {
            tokio::select! {
                r = tokio::time::timeout(limit, child.wait()) => match r {
                    Ok(Ok(status)) => Some(Ok(status)),
                    Ok(Err(e)) => Some(Err(ImageHubError::new(
                        codes::GC_FAILED,
                        format!("等待 registry GC 结束失败: {e}"),
                    ))),
                    Err(_) => Some(Err(ImageHubError::new(
                        codes::GC_FAILED,
                        format!("registry GC 执行超时（{}）", format_duration(limit)),
                    ))),
                },
                _ = cancel.cancelled() => None,
            }
        };

        let status = match waited {
            None => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                if let Some(h) = out_task {
                    let _ = h.await;
                }
                if let Some(h) = err_task {
                    let _ = h.await;
                }
                let _ = log.send("垃圾回收已被用户取消，已终止 registry 进程".to_string());
                return Err(cancelled_error());
            }
            Some(Ok(status)) => status,
            Some(Err(e)) => {
                if e.msg.contains("超时") {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    let _ = log.send(format!(
                        "registry GC 执行超时（{}），已终止",
                        format_duration(limit)
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
        Ok(GcOutput {
            success: status.success(),
            stdout,
            stderr,
        })
    }

    fn available(&self) -> bool {
        self.resolved_bin().is_some() && self.storage_dir.is_dir()
    }

    fn unavailable_reason(&self) -> Option<String> {
        if self.resolved_bin().is_none() {
            return Some(format!(
                "registry 二进制不可用（{}）：请使用内置镜像或配置 --registry-bin",
                self.bin
            ));
        }
        if !self.storage_dir.is_dir() {
            return Some(format!(
                "registry 存储目录不存在（{}）：请确认已挂载 registry 存储卷并配置 --registry-storage-dir",
                self.storage_dir.display()
            ));
        }
        None
    }

    fn unavailable_code(&self) -> Option<&'static str> {
        if self.resolved_bin().is_none() {
            return Some("registry_binary_missing");
        }
        if !self.storage_dir.is_dir() {
            return Some("storage_dir_missing");
        }
        None
    }

    fn storage_dir(&self) -> Option<&Path> {
        Some(&self.storage_dir)
    }

    fn bin(&self) -> &str {
        &self.bin
    }
}

// ==================== 纯函数 ====================

/// 构造 GC 参数（不含二进制本身）：`garbage-collect <config> [--delete-untagged] [--dry-run]`
pub fn build_gc_args(spec: &GcSpec) -> Vec<String> {
    let mut args = vec![
        "garbage-collect".to_string(),
        spec.config_path.display().to_string(),
    ];
    if spec.delete_untagged {
        args.push("--delete-untagged".to_string());
    }
    if spec.dry_run {
        args.push("--dry-run".to_string());
    }
    args
}

/// 生成 GC 配置（仅含存储驱动；官方镜像不含自带 config，见 D-IH-60）
pub fn gc_config_yaml(storage_dir: &Path) -> String {
    format!(
        "version: 0.1\nstorage:\n  filesystem:\n    rootdirectory: {}\n",
        storage_dir.display()
    )
}

/// 写入 GC 临时配置（父目录自动创建）
pub fn write_gc_config(path: &Path, storage_dir: &Path) -> Result<(), ImageHubError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            ImageHubError::new(
                codes::IO_FAILED,
                format!("创建 GC 配置目录 {} 失败: {e}", parent.display()),
            )
        })?;
    }
    std::fs::write(path, gc_config_yaml(storage_dir)).map_err(|e| {
        ImageHubError::new(
            codes::IO_FAILED,
            format!("写入 GC 配置 {} 失败: {e}", path.display()),
        )
    })
}

/// 解析 GC 汇总行：`N blobs marked, X blobs and Y manifests eligible for deletion`
pub fn parse_gc_summary(line: &str) -> Option<GcCounters> {
    let line = line.trim();
    let (marked, rest) = line.split_once(" blobs marked, ")?;
    let blobs_marked: u64 = marked.trim().parse().ok()?;
    let (deleted, tail) = rest.split_once(" blobs and ")?;
    let blobs_deleted: u64 = deleted.trim().parse().ok()?;
    let (manifests, _) = tail.split_once(" manifests eligible for deletion")?;
    let manifests_deleted: u64 = manifests.trim().parse().ok()?;
    Some(GcCounters {
        blobs_marked,
        blobs_deleted,
        manifests_deleted,
    })
}

/// 从完整的 GC 输出（多行，stdout/stderr 合并）中提取汇总行计数（纯函数）。
///
/// 汇总行是**单行**（如 `37 blobs marked, 56 blobs and 17 manifests eligible for deletion`），
/// 位于 stdout 末尾；此处按行扫描并取最后一个匹配行。
pub fn parse_gc_summary_from_output(output: &str) -> Option<GcCounters> {
    output.lines().rev().find_map(parse_gc_summary)
}

/// 校验自动回收间隔（小时）：1~720，越界 → `20801`
pub fn validate_gc_interval(hours: u64) -> Result<u64, ImageHubError> {
    if !(MIN_GC_INTERVAL_HOURS..=MAX_GC_INTERVAL_HOURS).contains(&hours) {
        return Err(ImageHubError::new(
            codes::PARAM_INVALID,
            format!(
                "自动回收间隔非法（{hours} 小时）：允许范围 {MIN_GC_INTERVAL_HOURS}~{MAX_GC_INTERVAL_HOURS} 小时"
            ),
        ));
    }
    Ok(hours)
}

/// 解析 RFC3339 时间为 UTC（无效返回 None）
pub fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// 自动回收是否到期（纯函数，D-IH-62）：
/// 基准 = `last_run_at`（无效 / 缺失时用 `fallback`，通常为进程启动时刻）+ interval。
pub fn auto_gc_due(
    enabled: bool,
    last_run_at: Option<&str>,
    fallback: DateTime<Utc>,
    interval_hours: u64,
    now: DateTime<Utc>,
) -> bool {
    if !enabled {
        return false;
    }
    let reference = last_run_at.and_then(parse_rfc3339).unwrap_or(fallback);
    let due = reference + chrono::Duration::hours(interval_hours.min(i32::MAX as u64) as i64);
    now >= due
}

/// 自动回收下次预计触发时间（RFC3339；未启用或无基准时为 None）
pub fn compute_next_run_at(
    enabled: bool,
    last_run_at: Option<&str>,
    interval_hours: u64,
) -> Option<String> {
    if !enabled {
        return None;
    }
    let base = parse_rfc3339(last_run_at?)?;
    let next = base + chrono::Duration::hours(interval_hours.min(i32::MAX as u64) as i64);
    Some(next.to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// 间隔展示文案：整天的整数倍 → 「N 天」，否则「N 小时」
pub fn format_interval(hours: u64) -> String {
    if hours >= 24 && hours.is_multiple_of(24) {
        format!("{} 天", hours / 24)
    } else {
        format!("{hours} 小时")
    }
}

/// GC 完成摘要文案（纯函数，供任务终态进度展示）：
/// - 预演：`预演完成：将删除 X 个 blob / Y 个 manifest`
/// - 实际：`回收完成：删除 X 个 blob / Y 个 manifest`
pub fn gc_done_message(summary: &GcSummary) -> String {
    let verb = if summary.dry_run { "预演完成：将删除" } else { "回收完成：删除" };
    format!(
        "{verb} {} 个 blob / {} 个 manifest",
        summary.counters.blobs_deleted, summary.counters.manifests_deleted
    )
}

/// 时长展示（超时文案；与 skopeo 的 `format_timeout` 语义一致，独立实现避免跨域耦合）
fn format_duration(duration: Duration) -> String {
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

/// 递归统计目录占用字节数（filesystem 驱动下每个 blob 只存一份，不会重复计数）；
/// 根目录不可读 → None；子项读取失败跳过（尽力而为，D-IH-65）。
pub fn dir_size_bytes(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_file() {
        return Some(meta.len());
    }
    if !meta.is_dir() {
        return None;
    }
    let mut total: u64 = 0;
    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) if dir == path => return None,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file()
                && let Ok(m) = entry.metadata()
            {
                total = total.saturating_add(m.len());
            }
        }
    }
    Some(total)
}

// ==================== 测试替身 ====================

/// 内存 mock：记录调用、可注入失败 / 延迟（取消）/ 汇总行
#[derive(Default)]
pub struct MockRegistryGcRunner {
    /// 能力是否可用（默认 true，便于 GC 单测；置 false 验证 20812）
    pub is_available: bool,
    pub storage_dir: Option<PathBuf>,
    pub bin: String,
    /// true 时调用返回失败
    pub fail: bool,
    /// `Some(ms)`：模拟耗时 GC（期间响应取消）
    pub delay_ms: Option<u64>,
    /// 每次调用发送的日志行
    pub log_lines: Vec<String>,
    /// 汇总行（缺省自动按 counters 生成）
    pub summary_line: Option<String>,
    calls: std::sync::Mutex<Vec<GcSpec>>,
    cancelled_calls: std::sync::atomic::AtomicU64,
}

impl MockRegistryGcRunner {
    pub fn new() -> Self {
        Self {
            is_available: true,
            bin: "registry".to_string(),
            ..Default::default()
        }
    }

    /// 可用但存储目录缺失（`storage_dir` 为 None）
    pub fn unavailable() -> Self {
        Self {
            is_available: false,
            ..Self::new()
        }
    }

    /// 模拟耗时 GC（等待期间可取消）
    pub fn with_delay(delay_ms: u64) -> Self {
        Self {
            delay_ms: Some(delay_ms),
            ..Self::new()
        }
    }

    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::new()
        }
    }

    pub fn invocations(&self) -> Vec<GcSpec> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    pub fn cancelled_count(&self) -> u64 {
        self.cancelled_calls
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl RegistryGcRunner for MockRegistryGcRunner {
    async fn run(
        &self,
        spec: &GcSpec,
        log: LogSender,
        mut cancel: CancelToken,
    ) -> Result<GcOutput, ImageHubError> {
        for line in &self.log_lines {
            let _ = log.send(line.clone());
        }
        self.calls.lock().unwrap().push(spec.clone());
        if let Some(delay_ms) = self.delay_ms {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                _ = cancel.cancelled() => {
                    self.cancelled_calls
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _ = log.send("垃圾回收已被用户取消，已终止 registry 进程".to_string());
                    return Err(cancelled_error());
                }
            }
        }
        let summary = self
            .summary_line
            .clone()
            .unwrap_or_else(|| "10 blobs marked, 0 blobs and 0 manifests eligible for deletion".to_string());
        let _ = log.send(summary.clone());
        Ok(GcOutput {
            success: !self.fail,
            stdout: summary,
            stderr: if self.fail {
                "mock gc failure".to_string()
            } else {
                String::new()
            },
        })
    }

    fn available(&self) -> bool {
        self.is_available
    }

    fn unavailable_reason(&self) -> Option<String> {
        (!self.is_available).then(|| "mock GC 能力不可用".to_string())
    }

    fn storage_dir(&self) -> Option<&Path> {
        self.storage_dir.as_deref()
    }

    fn bin(&self) -> &str {
        &self.bin
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn spec(delete_untagged: bool, dry_run: bool) -> GcSpec {
        GcSpec {
            config_path: PathBuf::from("/tmp/gc.yml"),
            delete_untagged,
            dry_run,
            timeout: Some(Duration::from_secs(60)),
        }
    }

    #[test]
    fn build_args_variants() {
        assert_eq!(
            build_gc_args(&spec(false, false)),
            vec!["garbage-collect", "/tmp/gc.yml"]
        );
        assert_eq!(
            build_gc_args(&spec(true, false)),
            vec!["garbage-collect", "/tmp/gc.yml", "--delete-untagged"]
        );
        assert_eq!(
            build_gc_args(&spec(false, true)),
            vec!["garbage-collect", "/tmp/gc.yml", "--dry-run"]
        );
        assert_eq!(
            build_gc_args(&spec(true, true)),
            vec![
                "garbage-collect",
                "/tmp/gc.yml",
                "--delete-untagged",
                "--dry-run"
            ]
        );
    }

    #[test]
    fn config_yaml_points_rootdirectory() {
        let yaml = gc_config_yaml(Path::new("/var/lib/registry"));
        assert!(yaml.contains("version: 0.1"));
        assert!(yaml.contains("rootdirectory: /var/lib/registry"));
    }

    #[test]
    fn write_and_read_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sub").join("gc.yml");
        write_gc_config(&path, Path::new("/data/registry")).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("rootdirectory: /data/registry"));
    }

    #[test]
    fn parse_summary_line() {
        let c = parse_gc_summary("37 blobs marked, 16 blobs and 3 manifests eligible for deletion")
            .unwrap();
        assert_eq!(c.blobs_marked, 37);
        assert_eq!(c.blobs_deleted, 16);
        assert_eq!(c.manifests_deleted, 3);

        // 零删除
        let c = parse_gc_summary("93 blobs marked, 0 blobs and 0 manifests eligible for deletion")
            .unwrap();
        assert_eq!(c.blobs_deleted, 0);

        // 非汇总行
        assert!(parse_gc_summary("").is_none());
        assert!(parse_gc_summary("docker.io/library/alpine").is_none());
        assert!(parse_gc_summary("time=\"...\" level=info msg=\"Deleting layer link path\"").is_none());
        assert!(parse_gc_summary("x blobs marked, 0 blobs and 0 manifests eligible for deletion").is_none());
    }

    #[test]
    fn parse_summary_from_multiline_output() {
        // stdout 汇总行在末尾，stderr 追加在其后：必须按行扫描，不能整串 split
        let output = concat!(
            "gc/probe: layer link eligible for deletion: sha256:abc\n",
            "37 blobs marked, 56 blobs and 17 manifests eligible for deletion\n",
            "time=\"...\" level=info msg=\"Deleting layer link path: /docker/registry/v2/...\"\n",
            "time=\"...\" level=info msg=\"Deleting blob: /docker/registry/v2/...\"\n"
        );
        let c = parse_gc_summary_from_output(output).unwrap();
        assert_eq!(c.blobs_marked, 37);
        assert_eq!(c.blobs_deleted, 56);
        assert_eq!(c.manifests_deleted, 17);

        // 无汇总行
        assert!(parse_gc_summary_from_output("marking blob sha256:x\nmarking manifest sha256:y\n").is_none());
        // 整串传入单行解析会失败（回归保护）
        assert!(parse_gc_summary(output).is_none());
    }

    #[test]
    fn resolve_bin_rules() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bin = tmp.path().join("registry");
        std::fs::write(&bin, b"x").unwrap();

        // 绝对路径存在
        assert_eq!(
            resolve_registry_bin(bin.to_str().unwrap(), ""),
            Some(bin.clone())
        );
        // 绝对路径不存在
        assert!(resolve_registry_bin("/no/such/registry", "").is_none());
        // 相对路径在 PATH 中
        let path_env = format!("/nonexistent:{}", tmp.path().display());
        assert_eq!(resolve_registry_bin("registry", &path_env), Some(bin.clone()));
        // 相对路径不在 PATH
        assert!(resolve_registry_bin("registry", "/nonexistent").is_none());
        // 空
        assert!(resolve_registry_bin("  ", "/bin").is_none());
    }

    #[test]
    fn validate_interval_bounds() {
        assert!(validate_gc_interval(1).is_ok());
        assert!(validate_gc_interval(24).is_ok());
        assert!(validate_gc_interval(720).is_ok());
        assert_eq!(
            validate_gc_interval(0).unwrap_err().code,
            codes::PARAM_INVALID
        );
        assert_eq!(
            validate_gc_interval(721).unwrap_err().code,
            codes::PARAM_INVALID
        );
    }

    #[test]
    fn auto_gc_due_rules() {
        let now = Utc.with_ymd_and_hms(2026, 9, 24, 12, 0, 0).unwrap();
        let fallback = Utc.with_ymd_and_hms(2026, 9, 24, 0, 0, 0).unwrap();
        let last = "2026-09-24T00:00:00Z";

        // 未启用 → 从不触发
        assert!(!auto_gc_due(false, Some(last), fallback, 24, now));
        // 到期（last + 12h <= now）
        assert!(auto_gc_due(true, Some(last), fallback, 12, now));
        // 未到期（last + 24h > now）
        assert!(!auto_gc_due(true, Some(last), fallback, 24, now));
        // 无 last_run_at → 以 fallback 为基准（fallback + 24h > now）
        assert!(!auto_gc_due(true, None, fallback, 24, now));
        assert!(auto_gc_due(true, None, fallback, 12, now));
        // last_run_at 非法 → 退回 fallback
        assert!(!auto_gc_due(true, Some("not-a-time"), fallback, 24, now));
    }

    #[test]
    fn next_run_at_rules() {
        assert_eq!(compute_next_run_at(false, Some("2026-09-24T00:00:00Z"), 24), None);
        assert_eq!(
            compute_next_run_at(true, Some("2026-09-24T00:00:00Z"), 24).as_deref(),
            Some("2026-09-25T00:00:00Z")
        );
        assert_eq!(compute_next_run_at(true, None, 24), None);
        assert_eq!(compute_next_run_at(true, Some("bad"), 24), None);
    }

    #[test]
    fn format_interval_text() {
        assert_eq!(format_interval(1), "1 小时");
        assert_eq!(format_interval(23), "23 小时");
        assert_eq!(format_interval(24), "1 天");
        assert_eq!(format_interval(72), "3 天");
    }

    #[test]
    fn dir_size_counts_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a"), vec![0u8; 100]).unwrap();
        let sub = tmp.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("b"), vec![0u8; 50]).unwrap();
        assert_eq!(dir_size_bytes(tmp.path()), Some(150));

        // 文件路径
        assert_eq!(dir_size_bytes(&tmp.path().join("a")), Some(100));
        // 不存在
        assert!(dir_size_bytes(&tmp.path().join("nope")).is_none());
    }

    #[test]
    fn freed_bytes_requires_both() {
        assert_eq!(GcSummary::compute_freed(Some(100), Some(40)), Some(60));
        assert_eq!(GcSummary::compute_freed(Some(100), None), None);
        assert_eq!(GcSummary::compute_freed(None, Some(40)), None);
        // 防御：after 大于 before 时不为负
        assert_eq!(GcSummary::compute_freed(Some(40), Some(100)), Some(0));
    }

    #[test]
    fn done_message_distinguishes_dry_run() {
        let mut real = GcSummary {
            delete_untagged: true,
            ..Default::default()
        };
        real.counters = GcCounters {
            blobs_marked: 37,
            blobs_deleted: 16,
            manifests_deleted: 2,
        };
        assert_eq!(gc_done_message(&real), "回收完成：删除 16 个 blob / 2 个 manifest");

        let dry = GcSummary {
            dry_run: true,
            ..real.clone()
        };
        assert!(gc_done_message(&dry).starts_with("预演完成"));
    }

    #[tokio::test]
    async fn mock_runner_records_and_exposes_summary() {
        let runner = MockRegistryGcRunner::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out = runner
            .run(&spec(true, false), tx, CancelToken::never())
            .await
            .unwrap();
        assert!(out.success);
        assert_eq!(runner.call_count(), 1);
        let recorded = &runner.invocations()[0];
        assert!(recorded.delete_untagged);
        let mut lines = Vec::new();
        while let Ok(l) = rx.try_recv() {
            lines.push(l);
        }
        assert!(lines.iter().any(|l| l.contains("blobs marked")));
    }

    #[tokio::test]
    async fn mock_runner_returns_cancelled_when_cancel_signalled() {
        let runner = std::sync::Arc::new(MockRegistryGcRunner::with_delay(10_000));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let task = {
            let runner = std::sync::Arc::clone(&runner);
            tokio::spawn(async move {
                runner
                    .run(&spec(true, false), tx, CancelToken::new(cancel_rx))
                    .await
            })
        };
        // 等 mock 记录调用后请求取消
        for _ in 0..100 {
            if runner.call_count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        cancel_tx.send(true).unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert_eq!(err.code, codes::TASK_CANCELLED);
        assert_eq!(runner.cancelled_count(), 1);
    }
}