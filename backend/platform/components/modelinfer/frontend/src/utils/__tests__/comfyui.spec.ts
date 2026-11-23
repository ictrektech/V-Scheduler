import { describe, expect, it } from 'vitest';
import {
  baseName,
  buildFileTree,
  buildInstanceTree,
  collectFilePaths,
  inferTargetType,
  isValidTargetType,
  topDir,
  type TreeNode,
} from '../comfyui';
import type { ComfyuiWeightItem } from '@/api/deployments';

const files = [
  { rel_path: 'checkpoints/a.safetensors', size_bytes: 10 },
  { rel_path: 'checkpoints/b.safetensors', size_bytes: 20 },
  { rel_path: 'model_index.json', size_bytes: 2 },
  { rel_path: 'vae/sub/c.safetensors', size_bytes: 30 },
];

describe('buildFileTree', () => {
  it('按路径分层，目录在前、文件在后', () => {
    const tree = buildFileTree(files);
    const names = tree.map((n) => n.name);
    // 目录（checkpoints / vae）在前，文件（model_index.json）在后
    expect(names).toEqual(['checkpoints', 'vae', 'model_index.json']);
    const ckpt = tree.find((n) => n.name === 'checkpoints')!;
    expect(ckpt.isLeaf).toBe(false);
    expect(ckpt.checkable).toBe(false);
    expect(ckpt.children!.map((c) => c.name)).toEqual(['a.safetensors', 'b.safetensors']);
    expect(ckpt.children![0].checkable).toBe(true);
    expect(ckpt.children![0].relPath).toBe('checkpoints/a.safetensors');
    expect(ckpt.children![0].sizeBytes).toBe(10);
    const vae = tree.find((n) => n.name === 'vae')!;
    expect(vae.children![0].name).toBe('sub');
    expect(vae.children![0].children![0].relPath).toBe('vae/sub/c.safetensors');
  });

  it('collectFilePaths 仅返回文件叶子', () => {
    const paths = collectFilePaths(buildFileTree(files));
    expect(paths.sort()).toEqual([
      'checkpoints/a.safetensors',
      'checkpoints/b.safetensors',
      'model_index.json',
      'vae/sub/c.safetensors',
    ]);
  });

  it('空输入返回空树', () => {
    expect(buildFileTree([])).toEqual([]);
  });
});

function w(partial: Partial<ComfyuiWeightItem>): ComfyuiWeightItem {
  return {
    id: 'x',
    deployment_id: 'd',
    source: 'hf',
    weight_id: 'org/repo',
    file_path: '',
    target_type: '',
    link_name: '',
    link_rel: '',
    target_rel: '',
    created_at: '',
    updated_at: '',
    ...partial,
  };
}

describe('buildInstanceTree', () => {
  it('按目标类型（含子目录）分组展示链接', () => {
    const tree = buildInstanceTree([
      w({ target_type: 'checkpoints', link_name: 'a.safetensors', link_rel: 'checkpoints/a.safetensors' }),
      w({ target_type: 'checkpoints', link_name: 'b.safetensors', link_rel: 'checkpoints/b.safetensors' }),
      w({ target_type: 'my/sub', link_name: 'c.safetensors', link_rel: 'my/sub/c.safetensors' }),
    ]);
    const names = tree.map((n) => n.name);
    expect(names).toEqual(['checkpoints', 'my']);
    const ckpt = tree.find((n) => n.name === 'checkpoints')!;
    expect(ckpt.children!.map((c) => c.name)).toEqual(['a.safetensors', 'b.safetensors']);
    const my = tree.find((n) => n.name === 'my')!;
    expect(my.children![0].name).toBe('sub');
    expect(my.children![0].children![0].name).toBe('c.safetensors');
  });

  it('空台账返回空树', () => {
    expect(buildInstanceTree([])).toEqual([]);
  });
});

describe('inferTargetType / isValidTargetType', () => {
  it('有效类型', () => {
    for (const t of ['checkpoints', 'a/b', 'my_models/sub']) {
      expect(isValidTargetType(t)).toBe(true);
    }
    for (const t of ['', '/abs', '../x', 'a/../b', './a', 'a//b']) {
      expect(isValidTargetType(t)).toBe(false);
    }
  });

  it('自动推断取顶层目录', () => {
    expect(inferTargetType('checkpoints/a.safetensors', 'vae', true)).toBe('checkpoints');
  });

  it('根文件回退到所选类型', () => {
    expect(inferTargetType('model.safetensors', 'checkpoints', true)).toBe('checkpoints');
  });

  it('关闭自动推断时始终使用所选类型', () => {
    expect(inferTargetType('checkpoints/a.safetensors', 'vae', false)).toBe('vae');
  });

  it('顶层目录非法时回退', () => {
    expect(inferTargetType('../x/a', 'loras', true)).toBe('loras');
  });
});

describe('topDir / baseName', () => {
  it('topDir', () => {
    expect(topDir('a/b/c')).toBe('a');
    expect(topDir('a')).toBe('');
    expect(topDir('/a/b')).toBe('a');
  });
  it('baseName', () => {
    expect(baseName('a/b/c.safetensors')).toBe('c.safetensors');
    expect(baseName('c.safetensors')).toBe('c.safetensors');
  });
});