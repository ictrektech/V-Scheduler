# V-Scheduler 网关资源准入

本页说明统一 V-Scheduler 应用中 V-Router 网关组件的准入配置。VOS 只安装 `com.ictrek.v-scheduler`，内部保留网关与调度控制器两个进程，不再另装 V-Router。统一包设置 `V_ROUTER_SCHEDULER_REQUIRED=true`。所有经网关启动的本地及纳管远端模型计算先取得调度授权；云端供应商模型绕过本地调度。提示词和结果仍由网关直接转发。

模型客户端仍调用 `http://v-router:8080/v1/*`，或同域 `/api/com.ictrek.v-scheduler/v1/*`；调度公开 API 使用 `/scheduler/api/v1/*`。地址与原位数据迁移见[统一应用](unified-application.md)，应用任务接入见[应用接入指南](application-integration.md)。

## 管理员配置

在私有数据目录创建 `scheduler.local.json`（容器内 `/data/scheduler.local.json`），权限 `0600`；也可用 `V_ROUTER_SCHEDULER_CONFIG` 指定路径。使用调度控制器 URL（同一包内直连 `http://v-scheduler:38120/api/v1`；经网关则使用 `http://v-router:8080/scheduler/api/v1`）和受限 `v-router` submitter 令牌。修改后重启网关。安装依赖不会自动注入凭据或资源模板。

参照 [网关配置示例](../config/scheduler.example.json)，在 `models` 中按 `upstreamID/nativeModelID` 逐模型批准 `executor_id`、`max_input_bytes`（最高 32 MiB）、`limits`。VOS 强制逐模型策略：未批准模型返回 503，超输入预算返回 413，未批准或超限的资源参数返回 422。limits 包括输出 token、上下文、批量、图像尺寸、视频长度等实际支持字段；带 `speaker_model` 时还须在 `secondary_models` 明确批准联合预算。模板须覆盖服务默认参数、装载峰值和允许的最大输入，示例值不是生产容量承诺。

V-Scheduler 的 executor 指定真实节点、固定 CPU/RAM/GPU 增量预算及 `upstream_urls`；地址必须与网关上游 base URL 相符。常驻模型记入节点 `resident_baseline`，不逐请求重复记账。未知节点、过期遥测、排空或资源不足时拒绝；网关不会绕过拒绝直跑。VOS 本机 Agent 随控制器内置运行，不需另装采集服务；外部 worker 仍须纳管。自动发现不自动批准新模型。

| 资源/协议 | 准入与完成证据 |
|---|---|
| 外部云端供应商 | `kind=cloud` 直接转发；私网地址被标成 cloud 仍按纳管资源处理，除非管理员批准了绑定 base URL 的外部独立资源声明。改变地址使批准失效。 |
| 本机或其他纳管节点的 HTTP/JSON/SSE | `mode=permit`、`completion=synchronous`；转发前申请，执行中续租，完整终态响应后释放。 |
| WebSocket 实时模型 | 同上，另须 `websocket_completion=server_close`；执行端必须在计算已停止后发送正常关闭码 1000。客户端断开和错误关闭不算停止证据。 |
| Image Studio、ML Server、通用 async_task、原生 background Responses/batches/fine-tuning/videos | 创建前持有 permit；创建凭据写入私有 `scheduler-jobs` 日志，后台轮询到执行端终态后释放；重启只恢复轮询，不重放创建。 |
| 原应用已提交 Scheduler 任务的异步创建 | `mode=delegated` 仍先持桥接 permit。取得业务 job 与 Scheduler task 后，通过 `/permits/{id}/delegate` 验证批准执行器、业务 ID 和真实节点，原子移交，避免重复占用。 |
| 已运行任务中的嵌套模型调用 | 传 `X-VScheduler-Task-ID`；网关从认证身份取得 owner，通过 `/delegations/verify` 核验运行任务、用户、节点和剩余预算，申请关联子许可；父任务资源不重复扣除。 |

查询、取消、输入上传等不启动模型计算的请求不新建 permit。泛用异步服务须支持现有适配器的状态协议；不能把任意 202 当作已停止，也不能仅设置 delegated 就跳过准入。不符合契约的服务应先接入与验证。

## 中断、重启与资源保护

### 同步执行端交接

网关在申请普通或关联许可时生成一次性 `execution_token`，仅经服务间请求头
`X-VScheduler-Permit-ID`、`X-VScheduler-Executor-ID`、`X-VScheduler-Execution-Token`
交给执行端。所有客户端提供的同名头均在准入前移除，云端及未取得许可的请求不携带它们。
这些头不是浏览器凭据，也不应出现在日志、结果或 API 示例的真实值中。

V-Voice 等已适配执行端以自己的受限 submitter 调用 Scheduler
`POST /permits/{id}/verify`，核验模型、真实地址、节点及许可，并原子认领执行。
管理员须明确授予该身份对应 `permit_executors`；核验不再次预约，也不转移网关的续租和释放职责。
首次认领后不能再启动第二个执行或移交异步任务。直接调用 V-Voice 的请求由 V-Voice
自行申请其批准的许可，与网关转发共享同一核验及停止边界。需要 Scheduler 0.0.15 或更新版本。

客户端断开、传输失败、续租失败、创建结果未知、无法解析任务状态时，许可保留为 suspect。网络关闭不能证明服务停止；管理员核实后可调用 `/permits/{id}/confirm-stopped`。关联子调用未停止时，父任务即便报告终态也保留预约，且不能开始下一次重试。

异步日志目录权限 `0700`，文件 `0600`，保存轮询所需调用者身份，不保存模型输入；它属于敏感应用数据。ML Server 的终态查询会消费结果，网关缓存到原调用者读取，其他身份返回 404。云端请求不使用此本地日志。

网关默认最多 8 个活跃模型/资源请求、64 MiB 输入缓冲预算，超出返回 429；VOS 容器限制 768 MiB/2 CPU，Go 内存目标 576 MiB。调度器保留系统 RAM、CPU 和显存余量，按观测与全部未释放预算保守准入，统一内存计一次。资源估计必须经过真实模型峰值校准；调度不能替代执行端硬限制，也不能约束绕过网关的外部进程。

独立开发进程未开启强制开关时保留旧兼容行为，不能据此宣称生产请求已受调度。每个节点上线前必须验证本机/远端真实模型成功、资源忙拒绝、云端绕过、失联保留和重启恢复。

测试覆盖原生 HTTP、Responses/Messages 转换、WebSocket 双向帧、异步终态/用户隔离/恢复、资源参数和云端边界；Scheduler PostgreSQL 验证覆盖关联许可、预算和接力。见[验证记录](validation.md)。

内置 Ollama 同步请求遵循相同的响应规则：准入忙立即返回 429，缺少逐模型私有绑定立即返回 503。客户端可稍后重试；网关不在 HTTP 请求中隐藏等待或重复申请许可。

自带 Ollama 的 `suspect` 许可由控制器周期性核验。仅处理本应用、当前本机、自动执行器及固定三类后端的许可；存在活动许可时跳过。原生后端必须返回匹配模型与本次 nonce 的停止回执，确认加载循环和进程退出屏障后，才记录 `builtin_ollama_native_stop_confirmed` 并释放。旧版后端不支持回执、网络异常、过期回执或真正未退出时继续保留预约。其他应用和远端执行器不走此恢复路径。因待停止核验的预约无法准入时，API 返回 HTTP 429 / `WAIT_STOP_CONFIRMATION`。
