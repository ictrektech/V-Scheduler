> 此接口现由 V-Scheduler 模型仓库提供：内部 `http://v-scheduler:6005`，默认宿主机 `http://<vos-host>:36005`。共享目录、安装参数及完整接入见[模型管理](model-management.md)。

# 仓库文件下载 API / Repository File Downloads

HF 和 ModelScope 支持文件级下载；Ollama 仍按模型下载。所有路径均为仓库内相对路径，不允许绝对路径、`..`、反斜杠或通配符。API 仍通过现有下载任务执行，不返回文件内容。

## 查询远端文件 / Remote Files

`GET /api/v1/models/repository-files?source=hf&model_id=org/repo`

- `source`: `hf` 或 `ms`。
- `model_id`: `org/repo` 或匹配来源的 `hf://org/repo` / `ms://org/repo`。
- 只获取元数据，不下载权重。`size: null` 表示远端未提供大小；`total_size` 是已知大小之和。`downloaded` 表示同路径文件已存在于本地当前目录，不表示与远端最新版本哈希相同。

```json
{
  "model_id": "hf://org/repo",
  "source": "hf",
  "files": [
    {"path": "weights/model-Q4_K_M.gguf", "size": 4096, "downloaded": true},
    {"path": "weights/model-Q8_0.gguf", "size": 8192, "downloaded": false}
  ],
  "total_size": 12288
}
```

错误：`400` 来源/仓库参数无效；`422` 参数缺失或枚举无效；`502` 远端不可达、权限不足或元数据读取失败。私有/gated 仓库仍需部署环境具备有效平台凭据及访问权限。

## 下载和补下载 / Download And Add Files

`POST /api/v1/models/pull`

```json
{
  "model_id": "hf://org/repo",
  "source": "hf",
  "files": ["weights/model-Q4_K_M.gguf"],
  "append": true
}
```

- `files` 非空：只下载指定文件。省略、`null` 或 `[]` 保持旧行为，下载整个仓库。
- `append` 默认 `false`。设为 `true` 时必须提供非空 `files`，仅支持 HF/MS；仓库尚未下载时也可以使用。
- 仓库已 READY 时，用 `append: true` 添加新文件，无需 `force`。已落盘的文件不再下载；全部已存在时返回 `409`。原文件保留，新旧文件均可通过 `current` 访问；失败或取消恢复原 READY 模型。
- 分片权重请选择全部需要的分片，配置、VAE、文本编码器等依赖由调用方按实际工作流选择，不自动扩大下载范围。
- 同一模型只允许一个活跃下载；下载/删除冲突返回 `409`。路径验证失败返回 `422`。
- 返回 `{"task_id":"task-...","model_id":"hf://org/repo"}`。通过 `GET /api/v1/tasks` 或 `GET /api/v1/tasks/detail/{task_id}` 轮询进度，响应包含本次任务的 `files` 和 `append`；重试保留这两个参数。
- `GET /api/v1/models?status=all` 的 `files` 为累积选择范围。`null` 表示整仓/历史未知范围。READY 只表示请求范围完成，不能用于推断远端仓库已完整下载。后续 `/update` 保留累积文件范围。

再次添加 Q8：

```json
{
  "model_id": "hf://org/repo",
  "source": "hf",
  "files": ["weights/model-Q8_0.gguf"],
  "append": true
}
```

## 查询已下载文件 / Local Files

`GET /api/v1/models/files?model_id=hf%3A%2F%2Forg%2Frepo`

返回同样的文件列表结构，但只列当前本地可用目录的文件，所有 `downloaded` 均为 `true`；不包含缓存元数据、未完成文件或指向模型目录之外的链接。无需请求远端。模型不存在返回 `404`，Ollama 返回 `400`。

## Web 和安装配置 / Web And Installation

Web 下载页选择「选择文件 / 补下载」，读取文件列表后搜索、多选并提交。本地模型页可查看已下载文件，或点「添加文件」返回同仓库继续选择。

`HF_MIRROR_ENABLED` 是安装时的 boolean 开关，默认 `false`：关闭始终访问 `https://huggingface.co`；开启使用 `HF_ENDPOINT`（默认 `https://hf-mirror.com`，可设内部代理）。远端列表和权重下载使用同一端点。

English: HF/MS support repository-relative exact file paths through `files`. Use `append: true` to add new files without deleting existing weights; failed or cancelled additions restore the previous READY model. Remote and local file-list endpoints return `path`, nullable byte `size`, and `downloaded`. READY refers to the selected scope. The installer mirror switch defaults to off; enable it to use the configured mirror endpoint.
