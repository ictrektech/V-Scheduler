## 模型适用领域

模型适用领域是独立于模型能力、协议和调度策略的多选分类。现有模型默认 `general`（通用）；内置分类包括法律、数学、物理、医药、化学、生物、工程、计算机、金融、教育和人文。可保留通用并同时选择专业领域。

在“模型与上游”添加单个模型、添加上游所选模型或注册模型时，展开“适用领域（可多选）”。已添加的手动模型在“编辑元数据”中修改；托管/发现模型在“模型设置 → 适用领域”中修改。填写“新增共享分类名称”并点“新增分类”，创建后自动勾选，其他模型可立即复用。自定义分类在该 V-Scheduler 实例内共享，名称不要包含私密信息。领域不是对模型真实专业水平的认证，Auto 会使用这些标注匹配当前步骤；领域不改变模型能力或访问权限。

API 地址仍使用 `http://v-scheduler:8080`，VOS 前端同域为 `/api/com.ictrek.v-scheduler`，外部访问使用实际宿主机映射端口（默认 22888）。管理操作使用 VOS 用户 token，个人/模型推理 Key 不授予修改权限。只能修改自己可管理的模型，管理员可管理其他模型。

先通过 `GET /api/v1/model-domains` 获取 `{domains:[{id,name,builtin}]}`。新增分类用 `POST /api/v1/model-domains` 和 `{name:"专利"}`，新增返回 201 `{domain:{id,name,builtin:false},created:true}`，同名分类去空白并忽略大小写去重，返回 200 和已有 ID。自定义 ID 为稳定的 `custom-...`，后续使用 ID 而不是名称；名称长度 1–80 字符，无控制字符，最多 1000 个自定义分类。当前不提供重命名或删除分类接口。

```sh
curl --fail-with-body http://v-scheduler:8080/api/v1/model-domains \
  -H "Authorization: Bearer $VOS_TOKEN"
curl --fail-with-body -X POST http://v-scheduler:8080/api/v1/model-domains \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"name":"专利"}'
curl --fail-with-body -X PATCH http://v-scheduler:8080/api/v1/models \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"upstream_id":"my-provider","model":"my-model","domains":["law","mathematics"]}'
```

模型编辑 `PATCH /api/v1/models` 支持 `domains`，可与手动模型的其他元数据一起提交；只提交 model、upstream_id、domains 时也适用于已发现或应用托管模型，不改变其能力。注册 `POST /api/v1/registrations` 以及手动上游创建/编辑均支持顶层 domains 作为本次所选模型默认值；`models[].domains` 可逐模型覆盖。字段省略时保留已有设置，即使刷新上游、心跳恢复或重新注册也不丢失；`[]` 明确重置为通用，null、未知 ID 或非字符串元素返回 400。每个模型最多 32 个领域，重复 ID 去重。恢复心跳不会用旧注册快照覆盖已持久化的用户编辑。

```json
{
  "service_id": "my-vllm",
  "protocol": "chat_completions",
  "base_url": "http://my-vllm:8000/v1",
  "domains": ["general"],
  "models": [{"id":"my-model","endpoints":["chat_completions"],"domains":["law","medicine"]}]
}
```

批量操作使用 `PATCH /api/v1/models/domains`，完整替换全部目标的领域。每批 1–200 项，逐项提供 upstream_id 和 model（目录 ID 或原生 ID）。即使两个上游具有同名模型，也按上游分别设置。服务先校验全部目标及管理权限，再一次提交；任一目标无权限、不存在、分类非法或持久化失败时全部不修改。成功返回 `{updated:2,domains:["law"]}`，updated 是去重后的目标数量。

```json
{
  "models": [
    {"upstream_id":"provider-a","model":"same-model"},
    {"upstream_id":"provider-b","model":"same-model"}
  ],
  "domains": ["law"]
}
```

`GET /api/v1/models` 在每个模型上返回 domains，并在顶层返回 domain_catalog；`GET /v1/models` 的真实模型条目也包含 domains。领域及共享分类持久保存在原注册表内，不需要迁移现有模型文件或更改调用 model ID。401/403 分别处理登录/管理权限，400 修正输入，404 核对模型及上游；存储失败返回 500，不应当作成功。

### 请求与返回速查

| 方法及路径 | 必填字段 | 成功返回 |
| --- | --- | --- |
| GET `/api/v1/model-domains` | 无请求体 | 200：domains 数组，每项 id/name/builtin |
| POST `/api/v1/model-domains` | name：字符串 | 201 新增或 200 已存在：domain 对象、created 布尔值 |
| PATCH `/api/v1/models` | model、upstream_id、domains | 200：`{model:{...原有模型字段,domains:[...]}}` |
| PATCH `/api/v1/models/domains` | models 数组、domains 数组 | 200：`{updated:去重目标数,domains:[...]}` |

```sh
curl --fail-with-body -X PATCH http://v-scheduler:8080/api/v1/models/domains \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"models":[{"upstream_id":"provider-a","model":"same-model"},{"upstream_id":"provider-b","model":"same-model"}],"domains":["law"]}'
```

未知分类示例：HTTP 400，响应的 `error.code` 为 `INVALID_REQUEST`，`error.message` 为 `unknown domain ID; create the category first`；先创建分类并使用返回的 ID，再重试。缺少管理权限为 HTTP 403 / FORBIDDEN；完整错误信封沿用现有网关契约，保留 request_id 用于排查，不记录 token。对模型领域的写操作要求 upstream_id；其他旧版元数据修改请求不因此改变必填规则。共享分类读取沿用网关发现鉴权（包括允许的推理 Key），写操作仍要求 VOS 用户/管理员身份。

自动发现和注册模型也可编辑领域，但该操作不授予修改托管服务能力或配置的权限。统一 `/v1/models` 目录的 domains 扩展针对没有 X-VRouter-Upstream 的目录请求；显式透传某个上游的原生 `/models` 响应不保证带此扩展。Auto 虚拟入口可由管理员设置领域；业务操作条目不是领域设置目标。

### 内置分类 ID

| ID | 名称 |
| --- | --- |
| `general` | 通用 |
| `law` | 法律 |
| `mathematics` | 数学 |
| `physics` | 物理 |
| `medicine` | 医药 |
| `chemistry` | 化学 |
| `biology` | 生物 |
| `engineering` | 工程 |
| `computing` | 计算机 |
| `finance` | 金融 |
| `education` | 教育 |
| `humanities` | 人文 |

心跳的领域字段只是完整恢复快照中的元数据：普通心跳及已知服务恢复均保留持久设置，不用于修改领域；首次创建未知服务的完整心跳才采用快照领域。修改现有模型请使用 PATCH /api/v1/models、批量接口或重新 POST /api/v1/registrations。内置分类的 API name 为规范名称，界面会本地化显示；自定义名称保留创建者填写的文字。

### Auto 按步骤选择领域与复杂度

在 Auto 配置页的“适用领域（可多选）”选择允许使用的领域，也可直接新增共享分类。只选 `general` 时不限制候选模型领域，兼容原默认候选池；选择专业后，候选至少有一个所选领域才合格，同时保留 `general` 可使用标注通用的回退模型。所有候选仍需通过身份可见性、协议/工具/视觉能力、上下文上限、复杂度档位和资源准入检查，领域匹配不会绕过这些条件。

每次请求都会对当前步骤重新分类：规则读取最新用户消息，工具续轮参考最近消息及工具名/参数；不确定则为通用。规则按内置关键词及自定义分类名称匹配，是启发式判断。启用聊天模型或 Jev/OpenAI Decisions/System One 分类器时，会向分类器提供当前可见候选使用的领域词表（最多 64 类）；有效且置信度至少 0.7 的领域结果优先使用，缺失/非法/低置信度的领域结果回退规则。旧分类器只返回任务/复杂度也能继续使用。整体分类失败/超时仍使用既有规则回退，分类费用/云端许可保持原配置。

先匹配当前专业，再按均衡/速度/质量/成本策略排序；没有合格专业模型时，仅能回退到允许的通用模型；若也没有通用模型，返回既有“无合格候选”结果，不强行调用不匹配的专业模型。显式手工候选的复杂度档位仍有效；自动候选池不会凭模型名字猜测质量档位。领域标签由用户维护，不能代替真实模型能力验证。

例如 Harness 先请求法律分析、下一次请求数学推导，仍发送 `model=auto`，即可分别选择合格法律模型和数学模型。每次推理选型都基于当前上下文，应用要提供完整必要上下文。稳定会话 ID 不再固定整段对话：普通新步骤重新排序；但未完成的工具调用续轮沿用原模型，避免破坏工具上下文。工具续轮必须沿用 `X-VScheduler-Session-ID`（1–96 个 ASCII 字母、数字、下划线或连字符），按调用身份隔离、空闲 10 分钟到期。若原模型不可用/会话过期返回 409，缺少此头返回 422，由调用方明确重启工具轮。SSE 开始后不会拼接另一模型的响应。客户端只会重复请求 endpoint 并不等于已实现工具会话头转发，接入时需单独确认。

管理员可用下列两种 API 修改同一个 Auto 领域选择。最小 PATCH 仅改领域，不覆盖其他 Auto 配置；批量接口也接受同样的 auto/auto 目标，批内出现 Auto 时要求管理员。实际改变会递增 revision 并清除旧的会话选型缓存，工具回合进行中应避免修改全局配置。

```sh
curl --fail-with-body -X PATCH "$GATEWAY/api/v1/models" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"model":"auto","upstream_id":"auto","domains":["general","law","mathematics"]}'
curl --fail-with-body "$GATEWAY/api/v1/auto-routing" \
  -H "Authorization: Bearer $VOS_TOKEN" > auto-current.json
jq '.domains=["general","law","mathematics"]' auto-current.json > auto-next.json
curl --fail-with-body -X PUT "$GATEWAY/api/v1/auto-routing" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  --data-binary @auto-next.json
curl --fail-with-body "$GATEWAY/api/v1/auto-routing/test" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"model":"auto","messages":[{"role":"user","content":"Review this legal contract"}],"max_tokens":256}'
```

GET/PUT `/api/v1/auto-routing` 和 GET `/api/v1/auto-routing/candidates` 仅管理员可用。PUT 先读取完整配置、保留 revision 和其他字段后修改提交，revision 冲突为 409，重新读取再编辑；省略 domains 保留原选择，[] 重置通用。目录候选返回 `models[].domains` 和顶层 `domain_catalog`。POST `/api/v1/auto-routing/test` 允许已验证的模型调用身份，返回 `classification.domains`、`domain_source`（rules/classifier/rules_fallback）、可空的 `domain_confidence`，以及任务、复杂度、实际模型和候选排除原因。`domain_not_configured` 表示不在允许领域，`domain_mismatch` 表示不匹配本步骤，`domain_general_fallback` 表示选用了通用回退。测试选型不执行回答模型，但启用模型分类器仍可能产生分类费用。普通和流式实际推理沿用 `/v1/chat/completions`、`/v1/messages`，请求 model=auto，不新增专业请求头。

---

## Model domains

Domains are multiple-selection classifications, independent of capabilities, protocols and scheduling. Existing models default to `general`. Built-in options include Law, Mathematics, Physics, Medicine, Chemistry, Biology, Engineering, Computing, Finance, Education and Humanities. General may be selected alongside specialist categories.

When adding a single model, selecting a provider's models or registering a service, use Applicable domains. For existing manual models choose Edit metadata; for managed/discovered models use Model settings → Applicable domains. Enter a New shared category name and click Add category; it is selected automatically and becomes reusable by other models. Categories are shared across this V-Scheduler instance: do not put private information in their names. A domain label is not certification of expertise and is consumed by Auto when matching the current step, without changing model capabilities or access permissions.

The internal base is `http://v-scheduler:8080`; the VOS same-origin base is `/api/com.ictrek.v-scheduler`. External clients use the installed host mapping (default 22888). Writes require a VOS user token. Personal/model inference Keys cannot modify metadata. Owners manage their models; administrators may manage other models.

GET `/api/v1/model-domains` returns `{domains:[{id,name,builtin}]}`. POST to the same path with `{name:"Patents"}` creates a category: 201 `{domain:{id,name,builtin:false},created:true}`. Names are trimmed and compared case-insensitively; duplicates return 200 and the existing ID. Use the stable `custom-...` ID in subsequent assignments, not the display name. Names contain 1–80 characters without control characters; up to 1000 custom categories are supported. There is currently no category rename/delete API.

```sh
curl --fail-with-body http://v-scheduler:8080/api/v1/model-domains \
  -H "Authorization: Bearer $VOS_TOKEN"
curl --fail-with-body -X POST http://v-scheduler:8080/api/v1/model-domains \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"name":"Patents"}'
curl --fail-with-body -X PATCH http://v-scheduler:8080/api/v1/models \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"upstream_id":"my-provider","model":"my-model","domains":["law","mathematics"]}'
```

PATCH `/api/v1/models` accepts domains alongside manual model metadata. A domains-only payload containing model/upstream_id/domains also works for managed/discovered models without changing their capabilities. POST `/api/v1/registrations` and manual provider creation/update accept top-level domains as the default for the submitted models; `models[].domains` overrides that default per model. Omission preserves saved annotations through provider refresh, heartbeat recovery and re-registration; `[]` explicitly resets to General. Null, unknown IDs or non-string entries return 400. A model may have up to 32 domains; duplicate IDs are removed. Recovery heartbeats never replace saved user edits with stale registration snapshots.

```json
{
  "service_id": "my-vllm",
  "protocol": "chat_completions",
  "base_url": "http://my-vllm:8000/v1",
  "domains": ["general"],
  "models": [{"id":"my-model","endpoints":["chat_completions"],"domains":["law","medicine"]}]
}
```

PATCH `/api/v1/models/domains` atomically replaces the domain selection of 1–200 targets. Each entry requires upstream_id and model (catalog ID or native ID), so same-name models on different providers remain distinct. Every target and permission is validated before committing. An unauthorized/missing target, invalid category or storage failure changes none. Success returns `{updated:2,domains:["law"]}`, where updated counts distinct targets.

```json
{
  "models": [
    {"upstream_id":"provider-a","model":"same-model"},
    {"upstream_id":"provider-b","model":"same-model"}
  ],
  "domains": ["law"]
}
```

GET `/api/v1/models` returns per-model domains and top-level domain_catalog. Real-model entries in GET `/v1/models` also include domains. Categories and annotations persist in the existing registry; no weight-directory migration or model-ID change is needed. Handle 401/403 as authentication/management permission errors, 400 as invalid input and 404 as an unavailable target. Storage errors return 500 and must not be treated as successful updates.

### Request and response reference

| Method and path | Required fields | Success |
| --- | --- | --- |
| GET `/api/v1/model-domains` | No body | 200: domains array containing id/name/builtin |
| POST `/api/v1/model-domains` | name: string | 201 new or 200 existing: domain object and created boolean |
| PATCH `/api/v1/models` | model, upstream_id, domains | 200: `{model:{...existing model fields,domains:[...]}}` |
| PATCH `/api/v1/models/domains` | models array, domains array | 200: `{updated:distinct target count,domains:[...]}` |

```sh
curl --fail-with-body -X PATCH http://v-scheduler:8080/api/v1/models/domains \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"models":[{"upstream_id":"provider-a","model":"same-model"},{"upstream_id":"provider-b","model":"same-model"}],"domains":["law"]}'
```

An unknown category returns HTTP 400, error.code `INVALID_REQUEST` and error.message `unknown domain ID; create the category first`. Create the category and retry using its returned ID. Insufficient management access returns HTTP 403 / FORBIDDEN. The existing gateway error envelope remains unchanged; keep request_id for diagnostics without logging tokens. Domain writes require upstream_id; existing metadata-only requests keep their previous required-field rules. Catalogue reads follow gateway discovery authentication, including permitted inference Keys; writes still require VOS user/administrator identity.

Managed/discovered models allow domain editing without granting permission to rewrite their service capabilities/configuration. The domains extension in the unified `/v1/models` catalogue applies to requests without X-VRouter-Upstream; a pinned upstream's native `/models` response need not contain it. Administrators can assign domains to the Auto virtual entry; business-operation records are not domain-assignment targets.

### Built-in category IDs

| ID | Name |
| --- | --- |
| `general` | General |
| `law` | Law |
| `mathematics` | Mathematics |
| `physics` | Physics |
| `medicine` | Medicine |
| `chemistry` | Chemistry |
| `biology` | Biology |
| `engineering` | Engineering |
| `computing` | Computing |
| `finance` | Finance |
| `education` | Education |
| `humanities` | Humanities |

Domain fields in heartbeats are recovery-snapshot metadata. Ordinary heartbeats and recovery of known services retain persisted annotations and are not domain-edit operations; only a first full heartbeat creating an unknown service adopts snapshot domains. Edit existing models with PATCH /api/v1/models, the batch endpoint, or an explicit POST /api/v1/registrations. Built-in API names are canonical labels localized by the UI; custom names retain their author’s text.

### Auto: domain and complexity selection for each step

In Auto settings, select Applicable domains or create a shared category. Selecting only `general` leaves the domain pool unrestricted, preserving the default pool. Selecting specialties restricts candidates to models annotated with at least one selected domain; retaining `general` allows General fallback models. Visibility, protocol/tool/vision support, context limits, configured complexity tiers and resource admission still apply. Domain matching never grants execution permission.

Every request is classified again for its current step. Rules read the latest user message, or recent messages/tool names and arguments during a tool continuation; uncertainty yields General. Built-in keywords and custom category names provide heuristic matching. LLM and Jev/OpenAI Decisions/System One classifiers receive the domains used by currently visible candidates (up to 64 categories). Valid domain results with confidence at least 0.7 take precedence; missing, invalid or low-confidence domain answers fall back to rules. Existing classifiers returning only task/complexity remain compatible. Complete classifier failure/timeouts retain existing rule fallback and cloud-cost/permission controls.

Matching specialists rank ahead of General fallbacks, then the selected balanced/fast/quality/economy strategy applies. If no specialist is eligible, only an allowed General model can be used; if none exists, the existing no-eligible-candidate result applies. Explicit candidate complexity tiers remain enforced. The automatic pool does not infer quality tiers from model names. User-maintained labels do not certify actual model expertise.

For example, a Harness request for legal analysis followed by a mathematical derivation can both use `model=auto` and select different eligible specialists. Supply the context needed for each step. A stable session ID no longer pins the whole conversation: fresh steps rerank. Unfinished tool turns retain their model to preserve tool context. Tool continuations must forward the same `X-VScheduler-Session-ID` (1–96 ASCII letters, digits, underscores or hyphens); identity-isolated affinity expires after 10 idle minutes. An expired/unavailable session returns 409; a missing header returns 422. The caller must explicitly restart the tool turn. Models never switch midway through an SSE response. Repeated endpoint access alone does not establish tool-session header forwarding; verify the client integration separately.

Administrators can modify the same Auto domain selection through either API below. The minimal PATCH changes only domains. Batch assignment accepts the same auto/auto target; any batch containing Auto requires an administrator. Actual changes increment revision and clear previous selection-session caches, so avoid global configuration edits during an active tool turn.

```sh
curl --fail-with-body -X PATCH "$GATEWAY/api/v1/models" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"model":"auto","upstream_id":"auto","domains":["general","law","mathematics"]}'
curl --fail-with-body "$GATEWAY/api/v1/auto-routing" \
  -H "Authorization: Bearer $VOS_TOKEN" > auto-current.json
jq '.domains=["general","law","mathematics"]' auto-current.json > auto-next.json
curl --fail-with-body -X PUT "$GATEWAY/api/v1/auto-routing" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  --data-binary @auto-next.json
curl --fail-with-body "$GATEWAY/api/v1/auto-routing/test" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"model":"auto","messages":[{"role":"user","content":"Review this legal contract"}],"max_tokens":256}'
```

GET/PUT `/api/v1/auto-routing` and GET `/api/v1/auto-routing/candidates` require an administrator. For PUT, read the full configuration first, retain revision and other fields, then submit changes. A revision conflict returns 409; reload before editing again. Omitting domains preserves the stored selection; [] resets General. The candidate catalog returns `models[].domains` and top-level `domain_catalog`. POST `/api/v1/auto-routing/test` accepts verified inference identities and returns `classification.domains`, `domain_source` (rules/classifier/rules_fallback), nullable `domain_confidence`, task, complexity, chosen model and candidate exclusions. `domain_not_configured` means outside the allowed domains; `domain_mismatch` means incompatible with this step; `domain_general_fallback` identifies a General fallback. Selection testing does not execute the answer model, but model-based classification may incur cost. Normal and streaming inference continue through `/v1/chat/completions` or `/v1/messages`, with model=auto and no new domain header.
