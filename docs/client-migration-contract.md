# 客户端迁移协同契约

本记录用于 V-Scheduler 与七个客户端迁移会话共同核对，基线为 2026-10-16 的 `0.0.24`。它区分已有接口和待完成项；不能以依赖名称替换、服务健康或模型可见代替真实任务验收。

## 固定服务入口

| 功能 | Docker 网络入口 | 默认宿主机入口 |
|---|---|---|
| 统一推理 | `http://v-router:8080/v1` | `http://<vos-host>:22888/v1` |
| 调度 | `http://v-scheduler:38120/api/v1` | `http://<vos-host>:38120/api/v1` |
| 调度经网关代理 | `http://v-router:8080/scheduler/api/v1` | `http://<vos-host>:22888/scheduler/api/v1` |
| 模型仓库 | `http://v-scheduler-model-hub:6005/api/v1` | `http://<vos-host>:36005/api/v1` |

完整九项端口、兼容别名和安装参数见 [README](../README.md#host-alias内部端口与宿主机端口)。宿主机端口允许安装时修改，客户端容器应使用稳定的内部别名。

客户端依赖统一应用 `com.ictrek.v-scheduler`；打包时核验最新已发布 Release / App Store 版本。HybRAG 按本次迁移要求同时兼容旧 Model Hub/V-Router 与 V-Scheduler，不强制卸载旧依赖。其他客户端默认采用统一入口。

## 模型和身份

`MODEL_HUB_SHARED_MODELS_PATH` 继续选择原 Model Hub 的公共目录，目录类型提示保留 `[com.ictrek.model.storage]`。模型管理服务及三个 Ollama 在 `/data` 挂载此目录；客户端可以只读挂载到自己的 `/modelhub`，使用 `export/hf/.../current`、`export/ms/.../current`。容器内绝对路径不跨容器直接复用；必须按各自挂载点解析。注册表、别名、引用和选中文件范围原位保留。详见 [模型管理](model-management.md)。

业务用户身份、输出文件权限仍由客户端所属应用管理。调度调用使用该应用独立的服务身份，不能复用浏览器 Bearer、网关令牌或其他应用令牌。服务凭据只写入私有文件，不进入 discovery、浏览器、日志或共享模型目录。

网关与调度身份是两套契约。当前网关接受有效 VOS OIDC 用户令牌或管理员配置的 `V_ROUTER_API_KEY`；调度 submitter 令牌不用于网关鉴权。仅在 `V_ROUTER_REQUIRE_KEY=false` 且未配置网关 API Key 时，无凭据的推理请求以 `internal` 身份进入；它仍须通过本地计算的调度准入，且不能访问管理接口。只要请求携带无效 Bearer 就会返回 401，不能用任意占位 key。配置了非空网关 API Key 也会强制鉴权，即使 Require Key 开关关闭。后台客户端应兼容安装者开启鉴权的情况，不依赖匿名默认值；逐应用独立网关服务凭据尚未实现，不能宣称具备该隔离能力。

## 同一次计算的调度责任

| 实际计算 | 必须实现的接法 |
|---|---|
| 客户端调用统一网关的自带 Ollama | 网关申请并持有许可；客户端不重复预约。模型 ID 使用目录返回的 `v-scheduler/ollama-<role>/<native-model>`。 |
| 应用直接执行自己的同步本地模型 | 应用使用自己的固定执行器申请许可、续租，并在确认停止后释放。 |
| 网关调用应用的同步本地模型 | 应用核验并认领网关同一次许可，不再独立预约。 |
| 应用自有异步本地任务 | 持久化业务任务和 outbox，以稳定幂等键提交 `/tasks`，执行器实现 `start/inspect/cancel` 与停止证据。 |
| 真正外部云端计算 | 不预约本机 GPU；仍保留业务鉴权。远端 URL 本身不能证明是云端。 |

同步网关会清除调用者伪造的许可头，再向执行端传入 `X-VScheduler-Permit-ID`、`X-VScheduler-Execution-Token`、`X-VScheduler-Executor-ID`。执行端以自己的服务身份调用 `POST /permits/{id}/verify`，首次 `claim: true`，后续相同 `execution_id` 使用 `claim: false`。核验字段、权限与租约规则见 [接入标准](integration-standard.md#同步执行端许可核验)。

客户端断开不等于计算停止；停止未知时保留预约。异步桥接移交及嵌套调用必须使用现有 delegate / delegation 协议，见 [应用接入](application-integration.md)。

## 服务端现状与待完成项

- `0.0.24` 已有固定预算的执行器、服务身份管理、许可核验、异步任务及网关交接协议。现有管理接口不能解释为允许应用自报预算后立即执行。
- 自带 Ollama 已有专用自动绑定。它不构成其他应用的通用注册或自动授权接口。
- V-Voice 已有专用可信绑定接收流程；内置校准清单当前仅覆盖文档所列 Thor 模型和运行设置，不能外推到 L4T、AMD 或所有模型。见 [V-Voice 主动接入](v-voice-auto-binding.md)。
- V-Music、V-Sub、desensitize 的本地模型准入，以及 Image Studio 的安装后可信绑定，均需与客户端逐项补齐和校准。未校准的模型保持不可执行，不能用默认预算静默放行。
- V-ChatCut、HybRAG 需按实际推理链路区分统一网关调用与本地计算，不能仅凭应用名称决定是否需要执行器。
- 229 默认试聊等待/流输出问题仍在排查。受控小输入调用成功不能代替默认界面请求和客户端真实任务的验收。

## 分工与验收

V-Scheduler 会话负责服务端、可信绑定、统一文档、主仓 README 和旧源码子模块清理；迁移客户端会话负责 V-Voice、V-Sub、V-Music、Image Studio、V-ChatCut、desensitize、HybRAG 的客户端改动及发布。双方只提交各自文件和子模块引用，保留其他未提交改动。

两会话须协调机器操作窗口。目标为 192=`l4t`、229=`thor-spark`、232=`amd`，避免同时升级、重启或运行重模型。每个应用在其支持的 profile 上记录实际模型、任务、输出和调度账本；不支持的组合明确列出，不用其他机器结果代替。

完成标准包含共享目录及原模型保留、真实任务输出、同一次计算只占一份预算、忙碌与过期许可不绕过、取消后的停止确认、重启恢复、全部受影响 profile 构建发布及目标机器安装。当前本记录只完成接入契约对齐，不声明迁移已经完成。
