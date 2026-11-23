/**
 * 垃圾回收命名空间（zh-CN）：GC 弹窗、间隔预设与不可用原因。
 */
export default {
  gc: {
    title: '垃圾回收',
    unavailable: {
      title: '垃圾回收能力不可用',
      registry_binary_missing:
        'registry 二进制不可用：请使用内置镜像或配置 --registry-bin',
      storage_dir_missing:
        'registry 存储目录不存在：请确认已挂载 registry 存储卷并配置 --registry-storage-dir',
      fallback: 'registry 二进制或存储目录不可用',
    },
    busy: '已有垃圾回收任务正在执行',
    storage: '存储占用',
    storageDir: '存储目录',
    lastRun: '最近回收',
    nextRun: '下次自动回收',
    intro:
      '垃圾回收删除未被任何标签引用的镜像层（blob）与未打标签的 manifest，释放 registry 存储空间。回收期间 registry 仍对外服务；建议在低活动时段执行，避免与外部镜像推送并发。',
    auto: {
      title: '自动回收',
      enable: '启用自动回收',
      enableHint: '默认关闭；启用后按间隔自动执行（不立即触发）',
      interval: '回收间隔',
      deleteUntagged: '删除无标签镜像',
      deleteUntaggedHint: '回收改名 / 摘标签后遗留的未打标签 manifest 与其镜像层',
      current: '当前：{interval}',
      save: '保存自动回收配置',
    },
    manual: {
      title: '立即回收',
      deleteUntagged: '删除无标签镜像',
      dryRun: '预演',
      dryRunHint: '仅统计将删除的对象，不实际删除任何数据',
      run: '立即回收',
      runDry: '开始预演',
    },
    intervals: {
      h1: '每 1 小时',
      h3: '每 3 小时',
      h6: '每 6 小时',
      h12: '每 12 小时',
      h24: '每 24 小时（默认）',
      d3: '每 3 天',
      d7: '每 7 天',
    },
    interval: {
      days: '{count} 天',
      hours: '{count} 小时',
    },
    nextRunText: {
      disabled: '未启用自动回收',
      at: '预计 {time}',
      pending: '启用后按间隔计时',
    },
    messages: {
      loadFailed: '加载垃圾回收状态失败：{error}',
      invalidInterval: '自动回收间隔需为 1~720 小时的整数',
      savedEnabled: '已启用自动回收',
      savedDisabled: '已保存自动回收配置（未启用）',
      saveFailed: '保存失败：{error}',
      startedDry: '已开始预演（不会删除数据）',
      started: '已开始垃圾回收',
      triggerFailed: '触发失败：{error}',
    },
    result: {
      dryHeadline: '预演（未删除任何数据）',
      doneHeadline: '已完成回收',
      textDry: '将删除 {blobs} 个 blob / {manifests} 个 manifest',
      text: '删除 {blobs} 个 blob / {manifests} 个 manifest',
      marked: '仍被引用 {count} 个 blob',
      freed: '释放 {size}',
    },
  },
};