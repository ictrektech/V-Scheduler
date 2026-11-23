/**
 * 仓库设置命名空间（zh-CN）：上游认证与代理配置。
 */
export default {
  settings: {
    title: '仓库设置',
    tabs: {
      registry: '上游认证',
      proxy: '代理',
    },
    auth: {
      intro1:
        '配置上游 registry 的用户名/密码。拉取或同步命中该地址时，会先尽力执行 ',
      introCode: 'skopeo login',
      intro2: '（登录失败不中止拉取）。密码仅保存在设备本地，界面不回显。',
      host: 'Registry',
      username: '用户名',
      password: '密码',
      save: '保存',
    },
    columns: {
      username: '用户名',
      password: '密码',
      action: '操作',
    },
    status: {
      set: '已设置',
      unset: '未设置',
    },
    deleteConfirm: '删除 {host} 的认证信息？',
    empty: '尚未配置上游 registry 认证（默认匿名拉取）',
    proxy: {
      intro1:
        '启用后，拉取/同步镜像时会给 skopeo 进程注入以下代理环境变量；本地 registry 与回环地址会自动加入 ',
      introCode: 'no_proxy',
      intro2: '。',
      enable: '启用代理',
      save: '保存代理配置',
    },
    messages: {
      loadFailed: '加载设置失败：{error}',
      missingFields: '请填写 registry 地址与用户名',
      saved: '已保存（密码回显已关闭）',
      saveFailed: '保存失败：{error}',
      deleted: '已删除 {host} 的认证信息',
      deleteFailed: '删除失败：{error}',
      proxySaved: '代理配置已保存',
    },
  },
};