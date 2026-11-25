import { message } from 'ant-design-vue';
import { isRuntimeAuthError } from '@scheduler-auth';
/**
 * API 错误 -> 本地化提示 解析。
 *
 * 后端返回稳定数字码（`ResultBody.code`，本组件 207xx），本模块把码映射到 i18n key
 * `error.<code>`；未知码回退后端 `msg` 文本或通用兜底提示（不出现空白 / 静默失败）。
 */
import { hasTranslation, translate } from './index';
import { ModelinferApiError } from '@/api/client';

/** 数字码 -> i18n key（`error.<code>`，如 `error.20701`） */
export function errorCodeToI18nKey(code: number): string {
  return `error.${code}`;
}

/**
 * 解析错误为可展示的本地化文案。
 *
 * @param error 捕获到的错误（可为 `ModelinferApiError` / `Error` / 任意值）
 * @param fallback 无可用信息时的兜底文案（缺省使用通用提示）
 */
export function resolveApiErrorMessage(error: unknown, fallback?: string): string {
  const fb = fallback ?? translate('common.requestFailed');

  if (error instanceof ModelinferApiError) {
    if (typeof error.code === 'number') {
      const key = errorCodeToI18nKey(error.code);
      if (hasTranslation(key)) {
        const hint = translate(key);
        // NFS 共享错误需保留具体存储空间 / 路径，便于用户找到并修复配置。
        if (error.code === 20750 && error.message.trim() && error.message !== hint) {
          return `${hint}\n\n${error.message.trim()}`;
        }
        return hint;
      }
    }
    return error.message || fb;
  }

  if (error instanceof Error) {
    return error.message || fb;
  }

  return fb;
}

/** Embedded session recovery is shown once by the V-Scheduler workspace. */
export function reportApiError(error: unknown, prefix = ''): void {
  if (!isRuntimeAuthError(error)) message.error(prefix + resolveApiErrorMessage(error));
}
