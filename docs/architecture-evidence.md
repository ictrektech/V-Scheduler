# VOS 资源调度设计：本地架构核验

核验日期：2026-10-13。范围为本地 `docs/ictrek-monorepo/` 源码与正式规范，以及 Image Studio 源码；没有访问部署机器或验证运行中的集群。本文件是设计依据，不表示调度能力已经实现。

## 1. 可以复用的现有能力

| 能力 | 本地证据 | 设计影响 |
|---|---|---|
| 已有集群管理应用身份 | [manifest.yml:14-18](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/apps/com.ictrek.vos-cluster-manager/src/manifest.yml#L14-L18)：`com.ictrek.vos-cluster-manager`，集群管理 / Cluster Manager；[主容器](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/apps/com.ictrek.vos-cluster-manager/src/docker-compose.yml#L1-L19) | 名称与应用已存在；V-Scheduler 单独维护；该既有应用只作为接口证据/可选节点导入来源，不承载本方案新增模块；不代表任何目标机器已安装 |
| 已有子项目及部署前提 | [manifest projects](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/apps/com.ictrek.vos-cluster-manager/src/manifest.yml#L72-L105)、[projects/router.rs:155-166](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/domain/app/projects/router.rs#L155-L166) | 现有模板包含 etcd/postgres-ha/haproxy/temporal；V-Scheduler 自己声明 resource-agent 模板，目标节点须先安装 V-Scheduler；不修改此既有应用 |
| 多通道通信 | [服务发现规范](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/specs/docs/design/vos-app-spec/service-discovery.md#L19-L27)：Traefik 同步、Temporal、Celery、系统服务注入、直连并存 | 调度器不必成为所有 API 的转发入口；保留应用入口和同步数据通道 |
| Temporal 总线 | [规范](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/specs/docs/design/vos-app-spec/service-discovery.md#L63-L90)：Workflow/Activity、连接注入、确定性要求 | 复用持久化流程，不把 Temporal Task Queue 直接等同于 GPU 资源预约 |
| 集群节点注册 | [router.rs](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-cluster-manager/backend/src/router.rs#L37-L43)、[节点 DTO](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-cluster-manager/backend/src/etcd/dto.rs#L64-L80) | 复用节点身份、角色、API 地址；资源遥测另建结构 |
| manager / worker 区分 | [角色定义](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-cluster-manager/backend/src/etcd/dto.rs#L34-L40)：manager 也可运行工作负载 | 管理角色不等于禁止执行任务；需设置基础设施保留资源 |
| 集群基础设施编排 | [Temporal handler](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-cluster-manager/backend/src/handlers/infra_temporal.rs#L301-L334)：远端转发、projects 部署、HAProxy 刷新 | 复用部署及管理入口，不能据此声称已有业务任务调度 |
| Temporal 部署范围 | [Temporal nodes](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-cluster-manager/backend/src/handlers/infra_temporal.rs#L280-L293)：仅 manager 可部署 Temporal 服务 | 工作节点执行器不需要每台都部署 Temporal Server |
| projects 实例生命周期 | [projects 路由](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/domain/app/projects/router.rs#L6-L18)：manifest/list/status/deploy/remove，同步与异步 API | 用于部署执行服务或 Agent；不适合按每个业务请求反复部署整套服务 |
| projects 异步执行 | [ProjectService](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/domain/app/projects/service.rs#L1-L14)：HTTP 前置校验、Workflow、Activity 和健康等待 | 资源调度属于新业务域，可遵循现有异步任务模式 |
| 异步任务抽象 | [AsyncTaskDispatcher](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/async_task/dispatcher.rs#L19-L35)：幂等、任务元数据、状态、取消、终止；用户过滤说明需结合当前 API 实现判断 | 只理解现有任务观察语义，不修改 dispatcher；业务任务由 V-Scheduler 独立 ID/鉴权管理 |
| 当前任务操作鉴权限制 | [api.rs:13-18](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/async_task/api.rs#L13-L18)、[get_status:254-268](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/async_task/api.rs#L254-L268)、[cancel:414-429](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/async_task/api.rs#L414-L429)、[terminate:447-462](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/async_task/api.rs#L447-L462)：登录后按 task_id 操作，取消/终止只将操作者加入审计原因，未见 owner/admin 判定 | V-Scheduler 在自己的 API 落实 scope/所有者授权，不改变平台任务接口 |
| 当前任务列表范围 | [api.rs:271-276](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/async_task/api.rs#L271-L276) 明确去除跨用户隔离；[当前 list:295-335](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/async_task/api.rs#L295-L335) 仅提取登录用户并组装分页/状态/业务域查询 | dispatcher 文件头的早期用户过滤注释不能证明当前 HTTP 列表隔离；调度列表应另行实现明确授权过滤 |
| 异步任务请求 | [StartAsyncTaskRequest](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/async_task/dispatcher.rs#L144-L161)：域、类型、队列、实体、input、幂等键、用户 | 此结构当前没有资源预约、资源需求或优先级字段；任意 JSON input 不代表这些语义已实现 |
| 旧任务委派边界 | [TaskDispatcher trait](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/task/dispatcher.rs#L30-L41)、[TemporalDispatcher 实现](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-platform-backend/src/task/temporal_dispatcher.rs#L1-L25) | 已有启动任务的横切接口；trait 文件的 Phase 0 注释不能覆盖后来已有的实现 |
| caps/profile 兼容性 | [caps 规范](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/specs/docs/design/vos-app-spec/caps.md#L29-L41)、[匹配规则](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/specs/docs/design/vos-app-spec/caps.md#L100-L110) | 复用安装兼容性声明；profile 级独立评估，不能误当动态空闲资源或机器唯一类别 |
| 公开能力与秘密分离 | [vos-common caps](https://github.com/ictrektech/ictrek-monorepo/blob/46651cf5463ce3a77c82031973d4817a0e506883/components/vos-common/src/caps/model.rs#L1-L13) | 资源能力上报仅消费公开 DeviceCaps，不传播 caps secret |

## 2. Image Studio 接入证据与最小改动边界

- 业务提交入口为 [`POST /generate`](https://github.com/ictrektech/image-studio/blob/92de8f31b0bd9a7e10aef5989bd27ae0086b3d8e/app/routers/api.py#L116-L127)，先识别用户并调用 `create_job`。前端继续调用该接口即可，无需直接知道调度器。
- [`generation.py:27-29`](https://github.com/ictrektech/image-studio/blob/92de8f31b0bd9a7e10aef5989bd27ae0086b3d8e/app/services/generation.py#L27-L29) 使用进程内 Semaphore、asyncio Task 与子进程字典；这只能限制该进程并发，不能做跨应用、跨机器预约。
- 本地生成分支先 [`save_job` 后 `asyncio.create_task(run_job)`](https://github.com/ictrektech/image-studio/blob/92de8f31b0bd9a7e10aef5989bd27ae0086b3d8e/app/services/generation.py#L175-L180)；V-Router 分支同样[创建本地协程](https://github.com/ictrektech/image-studio/blob/92de8f31b0bd9a7e10aef5989bd27ae0086b3d8e/app/services/generation.py#L221-L226)。这些是替换为调度提交或许可申请的明确接缝。
- [`run_job`](https://github.com/ictrektech/image-studio/blob/92de8f31b0bd9a7e10aef5989bd27ae0086b3d8e/app/services/generation.py#L247-L265) 获得 Semaphore 后调用 sd.cpp；[`run_v_router_job`](https://github.com/ictrektech/image-studio/blob/92de8f31b0bd9a7e10aef5989bd27ae0086b3d8e/app/services/generation.py#L309-L320) 调用 V-Router。后者资源实际消耗可能在别的服务或外部供应商，不能把整个请求按本地 GPU 预约。
- 取消流程已有[协程取消及子进程终止](https://github.com/ictrektech/image-studio/blob/92de8f31b0bd9a7e10aef5989bd27ae0086b3d8e/app/services/generation.py#L229-L244)，可包装成执行器取消能力；跨节点租约和旧执行隔离必须新增。
- 任务 JSON 已有[本地落盘](https://github.com/ictrektech/image-studio/blob/92de8f31b0bd9a7e10aef5989bd27ae0086b3d8e/app/services/storage.py#L35-L50)，但[启动时 queued/running 被标为失败](https://github.com/ictrektech/image-studio/blob/92de8f31b0bd9a7e10aef5989bd27ae0086b3d8e/app/services/storage.py#L66-L79)，并非重启恢复执行。启用远端调度后必须调整孤儿任务判定，否则应用重启可能错误宣告远端任务失败。

建议第一阶段只接入 Image Studio 的本地 sd.cpp 耗时执行分支：业务校验、模板、历史、结果展示保留；新增调度适配层、执行器契约和 `job_id ↔ task_id` 映射。V-Router 分支待确认实际服务归属及资源责任后再接入。

## 3. 必须新增的能力

以下是设计增量，不能从现有节点管理、caps 或 Temporal 直接推导为已具备：

1. 节点动态资源遥测、设备 UUID、Agent 会话/启动代次，以及执行器能力注册。
2. 任务资源需求、可用执行器版本、数据可达性、资源池和重试幂等契约。
3. 事务化资源预约台账、领取/启动确认、预约租约、执行代次与旧凭证隔离。
4. 跨应用公平排队、配额、优先级老化、阻塞原因和大任务防饥饿规则。
5. 执行终止确认、资源释放确认、失联隔离、调度器重启对账和结果提交校验。
6. Agent 对未接入应用的常驻/外部负载观测，以及常驻模型资源预算。

## 4. 当前核验不能确认的事项

- 实际 VOS 部署版本、集群基础设施健康、Temporal search attributes 是否注册、数据库可用性和节点连接权限。
- 各应用是否全部采用平台 AsyncTaskDispatcher；本文件只深入核验 Image Studio，不声称全应用任务入口已统一。
- 全仓库不存在其他预约模块。本次有界核验仅说明上述接口未提供资源预约语义；zvec 索引缺失，使用本地精确搜索和源码读取，没有创建索引。
- Image Studio 跨节点存储和模型文件可达性，以及 V-Router 后端能否提供可信 GPU 预算。

结论：统一的是参与调度的执行许可和资源账本，业务入口仍可留在各应用。未接入应用可继续运行，但其资源必须作为外部负载或固定预算纳入容量；否则只能保证接入任务之间的预约公平，不能保证整机资源不被抢占。
