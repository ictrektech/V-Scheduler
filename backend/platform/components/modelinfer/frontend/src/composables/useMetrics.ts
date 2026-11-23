/**
 * 指标取数组合式函数（可复用）。
 *
 * 集中管理「时间区间 / 自定义区间 / 自动刷新 / 静默刷新 / 窗口 start-end / 加载态 / 栅格密度」，
 * fetcher 由调用方注入（返回 `{ groups, start, end }`）。节点详情与后续运行配置详情等页面复用。
 */
import { onBeforeUnmount, ref, watch } from 'vue';
import type { Dayjs } from 'dayjs';
import { DEFAULT_AUTO_REFRESH_SECS } from '@/utils/metrics';
import { translate } from '@/i18n';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import type { MetricGroup, MetricsQuery, MetricsResponse } from '@/api/nodes';

export type MetricsFetcher = (params: MetricsQuery) => Promise<MetricsResponse>;

export function useMetrics(fetcher: MetricsFetcher, onError?: (msg: string) => void) {
  const loading = ref(false);
  const groups = ref<MetricGroup[]>([]);
  /** 指标查询窗口（epoch 秒），来自接口返回的 start/end；用于横轴对齐所选区间 */
  const windowStart = ref<number | null>(null);
  const windowEnd = ref<number | null>(null);
  const rangeMode = ref('1h');
  const customRange = ref<[Dayjs, Dayjs] | null>(null);
  /** 自动刷新（默认关）；仅对滚动区间生效 */
  const autoRefresh = ref(false);
  const refreshSecs = ref(DEFAULT_AUTO_REFRESH_SECS);
  const lastUpdated = ref<number | null>(null);
  /** 面板栅格密度：false 两列 / true 单列全宽 */
  const wide = ref(false);

  let refreshTimer: ReturnType<typeof setInterval> | null = null;
  let inFlight = false;

  /**
   * 加载指标。`silent = true`（自动刷新）时不显示加载遮罩、不重复发起、失败不弹提示。
   */
  async function load(silent = false) {
    if (silent && inFlight) return;
    const params: MetricsQuery = {};
    if (rangeMode.value === 'custom') {
      if (!customRange.value) {
        if (!silent) onError?.(translate('metrics.customRangeRequired'));
        return;
      }
      params.start = customRange.value[0].unix();
      params.end = customRange.value[1].unix();
    } else {
      params.range = rangeMode.value;
    }
    inFlight = true;
    if (!silent) loading.value = true;
    try {
      const data = await fetcher(params);
      groups.value = data.groups;
      windowStart.value = data.start;
      windowEnd.value = data.end;
      lastUpdated.value = Math.floor(Date.now() / 1000);
    } catch (e) {
      if (!silent) onError?.(resolveApiErrorMessage(e));
    } finally {
      inFlight = false;
      if (!silent) loading.value = false;
    }
  }

  /** 重建自动刷新定时器（关闭 / 自定义区间 / 卸载时停止） */
  function manageAutoRefresh() {
    if (refreshTimer) {
      clearInterval(refreshTimer);
      refreshTimer = null;
    }
    if (autoRefresh.value && rangeMode.value !== 'custom') {
      refreshTimer = setInterval(() => {
        void load(true);
      }, refreshSecs.value * 1000);
    }
  }

  watch([rangeMode, customRange], () => {
    if (rangeMode.value !== 'custom' || customRange.value) void load();
  });
  // 自定义固定区间不支持自动刷新
  watch(rangeMode, (v) => {
    if (v === 'custom' && autoRefresh.value) autoRefresh.value = false;
  });
  watch([autoRefresh, refreshSecs, rangeMode], manageAutoRefresh);

  onBeforeUnmount(() => {
    if (refreshTimer) clearInterval(refreshTimer);
  });

  return {
    loading,
    groups,
    windowStart,
    windowEnd,
    rangeMode,
    customRange,
    autoRefresh,
    refreshSecs,
    lastUpdated,
    wide,
    load,
    manageAutoRefresh,
  };
}