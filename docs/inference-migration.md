# 推理接管与集群节点 / Inference migration and cluster nodes

## 中文

管理员从 V-Scheduler 左侧导航打开「推理部署」，通过「部署列表」「模型卡片」「模型广场」「节点监控」「推理设置」切换。选择部署详情仍在当前工作区。「高级管理 → VOS 集群管理」提供「集群概览」和「集群成员」。这些页面继续使用原有运行时和迁移 API。SSH 算力节点与 VOS 集群成员按相同 IP 关联。集群成员状态用于显示来源，调度准入仍由节点授权、执行器与预算决定。列表暂时读取失败时会保留上次读取结果并停用操作。身份失效或权限撤销时会清空列表。

接管旧 ModelInfer 时，先记录部署和容器状态，备份 ModelInfer 数据库及 `/var/lib/modelinfer/plaza`。在 V-Scheduler 应用存储的 `inference/plaza` 下准备相同的卡片目录结构。模型权重继续使用已配置的模型仓库共享路径。

1. 在「节点管理 → 迁移 ModelInfer 部署」查看已有部署状态，或调用 `GET /inference/migration/preview` 核对源表和行数。
2. 等待启动或停止作业结束，在 VOS 停用旧 ModelInfer 管理 App，并核对现有推理容器与请求状态。
3. 单击「导入旧部署」，或调用 `POST /inference/migration/import`。服务确认旧 App 已停用、校验模型卡包摘要，并提交完整导入事务。报告冲突时，核对两侧已有记录后重试。源数据库保留。
4. V-Scheduler 检查导入回执和旧 App 状态后启动内置运行时。用 `GET /inference/runtime/health` 确认就绪，在「推理部署」核对记录、运行状态和真实推理请求。
5. 在内置服务开始修改部署前，可启用旧管理 App 恢复控制。V-Scheduler 检测到旧 App 再次接管后会退出内置管理进程。在两边发生配置或生命周期变更后，恢复前需按实际部署状态核对数据。

API 基址为 `/api/com.ictrek.v-scheduler/scheduler/api/v1`，管理接口使用管理员 Bearer 凭据。原生 ModelInfer 接口在 `/inference/runtime/` 下，Cluster Manager 原生接口在 `/cluster/runtime/` 下。例如读取推理部署用 `GET /inference/runtime/deployments`，查询集群状态用 `GET /cluster/runtime/cluster/status`。查看原生接口时同时检查 HTTP 状态和响应中的数字业务码。

预检和导入接口无需请求参数。预检返回 `source_present`、`tables` 和 `deployments_by_state`。`tables` 的值为非负行数，源表缺失时为 `null`。导入回执返回 `already_imported` 和 `tables`，其中只列出已导入的表。运行时开始接管后，后续导入会返回 `MODELINFER_IMPORT_RUNTIME_ENABLED`。如果提交后连接中断，先检查运行时健康接口和部署清单，再决定是否重试。

VOS 集群管理按需部署子项目。跨主机操作要求目标节点也运行 V-Scheduler 集群管理，并配置对应的受信 HTTPS 地址和独立管理员凭据。应用存储 `cluster/peers.json` 的键为节点 IP，每个值包含 `origin` 和 `token`。私有 CA 可放在同目录的 `peer-ca.pem`。限制凭据文件访问并在修改后重启控制器。

新安装可在 VOS 应用配置中开启「内置推理运行时」。运行时启动前会确认旧 ModelInfer 未安装或已停用。目标节点应已具备模型卡要求的镜像，缺少时仍可由原 Image Hub 分发。

## English

Administrators open **Inference deployments** from the V-Scheduler sidebar and switch between **Deployments**, **Model cards**, **Model catalog**, **Node monitoring**, and **Inference settings**. Deployment details open in the same workspace. Go to **Advanced Management → VOS Cluster Manager** for **Cluster overview** and **Cluster members**. The pages use the existing runtime and migration API contracts. An SSH node and a VOS cluster member are linked by exact IP. Cluster membership identifies the source. Task admission uses separate permissions, executors and budgets. A temporary inventory error retains the last loaded rows and disables actions. Authentication or access errors clear the list.

For migration, record the current deployments and container state, then back up the ModelInfer database and `/var/lib/modelinfer/plaza`. Stage the same card directory structure in `inference/plaza` under V-Scheduler app storage. Weights continue to use the configured shared model repository.

1. Open **Node Management → Migrate ModelInfer deployments** to inspect deployment states. Use `GET /inference/migration/preview` to check source tables and row counts.
2. Wait for start and stop operations to finish. Disable the legacy ModelInfer management app in VOS and verify running inference containers and requests.
3. Select **Import legacy deployments** or call `POST /inference/migration/import`. The service checks that the legacy app is stopped, verifies model package digests, and commits the import in one transaction. Resolve destination conflicts and retry. The source database is retained.
4. V-Scheduler starts the runtime after checking the import receipt and the legacy app state. Check `GET /inference/runtime/health`, then verify deployment records, runtime status and a real inference request.
5. Before making deployment changes through the new runtime, the old management app can be enabled to recover control. V-Scheduler stops its embedded process when the legacy app resumes. Review deployment state before recovery if configuration or lifecycle changes have occurred.

Use `/api/com.ictrek.v-scheduler/scheduler/api/v1` and an administrator Bearer credential for management APIs. The native ModelInfer APIs are under `/inference/runtime/`, and Cluster Manager APIs are under `/cluster/runtime/`. For example, `GET /inference/runtime/deployments` lists deployments and `GET /cluster/runtime/cluster/status` reads cluster status. Check both the HTTP status and numeric application code for native APIs.

Preview and import take no request parameters. Preview returns `source_present`, `tables`, and `deployments_by_state`. Values in `tables` are nonnegative row counts, with `null` for an absent table. The import receipt includes `already_imported` and `tables`, which lists the imported tables. After the runtime takes ownership, further import attempts return `MODELINFER_IMPORT_RUNTIME_ENABLED`. If the connection drops after submission, check runtime health and the deployment list before retrying.

Cluster Manager projects are deployed on demand. Remote operations require V-Scheduler Cluster Manager on the target and a trusted HTTPS origin with a separate administrator credential. In `cluster/peers.json` under app storage, use the node IP as the key and supply `origin` and `token` for each peer. Place a private trust anchor at `cluster/peer-ca.pem` if needed. Restrict file access and restart the controller after changes.

On a new installation, enable **Embedded Inference Runtime** in the VOS app configuration. The runtime checks that the legacy ModelInfer app is absent or stopped before starting. Target nodes should already have the images required by the model card. The existing Image Hub can distribute missing images.
