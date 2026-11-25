/**
 * 控制台命名空间（zh-CN）：镜像列表首页与镜像详情页。
 */
export default {
  images: {
    title: '镜像仓库',
    subtitle:
      '当前离线镜像仓库（registry）中已上传的镜像。可在此从上游拉取镜像、同步更新、管理标签；点击镜像名进入详情。',
    actions: {
      pull: '拉取镜像',
      import: '导入镜像',
      gc: '垃圾回收',
      settings: '仓库设置',
      guide: '接入指引',
      tasks: '任务进度',
      tasksWithCount: '任务进度（{count}）',
      refresh: '刷新',
    },
    registry: {
      label: 'Registry 地址',
      hint: '（host 网络 HTTP，无认证；docker 客户端直连此地址）',
      placeholder: '选择网卡地址',
      copy: '复制',
      copySuccess: '已复制 registry 地址',
      note: '提示：设备可能具有多个网卡地址，请选择与镜像分发目标网络一致的地址。',
    },
    search: {
      placeholder: '搜索镜像仓库（如 docker.io/library）',
      count: '共 {count} 个仓库',
    },
    columns: {
      repository: '镜像仓库',
      tags: '标签',
      size: '大小',
      created: '创建时间',
      action: '操作',
    },
    row: {
      tagCount: '{count} 个标签',
      detail: '详情',
      sync: '同步',
      clone: '克隆',
      rename: '改名',
      deleteRepo: '删除整仓',
    },
    confirm: {
      deleteRepo: '确认删除仓库 {repository} 的全部标签？',
    },
    empty: '暂无镜像。可点击「拉取镜像」从上游拉取，或按「接入指引」推送镜像到本仓库。',
    messages: {
      loadFailed: '加载镜像失败：{error}',
      deleteSuccess: '已删除仓库 {repository} 的全部标签',
      deleteFailed: '删除失败：{error}',
      cancelRequested: '已请求停止，等待任务收敛',
      cancelFailed: '停止失败：{error}',
    },
  },
  detail: {
    titleFallback: '镜像详情',
    subtitle: '本地引用 {registry} / 标签 {count} 个 / 合计 {size}',
    actions: {
      back: '返回列表',
      guide: '接入指引',
      tasks: '任务进度',
      tasksWithCount: '任务进度（{count}）',
      syncAll: '同步全部',
      cloneRepo: '克隆整仓',
      renameRepo: '改名整仓',
      deleteRepo: '删除整仓',
      refresh: '刷新',
    },
    upstream: {
      label: '上游镜像名',
      copy: '复制',
      copySuccess: '已复制镜像名',
      hint1: 'registry 中仓库名即上游完整镜像名；下载到目标机器后可还原为 ',
      hint2: '。',
    },
    columns: {
      tag: '标签',
      digest: '摘要',
      size: '大小',
      platforms: '平台',
      created: '创建时间',
      action: '操作',
    },
    row: {
      copyDeploymentAddress: '复制部署镜像地址',
      download: '下载命令',
      sync: '同步',
      clone: '克隆',
      rename: '改名',
      delete: '删除',
    },
    confirm: {
      deleteTag: '确认删除 {repository}:{tag}？',
      deleteRepo: '确认删除仓库 {repository} 的全部标签？',
    },
    expand: {
      digest: '摘要',
      size: '大小',
      created: '创建时间',
      mediaType: 'Media Type',
      platforms: '平台条目',
      singlePlatform: '（单平台）',
    },
    platformCount: '{count} 个',
    empty: '该镜像暂无标签',
    messages: {
      loadFailed: '加载镜像详情失败：{error}',
      deleteTagSuccess: '已删除 {repository}:{tag}',
      deleteRepoSuccess: '已删除仓库 {repository} 的全部标签',
      deleteFailed: '删除失败：{error}',
      cancelRequested: '已请求停止，等待任务收敛',
      cancelFailed: '停止失败：{error}',
    },
    download: {
      title: '下载镜像 {repository}:{tag}',
      description:
        '推荐使用一键命令（脚本自动建立并清理 socat，拉取后名称与上游一致）；也可按手动步骤执行（末步停止 socat）。',
      oneLinerHint: '推荐：一键下载（自动建立并清理 socat，还原上游镜像名）',
    },
  },
};