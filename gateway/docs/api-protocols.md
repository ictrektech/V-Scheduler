> 迁移说明：网关现由 V-Scheduler 统一发布，源码位于 `gateway/`。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# Responses 与 Anthropic API

本文描述当前源码。端点与鉴权实时契约以应用的 API 文档 / `/api/v1/openapi.json` 为准。架构与转换边界见 [design.md §6](design.md#6-协议与适配边界)，性能实测见 [protocol-performance.md](protocol-performance.md)。

## 上游协议与路由

| 配置 | 行为 |
| --- | --- |
| `protocol=openai` 或旧手动记录未设置 | 原生 OpenAI 兼容透传；Responses 能力由上游提供 |
| `protocol=chat_completions` | `/v1/responses`、`/v1/messages` 转成上游 `/chat/completions`；其他适用端点保持原生转发，自动选择会排除已知不兼容接口 |
| `protocol=anthropic` | 原生 Messages 和资源透传，使用已保存的上游 `x-api-key`；Models 探测支持分页 |
| Model Hub 自动发现 | 缺省按 Chat Completions 接入；仍通过发现结果中的 `gateway_url`，保留 Model Hub 调度与统计 |

Base URL 均包含 `/v1`。手动界面可选择协议；旧客户端更新上游时省略 protocol 会保留已有配置。声明协议只决定路由方式，不保证模型支持视觉、工具调用、JSON schema 或其他能力。Anthropic 原生上游不自动提供 Responses 协议，须选择支持目标协议的上游。

自动分配、实例固定和注册租约详见 [注册与统一路由](routing.md)。

模型创建接口可使用聚合模型名，例如 `model-hub/<model>`。`X-VRouter-Upstream: <id>` 固定上游并要求身份，此时使用原生模型名；Responses/Messages 仍按该上游 protocol 决定是否转换。原生资源操作始终使用相同上游 ID，不根据资源名称猜测归属。

Model Hub 发现 URL 支持 `http://model-hub-backend:5005`（vos_default）和 `http://<host>:35005`，用 `V_ROUTER_MODEL_HUB_URL` 或 `-model-hub` 选择。两种入口已验证返回服务列表；推理请求继续使用返回的 gateway_url，不把管理端口 35005 当作推理端口。

## 已实现的转换范围

Responses：`model`、`input`（字符串、message、function_call、function_call_output）、`instructions`、`stream`、`max_output_tokens`、`temperature`、`top_p`、function `tools` / `tool_choice`、`parallel_tool_calls`、`store=false`、`text.format`（text/json_object/json_schema）。消息内容支持文本与 URL/内联图片，不支持供应商文件 ID。工具结果须为文本；多轮对话由调用方显式携带历史。

Anthropic Messages：`model`、`messages`、`system`、`max_tokens`、`stream`、`temperature`、`top_p`、`stop_sequences`、自定义 `tools` / `tool_choice`。内容支持文本、URL/base64 图片、tool_use / tool_result；工具调用和结果 ID 保持对应。工具参数流式片段转换为 `input_json_delta`。

两种协议都转换文本、工具结果对象、usage 和 SSE 结束事件；客户端取消会关闭上游请求。JSON schema/图片参数的正确映射已测试，是否生成合格结构化结果或理解图片仍取决于上游模型。网关不会代替模型执行工具。usage 来自上游；上游未报告的计数为 0，不做本地 token 估算。

无状态转换不支持 `previous_response_id`、服务端 conversation、background、store=true、托管工具、thinking、缓存控制或供应商资源生命周期。未知字段和不能映射的内容块返回 400，不静默丢字段；上游失败保留 HTTP 状态与 Retry-After，错误正文做协议封装且不回显凭证或请求体。流开始后以 error 事件结束，不发送虚假的完成事件。

## 原生接口族

Responses 创建/compact 与 `/v1/responses/{...}` 的查询、取消、删除、input_items 等操作依赖原生上游。`/v1/messages/{...}` 覆盖 Anthropic count_tokens、Message Batches 及其结果；`/v1/skills/{...}`、`/v1/files/{...}`、`/v1/models/{...}` 等通过固定上游访问。接口族代理不是供应商所有字段和权限的本地实现，不包含组织管理或实验性 Agents 全套接口。

Anthropic SDK 的 models/files 操作必须固定 Anthropic 上游；不固定上游的 `GET /v1/models` 是 OpenAI 格式的聚合列表。`anthropic-version`（缺省 2023-06-01）与 `anthropic-beta` 在原生通道保留，转换通道不会把 beta 功能伪装成已实现。注册模型可用 public/private 控制可见性；资源接口仍使用共享上游账户。

## SDK 调用

网关接受 Bearer 或 `x-api-key` 中的网关 Key / VOS token；两者同时提供时必须相同。调用方凭证不会转发给模型供应商。详见 [auth.md](auth.md)。

```python
import os
from openai import OpenAI
from anthropic import Anthropic

key = os.environ['VOS_TOKEN']
openai = OpenAI(base_url='http://<vos-host>:22888/v1', api_key=key)
result = openai.responses.create(model='<网关模型名>', input='你好', store=False)
print(result.output_text)

anthropic = Anthropic(base_url='http://<vos-host>:22888', api_key=key)
with anthropic.messages.stream(
    model='<网关模型名>', max_tokens=1024,
    messages=[{'role': 'user', 'content': '你好'}],
) as stream:
    for text in stream.text_stream:
        print(text, end='', flush=True)

# 原生 Anthropic 资源：固定上游，使用供应商原生模型名和参数。
native = Anthropic(base_url='http://<vos-host>:22888', api_key=key,
                  default_headers={'X-VRouter-Upstream': '<Anthropic 上游 ID>'})
models = native.models.list()
```

官方 SDK fixture 检查位于 `gateway/testdata/protocol_sdk_check.py`，所有生成请求都发往测试模型服务；它不验证真实模型质量，也不声称所有 SDK 可选字段均能映射。
