<script setup lang="ts">
/**
 * 模型家族图标头像：品牌渐变底 + 居中家族图标。
 *
 * 家族标识由**后端**推断并随列表返回（见 `openspec/changes/modelinfer-list-pagination`）；
 * 本组件只做呈现，未知家族回落 `generic` 通用图标与中性色。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { hexToRgba, modelFamilyIcon, modelFamilyLabel, modelFamilyTheme } from '@/utils/modelFamily';
import { modelIconUrl } from '@/utils/modelIconAssets';

const { t } = useI18n();

const props = withDefaults(defineProps<{ family?: string | null; size?: number }>(), {
  family: null,
  size: 60,
});

const theme = computed(() => modelFamilyTheme(props.family));
const label = computed(() => modelFamilyLabel(props.family, (k) => t(k)));

const iconUrl = computed(() => modelIconUrl(modelFamilyIcon(props.family)));

const boxStyle = computed(() => {
  const accent = theme.value.accent;
  const size = props.size;
  return {
    width: `${size}px`,
    height: `${size}px`,
    background: `linear-gradient(135deg, ${hexToRgba(accent, 0.18)} 0%, ${hexToRgba(accent, 0.05)} 100%)`,
    borderColor: hexToRgba(accent, 0.24),
  };
});

const imgStyle = computed(() => {
  const inner = Math.round(props.size * 0.54);
  return { width: `${inner}px`, height: `${inner}px` };
});
</script>

<template>
  <div class="mi-model-avatar" :style="boxStyle" :title="label">
    <img class="mi-model-avatar__img" :src="iconUrl" :alt="label" :style="imgStyle" />
  </div>
</template>

<style scoped>
.mi-model-avatar {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  border-radius: var(--mi-radius-lg);
  border: 1px solid transparent;
}
.mi-model-avatar__img {
  display: block;
  object-fit: contain;
}
</style>