# vLLM 推理实例与外部算力接入（0.0.20）

统一应用同时提供模型网关和资源控制器。**模型注册只把 API 加入目录；节点纳管才提供资源观测和预约。** 本文针对当前 0.0.20 实现，适用于已经提供兼容模型 API 的 vLLM 等服务，不负责安装模型或替它实现执行契约。

| 需求 | 当前接法 | 资源责任 |
|---|---|---|
| 经统一模型入口调用，并让 Scheduler 准入 | 由所属 VOS App 完成受控接入 + 批准真实节点、executor 和逐模型策略 | 网关在实际节点申请 permit；同步请求不进入持久任务队列，忙时返回 429 |
| 经统一模型入口调用，不让本 Scheduler 调度 | 管理员明确批准该服务为外部独立资源，注册时携带 `scheduler.mode=cloud` | 外部运行方负责容量、并发与停止，不计入本 Scheduler 预约 |
| 直接调用模型服务 | 使用服务自身 API；无需注册 | 完全由模型运行方管理，不能宣称受本 Scheduler 调度 |
| 只观察其他机器资源 | 在 VOS 安装本应用，复用所属应用已有监控 | 显示测量，不启动任务 |
| 调度其他机器的异步任务 | 在资源纳管之上增加业务 executor 适配器并提交 `/tasks` | Scheduler 排队，节点 Agent 驱动执行、续租与停止确认 |

`cloud` 是现有协议中的外部资源策略名。远程自建 vLLM 也必须由管理员确认其资源独立、由其他运行方管理，才能用这项显式豁免；远端 URL、公开模型或 `visibility=public` 都不会自动豁免。将本机或同一纳管资源池伪装成外部资源，会使预算失真。

## 1. 注册 vLLM 的模型 API

下文网关根地址为 `http://<vos-host>:22888`，调度根地址为它的 `/scheduler/api/v1`。VOS 容器内分别使用 `http://v-router:8080` 和 `http://v-router:8080/scheduler/api/v1`。跨不可信网络使用已验证证书的 HTTPS 入口；不要用 `curl -k`。

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

这是显式单模型注册，不探测 `/models`，成功也不证明模型就绪。需要自动读取模型列表时删除 `model`，由注册流程探测 `base_url/models`；仍应声明真实支持的 endpoints。Embedding 实例使用 `model_kind=embedding`、`endpoints=["embeddings"]` 和 `capabilities=["embedding"]`。不要为不支持的接口注册能力。

返回的 upstream ID 是 `registered-vllm-node-a-1`。控制面 `GET /api/v1/models` 返回每个模型的 `qualified`，本例为 `registered/demo-chat`；私有模型仅对所属身份与管理员可见。不同副本使用不同 `service_id`；相同模型可能有多个候选副本，因此**每个副本都要批准其真实节点与预算**。逐模型策略键使用 `registered-vllm-node-a-1/demo-chat`，不使用客户端的 `registered/demo-chat`。

进程存活期间每约 30 秒续期；注册成功返回后再开始心跳：

```bash
curl --fail-with-body -X POST \
  "$GATEWAY/api/v1/registrations/registered-vllm-node-a-1/heartbeat" \
  -H "Authorization: Bearer $GATEWAY_TOKEN" \
  -H 'Content-Type: application/json' -d '{"ttl_seconds":90}'
```

TTL 支持 30–3600 秒，默认 90 秒。心跳返回 404 时重新执行注册；网关重启或租约过期均可能需要重新注册。心跳不刷新模型列表，模型变化时重新注册。注册循环应保留所选调度策略，避免后续重注册丢失管理员批准的外部资源声明。退出时可 `DELETE /api/v1/registrations/registered-vllm-node-a-1`，退出新请求分配，已开始请求继续完成。

## 2. 让 Scheduler 调度该推理实例

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

`app_id` 应匹配网关 submitter 的 app 身份；示例为 `v-router`。`upstream_urls` 与注册的实际 base URL 必须匹配。模型权重、KV cache 等常驻分配写入节点 `resident_baseline`；executor 写单次允许计算的增量峰值。系统余量写入 `safety_reserve`。不要只按模型文件大小估显存。

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

网关申请 permit、续租并转发；完整同步/SSE 终态后提供停止证据再释放。资源不足返回 429，模型未批准或控制器不可用返回 503；不会改为直连。断开、超时或停止未知时可能保留 suspect 预约，须核验真实运行后处理。

**原生 vLLM 模型 API 不等于 Scheduler executor 适配器。** 本接法先提供网关侧准入；若还要保证所有直连入口、断连停止和租约失效后的执行约束，必须限制绕过入口，并由服务包装层实现许可核验、幂等 execution_id、硬限额和停止机制。许可头与 `/permits/{id}/verify` 见[网关资源准入](resource-scheduling.md#同步执行端交接)。不要将普通 vLLM 标为 `delegated`；它没有因此创建持久 Scheduler task，也不会自动获得 start/inspect/cancel。仅注册 vLLM URL 不能实现模型自动装卸、任务迁移或抢占。

## 3. 只路由模型，不让本 Scheduler 调度

管理员确认它属于独立的外部资源后，在第 1 节的注册 JSON 加入：

```json
{
  "scheduler": {"mode": "cloud"}
}
```

这是要合并的字段，不能单独作为注册请求。使用管理员网关身份重注册完整实例；普通身份提交 `scheduler` 字段返回 403。服务运行期的重注册也必须保留这份批准（应由受控管理员注册代理持有管理员身份，不要把它发给模型用户）。服务端把批准绑定到本次 `base_url`，客户端无需伪造 `upstream_url`。

此分支在加载本地调度配置前放行，不创建本地 permit，即使调度控制器暂不可用也可路由。网关用户鉴权、私有模型可见性和上游 API Key 仍有效。地址变化需要重新批准；重定向离开批准地址不能继承批准。私网地址也需明确的这项管理员声明，不能仅改成 cloud 类型就假定豁免。

若希望完全自行管理且不使用统一模型目录，直接访问原服务即可，不需要关闭统一应用的 `V_ROUTER_SCHEDULER_REQUIRED`。这个全局开关不能作为单个实例的豁免方式。

## 4. 通过 VOS App 管理外部节点

用户只通过 VOS 安装和管理应用。0.0.20 自动读取已安装 ModelInfer 的远端节点清单与现有监控，不要求登录远端机器或运行第二套 Agent。页面提供“预算规划”和所属应用管理入口；规划不会授予执行权限，也不会改变已有推理部署。

多机 vLLM 的真实执行、停止与故障恢复契约尚待 VOS App 管理适配器验证。已预留来源规划与控制接口；控制请求当前返回 501，并保持不可调度。待管理链路恢复后由应用实现方验证，用户不承担手工部署工作。详情见[节点管理](node-management.md)。

底层 `model.infer` 的主节点和 `placements` 协议可原子预约多个独立资源池，已用隔离协议环境验证；这不等于已接管 ModelInfer 的多机推理。观察节点不能作为执行器位置，未经验证不能把它们伪装成已纳管资源。

## 5. 在外部节点执行异步业务任务

本节仅面向业务 App 的集成开发；远端 VOS 管理适配器完成验证后才能启用，不是用户安装流程。中心增加业务 submitter 和它所属的 executor（`task_type` 使用真实业务类型），executor 绑定 `node_id=compute-a`；节点 Agent 的 `executors` 增加同 ID 的 `{id,url,health_url,token}` 适配器。URL 指向实现当前 start/inspect/cancel、持久句柄、fence、租约 watchdog 与停止证据的业务服务，**不能填写 vLLM `/v1` 作为这种适配器地址**。完整字段示例见 [controller](../config/controller.example.json) 与 [Agent](../config/agent.example.json)。

业务应用先持久保存自己的任务和待提交记录，再用自己的 submitter 令牌提交 `POST /tasks`，携带稳定 `Idempotency-Key`。当前 body 使用 `protocol_version`、`app_task_id`、`executor`、`parameters` 与空 `inputs: []`；参数格式由批准的业务 executor 定义。按[当前接入契约](integration-standard.md#当前-mvp-接入范围)查询、取消与跟踪状态。模型和输入必须预先在实际节点可达；当前没有跨节点模型/文件复制、任意远程命令或多 GPU 设备级调度。原生同步 permit 不会自动变成这个异步队列。

## 6. 上线验收

逐实例检查目录及可见性，真实请求成功且 permit 指向正确节点；资源不足、陈旧心跳和排空时不能到达模型执行端；正常终态释放、断连未知保持预约；重注册和 Agent 重启不重复启动计算。对外部独立资源策略，验证同一用户仍可路由且不产生本地 permit。资源页显示只能证明观测，不能替代真实模型、停止和故障恢复验收。

本页是接入操作说明，不会自动改变任何已安装设备的节点、预算、凭据或策略。
