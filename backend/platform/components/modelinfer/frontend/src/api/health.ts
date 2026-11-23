import { get } from './client';

export interface HealthData {
  status: string;
  version: string;
  platform_token_present: boolean;
  db_ready: boolean;
  timestamp: string;
}

export function fetchHealth(): Promise<HealthData> {
  return get<HealthData>('/api/health');
}
