/**
 * 错误码 → i18n 文案的纯函数解析（零依赖，便于 `node --test` 直接测试）。
 *
 * `i18n/api-error.ts` 在此之上接入 `ImageHubApiError` 与 i18n 实例。
 */

/** 错误码对应的 i18n key（`errors.<code>`） */
export function errorCodeKey(code: number): string {
  return `errors.${code}`;
}

/**
 * 解析错误码文案。
 *
 * @param code 后端业务错误码
 * @param hasTranslation 判断 key 是否存在
 * @param translate 翻译函数
 * @returns 已收录码的本地化文案；未收录码的「通用提示 + 错误码」文案
 */
export function resolveErrorCode(
  code: number,
  hasTranslation: (key: string) => boolean,
  translate: (key: string, params?: Record<string, unknown>) => string,
): string {
  const key = errorCodeKey(code);
  if (hasTranslation(key)) return translate(key);
  return translate('errors.unknownWithCode', { code });
}