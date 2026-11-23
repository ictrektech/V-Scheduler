# 应用后端部署 / Application backend deployments

## 中文

在 V-Scheduler 打开「API 文档 → 调度管理 API → 应用后端部署」。应用继续管理自己的镜像清单和后端设置。V-Scheduler 校验部署权限，按节点分配宿主机端口，并跟踪部署生命周期。应用可以用同一套 API 指定 SSH 节点或集群成员。

1. 管理员在「节点管理 → SSH 算力节点」登记可访问 Docker 的节点和 Profile，或在「高级管理 → VOS 集群管理」登记集群成员并配置受信 HTTPS 对端。部署目标须有 IPv4 地址。集群对端须运行同版本 V-Scheduler，开启集群管理，并具备平台 Projects 权限。集群成员的 Profile 由管理员授权时指定。
2. 用「节点管理 → 服务凭据」为应用创建服务凭据，通过 `GET /managed-nodes` 取得 SSH 节点 ID 或 `cluster_nodes[].id`。管理员调用 `PUT /backend-deployment-access/{app_id}`，提交 `targets` 和 `repositories`。每个目标包含 `kind`（`ssh` 或 `cluster`）、`id` 和已登记的 `profile`。每个仓库写完整注册表和路径，省略 tag/摘要。两个数组分别最多 64 项。空数组撤回新部署权限。应用可调用同路径的 GET 查看自己的授权。
3. 应用以自己的服务凭据调用 `POST /backend-deployments`，提交稳定的 `request_key`、`target`、带 tag 或 SHA-256 摘要的 `image`，以及 `container_port`（1 至 65535）。请求在数据库中入队。使用同一个键和相同参数可安全重试。不同参数复用同一键会产生冲突。镜像由目标节点拉取，私有仓库依赖该节点已有的拉取权限。
4. 轮询 `GET /backend-deployments/{id}`。状态为 `running` 且 `endpoint` 有值后，应用的后端可用此地址连接。建议保留应用原有的应用层健康检查或 HTTPS 代理。端口在每个控制器的目标 IPv4 上从 42000–42999 分配。应为部署保留该范围，避免与其他服务或控制器争用。只能映射一个 TCP 端口。
5. `POST /backend-deployments/{id}/stop` 异步清理容器或 VOS Project，保留端口。使用 `POST /backend-deployments/{id}/start` 以原参数重试失败或已停止实例。调用 `DELETE /backend-deployments/{id}`，确认进入 `removed` 后释放端口。管理员先删除该节点的后端部署，再删除 SSH 登记或修改其地址。

SSH 路径按 Profile 选择 Docker 运行时。集群路径使用 V-Scheduler 安装包的 `scheduler-app-backend` 项目。同一台机器按 IPv4 共享本控制器的宿主机端口分配。Jetson Orin、Jetson Thor、DGX Spark 及 ARM/AMD CUDA 使用 NVIDIA 容器运行时，其他 Profile 使用默认 Docker 运行时。接口不接收任意命令、环境变量、挂载、主机设备或特权参数。需自定义设备或卷的后端须通过各自的应用部署方案配置。

### 请求示例

VOS 基址为 `/api/com.ictrek.v-scheduler/scheduler/api/v1`。容器网络直连为 `http://v-scheduler:38120/api/v1`。传入 `Authorization: Bearer <token>`。JSON 请求使用 `Content-Type: application/json`。

```json
{
  "targets": [
    {"kind":"ssh","id":"node-35","profile":"jetson-thor"},
    {"kind":"cluster","id":"member-a","profile":"jetson-thor"}
  ],
  "repositories": ["registry.example.com/demo/backend"]
}
```

```json
{
  "request_key":"backend-1",
  "target":{"kind":"ssh","id":"node-35","profile":"jetson-thor"},
  "image":"registry.example.com/demo/backend:1.0",
  "container_port":8080
}
```

HTTP 200 返回 `code=OK` 以及 `data`。获取部署列表时 `data.items` 最多包含最近 200 条记录。应用只能查看和操作自己的实例，管理员可查看或清理各应用。授权接口和请求字段的完整协议见应用内 API 文档。

| 状态 | 含义与操作 |
| --- | --- |
| `pending` | 已排队或项目启动中，继续轮询。 |
| `running` | 运行时报告实例正在运行。最近状态检查失败时 `endpoint` 为 null。 |
| `failed` | 查看 `last_error`，修正授权、镜像或节点后重启。 |
| `stopping` / `deleting` | 等待运行时确认清理。 |
| `stopped` | 实例已退出或清理，保留端口供重新启动。 |
| `removed` | 确认清理并释放端口，保留幂等记录。重新部署时使用新的键。 |
| `cleanup_failed` | 清理未确认，检查节点并重试停止或删除。端口保持预留。 |

当创建接口返回 409，按业务码检查 Profile、幂等键或端口范围。操作处理中返回 `BACKEND_BUSY` 时，先轮询状态再重试。返回 503 时检查目标节点可达性或调度器数据库。若 `last_error` 为 `BACKEND_START_FAILED`，检查镜像拉取、宿主机端口与目标运行时；若为 `BACKEND_RUNTIME_UNAVAILABLE`，等待下一次状态检测并排查对端连接。

## English

Open **API Documentation → Scheduling API → Application backend deployments** in V-Scheduler. The application keeps its image list and selects the backend to deploy. V-Scheduler authorizes deployment, assigns a host port, and tracks the instance lifecycle through the same API for both node types.

1. An administrator registers a Docker-capable SSH target in **Node Management → SSH compute nodes**, or registers a VOS cluster member in **Advanced management → VOS Cluster Manager** and configures the trusted HTTPS peer. Targets require an IPv4 address. A cluster target runs the same V-Scheduler version with its Cluster Manager enabled and VOS Projects access. The administrator specifies the cluster target’s Profile in the deployment grant.
2. Create a service credential for the application under **Node Management → Service credentials**. Read SSH node IDs and `cluster_nodes[].id` with `GET /managed-nodes`. The administrator submits `targets` and `repositories` to `PUT /backend-deployment-access/{app_id}`. Each target specifies its `kind` (`ssh` or `cluster`), `id`, and registered `profile`. Repository entries include the registry and full path without a tag or digest. Each array accepts up to 64 entries. Empty arrays revoke new placement access. The application can read its own grant via GET on the same path.
3. The application calls `POST /backend-deployments` with its service credential, a stable `request_key`, the selected `target`, an `image` with tag or SHA-256 digest, and `container_port` (1 through 65535). The response records the queued request. Replay the same input with the same key to recover that record. Changed input with the same key returns a conflict. The target pulls the image using its existing registry access.
4. Poll `GET /backend-deployments/{id}`. When its state is `running` and it provides an `endpoint`, the application’s backend can connect to that address. Keep the application’s HTTP health check or HTTPS proxy when needed. Ports 42000–42999 are reserved per target IPv4 address and controller database. Reserve that range for these deployments to avoid competing services or controllers. Each instance exposes one TCP port.
5. Use `POST /backend-deployments/{id}/stop` to remove the runtime and keep the reservation. Use `POST /backend-deployments/{id}/start` to retry a stopped or failed deployment with its original configuration. Use `DELETE /backend-deployments/{id}` to release the port after cleanup is confirmed. Remove the deployments before deleting their SSH registration or changing its address.

The SSH adapter selects the Docker runtime from the Profile. The cluster adapter uses the V-Scheduler package’s `scheduler-app-backend` project. The same IPv4 address shares one port pool within the controller. Jetson Orin, Jetson Thor, DGX Spark, ARM CUDA and AMD CUDA select the NVIDIA runtime. Other profiles use the default Docker runtime. The API accepts an image and one port. Backends that need custom commands, environment variables, host mounts, devices or privileges require their own deployment configuration.

### API usage

Use `/api/com.ictrek.v-scheduler/scheduler/api/v1` under VOS or `http://v-scheduler:38120/api/v1` on the app network. Send `Authorization: Bearer <token>` and use `Content-Type: application/json` for JSON request bodies. The JSON examples in the Chinese section use the same protocol fields.

Successful requests return HTTP 200, `code=OK` and `data`. List responses return up to 200 recent deployments in `data.items`. Applications can read and manage their own deployments. Administrators can inspect and remove deployments across applications. Refer to the in-app API reference for request fields, full response schemas and error codes.

| State | Meaning and action |
| --- | --- |
| `pending` | Queued or deploying. Continue polling. |
| `running` | The runtime reports the instance running. A failed status check clears `endpoint` until recovery. |
| `failed` | Check `last_error`, then correct the grant, image or target and retry. |
| `stopping` / `deleting` | Wait for confirmation from the runtime. |
| `stopped` | The runtime has exited or been removed. Its port is retained for restart. |
| `removed` | Removal confirmed and port released. The original request key still identifies this record. Use a new key for a new deployment. |
| `cleanup_failed` | Cleanup could not be confirmed. The port remains reserved. Check the target and retry stop or removal. |

Use the business code on HTTP 409 to diagnose a Profile mismatch, conflicting request key, exhausted port pool or a busy lifecycle operation. When a request returns 503, check target or database availability. For `BACKEND_START_FAILED`, check the image pull, target runtime and host port. For `BACKEND_RUNTIME_UNAVAILABLE`, check the peer connection and wait for the next status probe.

## 在应用内登记与部署 / Register and deploy in the workspace

管理员打开「节点管理 → 应用后端」。可用节点区显示主机名、完整 IP、节点类型和 Profile。「登记应用」保存应用 ID、允许的镜像仓库和各目标的 Profile。「部署后端」选择已登记的 SSH 节点或 VOS 集群成员，并填写镜像、容器端口和部署参数。应用也可以用自己的服务凭据提交同一份部署请求。

Administrators can open **Node Management → Application backends**. Available targets show their hostname, address, node type and profile. **Register application** saves the application ID, approved repositories and target profiles. **Deploy backend** lets an administrator submit the selected SSH node or cluster member, image, port and settings. Applications can submit the same request with their own service credential.

### 镜像准备示例 / Image preparation example

同一个后端可分别发布 `registry.example.com/team/backend:jetson-orin-v1`、`:jetson-thor-v1`、`:arm-cpu-v1` 和 `:amd-cuda-v1`。Orin、Thor 和 ARM CPU 镜像使用 `linux/arm64`，AMD CUDA 镜像使用 `linux/amd64`。GPU 镜像还需验证目标节点的驱动和运行时。后端监听 `0.0.0.0:8080`，部署请求填入实际监听端口。生产可使用带 SHA-256 摘要的引用，以及已在目标 Profile 验证的镜像。

Publish separate tags such as `registry.example.com/team/backend:jetson-orin-v1`, `:jetson-thor-v1`, `:arm-cpu-v1` and `:amd-cuda-v1` for the same backend. Build the Orin, Thor and ARM CPU images for `linux/arm64` and the AMD CUDA image for `linux/amd64`. Verify driver and runtime compatibility for GPU images. Run the application on `0.0.0.0:8080` in this example and submit the actual listening port. For production, use a SHA-256 digest and images verified on their target profile.

```json
{
  "targets": [
    {"kind":"ssh","id":"node-orin","profile":"jetson-orin"},
    {"kind":"cluster","id":"member-thor","profile":"jetson-thor"}
  ],
  "repositories": ["registry.example.com/team/backend"]
}
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
