package api

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/ictrektech/v-router/internal/proxy"
	"github.com/ictrektech/v-router/internal/registry"
)

// Model-based calls share one forwarding path, including multipart uploads.
// Resource APIs have provider-owned IDs, so they must pin an upstream explicitly.
func (s *Server) extendedRoutes() []apiRouteSpec {
	routes := []apiRouteSpec{{Method: "GET", Path: "/api/v1/auth/me", Group: "manage", Auth: "manage", Handler: s.handleMe, Summary: "验证当前调用身份", Desc: "校验 VOS OIDC Bearer token 或共享网关 Key，返回 identity、auth_method、permissions、resource_isolation=true 和 registration_scopes。注册服务可选择 public/private；private 只对注册者身份和管理员可见。"}, {Method: "GET", Path: "/api/v1/openapi.json", Group: "manage", Auth: "manage", Handler: s.handleOpenAPI, Summary: "OpenAPI 3.1 契约", Desc: "从运行时路由表自动生成；扩展字段和上游返回体保留，随版本即时更新。"}}
	routes = append(routes, apiRouteSpec{Method: "GET", Path: "/v1/audio/voices", Group: "openai", Auth: "inference", Handler: s.handleAudioVoices, Summary: "语音合成 Voice 列表", Desc: "按 model 查询参数路由到 TTS 上游，替换为原生模型名并透传响应；上游返回的 model、default_voice 和同值 voices[]/data[] 列表原样保留，客户端只能使用返回的 voice id。", Example: `curl 'http://<vos-host>:22888/v1/audio/voices?model=v-voice/kokoro-multilang'`})
	routes = append(routes, apiRouteSpec{Method: "POST", Path: "/v1/messages", Group: "anthropic", Auth: "inference", Handler: s.handleMessages, Summary: "Anthropic Messages", Desc: "Anthropic 原生上游直接转发；chat_completions 上游由 Go 进程内转换文本、图片输入与 function tools，支持 SSE。无状态转换不支持服务端工具、缓存或 thinking；图片和工具能力仍由模型决定。支持 x-api-key 网关身份。"})
	routes = append(routes,
		apiRouteSpec{Method: "POST", Path: "/v1/ml/predictions", Group: "openai", Auth: "inference", Handler: s.handleMLPrediction, Summary: "ML Server 图片推理扩展", Desc: "multipart/form-data：model 与 image 必填。按 ml-server 模型自动适配 CLIP 图片向量、人脸识别、行人 ReID 或 OCR；可选 min_score、max_resolution、score_mode。上游返回保持 Immich ML Server 格式，embedding 字段仍是 JSON 数组字符串。", Example: `curl http://<vos-host>:22888/v1/ml/predictions -F model=ml-server/PP-OCRv5_server -F image=@image.jpg -F min_score=0.5`},
		apiRouteSpec{Method: "POST", Path: "/v1/ml/video/structure", Group: "openai", Auth: "inference", Handler: s.handleMLVideoCreate, Summary: "ML Server 视频结构化任务", Desc: "V-Router 扩展接口。JSON 中 model=ml-server/video-structure，asset_id、video_path 必填，可选 detection 与逐场景 tasks；video_path 必须已存在于 ML Server 可访问的共享存储。", Example: `curl http://<vos-host>:22888/v1/ml/video/structure -H 'Content-Type: application/json' -d '{"model":"ml-server/video-structure","asset_id":"asset-001","video_path":"video/example.mp4"}'`},
		apiRouteSpec{Method: "GET", Path: "/v1/ml/video/structure/tasks/{task_id}", Group: "openai", Auth: "inference", Handler: s.handleMLVideoTask, Summary: "查询 ML Server 视频结构化任务", Desc: "model 查询参数必填。queued/running 可重复查询；succeeded/failed 终态首次读取后即由 ML Server 消费，后续查询同一 task_id 返回 404。", Example: `curl 'http://<vos-host>:22888/v1/ml/video/structure/tasks/<task-id>?model=ml-server/video-structure'`},
	)
	for _, e := range []struct{ path, title, format string }{
		{"/completions", "文本补全", "application/json"},
		{"/responses", "Responses 多模态响应", "application/json"},
		{"/responses/compact", "Responses 上下文压缩", "application/json"},
		{"/images/generations", "图片生成", "application/json"},
		{"/images/edits", "图片编辑", "multipart/form-data"},
		{"/images/variations", "图片变体", "multipart/form-data"},
		{"/videos/generations", "视频生成", "application/json"},
		{"/audio/transcriptions", "语音识别 ASR（支持热词文本、热词文件和增强分数）", "multipart/form-data"},
		{"/audio/translations", "语音翻译", "multipart/form-data"},
		{"/audio/speech", "语音合成 TTS", "application/json"},
		{"/audio/music/generations", "音乐生成（OpenAI 风格扩展）", "application/json"},
		{"/audio/speakers/identify", "声纹识别 1:N（说话人归属）", "multipart/form-data"},
		{"/audio/speakers/verify", "声纹验证 1:1（说话人比对）", "multipart/form-data"},
		{"/moderations", "内容审核", "application/json"},
		{"/rerank", "重排序（兼容扩展，非 OpenAI 标准）", "application/json"},
	} {
		path := e.path
		example := `curl http://<vos-host>:22888/v1` + path + ` -H 'Content-Type: application/json' -d '{"model":"<model>","input":"你好"}'`
		if e.format == "multipart/form-data" {
			example = `curl http://<vos-host>:22888/v1` + path + ` -F model='<model>' -F file=@audio.wav`
		}
		if strings.HasPrefix(path, "/audio/trans") {
			example = `curl http://<vos-host>:22888/v1` + path + ` -F model='<model>' -F file=@audio.wav -F response_format=verbose_json -F 'timestamp_granularities[]=word' -F hotwords='V-Voice,sherpa-onnx' -F hotwords_file=@hotwords.txt -F hotwords_score=1.5`
		}
		if strings.HasPrefix(path, "/images/") {
			example = `curl http://<vos-host>:22888/v1` + path + ` -H 'Content-Type: application/json' -d '{"model":"<model>","prompt":"一座山"}'`
			if e.format == "multipart/form-data" {
				example = `curl http://<vos-host>:22888/v1` + path + ` -F model='<model>' -F image=@image.png -F prompt='一座山'`
			}
		}
		if path == "/completions" {
			example = `curl http://<vos-host>:22888/v1/completions -H 'Content-Type: application/json' -d '{"model":"<model>","prompt":"你好"}'`
		}
		if path == "/rerank" {
			example = `curl http://<vos-host>:22888/v1/rerank -H 'Content-Type: application/json' -d '{"model":"<model>","query":"问题","documents":["文档"]}'`
		}
		if path == "/audio/speakers/identify" {
			example = `curl http://<vos-host>:22888/v1/audio/speakers/identify -F model='<model>' -F file=@sample.wav -F threshold=0.6`
		}
		if path == "/audio/speakers/verify" {
			example = `curl http://<vos-host>:22888/v1/audio/speakers/verify -F model='<model>' -F speaker=sp_1a2b3c4d -F file=@sample.wav`
		}
		if path == "/responses/compact" {
			example = `curl http://<vos-host>:22888/v1/responses/compact -H 'Content-Type: application/json' -d '{"model":"<model>","input":[{"role":"user","content":"你好"}]}'`
		}
		if path == "/images/variations" {
			example = `curl http://<vos-host>:22888/v1/images/variations -F model='<model>' -F image=@image.png`
		}
		if path == "/videos/generations" {
			example = `curl http://<vos-host>:22888/v1/videos/generations -H 'Content-Type: application/json' -d '{"model":"<model>","prompt":"一只小猫在月球上奔跑"}'`
		}
		if path == "/audio/speech" {
			example = `curl http://<vos-host>:22888/v1/audio/speech -H 'Content-Type: application/json' -d '{"model":"<model>","input":"你好","voice":"alloy"}' --output speech.mp3`
		}
		description := "按 model 路由并替换原生模型名；支持 " + e.format + "，上游响应（含 SSE、二进制和错误）原样转发。上传上限 32 MiB；能力与参数取决于上游。指定 X-VRouter-Upstream 时需登录，使用上游原生请求体，可省略 model。"
		if path == "/audio/music/generations" {
			example = `curl http://<vos-host>:22888/v1/audio/music/generations -H 'Content-Type: application/json' -d '{"model":"v-music/stable-audio-3-small-music","prompt":"warm cinematic ambient music","duration":10,"instrumental":true,"response_format":"wav"}' --output music.wav`
			description = "OpenAI 风格扩展（OpenAI 官方当前无音乐生成端点）。V-Router 按 v-music/<模型 key> 自动路由并将 model 还原为原生 key，其余 JSON 字段和 WAV/FLAC 二进制响应原样透传。"
		}
		if strings.HasPrefix(path, "/audio/trans") {
			description += " 热词模型可透传 hotwords 文本、hotwords_file UTF-8 文件、hotwords_set 已保存热词集 ID（可重复，与文本/文件在服务端合并去重）和 hotwords_score；V-Router 不解析或改写热词内容。timestamp_granularities[] 也原样透传；V-Voice 的 verbose_json 可返回 OpenAI 风格 words/segments，具体能力以发现元数据为准。V-Voice 模型还可透传 speaker_model（填 v-voice/ 前缀的声纹模型 ID，V-Router 在转发边界还原为原生 ID）与 speaker_threshold，提供时 verbose_json 附带 speaker 归属块，V-Router 不裁剪。"
		}
		if path == "/responses" {
			description = "原生上游直接转发；Model Hub/仅 chat_completions 上游由 Go 转换文本、图片输入、function tools、结构化输出和 SSE，须 store=false 或省略。转换不支持 previous_response_id、background、托管工具或服务端状态，不支持的字段返回 400。显式上游仍按配置转换，使用原生模型名。"
		}
		if path == "/videos/generations" {
			description += " 上游返回 2xx 且含 id 时记录任务创建者；后续经 /v1/videos/{id} 的查询、取消按资源归属拦截跨账户访问。"
		}
		if path == "/audio/speakers/identify" {
			description = "1:N 说话人归属。multipart 字段：model 必填（v-voice/ 前缀声纹模型 ID）、file 必填、threshold 可选（0..1，默认 0.6）。响应包含 identified（是否超过阈值命中）、speaker（{id,name,score}，未命中为 null）、score、threshold 与 ranked（Top-5）。声纹经 V-Voice 管理面注册（宿主机级共享），V-Router 不代理注册接口。"
		}
		if path == "/audio/speakers/verify" {
			description = "1:1 声纹验证。multipart 字段：model 必填（v-voice/ 前缀声纹模型 ID）、speaker 必填（声纹 id 或名称）、file 必填、threshold 可选（0..1，默认 0.6）。响应 verified、score、threshold、speaker；speaker 不存在由上游返回 404。"
		}
		routes = append(routes, apiRouteSpec{Method: "POST", Path: "/v1" + path, Group: "openai", Auth: "inference", Summary: e.title, Desc: description, Example: example, Handler: func(w http.ResponseWriter, r *http.Request) {
			s.forwardToModel(w, r, path, strings.TrimPrefix(path, "/"))
		}})
	}
	// Query-parameter routed audio endpoints: bodies stream end to end.
	routes = append(routes,
		apiRouteSpec{
			Method: "POST", Path: "/v1/audio/streaming/transcriptions", Group: "openai", Auth: "inference",
			Handler: s.handleStreamingTranscriptions,
			Summary: "流式语音识别（HTTP 分块上传）",
			Desc:    "仅声明 streaming capability 的模型（model 为查询参数，v-voice/<模型 key>）。请求体是 chunked 音频字节流（format=pcm|wav|auto，pcm 默认 16kHz s16le），V-Router 边到边透传、不整体缓冲，也不设置整体超时；hotwords、hotwords_set（可重复）、hotwords_score、response_format、speaker_model（v-voice/ 前缀 ID，转发时还原原生）、speaker_threshold 均经查询参数透传。body 结束后返回最终转写，verbose_json 字段与整句接口一致并附加 streaming:true；流式连接存续期间占用该模型一个并发槽。",
			Example: `curl -N -X POST 'http://<vos-host>:22888/v1/audio/streaming/transcriptions?model=v-voice/zipformer-zh-streaming&format=pcm' --header 'Transfer-Encoding: chunked' --data-binary @audio.pcm`,
		},
		apiRouteSpec{
			Method: "GET", Path: "/v1/audio/streaming/ws", Group: "openai", Auth: "inference",
			Handler: s.handleStreamingWS,
			Summary: "流式语音识别（WebSocket 实时）",
			Desc:    "WebSocket 升级连接（model 为查询参数，format 仅 pcm|wav）。客户端发送 s16le 单声道 PCM 二进制分片，服务端回 {type:partial,text,is_endpoint}，客户端发文本帧 {type:finish} 或断开后回 {type:final,text,tokens,timestamps}；连接参数带 speaker_model 时 final 附带 speaker 归属块。错误按 4404（模型不存在）、4409（未就绪）、4422（参数不匹配）、4500（推理失败）关闭，V-Router 原样透传帧与关闭码。连接存续期间占用该模型一个并发槽。",
		},
		apiRouteSpec{
			Method: "GET", Path: "/v1/audio/speakers", Group: "openai", Auth: "inference",
			Handler: s.handleAudioSpeakers,
			Summary: "声纹库列表",
			Desc:    "按 model 查询参数（v-voice/<声纹模型 key>）路由到声纹模型，返回已注册声纹：speakers[]/data[]（同值）每项含 id、name、dimension、created_at，另含 default_threshold；不返回声纹向量。声纹注册/删除在 V-Voice 管理面完成，不属于本接口。",
			Example: `curl 'http://<vos-host>:22888/v1/audio/speakers?model=v-voice/speaker-campplus-zh-en'`,
		},
	)
	// No arbitrary URL forwarding: only these provider API families are exposed.
	for _, family := range []string{"messages", "skills", "responses", "chat/completions", "files", "uploads", "batches", "images", "audio", "videos", "fine_tuning", "vector_stores", "assistants", "threads", "conversations", "realtime", "containers", "models"} {
		for _, tail := range []string{"", "/{resource_path...}"} {
			for _, method := range []string{"GET", "POST", "DELETE", "PATCH"} {
				path := "/v1/" + family + tail
				if (path == "/v1/models" && method == "GET") || (path == "/v1/chat/completions" && method == "POST") || (path == "/v1/responses" && method == "POST") || (path == "/v1/messages" && method == "POST") {
					continue
				}
				routes = append(routes, apiRouteSpec{Method: method, Path: path, Group: "resource", Auth: "manage", Summary: family + " · 指定上游透传", Desc: "必须携带 X-VRouter-Upstream: <上游 ID> 和管理身份，使用上游原生参数/模型名；创建、查询、取消、下载均固定同一上游。public 注册服务对所有调用者可见；private 注册服务只对注册者身份和管理员可见。资源归属：经 V-Router 创建成功的资源（POST /v1/<family> 返回 2xx 且含 id）会记录创建者，其他用户随后访问该资源返回 404，owner 和管理员不受限；列表接口仍返回上游共享账户的全部资源（共享供应商账户的固有限制），在本功能上线前或绕过 V-Router 创建的资源不受限。保留查询参数、multipart、SSE、二进制及 WebSocket 升级；不支持的操作由上游返回错误。", Handler: s.handlePinned})
			}
		}
	}
	return routes
}

// acquireQueryModel resolves a ?model= qualified model for query-routed
// audio endpoints, rewrites the query to the upstream-native id and sets the
// routing headers. ok=false means a response has already been written.
func (s *Server) acquireQueryModel(w http.ResponseWriter, r *http.Request, pathSuffix string) (u registry.Upstream, requested, nativeID string, release func(), ok bool) {
	requested = strings.TrimSpace(r.URL.Query().Get("model"))
	if requested == "" {
		writeRequestError(w, r, http.StatusBadRequest, "model query parameter is required")
		return registry.Upstream{}, "", "", nil, false
	}
	id := s.requestIdentity(r)
	u, nativeID, release, err := s.reg.AcquireFor(requested, pathSuffix, "", id.ownerKey(), id.Admin)
	if err != nil {
		s.routingError(w, r, err)
		return registry.Upstream{}, "", "", nil, false
	}
	query := r.URL.Query()
	query.Set("model", nativeID)
	r.URL.RawQuery = query.Encode()
	w.Header().Set("X-VRouter-Upstream", u.ID)
	w.Header().Set("X-VRouter-Routing", "least-inflight")
	return u, requested, nativeID, release, true
}

// proxyQueryModel forwards a query-routed audio call, feeds the breaker and
// records one usage row. acquireSuffix selects the model (its declared
// endpoint), proxyPath is the upstream URL suffix; both are identical except
// for voices. The body streams (chunked audio bytes, binary audio or a
// WebSocket upgrade), so the handler blocks until the exchange finishes.
func (s *Server) proxyQueryModel(w http.ResponseWriter, r *http.Request, acquireSuffix, proxyPath string, stream bool) {
	u, requested, nativeID, release, ok := s.acquireQueryModel(w, r, acquireSuffix)
	if !ok {
		return
	}
	defer release()
	// V-Voice's streaming endpoints take secondary model references as query
	// parameters; restore the native id at the forwarding boundary.
	if ref := strings.TrimSpace(r.URL.Query().Get("speaker_model")); ref != "" {
		if native := u.NativeModelRef(ref); native != ref {
			query := r.URL.Query()
			query.Set("speaker_model", native)
			r.URL.RawQuery = query.Encode()
		}
	}
	obs := s.newBreakerObserver(r, u.ID)
	defer obs.done()
	rec := &statusRecorder{ResponseWriter: w, status: http.StatusOK}
	start := time.Now()
	r = r.WithContext(proxy.WithModel(r.Context(), nativeID))
	proxy.To(rec, r, u, proxyPath, obs.observe)
	s.usage.record(usageRecord{
		Time: start.UTC(), User: identityFrom(r).label(), Model: requested, NativeID: nativeID,
		UpstreamID: u.ID, UpstreamName: u.Name, Endpoint: strings.TrimPrefix(proxyPath, "/"),
		Status: rec.status, LatencyMS: time.Since(start).Milliseconds(), Stream: stream,
	})
}

func (s *Server) handleAudioVoices(w http.ResponseWriter, r *http.Request) {
	s.proxyQueryModel(w, r, "/audio/speech", "/audio/voices", false)
}

// handleStreamingTranscriptions proxies chunked audio bytes to a streaming
// ASR model: the request body is relayed edge to edge without buffering and
// no gateway-wide deadline applies (the HTTP server sets none either).
func (s *Server) handleStreamingTranscriptions(w http.ResponseWriter, r *http.Request) {
	s.proxyQueryModel(w, r, "/audio/streaming/transcriptions", "/audio/streaming/transcriptions", true)
}

// handleStreamingWS proxies a WebSocket realtime recognition connection;
// ReverseProxy performs the upgrade and the concurrency slot is held until
// the upgraded connection closes.
func (s *Server) handleStreamingWS(w http.ResponseWriter, r *http.Request) {
	s.proxyQueryModel(w, r, "/audio/streaming/ws", "/audio/streaming/ws", true)
}

func (s *Server) handleAudioSpeakers(w http.ResponseWriter, r *http.Request) {
	s.proxyQueryModel(w, r, "/audio/speakers", "/audio/speakers", false)
}

func (s *Server) handlePinned(w http.ResponseWriter, r *http.Request) {
	id := r.Header.Get("X-VRouter-Upstream")
	if id == "" {
		writeRequestError(w, r, 400, "X-VRouter-Upstream header is required for resource APIs; use a registered upstream ID")
		return
	}
	u, ok := s.reg.Get(id)
	ident := s.requestIdentity(r)
	if !ok || !u.VisibleTo(ident.ownerKey(), ident.Admin) {
		writeRequestError(w, r, 404, "unknown upstream")
		return
	}
	if !u.Healthy {
		writeRequestError(w, r, 503, "upstream is unhealthy")
		return
	}
	if !s.enforceResourceOwner(w, r, u) {
		return
	}
	s.forwardPinned(w, r, u)
}

func (s *Server) forwardPinned(w http.ResponseWriter, r *http.Request, u registry.Upstream) {
	w.Header().Set("X-VRouter-Upstream", u.ID)
	w.Header().Set("X-VRouter-Routing", "pinned")
	// Avoid path normalization escaping the selected provider API root.
	for _, segment := range strings.Split(r.URL.Path, "/") {
		if segment == ".." || segment == "." {
			writeRequestError(w, r, 400, "invalid resource path")
			return
		}
	}
	obs := s.newBreakerObserver(r, u.ID)
	defer obs.done()
	model, stream := "", false
	if isInferenceUsagePath(r.Method, r.URL.Path) {
		body, parsedModel, parsedStream, _, err := readModelBody(w, r)
		if err != nil {
			status := http.StatusBadRequest
			var tooLarge *http.MaxBytesError
			if errors.As(err, &tooLarge) {
				status = http.StatusRequestEntityTooLarge
			}
			writeRequestError(w, r, status, err.Error())
			return
		}
		model, stream = parsedModel, parsedStream
		if r.URL.Path == "/v1/responses" {
			var request map[string]any
			if json.Unmarshal(body, &request) == nil && request["background"] == true {
				r = r.WithContext(proxy.WithBackground(r.Context()))
			}
		}
		r.Body = io.NopCloser(bytes.NewReader(body))
		r.ContentLength = int64(len(body))
	}
	rec := &statusRecorder{ResponseWriter: w, status: 200}
	start := time.Now()
	if model == "" {
		model = r.URL.Query().Get("model")
	}
	r = r.WithContext(proxy.WithModel(r.Context(), model))
	proxy.To(rec, r, u, strings.TrimPrefix(r.URL.Path, "/v1"), obs.observe)
	s.recordCreatedResource(rec, r, u)
	tokens := rec.tokenUsage()
	s.usage.record(usageRecord{
		Time: start.UTC(), User: identityFrom(r).label(), Model: model, NativeID: model,
		UpstreamID: u.ID, UpstreamName: u.Name, Endpoint: r.Method + " " + r.URL.Path,
		Status: rec.status, LatencyMS: time.Since(start).Milliseconds(), Stream: stream,
		UsageReported: tokens.Reported > 0, InputTokens: tokens.Input, OutputTokens: tokens.Output,
		TotalTokens: tokens.Total, CachedTokens: tokens.Cached, ReasoningTokens: tokens.Reasoning,
	})
}

func isInferenceUsagePath(method, path string) bool {
	if method != http.MethodPost {
		return false
	}
	switch path {
	case "/v1/chat/completions", "/v1/completions", "/v1/embeddings", "/v1/responses", "/v1/messages":
		return true
	default:
		return false
	}
}
