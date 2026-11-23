#!/bin/sh
# keepalived 容器入口：根据编排方注入的 env 渲染 VRRP 配置，然后前台运行 keepalived。
#
# 只要 keepalived 持有 VIP，SMB/NFS 客户端访问 VIP 就会落到当前 master 的
# samba/nfsd；健康检查失败（drain / 端口未监听）时优先级下降，VIP 漂移。
set -eu

: "${KEEPALIVED_NODE_IP:?KEEPALIVED_NODE_IP required}"
: "${KEEPALIVED_VIP:=}"
: "${KEEPALIVED_VIP_PREFIX:=24}"
: "${KEEPALIVED_PRIORITY:=100}"
: "${KEEPALIVED_VRRP_ID:=51}"
: "${KEEPALIVED_SERVICES:=smb}"
: "${KEEPALIVED_PEERS:=}"
: "${KEEPALIVED_INTERFACE:=}"
: "${KEEPALIVED_HEALTH_URL_BASE:=http://127.0.0.1:19906/app/com.ictrek.vos-cluster-manager}"
: "${KEEPALIVED_RGW_VIP:=}"
: "${KEEPALIVED_RGW_VIP_PREFIX:=24}"
: "${KEEPALIVED_RGW_PEERS:=}"
: "${KEEPALIVED_RGW_PRIORITY:=100}"
: "${KEEPALIVED_RGW_VRRP_ID:=52}"
: "${KEEPALIVED_RGW_HEALTH_URL:=http://127.0.0.1:7480/}"

IFACE="$KEEPALIVED_INTERFACE"
if [ -z "$IFACE" ]; then
    IFACE=$(ip -o -4 addr show | awk -v ip="$KEEPALIVED_NODE_IP" '$4 ~ ("^" ip "/") {print $2; exit}')
fi
if [ -z "$IFACE" ]; then
    IFACE=$(ip -o route get 1.1.1.1 2>/dev/null | awk '{for (i = 1; i <= NF; i++) if ($i == "dev") {print $(i + 1); exit}}')
fi
if [ -z "$IFACE" ]; then
    echo "keepalived: cannot detect interface for ${KEEPALIVED_NODE_IP}" >&2
    exit 1
fi

mkdir -p /etc/keepalived /var/lib/keepalived
printf '%s\n' "$KEEPALIVED_SERVICES" | tr ',' '\n' | sed '/^$/d' > /etc/keepalived/services

cat > /etc/keepalived/check-gateway.sh <<'CHECK'
#!/bin/sh
set -eu
base="${KEEPALIVED_HEALTH_URL_BASE:-http://127.0.0.1:19906/app/com.ictrek.vos-cluster-manager}"
for svc in $(cat /etc/keepalived/services); do
    [ -n "$svc" ] || continue
    curl -fsS --max-time 2 "$base/api/gateway/health?service=$svc" >/dev/null || exit 1
done
CHECK
chmod 0755 /etc/keepalived/check-gateway.sh

STATE=MASTER
UNICAST=""
PREEMPT=""
if [ -n "$KEEPALIVED_PEERS" ]; then
    STATE=BACKUP
    PREEMPT="    preempt_delay 10
"
    UNICAST="    unicast_src_ip ${KEEPALIVED_NODE_IP}
    unicast_peer {
"
    for peer in $(printf '%s' "$KEEPALIVED_PEERS" | tr ',' ' '); do
        [ -n "$peer" ] || continue
        UNICAST="${UNICAST}        ${peer}
"
    done
    UNICAST="${UNICAST}    }
"
fi

# RGW 独立 VIP：同一 keepalived 容器内第二个 vrrp_instance；未配置则不渲染。
RGW_PEERS="$KEEPALIVED_RGW_PEERS"
[ -n "$RGW_PEERS" ] || RGW_PEERS="$KEEPALIVED_PEERS"
RGW_STATE=MASTER
RGW_UNICAST=""
RGW_PREEMPT=""
if [ -n "$RGW_PEERS" ]; then
    RGW_STATE=BACKUP
    RGW_PREEMPT="    preempt_delay 10
"
    RGW_UNICAST="    unicast_src_ip ${KEEPALIVED_NODE_IP}
    unicast_peer {
"
    for peer in $(printf '%s' "$RGW_PEERS" | tr ',' ' '); do
        [ -n "$peer" ] || continue
        RGW_UNICAST="${RGW_UNICAST}        ${peer}
"
    done
    RGW_UNICAST="${RGW_UNICAST}    }
"
fi

cat > /etc/keepalived/keepalived.conf <<EOF
global_defs {
    router_id KEEPALIVED_$(printf '%s' "$KEEPALIVED_NODE_IP" | tr '.' '-')
    enable_script_security
    script_user root
}

vrrp_script chk_gateway {
    script "/etc/keepalived/check-gateway.sh"
    interval 2
    timeout 3
    fall 3
    rise 2
    weight -30
}
EOF

if [ -n "$KEEPALIVED_VIP" ]; then
cat >> /etc/keepalived/keepalived.conf <<EOF

vrrp_instance VOS_FILE_GATEWAY {
    state ${STATE}
    interface ${IFACE}
    virtual_router_id ${KEEPALIVED_VRRP_ID}
    priority ${KEEPALIVED_PRIORITY}
    advert_int 1
${PREEMPT}${UNICAST}    virtual_ipaddress {
        ${KEEPALIVED_VIP}/${KEEPALIVED_VIP_PREFIX} dev ${IFACE}
    }
    track_script {
        chk_gateway
    }
    notify_master "/etc/keepalived/notify.sh MASTER"
    notify_backup "/etc/keepalived/notify.sh BACKUP"
    notify_fault "/etc/keepalived/notify.sh FAULT"
    notify_stop "/etc/keepalived/notify.sh STOP"
    garp_master_delay 1
    garp_master_repeat 3
}
EOF
fi

if [ -n "$KEEPALIVED_RGW_VIP" ]; then
    cat > /etc/keepalived/check-rgw.sh <<'CHECK'
#!/bin/sh
url="${KEEPALIVED_RGW_HEALTH_URL:-http://127.0.0.1:7480/}"
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$url" 2>/dev/null || printf '000')
case "$code" in
    200|403) exit 0 ;;
    *) exit 1 ;;
esac
CHECK
    chmod 0755 /etc/keepalived/check-rgw.sh
    cat >> /etc/keepalived/keepalived.conf <<EOF

vrrp_script chk_rgw_haproxy {
    script "/etc/keepalived/check-rgw.sh"
    interval 2
    timeout 3
    fall 3
    rise 2
    weight -30
}

vrrp_instance VOS_RGW_GATEWAY {
    state ${RGW_STATE}
    interface ${IFACE}
    virtual_router_id ${KEEPALIVED_RGW_VRRP_ID}
    priority ${KEEPALIVED_RGW_PRIORITY}
    advert_int 1
${RGW_PREEMPT}${RGW_UNICAST}    virtual_ipaddress {
        ${KEEPALIVED_RGW_VIP}/${KEEPALIVED_RGW_VIP_PREFIX} dev ${IFACE}
    }
    track_script {
        chk_rgw_haproxy
    }
    notify_master "/etc/keepalived/notify.sh MASTER VOS_RGW_GATEWAY"
    notify_backup "/etc/keepalived/notify.sh BACKUP VOS_RGW_GATEWAY"
    notify_fault "/etc/keepalived/notify.sh FAULT VOS_RGW_GATEWAY"
    notify_stop "/etc/keepalived/notify.sh STOP VOS_RGW_GATEWAY"
    garp_master_delay 1
    garp_master_repeat 3
}
EOF
fi

keepalived -t -f /etc/keepalived/keepalived.conf
exec keepalived -n -l -f /etc/keepalived/keepalived.conf