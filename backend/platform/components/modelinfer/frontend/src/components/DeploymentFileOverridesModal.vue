<script setup lang="ts">
/** 运行配置级模型卡片脚本 override：列表 / 查看原文 / 编辑 / 删除。 */
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { message, Modal } from 'ant-design-vue';
import {
  DeleteOutlined,
  EditOutlined,
  FileTextOutlined,
  ReloadOutlined,
} from '@ant-design/icons-vue';
import {
  createDeploymentFileOverride,
  deleteDeploymentFileOverride,
  getDeploymentCardFileContent,
  getDeploymentFileOverride,
  listDeploymentFileOverrides,
  updateDeploymentFileOverride,
  type DeploymentScriptFile,
} from '@/api/deployments';
import { formatBytes } from '@/utils/format';
import { resolveApiErrorMessage } from '@/i18n/api-error';

const { t } = useI18n();
const props = defineProps<{
  open: boolean;
  deploymentId: string;
  deploymentName?: string;
  deploymentState?: string;
}>();
const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>();

const loading = ref(false);
const saving = ref(false);
const deleting = ref(false);
const files = ref<DeploymentScriptFile[]>([]);
const selectedPath = ref('');
const overrideId = ref('');
const originalContent = ref('');
const content = ref('');
const maxContentBytes = ref(1024 * 1024);
const maxFiles = ref(64);

const activeFile = computed(() => files.value.find((f) => f.path === selectedPath.value) || null);
const overriddenCount = computed(() => files.value.filter((f) => !!f.override_id).length);
const dirty = computed(
  () => !!activeFile.value && content.value !== originalContent.value,
);
const isRunning = computed(() =>
  ['starting', 'running', 'stopping', 'degraded'].includes(props.deploymentState || ''),
);
const contentBytes = computed(() => new TextEncoder().encode(content.value).length);

async function loadFiles(keepSelection = true) {
  loading.value = true;
  try {
    const data = await listDeploymentFileOverrides(props.deploymentId);
    files.value = data.files;
    maxContentBytes.value = data.max_content_bytes;
    maxFiles.value = data.max_files;
    if (!keepSelection || !files.value.some((f) => f.path === selectedPath.value)) {
      selectedPath.value = files.value[0]?.path || '';
    }
    if (selectedPath.value) await selectFile(selectedPath.value);
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    loading.value = false;
  }
}

async function selectFile(path: string) {
  if (saving.value) return;
  selectedPath.value = path;
  const file = files.value.find((f) => f.path === path);
  if (!file) return;
  overrideId.value = file.override_id || '';
  try {
    if (file.override_id) {
      const detail = await getDeploymentFileOverride(props.deploymentId, file.override_id);
      originalContent.value = detail.original_content;
      content.value = detail.content;
    } else {
      const original = await getDeploymentCardFileContent(props.deploymentId, file.path);
      originalContent.value = original.content;
      content.value = original.content;
    }
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  }
}

async function save() {
  if (!selectedPath.value) return;
  if (contentBytes.value > maxContentBytes.value) {
    message.warning(t('fileOverride.msg.tooLarge', { max: formatBytes(maxContentBytes.value) }));
    return;
  }
  saving.value = true;
  try {
    let detail;
    if (overrideId.value) {
      detail = await updateDeploymentFileOverride(props.deploymentId, overrideId.value, content.value);
    } else {
      detail = await createDeploymentFileOverride(props.deploymentId, {
        path: selectedPath.value,
        content: content.value,
      });
    }
    overrideId.value = detail.id;
    originalContent.value = detail.original_content;
    content.value = detail.content;
    const target = files.value.find((f) => f.path === selectedPath.value);
    if (target) {
      target.override_id = detail.id;
      target.updated_at = detail.updated_at;
      target.size_bytes = detail.size_bytes;
    }
    message.success(t('fileOverride.msg.saved'));
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    saving.value = false;
  }
}

function remove() {
  if (!overrideId.value || !selectedPath.value) return;
  Modal.confirm({
    title: t('fileOverride.delete.title'),
    content: t('fileOverride.delete.content', { path: selectedPath.value }),
    okText: t('common.delete'),
    okType: 'danger',
    cancelText: t('common.cancel'),
    async onOk() {
      deleting.value = true;
      try {
        await deleteDeploymentFileOverride(props.deploymentId, overrideId.value);
        overrideId.value = '';
        const target = files.value.find((f) => f.path === selectedPath.value);
        if (target) {
          target.override_id = '';
          target.updated_at = undefined;
        }
        await loadFiles();
        message.success(t('fileOverride.msg.deleted'));
      } finally {
        deleting.value = false;
      }
    },
  });
}

function close() {
  emit('update:open', false);
}

watch(
  () => props.open,
  (open) => {
    if (open) {
      selectedPath.value = '';
      void loadFiles(false);
    }
  },
);
</script>

<template>
  <a-modal
    :open="open"
    :title="t('fileOverride.title', { name: deploymentName || deploymentId })"
    :footer="null"
    width="1080px"
    :mask-closable="false"
    @cancel="close"
  >
    <a-alert
      type="info"
      show-icon
      :message="t('fileOverride.hint', { max: maxFiles })"
      style="margin-bottom: 12px"
    />
    <a-alert
      v-if="isRunning"
      type="warning"
      show-icon
      :message="t('fileOverride.runningHint')"
      style="margin-bottom: 12px"
    />

    <a-spin :spinning="loading">
      <div class="mi-fo">
        <aside class="mi-fo__list">
          <div class="mi-fo__list-head">
            <span>{{ t('fileOverride.files', { overridden: overriddenCount, total: files.length }) }}</span>
            <a-button size="small" type="text" @click="loadFiles()">
              <ReloadOutlined />
            </a-button>
          </div>
          <a-empty v-if="!files.length" :description="t('fileOverride.empty')" />
          <button
            v-for="file in files"
            :key="file.path"
            type="button"
            class="mi-fo__file"
            :class="{ 'mi-fo__file--active': file.path === selectedPath }"
            @click="selectFile(file.path)"
          >
            <FileTextOutlined />
            <span class="mi-fo__path" :title="file.path">{{ file.path }}</span>
            <a-tag v-if="file.override_id" color="orange">{{ t('fileOverride.overridden') }}</a-tag>
            <span class="mi-hint">{{ formatBytes(file.size_bytes) }}</span>
          </button>
        </aside>

        <section v-if="activeFile" class="mi-fo__editor">
          <div class="mi-fo__head">
            <div>
              <div class="mi-mono mi-fo__selected">{{ activeFile.path }}</div>
              <div class="mi-hint">
                {{ overrideId ? t('fileOverride.editing') : t('fileOverride.copyHint') }}
              </div>
            </div>
            <a-space>
              <a-button
                v-if="overrideId"
                danger
                :loading="deleting"
                :disabled="saving"
                @click="remove"
              >
                <DeleteOutlined />
                {{ t('fileOverride.restoreOriginal') }}
              </a-button>
              <a-button type="primary" :loading="saving" :disabled="!dirty && !!overrideId" @click="save">
                <EditOutlined />
                {{ overrideId ? t('common.save') : t('fileOverride.create') }}
              </a-button>
            </a-space>
          </div>
          <a-textarea
            v-model:value="content"
            class="mi-fo__textarea"
            :rows="20"
            spellcheck="false"
          />
          <div class="mi-fo__meta">
            <span>{{ t('fileOverride.size', { size: formatBytes(contentBytes), max: formatBytes(maxContentBytes) }) }}</span>
            <a-tag v-if="dirty" color="gold">{{ t('fileOverride.unsaved') }}</a-tag>
          </div>

          <a-collapse ghost>
            <a-collapse-panel key="original" :header="t('fileOverride.viewOriginal')">
              <pre class="mi-fo__original">{{ originalContent }}</pre>
            </a-collapse-panel>
          </a-collapse>
        </section>

        <a-empty v-else class="mi-fo__placeholder" :description="t('fileOverride.selectFile')" />
      </div>
    </a-spin>

    <div class="mi-modal-footer">
      <a-button @click="close">{{ t('common.close') }}</a-button>
    </div>
  </a-modal>
</template>

<style scoped>
.mi-fo {
  display: grid;
  grid-template-columns: 330px minmax(0, 1fr);
  gap: 14px;
  min-height: 440px;
}
.mi-fo__list {
  border: 1px solid var(--mi-border);
  border-radius: var(--mi-radius-md);
  background: var(--mi-surface-2);
  padding: 8px;
  max-height: 580px;
  overflow: auto;
}
.mi-fo__list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-weight: 600;
  padding: 4px 4px 8px;
}
.mi-fo__file {
  width: 100%;
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto;
  gap: 6px;
  align-items: center;
  border: 0;
  background: transparent;
  color: var(--mi-text);
  padding: 8px;
  border-radius: var(--mi-radius-sm);
  text-align: left;
  cursor: pointer;
}
.mi-fo__file:hover,
.mi-fo__file--active {
  background: var(--mi-surface);
  box-shadow: inset 3px 0 var(--mi-primary);
}
.mi-fo__path {
  font-family: var(--mi-font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mi-fo__file .mi-hint {
  grid-column: 2 / 4;
  font-size: 11px;
}
.mi-fo__editor {
  min-width: 0;
}
.mi-fo__head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 10px;
}
.mi-fo__selected {
  font-size: 13px;
}
.mi-fo__textarea {
  font-family: var(--mi-font-mono) !important;
  font-size: 12px;
  line-height: 1.55;
}
.mi-fo__meta {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-top: 6px;
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-hint);
}
.mi-fo__original {
  margin: 0;
  max-height: 320px;
  overflow: auto;
  padding: 10px;
  border-radius: var(--mi-radius-sm);
  background: var(--mi-surface-2);
  border: 1px solid var(--mi-border);
  font-family: var(--mi-font-mono);
  font-size: 12px;
  white-space: pre-wrap;
}
.mi-fo__placeholder {
  align-self: center;
}
.mi-modal-footer {
  display: flex;
  justify-content: flex-end;
  margin-top: 16px;
}
</style>
