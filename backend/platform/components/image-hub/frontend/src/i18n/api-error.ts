import { ImageHubApiError } from '@/api/client';
import { hasTranslation, translate } from './index';
import { resolveErrorCode } from './error-code';

/**
 * 将 API 错误解析为当前语言的用户提示。
 *
 * 优先级：
 * 1. `ImageHubApiError` 且存在 `errors.<code>` → 本地化错误码文案；
 * 2. `ImageHubApiError` 但错误码未收录 → 通用提示 + 错误码（`errors.unknownWithCode`）；
 * 3. 其它异常（网络 / 超时 / 前端逻辑）→ `fallbackKey`（默认 `errors.unknown`）。
 *
 * 后端 `msg` 不直接展示（英文界面避免混入中文）；需要排障时查看控制台 / 任务日志。
 *
 * @param error 捕获到的任意异常
 * @param fallbackKey 未知异常使用的 i18n key
 */
export function resolveApiErrorMessage(
  error: unknown,
  fallbackKey = 'errors.unknown',
): string {
  if (error instanceof ImageHubApiError) {
    return resolveErrorCode(error.code, hasTranslation, translate);
  }
  return translate(fallbackKey);
}

/**
 * 解析任务失败原因（任务记录的结构化 `error_code` 优先）。
 *
 * 旧记录 / 无错误码时回退后端 `error` 原文（可能是中文调试文本）。
 */
export function resolveTaskErrorMessage(
  errorCode: number | null | undefined,
  rawError: string | null | undefined,
): string | null {
  if (typeof errorCode === 'number') {
    return resolveErrorCode(errorCode, hasTranslation, translate);
  }
  return rawError || null;
}