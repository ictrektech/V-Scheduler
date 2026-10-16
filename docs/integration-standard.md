# V-Scheduler 公开 API 与接入标准 v1

> 接入操作统一参见 [V-Scheduler 统一接入指南](application-integration.md)。本页保留协议或部署参考。

日期：2026-10-15。状态：完整目标协议与 MVP 实现范围。适用于所有接入方，不以是否位于 VAI `apps/` 或是否运行 VOS 为前提。[设计](design.md)规定资源与恢复不变量；[OpenAPI](../api/openapi-v1.json)描述当前已实现的 v1 HTTP 路由。三者变更必须一起维护。

本文的 V-Scheduler 指包含 V-Router 网关与调度控制器的统一 VOS 应用；以下协议属于调度控制器。模型请求仍由网关 `/v1/*` 转发。安装迁移见[统一应用](unified-application.md)，选型见[应用接入指南](application-integration.md)。

## 1. 接入角色和保证等级

| 角色 | 必须实现 | 无需实现 |
|---|---|---|
| submitter：应用、脚本、外部 Web 服务 | 服务身份、幂等提交、状态查询/事件去重、取消语义、artifact 引用 | 资源探测、Agent、VOS API |
| worker：标准 Agent 或第三方实现 | 认证注册、资源/执行器上报、命令接收去重、lease、start/inspect/cancel、持久化与停止证据 | VOS projects、VAI 目录、业务前端 |
| executor：受控业务运行时 | worker 管理下的执行句柄、幂等启动、停止核验、结果暂存、执行器 watchdog | 全局排队算法 |
| observer：旧应用监测适配 | 可核验资源观测/固定预算 | 完整任务恢复；不得声称全局排队保证 |

submitter 只能请求已注册的执行器和 task type。注册第三方 worker 需管理员批准能力与资源池；不允许任意公开客户端上传命令后在节点运行。

最低兼容等级：S1 提交/查询/取消；W1 受控单机执行；W2 持久恢复与安全重试；W3 跨机数据/输出兼容。不能安全停止或持久定位执行的 worker 标为观察/不可重试，不通过 W2。等级是验收条件，不是客户端自报字段。

## 2. 地址、发现和版本

- 统一应用公开 base URL：`http://<vos-host>:22888/scheduler/api/v1`，或 VOS 网关 `/api/com.ictrek.v-scheduler/scheduler/api/v1`。
- 控制器内部与旧客户端兼容 base URL：`https://<controller>/api/v1`；保留已有 Agent、执行器和服务客户端。
- 旧 VOS 包挂载 `/app/com.ictrek.v-scheduler/api/v1` 保留兼容；新客户端使用上面的显式调度前缀。普通模型调用使用网关 `/v1/*`。
- `GET /discovery` 返回协议版本、实际开启的能力、认证模式和本实例 ID；不返回节点地址、任务、密钥或容量细节。
- HTTP 协议主版本在路径；`protocol_version=1` 标识消息主版本；执行器另有 `contract_version`，模型 ID/版本另行匹配。
- v1 只增加可选响应字段；客户端忽略未知可选字段，遇到未知状态禁止将其当成成功。新增必需字段、删字段或改变单位需 v2。
- 本文定义的所有必需端点只有完成实现和 conformance 后才在 discovery 声明；当前已支持 permits，webhook 和 SDK 不作为第一版已支持能力。

## 3. 认证与授权

服务与 worker 使用 `Authorization: Bearer <service-access-token>`。本应用保存凭据 hash/验证材料，管理员创建、撤销、轮换，并绑定 workspace、app ID、允许 executor 和 scope；实际令牌格式是服务端实现细节，不能要求接入方读取 VOS 本地存储。

scope 最少分为 `tasks:submit/read/cancel/retry`、`artifacts:register/read`、`workers:connect`、`admin`。worker 只能领取自己的节点命令、续租自己的当前 attempt、回报自己的结果；submitter 不能伪造 worker 报告。

workspace/app/owner 由验证过的身份决定。业务 user label 不授予跨用户权限。VOS 管理 UI 复用已存在 OIDC/userinfo；外部管理 UI 可配置受信 OIDC。非 VOS 服务不要求持有 VOS JWT。第一版按服务身份隔离任务；用户级委托作为明确验证过的扩展，不伪称已取得任意业务用户身份。

远程连接使用 HTTPS 并核验服务器证书，不默认关闭 TLS 验证；明确批准的同机/隔离内网可用 HTTP。公开 API 意味着协议可用文档描述，不意味着匿名执行或匿名查看任务。

## 4. 通用 HTTP 约定

JSON 使用 UTF-8，时间为 RFC3339 UTC，CPU 单位 millicores、内存 MiB、duration 秒、fence/sequence 为非负整数。request ID 由服务端返回，客户端可传关联 ID；不能用请求 ID代替幂等键。

响应结构延续 VOS ResultBody 风格，但由本应用实现，不依赖 VOS：

```json
{"code":"OK","category":"v-scheduler","data":{"task_id":"task-example","status":"queued","version":1},"msg":"accepted","version":"1","request_id":"req-example"}
```

顶层 version 是协议主版本的字符串；`data.version` 是任务状态版本整数。错误仍有上述 envelope，`data` 可含 retryable/retry_after_seconds/details。成功 HTTP 不意味着业务计算完成。

| HTTP | 稳定 code 示例 | 客户端动作 |
|---|---|---|
| 200 / 201 / 202 | OK | 按返回状态处理；202 是受理 |
| 400 / 422 | INVALID_REQUEST / INVALID_RESOURCE_SPEC / UNSUPPORTED_PROTOCOL | 修改请求，不能盲重试 |
| 401 / 403 | UNAUTHENTICATED / FORBIDDEN | 检查令牌或授权 |
| 404 | NOT_FOUND | 不泄露其他 scope 的任务是否存在 |
| 409 | IDEMPOTENCY_CONFLICT / STATE_CONFLICT / STALE_ATTEMPT | 原键内容冲突不能换键绕过；旧 attempt 停止并对账 |
| 429 | QUOTA_EXCEEDED | 遵守 Retry-After，不新建幂等键 |
| 503 | TEMPORARILY_UNAVAILABLE | 有界退避，原键重试 |

列表 `limit` 默认 50、最大 200，使用不透明 cursor；事件 cursor 超出保留窗口返回 410/CURSOR_EXPIRED，客户端重新查询自己的任务快照后从新 cursor 继续。单个请求体上限、事件保留期及排队上限由 discovery/运维配置明确。

## 5. submitter 接口

下列短路径均相对于第 2 节的调度 base URL；统一网关为 `/scheduler/api/v1`，旧控制器直连才使用 `/api/v1`：

| 方法/路径 | 行为 |
|---|---|
| GET `/discovery` | 非敏感协议/能力发现 |
| GET `/executors` | 返回身份允许使用的 executor/type/contract/model 与资源模板 |
| POST `/artifacts` | 登记已上传到批准存储的对象，返回 artifact_id；不接收任意宿主机路径/任意下载 URL |
| POST `/tasks` | 持久提交，`Idempotency-Key` 必需；新任务 202，重复相同内容 200 |
| GET `/tasks` | 授权范围内分页列表 |
| GET `/tasks/{task_id}` | 状态、进度、等待原因、attempt、结果 artifact 引用 |
| POST `/tasks/{task_id}/cancel` | 请求取消，可重复；正在取消返回 202，终态返回 200及当前状态 |
| POST `/tasks/{task_id}/retry` | 授权显式重试；旧执行 unknown/未停止则 409 |
| GET `/events` | 基于 cursor 的授权增量事件，含提交时状态与进度快照；至少一次、按任务 version 去重 |

`Idempotency-Key` 与 body 中 `idempotency_key` 若同时给出必须一致；推荐只用 header。去重作用域为 workspace + app + key。相同键不同规范化内容返回 409，网络超时后保留原键查询/重提。手工 retry 另需幂等键以防一次点击产生多个新 attempt。

排队中的任务在 `wait_reasons` 返回当前快照原因：`WAIT_NODE_OFFLINE`、`WAIT_NODE_DRAINING`、`WAIT_TELEMETRY_STALE`、`WAIT_EXECUTOR_UNAVAILABLE`、`WAIT_EXECUTOR_UNHEALTHY`、`WAIT_CAPACITY`、`WAIT_OBSERVED_PRESSURE` 或 `WAIT_PRIOR_EXECUTION`；自动重试退避期间返回 `WAIT_RETRY_BACKOFF`。原因可能随心跳、探测、预约变化而变化，不是事件状态转移。承诺容量由 executor 的 `resources`、节点 `resident_baseline` 和未释放预约共同计算；观测余量只作保守准入检查，不能替代事务预约。

任务至少包含 `protocol_version`、`app_task_id`、`executor`、`parameters`、输入 artifact 列表；resource、timeout、retry 的可选覆盖必须在批准模板范围内。客户端不能任意申报零资源、优先级或任意镜像。完整示例见 [设计第 5 节](design.md#5-任务协议-v1拟新增)。

幂等提交示意（所有域名/令牌为占位）：

```http
POST /scheduler/api/v1/tasks HTTP/1.1
Host: scheduler.example.invalid
Authorization: Bearer <service-access-token>
Idempotency-Key: business-job-001
Content-Type: application/json

{"protocol_version":1,"app_task_id":"business-job-001","executor":{"id":"example.video.generate","contract_version":"1","task_type":"video.generate"},"parameters":{"duration_seconds":5},"inputs":[]}
```

完整流程：批准服务身份 → 发现执行器 → 登记输入 → 原应用保存 job/outbox → 提交 → 持久化返回 task ID → 查询/拉事件 → 下载已发布结果或请求取消。应用业务结果仍由应用自己授权交付。

## 6. worker 公开接入标准

0.0.20 的内部组件登记协议由 VOS App 实现方使用，用户仍只安装 App，见[节点管理](node-management.md)。内部协议中：一次性邀请绑定 node_id，Agent 自行持久保存 worker_token 与 session_id，`POST /workers/register` 只创建 pending 申请，管理员批准后才开始心跳和执行。下表是完整协议目标；尚未落地的独立 ACK、cursor 等字段不能发送给当前严格结构，精确字段以 OpenAPI 为准。

管理员在中心批准 worker 登记并发放受限身份；worker 主动出站连接，不要求中心对外部机器开任意 SSH/管理端口。

| 方法/路径 | 必需字段/行为 |
|---|---|
| POST `/workers/register` | enrollment 授权、node_id、instance/boot、protocol_version；绑定 scope，返回 session_id |
| POST `/workers/{node_id}/heartbeat` | session、boot、report_seq、资源池、执行器、正在执行 attempt；乱序报告忽略，旧会话拒绝 |
| POST `/workers/{node_id}/commands/claim` | session 和领取 cursor；返回本节点 command 列表，可重复投递 |
| POST `/workers/{node_id}/commands/{command_id}/ack` | accepted/rejected 及 evidence；ACK 前 durable journal，不能凭受理就报告 running |
| POST `/attempts/{attempt_id}/renew` | reservation_id、fence、session、report_seq；只续当前 attempt，返回 TTL |
| POST `/attempts/{attempt_id}/report` | fence、session、状态、可选进度/阶段/说明或停止证据/暂存结果；旧 attempt 拒绝 |

执行端的最低要求是开始与结束；不提供中间进度时 `progress` 可省略。能感知运行过程的执行器可在 `progress` 中报告 `percent`（0–1 小数），或额外报告 `stage`（最多 128 字节）和 `message`（最多 512 字节）；顶层的阶段/说明会并入最新进度快照。执行器的 `inspect` 响应也可带这两个可选字段，由标准 Agent 透传。任务查询给最新快照，`/events` 给每个已提交状态版本的快照；旧版本事件的 `progress` 为 `{}`。业务方按 `task_id + version` 去重，不能从消息或进度推断任务已成功，终态仍以 `status` 为准。详见 [OpenAPI Report](../api/openapi-v1.json)。

命令至少包含 task/attempt/reservation/command ID、fence、executor contract、输入与模板参数、资源池绑定、lease TTL；worker 必须验证身份、资源容量、版本和数据可达性后启动。同一 attempt 重复命令只返回原 execution handle。

TTL 从接收时的单调时钟计算，过期停止；控制器资源仍保持 suspect 至核验实际退出。不能因返回租约超时就重派旧执行未知的任务。fence 只隔离旧结果，不自动停止旧 GPU。输出由当前 attempt 写到独立暂存位置，controller 条件提交后才发布。

第三方 worker 的规范化操作必须等价于 `start/inspect/cancel/collect`；本地实现可以使用其自己的 API/进程/容器，但要满足上述幂等、持久化、停止证据和 watchdog。平台/API 超时和 worker 崩溃都不得无条件释放资源。

资源报告独立于 VOS：arch/device/runtime、capacity/allocatable、memory_pool_id、accelerator ID、固定预算、resident model、executor 清单；统一内存只能一个预算池。未批准的执行器上报先 pending，不自动接收所有 task type。

## 7. 数据标准

artifact 必需属性：workspace 作用域、批准 storage_id + object_key、大小、SHA-256、媒体类型、可访问节点集合/传输方式。注册只是元数据，不是文件上传完成证明；执行前校验对象存在、权限和校验和。

第一版支持管理员批准的共享对象存储/共享目录映射，不接受客户端任意路径。VOS hints 只是宿主共享目录选择适配；跨机/外部使用显式对象存储或传输适配，不能把相同路径字符串视为同一文件。预签名 URL/直接上传可后续扩展，只有实际实现才在 discovery 声明。

## 8. 接入验收与发布标准

S1：非 VOS HTTP 客户端提交、查询、取消；错误码/权限范围/幂等冲突/事件重复与 cursor 过期处理正确。

W1：能力匹配、资源二次核验、启动去重、running 与 ACK 分离、停止后释放。

W2：启动 ACK 丢失、journal/控制器重启、网络分区、租约过期、旧结果延迟、输出发布丢失；无并行新旧 attempt，未知保留隔离。

W3：输入跨机授权、模型/contract 版本、输出数值容差、多 app 公平性和配额通过。登记不存在的 contract/version 时必须拒绝。

实施时提供与 OpenAPI 同步的 JSON Schema、契约测试与可复现 demo；SDK 是可选便利层，标准客户端只需 HTTP/JSON。当前 OpenAPI 不描述尚未冻结的全部 worker payload 子字段，标注 DraftMessage；不能据此宣称 worker 标准已经冻结。Phase 0 需补齐资源/命令/事件 schema，Phase 4 前提供独立第三方实现的 conformance 证据。


## 当前 MVP 接入范围

接入方不必按“调度器 → V-Router → 原应用”串联。原生异步重任务由原应用提交任务并保留结果所有权；V-Router 为实际经它路由的本地/纳管远端模型计算申请准入。若执行器已持有任务预约，V-Router 的对应上游须经管理员批准为 `delegated`，避免重复扣同一节点预算；云端上游绕过本机资源预约。按 task type 和真实执行位置选择唯一责任方，见[设计第 2.3 节](design.md#23-v-router-不是所有算力应用的必经层)。

以 OpenAPI 与 discovery 声明为准。controller 当前使用受保护配置中的 admin/submitter/worker 凭据，submitter 按 app_id 隔离，worker 绑定 node_id；VOS 管理界面通过平台 `/v1000/user/check` 校验启用的 system_admin；0.0.20 已提供受控动态登记与管理 API，artifact 上传仍不支持。任务只引用预先批准的 executor，资源预算来自其模板，inputs 必须为空；应用保留自己授权的任务与文件。原协议中 workspace、artifact、优先级等仍属于后续目标，不能发送给当前严格的提交结构。动态登记使用当前 OpenAPI 的邀请、worker 登记及执行器审批结构，不能提交任意命令。

### 本地与跨节点资源如何选择接口

同一 VOS 的 V-Voice 提供专用主动接入：统一包核验应用 context 身份与实际安装状态，
仅按已校准、与真实硬件和运行设置匹配的预算，为就绪模型建立身份、执行器并推送
绑定。网关和 Agent 热更新，不要求用户分发凭据。它不属于通用公开动态注册 API，
也不改变未批准模型、过期遥测或资源不足时的拒绝规则，见 [V-Voice 主动接入](v-voice-auto-binding.md)。

| 实际资源 | 接入接口 | 预约归属 |
|---|---|---|
| 外部云端模型 | V-Router `/v1/*`；不调用本 API | 云端供应商管理，不扣 V-Scheduler 节点容量 |
| 本 VOS 节点或其他已纳管节点的同步/流式模型 | V-Router `/v1/*`，由网关以其服务身份调用 `POST /permits` | V-Scheduler 按真实执行节点的 Agent 报告、executor 固定模板和未释放预约准入 |
| 本 VOS 节点或其他已纳管节点的异步长任务 | 业务应用以自己的服务身份调用 `POST /tasks` | V-Scheduler 排队、续租与记录状态，业务应用保留用户授权和结果 |
| 未纳管的远端自建服务 | 先决定资源责任和纳管方式 | 未纳管时不得声称本 API 已调度；远端 URL 不能自动视为云端 |

VOS 只安装统一的 `com.ictrek.v-scheduler`；其他应用依赖该 app ID，打包时解析当前已发布依赖版本，不再强制依赖独立 V-Router。包内网关设置 `V_ROUTER_SCHEDULER_REQUIRED=true`。本机 Agent 内置，但服务凭据、固定预算和 executor 仍须逐节点批准与验证。缺少有效配置时本地网关请求返回 503，不回退为直连；容量不足返回 429。云端模型在网关跳过许可逻辑。`POST /permits` 的请求是 `{"executor_id":"<批准的执行器>","upstream_url":"<批准的模型服务 base URL>","model_id":"<原生模型 ID>"}`，带服务 Bearer 和稳定的 `Idempotency-Key`；许可 ID 只由网关持有。正常同步结束调用 `/permits/{id}/release` 并提供 `stopped=true` 与证据；断连或执行状态未知用 `stopped=false`，待核实后调用 `/permits/{id}/confirm-stopped`。流式计算不能在首个响应字节后释放。V-Router 的私有配置及 `delegated` 策略见[网关资源准入指南](resource-scheduling.md)。

异步任务提交 `POST /tasks`，使用业务任务 ID 作为稳定幂等键；返回 `task_id` 后查询 `/tasks/{id}` 或按版本消费 `/events`。取消通过 `/tasks/{id}/cancel` 请求停止，停止证据未确认前保持预约。业务执行器已持有任务预约且经 V-Router 调用同一计算时，上游必须由管理员核准为 `delegated`，通过 `/delegations/verify` 申请关联子许可，不重复扣父任务预算。一个计算只有一个预约所有者；调度控制器不转发模型请求体或响应，数据转发由同一应用内的网关负责。

Image Studio 用业务任务 ID 作为幂等键，保留原业务入口与输出。Rust Agent 将 command 持久化后通过 start/inspect/cancel 执行契约驱动它。V-Router 对批准的同步本地 upstream 申请短租约 permit，delegated 路径核验父任务及关联子许可或桥接移交，cloud 路径直接请求供应商。两种预约共享节点预算，未知停止不能自动释放。失败重试/人工删除约束见 README，节点与应用位置见节点资源文档。

### 逐模型、异步及系统余量

VOS 网关必须在私有配置 `models[upstreamID/nativeModelID]` 批准输入上限、资源参数与 executor；未知模型不直接执行。executor 的 `upstream_urls` 把资源预约绑定实际服务地址。`GET /permits` 提供 app 隔离的模型许可历史。

异步创建先取得桥接许可，只有 `/permits/{id}/delegate` 核验同节点、批准的业务 executor 与 app_task_id 后才能移交；否则网关持久化 job 并轮询终态。已运行任务的嵌套调用通过 `/delegations/verify` 核验用户、节点、地址与父预算，使用幂等键申请关联子许可。子调用未停止时父任务继续占用资源，也不能重试。WebSocket 仅批准执行端在停止后发送正常关闭码 1000 的契约。

`resident_baseline` 是常驻预算；`safety_reserve` 可增加系统余量。默认至少保留 2 GiB 或系统 RAM 的 10%、一个 CPU 核（单核半核）、独立显存的 256 MiB 或 5%。观测与所有未释放预约保守共同约束准入；CPU 占用达到 95% 暂停新计算。统一内存按 RAM 与 GPU 需求之和校验一次。Controller/Agent/网关容器自身也有内存与 CPU 上限。预算需实测峰值校准，外部未纳管进程与执行端越界仍需要硬限制。

## 负载统计与许可历史

`GET /statistics` 使用同一服务认证，普通服务只读取自身 app 的固定 24 小时样本；管理员可读取全部。`first_queue_wait` 是窗口内提交任务的首次分配等待，重试不增加该样本；`attempts.occupancy` 是窗口内已确认停止的每次执行从分配至停止的占用时长。`attempts.failure_rate` 为失败 /（成功 + 失败），取消不计入。软删除不会移除审计样本。

`permits.occupancy` 只使用窗口内真实记录 `released_at` 的许可；旧记录时间未知时不补造。释放表示停止或移交，不提供推理成功率。`P50` 无样本和 `P95` 少于 20 个样本时为 null，不构成 SLO 承诺。

`GET /permits` 可用 `state=active|suspect|released` 过滤并复用 `cursor` / `limit` 分页；权限隔离先于筛选和分页。`released_at` 为首次确认停止或移交时间，续租、重复释放、重复移交都不能延长它，旧记录没有时间时为 null。所有功能使用 PostgreSQL 内置能力，不要求安装搜索或向量扩展。

## 同步执行端许可核验

`POST /permits/{id}/verify` 不新增预约。网关申请许可时可提供 32–128 字符的随机
`execution_token`，调度器只保存哈希。执行端使用自己的 submitter 身份；管理员须在
该 principal 的 `permit_executors` 中明确批准允许核验的网关执行器，不能借此查询、
续租或释放网关的其他许可。

请求包含 `executor_id`、`upstream_url`、原生 `model_id`、`execution_token`、
执行端生成的稳定 `execution_id` 和 `claim`。首次 `claim: true` 原子认领，重复启动
返回 409；运行中的 `claim: false` 仅核验同一执行的有效租约，返回剩余
`lease_seconds`。错误模型、节点、地址、凭据、执行 ID、已释放或过期许可均拒绝。
已认领的许可不能再移交异步任务。凭据只能服务间传输，不进入浏览器、日志或结果。

执行端必须阻止重复执行并落实租约失效后的停止或未知隔离。核验成功不证明执行完成，
原许可所有者仍负责续租和确认停止后的释放。

## 单任务状态时间线

`GET /tasks/{id}/events` 复用任务可见性与服务认证。查询参数 `limit` 为 1–200，默认 50；`cursor=0` 或省略时返回最新事件，传入 `next_cursor` 时读取严格早于该游标的下一页。记录含 `cursor`、`task_id`、`status`、`version`、保存的 `progress` 和实际 `created_at`，按 cursor 降序排列；`next_cursor=null` 表示没有更早页。普通服务不能查询其他 app 的任务，返回 404。旧事件没有进度快照时仍为空对象。此接口用于详情时间线，不改变全局 `/events` 的正向增量协议。

## 多节点在线许可与管理 API（0.0.20）

`GET /management` 供管理员读取待审批节点、执行器、邀请和凭据元数据、近期操作记录；不会返回存储的令牌或哈希。`POST /enrollments` 创建有期限的单次邀请，登记使用邀请 Bearer；worker 无管理员权限。节点/执行器审批与预算更新使用版本号，资源未确认停止时拒绝修改或撤销。服务身份仍按 app_id 隔离。原有静态身份和 V-Voice 专用自动绑定保持兼容。

`model.infer` 执行器的 `node_id/resources` 是主执行节点，`placements` 是其余计算节点与各自固定预算（总计最多 32 个节点，不重复主节点）。普通 `POST /permits` 无需由客户端提交预算；控制器按管理员模板原子检查并预约全部节点。许可历史附带 placements，资源台账逐节点列出。任意参与节点的未知停止保留整项预约，不能仅释放 head 节点。

此能力不提供模型分片或多节点异步 task：含 placements 的执行器仅可为 model.infer，多节点许可不能 `/delegate` 给单节点业务任务，也不能用作单节点父任务的关联子许可。正在运行的 ModelInfer 部署不被自动重启或接管；来源清单只用于观察与管理员核验。见[完整流程与维护边界](node-management.md)。

### VOS App 管理约束

最终用户只通过 VOS 安装和管理应用，不手工部署外部 Agent。登记协议是应用内部组件接口，不能据此要求用户到计算节点执行命令。ModelInfer 的发现与监控复用现有 VOS App；`POST /node-sources/modelinfer/{id}` 只保存规划，`POST /node-sources/modelinfer/{id}/control` 当前返回 501 `VOS_NODE_CONTROL_PENDING_VERIFICATION`。规划节点不会创建 worker、心跳或预约能力。真实远端调度须完成管理适配器的执行/停止/故障恢复验证后启用。
