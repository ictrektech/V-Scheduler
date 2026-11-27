import { buildQuery, type PageMeta, type PageQuery } from './paging';
import { del, get, post, postForm, put } from './client';
import i18n from '@/i18n';

export interface PlazaSourceItem {
  id: string;
  name: string;
  base_url: string;
  enabled: boolean;
  last_synced_at?: string | null;
  card_count: number;
  created_at: string;
  updated_at: string;
}

export interface PlazaSourceListData {
  data: PlazaSourceItem[];
  total: number;
}

export interface PlazaSourceCreateReq {
  name: string;
  base_url: string;
  enabled?: boolean;
}

/** 广场卡片的一个版本（分组内条目） */
export interface PlazaCardVersion {
  version: string;
  size_bytes?: number | null;
  sha256?: string | null;
  /** manifest 绝对 URL */
  manifest: string;
  /** model.tar.gz 绝对 URL */
  package: string;
  /** 该版本是否已登记到本地 */
  registered: boolean;
  /** 已登记主键 `<card_id>@<version>` */
  plaza_model_id?: string | null;
}

/** 广场卡片分组：同一 card_id 的多个版本聚合为一张卡片 */
export interface PlazaCardGroup {
  id: string;
  /** 卡片展示名（manifest 的多语言值：对象或字符串），经 displayText() 取值 */
  name?: unknown;
  description?: unknown;
  category: string[];
  tags: string[];
  icon: string;
  /** 最新版本权重 id */
  model?: string;
  /** 最新版本镜像声明（string 或 {name, arch}） */
  images?: unknown[];
  /** 最新版本号（版本号最大者） */
  latest_version: string;
  /** 已登记版本数 */
  registered_count: number;
  /** 家族标识（后端 SSOT 推断，未命中为 `generic`） */
  family: string;
  /** 版本列表（降序） */
  versions: PlazaCardVersion[];
}

/** 广场卡片未筛选统计（页头统计条） */
export interface PlazaCardStats {
  cards: number;
  downloaded: number;
}

export interface PlazaCardListData extends PageMeta {
  data: PlazaCardGroup[];
  /** 未筛选统计 */
  stats: PlazaCardStats;
  source_id: string;
}

/** 广场卡片列表查询参数 */
export interface PlazaCardsQuery extends PageQuery {
  keyword?: string;
  registered_only?: boolean;
  /** 强制重新拉取上游索引并刷新同步元数据 */
  refresh?: boolean;
}

export interface PlazaModelItem {
  id: string;
  model_id: string;
  model: string;
  name: string;
  version: string;
  source: string;
  source_id: string;
  path: string;
  package_path: string;
  package_sha256: string;
  manifest_json: string;
  state: string;
  size_bytes: number;
  created_at: string;
  updated_at: string;
}

/** 本地模型卡片的一个版本（PlazaModelItem + 是否最新） */
export interface LocalModelVersion extends PlazaModelItem {
  latest: boolean;
}

/** 本地模型卡片分组：同一 model_id 的多个已登记版本聚合为一张卡片 */
export interface LocalModelGroup {
  model_id: string;
  /** 展示名（取最新版本的登记名） */
  name: string;
  /** 权重 id（manifest `model`） */
  model: string;
  /** 来源集合（`plaza` / `local`） */
  sources: string[];
  /** 最新版本镜像声明（string 或 {name, arch}） */
  images?: unknown[];
  latest_version: string;
  total_size_bytes: number;
  /** 家族标识（后端 SSOT 推断，未命中为 `generic`） */
  family: string;
  /** 版本列表（降序） */
  versions: LocalModelVersion[];
}

/** 本地卡片未筛选统计（页头统计条 / 家族下拉） */
export interface LocalModelStats {
  cards: number;
  versions: number;
  size_bytes: number;
  /** 出现过的家族标识 */
  families: string[];
}

export interface LocalModelListData extends PageMeta {
  data: LocalModelGroup[];
  /** 未筛选统计 */
  stats: LocalModelStats;
  plaza_server: string;
}

/** 本地卡片列表查询参数 */
export interface LocalModelsQuery extends PageQuery {
  keyword?: string;
  family?: string;
  source?: string;
  sort?: string;
}

export function listPlazaSources(): Promise<PlazaSourceListData> {
  return get<PlazaSourceListData>('/api/plaza/sources');
}

export function createPlazaSource(req: PlazaSourceCreateReq): Promise<PlazaSourceItem> {
  return post<PlazaSourceItem>('/api/plaza/sources', req);
}

export function updatePlazaSource(id: string, req: Partial<PlazaSourceCreateReq>): Promise<PlazaSourceItem> {
  return put<PlazaSourceItem>(`/api/plaza/sources/${id}`, req);
}

export function deletePlazaSource(id: string): Promise<{ deleted: string }> {
  return del<{ deleted: string }>(`/api/plaza/sources/${id}`);
}

export function listPlazaCards(
  sourceId: string,
  params: PlazaCardsQuery = {},
): Promise<PlazaCardListData> {
  return get<PlazaCardListData>(`/api/plaza/sources/${sourceId}/cards${buildQuery({ ...params })}`);
}

/** 本地卡片列表（服务端分页 / 筛选 / 排序） */
export function listPlazaModelGroups(params: LocalModelsQuery = {}): Promise<LocalModelListData> {
  return get<LocalModelListData>(`/api/plaza/models${buildQuery({ ...params })}`);
}

export function fetchCardManifest(sourceId: string, path: string): Promise<{ manifest: string }> {
  return get<{ manifest: string }>(`/api/plaza/sources/${sourceId}/manifest`, { path });
}

export function registerPlazaModel(req: {
  source_id: string;
  model_id: string;
  version: string;
}): Promise<PlazaModelItem> {
  return post<PlazaModelItem>('/api/plaza/models', req);
}

/**
 * 本地模型卡片（按 card_id 聚合，含全部已登记版本）。
 *
 * 用于**下拉选项**等需要"全部卡片"的场景：以最大页大小拉取（上限 100）。
 * 列表页请使用 `listPlazaModelGroups(params)` 做服务端分页。
 */
export function listPlazaModels(): Promise<LocalModelListData> {
  return listPlazaModelGroups({ page_size: 100 });
}

export function deletePlazaModel(id: string): Promise<{ deleted: string }> {
  return del<{ deleted: string }>(`/api/plaza/models/${id}`);
}

/**
 * 上传本地模型卡片包（包含 `model.tar.gz` 的 `tar.*` 归档），登记为本地模型卡片
 * （`source = "local"`）。
 */
export function uploadLocalCard(file: File): Promise<PlazaModelItem> {
  const form = new FormData();
  form.append('file', file, file.name);
  return postForm<PlazaModelItem>('/api/plaza/models/upload', form);
}

/** 展示名（多语言对象取当前应用语言 / zh-CN / 首个字符串） */
export function displayText(v: unknown): string {
  if (v == null) return '';
  if (typeof v === 'string') return v;
  if (typeof v === 'object') {
    const obj = v as Record<string, unknown>;
    const locale = String(i18n.global.locale.value);
    if (typeof obj[locale] === 'string') return obj[locale] as string;
    if (typeof obj['zh-CN'] === 'string') return obj['zh-CN'] as string;
    const first = Object.values(obj).find((x) => typeof x === 'string');
    if (typeof first === 'string') return first;
  }
  return '';
}