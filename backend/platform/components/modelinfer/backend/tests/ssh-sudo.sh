#!/usr/bin/env bash
# 本地隔离 SSH 集成验证：只监听回环随机端口，结束后清理容器与临时密钥。
set -euo pipefail
cd "$(dirname "$0")/.."
test_dir=$(mktemp -d)
test_container=""
cleanup() {
  if [ -n "$test_container" ]; then docker rm -f "$test_container" >/dev/null; fi
  rm -rf "$test_dir"
}
trap cleanup EXIT
docker build -t modelinfer-ssh-sudo-test tests/fixtures/ssh-sudo
ssh-keygen -q -t ed25519 -N '' -f "$test_dir/id_ed25519"
test_container=$(docker run -d -p 127.0.0.1::22 modelinfer-ssh-sudo-test)
docker cp "$test_dir/id_ed25519.pub" "$test_container:/tmp/test-key.pub" >/dev/null
docker exec "$test_container" sh -c 'for user in root admin nopass denied ttyuser; do dir=$(getent passwd "$user" | cut -d: -f6); mkdir -p "$dir/.ssh"; cp /tmp/test-key.pub "$dir/.ssh/authorized_keys"; chown -R "$user:$user" "$dir/.ssh"; chmod 700 "$dir/.ssh"; chmod 600 "$dir/.ssh/authorized_keys"; done'
export MODELINFER_TEST_SSH_PORT
MODELINFER_TEST_SSH_PORT=$(docker port "$test_container" 22/tcp | cut -d: -f2)
export MODELINFER_TEST_SSH_KEY="$test_dir/id_ed25519"
cargo test --test ssh_sudo --locked -- --ignored --nocapture
