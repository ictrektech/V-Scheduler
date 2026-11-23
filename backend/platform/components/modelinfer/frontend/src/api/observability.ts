import { get, put, runtimeHeaders, runtimeUrl } from './client';

/** 运行记录 */
export interface RunItem {
  id: string;
  deployment_id: string;
  kind: string;
  state: string;
  trigger: string;
  started_at: string;
  finished_at?: string | null;
  summary: string;
  error: string;
  /** 是否存在 Temporal 工作流（决定能否按需查询 Temporal 活动） */
  has_temporal: boolean;
  /** 当前正在执行的步骤（无则 null） */
  active_step?: StepBrief | null;
}

/** 当前步骤摘要 */
export interface StepBrief {
  step: string;
  stage: string;
  title: string;
  node_name: string;
  started_at: string;
}

/** 运行步骤（编排活动 / 阶段） */
export interface RunStepItem {
  id: string;
  run_id: string;
  deployment_id: string;
  kind: string;
  step: string;
  stage: string;
  title: string;
  state: string;
  orchestrator: string;
  node_id: string;
  node_name: string;
  node_host: string;
  seq: number;
  detail: unknown;
  error: string;
  started_at: string;
  finished_at?: string | null;
}

/** 脚本执行日志 */
export interface RunLogItem {
  id: string;
  run_id: string;
  kind: string;
  stage: string;
  node_id: string;
  node_name: string;
  node_host: string;
  attempt: number;
  exit_code: number;
  stdout: string;
  stderr: string;
  stdout_bytes: number;
  stderr_bytes: number;
  truncated: boolean;
  started_at: string;
  finished_at: string;
}

/** 节点结构化数据 */
export interface NodeDataItem {
  id: string;
  stage: string;
  node_id: string;
  node_name: string;
  node_host: string;
  data: unknown;
  created_at: string;
}

/** 全局结构化数据 */
export interface GlobalDataItem {
  id: string;
  stage: string;
  data: unknown;
  node_count: number;
  created_at: string;
}

/** 容器清单 */
export interface ContainerItem {
  id: string;
  deployment_id: string;
  node_id: string;
  node_name: string;
  node_host: string;
  name: string;
  image: string;
  run_id: string;
  stage: string;
  updated_at: string;
}

export interface RunDetail {
  run: RunItem;
  steps: RunStepItem[];
  node_data: NodeDataItem[];
  global_data: GlobalDataItem[];
}

/** Temporal 活动（按需查询，不落库） */
export interface TemporalActivityItem {
  activity_id: string;
  activity_type: string;
  state: string;
  attempt: number;
  scheduled_time?: string | null;
  last_failure: string;
}

/** Temporal 工作流执行描述（按需查询，不落库） */
export interface TemporalWorkflowItem {
  workflow_id: string;
  run_id: string;
  workflow_type: string;
  status: string;
  task_queue: string;
  history_length: number;
  start_time?: string | null;
  close_time?: string | null;
  pending_activities: TemporalActivityItem[];
}

export interface SettingsData {
  health_history_keep: number;
  run_history_keep: number;
  max_health_history_keep: number;
  max_run_history_keep: number;
}

export function listRuns(
  deploymentId: string,
  params?: { kind?: string; limit?: number },
): Promise<{ data: RunItem[]; total: number }> {
  return get<{ data: RunItem[]; total: number }>(`/api/deployments/${deploymentId}/runs`, params);
}

export function getRun(deploymentId: string, runId: string): Promise<RunDetail> {
  return get<RunDetail>(`/api/deployments/${deploymentId}/runs/${runId}`);
}

export function listRunSteps(
  deploymentId: string,
  runId: string,
): Promise<{ data: RunStepItem[]; total: number }> {
  return get<{ data: RunStepItem[]; total: number }>(
    `/api/deployments/${deploymentId}/runs/${runId}/steps`,
  );
}

/** 按需查询 Temporal 工作流执行描述（不落库） */
export function getRunTemporal(
  deploymentId: string,
  runId: string,
): Promise<TemporalWorkflowItem> {
  return get<TemporalWorkflowItem>(`/api/deployments/${deploymentId}/runs/${runId}/temporal`);
}

export function listRunLogs(
  deploymentId: string,
  runId: string,
  params?: { stage?: string; node_id?: string },
): Promise<{ data: RunLogItem[]; total: number }> {
  return get<{ data: RunLogItem[]; total: number }>(
    `/api/deployments/${deploymentId}/runs/${runId}/logs`,
    params,
  );
}

export function listRunData(
  deploymentId: string,
  runId: string,
  params?: { stage?: string },
): Promise<{ node_data: NodeDataItem[]; global_data: GlobalDataItem[] }> {
  return get<{ node_data: NodeDataItem[]; global_data: GlobalDataItem[] }>(
    `/api/deployments/${deploymentId}/runs/${runId}/data`,
    params,
  );
}

export function listContainers(deploymentId: string): Promise<{ data: ContainerItem[]; total: number }> {
  return get<{ data: ContainerItem[]; total: number }>(`/api/deployments/${deploymentId}/containers`);
}

/** 容器日志流式（SSE）地址；相对当前文档基路径（应用可托管在子路径下） */
export function containerLogStreamUrl(
  deploymentId: string,
  nodeId: string,
  name: string,
  tail = 30,
): string {
  return runtimeUrl(
    `api/deployments/${encodeURIComponent(deploymentId)}` +
    `/nodes/${encodeURIComponent(nodeId)}` +
    `/containers/${encodeURIComponent(name)}/stream?tail=${tail}`
  );
}

export async function readContainerLogStream(
  url: string,
  signal: AbortSignal,
  onMessage: (data: string) => void,
): Promise<void> {
  const headers = await runtimeHeaders();
  const response = await fetch(url, {
    headers: { ...headers, Accept: 'text/event-stream' },
    signal,
    redirect: 'error',
  });
  if (!response.ok || !response.body ||
      !response.headers.get('content-type')?.startsWith('text/event-stream')) {
    throw new Error('Container log stream unavailable');
  }
  const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
  let pending = '';
  let lines: string[] = [];
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      pending += value;
      let end: number;
      while ((end = pending.indexOf('\n')) >= 0) {
        const line = pending.slice(0, end).replace(/\r$/, '');
        pending = pending.slice(end + 1);
        if (line === '') {
          if (lines.length) onMessage(lines.join('\n'));
          lines = [];
        } else if (line.startsWith('data:')) {
          lines.push(line.slice(5).replace(/^ /, ''));
        }
      }
    }
  } finally {
    await reader.cancel();
  }
}

export function getSettings(): Promise<SettingsData> {
  return get<SettingsData>('/api/settings');
}

export function updateSettings(req: {
  health_history_keep: number;
  run_history_keep: number;
}): Promise<SettingsData> {
  return put<SettingsData>('/api/settings', req);
}
