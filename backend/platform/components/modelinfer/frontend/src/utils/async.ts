/**
 * 列表页与服务端分页配套的小工具（去抖 + 过期响应丢弃）。
 *
 * 见 `openspec/changes/modelinfer-list-pagination/design.md` §8：关键词输入去抖后请求、
 * 筛选 / 排序变更重置页码、竞态时丢弃过期响应。
 */

/** 去抖包装（带 `cancel`，组件卸载时可清理） */
export function debounce<A extends unknown[]>(fn: (...args: A) => void, ms = 300) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const wrapped = (...args: A) => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
  wrapped.cancel = () => {
    if (timer) clearTimeout(timer);
  };
  return wrapped;
}

/**
 * 递增序号守卫：用于丢弃过期响应（避免慢响应覆盖新结果）。
 *
 * ```ts
 * const guard = createSeqGuard();
 * const seq = guard.next();
 * const data = await fetchSomething();
 * if (!guard.isCurrent(seq)) return;   // 已有更新的请求，丢弃本次结果
 * ```
 */
export function createSeqGuard() {
  let current = 0;
  return {
    /** 开始一次新请求，返回其序号 */
    next(): number {
      current += 1;
      return current;
    },
    /** 该序号是否仍是最新请求 */
    isCurrent(seq: number): boolean {
      return seq === current;
    },
  };
}