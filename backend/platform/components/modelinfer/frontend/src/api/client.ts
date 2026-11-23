import axios from 'axios';
import type { AxiosRequestConfig } from 'axios';
import { getVOSBearerToken } from '@scheduler-auth';

const controller = import.meta.env.VITE_SCHEDULER_INFERENCE === 'true'
  ? '/api/com.ictrek.v-scheduler/scheduler/api/v1/inference/runtime/'
  : null;

export const embeddedRuntime = controller !== null;

export function runtimeUrl(path: string): string {
  return controller ? controller + path.replace(/^\/?api\//, '') : './' + path.replace(/^\//, '');
}

export async function runtimeHeaders(): Promise<Record<string, string>> {
  if (!controller) return {};
  const token = await getVOSBearerToken();
  if (!token) throw new Error('VOS authentication required');
  return { Authorization: `Bearer ${token}` };
}

/**
 * VOS HTTP 统一响应结构（对齐 specs/docs/design/vos-http-response-spec/）
 * code == 0 成功；code != 0 失败（msg 仅调试辅助）
 */
export interface ResultBody<T> {
  code: number;
  category: string;
  data?: T;
  msg: string;
  version: string;
}

/**
 * 携带后端稳定数字码的 API 错误。
 * 视图层经 `i18n/api-error.ts` 的 `resolveApiErrorMessage` 按码渲染本地化提示。
 */
export class ModelinferApiError extends Error {
  /** 后端业务码（ResultBody.code，!= 0） */
  readonly code: number;
  /** 后端 category（如 `modelinfer`） */
  readonly category: string;
  /** 结构化错误详情（如启动前缺失资源清单） */
  readonly data: unknown;

  constructor(code: number, msg: string, category: string, data?: unknown) {
    super(msg || `[${code}] ${category}`);
    this.name = 'ModelinferApiError';
    this.code = code;
    this.category = category;
    this.data = data;
  }
}

const client = axios.create({
  timeout: 120_000,
});

/** 请求包装：解析 ResultBody，按 code 判成败 */
export async function request<T>(config: AxiosRequestConfig): Promise<T> {
  const resp = await client.request<ResultBody<T>>({
    ...config,
    url: runtimeUrl(config.url ?? ''),
    headers: { ...config.headers, ...await runtimeHeaders() },
  });
  const body = resp.data;
  if (body.code !== 0) {
    throw new ModelinferApiError(body.code, body.msg, body.category, body.data);
  }
  return body.data as T;
}

export function get<T>(url: string, params?: Record<string, unknown>): Promise<T> {
  return request<T>({ url, method: 'GET', params });
}

export function post<T>(url: string, data?: unknown): Promise<T> {
  return request<T>({ url, method: 'POST', data });
}

export function put<T>(url: string, data?: unknown): Promise<T> {
  return request<T>({ url, method: 'PUT', data });
}

export function del<T>(url: string, params?: Record<string, unknown>): Promise<T> {
  return request<T>({ url, method: 'DELETE', params });
}

/**
 * multipart/form-data 上传（默认 10 分钟超时，适配较大的模型卡片包）。
 * 不显式设置 Content-Type，由浏览器/axios 自动补齐 boundary。
 */
export function postForm<T>(url: string, form: FormData, timeout = 600_000): Promise<T> {
  return request<T>({ url, method: 'POST', data: form, timeout });
}

export default client;
