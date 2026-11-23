import { describe, expect, it } from 'vitest';
import { createI18n } from 'vue-i18n';
import zhCN from '../locales/zh-CN';
import en from '../locales/en';
import { translate, hasTranslation } from '../index';
import { resolveApiErrorMessage } from '../api-error';
import { ModelinferApiError } from '@/api/client';

/** 按点分路径取叶子值 */
function pick(obj: Record<string, unknown>, path: string): unknown {
  return path.split('.').reduce<unknown>((acc, k) => {
    if (acc && typeof acc === 'object') return (acc as Record<string, unknown>)[k];
    return undefined;
  }, obj);
}

/** 独立 i18n 实例翻译（避免污染全局单例语言状态） */
function translateIn(locale: 'zh-CN' | 'en', key: string, named?: Record<string, unknown>): string {
  const messages = (locale === 'zh-CN' ? zhCN : en) as unknown as Record<string, unknown>;
  const i18n = createI18n({
    legacy: false,
    locale,
    messages: { [locale]: messages } as never,
  });
  const t = (i18n.global as unknown as {
    t: (k: string, n?: Record<string, unknown>) => string;
  }).t;
  return t(key, named);
}

/** 递归收集叶子 key（点分路径） */
function flatten(obj: Record<string, unknown>, prefix = ''): string[] {
  const out: string[] = [];
  for (const [k, v] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      out.push(...flatten(v as Record<string, unknown>, path));
    } else {
      out.push(path);
    }
  }
  return out.sort();
}

describe('i18n key 对称', () => {
  it('zh-CN 与 en 的叶子 key 完全一致', () => {
    const zhKeys = flatten(zhCN as unknown as Record<string, unknown>);
    const enKeys = flatten(en as unknown as Record<string, unknown>);
    expect(enKeys).toEqual(zhKeys);
  });

  it('无空文案', () => {
    const collectValues = (obj: Record<string, unknown>): string[] => {
      const out: string[] = [];
      for (const v of Object.values(obj)) {
        if (v && typeof v === 'object' && !Array.isArray(v)) {
          out.push(...collectValues(v as Record<string, unknown>));
        } else if (typeof v === 'string') {
          out.push(v);
        }
      }
      return out;
    };
    for (const v of [...collectValues(zhCN as unknown as Record<string, unknown>), ...collectValues(en as unknown as Record<string, unknown>)]) {
      expect(v.trim().length).toBeGreaterThan(0);
    }
  });
});

describe('错误码本地化', () => {
  it('已知错误码 20701 有 i18n 且中文命中', () => {
    expect(hasTranslation('error.20701')).toBe(true);
    expect(translate('error.20701')).toBe('参数校验失败');
  });

  it('已知码解析为本地化文案', () => {
    const err = new ModelinferApiError(20701, 'param invalid', 'modelinfer');
    expect(resolveApiErrorMessage(err)).toBe('参数校验失败');
  });

  it('未知码回退后端 msg', () => {
    const err = new ModelinferApiError(20888, 'backend raw message', 'modelinfer');
    expect(resolveApiErrorMessage(err)).toBe('backend raw message');
  });

  it('NFS 共享错误保留目标存储空间及路径，并提供开启共享的指引', () => {
    const detail = '权重目录 /share/weights/models 所在的存储空间「模型盘」（volume-1）未开启 NFS 共享。';
    const text = resolveApiErrorMessage(new ModelinferApiError(20750, detail, 'modelinfer'));
    expect(text).toContain(detail);
    expect(text).toContain('开启 NFS 共享');
    expect(translateIn('en', 'error.20750')).toContain('Enable NFS sharing');
    expect(translateIn('zh-CN', 'runs.startFailed', { name: '模型配置' })).toBe('启动失败：模型配置');
    expect(translateIn('en', 'runs.startFailed', { name: 'model' })).toBe('Failed to start: model');
  });

  it('NFS 共享错误无详情时仍保留完整指引', () => {
    const hint = translate('error.20750');
    expect(resolveApiErrorMessage(new ModelinferApiError(20750, '  ', 'modelinfer'))).toBe(hint);
    expect(resolveApiErrorMessage(new ModelinferApiError(20750, hint, 'modelinfer'))).toBe(hint);
  });

  it('非 API 错误回退通用提示', () => {
    expect(resolveApiErrorMessage('boom')).toBe('请求失败，请稍后重试');
  });
});

describe('i18n 消息中的字面量 @（模型主键 <card_id>@<version>）', () => {
  it('所有文案均无未转义的 @（字面量必须写作 {\'@\'}）', () => {
    const offenders: string[] = [];
    for (const [loc, messages] of [
      ['zh-CN', zhCN],
      ['en', en],
    ] as const) {
      const obj = messages as unknown as Record<string, unknown>;
      for (const key of flatten(obj)) {
        const value = pick(obj, key);
        // `{'@'}` 是 vue-i18n 对字面量 @ 的转义；去掉后若仍残留 @ 则运行时编译必失败
        if (typeof value === 'string' && value.replace(/\{'@'\}/g, '').includes('@')) {
          offenders.push(`${loc}:${key}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it('删除确认 / 上传成功 / 广场登记文案保留 @ 并完成插值', () => {
    expect(translateIn('zh-CN', 'cards.deleteConfirm', { modelId: 'com.ictrek.x', version: '1.0.0' })).toBe(
      '确定删除 com.ictrek.x@1.0.0？本地文件（model.tar.gz 与解压目录）一并删除。',
    );
    expect(translateIn('zh-CN', 'cards.uploadSuccess', { name: 'X', modelId: 'com.ictrek.x', version: '1.0.0' })).toBe(
      '本地模型卡片已登记：X（com.ictrek.x@1.0.0）',
    );
    expect(translateIn('zh-CN', 'plaza.download.registered', { model: 'com.ictrek.x', version: '1.0.0' })).toBe(
      'com.ictrek.x@1.0.0 已下载并登记到本地模型卡片',
    );

    expect(translateIn('en', 'cards.deleteConfirm', { modelId: 'com.ictrek.x', version: '1.0.0' })).toBe(
      'Delete com.ictrek.x@1.0.0? The local files (model.tar.gz and extracted directory) will be deleted as well.',
    );
    expect(translateIn('en', 'cards.uploadSuccess', { name: 'X', modelId: 'com.ictrek.x', version: '1.0.0' })).toBe(
      'Local model card registered: X (com.ictrek.x@1.0.0)',
    );
    expect(translateIn('en', 'plaza.download.registered', { model: 'com.ictrek.x', version: '1.0.0' })).toBe(
      'com.ictrek.x@1.0.0 downloaded and registered to local model cards',
    );
  });
});
