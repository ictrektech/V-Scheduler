# 跨节点模型权重传输规范 / Cross-node model weight transfer

适用于 Image Studio 等需要在已部署计算节点上准备模型的应用。主应用读取本节点已准备的权重，向 V-Scheduler 的部署通道上传。Scheduler 根据 deployment ID 将请求转给该部署的受信 SSH 节点或 VOS 集群成员。权重落在目标 worker 的持久卷。应用以文件清单和回执处理传输，宿主机目录由部署方管理。

This flow prepares a model for an already deployed compute worker. The main application reads its prepared weights and sends them to the Scheduler deployment channel. Scheduler forwards requests for that deployment through its trusted SSH or VOS cluster connection. The target worker writes to its persistent volume. The caller works with file manifests and receipts.

应用可将支持此协议的发布镜像部署为 `application-backend-v1`，复用权重通道和异步模型登记。现有 Image Studio 的 `image-studio-worker-v1` 继续兼容。应用 worker 实现接收、校验和执行，Scheduler 管理授权、节点和任务租约。接入要求见[应用后端部署](application-backend-deployment.md)。

Applications can deploy a published image that implements this protocol in `application-backend-v1` mode to use weight transfer and asynchronous model registration. Existing Image Studio `image-studio-worker-v1` deployments remain compatible. Workers receive and verify files and run inference. Scheduler owns authorization, node selection and task leases. See [application backend deployments](application-backend-deployment.md).

## 入口与认证 / Entry point and identity

在 V-Scheduler 打开「API 文档 → 跨节点模型权重传输」。该标签提供接口索引、请求和响应字段，并可下载本规范。

Open **API Documentation → Cross-node model weight transfer** in V-Scheduler for the endpoint index, request and response fields, and this downloadable guide.

- VOS 同域 / VOS same origin: `https://<vos-host>/api/com.ictrek.v-scheduler/scheduler/api/v1`
- 内部服务 / Internal service: `http://v-scheduler:38120/api/v1`
- 下表从 `/backend-deployments/{id}` 开始，`id` 取实际运行中的部署 ID。每次请求使用 `Authorization: Bearer <scheduler-service-token>`。应用服务身份只能操作本应用的部署，管理员通过 VOS 登录操作。Image Studio 的浏览器管理员调用应用自己的准备接口，由应用后端使用其 Scheduler 服务身份。
- Paths below start at `/backend-deployments/{id}`, using the running deployment's ID. Send the application's Scheduler service bearer on each request. Application identity is scoped to its own deployment. VOS administrators use their VOS session. Image Studio's browser sends preparation requests to Image Studio, which uses its Scheduler service identity.

## 顺序及数据格式 / Sequence and payloads

| Method and suffix | Request / 请求 | Success `data` / 成功结果 |
| --- | --- | --- |
| `GET /model-hub` | Probe the target mount / 探测目标共享挂载 | `state` is `ready` or `unavailable` |
| `POST /transfers` | JSON manifest / JSON 清单 | Transfer receipt / 传输回执 |
| `GET /transfers/{transfer_id}` | Read the current offset after an interruption / 断线后读取当前偏移 | Transfer receipt |
| `PUT /transfers/{transfer_id}/files/{file_id}?offset=<bytes>` | `Content-Type: application/octet-stream`, 1 byte to 1 MiB / 二进制原始数据 | `received_bytes` |
| `POST /transfers/{transfer_id}/complete` | Empty body; verify size and SHA-256 on the worker / 空请求体，在 worker 上校验完整长度与摘要 | Transfer receipt with `verified: true` |
| `GET /models` | Empty request / 空请求 | `items[]` with `deployment_id`, `model_id`, `executor_id`, `node_id`, `profile`, `task_type` / 当前可选的模型部署 |
| `POST /models` | JSON `{"model_id":"sd-turbo","transfer_id":"<receipt id>"}` | `executor_id`, `node_id`, `profile`, `model_limits`, `model_parameters` |
| `DELETE /models/{model_id}` | Empty body. Worker must support model deletion. / 空请求体，需要支持删除的 worker。 | `deleted_transfers` is the number of staged or verified transfer records removed. / 本次清理的未完成及已校验传输记录数。 |

共享就绪时使用 `source: "shared"` 并在每个文件填相对于 Model Hub 根目录的 `shared_path`。若挂载探测为 `unavailable`，或共享准备返回 `409 BACKEND_SHARED_MODEL_HUB_UNAVAILABLE`，创建新的复制清单时省略 `source` 和所有 `shared_path`。传输和校验必须用同一部署的回执。

Use `source: "shared"` with a per-file `shared_path` relative to the Model Hub root when the share is ready. On `unavailable` or `409 BACKEND_SHARED_MODEL_HUB_UNAVAILABLE`, begin a copy manifest with both `source` and `shared_path` omitted. Use the receipt from that same deployment for upload and verification.

Example copy manifest / 复制清单示例（文件名、长度与摘要用实际值）:

```json
{
  "model_id": "sd-turbo",
  "files": [{
    "file_id": "model",
    "path": "sd_turbo.safetensors",
    "size_bytes": 4096,
    "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
  }]
}
```

- 按实际模型 ID（Image Studio 使用 `model_key[@variant]`）列全文件，包括主权重的 `model` 和该模型需要的辅助角色。每个 `file_id` 唯一。Image Studio 的角色 ID 为小写字母开头，最多 64 字符，由小写字母、数字和下划线组成。API 清单限 64 KiB、1–32 文件，每个文件 1 字节至 1 TiB。填写 64 位小写十六进制 SHA-256。
- `path` 只填目标文件原始 basename，长度 1–200 字节，禁止目录分隔符、控制字符和点开头的名字。清单不提供绝对路径、挂载参数、宿主机地址或 SSH 凭据。
- Include all required model files and roles (Image Studio uses `model_key[@variant]` and reserves `model` for the main weight). Use unique file IDs. Image Studio role IDs match `[a-z][a-z0-9_]{0,63}`. Manifest limit is 64 KiB, 1–32 files, each 1 byte–1 TiB. Use lowercase SHA-256 hex.
- `path` is the original filename only, 1–200 bytes. Directory separators, control characters and leading dots are rejected. Host paths, mount options, addresses and SSH credentials are outside the manifest.

Scheduler 成功信封中的 `data` 含 `transfer_id`、`model_id`、`source`（`copy` 或 `shared`）、`verified` 和 `files[]`，每个文件回执含 `file_id`、`received_bytes`。复用相同清单可继续同一传输。每次按回执的字节偏移继续写，从 0 开始，禁止跳过空洞或超出长度。中断后查询实际偏移，已接收区间仅允许重复上传相同字节。短暂通道错误可退避后查询回执。目标校验成功后才能登记。符合目标和资源预算要求时可直接登记，匹配实测校准时采用校准策略。校验失败先核对源文件及摘要，避免覆盖已登记的健康传输。

In the Scheduler success envelope, `data` has `transfer_id`, `model_id`, `source` (`copy` or `shared`), `verified`, and `files[]` with `file_id` and `received_bytes`. Begin again with the same manifest to resume it. Continue from the returned byte offset, starting at zero. Writes cannot skip ahead or exceed the declared size. Re-query the receipt after interruption. Retries of received data must match the persisted bytes. Back off after a transient channel failure, then query again. Register only once verified. A verified model can register under the direct policy when the target and budgets qualify. A matching measured calibration supplies the calibrated policy.

| Failure / 失败 | Recovery / 处理 |
| --- | --- |
| 401 / 403 | Verify identity and deployment access / 检查身份和部署权限 |
| 404 `BACKEND_TRANSFER_NOT_FOUND` | Check the deployment and IDs; recreate from the same manifest if absent / 核对部署及 ID，缺失时重建原清单 |
| 409 `BACKEND_STATE_CONFLICT` | Wait for the deployment to run / 等待部署进入运行状态 |
| 409 `BACKEND_TRANSFER_CONFLICT` | Re-read offsets and check completeness / 重读偏移并检查文件是否已完整 |
| 409 `BACKEND_TASKS_ACTIVE` | Wait for jobs using this model to finish before deleting / 等待使用该模型的任务结束后再删除 |
| 409 `BACKEND_MODEL_DELETING` | Retry pending deletion before preparing or dispatching this model / 完成该模型的待处理删除后再准备或调度 |
| 409 `BACKEND_MODEL_DELETE_UNSUPPORTED` | Update the deployment's worker image / 更新该部署的计算后端镜像 |
| 413 / 415 | Keep chunks within 1 MiB and set their content type / 校验分块长度、使用正确 Content-Type |
| 422 `BACKEND_TRANSFER_INVALID` | Check fields, model roles, file bytes and SHA-256 / 检查清单、文件角色、内容和摘要 |
| 429 / 502 | Back off, then read the receipt before sending more bytes / 退避并读取回执后再续传 |

## Image Studio 实现 / Image Studio implementation

管理员从 Image Studio 的计算节点页面准备模型，或用 `POST /api/v1/compute/deployments/{deployment_id}/models` 提交 `model_key` 与可选的 `model_variant`。应用读取已准备源文件，探测共享、回退到复制并登记，浏览器无需接触通道密钥。worker 的默认路径为：

The administrator uses Image Studio's compute page or `POST /api/v1/compute/deployments/{deployment_id}/models` with `model_key` and optional `model_variant`. The application reads prepared source weights, probes shared storage, falls back to transfer and registers the model. Worker storage defaults to:

```text
$IMAGE_STUDIO_DATA_DIR/weight-transfers/<transfer_id>/
  manifest.json
  verified.json
  <file_id>/<filename>
```

默认 `IMAGE_STUDIO_DATA_DIR=/data`。这是 Image Studio worker 的持久化布局，其他应用按自身 worker 的持久存储实现相同接口。主应用以回执为准，运行时仅使用已校验文件。共享模式使用只读 `/modelhub-shared`，只为验证及登记保存记录，无需重复写入权重副本。

`IMAGE_STUDIO_DATA_DIR` defaults to `/data`. This is Image Studio's worker layout. Other applications implement the same channel contract with their own worker persistence. The worker uses verified files for execution. Shared mode reads the read-only `/modelhub-shared` mount and persists verification metadata without copying weights.

删除接口清理目标端指定模型的所有传输记录和本地副本，再解除调度登记。目标端保留主节点和 NFS 上的文件。中途中断时重试相同的 `DELETE`，成功回执允许返回零。Image Studio 的管理员界面和 `DELETE /api/v1/compute/deployments/{deployment_id}/models/{model_id}` 使用这条流程。

Deletion removes this model's transfer records and copied weights from the target, then removes its scheduling registration. It preserves the main-node and NFS files. Retry the same `DELETE` after an interruption. A successful retry may report zero. Image Studio's administrator UI and `DELETE /api/v1/compute/deployments/{deployment_id}/models/{model_id}` use this flow.
