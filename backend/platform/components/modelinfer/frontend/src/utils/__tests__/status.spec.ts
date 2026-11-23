import { describe, expect, it } from 'vitest';
import { statusMeta } from '../status';

describe('statusMeta', () => {
  it('部署状态返回 i18n key 与语义色', () => {
    expect(statusMeta('deployment', 'running')).toEqual({
      labelKey: 'status.deployment.running',
      fallback: '',
      tone: 'success',
    });
    expect(statusMeta('deployment', 'stopped')).toEqual({
      labelKey: 'status.deployment.stopped',
      fallback: '',
      tone: 'neutral',
    });
    expect(statusMeta('deployment', 'failed')).toEqual({
      labelKey: 'status.deployment.failed',
      fallback: '',
      tone: 'error',
    });
    expect(statusMeta('deployment', 'starting')).toEqual({
      labelKey: 'status.deployment.starting',
      fallback: '',
      tone: 'processing',
    });
  });

  it('健康 / 版本 / 来源 / 节点 / 网关', () => {
    expect(statusMeta('health', 'unknown')).toEqual({
      labelKey: 'status.health.unknown',
      fallback: '',
      tone: 'neutral',
    });
    expect(statusMeta('version', 'available')).toEqual({
      labelKey: 'status.version.available',
      fallback: '',
      tone: 'success',
    });
    expect(statusMeta('source', 'local')).toEqual({
      labelKey: 'status.source.local',
      fallback: '',
      tone: 'neutral',
    });
    expect(statusMeta('source', 'plaza')).toEqual({
      labelKey: 'status.source.plaza',
      fallback: '',
      tone: 'processing',
    });
    expect(statusMeta('node', 'online')).toEqual({
      labelKey: 'status.node.online',
      fallback: '',
      tone: 'success',
    });
    expect(statusMeta('vrouter', 'registered')).toEqual({
      labelKey: 'status.vrouter.registered',
      fallback: '',
      tone: 'success',
    });
  });

  it('未知取值回落原值；空值为 common.none', () => {
    expect(statusMeta('deployment', 'weird')).toEqual({
      labelKey: '',
      fallback: 'weird',
      tone: 'neutral',
    });
    expect(statusMeta('deployment', null)).toEqual({
      labelKey: 'common.none',
      fallback: '',
      tone: 'neutral',
    });
    expect(statusMeta('deployment', '')).toEqual({
      labelKey: 'common.none',
      fallback: '',
      tone: 'neutral',
    });
  });

  it('运行记录：状态 / 类型 / 触发', () => {
    expect(statusMeta('run', 'succeeded').labelKey).toBe('status.run.succeeded');
    expect(statusMeta('run', 'succeeded').tone).toBe('success');
    expect(statusMeta('run', 'failed').labelKey).toBe('status.run.failed');
    expect(statusMeta('run', 'running').tone).toBe('processing');
    expect(statusMeta('runKind', 'start').labelKey).toBe('status.runKind.start');
    expect(statusMeta('runKind', 'health').labelKey).toBe('status.runKind.health');
    expect(statusMeta('trigger', 'scheduler').labelKey).toBe('status.trigger.scheduler');
    expect(statusMeta('trigger', 'manual').labelKey).toBe('status.trigger.manual');
  });

  it('运行步骤：状态 / 编排来源', () => {
    expect(statusMeta('stepState', 'running').labelKey).toBe('status.stepState.running');
    expect(statusMeta('stepState', 'succeeded').labelKey).toBe('status.stepState.succeeded');
    expect(statusMeta('stepState', 'failed').labelKey).toBe('status.stepState.failed');
    expect(statusMeta('orchestrator', 'temporal').labelKey).toBe('status.orchestrator.temporal');
    expect(statusMeta('orchestrator', 'scheduler').labelKey).toBe('status.orchestrator.scheduler');
  });

  it('节点：认证方式 / GPU 厂商', () => {
    expect(statusMeta('authMethod', 'password').labelKey).toBe('status.authMethod.password');
    expect(statusMeta('authMethod', 'key').labelKey).toBe('status.authMethod.key');
    expect(statusMeta('gpuVendor', 'nvidia').labelKey).toBe('status.gpuVendor.nvidia');
    expect(statusMeta('gpuVendor', 'amd').labelKey).toBe('status.gpuVendor.amd');
    expect(statusMeta('gpuVendor', 'intel').labelKey).toBe('status.gpuVendor.intel');
    // 未知厂商回显原值
    expect(statusMeta('gpuVendor', 'apple')).toEqual({
      labelKey: '',
      fallback: 'apple',
      tone: 'neutral',
    });
  });
});