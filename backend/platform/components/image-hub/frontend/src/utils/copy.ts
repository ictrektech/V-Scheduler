/**
 * 克隆 / 改名目标推算与校验纯函数（与后端 `handlers/copy.rs` 校验语义对齐）。
 *
 * 语义（D-IH-50 ~ D-IH-52 / D-IH-56）：
 * - 单 tag：目标 = `target_repository`（缺省 = 源仓库） + `target_tag`（缺省 = 源 tag）
 * - 整仓：目标 = `target_repository`（必填，且必须与源仓库不同），各标签保持原名
 * - 目标名按镜像引用规则归一（缺省 `docker.io`、单段名补 `library/`）
 * - 目标与源相同 / 目标仓库名非法 / 整仓带 tag → 视为不可提交（后端对应 20801 / 20809）
 *
 * 本模块仅依赖 `utils/image.ts`（零三方依赖，便于 `node --test` 直接测试）。
 */

import { isValidTag, parseImageRef, upstreamRepository } from './image.ts';

export type CopyMode = 'clone' | 'rename';
export type CopyScope = 'tag' | 'repository';

export interface CopyPreview {
  scope: CopyScope;
  /** 归一后的目标引用预览（整仓时为 `<repo>:<tag 保持原名>`）；不可提交时为 null */
  targetRef: string | null;
  /** 校验错误的 i18n key（null = 可提交；文案由组件层翻译） */
  errorKey: string | null;
}

/** 从名称中取出显式 tag（最后一个 `:` 之后且不含 `/`） */
export function explicitTag(name: string): string | null {
  const raw = (name ?? '').trim();
  const idx = raw.lastIndexOf(':');
  if (idx < 0) return null;
  const tail = raw.slice(idx + 1);
  if (tail.includes('/') || tail.length === 0) return null;
  return tail;
}

/** 去除名称中的显式 tag（保留仓库部分） */
export function withoutTag(name: string): string {
  const raw = (name ?? '').trim();
  const idx = raw.lastIndexOf(':');
  if (idx < 0 || raw.slice(idx + 1).includes('/')) return raw;
  return raw.slice(0, idx);
}

/**
 * 推算克隆 / 改名目标并校验。
 *
 * @param sourceRepository 源仓库名（registry 中的名字，如 `docker.io/library/alpine`）
 * @param sourceTag 源 tag；缺省表示整仓
 * @param targetRepositoryInput 用户输入的目标仓库名（可空）
 * @param targetTagInput 用户输入的目标 tag（仅单 tag 模式使用）
 */
export function copyPreview(
  sourceRepository: string,
  sourceTag: string | undefined,
  targetRepositoryInput: string,
  targetTagInput: string,
): CopyPreview {
  const scope: CopyScope = sourceTag ? 'tag' : 'repository';
  const repoRaw = (targetRepositoryInput ?? '').trim() || (scope === 'tag' ? sourceRepository : '');
  if (!repoRaw) {
    return { scope, targetRef: null, errorKey: 'copy.errors.targetRequired' };
  }
  if (explicitTag(repoRaw)) {
    return { scope, targetRef: null, errorKey: 'copy.errors.targetHasTag' };
  }
  const parsed = parseImageRef(repoRaw);
  if (!parsed) {
    return { scope, targetRef: null, errorKey: 'copy.errors.targetInvalid' };
  }
  const normalizedRepo = upstreamRepository(parsed);

  if (scope === 'repository') {
    if ((targetTagInput ?? '').trim()) {
      return { scope, targetRef: null, errorKey: 'copy.errors.repoTagNotSupported' };
    }
    if (normalizedRepo === sourceRepository) {
      return { scope, targetRef: null, errorKey: 'copy.errors.sameRepo' };
    }
    return { scope, targetRef: normalizedRepo, errorKey: null };
  }

  const tag = (targetTagInput ?? '').trim() || sourceTag || '';
  if (!isValidTag(tag)) {
    return { scope, targetRef: null, errorKey: 'copy.errors.tagInvalid' };
  }
  if (normalizedRepo === sourceRepository && tag === sourceTag) {
    return { scope, targetRef: null, errorKey: 'copy.errors.sameTarget' };
  }
  return { scope, targetRef: `${normalizedRepo}:${tag}`, errorKey: null };
}

/** 操作动词 i18n key（用于标题 / 提示文案） */
export function copyVerbKey(mode: CopyMode): string {
  return mode === 'rename' ? 'copy.verb.rename' : 'copy.verb.clone';
}

/**
 * 源引用展示结构（整仓时 `ref` 为仓库名，`allTags` 为 true，由组件层追加「全部标签」本地化文案）。
 */
export function sourceRefParts(
  repository: string,
  tag?: string,
): { ref: string; allTags: boolean } {
  return { ref: tag ? `${repository}:${tag}` : repository, allTags: !tag };
}