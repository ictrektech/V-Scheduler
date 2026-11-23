#!/usr/bin/env bash
#
# image-hub.sh — 通过 VOS image-hub（离线镜像中转）一键拉取 / 上传 docker 镜像。
#
# 设计要点（见 components/image-hub/openspec/changes/image-hub-client-script/ design.md
# D-IH-68 ~ D-IH-77）：
#   - 地址参数化：--ih-addr <host:port> 或 IH_ADDR=<host:port>，脚本内不硬编码任何 IP；
#   - 名称保留：上传/拉取均以「上游完整名称」作为仓库名，拉取后 retag 还原上游名称与 tag；
#   - socat 自管理：自动挑选本地空闲端口建立转发（docker 对 127.0.0.1 免 HTTPS），
#     端口冲突自动退避；无论以何种方式退出都尽力终止 socat 与派生连接子进程，避免泄露；
#   - docker 参数透传：镜像名之后的参数原样转发给 docker pull / docker push。
#
# 用法：
#   image-hub.sh [--ih-addr <host:port>] pull <IMAGE[:TAG]> [--] [docker 参数...]
#   image-hub.sh [--ih-addr <host:port>] push <IMAGE[:TAG]> [--] [docker 参数...]
#
# 依赖：bash、docker、socat（timeout 为可选增强，缺失时降级）。
# 安全边界：本脚本不引入任何凭据；image-hub registry 为 HTTP、无认证，仅适用于内网受信环境。

set -euo pipefail

DEFAULT_REGISTRY="docker.io"

IH_ADDR="${IH_ADDR:-}"
IH_LOCAL_PORT="${IH_LOCAL_PORT:-}"
IH_DOCKER_ARGS="${IH_DOCKER_ARGS:-}"
IH_SOCAT_TIMEOUT="${IH_SOCAT_TIMEOUT:-900}"
IH_MAX_PORT_TRIES="${IH_MAX_PORT_TRIES:-20}"

# 运行期状态
SOCAT_PID=""
SOCAT_PGID=""
SOCAT_PORT=""
DOCKER_PID=""
BOUNCE_REF=""
CLEANED=0

# 归一结果（parse_image_ref 输出）
IH_TAG=""
IH_UPSTREAM_REPO=""
IH_UPSTREAM_REF=""

info() { printf '[image-hub] %s\n' "$*"; }
err() { printf '[image-hub][error] %s\n' "$*" >&2; }

usage() {
  cat <<'EOF'
image-hub.sh — 通过 image-hub 拉取 / 上传 docker 镜像（自动管理 socat，免配置 HTTPS）

用法:
  image-hub.sh [--ih-addr <host:port>] pull <IMAGE[:TAG]> [--] [docker 参数...]
  image-hub.sh [--ih-addr <host:port>] push <IMAGE[:TAG]> [--] [docker 参数...]

说明:
  pull   从 image-hub 拉取镜像并还原为上游名称/tag，删除本地中转引用
  push   将本地上游镜像上传到 image-hub（registry 中仓库名保留上游完整名称）

参数:
  --ih-addr <host:port>   image-hub registry 地址（等价于环境变量 IH_ADDR）
  <IMAGE[:TAG]>           上游镜像名，如 docker.io/library/alpine:3（tag 缺省 latest）
  [docker 参数...]        镜像名之后的参数原样传给 docker pull / docker push
                          （如 --platform linux/arm64），可选用 `--` 分隔

环境变量:
  IH_ADDR           image-hub 地址 <host:port>（--ih-addr 优先）
  IH_LOCAL_PORT     本地 socat 起始端口（缺省自动挑选空闲端口）
  IH_DOCKER_ARGS    追加透传给 docker 的参数（按空白切分）
  IH_SOCAT_TIMEOUT  socat 兜底生存秒数（默认 900，防止异常退出后残留）

示例:
  image-hub.sh --ih-addr 10.10.11.90:19913 pull docker.io/library/alpine:3
  image-hub.sh --ih-addr 10.10.11.90:19913 push docker.io/library/alpine:3
  curl -k -fsSL https://<管理面>/app/com.ictrek.image-hub/image-hub.sh \
    | bash -s -- --ih-addr 10.10.11.90:19913 pull docker.io/library/alpine:3

  说明：管理面经 Traefik 以自签名证书提供 HTTPS，故 curl 使用 -k（跳过证书校验）。

退出码: 0 成功 / 2 参数错误 / 3 socat 端口不可用 / 4 缺少 docker 或 socat /
        5 push 时本地镜像不存在 / 其它为 docker 返回码
EOF
}

# ---- 清理：无论以何种方式退出都尽力终止 socat，并移除中转标签 ----
remove_bounce_tag() {
  [ -n "${BOUNCE_REF:-}" ] || return 0
  docker rmi "$BOUNCE_REF" >/dev/null 2>&1 || true
  BOUNCE_REF=""
}

cleanup() {
  if [ "$CLEANED" = "1" ]; then
    return 0
  fi
  CLEANED=1
  trap - EXIT INT TERM HUP QUIT
  local _i=0

  # 终止仍在运行的 docker 子进程（信号中断时）
  if [ -n "${DOCKER_PID:-}" ] && kill -0 "$DOCKER_PID" 2>/dev/null; then
    kill -TERM "$DOCKER_PID" 2>/dev/null || true
    DOCKER_PID=""
  fi

  if [ -n "${SOCAT_PGID:-}" ]; then
    if kill -0 "$SOCAT_PGID" 2>/dev/null; then
      # 优先按进程组终止（含 socat fork 出的连接子进程）
      kill -TERM -- "-$SOCAT_PGID" 2>/dev/null || kill -TERM "$SOCAT_PGID" 2>/dev/null || true
    fi
    _i=0
    while [ "$_i" -lt 10 ] && kill -0 "$SOCAT_PGID" 2>/dev/null; do
      sleep 0.1
      _i=$((_i + 1))
    done
    kill -KILL -- "-$SOCAT_PGID" 2>/dev/null || kill -KILL "$SOCAT_PGID" 2>/dev/null || true
    wait "$SOCAT_PGID" 2>/dev/null || true
    SOCAT_PGID=""
  fi

  # 兜底：精确匹配本脚本为该 registry 启动的 socat（端口为本脚本所选，不影响其它服务）
  if [ -n "${SOCAT_PORT:-}" ] && command -v pkill >/dev/null 2>&1; then
    pkill -f "socat TCP-LISTEN:${SOCAT_PORT},bind=127.0.0.1,fork,reuseaddr TCP:${IH_HOST}:${IH_PORT}" \
      >/dev/null 2>&1 || true
  fi

  remove_bounce_tag
  return 0
}

# 启动 socat 到指定本地端口；成功返回 0，失败（端口被占 / 启动即退出）返回 1
try_start_socat() {
  local lport="$1"
  local -a cmd
  cmd=(socat "TCP-LISTEN:${lport},bind=127.0.0.1,fork,reuseaddr" "TCP:${IH_HOST}:${IH_PORT}")
  if command -v timeout >/dev/null 2>&1; then
    cmd=(timeout --signal=TERM "${IH_SOCAT_TIMEOUT}" "${cmd[@]}")
  fi

  # 独立进程组启动，便于整体终止（含派生连接子进程）
  set -m 2>/dev/null || true
  "${cmd[@]}" >/dev/null 2>&1 &
  local pid=$!
  set +m 2>/dev/null || true
  # 立即登记 PID/端口：即使此时收到信号，cleanup 也能定位并终止该 socat
  SOCAT_PID="$pid"
  SOCAT_PGID="$pid"
  SOCAT_PORT="$lport"

  sleep 0.3
  if kill -0 "$pid" 2>/dev/null; then
    return 0
  fi
  # 启动失败（端口被占用等）：清理登记信息后返回 1
  SOCAT_PID=""
  SOCAT_PGID=""
  SOCAT_PORT=""
  wait "$pid" 2>/dev/null || true
  return 1
}

# 挑选空闲端口并启动 socat；端口冲突时自动退避
pick_and_start_socat() {
  local base
  if [ -n "$IH_LOCAL_PORT" ]; then
    printf '%s' "$IH_LOCAL_PORT" | grep -qE '^[0-9]+$' || {
      err "IH_LOCAL_PORT 非法（应为端口号）：$IH_LOCAL_PORT"
      exit 2
    }
    [ "$IH_LOCAL_PORT" -ge 1 ] && [ "$IH_LOCAL_PORT" -le 65535 ] || {
      err "IH_LOCAL_PORT 越界（1~65535）：$IH_LOCAL_PORT"
      exit 2
    }
    base="$IH_LOCAL_PORT"
  else
    base=$((40000 + RANDOM % 20000))
  fi

  local i p
  for i in $(seq 0 "$((IH_MAX_PORT_TRIES - 1))"); do
    p=$((base + i))
    [ "$p" -le 65535 ] || break
    # 默认避开远端 registry 端口，减少与本机既有服务冲突
    if [ -z "$IH_LOCAL_PORT" ] && [ "$p" = "$IH_PORT" ]; then
      continue
    fi
    if try_start_socat "$p"; then
      info "本地端口映射 127.0.0.1:${p} -> ${IH_HOST}:${IH_PORT}（socat pid=${SOCAT_PID}）"
      return 0
    fi
  done

  err "无法找到可用本地端口（起始 ${base}，已尝试 ${IH_MAX_PORT_TRIES} 个）"
  exit 3
}

# 解析 / 归一镜像引用，结果写入 IH_* 全局变量；非法返回 1
parse_image_ref() {
  local raw="$1"
  # 去除首尾空白
  raw="${raw#"${raw%%[![:space:]]*}"}"
  raw="${raw%"${raw##*[![:space:]]}"}"

  [ -n "$raw" ] || return 1
  case "$raw" in *"@"*) return 1 ;; esac
  case "$raw" in *[[:space:]]*) return 1 ;; esac

  local name_part tag="latest"
  case "$raw" in
    *:*)
      local after="${raw##*:}"
      case "$after" in
        */*) name_part="$raw" ;;
        *) name_part="${raw%:*}"; tag="$after" ;;
      esac
      ;;
    *) name_part="$raw" ;;
  esac
  [ -n "$name_part" ] || return 1
  printf '%s' "$tag" | grep -qE '^[A-Za-z0-9._-]{1,128}$' || return 1

  local registry="$DEFAULT_REGISTRY" repo=""
  case "$name_part" in
    */*)
      local first="${name_part%%/*}"
      local rest="${name_part#*/}"
      [ -n "$first" ] && [ -n "$rest" ] || return 1
      case "$first" in
        *.*|*:*|localhost)
          registry="$(printf '%s' "$first" | tr 'A-Z' 'a-z')"
          repo="$rest"
          ;;
        *) repo="$name_part" ;;
      esac
      ;;
    *) repo="library/$name_part" ;;
  esac

  case "$repo" in
    /*|*/|*..*|*//*) return 1 ;;
  esac
  printf '%s' "$repo" | grep -qE '^[a-z0-9._/-]+$' || return 1

  IH_TAG="$tag"
  IH_UPSTREAM_REPO="${registry}/${repo}"
  IH_UPSTREAM_REF="${registry}/${repo}:${tag}"
  return 0
}

# 以前台语义执行 docker 命令，但经「后台 + wait」以便信号能被及时处理
# （bash 在 wait 异步命令时收到带 trap 的信号会立即中断 wait 并执行 trap）。
run_docker() {
  "$@" &
  DOCKER_PID=$!
  local rc=0
  wait "$DOCKER_PID" || rc=$?
  DOCKER_PID=""
  return "$rc"
}

run_pull() {
  pick_and_start_socat
  # 中转引用必须使用「上游完整仓库名」（含 registry host，如 docker.io/library/alpine），
  # 这样 registry 中存储的仓库名才与上游一致（名称保留策略，见 design §3）。
  BOUNCE_REF="127.0.0.1:${SOCAT_PORT}/${IH_UPSTREAM_REPO}:${IH_TAG}"
  info "拉取镜像 ${BOUNCE_REF}"
  run_docker docker pull ${docker_args[@]+"${docker_args[@]}"} "$BOUNCE_REF"
  info "还原上游镜像名 ${IH_UPSTREAM_REF}"
  docker tag "$BOUNCE_REF" "$IH_UPSTREAM_REF"
  docker rmi "$BOUNCE_REF" >/dev/null 2>&1 || true
  BOUNCE_REF=""
  info "完成：本地镜像 ${IH_UPSTREAM_REF} 已就绪"
}

run_push() {
  docker image inspect "$IH_UPSTREAM_REF" >/dev/null 2>&1 || {
    err "本地不存在镜像 ${IH_UPSTREAM_REF}，请先拉取或构建该镜像"
    exit 5
  }
  pick_and_start_socat
  BOUNCE_REF="127.0.0.1:${SOCAT_PORT}/${IH_UPSTREAM_REPO}:${IH_TAG}"
  info "上传本地镜像 ${IH_UPSTREAM_REF} -> ${BOUNCE_REF}"
  docker tag "$IH_UPSTREAM_REF" "$BOUNCE_REF"
  run_docker docker push ${docker_args[@]+"${docker_args[@]}"} "$BOUNCE_REF"
  docker rmi "$BOUNCE_REF" >/dev/null 2>&1 || true
  BOUNCE_REF=""
  info "完成：registry 中仓库名为 ${IH_UPSTREAM_REPO}（标签 ${IH_TAG}）"
}

main() {
  local mode="" image=""
  local -a docker_args=()

  while [ $# -gt 0 ]; do
    case "$1" in
      -h|--help)
        usage
        exit 0
        ;;
      --ih-addr)
        [ $# -ge 2 ] || { err "--ih-addr 需要一个 <host:port> 参数"; usage; exit 2; }
        IH_ADDR="$2"
        shift 2
        ;;
      --ih-addr=*)
        IH_ADDR="${1#*=}"
        shift
        ;;
      pull|push)
        mode="$1"
        shift
        break
        ;;
      --)
        shift
        break
        ;;
      *)
        err "未知参数：$1"
        usage
        exit 2
        ;;
    esac
  done

  [ -n "$mode" ] || { err "缺少子命令（pull | push）"; usage; exit 2; }
  if [ "$#" -ge 1 ] && [ "$1" = "--" ]; then
    shift
  fi
  [ "$#" -ge 1 ] || { err "缺少镜像名 IMAGE[:TAG]"; usage; exit 2; }
  image="$1"
  shift
  docker_args=("$@")
  # 允许用 `--` 分隔镜像名与 docker 透传参数
  if [ "${#docker_args[@]}" -gt 0 ] && [ "${docker_args[0]}" = "--" ]; then
    docker_args=("${docker_args[@]:1}")
  fi
  if [ -n "$IH_DOCKER_ARGS" ]; then
    local -a _extra=()
    read -r -a _extra <<<"$IH_DOCKER_ARGS"
    docker_args+=("${_extra[@]}")
  fi

  # ---- 地址校验 ----
  if [ -z "$IH_ADDR" ]; then
    err "缺少 image-hub 地址：请设置 IH_ADDR=<host:port> 或使用 --ih-addr <host:port>"
    usage
    exit 2
  fi
  IH_HOST="${IH_ADDR%:*}"
  IH_PORT="${IH_ADDR##*:}"
  if [ -z "$IH_HOST" ] || [ "$IH_HOST" = "$IH_ADDR" ] || ! printf '%s' "$IH_PORT" | grep -qE '^[0-9]+$'; then
    err "地址格式应为 <host>:<port>：$IH_ADDR"
    exit 2
  fi
  if [ "$IH_PORT" -lt 1 ] || [ "$IH_PORT" -gt 65535 ]; then
    err "地址端口越界（1~65535）：$IH_PORT"
    exit 2
  fi
  printf '%s' "$IH_MAX_PORT_TRIES" | grep -qE '^[0-9]+$' || { err "IH_MAX_PORT_TRIES 非法：$IH_MAX_PORT_TRIES"; exit 2; }
  [ "$IH_MAX_PORT_TRIES" -ge 1 ] || { err "IH_MAX_PORT_TRIES 应 >= 1：$IH_MAX_PORT_TRIES"; exit 2; }

  # ---- 依赖校验 ----
  command -v docker >/dev/null 2>&1 || { err "未找到 docker 命令，请先安装 docker 客户端"; exit 4; }
  command -v socat >/dev/null 2>&1 || { err "未找到 socat 命令，请先安装 socat（如 apt-get install -y socat）"; exit 4; }

  # ---- 镜像引用归一 ----
  if ! parse_image_ref "$image"; then
    err "镜像引用非法：$image（应为 IMAGE[:TAG]，如 docker.io/library/alpine:3；不支持 @digest）"
    exit 2
  fi
  if [ "$IH_UPSTREAM_REF" != "$image" ]; then
    info "已归一镜像名：$image -> $IH_UPSTREAM_REF"
  fi

  trap 'cleanup' EXIT
  trap 'cleanup; exit 130' INT
  trap 'cleanup; exit 143' TERM
  trap 'cleanup; exit 129' HUP
  trap 'cleanup; exit 131' QUIT

  case "$mode" in
    pull) run_pull ;;
    push) run_push ;;
  esac

  # 正常完成后主动停止 socat（EXIT trap 亦会兜底）
  cleanup
}

main "$@"