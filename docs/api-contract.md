# 统一配置、身份与错误契约

Router、Scheduler 与模型仓库在同一 Rust 进程运行，使用同一份[错误契约](../api/error-contract.json)回归。完整接入操作见[统一接入指南](application-integration.md)。

## 地址与配置

| 配置 | 含义与优先级 |
|---|---|
| `SCHEDULER_VOS_URL` | 网关与控制器共用的 VOS 根地址；只接受不带凭据、查询参数或子路径的 HTTP(S) origin。空值沿用 VOS 注入的 `VOS_HOST_GW_IP:8105`。 |
| `VOS_OIDC_USERINFO_URL` | 保留的网关 userinfo 精确地址覆盖；未设置时从上述共同根地址解析。控制器仍使用同一 VOS 的 `/v1000/user/check` 核验管理员。 |
| `V_ROUTER_CONTROLLER_URL` | 兼容保留的控制器地址；统一进程使用内存分发，安装地址仍为 `http://v-scheduler:38120`。 |
| `V_SCHEDULER_MODEL_MANAGEMENT_URL` | 网关到模型仓库的内部根地址，安装默认 `http://v-scheduler-model-hub:6005`。配置无效或服务离线返回明确的 JSON 503。 |

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
