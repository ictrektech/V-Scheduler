import { get } from './client';

export interface ModelHubVariant {
  source: string;
  model_id: string;
  rel_path: string;
  abs_path: string;
}

export interface ModelHubInfo {
  dir: string;
  exists: boolean;
  hf_count: number;
  ms_count: number;
  models: ModelHubVariant[];
}

export interface VariantListData {
  data: ModelHubVariant[];
  total: number;
  weight_id: string;
}

/** 变体内一个文件 */
export interface VariantFile {
  rel_path: string;
  size_bytes: number;
}

/** 变体文件清单 */
export interface VariantTreeData {
  source: string;
  weight_id: string;
  rel_root: string;
  files: VariantFile[];
  total: number;
}

export function fetchModelHubInfo(): Promise<ModelHubInfo> {
  return get<ModelHubInfo>('/api/model-hub/info');
}

export function matchModelVariants(weightId: string): Promise<VariantListData> {
  return get<VariantListData>('/api/model-hub/match', { weight_id: weightId });
}

/** 列举 model-hub 某变体内的文件（供 ComfyUI 权重挑选） */
export function fetchVariantFiles(source: string, weightId: string): Promise<VariantTreeData> {
  return get<VariantTreeData>('/api/model-hub/tree', { source, weight_id: weightId });
}