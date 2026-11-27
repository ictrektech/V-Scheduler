import { describe, expect, it } from 'vitest';
import type { MonitorStep, NodeMonitor } from '@/api/nodes';
import { monitorImageHubGuidance } from '../monitorError';

const IMAGE = 'docker.io/prom/node-exporter:v1.12.0';

function step(overrides: Partial<MonitorStep>): MonitorStep {
  return {
    name: 'ensure-image',
    title: '镜像就绪',
    state: 'failed',
    detail: '',
    started_at: '2026-11-18T00:00:00Z',
    finished_at: '2026-11-18T00:00:01Z',
    ...overrides,
  };
}

function monitor(overrides: Partial<NodeMonitor> = {}): NodeMonitor {
  return {
    node_id: 'n1',
    component: 'node-exporter',
    category: 'system',
    state: 'failed',
    service_id: 'svc-n1',
    service_name: 'modelinfer-node-exporter',
    image: IMAGE,
    workflow_id: 'wf1',
    last_error: '',
    steps: [],
    deployed_at: null,
    created_at: '2026-11-18T00:00:00Z',
    updated_at: '2026-11-18T00:00:01Z',
    ...overrides,
  };
}

describe('monitorImageHubGuidance', () => {
  it('20737（节点与 image-hub 均无镜像）→ missing，并返回完整镜像名', () => {
    const guidance = monitorImageHubGuidance(
      monitor({
        last_error: '[NodeMonitorDeployWorkflow][ensure-image] 镜像就绪失败: [20737] 监控镜像在节点与 image-hub 上均不存在',
        steps: [step({ detail: '监控镜像在节点与 image-hub 上均不存在: docker.io/prom/node-exporter:v1.12.0' })],
      }),
    );
    expect(guidance).toEqual({ issue: 'missing', image: IMAGE });
  });

  it('20738（经 image-hub 拉取失败）→ pullFailed', () => {
    const guidance = monitorImageHubGuidance(
      monitor({
        last_error: '[NodeMonitorDeployWorkflow][ensure-image] 镜像就绪失败: [20738] 经 image-hub 拉取后仍缺少监控镜像',
        steps: [step({ detail: '经 image-hub 拉取后仍缺少监控镜像 docker.io/prom/node-exporter:v1.12.0' })],
      }),
    );
    expect(guidance).toEqual({ issue: 'pullFailed', image: IMAGE });
  });

  it('20738 优先于 stderr 中的 not found，不误判为镜像缺失', () => {
    const guidance = monitorImageHubGuidance(
      monitor({
        last_error: '[20738] 经 image-hub 拉取监控镜像失败',
        steps: [step({ detail: '经 image-hub(docker.io) 拉取监控镜像失败: manifest not found' })],
      }),
    );
    expect(guidance?.issue).toBe('pullFailed');
  });

  it('20730（image-hub 不可达）→ unreachable', () => {
    const guidance = monitorImageHubGuidance(
      monitor({
        last_error: '[20730] image-hub registry 不可达',
        steps: [step({ detail: 'image-hub registry 不可达' })],
      }),
    );
    expect(guidance).toEqual({ issue: 'unreachable', image: IMAGE });
  });

  it('历史记录缺少错误码时按 image-hub 文本判定缺失', () => {
    const guidance = monitorImageHubGuidance(
      monitor({
        last_error: '镜像就绪失败',
        steps: [step({ detail: '监控镜像在节点与 image-hub 上均不存在，已中止' })],
      }),
    );
    expect(guidance?.issue).toBe('missing');
  });

  it('image 为空时从错误文本兜底提取镜像', () => {
    const guidance = monitorImageHubGuidance(
      monitor({
        image: '',
        last_error: '[20737] 监控镜像在节点与 image-hub 上均不存在',
        steps: [step({ detail: '...: docker.io/prom/node-exporter:v1.12.0' })],
      }),
    );
    expect(guidance?.image).toBe(IMAGE);
  });

  it('非失败状态不提示', () => {
    expect(monitorImageHubGuidance(monitor({ state: 'deployed', last_error: 'image-hub' }))).toBeNull();
  });

  it('非 image-hub 的部署失败不误导到镜像下载', () => {
    expect(
      monitorImageHubGuidance(
        monitor({
          last_error: '容器启动失败: port already in use',
          steps: [step({ name: 'deploy', detail: '容器启动失败: port already in use' })],
        }),
      ),
    ).toBeNull();
  });
});
