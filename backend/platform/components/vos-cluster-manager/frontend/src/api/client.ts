import axios from 'axios';
import type { AxiosRequestConfig } from 'axios';
import { getVOSBearerToken } from '@scheduler-auth';

const runtime = import.meta.env.VITE_SCHEDULER_CLUSTER === 'true'
  ? '/api/com.ictrek.v-scheduler/scheduler/api/v1/cluster/runtime/'
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

/** 请求包装：解析 ResultBody，按 code 判成败；HTTP 层异常映射为 2xxx 内部错误 */
export async function request<T>(config: AxiosRequestConfig): Promise<T> {
  let requestConfig = config;
  if (runtime) {
    const token = await getVOSBearerToken();
    if (!token) throw new Error('VOS authentication required');
    requestConfig = {
      ...config,
      url: config.url?.replace(/^\/?api\//, ''),
      headers: { ...config.headers, Authorization: `Bearer ${token}` },
    };
  }
  const resp = await client.request<ResultBody<T>>(requestConfig);
  const body = resp.data;
  if (body.code !== 0) {
    throw new Error(`[${body.code}] ${body.msg || body.category}`);
  }
  return body.data as T;
}

/** 单请求覆盖项（如长耗时编排接口放宽超时；缺省沿用实例 30s） */
export interface RequestOptions {
  /** 本次请求超时（毫秒） */
  timeout?: number;
}

/** 便捷 GET */
export function get<T>(url: string, params?: Record<string, unknown>): Promise<T> {
  return request<T>({ url, method: 'GET', params });
}

/** 便捷 POST */
export function post<T>(url: string, data?: unknown, options?: RequestOptions): Promise<T> {
  return request<T>({ url, method: 'POST', data, ...options });
}

/** 便捷 DELETE */
export function del<T>(url: string, params?: Record<string, unknown>, options?: RequestOptions): Promise<T> {
  return request<T>({ url, method: 'DELETE', params, ...options });
}

export default client;
