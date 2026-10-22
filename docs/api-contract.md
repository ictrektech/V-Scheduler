# 统一配置、身份与错误契约

Router、Scheduler 与模型仓库在同一 Rust 进程运行，使用同一份[错误契约](../api/error-contract.json)回归。完整接入操作见[统一接入指南](application-integration.md)。

## 地址与配置

| 配置 | 含义与优先级 |
|---|---|
| `SCHEDULER_VOS_URL` | 网关与控制器共用的 VOS 根地址；只接受不带凭据、查询参数或子路径的 HTTP(S) origin。空值沿用 VOS 注入的 `VOS_HOST_GW_IP:8105`。 |
| `VOS_OIDC_USERINFO_URL` | 保留的网关 userinfo 精确地址覆盖；未设置时从上述共同根地址解析。控制器仍使用同一 VOS 的 `/v1000/user/check` 核验管理员。 |
| `V_ROUTER_CONTROLLER_URL` | 兼容保留的控制器地址；统一进程使用内存分发，安装地址仍为 `http://v-scheduler:38120`。 |
| `V_SCHEDULER_MODEL_MANAGEMENT_URL` | 网关到模型仓库的内部根地址，安装默认 `http://v-scheduler:6005`。配置无效或服务离线返回明确的 JSON 503。 |

前端统一解析三种服务路径：网关 `/api/v1/*`、控制器 `/scheduler/api/v1/*`、模型仓库 `/model-hub/api/v1/*`。VOS 页面全部使用 `/api/com.ictrek.v-scheduler` 前缀；宿主端口页面使用同域根路径。任务与下载任务保持独立命名空间。九组内外端口及全部兼容别名见 [README](../README.md#host-alias内部端口与宿主机端口)。

## 身份边界

| 身份 | 可使用的功能 |
|---|---|
| VOS 普通账号 | 按自身权限访问网关；通过统一入口访问模型仓库。不会因此获得控制器管理员权限。 |
| VOS 管理员 | 网关账号能力及控制器管理能力。网关 userinfo 与控制器管理员检查保留各自职责。 |
| 网关管理员 Key | 网关管理及统一模型仓库入口；不能替代控制器的服务/管理员身份。 |
| Scheduler submitter / worker | 对应应用和执行器的任务、许可或执行协议；不能替代网关账号 Key。 |
| 应用专用后台推理 Key | 仅批准的推理能力；不能访问网关或统一模型仓库管理接口。 |
| 上游模型 API Key | 仅由网关访问对应模型服务；不作为前端用户凭据。 |

统一模型仓库代理复用网关已有账号鉴权。原生模型仓库端口保持原 Model Hub 的受信任内网契约，供已有容器注册、发现和管理；三个 Ollama 原生管理入口也保持原契约。此次没有将管理 Key、submitter token、上游 Key 合为一把密钥，也不改变原生接口访问策略。

推理等待队列同样按网关用户/独立应用身份隔离，匿名内网调用可保留立即准入，但不能加入或管理等待队列；网关管理员可查看所有等待项。驻留策略和完整模型接入状态仅允许网关管理员。内置服务卡片预热复用同一资源准入，不新增用户身份权限。

所有入口接受相同的 Bearer 语法，拒绝错误 scheme、多余字段和空令牌。前端每次请求读取当前 VOS token；401 时共享既有凭据刷新机制，最多重试一次；403 不刷新、不重放操作。资源不足不触发令牌刷新。取消请求继续使用原 AbortSignal。

## 错误响应

应用自己生成的 API 错误包含：

```json
{
  "code": "WAIT_RESOURCE",
  "category": "v-scheduler",
  "data": {},
  "msg": "local resources busy; retry later",
  "version": "1",
  "request_id": "example-trace-id",
  "error": {"code": "WAIT_RESOURCE", "message": "local resources busy; retry later"}
}
```

`code` 是机器判断依据，`msg` 为可变说明；`request_id` 与 `X-Request-ID` 相同。401 同时带 `WWW-Authenticate: Bearer`。模型仓库继续提供原 `detail`；网关继续提供 `error.message`；Messages 协议继续提供 `type: error`，兼容已有客户端。原有成功响应、OpenAI/Responses/Messages 成功协议和上游流式数据不套新信封。

| HTTP | 通用 code | 客户端处理 |
|---|---|---|
| 400 / 422 | `INVALID_REQUEST` | 修正参数或 JSON，不重复同一错误请求。 |
| 401 | `UNAUTHENTICATED` | 修复该接口要求的身份；浏览器最多刷新一次。 |
| 403 | `FORBIDDEN` | 身份有效但权限不足；不会通过刷新身份提升权限。 |
| 404 / 405 | `NOT_FOUND` / `METHOD_NOT_ALLOWED` | 检查命名空间、资源和方法。 |
| 409 | `STATE_CONFLICT` | 读取最新状态或版本后处理。 |
| 413 | `REQUEST_TOO_LARGE` | 减少输入。 |
| 429 | `WAIT_RESOURCE` | 保留既有资源规则，稍后重试。 |
| 502 | `UPSTREAM_UNAVAILABLE` | 检查目标服务。 |
| 503 / 504 | `TEMPORARILY_UNAVAILABLE` | 检查依赖及配置，不绕过调度执行。 |

控制器的具体业务码，如 `WAIT_STOP_CONFIRMATION`、`EXECUTOR_NOT_ALLOWED`、`UPSTREAM_NOT_APPROVED`，经网关仍保留。网关代替用户申请许可时，控制器拒绝的是网关服务身份，因此此类 401/403 对推理调用者仍返回 503，同时保留控制器业务码和请求编号；不会误导浏览器去刷新用户身份。控制器返回的任意内部说明或凭据不会被直接透传。上游自身返回的推理协议错误保持其原始协议。

## 验证入口

- Rust `cargo test --locked` 验证统一错误、旧状态兼容、协议转换、SSE、WebSocket 与许可生命周期。
- 前端 `npm test` 覆盖三种服务路径、三种旧错误形状、令牌轮换、一次 401 重试及 403 不重放。
- `scripts/e2e-api-contract.py` 启动实际统一 Rust 服务、独立 PostgreSQL 与合成 VOS/模型端点，验证身份矩阵、代理、畸形 JSON、故障隔离、三种对话协议及 SSE。该脚本不调用真实模型或已安装应用。
- `scripts/e2e-model-hub.py` 验证原模型接管、选定文件下载、追加/更新/重试、导入、服务管理及原文件保留；可用 `--binary` 运行构建主机上的候选二进制，或保留原容器运行方式。


## 本机内存余量与连续调用

HTTP 429 的 `WAIT_MEMORY_HEADROOM` 表示观测可用 RAM 小于本次预算、未释放预约与系统安全余量之和，区别于批准容量不足、节点不可用或普通并发占满。可结合节点 API 的预算、常驻基线、有效安全余量和资源采样核对；总内存较大或 CPU 空闲不代表本次模型已具备足够 RAM。应卸载空闲模型或显式选择已校准的低内存执行配置，不能清空预约或下调安全余量来放行。

本机内置执行组件在准入时重新读取宿主可用 RAM，避免上一笔推理的采样窗口造成卸载后仍被旧低余量拒绝；不会刷新 CPU、GPU 或整个遥测的采样时间，也不把远端节点的未知资源视为本机空闲。

`WAIT_STOP_CONFIRMATION` 仍要求原生停止证明。统一推理网关对尚未获得许可的新请求最多等待八秒，间隔重试准入；只在 Controller 确认许可后才发送一次原生推理。普通容量不足不会因此长时间排队，无法确认停止时仍返回 429。重试不会释放其他任务或绕过停止校验。客户端断连不能代替执行端停止确认；是否已经取得许可、启动原生计算，须按实际执行状态核验。


完成同步 JSON、音频/文本流 EOF、SSE 终帧和服务端 WebSocket 关闭时，网关先等待该许可的释放调用，再向调用方交付完成边界，避免上一笔结果已返回但异步释放尚未写入 Controller，导致紧接着的新请求被 `WAIT_RESOURCE` 拒绝。释放接口不可用时保留原有失败与对账语义；截断、客户端中断或原生停止未知仍不会被当成已停止。

内置 Ollama 的人工校准策略会补齐并验证批准的 `options.num_ctx`、`options.num_thread`、`options.num_gpu`、`options.num_batch` 和输出 token 上限；超界请求在申请许可前拒绝。输入字节超限的 413 错误同时给出实际字节数和已批准上限。自动估算模型与其他节点的预算不因此降低。
人工校准的上下文、线程数和 batch 必须为正值；`0` 不得启用未实测的原生自动设置。CPU 执行的 `num_gpu=0` 保持有效。

## 启动与准入诊断

`GET http://v-scheduler:8080/healthz` 只报告进程存活。`GET /readyz` 免鉴权，报告内置模型目录的 `initializing/ready/unavailable/disabled`；未就绪返回 503 与 `Retry-After: 1`，不加载或预热模型。管理状态 `/api/v1/status` 同时提供 `catalog_readiness`，界面自动更新提示；已有健康云模型不会被首次内置发现阻塞。

首次发现尚未结束且没有可见模型时，`/v1/models` 返回 `503 MODEL_CATALOG_INITIALIZING`；已探测但不可用的内置后端返回 `503 UPSTREAM_UNAVAILABLE`，健康后端真正缺少的模型仍返回 404。`SCHEDULER_ADMISSION_TIMEOUT` 表示内存分发的控制器在三秒内未完成准入或正文读取，不等于资源不足，也不表示推理已经发送。容量不足继续使用 429 和具体 WAIT 码。客户端仅对可重试的发现操作重试，不自动重放推理创建。


## 有界排队与驻留契约

请求头、取消结果、等待优先级及驻留管理见[统一指南](application-integration.md#inference-queue-residency)。`QUEUE_TIMEOUT`、`QUEUE_CANCELLED`、`QUEUE_FULL` 和 `QUEUE_ID_IN_USE` 在网关保持稳定机器码。队列仅管理尚未执行的请求，不能把客户端断开等同于原生计算停止。队列 ID 不代替业务幂等键；进程重启不会重放内存中的请求。

驻留设置是网关数据目录下的私有 `residency.json`，不写入共享模型注册表。驻留预约可跨多次同模型调用保留，首次释放时间仍按最终卸载记一次；每次请求用量仍独立记录。保留完整预算的空闲模型也会表现为活动许可，不能将活动许可数量直接当作生成请求数。

控制器内部 `/inference-waiters` 只保存短期调度提示，五秒失效，不保存推理正文、令牌或可重放任务；`/permits/inspect` 不创建预约，校验当前身份、批准及节点；`/permits/{id}/reuse-resident` 仅允许原所属内置 Ollama，在原预约有效且身份、执行器、节点、预算和旧执行证明匹配时轮换证明。外部已认领执行不具备驻留复用豁免。

兼容停用：将驻留秒数设为 0 并等待原生卸载确认，客户端省略排队头或设置等待为 0，即可恢复之前的逐次卸载和立即准入行为，无需删除权重或恢复旧数据库。本轮数据库新增一张短期等待提示表；不支持用旧二进制直接覆盖新数据库的迁移版本，也不应为回退恢复陈旧业务账本。

内置 Ollama 权重更新和删除会先核验驻留卸载及未释放预约；有排队、执行或下载任务时返回 409，请在任务结束后重试。自动资源绑定按当前原生模型大小核对；未完成绑定返回 `503 MODEL_BINDING_PENDING`，不会沿用旧权重的预算。旧 Model Hub 或直接操作原生管理端口的写入方，应先停止该模型并确认卸载，再变更共享权重；进程内互斥不覆盖外部写入方。

开启内置 Ollama 驻留后，已验证身份未提供排队头的同步请求默认等待最多 30 秒；显式 `X-VScheduler-Queue-Wait-Seconds: 0` 保持立即准入。匿名及其他后端请求保持原行为，所有等待仍核验真实资源与停止屏障。

## 模型预算校准与单模型放行

管理员可在“模型”页查看每个内置 Ollama 模型和每个应用模型当前使用的预算，并选择人工校准、直接放行或恢复默认；预算接口为 `GET /api/v1/model-budgets`、`POST /api/v1/model-budgets/calibrate` 和 `POST /api/v1/model-budgets/restore`，单模型放行为 `PUT /api/v1/model-routing`，四项均要求管理员身份。

`/model-budgets` 为每个已纳管的 `v-scheduler/ollama-<role>/<model>` 返回一条记录：`mode` 为 `automatic`（按模型大小估算）、`calibrated`（人工预算）或 `direct`（不申请调度许可），并包含当前 `resources`（direct 为 null）、当前节点自动估算的 `default_resources`、`limits` 与 `default_limits`、节点 `capacity` 及执行器状态。界面展示不作为准入依据，实际准入仍读取已保存的执行器配置和绑定。

`/model-budgets/calibrate` 接受 `model_id`、可选 `mode`、`resources` 和 `limits`。省略 `mode` 等同 `calibrated`：`resources` 必须给出正的 CPU 毫核和内存 MiB；显存大于 0 时必须同时给出 GPU 槽位，并且整体扣除常驻基线后不能超过当前节点容量。`limits` 必须恰好包含 `max_tokens`、`options.num_ctx`、`options.num_thread`、`options.num_gpu`、`options.num_batch`、`options.num_predict` 六项；省略时使用批准默认值（4096、8192、4、999、512、4096）。保存后按指纹创建或复用校准执行器，并停用上一份校准执行器；超界请求仍在申请许可前拒绝。

`mode=direct` 表示不做资源限制、不主动调度直接放行：请求中不得同时给出 `resources` 或 `limits`。绑定会移除执行器与运行上限，网关核对上游地址后直接转发，不申请许可、不排队、不做资源准入；该模型的请求不再占用调度预算。管理员可随时重新校准或恢复默认预算。

`/model-budgets/restore` 仅需 `model_id`，重新按当前原生模型大小和节点预算计算自动估算并启用自动执行器，同时移除人工运行上限。自动模式本身会直接返回未校准结果。

应用托管的模型（Image Studio、V-Voice、ML Server、私有 V-Sub 等）使用同一组接口校准：额外传入 `upstream_url`（网关转发该上游时使用的 base URL），`model_id` 使用原生模型名。校准结果写入 `model-budgets.local.json`，按上游 URL 与原生模型名隔离；申请许可时临时以该资源替换执行器的准入占用，不改动应用自己的执行器或配置，也不支持运行上限。该路径的 `mode` 只能是 `calibrated`（或省略），`mode=direct` 返回 `422 MODEL_BUDGET_DIRECT_VIA_GATEWAY`；恢复时同样传入 `upstream_url`。`/model-budgets` 会在内置 Ollama 记录后追加这些覆盖记录，并带上 `upstream_url` 与 `native_model`。

单模型直接放行由网关管理，不走控制器：`PUT /api/v1/model-routing` 接受 `{"model_id":"<upstream_id>/<原生模型名>","direct":true|false}`，仅管理员可调用。`direct=true` 写入网关状态文件，`model_view` 返回 `direct: true`；该模型的请求在读取任何调度策略前直接转发，不申请许可、不排队、不计预算。`direct=false` 删除记录并恢复上游策略；上游整体直接放行仍然优先。

非法型号、字段或超界预算返回 `422 INVALID_MODEL_BUDGET`；已存在的非内置绑定返回 `409 MODEL_BINDING_UNMANAGED`；未发现绑定返回 `404 MODEL_BINDING_NOT_FOUND`；节点采样或预算存储不可用返回 `503 RESOURCE_SAMPLE_NOT_READY` 或 `503 MODEL_BUDGET_STORAGE_UNAVAILABLE`。校准、直接放行和恢复分别写入 `model.budget.calibrated`、`model.budget.direct`、`model.budget.restored` 审计记录。

## 应用统一生命周期协议

调度命名空间新增 `GET/POST /integrations`、`POST /integrations/heartbeat`、`POST /permits/{id}/stop` 和 `POST /permits/{id}/stopped`。服务身份按 app 隔离，登记仅引用已批准的执行器；应用心跳不能上报或提高节点预算。停止请求保持预约，回执核验原执行身份及退出/卸载证据后才释放。既有自动绑定保持兼容。完整字段、时序与错误见[统一流程](application-integration.md#unified-lifecycle)及 OpenAPI。
