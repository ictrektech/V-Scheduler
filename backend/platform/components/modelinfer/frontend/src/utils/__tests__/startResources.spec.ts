import { describe, expect, it, vi } from 'vitest';
import { ModelinferApiError, request } from '@/api/client';
import client from '@/api/client';
import { missingStartResources } from '../startResources';

const resources = {
  weights: [{ model_id: 'org/override', source: 'ms', overridden: true }],
  images: ['registry.example/engine:2'],
  images_source: 'override',
};

describe('启动资源错误', () => {
  it('请求层保留后端返回的生效资源名称和来源', async () => {
    const mock = vi.spyOn(client, 'request').mockResolvedValueOnce({
      data: { code: 20751, msg: 'missing', category: 'modelinfer', data: resources },
    });
    try {
      await request({ url: '/api/deployments/d/start', method: 'POST' });
      expect.fail('应抛出业务错误');
    } catch (error) {
      expect(missingStartResources(error)).toEqual(resources);
    } finally {
      mock.mockRestore();
    }
  });

  it('支持仅缺权重或仅缺镜像', () => {
    for (const data of [{ ...resources, images: [] }, { ...resources, weights: [] }]) {
      expect(missingStartResources(new ModelinferApiError(20751, '', 'modelinfer', data))).toEqual(data);
    }
  });

  it('连接错误、旧响应和非法清单使用普通错误提示', () => {
    expect(missingStartResources(new ModelinferApiError(20730, '', 'modelinfer', resources))).toBeNull();
    expect(missingStartResources(new Error('missing'))).toBeNull();
    for (const data of [undefined, null, {}, { ...resources, weights: [null] },
      { ...resources, images: [123] }, { ...resources, images: [], weights: [] }]) {
      expect(missingStartResources(new ModelinferApiError(20751, '', 'modelinfer', data))).toBeNull();
    }
  });
});
