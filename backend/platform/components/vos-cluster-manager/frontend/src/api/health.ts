import { get } from './client';

export interface HealthData {
  status: string;
  version: string;
  /** etcd 端点是否已配置 */
  etcd_configured: boolean;
  /** etcd 实探测结果（Phase 1+） */
  etcd_ok: boolean;
  /** 管理面状态库可用性（平台 postgres，未注入/连接失败为 false） */
  db_ok: boolean;
  timestamp: string;
}

export function fetchHealth(): Promise<HealthData> {
  return get<HealthData>('/api/health');
}