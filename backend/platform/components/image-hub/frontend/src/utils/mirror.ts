/**
 * 镜像源（mirror）纯函数（与后端 `mirror.rs` 规则等价）。
 *
 * 语义（D-IH-32）：
 * - 镜像源**仅替换实际拉取来源的 registry host**，路径与 tag 原样保留；
 * - 推入本地 registry 的仓库名恒由**上游完整镜像名**派生（镜像源主机名绝不进入落地名）；
 * - 候选目录来自后端 `GET /api/registry/mirrors`，前端仅按上游 registry 过滤；
 * - 默认不使用镜像源，也支持手动输入任意合法主机名。
 *
 * 本模块保持零依赖（便于 `node --test` 直接测试）。
 */

/** 镜像源目录项（与后端 `MirrorOption` 对应） */
export interface MirrorOption {
  /** 该镜像源对应的上游 registry（据此匹配候选） */
  upstream_registry: string;
  /** 镜像源主机名（可含端口） */
  host: string;
  /** 展示名（提供方，后端中文名，作为未知 provider 的回退） */
  name: string;
  /** 提供方稳定标识（如 daocloud / 1panel / 1ms / xuanyuan / dockerproxy；旧后端可能缺失） */
  provider?: string;
}

/** 归一后的镜像引用（结构上与 `utils/image.ts` 的 `ParsedImageRef` 兼容） */
export interface MirrorImageRef {
  registry: string;
  repository: string;
  tag: string;
}

/** 归一化 registry / 主机名：去空白、去尾斜杠、小写（与后端 `normalize_host` 等价） */
export function normalizeRegistry(value: string | null | undefined): string {
  return (value ?? '').trim().replace(/\/+$/, '').toLowerCase();
}

/** 按上游 registry 过滤候选镜像源（大小写无关；未收录 → 空数组） */
export function filterMirrorsByRegistry(
  catalog: MirrorOption[] | null | undefined,
  registry: string,
): MirrorOption[] {
  const want = normalizeRegistry(registry);
  if (!want) return [];
  return (catalog ?? []).filter((m) => normalizeRegistry(m.upstream_registry) === want);
}

/**
 * 校验手动输入的镜像源主机名（与后端 `validate_registry_host` 字符集一致）：
 * 非空、仅 `[a-z0-9._:-]`、且不含 `/`。
 */
export function isValidMirrorHost(value: string): boolean {
  const v = normalizeRegistry(value);
  return v.length > 0 && !v.includes('/') && /^[a-z0-9._:-]+$/.test(v);
}

/**
 * 实际拉取源引用：镜像源非空时为 `<mirror>/<上游 repository>:<tag>`，
 * 否则回落上游引用（`<上游 registry>/<repository>:<tag>`）。
 */
export function resolveMirrorSourceRef(
  mirrorHost: string | null | undefined,
  ref: MirrorImageRef,
): string {
  const host = normalizeRegistry(mirrorHost);
  const base = host ? `${host}/${ref.repository}` : `${ref.registry}/${ref.repository}`;
  return `${base}:${ref.tag}`;
}