/**
 * 拉取架构选择纯函数与候选（与后端 `ArchMode` / skopeo `--multi-arch` 平台列表契约一致）。
 *
 * 约束（与 `skopeo 1.24.1` 实测一致）：
 * - `--multi-arch` 平台列表仅接受 `os/arch`（如 `linux/amd64`），**不支持 variant**；
 * - 平台过滤按 `os` + `architecture` 匹配、忽略 variant（故 `linux/arm` 可命中源中的
 *   `linux/arm/v7`）。
 *
 * 因此候选使用 **docker 标准完整平台名** `os/arch`（与镜像 index 平台条目、`docker pull
 * --platform` 口径一致），不提供 `linux/arm/v7` / `linux/arm64/v8` 等 variant 形式。
 *
 * 本模块保持零依赖（便于 `node --test` 直接测试）。
 */

/** 常见平台候选（docker 标准完整平台名 `os/arch`；`linux/arm` 可命中源中的 `linux/arm/v7`） */
export const ARCH_OPTIONS: readonly string[] = [
  'linux/amd64',
  'linux/arm64',
  'linux/arm',
  'linux/386',
  'linux/ppc64le',
  'linux/s390x',
  'linux/riscv64',
];

/** 「指定架构」模式默认预选（离线中转最常见的两种平台） */
export const DEFAULT_SPECIFIC_ARCHS: readonly string[] = ['linux/amd64', 'linux/arm64'];

/** 架构选择模式（与后端 `arch` 请求契约一致） */
export type ArchMode = 'all' | 'host' | 'specific';

/** 归一化架构列表：去空白 / 去空项 / 去重（保序） */
export function normalizeArchList(archs: readonly string[]): string[] {
  const out: string[] = [];
  for (const raw of archs) {
    const arch = raw.trim();
    if (arch && !out.includes(arch)) out.push(arch);
  }
  return out;
}

/**
 * 计算提交给后端的 `arch` 字段值：
 * - `all` → `all`（全部架构）
 * - `host` → `host`（本地架构）
 * - `specific` → 所选平台以 `,` 拼接（保持顺序、去重、去空白；如 `linux/amd64,linux/arm64`）
 */
export function archRequestValue(mode: ArchMode, archs: readonly string[]): string {
  if (mode === 'all') return 'all';
  if (mode === 'host') return 'host';
  return normalizeArchList(archs).join(',');
}

/** 任务「架构」展示结构（组件层据此本地化 `all` / `host` / 空值） */
export type ArchDisplay =
  | { kind: 'none' }
  | { kind: 'all' }
  | { kind: 'host' }
  | { kind: 'list'; value: string };

/**
 * 任务「架构」展示结构：
 * - 空 → `none`（组件层显示 `-`）
 * - `all` → `all`；`host` / `system` → `host`
 * - 其余（平台列表）→ `list` 原样展示（如 `linux/amd64,linux/arm64`）
 */
export function formatArch(value: string | null | undefined): ArchDisplay {
  const v = (value ?? '').trim();
  if (!v) return { kind: 'none' };
  if (v === 'all') return { kind: 'all' };
  if (v === 'host' || v === 'system') return { kind: 'host' };
  return { kind: 'list', value: v };
}