<script setup lang="ts">
/**
 * Grafana 风格时序面板（echarts 封装）：
 * - 折线（单序列加轻微面积、多序列不加）；
 * - 图下**自定义图例**：色点 + 名称 + 当前值，点击切换显隐；
 * - tooltip 展示该时刻全部可见序列并按单位格式化；
 * - 坐标轴量纲友好格式化；
 * - 自适应尺寸，空数据占位。
 */
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import * as echarts from 'echarts/core';
import { LineChart } from 'echarts/charts';
import { GridComponent, TooltipComponent } from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';
import { ExpandOutlined } from '@ant-design/icons-vue';
import { formatAxis, formatMetricValue, lastValue } from '@/utils/metrics';
import { useAppStore } from '@/stores/app';

echarts.use([LineChart, GridComponent, TooltipComponent, CanvasRenderer]);

export interface ChartSeries {
  name: string;
  points: [number, number][];
}

const props = withDefaults(
  defineProps<{
    title: string;
    unit?: string;
    series: ChartSeries[];
    loading?: boolean;
    height?: number;
    /** 是否显示「放大」入口（弹窗内复用时可关闭） */
    expandable?: boolean;
    /** 横轴窗口起始（epoch 秒）；与 xMax 一起让横轴对齐所选时间区间（区间前段无数据也保持完整跨度） */
    xMin?: number | null;
    xMax?: number | null;
  }>(),
  { unit: '', height: 190, expandable: true, xMin: null, xMax: null },
);

const emit = defineEmits<{ (e: 'expand'): void }>();

const { t } = useI18n();
const appStore = useAppStore();

/** 图表坐标轴 / 网格颜色随主题切换（暗色下用浅灰，避免不可读） */
const axisColor = computed(() => (appStore.isDark ? '#8b8f99' : '#8c8c8c'));
const axisLineColor = computed(() => (appStore.isDark ? '#3a3c45' : '#e8e8e8'));
const splitLineColor = computed(() => (appStore.isDark ? '#2b2d36' : '#f0f0f0'));

/** Grafana 常用配色 */
const PALETTE = [
  '#73BF69',
  '#5794F2',
  '#F2CC0C',
  '#FF9830',
  '#FF7383',
  '#B877D9',
  '#37872D',
  '#8AB8FF',
  '#FADE2A',
  '#E0752D',
];

const el = ref<HTMLDivElement | null>(null);
const chart = shallowRef<echarts.ECharts | null>(null);
const hidden = ref<Set<string>>(new Set());

const hasData = computed(() => props.series.some((s) => s.points.length > 0));

/** 序列名 → 固定颜色（按原始顺序，切换显隐不变色） */
function colorOf(name: string): string {
  const idx = props.series.findIndex((s) => s.name === name);
  return PALETTE[(idx < 0 ? 0 : idx) % PALETTE.length];
}

const visible = computed(() => props.series.filter((s) => !hidden.value.has(s.name)));

function toggle(name: string) {
  const next = new Set(hidden.value);
  if (next.has(name)) next.delete(name);
  else next.add(name);
  hidden.value = next;
}

function toOption() {
  const shown = visible.value;
  const single = shown.length === 1;
  return {
    animation: false,
    grid: { left: 58, right: 14, top: 10, bottom: 10 },
    tooltip: {
      trigger: 'axis',
      valueFormatter: (v: unknown) => formatMetricValue(typeof v === 'number' ? v : Number(v), props.unit),
    },
    xAxis: {
      type: 'time',
      // 对齐所选时间区间：即使区间前段无数据，横轴也保持完整跨度（epoch 秒 → 毫秒）
      min: props.xMin != null ? props.xMin * 1000 : undefined,
      max: props.xMax != null ? props.xMax * 1000 : undefined,
      axisLabel: { hideOverlap: true, color: axisColor.value, fontSize: 11 },
      axisLine: { lineStyle: { color: axisLineColor.value } },
    },
    yAxis: {
      type: 'value',
      scale: true,
      axisLabel: {
        formatter: (v: number) => formatAxis(v, props.unit),
        color: axisColor.value,
        fontSize: 11,
      },
      splitLine: { lineStyle: { color: splitLineColor.value } },
    },
    series: shown.map((s) => ({
      name: s.name,
      type: 'line',
      showSymbol: false,
      smooth: false,
      lineStyle: { width: 1.6, color: colorOf(s.name) },
      itemStyle: { color: colorOf(s.name) },
      areaStyle: single ? { opacity: 0.12 } : undefined,
      connectNulls: true,
      data: s.points.map(([ts, v]) => [ts * 1000, Number.isFinite(v) ? v : null]),
    })),
  };
}

function render() {
  chart.value?.setOption(toOption(), true);
}

let resizeObserver: ResizeObserver | null = null;

onMounted(() => {
  if (el.value) {
    chart.value = echarts.init(el.value);
    render();
    resizeObserver = new ResizeObserver(() => chart.value?.resize());
    resizeObserver.observe(el.value);
  }
});

// 序列变化（区间切换 / 刷新）→ 重置显隐并重绘
watch(
  () => props.series,
  () => {
    hidden.value = new Set();
    render();
  },
  { deep: true },
);
watch(hidden, render);
watch(() => [props.xMin, props.xMax], render);
watch(() => props.height, () => chart.value?.resize());
// 主题切换 → 重绘坐标轴 / 网格颜色
watch(() => appStore.isDark, render);

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  chart.value?.dispose();
  chart.value = null;
});
</script>

<template>
  <div class="mi-panel">
    <div class="mi-panel__head">
      <span class="mi-panel__title">{{ title }}</span>
      <span class="mi-panel__tools">
        <span v-if="unit" class="mi-panel__unit">{{ unit }}</span>
        <button
          v-if="expandable"
          type="button"
          class="mi-panel__expand"
          :title="t('metrics.chart.expand')"
          @click="emit('expand')"
        >
          <ExpandOutlined />
        </button>
      </span>
    </div>
    <div ref="el" class="mi-panel__plot" :style="{ height: `${height}px` }" />
    <div v-if="!hasData" class="mi-panel__empty">{{ t('metrics.chart.noData') }}</div>
    <div v-if="series.length" class="mi-panel__legend">
      <button
        v-for="s in series"
        :key="s.name"
        type="button"
        class="mi-legend"
        :class="{ 'mi-legend--off': hidden.has(s.name) }"
        :title="hidden.has(s.name) ? t('metrics.chart.clickShow') : t('metrics.chart.clickHide')"
        @click="toggle(s.name)"
      >
        <span class="mi-legend__dot" :style="{ background: colorOf(s.name) }" />
        <span class="mi-legend__name">{{ s.name }}</span>
        <span class="mi-legend__value">{{ formatMetricValue(lastValue(s.points), unit) }}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.mi-panel {
  position: relative;
  border: 1px solid var(--mi-border);
  border-radius: 8px;
  background: var(--mi-surface);
  padding: 10px 12px 8px;
}
.mi-panel__head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  margin-bottom: 2px;
}
.mi-panel__title {
  font-size: 13px;
  font-weight: 600;
  color: var(--mi-text);
}
.mi-panel__tools {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.mi-panel__unit {
  font-size: var(--mi-font-hint);
  color: var(--mi-text-secondary);
}
.mi-panel__expand {
  border: none;
  background: transparent;
  padding: 0 2px;
  cursor: pointer;
  color: var(--mi-text-secondary);
  font-size: 13px;
  line-height: 1;
}
.mi-panel__expand:hover {
  color: var(--mi-primary);
}
.mi-panel__plot {
  width: 100%;
}
.mi-panel__empty {
  position: absolute;
  left: 0;
  right: 0;
  top: 50px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-hint);
  pointer-events: none;
}
.mi-panel__legend {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 14px;
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px dashed var(--mi-border);
  max-height: 56px;
  overflow-y: auto;
}
.mi-legend {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 0;
  border: none;
  background: transparent;
  cursor: pointer;
  font-size: var(--mi-font-hint);
  color: var(--mi-text-secondary);
  line-height: 18px;
}
.mi-legend--off {
  opacity: 0.4;
  text-decoration: line-through;
}
.mi-legend__dot {
  width: 8px;
  height: 8px;
  border-radius: 2px;
  flex: none;
}
.mi-legend__name {
  color: var(--mi-text);
}
.mi-legend__value {
  font-variant-numeric: tabular-nums;
  color: var(--mi-text-secondary);
}
</style>