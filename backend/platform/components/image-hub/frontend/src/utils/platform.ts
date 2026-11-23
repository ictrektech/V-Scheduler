/**
 * 平台条目展示纯函数（与后端 `registry.rs` 的平台条目契约一致）。
 *
 * 平台条目来源有两类（后端统一为 `platforms[]`）：
 * - manifest list / image index：索引声明多个平台；
 * - 单平台 image manifest：由后端从 config blob 的 `os`/`architecture`/`variant` 合成**单条**。
 *
 * 本模块保持零依赖（便于 `node --test` 直接测试）。
 */

/** 平台条目（与后端 `PlatformEntry` 一致） */
export interface PlatformLike {
  os?: string | null;
  architecture?: string | null;
  variant?: string | null;
  digest?: string | null;
  size_bytes?: number | null;
}

/** 平台条目文案：`os/architecture[/variant]`（缺失段以 `?` 占位，语言中立） */
export function platformEntryText(entry: PlatformLike): string {
  const os = entry.os || '?';
  const arch = entry.architecture || '?';
  return entry.variant ? `${os}/${arch}/${entry.variant}` : `${os}/${arch}`;
}

/** 详情页「平台」列展示结构（组件层据此本地化数量与占位） */
export type PlatformColumnParts =
  | { kind: 'none' }
  | { kind: 'single'; value: string }
  | { kind: 'count'; value: number };

/**
 * 详情页「平台」列展示结构：
 * - 1 条 → `single`，值为 `os/arch[/variant]`（单平台镜像走此分支）
 * - 多于 1 条 → `count`，值为数量（组件层翻译为「N 个」/「N platforms」）
 * - 0 条 → `none`（组件层显示占位 `-`，不臆测为「单平台」）
 */
export function platformColumnParts(
  platforms: PlatformLike[] | null | undefined,
): PlatformColumnParts {
  const list = platforms ?? [];
  if (list.length === 0) return { kind: 'none' };
  if (list.length === 1) return { kind: 'single', value: platformEntryText(list[0]) };
  return { kind: 'count', value: list.length };
}

/** 是否展示平台条目表格（展开行） */
export function hasPlatforms(platforms: PlatformLike[] | null | undefined): boolean {
  return (platforms?.length ?? 0) > 0;
}