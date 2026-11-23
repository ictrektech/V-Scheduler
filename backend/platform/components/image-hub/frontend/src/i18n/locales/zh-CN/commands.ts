/**
 * 命令生成命名空间（zh-CN）：手动上传/下载步骤标题（命令本体语言中立）。
 */
export default {
  commands: {
    pull: {
      socat:
        '将远端 {ip}:{port} 映射到本机 {port} 端口（docker 对 127.0.0.1 默认走 HTTP，无需配置 HTTPS）',
      pull: '拉取中转仓库镜像',
      tag: '去除前缀，还原上游镜像名称',
      rmi: '删除中转仓库镜像',
      stop: '停止后台 socat 端口映射进程（避免长期占用本地端口）',
    },
    push: {
      socat: '将远端 {ip}:{port} 映射到本机 {port} 端口',
      tag: '给上游镜像打上中转仓库标签（保留上游完整名称，如 docker.io/foo/bar）',
      push: '推送镜像到中转仓库',
      stop: '停止后台 socat 端口映射进程（避免长期占用本地端口）',
    },
  },
};