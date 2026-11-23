//! 探活失败自动重启：判定（纯函数）+ 调度（进程级）+ 运行态持久化
//!
//! 背景：健康巡检（[`crate::deploy::health`]）只把 `health-probe` 结果写入
//! `health_status`，不负责恢复。推理引擎崩溃 / 容器退出 / 掉电后业务无法自愈。本模块在健康巡检
//! **之后**、以同一周期运行，对开启 `auto_restart_enabled` 的运行配置执行：
//!
//! ```text
//! running + unhealthy 且连续失败 >= 阈值
//!        └─► 下发停止（清理节点侧副作用）
//!                 └─► state=stopped 且过 backoff 截止 ─► 下发启动
//!                          ├─ state=running  ─► 回 idle，计数清零
//!                          └─ 启动失败        ─► 再清理 + 指数 backoff → 重试（无限）
//! ```
//!
//! 关键约束（设计见 `specs/docs/design/modelinfer/auto-restart.md`）：
//!
//! - **纯函数决策**：[`decide`] 只输入运行态 + 现状 + 当前时间，输出「新运行态 + 动作」，
//!   便于穷举单测；动作执行走 [`crate::deploy::control::Control`]，与手动启停同源；
//! - **运行态持久化**（相位 / 失败起点 / next_at / 计数）：进程重启与整机掉电后自动续跑，
//!   不会把已累计的失败时长清零，也不会因为重启而重复等待一个完整阈值周期；
//! - **用户手动停止 / 启动立即取消循环**：由 HTTP handler 调用 [`reset`]（调度器自身不调用）。
//!
//! # 示例
//!
//! ```no_run
//! use chrono::FixedOffset;
//! use modelinfer::deploy::autorestart::{AutoRestartConfig, tick};
//!
//! # async fn demo(db: sea_orm::DatabaseConnection) {
//! let cfg = AutoRestartConfig::default();
//! let now = modelinfer::time::now();
//! // control = None：仅记录判定结果，不下发工作流（Temporal 不可用场景）
//! let _ = tick(&db, None, &cfg, now).await;
//! # let _ = FixedOffset::east_opt(8 * 3600);
//! # }
//! ```

use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::Serialize;

use crate::deploy::control::{Control, TRIGGER_AUTO_RESTART};
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
    ///
    /// # 示例
    ///
    /// ```
    /// use modelinfer::deploy::autorestart::BackoffPolicy;
    /// let p = BackoffPolicy { base_secs: 15, max_secs: 300 };
    /// assert_eq!(p.backoff(1), 15);
    /// assert_eq!(p.backoff(2), 30);
    /// assert_eq!(p.backoff(5), 240);
    /// assert_eq!(p.backoff(6), 300);
    /// assert_eq!(p.backoff(20), 300);
    /// ```
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
    /// 本轮连续失败起点（CST ISO-8601）
    pub failed_since: String,
    /// backoff 截止时间（CST ISO-8601）
    pub next_at: String,
    /// 当前循环内已下发的启动次数
    pub attempts: i32,
    /// 最近一次动作下发时间（CST ISO-8601）
    pub last_at: String,
    /// 最近一次失败原因
    pub last_error: String,
}

/// 决策输入（纯数据，便于单测穷举）
#[derive(Debug, Clone)]
pub struct AutoRestartInput<'a> {
    /// 是否开启自动重启
    pub enabled: bool,
    /// 连续失败阈值（秒）
    pub fail_secs: i64,
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
    /// `stopped`，与「已下发但尚未置位」无法从状态区分；该标记消除歧义，使失败可立即进入 backoff。
    pub start_run_failed: bool,
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

/// 纯函数决策：依据运行态与当前部署 / 健康状态给出「新运行态 + 动作」
///
/// 决策表见 `specs/docs/design/modelinfer/auto-restart.md` §3（D-MI-AR-06）。
pub fn decide(input: &AutoRestartInput<'_>, now: DateTime<FixedOffset>) -> Decision {
    let cur = &input.runtime;
    let now_epoch = now.timestamp();

    // 1. 关闭开关：运行态清零，不下发
    if !input.enabled {
        if *cur != RuntimeState::default() {
            return Decision {
                runtime: RuntimeState::default(),
                action: Action::None,
            };
        }
        return Decision::keep(cur);
    }

    let failed_since = time::parse_iso_epoch(&cur.failed_since);
    let next_at = time::parse_iso_epoch(&cur.next_at);
    let last_at = time::parse_iso_epoch(&cur.last_at);
    let elapsed_since_last = last_at.map(|t| now_epoch - t);

    match cur.phase.as_str() {
        // 2~4. idle：累计连续失败 / 达阈值触发 / 健康清零
        PHASE_IDLE => {
            if input.state == "running" && input.health_status == "unhealthy" {
                match failed_since {
                    None => Decision {
                        runtime: RuntimeState {
                            failed_since: now_iso(now),
                            ..cur.clone()
                        },
                        action: Action::None,
                    },
                    // 阈值到达 → 进入 stopping 并下发停止
                    Some(t) if now_epoch.saturating_sub(t) >= input.fail_secs.max(1) => Decision {
                        runtime: RuntimeState {
                            phase: PHASE_STOPPING.to_string(),
                            failed_since: cur.failed_since.clone(),
                            next_at: String::new(),
                            attempts: 1,
                            last_at: now_iso(now),
                            last_error: String::new(),
                        },
                        action: Action::Stop,
                    },
                    _ => Decision::keep(cur),
                }
            } else if input.state == "running" && input.health_status == "healthy" {
                if cur.failed_since.is_empty() {
                    Decision::keep(cur)
                } else {
                    Decision {
                        runtime: RuntimeState {
                            failed_since: String::new(),
                            ..cur.clone()
                        },
                        action: Action::None,
                    }
                }
            } else {
                Decision::keep(cur)
            }
        }
        // 5~7. stopping：等待停止完成 → 到点启动；卡死 / 未生效 → 重复下发停止
        PHASE_STOPPING => {
            if input.state == "stopped" && input.desired_state == "stopped" {
                let ready = next_at.map(|t| now_epoch >= t).unwrap_or(true);
                if !ready {
                    return Decision::keep(cur);
                }
                Decision {
                    runtime: RuntimeState {
                        phase: PHASE_STARTING.to_string(),
                        failed_since: cur.failed_since.clone(),
                        next_at: cur.next_at.clone(),
                        attempts: cur.attempts.max(1),
                        last_at: now_iso(now),
                        last_error: cur.last_error.clone(),
                    },
                    action: Action::Start,
                }
            } else if input.state == "stopping" || input.state == "failed" {
                let stuck = elapsed_since_last.map(|e| e >= STOP_STUCK_SECS).unwrap_or(true);
                if stuck {
                    Decision {
                        runtime: RuntimeState {
                            last_at: now_iso(now),
                            last_error: format!(
                                "停止清理未在 {STOP_STUCK_SECS} 秒内完成（state={}），重新下发停止",
                                input.state
                            ),
                            ..cur.clone()
                        },
                        action: Action::Stop,
                    }
                } else {
                    Decision::keep(cur)
                }
            } else if input.state == "running" {
                // 停止未生效（下发失败 / 尚未推进）：宽限后重复下发
                let stale = elapsed_since_last
                    .map(|e| e >= STOP_DISPATCH_GRACE_SECS)
                    .unwrap_or(true);
                if stale {
                    Decision {
                        runtime: RuntimeState {
                            last_at: now_iso(now),
                            last_error: "停止未生效，重新下发停止".to_string(),
                            ..cur.clone()
                        },
                        action: Action::Stop,
                    }
                } else {
                    Decision::keep(cur)
                }
            } else {
                Decision::keep(cur)
            }
        }
        // 8~9. starting：成功清零；失败进入 backoff 并清理
        PHASE_STARTING => {
            if input.state == "running" {
                Decision {
                    runtime: RuntimeState::default(),
                    action: Action::None,
                }
            } else if input.state == "failed" || input.state == "stopped" {
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
                Decision {
                    runtime: RuntimeState {
                        phase: PHASE_STOPPING.to_string(),
                        failed_since: cur.failed_since.clone(),
                        next_at: time::iso_after(now, wait),
                        attempts,
                        last_at: now_iso(now),
                        last_error: format!(
                            "第 {attempts} 次自动启动未成功（state={}），{wait} 秒后重试",
                            input.state
                        ),
                    },
                    action: Action::Stop,
                }
            } else {
                Decision::keep(cur)
            }
        }
        _ => Decision::keep(cur),
    }
}

/// 从实体加载运行态
pub fn runtime_of(m: &deployment::Model) -> RuntimeState {
    RuntimeState {
        phase: m.auto_restart_phase.clone(),
        failed_since: m.auto_restart_failed_since.clone(),
        next_at: m.auto_restart_next_at.clone(),
        attempts: m.auto_restart_attempts,
        last_at: m.auto_restart_last_at.clone(),
        last_error: m.auto_restart_last_error.clone(),
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
    am.auto_restart_next_at = Set(rt.next_at.clone());
    am.auto_restart_attempts = Set(rt.attempts);
    am.auto_restart_last_at = Set(rt.last_at.clone());
    am.auto_restart_last_error = Set(rt.last_error.clone());
    am.updated_at = Set(time::now_iso());
    am.update(db)
        .await
        .map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("更新自动重启状态失败: {e}")))?;
    Ok(())
}

/// 清零运行态（用户手动启动 / 停止时调用）
///
/// 语义：用户操作 = **取消自动重启循环**。调度器自身的动作 MUST NOT 调用（否则自我打断）。
pub async fn reset(db: &DatabaseConnection, deployment_id: &str) -> Result<(), ModelinferError> {
    save(db, deployment_id, &RuntimeState::default()).await
}

/// 单次调度报告（日志 / 测试用）
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct TickReport {
    /// 扫描到的开启自动重启的运行配置数
    pub scanned: usize,
    /// 实际下发停止的次数
    pub stops: usize,
    /// 实际下发启动的次数
    pub starts: usize,
    /// 决策 / 下发失败的次数
    pub errors: usize,
}

/// 本次自动启动下发的启动 run 是否已收尾为失败
///
/// 查询最近一条 `kind=start` 的 run：若其 `started_at` 不早于 `since`（本次下发时间）且
/// `state=failed`，则视为「本轮启动已明确失败」。用于区分「阶段脚本失败但 cleanup 后状态已回到
/// `stopped`」与「已下发但 workflow 尚未置位」。
async fn start_run_failed_since(
    db: &DatabaseConnection,
    deployment_id: &str,
    since: &str,
) -> Result<bool, ModelinferError> {
    let runs = crate::observability::store::list_runs(db, deployment_id, Some("start"), Some(1))
        .await?;
    let Some(latest) = runs.first() else {
        return Ok(false);
    };
    Ok(latest.started_at.as_str() >= since && latest.state == "failed")
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
        .filter(deployment::Column::AutoRestartEnabled.eq(true))
        .all(db)
        .await
        .map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("查询运行配置失败: {e}")))?;

    let mut report = TickReport::default();
    for m in rows {
        report.scanned += 1;
        let current = runtime_of(&m);
        let start_run_failed = current.phase == PHASE_STARTING
            && !current.last_at.is_empty()
            && start_run_failed_since(db, &m.id, &current.last_at)
                .await
                .unwrap_or(false);
        let input = AutoRestartInput {
            enabled: m.auto_restart_enabled,
            fail_secs: m.auto_restart_fail_secs as i64,
            runtime: current.clone(),
            state: &m.state,
            desired_state: &m.desired_state,
            health_status: &m.health_status,
            start_run_failed,
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
                let reason = format!(
                    "探活连续失败达到 {} 秒，自动重启：停止清理",
                    m.auto_restart_fail_secs
                );
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
        enabled: bool,
        fail_secs: i64,
        runtime: RuntimeState,
        state: &'a str,
        desired: &'a str,
        health: &'a str,
    ) -> AutoRestartInput<'a> {
        AutoRestartInput {
            enabled,
            fail_secs,
            runtime,
            state,
            desired_state: desired,
            health_status: health,
            start_run_failed: false,
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
        }
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let p = BackoffPolicy { base_secs: 15, max_secs: 300 };
        assert_eq!(p.backoff(1), 15);
        assert_eq!(p.backoff(2), 30);
        assert_eq!(p.backoff(3), 60);
        assert_eq!(p.backoff(4), 120);
        assert_eq!(p.backoff(5), 240);
        assert_eq!(p.backoff(6), 300);
        assert_eq!(p.backoff(1000), 300);
        // 非法 / 异常输入不 panic
        assert_eq!(p.backoff(0), 15);
        assert_eq!(p.backoff(-5), 15);
        let weird = BackoffPolicy { base_secs: 0, max_secs: 0 };
        assert_eq!(weird.backoff(3), 1, "base 归一到 1、max 归一到 base");
    }

    #[test]
    fn disabled_clears_runtime_and_never_acts() {
        let now = at("2026-10-21T12:00:00+08:00");
        let r = rt(PHASE_STARTING, "", "", 3, "2026-10-21T11:00:00+08:00");
        let d = decide(&input(false, 300, r, "running", "running", "unhealthy"), now);
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime, RuntimeState::default());

        // 已是默认态：保持（不产生无意义写入）
        let d = decide(&input(false, 300, RuntimeState::default(), "running", "running", "healthy"), now);
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime, RuntimeState::default());
    }

    #[test]
    fn idle_records_first_failure_without_restart() {
        let now = at("2026-10-21T12:00:00+08:00");
        let d = decide(
            &input(true, 300, RuntimeState::default(), "running", "running", "unhealthy"),
            now,
        );
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime.failed_since, "2026-10-21T12:00:00+08:00");
    }

    #[test]
    fn idle_triggers_stop_after_threshold() {
        let now = at("2026-10-21T12:05:00+08:00");
        let r = rt("", "2026-10-21T12:00:00+08:00", "", 0, "");
        let d = decide(&input(true, 300, r, "running", "running", "unhealthy"), now);
        assert_eq!(d.action, Action::Stop);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
        assert_eq!(d.runtime.attempts, 1);
        assert_eq!(d.runtime.last_at, "2026-10-21T12:05:00+08:00");
    }

    #[test]
    fn idle_waits_before_threshold() {
        let now = at("2026-10-21T12:04:59+08:00");
        let r = rt("", "2026-10-21T12:00:00+08:00", "", 0, "");
        let d = decide(&input(true, 300, r, "running", "running", "unhealthy"), now);
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime.phase, "");
    }

    #[test]
    fn healthy_clears_failure_timer() {
        let now = at("2026-10-21T12:01:00+08:00");
        let r = rt("", "2026-10-21T12:00:00+08:00", "", 0, "");
        let d = decide(&input(true, 300, r, "running", "running", "healthy"), now);
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime.failed_since, "");
    }

    #[test]
    fn stopping_launches_start_once_stopped_and_backoff_passed() {
        let now = at("2026-10-21T12:06:00+08:00");
        let r = rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let d = decide(&input(true, 300, r.clone(), "stopped", "stopped", "unknown"), now);
        assert_eq!(d.action, Action::Start);
        assert_eq!(d.runtime.phase, PHASE_STARTING);

        // next_at 未到 → 不启动
        let r2 = rt(PHASE_STOPPING, "", "2026-10-21T12:10:00+08:00", 2, "2026-10-21T12:05:00+08:00");
        let d = decide(&input(true, 300, r2, "stopped", "stopped", "unknown"), now);
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
    }

    #[test]
    fn stopping_does_not_start_while_still_stopping() {
        let now = at("2026-10-21T12:06:00+08:00");
        let r = rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:05:30+08:00");
        let d = decide(&input(true, 300, r, "stopping", "stopping", "unknown"), now);
        assert_eq!(d.action, Action::None);
    }

    #[test]
    fn stopping_retries_when_stuck_or_not_effective() {
        // 卡在 stopping 超时 → 重复下发停止
        let now = at("2026-10-21T12:30:00+08:00");
        let r = rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:00:00+08:00");
        let d = decide(&input(true, 300, r, "stopping", "stopping", "unknown"), now);
        assert_eq!(d.action, Action::Stop);
        assert!(d.runtime.last_error.contains("停止清理未在"));

        // 仍 running（停止未生效）且超过下发宽限 → 重复下发
        let r = rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:00:00+08:00");
        let d = decide(&input(true, 300, r, "running", "stopping", "unknown"), now);
        assert_eq!(d.action, Action::Stop);
    }

    #[test]
    fn starting_success_resets_everything() {
        let now = at("2026-10-21T12:07:00+08:00");
        let r = rt(PHASE_STARTING, "2026-10-21T12:00:00+08:00", "", 3, "2026-10-21T12:05:00+08:00");
        let d = decide(&input(true, 300, r, "running", "running", "healthy"), now);
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime, RuntimeState::default());
    }

    #[test]
    fn starting_failure_enters_backoff_with_growing_wait() {
        let now = at("2026-10-21T12:10:00+08:00");
        // 第 1 次启动失败 → attempts=2 → 等待 30s
        let r = rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let d = decide(&input(true, 300, r, "failed", "stopped", "unknown"), now);
        assert_eq!(d.action, Action::Stop);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
        assert_eq!(d.runtime.attempts, 2);
        assert_eq!(d.runtime.next_at, "2026-10-21T12:10:30+08:00");
        assert!(d.runtime.last_error.contains("第 2 次"));

        // 第 5 次失败 → 等待 240s
        let r = rt(PHASE_STARTING, "", "", 4, "2026-10-21T12:05:00+08:00");
        let d = decide(&input(true, 300, r, "failed", "stopped", "unknown"), now);
        assert_eq!(d.runtime.attempts, 5);
        assert_eq!(d.runtime.next_at, "2026-10-21T12:14:00+08:00");
    }

    #[test]
    fn starting_failed_state_is_judged_immediately() {
        let now = at("2026-10-21T12:05:30+08:00");
        // workflow 已明确失败：即使距下发只有 30s，也立即进入 backoff（不必等满 180s 宽限）
        let r = rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let d = decide(&input(true, 300, r, "failed", "stopped", "unknown"), now);
        assert_eq!(d.action, Action::Stop);
        assert_eq!(d.runtime.attempts, 2);
        assert_eq!(d.runtime.next_at, "2026-10-21T12:06:00+08:00");
    }

    #[test]
    fn starting_failed_run_judged_immediately_even_when_stopped() {
        let now = at("2026-10-21T12:05:30+08:00");
        let r = rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let mut inp = input(true, 300, r, "stopped", "stopped", "unknown");
        inp.start_run_failed = true;
        let d = decide(&inp, now);
        assert_eq!(d.action, Action::Stop);
        assert_eq!(d.runtime.attempts, 2);
        assert_eq!(d.runtime.phase, PHASE_STOPPING);
    }

    #[test]
    fn starting_failure_within_grace_is_not_judged() {
        let now = at("2026-10-21T12:06:00+08:00");
        // 刚下发启动 60s（< 180s 宽限），状态尚未推进到 starting
        let r = rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let d = decide(&input(true, 300, r, "stopped", "stopped", "unknown"), now);
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime.phase, PHASE_STARTING);
    }

    #[test]
    fn starting_in_progress_is_idle() {
        let now = at("2026-10-21T12:06:00+08:00");
        let r = rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00");
        let d = decide(&input(true, 300, r, "starting", "running", "unknown"), now);
        assert_eq!(d.action, Action::None);
    }

    #[test]
    fn unknown_phase_is_treated_as_idle_but_preserved() {
        let now = at("2026-10-21T12:06:00+08:00");
        let r = rt("weird", "", "", 0, "");
        let d = decide(&input(true, 300, r.clone(), "running", "running", "unhealthy"), now);
        assert_eq!(d.action, Action::None);
        assert_eq!(d.runtime, r, "未知相位不改写运行态（人工介入排障）");
    }

    // ---- tick 集成（sqlite + MockControl） ----

    async fn seed_deployment(db: &DatabaseConnection, enabled: bool, fail_secs: i32) -> String {
        let now = time::now_iso();
        let manifest = serde_json::json!({
            "id": "com.ictrek.ar-test",
            "version": "1.0.0",
            "model": "org/w",
            "lifecycle": { "start": "./s.sh", "health-probe": "./hp.sh" },
        });
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
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();

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
        platform.device = DeviceInfo { ip_addr: vec!["10.0.0.5".into()] };

        let node = nodes::create_node(
            db,
            &MockSshExec::new().with_rule("hostname", 0, "h1"),
            crate::nodes::NodeCreate {
                profile: None,
                name: "n1".into(),
                host: "10.0.0.1".into(),
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
                auto_restart_enabled: enabled,
                auto_restart_fail_secs: Some(fail_secs as i64),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id;
        // 保持临时目录存活（resolved 路径已落库）
        std::mem::forget(tmp);
        id
    }

    async fn set_state(db: &DatabaseConnection, id: &str, state: &str, desired: &str, health: &str) {
        let m = deployment::Entity::find_by_id(id.to_string()).one(db).await.unwrap().unwrap();
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
        let m = deployment::Entity::find_by_id(id.to_string()).one(db).await.unwrap().unwrap();
        runtime_of(&m)
    }

    #[tokio::test]
    async fn tick_full_restart_cycle() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, true, 60).await;
        let cfg = AutoRestartConfig::default();
        let control = MockControl::new();
        let t0 = at("2026-10-21T12:00:00+08:00");

        // running + unhealthy → 记录失败起点（不重启）
        set_state(&db, &id, "running", "running", "unhealthy").await;
        tick(&db, Some(&control), &cfg, t0).await.unwrap();
        assert_eq!(control.calls().len(), 0);
        assert_eq!(get_rt(&db, &id).await.failed_since, "2026-10-21T12:00:00+08:00");

        // 60s 后仍失败 → 下发停止
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:01:00+08:00")).await.unwrap();
        assert_eq!(control.stop_count(), 1);
        assert_eq!(get_rt(&db, &id).await.phase, PHASE_STOPPING);

        // 停止完成 → 下发启动
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:01:10+08:00")).await.unwrap();
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
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:02:00+08:00")).await.unwrap();
        assert_eq!(get_rt(&db, &id).await, RuntimeState::default());
        assert_eq!(control.stop_count(), 1);
        assert_eq!(control.start_count(), 1);
    }

    #[tokio::test]
    async fn tick_start_failure_grows_backoff_and_retries() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, true, 30).await;
        let cfg = AutoRestartConfig::default();
        let control = MockControl::new();

        // 已在重启循环中，启动失败一次
        seed_runtime(
            &db,
            &id,
            rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:00:00+08:00"),
        )
        .await;
        set_state(&db, &id, "failed", "stopped", "unknown").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:05:00+08:00")).await.unwrap();
        let r = get_rt(&db, &id).await;
        assert_eq!(control.stop_count(), 1);
        assert_eq!(r.attempts, 2);
        assert_eq!(r.phase, PHASE_STOPPING);
        assert_eq!(r.next_at, "2026-10-21T12:05:30+08:00");

        // 停止完成但 backoff 未到 → 不下发
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:05:10+08:00")).await.unwrap();
        assert_eq!(control.start_count(), 0);

        // 到点 → 下发启动
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:05:31+08:00")).await.unwrap();
        assert_eq!(control.start_count(), 1);

        // 再次失败（第 2 次）→ attempts=3、等待 60s
        set_state(&db, &id, "failed", "stopped", "unknown").await;
        tick(&db, Some(&control), &cfg, at("2026-10-21T12:10:00+08:00")).await.unwrap();
        let r = get_rt(&db, &id).await;
        assert_eq!(r.attempts, 3);
        assert_eq!(r.next_at, "2026-10-21T12:11:00+08:00");
    }

    #[tokio::test]
    async fn tick_skips_disabled_and_resets_stale_runtime() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, false, 300).await;
        set_state(&db, &id, "running", "running", "unhealthy").await;
        let control = MockControl::new();
        let report = tick(&db, Some(&control), &AutoRestartConfig::default(), time::now())
            .await
            .unwrap();
        assert_eq!(report.scanned, 0, "未开启自动重启的运行配置不参与调度");
        assert_eq!(control.calls().len(), 0);
    }

    #[tokio::test]
    async fn reset_cancels_loop() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, true, 30).await;
        seed_runtime(
            &db,
            &id,
            rt(PHASE_STOPPING, "2026-10-21T12:00:00+08:00", "", 3, "2026-10-21T12:04:00+08:00"),
        )
        .await;
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        reset(&db, &id).await.unwrap();
        let control = MockControl::new();
        tick(&db, Some(&control), &AutoRestartConfig::default(), at("2026-10-21T12:06:00+08:00"))
            .await
            .unwrap();
        assert_eq!(control.calls().len(), 0, "reset 后 MUST NOT 再下发启动");
        assert_eq!(get_rt(&db, &id).await.phase, "");
    }

    #[tokio::test]
    async fn tick_without_control_only_records_decisions() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, true, 30).await;
        seed_runtime(&db, &id, rt("", "2026-10-21T12:00:00+08:00", "", 0, "")).await;
        set_state(&db, &id, "running", "running", "unhealthy").await;
        tick(&db, None, &AutoRestartConfig::default(), at("2026-10-21T12:01:00+08:00"))
            .await
            .unwrap();
        // 相位推进为 stopping（决策已持久化），但没有任何动作可下发
        assert_eq!(get_rt(&db, &id).await.phase, PHASE_STOPPING);
    }

    #[tokio::test]
    async fn tick_start_run_failure_backs_off_without_waiting_grace() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, true, 30).await;
        // 已下发启动（last_at 刚过 10s），启动 run 已收尾失败
        seed_runtime(&db, &id, rt(PHASE_STARTING, "", "", 1, "2026-10-21T12:05:00+08:00")).await;
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        let run_id = crate::observability::store::create_run(
            &db,
            &id,
            crate::observability::kind::START,
            crate::deploy::control::TRIGGER_AUTO_RESTART,
            "自动重启：开始启动",
            &crate::deploy::control::start_workflow_id(&id),
        )
        .await
        .unwrap();
        let _ = run_id;
        crate::observability::finish_and_prune(
            &db,
            &id,
            crate::observability::SettingsDefaults::default(),
            &crate::observability::store::list_runs(&db, &id, Some("start"), Some(1))
                .await
                .unwrap()[0]
                .id,
            crate::observability::state::FAILED,
            "阶段 start 执行失败",
            "阶段 start 执行失败",
        )
        .await
        .unwrap();

        let control = MockControl::new();
        tick(&db, Some(&control), &AutoRestartConfig::default(), at("2026-10-21T12:05:10+08:00"))
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
        let id = seed_deployment(&db, true, 30).await;
        seed_runtime(&db, &id, rt(PHASE_STOPPING, "", "", 1, "2026-10-21T12:00:00+08:00")).await;
        set_state(&db, &id, "stopped", "stopped", "unknown").await;
        let control = MockControl::new();
        control.fail_start("temporal down");
        tick(&db, Some(&control), &AutoRestartConfig::default(), at("2026-10-21T12:05:00+08:00"))
            .await
            .unwrap();
        let r = get_rt(&db, &id).await;
        assert_eq!(r.attempts, 2);
        assert_eq!(r.phase, PHASE_STOPPING);
        assert!(!r.next_at.is_empty());
        assert!(r.last_error.contains("下发启动失败"));
    }
}