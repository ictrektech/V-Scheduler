# V-Scheduler 公开 API 与接入标准 v1

日期：2026-10-13。状态：完整目标协议与 MVP 实现范围。适用于所有接入方，不以是否位于 VAI `apps/` 或是否运行 VOS 为前提。[设计](design.md)规定资源与恢复不变量；[OpenAPI](../api/openapi-v1.json)描述当前已实现的 v1 HTTP 路由。三者变更必须一起维护。

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

- 核心 base URL：`https://<controller>/api/v1`。
- VOS 包挂载：`https://<vos-host>/app/com.ictrek.v-scheduler/api/v1`；内网可经已有 VOS 网关访问同一路径。
- `GET /discovery` 返回协议版本、实际开启的能力、认证模式和本实例 ID；不返回节点地址、任务、密钥或容量细节。
- HTTP 协议主版本在路径；`protocol_version=1` 标识消息主版本；执行器另有 `contract_version`，模型 ID/版本另行匹配。
- v1 只增加可选响应字段；客户端忽略未知可选字段，遇到未知状态禁止将其当成成功。新增必需字段、删字段或改变单位需 v2。
- 本文定义的所有必需端点只有完成实现和 conformance 后才在 discovery 声明；permits、webhook、SDK 不作为第一版已支持能力。

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

所有路径以下以 `/api/v1` 为基准：

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

任务至少包含 `protocol_version`、`app_task_id`、`executor`、`parameters`、输入 artifact 列表；resource、timeout、retry 的可选覆盖必须在批准模板范围内。客户端不能任意申报零资源、优先级或任意镜像。完整示例见 [设计第 5 节](design.md#5-任务协议-v1拟新增)。

幂等提交示意（所有域名/令牌为占位）：

```http
POST /api/v1/tasks HTTP/1.1
Host: scheduler.example.invalid
Authorization: Bearer <service-access-token>
Idempotency-Key: business-job-001
Content-Type: application/json

{"protocol_version":1,"app_task_id":"business-job-001","executor":{"id":"example.video.generate","contract_version":"1","task_type":"video.generate"},"parameters":{"duration_seconds":5},"inputs":[]}
```

完整流程：批准服务身份 → 发现执行器 → 登记输入 → 原应用保存 job/outbox → 提交 → 持久化返回 task ID → 查询/拉事件 → 下载已发布结果或请求取消。应用业务结果仍由应用自己授权交付。

## 6. worker 公开接入标准

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

以 OpenAPI 与 discovery 声明为准。controller 当前使用受保护配置中的 admin/submitter/worker 凭据，submitter 按 app_id 隔离，worker 绑定 node_id；没有动态注册、OIDC 管理登录或 artifact 上传 API。任务只引用预先批准的 executor，资源预算来自其模板，inputs 必须为空；应用保留自己授权的任务与文件。原协议中 workspace、artifact、优先级、动态能力注册等属于后续目标，不能发送给当前严格的提交结构。

Image Studio 用业务任务 ID 作为幂等键，保留原业务入口与输出。Rust Agent 将 command 持久化后通过 start/inspect/cancel 执行契约驱动它。V-Router 对批准的同步本地 upstream 申请短租约 permit，delegated 路径不重复申请，cloud 路径直接请求供应商。两种预约共享节点预算，未知停止不能自动释放。失败重试/人工删除约束见 README，节点与应用位置见节点资源文档。
