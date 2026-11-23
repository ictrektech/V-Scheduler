import { delWithBody, get, post, put, request } from './client';
import type { MirrorOption } from '@/utils/mirror';
import type { GcStatus, GcSummary } from '@/utils/gc';
import type { TaskProgressDetail, TaskStatus } from '@/utils/task';

export type { MirrorOption, TaskProgressDetail, TaskStatus, GcStatus, GcSummary };

/** manifest list / image index 的平台条目（多架构镜像） */
export interface PlatformEntry {
  os: string | null;
  architecture: string | null;
  variant: string | null;
  digest: string | null;
  size_bytes: number | null;
}

/** 单个标签摘要 */
export interface ImageTag {
  tag: string;
  digest: string | null;
  size_bytes: number | null;
  created: string | null;
  media_type: string | null;
  platforms?: PlatformEntry[];
}

/** 一个仓库及其标签（无标签仓库不会出现在列表中，故无需「是否可删」标记） */
export interface RepositoryImages {
  repository: string;
  tags: ImageTag[];
}

export interface ImageListData {
  count: number;
  images: RepositoryImages[];
}

/** 仓库详情（单仓库标签列表） */
export interface RepositoryDetail {
  repository: string;
  tags: ImageTag[];
}

export interface RegistryInfo {
  registry_port: number;
}

export interface NetworkAddresses {
  ip_addr: string[];
}

export interface DeleteResult {
  repository: string;
  deleted_tags: string[];
}

/** 任务状态（含已取消） */
export type JobStatus = TaskStatus;

/** 异步任务 */
export interface Job {
  id: string;
  kind: string;
  title: string;
  /** 实际拉取源（使用镜像源时为 `<mirror>/<repo>:<tag>`） */
  source: string;
  /** 本地落地引用（恒为上游完整镜像名派生，不含镜像源主机名） */
  dest: string;
  /** 本次使用的镜像源主机名（未使用为 null） */
  mirror: string | null;
  arch: string;
  status: JobStatus;
  /** 友好中文进度摘要（非原始输出行；前端优先按结构字段本地化） */
  progress: string;
  /** 结构化进度（阶段 / 计数 / 百分比；暂不可推导的百分比为 null） */
  progress_detail: TaskProgressDetail;
  /** 是否已请求停止（运行中且为 true 时 UI 显示「正在停止…」） */
  cancel_requested: boolean;
  /** 单次上游传输超时（分钟；`0` = 无限制） */
  timeout_minutes: number;
  /** 传输失败重试次数（失败后最多再试 N 次） */
  retry_times: number;
  /** registry 垃圾回收结果摘要（仅 kind = gc 任务；其余任务为 null） */
  gc?: GcSummary | null;
  logs: string[];
  created_at: string;
  finished_at: string | null;
  /** 失败原因原文（后端调试文案；前端优先用 error_code 本地化） */
  error: string | null;
  /** 失败错误码（image-hub 208xx；成功/取消/旧记录为 null） */
  error_code?: number | null;
}

/** 镜像源目录响应 */
export interface MirrorListData {
  count: number;
  mirrors: MirrorOption[];
}

export interface TaskListData {
  count: number;
  tasks: Job[];
}

export interface TaskStarted {
  task_id: string;
}

/** 停止请求结果 */
export interface TaskCancelResult {
  id: string;
  status: JobStatus;
  cancel_requested: boolean;
}

/** 上游 registry 认证视图（密码只写不读） */
export interface RegistryView {
  host: string;
  username: string;
  has_password: boolean;
}

export interface ProxyConfig {
  enabled: boolean;
  http_proxy: string;
  https_proxy: string;
  no_proxy: string;
}

export interface SettingsData {
  registries: RegistryView[];
  proxy: ProxyConfig;
}

export interface RegistryListData {
  registries: RegistryView[];
}

export type ArchChoice = 'all' | 'host' | string;

/** 获取 registry 展示端口 */
export function fetchRegistryInfo(): Promise<RegistryInfo> {
  return get<RegistryInfo>('api/registry/info');
}

/** 枚举 registry 中的镜像 */
export function fetchImages(): Promise<ImageListData> {
  return get<ImageListData>('api/registry/images');
}

/** 单仓库标签详情（含多架构平台条目） */
export function fetchImageDetail(repository: string): Promise<RepositoryDetail> {
  return get<RepositoryDetail>('api/registry/images/detail', { repository });
}

/** 删除镜像：tag 缺省 = 删除整仓全部标签 */
export function deleteImage(repository: string, tag?: string): Promise<DeleteResult> {
  return delWithBody<DeleteResult>('api/registry/images', { repository, tag });
}

/** 内置镜像源目录（上游 registry → 候选镜像源） */
export function fetchMirrors(): Promise<MirrorListData> {
  return get<MirrorListData>('api/registry/mirrors');
}

/**
 * 从上游拉取镜像并推入本地 registry（mirror 缺省 = 不使用镜像源；
 * `timeoutMinutes = 0` = 无限制；`retryTimes` 缺省由后端取默认 2）
 */
export function pullImage(
  image: string,
  arch: ArchChoice,
  insecure: boolean,
  mirror?: string | null,
  timeoutMinutes?: number | null,
  retryTimes?: number | null,
): Promise<TaskStarted> {
  return post<TaskStarted>('api/registry/pull', {
    image,
    arch,
    insecure,
    mirror: mirror || null,
    timeout_minutes: timeoutMinutes ?? null,
    retry_times: retryTimes ?? null,
  });
}

/** 同步本地已有镜像（tag 缺省同步全部标签；mirror 仅改变实际拉取来源） */
export function syncImage(
  repository: string,
  tag: string | undefined,
  arch: ArchChoice,
  insecure: boolean,
  mirror?: string | null,
  timeoutMinutes?: number | null,
  retryTimes?: number | null,
): Promise<TaskStarted> {
  return post<TaskStarted>('api/registry/sync', {
    repository,
    tag,
    arch,
    insecure,
    mirror: mirror || null,
    timeout_minutes: timeoutMinutes ?? null,
    retry_times: retryTimes ?? null,
  });
}

/** 请求停止运行中的任务（幂等；不存在 20808 / 已结束 20801） */
export function cancelTask(id: string): Promise<TaskCancelResult> {
  return post<TaskCancelResult>(`api/tasks/${encodeURIComponent(id)}/cancel`, {});
}

/** 克隆 / 改名请求参数（tag 缺省 = 整仓；目标缺省 = 与源相同） */
export interface CopyRequest {
  repository: string;
  tag?: string;
  target_repository?: string;
  target_tag?: string;
  timeout_minutes?: number | null;
  retry_times?: number | null;
}

/** 克隆本地镜像 / 标签到新名称（源保持不变） */
export function cloneImage(req: CopyRequest): Promise<TaskStarted> {
  return post<TaskStarted>('api/registry/clone', req);
}

/** 改名本地镜像 / 标签（复制到新名称后删除源引用） */
export function renameImage(req: CopyRequest): Promise<TaskStarted> {
  return post<TaskStarted>('api/registry/rename', req);
}

/** 归档导入选项 */
export interface ImportOptions {
  /** 目标仓库名（缺省 = 保留归档内原始名称） */
  repository?: string | null;
  /** 目标标签（缺省 = 保留归档内原始标签） */
  tag?: string | null;
  timeoutMinutes?: number | null;
  retryTimes?: number | null;
}

/**
 * 上传镜像归档并导入本地 registry（docker-archive / OCI 归档，后缀不限）。
 *
 * `onProgress` 为上传进度百分比（0~100）；上传完成后后端异步导入，返回任务标识。
 */
export function importArchive(
  file: File,
  opts: ImportOptions = {},
  onProgress?: (percent: number) => void,
): Promise<TaskStarted> {
  const form = new FormData();
  form.append('file', file);
  if (opts.repository) form.append('repository', opts.repository);
  if (opts.tag) form.append('tag', opts.tag);
  if (opts.timeoutMinutes !== undefined && opts.timeoutMinutes !== null) {
    form.append('timeout_minutes', String(opts.timeoutMinutes));
  }
  if (opts.retryTimes !== undefined && opts.retryTimes !== null) {
    form.append('retry_times', String(opts.retryTimes));
  }
  return request<TaskStarted>({
    url: 'api/registry/import',
    method: 'POST',
    data: form,
    // 上传大镜像归档远超默认 30s
    timeout: 0,
    onUploadProgress: (e) => {
      if (!onProgress) return;
      const total = e.total ?? 0;
      if (total > 0) onProgress(Math.round((e.loaded / total) * 100));
    },
  });
}

/** 任务列表 */
export function fetchTasks(): Promise<TaskListData> {
  return get<TaskListData>('api/tasks');
}

/** 任务详情 */
export function fetchTask(id: string): Promise<Job> {
  return get<Job>(`api/tasks/${encodeURIComponent(id)}`);
}

/** 清理终态任务记录 */
export function deleteTask(id: string): Promise<{ removed: string }> {
  return delWithBody<{ removed: string }>(`api/tasks/${encodeURIComponent(id)}`);
}

/** 设置（上游认证 + 代理） */
export function fetchSettings(): Promise<SettingsData> {
  return get<SettingsData>('api/settings');
}

/** 新增/更新上游认证（password 为空保留原密码） */
export function saveRegistryCredential(
  host: string,
  username: string,
  password: string,
): Promise<RegistryListData> {
  return put<RegistryListData>('api/settings/registries', { host, username, password });
}

/** 删除上游认证 */
export function deleteRegistryCredential(host: string): Promise<RegistryListData> {
  return delWithBody<RegistryListData>('api/settings/registries', { host });
}

/** 更新代理配置 */
export function saveProxy(proxy: ProxyConfig): Promise<ProxyConfig> {
  return put<ProxyConfig>('api/settings/proxy', proxy);
}

/** 平台设备网卡地址列表（失败时返回空数组） */
export function fetchAddresses(): Promise<NetworkAddresses> {
  return get<NetworkAddresses>('api/network/addresses');
}

/** 更新自动回收配置请求（interval_hours / delete_untagged 缺省保留现值） */
export interface GcConfigRequest {
  enabled: boolean;
  interval_hours?: number;
  delete_untagged?: boolean;
}

/** 手动触发 GC 选项 */
export interface GcRunOptions {
  /** 是否删除未打标签 manifest（缺省取后端配置值） */
  deleteUntagged?: boolean | null;
  /** 预演（只统计不删除） */
  dryRun?: boolean;
  timeoutMinutes?: number | null;
}

/** registry 垃圾回收状态与配置（含存储占用、能力、下次运行时间） */
export function fetchGcStatus(): Promise<GcStatus> {
  return get<GcStatus>('api/registry/gc');
}

/** 保存自动回收配置（返回更新后的状态） */
export function saveGcConfig(req: GcConfigRequest): Promise<GcStatus> {
  return put<GcStatus>('api/registry/gc', req);
}

/** 手动触发垃圾回收（异步任务；支持预演 / 删除无标签 manifest） */
export function runGc(opts: GcRunOptions = {}): Promise<TaskStarted> {
  return post<TaskStarted>('api/registry/gc/run', {
    delete_untagged: opts.deleteUntagged ?? null,
    dry_run: opts.dryRun ?? false,
    timeout_minutes: opts.timeoutMinutes ?? null,
  });
}