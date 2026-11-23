import { describe, expect, it } from 'vitest';
import { parseManifest } from '../manifest';

const SAMPLE = `id: com.ictrek.demo
name:
  zh-CN: 演示模型
  en: Demo Model
version: 1.0.0
author: ictrek
arch: arm64
category: [llm, moe]
tags: [qwen, vllm]
caps:
  max-nodes: 4
model: Qwen/Qwen3.8-27B
images:
  - registry.ictrek.internal/ictrek/vllm:v1
  - name: registry.ictrek.internal/ictrek/vllm-arm64:v1
    arch: arm64
configs:
  - name: MAX_MODEL_LEN
    type: integer
    title:
      zh-CN: 最大上下文长度
    default: 32768
    minimum: 1024
    maximum: 262144
  - name: MOE_BACKEND
    type: string
    default: marlin
    enum: [marlin, cutlass]
  - name: API_KEY
    type: string
    description:
      zh-CN: 留空自动生成
lifecycle:
  start: ./scripts/start.sh
  health-probe: ["python", "scripts/health.py"]
lifecycle-order:
  start: worker-first-reverse
lifecycle-timeout:
  start: 3600
`;

describe('parseManifest', () => {
  it('解析基本信息与多语言名称', () => {
    const v = parseManifest(SAMPLE)!;
    expect(v).not.toBeNull();
    expect(v.id).toBe('com.ictrek.demo');
    expect(v.name).toBe('演示模型');
    expect(v.version).toBe('1.0.0');
    expect(v.arch).toBe('arm64');
    expect(v.model).toBe('Qwen/Qwen3.8-27B');
    expect(v.category).toEqual(['llm', 'moe']);
    expect(v.maxNodes).toBe('4');
  });

  it('解析镜像（字符串与对象 / 通用架构）', () => {
    const v = parseManifest(SAMPLE)!;
    expect(v.images).toEqual([
      { name: 'registry.ictrek.internal/ictrek/vllm:v1', arch: '（通用）' },
      { name: 'registry.ictrek.internal/ictrek/vllm-arm64:v1', arch: 'arm64' },
    ]);
  });

  it('解析可用配置（默认值 / 约束 / 必填）', () => {
    const v = parseManifest(SAMPLE)!;
    expect(v.configs).toHaveLength(3);
    const maxLen = v.configs[0];
    expect(maxLen.name).toBe('MAX_MODEL_LEN');
    expect(maxLen.title).toBe('最大上下文长度');
    expect(maxLen.defaultValue).toBe('32768');
    expect(maxLen.constraint).toBe('≥ 1024，≤ 262144');
    expect(maxLen.required).toBe(false);

    const moe = v.configs[1];
    expect(moe.constraint).toBe('枚举：marlin / cutlass');

    const apiKey = v.configs[2];
    expect(apiKey.defaultValue).toBe('（未定义，必填）');
    expect(apiKey.required).toBe(true);
    expect(apiKey.constraint).toBe('');
    expect(apiKey.description).toBe('留空自动生成');
  });

  it('解析生命周期（脚本 / 顺序 / 超时）', () => {
    const v = parseManifest(SAMPLE)!;
    expect(v.lifecycle).toEqual([
      { stage: 'start', script: './scripts/start.sh', order: 'worker-first-reverse', timeout: 3600 },
      { stage: 'health-probe', script: 'python scripts/health.py', order: 'head-first', timeout: undefined },
    ]);
  });

  it('缺 caps / 非法输入回落', () => {
    const noCaps = parseManifest('id: x\nversion: 1.0.0\nmodel: y\n')!;
    expect(noCaps.maxNodes).toBe('不限制');
    expect(noCaps.images).toEqual([]);
    expect(noCaps.configs).toEqual([]);
    expect(noCaps.lifecycle).toEqual([]);

    expect(parseManifest('')).toBeNull();
    expect(parseManifest('   ')).toBeNull();
    expect(parseManifest('just a string')).toBeNull();
    expect(parseManifest(': : not valid yaml :')).toBeNull();
    expect(parseManifest('- a\n- b\n')).toBeNull();
  });
});