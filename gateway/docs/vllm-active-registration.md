> 迁移说明：网关现由 V-Scheduler 统一发布，源码位于 `gateway/`。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# vLLM 主动注册为 public / private 模型开发指南

本文说明 VOS 内或宿主机上的 vLLM 服务如何主动注册到 V-Router，并选择成为所有用户可见的 `public` 模型，或仅注册账户可见的 `private` 模型。

vLLM 本身不会调用 V-Router 注册接口，因此“主动注册”应由 vLLM 的启动脚本、进程管理器或同容器网络中的轻量 sidecar 实现。注册器必须等 vLLM 真正可用后再注册，并负责心跳、重新注册和退出注销。

## 1. public 和 private 的区别

| `visibility` | 模型可见性和调用权限 | 推荐场景 |
| --- | --- | --- |
| `public` | 所有 VOS 用户可见、可调用；V-Router 未强制鉴权时，内网匿名调用也可见 | 共享 GPU 推理节点、供整台 VOS 使用的公共模型 |
| `private` | 只有注册者同一 VOS 身份和管理员可见、可调用 | 个人模型、测试模型、按用户隔离的推理实例 |

可见性不等于所有权：无论模型是 `public` 还是 `private`，注册时都会记录 owner。后续更新、心跳和注销只能由原 owner 或管理员完成。

使用 VOS OIDC token 注册时，owner 是 userinfo 返回的稳定 `identity.sub`。同一账户的不同 access token 只要解析到相同 `sub`，都可以管理该注册。使用 V-Router 共享网关 Key 注册时，owner 是 `gateway-admin`。

建议：

- 按用户隔离的 `private` 模型必须使用该用户的 VOS OIDC token 注册。
- 系统级 `public` vLLM 可以使用专门的 VOS 管理账户 token，或由受控后端使用网关 Key 注册。
- 不要把 OIDC token 或网关 Key 写入镜像、仓库、前端代码或日志。

## 2. 网络和 vLLM 前置条件

标准 vLLM OpenAI 服务通常监听：

```text
http://vllm-backend:8000/v1
```

注册的 `base_url` 必须从 **V-Router 容器内部**可达，并且 vLLM 服务需至少满足：

```http
GET  /v1/models
POST /v1/chat/completions
```

如果 vLLM 和 V-Router 都运行在 VOS 中，推荐加入 `vos_default` 网络并使用稳定别名：

```yaml
services:
  vllm-backend:
    image: ${VLLM_IMAGE}
    command:
      - --model
      - /models/Qwen3-8B
      - --served-model-name
      - qwen3-8b
      - --host
      - 0.0.0.0
      - --port
      - "8000"
    networks:
      vos_default:
        aliases:
          - vllm-backend

networks:
  vos_default:
    external: true
```

此时注册地址填写 `http://vllm-backend:8000/v1`。不要填写 `localhost`；在 V-Router 容器中，`localhost` 指向 V-Router 自己。

跨主机注册必须使用两台机器之间真实可达的内网 IP，不能使用 `tc192`、`tc229` 等仅供 SSH 客户端解析的别名。例如 vLLM 在 `192.168.1.229:22222`、V-Router 所在 VOS 在 `192.168.1.192` 时，注册的 `base_url` 应为：

```text
http://192.168.1.229:22222/v1
```

注册前先在 V-Router 所在的 `192.168.1.192` 上验证：

```bash
curl --fail --max-time 8 http://192.168.1.229:22222/v1/models
```

这一步必须返回 2xx 和真实模型列表。不要仅凭 vLLM 宿主机本机访问成功，就认定 V-Router 所在机器或容器也能访问。

若 vLLM 启用了 `--api-key`，注册负载中的 `api_key` 填 vLLM 自己的密钥。它只用于 V-Router 调用 vLLM，与调用注册接口时使用的 VOS token / 网关 Key 是两种凭证。

### 2.1 低资源测试实例

验收注册功能时优先复用已经运行的健康 vLLM，避免重复加载模型。如果必须临时启动，选择最小的完整模型，并同时限制上下文、批处理和并发：

```bash
vllm serve /data/models/<small-model> \
  --served-model-name vrouter-registration-test \
  --host 0.0.0.0 \
  --port 22229 \
  --max-model-len 1024 \
  --max-num-seqs 1 \
  --max-num-batched-tokens 1024 \
  --enforce-eager \
  --no-enable-prefix-caching
```

以上参数名已在 vLLM `0.27.1` 验证；其他版本先以 `vllm serve --help` 为准。例如部分版本使用 `--no-enable-prefix-caching`，而不是 `--disable-prefix-caching`。

不要为了“低占用”盲目把 `--gpu-memory-utilization` 调得很低。该值是 vLLM 的总内存预算，并不只是 KV cache 比例；低于“模型权重 + 引擎开销 + 最小 cache block”时会在权重加载后报 `No available memory for the cache blocks`。应先用小模型、短上下文和单并发降低需求，再根据该主机的实测余量设置内存预算。

临时容器退出后必须同时检查容器列表和 `nvidia-smi`。vLLM 采用多进程，异常启动可能留下 `VLLM::EngineCore`；只清理能够确认属于本次测试的 PID，不得按进程名批量终止其他 vLLM。

## 3. 推荐方式：探测 vLLM `/v1/models`

标准 vLLM 服务应采用模型 API 注册。V-Router 会在接受注册前请求上游 `/models`；探测失败时注册返回 502，不会留下一个虚假的健康模型。

先确认注册身份：

```bash
curl "$V_ROUTER_URL/api/v1/auth/me" \
  -H "Authorization: Bearer $V_ROUTER_REGISTRATION_TOKEN"
```

### 注册为 public

```bash
curl "$V_ROUTER_URL/api/v1/registrations" \
  -H "Authorization: Bearer $V_ROUTER_REGISTRATION_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "service_id": "tc229-vllm-qwen3-1",
    "name": "tc229 vLLM Qwen3 1",
    "visibility": "public",
    "base_url": "http://vllm-backend:8000/v1",
    "protocol": "chat_completions",
    "model_kind": "chat",
    "ttl_seconds": 90
  }'
```

### 注册为 private

请求内容相同，只把 `visibility` 改为 `private`，并确保 Bearer token 属于目标 VOS 用户：

```bash
curl "$V_ROUTER_URL/api/v1/registrations" \
  -H "Authorization: Bearer $VOS_USER_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "service_id": "tc229-vllm-qwen3-private-1",
    "name": "My tc229 vLLM Qwen3",
    "visibility": "private",
    "base_url": "http://vllm-backend:8000/v1",
    "protocol": "chat_completions",
    "model_kind": "chat",
    "ttl_seconds": 90
  }'
```

这里使用 `chat_completions`，表示上游原生提供 Chat Completions，V-Router 可以按自身协议能力把 `/v1/responses` 和 `/v1/messages` 转换到 Chat Completions。若 vLLM 实例确实原生支持需要透传的其他 OpenAI 接口，也可选择 `openai`，但不要声明模型实际不支持的能力。

假设 vLLM `/v1/models` 返回模型 ID `qwen3-8b`，注册后：

- 稳定上游 ID：`registered-tc229-vllm-qwen3-1`
- V-Router 模型 ID：`registered/qwen3-8b`

`visibility` 省略时默认为 `public`，但建议始终显式填写，避免部署意图不清楚。

## 4. 只注册指定模型和接口

如果不希望 V-Router 导入 `/v1/models` 返回的全部模型，可以显式注册一个模型。显式模型注册不会探测 `/models`，因此必须准确声明 `endpoints`：

```bash
curl "$V_ROUTER_URL/api/v1/registrations" \
  -H "Authorization: Bearer $VOS_USER_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "service_id": "tc229-vllm-qwen3-private-1",
    "name": "My tc229 vLLM Qwen3",
    "visibility": "private",
    "base_url": "http://vllm-backend:8000/v1",
    "protocol": "chat_completions",
    "model": "qwen3-8b",
    "model_kind": "chat",
    "endpoints": ["chat_completions", "responses", "messages"],
    "capabilities": ["chat", "completion", "tools"],
    "ttl_seconds": 90
  }'
```

只有确认该模型及当前 vLLM 启动参数支持工具调用时，才加入 `tools`。如果只支持基础对话，应移除它。不要为了让模型出现在更多界面里虚报接口或能力。

一个 vLLM API 暴露多个不同类型模型时，可以为探测结果逐模型覆盖：

```json
{
  "service_id": "tc229-vllm-mixed-1",
  "visibility": "public",
  "base_url": "http://vllm-backend:8000/v1",
  "protocol": "openai",
  "model_endpoints": {
    "qwen3-8b": ["chat_completions", "responses"],
    "bge-m3": ["embeddings"]
  },
  "model_capabilities": {
    "qwen3-8b": ["chat", "completion", "tools"],
    "bge-m3": ["embedding"]
  },
  "ttl_seconds": 90
}
```

## 5. 多副本和稳定 service_id

每个真实 vLLM 副本必须使用不同、稳定的 `service_id`：

```text
tc229-vllm-qwen3-1
tc229-vllm-qwen3-2
```

同一副本重启后继续使用原 ID。V-Router 会把它们识别为两个可独立调度的上游实例。不要让多个存活副本共用一个 ID，否则它们会不断覆盖同一租约和地址，无法正确反映健康状态与并发。

同一个 owner 重新提交相同 `service_id` 会更新注册；其他 owner 尝试接管会收到 409。更换模型、Base URL、协议、可见范围、vLLM API Key 或能力声明时，应重新提交完整注册请求。

## 6. 心跳、重新注册和注销

租约默认 90 秒，允许 30 到 3600 秒。注册成功响应会包含 `heartbeat_after_seconds`，90 秒租约通常返回 30。

```bash
curl -X POST \
  "$V_ROUTER_URL/api/v1/registrations/registered-tc229-vllm-qwen3-1/heartbeat" \
  -H "Authorization: Bearer $V_ROUTER_REGISTRATION_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"ttl_seconds":90}'
```

注册器应按以下规则处理：

| 结果 | 动作 |
| --- | --- |
| 2xx | 按响应建议的间隔继续心跳 |
| 404 | 注册已过期、V-Router 已重启或当前身份不是 owner；重新检查 vLLM 后发送完整注册请求 |
| 401 | 刷新或修复注册凭证，不得去掉凭证降级重试 |
| 409（注册时） | ID 已被其他 owner 占用；停止抢占并修正 `service_id` 或账户 |
| 网络错误或 5xx | 有界退避重试；不要在 vLLM 不健康时持续声明健康 |

注册快照和租约不落盘。V-Router 重启后，下一次心跳会返回 404，注册器必须自动重注册。建议生命周期：

1. 等待 vLLM `/v1/models` 返回 2xx。
2. 发送完整注册请求。
3. 每 `TTL / 3` 心跳。
4. 心跳 404 时回到步骤 1。
5. vLLM 健康检查失败时停止心跳，让租约自然过期；恢复后重新注册。
6. 正常退出时尽力 DELETE 注销。

```bash
curl -X DELETE \
  "$V_ROUTER_URL/api/v1/registrations/registered-tc229-vllm-qwen3-1" \
  -H "Authorization: Bearer $V_ROUTER_REGISTRATION_TOKEN"
```

## 7. 注册器配置建议

可以把注册逻辑作为 vLLM 容器的配套 sidecar，使用以下环境变量：

```yaml
services:
  vllm-registrar:
    image: ${VLLM_REGISTRAR_IMAGE}
    environment:
      V_ROUTER_URL: http://v-router:8080
      V_ROUTER_REGISTRATION_TOKEN: ${V_ROUTER_REGISTRATION_TOKEN}
      VLLM_BASE_URL: http://vllm-backend:8000/v1
      VLLM_SERVICE_ID: tc229-vllm-qwen3-1
      VLLM_VISIBILITY: public
      VLLM_PROTOCOL: chat_completions
      VLLM_MODEL_KIND: chat
      VLLM_REGISTRATION_TTL: "90"
    networks:
      - vos_default
```

注册器实现要求：

- `VLLM_VISIBILITY` 只允许 `public` 或 `private`，未知值直接启动失败。
- token 从 secret 或应用已有 OAuth 刷新逻辑读取，不打印 Authorization 头。
- `private` 模式在注册前调用 `/api/v1/auth/me`，确认得到普通 VOS 用户身份及预期 `sub`。
- 只有 vLLM 健康时才注册和续租。
- 注册失败使用有上限的指数退避，例如 1、2、4、8、16、30 秒。
- 记录 `service_id`、上游 ID、模型 ID、可见范围、到期时间和状态码，但不记录凭证。

## 8. 调用与验收

查询模型：

```bash
curl "$V_ROUTER_URL/v1/models" \
  -H "Authorization: Bearer $CALLER_VOS_TOKEN"
```

调用对话：

```bash
curl "$V_ROUTER_URL/v1/chat/completions" \
  -H "Authorization: Bearer $CALLER_VOS_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "registered/qwen3-8b",
    "messages": [{"role":"user","content":"你好"}]
  }'
```

带 reasoning 的模型可能先消耗输出预算生成推理内容；`max_tokens` 太小时会合法返回 `content: null`、`finish_reason: length`，不能据此判断 V-Router 丢失正文。验收时应给足少量输出预算，并同时检查 `choices`、`message.content` / 上游扩展的 reasoning 字段、`finish_reason` 和 `usage`。需要排查时，用完全相同的请求分别直连 vLLM 和经过 V-Router 调用，比较响应而不是只断言正文非空。

其他 VOS 应用的浏览器前端使用同域路径 `/api/com.ictrek.v-scheduler/v1/...`；容器后端使用 `http://v-router:8080/v1/...`。需要按用户归集用量时，调用方仍应转发当前用户的 VOS OIDC token。

tc229 验收至少包括：

1. 从 V-Router 容器访问 vLLM `/v1/models` 成功。
2. 注册响应为 2xx，返回预期上游 ID 和心跳间隔。
3. `public` 模型可被注册者、其他 VOS 用户看到并完成真实推理。
4. `private` 模型可被同 `identity.sub` 的 token 看到并推理；其他用户和匿名调用看不到，直接指定模型也应失败。
5. 多副本分别显示为独立上游，并能被 V-Router 调度。
6. 重启 V-Router 后，注册器从心跳 404 自动恢复。
7. 停止一个 vLLM 副本且不注销，超过 TTL 后该副本退出新请求调度。
8. 日志中没有 OIDC token、网关 Key 或 vLLM API Key。
9. 临时 vLLM、注册器和测试网关均已删除；`nvidia-smi` 中没有本次测试遗留的 EngineCore。

只有 `/v1/models` 可见还不算完成；必须分别用符合范围和不符合范围的真实账户验证可见性，并完成至少一次真实 Chat Completions 调用。
