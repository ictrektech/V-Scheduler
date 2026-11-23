<script setup lang="ts">
/**
 * 通用指标工具栏（时间区间 / 自定义区间 / 栅格密度 / 自动刷新 / 更新于 / 刷新）。
 * 供 `MetricsBoard` 主体与放大弹窗复用；状态由父级持有（v-model 语义通过 emits 回传）。
 */
import { computed } from 'vue';
import type { Dayjs } from 'dayjs';
import { useI18n } from 'vue-i18n';
import { ReloadOutlined } from '@ant-design/icons-vue';
import { AUTO_REFRESH_OPTIONS, RANGE_OPTIONS, formatClock } from '@/utils/metrics';

const props = withDefaults(
  defineProps<{
    rangeMode: string;
    customRange: [Dayjs, Dayjs] | null;
    autoRefresh: boolean;
    refreshSecs: number;
    wide: boolean;
    lastUpdated?: number | null;
    loading?: boolean;
    /** 是否显示栅格密度切换（放大弹窗中通常隐藏） */
    showDensity?: boolean;
  }>(),
  { lastUpdated: null, loading: false, showDensity: true },
);

const emit = defineEmits<{
  (e: 'update:rangeMode', v: string): void;
  (e: 'update:customRange', v: [Dayjs, Dayjs] | null): void;
  (e: 'update:autoRefresh', v: boolean): void;
  (e: 'update:refreshSecs', v: number): void;
  (e: 'update:wide', v: boolean): void;
  (e: 'refresh'): void;
}>();

const { t } = useI18n();

const range = computed({ get: () => props.rangeMode, set: (v: string) => emit('update:rangeMode', v) });
const custom = computed({
  get: () => props.customRange,
  set: (v: [Dayjs, Dayjs] | null) => emit('update:customRange', v),
});
const auto = computed({ get: () => props.autoRefresh, set: (v: boolean) => emit('update:autoRefresh', v) });
const secs = computed({ get: () => props.refreshSecs, set: (v: number) => emit('update:refreshSecs', v) });
const wide = computed({ get: () => props.wide, set: (v: boolean) => emit('update:wide', v) });

const rangeOptions = computed(() => [
  ...RANGE_OPTIONS.map((o) => ({ label: t(o.labelKey), value: o.value })),
  { label: t('metrics.toolbar.custom'), value: 'custom' },
]);
const autoRefreshOptions = computed(() =>
  AUTO_REFRESH_OPTIONS.map((o) => ({ label: t(o.labelKey), value: o.seconds })),
);
</script>

<template>
  <a-space :size="8" wrap>
    <a-select v-model:value="range" size="small" style="width: 130px" :options="rangeOptions" />
    <a-range-picker
      v-if="range === 'custom'"
      v-model:value="custom"
      size="small"
      show-time
      style="width: 300px"
    />
    <a-radio-group v-if="showDensity" v-model:value="wide" size="small" button-style="solid">
      <a-radio-button :value="false">{{ t('metrics.toolbar.twoCol') }}</a-radio-button>
      <a-radio-button :value="true">{{ t('metrics.toolbar.oneCol') }}</a-radio-button>
    </a-radio-group>
    <a-tooltip
      :title="range === 'custom' ? t('metrics.toolbar.autoRefreshDisabled') : t('metrics.toolbar.autoRefreshHint')"
    >
      <span>
        <a-switch v-model:checked="auto" size="small" :disabled="range === 'custom'" />
        <span class="mi-hint" style="margin-left: 6px">{{ t('metrics.toolbar.autoRefreshLabel') }}</span>
      </span>
    </a-tooltip>
    <a-select
      v-if="auto"
      v-model:value="secs"
      size="small"
      style="width: 100px"
      :options="autoRefreshOptions"
    />
    <span v-if="lastUpdated" class="mi-hint">{{ t('metrics.toolbar.updatedAt', { time: formatClock(lastUpdated) }) }}</span>
    <a-button size="small" :loading="loading" @click="emit('refresh')">
      <ReloadOutlined />{{ t('common.refresh') }}
    </a-button>
  </a-space>
</template>