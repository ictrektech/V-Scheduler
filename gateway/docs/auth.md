> 迁移说明：网关现由 V-Scheduler 统一发布，Rust 源码位于 `backend/src/router/`；文中 Go 测试命令仅为历史记录。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# V-Router 用户鉴权与外部调用

V-Router 直接验证 VOS OIDC access token，不额外签发本地 JWT。浏览器在 VOS 同域 iframe 内通过自身 `oauth2.client` 的 Fastpath（PKCE authorize → token）取得 token；其他应用也可用自己的 Fastpath token 调用，无须共享 V-Router 的网关 Key。外部客户端使用 VOS 官方授权流程取得 token，再发送 Bearer 头。Anthropic SDK 也可通过 x-api-key 发送同一凭证；同时提供两个头时必须一致，无效凭证不会降级匿名。

## 先验证当前身份

```bash
# VOS_TOKEN 由调用方的 VOS 授权流程获得
BASE="http://<vos-host>:22888"
curl "$BASE/api/v1/auth/me" \
  -H "Authorization: Bearer $VOS_TOKEN"
```

响应包含 `identity.sub`、`identity.username`、`auth_method`、`permissions` 和 `resource_isolation`。`auth_method=vos_oidc` 表示当前 VOS 用户；`gateway_key` 表示共享网关凭证；`auth_disabled` 仅用于独立调试。不要把 `/healthz` 或匿名 `/v1/models` 成功当成用户登录成功。

## 权限与凭证

| 凭证 | 模型推理 | 管理 API / 指定上游资源 | 用量归属 |
| --- | --- | --- | --- |
| 有效 VOS OIDC token | 允许 | 允许；当前所有已登录用户共享管理权限 | 验证后的 VOS 用户 |
| 共享网关 Key（REQUIRE_KEY=true 且配置非空） | 允许 | 允许 | gateway-admin |
| 无凭证 | 仅 REQUIRE_KEY=false 时允许 | 401 | internal |
| 无效/过期 token | 401，不降级为匿名 | 401 | 不计为匿名成功请求 |

REQUIRE_KEY 默认 false，对容器网络、宿主机端口是同一策略。开启而未填写 Key 时，无凭证调用仍被拒绝，有效 OIDC token 可用。VOS 用户名由 `/v1000/oauth2/userinfo` 校验结果确定，不信任客户端自报的 `X-External-User-ID` 等用户头。成功验证缓存最多 60 秒，撤销可能在缓存到期后生效。

当前没有个人 Key 发放或模型级配额。`resource_isolation=true` 表示注册模型服务支持 public/private 可见范围；文件和任务仍使用共享上游账户，已登录且可访问该上游的用户可访问该账户资源。WeKnora 的个人 Key、知识库范围及 token exchange 是其自身功能，不能视为 V-Router 已提供的接口。

## 其他 VOS 应用的推荐接入方式

其他 VOS 应用不需要保存云端 API Key、上游地址或模型清单，也不要让用户在每个应用里重复配置模型。用户只在 V-Router 中完成 Model Hub、本地推理后端、云端算力 API、权重和并发设置；其他应用用当前 VOS 账户的 OIDC/Fastpath token 直接读取 V-Router 模型列表并调用统一接口。

前端同域接入流程：

1. 当前应用按自己的 `oauth2.client` 登录 VOS，取得 access token。
2. 调用 `GET /api/com.ictrek.v-scheduler/v1/models`，用返回的 `data[].id` 填充模型下拉框。
3. 需要显示来源、模型类型、能力或前缀时，再调用 `GET /api/com.ictrek.v-scheduler/api/v1/models`；返回的 `models[]` 包含 `qualified`、`kind`、`capabilities`、`upstream_kind`、`upstream_source` 和 `model_prefix`。
4. 用户选择模型后，应用调用 `/api/com.ictrek.v-scheduler/v1/chat/completions`、`/responses`、`/embeddings` 等统一接口。能力未知时应用侧不要硬拦截；显示 V-Router 或上游返回的具体错误。

```javascript
const token = await vosFastpathToken();

const modelList = await fetch('/api/com.ictrek.v-scheduler/v1/models', {
  headers: { Authorization: `Bearer ${token}` },
}).then(r => r.json());

const options = modelList.data.map(m => ({ value: m.id, label: m.id }));

const response = await fetch('/api/com.ictrek.v-scheduler/v1/chat/completions', {
  method: 'POST',
  headers: {
    Authorization: `Bearer ${token}`,
    'Content-Type': 'application/json',
  },
  body: JSON.stringify({
    model: options[0].value,
    messages: [{ role: 'user', content: '你好' }],
  }),
});
```

容器后端也可以通过 `http://v-router:8080/v1` 访问统一推理接口；若要按 VOS 用户归集用量，仍应转发当前用户的 VOS token。

## 应用与 SDK 示例

同域前端已经取得自己的 Fastpath access token 后：

```javascript
const response = await fetch('/api/com.ictrek.v-scheduler/v1/responses', {
  method: 'POST',
  headers: {
    Authorization: `Bearer ${accessToken}`,
    'Content-Type': 'application/json',
  },
  body: JSON.stringify({ model: '<网关模型名>', input: '你好' }),
});
if (response.status === 401) {
  // 重新通过当前应用的 VOS 授权流程取得 token；不要移除凭证重试。
}
```

外部 Python OpenAI SDK：

```python
import os
from openai import OpenAI

client = OpenAI(base_url='http://<vos-host>:22888/v1',
                api_key=os.environ['VOS_TOKEN'])
result = client.responses.create(model='<网关模型名>', input='你好', store=False)
```

Anthropic SDK 的 base_url 不包含 /v1，Messages 转换和原生资源示例见 [api-protocols.md](api-protocols.md)。

资源接口及供应商默认模型需要指定上游：

```python
provider = OpenAI(base_url='http://<vos-host>:22888/v1',
                  api_key=os.environ['VOS_TOKEN'],
                  default_headers={'X-VRouter-Upstream': '<上游页中的 ID>'})
# 同一个 provider 实例用于文件创建、列表、读取、删除；模型参数用上游原名。
files = provider.files.list()
```

网关向上游发送其已保存的 API Key，不转发 VOS Bearer token、VOS Cookie 或路由选择头。不要将共享网关 Key 嵌入公开网页。

## 401 排查与接口契约

1. 用 `/api/v1/auth/me` 检查真实身份；检查 Bearer 格式与 token 是否过期。
2. `/api/v1/auth-probe` 用于检查网关到 VOS userinfo 的连通性；可达不代表 token 有效。
3. 使用当前应用的 VOS Fastpath 重新授权；后端地址由 VOS_OIDC_USERINFO_URL、VOS_HOST_GW_IP 与既有兜底规则确定。
4. 在管理界面的 API 文档中下载 `/api/v1/openapi.json`；文档和路由从同一张表产生，随运行版本更新。

参考了 WeKnora 的 `docs/api/vos-external-api.md` 与 `docs/api/auth.md` 的普通用户直接携带 VOS token 模式；以本文列出的 V-Router 能力为准。
