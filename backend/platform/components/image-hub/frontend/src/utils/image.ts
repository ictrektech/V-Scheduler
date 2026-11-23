/**
 * 镜像引用解析/归一纯函数（与后端 `skopeo.rs::parse_image_ref` 规则等价）。
 *
 * 规则：
 * - 支持 `[registry/]repository[:tag]`；tag 缺省 `latest`
 * - 首段含 `.`/`:` 或为 `localhost` → registry host；否则补 `docker.io`
 * - 无 namespace 的单段名补 `library/`（如 `ubuntu` → `docker.io/library/ubuntu`）
 * - `@digest` 引用不支持 → 返回 null（后端对应 20809）
 * - 本地 registry 目标保留**上游完整镜像名**（registry 中仓库名 = 上游镜像名）
 */

/** 缺省上游 registry */
export const DEFAULT_REGISTRY = 'docker.io';

export interface ParsedImageRef {
  registry: string;
  /** namespace + 名称（如 `library/ubuntu`） */
  repository: string;
  tag: string;
}

/** 首段是否为 registry host（含 `.` / `:` 或为 `localhost`） */
export function looksLikeRegistryHost(segment: string): boolean {
  return segment === 'localhost' || segment.includes('.') || segment.includes(':');
}

/** 合法 tag：字母数字与 `._-`，长度 1..=128 */
export function isValidTag(tag: string): boolean {
  return tag.length >= 1 && tag.length <= 128 && /^[A-Za-z0-9._-]+$/.test(tag);
}

/** 合法仓库名：小写字母数字与 `._-/`，无 `..` / 空段 / 首尾斜杠 */
export function isValidRepository(repo: string): boolean {
  return (
    repo.length > 0 &&
    !repo.startsWith('/') &&
    !repo.endsWith('/') &&
    !repo.includes('..') &&
    repo.split('/').every((s) => s.length > 0) &&
    /^[a-z0-9._/-]+$/.test(repo)
  );
}

/** 解析并归一镜像引用；非法返回 null */
export function parseImageRef(input: string): ParsedImageRef | null {
  const raw = (input ?? '').trim();
  if (!raw || raw.includes('@') || /\s/.test(raw)) {
    return null;
  }

  let namePart = raw;
  let tag = 'latest';
  const idx = raw.lastIndexOf(':');
  if (idx > -1 && !raw.slice(idx + 1).includes('/')) {
    namePart = raw.slice(0, idx);
    tag = raw.slice(idx + 1);
  }
  if (!isValidTag(tag) || !namePart) {
    return null;
  }

  let registry = DEFAULT_REGISTRY;
  let repository: string;
  const slash = namePart.indexOf('/');
  if (slash > -1) {
    const first = namePart.slice(0, slash);
    const rest = namePart.slice(slash + 1);
    if (!first || !rest) {
      return null;
    }
    if (looksLikeRegistryHost(first)) {
      registry = first.toLowerCase();
      repository = rest;
    } else {
      repository = namePart;
    }
  } else {
    repository = `library/${namePart}`;
  }

  if (!isValidRepository(repository)) {
    return null;
  }
  return { registry, repository, tag };
}

/** 上游完整仓库名（`registry/repository`）——即 registry 中存储的仓库名 */
export function upstreamRepository(ref: ParsedImageRef): string {
  return `${ref.registry}/${ref.repository}`;
}

/** 上游完整引用（`registry/repository:tag`） */
export function upstreamReference(ref: ParsedImageRef): string {
  return `${upstreamRepository(ref)}:${ref.tag}`;
}

/** 归一后的上游引用字符串；非法返回 null */
export function normalizeImageRef(input: string): string | null {
  const parsed = parseImageRef(input);
  return parsed ? upstreamReference(parsed) : null;
}

/** 本地 registry 目标引用（`<destHost>/<上游完整仓库名>:<tag>`） */
export function localImageRef(destHost: string, ref: ParsedImageRef): string {
  return `${destHost}/${upstreamRepository(ref)}:${ref.tag}`;
}