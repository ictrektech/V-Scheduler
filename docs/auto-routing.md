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

## 选择路由方式：不必运行分类模型

「路由方式」和 balanced/fast/quality/economy 排序策略分别配置。前者决定任务档位，后者在通过全部硬约束的候选中排序。

| classifier | 界面选项 | 分类资源与行为 |
|---|---|---|
| rules | 本地规则（默认） | 关键词、输入结构及长度规则；不运行模型 |
| request_type | 按请求类型 | `X-VScheduler-Auto-Task` 指定 general/code/translation/summary/reasoning，未指定则用文本规则识别类型；按 `request_type_tiers` 选择档位，不运行模型 |
| context_size | 按上下文大小 | 按完整输入的 UTF-8 保守 token 上界、输出上限和最大声明图片预算分档；不运行模型 |
| llm | 聊天模型分类器（本地／云端） | 从 V-Scheduler 已添加的 Chat Completions 模型选择；原有供应商 Key 留在网关 |
| systemone / jev | System One / Jev | 使用已发现的决策模型，支持本地 Ollama 或云端服务 |

请求类型默认档位：general/code/summary 为 standard、translation 为 simple、reasoning 为 complex；可在界面逐项修改。当前类型是 Chat Completions 内的任务类别，图片、工具及结构化输出继续按实际能力过滤；不会跨协议选择 ASR 等模型。同一轮工具续调须保持相同类型头及会话 ID。请求方给出的类型是路由提示，不能授予未勾选模型或计算权限。类型不保证难度，明确复杂请求应使用合适档位配置。

上下文策略使用 `context_simple_max_tokens=2048`、`context_standard_max_tokens=8192`，大于后者为 complex；必须满足 `512 <= simple < standard <= 2097152`。这是保守预算，不是模型 tokenizer 的精确计数。短输入可能仍是复杂任务；选择此策略表示只按长度分档，建议先评测。

纯规则、请求类型和上下文策略不需要选择分类模型，也不发送分类文本到外部。所有方式仍检查模型 endpoint、可见性、Local-Only、工具/图片能力、上下文、输出上限及资源准入。

聊天分类模式沿用 `jev_model` / `jev_upstream_id` 保存分类实例以兼容原 API：选择支持 Chat Completions 的真实模型即可，不需要支持 System One。外部模型必须单独开启 `allow_cloud_classifier=true`；这不等于允许云端回答。Local-Only 请求不调用云端分类器。本地聊天分类经过原有许可并在分类后卸载。云端分类使用外部算力，不占本机 GPU 预算；网关仍有并发限制。

聊天分类返回的 JSON 只接受枚举 task、complexity 及 0..1 confidence；无效输出、低置信度、超时、不可用均回退本地规则。聊天模型自报 confidence 不等于已校准的正确率，优先选轻量、稳定输出 JSON 的模型并评测业务样本。

例如按请求类型保存完整配置：

```bash
curl -fsS "$GATEWAY/api/v1/auto-routing" -H "Authorization: Bearer $VOS_TOKEN" > auto-config.json
jq '.classifier="request_type" | .request_type_tiers={general:"standard",code:"standard",translation:"simple",summary:"standard",reasoning:"complex"}' auto-config.json > auto-next.json
curl -fsS -X PUT "$GATEWAY/api/v1/auto-routing" -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' --data-binary @auto-next.json
curl -fsS "$GATEWAY/api/v1/auto-routing/test" -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' -H 'X-VScheduler-Auto-Task: translation' -d '{"model":"auto","messages":[{"role":"user","content":"Hello"}],"max_tokens":8}'
```

切换上下文策略时，在最新 GET 配置中设置 `classifier=context_size` 与两个上下文阈值。切换聊天分类时设置 `classifier=llm`、实际模型实例、云端分类许可及超时。每次都读取最新 revision 后完整 PUT；界面与 API 共用同一配置。

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

`X-VScheduler-Auto-Policy` 可选 balanced/fast/quality/economy。`X-VScheduler-Auto-Local-Only: true` 只收紧策略，同时禁用本次云端分类（本地 System One 分类仍可使用）。请求不能放宽管理员的云端限制。`auto` 与 `X-VRouter-Upstream` 冲突返回 400。Auto 不会因为资源不足自动调用未勾选的云端模型，也不自动重放失败或已经输出的请求。

## System One 可选分类：本地 Ollama 或云端 Jev

默认 `classifier=rules`，无需额外模型。选择 `classifier=systemone` 后，可从已发现的 `systemone` 模型中选择分类器；旧 `classifier=jev` 继续兼容。API 字段沿用 `jev_model` 和 `jev_upstream_id`，二者仍绑定真实 qualified ID 和实例。

### 本地 Ollama

需要支持该模型版本的原生 Ollama 镜像；例如 `tev1:0.8b` 的当前清单要求 Ollama 至少 0.40.4。更新后在「模型仓库」下载 `tev1:0.8b`，等待模型发现和资源绑定就绪，确认 `/api/show` 声明 `decision`，网关目录声明 `systemone`。Tev1、Nimble 是兼容 Jev API 的独立本地模型，不是 Jev 官方权重；中文准确率、质量与置信阈值须单独评测。

在 Auto 分类方式选择「System One 本地／云端分类器」，选择该本地决策模型。无需启用云端分类许可；建议先使用 30000 毫秒超时（含加载及资源准入），允许范围 100..120000。分类模型只参与决策，不得勾作聊天回答候选。模型仓库手动启动此类模型时也按真实 `systemone` 能力预热，不伪造聊天请求；停止仍核对原生卸载。

```json
{
  "classifier": "systemone",
  "jev_model": "v-scheduler/ollama-qa/tev1:0.8b",
  "jev_upstream_id": "v-scheduler/ollama-qa",
  "allow_cloud_classifier": false,
  "classifier_timeout_ms": 30000
}
```

这是完整配置的字段片段，合并到最新 GET 配置后 PUT，不可用此片段覆盖候选配置。本地分类经过原有 permit 和队列，分类后卸载并核验资源释放，再准入回答模型，避免分类器常驻阻塞同一 GPU 的回答模型。`X-VScheduler-Auto-Local-Only=true` 允许该本地分类器，仍禁止云端分类和云端回答。决策来源显示 `local_systemone`。

### 云端 Jev 与 OpenRouter

可添加 `typesafe` 协议上游：官方 Base URL `https://api.typesafe.ai/v1`；已有 OpenRouter Key 可使用 `https://openrouter.ai/api/v1`，手动选择模型 `typesafe/jev-1.14`。OpenRouter 的普通模型目录可能不列出决策模型，可在添加上游时明确指定 `models:["typesafe/jev-1.14"]`；以原生 `/systemone` 实际调用确认，不要把 `typesafe/jev-router` 聊天路由产品当作本分类器。

配置真实目录中的 `jev_model`、`jev_upstream_id`，启用 `allow_cloud_classifier=true`。默认建议 1000 毫秒，跨境网络可在有界范围内调整；本次实测使用 5000 毫秒。云端回答开关与云端分类开关独立。网关复用已保存的上游 Key，Auto 配置不复制 Key；本地限定请求不会调用云端分类器。决策来源显示 `jev`。

分类仅判断 task 与 complexity，不能指定 URL 或授予权限。最多截取最近 5 条消息建立独立分类视图，超过 16000 UTF-8 字节回退规则，原回答输入不变。分类请求不携带用户的调度控制头，不递归 Auto；全局最多 2 个并发分类。失败、超时、忙碌、无权限或 complexity confidence <0.7 时回退规则，并冷却 30 秒。该阈值是起始配置，不代表回答正确率。

**测试选型会实际运行可选分类器**：本地模型消耗本地算力；云端模型会发送分类文本并可能计费。测试不会运行回答模型。统计独立记录分类请求 token，并在回答的 `auto_routing.classification` 中关联决策；尚不提供账单总额保证。云端 Jev 经 OpenRouter 的真实分类、异常回退、超时恢复和本地回答 JSON/SSE 已验证。

## 会话与边界

多轮使用稳定的 `X-VScheduler-Session-ID`（1..96 位 ASCII 字母、数字、`_`、`-`）。绑定按用户/应用隔离，空闲 10 分钟过期，最多保留 512 条，不落盘。仍满足硬约束和质量档位时优先沿用；升级需求或故障可重新选择。

工具结果继续调用 Auto 时必须带原 session ID。若原模型不可用、权限/能力不再满足或会话过期，返回 409，要求调用方明确重启工具轮次，不把工具结果静默交给另一个模型。没有 session ID 的工具续轮返回 422。SSE 已开始后绝不拼接另一个模型的响应。

首期不路由 embedding、rerank、图片/视频/音频生成、异步任务或带供应商持久会话资源的 API；向量索引必须固定 embedding 模型。尚无在线 bandit、自学习质量评分、TTFT 预测和自动质量重试，档位与价格来自管理员配置，候选效果须通过实际业务样本验证。
