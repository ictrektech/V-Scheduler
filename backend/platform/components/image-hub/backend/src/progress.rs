//! 任务进度域：skopeo 输出行解析、结构化进度与百分比推导。
//!
//! 设计要点（D-IH-37）：
//! - skopeo 1.24.1 **无** `--progress`（T013），无字节级进度可用；进度只能由**文本输出行**
//!   解析出「阶段 + 计数」；
//! - 百分比**只在可推导时**给出（多标签 / 多子镜像 `(n/m)`），否则为 `None`，由前端展示不定量
//!   进度条——不伪造精确度；
//! - 全部为纯函数 / 纯状态更新，便于单测（不依赖真实 skopeo 与网络）。

use serde::{Deserialize, Serialize};

/// 进度阶段常量
pub mod phase {
    /// 已登记、等待执行
    pub const QUEUED: &str = "queued";
    /// 上游登录
    pub const LOGIN: &str = "login";
    /// 获取清单 / 签名
    pub const PREPARING: &str = "preparing";
    /// 复制镜像层
    pub const COPYING: &str = "copying";
    /// 写入 manifest / 签名
    pub const WRITING: &str = "writing";
    /// registry 垃圾回收进行中（D-IH-64）
    pub const GC: &str = "gc";
    pub const DONE: &str = "done";
    pub const CANCELLED: &str = "cancelled";
    pub const FAILED: &str = "failed";
}

/// 进入日志与进度摘要的单行长度上限（字符数，超出截断）
pub const MAX_LINE_CHARS: usize = 300;

/// 清洗一行输出：按 `\r` 取最后一段（回车型进度输出不会堆成一行，见 T014）、
/// 去首尾空白、按字符边界截断超长行。
pub fn clean_line(raw: &str) -> String {
    let last = raw.rsplit('\r').next().unwrap_or(raw);
    let trimmed = last.trim();
    if trimmed.chars().count() <= MAX_LINE_CHARS {
        return trimmed.to_string();
    }
    let mut truncated: String = trimmed.chars().take(MAX_LINE_CHARS).collect();
    truncated.push('…');
    truncated
}

/// skopeo（或本应用注入）输出行对应的事件
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkopeoEvent {
    /// 本应用：已登录上游 registry `<host>`
    Login { host: String },
    /// `Getting image source signatures`
    GetSignatures,
    /// `Copying image sha256:… (n/m)` —— 多架构逐个子镜像
    CopyImage { current: u64, total: u64 },
    /// `Copying blob sha256:…`
    CopyBlob { digest: String },
    /// `Copying config sha256:…`
    CopyConfig { digest: String },
    /// `Writing manifest to image destination`
    WriteManifest,
    /// `Storing signatures`
    StoringSignatures,
    /// 本应用：第 N/M 次尝试开始（`retry_times > 0` 时）
    Attempt { attempt: u64, max: u64 },
    /// 本应用：第 N/M 次尝试失败，K 秒后重试（D-IH-45）
    RetryWait {
        attempt: u64,
        max: u64,
        delay_secs: u64,
    },
    /// 本应用：已推送 `<source>` → `<target>`
    Pushed { source: String, target: String },
    /// 本应用：任务完成
    Done,
    /// 本应用：任务失败
    Failed,
    /// 其它（无法识别）——`progress` 回落为原始行
    Other,
}

/// 解析一行输出为事件（尽力而为：无法识别统一返回 `Other`）
pub fn parse_skopeo_line(line: &str) -> SkopeoEvent {
    let line = line.trim();
    if line.is_empty() {
        return SkopeoEvent::Other;
    }
    if let Some(rest) = line.strip_prefix("已登录上游 registry ") {
        return SkopeoEvent::Login {
            host: rest.trim().to_string(),
        };
    }
    if let Some(rest) = line.strip_prefix("已推送 ") {
        if let Some((source, target)) = rest.split_once(" → ") {
            return SkopeoEvent::Pushed {
                source: source.trim().to_string(),
                target: target.trim().to_string(),
            };
        }
        return SkopeoEvent::Other;
    }
    if line == "任务完成" {
        return SkopeoEvent::Done;
    }
    if line.starts_with("任务失败") {
        return SkopeoEvent::Failed;
    }
    if let Some((attempt, max)) = parse_attempt_line(line) {
        if let Some((_, tail)) = line.split_once(" 次尝试失败，") {
            // `第 N/M 次尝试失败，K 秒后重试：原因`
            let delay_secs = tail
                .split_once(" 秒后重试")
                .and_then(|(num, _)| num.trim().parse::<u64>().ok())
                .unwrap_or(0);
            return SkopeoEvent::RetryWait {
                attempt,
                max,
                delay_secs,
            };
        }
        return SkopeoEvent::Attempt { attempt, max };
    }
    if line == "Getting image source signatures" {
        return SkopeoEvent::GetSignatures;
    }
    if let Some(rest) = line.strip_prefix("Copying image ") {
        if let Some(event) = parse_copy_image_counter(rest) {
            return event;
        }
        return SkopeoEvent::Other;
    }
    if let Some(rest) = line.strip_prefix("Copying blob ") {
        return SkopeoEvent::CopyBlob {
            digest: rest.trim().to_string(),
        };
    }
    if let Some(rest) = line.strip_prefix("Copying config ") {
        return SkopeoEvent::CopyConfig {
            digest: rest.trim().to_string(),
        };
    }
    if line == "Writing manifest to image destination" {
        return SkopeoEvent::WriteManifest;
    }
    if line == "Storing signatures" {
        return SkopeoEvent::StoringSignatures;
    }
    SkopeoEvent::Other
}

/// 解析本应用注入的尝试行 `第 N/M 次尝试[…]` → `(N, M)`（D-IH-47）
pub fn parse_attempt_line(line: &str) -> Option<(u64, u64)> {
    let rest = line.strip_prefix("第 ")?;
    let (nums, tail) = rest.split_once(" 次尝试")?;
    // 仅接受「空尾」或「失败…」两种（避免误吞其它文案）
    if !tail.is_empty() && !tail.starts_with("失败") {
        return None;
    }
    let (attempt, max) = nums.split_once('/')?;
    let attempt: u64 = attempt.trim().parse().ok()?;
    let max: u64 = max.trim().parse().ok()?;
    if max == 0 {
        return None;
    }
    Some((attempt, max))
}

/// 解析 `sha256:… (n/m)` 尾部的计数
fn parse_copy_image_counter(rest: &str) -> Option<SkopeoEvent> {
    let open = rest.rfind('(')?;
    let close = rest.rfind(')')?;
    if close < open {
        return None;
    }
    let inner = rest[open + 1..close].trim();
    let (current, total) = inner.split_once('/')?;
    let current: u64 = current.trim().parse().ok()?;
    let total: u64 = total.trim().parse().ok()?;
    if total == 0 {
        return None;
    }
    Some(SkopeoEvent::CopyImage { current, total })
}

/// 事件所属阶段（`None` 表示不改变当前阶段）
pub fn event_phase(event: &SkopeoEvent) -> Option<&'static str> {
    match event {
        SkopeoEvent::Login { .. } => Some(phase::LOGIN),
        SkopeoEvent::GetSignatures => Some(phase::PREPARING),
        SkopeoEvent::CopyImage { .. } | SkopeoEvent::CopyBlob { .. } => Some(phase::COPYING),
        SkopeoEvent::CopyConfig { .. }
        | SkopeoEvent::WriteManifest
        | SkopeoEvent::StoringSignatures => Some(phase::WRITING),
        SkopeoEvent::Done => Some(phase::DONE),
        SkopeoEvent::Failed => Some(phase::FAILED),
        // 尝试 / 退避不改变阶段（后续 skopeo 事件会推进阶段）
        SkopeoEvent::Attempt { .. } | SkopeoEvent::RetryWait { .. } => None,
        SkopeoEvent::Pushed { .. } | SkopeoEvent::Other => None,
    }
}

/// 进度统计上下文（由任务执行体提供：当前第几个标签、已完成标签数、总标签数）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgressContext {
    /// 当前正在传输的标签序号（1 起）
    pub item_index: u64,
    /// 已完成的标签数
    pub items_done: u64,
    /// 本次任务的总标签数
    pub items_total: u64,
}

/// 结构化进度（任务记录对外暴露）
///
/// `#[serde(default)]`：旧任务记录（无该字段 / 缺子字段）可正常加载。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgressDetail {
    /// 阶段（见 `phase` 常量）
    pub phase: String,
    /// 友好中文摘要
    pub message: String,
    /// 百分比（0..=100）；**不可推导时为 `None`**
    pub percent: Option<u8>,
    /// 已完成的标签数
    pub items_done: u64,
    /// 总标签数
    pub items_total: u64,
    /// 当前尝试序号（1 起；重试场景下由「尝试」事件更新）
    pub attempt: u64,
    /// 最大尝试次数（= 1 + 重试次数）
    pub max_attempts: u64,
    /// skopeo `Copying image (n/m)` 的当前序号
    pub sub_current: Option<u64>,
    /// skopeo `Copying image (n/m)` 的总数
    pub sub_total: Option<u64>,
    /// 当前任务已复制镜像层数
    pub layers_done: u64,
    /// 重试退避剩余秒数（处于退避等待时为 `Some`；其余阶段为 `None`）
    ///
    /// 前端据此本地化「N 秒后重试」；旧记录缺字段时按 `None` 加载。
    #[serde(default)]
    pub retry_in_secs: Option<u64>,
}

impl Default for ProgressDetail {
    fn default() -> Self {
        Self {
            phase: phase::QUEUED.to_string(),
            message: "等待执行".to_string(),
            percent: None,
            items_done: 0,
            items_total: 1,
            attempt: 0,
            max_attempts: 1,
            sub_current: None,
            sub_total: None,
            layers_done: 0,
            retry_in_secs: None,
        }
    }
}

impl ProgressDetail {
    /// 新建（登记任务时）：`items_total` 为本次任务的标签数
    pub fn queued(items_total: u64) -> Self {
        let total = items_total.max(1);
        Self {
            items_total: total,
            message: if total > 1 {
                format!("等待执行（共 {total} 个标签）")
            } else {
                "等待执行".to_string()
            },
            ..Default::default()
        }
    }

    /// 应用一个事件 + 上下文，更新阶段 / 计数 / 摘要 / 百分比
    pub fn apply(&mut self, event: &SkopeoEvent, ctx: &ProgressContext) {
        if let Some(p) = event_phase(event) {
            self.phase = p.to_string();
        }
        self.items_total = ctx.items_total.max(1);
        self.items_done = ctx.items_done.min(self.items_total);

        // 仅退避等待阶段携带剩余秒数；其余事件清除
        self.retry_in_secs = match event {
            SkopeoEvent::RetryWait { delay_secs, .. } => Some(*delay_secs),
            _ => None,
        };

        match event {
            SkopeoEvent::Attempt { attempt, max } | SkopeoEvent::RetryWait { attempt, max, .. } => {
                self.attempt = *attempt;
                self.max_attempts = (*max).max(1);
            }
            SkopeoEvent::CopyImage { current, total } => {
                self.sub_current = Some(*current);
                self.sub_total = Some(*total);
            }
            SkopeoEvent::CopyBlob { .. } | SkopeoEvent::CopyConfig { .. } => {
                self.layers_done += 1;
            }
            _ => {}
        }

        if let Some(message) = event_message(event, ctx, self.layers_done) {
            self.message = message;
        }
        self.percent = compute_percent(
            &self.phase,
            self.items_done,
            self.items_total,
            self.sub_current,
            self.sub_total,
        );
    }

    /// 终态落定（done / cancelled / failed）
    pub fn mark_terminal(&mut self, terminal_phase: &str, message: impl Into<String>) {
        self.phase = terminal_phase.to_string();
        self.message = message.into();
        self.retry_in_secs = None;
        self.percent = match terminal_phase {
            phase::DONE => Some(100),
            _ => None,
        };
        if terminal_phase == phase::DONE {
            self.items_done = self.items_total;
        }
    }
}

/// 生成事件对应的中文摘要（`None` 表示保持原摘要不变）
fn event_message(event: &SkopeoEvent, ctx: &ProgressContext, layers_done: u64) -> Option<String> {
    let suffix = if ctx.items_total > 1 {
        format!(
            " —— 第 {}/{} 个标签",
            ctx.item_index.max(1),
            ctx.items_total
        )
    } else {
        String::new()
    };
    let message = match event {
        SkopeoEvent::Login { host } => format!("正在登录上游 registry {host}{suffix}"),
        SkopeoEvent::GetSignatures => format!("正在获取镜像清单{suffix}"),
        SkopeoEvent::CopyImage { current, total } => format!(
            "正在复制镜像 —— 第 {current}/{total} 个子镜像，已复制 {layers_done} 层{suffix}"
        ),
        SkopeoEvent::CopyBlob { .. } | SkopeoEvent::CopyConfig { .. } => {
            format!("正在复制镜像层（已复制 {layers_done} 层）{suffix}")
        }
        SkopeoEvent::WriteManifest => format!("正在写入 manifest{suffix}"),
        SkopeoEvent::StoringSignatures => format!("正在写入签名{suffix}"),
        SkopeoEvent::Attempt { attempt, max } => format!("第 {attempt}/{max} 次尝试{suffix}"),
        SkopeoEvent::RetryWait {
            attempt,
            max,
            delay_secs,
        } => format!("第 {attempt}/{max} 次尝试失败，{delay_secs} 秒后重试{suffix}"),
        SkopeoEvent::Pushed { source, target } => format!("已推送 {source} → {target}"),
        SkopeoEvent::Done => "任务完成".to_string(),
        SkopeoEvent::Failed => "任务失败".to_string(),
        SkopeoEvent::Other => return None,
    };
    Some(message)
}

/// 百分比推导（D-IH-37）：仅在可推导时返回 `Some`，绝不伪造
pub fn compute_percent(
    phase_name: &str,
    items_done: u64,
    items_total: u64,
    sub_current: Option<u64>,
    sub_total: Option<u64>,
) -> Option<u8> {
    let total = items_total.max(1) as f64;
    match phase_name {
        phase::DONE => Some(100),
        phase::WRITING => {
            // 写 manifest 即当前标签收尾：按「已完成 + 当前」比例给 0..=99
            Some(ratio_to_percent((items_done as f64 + 1.0) / total, 99))
        }
        phase::COPYING => {
            let sub_ratio = match (sub_current, sub_total) {
                (Some(current), Some(sub)) if sub > 1 => (current.min(sub)) as f64 / sub as f64,
                _ => 0.0,
            };
            let derivable = items_total > 1 || sub_total.is_some_and(|s| s > 1);
            if !derivable {
                return None;
            }
            let ratio = if items_total > 1 {
                (items_done as f64 + sub_ratio) / total
            } else {
                sub_ratio
            };
            Some(ratio_to_percent(ratio, 99))
        }
        _ => None,
    }
}

/// 比例 → 百分比（截断到 `[0, cap]`）
fn ratio_to_percent(ratio: f64, cap: u8) -> u8 {
    let value = (ratio * 100.0).floor();
    value.clamp(0.0, cap as f64) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(item_index: u64, items_done: u64, items_total: u64) -> ProgressContext {
        ProgressContext {
            item_index,
            items_done,
            items_total,
        }
    }

    #[test]
    fn clean_line_handles_carriage_return_and_truncation() {
        assert_eq!(clean_line("  hello  "), "hello");
        // 回车型输出只保留最后一段（T014）
        assert_eq!(clean_line("10%\r50%\r100% done"), "100% done");
        assert_eq!(clean_line(""), "");

        let long = "x".repeat(MAX_LINE_CHARS + 50);
        let cleaned = clean_line(&long);
        assert_eq!(cleaned.chars().count(), MAX_LINE_CHARS + 1);
        assert!(cleaned.ends_with('…'));
    }

    #[test]
    fn parse_real_skopeo_lines() {
        assert_eq!(
            parse_skopeo_line("Getting image source signatures"),
            SkopeoEvent::GetSignatures
        );
        assert_eq!(
            parse_skopeo_line(
                "Copying image sha256:9675ab89f70d23a4f6e26180d810518e2aec3101d5bcb08d2afc0750358be1d6 (6/12)"
            ),
            SkopeoEvent::CopyImage {
                current: 6,
                total: 12
            }
        );
        assert_eq!(
            parse_skopeo_line("Copying blob sha256:7eedb4ba126ef2ed2a51e0ceccdb2d3817a9729dc9444a4ea79c0f96f5a223ce"),
            SkopeoEvent::CopyBlob {
                digest: "sha256:7eedb4ba126ef2ed2a51e0ceccdb2d3817a9729dc9444a4ea79c0f96f5a223ce".into()
            }
        );
        assert_eq!(
            parse_skopeo_line("Copying config sha256:68aeef3e3ec2909b25343754252ab926c74f7788175440efb3f3f9fe43af6a30"),
            SkopeoEvent::CopyConfig {
                digest: "sha256:68aeef3e3ec2909b25343754252ab926c74f7788175440efb3f3f9fe43af6a30".into()
            }
        );
        assert_eq!(
            parse_skopeo_line("Writing manifest to image destination"),
            SkopeoEvent::WriteManifest
        );
        assert_eq!(
            parse_skopeo_line("Storing signatures"),
            SkopeoEvent::StoringSignatures
        );
    }

    #[test]
    fn parse_own_lines() {
        assert_eq!(
            parse_skopeo_line("已登录上游 registry docker.m.daocloud.io"),
            SkopeoEvent::Login {
                host: "docker.m.daocloud.io".into()
            }
        );
        assert_eq!(
            parse_skopeo_line("已推送 docker.m.daocloud.io/library/ubuntu:22.04 → docker.io/library/ubuntu:22.04"),
            SkopeoEvent::Pushed {
                source: "docker.m.daocloud.io/library/ubuntu:22.04".into(),
                target: "docker.io/library/ubuntu:22.04".into()
            }
        );
        assert_eq!(parse_skopeo_line("任务完成"), SkopeoEvent::Done);
        assert_eq!(
            parse_skopeo_line("任务失败：skopeo copy … 失败"),
            SkopeoEvent::Failed
        );
        // 无法识别 → Other（progress 回落原始行）
        assert_eq!(parse_skopeo_line("something new"), SkopeoEvent::Other);
        assert_eq!(parse_skopeo_line("Copying image sha256:abc"), SkopeoEvent::Other);
        assert_eq!(parse_skopeo_line("Copying image sha256:abc (x/y)"), SkopeoEvent::Other);
    }

    #[test]
    fn apply_tracks_sub_image_and_layers() {
        let mut d = ProgressDetail::queued(1);
        assert_eq!(d.phase, phase::QUEUED);
        assert_eq!(d.percent, None);

        d.apply(&SkopeoEvent::GetSignatures, &ctx(1, 0, 1));
        assert_eq!(d.phase, phase::PREPARING);
        assert_eq!(d.message, "正在获取镜像清单");
        assert_eq!(d.percent, None, "单架构复制前不可推导百分比");

        d.apply(&SkopeoEvent::CopyImage { current: 6, total: 12 }, &ctx(1, 0, 1));
        assert_eq!(d.phase, phase::COPYING);
        assert_eq!(d.sub_current, Some(6));
        assert_eq!(d.sub_total, Some(12));
        assert_eq!(d.percent, Some(50));
        assert!(d.message.contains("第 6/12 个子镜像"), "{}", d.message);

        d.apply(&SkopeoEvent::CopyBlob { digest: "sha256:a".into() }, &ctx(1, 0, 1));
        d.apply(&SkopeoEvent::CopyBlob { digest: "sha256:b".into() }, &ctx(1, 0, 1));
        d.apply(&SkopeoEvent::CopyConfig { digest: "sha256:c".into() }, &ctx(1, 0, 1));
        assert_eq!(d.layers_done, 3);
        assert!(d.message.contains("已复制 3 层"), "{}", d.message);

        d.apply(&SkopeoEvent::WriteManifest, &ctx(1, 0, 1));
        assert_eq!(d.phase, phase::WRITING);
        assert_eq!(d.percent, Some(99));

        d.mark_terminal(phase::DONE, "任务完成");
        assert_eq!(d.percent, Some(100));
        assert_eq!(d.items_done, d.items_total);
    }

    #[test]
    fn retry_backoff_seconds_are_exposed_and_cleared() {
        let mut d = ProgressDetail::queued(1);
        assert_eq!(d.retry_in_secs, None);

        d.apply(
            &SkopeoEvent::RetryWait {
                attempt: 1,
                max: 3,
                delay_secs: 5,
            },
            &ctx(1, 0, 1),
        );
        assert_eq!(d.attempt, 1);
        assert_eq!(d.max_attempts, 3);
        assert_eq!(d.retry_in_secs, Some(5));

        // 重试开始（Attempt）→ 退避秒数清除
        d.apply(&SkopeoEvent::Attempt { attempt: 2, max: 3 }, &ctx(1, 0, 1));
        assert_eq!(d.retry_in_secs, None);

        // 终态同样清除
        d.apply(
            &SkopeoEvent::RetryWait {
                attempt: 2,
                max: 3,
                delay_secs: 10,
            },
            &ctx(1, 0, 1),
        );
        d.mark_terminal(phase::FAILED, "任务失败");
        assert_eq!(d.retry_in_secs, None);
    }

    #[test]
    fn multi_tag_progress_is_derivable() {
        let mut d = ProgressDetail::queued(4);
        assert_eq!(d.message, "等待执行（共 4 个标签）");

        // 第 2 个标签，已完成 1 个，复制中
        d.apply(&SkopeoEvent::GetSignatures, &ctx(2, 1, 4));
        assert_eq!(d.message, "正在获取镜像清单 —— 第 2/4 个标签");
        assert_eq!(d.percent, None, "preparing 阶段无百分比");

        d.apply(&SkopeoEvent::CopyBlob { digest: "sha256:a".into() }, &ctx(2, 1, 4));
        assert_eq!(d.percent, Some(25), "已完成 1/4");

        // 第 4 个标签的收尾
        d.apply(&SkopeoEvent::WriteManifest, &ctx(4, 3, 4));
        assert_eq!(d.percent, Some(99));

        d.mark_terminal(phase::CANCELLED, "任务已取消");
        assert_eq!(d.phase, phase::CANCELLED);
        assert_eq!(d.percent, None);
    }

    #[test]
    fn compute_percent_never_fakes() {
        // 单架构、无子镜像计数 → 不可推导
        assert_eq!(compute_percent(phase::COPYING, 0, 1, None, None), None);
        assert_eq!(compute_percent(phase::COPYING, 0, 1, Some(1), Some(1)), None);
        // 多子镜像
        assert_eq!(compute_percent(phase::COPYING, 0, 1, Some(3), Some(4)), Some(75));
        assert_eq!(compute_percent(phase::COPYING, 0, 1, Some(9), Some(4)), Some(99), "越界封顶 99");
        // 多标签
        assert_eq!(compute_percent(phase::COPYING, 2, 4, None, None), Some(50));
        // 收尾 / 完成
        assert_eq!(compute_percent(phase::WRITING, 0, 1, None, None), Some(99));
        assert_eq!(compute_percent(phase::DONE, 0, 1, None, None), Some(100));
        // 其它阶段不推导
        assert_eq!(compute_percent(phase::LOGIN, 0, 1, None, None), None);
        assert_eq!(compute_percent(phase::QUEUED, 0, 1, None, None), None);
    }

    #[test]
    fn queued_sets_total_and_message() {
        let d = ProgressDetail::queued(0);
        assert_eq!(d.items_total, 1);
        assert_eq!(d.message, "等待执行");
        let d = ProgressDetail::queued(3);
        assert_eq!(d.items_total, 3);
        assert!(d.message.contains("3 个标签"));
    }

    #[test]
    fn progress_detail_serde_roundtrip() {
        let mut d = ProgressDetail::queued(2);
        d.apply(
            &SkopeoEvent::CopyImage { current: 2, total: 4 },
            &ctx(1, 0, 2),
        );
        let json = serde_json::to_string(&d).unwrap();
        let back: ProgressDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
        // 旧记录缺字段时按默认值加载
        let legacy: ProgressDetail = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.phase, phase::QUEUED);
        assert_eq!(legacy.percent, None);
    }
}