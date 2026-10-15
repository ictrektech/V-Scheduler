// Package api exposes the OpenAI-compatible inference API and the
// management API of the gateway.
package api

import (
	"encoding/json"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

// Server carries shared state for all HTTP handlers.
type Server struct {
	vSubDiscovery *registry.VSubDiscovery
	reg           *registry.Registry
	version       string
	apiKey        string
	keyRequired   bool
	usage         *usageLog

	// OIDC identity verification against the VOS userinfo endpoint.
	userInfoURL       string
	authDisabled      bool
	oidcClient        *http.Client
	idMu              sync.RWMutex
	identityCache     map[string]cachedIdentity
	lastUserInfoError string
	probeOKUntil      time.Time

	// allowLoopbackUpstreams relaxes the proxy-loop guard for tests, where
	// fake upstreams listen on 127.0.0.1.
	allowLoopbackUpstreams bool
}

func New(reg *registry.Registry, version string) http.Handler {
	return newServer(reg, version).routes()
}

func newServer(reg *registry.Registry, version string) *Server {
	s := &Server{
		reg:           reg,
		version:       version,
		usage:         newUsageLog(envOr("V_ROUTER_USAGE_PATH", "")),
		userInfoURL:   resolveUserInfoURL(),
		authDisabled:  os.Getenv("V_ROUTER_AUTH_DISABLED") == "true",
		oidcClient:    &http.Client{Timeout: 10 * time.Second},
		identityCache: map[string]cachedIdentity{},
	}
	s.vSubDiscovery = registry.NewVSubDiscovery(reg, strings.Split(envOr("V_ROUTER_V_SUB_URLS", "http://v-sub-backend:7090,http://host.docker.internal:22703"), ","))
	if os.Getenv("V_ROUTER_REQUIRE_KEY") == "true" {
		s.keyRequired = true
		s.apiKey = os.Getenv("V_ROUTER_API_KEY")
	}
	return s
}

// apiRouteSpec is the single source of truth for one endpoint: routes()
// registers the mux handler from it and GET /api/v1/apidoc renders the
// documentation from it, so the docs page can never drift from what the
// running binary actually serves.
type apiRouteSpec struct {
	Method  string
	Path    string
	Group   string // system | openai | manage
	Summary string
	Desc    string
	Example string
	// Auth names the guard routes() applies: public = 无鉴权,
	// inference = 匿名内网/网关 Key/OIDC 用户归因, manage = 必须登录.
	Auth    string
	Handler http.HandlerFunc
}

func (s *Server) spec() []apiRouteSpec {
	routes := []apiRouteSpec{
		{
			Method: "GET", Path: "/healthz", Group: "system", Auth: "public",
			Handler: s.handleHealthz,
			Summary: "健康检查",
			Desc:    "容器健康检查端点，返回 200 和 ok，无需鉴权。",
		},
		{
			Method: "GET", Path: "/api/v1/auth-probe", Group: "system", Auth: "public",
			Handler: s.handleAuthProbe,
			Summary: "登录诊断",
			Desc:    "免鉴权的 OIDC 连通性诊断：报告 userinfo 端点是否可达与最近错误，用于排查管理界面 401。",
		},
		{
			Method: "GET", Path: "/v1/models", Group: "openai", Auth: "inference",
			Handler: s.handleListModels,
			Summary: "模型列表",
			Desc:    "合并所有健康上游的模型。只返回带来源前缀的模型名，例如 model-hub/xxx、image-studio/xxx、registered/xxx、manual/xxx、com.ictrek.ai/xxx。",
			Example: `curl http://<vos-host>:22888/v1/models`,
		},
		{
			Method: "POST", Path: "/v1/chat/completions", Group: "openai", Auth: "inference",
			Handler: s.handleChatCompletions,
			Summary: "对话补全（支持 SSE 流式）",
			Desc:    "OpenAI 兼容。model 必须使用 /v1/models 返回的带来源前缀名称；请求原样透传到上游，stream=true 时逐 token 转发。",
			Example: `curl -N http://<vos-host>:22888/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"model-hub/qwen3.5:2b","stream":true,"messages":[{"role":"user","content":"你好"}]}'`,
		},
		{
			Method: "POST", Path: "/v1/embeddings", Group: "openai", Auth: "inference",
			Handler: s.handleEmbeddings,
			Summary: "文本向量",
			Desc:    "OpenAI 兼容。路由到 embedding 角色的上游（如 Model Hub 的 bge-m3，或自动发现 ML Server 的 Qwen3/CLIP 文本向量）。ML Server 不报告 tokenizer 用量，因此响应省略 usage，调用仍正常完成。",
			Example: `curl http://<vos-host>:22888/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{"model":"model-hub/bge-m3","input":"你好"}'`,
		},
		{
			Method: "GET", Path: "/api/v1/status", Group: "manage", Auth: "manage",
			Handler: s.handleStatus,
			Summary: "网关状态",
			Desc:    "版本、上游健康数、模型数、累计请求、是否启用 API Key。",
		},
		{
			Method: "GET", Path: "/api/v1/models", Group: "manage", Auth: "manage",
			Handler: s.handleListModelsRich,
			Summary: "模型列表（含元数据）",
			Desc:    "管理视图：每个模型带上游名称、上游类型、是否短名别名等元数据。",
		},
		{
			Method: "GET", Path: "/api/v1/usage", Group: "manage", Auth: "manage",
			Handler: s.handleUsage,
			Summary: "用量统计",
			Desc:    "从 JSONL 历史按模型、上游和用户汇总请求及上游报告的输入/输出 Token；period 支持 day（默认）、week、month、all。上游不报告 usage 时请求仍正常完成。",
		},
		{
			Method: "GET", Path: "/api/v1/upstreams", Group: "manage", Auth: "manage",
			Handler: s.handleListUpstreams,
			Summary: "上游列表",
			Desc:    "当前用户可见的全部上游（本地发现 + 云端/手动添加 + 主动注册），密钥不回显。每个上游带 status 状态分档：healthy 健康；paused 调度已暂停；lease_expired 注册租约过期；circuit_open 连续连接失败熔断中（failures 连续失败次数、circuit_open_until 冷却结束时间，冷却后下一个请求自动探测恢复）；auth_failed 模型探测返回 401/403（detail 带原始原因）；discovery_failed 发现或探测失败；no_models 上游可达但没有可用模型；full 进行中请求已达 max_inflight。manageable 表示当前身份能否修改、刷新、删除该上游或配置其调度（owner 或管理员）。private 上游只出现在 owner 和管理员的列表里。",
		},
		{
			Method: "POST", Path: "/api/v1/upstreams/probe", Group: "manage", Auth: "manage",
			Handler: s.handleProbeUpstream,
			Summary: "测试云端/手动上游",
			Desc:    "按当前表单参数探测模型列表；可提交 test_model 做一次轻量联通测试。失败只返回诊断结果，不保存上游。",
		},
		{
			Method: "POST", Path: "/api/v1/upstreams", Group: "manage", Auth: "manage",
			Handler: s.handleCreateUpstream,
			Summary: "添加云端或手动上游",
			Desc:    "登记上游，kind 支持 manual/cloud，protocol 支持 openai（原生）、chat_completions（转换 Responses/Messages）、anthropic（原生）、offline_asr（离线 ASR /transcribe 适配）、image_studio（Image Studio 生图/生视频适配）、async_task（通用异步图片任务创建/轮询适配）。云端模型前缀按域名反转生成，例如 ai.ictrek.com -> com.ictrek.ai。默认保存前探测模型列表；async_task 改为校验单模型及任务字段，不创建付费任务。visibility 支持 public（共享，默认，所有调用者可见可用）和 private（仅创建者与管理员可见可用）；创建者身份记录为 owner，之后只有 owner 和管理员能修改、刷新、删除该上游或配置调度。历史存量上游 owner 为空，升级后保持共享且所有登录用户可管理。",
			Example: `curl -X POST http://<vos-host>:22888/api/v1/upstreams \
  -H "Content-Type: application/json" \
  -d '{"kind":"cloud","name":"ICTrek AI","base_url":"https://ai.ictrek.com/v1","api_key":"sk-...","visibility":"public"}'`,
		},
		{
			Method: "PUT", Path: "/api/v1/upstreams/{id}", Group: "manage", Auth: "manage",
			Handler: s.handleUpdateUpstream,
			Summary: "更新云端或手动上游",
			Desc:    "修改名称、地址、协议、密钥或可见范围；api_key 留空表示保留原密钥。云端上游会按新 base_url 重新计算模型前缀。仅 owner 和管理员可调用：非 owner 修改他人 public 上游返回 403，访问他人 private 上游返回 404。owner 不可转移。",
		},
		{
			Method: "POST", Path: "/api/v1/upstreams/{id}/refresh", Group: "manage", Auth: "manage",
			Handler: s.handleRefreshUpstream,
			Summary: "刷新上游模型",
			Desc:    "重新探测 API 创建的云端/手动上游并刷新其模型列表。仅 owner 和管理员可调用。",
		},
		{
			Method: "DELETE", Path: "/api/v1/upstreams/{id}", Group: "manage", Auth: "manage",
			Handler: s.handleDeleteUpstream,
			Summary: "删除云端或手动上游",
			Desc:    "删除后其模型立即从 /v1/models 移除，同时清除该上游的资源归属记录；Model Hub 发现和后端注册的上游不可在此删除。仅 owner 和管理员可调用。",
		},
		{
			Method: "GET", Path: "/api/v1/apidoc", Group: "manage", Auth: "manage",
			Handler: s.handleApidoc,
			Summary: "API 文档数据",
			Desc:    "返回当前版本全部端点的说明与示例；管理界面的“API 文档”页由此实时渲染，永远与运行中的接口一致。",
		},
	}
	routes = append(routes, s.extendedRoutes()...)
	routes = append(routes, s.videoStructureRoutes()...)
	return append(routes, s.routingRoutes()...)
}

var apiDocGroups = []struct {
	ID    string `json:"id"`
	Title string `json:"title"`
}{
	{"system", "系统"},
	{"openai", "推理 API（OpenAI 兼容）"},
	{"anthropic", "推理 API（Anthropic 兼容）"},
	{"manage", "管理 API"},
	{"resource", "资源接口 · 指定上游"},
	{"video-structure", "视频结构化 API"},
}

func (s *Server) routes() http.Handler {
	mux := http.NewServeMux()
	for _, r := range s.spec() {
		h := r.Handler
		switch r.Auth {
		case "inference":
			h = s.guardAPI(h)
		case "manage":
			h = s.requireIdentity(h)
		}
		mux.HandleFunc(r.Method+" "+r.Path, h)
	}
	unifiedController(mux)
	return protectGateway(mux)
}

// handleApidoc renders the documentation for every registered endpoint from
// the same spec that drives mux registration.
func (s *Server) handleApidoc(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	type endpoint struct {
		Method  string `json:"method"`
		Path    string `json:"path"`
		Summary string `json:"summary"`
		Desc    string `json:"desc"`
		Auth    string `json:"auth"`
		Example string `json:"example,omitempty"`
	}
	groups := make([]map[string]any, 0, len(apiDocGroups))
	for _, g := range apiDocGroups {
		eps := []endpoint{}
		for _, r := range s.spec() {
			if r.Group == g.ID {
				eps = append(eps, endpoint{r.Method, r.Path, r.Summary, r.Desc, r.Auth, r.Example})
			}
		}
		groups = append(groups, map[string]any{"id": g.ID, "title": g.Title, "endpoints": eps})
	}
	writeDocumentation(w, r, map[string]any{
		"version":      s.version,
		"key_required": s.requiresKey(),
		"oidc_enabled": s.userInfoURL != "" && !s.authDisabled,
		"identity": []map[string]string{
			{
				"channel": "VOS 内应用 / 管理界面",
				"method":  "Authorization: Bearer <VOS OIDC access_token>",
				"effect":  "调用归集到对应 VOS 用户；管理 API 必须以此登录（或使用网关 Key）。其他 VOS 应用的 fastpath token 同样有效：其前端携带自身 token 调用同域 /api/com.ictrek.v-router/v1/* 即按该应用当前用户归集，无需向 V-Router 注册",
			},
			{
				"channel": "局域网 / 外部客户端",
				"method":  "Authorization: Bearer <V_ROUTER_API_KEY>（启用校验后）",
				"effect":  "以 gateway-admin 身份调用，计入管理员用量",
			},
			{
				"channel": "VOS 内网匿名调用",
				"method":  "无凭证",
				"effect":  "仅关闭 REQUIRE_KEY 时模型推理可匿名使用，归集为 internal；管理 API 和显式上游资源接口拒绝匿名",
			},
		},
		"integration": []map[string]string{
			{"title": "局域网客户端：VOS 登录后直连映射端口", "example": `# VOS_API is the reachable VOS API base URL (for example http://<vos-host>:8105).
# login.json contains {"username":"<username>","password":"<password>","application":"nas"}.
# Keep login.json private; never store credentials in client source code.
VOS_API=http://<vos-host>:8105
ROUTER=http://<vos-host>:22888
VOS_TOKEN=$(curl --fail-with-body -sS "$VOS_API/v1000/user/login" \
  -H 'Content-Type: application/json' --data-binary @login.json | jq -er '.data.access_token')
curl --fail-with-body "$ROUTER/api/v1/auth/me" -H "Authorization: Bearer $VOS_TOKEN"
curl --fail-with-body "$ROUTER/v1/models" -H "Authorization: Bearer $VOS_TOKEN"
curl --fail-with-body "$ROUTER/v1/chat/completions" \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"model":"<qualified-model-id-from-models>","messages":[{"role":"user","content":"Hello"}]}'`, "note": "先通过 VOS API 的 POST /v1000/user/login 获取 data.access_token，再携带 Authorization: Bearer <access_token> 调用 V-Router 映射端口。默认 22888，可通过安装参数 V_ROUTER_HOST_PORT 修改；VOS API 地址使用设备实际可访问的 API 入口（例如 http://<vos-host>:8105，或平台已配置的 HTTPS 反代入口）。V-Router 会向 VOS userinfo 校验 token，并按该 VOS 用户归集用量及应用资源权限，无需另建 V-Router 账户。先调用 /api/v1/auth/me 核对身份；401 时重新登录或刷新 token，不能用 VOS 密码替代 Bearer token。服务端调用不受浏览器 CORS 限制；跨域网页应通过同域代理接入。"},
			{"title": "算力路径：云端、本机与其他节点", "example": `云端模型：应用 → V-Router → 云端供应商（无需本地许可）
本机或已纳管其他节点的同步模型：应用 → V-Router → V-Scheduler permit → 模型服务
自有异步任务：应用 → V-Scheduler task → 已批准的节点执行器`, "note": "按实际执行节点分类，不按 URL 猜测。云端上游不占 V-Scheduler 的本地 CPU/GPU；其他已纳管节点按目标节点资源池预约。未纳管远端上游不能声称由调度器管理。同一计算只由 permit 或 task 预约一次。"},
			{"title": "本地模型接入：由网关申请许可", "example": `const result = await fetch('/api/com.ictrek.v-router/v1/chat/completions', {
  method: 'POST',
  headers: {Authorization: 'Bearer ' + token, 'Content-Type': 'application/json'},
  body: JSON.stringify({model: 'model-hub/<model>', messages: [{role: 'user', content: 'Hello'}]})
});`, "note": "调用方只使用 V-Router 模型 API，不持有 V-Scheduler 服务令牌。管理员须安装 Scheduler、批准真实节点与带 upstream_urls 的 executor，在 scheduler.local.json 的 models[upstreamID/nativeModelID] 批准输入与资源参数上限。VOS 包依赖 Scheduler >=0.0.11；未批准/缺配置返回 503，资源忙返回 429，超输入返回 413，超参数返回 422。HTTP/SSE、WebSocket 和异步创建均先准入；异步接力、运行任务子调用须核验任务、用户、节点及预算，不能凭 delegated 跳过。WebSocket 须批准 server_close 停止契约；未知停止保留预约，异步日志重启恢复轮询。云端绕过本地许可。"},
			{"title": "其他 VOS 应用：用 V-Router 模型列表生成下拉框", "example": `const token = await vosFastpathToken();
const models = await fetch('/api/com.ictrek.v-router/v1/models', {
  headers: { Authorization: 'Bearer ' + token }
}).then(r => r.json());
// 用 models.data[].id 填充模型下拉框；不要在本应用保存云端 key、上游地址或模型清单。`, "note": "推荐方式是用户先在 V-Router 里配置 Model Hub、云端 API、上游 key、权重和并发；其他应用只读取模型列表并调用统一 /v1/*。同一个 VOS 账户在任一应用登录后，其 Fastpath/OIDC token 可直接访问 V-Router，同域路径为 /api/com.ictrek.v-router。"},
			{"title": "其他 VOS 应用：读取带来源元数据的模型列表", "example": `const detail = await fetch('/api/com.ictrek.v-router/api/v1/models', {
  headers: { Authorization: 'Bearer ' + token }
}).then(r => r.json());
// detail.models 包含 qualified、kind、capabilities、endpoints、upstream_kind、model_prefix，可用于筛选对话/Embedding 下拉。`, "note": "普通下拉框用 /v1/models 即可；需要按能力筛选时用 /api/v1/models。能力未知时不要在应用侧硬拦截，调用后显示 V-Router 或上游返回的具体错误。"},
			{"title": "指定来源与指定上游", "example": `curl http://<vos-host>:22888/v1/chat/completions -H "Authorization: Bearer $VOS_TOKEN" -H "Content-Type: application/json" -d '{"model":"model-hub/qwen3:0.6b","messages":[{"role":"user","content":"你好"}]}'`, "note": "模型名必须带来源前缀，例如 model-hub/、registered/、com.ictrek.ai/。需要固定实例时附加 X-VRouter-Upstream:<ID> 严格指定后端（模型用上游原生名）。"},
			{"title": "注册模型 API：public 或 private", "example": `curl http://<vos-host>:22888/api/v1/registrations \
  -H "Authorization: Bearer $VOS_TOKEN" -H "Content-Type: application/json" \
  -d '{"service_id":"vllm-node-1","visibility":"public","base_url":"http://<backend>:8000/v1","protocol":"chat_completions","model_kind":"chat","ttl_seconds":90}'`, "note": "visibility=public 时所有 VOS 用户和匿名内网推理都可在 /v1/models 看到；visibility=private 时只对注册者同一 VOS token 和管理员可见。每个实例用稳定且不同的 service_id；模型变化需再次注册。"},
			{"title": "注册单个模型：声明接口和能力", "example": `curl http://<vos-host>:22888/api/v1/registrations \
  -H "Authorization: Bearer $VOS_TOKEN" -H "Content-Type: application/json" \
  -d '{"service_id":"embed-node-1","visibility":"private","base_url":"http://<backend>:8000/v1","protocol":"openai","model":"bge-m3","endpoints":["embeddings"],"capabilities":["embedding"],"ttl_seconds":90}'`, "note": "单模型注册不会探测 /models，必须提供 model 和 endpoints。endpoints 决定可访问接口，capabilities/model_kind 用于旧客户端和能力兜底；不支持的接口由 V-Router 或上游返回具体错误。"},
			{"title": "注册离线 ASR 服务", "example": `curl http://<vos-host>:22888/api/v1/registrations \
  -H 'Content-Type: application/json' \
  -d '{"service_id":"asr-192","visibility":"public","base_url":"http://192.168.1.192:27100","protocol":"offline_asr","model":"offline-asr","endpoints":["audio/transcriptions"],"capabilities":["asr"],"ttl_seconds":90}'`, "note": "offline_asr 将 OpenAI 兼容 /v1/audio/transcriptions 的 multipart file 转成上游 POST /transcribe?lang=<language> 原始音频调用，适合 VOS 或宿主机上的离线语音识别服务。"},
			{"title": "注册租约续期和注销", "example": `curl -X POST http://<vos-host>:22888/api/v1/registrations/registered-vllm-node-1/heartbeat \
  -H "Authorization: Bearer $VOS_TOKEN" -H "Content-Type: application/json" -d '{"ttl_seconds":90}'

curl -X DELETE http://<vos-host>:22888/api/v1/registrations/registered-vllm-node-1 \
  -H "Authorization: Bearer $VOS_TOKEN"`, "note": "响应返回 registered-<service_id>，建议每 ttl/3 秒续期。心跳 404 表示租约过期、网关重启或当前身份不是 owner/admin；需要重新注册。"},
			{"title": "先验证当前身份", "example": `curl http://<vos-host>:22888/api/v1/auth/me -H "Authorization: Bearer $VOS_TOKEN"`, "note": "其他 VOS 应用携带自身 Fastpath token 即可。401 时重新授权；无效凭证不会降级匿名。注册服务支持 public/private 可见范围；private 只对注册者身份和管理员可见。"},
			{"title": "固定上游访问资源", "example": `curl http://<vos-host>:22888/v1/files -H "Authorization: Bearer $VOS_TOKEN" -H "X-VRouter-Upstream: <id>"`, "note": "从上游页取得 ID，创建、查询和删除保持同一上游；模型字段使用原生名称。经 V-Router 创建的资源会记录创建者，其他用户访问这些资源返回 404（owner 和管理员不受限）；列表返回上游共享账户的全部资源。不要把共享网关 Key 放到公开前端。"},
			{"title": "V-Voice 语音转写：热词、时间戳与说话人归属", "example": `curl http://<vos-host>:22888/v1/audio/transcriptions \
  -H "Authorization: Bearer $VOS_TOKEN" \
  -F model=v-voice/qwen3-asr-0.6b -F file=@sample.wav \
  -F hotwords='V-Router,昇腾' -F hotwords_set=hw_1a2b3c4d \
  -F 'timestamp_granularities[]=word' \
  -F speaker_model=v-voice/speaker-campplus-zh-en -F speaker_threshold=0.6 \
  -F response_format=verbose_json`, "note": "model 与 speaker_model 都填 v-voice/ 前缀的网关模型 ID，V-Router 在转发边界还原原生 ID。verbose_json 返回 text、words/segments（以 word_timestamps capability 为准）、hotwords（含 applied 与生效 sets）和 speaker 归属块；V-Router 不裁剪任何字段。按 capabilities 筛选模型，不要硬编码模型清单。"},
			{"title": "V-Voice 流式识别与声纹接入", "example": `curl -N -X POST 'http://<vos-host>:22888/v1/audio/streaming/transcriptions?model=v-voice/zipformer-zh-streaming&format=pcm' \
  -H "Authorization: Bearer $VOS_TOKEN" -H 'Transfer-Encoding: chunked' --data-binary @audio.pcm

curl 'http://<vos-host>:22888/v1/audio/speakers?model=v-voice/speaker-campplus-zh-en' -H "Authorization: Bearer $VOS_TOKEN"
curl 'http://<vos-host>:22888/v1/audio/speakers/identify' -H "Authorization: Bearer $VOS_TOKEN" -F model=v-voice/speaker-campplus-zh-en -F file=@sample.wav`, "note": "流式接口仅服务声明 streaming capability 的模型，请求体边到边透传、无整体超时，连接期间占用一个并发槽；WebSocket 实时通道为 GET /v1/audio/streaming/ws?model=...（二进制 PCM 上行、partial/final 下行）。声纹注册/删除在 V-Voice 管理面完成，经 V-Router 只做列表、identify 与 verify；详细协议和 Python WebSocket 示例见 docs/v-voice-hotwords.md。"},
		},
		"groups": groups,
		"base_urls": map[string]string{
			"internal":    "http://v-router:8080/v1",
			"lan":         "http://<vos-host>:" + envOr("V_ROUTER_HOST_PORT", "22888") + "/v1",
			"same_origin": gatewayAPIBase() + "/v1",
		},
	})
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func (s *Server) handleHealthz(w http.ResponseWriter, _ *http.Request) {
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte("ok"))
}

// statusRecorder captures the response status for usage logging while
// passing Flush through so SSE streaming keeps working.
type statusRecorder struct {
	http.ResponseWriter
	status   int
	observer usageObserver
}

func (r *statusRecorder) WriteHeader(code int) {
	r.status = code
	r.ResponseWriter.WriteHeader(code)
}

func (r *statusRecorder) Unwrap() http.ResponseWriter { return r.ResponseWriter }

func (r *statusRecorder) Write(p []byte) (int, error) {
	r.observer.Write(p)
	return r.ResponseWriter.Write(p)
}

func (r *statusRecorder) tokenUsage() tokenUsage { return r.observer.usage() }

func (r *statusRecorder) Flush() {
	if f, ok := r.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]any{"error": map[string]string{"message": msg}})
}
