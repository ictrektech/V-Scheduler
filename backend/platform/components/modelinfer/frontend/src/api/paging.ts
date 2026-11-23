/**
 * 列表分页契约（前端侧）。
 *
 * 与后端 `src/paging.rs` 对齐：请求 `page_num` / `page_size`，响应返回
 * `total`（**筛选后**总数）/ `pages` / `page_num` / `page_size`，各列表另有 `stats`
 * （未筛选全量聚合，供页头统计条）。见
 * `openspec/changes/modelinfer-list-pagination/design.md` §2。
 */

/** 分页响应元数据（由各列表响应展开） */
export interface PageMeta {
  /** 筛选后总条数 */
  total: number;
  /** 总页数（最小 1） */
  pages: number;
  page_num: number;
  page_size: number;
}

/** 分页请求参数（可选） */
export interface PageQuery {
  page_num?: number;
  page_size?: number;
}

/** 表格类默认页大小（与后端一致） */
export const DEFAULT_PAGE_SIZE = 20;
/** 卡片网格类默认页大小（与后端一致） */
export const GRID_PAGE_SIZE = 24;
/** 页大小可选项 */
export const PAGE_SIZE_OPTIONS = ['10', '20', '24', '50', '100'];

type QueryValue = string | number | boolean | undefined | null;

/**
 * 拼接查询串：跳过 `undefined` / `null` / 空字符串；布尔按 `true` / `false` 输出。
 * 返回 `''` 或 `'?a=1&b=2'`。
 */
export function buildQuery(params: Record<string, QueryValue>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null) continue;
    if (typeof value === 'string' && value.trim() === '') continue;
    if (typeof value === 'boolean') {
      search.set(key, value ? 'true' : 'false');
      continue;
    }
    search.set(key, String(value));
  }
  const s = search.toString();
  return s ? `?${s}` : '';
}