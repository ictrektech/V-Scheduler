#!/usr/bin/env bash
# image-hub 单容器 entrypoint：在同一容器内启动 registry 数据面与管理服务。
#
# 组织（见 components/image-hub/openspec/changes/image-hub-registry-gc/ D-IH-59）：
#   - registry：官方二进制 /usr/local/bin/registry，HTTP、无认证、删除开启，监听 :19913；
#   - 管理面：/usr/local/bin/image-hub（Rust 后端 + Vue 前端 + skopeo），监听 :19914。
#   两者共享容器内文件系统，registry 存储目录即管理面 GC 的操作目录。
#
# 由 tini 作为 PID 1 托管本脚本；本脚本负责生成 registry config、拉起两个子进程、
# 等待 registry 就绪、转发退出信号并在任一子进程退出时整体退出。
set -euo pipefail

REGISTRY_BIN="${VOS_IMAGE_HUB_REGISTRY_BIN:-/usr/local/bin/registry}"
REGISTRY_PORT="${VOS_IMAGE_HUB_REGISTRY_PORT:-19913}"
REGISTRY_STORAGE_DIR="${VOS_IMAGE_HUB_REGISTRY_STORAGE_DIR:-/var/lib/registry}"
REGISTRY_LOG_LEVEL="${VOS_IMAGE_HUB_LOG_LEVEL:-info}"
REGISTRY_CONFIG="${VOS_IMAGE_HUB_REGISTRY_CONFIG:-/etc/image-hub/registry-config.yml}"

mkdir -p "$(dirname "${REGISTRY_CONFIG}")" "${REGISTRY_STORAGE_DIR}"

# 生成 registry config（官方发行包不含自带 config；存储删除开启以支持镜像删除）
cat > "${REGISTRY_CONFIG}" <<EOF
version: 0.1
log:
  level: ${REGISTRY_LOG_LEVEL}
storage:
  filesystem:
    rootdirectory: ${REGISTRY_STORAGE_DIR}
  delete:
    enabled: true
http:
  addr: 0.0.0.0:${REGISTRY_PORT}
EOF

echo "[image-hub-entrypoint] 启动 registry（${REGISTRY_STORAGE_DIR} -> :${REGISTRY_PORT}）"
"${REGISTRY_BIN}" serve "${REGISTRY_CONFIG}" &
registry_pid=$!

hub_pid=""
shutdown() {
  echo "[image-hub-entrypoint] 收到停止信号，正在退出…"
  kill -TERM "${registry_pid}" ${hub_pid:+"${hub_pid}"} 2>/dev/null || true
}
trap shutdown TERM INT

# 等待 registry 就绪（最多约 15s；未就绪也继续启动管理面，health 会反映 registry 不可达）
for _ in $(seq 1 30); do
  if curl -fsS -o /dev/null "http://127.0.0.1:${REGISTRY_PORT}/v2/" 2>/dev/null; then
    echo "[image-hub-entrypoint] registry 已就绪"
    break
  fi
  if ! kill -0 "${registry_pid}" 2>/dev/null; then
    echo "[image-hub-entrypoint][warn] registry 进程已退出，继续启动管理面" >&2
    break
  fi
  sleep 0.5
done

echo "[image-hub-entrypoint] 启动管理服务 image-hub $*"
/usr/local/bin/image-hub "$@" &
hub_pid=$!

# 任一子进程退出即整体退出（保持容器生命周期与两个进程一致）
status=0
wait -n "${registry_pid}" "${hub_pid}" || status=$?
shutdown
wait "${registry_pid}" 2>/dev/null || true
wait "${hub_pid}" 2>/dev/null || true
exit "${status}"