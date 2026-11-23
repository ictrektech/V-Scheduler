<script setup lang="ts">
/**
 * 运行历史紧凑列表（左列）。
 *
 * 运行配置详情页「运行与步骤」的左栏：以紧凑行展示 run，整行可点击选中；
 * 摘要单行截断（完整摘要经悬浮提示可得），不重复渲染错误全文；运行中 run 展示当前步骤。
 * 纯呈现组件：筛选选项与选中态由父级传入，选择经 `select` 事件回传。
 */
import { useI18n } from 'vue-i18n';
import StatusTag from '@/components/StatusTag.vue';
import TimeText from '@/components/TimeText.vue';
import { formatDuration } from '@/utils/format';
import type { RunItem } from '@/api/observability';

const { t } = useI18n();

const props = defineProps<{
  runs: RunItem[];
  selectedId: string;
  filter: string;
  filterOptions: Array<{ label: string; value: string }>;
}>();

const emit = defineEmits<{
  select: [id: string];
  'update:filter': [value: string];
}>();

/** 结束为空且未完成时显示「进行中」 */
function running(r: RunItem): boolean {
  return !r.finished_at;
}
</script>

<template>
  <div class="mi-rl">
    <div class="mi-rl__head">
      <a-select
        :value="props.filter"
        size="small"
        style="width: 100%"
        :options="props.filterOptions"
        @change="(v: any) => emit('update:filter', String(v))"
      />
    </div>
    <div class="mi-rl__list">
      <div
        v-for="r in props.runs"
        :key="r.id"
        class="mi-rl__item"
        :class="{ 'mi-rl__item--active': r.id === props.selectedId }"
        role="button"
        tabindex="0"
        @click="emit('select', r.id)"
        @keydown.enter="emit('select', r.id)"
        @keydown.space.prevent="emit('select', r.id)"
      >
        <div class="mi-rl__line1">
          <StatusTag kind="runKind" :status="r.kind" />
          <StatusTag kind="run" :status="r.state" />
          <span class="mi-rl__dur mi-mono">{{ formatDuration(r.started_at, r.finished_at) }}</span>
        </div>
        <div class="mi-rl__line2">
          <TimeText :value="r.started_at" />
          <span v-if="running(r)" class="mi-hint">· {{ t('runDetail.history.runningNow') }}</span>
        </div>
        <div class="mi-rl__summary" :title="r.summary || r.error || ''">
          {{ r.summary || r.error || '-' }}
        </div>
        <div v-if="r.active_step" class="mi-rl__active">
          {{ t('runDetail.history.currentPrefix') }}{{ r.active_step.title || r.active_step.step }}
        </div>
      </div>
      <a-empty
        v-if="!props.runs.length"
        :description="props.filter === 'all' ? t('runDetail.history.noRuns') : t('runDetail.history.noRunsByType')"
      />
    </div>
  </div>
</template>

<style scoped>
.mi-rl {
  display: flex;
  flex-direction: column;
  min-height: 0;
}
.mi-rl__head {
  margin-bottom: 8px;
}
.mi-rl__list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: calc(100vh - 340px);
  overflow-y: auto;
  padding-right: 2px;
}
.mi-rl__item {
  border: 1px solid var(--mi-border);
  border-radius: var(--mi-radius-md);
  background: var(--mi-surface);
  padding: 8px 10px;
  cursor: pointer;
  transition:
    background 0.15s,
    border-color 0.15s,
    box-shadow 0.15s;
}
.mi-rl__item:hover {
  border-color: var(--mi-primary);
}
.mi-rl__item--active {
  background: var(--mi-surface-2);
  box-shadow: inset 3px 0 0 var(--mi-primary);
  border-color: var(--mi-primary);
}
.mi-rl__line1 {
  display: flex;
  align-items: center;
  gap: 6px;
}
.mi-rl__dur {
  margin-left: auto;
  color: var(--mi-text-secondary);
  font-size: 12px;
}
.mi-rl__line2 {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-top: 5px;
  color: var(--mi-text-secondary);
  font-size: 12px;
}
.mi-rl__summary {
  margin-top: 4px;
  color: var(--mi-text-secondary);
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.mi-rl__active {
  margin-top: 4px;
  color: var(--mi-processing);
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 窄宽回退：取消限高，随页面自然排列 */
@media (max-width: 960px) {
  .mi-rl__list {
    max-height: none;
  }
}
</style>