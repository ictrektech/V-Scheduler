/**
 * ComfyUI 权重管理纯函数：目录树构建与目标类型推断（无 Vue / DOM 依赖，便于单测）。
 *
 * 用于运行配置详情页「添加 ComfyUI 权重」双栏迁移弹窗：
 * - 左栏：源模型文件树（扁平相对路径 → 分层树，文件可勾选）；
 * - 右栏：实例权重目录树（按目标类型分组展示已有软链接）；
 * - 目标类型推断：取所选文件的顶层目录（合法时），否则回退到用户选择类型。
 */
import type { ComfyuiWeightItem } from '@/api/deployments';
import type { VariantFile } from '@/api/modelHub';

/** 目录树节点（映射到 antd `a-tree`，经 `field-names` 绑定 key / name / children） */
export type TreeNode = {
  /** 唯一 key（完整相对路径） */
  key: string;
  /** 展示名（路径段） */
  name: string;
  /** 是否叶子（文件） */
  isLeaf: boolean;
  /** 是否可勾选（仅文件可勾选） */
  checkable: boolean;
  /** 文件相对路径（仅叶子节点） */
  relPath?: string;
  /** 文件大小（字节，仅叶子节点） */
  sizeBytes?: number;
  children?: TreeNode[];
};

interface Builder {
  name: string;
  key: string;
  isLeaf: boolean;
  relPath?: string;
  sizeBytes?: number;
  children: Map<string, Builder>;
}

function newDir(name: string, key: string): Builder {
  return { name, key, isLeaf: false, children: new Map() };
}

/** 把一条相对路径插入树 */
function insertPath(
  root: Builder,
  relPath: string,
  sizeBytes: number,
  leafName?: string,
): void {
  const segs = relPath.split('/').filter(Boolean);
  if (segs.length === 0) return;
  let cur = root;
  let acc = '';
  for (let i = 0; i < segs.length; i += 1) {
    const seg = segs[i];
    acc = acc ? `${acc}/${seg}` : seg;
    const leaf = i === segs.length - 1;
    if (leaf) {
      cur.children.set(seg, {
        name: leafName ?? seg,
        key: acc,
        isLeaf: true,
        relPath,
        sizeBytes,
        children: new Map(),
      });
    } else {
      if (!cur.children.has(seg)) cur.children.set(seg, newDir(seg, acc));
      cur = cur.children.get(seg)!;
    }
  }
}

/** 把内部树排序并转换为输出节点（目录在前、同类型按名升序） */
function finalize(node: Builder): TreeNode[] {
  const items = [...node.children.values()];
  items.sort((a, b) => {
    if (a.isLeaf !== b.isLeaf) return a.isLeaf ? 1 : -1;
    return a.name.localeCompare(b.name);
  });
  return items.map((b) => {
    const out: TreeNode = {
      key: b.key,
      name: b.name,
      isLeaf: b.isLeaf,
      checkable: b.isLeaf,
    };
    if (b.isLeaf) {
      out.relPath = b.relPath;
      out.sizeBytes = b.sizeBytes;
    } else {
      out.children = finalize(b);
    }
    return out;
  });
}

/** 由扁平文件清单构建分层目录树（文件节点携带 relPath / sizeBytes） */
export function buildFileTree(files: VariantFile[]): TreeNode[] {
  const root = newDir('', '');
  for (const f of files) insertPath(root, f.rel_path, f.size_bytes);
  return finalize(root);
}

/** 由实例权重台账构建目录树（按 target_type 分组；叶子为链接名） */
export function buildInstanceTree(weights: ComfyuiWeightItem[]): TreeNode[] {
  const root = newDir('', '');
  for (const w of weights) {
    // 以 link_rel（`<type>/<name>`）作为实例内相对路径；叶子名取 link_name
    insertPath(root, w.link_rel, 0, w.link_name);
  }
  return finalize(root);
}

/** 收集树中所有文件节点的相对路径（用于校验勾选） */
export function collectFilePaths(nodes: TreeNode[]): string[] {
  const out: string[] = [];
  const walk = (list: TreeNode[]) => {
    for (const n of list) {
      if (n.isLeaf && n.relPath) out.push(n.relPath);
      if (n.children) walk(n.children);
    }
  };
  walk(nodes);
  return out;
}

/** 目标类型合法性（与后端 `sanitize_target_type` 语义对齐） */
export function isValidTargetType(raw: string): boolean {
  const t = (raw ?? '').trim();
  if (!t || t.startsWith('/')) return false;
  const trimmed = t.replace(/^\/+|\/+$/g, '');
  if (!trimmed) return false;
  return !trimmed
    .split('/')
    .some((seg) => seg === '' || seg === '.' || seg === '..');
}

/** 文件的顶层目录（无目录返回空串） */
export function topDir(filePath: string): string {
  const p = (filePath ?? '').trim().replace(/^\/+/, '');
  const idx = p.indexOf('/');
  return idx > 0 ? p.slice(0, idx) : '';
}

/**
 * 目标类型推断（见 design D-MI-CU-03/04）
 *
 * - 未开启自动推断 / 文件无顶层目录 / 顶层目录非法 → 回退 `fallback`；
 * - 否则取顶层目录。
 */
export function inferTargetType(filePath: string, fallback: string, auto: boolean): string {
  if (!auto) return fallback.trim();
  const dir = topDir(filePath);
  if (dir && isValidTargetType(dir)) return dir;
  return fallback.trim();
}

/** 文件基名 */
export function baseName(filePath: string): string {
  const p = (filePath ?? '').trim();
  const idx = p.lastIndexOf('/');
  return idx >= 0 ? p.slice(idx + 1) : p;
}