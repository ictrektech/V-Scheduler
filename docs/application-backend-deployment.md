# 应用后端部署 / Application backend deployments

## 中文

在 V-Scheduler 打开「API 文档 → 调度管理 API → 应用后端部署」。应用继续管理自己的镜像清单和后端设置。V-Scheduler 校验部署权限，按节点分配宿主机端口，并跟踪部署生命周期。应用可以用同一套 API 指定 SSH 节点或集群成员。

1. 管理员在「节点管理 → SSH 算力节点」登记可访问 Docker 的节点，或在独立的 VOS 集群管理应用中登记受信集群成员。节点登记后对所有已认证的 VOS 应用可见。部署目标须具备 IPv4 地址和固定硬件 Profile。集群对端须运行兼容的 V-Scheduler，开启集群管理并具备平台 Projects 权限。
2. 应用用服务凭据调用 `GET /managed-nodes` 读取全部节点目录。通过 `GET /backend-deployment-access/{app_id}` 读取其当前匹配的目标与发布镜像。自动记录返回 `automatic: true`、`targets`、`repositories` 和按固定硬件 Profile 索引的 `images`。已安装应用在 Compose 顶层 `x-v-scheduler-backend-images` 声明单行 JSON 清单。应用直接选择匹配的目标，无需逐节点放行。
3. 应用以自己的服务凭据调用 `POST /backend-deployments`，提交稳定的 `request_key`、`target`、与目标 Profile 的发布清单完全匹配、带 tag 或 SHA-256 摘要的 `image`，以及 `container_port`（1 至 65535）。请求在数据库中入队。使用同一个键和相同参数可安全重试。不同参数复用同一键会产生冲突。
4. 轮询 `GET /backend-deployments/{id}`。状态为 `running` 且 `endpoint` 有值后，应用的后端可用此地址连接。建议保留应用原有的应用层健康检查或 HTTPS 代理。端口在每个控制器的目标 IPv4 上从 42000–42999 分配。应为部署保留该范围，避免与其他服务或控制器争用。只能映射一个 TCP 端口。
5. `POST /backend-deployments/{id}/stop` 异步清理容器或 VOS Project，保留端口。使用 `POST /backend-deployments/{id}/start` 以原参数重试失败或已停止实例。调用 `DELETE /backend-deployments/{id}`，确认进入 `removed` 后释放端口。管理员先删除该节点的后端部署，再删除 SSH 登记或修改其地址。

SSH 路径按 Profile 选择 Docker 运行时。集群路径使用 V-Scheduler 安装包的 `scheduler-app-backend` 项目。同一台机器按 IPv4 共享本控制器的宿主机端口分配。Jetson Orin、Jetson Thor、DGX Spark 及 ARM/AMD CUDA 使用 NVIDIA 容器运行时，其他 Profile 使用默认 Docker 运行时。接口不接收任意命令、环境变量、挂载、主机设备或特权参数。需自定义设备或卷的后端须通过各自的应用部署方案配置。

SSH 节点已有匹配架构的镜像时直接使用。需要拉取私有镜像时，管理员可在「镜像仓库 → 设置 → 上游认证」配置仓库的拉取凭据。Scheduler 通过 SSH 将匹配的凭据放入目标节点的临时 Docker 配置，拉取后清理。目标节点也可使用自己的现有 Docker 认证。目标拉取失败时，主节点可从上游取目标架构的带标签镜像，生成归档并通过 SSH 流式导入目标 Docker。主、从节点的 CPU 架构可以不同，归档不在主节点启动。准备镜像期间仍为 `pending`。摘要引用由节点直拉。集群成员仍通过其本地 Projects 拉取镜像。

在「节点管理 → SSH 算力节点」添加或编辑节点时，用「应用后端的 Model Hub 权重」选择优先使用只读 NFS 或直接复制。管理员可在 `POST /managed-nodes` 或 `PUT /managed-nodes/{id}` 发送布尔字段 `app_model_hub_nfs`。创建时默认 `true`。编辑时省略或传 `null` 保留设置。管理员从节点列表与写入回执读取该字段。关闭后，新的模型准备立即走复制。已有后端停止并重新启动后更新挂载，部署持久卷保留。原先走共享的模型需要重新准备。

删除单个 Image Studio 远端模型时，使用 `DELETE /backend-deployments/{id}/models/{model_id}`。该操作要求部署所属应用或管理员身份及支持删除的 Worker。存在活跃任务时返回 `BACKEND_TASKS_ACTIVE`。清理失败后可重试同一请求，期间该模型停止新任务准入。成功回执的 `data.deleted_transfers` 为本次移除的目标传输记录数。

### 请求示例

VOS 基址为 `/api/com.ictrek.v-scheduler/scheduler/api/v1`。容器网络直连为 `http://v-scheduler:38120/api/v1`。传入 `Authorization: Bearer <token>`。JSON 请求使用 `Content-Type: application/json`。

```yaml
x-v-scheduler-backend-images: {"jetson-thor":"registry.example.com/demo/backend:1.0"}
```

```json
{
  "request_key":"backend-1",
  "target":{"kind":"ssh","id":"node-35","profile":"jetson-thor"},
  "image":"registry.example.com/demo/backend:1.0",
  "container_port":8080
}
```

HTTP 200 返回 `code=OK` 以及 `data`。获取部署列表时 `data.items` 最多包含最近 200 条记录。应用只能查看和操作自己的实例，管理员可查看或清理各应用。自动部署选项和请求字段的完整协议见应用内 API 文档。

| 状态 | 含义与操作 |
| --- | --- |
| `pending` | 已排队、正在准备镜像或正在启动，继续轮询。 |
| `running` | 运行时报告实例正在运行。最近状态检查失败时 `endpoint` 为 null。 |
| `failed` | 查看 `last_error`，修正镜像或节点问题后重启。 |
| `stopping` / `deleting` | 等待运行时确认清理。 |
| `stopped` | 实例已退出或清理，保留端口供重新启动。 |
| `removed` | 确认清理并释放端口，保留幂等记录。重新部署时使用新的键。 |
| `cleanup_failed` | 清理未确认，检查节点并重试停止或删除。端口保持预留。 |

当创建接口返回 409，按业务码检查 Profile、幂等键或端口范围。操作处理中返回 `BACKEND_BUSY` 时，先轮询状态再重试。返回 503 时检查目标节点可达性或调度器数据库。若 `last_error` 为 `BACKEND_START_FAILED`，检查镜像拉取、宿主机端口与目标运行时；若为 `BACKEND_RUNTIME_UNAVAILABLE`，等待下一次状态检测并排查对端连接。

## English

Open **API Documentation → Scheduling API → Application backend deployments** in V-Scheduler. The application keeps its image list and selects the backend to deploy. V-Scheduler authorizes deployment, assigns a host port, and tracks the instance lifecycle through the same API for both node types.

1. An administrator adds a Docker-capable SSH node in **Node Management → SSH compute nodes** or adds a trusted member in the standalone **VOS Cluster Manager** app. Registered nodes are visible to every authenticated VOS app. A target needs an IPv4 address and a fixed hardware Profile. Cluster targets require a compatible V-Scheduler, the Cluster Manager and VOS Projects access.
2. Call `GET /managed-nodes` with the app’s service credential to read the full inventory. Use `GET /backend-deployment-access/{app_id}` for the current matching targets and published images. Automatic records return `automatic: true`, `targets`, `repositories`, and an `images` map keyed by fixed hardware Profile. The installed Compose file declares the image catalog as single-line JSON under the top-level `x-v-scheduler-backend-images` field. The app selects and deploys to a matching node directly.
3. The application calls `POST /backend-deployments` with its service credential, a stable `request_key`, the selected `target`, the exact published `image` for that Profile including its tag or SHA-256 digest, and `container_port` (1 through 65535). The response records the queued request. Replay the same input with the same key to recover that record. Changed input with the same key returns a conflict.
4. Poll `GET /backend-deployments/{id}`. When its state is `running` and it provides an `endpoint`, the application’s backend can connect to that address. Keep the application’s HTTP health check or HTTPS proxy when needed. Ports 42000–42999 are reserved per target IPv4 address and controller database. Reserve that range for these deployments to avoid competing services or controllers. Each instance exposes one TCP port.
5. Use `POST /backend-deployments/{id}/stop` to remove the runtime and keep the reservation. Use `POST /backend-deployments/{id}/start` to retry a stopped or failed deployment with its original configuration. Use `DELETE /backend-deployments/{id}` to release the port after cleanup is confirmed. Remove the deployments before deleting their SSH registration or changing its address.

The SSH adapter selects the Docker runtime from the Profile. The cluster adapter uses the V-Scheduler package’s `scheduler-app-backend` project. The same IPv4 address shares one port pool within the controller. Jetson Orin, Jetson Thor, DGX Spark, ARM CUDA and AMD CUDA select the NVIDIA runtime. Other profiles use the default Docker runtime. The API accepts an image and one port. Backends that need custom commands, environment variables, host mounts, devices or privileges require their own deployment configuration.

SSH nodes reuse images that are already present for the target architecture. For private images, an administrator can configure pull credentials in **Image Hub → Settings → Upstream authentication**. Scheduler supplies only the matching credential through a temporary Docker config on the target and removes it after the pull. The node can also use its own Docker authentication. If the target cannot pull a tagged image, the controller can fetch the target platform and stream an archive through SSH into the node's Docker daemon. The controller and target may use different CPU architectures. The deployment stays `pending` until image preparation and startup finish. Digest references use direct pulls. Cluster members continue to pull through their local Projects service.

Under **Node Management → SSH compute nodes**, choose read-only NFS or direct copying in **Model Hub weights for application backends** when adding or editing a node. Administrators send the boolean `app_model_hub_nfs` in `POST /managed-nodes` or `PUT /managed-nodes/{id}`. It defaults to `true` on creation. An omitted or null field preserves the setting on update. Administrators can read it in the inventory and write response. Disabling NFS applies to new model preparations immediately. Stop and start an existing backend to update its mount. Its persistent volume is retained. Prepare models that used the old share again.

To remove one Image Studio model from a compute node, call `DELETE /backend-deployments/{id}/models/{model_id}` as the owning app or an administrator. The worker must support model deletion. Active tasks return `BACKEND_TASKS_ACTIVE`. Pending cleanup blocks new admissions for the model. Retry the same request after a cleanup failure. On success, `data.deleted_transfers` is the number of target transfer records removed in this attempt.

### API usage

Use `/api/com.ictrek.v-scheduler/scheduler/api/v1` under VOS or `http://v-scheduler:38120/api/v1` on the app network. Send `Authorization: Bearer <token>` and use `Content-Type: application/json` for JSON request bodies. The JSON examples in the Chinese section use the same protocol fields.

Successful requests return HTTP 200, `code=OK` and `data`. List responses return up to 200 recent deployments in `data.items`. Applications can read and manage their own deployments. Administrators can inspect and remove deployments across applications. Refer to the in-app API reference for request fields, full response schemas and error codes.

| State | Meaning and action |
| --- | --- |
| `pending` | Queued, preparing the image, or starting. Continue polling. |
| `running` | The runtime reports the instance running. A failed status check clears `endpoint` until recovery. |
| `failed` | Check `last_error`, then correct the image or target and retry. |
| `stopping` / `deleting` | Wait for confirmation from the runtime. |
| `stopped` | The runtime has exited or been removed. Its port is retained for restart. |
| `removed` | Removal confirmed and port released. The original request key still identifies this record. Use a new key for a new deployment. |
| `cleanup_failed` | Cleanup could not be confirmed. The port remains reserved. Check the target and retry stop or removal. |

Use the business code on HTTP 409 to diagnose a Profile mismatch, conflicting request key, exhausted port pool or a busy lifecycle operation. When a request returns 503, check target or database availability. For `BACKEND_START_FAILED`, check the image pull, target runtime and host port. For `BACKEND_RUNTIME_UNAVAILABLE`, check the peer connection and wait for the next status probe.

## 通用 worker 接入 / Generic worker integration

应用在已安装的 VOS Compose 中用 `x-v-scheduler-backend-images` 声明各目标 Profile 的发布镜像。创建部署时发送 `config: {"mode":"application-backend-v1"}`。Scheduler 校验镜像与目标 Profile 后，为实例创建私有通道，并把凭据注入 `V_SCHEDULER_BACKEND_CHANNEL_TOKEN`。Worker 必须对每个 `/scheduler/*` 路由核验该 Bearer 凭据。凭据由节点管理服务转发，部署查询和浏览器响应均不返回。应用读取 `V_SCHEDULER_BACKEND_CONFIG_B64`，按需要初始化自己的运行时。现有 `image-studio-worker-v1` 部署继续使用原协议。

Declare the published image for each fixed hardware Profile in the installed VOS Compose file using `x-v-scheduler-backend-images`. Include `config: {"mode":"application-backend-v1"}` when creating the deployment. Scheduler checks the exact published image and target Profile, then injects the deployment channel credential as `V_SCHEDULER_BACKEND_CHANNEL_TOKEN`. The worker must verify it as a bearer credential on each `/scheduler/*` route. Deployment queries and browser responses never return it. Decode `V_SCHEDULER_BACKEND_CONFIG_B64` to initialize app-specific settings. Existing `image-studio-worker-v1` deployments retain their original contract.

1. **准备模型 / Prepare model.** 通过[权重传输协议](model-weight-transfer.md)准备并校验文件。Worker 报告 `GET /scheduler/capabilities`，返回 `protocol_version: 1`、运行设置 `execution_settings` 对象、`async_models: [{"id":"<model-id>","task_type":"<application.task>"}]`，支持删除时返回 `weight_model_delete: true`。 / Use the [weight transfer protocol](model-weight-transfer.md). Implement `GET /scheduler/capabilities` with `protocol_version: 1`, an `execution_settings` object, `async_models: [{"id":"<model-id>","task_type":"<application.task>"}]`, and `weight_model_delete: true` when supported.
2. **登记 / Register.** 权重校验后，用应用服务身份提交 `POST /backend-deployments/{id}/models`，JSON 含 `model_id` 和 `transfer_id`。目标须有最新心跳，Worker 须唯一声明该模型。匹配的实测校准可覆盖预算和参数。无需校准也可登记，预算采用部署 CPU 和内存限额，未配置时分别以 4 核、4096 MiB 为上限并受节点静态预算约束。GPU Profile 预约一个 GPU 槽。Worker 可在该模型下提供 `model_limits` 对象。直接登记返回 `model_parameters: {"policy":"direct"}`。 / After verifying weights, call the registration endpoint with the owning app identity and the model and transfer IDs. The target needs a fresh heartbeat and a unique worker capability for the model. A matching measurement can override the budget and parameters. Otherwise the deployment CPU and memory limits apply, or up to four cores and 4096 MiB within the node's static budget. GPU profiles reserve one GPU slot. The worker can declare a `model_limits` object. Registration without a calibration returns `model_parameters: {"policy":"direct"}`.
3. **列出节点并提交 / List placements and submit.** 调用 `GET /backend-deployments/{id}/models` 获取已登记的模型、节点和执行器。在应用中按 `node_id` 展示后端选项，并把选中条目的 `executor_id` 用在 `POST /tasks` 的 `executor.id`。任务归应用的服务身份所有。提交成功返回 HTTP 202 和 `queued`。目标暂时资源不足或同一 Worker 仍在处理任务时会继续等待；可查任务状态或取消，始终使用已选节点。 / Read `GET /backend-deployments/{id}/models`, show the available options by `node_id`, and pass the selected row's `executor_id` as `executor.id` to `POST /tasks` using the owning app's service identity. New tasks return HTTP 202 and `queued`. Temporary resource pressure or another active task on the worker keeps the request queued on the selected node. Poll or cancel using the task ID.
4. **执行与结束 / Execute and confirm stop.** Worker 在 `POST /scheduler/executions/{attempt_id}/{action}` 实现 `start`、`inspect`、`cancel`。每次接收 `task_id`、`executor_id`、`fence`、`lease_seconds`、`parameters`、`preempted`，启动另有 `model_id`、`transfer_id`、`model_limits`、`model_parameters`。以 attempt 和 fence 防重，遵循租约有效期，确认计算停止后才报告 `stopped: true`。输出和业务状态归应用。 / Implement the three actions on the same route. Apply attempt and fence checks, honor the renewable lease, and report `stopped: true` only when compute has stopped. The application owns outputs and business state.

随包提供的首个 V-Voice 远端校准适用于 Jetson Orin 上的 `sensevoice-int8` CUDA 整句转写，使用 4 线程，文件上限 2 MiB，源采样率上限 48 kHz，时长上限 20 秒。未匹配校准时，V-Voice Worker 声明相同的输入边界。 / The first bundled remote V-Voice calibration covers one-shot CUDA SenseVoice on Jetson Orin with four threads, a 2 MiB input limit, up to 48 kHz source audio and 20 seconds. The V-Voice worker declares the same input bounds when no calibration matches.

当前通用执行协议覆盖异步任务。任务 parameters 序列化后最多 32 KiB，校准的 model_limits 与 model_parameters 合计最多 16 KiB。媒体输入使用分块通道。实时 WebSocket 和同步在线许可有各自生命周期。 / The generic execution protocol covers asynchronous jobs. Serialized task parameters are limited to 32 KiB. The calibrated model_limits and model_parameters together have a 16 KiB limit. Use the chunk input channel for media. Real-time WebSocket and synchronous permits have separate lifecycles.

## 应用内接入流程 / Application-owned integration

应用的部署页面调用本应用后端。本应用后端持有自己的 Scheduler 服务身份，按下表完成节点发现、部署、权重准备和模型登记。用户在该应用的业务页选择模型与执行节点。V-Scheduler 提供机制和资源管理，应用决定模型目录、支持的节点与 Profile、操作界面和任务参数。

The application's deployment UI calls its own backend. That backend uses its Scheduler service identity to discover nodes, deploy, prepare weights and register models. The application presents the model and execution node in its own workflow. V-Scheduler provides orchestration and resource management. Each application owns its model catalog, supported Profiles, task parameters and user interface.

| 步骤 / Step | 应用服务端调用 / Calls from the application backend |
| --- | --- |
| 发现 / Discover | `GET /managed-nodes` 读取节点名称和 Profile。以 `GET /backend-deployment-access/{app_id}` 的 `targets`、`images` 为准，向管理员显示本应用获准使用的节点及其发布镜像。 / Read node labels and Profiles, and intersect them with the application's permitted targets and release images. |
| 部署 / Deploy | `POST /backend-deployments` 提交稳定的 `request_key`、`target.kind`、`target.id`、`target.profile`、该 Profile 的 `image`、`container_port` 和 `config.mode=application-backend-v1`。保留返回的 `id`，轮询 `GET /backend-deployments/{id}` 到 `running`，按 `last_error` 提示恢复操作。 / Submit the chosen target, published image and stable request key. Poll deployment status and show actionable errors. |
| 校验权重 / Verify weights | 按[权重传输协议](model-weight-transfer.md)提交清单、传输或引用只读共享文件、完成校验，取得 `transfer_id`。 / Complete the weights protocol with a verified transfer ID. |
| 登记 / Register | `POST /backend-deployments/{id}/models` 提交 `model_id` 与 `transfer_id`。`GET /backend-deployments/{id}/models` 返回可选部署、模型、`node_id`、`executor_id`、`profile` 和 `task_type`。 / Register the verified model and use the list to populate model and node choices. |
| 执行 / Execute | `POST /tasks` 使用所选 `executor.id` 和 Worker 声明的类型。通过 `GET /tasks/{id}` 查询进度、结果和实际 `node_id`、`executor_id`。若任务未结束，可请求 `POST /tasks/{id}/cancel`。 / Submit to the selected executor, poll the task and compare the actual node and executor with the chosen placement. |

使用 VOS 同域前缀 `/api/com.ictrek.v-scheduler/scheduler/api/v1`，服务容器可使用 `http://v-scheduler:8080/scheduler/api/v1`。浏览器使用 VOS 用户身份调用所属应用。后端从所属应用私有存储读取 Scheduler 服务凭据。管理员用 `POST /service-identities` 为对应 `app_id` 发放服务身份。服务身份只操作本应用的部署和模型登记。Worker 内部通道另用 Scheduler 注入的令牌。

Use `/api/com.ictrek.v-scheduler/scheduler/api/v1` under VOS or `http://v-scheduler:8080/scheduler/api/v1` from a service container. The browser uses VOS user identity with its application. The backend uses its own Scheduler service identity from private storage. Administrators issue it with `POST /service-identities` for that `app_id`. Operations are scoped to the owning application. The private Worker channel uses a separate injected token.

容器运行中仍可能收到 `409 BACKEND_EXECUTOR_TARGET_NOT_READY`。登记需要当前目标关联唯一的已批准计算 Agent，使用 `GET /nodes` 核对心跳和 `report.application_backend_dispatch=true`。SSH 目标通过 `POST /enrollments` 的 `discovery_source=modelinfer` 与 `discovery_id=<target.id>` 关联；集群目标使用关联同一 `vos_member_id` 的 Agent。完成[节点注册和审批](application-integration.md)后，可用相同 `transfer_id` 重试登记。

A running container can still return `409 BACKEND_EXECUTOR_TARGET_NOT_READY`. Registration needs one approved compute agent linked to the target, with a fresh heartbeat and `report.application_backend_dispatch=true` in `GET /nodes`. For SSH, link the invitation using `discovery_source=modelinfer` and `discovery_id=<target.id>` in `POST /enrollments`. For a cluster target, match its `vos_member_id`. Complete [node enrollment and approval](application-integration.md), then retry with the same transfer ID.

## 在调度界面部署 / Deploy from the Scheduler workspace

管理员打开「节点管理 → 应用后端」。可用节点区显示主机名、IP、节点类型和 Profile。「部署后端」在选定应用和兼容节点后显示对应发布镜像。已有实例可以启动、停止或删除。

Administrators can open **Node Management → Application backends**. The inventory shows hostnames, IP addresses, target types, and Profiles. **Deploy backend** selects the published image for the chosen app and compatible node. Start, stop, or remove existing deployments on the same page.

### 镜像准备示例 / Image preparation example

同一个后端可分别发布 `registry.example.com/team/backend:jetson-orin-v1`、`:jetson-thor-v1`、`:arm-cpu-v1` 和 `:amd-cuda-v1`。Orin、Thor 和 ARM CPU 镜像使用 `linux/arm64`，AMD CUDA 镜像使用 `linux/amd64`。GPU 镜像还需验证目标节点的驱动和运行时。后端监听 `0.0.0.0:8080`，部署请求填入实际监听端口。生产可使用带 SHA-256 摘要的引用，以及已在目标 Profile 验证的镜像。

Publish separate tags such as `registry.example.com/team/backend:jetson-orin-v1`, `:jetson-thor-v1`, `:arm-cpu-v1` and `:amd-cuda-v1` for the same backend. Build the Orin, Thor and ARM CPU images for `linux/arm64` and the AMD CUDA image for `linux/amd64`. Verify driver and runtime compatibility for GPU images. Run the application on `0.0.0.0:8080` in this example and submit the actual listening port. For production, use a SHA-256 digest and images verified on their target profile.

```yaml
x-v-scheduler-backend-images: {"jetson-orin":"registry.example.com/team/backend:jetson-orin-v1","jetson-thor":"registry.example.com/team/backend:jetson-thor-v1"}
```

```json
{
  "request_key":"worker-on-thor",
  "target":{"kind":"cluster","id":"member-thor","profile":"jetson-thor"},
  "image":"registry.example.com/team/backend:jetson-thor-v1",
  "container_port":8080,
  "host_port":42080,
  "cpu_millicores":1500,
  "memory_mib":2048,
  "config":{"workers":2,"batch_size":4}
}
```

部署到 SSH 时，将 `target` 改为 `{"kind":"ssh","id":"node-orin","profile":"jetson-orin"}`，将镜像标签改为相应的 Orin 版，并使用新的 `request_key`。省略 `host_port` 可自动分配。CPU 和内存限制为可选项，分别以 mCPU 和 MiB 表示。应用镜像把 `V_SCHEDULER_BACKEND_CONFIG_B64` 解码为 UTF-8 JSON 后读取自定义字段。使用应用自己的机密管理渠道传递凭据。

For SSH, use `{"kind":"ssh","id":"node-orin","profile":"jetson-orin"}`, select the Orin image tag, and supply a new `request_key`. Omit `host_port` for automatic allocation. CPU and memory caps are optional and measured in mCPU and MiB. Decode `V_SCHEDULER_BACKEND_CONFIG_B64` as Base64-encoded UTF-8 JSON to read application settings. Use the application's secret management mechanism for credentials.
