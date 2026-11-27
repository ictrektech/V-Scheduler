//! 自动重启：判定（纯函数）+ 调度（进程级）+ 运行态持久化
//!
//! 运行配置可选两种 docker 风格策略：
//!
//! - `never`（创建默认）：不自动重启；
//! - `unless-stop`：除用户显式停止外，**启动失败**或**连续探活失败达到阈值**都会触发自动重启。
//!
//! 调度器在健康巡检之后、以同一周期运行：
//!
//! ```text
//! running + unhealthy 且连续失败次数 >= 阈值
//!        └─► 下发停止（清理节点侧副作用）
//!                 └─► state=stopped 且过 backoff 截止 ─► 下发启动
//!                          ├─ state=running  ─► 回 idle，计数清零
//!                          └─ 启动失败        ─► 再清理 + 指数 backoff → 重试（无限）
//!
//! 首次启动失败（state=failed / 启动 run failed）同样进入上述循环。
//! ```
//!
//! 关键约束（设计见 `specs/docs/design/modelinfer/auto-restart.md`）：
//!
//! - **纯函数决策**：[`decide`] 只输入运行态 + 现状 + 当前时间，输出「新运行态 + 动作」，
//!   便于穷举单测；动作执行走 [`crate::deploy::control::Control`]，与手动启停同源；
//! - **运行态持久化**（相位 / 连续失败次数 / next_at / 计数）：进程重启与整机掉电后自动续跑；
//! - **用户手动停止 / 启动立即取消循环**：由 HTTP handler 调用 [`reset`]（调度器自身不调用），
//!   reset 同时记录抑制起点，避免旧启动失败 run 在人工操作后再次触发。

use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde::Serialize;

use crate::deploy::control::{Control, TRIGGER_AUTO_RESTART};
use crate::deployments::validate;
use crate::entities::deployment;
use crate::error::{ModelinferError, codes};
use crate::time;

/// 已下发启动后的失败检测宽限期（秒）
///
/// 覆盖「启动工作流已下发、`prepare` 尚未把状态置为 `starting`」的窗口，避免把正常启动误判为失败。
pub const START_GRACE_SECS: i64 = 180;
/// 已下发停止后的重复下发间隔（秒）：停止工作流卡死 / 丢失时不至于永久停滞
pub const STOP_STUCK_SECS: i64 = 900;
/// 已下发停止但状态仍为 `running` 时的重复下发间隔（秒，动作未生效 / 下发失败）
pub const STOP_DISPATCH_GRACE_SECS: i64 = 120;

/// 自动重启相位：idle（空串，未在循环中）
pub const PHASE_IDLE: &str = "";
/// 自动重启相位：已下发停止，待启动
pub const PHASE_STOPPING: &str = "stopping";
/// 自动重启相位：已下发启动，待结果
pub const PHASE_STARTING: &str = "starting";

/// 自动重启策略：不自动重启（创建默认）
pub const POLICY_NEVER: &str = validate::AUTO_RESTART_POLICY_NEVER;
/// 自动重启策略：除用户显式停止外，启动失败 / 探活失败均自动重启
pub const POLICY_UNLESS_STOP: &str = validate::AUTO_RESTART_POLICY_UNLESS_STOP;

/// backoff 策略（指数增长 + 上限）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    /// 首次重试等待（秒）
    pub base_secs: i64,
    /// 等待上限（秒）
    pub max_secs: i64,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            base_secs: 15,
            max_secs: 300,
        }
    }
}

impl BackoffPolicy {
    /// 归一化（base ≥ 1，max ≥ base）
    pub fn normalized(self) -> Self {
        let base = self.base_secs.max(1);
        Self {
            base_secs: base,
            max_secs: self.max_secs.max(base),
        }
    }

    /// 第 `attempts` 次启动失败后的等待秒数：`min(base * 2^(attempts-1), max)`
    pub fn backoff(&self, attempts: i32) -> i64 {
        let p = self.normalized();
        let n = attempts.clamp(1, 16) as u32;
        let factor = 1i64.checked_shl(n - 1).unwrap_or(i64::MAX);
        p.base_secs.saturating_mul(factor).min(p.max_secs)
    }
}

/// 调度配置（来自启动参数；内部运维参数，不进应用配置项）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutoRestartConfig {
    /// backoff 策略
    pub backoff: BackoffPolicy,
}

/// 自动重启运行态（持久化在 `modelinfer_deployments`）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RuntimeState {
    /// 相位：`""` / `stopping` / `starting`
    pub phase: String,
    /// 本轮连续失败起点（CST ISO-8601；仅展示 / 诊断）
    pub failed_since: String,
    /// 当前连续探活失败次数
    pub consecutive_failures: i32,
    /// backoff 截止时间（CST ISO-8601）
    pub next_at: String,
    /// 当前循环内已下发的启动次数
    pub attempts: i32,
    /// 最近一次动作下发时间（CST ISO-8601）
    pub last_at: String,
    /// 最近一次失败原因
    pub last_error: String,
    /// 用户手动启停 / 配置变更后的抑制起点：早于该时间的启动失败不再触发自动重启
    pub ignore_before: String,
}

/// 决策输入（纯数据，便于单测穷举）
#[derive(Debug, Clone)]
pub struct AutoRestartInput<'a> {
    /// 自动重启策略（`never` / `unless-stop`）
    pub policy: &'a str,
    /// 连续探活失败次数阈值
    pub fail_count: i32,
    /// 当前运行态
    pub runtime: RuntimeState,
    /// 部署状态
    pub state: &'a str,
    /// 期望状态
    pub desired_state: &'a str,
    /// 健康状态
    pub health_status: &'a str,
    /// 本轮自动启动是否已**明确失败**（启动 run 在本次下发后已收尾为 `failed`）
    ///
    /// 阶段脚本失败时启动 workflow 会先 `mark_failed` 再执行 `cleanup`，最终状态回到
    /// `stopped`，与「已下发但尚未置位」无法从状态区分；该标记消除歧义。
    pub start_run_failed: bool,
    /// idle 阶段是否存在**未被人工操作抑制**的失败启动 run（覆盖首次启动失败）
    pub unhandled_start_failure: bool,
    /// backoff 策略
    pub backoff: BackoffPolicy,
}

/// 决策动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 不下发
    None,
    /// 下发停止工作流
    Stop,
    /// 下发启动工作流
    Start,
}

/// 决策结果：新的运行态 + 动作
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// 决策后的运行态（需持久化）
    pub runtime: RuntimeState,
    /// 需要下发的动作
    pub action: Action,
}

impl Decision {
    fn keep(runtime: &RuntimeState) -> Self {
        Self {
            runtime: runtime.clone(),
            action: Action::None,
        }
    }
}

fn now_iso(now: DateTime<FixedOffset>) -> String {
    now.to_rfc3339()
}

/// 有效自动重启策略（兼容历史 `auto_restart_enabled`）
pub fn effective_policy(m: &deployment::Model) -> String {
    let raw = m.auto_restart_policy.trim();
    if raw.is_empty() {
        return if m.auto_restart_enabled {
            POLICY_UNLESS_STOP.to_string()
        } else {
            POLICY_NEVER.to_string()
        };
    }
    match raw.to_ascii_lowercase().as_str() {
        "never" => POLICY_NEVER.to_string(),
        "unless-stop" | "unless_stop" | "unless-stopped" | "unless_stopped" => {
            POLICY_UNLESS_STOP.to_string()
        }
        _ => POLICY_NEVER.to_string(),
    }
}

/// 有效连续探活失败次数阈值（历史秒阈值按 30s 周期折算）
pub fn effective_fail_count(m: &deployment::Model) -> i32 {
    if m.auto_restart_policy.trim().is_empty() && m.auto_restart_enabled {
        return validate::legacy_auto_restart_fail_count(m.auto_restart_fail_secs as i64);
    }
    if m.auto_restart_fail_count > 0 {
        return m.auto_restart_fail_count.clamp(
            validate::MIN_AUTO_RESTART_FAIL_COUNT as i32,
            validate::MAX_AUTO_RESTART_FAIL_COUNT as i32,
        );
    }
    validate::DEFAULT_AUTO_RESTART_FAIL_COUNT
}

/// 纯函数决策：依据运行态与当前部署 / 健康状态给出「新运行态 + 动作」
pub fn decide(input: &AutoRestartInput<'_>, now: DateTime<FixedOffset>) -> Decision {
    let cur = &input.runtime;
    let now_epoch = now.timestamp();

    // 1. 非 unless-stop：运行态清零，不下发
    if input.policy != POLICY_UNLESS_STOP {
        if *cur != RuntimeState::default() {
            return Decision {
                runtime: RuntimeState::default(),
                action: Action::None,
            };
        }
        return Decision::keep(cur);
    }

    let next_at = time::parse_iso_epoch(&cur.next_at);
    let last_at = time::parse_iso_epoch(&cur.last_at);
    let elapsed_since_last = last_at.map(|t| now_epoch - t);
    let threshold = input.fail_count.clamp(1, 100);

    match cur.phase.as_str() {
        // 2~5. idle：累计连续探活失败 / 达阈值触发 / 健康清零 / 首次启动失败
        PHASE_IDLE => {
            if input.state == "running" && input.health_status == "unhealthy" {
                let failures = cur.consecutive_failures.saturating_add(1);
                if failures >= threshold {
                    return Decision {
                        runtime: RuntimeState {
                            phase: PHASE_STOPPING.to_string(),
                            failed_since: String::new(),
                            consecutive_failures: 0,
                            next_at: String::new(),
                            attempts: 1,
                            last_at: now_iso(now),
                            last_error: format!(
                                "连续探活失败 {failures} 次（阈值 {threshold}），自动重启：停止清理"
                            ),
                            ignore_before: cur.ignore_before.clone(),
                        },
                        action: Action::Stop,
                    };
                }
                return Decision {
                    runtime: RuntimeState {
                        failed_since: if cur.failed_since.is_empty() {
                            now_iso(now)
                        } else {
                            cur.failed_since.clone()
                        },
                        consecutive_failures: failures,
                        ..cur.clone()
                    },
                    action: Action::None,
                };
            }
            if input.state == "running" && input.health_status == "healthy" {
                if cur.failed_since.is_empty() && cur.consecutive_failures == 0 {
                    return Decision::keep(cur);
                }
                return Decision {
                    runtime: RuntimeState {
                        failed_since: String::new(),
                        consecutive_failures: 0,
                        ..cur.clone()
                    },
                    action: Action::None,
                };
            }
            // 首次 / 非循环内启动失败：under unless-stop 同样自动重启。
            // `state=failed` + 空 ignore 覆盖 prepare 早期失败（可能没有启动 run）的兜底。
            if input.unhandled_start_failure
                || (input.state == "failed" && cur.ignore_before.is_empty())
            {
                let wait = input.backoff.backoff(1);
                return Decision {
                    runtime: RuntimeState {
                        phase: PHASE_STOPPING.to_string(),
                        failed_since: String::new(),
                        consecutive_failures: 0,
                        next_at: time::iso_after(now, wait),
                        attempts: 1,
                        last_at: now_iso(now),
                        last_error: format!("启动失败，{wait} 秒后自动重试（先停止清理）"),
                        ignore_before: cur.ignore_before.clone(),
                    },
                    action: Action::Stop,
                };
            }
            Decision::keep(cur)
        }
        // 6~8. stopping：等待停止完成 → 到点启动；卡死 / 未生效 → 重复下发停止
        PHASE_STOPPING => {
            if input.state == "stopped" && input.desired_state == "stopped" {
                let ready = next_at.map(|t| now_epoch >= t).unwrap_or(true);
                if !ready {
                    return Decision::keep(cur);
                }
                return Decision {
                    runtime: RuntimeState {
                        phase: PHASE_STARTING.to_string(),
                        failed_since: cur.failed_since.clone(),
                        consecutive_failures: cur.consecutive_failures,
                        next_at: cur.next_at.clone(),
                        attempts: cur.attempts.max(1),
                        last_at: now_iso(now),
                        last_error: cur.last_error.clone(),
                        ignore_before: cur.ignore_before.clone(),
                    },
                    action: Action::Start,
                };
            }
            if input.state == "stopping" || input.state == "failed" {
                let stuck = elapsed_since_last
                    .map(|e| e >= STOP_STUCK_SECS)
                    .unwrap_or(true);
                if stuck {
                    return Decision {
                        runtime: RuntimeState {
                            last_at: now_iso(now),
                            last_error: format!(
                                "停止清理未在 {STOP_STUCK_SECS} 秒内完成（state={}），重新下发停止",
                                input.state
                            ),
                            ..cur.clone()
                        },
                        action: Action::Stop,
                    };
                }
                return Decision::keep(cur);
            }
            if input.state == "running" {
                let stale = elapsed_since_last
                    .map(|e| e >= STOP_DISPATCH_GRACE_SECS)
                    .unwrap_or(true);
                if stale {
                    return Decision {
                        runtime: RuntimeState {
                            last_at: now_iso(now),
                            last_error: "停止未生效，重新下发停止".to_string(),
                            ..cur.clone()
                        },
                        action: Action::Stop,
                    };
                }
            }
            Decision::keep(cur)
        }
        // 9~10. starting：成功清零；失败进入 backoff 并清理
        PHASE_STARTING => {
            if input.state == "running" {
                return Decision {
                    runtime: RuntimeState::default(),
                    action: Action::None,
                };
            }
            if input.state == "failed" || input.state == "stopped" {
                // `failed` 是启动 workflow 明确失败（已收尾），可立即进入 backoff；
                // `stopped` 可能是「已下发启动、workflow 尚未把状态置为 starting」，需宽限。
                let elapsed_ok = input.state == "failed"
                    || input.start_run_failed
                    || elapsed_since_last
                        .map(|e| e >= START_GRACE_SECS)
                        .unwrap_or(true);
                if !elapsed_ok {
                    return Decision::keep(cur);
                }
                let attempts = cur.attempts.max(0) + 1;
                let wait = input.backoff.backoff(attempts);
                return Decision {
                    runtime: RuntimeState {
                        phase: PHASE_STOPPING.to_string(),
                        failed_since: String::new(),
                        consecutive_failures: 0,
                        next_at: time::iso_after(now, wait),
                        attempts,
                        last_at: now_iso(now),
                        last_error: format!(
                            "第 {attempts} 次自动启动未成功（state={}），{wait} 秒后重试",
                            input.state
                        ),
                        ignore_before: cur.ignore_before.clone(),
                    },
                    action: Action::Stop,
                };
            }
            Decision::keep(cur)
        }
        _ => Decision::keep(cur),
    }
}

/// 从实体加载运行态
pub fn runtime_of(m: &deployment::Model) -> RuntimeState {
    RuntimeState {
        phase: m.auto_restart_phase.clone(),
        failed_since: m.auto_restart_failed_since.clone(),
        consecutive_failures: m.auto_restart_consecutive_failures,
        next_at: m.auto_restart_next_at.clone(),
        attempts: m.auto_restart_attempts,
        last_at: m.auto_restart_last_at.clone(),
        last_error: m.auto_restart_last_error.clone(),
        ignore_before: m.auto_restart_ignore_before.clone(),
    }
}

/// 持久化运行态（仅写运行态列，不触碰配置列）
pub async fn save(
    db: &DatabaseConnection,
    deployment_id: &str,
    rt: &RuntimeState,
) -> Result<(), ModelinferError> {
    let m = deployment::Entity::find_by_id(deployment_id.to_string())
        .one(db)
        .await
        .map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("查询运行配置失败: {e}")))?
        .ok_or_else(|| ModelinferError::new(codes::NOT_FOUND, "运行配置不存在"))?;
    let mut am: deployment::ActiveModel = m.into();
    am.auto_restart_phase = Set(rt.phase.clone());
    am.auto_restart_failed_since = Set(rt.failed_since.clone());
    am.auto_restart_consecutive_failures = Set(rt.consecutive_failures);
    am.auto_restart_next_at = Set(rt.next_at.clone());
    am.auto_restart_attempts = Set(rt.attempts);
    am.auto_restart_last_at = Set(rt.last_at.clone());
    am.auto_restart_last_error = Set(rt.last_error.clone());
    am.auto_restart_ignore_before = Set(rt.ignore_before.clone());
    // 运行态计数/相位变化不刷新 `updated_at`：避免探活失败期间列表排序每 30s 抖动；
    // 真正的状态迁移由 apply_state / 工作流更新 `updated_at`。
    am.update(db).await.map_err(|e| {
        ModelinferError::new(codes::DB_FAILED, format!("更新自动重启状态失败: {e}"))
    })?;
    Ok(())
}

/// 清零运行态（用户手动启动 / 停止、配置变更时调用）
///
/// 语义：用户操作 = **取消自动重启循环**。同时记录 `ignore_before=now`，使人工操作前
/// 已失败的启动 run 不会在下一轮被误判为新失败。
pub async fn reset(db: &DatabaseConnection, deployment_id: &str) -> Result<(), ModelinferError> {
    let rt = RuntimeState {
        ignore_before: time::now_iso(),
        ..Default::default()
    };
    save(db, deployment_id, &rt).await
}

/// 单次调度报告（日志 / 测试用）
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct TickReport {
    /// 扫描到的开启 unless-stop 的运行配置数
    pub scanned: usize,
    /// 实际下发停止的次数
    pub stops: usize,
    /// 实际下发启动的次数
    pub starts: usize,
    /// 决策 / 下发失败的次数
    pub errors: usize,
}

fn iso_ge(a: &str, b: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(a.trim()),
        DateTime::parse_from_rfc3339(b.trim()),
    ) {
        (Ok(a), Ok(b)) => a >= b,
        _ => true,
    }
}

fn iso_gt(a: &str, b: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(a.trim()),
        DateTime::parse_from_rfc3339(b.trim()),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => !b.trim().is_empty(),
    }
}

/// 最近一条启动 run（用于识别「本轮启动已失败」与「首次启动失败」）
async fn latest_start_run(
    db: &DatabaseConnection,
    deployment_id: &str,
) -> Result<Option<crate::observability::store::RunView>, ModelinferError> {
    let runs =
        crate::observability::store::list_runs(db, deployment_id, Some("start"), Some(1)).await?;
    Ok(runs.into_iter().next())
}

/// 执行一轮自动重启调度
///
/// `control = None`（Temporal 不可用）时只做判定与运行态持久化，不下发动作。
pub async fn tick(
    db: &DatabaseConnection,
    control: Option<&dyn Control>,
    cfg: &AutoRestartConfig,
    now: DateTime<FixedOffset>,
) -> Result<TickReport, ModelinferError> {
    let rows = deployment::Entity::find()
        .all(db)
        .await
        .map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("查询运行配置失败: {e}")))?;

    let mut report = TickReport::default();
    for m in rows {
        let policy = effective_policy(&m);
        if policy == POLICY_UNLESS_STOP {
            report.scanned += 1;
        }
        let current = runtime_of(&m);
        let latest = latest_start_run(db, &m.id).await.unwrap_or(None);
        let start_run_failed = current.phase == PHASE_STARTING
            && latest
                .as_ref()
                .map(|r| {
                    r.state == "failed"
                        && (current.last_at.is_empty() || iso_ge(&r.started_at, &current.last_at))
                })
                .unwrap_or(false);
        let unhandled_start_failure = latest
            .as_ref()
            .map(|r| r.state == "failed" && iso_gt(&r.started_at, &current.ignore_before))
            .unwrap_or(false);
        let input = AutoRestartInput {
            policy: &policy,
            fail_count: effective_fail_count(&m),
            runtime: current.clone(),
            state: &m.state,
            desired_state: &m.desired_state,
            health_status: &m.health_status,
            start_run_failed,
            unhandled_start_failure,
            backoff: cfg.backoff,
        };
        let decision = decide(&input, now);
        let mut runtime = decision.runtime.clone();
        if runtime != current
            && let Err(e) = save(db, &m.id, &runtime).await
        {
            report.errors += 1;
            tracing::warn!(
                target: "modelinfer_autorestart",
                deployment = %m.id,
                error = %e.msg,
                "持久化自动重启运行态失败（本轮跳过动作）"
            );
            continue;
        }
        if decision.action == Action::None {
            continue;
        }
        let Some(ctl) = control else {
            tracing::warn!(
                target: "modelinfer_autorestart",
                deployment = %m.id,
                phase = %runtime.phase,
                action = ?decision.action,
                "Temporal 不可用，自动重启动作未下发（下轮重试）"
            );
            continue;
        };
        match decision.action {
            Action::Stop => {
                let reason = if runtime.last_error.is_empty() {
                    "自动重启：停止清理".to_string()
                } else {
                    format!("自动重启：停止清理（{}）", runtime.last_error)
                };
                match ctl.stop(&m.id, false, &reason).await {
                    Ok(()) => {
                        report.stops += 1;
                        tracing::warn!(
                            target: "modelinfer_autorestart",
                            deployment = %m.id,
                            attempts = runtime.attempts,
                            "已下发自动重启停止"
                        );
                    }
                    Err(e) => {
                        report.errors += 1;
                        runtime.last_error = format!("下发停止失败: {}", e.msg);
                        let _ = save(db, &m.id, &runtime).await;
                        tracing::warn!(
                            target: "modelinfer_autorestart",
                            deployment = %m.id,
                            error = %e.msg,
                            "下发自动重启停止失败（下轮重试）"
                        );
                    }
                }
            }
            Action::Start => match ctl.start(db, &m.id, TRIGGER_AUTO_RESTART).await {
                Ok(()) => {
                    report.starts += 1;
                    tracing::warn!(
                        target: "modelinfer_autorestart",
                        deployment = %m.id,
                        attempts = runtime.attempts,
                        "已下发自动重启启动"
                    );
                }
                Err(e) => {
                    report.errors += 1;
                    // 下发失败（Temporal 抖动 / 工作流启动失败等）：直接进入 backoff，避免空转
                    let attempts = runtime.attempts.max(0) + 1;
                    let wait = cfg.backoff.backoff(attempts);
                    runtime.phase = PHASE_STOPPING.to_string();
                    runtime.attempts = attempts;
                    runtime.next_at = time::iso_after(now, wait);
                    runtime.last_at = now_iso(now);
                    runtime.last_error = format!("下发启动失败（{wait} 秒后重试）: {}", e.msg);
                    let _ = save(db, &m.id, &runtime).await;
                    tracing::warn!(
                        target: "modelinfer_autorestart",
                        deployment = %m.id,
                        error = %e.msg,
                        wait,
                        "下发自动重启启动失败"
                    );
                }
            },
            Action::None => {}
        }
    }
    Ok(report)
}

/// 启动进程级自动重启调度器（后台 tokio 任务，非阻塞）
///
/// 周期与健康巡检一致（由调用方传同一个 interval）；`control = None` 时只做判定与日志。
pub fn spawn_scheduler(
    db: DatabaseConnection,
    control: Option<std::sync::Arc<dyn Control>>,
    cfg: AutoRestartConfig,
    interval_secs: u64,
) {
    let interval = interval_secs.max(5);
    tokio::spawn(async move {
        tracing::info!(
            target: "modelinfer_autorestart",
            interval,
            enabled = control.is_some(),
            base_secs = cfg.backoff.base_secs,
            max_secs = cfg.backoff.max_secs,
            "自动重启调度器启动（进程级，非 Temporal workflow）"
        );
        loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            if let Err(e) = tick(&db, control.as_deref(), &cfg, time::now()).await {
                tracing::warn!(
                    target: "modelinfer_autorestart",
                    error = %e.msg,
                    "自动重启调度失败"
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, Set};
    use serde_json::Map;

    use crate::deploy::control::MockControl;
    use crate::deployments::{self, DeploymentCreate};
    use crate::nodes;
    use crate::platform::{DeviceInfo, MockPlatformClient, NfsShareInfo, VolumeInfo};
    use crate::ssh::MockSshExec;

    fn at(iso: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(iso).unwrap()
    }

    fn input<'a>(
        policy: &'a str,
        fail_count: i32,
        runtime: RuntimeState,
        state: &'a str,
        desired: &'a str,
        health: &'a str,
    ) -> AutoRestartInput<'a> {
        AutoRestartInput {
            policy,
            fail_count,
            runtime,
            state,
            desired_state: desired,
            health_status: health,
            start_run_failed: false,
            unhandled_start_failure: false,
            backoff: BackoffPolicy::default(),
        }
    }

    fn rt(
        phase: &str,
        failed_since: &str,
        next_at: &str,
        attempts: i32,
        last_at: &str,
    ) -> RuntimeState {
        RuntimeState {
            phase: phase.into(),
            failed_since: failed_since.into(),
            next_at: next_at.into(),
            attempts,
            last_at: last_at.into(),
            last_error: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let p = BackoffPolicy {
            base_secs: 15,
            max_secs: 300,
        };
        assert_eq!(p.backoff(1), 15);
        assert_eq!(p.backoff(2), 30);
        assert_eq!(p.backoff(3), 60);
        assert_eq!(p.backoff(4), 120);
        assert_eq!(p.backoff(5), 240);
        assert_eq!(p.backoff(6), 300);
        assert_eq!(p.backoff(1000), 300);
        assert_eq!(p.backoff(0), 15);
        assert_eq!(p.backoff(-5), 15);
        let weird = BackoffPolicy {
            base_secs: 0,
            max_secs: 0,
        };
        assert_eq!(weird.backoff(3), 1, "base 归一到 1、max 归一到 base");
    }

    #[test]
    fn never_clears_runtime_and_never_acts() {
        let now = at("2026-10-21T12:00:00+08:00");
        let r = rt(PHASE_STARTING, "", "", 3, "2026-10-21T11:00:00+08:00");
        let d = decide(
            &input(POLICY_NEVER, 10, r, "running", "running", "unhealthy"),
            now,
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime, RuntimeState::default());

        let d = decide(
            &input(
                POLICY_NEVER,
                10,
                RuntimeState::default(),
                "running",
                "running",
                "healthy",
            ),
            now,
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime, RuntimeState::default());
    }

    #[test]
    fn idle_counts_consecutive_failures_without_restart() {
        let now = at("2026-10-21T12:00:00+08:00");
        let d = decide(
            &input(
                POLICY_UNLESS_STOP,
                3,
                RuntimeState::default(),
                "running",
                "running",
                "unhealthy",
            ),
            now,
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime.consecutive_failures, 1);
        assert_eq!(d.runtime.failed_since, "2026-10-21T12:00:00+08:00");

        let mut r = d.runtime;
        r.consecutive_failures = 2;
        r.failed_since = "2026-10-21T12:00:00+08:00".into();
        let d = decide(
            &input(
                POLICY_UNLESS_STOP,
                4,
                r.clone(),
                "running",
                "running",
                "unhealthy",
            ),
            at("2026-10-21T12:01:00+08:00"),
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(
            d.runtime.consecutive_failures, 3,
            "阈值 4 时第 3 次仍不重启"
        );

        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "running", "running", "unhealthy"),
            at("2026-10-21T12:01:00+08:00"),
        );
        assert_eq!(d.action, Action::Stop, "第 3 次达到阈值 → 自动重启");
    }

    #[test]
    fn idle_triggers_stop_when_failure_count_reaches_threshold() {
        let now = at("2026-10-21T12:05:00+08:00");
        let mut r = rt("", "2026-10-21T12:00:00+08:00", "", 0, "");
        r.consecutive_failures = 2;
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "running", "running", "unhealthy"),
            now,
        );
        assert_eq!(d.action, Action::Stop);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
        assert_eq!(d.runtime.attempts, 1);
        assert_eq!(d.runtime.consecutive_failures, 0);
        assert_eq!(d.runtime.last_at, "2026-10-21T12:05:00+08:00");
        assert!(d.runtime.last_error.contains("连续探活失败 3 次"));
    }

    #[test]
    fn healthy_clears_failure_count() {
        let now = at("2026-10-21T12:01:00+08:00");
        let mut r = rt("", "2026-10-21T12:00:00+08:00", "", 0, "");
        r.consecutive_failures = 2;
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "running", "running", "healthy"),
            now,
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime.failed_since, "");
        assert_eq!(d.runtime.consecutive_failures, 0);
    }

    #[test]
    fn idle_first_start_failure_triggers_restart() {
        let now = at("2026-10-21T12:00:00+08:00");
        let mut inp = input(
            POLICY_UNLESS_STOP,
            3,
            RuntimeState::default(),
            "stopped",
            "stopped",
            "unknown",
        );
        inp.unhandled_start_failure = true;
        let d = decide(&inp, now);
        assert_eq!(d.action, Action::Stop);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
        assert_eq!(d.runtime.attempts, 1);
        assert_eq!(d.runtime.next_at, "2026-10-21T12:00:15+08:00");

        // 人工 reset 后（ignore_before=now）同样的旧失败 run 不再触发
        let r = RuntimeState {
            ignore_before: "2026-10-21T12:00:00+08:00".into(),
            ..Default::default()
        };
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "stopped", "stopped", "unknown"),
            at("2026-10-21T12:00:30+08:00"),
        );
        assert_eq!(d.action, Action::None);
    }

    #[test]
    fn idle_state_failed_without_run_is_restarted_when_not_suppressed() {
        let now = at("2026-10-21T12:00:00+08:00");
        let d = decide(
            &input(
                POLICY_UNLESS_STOP,
                3,
                RuntimeState::default(),
                "failed",
                "stopped",
                "unknown",
            ),
            now,
        );
        assert_eq!(d.action, Action::Stop);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
    }

    #[test]
    fn stopping_launches_start_once_stopped_and_backoff_passed() {
        let now = at("2026-10-21T12:06:00+08:00");
        let r = rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let d = decide(
            &input(
                POLICY_UNLESS_STOP,
                3,
                r.clone(),
                "stopped",
                "stopped",
                "unknown",
            ),
            now,
        );
        assert_eq!(d.action, Action::Start);
        assert_eq!(d.runtime.phase, PHASE_STARTING);

        let r2 = rt(
            PHASE_STOPPING,
            "",
            "2026-10-21T12:10:00+08:00",
            2,
            "2026-10-21T12:05:00+08:00",
        );
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r2, "stopped", "stopped", "unknown"),
            now,
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
    }

    #[test]
    fn stopping_does_not_start_while_still_stopping() {
        let now = at("2026-10-21T12:06:00+08:00");
        let r = rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:05:30+08:00");
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "stopping", "stopping", "unknown"),
            now,
        );
        assert_eq!(d.action, Action::None);
    }

    #[test]
    fn stopping_retries_when_stuck_or_not_effective() {
        let now = at("2026-10-21T12:30:00+08:00");
        let r = rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:00:00+08:00");
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "stopping", "stopping", "unknown"),
            now,
        );
        assert_eq!(d.action, Action::Stop);
        assert!(d.runtime.last_error.contains("停止清理未在"));

        let r = rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:00:00+08:00");
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "running", "stopping", "unknown"),
            now,
        );
        assert_eq!(d.action, Action::Stop);
    }

    #[test]
    fn starting_success_resets_everything() {
        let now = at("2026-10-21T12:07:00+08:00");
        let r = rt(
            PHASE_STARTING,
            "2026-10-21T12:00:00+08:00",
            "",
            3,
            "2026-10-21T12:05:00+08:00",
        );
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "running", "running", "healthy"),
            now,
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime, RuntimeState::default());
    }

    #[test]
    fn starting_failure_enters_backoff_with_growing_wait() {
        let now = at("2026-10-21T12:10:00+08:00");
        let r = rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "failed", "stopped", "unknown"),
            now,
        );
        assert_eq!(d.action, Action::Stop);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
        assert_eq!(d.runtime.attempts, 2);
        assert_eq!(d.runtime.next_at, "2026-10-21T12:10:30+08:00");
        assert!(d.runtime.last_error.contains("第 2 次"));

        let r = rt(PHASE_STARTING, "", "", 4, "2026-10-21T12:05:00+08:00");
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "failed", "stopped", "unknown"),
            now,
        );
        assert_eq!(d.runtime.attempts, 5);
        assert_eq!(d.runtime.next_at, "2026-10-21T12:14:00+08:00");
    }

    #[test]
    fn starting_failed_run_judged_immediately_even_when_stopped() {
        let now = at("2026-10-21T12:05:30+08:00");
        let r = rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let mut inp = input(POLICY_UNLESS_STOP, 3, r, "stopped", "stopped", "unknown");
        inp.start_run_failed = true;
        let d = decide(&inp, now);
        assert_eq!(d.action, Action::Stop);
        assert_eq!(d.runtime.attempts, 2);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
    }

    #[test]
    fn starting_failure_within_grace_is_not_judged() {
        let now = at("2026-10-21T12:06:00+08:00");
        let r = rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let d = decide(
            &input(POLICY_UNLESS_STOP, 3, r, "stopped", "stopped", "unknown"),
            now,
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime.phase, PHASE_STARTING);
    }

    #[test]
    fn unknown_phase_is_treated_as_idle_but_preserved() {
        let now = at("2026-10-21T12:06:00+08:00");
        let r = rt("weird", "", "", 0, "");
        let d = decide(
            &input(
                POLICY_UNLESS_STOP,
                3,
                r.clone(),
                "running",
                "running",
                "unhealthy",
            ),
            now,
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime, r);
    }

    // ---- tick 集成（sqlite + MockControl） ----

    async fn seed_deployment(db: &DatabaseConnection, policy: &str, fail_count: i32) -> String {
        let now = time::now_iso();
        let manifest = serde_json::json!({
            "id": "com.ictrek.ar-test",
            "version": "1.0.0",
            "model": "org/w",
            "lifecycle": { "start": "./s.sh", "health-probe": "./hp.sh" },
        });
        if crate::entities::plaza_model::Entity::find_by_id("com.ictrek.ar-test@1.0.0")
            .one(db)
            .await
            .unwrap()
            .is_none()
        {
            crate::entities::plaza_model::ActiveModel {
                id: Set("com.ictrek.ar-test@1.0.0".into()),
                model_id: Set("com.ictrek.ar-test".into()),
                model: Set("org/w".into()),
                name: Set("ar".into()),
                version: Set("1.0.0".into()),
                source: Set("plaza".into()),
                source_id: Set(String::new()),
                path: Set("/tmp/x".into()),
                package_path: Set("/tmp/x/model.tar.gz".into()),
                package_sha256: Set(String::new()),
                manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
                state: Set("available".into()),
                size_bytes: Set(0),
                created_at: Set(now.clone()),
                updated_at: Set(now.clone()),
            }
            .insert(db)
            .await
            .unwrap();
        }

        let mut platform = MockPlatformClient::new();
        platform.app_configs = serde_json::json!({
            "MODELINFER_MODEL_HUB_PATH": {
                "source_type": "host_path",
                "value": "/share/v1/model_hub",
                "display_path": "/share/v1/model_hub"
            }
        });
        platform.volumes = vec![VolumeInfo {
            uuid: "v1".into(),
            name: "v1".into(),
            mount_point: "/share/v1".into(),
            state: "Running".into(),
        }];
        platform.nfs_shares = vec![NfsShareInfo {
            uuid: "s-v1".into(),
            volume_uuid: "v1".into(),
            name: "v1".into(),
            mount_point: "/share/v1".into(),
            read_only: false,
            state: "Running".into(),
            mounted: true,
        }];
        platform.device = DeviceInfo {
            ip_addr: vec!["10.0.0.5".into()],
        };

        let node = nodes::create_node(
            db,
            &MockSshExec::new().with_rule("hostname", 0, "h1"),
            crate::nodes::NodeCreate {
                profile: None,
                name: "n1".into(),
                host: "10.0.0.1".into(),
                access_mode: "ssh".into(),
                ssh_port: 22,
                ssh_user: "root".into(),
                auth_method: "password".into(),
                ssh_password: "x".into(),
                ssh_key: String::new(),
                sudo_password: String::new(),
                hostname: Some("h1".into()),
                socket_ifname: Some("eth0".into()),
                ib_hca: Some(String::new()),
                fscache: false,
                app_model_hub_nfs: true,
                node_exporter_port: crate::nodes::DEFAULT_NODE_EXPORTER_PORT,
                nvidia_gpu_exporter_port: crate::nodes::DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
            },
        )
        .await
        .unwrap()
        .id;

        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/org/w/current")).unwrap();
        let id = deployments::create_deployment(
            db,
            tmp.path(),
            DeploymentCreate {
                name: "ar".into(),
                plaza_model_id: "com.ictrek.ar-test@1.0.0".into(),
                hub_source: "hf".into(),
                node_ids: vec![node],
                configs: Map::new(),
                auto_restart_policy: Some(policy.into()),
                auto_restart_fail_count: Some(fail_count as i64),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id;
        std::mem::forget(tmp);
        id
    }

    async fn set_state(
        db: &DatabaseConnection,
        id: &str,
        state: &str,
        desired: &str,
        health: &str,
    ) {
        let m = deployment::Entity::find_by_id(id.to_string())
            .one(db)
            .await
            .unwrap()
            .unwrap();
        let mut am: deployment::ActiveModel = m.into();
        am.state = Set(state.into());
        am.desired_state = Set(desired.into());
        am.health_status = Set(health.into());
        am.update(db).await.unwrap();
    }

    async fn seed_runtime(db: &DatabaseConnection, id: &str, runtime: RuntimeState) {
        save(db, id, &runtime).await.unwrap();
    }

    async fn get_rt(db: &DatabaseConnection, id: &str) -> RuntimeState {
        let m = deployment::Entity::find_by_id(id.to_string())
            .one(db)
            .await
            .unwrap()
            .unwrap();
        runtime_of(&m)
    }

    async fn seed_failed_start_run(db: &DatabaseConnection, deployment_id: &str, started_at: &str) {
        let run_id = crate::observability::store::create_run(
            db,
            deployment_id,
            crate::observability::kind::START,
            TRIGGER_AUTO_RESTART,
            "阶段 start 执行失败",
            "",
        )
        .await
        .unwrap();
        let m = crate::entities::run::Entity::find_by_id(run_id)
            .one(db)
            .await
            .unwrap()
            .unwrap();
        let mut am: crate::entities::run::ActiveModel = m.into();
        am.state = Set(crate::observability::state::FAILED.into());
        am.started_at = Set(started_at.into());
        am.finished_at = Set(Some(started_at.into()));
        am.error = Set("stage failed".into());
        am.update(db).await.unwrap();
    }

    #[tokio::test]
    async fn tick_full_restart_cycle_by_probe_failure_count() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, POLICY_UNLESS_STOP, 2).await;
        let cfg = AutoRestartConfig::default();
        let control = MockControl::new();
        let t0 = at("2026-10-21T12:00:00+08:00");

        // 第 1 次失败：累计计数，不重启
        set_state(&db, &id, "running", "running", "unhealthy").await;
        tick(&db, Some(&control), &cfg, t0).await.unwrap();
        assert_eq!(control.calls().len(), 0);
        assert_eq!(get_rt(&db, &id).await.consecutive_failures, 1);

        // 第 2 次失败：达到阈值 → 下发停止
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:00:30+08:00"))
            .await
            .unwrap();
        assert_eq!(control.stop_count(), 1);
        assert_eq!(get_rt(&db, &id).await.phase, PHASE_STOPPING);

        // 停止完成 → 下发启动
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:00:40+08:00"))
            .await
            .unwrap();
        assert_eq!(control.start_count(), 1);
        assert_eq!(
            control.calls()[1],
            crate::deploy::control::ControlCall::Start {
                deployment_id: id.clone(),
                trigger: TRIGGER_AUTO_RESTART.into()
            }
        );
        assert_eq!(get_rt(&db, &id).await.phase, PHASE_STARTING);

        // 启动成功 → 清零
        set_state(&db, &id, "running", "running", "healthy").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:01:00+08:00"))
            .await
            .unwrap();
        assert_eq!(get_rt(&db, &id).await, RuntimeState::default());
        assert_eq!(control.stop_count(), 1);
        assert_eq!(control.start_count(), 1);
    }

    #[tokio::test]
    async fn tick_first_start_failure_auto_restarts() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, POLICY_UNLESS_STOP, 3).await;
        let cfg = AutoRestartConfig::default();
        let control = MockControl::new();
        let t0 = at("2026-10-21T12:00:00+08:00");

        // 用户手动启动后 reset 写入了 ignore_before；随后启动 run 在 t0+1s 失败并 cleanup 到 stopped。
        let r = RuntimeState {
            ignore_before: "2026-10-21T12:00:00+08:00".into(),
            ..Default::default()
        };
        seed_runtime(&db, &id, r).await;
        seed_failed_start_run(&db, &id, "2026-10-21T12:00:01+08:00").await;
        set_state(&db, &id, "stopped", "stopped", "unknown").await;

        tick(&db, Some(&control), &cfg, t0).await.unwrap();
        let r = get_rt(&db, &id).await;
        assert_eq!(control.stop_count(), 1, "首次启动失败也应自动停止清理");
        assert_eq!(r.phase, PHASE_STOPPING);
        assert_eq!(r.attempts, 1);
        assert_eq!(r.next_at, "2026-10-21T12:00:15+08:00");

        // 停止完成且过 backoff → 下发启动
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:00:16+08:00"))
            .await
            .unwrap();
        assert_eq!(control.start_count(), 1);
    }

    #[tokio::test]
    async fn tick_reset_suppresses_stale_failed_run() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, POLICY_UNLESS_STOP, 3).await;
        seed_failed_start_run(&db, &id, "2026-10-21T12:00:01+08:00").await;
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        reset(&db, &id).await.unwrap();

        let control = MockControl::new();
        tick(
            &db,
            Some(&control),
            &AutoRestartConfig::default(),
            at("2026-10-21T12:00:30+08:00"),
        )
        .await
        .unwrap();
        assert_eq!(
            control.calls().len(),
            0,
            "人工 reset 后旧失败 run 不得再触发"
        );
        assert_eq!(get_rt(&db, &id).await.phase, "");
    }

    #[tokio::test]
    async fn tick_start_failure_grows_backoff_and_retries() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, POLICY_UNLESS_STOP, 3).await;
        let cfg = AutoRestartConfig::default();
        let control = MockControl::new();

        seed_runtime(
            &db,
            &id,
            rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:00:00+08:00"),
        )
        .await;
        set_state(&db, &id, "failed", "stopped", "unknown").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:05:00+08:00"))
            .await
            .unwrap();
        let r = get_rt(&db, &id).await;
        assert_eq!(control.stop_count(), 1);
        assert_eq!(r.attempts, 2);
        assert_eq!(r.phase, PHASE_STOPPING);
        assert_eq!(r.next_at, "2026-10-21T12:05:30+08:00");

        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:05:10+08:00"))
            .await
            .unwrap();
        assert_eq!(control.start_count(), 0);

        tick(&db, Some(&control), &cfg, at("2026-10-21T12:05:31+08:00"))
            .await
            .unwrap();
        assert_eq!(control.start_count(), 1);

        set_state(&db, &id, "failed", "stopped", "unknown").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:10:00+08:00"))
            .await
            .unwrap();
        let r = get_rt(&db, &id).await;
        assert_eq!(r.attempts, 3);
        assert_eq!(r.next_at, "2026-10-21T12:11:00+08:00");
    }

    #[tokio::test]
    async fn tick_skips_never_policy() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, POLICY_NEVER, 3).await;
        set_state(&db, &id, "running", "running", "unhealthy").await;
        let control = MockControl::new();
        let report = tick(
            &db,
            Some(&control),
            &AutoRestartConfig::default(),
            time::now(),
        )
        .await
        .unwrap();
        assert_eq!(report.scanned, 0);
        assert_eq!(control.calls().len(), 0);
    }

    #[tokio::test]
    async fn tick_without_control_only_records_decisions() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, POLICY_UNLESS_STOP, 1).await;
        set_state(&db, &id, "running", "running", "unhealthy").await;
        tick(
            &db,
            None,
            &AutoRestartConfig::default(),
            at("2026-10-21T12:01:00+08:00"),
        )
        .await
        .unwrap();
        assert_eq!(get_rt(&db, &id).await.phase, PHASE_STOPPING);
    }

    #[tokio::test]
    async fn tick_start_run_failure_backs_off_without_waiting_grace() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, POLICY_UNLESS_STOP, 3).await;
        seed_runtime(
            &db,
            &id,
            rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00"),
        )
        .await;
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        seed_failed_start_run(&db, &id, "2026-10-21T12:05:01+08:00").await;

        let control = MockControl::new();
        tick(
            &db,
            Some(&control),
            &AutoRestartConfig::default(),
            at("2026-10-21T12:05:10+08:00"),
        )
        .await
        .unwrap();
        let r = get_rt(&db, &id).await;
        assert_eq!(r.attempts, 2, "启动 run 失败应立即进入 backoff");
        assert_eq!(r.phase, PHASE_STOPPING);
        assert_eq!(control.stop_count(), 1);
    }

    #[tokio::test]
    async fn tick_start_dispatch_failure_goes_backoff() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, POLICY_UNLESS_STOP, 3).await;
        seed_runtime(
            &db,
            &id,
            rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:00:00+08:00"),
        )
        .await;
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        let control = MockControl::new();
        control.fail_start("temporal down");
        tick(
            &db,
            Some(&control),
            &AutoRestartConfig::default(),
            at("2026-10-21T12:05:00+08:00"),
        )
        .await
        .unwrap();
        let r = get_rt(&db, &id).await;
        assert_eq!(r.attempts, 2);
        assert_eq!(r.phase, PHASE_STOPPING);
        assert!(!r.next_at.is_empty());
        assert!(r.last_error.contains("下发启动失败"));
    }
}
