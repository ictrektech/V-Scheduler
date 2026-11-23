//! 异步任务管理：远程拉取 / 同步为长时任务，进程内调度 + 进度/日志 + JSON 持久化。
//!
//! - `JobManager::start_*` 登记任务后立即返回 `task_id`，执行在 `tokio::spawn` 中进行
//! - 任务日志仅保留最近 N 行；`progress` 为**友好中文摘要**（`progress_detail.message`），
//!   原始输出保留在 `logs` 供排障（D-IH-38）
//! - 运行中任务可经 `cancel()` **协作式取消**：`watch` 通知 → 终止 skopeo 子进程 →
//!   `cancelled` 终态（D-IH-39 / D-IH-40）
//! - 单次上游传输超时按任务配置（缺省 60 分钟；`0` = 无限制，D-IH-44）
//! - 传输失败按任务配置重试（缺省 2 次、指数退避，D-IH-45）
//! - 持久化 `<data-dir>/jobs.json`：创建与终态落盘；启动时把仍运行中的任务标记为
//!   `failed`（服务重启中断），其中已请求取消者标记为 `cancelled`
//! - 执行体调用 `skopeo::perform_transfer`（可单测直接 await，不依赖 spawn）

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::error::{ImageHubError, codes};
use crate::gc::{
    GC_CONFIG_RELATIVE_PATH, GcRequest, GcSummary, RegistryGcRunner, dir_size_bytes, gc_done_message,
    parse_gc_summary, parse_gc_summary_from_output, write_gc_config,
};
use crate::progress::{
    ProgressContext, ProgressDetail, SkopeoEvent, clean_line, phase, parse_skopeo_line,
};
use crate::registry::{RegistryClient, delete_image};
use crate::settings::{SettingsStore, find_credential, write_json_atomic};
use crate::skopeo::{
    ArchMode, ArchiveKind, CancelToken, DEFAULT_RETRY_TIMES, ImageRef, RETRY_BACKOFF_BASE_SECS,
    SkopeoRunner, SourceKind, TransferContext, TransferPlan, detect_archive, perform_transfer,
    plan_import_targets,
};

/// 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    /// 已取消（用户中途停止，终态）
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
        )
    }

    /// 字符串形式（对外展示 / 响应）
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
}

/// 一个异步任务
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// `pull` / `sync`
    pub kind: String,
    pub title: String,
    /// 上游来源（**实际拉取源**：镜像完整引用或仓库名；使用镜像源时为 `<mirror>/<repo>:<tag>`）
    pub source: String,
    /// 本地 registry 目标引用（恒为上游完整镜像名派生，不含镜像源主机名）
    pub dest: String,
    /// 本次使用的镜像源主机名（未使用为 `None`；旧记录缺字段时按 `None` 加载）
    #[serde(default)]
    pub mirror: Option<String>,
    /// 架构选择（`all` / `host` / 具体架构）
    pub arch: String,
    pub status: JobStatus,
    /// **友好中文进度摘要**（取自 `progress_detail.message`；无法识别的事件回落原始行）
    pub progress: String,
    /// 结构化进度（阶段 / 计数 / 百分比）
    #[serde(default)]
    pub progress_detail: ProgressDetail,
    /// 是否已请求停止（UI 据此显示「正在停止…」）
    #[serde(default)]
    pub cancel_requested: bool,
    /// 单次上游传输超时（分钟；`0` 表示无限制，D-IH-44）
    #[serde(default = "default_timeout_minutes")]
    pub timeout_minutes: u64,
    /// 传输失败重试次数（D-IH-45）；旧记录缺字段时为默认值
    #[serde(default = "default_retry_times")]
    pub retry_times: u64,
    /// 最近日志（仅保留尾部 N 行，原始输出）
    pub logs: Vec<String>,
    /// registry 垃圾回收结果摘要（仅 `kind = gc` 任务；`None` 兼容旧记录 / 非 GC 任务）
    #[serde(default)]
    pub gc: Option<GcSummary>,
    pub created_at: String,
    pub finished_at: Option<String>,
    /// 失败原因原文（后端调试文案；前端优先用 `error_code` 本地化）
    pub error: Option<String>,
    /// 失败错误码（image-hub 208xx；成功 / 取消 / 旧记录为 `None`）
    #[serde(default)]
    pub error_code: Option<i32>,
}

/// 旧记录缺 `timeout_minutes` 时的缺省值（分钟）
fn default_timeout_minutes() -> u64 {
    crate::skopeo::DEFAULT_TRANSFER_TIMEOUT_MINUTES
}

/// 旧记录缺 `retry_times` 时的缺省值
fn default_retry_times() -> u64 {
    DEFAULT_RETRY_TIMES
}

impl Job {
    /// 追加一行**原始日志**（清洗空行、`\r` 行与超长行；裁剪到上限），不改动进度
    pub fn push_log(&mut self, line: impl Into<String>, max_logs: usize) {
        let line = clean_line(&line.into());
        if line.is_empty() {
            return;
        }
        self.logs.push(line);
        if self.logs.len() > max_logs {
            let overflow = self.logs.len() - max_logs;
            self.logs.drain(0..overflow);
        }
    }

    /// 观察一行输出：写入原始日志 + 解析事件更新结构化进度 / 友好摘要（D-IH-38）
    pub fn observe_line(
        &mut self,
        line: impl Into<String>,
        max_logs: usize,
        ctx: &ProgressContext,
    ) {
        let line = clean_line(&line.into());
        if line.is_empty() {
            return;
        }
        self.push_log(line.clone(), max_logs);
        let event = parse_skopeo_line(&line);
        match event {
            SkopeoEvent::Other => {
                // 无法识别：摘要回落原始行（保证仍有可读线索）
                self.progress = line;
            }
            event => {
                self.progress_detail.apply(&event, ctx);
                self.progress = self.progress_detail.message.clone();
            }
        }
    }
}

/// 任务 ID 生成（纯函数，便于单测）：`<kind>-<millis>-<seq>`
pub fn next_job_id(kind: &str, millis: u64, seq: u64) -> String {
    format!("{kind}-{millis}-{seq}")
}

/// 删除文件（不存在视为成功；其余失败仅告警）
fn remove_file_quiet(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(path = %path.display(), err = %e, "删除临时文件失败"),
    }
}

/// 当前 Unix 毫秒
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// RFC3339（秒精度，UTC）
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// 改名的源引用清理动作（D-IH-51 / D-IH-58）：**全部复制成功后**执行
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupAction {
    /// 摘除单个标签（`DELETE .../manifests/<tag>`，仅去标签，不影响同 digest 的其它 tag）
    Untag { repository: String, tag: String },
    /// 删除整个源仓库（按 digest 去重删除全部标签）
    DeleteRepository { repository: String },
}

/// 本地复制目标（克隆 / 改名：一个源 tag → 一个目标引用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCopyTarget {
    pub source_tag: String,
    /// 目标引用（registry / repository / tag）
    pub target: ImageRef,
}

/// 归档导入参数（任务标题 / 目标名 / 传输配置）
#[derive(Debug, Clone)]
pub struct ImportSpec {
    pub path: PathBuf,
    /// 上传时的原始文件名（用于任务标题 / 来源展示；缺省退化为临时文件名）
    pub original_name: Option<String>,
    pub requested_repository: Option<String>,
    pub requested_tag: Option<String>,
    pub transfer_timeout: Duration,
    pub retry_times: u64,
}

/// 任务管理器
pub struct JobManager {
    runner: Arc<dyn SkopeoRunner>,
    /// registry 垃圾回收执行器（D-IH-59 / D-IH-64）
    gc: Arc<dyn RegistryGcRunner>,
    settings: Arc<dyn SettingsStore>,
    /// registry 客户端（改名清理：摘标签 / 删除源仓库）
    registry: Arc<dyn RegistryClient>,
    registry_host: String,
    data_dir: PathBuf,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    /// 运行中任务的取消发送端（key = 任务 id）
    running: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    seq: AtomicU64,
    max_logs: usize,
    max_jobs: usize,
    /// 传输失败重试的退避基准时长（生产 = `RETRY_BACKOFF_BASE_SECS`；单测注入极小值）
    retry_backoff_base: Duration,
}

impl JobManager {
    /// 默认日志行上限
    pub const DEFAULT_MAX_LOGS: usize = 300;
    /// 默认保留任务数
    pub const DEFAULT_MAX_JOBS: usize = 100;

    /// 构造并从 `<data-dir>/jobs.json` 加载历史任务（运行中 → failed 中断 / cancelled），
    /// 并清理遗留的导入临时文件（`<data-dir>/tmp/import-*.bin`）
    pub fn new(
        runner: Arc<dyn SkopeoRunner>,
        gc: Arc<dyn RegistryGcRunner>,
        settings: Arc<dyn SettingsStore>,
        registry: Arc<dyn RegistryClient>,
        registry_host: impl Into<String>,
        data_dir: impl Into<PathBuf>,
    ) -> Self {
        let data_dir = data_dir.into();
        let manager = Self {
            runner,
            gc,
            settings,
            registry,
            registry_host: registry_host.into(),
            data_dir: data_dir.clone(),
            jobs: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(Mutex::new(HashMap::new())),
            seq: AtomicU64::new(0),
            max_logs: Self::DEFAULT_MAX_LOGS,
            max_jobs: Self::DEFAULT_MAX_JOBS,
            retry_backoff_base: Duration::from_secs(RETRY_BACKOFF_BASE_SECS),
        };
        manager.load_persisted();
        manager.cleanup_import_temps();
        manager
    }

    /// 清理遗留的导入临时文件（服务重启后运行中任务已判定为中断，文件不再需要）
    fn cleanup_import_temps(&self) {
        let tmp = self.data_dir.join("tmp");
        let Ok(entries) = std::fs::read_dir(&tmp) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("import-")
                && let Err(e) = std::fs::remove_file(entry.path())
            {
                tracing::warn!(path = %entry.path().display(), err = %e, "清理遗留导入临时文件失败");
            }
        }
    }

    fn jobs_path(&self) -> PathBuf {
        self.data_dir.join("jobs.json")
    }

    fn load_persisted(&self) {
        let path = self.jobs_path();
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let mut persisted: Vec<Job> = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(path = %path.display(), err = %e, "解析任务记录失败，忽略");
                return;
            }
        };
        for job in persisted.iter_mut() {
            if job.status.is_terminal() {
                // 旧记录（无 progress_detail / 阶段为初始值）补全终态阶段，
                // 避免前端历史任务显示为「等待执行」；`progress` 已有值时保留
                if job.progress_detail.phase == phase::QUEUED {
                    let (terminal_phase, message) = match job.status {
                        JobStatus::Succeeded => (phase::DONE, "任务完成"),
                        JobStatus::Cancelled => (phase::CANCELLED, "任务已取消"),
                        JobStatus::Failed => (phase::FAILED, "任务失败"),
                        _ => (phase::QUEUED, "等待执行"),
                    };
                    job.progress_detail.mark_terminal(terminal_phase, message);
                    if job.progress.trim().is_empty() {
                        job.progress = job.progress_detail.message.clone();
                    }
                }
                continue;
            }
            // 运行中任务在重启后都已中断；已请求取消者按「已取消」处置（D-IH-40）
            job.finished_at = Some(now_rfc3339());
            if job.cancel_requested {
                job.status = JobStatus::Cancelled;
                job.progress_detail
                    .mark_terminal(phase::CANCELLED, "任务已取消（服务重启）");
                job.progress = "任务已取消（服务重启）".to_string();
                job.error = Some("服务重启，任务已取消".to_string());
            } else {
                job.status = JobStatus::Failed;
                job.error = Some("服务重启，任务中断".to_string());
                job.progress_detail
                    .mark_terminal(phase::FAILED, "任务中断（服务重启）");
                job.progress = "任务中断（服务重启）".to_string();
            }
        }
        let mut map = self.jobs.lock().unwrap();
        for job in persisted {
            map.insert(job.id.clone(), job);
        }
    }

    fn persist(&self) {
        let snapshot = self.list();
        if let Err(e) = write_json_atomic(&self.jobs_path(), &snapshot) {
            tracing::warn!(err = %e.msg, "写入任务记录失败");
        }
    }

    fn alloc_id(&self, kind: &str) -> String {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        next_job_id(kind, now_millis(), seq)
    }

    /// 启动拉取任务
    ///
    /// - `mirror` 为可选镜像源主机名：仅作为**实际拉取来源**，不影响落地仓库名（D-IH-32）
    /// - `transfer_timeout` 为单次上游传输超时（`Duration::ZERO` = 无限制，D-IH-44）
    /// - `retry_times` 为传输失败后的重试次数（D-IH-45）
    #[allow(clippy::too_many_arguments)]
    pub fn start_pull(
        self: &Arc<Self>,
        image: ImageRef,
        arch: ArchMode,
        insecure: bool,
        mirror: Option<String>,
        transfer_timeout: Duration,
        retry_times: u64,
    ) -> String {
        let dest = image.dest_ref(&self.registry_host);
        let plan = TransferPlan {
            image,
            arch,
            insecure,
            source: SourceKind::Registry {
                registry_override: mirror.clone(),
            },
            timeout: Some(transfer_timeout),
            retry_times,
        };
        let title = format!("拉取 {}", plan.image.reference());
        let source = plan.source_display(&self.display_ctx());
        let arch_str = plan.arch.as_request_str();
        self.spawn_job(
            "pull",
            title,
            source,
            dest,
            mirror,
            transfer_timeout,
            retry_times,
            arch_str,
            vec![plan],
            None,
        )
    }

    /// 启动同步任务（对给定 tag 列表逐个重新拉取）
    ///
    /// `mirror` 语义与拉取一致：仅改变实际拉取来源，本地仓库名不变。
    /// `transfer_timeout` 为**每个标签各自**的单次传输超时（D-IH-44；`Duration::ZERO` = 无限制）；
    /// `retry_times` 为**每个标签各自**的失败重试次数（D-IH-45）。
    #[allow(clippy::too_many_arguments)]
    pub fn start_sync(
        self: &Arc<Self>,
        registry: String,
        repository: String,
        tags: Vec<String>,
        arch: ArchMode,
        insecure: bool,
        mirror: Option<String>,
        transfer_timeout: Duration,
        retry_times: u64,
    ) -> String {
        let upstream_repo = format!("{registry}/{repository}");
        let plans: Vec<TransferPlan> = tags
            .into_iter()
            .map(|tag| TransferPlan {
                image: ImageRef {
                    registry: registry.clone(),
                    repository: repository.clone(),
                    tag,
                },
                arch: arch.clone(),
                insecure,
                source: SourceKind::Registry {
                    registry_override: mirror.clone(),
                },
                timeout: Some(transfer_timeout),
                retry_times,
            })
            .collect();

        let title = match plans.len() {
            1 => format!("同步 {}:{}", upstream_repo, plans[0].image.tag),
            n => format!("同步 {upstream_repo}（{n} 个标签）"),
        };
        let source = plans
            .first()
            .map(|p| p.source_display(&self.display_ctx()))
            .unwrap_or_else(|| upstream_repo.clone());
        let dest = plans
            .first()
            .map(|p| p.image.dest_ref(&self.registry_host))
            .unwrap_or_default();
        let arch_str = arch.as_request_str();
        self.spawn_job(
            "sync",
            title,
            source,
            dest,
            mirror,
            transfer_timeout,
            retry_times,
            arch_str,
            plans,
            None,
        )
    }

    /// 展示用的传输上下文（仅用于拼接 `source` 等展示字段，不含凭据）
    fn display_ctx(&self) -> TransferContext {
        TransferContext {
            registry_host: self.registry_host.clone(),
            data_dir: self.data_dir.clone(),
            proxy: Default::default(),
            credential: None,
            retry_backoff_base: self.retry_backoff_base,
        }
    }

    /// 启动本地复制任务（克隆 / 改名，D-IH-50 ~ D-IH-52）
    ///
    /// - `rename = true` 时在**全部复制成功后**执行 `cleanup`（摘标签 / 删除源仓库，D-IH-51）
    /// - `targets` 每项对应一个源 tag → 目标引用（整仓改名为 N 项，tag 名保持）
    pub fn start_local_copy(
        self: &Arc<Self>,
        rename: bool,
        source_repository: &str,
        targets: Vec<LocalCopyTarget>,
        cleanup: Option<CleanupAction>,
        transfer_timeout: Duration,
        retry_times: u64,
    ) -> String {
        let plans: Vec<TransferPlan> = targets
            .iter()
            .map(|t| TransferPlan {
                image: t.target.clone(),
                arch: ArchMode::All,
                insecure: true,
                source: SourceKind::Local {
                    repository: source_repository.to_string(),
                    tag: t.source_tag.clone(),
                },
                timeout: Some(transfer_timeout),
                retry_times,
            })
            .collect();
        let kind = if rename { "rename" } else { "clone" };
        let verb = if rename { "改名" } else { "克隆" };
        let source_prefix = format!("{}/{}", self.registry_host, source_repository);
        let first = targets
            .first()
            .expect("克隆/改名至少一个目标（handler 保证）");
        let title = match targets.len() {
            1 => format!(
                "{verb} {}:{} → {}",
                source_prefix,
                first.source_tag,
                first.target.dest_ref(&self.registry_host)
            ),
            n => format!(
                "{verb} {source_prefix} → {}/{}（{n} 个标签）",
                self.registry_host,
                first.target.upstream_repository()
            ),
        };
        let source = format!("{source_prefix}:{}", first.source_tag);
        let dest = first.target.dest_ref(&self.registry_host);
        self.spawn_job(
            kind,
            title,
            source,
            dest,
            None,
            transfer_timeout,
            retry_times,
            "all".to_string(),
            plans,
            cleanup,
        )
    }

    /// 启动归档导入任务（D-IH-53 ~ D-IH-57）：归档解析在**任务执行体内**完成，
    /// 解析后回填标题 / 源 / 目标，再逐个导入；终态清理上传的临时文件。
    pub fn start_import(self: &Arc<Self>, spec: ImportSpec) -> String {
        let file_name = spec.original_name.clone().unwrap_or_else(|| {
            spec.path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        });
        let (id, cancel_rx) = self.register_job(
            "import",
            format!("导入镜像归档 {file_name}"),
            file_name,
            String::new(),
            None,
            spec.transfer_timeout,
            spec.retry_times,
            "all".to_string(),
            1,
        );
        let this = Arc::clone(self);
        let task_id = id.clone();
        tokio::spawn(async move {
            this.run_import_job(&task_id, spec, cancel_rx).await;
            this.running.lock().unwrap().remove(&task_id);
        });
        id
    }

    /// 启动 registry 垃圾回收任务（D-IH-59 ~ D-IH-66）
    ///
    /// 边界：
    /// - GC 能力不可用（二进制缺失 / 存储目录未配置）→ `20812`；
    /// - 存在运行中任务（拉取/同步/克隆/改名/导入/GC）→ `20801`（串行互斥，D-IH-63）。
    pub fn start_gc(self: &Arc<Self>, request: GcRequest) -> Result<String, ImageHubError> {
        if let Some(reason) = self.gc.unavailable_reason() {
            return Err(ImageHubError::new(codes::GC_UNAVAILABLE, reason));
        }
        if self.has_running() {
            return Err(ImageHubError::new(
                codes::PARAM_INVALID,
                "有任务正在执行（镜像传输或垃圾回收），请稍后再试",
            ));
        }

        let title = if request.dry_run {
            "垃圾回收（预演）".to_string()
        } else {
            "垃圾回收".to_string()
        };
        let source = self
            .gc
            .storage_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| self.gc.bin().to_string());
        let (id, cancel_rx) = self.register_job(
            "gc",
            title,
            source,
            String::new(),
            None,
            request.timeout,
            1,
            "all".to_string(),
            1,
        );
        let this = Arc::clone(self);
        let task_id = id.clone();
        tokio::spawn(async move {
            this.run_gc_job(&task_id, request, cancel_rx).await;
            this.running.lock().unwrap().remove(&task_id);
        });
        Ok(id)
    }

    /// 执行 GC 任务：占用统计 → 生成 config → 执行 → 解析计数 → 终态
    async fn run_gc_job(&self, id: &str, request: GcRequest, cancel_rx: watch::Receiver<bool>) {
        let cancel = CancelToken::new(cancel_rx);
        let dry_run = request.dry_run;
        let start_message = if dry_run {
            "正在预演垃圾回收（不会删除任何数据）…"
        } else {
            "正在扫描并回收未被引用的镜像层…"
        };
        self.update_job(id, |job| {
            job.progress_detail.phase = phase::GC.to_string();
            job.progress_detail.message = start_message.to_string();
            job.progress = start_message.to_string();
            job.gc = Some(GcSummary {
                dry_run,
                delete_untagged: request.delete_untagged,
                ..Default::default()
            });
        });
        self.persist();

        let storage_dir = self.gc.storage_dir().map(|p| p.to_path_buf());
        let Some(storage_dir) = storage_dir else {
            self.finish(
                id,
                Some(ImageHubError::new(
                    codes::GC_UNAVAILABLE,
                    "registry 存储目录不可用",
                )),
                false,
            );
            return;
        };

        let before = self.storage_size(Some(storage_dir.clone())).await;
        let config_path = self.data_dir.join(GC_CONFIG_RELATIVE_PATH);
        if let Err(e) = write_gc_config(&config_path, &storage_dir) {
            self.finish(id, Some(e), false);
            return;
        }

        let spec = request.to_spec(&config_path);
        let log = self.log_sender_gc(id);
        let outcome = self.gc.run(&spec, log, cancel).await;
        let after = self.storage_size(Some(storage_dir)).await;

        let (error, cancelled, success) = match outcome {
            Ok(out) if out.success => {
                let counters = parse_gc_summary_from_output(&out.combined()).unwrap_or_default();
                self.update_job(id, |job| {
                    if let Some(gc) = job.gc.as_mut() {
                        gc.counters = counters.clone();
                        gc.storage_bytes_before = before;
                        gc.storage_bytes_after = after;
                        gc.freed_bytes = GcSummary::compute_freed(before, after);
                    }
                });
                (None, false, true)
            }
            Ok(_) => (
                Some(ImageHubError::new(
                    codes::GC_FAILED,
                    "registry GC 执行失败（详见日志）",
                )),
                false,
                false,
            ),
            Err(e) if e.code == codes::TASK_CANCELLED => (None, true, false),
            Err(e) => (Some(e), false, false),
        };

        self.finish(id, error, cancelled);
        if success {
            // 终态进度文案用 GC 摘要（含删除计数），比通用「任务完成」更可读
            let message = self
                .get(id)
                .ok()
                .and_then(|j| j.gc)
                .map(|gc| gc_done_message(&gc))
                .unwrap_or_else(|| "任务完成".to_string());
            self.update_job(id, |job| {
                job.progress_detail.mark_terminal(phase::DONE, message.clone());
                job.progress = message;
            });
            self.persist();
        }
    }

    /// GC 输出行 drain：原始行入日志；汇总行更新计数与进度摘要
    fn log_sender_gc(&self, id: &str) -> crate::skopeo::LogSender {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let jobs = Arc::clone(&self.jobs);
        let job_id = id.to_string();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let clean = clean_line(&line);
                if clean.is_empty() {
                    continue;
                }
                let counters = parse_gc_summary(&clean);
                if let Ok(mut map) = jobs.lock()
                    && let Some(job) = map.get_mut(&job_id)
                {
                    job.push_log(clean, JobManager::DEFAULT_MAX_LOGS);
                    if let Some(c) = counters {
                        if let Some(gc) = job.gc.as_mut() {
                            gc.counters = c.clone();
                        }
                        job.progress_detail.message = format!(
                            "垃圾回收进行中（已删除 {} 个 blob / {} 个 manifest）",
                            c.blobs_deleted, c.manifests_deleted
                        );
                        job.progress = job.progress_detail.message.clone();
                    }
                }
            }
        });
        tx
    }

    /// 统计 registry 存储目录占用（阻塞 IO 移出运行时；失败 / 无目录 → None）
    async fn storage_size(&self, dir: Option<PathBuf>) -> Option<u64> {
        let dir = dir?;
        tokio::task::spawn_blocking(move || dir_size_bytes(&dir))
            .await
            .ok()
            .flatten()
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_job(
        self: &Arc<Self>,
        kind: &str,
        title: String,
        source: String,
        dest: String,
        mirror: Option<String>,
        transfer_timeout: Duration,
        retry_times: u64,
        arch: String,
        plans: Vec<TransferPlan>,
        cleanup: Option<CleanupAction>,
    ) -> String {
        let items_total = plans.len().max(1) as u64;
        let (id, cancel_rx) = self.register_job(
            kind,
            title,
            source,
            dest,
            mirror,
            transfer_timeout,
            retry_times,
            arch,
            items_total,
        );

        let this = Arc::clone(self);
        let task_id = id.clone();
        tokio::spawn(async move {
            this.run_job(&task_id, plans, cleanup, cancel_rx).await;
            this.running.lock().unwrap().remove(&task_id);
        });
        id
    }

    /// 登记任务并返回 `(task_id, 取消接收端)`
    #[allow(clippy::too_many_arguments)]
    fn register_job(
        &self,
        kind: &str,
        title: String,
        source: String,
        dest: String,
        mirror: Option<String>,
        transfer_timeout: Duration,
        retry_times: u64,
        arch: String,
        items_total: u64,
    ) -> (String, watch::Receiver<bool>) {
        let id = self.alloc_id(kind);
        let items_total = items_total.max(1);
        let mut progress_detail = ProgressDetail::queued(items_total);
        progress_detail.items_total = items_total;
        // 最大尝试次数在登记时即可展示（= 1 + 重试次数）
        progress_detail.max_attempts = retry_times.saturating_add(1);
        let job = Job {
            id: id.clone(),
            kind: kind.to_string(),
            title,
            source,
            dest,
            mirror,
            arch,
            status: JobStatus::Running,
            progress: progress_detail.message.clone(),
            progress_detail,
            cancel_requested: false,
            timeout_minutes: transfer_timeout.as_secs() / 60,
            retry_times,
            logs: Vec::new(),
            gc: None,
            created_at: now_rfc3339(),
            finished_at: None,
            error: None,
            error_code: None,
        };
        self.jobs.lock().unwrap().insert(id.clone(), job);
        self.persist();
        tracing::info!(job_id = %id, kind, "启动镜像任务");

        // 取消通道：登记发送端，执行体持接收端
        let (cancel_tx, cancel_rx) = watch::channel(false);
        self.running.lock().unwrap().insert(id.clone(), cancel_tx);
        (id, cancel_rx)
    }

    /// 直接观察一行输出（任务元信息回填 / 清理阶段日志）
    fn observe_job_line(&self, id: &str, line: &str) {
        let ctx = ProgressContext {
            item_index: 1,
            items_done: 0,
            items_total: 1,
        };
        if let Ok(mut map) = self.jobs.lock()
            && let Some(job) = map.get_mut(id)
        {
            job.observe_line(line, Self::DEFAULT_MAX_LOGS, &ctx);
        }
    }

    /// 就地更新任务记录（不落盘；调用方按需 `persist`）
    fn update_job(&self, id: &str, f: impl FnOnce(&mut Job)) {
        if let Ok(mut map) = self.jobs.lock()
            && let Some(job) = map.get_mut(id)
        {
            f(job);
        }
    }

    /// 执行传输任务（先 login 尽力而为，再 copy；可被单测直接 await）
    ///
    /// `cancel_rx` 被置位时**协作式取消**：终止当前 skopeo 子进程、停止后续标签，
    /// 终态落 `cancelled`（D-IH-39 / D-IH-40）。
    /// 全部复制成功后执行 `cleanup`（改名，D-IH-51 / D-IH-58）。
    pub async fn run_job(
        &self,
        id: &str,
        plans: Vec<TransferPlan>,
        cleanup: Option<CleanupAction>,
        cancel_rx: watch::Receiver<bool>,
    ) {
        let cancel = CancelToken::new(cancel_rx);
        let (error, cancelled) = self.run_plans_with_progress(id, &plans, &cancel).await;
        let error = if error.is_none() && !cancelled {
            self.run_cleanup(id, cleanup.as_ref()).await
        } else {
            error
        };
        self.finish(id, error, cancelled);
    }

    /// 执行归档导入任务：任务内识别归档 → 推算目标 → 逐个导入 → 清理临时文件
    async fn run_import_job(&self, id: &str, spec: ImportSpec, cancel_rx: watch::Receiver<bool>) {
        let cancel = CancelToken::new(cancel_rx);
        let plans = self.detect_and_plan(id, &spec, &cancel).await;
        let (error, cancelled) = match plans {
            Ok(plans) => self.run_plans_with_progress(id, &plans, &cancel).await,
            // 归档识别阶段被停止：按取消处置（而非失败）
            Err(e) if e.code == codes::TASK_CANCELLED => (None, true),
            Err(e) => (Some(e), false),
        };
        // 无论成败都清理上传的临时文件
        remove_file_quiet(&spec.path);
        self.finish(id, error, cancelled);
    }

    /// 识别归档并推算导入目标（同时回填任务的标题 / 源 / 目标 / 计数）
    async fn detect_and_plan(
        &self,
        id: &str,
        spec: &ImportSpec,
        cancel: &CancelToken,
    ) -> Result<Vec<TransferPlan>, ImageHubError> {
        let tmpdir = self.data_dir.join("tmp");
        self.observe_job_line(id, "正在识别归档格式…");
        let detection = detect_archive(
            self.runner.as_ref(),
            &spec.path,
            &tmpdir,
            self.log_sender_direct(id),
            cancel,
            None,
        )
        .await?;

        let targets = plan_import_targets(
            spec.requested_repository.as_deref(),
            spec.requested_tag.as_deref(),
            &detection.names,
        )?;
        let plans: Vec<TransferPlan> = targets
            .iter()
            .map(|t| TransferPlan {
                image: t.image.clone(),
                arch: ArchMode::All,
                insecure: true,
                source: match detection.kind {
                    ArchiveKind::DockerArchive => SourceKind::DockerArchive {
                        path: spec.path.clone(),
                        inner: t.inner.clone(),
                    },
                    ArchiveKind::OciArchive => SourceKind::OciArchive {
                        path: spec.path.clone(),
                        inner: t.inner.clone(),
                    },
                },
                timeout: Some(spec.transfer_timeout),
                retry_times: spec.retry_times,
            })
            .collect();

        let file_name = spec.original_name.clone().unwrap_or_else(|| {
            spec.path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        });
        let first = plans
            .first()
            .expect("plan_import_targets 至少返回一个目标");
        let title = match plans.len() {
            1 => format!(
                "导入 {file_name} → {}",
                first.image.dest_ref(&self.registry_host)
            ),
            n => format!("导入 {file_name}（{n} 个名称）"),
        };
        let dest = first.image.dest_ref(&self.registry_host);
        // 展示用来源（归档 transport + 上传文件名；真实临时路径见日志）
        let source = format!("{}:{file_name}", detection.kind.transport());
        let items_total = plans.len().max(1) as u64;
        self.update_job(id, |job| {
            job.title = title;
            job.source = source;
            job.dest = dest;
            job.progress_detail.items_total = items_total;
            job.progress_detail.max_attempts = spec.retry_times.saturating_add(1);
        });
        self.persist();
        Ok(plans)
    }

    /// 直接发送日志的发送端（丢弃接收端：接收方为任务记录本身，经 `observe_job_line` 写入）
    fn log_sender_direct(&self, id: &str) -> crate::skopeo::LogSender {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let jobs = Arc::clone(&self.jobs);
        let job_id = id.to_string();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let ctx = ProgressContext {
                    item_index: 1,
                    items_done: 0,
                    items_total: 1,
                };
                if let Ok(mut map) = jobs.lock()
                    && let Some(job) = map.get_mut(&job_id)
                {
                    job.observe_line(line, JobManager::DEFAULT_MAX_LOGS, &ctx);
                }
            }
        });
        tx
    }

    /// 执行清理动作（改名）：全部复制成功后摘除源标签 / 删除源仓库；
    /// 失败返回错误信息（明确「复制已成功」以免误判为数据丢失）
    async fn run_cleanup(
        &self,
        id: &str,
        cleanup: Option<&CleanupAction>,
    ) -> Option<ImageHubError> {
        let action = cleanup?;
        self.observe_job_line(id, "复制完成，正在删除源引用…");
        let result: Result<String, ImageHubError> = match action {
            CleanupAction::Untag { repository, tag } => self
                .registry
                .untag_manifest(repository, tag)
                .await
                .map(|()| format!("已移除源标签 {repository}:{tag}")),
            CleanupAction::DeleteRepository { repository } => {
                delete_image(self.registry.as_ref(), repository, None)
                    .await
                    .map(|r| format!("已删除源仓库 {repository}（{} 个标签）", r.deleted_tags.len()))
            }
        };
        match result {
            Ok(msg) => {
                self.observe_job_line(id, &msg);
                None
            }
            Err(e) => Some(ImageHubError::new(
                codes::DELETE_FAILED,
                format!("复制已成功，但源引用未删除：{}", e.msg),
            )),
        }
    }

    /// 执行计划列表（login → copy，带超时 / 重试 / 取消），返回 `(错误, 是否取消)`
    async fn run_plans_with_progress(
        &self,
        id: &str,
        plans: &[TransferPlan],
        cancel: &CancelToken,
    ) -> (Option<ImageHubError>, bool) {
        let settings = self.settings.load().unwrap_or_default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        // 进度上下文（原子值供 drain 任务实时读取）
        let items_total = plans.len().max(1) as u64;
        let current_item = Arc::new(AtomicU64::new(1));
        let items_done = Arc::new(AtomicU64::new(0));

        let jobs = Arc::clone(&self.jobs);
        let drain_id = id.to_string();
        let drain_current = Arc::clone(&current_item);
        let drain_done = Arc::clone(&items_done);
        let drain = tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let ctx = ProgressContext {
                    item_index: drain_current.load(Ordering::SeqCst),
                    items_done: drain_done.load(Ordering::SeqCst),
                    items_total,
                };
                if let Ok(mut map) = jobs.lock()
                    && let Some(job) = map.get_mut(&drain_id)
                {
                    job.observe_line(line, JobManager::DEFAULT_MAX_LOGS, &ctx);
                }
            }
        });

        let mut error: Option<ImageHubError> = None;
        let mut cancelled = false;

        for (index, plan) in plans.iter().enumerate() {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            current_item.store(index as u64 + 1, Ordering::SeqCst);
            items_done.store(index as u64, Ordering::SeqCst);
            // 凭据按**生效源 host** 查找（提供镜像源时为镜像源主机名，D-IH-34；
            // 本地 / 归档源不参与登录，见 `needs_login`）
            let source_registry = plan.source_registry(&self.display_ctx());
            let ctx = TransferContext {
                registry_host: self.registry_host.clone(),
                data_dir: self.data_dir.clone(),
                proxy: settings.proxy.clone(),
                credential: find_credential(&settings, &source_registry).cloned(),
                retry_backoff_base: self.retry_backoff_base,
            };
            let outcome =
                perform_transfer(self.runner.as_ref(), plan, &ctx, tx.clone(), cancel).await;
            match outcome {
                Ok(()) => items_done.store(index as u64 + 1, Ordering::SeqCst),
                Err(e) if e.code == codes::TASK_CANCELLED => {
                    cancelled = true;
                    break;
                }
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        if cancel.is_cancelled() {
            cancelled = true;
            error = None;
        }
        drop(tx);
        let _ = drain.await;
        (error, cancelled)
    }

    /// 请求停止一个运行中的任务（幂等）；返回请求后的任务快照
    ///
    /// 边界（D-IH-40）：任务不存在 → `20808`；已终态 → `20801`；已请求过 → 幂等 `Ok`。
    pub fn cancel(&self, id: &str) -> Result<Job, ImageHubError> {
        let snapshot = {
            let mut map = self.jobs.lock().unwrap();
            let job = map.get_mut(id).ok_or_else(|| {
                ImageHubError::new(codes::TASK_NOT_FOUND, format!("任务不存在: {id}"))
            })?;
            if job.status.is_terminal() {
                return Err(ImageHubError::new(
                    codes::PARAM_INVALID,
                    format!("任务 {id} 已结束（{}），无需停止", job.status.as_str()),
                ));
            }
            if !job.cancel_requested {
                job.cancel_requested = true;
                job.push_log("正在停止任务…", self.max_logs);
                job.progress = "正在停止…".to_string();
            }
            job.clone()
        };
        // 通知执行体（发送端不存在说明任务尚未登记或已结束，忽略即可）
        if let Some(tx) = self.running.lock().unwrap().get(id) {
            let _ = tx.send(true);
        }
        self.persist();
        tracing::info!(job_id = %id, "请求停止任务");
        Ok(snapshot)
    }

    fn finish(&self, id: &str, error: Option<ImageHubError>, cancelled: bool) {
        {
            let mut map = self.jobs.lock().unwrap();
            let Some(job) = map.get_mut(id) else {
                return;
            };
            job.finished_at = Some(now_rfc3339());
            if cancelled {
                job.status = JobStatus::Cancelled;
                job.push_log("任务已被用户取消", self.max_logs);
                job.progress_detail
                    .mark_terminal(phase::CANCELLED, "任务已取消");
                job.progress = job.progress_detail.message.clone();
                job.error = None;
                job.error_code = None;
            } else {
                match error {
                    Some(err) => {
                        job.status = JobStatus::Failed;
                        job.push_log(format!("任务失败：{}", err.msg), self.max_logs);
                        job.progress_detail
                            .mark_terminal(phase::FAILED, format!("任务失败：{}", err.msg));
                        job.progress = job.progress_detail.message.clone();
                        job.error = Some(err.msg);
                        job.error_code = Some(err.code);
                    }
                    None => {
                        job.status = JobStatus::Succeeded;
                        job.push_log("任务完成", self.max_logs);
                        job.progress_detail.mark_terminal(phase::DONE, "任务完成");
                        job.progress = job.progress_detail.message.clone();
                        job.error = None;
                        job.error_code = None;
                    }
                }
            }
        }
        self.trim();
        self.persist();
    }

    /// 裁剪过期任务（仅删除最旧的终态任务）
    fn trim(&self) {
        let mut map = self.jobs.lock().unwrap();
        if map.len() <= self.max_jobs {
            return;
        }
        let mut terminal: Vec<(String, String)> = map
            .values()
            .filter(|j| j.status.is_terminal())
            .map(|j| (j.created_at.clone(), j.id.clone()))
            .collect();
        terminal.sort();
        let mut excess = map.len() - self.max_jobs;
        for (_, id) in terminal {
            if excess == 0 {
                break;
            }
            map.remove(&id);
            excess -= 1;
        }
    }

    /// 任务列表（按创建时间倒序）
    pub fn list(&self) -> Vec<Job> {
        let map = self.jobs.lock().unwrap();
        let mut jobs: Vec<Job> = map.values().cloned().collect();
        jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        jobs
    }

    /// 查询单个任务（不存在 → 20808）
    pub fn get(&self, id: &str) -> Result<Job, ImageHubError> {
        self.jobs
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| ImageHubError::new(codes::TASK_NOT_FOUND, format!("任务不存在: {id}")))
    }

    /// 清理终态任务记录（运行中不可清理）
    pub fn remove(&self, id: &str) -> Result<(), ImageHubError> {
        let mut map = self.jobs.lock().unwrap();
        let job = map
            .get(id)
            .ok_or_else(|| ImageHubError::new(codes::TASK_NOT_FOUND, format!("任务不存在: {id}")))?;
        if !job.status.is_terminal() {
            return Err(ImageHubError::new(
                codes::PARAM_INVALID,
                format!("任务 {id} 仍在执行，无法清理"),
            ));
        }
        map.remove(id);
        drop(map);
        self.persist();
        Ok(())
    }

    /// 是否有运行中的任务
    pub fn has_running(&self) -> bool {
        self.jobs
            .lock()
            .unwrap()
            .values()
            .any(|j| !j.status.is_terminal())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::MemorySettingsStore;
    use crate::skopeo::MockSkopeoRunner;

    fn manager(runner: Arc<dyn SkopeoRunner>, dir: &std::path::Path) -> Arc<JobManager> {
        manager_with_registry(runner, Arc::new(crate::registry::MockRegistryClient::new()), dir)
    }

    fn manager_with_registry(
        runner: Arc<dyn SkopeoRunner>,
        registry: Arc<dyn crate::registry::RegistryClient>,
        dir: &std::path::Path,
    ) -> Arc<JobManager> {
        manager_full(
            runner,
            registry,
            Arc::new(crate::gc::MockRegistryGcRunner::new()),
            dir,
        )
    }

    fn manager_full(
        runner: Arc<dyn SkopeoRunner>,
        registry: Arc<dyn crate::registry::RegistryClient>,
        gc: Arc<dyn crate::gc::RegistryGcRunner>,
        dir: &std::path::Path,
    ) -> Arc<JobManager> {
        let mgr = JobManager::new(
            runner,
            gc,
            Arc::new(MemorySettingsStore::new()),
            registry,
            "127.0.0.1:19913",
            dir,
        );
        // 单测缩短重试退避，避免等待真实的 5s+
        let mut mgr = mgr;
        mgr.retry_backoff_base = Duration::from_millis(1);
        Arc::new(mgr)
    }

    fn transfer_timeout() -> Duration {
        Duration::from_secs(crate::skopeo::DEFAULT_TRANSFER_TIMEOUT_MINUTES * 60)
    }

    async fn wait_terminal(mgr: &Arc<JobManager>, id: &str) -> Job {
        for _ in 0..200 {
            let job = mgr.get(id).unwrap();
            if job.status.is_terminal() {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        mgr.get(id).unwrap()
    }

    #[test]
    fn job_id_format_is_deterministic() {
        assert_eq!(next_job_id("pull", 1000, 7), "pull-1000-7");
    }

    fn sample_job() -> Job {
        Job {
            id: "j".into(),
            kind: "pull".into(),
            title: "t".into(),
            source: "s".into(),
            dest: "d".into(),
            mirror: None,
            arch: "all".into(),
            status: JobStatus::Running,
            progress: String::new(),
            progress_detail: ProgressDetail::queued(1),
            cancel_requested: false,
            timeout_minutes: crate::skopeo::DEFAULT_TRANSFER_TIMEOUT_MINUTES,
            retry_times: DEFAULT_RETRY_TIMES,
            logs: Vec::new(),
            gc: None,
            created_at: now_rfc3339(),
            finished_at: None,
            error: None,
            error_code: None,
        }
    }

    fn progress_ctx() -> ProgressContext {
        ProgressContext {
            item_index: 1,
            items_done: 0,
            items_total: 1,
        }
    }

    #[test]
    fn observe_line_caps_logs_and_writes_friendly_progress() {
        let mut job = sample_job();
        let ctx = progress_ctx();
        job.observe_line("  ", 3, &ctx);
        assert_eq!(job.logs.len(), 0);

        // 原始行进日志，进度为中文摘要（非原始行）
        job.observe_line("Getting image source signatures", 3, &ctx);
        assert_eq!(job.logs, vec!["Getting image source signatures"]);
        assert_eq!(job.progress, "正在获取镜像清单");
        assert_eq!(job.progress_detail.phase, phase::PREPARING);

        job.observe_line("Copying blob sha256:aaa", 3, &ctx);
        assert_eq!(job.progress_detail.layers_done, 1);
        assert!(job.progress.contains("已复制 1 层"), "{}", job.progress);

        job.observe_line("Copying image sha256:bbb (3/12)", 3, &ctx);
        assert_eq!(job.progress_detail.sub_current, Some(3));
        assert_eq!(job.progress_detail.percent, Some(25));

        // 无法识别的事件：摘要回落原始行，日志仍保留
        job.observe_line("some unknown output", 3, &ctx);
        assert_eq!(job.progress, "some unknown output");
        assert_eq!(job.logs.len(), 3, "日志裁剪到上限");
        assert_eq!(job.logs[2], "some unknown output");
    }

    #[tokio::test]
    async fn pull_job_reaches_succeeded() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mgr = manager(runner.clone(), tmp.path());
        let image = crate::skopeo::parse_image_ref("docker.m.daocloud.io/library/alpine:3.20").unwrap();
        let id = mgr.start_pull(image, ArchMode::All, true, None, transfer_timeout(), DEFAULT_RETRY_TIMES);

        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded);
        assert!(job.error.is_none());
        assert!(job.finished_at.is_some());
        assert!(job.logs.iter().any(|l| l.contains("任务完成")));
        assert!(mgr.list().iter().any(|j| j.id == id));
        // mock 记录了 copy 调用
        assert_eq!(runner.argv_list().len(), 1);
        assert_eq!(runner.argv_list()[0][2], "copy");
        // 终态可清理
        mgr.remove(&id).unwrap();
        assert!(mgr.get(&id).is_err());
    }

    #[tokio::test]
    async fn pull_job_failure_maps_skopeo_failed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mgr = manager(Arc::new(MockSkopeoRunner::failing()), tmp.path());
        let image = crate::skopeo::parse_image_ref("docker.io/library/ubuntu:22.04").unwrap();
        let id = mgr.start_pull(image, ArchMode::Host, false, None, transfer_timeout(), DEFAULT_RETRY_TIMES);

        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Failed);
        assert!(job.error.as_deref().unwrap().contains("skopeo copy"));
    }

    #[tokio::test]
    async fn sync_job_runs_one_copy_per_tag() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mgr = manager(runner.clone(), tmp.path());
        let id = mgr.start_sync(
            "docker.m.daocloud.io".into(),
            "library/alpine".into(),
            vec!["3.20".into(), "3.21".into()],
            ArchMode::All,
            true,
            None,
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(runner.argv_list().len(), 2, "两个 tag 各一次 copy");
    }

    #[tokio::test]
    async fn pull_job_with_mirror_keeps_upstream_dest_and_records_mirror() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mgr = manager(runner.clone(), tmp.path());
        let image = crate::skopeo::parse_image_ref("ubuntu:22.04").unwrap();
        let id = mgr.start_pull(
            image,
            ArchMode::All,
            true,
            Some("docker.m.daocloud.io".into()),
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );

        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.mirror.as_deref(), Some("docker.m.daocloud.io"));
        // source = 实际拉取源；dest = 上游完整名派生（不含镜像源主机名）
        assert_eq!(job.source, "docker.m.daocloud.io/library/ubuntu:22.04");
        assert_eq!(job.dest, "127.0.0.1:19913/docker.io/library/ubuntu:22.04");
        assert_eq!(job.title, "拉取 docker.io/library/ubuntu:22.04");

        let joined = runner.argv_list()[0].join(" ");
        assert!(joined.contains("docker://docker.m.daocloud.io/library/ubuntu:22.04"));
        assert!(joined.contains("docker://127.0.0.1:19913/docker.io/library/ubuntu:22.04"));
    }

    #[tokio::test]
    async fn sync_job_with_mirror_uses_mirror_as_source_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mgr = manager(runner.clone(), tmp.path());
        let id = mgr.start_sync(
            "docker.io".into(),
            "library/ubuntu".into(),
            vec!["22.04".into()],
            ArchMode::All,
            true,
            Some("docker.m.daocloud.io".into()),
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.mirror.as_deref(), Some("docker.m.daocloud.io"));
        assert_eq!(job.source, "docker.m.daocloud.io/library/ubuntu:22.04");
        assert_eq!(job.dest, "127.0.0.1:19913/docker.io/library/ubuntu:22.04");
        assert_eq!(job.title, "同步 docker.io/library/ubuntu:22.04");
    }

    #[tokio::test]
    async fn sync_job_without_mirror_is_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mgr = manager(runner.clone(), tmp.path());
        let id = mgr.start_sync(
            "docker.io".into(),
            "library/ubuntu".into(),
            vec!["22.04".into()],
            ArchMode::All,
            true,
            None,
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.mirror, None);
        assert_eq!(job.source, "docker.io/library/ubuntu:22.04");
        assert_eq!(job.dest, "127.0.0.1:19913/docker.io/library/ubuntu:22.04");
    }

    #[test]
    fn legacy_terminal_job_gets_terminal_phase() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("jobs.json");
        let legacy = serde_json::json!([{
            "id": "pull-1-0",
            "kind": "pull",
            "title": "t",
            "source": "s",
            "dest": "d",
            "arch": "all",
            "status": "succeeded",
            "progress": "任务完成",
            "logs": ["任务完成"],
            "created_at": "2026-09-23T00:00:00Z",
            "finished_at": "2026-09-23T00:01:00Z",
            "error": null
        }]);
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());
        let loaded = mgr.list();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].progress_detail.phase, phase::DONE);
        assert_eq!(loaded[0].progress_detail.percent, Some(100));
        assert_eq!(loaded[0].progress, "任务完成");
    }

    #[test]
    fn persisted_cancel_requested_job_becomes_cancelled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("jobs.json");
        let running = Job {
            cancel_requested: true,
            ..sample_job()
        };
        write_json_atomic(&path, &vec![running]).unwrap();
        let mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());
        let loaded = mgr.list();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].status, JobStatus::Cancelled);
        assert_eq!(loaded[0].progress_detail.phase, phase::CANCELLED);
        assert!(loaded[0].finished_at.is_some());
    }

    #[test]
    fn legacy_jobs_json_without_mirror_field_loads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("jobs.json");
        // 旧版本记录：无 mirror 字段
        let legacy = serde_json::json!([{
            "id": "pull-9-0",
            "kind": "pull",
            "title": "拉取 docker.io/library/ubuntu:22.04",
            "source": "docker.io/library/ubuntu:22.04",
            "dest": "127.0.0.1:19913/docker.io/library/ubuntu:22.04",
            "arch": "all",
            "status": "succeeded",
            "progress": "任务完成",
            "logs": ["任务完成"],
            "created_at": "2026-09-23T00:00:00Z",
            "finished_at": "2026-09-23T00:01:00Z",
            "error": null
        }]);
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        let mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());
        let loaded = mgr.list();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].mirror, None);
        assert_eq!(loaded[0].status, JobStatus::Succeeded);
        // 新字段缺省（终态旧记录补全为 done，避免显示「等待执行」）
        assert_eq!(loaded[0].timeout_minutes, 60);
        assert!(!loaded[0].cancel_requested);
        assert_eq!(loaded[0].progress_detail.phase, phase::DONE);
    }

    #[tokio::test]
    async fn cancel_running_job_marks_cancelled_and_stops_transfer() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::with_delay(10_000));
        let mgr = manager(runner.clone(), tmp.path());
        let image = crate::skopeo::parse_image_ref("ubuntu:22.04").unwrap();
        let id = mgr.start_pull(image, ArchMode::All, true, None, transfer_timeout(), DEFAULT_RETRY_TIMES);

        // 等 copy 真正启动
        for _ in 0..100 {
            if !runner.invocations().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(runner.invocations().len(), 1, "copy 已启动");

        let started = std::time::Instant::now();
        let snapshot = mgr.cancel(&id).unwrap();
        assert!(snapshot.cancel_requested);
        assert!(!snapshot.status.is_terminal(), "取消是异步的，先置位");

        let job = wait_terminal(&mgr, &id).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "取消应立即收敛");
        assert_eq!(job.status, JobStatus::Cancelled);
        assert!(job.cancel_requested);
        assert!(job.error.is_none(), "取消不是失败：{:?}", job.error);
        assert!(job.finished_at.is_some());
        assert_eq!(job.progress_detail.phase, phase::CANCELLED);
        assert_eq!(job.progress, "任务已取消");
        assert!(job.logs.iter().any(|l| l.contains("任务已被用户取消")));
        assert_eq!(runner.cancelled_count(), 1, "skopeo 调用被取消");
        // 不再新增调用
        assert_eq!(runner.invocations().len(), 1);
        // 已取消亦为终态：可清理 / 二次取消返回 20801
        assert_eq!(mgr.cancel(&id).unwrap_err().code, codes::PARAM_INVALID);
        mgr.remove(&id).unwrap();
    }

    #[tokio::test]
    async fn cancel_sync_stops_remaining_tags() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::with_delay(5_000));
        let mgr = manager(runner.clone(), tmp.path());
        let id = mgr.start_sync(
            "docker.io".into(),
            "library/ubuntu".into(),
            vec!["1".into(), "2".into(), "3".into()],
            ArchMode::All,
            true,
            None,
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let job0 = mgr.get(&id).unwrap();
        assert_eq!(job0.progress_detail.items_total, 3, "多标签任务登记总标签数");
        assert_eq!(job0.timeout_minutes, 60);

        for _ in 0..100 {
            if !runner.invocations().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        mgr.cancel(&id).unwrap();
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Cancelled);
        assert_eq!(runner.invocations().len(), 1, "取消后不再处理后续标签");
    }

    #[tokio::test]
    async fn cancel_unknown_task_is_20808() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());
        assert_eq!(mgr.cancel("nope").unwrap_err().code, codes::TASK_NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_terminal_task_is_20801_and_idempotent_while_running() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::with_delay(500));
        let mgr = manager(runner, tmp.path());
        let image = crate::skopeo::parse_image_ref("ubuntu:22.04").unwrap();
        let id = mgr.start_pull(image, ArchMode::All, true, None, transfer_timeout(), DEFAULT_RETRY_TIMES);

        // 运行中重复取消：幂等
        mgr.cancel(&id).unwrap();
        mgr.cancel(&id).unwrap();

        // 已完成的任务不可取消
        let started = std::time::Instant::now();
        let id2 = mgr.start_pull(
            crate::skopeo::parse_image_ref("ubuntu:24.04").unwrap(),
            ArchMode::Host,
            true,
            None,
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let done = wait_terminal(&mgr, &id2).await;
        assert_eq!(done.status, JobStatus::Succeeded);
        assert_eq!(mgr.cancel(&id2).unwrap_err().code, codes::PARAM_INVALID);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[tokio::test]
    async fn pull_job_records_timeout_minutes_and_passes_it_to_copy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mgr = manager(runner.clone(), tmp.path());
        let image = crate::skopeo::parse_image_ref("ubuntu:22.04").unwrap();
        let id = mgr.start_pull(
            image,
            ArchMode::Host,
            true,
            Some("docker.m.daocloud.io".into()),
            Duration::from_secs(480 * 60),
            DEFAULT_RETRY_TIMES,
        );
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.timeout_minutes, 480);

        let calls = runner.invocations();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].argv[2], "copy");
        assert_eq!(calls[0].timeout, Some(Duration::from_secs(480 * 60)));
    }

    #[tokio::test]
    async fn cancelled_task_progress_is_readable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::with_delay(10_000));
        let mgr = manager(runner, tmp.path());
        let image = crate::skopeo::parse_image_ref("docker.io/library/ubuntu:22.04").unwrap();
        let id = mgr.start_pull(image, ArchMode::All, true, None, transfer_timeout(), DEFAULT_RETRY_TIMES);
        for _ in 0..100 {
            let job = mgr.get(&id).unwrap();
            if job.progress_detail.phase == phase::QUEUED {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            } else {
                break;
            }
        }
        mgr.cancel(&id).unwrap();
        let job = wait_terminal(&mgr, &id).await;
        // 终态摘要为中文（非原始行）
        assert_eq!(job.progress, "任务已取消");
        assert!(!job.progress.contains("sha256:"));
    }

    #[tokio::test]
    async fn unknown_task_is_20808_and_running_not_removable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());
        let err = mgr.get("nope").unwrap_err();
        assert_eq!(err.code, codes::TASK_NOT_FOUND);
        assert_eq!(mgr.remove("nope").unwrap_err().code, codes::TASK_NOT_FOUND);
    }

    #[test]
    fn persisted_jobs_roundtrip_marks_running_as_failed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("jobs.json");
        let running = Job {
            id: "pull-1-0".into(),
            kind: "pull".into(),
            title: "t".into(),
            source: "s".into(),
            dest: "d".into(),
            mirror: None,
            arch: "all".into(),
            status: JobStatus::Running,
            progress: "copying".into(),
            progress_detail: ProgressDetail::queued(1),
            cancel_requested: false,
            timeout_minutes: 60,
            retry_times: DEFAULT_RETRY_TIMES,
            logs: vec!["copying".into()],
            gc: None,
            created_at: "2026-09-23T00:00:00Z".into(),
            finished_at: None,
            error: None,
            error_code: None,
        };
        let done = Job {
            status: JobStatus::Succeeded,
            id: "pull-2-1".into(),
            finished_at: Some("2026-09-23T00:01:00Z".into()),
            ..running.clone()
        };
        write_json_atomic(&path, &vec![running, done]).unwrap();

        let mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());
        let loaded = mgr.list();
        assert_eq!(loaded.len(), 2);
        let interrupted = loaded.iter().find(|j| j.id == "pull-1-0").unwrap();
        assert_eq!(interrupted.status, JobStatus::Failed);
        assert!(interrupted.error.as_deref().unwrap().contains("重启"));
        let ok = loaded.iter().find(|j| j.id == "pull-2-1").unwrap();
        assert_eq!(ok.status, JobStatus::Succeeded);
    }

    // ==================== 克隆 / 改名 / 导入 ====================

    use crate::registry::MockRegistryClient;
    use crate::skopeo::parse_image_ref;

    fn local_target(source_tag: &str, target: &str) -> LocalCopyTarget {
        LocalCopyTarget {
            source_tag: source_tag.to_string(),
            target: parse_image_ref(target).unwrap(),
        }
    }

    /// 写一个含 `index.json`（ref.name 注解）的未压缩 OCI 归档
    fn write_oci_tar(path: &std::path::Path, names: &[&str]) {
        let manifests: Vec<serde_json::Value> = names
            .iter()
            .map(|n| {
                serde_json::json!({
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                    "size": 1,
                    "annotations": { "org.opencontainers.image.ref.name": n }
                })
            })
            .collect();
        let index = serde_json::json!({ "schemaVersion": 2, "manifests": manifests });
        let payload = serde_json::to_vec(&index).unwrap();
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "index.json", payload.as_slice())
            .unwrap();
        std::fs::write(path, builder.into_inner().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn clone_repository_copies_all_tags() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mgr = manager(runner.clone(), tmp.path());
        let id = mgr.start_local_copy(
            false,
            "docker.io/library/alpine",
            vec![
                local_target("3.19", "local/alpine:3.19"),
                local_target("3.20", "local/alpine:3.20"),
            ],
            None,
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.kind, "clone");
        assert!(job.title.contains("克隆"), "{}", job.title);
        assert!(job.title.contains("2 个标签"), "{}", job.title);
        assert_eq!(job.progress_detail.items_total, 2);
        assert_eq!(job.arch, "all");
        assert_eq!(job.dest, "127.0.0.1:19913/docker.io/local/alpine:3.19");
        assert_eq!(runner.call_count(), 2);
        let joined = runner.argv_list()[0].join(" ");
        assert!(joined.contains("docker://127.0.0.1:19913/docker.io/library/alpine:3.19"), "{joined}");
    }

    #[tokio::test]
    async fn rename_tag_runs_copy_then_untags_source() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mut registry = MockRegistryClient::new();
        // 同仓库两个 tag 指向同一 digest（改名核心风险场景）
        registry.seed("docker.io/library/alpine", "3.19", "sha256:same");
        registry.seed("docker.io/library/alpine", "3.20", "sha256:same");
        let registry = Arc::new(registry);
        let mgr = manager_with_registry(runner.clone(), registry.clone(), tmp.path());

        let id = mgr.start_local_copy(
            true,
            "docker.io/library/alpine",
            vec![local_target("3.19", "docker.io/library/alpine:stable")],
            Some(CleanupAction::Untag {
                repository: "docker.io/library/alpine".into(),
                tag: "3.19".into(),
            }),
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
        assert_eq!(job.kind, "rename");
        assert_eq!(runner.call_count(), 1);
        assert_eq!(
            registry.untagged.lock().unwrap().clone(),
            vec![("docker.io/library/alpine".to_string(), "3.19".to_string())]
        );
        // 只摘除源标签，同 digest 的另一个 tag 仍在
        assert_eq!(
            registry.list_tags("docker.io/library/alpine").await.unwrap(),
            vec!["3.20"]
        );
        assert!(job.logs.iter().any(|l| l.contains("已移除源标签")));
    }

    #[tokio::test]
    async fn rename_repository_deletes_source_repository() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mut registry = MockRegistryClient::new();
        registry.seed("docker.io/library/alpine", "3.19", "sha256:a");
        registry.seed("docker.io/library/alpine", "3.20", "sha256:b");
        let registry = Arc::new(registry);
        let mgr = manager_with_registry(runner.clone(), registry.clone(), tmp.path());

        let id = mgr.start_local_copy(
            true,
            "docker.io/library/alpine",
            vec![
                local_target("3.19", "local/alpine:3.19"),
                local_target("3.20", "local/alpine:3.20"),
            ],
            Some(CleanupAction::DeleteRepository {
                repository: "docker.io/library/alpine".into(),
            }),
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
        assert_eq!(runner.call_count(), 2, "复制两个标签");
        let deleted = registry.deleted.lock().unwrap().clone();
        assert_eq!(deleted.len(), 2, "{deleted:?}");
        assert!(deleted.iter().all(|(r, _)| r == "docker.io/library/alpine"));
        assert!(registry.list_tags("docker.io/library/alpine").await.unwrap().is_empty());
        assert!(job.logs.iter().any(|l| l.contains("已删除源仓库")));
    }

    #[tokio::test]
    async fn rename_cleanup_failure_reports_copy_succeeded() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        // registry 中无该标签 → 摘除失败
        let registry = Arc::new(MockRegistryClient::new());
        let mgr = manager_with_registry(runner, registry, tmp.path());

        let id = mgr.start_local_copy(
            true,
            "docker.io/library/alpine",
            vec![local_target("3.19", "docker.io/library/alpine:stable")],
            Some(CleanupAction::Untag {
                repository: "docker.io/library/alpine".into(),
                tag: "3.19".into(),
            }),
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Failed);
        let err = job.error.as_deref().unwrap();
        assert!(err.contains("复制已成功，但源引用未删除"), "{err}");
    }

    #[tokio::test]
    async fn import_job_detects_oci_archive_and_preserves_names() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("import-1.bin");
        write_oci_tar(
            &path,
            &[
                "docker.io/library/alpine:3.19",
                "docker.io/library/alpine:3.20",
            ],
        );
        // 第 1 次调用（list-tags docker-archive）失败 → 读 index.json 识别为 OCI 归档；
        // 后续 copy 调用成功
        let runner = Arc::new(MockSkopeoRunner::failing_then_ok(1));
        let mgr = manager(runner.clone(), tmp.path());
        let id = mgr.start_import(ImportSpec {
            path: path.clone(),
            original_name: None,
            requested_repository: None,
            requested_tag: None,
            transfer_timeout: transfer_timeout(),
            retry_times: DEFAULT_RETRY_TIMES,
        });

        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
        assert_eq!(job.kind, "import");
        assert!(job.source.starts_with("oci-archive:"), "{}", job.source);
        assert_eq!(job.dest, "127.0.0.1:19913/docker.io/library/alpine:3.19");
        assert_eq!(job.progress_detail.items_total, 2);
        assert_eq!(runner.call_count(), 3, "1 次 list-tags + 2 次 copy");
        let argv = runner.argv_list();
        assert_eq!(argv[0][2], "list-tags");
        assert_eq!(argv[1][2], "copy");
        assert!(argv[1].join(" ").contains(":docker.io/library/alpine:3.19"), "{:?}", argv[1]);
        assert!(argv[2].join(" ").contains(":docker.io/library/alpine:3.20"), "{:?}", argv[2]);
        assert!(job.logs.iter().any(|l| l.contains("识别为 OCI 归档")));
        assert!(!path.exists(), "导入完成后应删除临时文件");
    }

    #[tokio::test]
    async fn import_with_requested_name_uses_archive_default_ref() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("import-2.bin");
        // Mock 的 list-tags 恒成功（视为 docker-archive）；内容无需是真实归档
        std::fs::write(&path, b"whatever").unwrap();
        let runner = Arc::new(MockSkopeoRunner::new());
        let mgr = manager(runner.clone(), tmp.path());
        let id = mgr.start_import(ImportSpec {
            path: path.clone(),
            original_name: Some("alpine-saved.tar".into()),
            requested_repository: Some("docker.io/library/alpine".into()),
            requested_tag: Some("stable".into()),
            transfer_timeout: transfer_timeout(),
            retry_times: DEFAULT_RETRY_TIMES,
        });
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
        assert_eq!(job.source, "docker-archive:alpine-saved.tar");
        assert!(job.title.contains("alpine-saved.tar"), "{}", job.title);
        assert_eq!(job.dest, "127.0.0.1:19913/docker.io/library/alpine:stable");
        let argv = runner.argv_list();
        assert_eq!(argv.len(), 2, "1 次 list-tags + 1 次 copy");
        let copy = argv[1].join(" ");
        assert!(copy.contains(&format!("docker-archive:{}", path.display())), "{copy}");
        assert!(copy.ends_with("docker://127.0.0.1:19913/docker.io/library/alpine:stable"), "{copy}");
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn import_without_name_and_without_archive_names_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("import-3.bin");
        std::fs::write(&path, b"whatever").unwrap();
        // Mock 的 list-tags 成功但输出无 Tags → 无原始名称
        let mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());
        let id = mgr.start_import(ImportSpec {
            path: path.clone(),
            original_name: None,
            requested_repository: None,
            requested_tag: None,
            transfer_timeout: transfer_timeout(),
            retry_times: DEFAULT_RETRY_TIMES,
        });
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Failed);
        assert!(
            job.error.as_deref().unwrap().contains("请填写目标镜像名"),
            "{:?}",
            job.error
        );
        assert!(!path.exists(), "失败也应删除临时文件");
    }

    #[test]
    fn leftover_import_temp_files_are_cleaned_on_startup() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tmpdir = tmp.path().join("tmp");
        std::fs::create_dir_all(&tmpdir).unwrap();
        let leftover = tmpdir.join("import-123-0.bin");
        std::fs::write(&leftover, b"x").unwrap();
        let keep = tmpdir.join("auth.json");
        std::fs::write(&keep, b"{}").unwrap();

        let _mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());
        assert!(!leftover.exists(), "启动应清理遗留导入文件");
        assert!(keep.exists(), "非导入文件不应被清理");
    }

    // ==================== GC 任务（D-IH-59 ~ D-IH-66） ====================

    fn gc_manager(
        runner: Arc<dyn crate::gc::RegistryGcRunner>,
        dir: &std::path::Path,
    ) -> Arc<JobManager> {
        manager_full(
            Arc::new(MockSkopeoRunner::new()),
            Arc::new(crate::registry::MockRegistryClient::new()),
            runner,
            dir,
        )
    }

    fn gc_runner_with_storage(summary: &str) -> (tempfile::TempDir, Arc<crate::gc::MockRegistryGcRunner>) {
        let storage = tempfile::TempDir::new().unwrap();
        std::fs::write(storage.path().join("blob"), vec![0u8; 4096]).unwrap();
        let mut runner = crate::gc::MockRegistryGcRunner::new();
        runner.storage_dir = Some(storage.path().to_path_buf());
        runner.summary_line = Some(summary.to_string());
        (storage, Arc::new(runner))
    }

    fn gc_request(dry_run: bool) -> GcRequest {
        GcRequest {
            delete_untagged: true,
            dry_run,
            timeout: transfer_timeout(),
        }
    }

    #[tokio::test]
    async fn gc_job_succeeds_and_reports_summary() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_storage, runner) =
            gc_runner_with_storage("37 blobs marked, 16 blobs and 3 manifests eligible for deletion");
        let mgr = gc_manager(runner.clone(), tmp.path());

        let id = mgr.start_gc(gc_request(false)).unwrap();
        assert!(id.starts_with("gc-"), "{id}");
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded, "{job:?}");
        assert_eq!(job.kind, "gc");
        assert_eq!(job.title, "垃圾回收");
        let gc = job.gc.unwrap();
        assert!(!gc.dry_run);
        assert_eq!(gc.counters.blobs_marked, 37);
        assert_eq!(gc.counters.blobs_deleted, 16);
        assert_eq!(gc.counters.manifests_deleted, 3);
        assert!(gc.storage_bytes_before.is_some());
        assert_eq!(gc.freed_bytes, Some(0), "mock 未真正删除，占用不变");
        assert!(job.progress.contains("回收完成"), "{}", job.progress);
        assert_eq!(runner.call_count(), 1);
        assert!(runner.invocations()[0].delete_untagged);
        // 终态可清理
        mgr.remove(&id).unwrap();
    }

    #[tokio::test]
    async fn gc_dry_run_is_marked_and_keeps_delete_flag() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_storage, runner) =
            gc_runner_with_storage("37 blobs marked, 16 blobs and 0 manifests eligible for deletion");
        let mgr = gc_manager(runner.clone(), tmp.path());
        let id = mgr.start_gc(gc_request(true)).unwrap();
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.title, "垃圾回收（预演）");
        let gc = job.gc.unwrap();
        assert!(gc.dry_run);
        assert_eq!(gc.counters.blobs_deleted, 16);
        assert!(job.progress.starts_with("预演完成"), "{}", job.progress);
        assert!(runner.invocations()[0].dry_run);
    }

    #[tokio::test]
    async fn gc_job_failure_maps_gc_failed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let storage = tempfile::TempDir::new().unwrap();
        let mut runner = crate::gc::MockRegistryGcRunner::failing();
        runner.storage_dir = Some(storage.path().to_path_buf());
        let mgr = gc_manager(Arc::new(runner), tmp.path());
        let id = mgr.start_gc(gc_request(false)).unwrap();
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Failed);
        assert!(job.error.as_deref().unwrap().contains("GC 执行失败"), "{:?}", job.error);
        assert!(!job.gc.unwrap().dry_run);
    }

    #[tokio::test]
    async fn gc_rejected_when_busy_and_when_unavailable() {
        let tmp = tempfile::TempDir::new().unwrap();
        // 忙碌：以带延迟的 skopeo 制造运行中任务
        let delayed = Arc::new(MockSkopeoRunner::with_delay(10_000));
        let (_storage, runner) =
            gc_runner_with_storage("10 blobs marked, 0 blobs and 0 manifests eligible for deletion");
        let mgr = manager_full(
            delayed.clone(),
            Arc::new(crate::registry::MockRegistryClient::new()),
            runner.clone(),
            tmp.path(),
        );
        let image = crate::skopeo::parse_image_ref("docker.io/library/ubuntu:22.04").unwrap();
        let _pull_id = mgr.start_pull(
            image,
            ArchMode::All,
            true,
            None,
            transfer_timeout(),
            DEFAULT_RETRY_TIMES,
        );
        let err = mgr.start_gc(gc_request(false)).unwrap_err();
        assert_eq!(err.code, codes::PARAM_INVALID, "{}", err.msg);
        assert_eq!(runner.call_count(), 0, "忙碌时不应执行 GC");

        // 能力不可用
        let mgr2 = gc_manager(Arc::new(crate::gc::MockRegistryGcRunner::unavailable()), tmp.path());
        let err = mgr2.start_gc(gc_request(false)).unwrap_err();
        assert_eq!(err.code, codes::GC_UNAVAILABLE, "{}", err.msg);
    }

    #[tokio::test]
    async fn gc_job_can_be_cancelled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let storage = tempfile::TempDir::new().unwrap();
        let mut runner = crate::gc::MockRegistryGcRunner::with_delay(10_000);
        runner.storage_dir = Some(storage.path().to_path_buf());
        let runner = Arc::new(runner);
        let mgr = gc_manager(Arc::clone(&runner) as Arc<dyn crate::gc::RegistryGcRunner>, tmp.path());
        let id = mgr.start_gc(gc_request(false)).unwrap();
        for _ in 0..200 {
            if runner.call_count() > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        mgr.cancel(&id).unwrap();
        let job = wait_terminal(&mgr, &id).await;
        assert_eq!(job.status, JobStatus::Cancelled);
        assert_eq!(runner.cancelled_count(), 1);
    }

    #[test]
    fn legacy_job_without_gc_field_loads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("jobs.json");
        let legacy = serde_json::json!([{
            "id": "pull-9-0",
            "kind": "pull",
            "title": "t",
            "source": "s",
            "dest": "d",
            "arch": "all",
            "status": "succeeded",
            "progress": "任务完成",
            "logs": ["任务完成"],
            "created_at": "2026-09-24T00:00:00Z",
            "finished_at": "2026-09-24T00:01:00Z",
            "error": null
        }]);
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());
        let loaded = mgr.list();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].gc.is_none());
        assert!(loaded[0].error_code.is_none(), "旧记录 error_code 应为 None");
        assert!(
            loaded[0].progress_detail.retry_in_secs.is_none(),
            "旧记录 retry_in_secs 应为 None"
        );
    }

    #[test]
    fn finish_records_error_code_and_clears_it_on_success() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mgr = manager(Arc::new(MockSkopeoRunner::new()), tmp.path());

        let (failed_id, _rx) = mgr.register_job(
            "pull",
            "拉取 x".into(),
            "s".into(),
            "d".into(),
            None,
            Duration::from_secs(60),
            2,
            "all".into(),
            1,
        );
        mgr.finish(
            &failed_id,
            Some(ImageHubError::new(codes::SKOPEO_FAILED, "boom")),
            false,
        );
        let failed = mgr.get(&failed_id).unwrap();
        assert_eq!(failed.status, JobStatus::Failed);
        assert_eq!(failed.error_code, Some(codes::SKOPEO_FAILED));
        assert_eq!(failed.error.as_deref(), Some("boom"));

        let (ok_id, _rx2) = mgr.register_job(
            "pull",
            "拉取 y".into(),
            "s".into(),
            "d".into(),
            None,
            Duration::from_secs(60),
            0,
            "all".into(),
            1,
        );
        mgr.finish(&ok_id, None, false);
        let ok = mgr.get(&ok_id).unwrap();
        assert_eq!(ok.status, JobStatus::Succeeded);
        assert!(ok.error_code.is_none());
    }
}