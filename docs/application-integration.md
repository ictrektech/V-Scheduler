# 其他应用接入 V-Scheduler

V-Scheduler 统一应用内保留网关与调度控制器。普通模型客户端调用原 V-Router `/v1/*`：网关负责目录、鉴权、协议转换与转发，并为本地/纳管计算申请调度授权。控制器管理节点容量、预约、队列和执行状态，公开前缀为 `/scheduler/api/v1`。业务应用仍负责用户、业务任务、结果文件及其权限；自有受控异步执行器继续使用任务契约。旧端口与数据迁移见[统一应用](unified-application.md)。

vLLM 类服务的完整注册、启用／不启用调度以及外部节点接入步骤见 [vLLM 与外部算力接入（0.0.20）](vllm-and-remote-nodes.md)。

## 统一应用的地址与依赖

VOS 只依赖并安装 `com.ictrek.v-scheduler`；打包时从当前已发布 Release / App Store 解析依赖版本，不再声明必需依赖独立 `com.ictrek.v-router`。升级旧部署时先卸载独立 Router 并保留、原位挂载其数据，避免 `22888` 端口、别名和数据写入冲突。

| 调用方 | 模型 API base | 调度 API base |
|---|---|---|
| VOS 后端容器 | `http://v-router:8080/v1` | `http://v-router:8080/scheduler/api/v1` |
| 同域前端 | `/api/com.ictrek.v-scheduler/v1` | `/api/com.ictrek.v-scheduler/scheduler/api/v1` |
| 局域网服务 | `http://<vos-host>:22888/v1` | `http://<vos-host>:22888/scheduler/api/v1` |

下文 `/tasks`、`/permits`、`/executors` 等短路径均相对于调度 API base。网关 `/api/v1/*` 保留模型与上游管理含义。旧网关别名、旧同域路径及控制器直连 `/api/v1` 仅为已有客户端保留兼容。

VOS 本机资源采集内置于应用，无需应用开发者部署独立 Agent；外部节点通过所属 VOS 应用发现和管理，复用其已有监控；Scheduler 预算规划不直接授予执行权限，远端调度控制待验证，见[VOS 节点管理](node-management.md)。用户无需 SSH 或另装 Agent。自动发现模型不等于批准计算，仍需实际峰值预算与执行器授权。

## 先确认计算发生在哪里

| 调用链路 | 应由谁取得调度授权 | 当前可用接法 |
|---|---|---|
| 应用 A → A 的本地模型 | A | A 持久提交 `POST /tasks`，由批准的 executor 执行；同步模型可由接入网关申请 `POST /permits`。 |
| 应用 B → V-Router → A 的本地模型 | V-Router 与 A 按同一次计算交接 | 同步转发由 V-Router 申请 permit；A 的异步执行由 A 提交 task，V-Router 的桥接 permit 经核验后移交。 |
| 应用 B → A 的模型 API → A 的本地模型 | A | 与 A 自己发起的任务走同一个提交、执行及停止流程。调用者身份只决定业务授权，不绕过调度。 |
| A 或 B 调用真正的外部云端模型 | 外部供应商 | 可以不经过 V-Scheduler；即使调用链经过它，也不占本地 CPU/GPU 预约。V-Router 仍须鉴权和路由。 |
| 模型在另一台已纳管节点上运行 | 实际执行节点对应的 Scheduler 资源池 | 先纳管该节点和 executor，再按其真实位置申请 permit 或提交 task。远端 URL 不自动等于云端。 |

**同一次实际计算只占一次预算。** 不允许 V-Router 和 A 各自独立预约完整 GPU。同步转发由网关持有 permit；异步创建先由网关持桥接 permit，确认 A 的持久任务及批准的 executor 后才经 `/permits/{id}/delegate` 移交。执行中再嵌套调用模型时，经 `/delegations/verify` 核验父任务及预算。仅给上游写 `delegated` 不构成交接证据；未适配交接的执行入口须先完成适配。

## 接入一个本地异步应用

1. 在 A 保留原有创建、查询、取消和输出 API；给每个业务任务生成稳定 `app_task_id`，先把任务及待提交请求写入持久 outbox。重复网络请求继续使用同一个 `Idempotency-Key`，不能以新键重试同一次计算。
2. 管理员在目标节点核验 Agent、模型文件、实际峰值和驻留占用，批准 `executor_id`、`contract_version`、`task_type`、固定 CPU/RAM/GPU 模板及允许的服务身份。跨节点执行还须验证模型、输入与输出在该节点可达。
3. A 用自己的 submitter 令牌调用 `GET /executors` 核验可用契约，再调用 `POST /tasks`。当前 MVP 的任务 body 使用 `protocol_version`、`app_task_id`、`executor`、`parameters`、`inputs: []`；不要发送尚未实现的 artifact 或任意宿主路径。任务受理后保存 Scheduler `task_id`，以 `GET /tasks/{id}` 或 `/events` 跟踪。查询失败时保留原业务任务并重试查询/提交，不直接启动模型。
4. 执行端实现幂等 `start/inspect/cancel`、持久执行句柄、fence 检查和租约到期停止；Agent 收到重复命令须返回同一执行，不启动第二个进程。结果先由 A 写在其私有目录；只有当前 attempt 终态成功后向业务用户公布。
5. 用户取消时 A 请求 `POST /tasks/{id}/cancel`，执行端确认进程停止后才能释放预约或删除输出。执行是否停止未知时资源保持 suspect，不能靠 HTTP 断开或租约过期推断 GPU 已空闲。

Image Studio 的原生图片任务是上述流程的参考实现：用户仍经其 `/api/v1/generate` 创建工作，Image Studio 保存 job/outbox，Scheduler 调度其本地 executor，Image Studio 管理输出。经 V-Router 调用同一个原生模型时，须使用上述桥接移交；Image Studio 自己调用远程云端模型没有本地 GPU 预约。不同应用可复用协议与验收条件，不能复用 Image Studio 的业务文件或凭据。

## 接入一个同步或流式模型

将模型登记到 V-Router，按 `upstreamID/nativeModelID` 在私有 `scheduler.local.json` 中批准模型、真实计算地址、`executor_id`、输入和输出限制。V-Router 在转发前以受限服务身份调用 `POST /permits`，运行中续租，完整终态响应后提供停止证据并释放。资源忙返回 429，配置缺失或 Scheduler 不可用返回 503；不得在错误时直连执行端。客户端断开、续租失败或停止状态不明时保留 suspect，待执行端核验。WebSocket 还需明确执行端在停止后正常关闭的契约。

A 若同时提供直接调用和 V-Router 调用的同步入口，必须让两条入口使用同一授权和交接契约；不能让 A 在网关已预约时再次独立申请完整预算。V-Router 为同步 permit 或 delegated 子许可绑定随机 execution_token，通过私有头交给后端；后端用自己的 submitter 身份调用 `POST /permits/{id}/verify` 原子认领，核验模型、真实地址、执行器和租约。后端只获明确的 permit_executors 核验权限，不获网关许可的续租/释放权。后续核验保持相同 execution_id；重复认领、错误绑定、过期许可和已终止父任务均不能启动新计算。详见[接入标准](integration-standard.md)。新协议仍须逐应用实测，不能仅凭入口已注册就宣称安全。

## 安装、权限与逐节点验收

V-Voice 已提供同一 VOS 内的专用授权接收端。统一应用可自动交接独立服务身份、
执行器及网关同步验票权限，省去用户手工分发令牌；仍只允许已校准且就绪的模型。
流程、校准清单、持久 revision 与凭据轮换见 [V-Voice 主动接入](v-voice-auto-binding.md)。

- VOS 包依赖只检查应用安装及运行，不提供 Scheduler URL、令牌、节点容量或执行器。服务令牌放在 A 或 V-Router 的私有数据目录，权限 `0600`；浏览器与普通用户不能持有它。为不同应用绑定各自 `app_id` 和允许的 executor。
- 对每个节点分别验证：真实模型完成一项任务、重复提交仅执行一次、忙碌/过期遥测/排空时请求不能到达执行端、取消后有停止证据、断连与重启后资源状态正确。云端请求不产生本地 permit。检查实际执行节点，而非仅看另一个节点的成功结果。
- 当前实现的精确字段和能力以 [`GET /discovery`、公开 API 与接入标准](integration-standard.md)和 [OpenAPI](../api/openapi-v1.json)为准；V-Router 的具体配置和桥接协议见[网关资源准入指南](resource-scheduling.md)。
