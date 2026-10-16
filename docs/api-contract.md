# 统一配置、身份与错误契约

Go 网关和 Rust 服务继续分别运行，使用同一份[错误契约](../api/error-contract.json)做跨语言回归。完整接入操作见[统一接入指南](application-integration.md)。

## 地址与配置

| 配置 | 含义与优先级 |
|---|---|
| `SCHEDULER_VOS_URL` | 网关与控制器共用的 VOS 根地址；只接受不带凭据、查询参数或子路径的 HTTP(S) origin。空值沿用 VOS 注入的 `VOS_HOST_GW_IP:8105`。 |
| `VOS_OIDC_USERINFO_URL` | 保留的网关 userinfo 精确地址覆盖；未设置时从上述共同根地址解析。控制器仍使用同一 VOS 的 `/v1000/user/check` 核验管理员。 |
| `V_ROUTER_CONTROLLER_URL` | 网关到控制器的内部根地址，安装默认 `http://v-scheduler:38120`。 |
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

Go 与 Rust 接受相同的 Bearer 语法，拒绝错误 scheme、多余字段和空令牌。前端每次请求读取当前 VOS token；401 时共享既有凭据刷新机制，最多重试一次；403 不刷新、不重放操作。资源不足不触发令牌刷新。取消请求继续使用原 AbortSignal。

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

- Go `go test ./...` 与 Rust `cargo test --locked` 共同读取 `api/error-contract.json`；网关镜像构建上下文包含该文件。
- 前端 `npm test` 覆盖三种服务路径、三种旧错误形状、令牌轮换、一次 401 重试及 403 不重放。
- `scripts/e2e-api-contract.py` 启动实际 Go/Rust 服务、独立 PostgreSQL 与合成 VOS/模型端点，验证身份矩阵、代理、畸形 JSON、故障隔离、三种对话协议及 SSE。该脚本不调用真实模型或已安装应用。
- `scripts/e2e-model-hub.py` 验证原模型接管、选定文件下载、追加/更新/重试、导入、服务管理及原文件保留；可用 `--binary` 运行构建主机上的候选二进制，或保留原容器运行方式。
