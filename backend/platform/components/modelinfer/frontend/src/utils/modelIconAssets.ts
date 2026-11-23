/**
 * 模型家族图标 URL 解析（Vite 资产）。
 *
 * 图标资产位于 `assets/model-icons/*.svg`，文件名即家族键（`<family>.svg`），
 * `generic.svg` 为通用兜底。通过 `import.meta.glob` 交给 Vite 打包为带内容 hash 的
 * 相对 URL，随 `base: './'` 子路径托管正确解析，且**不依赖运行时 CDN**。
 */
const modules = import.meta.glob('../assets/model-icons/*.svg', {
  eager: true,
  query: '?url',
  import: 'default',
}) as Record<string, string>;

const ICON_URLS: Record<string, string> = {};
for (const [path, url] of Object.entries(modules)) {
  const name = path.split('/').pop()?.replace(/\.svg$/, '') ?? '';
  if (name) ICON_URLS[name] = url;
}

/** 取家族图标 URL；未知家族回落 `generic.svg`（再兜底为空串） */
export function modelIconUrl(family?: string | null): string {
  if (family && ICON_URLS[family]) return ICON_URLS[family];
  return ICON_URLS.generic ?? '';
}