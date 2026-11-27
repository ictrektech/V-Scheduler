export default {
  fileOverride: {
    title: '卡片文件 Override：{name}',
    hint: '只允许覆盖模型卡片 scripts/ 下的文本脚本；原始卡片不会被修改。每个运行配置独立维护，最多 {max} 个文件；切换卡片版本会清除全部 override。',
    runningHint: '该运行配置当前处于活跃状态。保存后不会立即修改运行中的节点文件，将在下一次启动时生效。',
    files: '脚本文件（已覆盖 {overridden} / {total}）',
    empty: '模型卡片没有可覆盖的 scripts 文件',
    overridden: '已覆盖',
    editing: '正在编辑该运行配置的 override',
    copyHint: '保存后将从原始脚本复制一份 override，仅属于当前运行配置',
    create: '创建 Override',
    restoreOriginal: '删除并恢复原始',
    viewOriginal: '查看卡片原始内容',
    size: '当前 {size} / 上限 {max}',
    unsaved: '未保存',
    selectFile: '请选择一个脚本文件',
    delete: {
      title: '删除文件 Override',
      content: '确定删除 {path} 的 override？后续部署将恢复使用卡片原始文件。',
    },
    msg: {
      saved: 'Override 已保存，将在下次启动时生效',
      deleted: 'Override 已删除，已恢复卡片原始脚本',
      tooLarge: 'Override 内容超过上限 {max}',
    },
  },
} as const
