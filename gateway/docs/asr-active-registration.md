> 迁移说明：网关现由 V-Scheduler 统一发布，源码位于 `gateway/`。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# tc229 VOS ASR 主动注册开发指南

本文说明同一 VOS 账户下运行的 ASR 服务，如何在启动后主动注册到 V-Router，并通过 V-Router 的标准 OpenAI 兼容接口供该账户的其他应用使用。

本文按当前 V-Router 实现编写：注册使用 VOS OIDC access token，注册资源归属于 token 对应的 `identity.sub`；ASR 采用 `offline_asr` 协议，由 V-Router 把标准 `POST /v1/audio/transcriptions` 请求转换为 ASR 服务的 `POST /transcribe` 请求。

## 1. 目标和边界

推荐流程如下：

1. ASR 服务启动并通过自身健康检查。
2. ASR 使用当前 VOS 账户的 OIDC access token 调用 V-Router 注册接口。
3. V-Router 以 `private` 可见范围保存注册，使模型仅对同一 VOS 账户和管理员可见。
4. ASR 每隔约 30 秒续租一次；租约默认 90 秒。
5. 同一账户下的其他 VOS 应用携带各自取得的同账户 OIDC token，通过 V-Router 的标准 `/v1/audio/transcriptions` 使用该模型。
6. ASR 正常退出时尽力注销；异常退出时由租约自动清理。

这里的“同账户”以 VOS OIDC userinfo 返回的稳定 `sub` 为准，不以客户端填写的用户名、请求头或容器名为准。不同应用取得的 access token 可以不同，只要它们解析到同一个 `sub`，就属于同一账户。

## 2. 网络和接口约定

ASR 与 V-Router 都应加入 VOS 的 `vos_default` Docker 网络。ASR 注册的 `base_url` 必须从 **V-Router 容器内部**可达，推荐使用稳定的 Compose 网络别名：

```text
http://asr-backend:8000
```

不要把 `localhost` 写成 `base_url`，因为在 V-Router 容器内它指向 V-Router 自身。只有 ASR 确实运行在 tc229 宿主机而不在 `vos_default` 网络时，才使用 V-Router 容器可达的宿主机地址。

`offline_asr` 协议要求 ASR 暴露：

```http
POST /transcribe?lang=zh
Content-Type: application/octet-stream

<原始音频文件字节>
```

ASR 成功响应支持以下任一种格式：

```json
{"text":"识别结果"}
```

```json
{
  "data": {
    "full_text": "识别结果",
    "segments": [],
    "speakers": [],
    "stats": {}
  }
}
```

也可以直接返回纯文本。V-Router 会统一返回至少包含 `text` 的 OpenAI 兼容 JSON。若注册时提交了 `api_key`，V-Router 调用 ASR 时会发送 `Authorization: Bearer <api_key>`；这个上游密钥与用于注册的 VOS OIDC token 是两种不同凭证。

## 3. VOS Compose 配置

下面只展示与注册有关的字段，实际镜像、端口和健康检查应沿用 ASR 应用现有配置：

```yaml
services:
  asr-backend:
    image: ${ASR_BACKEND_IMAGE}
    environment:
      V_ROUTER_URL: http://v-router:8080
      V_ROUTER_REGISTRATION_TOKEN: ${V_ROUTER_REGISTRATION_TOKEN}
      ASR_PUBLIC_BASE_URL: http://asr-backend:8000
      ASR_SERVICE_ID: tc229-asr-1
      ASR_MODEL_ID: whisper-large-v3
      ASR_REGISTRATION_TTL: "90"
    networks:
      vos_default:
        aliases:
          - asr-backend

networks:
  vos_default:
    external: true
```

注意：

- `V_ROUTER_REGISTRATION_TOKEN` 必须来自与使用者相同的 VOS 账户授权流程，并作为 secret 注入；不要写进镜像、Compose 模板、日志或前端代码。
- access token 过期时，ASR 应通过应用已有的 VOS OAuth/Fastpath 授权和刷新机制取得新 token。不要在 401 后去掉认证重试，否则管理接口仍会拒绝请求。
- 若一个 ASR 应用运行多个副本，每个副本必须使用不同且稳定的 `ASR_SERVICE_ID`，例如 `tc229-asr-1`、`tc229-asr-2`。同一副本重启后继续使用原 ID。
- 如果服务需要被所有 VOS 用户共享，可将下文的 `visibility` 改为 `public`；“同账户注册、同账户使用”应保持 `private`。

## 4. 注册请求

先用同一个 token 确认 V-Router 识别到的账户：

```bash
curl "$V_ROUTER_URL/api/v1/auth/me" \
  -H "Authorization: Bearer $V_ROUTER_REGISTRATION_TOKEN"
```

然后注册单个 ASR 模型：

```bash
curl "$V_ROUTER_URL/api/v1/registrations" \
  -H "Authorization: Bearer $V_ROUTER_REGISTRATION_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "service_id": "tc229-asr-1",
    "name": "tc229 ASR 1",
    "visibility": "private",
    "base_url": "http://asr-backend:8000",
    "protocol": "offline_asr",
    "model": "whisper-large-v3",
    "model_kind": "unknown",
    "endpoints": ["audio/transcriptions"],
    "capabilities": ["asr"],
    "ttl_seconds": 90
  }'
```

注册成功后，上游 ID 固定为：

```text
registered-tc229-asr-1
```

对外模型名为：

```text
registered/whisper-large-v3
```

相同 owner 使用相同 `service_id` 再次注册会更新这个实例；其他账户不能接管该 ID。模型 ID、地址、协议、可见范围或 ASR 上游密钥变化时，应重新发送完整注册请求，而不是只续租。

## 5. 心跳、重注册和注销

续租请求必须携带仍能解析到同一个 VOS `sub` 的 token：

```bash
curl -X POST \
  "$V_ROUTER_URL/api/v1/registrations/registered-tc229-asr-1/heartbeat" \
  -H "Authorization: Bearer $V_ROUTER_REGISTRATION_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"ttl_seconds":90}'
```

推荐每 `TTL / 3` 续租一次，即 90 秒租约每 30 秒一次。处理规则：

| 情况 | 服务端动作 |
| --- | --- |
| 2xx | 继续按间隔续租 |
| 404 | 租约已过期、V-Router 已重启或注册不存在；立即重新发送完整注册请求 |
| 401 | token 无效或过期；刷新同一账户 token 后再注册，不能降级为匿名 |
| 409 | `service_id` 已被其他 owner 占用；停止抢占并报告配置冲突 |
| 网络错误或 5xx | 在租约到期前短间隔重试；持续失败则重新走健康检查和注册流程 |

注册快照只保存在 V-Router 内存中，不落盘。因此 V-Router 重启后，ASR 必须能从心跳 404 自动恢复，而不能依赖人工重新登记。

正常退出时可以尽力注销：

```bash
curl -X DELETE \
  "$V_ROUTER_URL/api/v1/registrations/registered-tc229-asr-1" \
  -H "Authorization: Bearer $V_ROUTER_REGISTRATION_TOKEN"
```

注销只阻止新请求进入，已经开始的识别请求会继续完成。进程崩溃或机器断电时不必依赖注销，租约到期会自动移除模型。

## 6. Python 注册器示例

下面示例使用 `httpx`。它只展示注册生命周期；`load_vos_token()` 必须接入 ASR 应用现有的 VOS 同账户授权/刷新机制，不能把 token 硬编码到源码。

```python
import asyncio
import os

import httpx


ROUTER = os.environ.get("V_ROUTER_URL", "http://v-router:8080").rstrip("/")
SERVICE_ID = os.environ.get("ASR_SERVICE_ID", "tc229-asr-1")
MODEL_ID = os.environ.get("ASR_MODEL_ID", "whisper-large-v3")
ASR_BASE_URL = os.environ.get("ASR_PUBLIC_BASE_URL", "http://asr-backend:8000")
TTL = int(os.environ.get("ASR_REGISTRATION_TTL", "90"))
UPSTREAM_ID = f"registered-{SERVICE_ID}"


async def load_vos_token() -> str:
    # 实际项目中应调用应用已有的 VOS OAuth/Fastpath token 获取或刷新逻辑。
    token = os.environ.get("V_ROUTER_REGISTRATION_TOKEN", "").strip()
    if not token:
        raise RuntimeError("missing same-account VOS registration token")
    return token


async def wait_until_asr_ready(client: httpx.AsyncClient) -> None:
    delay = 1
    while True:
        try:
            response = await client.get(f"{ASR_BASE_URL}/health", timeout=5)
            if response.is_success:
                return
        except httpx.HTTPError:
            pass
        await asyncio.sleep(delay)
        delay = min(delay * 2, 30)


async def register(client: httpx.AsyncClient) -> None:
    token = await load_vos_token()
    response = await client.post(
        f"{ROUTER}/api/v1/registrations",
        headers={"Authorization": f"Bearer {token}"},
        json={
            "service_id": SERVICE_ID,
            "name": f"tc229 ASR {SERVICE_ID}",
            "visibility": "private",
            "base_url": ASR_BASE_URL,
            "protocol": "offline_asr",
            "model": MODEL_ID,
            "model_kind": "unknown",
            "endpoints": ["audio/transcriptions"],
            "capabilities": ["asr"],
            "ttl_seconds": TTL,
        },
        timeout=10,
    )
    response.raise_for_status()


async def heartbeat(client: httpx.AsyncClient) -> bool:
    token = await load_vos_token()
    response = await client.post(
        f"{ROUTER}/api/v1/registrations/{UPSTREAM_ID}/heartbeat",
        headers={"Authorization": f"Bearer {token}"},
        json={"ttl_seconds": TTL},
        timeout=10,
    )
    if response.status_code == 404:
        return False
    response.raise_for_status()
    return True


async def registration_loop() -> None:
    async with httpx.AsyncClient() as client:
        await wait_until_asr_ready(client)
        retry_delay = 1
        while True:
            try:
                await register(client)
                retry_delay = 1
                while True:
                    await asyncio.sleep(max(10, TTL // 3))
                    if not await heartbeat(client):
                        break
            except (httpx.HTTPError, RuntimeError) as exc:
                # 生产环境记录状态码和 service_id 即可，不要打印 token。
                print(f"V-Router registration unavailable: {type(exc).__name__}")
                await asyncio.sleep(retry_delay)
                retry_delay = min(retry_delay * 2, 30)


if __name__ == "__main__":
    asyncio.run(registration_loop())
```

生产实现还应监听 SIGTERM，在进程退出前发送 DELETE；但注销失败不应阻塞容器退出，因为 TTL 会完成兜底清理。

## 7. 同账户应用如何调用

同一 VOS 账户下的调用应用使用自己的 OAuth/Fastpath 流程取得 access token。先确认该 token 也解析到相同的 `identity.sub`：

```bash
curl "$V_ROUTER_URL/api/v1/auth/me" \
  -H "Authorization: Bearer $CALLER_VOS_TOKEN"
```

查询模型列表：

```bash
curl "$V_ROUTER_URL/v1/models" \
  -H "Authorization: Bearer $CALLER_VOS_TOKEN"
```

列表中应出现 `registered/whisper-large-v3`。随后使用标准 OpenAI ASR 接口：

```bash
curl "$V_ROUTER_URL/v1/audio/transcriptions" \
  -H "Authorization: Bearer $CALLER_VOS_TOKEN" \
  -F 'model=registered/whisper-large-v3' \
  -F 'file=@sample.wav' \
  -F 'language=zh'
```

其他 VOS 应用的浏览器前端应使用同域地址：

```text
/api/com.ictrek.v-scheduler/v1/models
/api/com.ictrek.v-scheduler/v1/audio/transcriptions
```

容器后端可直接使用 `http://v-router:8080/v1/...`。无论哪种方式，都应转发当前用户的 OIDC token，才能看到该账户的 private 模型并把用量归属到正确用户。

## 8. tc229 验收清单

开发完成后至少验证以下项目：

1. 在 V-Router 容器内请求 `http://asr-backend:8000/health` 成功。
2. 使用注册 token 调用 `/api/v1/auth/me`，记录其 `identity.sub`；不要记录 token 本身。
3. 启动 ASR 后，注册返回成功，上游 ID 为 `registered-<service_id>`。
4. 使用另一个由同一 VOS 账户取得的 token 调用 `/api/v1/auth/me`，确认 `identity.sub` 相同。
5. 同账户 token 的 `/v1/models` 能看到 `registered/<model>`；不同账户看不到 private 模型。
6. 用一段真实 WAV 文件调用 `/v1/audio/transcriptions`，确认 V-Router 返回 `{"text":"..."}`。
7. 重启 V-Router，确认下一次心跳收到 404 后 ASR 自动重新注册，模型恢复出现。
8. 停止 ASR 且不执行注销，等待超过 TTL，确认模型自动从列表消失。
9. 检查日志，确认没有输出 OIDC token、ASR 上游密钥或完整 Authorization 头。

只看到注册接口返回 2xx 不算完成；必须以同账户真实 token 完成模型可见性和音频转写调用，才证明注册、权限、网络与协议适配全部接通。
