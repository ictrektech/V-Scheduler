<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import {
  deleteRegistryCredential,
  fetchSettings,
  saveProxy,
  saveRegistryCredential,
  type ProxyConfig,
  type RegistryView,
} from '@/api/imageHub';
import { resolveApiErrorMessage } from '@/i18n/api-error';

/**
 * 仓库设置弹窗：
 * - 上游 registry 认证（新增/删除；密码只写不读）
 * - 代理配置（启用/禁用 http_proxy / https_proxy / no_proxy）
 */
const { t } = useI18n();

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ 'update:open': [boolean]; changed: [] }>();

const visible = computed({
  get: () => props.open,
  set: (v: boolean) => emit('update:open', v),
});

const tab = ref<'registry' | 'proxy'>('registry');
const loading = ref(false);
const saving = ref(false);

const registries = ref<RegistryView[]>([]);
const newCred = reactive({ host: '', username: '', password: '' });

const proxy = reactive<ProxyConfig>({
  enabled: false,
  http_proxy: '',
  https_proxy: '',
  no_proxy: '',
});

async function load(): Promise<void> {
  loading.value = true;
  try {
    const data = await fetchSettings();
    registries.value = data.registries ?? [];
    Object.assign(proxy, data.proxy ?? proxy);
  } catch (e) {
    message.error(t('settings.messages.loadFailed', { error: resolveApiErrorMessage(e) }));
  } finally {
    loading.value = false;
  }
}

watch(
  () => props.open,
  (open) => {
    if (open) {
      tab.value = 'registry';
      newCred.host = '';
      newCred.username = '';
      newCred.password = '';
      void load();
    }
  },
);

async function addCredential(): Promise<void> {
  if (!newCred.host.trim() || !newCred.username.trim()) {
    message.error(t('settings.messages.missingFields'));
    return;
  }
  saving.value = true;
  try {
    const res = await saveRegistryCredential(newCred.host.trim(), newCred.username.trim(), newCred.password);
    registries.value = res.registries ?? [];
    newCred.host = '';
    newCred.username = '';
    newCred.password = '';
    message.success(t('settings.messages.saved'));
    emit('changed');
  } catch (e) {
    message.error(t('settings.messages.saveFailed', { error: resolveApiErrorMessage(e) }));
  } finally {
    saving.value = false;
  }
}

async function removeCredential(host: string): Promise<void> {
  try {
    const res = await deleteRegistryCredential(host);
    registries.value = res.registries ?? [];
    message.success(t('settings.messages.deleted', { host }));
    emit('changed');
  } catch (e) {
    message.error(t('settings.messages.deleteFailed', { error: resolveApiErrorMessage(e) }));
  }
}

async function saveProxyConfig(): Promise<void> {
  saving.value = true;
  try {
    const res = await saveProxy({ ...proxy });
    Object.assign(proxy, res);
    message.success(t('settings.messages.proxySaved'));
    emit('changed');
  } catch (e) {
    message.error(t('settings.messages.saveFailed', { error: resolveApiErrorMessage(e) }));
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <a-modal v-model:open="visible" :title="t('settings.title')" width="760px" :footer="null">
    <a-tabs v-model:activeKey="tab">
      <!-- 上游认证 -->
      <a-tab-pane key="registry" :tab="t('settings.tabs.registry')">
        <p class="ih-muted" style="margin-top: 0">
          {{ t('settings.auth.intro1') }}<code>{{ t('settings.auth.introCode') }}</code>{{ t('settings.auth.intro2') }}
        </p>

        <a-form layout="inline" class="cred-form" @submit.prevent>
          <a-form-item :label="t('settings.auth.host')">
            <a-input v-model:value="newCred.host" style="width: 200px" placeholder="docker.m.daocloud.io" />
          </a-form-item>
          <a-form-item :label="t('settings.auth.username')">
            <a-input v-model:value="newCred.username" style="width: 140px" placeholder="username" />
          </a-form-item>
          <a-form-item :label="t('settings.auth.password')">
            <a-input-password v-model:value="newCred.password" style="width: 160px" placeholder="password" />
          </a-form-item>
          <a-form-item>
            <a-button type="primary" :loading="saving" @click="addCredential">
              {{ t('settings.auth.save') }}
            </a-button>
          </a-form-item>
        </a-form>

        <a-table
          :data-source="registries"
          :loading="loading"
          :pagination="false"
          row-key="host"
          size="small"
          style="margin-top: 12px"
        >
          <a-table-column :title="t('settings.auth.host')" data-index="host" key="host" />
          <a-table-column :title="t('settings.columns.username')" data-index="username" key="username" />
          <a-table-column :title="t('settings.columns.password')" key="password" width="100">
            <template #default="{ record }">
              <a-tag :color="record.has_password ? 'green' : 'default'">
                {{ record.has_password ? t('settings.status.set') : t('settings.status.unset') }}
              </a-tag>
            </template>
          </a-table-column>
          <a-table-column :title="t('settings.columns.action')" key="action" width="90">
            <template #default="{ record }">
              <a-popconfirm
                :title="t('settings.deleteConfirm', { host: record.host })"
                :ok-text="t('common.delete')"
                :cancel-text="t('common.cancel')"
                @confirm="removeCredential(record.host)"
              >
                <a-button type="link" danger size="small">{{ t('common.delete') }}</a-button>
              </a-popconfirm>
            </template>
          </a-table-column>
          <template #emptyText>
            <span class="ih-muted">{{ t('settings.empty') }}</span>
          </template>
        </a-table>
      </a-tab-pane>

      <!-- 代理 -->
      <a-tab-pane key="proxy" :tab="t('settings.tabs.proxy')">
        <p class="ih-muted" style="margin-top: 0">
          {{ t('settings.proxy.intro1') }}<code>{{ t('settings.proxy.introCode') }}</code>{{ t('settings.proxy.intro2') }}
        </p>
        <div class="proxy-row">
          <span class="proxy-label">{{ t('settings.proxy.enable') }}</span>
          <a-switch v-model:checked="proxy.enabled" />
        </div>
        <div class="proxy-row">
          <span class="proxy-label">http_proxy</span>
          <a-input v-model:value="proxy.http_proxy" :disabled="!proxy.enabled" placeholder="http://proxy:3128" />
        </div>
        <div class="proxy-row">
          <span class="proxy-label">https_proxy</span>
          <a-input v-model:value="proxy.https_proxy" :disabled="!proxy.enabled" placeholder="http://proxy:3128" />
        </div>
        <div class="proxy-row">
          <span class="proxy-label">no_proxy</span>
          <a-input v-model:value="proxy.no_proxy" :disabled="!proxy.enabled" placeholder="example.com,10.0.0.0/8" />
        </div>
        <div class="footer">
          <a-button type="primary" :loading="saving" @click="saveProxyConfig">
            {{ t('settings.proxy.save') }}
          </a-button>
        </div>
      </a-tab-pane>
    </a-tabs>
  </a-modal>
</template>

<style scoped>
.cred-form {
  margin-bottom: 4px;
}
.proxy-row {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 10px;
}
.proxy-label {
  width: 96px;
  flex: none;
  font-size: 13px;
  color: var(--ih-text-secondary);
}
.footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}
code {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  background: var(--ih-surface-alt);
  border: 1px solid var(--ih-border);
  border-radius: 4px;
  padding: 0 4px;
  font-size: 12px;
}
</style>