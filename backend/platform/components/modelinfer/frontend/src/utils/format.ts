/**
 * 展示层格式化工具。
 *
 * - 字节 ↔ IEC 单位换算只发生在此处（见仓库根 `.agents/rules/01-general.md`「存储容量单位规约」）。
 * - 时间统一以本地时区（CST / Asia/Shanghai）呈现，输入兼容带纳秒的 RFC3339 串。
 */
import dayjs from 'dayjs';
import utc from 'dayjs/plugin/utc';
import timezone from 'dayjs/plugin/timezone';
import relativeTime from 'dayjs/plugin/relativeTime';
import 'dayjs/locale/zh-cn';
import 'dayjs/locale/en';
import { translate } from '@/i18n';

dayjs.extend(utc);
dayjs.extend(timezone);
dayjs.extend(relativeTime);
// dayjs locale 由 `i18n/index.ts` 的 setLanguage / 首帧同步设置（默认 zh-cn）

/** 时间语义统一 CST（Asia/Shanghai），不依赖运行宿主时区（见仓库根时区规约） */
const TIMEZONE = 'Asia/Shanghai';

const IEC_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'] as const;

/**
 * 格式化字节数为 IEC 单位（1024 进制，自动选择最贴近的单位）。
 *
 * - `null` / `undefined` → `-`
 * - `< 1 KiB` → 字节整数（如 `512 B`）
 * - 其余按量级选择单位并保留 1 位小数
 */
export function formatBytes(bytes?: number | null): string {
  if (bytes == null || Number.isNaN(bytes)) return '-';
  if (bytes < 1024) return `${bytes} B`;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < IEC_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${IEC_UNITS[unit]}`;
}

/**
 * 解析后端时间串为 CST（Asia/Shanghai）dayjs；兼容带纳秒的 RFC3339（如
 * `2026-09-22T21:56:34.585257561+08:00`，多余小数位截断到毫秒）。解析失败返回 null。
 */
function toDayjs(value?: string | null): dayjs.Dayjs | null {
  if (!value) return null;
  const raw = String(value).trim();
  if (!raw) return null;
  const cleaned = raw.replace(/(\.\d{3})\d+/, '$1');
  const d = dayjs(cleaned);
  if (d.isValid()) return d.tz(TIMEZONE);
  const fallback = dayjs(raw);
  return fallback.isValid() ? fallback.tz(TIMEZONE) : null;
}

/** 本地时间（分钟精度）：`2026-09-22 21:56`；空值 → `-`，无法解析 → 原样回显 */
export function formatTime(value?: string | null): string {
  const d = toDayjs(value);
  return d ? d.format('YYYY-MM-DD HH:mm') : value ? String(value) : '-';
}

/** 本地时间（秒精度）：用于悬浮提示 */
export function formatFullTime(value?: string | null): string {
  const d = toDayjs(value);
  return d ? d.format('YYYY-MM-DD HH:mm:ss') : value ? String(value) : '-';
}

/** 相对时间：`3 分钟前` / `2 天前`；空值 → `-`，无法解析 → 原样回显 */
export function formatRelativeTime(value?: string | null): string {
  const d = toDayjs(value);
  return d ? d.fromNow() : value ? String(value) : '-';
}

/** 时间悬浮提示：`2026-09-22 21:56:34（3 分钟前）` */
export function formatTimeTooltip(value?: string | null): string {
  const d = toDayjs(value);
  if (!d) return value ? String(value) : '-';
  return translate('common.timeTooltip', {
    time: d.format('YYYY-MM-DD HH:mm:ss'),
    relative: d.fromNow(),
  });
}

/** 秒 → 人类可读时长：`45s` / `3m20s` / `1h05m` / `2d3h` */
export function formatSeconds(totalSeconds: number): string {
  if (!Number.isFinite(totalSeconds) || totalSeconds < 0) return '-';
  const s = Math.floor(totalSeconds);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rs = s % 60;
  if (m < 60) return rs ? `${m}m${rs}s` : `${m}m`;
  const h = Math.floor(m / 60);
  const rm = m % 60;
  if (h < 24) return rm ? `${h}h${rm}m` : `${h}h`;
  const d = Math.floor(h / 24);
  const rh = h % 24;
  return rh ? `${d}d${rh}h` : `${d}d`;
}

/** 起止时间 → 耗时（结束为空按「至今」计算）；无法解析返回 `-` */
export function formatDuration(from?: string | null, to?: string | null): string {
  const a = toDayjs(from);
  if (!a) return '-';
  const b = toDayjs(to) ?? dayjs().tz(TIMEZONE);
  return formatSeconds(b.diff(a, 'second'));
}

/**
 * 将步骤结果摘要 `detail`（各活动自定义 JSON）压缩为**单行关键值摘要**，
 * 仅取顶层**标量**与数组 / 对象的大小，便于在折叠面板标题中一行展示。
 *
 * 纯函数、容错：非对象 / 空对象 / 无可展示项 → 空串（调用方自行决定不展示）。
 * MUST NOT 抛错，也 MUST NOT 因摘要失败影响步骤渲染。
 */
export function summarizeDetail(detail: unknown, maxEntries = 4): string {
  if (detail == null) return '';
  if (Array.isArray(detail))
    return detail.length ? `${detail.length} ${translate('common.itemsSuffix')}` : '';
  if (typeof detail !== 'object') return '';
  const parts: string[] = [];
  for (const [key, value] of Object.entries(detail as Record<string, unknown>)) {
    if (value == null || value === '') continue;
    if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
      parts.push(`${key}=${value}`);
    } else if (Array.isArray(value)) {
      parts.push(`${key}[${value.length}]`);
    } else if (typeof value === 'object') {
      parts.push(`${key}{${Object.keys(value as object).length}}`);
    }
    if (parts.length >= maxEntries) break;
  }
  return parts.join(' · ');
}