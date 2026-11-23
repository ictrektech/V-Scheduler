# SMB / NFS 网关 VIP（keepalived 直连 samba/nfsd）

本组件为 CephFS 存储空间提供 SMB（445）/ NFS（2049）的 **keepalived VIP**。**用户配置入口在 vos-ceph**（`#/file-gateway` + `/api/ceph/gateway/*`），本组件的 `/api/gateway/*` 与 `/api/infra/keepalived/*` 供内部编排调用。

数据面不再经过 HAProxy 的 445/2049 listener：VIP 直接落在当前 keepalived master 的网关节点上，客户端访问 VIP 即由该节点的 samba/nfsd 提供服务。HAProxy 仅保留 postgres HA（15432）与 temporal（7133）的入口。

## 1. 角色

- **所有节点**把同一 CephFS subvolume 挂到 `/share/<space_id>`（由 ceph mount agent 负责）。
- **网关节点**额外运行平台 SMB/NFS 服务并持有 share 记录；同时也是 keepalived VRRP 候选。
- **VIP** 由 keepalived 以 active/backup 持有；同一时刻只有一个节点拥有 VIP。

## 2. 网关集合 / 健康 API

```text
GET  /api/gateway/backends
POST /api/gateway/backends
     { "service": "smb"|"nfs", "node_ips": ["10.100.10.21","10.100.10.22"] }
POST /api/gateway/drain
     { "service": "smb", "node_ip": "10.100.10.21", "drain": true, "reason": "..." }
GET  /api/gateway/status
GET  /api/gateway/health?service=smb|nfs      # keepalived vrrp_script 检查（HTTP 状态码）
```

- `replace_gateway_backends` 是全量替换；变更后由 vos-ceph `apply` 重新部署 keepalived。
- `drain=false`（恢复）默认要求本机服务端口在监听；`force=true` 可跳过（仅运维）。
- `gateway/health` 返回 `200` 才会保持高优先级；排空/未登记/端口未监听返回 `503`。

## 3. keepalived 编排 API

```text
POST /api/infra/keepalived/deploy
     { "node_ip": "...", "vip": "...", "vip_prefix": 24,
       "peers": ["..."], "priority": 100, "vrrp_id": 51,
       "services": ["smb"] }
POST /api/infra/keepalived/remove
     { "node_ip": "..." }   # 或 compose_project_name
GET  /api/infra/keepalived/instances
GET  /api/infra/keepalived/status
```

每个网关节点一个 `cluster-keepalived-<ip-slug>` project 实例。容器使用
`network_mode: host` + `cap_add: [NET_ADMIN]`，镜像内 entrypoint 根据 env 渲染
`keepalived.conf` 并自动识别节点网卡。

## 4. VRRP ID 管理（VCM 数据库分配）

VRRP ID 不再由用户填写。VCM 管理面表 `vcm_vrrp_allocation`（`vip` 唯一、`vrrp_id` 唯一）
负责分配与复用：

```text
POST /api/infra/keepalived/vrrp-id
     { "vip": "10.100.10.240", "preferred": 51 }   # preferred 可省略
POST /api/infra/keepalived/vrrp-id/release
     { "vip": "10.100.10.240" }
```

- 同一 VIP 重复 apply 返回同一 ID；
- `preferred` 空闲则优先使用，否则选最小空闲 ID（默认优先 51）；
- vos-ceph `apply` 固定以**排序后的首个网关节点**作为 allocator，拿到 ID 后回写
  etcd `/vos-ceph/gateway/config`，并传给所有 peer；allocator 变化时以缓存的 ID 作为
  preferred，保持 VRRP 组稳定；
- 前端只展示只读的 VRRP ID（应用后显示），不提供编辑。

VIP 的**子网掩码位数**（如 24 = 255.255.255.0）仍由配置提供，默认 24。

## 5. keepalived 环境变量（VCM 注入，镜像内渲染）

```text
KEEPALIVED_NODE_IP=10.100.10.21
KEEPALIVED_VIP=10.100.10.240
KEEPALIVED_VIP_PREFIX=24
KEEPALIVED_PEERS=10.100.10.22
KEEPALIVED_PRIORITY=100
KEEPALIVED_VRRP_ID=51
KEEPALIVED_INTERFACE=          # 空 = 按 NODE_IP 自动识别
KEEPALIVED_SERVICES=smb        # smb / nfs / smb,nfs
KEEPALIVED_HEALTH_URL_BASE=http://127.0.0.1:19906/app/com.ictrek.vos-cluster-manager
```

`vrrp_script` 每 2s 调用本机 `/api/gateway/health?service=smb|nfs`；连续 3 次失败
降低优先级 30，VIP 漂移到其他网关节点。`drain` 因此可直接触发 VIP 切换。

## 6. 前置条件

- VIP 必须是**独立 IP**，不能是任何网关节点自身 IP。
- 网关节点上 **不能有 HAProxy 监听 445/2049**；当前 HAProxy 只保留 PG/temporal 入口。
- samba-service 监听 `0.0.0.0:445` 即可；VIP 由 keepalived 加到当前 master 网卡。
- 网关节点之间必须保持 **SMB 用户 / 密码 / 共享一致**；否则 VIP 切换后认证或目录视图会变化
  （平台用户是每节点本地库，keepalived 不解决身份同步）。

## 7. 故障与回切

1. 网关服务端口未监听 / 主动 `drain` → health 503 → keepalived 降低优先级 → VIP 漂移。
2. 节点宕机 → VRRP 超时 → backup 接管 VIP；samba 在 backup 已监听 445，无需重新 bind。
3. 节点恢复 → agent 重新挂载 → 端口监听 → health 200 → 按优先级回切（`preempt_delay 10`）。
4. keepalived 容器收到 SIGTERM 时会移除 VIP；异常强杀可能残留地址，需用 `keepalived/status`
   或 `ip addr` 核对。

## 8. 安全

- `NET_ADMIN` 只授予 keepalived 容器，不授予 VCM 主容器。
- VRRP 使用 unicast peer（`unicast_src_ip` + `unicast_peer`），不依赖 multicast。
- 网关健康端点仅返回 200/503 与最小原因，不返回用户/共享数据。
- 排空/恢复/forced 操作均写管理面操作日志（operator、service、node、reason）。
- 不要把 SMB 网关与 etcd/ceph 管理端口放在同一 VIP。
## 7. RGW 对象存储 VIP（HAProxy）

RGW 使用独立 VIP（默认示例 `192.168.100.241:7480`），与文件网关 VIP/VRRP 独立：

- 每节点 HAProxy `listen rgw`：`bind 127.0.0.1:7480` + `<RGW_VIP>:7480`，`mode tcp`、`option tcp-check`，后端为各节点 RGW `:7480`。
- keepalived 同一容器内增加 `vrrp_instance VOS_RGW_GATEWAY`，健康检查 `http://127.0.0.1:7480/`（接受 200/403），状态文件 `state-VOS_RGW_GATEWAY`。
- backup 节点需启用 `net.ipv4.ip_nonlocal_bind=1`，否则无法绑定未持有的 VIP。
- VRRP ID 必须通过 VCM `/api/infra/keepalived/vrrp-id` 从 etcd 全局占用视图分配，不能手填或复用文件网关 51。
- RGW 停用/HA 停用时释放 VRRP，但保留 RGW 用户、桶、对象与节点直连端点。

故障排查：

```bash
# 本机 HAProxy RGW listener
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7480/

# RGW VIP
curl -s -o /dev/null -w '%{http_code}\n' http://192.168.100.241:7480/

# VRRP 状态
cat /var/lib/vos-projects/cluster-keepalived-<ip-slug>/state-VOS_RGW_GATEWAY
```
