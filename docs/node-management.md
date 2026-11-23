# VOS 节点管理

## SSH 算力节点 / SSH compute nodes

VOS 登录用户可在「V-Scheduler → 节点管理 → SSH 算力节点」查看节点名称、在线状态、架构和 GPU。管理员可添加节点，填写主机、SSH 端口和用户，选择私钥或密码后点击「连接并保存」。保存前会验证连接并扫描硬件。编辑时留空的凭据继续使用保存值。点击「重新扫描」可重新连接并更新状态。使用登录用户执行扫描和容器验证；填写 sudo 密码时使用 sudo。目标账号须能访问 Docker。

「验证容器运行」会在节点上拉取 `SCHEDULER_NODE_TEST_IMAGE`（默认 `docker.io/library/busybox:1.37.0`），启动一个执行 `/bin/true` 的临时容器并清理容器。镜像会留在节点的 Docker 缓存。部署在限制外网的网络时，将该设置指向节点可访问且包含 `/bin/true` 的镜像。完成验证的节点当前可用于状态和运行环境检查。模型部署和资源准入继续由对应的部署流程负责。

Signed-in VOS users can open **V-Scheduler → Node Management → SSH compute nodes** to view node names, online status, architecture, and GPUs. Administrators can add a node with its host, SSH port, user, and private key or password. **Connect and save** verifies SSH and scans hardware before saving. Leaving a credential blank when editing retains the saved value. **Rescan** reconnects and updates status. Operations use the login account, or sudo if a sudo password is supplied. The account needs access to Docker.

Profile 默认为自动识别。管理员可选择 `jetson-thor`、`dgx-spark`、`jetson-orin`、`arm-cpu`、`amd-cpu`、`amd-cuda`、`arm-cuda` 或 `sophon-bm1688`，选择自动识别可清除覆盖。DGX Spark / GB10 单独识别为 `dgx-spark`，并兼容 `jetson-thor`。用 `GET /managed-nodes?profile=jetson-thor` 筛选时会返回两类节点，用 `profile=dgx-spark` 可只匹配 Spark。已授权应用可用自己的服务凭据读取脱敏清单。

The default profile is detected from the node. Administrators can override it with `jetson-thor`, `dgx-spark`, `jetson-orin`, `arm-cpu`, `amd-cpu`, `amd-cuda`, `arm-cuda`, or `sophon-bm1688`. Select Auto-detect to clear an override. DGX Spark / GB10 is identified as `dgx-spark` and also matches `jetson-thor`. Request `GET /managed-nodes?profile=jetson-thor` for both, or `profile=dgx-spark` for Spark matches only. Authorized applications can use their service credentials to read the public inventory.

**Check container runtime** pulls `SCHEDULER_NODE_TEST_IMAGE` (default `docker.io/library/busybox:1.37.0`), runs `/bin/true` in a temporary container, and removes the container. The image stays in the Docker cache. On restricted networks, configure a reachable image containing `/bin/true`. Model deployment and resource admission use their existing workflows.

| Method and v1 path | Access | Operation |
| --- | --- | --- |
| `GET /managed-nodes` | Signed-in VOS users and authorized applications / 已登录 VOS 用户及已授权应用 | Returns `items` and `can_manage`. The public projection includes `id`, `name`, `status` (`online` or `offline`), `arch`, `gpu_model`, `profile`, `detected_profile`, `compatible_profiles`, and `last_checked_at`. Administrators also receive connection settings, credential-presence flags, and hardware scan details. |
| `POST /managed-nodes` | Administrator / 管理员 | Verify and save an SSH node. Requires `name` and `host`. `ssh_port` defaults to 22 (1–65535), `ssh_user` defaults to `root`, and `auth_method` is `password` (default) or `key`. Supply `ssh_password` or `ssh_key` for the selected mode. Optional `sudo_password` enables sudo. |
| `PUT /managed-nodes/{id}` | Administrator / 管理员 | Update provided connection fields. If changing the authentication mode, supply its credential. Omit a credential to retain it. An empty `sudo_password` clears explicit sudo. |
| `POST /managed-nodes/{id}/rescan` | Administrator / 管理员 | Reconnect using saved credentials and refresh the hardware inventory. An SSH failure returns HTTP 502 with `NODE_SSH_FAILED` and preserves the last successful result. / 复用已保存的凭据刷新硬件清单。SSH 失败时返回 HTTP 502 和 `NODE_SSH_FAILED`，保留上一次成功的结果。 |
| `POST /managed-nodes/{id}/runtime` | Administrator / 管理员 | Pull the configured test image and run the temporary container. Returns `image`, `pulled`, and `started`. |
| `DELETE /managed-nodes/{id}` | Administrator / 管理员 | Remove the saved configuration. Returns `removed`. |

Use the VOS Bearer token and the same-origin base `/api/com.ictrek.v-scheduler/scheduler/api/v1`. Sending an SSH password, sudo password, or private key does not cause it to be returned in API responses. A connection, image pull, or container error is surfaced as an operation-specific error code. The test uses the fixed image setting and a fixed command.

用户通过 VOS 安装和管理本应用，无需 SSH 登录计算节点、下载程序或手工运行 Agent。本机采集组件随 App 自动运行；外部设备复用其所属 VOS 应用已有的管理能力。

## 在界面管理

1. 安装或升级 V-Scheduler 后，使用已有 VOS 管理员身份打开“节点管理”。不需要填写额外登录令牌。
2. 页面自动读取 VOS 原生集群成员与同机已安装 ModelInfer 的节点清单。两个来源分别显示状态；原生集群接口不可用时，ModelInfer 节点仍可显示。
3. ModelInfer 节点显示架构、GPU 型号、已有运行部署状态，以及其现有监控提供的 CPU、内存和 GPU 使用率。无需在计算节点额外部署采集程序。“打开 ModelInfer 管理”进入所属应用，继续使用该应用的节点与部署管理。
4. 点击节点的“预算规划”，保存显示名称、规划容量、常驻占用和系统保留。规划持久化在 Scheduler 自己的数据库中，使用版本号保护并发编辑；不更改 ModelInfer 部署，也不创建可执行 worker 或增加可预约资源。
5. 已由应用内部受控接入的节点可编辑批准预算、排空或恢复接收；执行器按批准模板定义，服务凭据按应用隔离，操作保留审计记录。仍有未确认停止的任务或许可时，拒绝修改、撤销或借此释放资源。

## 监控与控制边界

ModelInfer 接口由已安装应用的受信 VOS 地址与配置端口解析。只读取节点清单和 `GET /api/nodes/{id}/metrics`，不使用清单中的 SSH 地址，不复制 SSH 用户、密码、密钥、网络接口或错误原文。

指标仅保留数值与采样时间，去掉 Prometheus labels。超过 90 秒、非有限值、缺失的序列显示未知，不显示为零或空闲。Spark 等统一内存设备不会因为有 GPU 型号就被伪造为独立显存池；监控值也不自动成为调度预算。

现有 ModelInfer 远端管理与部署继续由其所属 VOS App 负责。当前未验证远端调度所需的完整执行、停止与故障恢复契约，所以这一路节点明确显示“调度控制待验证”，不会变为可调度。用户无需执行任何手工安装步骤。待管理链路具备条件后，由应用实现方验证并接入。

## 为后续 VOS App 管理适配器保留的接口

以下是应用开发接口，不是用户安装程序的流程：

| 接口 | 行为 |
|---|---|
| `GET /api/v1/nodes` | 合并本机、平台与应用发现的节点，包含来源新鲜度、脱敏指标和 `source_settings` |
| `POST /api/v1/node-sources/modelinfer/{id}` | 管理员保存规划：`version`、`display_name`、`capacities`、`resident_baseline`、`safety_reserve`；不会增加执行权限 |
| `POST /api/v1/node-sources/modelinfer/{id}/control` | 预留 `pause` / `resume` / `confirm-stopped` 控制契约；当前返回 HTTP 501、`VOS_NODE_CONTROL_PENDING_VERIFICATION`，不运行远端命令 |
| `GET /api/v1/management` | 管理员读取预算、执行器、服务身份元数据和操作记录，不返回令牌或哈希 |

规划接口使用乐观版本：首次创建 `version=0`，更新携带读取到的版本；冲突返回 409。未知来源 ID 返回 404。所有管理接口校验 VOS 管理员或内部管理员服务身份。

内部 worker 登记、单次邀请和执行器审批协议供 VOS App 自身的受控组件使用，不能要求最终用户在外部机器运行这些组件。本机仍使用随 App 安装的内置采集与执行服务。

## 多节点资源准入的实现范围

底层协议已支持固定 `model.infer` 模板：`node_id/resources` 表示主计算节点，`placements` 表示其余计算节点的独立预算。总计最多 32 个节点，不能重复主节点或其他节点。所有参与者须已通过受控应用接入并具有新鲜遥测；观察节点及预算规划不能作为可执行节点。

一次请求在同一事务中检查、预约全部参与节点。任一节点忙碌、排空或失联时整单拒绝；停止不明时保留全部预约。资源页逐节点记账，许可历史显示全部计算位置。模型分片、互联和部署生命周期继续由所属运行时负责。

这不表示 ModelInfer 发现的多机 vLLM 已接入调度。真实远端执行仍待 VOS App 管理适配器验证。异步多节点 gang 调度、任意远程 shell、文件复制、模型迁移和 HA 不在本版范围内。

公开字段见 [OpenAPI](../api/openapi-v1.json)，业务接入见 [接入标准](integration-standard.md)。

## 应用统一接入

「节点管理 → 应用统一接入」每 5 秒刷新已启用统一生命周期协议的应用、心跳、执行器批准预算、节点状态和未释放执行。可请求停止并卸载模型；界面保持“等待原生停止核验”，直到执行端回执通过后才移除。未接入的旧应用保留原自动绑定，不显示虚假的可停止状态。

节点约每 2 秒报告、超过 12 秒无心跳暂停新准入；应用心跳按响应间隔（当前 5 秒），20 秒过期暂停已登记执行器的新准入。两类心跳均不直接释放在途预约。接口、身份与完整停止示例见[统一接入指南](application-integration.md#unified-lifecycle)。
