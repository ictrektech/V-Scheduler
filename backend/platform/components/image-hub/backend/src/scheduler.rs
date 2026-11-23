//! registry 垃圾回收（GC）自动调度器（D-IH-62）。
//!
//! - 后台任务按固定 tick（默认 60s）检查自动回收是否到期；
//! - 到期且**无运行中任务**时触发一次自动 GC（`delete_untagged` 取配置值）并更新 `last_run_at`；
//! - 存在运行中任务时跳过本轮（不推进计时基准），待后续检查；
//! - 到期判定与下次时间均为纯函数（`gc::auto_gc_due` / `gc::compute_next_run_at`），
//!   调度器自身仅负责 I/O 与触发，`tick_once_at` 可注入「当前时刻」供单测。

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::gc::{DEFAULT_GC_TIMEOUT_MINUTES, GcRequest, auto_gc_due};
use crate::state::AppState;

/// 调度检查间隔默认值（秒）
pub const DEFAULT_GC_TICK_SECS: u64 = 60;

/// GC 自动回收调度器
pub struct GcScheduler {
    state: AppState,
    tick: Duration,
    /// 进程启动时刻：`last_run_at` 缺失时的自动回收计时基准
    started_at: DateTime<Utc>,
}

impl GcScheduler {
    pub fn new(state: AppState) -> Self {
        Self::with_tick(state, Duration::from_secs(DEFAULT_GC_TICK_SECS))
    }

    /// 指定 tick（单测可缩短）
    pub fn with_tick(state: AppState, tick: Duration) -> Self {
        Self {
            state,
            tick,
            started_at: Utc::now(),
        }
    }

    /// 覆盖计时基准（单测注入）
    #[must_use]
    pub fn with_started_at(mut self, started_at: DateTime<Utc>) -> Self {
        self.started_at = started_at;
        self
    }

    /// 启动后台循环（在 `main` 中以 `tokio::spawn` 运行）
    pub async fn run(self) {
        let mut interval = tokio::time::interval(self.tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            self.tick_once().await;
        }
    }

    /// 一次调度检查（使用当前时刻）
    pub async fn tick_once(&self) -> Option<String> {
        self.tick_once_at(Utc::now()).await
    }

    /// 一次调度检查（注入「当前时刻」，便于单测）
    ///
    /// 返回本次自动触发的任务 id（未触发为 `None`）。
    pub async fn tick_once_at(&self, now: DateTime<Utc>) -> Option<String> {
        let mut settings = match self.state.settings.load() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(err = %e.msg, "读取设置失败，跳过自动 GC 检查");
                return None;
            }
        };
        let gc = settings.gc.clone();
        if !auto_gc_due(
            gc.enabled,
            gc.last_run_at.as_deref(),
            self.started_at,
            gc.interval_hours,
            now,
        ) {
            return None;
        }
        if self.state.jobs.has_running() {
            tracing::debug!("自动 GC 到期，但有任务正在执行，跳过本轮");
            return None;
        }
        let request = GcRequest {
            delete_untagged: gc.delete_untagged,
            dry_run: false,
            timeout: Duration::from_secs(DEFAULT_GC_TIMEOUT_MINUTES * 60),
        };
        match self.state.jobs.start_gc(request) {
            Ok(id) => {
                settings.gc.last_run_at = Some(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
                if let Err(e) = self.state.settings.save(&settings) {
                    tracing::warn!(err = %e.msg, "更新 GC last_run_at 失败");
                }
                tracing::info!(job_id = %id, "已触发自动垃圾回收");
                Some(id)
            }
            Err(e) => {
                tracing::warn!(err = %e.msg, "自动垃圾回收触发失败");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use chrono::TimeZone;

    use crate::gc::MockRegistryGcRunner;
    use crate::platform::MockPlatformClient;
    use crate::registry::MockRegistryClient;
    use crate::settings::{GcSettings, MemorySettingsStore, Settings, SettingsStore};

    fn state_with_gc(gc: GcSettings, available: bool) -> (AppState, Arc<MemorySettingsStore>) {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        let frontend = Box::leak(Box::new(tmp)).path().join("dist");

        let store = Arc::new(MemorySettingsStore::with(Settings {
            gc,
            ..Default::default()
        }));
        let runner = Arc::new(if available {
            MockRegistryGcRunner::new()
        } else {
            MockRegistryGcRunner::unavailable()
        });
        let state = AppState::for_test_with_gc(
            frontend,
            Arc::new(MockRegistryClient::new()),
            Arc::new(MockPlatformClient::new()),
            store.clone(),
            runner,
        );
        (state, store)
    }

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
    }

    #[tokio::test]
    async fn disabled_never_triggers() {
        let (state, _store) = state_with_gc(
            GcSettings {
                enabled: false,
                interval_hours: 1,
                ..Default::default()
            },
            true,
        );
        let scheduler = GcScheduler::with_tick(state, Duration::from_millis(10));
        assert_eq!(scheduler.tick_once_at(at(2026, 9, 25, 0)).await, None);
    }

    #[tokio::test]
    async fn due_triggers_and_updates_last_run() {
        let (state, store) = state_with_gc(
            GcSettings {
                enabled: true,
                interval_hours: 1,
                delete_untagged: false,
                last_run_at: Some("2026-09-24T00:00:00Z".into()),
            },
            true,
        );
        let scheduler = GcScheduler::with_tick(state.clone(), Duration::from_millis(10));
        // 距 last_run_at 不足 1 小时 → 不触发
        assert_eq!(scheduler.tick_once_at(at(2026, 9, 24, 0)).await, None);
        // 到期 → 触发
        let id = scheduler.tick_once_at(at(2026, 9, 24, 2)).await;
        assert!(id.as_deref().unwrap_or_default().starts_with("gc-"), "{id:?}");
        let saved = store.load().unwrap();
        assert_eq!(saved.gc.last_run_at.as_deref(), Some("2026-09-24T02:00:00Z"));
        // 触发后再次检查不到期
        assert_eq!(scheduler.tick_once_at(at(2026, 9, 24, 2)).await, None);
    }

    #[tokio::test]
    async fn busy_skips_without_advancing_baseline() {
        let (state, store) = state_with_gc(
            GcSettings {
                enabled: true,
                interval_hours: 1,
                last_run_at: Some("2026-09-24T00:00:00Z".into()),
                ..Default::default()
            },
            true,
        );
        // 制造一个运行中任务（mock skopeo 延迟）
        let image = crate::skopeo::parse_image_ref("docker.io/library/ubuntu:22.04").unwrap();
        let _id = state.jobs.start_pull(
            image,
            crate::skopeo::ArchMode::All,
            true,
            None,
            Duration::from_secs(60),
            crate::skopeo::DEFAULT_RETRY_TIMES,
        );
        assert!(state.jobs.has_running());

        let scheduler = GcScheduler::with_tick(state.clone(), Duration::from_millis(10));
        assert_eq!(scheduler.tick_once_at(at(2026, 9, 24, 5)).await, None);
        // 基准未被推进
        let saved = store.load().unwrap();
        assert_eq!(saved.gc.last_run_at.as_deref(), Some("2026-09-24T00:00:00Z"));
    }

    #[tokio::test]
    async fn unavailable_runner_does_not_trigger() {
        let (state, store) = state_with_gc(
            GcSettings {
                enabled: true,
                interval_hours: 1,
                last_run_at: Some("2026-09-24T00:00:00Z".into()),
                ..Default::default()
            },
            false,
        );
        let scheduler = GcScheduler::with_tick(state, Duration::from_millis(10));
        assert_eq!(scheduler.tick_once_at(at(2026, 9, 24, 5)).await, None);
        // 能力不可用时不推进基准（每轮都会重试）
        assert_eq!(
            store.load().unwrap().gc.last_run_at.as_deref(),
            Some("2026-09-24T00:00:00Z")
        );
    }
}