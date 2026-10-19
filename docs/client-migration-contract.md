# 客户端迁移协同契约

> 接入操作统一参见 [V-Scheduler 统一接入指南](application-integration.md)。本页保留迁移协同及历史验收记录。

本记录用于 V-Scheduler 与七个客户端迁移会话共同核对，已安装基线为 2026-10-16 的 `0.0.26`。它区分已有接口和待完成项；不能以依赖名称替换、服务健康或模型可见代替真实任务验收。

## 固定服务入口

| 功能 | Docker 网络入口 | 默认宿主机入口 |
|---|---|---|
| 统一推理 | `http://v-router:8080/v1` | `http://<vos-host>:22888/v1` |
| 调度 | `http://v-scheduler:38120/api/v1` | `http://<vos-host>:38120/api/v1` |
| 调度经网关代理 | `http://v-router:8080/scheduler/api/v1` | `http://<vos-host>:22888/scheduler/api/v1` |
| 模型仓库 | `http://v-scheduler:6005/api/v1` | `http://<vos-host>:36005/api/v1` |

完整九项端口、兼容别名和安装参数见 [README](../README.md#host-alias内部端口与宿主机端口)。宿主机端口允许安装时修改，客户端容器应使用稳定的内部别名。

客户端依赖统一应用 `com.ictrek.v-scheduler`；打包时核验最新已发布 Release / App Store 版本。HybRAG 按本次迁移要求同时兼容旧 Model Hub/V-Router 与 V-Scheduler，不强制卸载旧依赖。其他客户端默认采用统一入口。

## 模型和身份

`MODEL_HUB_SHARED_MODELS_PATH` 继续选择原 Model Hub 的公共目录，目录类型提示保留 `[com.ictrek.model.storage]`。模型管理服务及三个 Ollama 在 `/data` 挂载此目录；客户端可以只读挂载到自己的 `/modelhub`，使用 `export/hf/.../current`、`export/ms/.../current`。容器内绝对路径不跨容器直接复用；必须按各自挂载点解析。注册表、别名、引用和选中文件范围原位保留。详见 [模型管理](model-management.md)。

业务用户身份、输出文件权限仍由客户端所属应用管理。调度调用使用该应用独立的服务身份，不能复用浏览器 Bearer、网关令牌或其他应用令牌。服务凭据只写入私有文件，不进入 discovery、浏览器、日志或共享模型目录。

网关与调度身份是两套契约。当前网关接受有效 VOS OIDC 用户令牌或管理员配置的 `V_ROUTER_API_KEY`；调度 submitter 令牌不用于网关鉴权。仅在 `V_ROUTER_REQUIRE_KEY=false` 且未配置网关 API Key 时，无凭据的推理请求以 `internal` 身份进入；它仍须通过本地计算的调度准入，且不能访问管理接口。只要请求携带无效 Bearer 就会返回 401，不能用任意占位 key。配置了非空网关 API Key 也会强制鉴权，即使 Require Key 开关关闭。后台客户端应兼容安装者开启鉴权的情况，不依赖匿名默认值；源码中的 HybRAG 专用后台网关身份已通过隔离协议验证，使用独立的推理范围凭据，见 [HybRAG 后台身份](hybrag-gateway-identity.md)；尚未随 `0.0.26` 发布，不能将该开发结果视为现有安装能力。

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

- `0.0.26` 已有固定预算执行器、服务身份、许可核验、异步任务和网关交接。试聊已修复 reasoning 流显示及完整 SSE `[DONE]` 后断连引发的停止误判；192、229、232 均通过真实流调用，229 已验证默认模型在界面显示完整回复。
- 自带三个 Ollama 使用专用自动绑定；该能力不允许其他应用自报预算后立即执行。
- 开发中的 Voice 新清单按实际 provider、线程和输入范围限定 L4T / AMD，仍须完成最终客户端真实绑定验收，见 [V-Voice 主动接入](v-voice-auto-binding.md)。
- Music / Desensitize / Image 使用固定应用可信绑定。Thor Music 真实直调及网关 WAV 已验收；Thor Image 真实直调及网关 PNG、业务取消后原生进程退出和预算释放已验收。这些是隔离候选实例结果，尚未作为新版安装发布，见 [固定应用绑定](application-auto-binding.md)。
- V-Sub 动态私有 namespace 使用独立 Root broker 契约，已通过隔离协议验收；L4T 最终 Root worker 的真实 20 秒字幕直调、网关、越界拒绝与业务取消已验收，L4T 与 Thor 的 worker generation 重启均已补齐，见 [V-Sub 可信绑定](v-sub-auto-binding.md)。
- HybRAG 后台推理凭据协议已通过隔离测试；实际知识库任务、旧后端兼容及客户端最终镜像验收仍须完成。Desensitize 的 OCR / NER 已分别完成 AMD、Thor、L4T 的真实线程策略资源实测，Thor 真实业务 API / 自动绑定及边界拒绝已通过。
- 未校准的模型或 profile 保持不可执行，不能用其他机器结果或默认预算代替。

## 分工与验收

V-Scheduler 会话负责服务端、可信绑定、统一文档、主仓 README 和旧源码子模块清理；迁移客户端会话负责 V-Voice、V-Sub、V-Music、Image Studio、V-ChatCut、desensitize、HybRAG 的客户端改动及发布。双方只提交各自文件和子模块引用，保留其他未提交改动。

两会话须协调机器操作窗口。目标为 192=`l4t`、229=`thor-spark`、232=`amd`，避免同时升级、重启或运行重模型。每个应用在其支持的 profile 上记录实际模型、任务、输出和调度账本；不支持的组合明确列出，不用其他机器结果代替。

完成标准包含共享目录及原模型保留、真实任务输出、同一次计算只占一份预算、忙碌与过期许可不绕过、取消后的停止确认、重启恢复、全部受影响 profile 构建发布及目标机器安装。实际部署结果与物理资源受限项目按发布验收记录，不能将 429 容量拒绝记为模型输出成功。

2026-10-16 补充：L4T / Thor V-Sub 最终 Root 真实直调、网关、取消、generation 轮换均通过；Thor V-Voice 真实 TTS / ASR 直调与网关通过，L4T 最终客户端输入边界拒绝验证通过。192 的可用 RAM 无法同时满足已测 Voice 预算与宿主余量；232 原 Model Hub QA 持续占用 CPU，保留其工作负载并记录容量限制。
