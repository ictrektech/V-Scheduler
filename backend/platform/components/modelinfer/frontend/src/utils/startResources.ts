import { ModelinferApiError } from '@/api/client';

export interface MissingStartResources {
  weights: { source: string; model_id: string; overridden: boolean }[];
  images: string[];
  images_source: 'card' | 'override';
}

/** 只消费稳定业务码与结构化数据，避免从后端错误文案猜测资源名称。 */
export function missingStartResources(error: unknown): MissingStartResources | null {
  if (!(error instanceof ModelinferApiError) || error.code !== 20751) return null;
  const data = error.data as Partial<MissingStartResources> | null;
  if (
    !data ||
    !Array.isArray(data.weights) ||
    !data.weights.every((w) => w && typeof w.source === 'string' &&
      typeof w.model_id === 'string' && typeof w.overridden === 'boolean') ||
    !Array.isArray(data.images) ||
    !data.images.every((image) => typeof image === 'string') ||
    !['card', 'override'].includes(data.images_source ?? '') ||
    (!data.weights.length && !data.images.length)
  ) return null;
  return data as MissingStartResources;
}
