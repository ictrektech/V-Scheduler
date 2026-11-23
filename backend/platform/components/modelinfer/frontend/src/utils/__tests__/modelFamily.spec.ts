import { describe, expect, it } from 'vitest';
import {
  GENERIC_FAMILY,
  MODEL_FAMILIES,
  hexToRgba,
  modelFamilyIcon,
  modelFamilyLabel,
  modelFamilyTheme,
} from '../modelFamily';

describe('modelFamilyTheme', () => {
  it('按家族键返回展示主题', () => {
    expect(modelFamilyTheme('deepseek').labelKey).toBe('modelFamily.deepseek');
    expect(modelFamilyTheme('deepseek').accent).toBe('#4d6bfe');
    expect(modelFamilyTheme('qwen').labelKey).toBe('modelFamily.qwen');
  });

  it('modelFamilyLabel 经传入 t 渲染（默认中文）', () => {
    expect(modelFamilyLabel('deepseek', (k) => (k === 'modelFamily.deepseek' ? 'DeepSeek' : k))).toBe(
      'DeepSeek',
    );
    expect(modelFamilyLabel('qwen', (k) => (k === 'modelFamily.qwen' ? 'Qwen（通义千问）' : k))).toBe(
      'Qwen（通义千问）',
    );
    // 缺省使用 i18n 全局单例（默认 zh-CN）
    expect(modelFamilyLabel('deepseek')).toBe('DeepSeek');
  });

  it('未知 / 空家族回落兜底且不抛错', () => {
    expect(modelFamilyTheme('nope').family).toBe('generic');
    expect(modelFamilyTheme(null).family).toBe('generic');
    expect(modelFamilyTheme(undefined).family).toBe('generic');
    expect(modelFamilyTheme('').family).toBe(GENERIC_FAMILY.family);
  });
});

describe('modelFamilyIcon', () => {
  it('图标文件名与家族键一致', () => {
    expect(modelFamilyIcon('glm')).toBe('glm');
    expect(modelFamilyIcon('kimi')).toBe('kimi');
  });

  it('未知家族回落 generic', () => {
    expect(modelFamilyIcon('nope')).toBe('generic');
    expect(modelFamilyIcon(null)).toBe('generic');
  });
});

describe('家族呈现表', () => {
  it('包含兜底家族且键唯一', () => {
    const keys = MODEL_FAMILIES.map((f) => f.family);
    expect(keys).toContain('generic');
    expect(new Set(keys).size).toBe(keys.length);
  });

  it('每个家族都有非空 labelKey 与合法品牌色', () => {
    for (const def of MODEL_FAMILIES) {
      expect(def.labelKey.startsWith('modelFamily.')).toBe(true);
      expect(def.accent).toMatch(/^#[0-9a-f]{6}$/i);
    }
  });
});

describe('hexToRgba', () => {
  it('支持 6 位与 3 位十六进制', () => {
    expect(hexToRgba('#4d6bfe', 0.5)).toBe('rgba(77, 107, 254, 0.5)');
    expect(hexToRgba('#fff', 1)).toBe('rgba(255, 255, 255, 1)');
  });

  it('非法输入回落中性灰且不抛错', () => {
    expect(hexToRgba('not-a-color', 0.2)).toBe('rgba(100, 116, 139, 0.2)');
    expect(hexToRgba('#12', 0.2)).toBe('rgba(100, 116, 139, 0.2)');
  });
});