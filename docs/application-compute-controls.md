# 应用计算启动与停止 API

应用登记已批准的算力需求时，可在原 `POST /integrations` 中附带 `controls`。Scheduler 随后可以主动调用应用的启动和停止/卸载接口；省略该字段时，应用保持原有自行执行、心跳领取停止请求的行为。

以下路径相对于调度 API：容器内 `http://v-scheduler:38120/api/v1`，经网关 `http://v-scheduler:8080/scheduler/api/v1`，局域网 `http://<vos-host>:22888/scheduler/api/v1`，VOS 同域 `/api/com.ictrek.v-scheduler/scheduler/api/v1`。应用使用自己的 Scheduler submitter 身份；网关模型 Key 不能替代它。

## 登记算力与可选控制接口

先由管理员批准 executor 的应用、节点、CPU/内存/GPU 预算及 `upstream_urls`，再由应用注册：

```json
{
  "protocol_version": 1,
  "revision": 0,
  "executors": ["my-compute"],
  "stop_protocol": "permit-native-exit-v1",
  "controls": [{
    "executor_id": "my-compute",
    "start_url": "http://my-app:9000/compute/start",
    "stop_url": "http://my-app:9000/compute/stop-and-unload"
  }]
}
```

```bash
SCHEDULER=http://v-scheduler:8080/scheduler/api/v1
curl -fsS "$SCHEDULER/integrations" \
  -H "Authorization: Bearer $APPLICATION_TOKEN" -H 'Content-Type: application/json' \
  --data-binary @registration.json
```

保存返回的 revision，继续按原协议每 5 秒提交 `/integrations/heartbeat`。完整替换注册时使用最新 revision；同内容不重复递增，有未结束执行时不能更换控制地址、预算或注销。注册不会自行批准预算。

`controls` 最多 64 项，每个 executor 只能一项。`stop_url` 必填；只登记停止 API 时省略 `start_url`，适用于应用自行启动的已有 permit。启动控制要求应用拥有该 executor；已获 `permit_executors` 核验授权的接收端可为相应执行器登记停止控制。

控制地址必须与该 executor 已批准的某个 `upstream_urls` **同 scheme/host/port**；路径可不同，不能带账号密码、查询串或 fragment，不跟随 HTTP 重定向。不向回调转交 VOS token、网关 Key 或应用 submitter token。控制接口通过下述许可认领与回执证明绑定计算，不能仅凭网络来源或请求中的模型名启动。

## 由 Scheduler 启动

`POST /integrations/executions` 支持普通模型推理及已批准的其他计算 executor，先调用同一资源准入逻辑，再发起一次启动回调。请求的 `upstream_url` 必须是已批准的原始值，不能以控制路径替代。

```json
{
  "request": {
    "executor_id": "my-compute",
    "upstream_url": "http://my-app:9000/v1",
    "model_id": "my-calculation",
    "execution_token": "replace-with-a-random-32-to-128-character-secret"
  },
  "input": {"parameters": {"size": 128}}
}
```

```bash
curl -fsS "$SCHEDULER/integrations/executions" \
  -H "Authorization: Bearer $APPLICATION_TOKEN" \
  -H 'Content-Type: application/json' -H 'Idempotency-Key: stable-business-request-id' \
  --data-binary @execution.json
```

`execution_token` 使用随机秘密，例如 Python `secrets.token_hex(32)`；不要写入日志、浏览器历史或仓库。`input` 是由应用解释的 JSON，最大 256 KiB，应用仍须校验它符合该预算对应的输入范围。Scheduler 仅存输入摘要，不保存输入正文或 execution_token。幂等键也可通过顶层 `idempotency_key` 提交，二者同时设置须相同。相同键但参数、输入或证明不同返回 409；相同键不会重复发起启动。

管理员可加顶层 `app_id`，为已注册应用启动计算；普通应用只能指定自己。管理界面「节点管理 → 应用统一接入」显示控制接口状态，支持输入 JSON 启动、停止和查看执行阶段。启动仍受原节点、预算、心跳、并发和停止约束。

启动回调固定 POST，HTTP 超时 5 秒，携带：

```json
{
  "protocol_version": 1,
  "command_id": "start:<permit_id>",
  "action": "start",
  "permit_id": "<permit_id>",
  "executor_id": "my-compute",
  "upstream_url": "http://my-app:9000/v1",
  "model_id": "my-calculation",
  "execution_id": "<stable-execution-id>",
  "execution_token": "<execution-secret>",
  "input": {"parameters": {"size": 128}}
}
```

应用先将命令和取消栅栏持久化，以自己的 submitter 凭据调用 `/permits/{id}/verify`，提交相同 executor/upstream/model/execution_id/execution_token 和 `claim:true`。**只有认领成功才启动计算**；重复认领不能创建第二个执行。运行中以 `claim:false` 核验租约，租约失效或收到停止意图必须停止，结果未知保留隔离状态。

启动接口应快速返回 `{ "permit_id":"...", "execution_id":"...", "started":true }`，不能等整个长任务完成。Scheduler 同时确认 permit 已认领，才报告 `phase=running`。超时、错误或回执不匹配返回 `phase=unknown`，主动请求停止，预约保持占用；不会自动重放启动。Controller 重启保留意图，超过 15 秒仍停在 starting 的执行转为停止核验。

## 停止或卸载

使用原 `POST /permits/{id}/stop`，或管理界面「停止并卸载」。Scheduler 将持久化 nonce，并主动 POST `stop_url`；停止接口可以执行取消、卸载权重，或关闭实际计算进程。

停止命令包含 `command_id`（等于 nonce）、`nonce`、`action=cancel-and-unload`、permit/executor/model、实际认领的 `execution_id`（未认领时为 null）及 `planned_execution_id`。重试和 Controller 重启继续使用同一 nonce。应用应以自己的 `/integrations/heartbeat` 中的停止命令校验 nonce；仅返回 HTTP 200 不代表成功。

确认原生计算真正停止、预算所计内存/权重已释放后，返回以下精确结构（也可放在 `data` 中）：

```json
{
  "nonce": "<received-nonce>",
  "execution_id": "<claimed-execution-id-or-null>",
  "execution_token": "<original-execution-secret>",
  "native_exit_barrier": true,
  "model_unloaded": true,
  "evidence": "Native process exited; its compute allocations were freed"
}
```

沿用现有 native-stop 协议：`model_unloaded=true` 表示此次预算计入的计算分配已释放；应用只关掉计算但仍持有被该预算计入的权重内存时，不能宣称完成。没有权重的计算进程退出并释放其分配也可满足条件。也可由应用直接调用原 `/permits/{id}/stopped` 提交同样证明。

停止先于启动到达时，必须先持久化该计划执行的取消栅栏，拒绝随后迟到的启动；未认领的执行只有确认从未启动后才返回 `execution_id:null` 的证明。无证明、错 nonce、错 execution、错 token、尚未卸载或网络失败均保留预约并继续有界重试。应用心跳/节点遥测过期、租约到期或启动结果不明也会触发停止。排空节点只阻止新准入，不强制终止已在执行的计算。

正常计算完成后，应用仍可按现有 `/permits/{id}/release` 协议报告真正停止；有待处理停止命令时必须走 nonce 证明。此功能不自动重放业务任务、不提供断点恢复，也不改变既有 `/tasks` 的 Agent 执行契约。
