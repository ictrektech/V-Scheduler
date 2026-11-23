/**
 * IP 排序工具函数（对齐 vivibit-web 服务管理页语义）。
 *
 * 用于设备 IP 列表排序：优先将与当前网页访问主机相同的 IP 排到最前，
 * 将回环、Docker 网关等本地地址排到末尾，方便用户默认选中最合适的地址。
 */

/**
 * 将可能带方括号（IPv6）或端口号的地址规整为纯主机部分（小写）。
 */
export function normalizeHost(value: string): string {
  const trimmed = value.trim().toLowerCase();
  // 带方括号的 IPv6：`[::1]:5666` → `::1`
  const bracketed = trimmed.match(/^\[([^\]]+)\]/);
  if (bracketed?.[1]) return bracketed[1];
  // IPv4（可带端口号）：`192.168.1.100:5666` → `192.168.1.100`
  const ipv4 = trimmed.match(/^(\d{1,3}(?:\.\d{1,3}){3})(?::\d+)?$/);
  if (ipv4?.[1]) return ipv4[1];
  // 主机名（可带端口号）：`nas.local:5666` → `nas.local`
  const hostPort = trimmed.match(/^([a-z0-9][a-z0-9.-]*):(\d+)$/);
  if (hostPort?.[1]) return hostPort[1];
  // 其余包含冒号者视为 IPv6（如 `::1`、`fe80::1`）
  return trimmed;
}

/**
 * 判断 IP 是否为本地低优先级地址（回环 / Docker 网段网关等）。
 */
export function isLocalIp(ip: string): boolean {
  const host = normalizeHost(ip);
  // IPv6 回环
  if (host === '::1') return true;
  // IPv4 回环 127.0.0.0/8
  if (host.startsWith('127.')) return true;
  // Docker 默认地址池 172.16.0.0/12（如网关 172.17.0.1）
  const parts = host.split('.');
  if (parts.length === 4) {
    const first = Number(parts[0]);
    const second = Number(parts[1]);
    if (first === 172 && second >= 16 && second <= 31) return true;
  }
  return false;
}

/**
 * 对设备 IP 列表排序，返回新数组（不修改原数组）：
 * 1. 与当前网页访问主机相同的 IP 排在最前（第一优先级）
 * 2. 其余可路由 IP 保持原有相对顺序（第二优先级）
 * 3. 非当前访问主机的回环 / Docker 网关等本地 IP 排到末尾（低优先级）
 */
export function sortDeviceIpList(ipList: string[], currentHost: string): string[] {
  const host = normalizeHost(currentHost);

  const priority = (ip: string): number => {
    const clean = normalizeHost(ip);
    if (clean === host) return 0;
    if (isLocalIp(clean)) return 2;
    return 1;
  };

  return [...ipList].sort((a, b) => priority(a) - priority(b));
}

/**
 * 兜底地址列表：平台网卡列表为空时使用当前页面主机名，保证始终可渲染地址。
 */
export function addressListOrFallback(ipList: string[], currentHost: string): string[] {
  const sorted = sortDeviceIpList(ipList ?? [], currentHost);
  if (sorted.length > 0) return sorted;
  const fallback = (currentHost || '').trim();
  return fallback ? [fallback] : [];
}