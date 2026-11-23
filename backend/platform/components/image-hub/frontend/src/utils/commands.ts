/**
 * 镜像上传/下载命令生成（纯函数，便于单测）。
 *
 * 设计要点（见 OpenSpec D-IH-03）：registry 中仓库名保留上游完整名称
 * （如 `docker.io/foo/bar`），拉取后用 `socat` + `127.0.0.1` 免 HTTPS 访问，
 * 再 retag 还原上游名称，保证分发后镜像 tag 与名称与上游一致。
 */

/**
 * 命令步骤：命令本体语言中立，标题以 i18n key + 插值参数表达，由组件层翻译。
 */
export interface CommandStep {
  /** 步骤标题 i18n key（`commands.*`） */
  titleKey: string;
  /** 标题插值参数（如 `{ ip, port }`） */
  titleParams?: Record<string, string | number>;
  command: string;
}

/** registry 展示地址（`<ip>:<port>`） */
export function registryAddress(ip: string, port: number): string {
  return `${ip}:${port}`;
}

/** socat 端口映射命令（将远端 registry 端口映射到本机，docker 对 127.0.0.1 免 HTTPS） */
export function socatForward(ip: string, port: number): string {
  return `socat TCP-LISTEN:${port},fork,reuseaddr TCP:${ip}:${port} &`;
}

/** 停止上面 socatForward 建立的后台进程，避免长期占用本地端口（手动执行时的收尾步骤） */
export function socatStop(port: number): string {
  return `pkill -f 'socat TCP-LISTEN:${port}' || true`;
}

/**
 * 从页面地址推导 `image-hub.sh` 的下载 URL（纯函数，便于单测）。
 *
 * 应用经 VOS 子路径 iframe 托管（如 `http://host/app/com.ictrek.image-hub/#/images`），
 * 脚本随前端 dist 位于同一目录（`<url-base>/image-hub.sh`）。hash 路由下基于 pathname 解析。
 */
export function scriptUrlFrom(locationHref: string): string {
  return new URL('image-hub.sh', locationHref).href;
}

export type OneLinerMode = 'pull' | 'push';

/**
 * 脚本拉取命令前缀：管理面经 Traefik 以**自签名证书**提供 HTTPS，故默认带 `-k`
 * （跳过证书校验）；对 http 地址无副作用。
 */
export const CURL_FETCH = 'curl -k -fsSL';

/** 下载脚本到本地（不执行） */
export function buildScriptDownload(scriptUrl: string, dest = 'image-hub.sh'): string {
  return `${CURL_FETCH} ${scriptUrl} -o ${dest}`;
}

/**
 * 一键执行命令（推荐）：经 `image-hub.sh` 拉取/上传。
 *
 * 形如 `curl -k -fsSL <script-url> | bash -s -- --ih-addr <ip:port> <pull|push> <IMAGE:TAG>`；
 * 脚本内部自动建立并清理 socat，无需用户手工分步执行。
 */
export function buildOneLiner(
  scriptUrl: string,
  ihAddr: string,
  mode: OneLinerMode,
  upstream: string,
): string {
  return `${CURL_FETCH} ${scriptUrl} | bash -s -- --ih-addr ${ihAddr} ${mode} ${upstream}`;
}

/** 「先下载再执行」的等价命令（下载 → 赋可执行权限 → 运行） */
export function buildDownloadAndRun(
  scriptUrl: string,
  ihAddr: string,
  mode: OneLinerMode,
  upstream: string,
  dest = 'image-hub.sh',
): string {
  return `${buildScriptDownload(scriptUrl, dest)} && chmod +x ${dest} && ./${dest} --ih-addr ${ihAddr} ${mode} ${upstream}`;
}

/** 中转仓库中的本地拉取名（经 127.0.0.1 免 HTTPS） */
export function localRef(port: number, repository: string, tag: string): string {
  return `127.0.0.1:${port}/${repository}:${tag}`;
}

/** 上游完整镜像名 */
export function upstreamRef(repository: string, tag: string): string {
  return `${repository}:${tag}`;
}

/**
 * 下载命令（手动执行）：socat 映射 → pull → retag 还原上游名 → rmi 清理中转名 →
 * 停止 socat（避免后台进程残留）
 */
export function buildPullCommands(
  ip: string,
  port: number,
  repository: string,
  tag: string,
): CommandStep[] {
  const local = localRef(port, repository, tag);
  const upstream = upstreamRef(repository, tag);
  return [
    {
      titleKey: 'commands.pull.socat',
      titleParams: { ip, port },
      command: socatForward(ip, port),
    },
    { titleKey: 'commands.pull.pull', command: `docker pull ${local}` },
    { titleKey: 'commands.pull.tag', command: `docker tag ${local} ${upstream}` },
    { titleKey: 'commands.pull.rmi', command: `docker rmi ${local}` },
    { titleKey: 'commands.pull.stop', command: socatStop(port) },
  ];
}

/**
 * 上传命令（手动执行）：socat 映射 → tag 保留上游完整名称 → push → 停止 socat
 */
export function buildPushCommands(
  ip: string,
  port: number,
  repository: string,
  tag: string,
): CommandStep[] {
  const local = localRef(port, repository, tag);
  const upstream = upstreamRef(repository, tag);
  return [
    {
      titleKey: 'commands.push.socat',
      titleParams: { ip, port },
      command: socatForward(ip, port),
    },
    {
      titleKey: 'commands.push.tag',
      command: `docker tag ${upstream} ${local}`,
    },
    { titleKey: 'commands.push.push', command: `docker push ${local}` },
    { titleKey: 'commands.push.stop', command: socatStop(port) },
  ];
}

/** 步骤标题翻译（`translate` 由调用方传入 i18n 的 `t`，保持本模块零依赖） */
export function commandStepTitle(
  step: CommandStep,
  translate: (key: string, params?: Record<string, unknown>) => string,
): string {
  return translate(step.titleKey, step.titleParams);
}

/**
 * 将命令步骤拼成可复制的纯文本（含注释标题）。
 *
 * @param steps 命令步骤
 * @param translate i18n 翻译函数（组件层传入 `useI18n().t`）
 */
export function buildPlainText(
  steps: CommandStep[],
  translate: (key: string, params?: Record<string, unknown>) => string,
): string {
  return steps
    .map((s) => `# ${commandStepTitle(s, translate)}\n${s.command}`)
    .join('\n\n');
}