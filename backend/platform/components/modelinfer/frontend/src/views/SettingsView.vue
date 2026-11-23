<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import { ReloadOutlined } from '@ant-design/icons-vue';
import PageShell from '@/components/PageShell.vue';
import CopyText from '@/components/CopyText.vue';
import { fetchHealth, type HealthData } from '@/api/health';
import { fetchNfsInfo, type NfsResolveResult } from '@/api/nfs';
import { fetchModelHubInfo, type ModelHubInfo } from '@/api/modelHub';
import { getSettings, updateSettings, type SettingsData } from '@/api/observability';
import { getVrouterStatus, type VrouterStatus } from '@/api/deployments';
import { resolveApiErrorMessage } from '@/i18n/api-error';

const { t } = useI18n();
const loading = ref(false);
const health = ref<HealthData | null>(null);
const healthError = ref('');
const nfsInfo = ref<NfsResolveResult | null>(null);
const nfsError = ref('');
const hub = ref<ModelHubInfo | null>(null);
const vrouter = ref<VrouterStatus | null>(null);

const settings = reactive({ health_history_keep: 10, run_history_keep: 20 });
const settingsLimits = reactive({ max_health: 100, max_run: 200 });
const settingsSaving = ref(false);

async function loadSettings() {
  try {
    const s: SettingsData = await getSettings();
    settings.health_history_keep = s.health_history_keep;
    settings.run_history_keep = s.run_history_keep;
    settingsLimits.max_health = s.max_health_history_keep;
    settingsLimits.max_run = s.max_run_history_keep;
  } catch (e) {
    message.error(`${t('settings.msg.settingsLoadFailed')}${resolveApiErrorMessage(e)}`);
  }
}

async function saveSettings() {
  settingsSaving.value = true;
  try {
    const s = await updateSettings({
      health_history_keep: settings.health_history_keep,
      run_history_keep: settings.run_history_keep,
    });
    settings.health_history_keep = s.health_history_keep;
    settings.run_history_keep = s.run_history_keep;
    message.success(t('settings.msg.settingsSaved'));
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    settingsSaving.value = false;
  }
}

async function load() {
  loading.value = true;
  try {
    health.value = await fetchHealth();
    healthError.value = '';
  } catch (e) {
    healthError.value = resolveApiErrorMessage(e);
  }
  try {
    nfsInfo.value = await fetchNfsInfo();
    nfsError.value = '';
  } catch (e) {
    nfsInfo.value = null;
    nfsError.value = resolveApiErrorMessage(e);
  }
  try {
    hub.value = await fetchModelHubInfo();
  } catch (e) {
    hub.value = null;
    message.error(`${t('settings.msg.hubLoadFailed')}${resolveApiErrorMessage(e)}`);
  }
  try {
    vrouter.value = await getVrouterStatus();
  } catch {
    vrouter.value = null;
  }
  await loadSettings();
  loading.value = false;
}

onMounted(load);
</script>

<template>
  <PageShell :title="t('settings.title')" :subtitle="t('settings.subtitle')" :loading="loading">
    <template #actions>
      <a-button @click="load">
        <ReloadOutlined />
        {{ t('common.refresh') }}
      </a-button>
    </template>

    <a-card size="small" :title="t('settings.health.title')" style="margin-bottom: 16px">
      <a-alert v-if="healthError" type="warning" show-icon :message="t('settings.health.loadFailed')" :description="healthError" />
      <div class="mi-grid">
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('settings.health.status') }}</div>
          <div class="mi-kv__v">
            <a-tag :color="!health ? 'default' : health.status === 'ok' ? 'green' : 'red'">
              {{ health?.status ?? '-' }}
            </a-tag>
          </div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('settings.health.version') }}</div>
          <div class="mi-kv__v mi-mono">{{ health?.version ?? '-' }}</div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('settings.health.dbReady') }}</div>
          <div class="mi-kv__v">
            <a-tag :color="!health ? 'default' : health.db_ready ? 'green' : 'red'">{{ !health ? '—' : health.db_ready ? t('common.yes') : t('common.no') }}</a-tag>
          </div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('settings.health.platformToken') }}</div>
          <div class="mi-kv__v">
            <a-tag :color="!health ? 'default' : health.platform_token_present ? 'green' : 'red'">
              {{ !health ? '—' : health.platform_token_present ? t('settings.health.tokenPresent') : t('settings.health.tokenMissing') }}
            </a-tag>
          </div>
        </div>
      </div>
    </a-card>

    <a-card size="small" :title="t('settings.retention.title')" style="margin-bottom: 16px">
      <div class="mi-keep">
        <div class="mi-keep__row">
          <span class="mi-keep__label">{{ t('settings.retention.healthKeep') }}</span>
          <a-input-number
            v-model:value="settings.health_history_keep"
            :min="1"
            :max="settingsLimits.max_health"
            style="width: 120px"
          />
          <span class="mi-hint">{{ t('settings.retention.timesLimit', { max: settingsLimits.max_health }) }}</span>
        </div>
        <div class="mi-keep__row">
          <span class="mi-keep__label">{{ t('settings.retention.runKeep') }}</span>
          <a-input-number
            v-model:value="settings.run_history_keep"
            :min="1"
            :max="settingsLimits.max_run"
            style="width: 120px"
          />
          <span class="mi-hint">{{ t('settings.retention.timesLimit', { max: settingsLimits.max_run }) }}</span>
        </div>
        <div class="mi-keep__row">
          <a-button type="primary" :loading="settingsSaving" @click="saveSettings">{{ t('common.save') }}</a-button>
          <span class="mi-hint">{{ t('settings.retention.hint') }}</span>
        </div>
      </div>
    </a-card>

    <a-card size="small" :title="t('settings.vrouter.title')" style="margin-bottom: 16px">
      <div v-if="vrouter" class="mi-grid">
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('settings.vrouter.installed') }}</div>
          <div class="mi-kv__v">
            <a-tag :color="vrouter.installed ? 'green' : 'default'">{{ vrouter.installed ? t('common.yes') : t('common.no') }}</a-tag>
          </div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('settings.vrouter.reachable') }}</div>
          <div class="mi-kv__v">
            <a-tag :color="vrouter.reachable ? 'green' : 'red'">{{ vrouter.reachable ? t('common.yes') : t('common.no') }}</a-tag>
          </div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('settings.vrouter.credential') }}</div>
          <div class="mi-kv__v">
            <a-tag :color="vrouter.credential_present ? 'green' : 'orange'">
              {{ vrouter.credential_present ? t('settings.vrouter.credentialReady') : t('settings.vrouter.credentialMissing') }}
            </a-tag>
          </div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('settings.vrouter.registeredCount') }}</div>
          <div class="mi-kv__v">{{ vrouter.registered_count }}</div>
        </div>
        <div v-if="vrouter.message" class="mi-kv mi-kv--wide">
          <div class="mi-kv__k">{{ t('settings.vrouter.message') }}</div>
          <div class="mi-kv__v mi-hint">{{ vrouter.message }}</div>
        </div>
      </div>
      <a-empty v-else :description="t('settings.notAvailable')" />
    </a-card>

    <a-card size="small" :title="t('settings.hub.title')" style="margin-bottom: 16px">
      <template v-if="hub">
        <div class="mi-grid">
          <div class="mi-kv mi-kv--wide">
            <div class="mi-kv__k">{{ t('settings.hub.dir') }}</div>
            <div class="mi-kv__v"><CopyText :text="hub.dir" /></div>
          </div>
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.hub.exists') }}</div>
            <div class="mi-kv__v">
              <a-tag :color="hub.exists ? 'green' : 'red'">{{ hub.exists ? t('common.yes') : t('common.no') }}</a-tag>
            </div>
          </div>
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.hub.hfCount') }}</div>
            <div class="mi-kv__v">{{ hub.hf_count }}</div>
          </div>
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.hub.msCount') }}</div>
            <div class="mi-kv__v">{{ hub.ms_count }}</div>
          </div>
          <div class="mi-kv mi-kv--wide">
            <div class="mi-kv__k">{{ t('settings.hub.readyModels') }}</div>
            <div class="mi-kv__v">
              <template v-if="hub.models.length">
                <a-tag v-for="m in hub.models" :key="m.abs_path" :color="m.source === 'hf' ? 'blue' : 'green'">
                  {{ m.source }}:{{ m.model_id }}
                </a-tag>
              </template>
              <span v-else class="mi-hint">{{ t('settings.none') }}</span>
            </div>
          </div>
        </div>
      </template>
      <a-empty v-else :description="t('settings.notAvailable')" />
    </a-card>

    <a-card size="small" :title="t('settings.nfs.title')">
      <template v-if="nfsInfo">
        <div class="mi-grid">
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.nfs.configName') }}</div>
            <div class="mi-kv__v mi-mono">{{ nfsInfo.config_name }}</div>
          </div>
          <div class="mi-kv mi-kv--wide">
            <div class="mi-kv__k">{{ t('settings.nfs.hostPath') }}</div>
            <div class="mi-kv__v"><CopyText :text="nfsInfo.host_path" /></div>
          </div>
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.nfs.volume') }}</div>
            <div class="mi-kv__v">{{ nfsInfo.volume_name }}</div>
          </div>
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.nfs.volumeUuid') }}</div>
            <div class="mi-kv__v"><CopyText :text="nfsInfo.volume_uuid" secondary /></div>
          </div>
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.nfs.server') }}</div>
            <div class="mi-kv__v mi-mono">{{ nfsInfo.nfs_server }}</div>
          </div>
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.nfs.exportPath') }}</div>
            <div class="mi-kv__v"><CopyText :text="nfsInfo.nfs_export_path" secondary /></div>
          </div>
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.nfs.subPath') }}</div>
            <div class="mi-kv__v mi-mono">{{ nfsInfo.sub_path || '-' }}</div>
          </div>
          <div class="mi-kv mi-kv--wide">
            <div class="mi-kv__k">{{ t('settings.nfs.mountSource') }}</div>
            <div class="mi-kv__v"><CopyText :text="nfsInfo.mount_source" /></div>
          </div>
          <div class="mi-kv">
            <div class="mi-kv__k">{{ t('settings.nfs.accessLabel') }}</div>
            <div class="mi-kv__v">
              <a-tag :color="nfsInfo.read_only ? 'orange' : 'green'">{{ nfsInfo.read_only ? t('settings.nfs.readOnly') : t('settings.nfs.readWrite') }}</a-tag>
            </div>
          </div>
        </div>
      </template>
      <a-empty v-else :description="nfsError || t('settings.notAvailable')" />
    </a-card>
  </PageShell>
</template>

<style scoped>
.mi-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 14px 24px;
}
.mi-kv--wide {
  grid-column: 1 / -1;
}
.mi-kv__k {
  font-size: 12px;
  color: var(--mi-text-secondary);
  margin-bottom: 4px;
}
.mi-kv__v {
  font-size: 13px;
  color: var(--mi-text);
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}
.mi-hint {
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-hint);
}

.mi-keep {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.mi-keep__row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.mi-keep__label {
  min-width: 150px;
  color: var(--mi-text);
}
</style>