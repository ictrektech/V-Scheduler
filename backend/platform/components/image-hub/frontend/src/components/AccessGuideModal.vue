<script setup lang="ts">
import { computed, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import type { CommandStep } from '@/utils/commands';
import {
  buildDownloadAndRun,
  buildOneLiner,
  buildPlainText,
  buildPullCommands,
  buildPushCommands,
  buildScriptDownload,
  commandStepTitle,
  scriptUrlFrom,
} from '@/utils/commands';
import { parseImageRef, upstreamReference, upstreamRepository } from '@/utils/image';

/**
 * 接入指引弹窗：由镜像仓库页（及详情页）「接入指引」按钮打开。
 *
 * 上传 / 下载各提供两种方式：
 * - **一键执行（默认推荐）**：`curl -fsSL <script-url> | bash -s -- --ih-addr <地址> ...`，
 *   由 image-hub.sh 自动完成 socat 建立与清理；
 * - **手动执行**：分步命令，末步停止 socat 后台进程，避免遗留进程占用端口。
 *
 * registry 地址状态由父页持有并经 props 下发，保证弹窗与页面展示/命令使用同一所选地址。
 */
const { t } = useI18n();

const props = defineProps<{
  open: boolean;
  addresses: string[];
  selectedIp: string;
  port: number;
  registryUrl: string;
}>();

const emit = defineEmits<{
  'update:open': [boolean];
  'update:selectedIp': [string];
}>();

const visible = computed({
  get: () => props.open,
  set: (v: boolean) => emit('update:open', v),
});

const ipModel = computed({
  get: () => props.selectedIp,
  set: (v: string) => emit('update:selectedIp', v),
});

type Mode = 'one' | 'manual';
const pushMode = ref<Mode>('one');
const pullMode = ref<Mode>('one');

/** 示例镜像名（可编辑）；形如 `docker.io/foo/bar:tag` */
const imageRef = ref('docker.io/foo/bar:tag');

/** 归一后的上游镜像引用信息（与后端 / 前端 image.ts 规则一致） */
const parsed = computed(() => parseImageRef(imageRef.value));
const upstreamRef = computed(() => (parsed.value ? upstreamReference(parsed.value) : ''));
const repository = computed(() => (parsed.value ? upstreamRepository(parsed.value) : ''));
const tag = computed(() => parsed.value?.tag ?? '');

/** image-hub 地址（`<ip>:<port>`，与所选网卡地址一致） */
const ihAddr = computed(() => (props.selectedIp ? `${props.selectedIp}:${props.port}` : ''));

/** image-hub.sh 下载地址（随前端 dist 同目录托管） */
const scriptUrl = ref('');
if (typeof window !== 'undefined') {
  scriptUrl.value = scriptUrlFrom(window.location.href);
}
/** 仅下载脚本的命令（管理面自签名 HTTPS，默认带 -k） */
const scriptDownloadCmd = computed(() =>
  scriptUrl.value ? buildScriptDownload(scriptUrl.value) : '',
);

const localPreview = computed(() =>
  parsed.value ? `127.0.0.1:${props.port}/${repository.value}:${tag.value}` : '',
);

const pushOneLiner = computed(() =>
  parsed.value && ihAddr.value
    ? buildOneLiner(scriptUrl.value, ihAddr.value, 'push', upstreamRef.value)
    : '',
);
const pullOneLiner = computed(() =>
  parsed.value && ihAddr.value
    ? buildOneLiner(scriptUrl.value, ihAddr.value, 'pull', upstreamRef.value)
    : '',
);
const pushDownloadLine = computed(() =>
  parsed.value && ihAddr.value
    ? buildDownloadAndRun(scriptUrl.value, ihAddr.value, 'push', upstreamRef.value)
    : '',
);
const pullDownloadLine = computed(() =>
  parsed.value && ihAddr.value
    ? buildDownloadAndRun(scriptUrl.value, ihAddr.value, 'pull', upstreamRef.value)
    : '',
);

const pushSteps = computed(() =>
  parsed.value && props.selectedIp
    ? buildPushCommands(props.selectedIp, props.port, repository.value, tag.value)
    : [],
);
const pullSteps = computed(() =>
  parsed.value && props.selectedIp
    ? buildPullCommands(props.selectedIp, props.port, repository.value, tag.value)
    : [],
);

/** 原理说明插值上下文 */
const principleParams = computed(() => ({
  upstream: 'docker.io/foo/bar',
  port: props.port,
}));

async function copy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    message.success(t('common.copied'));
  } catch {
    message.error(t('common.copyFailed'));
  }
}

async function copyAddress(): Promise<void> {
  await copy(props.registryUrl);
}

/** 供纯函数使用的翻译闭包（保持 utils 零 i18n 依赖） */
function translate(key: string, params?: Record<string, unknown>): string {
  return t(key, params as Record<string, never>);
}

function stepTitle(step: CommandStep): string {
  return commandStepTitle(step, translate);
}
</script>

<template>
  <a-modal
    v-model:open="visible"
    :title="t('guide.title')"
    width="880px"
    :footer="null"
    wrap-class-name="ih-guide-modal"
  >
    <p class="ih-muted" style="margin-top: 0">{{ t('guide.intro') }}</p>

    <!-- Registry 地址 -->
    <div class="ih-card" style="margin-bottom: var(--ih-space-lg)">
      <div class="addr-row">
        <div class="addr-label">
          <span>{{ t('guide.address') }}</span>
          <span class="addr-hint">{{ t('guide.addressHint') }}</span>
        </div>
        <div class="addr-controls">
          <a-select
            v-model:value="ipModel"
            :options="addresses.map((ip) => ({ label: ip, value: ip }))"
            :disabled="addresses.length === 0"
            style="width: 200px"
            :placeholder="t('guide.chooseAddress')"
          />
          <a-input :value="registryUrl" readonly style="width: 260px">
            <template #addonAfter>
              <a @click="copyAddress">{{ t('guide.copy') }}</a>
            </template>
          </a-input>
        </div>
      </div>
    </div>

    <!-- 镜像名输入 -->
    <div class="ih-card" style="margin-bottom: var(--ih-space-lg)">
      <div class="ref-row">
        <span class="ref-label">{{ t('guide.upstream') }}</span>
        <a-input
          v-model:value="imageRef"
          style="width: 320px"
          :placeholder="t('guide.upstreamPlaceholder')"
        />
        <span v-if="!parsed" class="ref-error">{{ t('guide.invalid') }}</span>
      </div>
      <div v-if="parsed" class="ih-muted ref-hint">
        {{ t('guide.normalized', { ref: upstreamRef, local: localPreview, repository }) }}
      </div>
    </div>

    <!-- 上传 -->
    <div class="ih-card" style="margin-bottom: var(--ih-space-lg)">
      <div class="section-head">
        <h3>{{ t('guide.upload.title') }}</h3>
        <a-radio-group v-model:value="pushMode" size="small" button-style="solid">
          <a-radio-button value="one">{{ t('guide.mode.one') }}</a-radio-button>
          <a-radio-button value="manual">{{ t('guide.mode.manual') }}</a-radio-button>
        </a-radio-group>
      </div>

      <template v-if="pushMode === 'one'">
        <div class="step">
          <div class="step-head">
            <span class="step-title">{{ t('guide.upload.oneTitle') }}</span>
            <a-button
              type="link"
              size="small"
              :disabled="!pushOneLiner"
              @click="copy(pushOneLiner)"
            >
              {{ t('common.copy') }}
            </a-button>
          </div>
          <pre class="ih-cmd">{{ pushOneLiner || t('guide.noCommand') }}</pre>
          <div v-if="pushDownloadLine" class="ih-muted oneliner-alt">
            {{ t('guide.downloadAlt') }}<code>{{ pushDownloadLine }}</code>
          </div>
        </div>
        <div class="ih-muted tip">{{ t('guide.prereq') }}</div>
      </template>

      <template v-else>
        <div class="section-head sub">
          <span class="ih-muted">{{ t('guide.manualHint') }}</span>
          <a-button
            size="small"
            :disabled="pushSteps.length === 0"
            @click="copy(buildPlainText(pushSteps, translate))"
          >
            {{ t('guide.copyAll') }}
          </a-button>
        </div>
        <div v-for="(step, index) in pushSteps" :key="index" class="step">
          <div class="step-head">
            <span class="step-title">{{ index + 1 }}. {{ stepTitle(step) }}</span>
            <a-button type="link" size="small" @click="copy(step.command)">{{ t('common.copy') }}</a-button>
          </div>
          <pre class="ih-cmd">{{ step.command }}</pre>
        </div>
      </template>
    </div>

    <!-- 下载 -->
    <div class="ih-card" style="margin-bottom: var(--ih-space-lg)">
      <div class="section-head">
        <h3>{{ t('guide.download.title') }}</h3>
        <a-radio-group v-model:value="pullMode" size="small" button-style="solid">
          <a-radio-button value="one">{{ t('guide.mode.one') }}</a-radio-button>
          <a-radio-button value="manual">{{ t('guide.mode.manual') }}</a-radio-button>
        </a-radio-group>
      </div>

      <template v-if="pullMode === 'one'">
        <div class="step">
          <div class="step-head">
            <span class="step-title">{{ t('guide.download.oneTitle') }}</span>
            <a-button
              type="link"
              size="small"
              :disabled="!pullOneLiner"
              @click="copy(pullOneLiner)"
            >
              {{ t('common.copy') }}
            </a-button>
          </div>
          <pre class="ih-cmd">{{ pullOneLiner || t('guide.noCommand') }}</pre>
          <div v-if="pullDownloadLine" class="ih-muted oneliner-alt">
            {{ t('guide.downloadAlt') }}<code>{{ pullDownloadLine }}</code>
          </div>
        </div>
        <div class="ih-muted tip">{{ t('guide.prereq') }}</div>
      </template>

      <template v-else>
        <div class="section-head sub">
          <span class="ih-muted">{{ t('guide.manualHint') }}</span>
          <a-button
            size="small"
            :disabled="pullSteps.length === 0"
            @click="copy(buildPlainText(pullSteps, translate))"
          >
            {{ t('guide.copyAll') }}
          </a-button>
        </div>
        <div v-for="(step, index) in pullSteps" :key="index" class="step">
          <div class="step-head">
            <span class="step-title">{{ index + 1 }}. {{ stepTitle(step) }}</span>
            <a-button type="link" size="small" @click="copy(step.command)">{{ t('common.copy') }}</a-button>
          </div>
          <pre class="ih-cmd">{{ step.command }}</pre>
        </div>
      </template>
    </div>

    <!-- 原理与注意事项 -->
    <div class="ih-card">
      <h3>{{ t('guide.principles.title') }}</h3>
      <ul>
        <li>
          <b>{{ t('guide.principles.name.name') }}</b>{{ t('guide.principles.colon') }}{{ t('guide.principles.nameItem', principleParams) }}
        </li>
        <li>
          <b>{{ t('guide.principles.name.socat') }}</b>{{ t('guide.principles.colon') }}{{ t('guide.principles.socatItem') }}
        </li>
        <li>
          <b>{{ t('guide.principles.name.script') }}</b>{{ t('guide.principles.colon') }}{{ t('guide.principles.scriptItem', { command: scriptDownloadCmd }) }}
        </li>
        <li>
          <b>{{ t('guide.principles.name.delete') }}</b>{{ t('guide.principles.colon') }}{{ t('guide.principles.deleteItem') }}
        </li>
        <li>
          <b>{{ t('guide.principles.name.security') }}</b>{{ t('guide.principles.colon') }}{{ t('guide.principles.securityItem') }}
        </li>
      </ul>
    </div>
  </a-modal>
</template>

<style scoped>
.addr-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--ih-space-lg);
  flex-wrap: wrap;
}
.addr-label {
  font-size: 14px;
  font-weight: 500;
  color: var(--ih-text);
}
.addr-hint {
  font-size: 12px;
  font-weight: 400;
  color: var(--ih-text-tertiary);
  margin-left: var(--ih-space-xs);
}
.addr-controls {
  display: flex;
  align-items: center;
  gap: var(--ih-space-sm);
}
.ref-row {
  display: flex;
  align-items: center;
  gap: var(--ih-space-sm);
  flex-wrap: wrap;
}
.ref-label {
  font-size: 13px;
  color: var(--ih-text-secondary);
}
.ref-error {
  font-size: 12px;
  color: var(--ih-danger);
}
.ref-hint {
  margin-top: var(--ih-space-xs);
  font-size: 12px;
}
.section-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--ih-space);
  margin-bottom: var(--ih-space);
}
.section-head h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--ih-text);
}
.section-head.sub {
  margin-top: var(--ih-space-sm);
}
.step {
  margin-bottom: var(--ih-space);
}
.step-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--ih-space-sm);
  margin-bottom: 4px;
}
.step-title {
  font-size: 12.5px;
  color: var(--ih-text-secondary);
}
.oneliner-alt {
  margin-top: 4px;
  font-size: 12px;
  word-break: break-all;
}
.tip {
  font-size: 12px;
}
code {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  background: var(--ih-surface-alt);
  border: 1px solid var(--ih-border);
  border-radius: 4px;
  padding: 0 4px;
  font-size: 12px;
  word-break: break-all;
}
</style>