# etcd 认证 / TLS 迁移与运维

本文是 `cephfs-multinode-storage-provider`（D9）的 etcd 安全前置操作手册。目标：把集群 etcd
从「明文、匿名」迁移到「TLS + scoped 身份」，并保证迁移可回滚。

## 1. 凭据布局

宿主根目录默认 `/etc/vos/etcd`（可用 `VOS_CLUSTER_MANAGER_ETCD_CREDENTIAL_DIR` 覆盖），
挂载到 `vos-cluster-manager` 容器同路径，etcd 子项目只读挂载 `pki/`。

```text
/etc/vos/etcd/
├── pki/                     # 0700
│   ├── ca.crt               # 0644，集群内部 CA
│   ├── ca.key               # 0600，只在执行 bootstrap 的节点存在，不随凭据分发
│   ├── server.crt           # 0644，SAN 覆盖全部 etcd 成员 IP + 127.0.0.1 + localhost
│   ├── server.key           # 0600
│   └── root.password        # 0600，etcd root 用户口令（仅 bootstrap 节点）
└── clients/                 # 0700
    ├── vcm.json             # 0600，VCM 自身
    ├── vos-ceph.json        # 0600，vos-ceph 主容器
    └── agent-<node>.json    # 0600，每节点 mount agent
```

凭据文件内容为 JSON：`identity / username / password / ca_cert / client_cert / client_key /
endpoints / key_prefix / issued_at / generation`。任何接口和日志都不回显口令。

## 2. 角色与 ACL

| 角色 | 前缀 | 权限 |
|:--|:--|:--|
| `vcm` | `/vos/cluster/` | 读写 |
| `vcm-readonly` | `/vos/cluster/` | 只读 |
| `vos-ceph` | `/vos-ceph/` | 读写 |
| `vos-ceph-agent-<node>` | `/vos-ceph/` 读 + `/vos-ceph/spaces/` 写 | agent 状态写入 |

> 说明：当前 `/vos-ceph/*` schema 没有按节点拆分的 status 前缀，agent 的写权限到
> `/vos-ceph/spaces/` 级；这是 v1 已知限制，不要据此宣称节点级密钥隔离。

## 3. 迁移顺序（必须按序）

### 3.1 生成 PKI

在每个节点执行（同一份 CA 由首个节点生成后分发；服务端证书 SAN 需覆盖全部成员 IP）：

```bash
docker exec <vcm-container> vcm-etcd-auth pki \
  --node-ips 192.168.100.231,192.168.100.232,192.168.100.233 \
  --identities vcm,vos-ceph \
  --credential-dir /etc/vos/etcd
```

### 3.2 创建角色/用户（暂不启用认证）

```bash
docker exec <vcm-container> vcm-etcd-auth bootstrap \
  --auth-mode user \
  --node-ips 192.168.100.231,192.168.100.232,192.168.100.233 \
  --no-enable-auth
```

此步骤会生成 `clients/*.json`。把对应身份文件分发到各节点（权限 0600）：

- `vcm.json` → 各节点 VCM 容器
- `vos-ceph.json` → 各节点 ceph 主容器
- `agent-<node>.json` → 各节点 cephfs-mount agent

### 3.3 切换客户端配置（保持 etcd 明文监听）

各组件设置：

```text
VOS_CLUSTER_MANAGER_ETCD_AUTH_MODE=auto
VOS_CEPH_ETCD_CA=/etc/vos/etcd/pki/ca.crt
VOS_CEPH_ETCD_CERT=/etc/vos/etcd/clients/vos-ceph.crt
VOS_CEPH_ETCD_KEY=/etc/vos/etcd/clients/vos-ceph.key
VOS_CEPH_ETCD_USERNAME=vos-ceph
VOS_CEPH_ETCD_PASSWORD=<clients/vos-ceph.json 中的 password>
```

重启 VCM / platform / ceph 后，用只读接口确认连接正常。

### 3.4 重新部署 etcd（TLS 监听）

把 etcd 子项目的部署 env 打开 TLS：

```text
ETCD_LISTEN_CLIENT_URLS=https://0.0.0.0:2379
ETCD_ADVERTISE_CLIENT_URLS=https://<node-ip>:2379
ETCD_CLIENT_CERT_AUTH=true
ETCD_TRUSTED_CA_FILE=/etc/vos/etcd/pki/ca.crt
ETCD_CERT_FILE=/etc/vos/etcd/pki/server.crt
ETCD_KEY_FILE=/etc/vos/etcd/pki/server.key
ETCD_CERT_HOST_DIR=/etc/vos/etcd/pki
```

`vos-cluster-manager` 在 `VOS_CLUSTER_MANAGER_ETCD_AUTH_MODE` 为 `user/mtls/mtls_user`
时会自动把这些 env 写入 etcd 子项目部署参数。然后重新 deploy etcd（数据目录保留）。

### 3.5 启用认证（最后一步）

```bash
docker exec <vcm-container> vcm-etcd-auth bootstrap \
  --auth-mode user \
  --node-ips 192.168.100.231,192.168.100.232,192.168.100.233 \
  --enable-auth
```

启用前必须确认：
- 所有客户端已改为带凭据连接；
- 至少一个 root 凭据可用（`pki/root.password`）；
- 明文 endpoint 不再被任何组件使用。

### 3.6 验证

```bash
docker exec <vcm-container> vcm-etcd-auth status
# 匿名访问应被拒绝
curl -s http://127.0.0.1:2379/v3/kv/range -d '{"key":"L3Zvcy8="}' | head
# 带凭据访问应成功（组件健康检查 / spaces API）
```

## 4. 轮换与吊销

```bash
# 轮换单个身份（重写 clients/<identity>.json，并同步分发）
docker exec <vcm-container> vcm-etcd-auth rotate --identity vos-ceph

# 吊销节点 agent（删除 etcd 用户 + 本机凭据）
docker exec <vcm-container> vcm-etcd-auth revoke --identity agent-192.168.100.232
```

轮换后重启对应组件；agent 被吊销后其空间会进入 degraded/unavailable，需重新签发。

## 5. 回滚

紧急情况下：

```bash
# 预检（不执行）
docker exec <vcm-container> vcm-etcd-auth disable
# 确认回滚（关闭 etcd 认证）
docker exec <vcm-container> vcm-etcd-auth disable --yes
```

随后把所有组件的 `VOS_*_ETCD_AUTH_MODE` 改为 `disabled` 并重启，恢复明文监听
（`ETCD_LISTEN_CLIENT_URLS=http://0.0.0.0:2379`）。回滚不删除 etcd 数据。

> 注意：关闭认证后，任何能访问 2379 的进程都可读取 `/vos-ceph/secret/*` 与
> `/vos/cluster/nodes/*`。回滚窗口应尽量短，并在恢复后重新启用。

## 6. 检查清单

- [ ] 所有 etcd 客户端已支持 TLS/凭据配置
- [ ] `pki/ca.key` 未分发到非 bootstrap 节点
- [ ] 凭据文件权限 0600、目录 0700
- [ ] 匿名 endpoint 已关闭
- [ ] `vcm-etcd-auth status` 显示 auth enabled 且角色/用户齐全
- [ ] 组件健康检查、spaces API、agent 状态均正常
- [ ] 回滚脚本与 root 口令可用