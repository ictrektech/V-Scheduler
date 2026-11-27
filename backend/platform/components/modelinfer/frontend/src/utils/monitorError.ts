import type { NodeMonitor } from '@/api/nodes';

/**
 * 监控组件部署失败中与 image-hub 镜像相关的分类。
 * - `missing`：节点与 image-hub 均无镜像（20737）
 * - `pullFailed`：经 image-hub 拉取失败 / 拉取后仍缺失（20738）
 * - `unreachable`：image-hub registry 地址无法确定或不可达（20730）
 */
export type MonitorImageHubIssue = 'missing' | 'pullFailed' | 'unreachable';

export interface MonitorImageHubGuidance {
  issue: MonitorImageHubIssue;
  /** 需要下载的镜像完整引用（优先取监控记录中的 image） */
  image: string;
}

const IMAGE_HUB_CODE = /\b(20730|20737|20738)\b/;
const IMAGE_REF = /\b[a-z0-9.-]+\.[a-z]{2,}\/[a-z0-9._/-]+(?::[a-z0-9._-]+)?/i;

/** 从错误文本中兜底提取镜像引用（历史记录 image 字段为空时使用） */
function imageFromError(text: string): string {
  return text.match(IMAGE_REF)?.[0] ?? '';
}

function classify(text: string, code: string): MonitorImageHubIssue {
  // 稳定错误码优先，避免拉取失败的 stderr（如 manifest not found）被误判为「镜像缺失」。
  if (code === '20737') return 'missing';
  if (code === '20730') return 'unreachable';
  if (code === '20738') return 'pullFailed';
  if (/不存在/.test(text) || /not\s+(?:exist|found|available)/i.test(text)) {
    return 'missing';
  }
  if (/不可达|unresolved|unreachable/i.test(text)) {
    return 'unreachable';
  }
  return 'pullFailed';
}

/**
 * 判断监控组件失败是否由 image-hub 镜像导致，并返回需要下载的镜像。
 *
 * 监控部署在 `ensure-image` 步骤失败时，错误会记录为 image-hub 分发失败；同时兼容
 * 失败信息中的稳定错误码（20730 / 20737 / 20738）与中文 / 英文提示文本。
 */
export function monitorImageHubGuidance(
  monitor: Pick<NodeMonitor, 'state' | 'image' | 'last_error' | 'steps'>,
): MonitorImageHubGuidance | null {
  if (monitor.state !== 'failed') return null;

  const failedEnsureImage = (monitor.steps ?? []).find(
    (step) => step.name === 'ensure-image' && step.state === 'failed',
  );
  const text = [monitor.last_error, failedEnsureImage?.detail]
    .filter((part): part is string => Boolean(part && part.trim()))
    .join('\n');
  if (!text) return null;

  const code = text.match(IMAGE_HUB_CODE)?.[1] ?? '';
  const mentionsImageHub = /image[-\s]?hub/i.test(text);
  // 仅对 image-hub 相关失败给出下载指引，避免把 SSH / 容器启动失败误导到镜像下载。
  if (!mentionsImageHub && !code) return null;

  const image = monitor.image?.trim() || imageFromError(text);
  return { issue: classify(text, code), image };
}
