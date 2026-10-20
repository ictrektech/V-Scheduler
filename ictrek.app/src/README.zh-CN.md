# V-Scheduler 资源调度

新增「推理等待队列」及「模型仓库 → 模型接入状态」。试聊默认有界等待 120 秒，支持查看位置和取消；应用可通过网关排队请求头选择交互或后台优先级。自带 Ollama 默认空闲保留 60 秒，同模型连续调用可复用；保留期间继续占完整资源预约，切换、到期或手动停止后先确认卸载再释放。管理员可设置 0..600 秒，0 为每次卸载。服务注册支持 metadata.model_bindings 精确关联仓库权重与后端模型名称，状态页统一显示路由、后端健康和调度绑定。现有模型目录、九组端口、host alias 和四个容器保持兼容。

统一包包含模型网关与调度控制器。模型调用使用宿主端口 22888 的 /v1；调度管理使用 /scheduler/api/v1。Router 直接连接应用自带的三个 Ollama 后端，不依赖 Model Hub 发现；同时自动发现 Image Studio、V-Voice、V-Music、ML Server 和视频结构化服务的可用模型。V-Voice 就绪后主动接入，自动绑定已校准的执行器与模型映射；详见界面的“应用接入”。

版本 __APP_VERSION__。

安装时会检查 PGV 依赖。首次打开应用并以 VOS 管理员登录后，自动创建专用数据库、普通数据库角色和本机资源采集身份；配置保存在应用专属 ${VOS_APP_STORAGE_PATH}/config 中。无需准备配置文件或执行终端命令。PGV 不可达时页面显示初始化状态。

管理界面支持按节点查看执行任务、等待顺序和近期结束任务，提供取消、受控重试和人工删除。连接前可查看明确标注的演示；样例操作不影响真实任务。界面默认跟随 VOS 语言，可选择简中、英文或繁中。

本机资源采集是本应用内置组件，安装后自动启动、初始化本机节点并保存内部身份，无需单独部署 Agent、运行终端命令或填写采集凭据。CPU 支持展开查看逐逻辑处理器占用；Jetson Thor 的 CPU/GPU 共用一份系统内存，不额外显示独立显存池。

安装位置与计算位置分别记录。V-Router 云端推理不预约本地资源，本地模型以实际执行节点申请许可。未确认停止的执行保留预约；只有已停止终态任务允许软删除，业务输出仍由原应用管理。

本版支持 VOS 管理员登录和自动本机采集；HA 与设备级多 GPU 分配尚未实现。

管理页提供最近 24 小时的排队等待、执行占用 P50/P95、任务失败率和模型许可历史。任务详情持续更新，支持状态时间线、每次执行的分配/停止时间、等待原因建议及受控重试。列表可按状态、应用、节点组合筛选，搜索业务任务 ID；默认最近 200 条，可加载更早任务。P95 至少需要 20 个有效样本，旧许可释放时间未知时保持未知。

节点管理复用已安装 ModelInfer 的远端节点清单与现有监控，用户无需逐台安装 Agent。可保存节点预算规划，管理已受控接入的预算、执行器和服务凭据，并查看操作记录。规划不会授予执行权限；远端调度控制接口待验证，已有部署仍由所属 VOS App 管理。

## Host alias、内部端口与宿主机端口

以下是所有 profile 共用的默认映射。`<vos-host>` 为安装该应用的 VOS 宿主机 IP 或域名；192、229、232 三台测试机分别使用 `192.168.1.192`、`192.168.1.229`、`192.168.1.232`。内部地址仅在 Docker `vos_default` 网络解析，完整 URL 在地址前加 `http://`。安装时修改端口参数后，以实际安装值为准；应用内「API 文档 → 模型管理 API」显示配置后的地址。

统一容器的正式 host alias 只有 `v-scheduler`，通过 `8080`、`38120`、`6005` 区分网关、调度和模型管理；该容器其余名称全部保留为兼容别名。新应用和已有应用后续适配均推荐使用 `v-scheduler`。三个 Ollama 独立容器保留各自名称。

| 功能 | Docker host alias:内部端口（含全部兼容别名） | 宿主机 host:外部端口 | 主要路径与用途 |
|---|---|---|---|
| 统一推理、网关管理及统一界面 | `v-scheduler:8080`; 兼容：`v-scheduler-gateway:8080`、`v-router:8080`、`v-router-backend:8080` | `<vos-host>:22888` | `/v1/*` 推理；`/api/v1/*` 网关管理；`/` 界面 |
| 调度接管与资源管理 | `v-scheduler:38120`; 兼容：`v-scheduler-backend:38120`、`v-scheduler-frontend:38120` | `<vos-host>:38120` | `/api/v1/*` 任务、许可、节点、执行器及统计 |
| 模型仓库管理 | `v-scheduler:6005`; 兼容：`v-scheduler-model-hub:6005`、`v-scheduler-model-hub-backend:6005` | `<vos-host>:36005` | `/api/v1/*` 模型、选中文件下载、更新、导入、别名、引用和后端管理；`/openapi.json` |
| Ollama QA 原生管理 | `v-scheduler-ollama-qa:11434` | `<vos-host>:12434` | `/api/*` 模型清单、拉取、详情、加载/卸载及原生推理 |
| Ollama QA 推理网关 | `v-scheduler-ollama-qa:11535` | `<vos-host>:12535` | `/v1/chat/completions` 聊天；`/metrics` 指标 |
| Ollama Embedding 原生管理 | `v-scheduler-ollama-embedding:11434` | `<vos-host>:12435` | `/api/*` 原生模型管理及向量推理 |
| Ollama Embedding 推理网关 | `v-scheduler-ollama-embedding:11535` | `<vos-host>:12536` | `/v1/embeddings` 向量；`/metrics` 指标 |
| Ollama Rerank 原生管理 | `v-scheduler-ollama-rerank:11434` | `<vos-host>:12436` | `/api/*` 原生模型管理及重排推理 |
| Ollama Rerank 推理网关 | `v-scheduler-ollama-rerank:11535` | `<vos-host>:12537` | `/v1/rerank` 重排；`/metrics` 指标 |

可配置的外部端口参数：

- `V_ROUTER_HOST_PORT`：默认 `22888`。
- `SCHEDULER_HOST_PORT`：默认 `38120`。
- `MODEL_HUB_BACKEND_HOST_PORT`：默认 `36005`。
- `OLLAMA_QA_HOST_PORT`：默认 `12434`。
- `OLLAMA_QA_GATEWAY_HOST_PORT`：默认 `12535`。
- `OLLAMA_EMBEDDING_HOST_PORT`：默认 `12435`。
- `OLLAMA_EMBEDDING_GATEWAY_HOST_PORT`：默认 `12536`。
- `OLLAMA_RERANK_HOST_PORT`：默认 `12436`。
- `OLLAMA_RERANK_GATEWAY_HOST_PORT`：默认 `12537`。

同一行的兼容别名指向同一个容器和端口，不产生额外服务或宿主机映射。`host.docker.internal` 是网关容器中的 Docker 宿主机别名（`host-gateway`），用于访问宿主机已发布端口，例如 `host.docker.internal:38120`；它不是新增服务，也没有单独的固定端口。

网关还在同一 `22888` 端口提供 `/scheduler/api/v1/*` 调度代理和 `/model-hub/api/v1/*` 模型管理代理。VOS 内统一入口为 `/app/com.ictrek.v-scheduler/`；模型管理前缀为 `/app/com.ictrek.v-scheduler/model-hub/api/v1/*`，网关 API 前缀为 `/api/com.ictrek.v-scheduler`。三个 Ollama 的 `11535` 是各后端推理/指标入口；跨后端统一模型名和调度入口使用 `v-scheduler:8080/v1/*` 或 `<vos-host>:22888/v1/*`。

这些别名由 V-Scheduler 自己声明；原 Model Hub 的别名、端口及共享目录保持原应用所有权。共享模型目录仍在安装时选择，沿用原 Model Hub 所选公共目录即可。
