#!/bin/sh
# keepalived notify：把 VRRP 状态写入 project data 目录，供 VCM status 读取。
set -eu
mkdir -p /var/lib/keepalived
instance="${2:-}"
if [ -n "$instance" ]; then
    file="/var/lib/keepalived/state-${instance}"
else
    file="/var/lib/keepalived/state"
fi
printf '%s\n' "$1" > "$file"
