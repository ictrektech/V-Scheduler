<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import { fetchGcStatus, runGc, saveGcConfig, type GcStatus } from '@/api/imageHub';
import { hasTranslation } from '@/i18n';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import { formatBytes, formatTime } from '@/utils/format';
import {
  GC_DEFAULT_INTERVAL_HOURS,
  GC_INTERVAL_PRESETS,
  gcIntervalParts,
  isValidGcInterval,
  nextRunKey,
} from '@/utils/gc';

/**
 * 垃圾回收弹窗：
 * - 展示 registry 存储占用与 GC 能力（不可用时禁用触发）
 * - 自动回收配置（默认关闭；开关 + 间隔 + 是否删除无标签 manifest）
 * - 「立即回收」（可选预演 / 删除无标签 manifest），发起异步任务
 */
const { t } = useI18n();

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ 'update:open': [boolean]; started: [taskId: string] }>();

const visible = computed({
  get: () => props.open,
  set: (v: boolean) => emit('update:open', v),
});

const loading = ref(false);
const saving = ref(false);
const running = ref(false);
const status = ref<GcStatus | null>(null);

/** 自动回收表单 */
const enabled = ref(false);
const intervalHours = ref<number>(GC_DEFAULT_INTERVAL_HOURS);
const autoDeleteUntagged = ref(true);

/** 手动回收表单 */
const dryRun = ref(false);
const manualDeleteUntagged = ref(true);

const intervalOptions = computed(() =>
  GC_INTERVAL_PRESETS.map((p) => ({ value: p.value, label: t(p.labelKey) })),
);

/** 当前间隔本地化文案（天 / 小时） */
const currentIntervalText = computed(() => {
  const parts = gcIntervalParts(intervalHours.value);
  return t(`gc.interval.${parts.unit}`, { count: parts.value });
});

/** 下次自动回收文案 */
const nextRunText = computed(() => {
  const key = nextRunKey(status.value?.next_run_at, status.value?.enabled ?? false);
  return key === 'gc.nextRunText.at'
    ? t(key, { time: formatTime(status.value?.next_run_at) })
    : t(key);
});

/** GC 不可用原因（优先稳定码，缺失回退后端原文） */
const unavailableText = computed(() => {
  const code = status.value?.unavailable_code;
  if (code && hasTranslation(`gc.unavailable.${code}`)) return t(`gc.unavailable.${code}`);
  return status.value?.unavailable_reason || t('gc.unavailable.fallback');
});

async function load(): Promise<void> {
  loading.value = true;
  try {
    const data = await fetchGcStatus();
    status.value = data;
    enabled.value = data.enabled;
    intervalHours.value = isValidGcInterval(data.interval_hours)
      ? data.interval_hours
      : GC_DEFAULT_INTERVAL_HOURS;
    autoDeleteUntagged.value = data.delete_untagged;
    manualDeleteUntagged.value = data.delete_untagged;
  } catch (e) {
    message.error(t('gc.messages.loadFailed', { error: resolveApiErrorMessage(e) }));
  } finally {
    loading.value = false;
  }
}

watch(
  () => props.open,
  (open) => {
    if (open) {
      dryRun.value = false;
      void load();
    }
  },
);

async function saveConfig(): Promise<void> {
  if (!isValidGcInterval(intervalHours.value)) {
    message.error(t('gc.messages.invalidInterval'));
    return;
  }
  saving.value = true;
  try {
    const data = await saveGcConfig({
      enabled: enabled.value,
      interval_hours: intervalHours.value,
      delete_untagged: autoDeleteUntagged.value,
    });
    status.value = data;
    manualDeleteUntagged.value = data.delete_untagged;
    message.success(
      enabled.value ? t('gc.messages.savedEnabled') : t('gc.messages.savedDisabled'),
    );
  } catch (e) {
    message.error(t('gc.messages.saveFailed', { error: resolveApiErrorMessage(e) }));
  } finally {
    saving.value = false;
  }
}

async function triggerNow(): Promise<void> {
  running.value = true;
  try {
    const res = await runGc({
      dryRun: dryRun.value,
      deleteUntagged: manualDeleteUntagged.value,
    });
    message.success(
      dryRun.value ? t('gc.messages.startedDry') : t('gc.messages.started'),
    );
    emit('started', res.task_id);
    visible.value = false;
  } catch (e) {
    message.error(t('gc.messages.triggerFailed', { error: resolveApiErrorMessage(e) }));
  } finally {
    running.value = false;
  }
}

const storageText = computed(() => formatBytes(status.value?.storage_bytes ?? null));
const available = computed(() => status.value?.available ?? false);
const busy = computed(() => !!status.value?.running_task_id);
</script>

<template>
  <a-modal v-model:open="visible" :title="t('gc.title')" width="720px" :footer="null">
    <a-spin :spinning="loading">
      <a-alert
        v-if="status && !available"
        type="warning"
        show-icon
        style="margin-bottom: 16px"
        :message="t('gc.unavailable.title')"
        :description="unavailableText"
      />
      <a-alert
        v-else-if="busy"
        type="info"
        show-icon
        style="margin-bottom: 16px"
        :message="t('gc.busy')"
      />

      <a-descriptions :column="2" size="small" bordered style="margin-bottom: 16px">
        <a-descriptions-item :label="t('gc.storage')">{{ storageText }}</a-descriptions-item>
        <a-descriptions-item :label="t('gc.storageDir')">
          <span class="mono">{{ status?.storage_dir || '-' }}</span>
        </a-descriptions-item>
        <a-descriptions-item :label="t('gc.lastRun')">
          {{ formatTime(status?.last_run_at) }}
        </a-descriptions-item>
        <a-descriptions-item :label="t('gc.nextRun')">{{ nextRunText }}</a-descriptions-item>
      </a-descriptions>

      <p class="ih-muted" style="margin-top: 0; font-size: 12px">{{ t('gc.intro') }}</p>

      <!-- 自动回收 -->
      <div class="gc-section">
        <div class="gc-title">{{ t('gc.auto.title') }}</div>
        <div class="gc-row">
          <span class="gc-label">{{ t('gc.auto.enable') }}</span>
          <a-switch v-model:checked="enabled" />
          <span class="ih-muted gc-hint">{{ t('gc.auto.enableHint') }}</span>
        </div>
        <div class="gc-row">
          <span class="gc-label">{{ t('gc.auto.interval') }}</span>
          <a-select
            v-model:value="intervalHours"
            :options="intervalOptions"
            :disabled="!enabled"
            style="width: 200px"
          />
          <a-input-number
            v-model:value="intervalHours"
            :min="1"
            :max="720"
            :disabled="!enabled"
            style="width: 140px"
            :addon-after="t('timeout.unitHour')"
          />
        </div>
        <div class="gc-row">
          <span class="gc-label">{{ t('gc.auto.deleteUntagged') }}</span>
          <a-switch v-model:checked="autoDeleteUntagged" :disabled="!enabled" />
          <span class="ih-muted gc-hint">{{ t('gc.auto.deleteUntaggedHint') }}</span>
        </div>
        <div class="gc-row">
          <span class="gc-label"></span>
          <span class="ih-muted gc-hint">
            {{ t('gc.auto.current', { interval: currentIntervalText }) }}
          </span>
        </div>
        <div class="footer">
          <a-button type="primary" :loading="saving" @click="saveConfig">
            {{ t('gc.auto.save') }}
          </a-button>
        </div>
      </div>

      <!-- 手动回收 -->
      <div class="gc-section">
        <div class="gc-title">{{ t('gc.manual.title') }}</div>
        <div class="gc-row">
          <span class="gc-label">{{ t('gc.manual.deleteUntagged') }}</span>
          <a-switch v-model:checked="manualDeleteUntagged" />
        </div>
        <div class="gc-row">
          <span class="gc-label">{{ t('gc.manual.dryRun') }}</span>
          <a-switch v-model:checked="dryRun" />
          <span class="ih-muted gc-hint">{{ t('gc.manual.dryRunHint') }}</span>
        </div>
        <div class="footer">
          <a-button
            type="primary"
            danger
            :loading="running"
            :disabled="!available || busy"
            @click="triggerNow"
          >
            {{ dryRun ? t('gc.manual.runDry') : t('gc.manual.run') }}
          </a-button>
        </div>
      </div>
    </a-spin>
  </a-modal>
</template>

<style scoped>
.gc-section {
  border-top: 1px solid var(--ih-border);
  padding-top: var(--ih-space);
  margin-top: var(--ih-space);
}
.gc-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--ih-text);
  margin-bottom: var(--ih-space);
}
.gc-row {
  display: flex;
  align-items: center;
  gap: var(--ih-space-sm);
  margin-bottom: var(--ih-space-sm);
  flex-wrap: wrap;
}
.gc-label {
  width: 108px;
  flex: none;
  font-size: 13px;
  color: var(--ih-text-secondary);
}
.gc-hint {
  font-size: 12px;
}
.mono {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  font-size: 12px;
  word-break: break-all;
}
.footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: var(--ih-space-sm);
}
</style>