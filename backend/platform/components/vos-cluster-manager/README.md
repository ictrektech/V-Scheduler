# vos-cluster-manager

VOS 集群管理应用：以**独立 VOS 应用**形态承载集群管理能力。主容器仅包含用户管理的前后端
服务（Rust 后端 + Vue 3 前端），**etcd 与 postgres-ha（patroni 高可用）作为平台 projects
子项目动态启停**；集群创建 / 成员增加时用户输入节点 IP 与管理员密码，持久化到 **etcd
记录的集群状态**中。

- **集群状态**：etcd（唯一权威信源，兼作 patroni DCS）+ 高可用 postgres（patroni 主从）
- **动态编排**：经平台 `/v1000/app/projects/deploy(_async)/remove(_async)` 启停 etcd、
  postgres-ha、haproxy、temporal、keepalived 子项目实例（vos-app-spec §3.9 projects 通用原语）
- **异步任务**：节点本地多步编排经平台 temporal（`VOS_HOST_GW_IP:VOS_TEMPORAL_PORT`）
  Workflow + Activity 实施；管理面状态以 sea-orm 记录在平台 postgres（
  `VOS_DB_*`，启动 schema sync 建表）
- **一期不做**：集群级 temporal（每集群独立部署 / 独立命名空间，二期评估）

> **设计缘起**：本组件延续了被废弃的 vos-platform-backend 集群方案
> （`components/vos-platform-backend/docs/cluster-system-branch-review.md`）的**核心架构**
> （etcd 拓扑权威信源 + patroni 高可用），但集群逻辑全部收敛到独立应用侧——平台只提供
> projects 通用原语（详见 `components/vos-platform-backend/docs/app-projects-design.md`）。

接口设计遵循 [VOS HTTP 响应结构统一规范](../../specs/docs/design/vos-http-response-spec/)，
响应体统一为 `ResultBody { code, category, data, msg, version }`，`category = "vos-cluster-manager"`。

## 技术栈

| 端 | 技术栈 |
|:---|:---|
| 后端 | Rust edition 2024 + tokio + axum 0.8 + trackerror（错误码 `[20500, +∞)`）；etcd 客户端 `etcd-client`（后续阶段接入） |
| 前端 | pnpm@11.10.0 + Vue 3.5 + TypeScript 5 + Vite 7（base `./`）+ Ant Design Vue 4（hash 路由） |
| 容器化 | multi-stage Dockerfile（builder: vos-builder / runtime: ubuntu:24.04），主容器仅前后端 |
| VOS 打包 | `apps/com.ictrek.vos-cluster-manager/`（五件套 + projects/ 子项目 + Makefile + CI） |

## 目录结构

```
components/vos-cluster-manager/
├── README.md              # 本文件
├── Dockerfile             # multi-stage 构建（主容器仅前后端）
├── Dockerfile.dockerignore
├── docs/
│   └── roadmap.md         # ★ 设计实施路线图（SSOT，阶段 / 依赖 / 里程碑）
├── backend/               # Rust 后端（axum HTTP 服务）
├── frontend/              # Vue 3 前端（Vite base='./' + hash 路由）
├── openspec/              # OpenSpec 变更管理
│   ├── README.md
│   ├── config.yaml
│   └── changes/           # 活跃变更（vos-cluster-manager-foundation 占位）
├── scripts/
│   └── container/         # start.sh + healthcheck.sh
└── .agents/               # vos-cluster-manager Agent 角色定义与长时任务状态
```

## 后端启动参数

后端启动时必须指定以下参数（均支持环境变量来源，优先级 CLI 参数 > 环境变量 > 默认值）：

| 参数 | 环境变量 | 说明 |
|:---|:---|:---|
| `--port` | `VOS_CLUSTER_MANAGER_PORT` | HTTP 监听端口（默认 18093） |
| `--frontend-dir` | `VOS_CLUSTER_MANAGER_FRONTEND_DIR` | 前端 dist 目录（构建产物） |
| `--data-dir` | `VOS_CLUSTER_MANAGER_DATA_DIR` | 数据目录（运行期临时文件 / 开发模式本地状态；管理面状态以平台 postgres 为准） |
| `--url-base` | `VOS_CLUSTER_MANAGER_URL_BASE` | （可选）URL 前缀，使整个工具可托管在任意子路径下 |
| `--log-level` | `VOS_CLUSTER_MANAGER_LOG_LEVEL` | 日志级别，默认 `info` |
| `--etcd-endpoints` | `VOS_CLUSTER_MANAGER_ETCD_ENDPOINTS` | （可选，逗号分隔）etcd 端点，默认 `127.0.0.1:2379`；后续阶段接入 |

## 路由设计

| 路由 | 说明 |
|:---|:---|
| `<url-base>/` | 托管前端 `index.html`（SPA） |
| `<url-base>/api/health` | 健康检查（`{ status, version, etcd_configured, timestamp }`） |
| `POST <url-base>/api/cluster/join` | 本节点入群：已安装本应用但未入群的节点，录入目标集群任一管理节点 IP + 管理员密码即可加入；**编排转由 seed 管理节点执行** `POST /api/nodes`（`join_infra=true`），本节点只等待本地 etcd 出现 `/vos/cluster/info` |
| `POST <url-base>/api/nodes` | 集群内**扩容**：向已创建的集群追加成员（`join_infra=true` 联动 learner 加入 → 部署 etcd(existing) → promote → pg join） |

当指定 `--url-base <prefix>` 时，以上所有路由均挂载到该前缀下，用于在 VOS 平台以任意子路径托管本组件。

## 数据目录布局

```
<data-dir>/                    # VOS 应用数据目录（/var/lib/vos-cluster-manager）
└── （运行期临时文件 / 开发模式本地状态）

管理面状态（集群注册 / 编排任务等）以 sea-orm 记录在平台 postgres（`VOS_DB_*`，
启动 schema sync 建表）；集群状态以 etcd 为准；
节点本地多步编排经平台 temporal（`VOS_HOST_GW_IP:VOS_TEMPORAL_PORT`）Workflow 实施。
```

## 阶段范围

| 阶段 | 能力 | 说明 |
|:---|:---|:---|
| Phase 0 | 组件骨架 | 前后端最小可运行 + 容器化 + VOS 应用打包（主容器仅前后端；manifest 声明 etcd / postgres-ha projects；patroni 镜像独立构建为应用资产） |
| Phase 1 | 集群注册与节点录入 | etcd 接入：集群初始化 / 节点录入（IP + 管理员密码）持久化到 etcd key |
| Phase 2 | etcd 子项目编排 | 经 projects API 动态启停 etcd 实例（多节点 raft） |
| Phase 3 | postgres-ha 子项目编排 | patroni bootstrap / join / failover，与 etcd DCS 联动 |
| Phase 4 | 集群创建编排 | 多节点一键创建 + 成员增删（本地 temporal workflow + saga 补偿） |
| Phase 5 | 前端完善 | 创建向导 / 节点管理 / 集群概览 / 拓扑 / 健康监控 |

> 完整设计实施路线图见 [`docs/roadmap.md`](docs/roadmap.md)。

## 文档

- **设计实施路线图**：见 [`docs/roadmap.md`](docs/roadmap.md)
- **需求分析 / 详细设计 / 任务规划**：见 [`openspec/changes/`](openspec/changes/)
- 组件开发规范：见组件内 `.agents/` 与 `openspec/README.md`