<script setup lang="ts">
/** 监控部署失败时的 image-hub 镜像指引：展示镜像完整名称并提供下载入口。 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import type { NodeMonitor } from '@/api/nodes';
import { openResourceHub } from '@/platform/resource-hub';
import { monitorImageHubGuidance } from '@/utils/monitorError';

const props = defineProps<{ monitor: NodeMonitor }>();
const { t } = useI18n();

const guidance = computed(() => monitorImageHubGuidance(props.monitor));
const image = computed(() => guidance.value?.image || t('nodeDetail.monitor.unknownImage'));
const hint = computed(() =>
  guidance.value
    ? t(`nodeDetail.monitor.imageHub.${guidance.value.issue}`, { image: image.value })
    : '',
);
</script>

<template>
  <div v-if="guidance" class="mi-image-hub-hint">
    <span class="mi-image-hub-hint__text">{{ hint }}</span>
    <a-button type="link" size="small" @click="openResourceHub('images')">
      {{ t('nodeDetail.monitor.openImageHub') }}
    </a-button>
  </div>
</template>

<style scoped>
.mi-image-hub-hint {
  display: flex;
  align-items: flex-start;
  flex-wrap: wrap;
  gap: 4px 8px;
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px dashed var(--mi-border);
}
.mi-image-hub-hint__text {
  color: var(--mi-text);
  line-height: 22px;
}
</style>
