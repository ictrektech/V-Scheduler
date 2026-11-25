import axios from 'axios';
import type { AxiosRequestConfig } from 'axios';
import { withRuntimeToken } from '@scheduler-auth';

const runtime = import.meta.env.VITE_SCHEDULER_IMAGE_HUB === 'true'
  ? '/app/com.ictrek.v-scheduler/api/v1/image-hub/runtime/'
  : null;

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

const client = axios.create({
  baseURL: runtime || './',
  timeout: 30_000,
});

/**
 * 带业务错误码的 API 错误。
 *
 * `code` 为后端 `ResultBody.code`（image-hub 区间 20800+），前端据此按 `errors.<code>`
 * 本地化提示；`rawMsg` 为后端调试文案（中文），仅用于日志/排障，不作为英文界面主文案。
 */
export class ImageHubApiError extends Error {
  readonly code: number;
  readonly category: string;
  readonly rawMsg: string;

  constructor(code: number, category: string, msg: string) {
    super(`[${code}] ${msg || category}`);
    this.name = 'ImageHubApiError';
    this.code = code;
    this.category = category;
    this.rawMsg = msg;
  }
}

/** 请求包装：解析 ResultBody，按 code 判成败；业务错误保留 code 供 i18n 映射 */
export async function request<T>(config: AxiosRequestConfig): Promise<T> {
  const send = (token?: string) => client.request<ResultBody<T>>(token ? {
    ...config,
    url: config.url?.replace(/^\/?api\//, ''),
    headers: { ...config.headers, Authorization: `Bearer ${token}` },
  } : config);
  const read = (config.method || 'GET').toUpperCase() === 'GET';
  const resp = runtime ? await withRuntimeToken(send, read) : await send();
  const body = resp.data;
  if (body.code !== 0) {
    throw new ImageHubApiError(body.code, body.category, body.msg);
  }
  return body.data as T;
}

/** 便捷 GET */
export function get<T>(url: string, params?: Record<string, unknown>): Promise<T> {
  return request<T>({ url, method: 'GET', params });
}

/** 便捷 DELETE（带 JSON body） */
export function delWithBody<T>(url: string, data?: unknown): Promise<T> {
  return request<T>({ url, method: 'DELETE', data });
}

/** 便捷 POST */
export function post<T>(url: string, data?: unknown): Promise<T> {
  return request<T>({ url, method: 'POST', data });
}

/** 便捷 PUT */
export function put<T>(url: string, data?: unknown): Promise<T> {
  return request<T>({ url, method: 'PUT', data });
}

export default client;