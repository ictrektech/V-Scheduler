> 迁移说明：网关现由 V-Scheduler 统一发布，Rust 源码位于 `backend/src/router/`；文中 Go 测试命令仅为历史记录。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# V-Router 接入 V-Voice 语音能力指南

V-Router 自动把同 VOS 的 V-Voice ready 模型合并到公共模型目录，模型 ID 使用
`v-voice/<原生模型 key>`，覆盖四类能力：整句 ASR（含热词与时间戳）、流式
ASR（HTTP 分块与 WebSocket）、TTS、声纹识别/验证。V-Voice 模型不按用户
隔离；V-Router 仍负责调用方认证、用量记录、模型路由和模型 ID 转换。

调用方只需要 V-Router 的统一入口：

| 调用位置 | Base URL |
|---|---|
| VOS 内应用（同域） | `/api/com.ictrek.v-scheduler` |
| VOS 内容器 | `http://v-router:8080/v1` |
| 局域网客户端 | `http://<vos-host>:22888/v1` |

鉴权：VOS 内携带当前用户 OIDC token；内网未强制 Key 时可匿名推理。
所有模型引用（`model` 与 `speaker_model`）都填 `v-voice/` 前缀的网关模型
ID，V-Router 在转发边界还原原生 ID，调用方不需要知道 V-Voice 原生命名。

## 能力发现

先读取管理模型列表，按 `endpoints` 与 `capabilities` 筛选：

- 整句 ASR：`audio/transcriptions` endpoint + `asr` capability；
- 热词：`hotwords` capability；
- 词/句时间戳：`word_timestamps` / `segment_timestamps` capability；
- 流式：`streaming` capability，对应 `audio/streaming/transcriptions` 与
  `audio/streaming/ws` endpoint；
- 声纹：`speaker` capability 与 `audio/speakers*` endpoint。

```bash
curl -fsS "$V_ROUTER_URL/api/v1/models" \
  -H "Authorization: Bearer $VOS_TOKEN"
```

## 转写请求

`POST /v1/audio/transcriptions` 使用 `multipart/form-data`。V-Router 将
`v-voice/` 模型 ID 还原为 V-Voice 原生 ID，除此之外完整保留每个 multipart
part、文件名、Content-Type、上游响应头、响应体和状态码。

| 字段 | 类型 | 必填 | 默认 | 说明 |
|---|---|---:|---|---|
| `file` | binary | 是 | - | 待识别音频，V-Router 整体请求上限 32 MiB |
| `model` | string | 是 | - | 带来源前缀的 V-Router 模型 ID |
| `language` | string | 否 | 自动 | 模型支持的语言代码 |
| `response_format` | string | 否 | `json` | `json`、`verbose_json`、`text`、`srt`、`vtt` |
| `hotwords` | string | 否 | 空 | 逗号或换行分隔，和文件内容合并去重 |
| `hotwords_set` | string，可重复 | 否 | - | 已保存热词集 ID，与 `hotwords`、`hotwords_file` 在服务端合并去重 |
| `hotwords_file` | UTF-8 file | 否 | - | 每行一个词组，最大 64 KiB |
| `hotwords_score` | number | 否 | `1.5` | `0..10`，只用于 Zipformer |
| `timestamp_granularities[]` | string，可重复 | 否 | - | `word`、`segment`；请求 OpenAI 风格时间戳 |
| `speaker_model` | string | 否 | 空 | 已就绪声纹模型 ID（如 `v-voice/speaker-campplus-zh-en`）；提供时 `verbose_json` 附带 `speaker` 归属块 |
| `speaker_threshold` | number | 否 | `0.6` | `0..1`；`speaker` 块余弦相似度阈值 |

Qwen3-ASR 示例：

```bash
curl -fsS "$V_ROUTER_URL/v1/audio/transcriptions" \
  -H "Authorization: Bearer $VOS_TOKEN" \
  -F model=v-voice/qwen3-asr-0.6b \
  -F file=@sample.wav \
  -F language=zh \
  -F 'hotwords=华为,昇腾,V-Voice' \
  -F hotwords_file=@hotwords.txt \
  -F 'timestamp_granularities[]=word' \
  -F 'timestamp_granularities[]=segment' \
  -F response_format=verbose_json
```

Zipformer 示例：

```bash
curl -fsS "$V_ROUTER_URL/v1/audio/transcriptions" \
  -H "Authorization: Bearer $VOS_TOKEN" \
  -F model=v-voice/zipformer-en \
  -F file=@english.wav \
  -F 'hotwords=OpenAI,V-Voice,sherpa-onnx' \
  -F hotwords_score=2.0 \
  -F response_format=json
```

热词文本、热词集和文件最多合并为 64 个唯一词组、合计 4096 UTF-8 bytes。文件内容是
请求数据，不是 V-Voice 或 V-Router 宿主机路径。已保存热词集在 V-Voice 管理面
维护（`/api/v1/hotwords`），不属于 V-Router 代理协议；`verbose_json.hotwords`
中的 `sets` 列出实际生效的热词集。

## OpenAI `words` / `segments`

V-Router 不转换或裁剪 ASR JSON。选择 `response_format=verbose_json` 时，
V-Voice 会在底层模型有 sherpa token 时间戳的情况下返回：

```json
{
  "text": "After early nightfall ...",
  "duration": 6.72,
  "words": [{"word": "After", "start": 0.36, "end": 0.84}],
  "segments": [{"id": 0, "start": 0.36, "end": 6.6, "text": "After early nightfall ..."}]
}
```

当前 SenseVoice、Paraformer 和 Zipformer 在 discovery 中声明
`word_timestamps`、`segment_timestamps`。Qwen3-ASR 0.6B 与 Whisper Tiny
当前返回空的 `words`/`segments`，因为底层没有时间戳；网关和 V-Voice 都不会
伪造。需要词级字幕或删词剪辑的客户端必须筛选 discovery capability，并重复
提交 `timestamp_granularities[]=word`。时间单位为秒。

## TTS voice 联动

V-Router 代理 `GET /v1/audio/voices?model=v-voice/<模型 key>`，会把带来源前缀
的模型 ID 还原后调用 V-Voice。响应包含 `model`、`default_voice` 以及同值的
`voices[]`/`data[]` 列表，每个元素至少有 `id` 和 `name`。客户端应在 TTS 模型
变化时重新读取该接口生成下拉框，提交所选 `id`，不得允许任意输入：

```bash
curl -fsS "$V_ROUTER_URL/v1/audio/voices?model=v-voice/kokoro-multilang" \
  -H "Authorization: Bearer $VOS_TOKEN"
```

## 流式识别与声纹

带 `streaming` capability 的模型（`zipformer-*-streaming`）支持两条额外通道。
两者都按 `model` 查询参数路由；V-Router 边到边透传、不整体缓冲、不设整体
超时，连接存续期间占用该模型一个并发槽。

### `POST /v1/audio/streaming/transcriptions`（HTTP 分块）

请求体是 chunked 音频字节流，参数全部经查询字符串传递：

| 查询参数 | 类型 | 必填 | 默认 | 说明 |
|---|---|---:|---|---|
| `model` | string | 是 | - | `v-voice/<流式模型 key>` |
| `format` | string | 否 | `pcm` | `pcm`（s16le 单声道原始字节）、`wav`（边解析 header 边流）、`auto`（缓冲后整体解码） |
| `sample_rate` | integer | 否 | `16000` | `format=pcm` 的采样率；`wav` 用文件头 |
| `hotwords` | string | 否 | 空 | 一次性热词，逗号/换行分隔（流式 Zipformer 支持） |
| `hotwords_set` | string，可重复 | 否 | - | 已保存热词集 ID，与 `hotwords` 合并去重 |
| `hotwords_score` | number | 否 | `1.5` | `0..10`，仅流式 Zipformer |
| `response_format` | string | 否 | `verbose_json` | `verbose_json`、`json`、`text` |
| `speaker_model` | string | 否 | 空 | `v-voice/<声纹模型 key>`；最终响应附带 `speaker` 归属块 |
| `speaker_threshold` | number | 否 | `0.6` | `0..1` |

```bash
curl -N -X POST "$V_ROUTER_URL/v1/audio/streaming/transcriptions?model=v-voice/zipformer-zh-streaming&format=pcm" \
  -H "Authorization: Bearer $VOS_TOKEN" \
  -H 'Transfer-Encoding: chunked' \
  --data-binary @audio.pcm
```

body 结束返回最终转写；`verbose_json` 字段与整句接口一致并附加
`"streaming": true`。非流式模型调用该接口不会进入上游：V-Router 按
`endpoints` 过滤后直接返回 400。

### `GET /v1/audio/streaming/ws`（WebSocket 实时识别）

连接参数与上表相同（`format` 仅 `pcm`/`wav`），握手后：

1. 客户端循环发送**二进制帧**：s16le 单声道 PCM 分片，建议 100ms/片；
2. 服务端在识别推进时回 JSON 文本帧：
   `{"type":"partial","text":"...","is_endpoint":false}`；
3. 客户端发送文本帧 `{"type":"finish"}`（或直接断开）后，服务端返回
   `{"type":"final","text":"...","tokens":[...],"timestamps":[...]}` 并关闭；
   连接参数带 `speaker_model` 时 `final` 附带 `speaker` 归属块；
4. 错误以 `{"type":"error","detail":"..."}` 回报，并按关闭码
   `4404`（模型不存在）、`4409`（未就绪）、`4422`（参数/能力不匹配）、
   `4500`（推理失败）关闭。V-Router 原样透传帧与关闭码。

```python
# pip install websockets
import asyncio, json, websockets

async def recognize(pcm_chunks):
    async with websockets.connect(
        f"{WS_URL}/v1/audio/streaming/ws?model=v-voice/zipformer-zh-streaming&format=pcm",
        extra_headers={"Authorization": f"Bearer {token}"},
    ) as ws:
        for chunk in pcm_chunks:          # 100ms s16le mono 16kHz
            await ws.send(chunk)
            while True:                    # 先收完已产生的 partial
                try:
                    msg = json.loads(await asyncio.wait_for(ws.recv(), 0.01))
                    if msg["type"] == "partial":
                        print("partial:", msg["text"])
                except asyncio.TimeoutError:
                    break
        await ws.send(json.dumps({"type": "finish"}))
        final = json.loads(await ws.recv())
        print("final:", final["text"])
```

### 声纹（说话人）接口

`kind=speaker` 的模型（`speaker-campplus-zh-en`）在 V-Router 下提供三个公共
推理接口。声纹注册/删除属于 V-Voice 管理面（`POST /api/v1/speakers`、
`DELETE /api/v1/speakers/{id}`，宿主机级共享），不在 V-Router 代理范围；
客户端经 V-Router 只做列表、识别与验证。

`GET /v1/audio/speakers?model=v-voice/speaker-campplus-zh-en`：

```json
{
  "object": "list",
  "model": "speaker-campplus-zh-en",
  "default_threshold": 0.6,
  "speakers": [{"id": "sp_1a2b3c4d", "name": "张三", "dimension": 192, "created_at": "..."}],
  "data": [{"id": "sp_1a2b3c4d", "name": "张三", "dimension": 192, "created_at": "..."}]
}
```

`speakers[]` 与 `data[]` 是同值别名；voiceprint 向量不返回。

`POST /v1/audio/speakers/identify`（1:N 归属，multipart）：

| 字段 | 类型 | 必填 | 说明 |
|---|---|---:|---|
| `model` | string | 是 | `v-voice/<声纹模型 key>` |
| `file` | file | 是 | 待归属音频 |
| `threshold` | number | 否 | `0..1`，默认 `0.6` |

响应：`identified`（是否命中）、`speaker`（`{id, name, score}`，未命中为
`null`）、`score`、`threshold`、`ranked[]`（Top-5）。

`POST /v1/audio/speakers/verify`（1:1 验证，multipart）：字段同上，另必填
`speaker`（声纹 `id` 或名称）；响应 `verified`、`score`、`threshold`、
`speaker`。`speaker` 不存在由上游返回 `404`。

```bash
curl "$V_ROUTER_URL/v1/audio/speakers/identify" \
  -H "Authorization: Bearer $VOS_TOKEN" \
  -F model=v-voice/speaker-campplus-zh-en -F file=@sample.wav

curl "$V_ROUTER_URL/v1/audio/speakers/verify" \
  -H "Authorization: Bearer $VOS_TOKEN" \
  -F model=v-voice/speaker-campplus-zh-en -F speaker=张三 -F file=@sample.wav
```

### 组合归属：转写 + 声纹一次完成

整句与流式转写都接受 `speaker_model`（填 `v-voice/` 前缀的声纹模型 ID，
V-Router 在转发边界还原原生 ID）与 `speaker_threshold`。提供时
`verbose_json` 附带 `speaker` 归属块：

```json
{
  "text": "...",
  "speaker": {
    "model": "speaker-campplus-zh-en",
    "threshold": 0.6,
    "identified": true,
    "speaker": {"id": "sp_1a2b3c4d", "name": "张三", "score": 0.83},
    "score": 0.83,
    "ranked": [{"id": "sp_1a2b3c4d", "name": "张三", "score": 0.83}]
  }
}
```

音频过短无法提取声纹时 `speaker` 块以 `error` 字段说明且不影响转写结果；
声纹库为空时 `identified=false`、`ranked=[]`。V-Router 不裁剪该块。

## 翻译请求

`POST /v1/audio/translations` 接受完全相同的字段并按同样方式透传。模型必须同时
声明 translation endpoint 和 hotwords capability；最终是否能把语音翻译为英文
仍由所选模型决定。

## 响应与错误

| 状态码 | 含义 |
|---:|---|
| `200` | V-Voice 推理成功；响应格式由 `response_format` 决定 |
| `400` | V-Router 路由拒绝：模型未声明该 endpoint（如非流式模型调流式接口、非声纹模型调 speakers） |
| `401` / `403` | V-Router 调用身份无效或权限不足 |
| `404` | 模型不存在、未被发现或类型不匹配；verify 的 `speaker` 不存在 |
| `409` | V-Voice 模型文件尚未 ready |
| `413` | V-Router 32 MiB 请求上限或 V-Voice 音频限制 |
| `422` | 支持热词的模型收到非法 UTF-8、超限热词、越界分数或其他字段校验失败 |
| `429` | 当前候选上游达到 V-Router 并发上限（含流式连接占用的槽） |
| `500` | V-Voice 推理失败 |

错误体保持上游的 `{"detail":"..."}` 或 V-Router 标准错误格式。客户端不得把
`422` 当成可重试错误；应改正字段内容。模型本身不支持热词不会触发错误。
WebSocket 通道的错误以上行关闭码表达（4404/4409/4422/4500），不走 HTTP 状态码。

## 接入检查表

把应用接入 V-Router 的 V-Voice 语音能力时，至少验证：

1. `/api/com.ictrek.v-scheduler/v1/models`（同域）或局域网 `/v1/models` 能列出
   所需的 `v-voice/` 模型；按 `api/v1/models` 的 `endpoints`/`capabilities`
   筛选能力，不要硬编码模型清单。
2. 整句转写：真实音频调用 `/v1/audio/transcriptions`，`model` 带
   `v-voice/` 前缀，收到 `{"text": ...}`。
3. 热词：对声明 `hotwords` 的模型提交 `hotwords`/`hotwords_set`/
   `hotwords_file`，用 `verbose_json.hotwords` 确认 `applied` 与生效
   `sets`；对不支持热词的模型确认字段被忽略且转写正常。
4. 时间戳：对声明 `word_timestamps` 的模型重复提交
   `timestamp_granularities[]=word` 并校验 `words[]` 非空。
5. 流式：流式模型走 chunked 或 WebSocket 通道，客户端按 100ms 分片上行，
   收到 `partial`/`final`；断开客户端后确认无残留并发占用。
6. 声纹：在 V-Voice 管理面注册声纹后，经 V-Router 完成 speakers 列表、
   identify、verify 各一次；`speaker_model` 组合归属确认 `verbose_json`
   含 `speaker` 块。
7. 用量：管理界面「用量」页能看到上述调用按用户归集。

## 接口契约

- 管理界面的“API 文档”来自 `GET /api/v1/apidoc`，本页所有端点的参数、
  示例与限制随版本实时生成。
- OpenAPI 3.1 从 `GET /api/v1/openapi.json` 下载，multipart schema 明确列出
  `hotwords`、`hotwords_set`、`hotwords_file`、`hotwords_score`、
  `speaker_model`、`speaker_threshold`、`timestamp_granularities[]`。
- 流式与声纹端点按 `model` 查询参数路由；`speaker_model` 填 `v-voice/`
  前缀 ID，V-Router 在转发边界还原原生 ID。
- `X-VRouter-Upstream` 不是普通 V-Voice 调用的必填字段；按模型 ID 路由即可。
- V-Router 不读取、分词、重新编码或保存热词与音频，只做二进制安全透传；
  声纹库与热词集数据都保存在 V-Voice 侧。
