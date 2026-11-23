import { get, post } from './client';

export interface NfsResolveResult {
  config_name: string;
  host_path: string;
  volume_uuid: string;
  volume_name: string;
  nfs_server: string;
  nfs_export_path: string;
  sub_path: string;
  read_only: boolean;
  mounted: boolean;
  mount_source: string;
}

export interface NodeMountResult {
  node_id: string;
  name: string;
  host: string;
  mount_dir: string;
  ok: boolean;
  message: string;
}

export interface MountResultData {
  results: NodeMountResult[];
  mount_dir: string;
  success_count: number;
  fail_count: number;
}

export interface MountReq {
  node_ids?: string[];
  mount_dir?: string;
}

export function fetchNfsInfo(): Promise<NfsResolveResult> {
  return get<NfsResolveResult>('/api/nfs/info');
}

export function mountNfs(req: MountReq): Promise<MountResultData> {
  return post<MountResultData>('/api/nfs/mount', req);
}

export function umountNfs(req: MountReq): Promise<MountResultData> {
  return post<MountResultData>('/api/nfs/umount', req);
}

export function nfsStatus(mountDir?: string): Promise<MountResultData> {
  return get<MountResultData>('/api/nfs/status', mountDir ? { mount_dir: mountDir } : undefined);
}
