# 推理接管与集群节点 / Inference migration and cluster nodes

## 中文

管理员从 V-Scheduler 左侧导航打开「推理部署」，通过「部署列表」「模型卡片」「模型广场」「节点监控」「推理设置」切换。选择部署详情仍在当前工作区。存储集群的概览和成员由独立的 VOS 集群管理应用提供。推理部署继续使用原有运行时和迁移 API。SSH 算力节点与 VOS 集群成员按相同 IP 关联。集群成员状态用于显示来源，调度准入仍由节点授权、执行器与预算决定。列表暂时读取失败时会保留上次读取结果并停用操作。身份失效或权限撤销时会清空列表。

接管旧 ModelInfer 时，先记录部署和容器状态，备份 ModelInfer 数据库及 `/var/lib/modelinfer/plaza`。在 V-Scheduler 应用存储的 `inference/plaza` 下准备相同的卡片目录结构。模型权重继续使用已配置的模型仓库共享路径。

1. 在「节点管理 → 迁移 ModelInfer 部署」查看已有部署状态，或调用 `GET /inference/migration/preview` 核对源表和行数。
2. 等待启动或停止作业结束，在 VOS 停用旧 ModelInfer 管理 App，并核对现有推理容器与请求状态。
3. 单击「导入旧部署」，或调用 `POST /inference/migration/import`。服务确认旧 App 已停用、校验模型卡包摘要，并提交完整导入事务。报告冲突时，核对两侧已有记录后重试。源数据库保留。
4. V-Scheduler 检查导入回执和旧 App 状态后启动内置运行时。用 `GET /inference/runtime/health` 确认就绪，在「推理部署」核对记录、运行状态和真实推理请求。
5. 在内置服务开始修改部署前，可启用旧管理 App 恢复控制。V-Scheduler 检测到旧 App 再次接管后会退出内置管理进程。在两边发生配置或生命周期变更后，恢复前需按实际部署状态核对数据。

API 基址为 `/api/com.ictrek.v-scheduler/scheduler/api/v1`，管理接口使用管理员 Bearer 凭据。原生 ModelInfer 接口在 `/inference/runtime/` 下，Cluster Manager 原生接口在 `/cluster/runtime/` 下。例如读取推理部署用 `GET /inference/runtime/deployments`，查询集群状态用 `GET /cluster/runtime/cluster/status`。查看原生接口时同时检查 HTTP 状态和响应中的数字业务码。

预检和导入接口无需请求参数。预检返回 `source_present`、`tables` 和 `deployments_by_state`。`tables` 的值为非负行数，源表缺失时为 `null`。导入回执返回 `already_imported` 和 `tables`，其中只列出已导入的表。运行时开始接管后，后续导入会返回 `MODELINFER_IMPORT_RUNTIME_ENABLED`。如果提交后连接中断，先检查运行时健康接口和部署清单，再决定是否重试。

VOS 集群管理按需部署子项目。跨主机操作要求目标节点也运行 V-Scheduler 集群管理，并配置对应的受信 HTTPS 地址和独立管理员凭据。应用存储 `cluster/peers.json` 的键为节点 IP，每个值包含 `origin` 和 `token`。私有 CA 可放在同目录的 `peer-ca.pem`。限制凭据文件访问并在修改后重启控制器。

VOS 确认旧 ModelInfer 未安装或已卸载后，V-Scheduler 自动启动内置推理运行时。要保留旧版部署，请先停用旧应用并从「节点管理 → 迁移 ModelInfer 部署」完成导入。保留的停用安装也可由管理员显式开启接管。目标节点应已具备模型卡要求的镜像，缺少时仍可由原 Image Hub 分发。

### 推理镜像登记

管理员在「推理部署 → 推理镜像」点击「登记镜像」，填写完整仓库、路径与显式标签或 `sha256` 摘要。勾选适用 Profile 后保存。新增硬件类型时，先填入自定义名称并点「登记 Profile」，再保存镜像。用「节点管理 → SSH 算力节点 → 编辑」把该名称手动分配给对应节点。普通 VOS 用户可从侧栏「推理镜像」查看登记清单。

内置 Profile 为 `jetson-thor`、`dgx-spark`、`jetson-orin`、`arm-cpu`、`amd-cpu`、`amd-cuda`、`arm-cuda` 和 `sophon-bm1688`。自定义名称以小写英文字母开头，只含小写英文字母、数字与分隔用的单个连字符，最长 64 个字符。保留名称 `auto`、`thor`、`thor-spark`、`l4t`。自动扫描仍识别内置 Profile。已有镜像可用「编辑 Profile」重新登记。

模型卡的具体运行时镜像可在「推理部署 → 模型广场」打开卡片的清单查看。新建运行配置选择卡片及版本后，「权重与镜像覆盖」显示默认镜像，可粘贴登记清单中的地址覆盖。通过目标 Profile 筛选镜像时，`dgx-spark` 兼容 `jetson-thor`。镜像登记保存地址与管理员确认的兼容信息。实际部署继续按模型卡配置校验节点、权重与镜像。

`GET /compute-profiles` 和 `GET /inference/images` 接受登录 VOS 用户或调度提交者的 Bearer 凭据。管理员可以 `POST /compute-profiles` 提交 `{"name":"arm-new-board"}`，再 `POST /inference/images` 提交 `{"image_ref":"registry.example.com/team/vllm:0.1","profiles":["arm-new-board"]}`。地址最长 512 字节。单次最多提交 64 个已登记 Profile，重复项去重。首次安装或升级会预置已观察到在 Thor、Orin 上运行的镜像记录，只写入地址及 Profile。管理员可修改适用 Profile，已运行记录独立显示。GET 返回 `items` 和 `can_manage`，镜像项含 `image_ref`、`profiles`、`tested_profiles`、`created_at` 和 `updated_at`。POST 返回已保存记录。同一地址重复提交会更新 Profile，保留创建时间。用 `GET /inference/images?profile=arm-new-board` 读取匹配清单。详细的状态码、字段与响应位于「API 文档 → 调度管理 API」。

## English

Administrators open **Inference deployments** from the V-Scheduler sidebar and switch between **Deployments**, **Model cards**, **Model catalog**, **Node monitoring**, and **Inference settings**. Deployment details open in the same workspace. Open the standalone **VOS Cluster Manager** app for the storage cluster overview and member list. Inference deployments use the existing runtime and migration APIs. An SSH node and a VOS cluster member are linked by exact IP. Cluster membership identifies the source. Task admission uses separate permissions, executors and budgets. A temporary inventory error retains the last loaded rows and disables actions. Authentication or access errors clear the list.

For migration, record the current deployments and container state, then back up the ModelInfer database and `/var/lib/modelinfer/plaza`. Stage the same card directory structure in `inference/plaza` under V-Scheduler app storage. Weights continue to use the configured shared model repository.

1. Open **Node Management → Migrate ModelInfer deployments** to inspect deployment states. Use `GET /inference/migration/preview` to check source tables and row counts.
2. Wait for start and stop operations to finish. Disable the legacy ModelInfer management app in VOS and verify running inference containers and requests.
3. Select **Import legacy deployments** or call `POST /inference/migration/import`. The service checks that the legacy app is stopped, verifies model package digests, and commits the import in one transaction. Resolve destination conflicts and retry. The source database is retained.
4. V-Scheduler starts the runtime after checking the import receipt and the legacy app state. Check `GET /inference/runtime/health`, then verify deployment records, runtime status and a real inference request.
5. Before making deployment changes through the new runtime, the old management app can be enabled to recover control. V-Scheduler stops its embedded process when the legacy app resumes. Review deployment state before recovery if configuration or lifecycle changes have occurred.

Use `/api/com.ictrek.v-scheduler/scheduler/api/v1` and an administrator Bearer credential for management APIs. The native ModelInfer APIs are under `/inference/runtime/`, and Cluster Manager APIs are under `/cluster/runtime/`. For example, `GET /inference/runtime/deployments` lists deployments and `GET /cluster/runtime/cluster/status` reads cluster status. Check both the HTTP status and numeric application code for native APIs.

Preview and import take no request parameters. Preview returns `source_present`, `tables`, and `deployments_by_state`. Values in `tables` are nonnegative row counts, with `null` for an absent table. The import receipt includes `already_imported` and `tables`, which lists the imported tables. After the runtime takes ownership, further import attempts return `MODELINFER_IMPORT_RUNTIME_ENABLED`. If the connection drops after submission, check runtime health and the deployment list before retrying.

Cluster Manager projects are deployed on demand. Remote operations require V-Scheduler Cluster Manager on the target and a trusted HTTPS origin with a separate administrator credential. In `cluster/peers.json` under app storage, use the node IP as the key and supply `origin` and `token` for each peer. Place a private trust anchor at `cluster/peer-ca.pem` if needed. Restrict file access and restart the controller after changes.

V-Scheduler starts its inference runtime automatically after VOS confirms that the legacy ModelInfer app is absent or uninstalled. To retain legacy deployments, stop the old app and import them from **Node Management → Migrate ModelInfer deployments** before uninstalling. An administrator can also explicitly take over a stopped installation. Target nodes should already have the images required by the model card. The existing Image Hub can distribute missing images.


### Inference image registration

Open **Inference deployments → Inference images** and select **Register image**. Enter the full registry and repository with an explicit tag or sha256 digest. Select the supported profiles and save. For new hardware, enter a custom name and select **Register profile** before saving the image. Assign the custom name manually under **Node management → SSH compute nodes → Edit**. Other signed-in VOS users can read the catalog from **Inference images** in the sidebar.

Built-in profiles are `jetson-thor`, `dgx-spark`, `jetson-orin`, `arm-cpu`, `amd-cpu`, `amd-cuda`, `arm-cuda`, and `sophon-bm1688`. Custom names start with a lowercase ASCII letter and use lowercase letters, digits, and single hyphens as separators, up to 64 characters. `auto`, `thor`, `thor-spark`, and `l4t` are reserved. Hardware scans continue to detect built-in profiles. Use **Edit profiles** to update an existing reference.

For a model card’s default images, open its manifest in **Inference deployments → Model catalog**. When creating a run configuration, select the card and version first. The weight and image overrides section shows the defaults and accepts an image reference from the catalog. Filtering for `dgx-spark` includes `jetson-thor` images. The catalog records the reference and administrator-declared compatibility. Deployments use the model card workflow to check node, weight, and image availability.

`GET /compute-profiles` and `GET /inference/images` accept Bearer credentials from signed-in VOS accounts or scheduler submitters. An administrator can send `{"name":"arm-new-board"}` to `POST /compute-profiles`, then send `{"image_ref":"registry.example.com/team/vllm:0.1","profiles":["arm-new-board"]}` to `POST /inference/images`. References are limited to 512 bytes and a request can supply up to 64 registered profile names. Duplicate selections are removed. Installation or upgrade seeds references observed running on Thor and Orin and stores catalog metadata. The observed profiles remain visible when supported profiles are edited. GET responses contain `items` and `can_manage`. Image items contain `image_ref`, `profiles`, `tested_profiles`, `created_at`, and `updated_at`. POST returns the saved record. Registering the same reference again updates its profiles and preserves the creation time. Use `GET /inference/images?profile=arm-new-board` to filter. See **API Documentation → Scheduler Management API** for field and error definitions.

### 查看节点实际资源 / View live node resources

管理员打开「资源与任务 → 执行资源」可查看仅观察节点的逻辑 CPU 数与利用率、系统内存已用／总量／可用、GPU 利用率，以及采样时间。内置推理和 SSH 节点管理启用后，采集使用节点保存的 SSH 连接。Jetson 或 GB10 的 GPU 与 CPU 共享系统内存，页面只计算一次。支持采集的独显会另外显示显存。缺失或超过 90 秒的采样显示为不可用。检查 SSH 连接配置后等待下一次采集。硬件空闲量与调度批准预算各自显示。

Administrators can open **Resources and tasks → Execution resources** for the logical CPU count and utilization, system memory used, total and available, GPU utilization, and sample timestamp on observation-only nodes. Enable the embedded inference runtime and SSH node management so the controller can sample through the node’s saved connection. Jetson and GB10 report shared system memory once. Discrete GPUs with supported telemetry show their own device memory. Missing or stale samples (90 seconds) are shown as unavailable. Verify the saved SSH connection and wait for the next sample. Scheduling budgets are displayed independently of hardware availability.

`GET /nodes` uses the existing scheduling API administrator Bearer token and takes no parameters. See **API Documentation → Scheduling API → Nodes and placements** for the response fields and errors. Read live samples from `data[].modelinfer_source.node.host_resources` when `modelinfer_source.fresh` is true. The SSH sample object can be absent when the connection or probe fails. GPU fields can be null for unsupported devices.

## ModelInfer 本机节点、部署文件和电源 / Local nodes, deployment files and power

管理员在「推理部署 → 节点监控」选择「添加当前节点」。需要在 VOS 应用配置中启用 `SCHEDULER_MODELINFER_HOST_ACCESS` 并将 `SCHEDULER_MODELINFER_PID_MODE` 设为 `host`。默认使用隔离的 PID 命名空间。启用后容器以特权模式访问当前宿主机，适用于由设备管理员维护的主机。扫描填入硬件信息，节点名称由管理员填写。分节点继续使用 SSH。添加通过 ModelInfer 管理的本机节点后，可用于推理运行配置。

在「推理部署 → 部署列表」打开运行配置的文件覆盖。选择模型卡中的脚本，编辑并保存，下一次启动部署会使用修改后的文件。删掉覆盖可在下次启动恢复卡片默认文件。模型卡原件由模型广场管理。节点列表和详情支持关机及重启，繁忙节点默认拒绝，确认采用强制模式会跳过任务和占用检查。接口成功表示命令已下发。

Administrators can choose **Add current node** under **Inference deployments → Node monitoring**. In the VOS app configuration, enable `SCHEDULER_MODELINFER_HOST_ACCESS` and set `SCHEDULER_MODELINFER_PID_MODE` to `host`. The default keeps the PID namespace isolated. The two settings allow privileged access to the current host and are intended for device administrators. The scan fills in the hardware details. Enter a node name before saving. Remote nodes use SSH. The local node is available to ModelInfer deployments.

Open a deployment's file overrides from **Inference deployments → Deployments**. Select a script from the model card and save an override to use it on the next deployment start. Removing the override restores the card's default on the next start. The node list and details provide shutdown and reboot. Requests are blocked when the node has active work. Explicit force skips both Scheduler and ModelInfer occupancy checks. A successful response confirms command dispatch.

API base: `/api/com.ictrek.v-scheduler/scheduler/api/v1/inference/runtime`. Use an administrator VOS bearer token. Direct controller access uses `http://<vos-host>:38120/api/v1/inference/runtime` by default. The native operations use the existing numeric ModelInfer envelope. An API client checks both HTTP status and the envelope `code` (0 means success). / 使用 VOS 管理员 Bearer。调用同时检查 HTTP 状态和 ModelInfer 响应的数字 `code`（0 为成功）。

| Method and path | Contract / 契约 |
| --- | --- |
| `GET /nodes/local-info` | Probe the current host without saving a node. Requires local host access. / 扫描本机并返回硬件信息。需启用本机免密访问。 |
| `POST /nodes` | Existing node fields apply. Set optional `access_mode` to `local` for the current host (SSH credentials are ignored) or leave it at the default `ssh` for remote nodes. / 沿用节点字段。指定 `local` 时忽略 SSH 凭据，默认 `ssh`。 |
| `POST /nodes/{id}/power` | JSON `{"action":"reboot","force":false}`. `action` is required and accepts `reboot` or `shutdown`. Optional boolean `force` defaults to false. Success returns `node_id`, `action`, `force` and ISO-8601 `accepted_at`. Scheduler returns HTTP 409 `NODE_HAS_SCHEDULER_WORK` for pending or active work. Native codes `20756` and `20757` indicate an occupied node and a failed command. / 可选 `force` 默认 false。409 表示有调度任务，20756 表示运行配置占用，20757 表示下发失败。 |
| `GET /deployments/{id}/file-overrides` | Returns `files` (each has `path`, `size_bytes`, `override_id` and nullable `updated_at`), plus `overridden`, `max_content_bytes` and `max_files`. / 查看文件和覆盖数量与限制。 |
| `GET /deployments/{id}/card-files/content?path=<relative-script>` | Read the original model card script. / 读取原始脚本。 |
| `POST /deployments/{id}/file-overrides` | JSON with required script `path` and optional `content`. Omitted or null content copies the original. / 提交路径和可选内容，未指定内容时复制原文。 |
| `GET /deployments/{id}/file-overrides/{override_id}` | Returns `id`, `deployment_id`, `path`, `content`, `original_content`, `size_bytes`, `created_at` and `updated_at`. Create and update return the same shape. / 读取覆盖详情。创建和更新返回相同字段。 |
| `PUT /deployments/{id}/file-overrides/{override_id}` | JSON with string `content`; replace a script override. / 替换覆盖内容。 |
| `DELETE /deployments/{id}/file-overrides/{override_id}` | Clear the deployment override. / 删除覆盖。 |

Overrides accept up to 64 script files per deployment and 1 MiB of UTF-8 content per file. Use a script path from the file list. Native `20753` means local host execution is unavailable, `20754` means the override input is invalid and `20755` means the model-card file or override was not found. / 每个部署最多覆盖 64 个文件，每个文件最多 1 MiB。使用文件清单里的路径。20753 表示本机执行不可用，20754 表示覆盖输入无效，20755 表示脚本或覆盖不存在。
