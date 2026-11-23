<script setup lang="ts">
/**
 * 通用指标板（数据驱动，可复用）。
 *
 * 由 `profile` 决定版面（分类分区 → 概览 stat 卡 + 分区趋势面板），`groups` 为后端指标组。
 * 面板**始终渲染**（即使无数据），受限设备缺少的指标以空态呈现，版面无跳变。
 * 工具栏（区间 / 自动刷新 / 密度）、面板放大弹窗均内置；节点详情与后续页面复用同一组件。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import type { Dayjs } from 'dayjs';
import { useI18n } from 'vue-i18n';
import MetricChart from './MetricChart.vue';
import MetricStat from './MetricStat.vue';
import MetricsToolbar from './MetricsToolbar.vue';
import { buildBoard, type BoardPanel, type MetricsProfile } from '@/utils/metrics';
import type { MetricGroup } from '@/api/nodes';

const props = withDefaults(
  defineProps<{
    profile: MetricsProfile;
    groups: MetricGroup[];
    loading?: boolean;
    lastUpdated?: number | null;
    rangeMode: string;
    customRange: [Dayjs, Dayjs] | null;
    autoRefresh: boolean;
    refreshSecs: number;
    wide: boolean;
    /** 查询窗口起始（epoch 秒），用于横轴对齐所选区间 */
    windowStart?: number | null;
    /** 查询窗口终点（epoch 秒） */
    windowEnd?: number | null;
  }>(),
  { loading: false, lastUpdated: null, windowStart: null, windowEnd: null },
);

const emit = defineEmits<{
  (e: 'update:rangeMode', v: string): void;
  (e: 'update:customRange', v: [Dayjs, Dayjs] | null): void;
  (e: 'update:autoRefresh', v: boolean): void;
  (e: 'update:refreshSecs', v: number): void;
  (e: 'update:wide', v: boolean): void;
  (e: 'refresh'): void;
}>();

const { t, locale } = useI18n();

/** 构建后的分类分区（语言切换时随 locale 重算） */
const board = computed(() => {
  // 显式读取 locale 建立响应式依赖，保证切换语言后标题重算
  void locale.value;
  return buildBoard(props.groups, props.profile, (k) => t(k));
});
const windowStart = computed(() => props.windowStart ?? null);
const windowEnd = computed(() => props.windowEnd ?? null);

/** 放大的面板 key（弹窗查看）：只存 key，由最新 board 派生，保证切区间/刷新同步 */
const expandedKey = ref<string | null>(null);
const expanded = computed<BoardPanel | null>(() => {
  for (const c of board.value) {
    const p = c.panels.find((x) => x.key === expandedKey.value);
    if (p) return p;
  }
  return null;
});

/** 视口高度（放大弹窗按 56vh 计算大图高度） */
const viewportH = ref(typeof window !== 'undefined' ? window.innerHeight : 800);
const bigChartHeight = computed(() => Math.max(360, Math.round(viewportH.value * 0.56)));
function onViewportResize() {
  viewportH.value = window.innerHeight;
}

onMounted(() => window.addEventListener('resize', onViewportResize));
onBeforeUnmount(() => window.removeEventListener('resize', onViewportResize));
</script>

<template>
  <div class="mi-board">
    <div class="mi-board__toolbar">
      <MetricsToolbar
        :range-mode="rangeMode"
        :custom-range="customRange"
        :auto-refresh="autoRefresh"
        :refresh-secs="refreshSecs"
        :wide="wide"
        :last-updated="lastUpdated"
        :loading="loading"
        @update:range-mode="emit('update:rangeMode', $event)"
        @update:custom-range="emit('update:customRange', $event)"
        @update:auto-refresh="emit('update:autoRefresh', $event)"
        @update:refresh-secs="emit('update:refreshSecs', $event)"
        @update:wide="emit('update:wide', $event)"
        @refresh="emit('refresh')"
      />
    </div>

    <a-spin :spinning="loading">
      <div v-for="c in board" :key="c.key" class="mi-cat">
        <div class="mi-cat__title">{{ c.title }}</div>
        <div class="mi-stat-grid">
          <MetricStat
            v-for="s in c.stats"
            :key="s.key"
            :title="s.title"
            :value="s.value"
            :unit="s.unit"
            :points="s.points"
            :note="s.note"
          />
        </div>
        <div class="mi-panel-grid" :class="{ 'mi-panel-grid--wide': wide }">
          <MetricChart
            v-for="p in c.panels"
            :key="p.key"
            :title="p.title"
            :unit="p.unit"
            :series="p.series"
            :x-min="windowStart"
            :x-max="windowEnd"
            @expand="expandedKey = p.key"
          />
        </div>
      </div>
    </a-spin>

    <!-- 面板放大弹窗（复用同一工具栏；内部图表随区间/刷新同步更新） -->
    <a-modal
      :open="expandedKey !== null"
      :title="expanded ? `${expanded.title} · ${t('metrics.chart.expand')}` : t('metrics.chart.expand')"
      width="92%"
      :footer="null"
      @cancel="expandedKey = null"
    >
      <div style="margin-bottom: 12px">
        <MetricsToolbar
          :range-mode="rangeMode"
          :custom-range="customRange"
          :auto-refresh="autoRefresh"
          :refresh-secs="refreshSecs"
          :wide="wide"
          :last-updated="lastUpdated"
          :loading="loading"
          :show-density="false"
          @update:range-mode="emit('update:rangeMode', $event)"
          @update:custom-range="emit('update:customRange', $event)"
          @update:auto-refresh="emit('update:autoRefresh', $event)"
          @update:refresh-secs="emit('update:refreshSecs', $event)"
          @update:wide="emit('update:wide', $event)"
          @refresh="emit('refresh')"
        />
      </div>
      <MetricChart
        v-if="expanded"
        :title="expanded.title"
        :unit="expanded.unit"
        :series="expanded.series"
        :height="bigChartHeight"
        :expandable="false"
        :x-min="windowStart"
        :x-max="windowEnd"
      />
      <a-empty v-else :description="t('metrics.chart.noDataInRange')" />
    </a-modal>
  </div>
</template>

<style scoped>
.mi-board__toolbar {
  display: flex;
  justify-content: flex-end;
  margin-bottom: 12px;
}
.mi-cat + .mi-cat {
  margin-top: 20px;
  padding-top: 16px;
  border-top: 1px dashed var(--mi-border);
}
.mi-cat__title {
  font-size: 14px;
  font-weight: 600;
  color: var(--mi-text);
  margin-bottom: 10px;
}
.mi-stat-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
  gap: 12px;
  margin-bottom: 12px;
}
.mi-panel-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(420px, 1fr));
  gap: 12px;
}
.mi-panel-grid--wide {
  grid-template-columns: 1fr;
}
</style>