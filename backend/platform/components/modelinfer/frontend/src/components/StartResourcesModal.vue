<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import CopyText from './CopyText.vue';
import { openResourceHub } from '@/platform/resource-hub';
import type { MissingStartResources } from '@/utils/startResources';

defineProps<{
  open: boolean;
  name: string;
  resources: MissingStartResources | null;
  checking: boolean;
}>();
const emit = defineEmits<{
  'update:open': [open: boolean];
  retry: [];
}>();
const { t } = useI18n();
</script>

<template>
  <a-modal
    :open="open"
    :title="t('runs.resources.title', { name })"
    :width="680"
    :mask-closable="false"
    @cancel="emit('update:open', false)"
  >
    <a-alert type="warning" show-icon :message="t('runs.resources.description')" />
    <div v-if="resources" class="mi-start-resources">
      <section v-if="resources.weights.length">
        <h4>{{ t('runs.resources.weights') }}</h4>
        <p class="mi-hint">{{ t('runs.resources.weightHint') }}</p>
        <div v-for="weight in resources.weights" :key="`${weight.source}:${weight.model_id}`" class="mi-resource">
          <div class="mi-resource__labels">
            <a-tag>{{ weight.source === 'ms' ? 'ModelScope' : 'Hugging Face' }}</a-tag>
            <a-tag v-if="weight.overridden" color="orange">{{ t('runs.overridden') }}</a-tag>
          </div>
          <CopyText :text="weight.model_id" />
        </div>
        <a-button @click="openResourceHub('weights')">{{ t('runs.resources.openWeights') }}</a-button>
      </section>
      <section v-if="resources.images.length">
        <h4>
          {{ t('runs.resources.images') }}
          <a-tag v-if="resources.images_source === 'override'" color="orange">{{ t('runs.overridden') }}</a-tag>
        </h4>
        <p class="mi-hint">{{ t('runs.resources.imageHint') }}</p>
        <div v-for="image in resources.images" :key="image" class="mi-resource">
          <CopyText :text="image" />
        </div>
        <a-button @click="openResourceHub('images')">{{ t('runs.resources.openImages') }}</a-button>
      </section>
    </div>
    <template #footer>
      <a-button @click="emit('update:open', false)">{{ t('common.close') }}</a-button>
      <a-button type="primary" :loading="checking" @click="emit('retry')">
        {{ t('runs.resources.retry') }}
      </a-button>
    </template>
  </a-modal>
</template>

<style scoped>
.mi-start-resources { display: grid; gap: 20px; margin-top: 20px; max-height: 55vh; overflow-y: auto; }
.mi-start-resources h4 { color: var(--mi-text); margin: 0 0 8px; }
.mi-resource { margin: 10px 0; padding: 10px 12px; background: var(--mi-surface-2); border-radius: 6px; }
.mi-resource__labels { margin-bottom: 6px; }
.mi-resource :deep(.mi-copy__btn) { opacity: 1; }
</style>
