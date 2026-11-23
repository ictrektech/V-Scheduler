//! 时间工具：统一中国标准时间（CST，UTC+8，IANA `Asia/Shanghai`）
//!
//! 对齐仓库根 `.agents/rules/01-general.md` 时区规约：所有时间语义使用 CST。

use chrono::{DateTime, FixedOffset, Utc};

/// 返回当前 CST 时间（UTC+8 固定偏移）
pub fn now() -> DateTime<FixedOffset> {
    Utc::now().with_timezone(&cst_offset())
}

/// 解析 ISO-8601 时间串为 epoch 秒（无法解析 / 空串返回 `None`）
///
/// 用于自动重启的计时比较（失败起点 / backoff 截止 / 最近动作时间）：落库为 CST ISO-8601，
/// 读取后统一按 epoch 秒做差，避免字符串比较与时区偏移问题。
pub fn parse_iso_epoch(s: &str) -> Option<i64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .ok()
        .map(|dt| dt.timestamp())
}

/// 把「当前 CST 时间 + 秒数」渲染为 ISO-8601（用于 backoff 截止时间）
pub fn iso_after(now: DateTime<FixedOffset>, secs: i64) -> String {
    (now + chrono::Duration::seconds(secs)).to_rfc3339()
}

/// CST（UTC+8）固定偏移
fn cst_offset() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+8 偏移合法")
}

/// 系统时间 → CST ISO-8601 字符串（用于 Temporal 返回的 SystemTime）
pub fn system_time_to_iso(t: std::time::SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    dt.with_timezone(&cst_offset()).to_rfc3339()
}

/// 返回当前 CST 时间的 ISO-8601 字符串（用于 JSON 响应）
pub fn now_iso() -> String {
    now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_utc_plus_8() {
        let t = now();
        assert_eq!(t.offset().local_minus_utc(), 8 * 3600);
        assert!(now_iso().contains("+08:00"));
    }

    #[test]
    fn parse_iso_epoch_roundtrip_and_invalid() {
        assert_eq!(parse_iso_epoch(""), None);
        assert_eq!(parse_iso_epoch("not-a-time"), None);
        let at = parse_iso_epoch("2026-10-21T12:00:00+08:00").unwrap();
        let later = parse_iso_epoch("2026-10-21T12:05:00+08:00").unwrap();
        assert_eq!(later - at, 300);
        // iso_after 与解析一致（+08:00 偏移保留）
        let rendered = iso_after(now(), 60);
        assert!(rendered.contains("+08:00"));
        assert!(parse_iso_epoch(&rendered).is_some());
    }
}
