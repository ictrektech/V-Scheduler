/**
 * 字节格式化（IEC 1024 进制，按量级自动选择单位；对齐仓库存储容量单位规约）。
 */
export function formatBytes(bytes?: number | null): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes) || bytes < 0) {
    return '-';
  }
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
  let value = bytes;
  let idx = 0;
  while (value >= 1024 && idx < units.length - 1) {
    value /= 1024;
    idx += 1;
  }
  const fixed = idx === 0 ? String(value) : value.toFixed(value >= 100 ? 0 : value >= 10 ? 1 : 2);
  return `${fixed} ${units[idx]}`;
}

/** 两位补零 */
function pad2(value: number): string {
  return String(value).padStart(2, '0');
}

/**
 * 时间戳格式化：`YYYY-MM-DD HH:mm:ss`（本地时区）。
 *
 * 使用语言中立格式，避免不同浏览器/系统 locale 导致同一时间展示不一致；
 * 无效值原样返回，空值返回 `-`。
 */
export function formatTime(value?: string | null): string {
  if (!value) return '-';
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return value;
  return (
    `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ` +
    `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`
  );
}