# V-Scheduler 统一接入指南

本文是应用和模型服务的统一操作入口，覆盖原 V-Router 模型调用、V-Scheduler 调度接管、vLLM 主动注册，以及是否由本 Scheduler 调度。VOS 安装统一应用 `com.ictrek.v-scheduler`；网关、调度器和模型仓库均由该应用提供。

- [选择接入方式与调度策略](#integration-mode)
- [地址、端口与身份](#addresses-and-identity)
- [vLLM 主动注册、public/private 与生命周期](#vllm-registration)
- [启用本实例调度](#scheduled-vllm)
- [仅路由外部独立资源](#external-vllm)
- [同步许可与异步任务](#application-contract)
- [共享模型目录](#shared-models)
- [远端节点与当前能力边界](#remote-nodes)
- [上线验收](#acceptance)

<a id="integration-mode"></a>
## 选择接入方式与调度策略

| 需求 | 接入方式 | 谁负责资源 |
|---|---|---|
| 只调用统一目录里的模型 | 使用网关 `/v1/*`；普通调用方不另申请许可 | 网关执行该模型已批准的策略 |
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
| VOS 后端容器 | `http://v-scheduler-gateway:8080/v1` | `http://v-scheduler:38120/api/v1` |
| 同域前端 | `/api/com.ictrek.v-scheduler/v1` | `/api/com.ictrek.v-scheduler/scheduler/api/v1` |
| 局域网服务 | `http://<vos-host>:22888/v1` | `http://<vos-host>:22888/scheduler/api/v1` |

下文 `/tasks`、`/permits`、`/executors` 等短路径均相对于调度 API base。网关 `/api/v1/*` 保留模型与上游管理含义。旧网关别名和旧同域路径为已有客户端保留兼容；控制器直连地址是上表中的正式调度入口。

VOS 本机资源采集内置于应用，无需应用开发者部署独立 Agent；外部节点通过所属 VOS 应用发现和管理，复用其已有监控；Scheduler 预算规划不直接授予执行权限，远端调度控制待验证，见[VOS 节点管理](node-management.md)。用户无需 SSH 或另装 Agent。自动发现模型不等于批准计算，仍需实际峰值预算与执行器授权。


同一容器网络的首选网关别名为 `v-scheduler-gateway:8080`，避免与旧独立 Router 的 `v-router` 兼容别名混淆。调度经网关代理时使用 `http://v-scheduler-gateway:8080/scheduler/api/v1`。网关 `/api/v1/*` 是模型与上游管理，控制器直连 `/api/v1/*` 是调度管理；两者不可混用。

模型仓库内部入口为 `http://v-scheduler-model-hub:6005/api/v1`，默认宿主机入口为 `http://<vos-host>:36005/api/v1`。所有九项内外端口、兼容别名及安装参数见 [README 端口表](../README.md#host-alias内部端口与宿主机端口)。应用内「API 文档 → 应用接入」显示当前安装配置的实际端口。

| 凭据 | 用途 | 不能替代 |
|---|---|---|
| VOS 用户 Bearer / 管理员配置的网关 Key | 模型调用；有相应权限时注册服务 | 调度 submitter 凭据 |
| Scheduler 独立服务凭据 | 应用自己的任务、许可及批准的 executor | 浏览器用户身份、网关注册身份 |
| vLLM 自身 API Key | 网关访问上游 vLLM，放入受保护注册配置的 `api_key` | 网关调用者凭据 |
| 专用后台推理身份 | 如 HybRAG 的受限后台模型调用 | 注册、网关管理、调度管理员权限 |

不同应用使用各自的服务身份；私钥不进入浏览器、discovery、共享模型目录或日志。无效 Bearer 返回 401，不能填写任意占位 Key。注册 `scheduler` 策略需要管理员权限；普通身份提交该字段返回 403。私有模型由注册请求的真实身份决定 owner；系统级管理员注册器不能代替另一个用户创建其私有身份。后台可信交接见 [HybRAG 身份参考](hybrag-gateway-identity.md)。

<a id="vllm-registration"></a>
## vLLM 主动注册与生命周期

下文网关根地址为 `http://<vos-host>:22888`，调度根地址为它的 `/scheduler/api/v1`。VOS 容器内分别使用 `http://v-scheduler-gateway:8080` 和 `http://v-scheduler:38120/api/v1`。跨不可信网络使用已验证证书的 HTTPS 入口；不要用 `curl -k`。

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

自动探测多个模型时，可用 `model_endpoints`、`model_capabilities` 按原生模型 ID 覆盖各模型的接口和能力；也支持用 `models` 显式登记多个模型。只为真实支持工具调用的模型声明 `tools`。字段定义见[注册 API](../gateway/internal/api/routing.go)。

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
流程、校准清单、持久 revision 与凭据轮换见 [V-Voice 主动接入](v-voice-auto-binding.md)。

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
