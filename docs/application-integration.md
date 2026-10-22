# V-Scheduler 统一接入指南

本文是应用和模型服务的统一操作入口，覆盖原 V-Router 模型调用、V-Scheduler 调度接管、vLLM 主动注册，以及是否由本 Scheduler 调度。VOS 安装统一应用 `com.ictrek.v-scheduler`；网关、调度器和模型仓库均由该应用提供。

- [选择接入方式与调度策略](#integration-mode)
- [地址、端口与身份](#addresses-and-identity)
- [一个推理实例的完整接入流程：服务管理、模型路由与调度](#managed-inference)
- [节点、应用、预算与停止的统一生命周期](#unified-lifecycle)
- [vLLM 主动注册、public/private 与生命周期](#vllm-registration)
- [启用本实例调度](#scheduled-vllm)
- [仅路由外部独立资源](#external-vllm)
- [同步许可与异步任务](#application-contract)
- [共享模型目录](#shared-models)
- [远端节点与当前能力边界](#remote-nodes)
- [推理排队、模型驻留与服务关联](#inference-queue-residency)
- [上线验收](#acceptance)

<a id="integration-mode"></a>
## 选择接入方式与调度策略

| 需求 | 接入方式 | 谁负责资源 |
|---|---|---|
| 只调用统一目录里的模型 | 使用网关 `/v1/*`；普通调用方不另申请许可 | 网关执行该模型已批准的策略 |
| 同时管理服务卡片、统一调用并调度资源 | 按[完整接入流程](#managed-inference)登记同一实例，分别完成管理契约、网关注册与资源批准 | 实际执行节点的 Scheduler 资源池 |
| 将 vLLM 等模型加入统一目录 | 启动就绪后主动注册，运行期心跳，退出注销 | 注册本身不纳管算力 |
| 让本 Scheduler 调度同步或流式计算 | 批准实际节点、executor、服务身份及逐模型 `permit` 策略 | 网关在实际节点申请、续租及释放许可 |
| 只路由外部独立运行的模型，不交给本 Scheduler 调度 | 管理员批准注册策略 `scheduler.mode=cloud` | 外部服务运行方负责容量、并发与停止 |
| 本应用自己执行异步重任务 | 保留业务任务与输出 API，使用 `/tasks` 和受控 executor | Scheduler 排队与预约，业务运行时执行并确认停止 |
| 完全自行调用和管理模型 | 直接使用模型服务自己的 API，无需注册 | 该服务运行方负责；不计入本 Scheduler 调度 |

**注册、可见性、调度是三件独立的事。** `visibility=public/private` 只决定哪些身份能看到和调用模型，不决定调度模式。注册成功、URL 位于远端、页面显示健康，也不代表资源已由 Scheduler 纳管。

当前“是否启用调度”通过已批准的实例策略和逐模型配置表达，不是普通模型用户可随意切换的开关。统一 VOS 包保留 `V_ROUTER_SCHEDULER_REQUIRED=true`，不能为单个服务关闭全局准入。`cloud` 是现有协议对外部独立资源的策略名称，也可用于经管理员确认由其他运行方管理的自建远端 vLLM；不能用它豁免本机或同一纳管资源池的计算。

<a id="addresses-and-identity"></a>
## 统一应用的地址与依赖

需要统一模型与调度服务的 VOS 客户端依赖 `com.ictrek.v-scheduler`；打包时从当前已发布 Release / App Store 解析依赖版本，不再声明必需依赖独立 `com.ictrek.v-router`。接管旧 Router 时按[原位迁移步骤](unified-application.md#retain-the-original-gateway-data-in-place)保留原数据；与旧服务共存时避免端口冲突及同一目录的双写。HybRAG 的 PGV 为唯一必需依赖，Scheduler、旧 Model Hub 和云端模型按可用服务接入。

| 调用方 | 模型 API base | 调度 API base |
|---|---|---|
| VOS 后端容器 | `http://v-scheduler:8080/v1` | `http://v-scheduler:38120/api/v1` |
| 同域前端 | `/api/com.ictrek.v-scheduler/v1` | `/api/com.ictrek.v-scheduler/scheduler/api/v1` |
| 局域网服务 | `http://<vos-host>:22888/v1` | `http://<vos-host>:22888/scheduler/api/v1` |

下文 `/tasks`、`/permits`、`/executors` 等短路径均相对于调度 API base。网关 `/api/v1/*` 保留模型与上游管理含义。旧网关别名和旧同域路径为已有客户端保留兼容；控制器直连地址是上表中的正式调度入口。

VOS 本机资源采集内置于应用，无需应用开发者部署独立 Agent；外部节点通过所属 VOS 应用发现和管理，复用其已有监控；Scheduler 预算规划不直接授予执行权限，远端调度控制待验证，见[VOS 节点管理](node-management.md)。用户无需 SSH 或另装 Agent。自动发现模型不等于批准计算，仍需实际峰值预算与执行器授权。


同一容器网络的首选网关别名为 `v-scheduler:8080`，避免与旧独立 Router 的 `v-router` 兼容别名混淆。调度经网关代理时使用 `http://v-scheduler:8080/scheduler/api/v1`。网关 `/api/v1/*` 是模型与上游管理，控制器直连 `/api/v1/*` 是调度管理；两者不可混用。

模型仓库内部入口为 `http://v-scheduler:6005/api/v1`，默认宿主机入口为 `http://<vos-host>:36005/api/v1`。所有九项内外端口、兼容别名及安装参数见 [README 端口表](../README.md#host-alias内部端口与宿主机端口)。应用内「API 文档 → 应用接入」显示当前安装配置的实际端口。

以下是 `docker-compose.yml` 实际声明的 host alias，所有 profile 共用；正式地址用于新接入，兼容地址用于已有客户端。`<vos-host>` 为安装该应用的宿主机 IP 或域名，外部端口是安装默认值，可在安装参数中修改。应用内「模型与网关 API」「模型管理 API」「应用接入」均显示正式与兼容地址及当前安装的实际映射端口；「调度管理 API」也列出控制器直连与新旧网关代理地址。

| 功能 | 正式 host alias:内部端口 | 兼容别名 | 宿主机 host:默认外部端口 |
|---|---|---|---|
| 统一推理 `/v1/*`、网关管理 `/api/v1/*`、界面 `/` | `v-scheduler:8080` | `v-scheduler-gateway:8080`、`v-router:8080`、`v-router-backend:8080` | `<vos-host>:22888` |
| 调度接管 `/api/v1/*` | `v-scheduler:38120` | `v-scheduler-backend:38120`、`v-scheduler-frontend:38120` | `<vos-host>:38120` |
| 模型仓库 `/api/v1/*`、`/openapi.json` | `v-scheduler:6005` | `v-scheduler-model-hub:6005`、`v-scheduler-model-hub-backend:6005` | `<vos-host>:36005` |
| Ollama QA 原生管理 `/api/*` | `v-scheduler-ollama-qa:11434` | — | `<vos-host>:12434` |
| Ollama QA 推理 `/v1/chat/completions`、指标 `/metrics` | `v-scheduler-ollama-qa:11535` | — | `<vos-host>:12535` |
| Ollama Embedding 原生管理 `/api/*` | `v-scheduler-ollama-embedding:11434` | — | `<vos-host>:12435` |
| Ollama Embedding 推理 `/v1/embeddings`、指标 `/metrics` | `v-scheduler-ollama-embedding:11535` | — | `<vos-host>:12536` |
| Ollama Rerank 原生管理 `/api/*` | `v-scheduler-ollama-rerank:11434` | — | `<vos-host>:12436` |
| Ollama Rerank 推理 `/v1/rerank`、指标 `/metrics` | `v-scheduler-ollama-rerank:11535` | — | `<vos-host>:12537` |

新应用和已有应用后续适配均推荐使用正式名称 `v-scheduler`，通过端口区分三个入口；其他统一容器名称全部为兼容别名，现有配置继续可用。旧客户端自动绑定沿用已核验的兼容地址；支持 `canonical-v-scheduler-v1` 的 HybRAG / V-Sub 先确认能力再迁移正式地址，保留原凭据并递增 revision。V-Voice 0.0.22 使用正式地址且复用 0.0.21 原生镜像。其他固定绑定仍按各自可信契约交接，不直接修改私钥或绕过来源校验。别名在 Docker `vos_default` 网络中解析；同一行的别名指向同一个容器，不会启动额外容器。Scheduler、Router、模型仓库和界面共用统一容器，三个 Ollama 各为独立容器。新客户端不要只配置 `v-router`：若仍安装旧独立 Router，该兼容名可能解析到旧容器。`host.docker.internal` 是统一容器用于访问宿主机映射端口的 `host-gateway` 别名，不是服务入口。单个 Ollama 入口不替代统一网关的模型命名和调度策略。


| 凭据 | 用途 | 不能替代 |
|---|---|---|
| VOS 用户 Bearer / 管理员配置的网关 Key | 模型调用；有相应权限时注册服务 | 调度 submitter 凭据 |
| Scheduler 独立服务凭据 | 应用自己的任务、许可及批准的 executor | 浏览器用户身份、网关注册身份 |
| vLLM 自身 API Key | 网关访问上游 vLLM，放入受保护注册配置的 `api_key` | 网关调用者凭据 |
| 专用后台推理身份 | 如 HybRAG 的受限后台模型调用 | 注册、网关管理、调度管理员权限 |

不同应用使用各自的服务身份；私钥不进入浏览器、discovery、共享模型目录或日志。无效 Bearer 返回 401，不能填写任意占位 Key。注册 `scheduler` 策略需要管理员权限；普通身份提交该字段返回 403。私有模型由注册请求的真实身份决定 owner；系统级管理员注册器不能代替另一个用户创建其私有身份。后台可信交接见 [HybRAG 身份参考](hybrag-gateway-identity.md)。

<a id="managed-inference"></a>
## 一个推理实例的完整接入流程

模型仓库中的「推理服务」（原 ModelHub 功能）、网关模型目录和 Scheduler 资源授权共同描述同一个实例。接入应用应在自己的 VOS App 启动与健康管理流程中维护这三部分，用稳定的实例 ID 关联它们；最终用户通过 VOS 安装应用，不需要 SSH 或逐台安装 Agent。

| 同一实例的记录 | 入口与职责 | 成功代表什么 |
|---|---|---|
| 推理服务卡片 | 模型仓库 `POST /inference-services/register`，保存健康、模型列表、运行状态和可选装卸模型契约 | 可以查看或管理该服务；不会自动加入网关模型目录，也不会创建许可 |
| 可调用模型 | 网关 `POST /api/v1/registrations`，维护 owner、public/private、原生模型与接口、租约 | 模型进入该身份可见的目录；仍需实际健康和调度批准 |
| 执行权限与预算 | Scheduler 批准实际节点、executor、服务身份，并配置网关逐模型策略，或完成应用专用自动绑定 | 请求可按批准的输入、资源和停止契约取得 permit/task；不保证任何时刻都有空闲资源 |

### 1. 准备实例、身份和模型文件

以下沿用本文 vLLM 示例：稳定实例 ID 为 `vllm-node-a-1`，网关 upstream ID 为 `registered-vllm-node-a-1`，原生模型 ID 为 `demo-chat`，客户端调用名为 `registered/demo-chat`。卡片、网关注册和执行器中的地址必须指向同一个实际实例；模型仓库的元数据不授予网关 owner 或 Scheduler 权限。

模型从安装时指定的公共目录读取，使用[共享模型目录](#shared-models)中的导出路径。权重下载完成只证明文件就绪，不证明推理进程已加载、接口健康或资源获批。先核验服务版本、模型、最大输入、运行参数及实际节点，按[启用本实例调度](#scheduled-vllm)批准资源与输入范围。服务开始接受计算前须启用原生准入或限制计算入口；健康与列模型接口可以先开放。

### 2. 登记模型仓库的推理服务卡片

首选经过统一网关访问：设 `MODEL_HUB=$GATEWAY/model-hub/api/v1`，其中 `GATEWAY=http://v-scheduler:8080`（容器内）或 `http://<vos-host>:22888`（宿主机映射），使用网关接受的 VOS 用户身份。它与网关注册入口共享应用入口，但调用者仍须具备各接口要求的权限，不能使用 Scheduler submitter 令牌替代。下例 `GATEWAY_TOKEN` 使用获准管理的 VOS 用户令牌。

需要直连时，模型仓库内部入口为 `http://v-scheduler:6005/api/v1`，宿主机默认入口为 `http://<vos-host>:36005/api/v1`。这个独立管理监听器面向可信管理网络，不能把任意 Bearer 当成它已具备网关用户隔离的证明，也不要将管理端口直接暴露给不可信调用方。

为同一 vLLM 实例准备 `service.json`：

```json
{
  "service_id": "vllm-node-a-1",
  "name": "vLLM node A replica 1",
  "kind": "openai_compatible",
  "base_url": "http://compute-a.example:8000",
  "gateway_url": "http://compute-a.example:8000/v1",
  "api_contract": {
    "health_path": "/health",
    "models_path": "/v1/models"
  },
  "metadata": {"gateway_upstream_id": "registered-vllm-node-a-1"}
}
```

```bash
curl --fail-with-body "$MODEL_HUB/inference-services/register" \
  -H "Authorization: Bearer $GATEWAY_TOKEN" \
  -H 'Content-Type: application/json' --data-binary @service.json
curl --fail-with-body "$MODEL_HUB/inference-services/vllm-node-a-1" \
  -H "Authorization: Bearer $GATEWAY_TOKEN"
```

`base_url` 与契约路径拼接，因此这里使用服务根地址；下一节网关注册的 `base_url` 则包含 `/v1`。`gateway_url` 是服务元数据，不会触发网关注册。`metadata.gateway_upstream_id` 仅供关联查看，不参与鉴权或自动创建调度策略。卡片可在「模型仓库 → 推理服务」找到，也可通过 `GET /inference-services?include_builtin=true` 连同内置服务查看。

原生 Ollama 使用 `kind=ollama`，`base_url` 指向原生 API 根地址，自动补齐 `/api/version`、`/api/tags`、`/api/ps` 及装卸契约。OpenAI/custom 服务必须显式声明自身支持的 `api_contract`；普通 vLLM 没有通用的模型装卸 API，示例只登记健康和模型列表。不要为它填写虚构的 start/stop 路径。当前卡片探测和操作不提供网关上游 `api_key` 的自动注入；需要受保护管理 API 的服务，应由应用提供受控管理适配入口，不能把密钥拼进 URL 或 metadata。

### 3. 注册模型路由并启用调度

对同一实例继续执行下节的完整 `registration.json`、注册及心跳流程，再完成[逐模型 permit 策略](#scheduled-vllm)。一次接入由应用管理流程协调三部分；当前不存在把服务卡片、网关注册、执行器批准合为一次事务的 API，不能只登记卡片便宣称已调度。任一步失败时保留可诊断状态，停止对外宣告就绪并重试失败步骤；没有有效授权时禁止计算。

自带的三个 Ollama 已有卡片，网关直接使用 `v-scheduler/ollama-qa`、`v-scheduler/ollama-embedding`、`v-scheduler/ollama-rerank`，由内置绑定流程配置对应模型策略；无需重新扫描旧 ModelHub 的 Ollama 或重复登记为另一组上游。对 V-Voice、V-Sub 等专用绑定应用，继续使用其 discovery/校准/绑定契约，不另建一套通用 executor 重复预约。

### 4. 调用、装卸、取消与退出采用同一生命周期

- 普通模型调用方使用网关 `/v1/*` 和自己的调用身份；网关取得许可后再转发。同一次计算只预约一次，原生服务按许可核验契约复用该预约。
- 卡片的 `POST /inference-services/{id}/models/start`、`.../stop`，body 至少含 `model`，可含 `confirm_timeout_seconds`（0–60，默认 8）。内置 Ollama 的启动通过统一排队与资源准入链路预热，停止通过原生停止核验释放驻留预约；详见[驻留与自动卸载](#inference-queue-residency)。外部服务的卡片操作仍是管理代理，不会自动申请 Scheduler 许可；start 可能实际预热计算，必须由接入应用在原生管理入口实施准入，并将驻留权重纳入预算。不能用卡片按钮或直连端口绕过调度。
- 卡片上的 `healthy`、`running_state_observed`、操作返回的确认信息属于管理观测；它们不能代替具体 permit/task 的停止证明。只有计算执行端确认该执行停止后才释放资源；未知状态保留预约并对账。
- 应用保持网关注册心跳；服务卡片保存在共享注册表中，没有网关注册那样的 TTL。心跳丢失不会自动删除卡片，删除卡片也不会注销网关或停止进程。
- 正常退出先停止新请求分配（注销网关注册），再完成或取消在途任务，核验原生停止并释放许可，最后卸载模型/关闭服务。永久移除时再 `DELETE /inference-services/{id}` 删除卡片；暂时重启保留卡片，恢复后复用同一 ID 更新卡片与网关注册。停用 executor 前确认对应任务、许可均已结束。

验收必须同时证明：卡片可见且健康、调用者能发现正确模型、资源批准指向实际节点、直调与网关共用一次准入、取消后原生停止、重启后重新绑定/注册可恢复。仅看卡片绿色、注册 HTTP 200 或模型文件存在，不能判定接入完成。

<a id="calibrated-local-models"></a>
### 内置 Ollama 的实测小模型配置

模型仓库里的内置服务卡片、网关模型目录和逐模型执行器仍是同一实例的三个视图。内置卡片无需重复注册；新增模型可以使用已批准的独立执行器，不能把文件大小估算当成实测峰值，也不能直接调低系统安全余量。

192 测试节点的 CPU 配置已用同版本 Ollama、只读共享权重、真实外层许可和 CPU/RAM 硬限额测量，并将实测权重复制为本地固定标签；复制只增加 manifest 引用，不复制权重 blob。原始下载模型和其他模型的默认预算保持原样。

| 模型目录中的调用名 | 单次批准 RAM | 已批准运行边界 |
|---|---|---|
| `v-scheduler/ollama-qa/v-scheduler-lite-qa:qwen2.5-0.5b-q2-20261019` | 896 MiB | CPU 4.5 核、GPU 0、context 512、batch 128、输出最多 128 tokens |
| `v-scheduler/ollama-embedding/v-scheduler-lite-embedding:all-minilm-22m-20261019` | 512 MiB | CPU 4.5 核、GPU 0、context 512、batch 128、向量维度 384 |

两者请求体上限均为 16 KiB；网关补齐已批准的运行参数，超界请求在计算前拒绝。控制器保留正常事务预约、系统安全余量、取消后的原生停止核验及独立服务身份。内置 Ollama 卡片启动复用上述受控链路；外部服务的启动/停止管理代理不会因此自动取得这些许可。

HybRAG 从「已注册的 V-Scheduler 模型」选择这两个调用名，embedding 维度选择 384；在这个低内存配置中关闭额外重排、查询改写和后台摘要，使用 512 上下文与 128 输出 token。已有知识库和默认模型不被替换，重新建库时才选择新 embedding 维度。HybRAG 需使用支持动态 Scheduler 模型身份交接的已发布版本（0.1.73 起），不将服务密钥填入模型表单。

这些是针对该节点实测并批准的配置，不是所有机器通用的推荐预算。更换固定标签的权重、Ollama 镜像、并发数或运行参数前，应先停用该审批、确认在途计算停止，再重新测量和批准；不能让新权重沿用旧预算。应用开发者通过本应用既有管理 API 维护执行器、独立身份与受保护的逐模型绑定，终端用户继续通过 VOS 安装和界面使用。完整 RAG 验收须确认实际回答引用了新建测试文档，不能仅以文件就绪、目录可见或 HTTP 200 作为成功。

<a id="unified-lifecycle"></a>
## 节点、应用、预算与停止的统一生命周期

最终用户只安装 VOS App。节点采集、应用身份交接、执行器发现、心跳和停止适配由应用自身管理；不要求用户 SSH 登录或手工部署 Agent。已有 V-Voice、V-Sub、Music、Image 等自动绑定继续使用已验证的原契约。以下是新应用可启用的 `application-lifecycle-v1`，不是旧应用已经全部切换的声明。

### 1. 自动发现并核验预算

本机内置组件约每 2 秒向 `/workers/{node_id}/heartbeat` 上报节点会话和资源采样；12 秒无节点心跳、资源采样过期或节点排空时拒绝新执行。应用自己的心跳不能提交节点遥测。批准容量、驻留基线、安全保留、已有预约和实际余量共同参与准入，不能从模型文件大小或自报空闲内存直接授予预算。

应用安装后的可信管理流程按实际 profile、硬件、模型和运行设置匹配校准，自动交接独立服务身份及已批准 executor。已存在的管理 API 负责预算批准和修改：`POST /nodes/{id}`、`POST /executors/{id}` 使用当前 version，不能在仍有未停止执行时缩减或撤销。普通调用者不能自批资源；未匹配校准的组合保持未就绪。外部 ModelInfer 节点仍须通过所属 VOS App 的执行、停止和恢复验证，监控可见不代表可以执行。

### 优先级、协作抢占与管理员强制尝试

`POST /tasks` 和 `POST /permits` 都接受可选的 `scheduling`；未传时保持旧请求语义，默认不允许抢占，也不允许被抢占。

```json
{
  "scheduling": {
    "priority": "high",
    "preemptible": false,
    "allow_preemption": true,
    "force_admission": false
  }
}
```

| 参数 | 含义与边界 |
|---|---|
| `priority` | `background`、`interactive`、`high`，分别为基础等级 0、1、2；等待每 30 秒加一级，同分按到达顺序，防止后台任务长期饥饿。 |
| `preemptible` | 当前执行允许被更高优先级请求停止。异步任务仅接受已批准 `retry_mode=idempotent` 且有剩余尝试次数的执行器；不支持重启的执行器在提交时明确拒绝。 |
| `allow_preemption` | 请求低优先级且显式允许被抢占的执行让出资源。只发停止意图，不直接释放预约；旧执行和权重确认退出后才运行新执行。 |
| `force_admission` | 默认关闭。管理员显式尝试较低的软余量，不改变模型预算、已占用资源、硬件容量、独立身份、节点新鲜度或停止证明；至少保留 1 GiB 主机内存及独立显存硬余量。仍可能返回资源不足。 |

网关 `/v1/*` 对应使用 `X-VScheduler-Priority`、`X-VScheduler-Preemptible`、`X-VScheduler-Allow-Preemption`、`X-VScheduler-Force-Admission`，布尔头只接受 `true` / `false`。建议同时设置 `X-VScheduler-Queue-Wait-Seconds: 120` 和唯一 `X-VScheduler-Request-ID`，总超时覆盖排队、冷加载和推理。试聊中也可选择优先级、双向抢占意图及管理员强制尝试。

强制尝试由控制器再次核验管理员身份：普通应用服务身份仍用于任务归属，额外以 `X-VScheduler-Admin-Authorization: Bearer <VOS 管理员 token>` 提交当前管理员身份；网关使用已核验的当前管理员登录身份交接。该身份只在当前请求传递，不写入任务、执行日志或重启回放；缺少/失效身份会拒绝强制请求。普通应用不能凭 `force_admission: true` 自行提升权限，成功的强制申请有独立审计记录。

仅向具有可核验原生停止协议且接收端心跳正常的执行器发送抢占命令；其余执行继续排队等待，不把允许抢占等同于强杀或跳过停止证明。

高优先级**排队任务**可以先于低优先级进入执行；**运行任务**只有双边显式同意才会协作停止。Image Studio 声明 `preemption_modes: ["restart"]` 后，旧 sd-cli 退出且预约释放，任务回到队列，使用新 attempt/fence 从头重跑，最多受已批准尝试次数限制。若旧执行已完成成功则保留成功，不重复生成；用户主动取消优先于重排。没有声明断点能力时不称作暂停续跑。V-Voice、V-Sub、Desensitize 的同步执行被抢占后不会重放原始请求，调用方收到中断/失败后自行决定是否重新提交。

各 App 的调用方式：V-Voice、V-Sub、Desensitize 接受上述请求头并通过私有身份转交给控制器；Image Studio 的图像/视频生成 JSON 接受上述 `scheduling` 对象，参数随原生任务持久化，管理员身份不持久化。队列视图与任务详情显示优先级、是否允许停止重排及抢占原因。跨应用调用继续只预约实际执行端的一份预算，不能同时重新申请第二份外层许可。

统一内存节点可以经管理员节点配置明确设置 `safety_reserve.memory_mib: 1024`；这只针对该节点的一份主机/CUDA 共享内存池。未配置时保留原自动安全余量，独立显存节点保持原策略。准入条件仍是「实际可用内存 ≥ 未兑现预约 + 本次实测预算 + 生效安全保留」，不是余量有 1 GiB 就运行任意模型。192 的 SD-Turbo 使用本机校准的 Q4 / `max_vram=2` 低内存参数，原共享权重不变；232 使用独立校准的 FP16 与显存预算，不套用另一台的结果。

### 2. 应用登记与定时心跳

以下路径都属于调度命名空间。容器直连根为 `http://v-scheduler:38120/api/v1`，经过统一网关为 `http://v-scheduler:8080/scheduler/api/v1`；宿主机默认分别为 `http://<vos-host>:38120/api/v1` 和 `http://<vos-host>:22888/scheduler/api/v1`。旧别名只供兼容，详见上方端口表。

| 方法与相对路径 | 身份与行为 |
|---|---|
| `GET /integrations` | 应用仅看自己，管理员可看全部；包含 revision、应用心跳、执行器批准预算、节点容量/保留/新鲜度及未释放执行 |
| `POST /integrations` | 应用独立服务身份引用已批准 executor；不接受自报预算、任意控制 URL 或其他应用 ID |
| `DELETE /integrations` | 应用退出前注销生命周期；有未结束执行或停止请求则拒绝，幂等重试 |
| `POST /integrations/heartbeat` | 应用发送当前 revision，响应给出下次间隔和待处理停止命令 |
| `POST /permits/{id}/stop` | 许可拥有者或管理员请求停止，不立即释放资源 |
| `POST /permits/{id}/stopped` | 注册的原生执行端提交匹配的停止回执，核验后释放该许可 |

首次登记请求：

```json
{"protocol_version":1,"revision":0,"executors":["approved-executor-id"],"stop_protocol":"permit-native-exit-v1"}
```

响应含 `app_id`、`revision`、`heartbeat_after_seconds=5`、`stale_after_seconds=20` 和 `budget_granted=false`。应用将 revision 与原执行状态私有持久化；按返回间隔发送 `{"revision":1}`，不能把当前常量当成永远固定的协议。超过 20 秒无应用心跳暂停这些执行器的新准入；心跳恢复、批准定义仍匹配后自动恢复。不会因过期释放已有预约或自动重放执行。

同内容、当前 revision 的重复登记不推进版本。版本冲突先 GET 自己的状态，再决定是否重登记；变更执行器集合时有活动任务、未释放许可或待停止请求会返回 409。一个执行器只由一个应用生命周期身份接收停止命令。身份轮换仍由原可信绑定流程负责，不向浏览器下发私有凭据。

模型服务卡片和 `/api/v1/registrations` 路由注册仍使用同一实例 ID 与明确映射，按前节流程完成。应用生命周期心跳、路由租约心跳、节点采样各有职责，不能互相替代。生命周期登记不会隐式创建卡片、放开模型可见性或批准任意远端 URL。

### 3. 调用、停止与资源释放

计算继续使用既有 permit 验票或持久 task，确保同一次计算只预约一次。请求 `POST /permits/{id}/stop` 后，响应 `state=stop_requested`、`resources_released=false`；许可不再允许续租或再次验票。已开始的计算由所属应用停止，不由 Scheduler 杀任意宿主进程。

应用在 heartbeat 响应的 `stop_requests` 中取得 `{permit_id, executor_id, node_id, model_id, execution_id, nonce, action:"cancel-and-unload"}`。未确认的命令重复投递且 nonce 不变；应用须以 permit + execution_id 幂等处理，核对自己的持久执行句柄，取消原生计算并卸载对应权重。不得终止同名的另一次新执行。`execution_id=null` 表示还未原子认领，应用取消本地待启动记录并确认没有执行后以 null 回执；停止命令之后的迟到认领会被拒绝。执行端自己的租约 watchdog 仍须独立工作。

原生退出屏障和权重卸载均确认后，用执行端服务身份向 `/permits/{id}/stopped` 回报：

```json
{"nonce":"heartbeat-command-nonce","execution_id":"original-native-execution-id","execution_token":"original-private-execution-token","native_exit_barrier":true,"model_unloaded":true,"evidence":"native process exited; model weights unloaded"}
```

凭据仅来自该执行原有的许可交接，不能从列表接口获取。控制器核对已注册接收者、执行器授权、nonce、已认领 execution_id 和原 execution_token 哈希。缺项、错误回执或仅取消输出而未卸载权重都不释放预算。成功回执可幂等重试；停止命令与确认记录持久化，服务重启后仍可继续核验。心跳丢失、HTTP 成功、空运行列表或租约到期均不构成停止证明。

异步应用沿用 `POST /tasks/{id}/cancel`，经原 executor 的 `inspect/cancel` 和 attempt/fence 停止报告完成核验；不能把 task 取消伪装为另一个同步 permit 的停止。内置 Ollama 继续使用已有原生停止证明与驻留管理，无需重新登记这套应用协议。

`STOP_ADAPTER_NOT_REGISTERED` 表示该执行尚无唯一已注册的停止适配，不能显示为已停；`STOP_REQUESTED` 表示当前执行已要求退出；`STOP_PROOF_MISMATCH` 表示回执与执行不匹配；`APPLICATION_HEARTBEAT_STALE` 暂停新执行；`EXECUTOR_APPROVAL_CHANGED` 要求重新匹配批准并登记。界面「节点管理 → 应用统一接入」汇总这些节点、预算和应用状态，并提供已接入执行的停止入口。

### 已适配的执行应用

V-Voice（含配乐）、V-Sub、Desensitize 与 Image Studio 在 VOS 启动及可信绑定完成后自动登记、心跳，无需手工录入身份或部署 Agent。控制器仅在应用 discovery 声明 `lifecycle_protocol_version=1` 时交接该协议，旧版仍沿用原绑定。V-ChatCut、HybRAG 等调用方沿用统一网关或任务接口，由实际执行端登记，调用方不重复申报同一份算力预算。

- V-Voice：停止当前请求；原生子进程退出或阻塞调用返回、缓存卸载后确认。
- V-Sub：Root broker 将停止传给私有 worker，等待原生子进程退出后确认。
- Desensitize：阻止后续计算，等待当前 ONNX 调用返回并卸载，不能强杀线程后直接释放。
- Image Studio 和 V-Voice 异步创作：使用 `/tasks/{id}/cancel`，沿用 attempt/fence 与原生退出核验；统一接入页面也显示这些任务。

应用心跳返回 `executions`（`permit_id`、`executor_id`、`execution_id`、`state`）及 `complete_execution_snapshot`。快照最多 256 项，截断时完整标记为 false；仅完整快照可用于清理本地已确认退出且已释放的回执。响应不包含执行令牌。停止证明保存在应用私有持久卷，绑定原 execution/token/nonce；重启后可重试已确认回执，不能推定未知执行已经停止。

<a id="vllm-registration"></a>
## vLLM 主动注册与生命周期

下文网关根地址为 `http://<vos-host>:22888`，调度根地址为它的 `/scheduler/api/v1`。VOS 容器内分别使用 `http://v-scheduler:8080` 和 `http://v-scheduler:38120/api/v1`。跨不可信网络使用已验证证书的 HTTPS 入口；不要用 `curl -k`。

设 `GATEWAY=http://<vos-host>:22888`、`SCHEDULER=$GATEWAY/scheduler/api/v1`。注册脚本使用自己的 VOS 身份或管理员批准的网关 Key。`GATEWAY_TOKEN` 与调度器服务令牌是不同凭据；不要把服务凭据交给浏览器。示例变量通过受保护的运行环境注入，JSON 文件设为 `0600`。`base_url` 必须是**网关容器可达**的 vLLM OpenAI API 根地址，通常包含 `/v1`；不能填远端机器自己的 `127.0.0.1`。

准备 `registration.json`（将 URL 和模型名替换为真实值）：

```json
{
  "service_id": "vllm-node-a-1",
  "name": "vLLM node A replica 1",
  "base_url": "http://compute-a.example:8000/v1",
  "protocol": "chat_completions",
  "visibility": "private",
  "model": "demo-chat",
  "model_kind": "chat",
  "endpoints": ["chat/completions"],
  "capabilities": ["chat"],
  "ttl_seconds": 90
}
```

```bash
curl --fail-with-body "$GATEWAY/api/v1/registrations" \
  -H "Authorization: Bearer $GATEWAY_TOKEN" \
  -H 'Content-Type: application/json' --data-binary @registration.json
```

若模型服务本身启用 API Key，在受保护的注册 JSON 中增加 `api_key`；它是上游凭据，与网关调用者的 Bearer 不同，不得提交到仓库。

注册由 VOS App 的启动/健康管理流程实现，vLLM 本体不会自行注册。先确认上游能实际提供模型，注册成功后才开始心跳。

这是显式单模型注册，不探测 `/models`，成功也不证明模型就绪。需要自动读取模型列表时删除 `model`，由注册流程探测 `base_url/models`；仍应声明真实支持的 endpoints。Embedding 实例使用 `model_kind=embedding`、`endpoints=["embeddings"]` 和 `capabilities=["embedding"]`。不要为不支持的接口注册能力。

自动探测多个模型时，可用 `model_endpoints`、`model_capabilities` 按原生模型 ID 覆盖各模型的接口和能力；也支持用 `models` 显式登记多个模型。只为真实支持工具调用的模型声明 `tools`。字段定义见[注册 API](../api/router-openapi-zh-CN.json)。

`visibility` 可选 `private` 或 `public`：private 只向所属身份与管理员提供模型；public 使模型按网关账户策略对其他用户可见。可见性不改变后续调度审批。仅把 `visibility` 改为 public 不会获得资源权限。

返回的 upstream ID 是 `registered-vllm-node-a-1`。控制面 `GET /api/v1/models` 返回每个模型的 `qualified`，本例为 `registered/demo-chat`；私有模型仅对所属身份与管理员可见。不同副本使用不同 `service_id`；相同模型可能有多个候选副本，因此**每个副本都要批准其真实节点与预算**。逐模型策略键使用 `registered-vllm-node-a-1/demo-chat`，不使用客户端的 `registered/demo-chat`。

注册响应包含 `heartbeat_after_seconds`，按返回间隔续期；90 秒租约通常为每 30 秒一次。注册成功返回后再开始心跳：

```bash
curl --fail-with-body -X POST \
  "$GATEWAY/api/v1/registrations/registered-vllm-node-a-1/heartbeat" \
  -H "Authorization: Bearer $GATEWAY_TOKEN" \
  -H 'Content-Type: application/json' -d '{"ttl_seconds":90}'
```

健康检查失败时停止心跳，让租约退出新请求分配；恢复后重新检查模型并完整注册。不同存活副本使用不同稳定 `service_id`，同一副本重启复用原 ID。其他 owner 尝试接管会收到 409。

TTL 支持 30–3600 秒，默认 90 秒。心跳返回 404 时重新执行注册；网关重启或租约过期均可能需要重新注册。心跳不刷新模型列表，模型变化时重新注册。注册循环应保留所选调度策略，避免后续重注册丢失管理员批准的外部资源声明。退出时可 `DELETE /api/v1/registrations/registered-vllm-node-a-1`，退出新请求分配，已开始请求继续完成。401 时修复或刷新身份；注册 409 时修正 owner 或 ID；网络错误与 5xx 使用有上限的退避重试，不能去掉鉴权重试。

<a id="scheduled-vllm"></a>
## 让 Scheduler 调度该推理实例

以下是应用开发者的受控集成协议；本机复用内置组件。ModelInfer 远端控制未验证时不能据此手工激活真实计算节点，最终用户不需要安装 Agent。管理员将下列 executor 合并进控制器 `/config/controller.json` 的 `executors`，保留原有条目。数字仅用于展示字段，必须用该硬件、模型和最大输入的实测峰值替换：

```json
{
  "id": "v-router.vllm-a.infer",
  "contract_version": "1",
  "task_type": "model.infer",
  "node_id": "compute-a",
  "app_id": "v-router",
  "resources": {
    "cpu_millicores": 1000, "memory_mib": 2048,
    "gpu_slots": 1, "gpu_memory_mib": 2048
  },
  "upstream_urls": ["http://compute-a.example:8000/v1"]
}
```

`app_id` 必须匹配实际申请许可的 submitter 身份；示例为 `v-router`。`permit_executors` 只授予执行端核验许可的权限，不能让另一个应用申请该执行器的许可。每个集成使用自己的服务身份及执行器，不借用内置 Ollama 的身份。`upstream_urls` 与注册的实际 base URL 必须匹配。模型权重、KV cache 等常驻分配写入节点 `resident_baseline`；executor 写单次允许计算的增量峰值。系统余量写入 `safety_reserve`。不要只按模型文件大小估显存。

除应用安装时的配置外，受控接入也可由管理员通过 `POST /api/v1/executors/{id}` 创建专属执行器，body 为 `{"version":0,"enabled":true,"definition":<上面的完整执行器定义>}`。该路径属于调度 API base，需管理员身份；新 ID 使用版本 0，后续修改携带返回版本。节点及执行器必须空闲；收到冲突时先查明已有执行，不通过重启或强制释放绕过。实际应用接入由其 VOS App 的管理流程完成，用户无需 SSH 或手工部署 Agent。停用时使用当前版本与同一完整定义提交 `enabled:false`；确认测试许可全部释放后再撤销本次服务身份。

在网关 `/data/scheduler.local.json` 的 `models` 中合并：

```json
{
  "registered-vllm-node-a-1/demo-chat": {
    "mode": "permit",
    "executor_id": "v-router.vllm-a.infer",
    "completion": "synchronous",
    "max_input_bytes": 1048576,
    "limits": {"max_tokens": 512, "n": 1}
  }
}
```

完整文件还需 `url`、网关专属 `token`、`require_model_policy: true`，参照 [scheduler.example.json](../config/scheduler.example.json)。只批准运行端实际强制执行的参数，并覆盖默认值、最大输入与上下文的峰值。通用配置在进程启动时读取，变更后需按维护窗口重启**对应 controller / gateway 应用服务**；这不是重启宿主机或 Docker。V-Voice 专用自动绑定的热更新不代表通用 vLLM 配置也会热更新。

调用仍使用统一入口与自己的用户凭据：

```bash
curl --fail-with-body "$GATEWAY/v1/chat/completions" \
  -H "Authorization: Bearer $GATEWAY_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"model":"registered/demo-chat","messages":[{"role":"user","content":"Hello"}],"max_tokens":512,"n":1}'
```

网关申请 permit、续租并转发；完整同步/SSE 终态后提供停止证据再释放。资源不足返回 429，模型未批准或控制器不可用返回 503；不会改为直连。断开、超时或停止未知时可能保留 suspect 预约，须核验真实运行后处理。统一包自带 Ollama 已支持原生停止屏障与自动释放；通用 vLLM 不自动继承这项证明，不能仅凭断连或空闲时间释放其未知执行预约。

**原生 vLLM 模型 API 不等于 Scheduler executor 适配器。** 本接法先提供网关侧准入；若还要保证所有直连入口、断连停止和租约失效后的执行约束，必须限制绕过入口，并由服务包装层实现许可核验、幂等 execution_id、硬限额和停止机制。许可头与 `/permits/{id}/verify` 见[网关资源准入](resource-scheduling.md#同步执行端交接)。不要将普通 vLLM 标为 `delegated`；它没有因此创建持久 Scheduler task，也不会自动获得 start/inspect/cancel。仅注册 vLLM URL 不能实现模型自动装卸、任务迁移或抢占。注册接口即使接受 `scheduler.mode=permit`，也不能替代严格模式的逐模型策略、有效服务凭据和实际节点批准。

<a id="external-vllm"></a>
## 只路由外部独立资源

管理员确认它属于独立的外部资源后，在本指南的完整注册 JSON 中加入：

```json
{
  "scheduler": {"mode": "cloud"}
}
```

这是要合并的字段，不能单独作为注册请求。使用管理员网关身份重注册完整实例；普通身份提交 `scheduler` 字段返回 403。服务运行期的重注册也必须保留这份批准（应由受控管理员注册代理持有管理员身份，不要把它发给模型用户）。服务端把批准绑定到本次 `base_url`，客户端无需伪造 `upstream_url`。

此分支在加载本地调度配置前放行，不创建本地 permit，即使调度控制器暂不可用也可路由。网关用户鉴权、私有模型可见性和上游 API Key 仍有效。地址变化需要重新批准；重定向离开批准地址不能继承批准。私网地址也需明确的这项管理员声明，不能仅改成 cloud 类型就假定豁免。

若希望完全自行管理且不使用统一模型目录，直接访问原服务即可，不需要关闭统一应用的 `V_ROUTER_SCHEDULER_REQUIRED`。这个全局开关不能作为单个实例的豁免方式。

<a id="application-contract"></a>
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
流程、校准清单、持久 revision 与凭据轮换见 [V-Voice 主动接入](v-voice-auto-binding.md)。 V-Voice 0.0.27 已退役四个 PyTorch 原生模型；控制器从新版声明中移除它们的绑定和网关策略，保留 ONNX、声纹与 audio.cpp GGUF 的独立审批，不能把目录保留视为全部模型已获准运行。

V-Voice 已验证版本的 Python 直调在准入响应丢失后，可按[未认领许可恢复规则](v-voice-auto-binding.md#准入响应丢失时的未认领许可恢复)自动回收过期且从未认领的预约。此规则不适用于 Router 的其他推理路径，也不把租约到期当成已启动计算的停止证明。

- VOS 包依赖只检查应用安装及运行，不提供 Scheduler URL、令牌、节点容量或执行器。服务令牌放在 A 或 V-Router 的私有数据目录，权限 `0600`；浏览器与普通用户不能持有它。为不同应用绑定各自 `app_id` 和允许的 executor。
- 对每个节点分别验证：真实模型完成一项任务、重复提交仅执行一次、忙碌/过期遥测/排空时请求不能到达执行端、取消后有停止证据、断连与重启后资源状态正确。云端请求不产生本地 permit。检查实际执行节点，而非仅看另一个节点的成功结果。
- 当前实现的精确字段和能力以 [`GET /discovery`、公开 API 与接入标准](integration-standard.md)和 [OpenAPI](../api/openapi-v1.json)为准；V-Router 的具体配置和桥接协议见[网关资源准入参考](resource-scheduling.md)。

<a id="shared-models"></a>
## 模型仓库与共享目录

安装 V-Scheduler 时继续选择原 Model Hub 的结构化公共目录，保留 `MODEL_HUB_SHARED_MODELS_PATH` 的公共目录引用及 `[com.ictrek.model.storage]` 用途提示。仓库管理和三个 Ollama 在各自容器内挂载为 `/data`；其他应用按自己的只读挂载点解析 `export/hf/.../current` 或 `export/ms/.../current`，不能直接照抄另一个容器的绝对路径。

模型安装、选定权重文件下载、追加文件、更新、导入、别名和引用管理见[模型管理 API](model-management.md)及[选中文件下载示例](file-download-api.md)。管理模型文件不代表已加载模型或已授予执行预算。内置 Ollama 在统一网关直接发布 `v-scheduler/ollama-qa/<native-model>`、`v-scheduler/ollama-embedding/<native-model>`、`v-scheduler/ollama-rerank/<native-model>`；调用者使用目录实际返回的模型 ID，不重复向旧 Model Hub 注册同一实例。

<a id="remote-nodes"></a>
## 通过 VOS App 管理外部节点

用户只通过 VOS 安装和管理应用。统一应用自动读取已安装 ModelInfer 的远端节点清单与现有监控，不要求登录远端机器或运行第二套 Agent。页面提供“预算规划”和所属应用管理入口；规划不会授予执行权限，也不会改变已有推理部署。

多机 vLLM 的真实执行、停止与故障恢复契约尚待 VOS App 管理适配器验证。已预留来源规划与控制接口；控制请求当前返回 501，并保持不可调度。待管理链路恢复后由应用实现方验证，用户不承担手工部署工作。详情见[节点管理](node-management.md)。

底层 `model.infer` 的主节点和 `placements` 协议可原子预约多个独立资源池，已用隔离协议环境验证；这不等于已接管 ModelInfer 的多机推理。观察节点不能作为执行器位置，未经验证不能把它们伪装成已纳管资源。

## 在外部节点执行异步业务任务

本节仅面向业务 App 的集成开发；远端 VOS 管理适配器完成验证后才能启用，不是用户安装流程。中心增加业务 submitter 和它所属的 executor（`task_type` 使用真实业务类型），executor 绑定 `node_id=compute-a`；节点 Agent 的 `executors` 增加同 ID 的 `{id,url,health_url,token}` 适配器。URL 指向实现当前 start/inspect/cancel、持久句柄、fence、租约 watchdog 与停止证据的业务服务，**不能填写 vLLM `/v1` 作为这种适配器地址**。完整字段示例见 [controller](../config/controller.example.json) 与 [Agent](../config/agent.example.json)。

业务应用先持久保存自己的任务和待提交记录，再用自己的 submitter 令牌提交 `POST /tasks`，携带稳定 `Idempotency-Key`。当前 body 使用 `protocol_version`、`app_task_id`、`executor`、`parameters` 与空 `inputs: []`；参数格式由批准的业务 executor 定义。按[当前接入契约](integration-standard.md#当前-mvp-接入范围)查询、取消与跟踪状态。模型和输入必须预先在实际节点可达；当前没有跨节点模型/文件复制、任意远程命令或多 GPU 设备级调度。原生同步 permit 不会自动变成这个异步队列。

<a id="acceptance"></a>
## 上线验收

逐实例检查目录及可见性，真实请求成功且 permit 指向正确节点；资源不足、陈旧心跳和排空时不能到达模型执行端；正常终态释放、断连未知保持预约；重注册和 Agent 重启不重复启动计算。对外部独立资源策略，验证同一用户仍可路由且不产生本地 permit。资源页显示只能证明观测，不能替代真实模型、停止和故障恢复验收。

本页是接入操作说明，不会自动改变任何已安装设备的节点、预算、凭据或策略。

## 运行状态与错误处理

配置优先级、各类凭据边界及机器可读错误字段见[统一配置、身份与错误契约](api-contract.md)。统一模型仓库代理使用网关登录身份；原生模型管理端口仍为原 Model Hub 的受信任内网契约。

| 状态 | 含义与处理 |
|---|---|
| 注册 2xx | 目录受理；显式单模型注册不证明服务就绪，更不证明已经取得计算许可 |
| 推理 429 | 当前资源忙；`WAIT_STOP_CONFIRMATION` 表示有执行仍待停止核验；稍后重试，不能换入口绕过 |
| 推理 503 | 缺少绑定、逐模型批准或调度不可用；修复配置，不能回退直连同一计算 |
| 401 / 403 | 检查调用身份与操作权限；可见性设置不授予管理员权限 |
| 心跳 404 | 租约已过期、服务重启或身份不匹配；核对健康与 owner 后完整重注册 |
| 取消或客户端断开 | 请求取消不等于已停止；确认原生退出后再释放，保留已返回的业务输出 |

带 reasoning 的模型可能先产生思考内容；输出预算不足时可以返回空正文和 `finish_reason=length`。验收应同时检查正文、reasoning、结束原因与 usage，不能只凭空正文判定路由丢失内容。

试聊的“停止”会取消当前请求。调用方不得在首个流式响应字节后释放许可；已确认停止的执行才能恢复预算。异步计算的同一份资源由 task/permit 交接，不能独立重复预约。

## 协议参考与历史说明

本页维护操作步骤；[OpenAPI](../api/openapi-v1.json)、[调度接入标准](integration-standard.md)、[网关资源准入](resource-scheduling.md)维护字段、权限及停止契约。已发布版本的具体部署与真实验证记录见[验证记录](validation.md)。旧 vLLM 和独立 V-Router 接入地址保留兼容入口，新的接入流程统一从本文开始。

## 启动与故障定位

模型调用前可查询 `GET http://v-scheduler:8080/readyz`（宿主机 `http://<vos-host>:22888/readyz`）。它不需要认证，不会加载模型，返回内置后端的首次发现与健康状态。`/healthz` 仅代表进程存活。`MODEL_CATALOG_INITIALIZING` 是尚在发现，`UPSTREAM_UNAVAILABLE` 是已探测后端不可用，404 才表示当前健康目录找不到模型；已就绪的其他模型仍可调用。

`SCHEDULER_ADMISSION_TIMEOUT` 是控制器准入响应超时，推理尚未发出；429 的 `WAIT_MEMORY_HEADROOM` / `WAIT_STOP_CONFIRMATION` 则是资源或原生停止核验条件。排查时保留 `code` 和 `request_id`，不要把这些错误都归为“模型不存在”，也不要因传输超时重放可能已开始的推理。

<a id="inference-queue-residency"></a>
## 推理排队、模型驻留与服务关联

普通应用继续请求 `http://v-scheduler:8080/v1/*`（宿主机默认 `http://<vos-host>:22888/v1/*`），不要为同一次网关推理额外申请一份许可。未指定排队等待时间时，已验证用户或应用调用启用驻留的内置 Ollama，默认最多等待 30 秒；其他情况保持立即准入或返回 429。需要明确禁止排队时设置 `X-VScheduler-Queue-Wait-Seconds: 0`。

需要等待资源的客户端应携带自身 VOS 用户身份或独立应用推理凭据，设置以下请求头；内网匿名请求不能查看、管理或加入身份隔离的等待队列：

```http
X-VScheduler-Queue-Wait-Seconds: 120
X-VScheduler-Priority: interactive
X-VScheduler-Request-ID: unique-request-uuid
```

等待范围为 1..300 秒；显式 `0` 表示不排队。省略时，已验证用户或应用调用自带 Ollama 且启用了驻留，会自动有界等待最多 30 秒，以协调并发请求和模型切换；关闭驻留、匿名调用和其他后端保持立即准入或拒绝。交互请求使用 `interactive`，后台批量请求使用 `background`。每等待 30 秒增加一级优先级，同级按到达顺序；原生后台任务也参与等待老化，不强制抢占已经运行的计算。队列最多 64 条、每个身份最多 8 条，同时仍受网关活动连接和输入缓冲上限约束。

- `GET http://v-scheduler:8080/api/v1/inference-queue`：查看请求 ID、模型、位置、等待时长和等待原因；普通用户/应用仅看自己的请求，管理员可看全部。
- `DELETE http://v-scheduler:8080/api/v1/inference-queue/<request-id>`：取消尚未执行的请求。`execution_started: false` 才表示确认取消了等待项；`null` 表示登记了取消意图，不能据此认定已运行计算停止。取消与 POST 到达顺序相反时，也会按身份保存有界取消标记。
- `QUEUE_TIMEOUT` / `QUEUE_CANCELLED`：本次排队请求没有开始推理；`QUEUE_FULL` 表示等待容量已满；`QUEUE_ID_IN_USE` 表示同 ID 仍在等待。请求 ID 是排队关联标识，不是跨完成记录的推理幂等键。

等待不占资源预约；只有排到后且真实准入成功才执行一次原始请求。队列不保存推理正文或凭据，也不会在进程重启后重放请求。客户端总超时应覆盖排队时间、模型冷加载和生成时间；网络断开后不要盲目重放未知执行结果。试聊默认等待 120 秒，显示队列位置，「停止」会同时取消等待并中断本次传输。

### 驻留与自动卸载

管理员在「模型仓库 → 模型接入状态」设置空闲保留时间，默认 60 秒，范围 0..600；`0` 恢复每次调用后卸载。设置保存在本实例的网关数据目录，升级/重启保留，不修改共享模型权重或原仓库注册表。

```sh
# These are gateway management endpoints, not port 6005 model download APIs.
# Host equivalent: BASE=http://<vos-host>:22888
BASE=http://v-scheduler:8080
curl -fsS "$BASE/api/v1/model-residency" -H "Authorization: Bearer $VOS_ADMIN_TOKEN"
curl -fsS -X PUT "$BASE/api/v1/model-residency" \
  -H "Authorization: Bearer $VOS_ADMIN_TOKEN" -H 'Content-Type: application/json' \
  -d '{"idle_seconds":60}'
```

当前实现保守保留一份空闲驻留预约，不并发复用同一许可。自带三个 Ollama 中，同一模型的连续调用可复用已加载权重及其预约，控制器重新核验身份、执行器、节点和资源定义并轮换执行证明。驻留期间保留完整的保守计算/内存预算，界面中的活动许可可能代表空闲驻留而非正在生成。遇到另一个本地模型/应用的准入（包括旧应用直接请求调度 API、原生后台任务领取）、空闲到期或手动停止时，先向固定原生后端取得匹配模型和挑战值的停止证明，再释放资源。证明失败仍保持预约或进入停止待核验；不能用超时、租约到期或空的运行列表冒充停止。

内置 Ollama 服务卡片的「启动模型」通过同一排队和资源准入链路预热，「停止模型」通过原生停止核验释放驻留预约。保留时间为 0 时，应先启用驻留再手动启动模型。默认不打开安装时的三个预热开关；本功能不会自动下载权重。外部推理后端继续使用已声明的管理接口，其驻留由外部运行时负责。

### 权重、服务、路由和调度状态

「模型仓库 → 模型接入状态」与管理员 API `GET http://v-scheduler:8080/api/v1/model-lifecycle` 展示同一关联结果。Ollama 使用精确模型名关联；HF/MS 与其他后端使用显式映射，不按名称相似度猜测：

```json
{
  "metadata": {
    "model_bindings": {
      "backend-served-model-name": "hf://org/repository"
    }
  }
}
```

该字段附在[完整接入流程](#managed-inference)的推理服务管理注册体中；界面的「关联仓库模型」和「后端实际模型名称」提供相同设置。服务卡片、网关注册和资源批准仍各有职责，但状态页会统一显示它们是否齐备：

| 状态码 | 含义与处理 |
|---|---|
| `NO_LINKED_GATEWAY_SERVICE` | 尚未关联服务；先登记服务及精确权重映射 |
| `GATEWAY_MODEL_NOT_DISCOVERED` | 服务已关联，网关尚无对应模型；检查网关注册、模型 ID、能力及可见范围 |
| `UPSTREAM_UNAVAILABLE` | 已知后端不可达；检查服务健康和网络 |
| `EXECUTION_NODE_UNAVAILABLE` | 执行节点、心跳或执行器健康尚未就绪 |
| `ROUTING_PAUSED` | 路由暂停；检查上游路由设置 |
| `WEIGHTS_NOT_READY` | 权重下载、导入或校验尚未完成 |
| `SCHEDULER_BINDING_PENDING` | 缺少对应调度绑定或模型策略；完成资源批准及可信绑定 |

`configured_callable` 表示接入配置齐备，不承诺当前容量一定可用；真实资源仍在调用时原子检查。所选文件下载完成的 `READY` 仅代表所选范围，不意味着已下载完整仓库或已经部署推理后端。

内置 Ollama 权重更新和删除会先核验驻留卸载及未释放预约；有排队、执行或下载任务时返回 409，请在任务结束后重试。自动资源绑定按当前原生模型大小核对；未完成绑定返回 `503 MODEL_BINDING_PENDING`，不会沿用旧权重的预算。旧 Model Hub 或直接操作原生管理端口的写入方，应先停止该模型并确认卸载，再变更共享权重；进程内互斥不覆盖外部写入方。

应用身份以 `userinfo` 核验；正式入口为 `v-scheduler`，旧 host alias 和已保存的兼容地址继续有效；不能用普通用户或其他应用代替执行应用身份。

GPU 槽位是调度并发计数，不是独立显存。统一内存预算只计一份：Thor 的 SenseVoice、Kokoro 和 CAM++ 使用实测的 4 GiB 共享内存预算；较大语音与音乐模型使用各自校准。192 的 CPU 语音记录为 1.75 GiB、GPU 并发计数 0，不因统一内存就自动改为 GPU 执行。

网关的管理员权限通过同一 VOS 的 `user/check` 核验角色、启用及删除状态；仅凭用户名或普通用户令牌不能开启强制尝试。

语音绑定的恢复与安装使用数据库内当前已批准的节点预算，不再用旧启动文件二次否决。已落盘但尚未安装进内存的 revision 会自动恢复；预算缩减仍须重新通过当前节点校验，停止证明与原许可不变。
