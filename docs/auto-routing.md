# Auto 自动选模型

Auto 在现有网关内选择真实模型，再走原有实例并发、队列、输入预算和资源许可。不下载模型，不改变管理员已批准的 direct/cloud/permit 策略。显式模型调用不变。首期支持 `/v1/chat/completions`（JSON、工具和 SSE），其他端点不接受 `model=auto`。

## 界面配置

使用 VOS 管理员或网关管理员 Key 打开「Auto 自动选型」。在候选目录勾选模型，每条记录绑定 **qualified model + upstream_id**；同名模型的不同实例分别勾选。已选但暂时离线的条目保留，可移除，恢复发现后再次参与选型。普通调用者不能修改配置，也不能通过 Auto 使用别人的私有模型。

启用 Auto 前至少选择一个候选。每个候选默认 `standard` 档位；根据实际任务评测设置 `simple`、`standard`、`complex`，不能单凭模型名字或参数量设置质量。复杂任务要求 complex，无合格模型时明确失败。可填写任务范围（空为全部）：`general,code,translation,summary,reasoning`。

配置真实服务的上下文和最大输出限制。Auto 使用 UTF-8 字节数加消息开销作为保守文本 token 上界；因此可能提前排除实际能装下的输入。支持图片还需要真实 `vision` 能力及管理员填写的每张图片 token 预算；该预算应覆盖后端允许的图片处理上限。工具、并行工具、结构化输出分别要求 `tools`、`parallel_tools`、`structured_output` 能力；未知不默认为支持。自动生成的通用元数据不证明这些特性可用。

四种策略均先满足任务档位与硬约束：

- balanced：满足最低档位后优先复用驻留、较低启动等待，再比较档位和费用。
- fast：优先较低的预计启动等待，再比较档位。
- quality：优先较高档位。
- economy：优先已知的较低预计 API 费用，未知价格排后；价格单位 USD/百万 token，显式 0 与未知不同。

启动等待目前是“未驻留装载估计 + inflight × 1 秒”的排序信号，不是实测 TTFT、队列期限或 SLA。冷启动估计可在界面调整。本地资源成本不等同 API 账单，不能将 economy 理解为免费或账单上限。

默认分类方式为本地规则，无额外模型、GPU 或外网依赖。不确定的普通请求走 standard，中文和英文的明确任务/复杂度信号影响档位；规则分类不保证回答质量。Auto 默认不允许云端回答：公开 DNS 域名及 cloud 上游需显式许可，短容器别名、私有 IP 属于本地范围。私有域名也保守归入云端范围；本地标记不能取代原有调度审批。

## 配置 API：勾选与命令使用同一份配置

以下接口均在网关 base 下：容器内 `http://v-scheduler:8080`；局域网 `http://<vos-host>:22888`（以安装端口为准）；同域 `/api/com.ictrek.v-scheduler`。不要使用模型管理的 `/model-hub` 前缀或控制器的 `38120` 管理端口。

```bash
GATEWAY=http://v-scheduler:8080
# VOS_TOKEN 使用获准的 VOS 管理员 token 或网关管理员 Key，勿写入仓库。
curl -fsS "$GATEWAY/api/v1/auto-routing/candidates" \
  -H "Authorization: Bearer $VOS_TOKEN"
curl -fsS "$GATEWAY/api/v1/auto-routing" \
  -H "Authorization: Bearer $VOS_TOKEN" > auto-config.json
```

`GET /api/v1/auto-routing/candidates` 返回 `models[]`，包括每个实例的 `qualified`、`upstream_id`、`capabilities`、`endpoints`、`protocol`。这是界面复选框的同一目录。`GET /api/v1/auto-routing` 返回当前完整配置。两个接口及写配置仅管理员可用；测试选型与推理允许已验证的普通模型调用身份，始终按该身份过滤候选。

编辑 `auto-config.json`，使用目录里的真实值。例如首次保存：

```json
{
  "revision": 0,
  "enabled": true,
  "policy": "balanced",
  "allow_cloud": false,
  "classifier": "rules",
  "candidates": [
    {
      "model": "registered/my-chat",
      "upstream_id": "registered-node-a",
      "tier": "complex",
      "tasks": [],
      "context_tokens": 32768,
      "max_output_tokens": 4096,
      "image_tokens": null,
      "input_price": null,
      "output_price": null,
      "cold_start_ms": 10000
    }
  ]
}
```

```bash
curl -fsS -X PUT "$GATEWAY/api/v1/auto-routing" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  --data-binary @auto-config.json
```

**PUT 完整替换配置**，未勾选的候选不再参与；省略字段恢复默认。`revision` 必须来自最新 GET，成功后递增，冲突返回 409。启用时空候选、未知候选、重复模型实例、错误字段或类型返回 422；禁用且空候选合法。暂时离线的原有候选可保留；新候选必须已发现且支持聊天。配置保存在原网关状态文件，重启后保留，更新失败不会替换内存中的有效配置。

## 测试选型和真实推理

模型列表与原有「试聊」也会展示可用的 `auto`；它没有可单独配置的直接放行或资源预算，预算属于最终真实模型。

界面保存后，输入 Chat Completions JSON，点击「测试选型」查看结果、分类来源、是否回退与每个可见候选的过滤原因；点击「实际调用」会真正执行。可开启流式。测试选型与实际请求共享选型函数，但测试不会占用回答模型的 permit 或创建会话绑定；运行状态变化可能导致下一次实际调用选择不同模型。

```bash
curl -fsS "$GATEWAY/api/v1/auto-routing/test" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"model":"auto","messages":[{"role":"user","content":"分析这段代码的并发错误"}],"max_tokens":256}'

curl -i "$GATEWAY/v1/chat/completions" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -H 'X-VScheduler-Auto-Policy: balanced' \
  -H 'X-VScheduler-Auto-Local-Only: true' \
  -H 'X-VScheduler-Queue-Wait-Seconds: 120' \
  -d '{"model":"auto","messages":[{"role":"user","content":"总结这段文字：……"}],"max_tokens":256,"stream":false}'
```

测试成功返回 `resolved_model`、`upstream_id`、`classification`、`candidates`、`reason`、`revision` 和决策 `id`。无合格候选时测试返回 200、空 `resolved_model` 和过滤原因，实际调用返回 503；禁用返回 404。`candidates` 不包含该调用者不可见的私有模型。

实际响应头 `X-VScheduler-Auto-Model` 是选择的 qualified ID，`X-VScheduler-Auto-Decision-ID` 可与统计中 `auto_routing` 记录关联。响应体保留上游模型名和正常 Chat/SSE 协议，不改写成决策 JSON。缺少输出上限时注入 `max_tokens=1024`；同时设置两种输出上限或 `n!=1` 会被拒绝。实际输入仍受后端预算校验。

`X-VScheduler-Auto-Policy` 可选 balanced/fast/quality/economy。`X-VScheduler-Auto-Local-Only: true` 只收紧策略，同时禁用本次 Jev 分类。请求不能放宽管理员的云端限制。`auto` 与 `X-VRouter-Upstream` 冲突返回 400。Auto 不会因为资源不足自动调用未勾选的云端模型，也不自动重放失败或已经输出的请求。

## Jev 可选分类

先在上游添加 TypeSafe/Jev 或相同原生协议的中转，完成模型发现。选择 `classifier=jev`，勾选“允许向 Jev 发送分类内容”，从目录选分类模型；云端回答开关与分类开关相互独立。API 在上述配置中增加：

```json
{
  "classifier": "jev",
  "jev_model": "ai.typesafe.api/jev-latest",
  "jev_upstream_id": "实际上游ID",
  "allow_cloud_classifier": true,
  "classifier_timeout_ms": 1000
}
```

这是完整配置中的字段片段，不能直接用它覆盖整个配置。分类器必须是已发现的 `typesafe` / `systemone` 模型，且调用者可见。网关复用已保存供应商密钥，不在 Auto 配置中复制 Key。

Jev 只判断 task 与 complexity，不输出 URL 或授权。使用最多最近 5 条消息的独立分类视图（当前超过 16000 UTF-8 字节直接回退规则），原回答输入保持不变。分类请求不携带用户的调度控制头，不递归 Auto，仍走原有资源许可；全局最多 2 个并发分类。失败、超时、繁忙、无权限或复杂度 confidence 小于 0.7 时回退规则，失败后冷却 30 秒。阈值是初始保守值，需要中文任务评测校准，不能当作目标回答正确率。

**测试选型也可能产生 Jev 分类费用并向其发送上述内容。** 本地限定请求不会调用 Jev。统计将分类推理作为独立请求记录 token，回答请求附带 `auto_routing.classification`；尚未将两者转换成实际账单或提供费用总额保证。

## 会话与边界

多轮使用稳定的 `X-VScheduler-Session-ID`（1..96 位 ASCII 字母、数字、`_`、`-`）。绑定按用户/应用隔离，空闲 10 分钟过期，最多保留 512 条，不落盘。仍满足硬约束和质量档位时优先沿用；升级需求或故障可重新选择。

工具结果继续调用 Auto 时必须带原 session ID。若原模型不可用、权限/能力不再满足或会话过期，返回 409，要求调用方明确重启工具轮次，不把工具结果静默交给另一个模型。没有 session ID 的工具续轮返回 422。SSE 已开始后绝不拼接另一个模型的响应。

首期不路由 embedding、rerank、图片/视频/音频生成、异步任务或带供应商持久会话资源的 API；向量索引必须固定 embedding 模型。尚无在线 bandit、自学习质量评分、TTFT 预测和自动质量重试，档位与价格来自管理员配置，候选效果须通过实际业务样本验证。
