/**
 * 接入指引命名空间（zh-CN）：上传/下载命令、一键/手动模式与原理说明。
 */
export default {
  guide: {
    title: '接入指引',
    intro:
      '通过本仓库中转第三方镜像：上传时保留上游完整名称，下载时经 socat + 127.0.0.1 免 HTTPS 并 retag 还原上游名称。推荐使用 image-hub.sh 一键执行（自动建立并清理 socat）。',
    address: 'Registry 地址',
    addressHint: '（host 网络 HTTP，无认证）',
    chooseAddress: '选择网卡地址',
    copy: '复制',
    upstream: '上游镜像名',
    upstreamPlaceholder: 'docker.io/foo/bar:tag',
    invalid: '格式应为 仓库:标签，如 docker.io/foo/bar:tag（或 alpine:3）',
    normalized:
      '归一的镜像名：{ref}；上传到本仓库后的中转引用：{local}（registry 中存储的仓库名即为上游完整名称 {repository}）',
    upload: {
      title: '一、上传镜像（在镜像来源机器执行）',
      oneTitle: '一键上传（脚本自动完成 socat 映射、打标签、推送与 socat 清理）',
    },
    download: {
      title: '二、下载镜像（在目标机器执行，拉取后名称与上游一致）',
      oneTitle: '一键下载（脚本自动完成 socat 映射、拉取、还原上游名称与 socat 清理）',
    },
    mode: {
      one: '一键执行（推荐）',
      manual: '手动执行',
    },
    noCommand: '请填写合法的上游镜像名并选择 registry 地址',
    downloadAlt: '也可先下载再执行：',
    prereq: '需目标机器具备 docker 与 socat；脚本不硬编码地址，经 --ih-addr 传入。',
    copyAll: '复制全部',
    manualHint: '手动分步执行（末步停止 socat 后台进程）',
    principles: {
      title: '原理与注意事项',
      colon: '：',
      name: {
        name: '名称保留',
        socat: 'socat 与免 HTTPS',
        script: '脚本获取',
        delete: '删除与回收',
        security: '安全边界',
      },
      nameItem:
        '上传时镜像的本地名即为 registry 中存储的仓库名。将上游完整名称 {upstream} 整体保留为本地名的一段（<registry-ip>:{port}/{upstream}:tag），registry 中的仓库名天然即为上游名称，tag 不变。',
      socatItem:
        'docker 对 127.0.0.1:<端口> 默认按 HTTP 处理，对其它主机名默认要求 HTTPS。一键脚本（image-hub.sh）会自动挑选本地空闲端口建立 socat 映射并在退出时尽力清理（含信号中断），端口冲突自动退避；手动执行时请务必执行末步以停止后台 socat。若目标机器就是本设备，可直接使用 registry 地址，无需 socat。',
      scriptItem:
        '{command} 或直接管道执行；脚本仅依赖 bash / docker / socat，不硬编码地址。（管理面经 Traefik 以自签名证书提供 HTTPS，故 curl 使用 -k 跳过证书校验。）',
      deleteItem:
        'registry 仅支持按 manifest digest 删除；删除后镜像层（blob）需 registry 垃圾回收（GC）才会释放磁盘。',
      securityItem:
        'registry 为 HTTP、无认证，仅适用于内网受信环境；请勿直接暴露到不可信网络。',
    },
  },
};