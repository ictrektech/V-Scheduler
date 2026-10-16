> 迁移说明：网关现由 V-Scheduler 统一发布，Rust 源码位于 `backend/src/router/`；文中 Go 测试命令仅为历史记录。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# V-Music 与 ML Server 接口

V-Router 每 30 秒自动发现同一 VOS 内的 V-Music 与 Immich ML Server。调用方先读取 `GET /v1/models`，使用带来源前缀的模型 ID；服务缺失时相关模型会静默移除，不影响其他上游。

## V-Music

发现顺序为 `http://v-music-backend:7080`、`http://host.docker.internal:22701`，仅公开 discovery 中 ready 且声明端点的模型。

### `POST /v1/audio/music/generations`

这是 OpenAI 风格扩展。V-Router 只把 `v-music/<模型>` 还原为原生 ID，其余 JSON 字段与 WAV/FLAC 响应不改写。

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `model` | 是 | `v-music/` 前缀模型 ID |
| `prompt` | 是 | 音乐描述，1–4000 字符 |
| `duration` | 否 | 秒数，默认 30，实际上限由模型决定 |
| `negative_prompt` | 否 | 不希望出现的内容 |
| `seed` | 否 | 随机种子 |
| `bpm` | 否 | 20–300 |
| `key` / `time_signature` | 否 | 调性 / 拍号 |
| `instrumental` | 否 | 默认 true |
| `response_format` | 否 | `wav` 或 `flac`，默认 `wav` |

```bash
curl http://<VOS-IP>:22888/v1/audio/music/generations \
  -H 'Content-Type: application/json' \
  -d '{"model":"v-music/stable-audio-3-small-music","prompt":"warm cinematic ambient music","duration":10,"instrumental":true,"response_format":"wav"}' \
  --output music.wav
```

## Immich ML Server

发现顺序为 `http://ml-server:3003`、`http://host.docker.internal:3003`。只有 `GET /` 返回 `service=immich-ml-server`、`status=running`，且 `/openapi.json` 声明对应路径时才注册模型。公开前缀为 `ml-server/`。

适配模型：`qwen3-embedding-0.6b`、`ViT-B-16-SigLIP2__webli`、`buffalo_l`、`osnet_ain_x1_0`、`PP-OCRv5_server`、`video-structure`；最终以目标机发现结果为准。

### `POST /v1/embeddings`（OpenAI 兼容）

适用于 Qwen3 和 CLIP 文本向量。`input` 支持字符串或非空字符串数组；返回 OpenAI `object=list`、`data[].embedding` 数值数组及索引。ML Server 不报告 tokenizer 用量，因此不伪造 `usage`。

```bash
curl http://<VOS-IP>:22888/v1/embeddings \
  -H 'Content-Type: application/json' \
  -d '{"model":"ml-server/qwen3-embedding-0.6b","input":["海边日落","雪山木屋"]}'
```

### `POST /v1/ml/predictions`（V-Router 扩展）

OpenAI 没有统一的人脸、图片向量、ReID、OCR 端点，因此使用 multipart 扩展。`model`、`image` 必填；V-Router 根据模型生成 ML Server `entries`。响应保持上游格式，向量字段仍是包含 JSON 数组的字符串。

| 模型 | 结果 | 可选参数 |
| --- | --- | --- |
| `ml-server/ViT-B-16-SigLIP2__webli` | CLIP 图片向量 | 无 |
| `ml-server/buffalo_l` | 人脸框、512 维向量、关键点、正脸分数 | `min_score` |
| `ml-server/osnet_ain_x1_0` | 行人 512 维向量及可选占比 | 无 |
| `ml-server/PP-OCRv5_server` | 文字框、框分数、文本、文本分数 | `min_score`、`max_resolution`、`score_mode=fast|slow` |

```bash
curl http://<VOS-IP>:22888/v1/ml/predictions \
  -F model=ml-server/PP-OCRv5_server -F image=@document.jpg \
  -F min_score=0.5 -F max_resolution=736 -F score_mode=fast
```

### `POST /v1/ml/video/structure`（V-Router 扩展）

`model`、`asset_id`、`video_path` 必填；`video_path` 是 ML Server 共享存储内的相对路径，不是上传 URL。可选 `detection.threshold`，以及逐场景 `tasks.facial-recognition`、`tasks.clip.visual`、`tasks.person-reid`、`tasks.ocr`。V-Router 移除路由字段 `model` 后转发。

```bash
curl http://<VOS-IP>:22888/v1/ml/video/structure \
  -H 'Content-Type: application/json' \
  -d '{"model":"ml-server/video-structure","asset_id":"asset-001","video_path":"video/example.mp4","detection":{"threshold":0.5}}'
```

### `GET /v1/ml/video/structure/tasks/{task_id}`（V-Router 扩展）

```bash
curl 'http://<VOS-IP>:22888/v1/ml/video/structure/tasks/<task-id>?model=ml-server%2Fvideo-structure'
```

状态为 `queued`、`running`、`succeeded` 或 `failed`。终态是消费式读取：第一次查询到 `succeeded`/`failed` 后任务被删除，之后同 ID 返回 404；默认未读取终态保留 3600 秒。

## 不公开的 ML Server 路径

`GET /status`、`GET /status/logs` 是运维页面/日志，`POST /api/video-structure/callback` 是内部回调，不作为 V-Router 消费接口。`GET /ping`、`GET /` 只用于发现。原生 `/predict` 不直接透出，以免绕过模型能力、身份归因和路由约束。

应用内“API 文档”和 `GET /api/v1/openapi.json` 与运行时路由表同源，也列出上述端点、字段和调用示例。
