package api

import (
	"github.com/ictrektech/v-router/internal/registry"
	"net/http"
	"reflect"
	"strings"
)

// OpenAPI describes the gateway contract, not a frozen copy of provider schemas.
// Provider extension fields and responses intentionally remain open objects.
func (s *Server) handleOpenAPI(w http.ResponseWriter, req *http.Request) {
	paths := map[string]any{}
	for _, r := range s.spec() {
		path := strings.ReplaceAll(r.Path, "...}", "}")
		item, ok := paths[path].(map[string]any)
		if !ok {
			item = map[string]any{}
			paths[path] = item
		}
		op := map[string]any{"summary": r.Summary, "description": r.Desc, "tags": []string{r.Group}, "x-gateway-auth": r.Auth, "responses": map[string]any{"200": map[string]any{"description": "成功；上游响应体、状态码、Content-Type 保留，可能为 JSON、SSE、音频或文件"}, "default": map[string]any{"description": "网关拒绝或上游原始错误"}}}
		params := []any{}
		for _, seg := range strings.Split(path, "/") {
			if strings.HasPrefix(seg, "{") {
				params = append(params, map[string]any{"name": strings.Trim(seg, "{}"), "in": "path", "required": true, "schema": map[string]string{"type": "string"}})
			}
		}
		if r.Auth != "public" {
			sec := []any{map[string]any{"bearerAuth": []string{}}, map[string]any{"apiKeyAuth": []string{}}}
			if r.Auth == "inference" && !s.requiresKey() {
				sec = append(sec, map[string]any{})
			}
			op["security"] = sec
		}
		if strings.HasPrefix(path, "/v1/") && path != "/v1/models" {
			params = append(params, map[string]any{"name": "X-VRouter-Upstream", "in": "header", "required": r.Auth == "manage", "description": "指定上游 ID；严格固定，不自动切换，需身份与原生模型名。不指定时按匹配模型、协议及健康状态，以进行中请求/权重选择，平局在当前模型与接口范围内按权重平滑轮换；全满返回429。模型前缀限定来源，短名跨来源（包括云端）。资源接口必须指定。", "schema": map[string]string{"type": "string"}})
		}
		if path == "/v1/audio/voices" {
			params = append(params, map[string]any{"name": "model", "in": "query", "required": true, "description": "V-Router qualified TTS model ID, e.g. v-voice/kokoro-multilang", "schema": map[string]string{"type": "string"}})
		}
		if path == "/v1/ml/video/structure/tasks/{task_id}" {
			params = append(params, map[string]any{"name": "model", "in": "query", "required": true, "description": "固定为 GET /v1/models 返回的 ml-server/video-structure", "schema": map[string]string{"type": "string"}})
		}
		if r.Auth == "inference" && path != "/v1/models" {
			op["responses"].(map[string]any)["429"] = map[string]any{"description": "候选后端全部达到 max_inflight，Retry-After: 1；不排队、不重试"}
			op["responses"].(map[string]any)["200"].(map[string]any)["headers"] = map[string]any{"X-VRouter-Upstream": map[string]any{"schema": map[string]string{"type": "string"}, "description": "实际命中的上游 ID"}, "X-VRouter-Routing": map[string]any{"schema": map[string]string{"type": "string"}, "description": "least-inflight 或 pinned"}}
		}
		if strings.Contains(path, "audio/trans") {
			responses := op["responses"].(map[string]any)
			responses["404"] = map[string]any{"description": "模型不存在、未发现或不支持该音频接口"}
			responses["409"] = map[string]any{"description": "V-Voice 模型文件尚未 ready"}
			responses["413"] = map[string]any{"description": "请求超过 V-Router 32 MiB 或 V-Voice 音频限制"}
			responses["422"] = map[string]any{"description": "支持热词的模型收到非法 UTF-8、超限热词、越界分数或其他字段校验失败；不支持热词的模型会忽略热词字段"}
		}
		if len(params) > 0 {
			op["parameters"] = params
		}
		if r.Method == "POST" || r.Method == "PUT" || r.Method == "PATCH" {
			schema := map[string]any{"type": "object", "additionalProperties": true}
			if r.Auth == "inference" {
				schema["properties"] = map[string]any{"model": map[string]string{"type": "string", "description": "网关模型全名/短名；指定上游时按上游协议"}, "stream": map[string]string{"type": "boolean"}}
			}
			if path == "/v1/audio/speech" {
				schema = map[string]any{
					"type":                 "object",
					"additionalProperties": true,
					"required":             []string{"model", "input"},
					"properties": map[string]any{
						"model":             map[string]any{"type": "string", "description": "V-Router qualified TTS model ID"},
						"input":             map[string]any{"type": "string", "minLength": 1, "maxLength": 20000},
						"voice":             map[string]any{"type": "string", "default": "default", "description": "ID from GET /v1/audio/voices for the selected model"},
						"response_format":   map[string]any{"type": "string", "enum": []string{"mp3", "opus", "aac", "flac", "wav", "pcm"}, "default": "mp3"},
						"speed":             map[string]any{"type": "number", "minimum": 0.25, "maximum": 4, "default": 1},
						"language":          map[string]any{"type": "string", "default": ""},
						"reference_audio":   map[string]any{"type": "string", "contentEncoding": "base64", "description": "V-Voice extension: reference audio bytes, never a server path; requires voice_cloning capability"},
						"reference_text":    map[string]any{"type": "string", "maxLength": 2000, "description": "V-Voice extension: reference transcript; some cloning models require it"},
						"voice_description": map[string]any{"type": "string", "maxLength": 500, "description": "V-Voice extension: voice design / instruction; requires voice_design or voice_instruction capability"},
						"emotion_text":      map[string]any{"type": "string", "maxLength": 500, "description": "V-Voice extension: IndexTTS emotion instruction; requires emotion_control capability"},
						"emotion_strength":  map[string]any{"type": "number", "minimum": 0, "maximum": 1, "default": 0.6},
					},
				}
			}
			if path == "/v1/audio/music/generations" {
				schema = map[string]any{
					"type": "object", "additionalProperties": true,
					"required": []string{"model", "prompt"},
					"properties": map[string]any{
						"model":           map[string]any{"type": "string", "description": "V-Router qualified music model ID, e.g. v-music/stable-audio-3-small-music"},
						"prompt":          map[string]any{"type": "string", "minLength": 1, "maxLength": 4000},
						"duration":        map[string]any{"type": "number", "minimum": 10, "default": 30, "description": "Seconds; the selected model and V-Music runtime may impose a lower maximum"},
						"negative_prompt": map[string]any{"type": "string", "maxLength": 4000, "default": ""},
						"seed":            map[string]any{"type": []string{"integer", "null"}},
						"bpm":             map[string]any{"type": []string{"integer", "null"}, "minimum": 20, "maximum": 300},
						"key":             map[string]any{"type": "string", "default": ""},
						"time_signature":  map[string]any{"type": "string", "default": ""},
						"instrumental":    map[string]any{"type": "boolean", "default": true},
						"response_format": map[string]any{"type": "string", "enum": []string{"wav", "flac"}, "default": "wav"},
					},
				}
			}
			if path == "/v1/ml/video/structure" {
				schema = map[string]any{"type": "object", "additionalProperties": false, "required": []string{"model", "asset_id", "video_path"}, "properties": map[string]any{
					"model": map[string]any{"type": "string", "const": "ml-server/video-structure"}, "asset_id": map[string]any{"type": "string"}, "video_path": map[string]any{"type": "string"},
					"detection": map[string]any{"type": "object", "properties": map[string]any{"threshold": map[string]any{"type": "number", "minimum": 0}}},
					"tasks":     map[string]any{"type": "object", "description": "逐场景 facial-recognition、clip.visual、person-reid、ocr 子任务；entries 结构与 /v1/ml/predictions 对应能力相同"},
				}}
			}
			if strings.HasPrefix(path, "/api/v1/upstreams") && (r.Method == "POST" && path == "/api/v1/upstreams" || r.Method == "PUT") {
				schema = payloadSchema(upstreamPayload{})
				schema["required"] = []string{"name", "base_url"}
			}
			if path == "/api/v1/registrations" {
				schema = payloadSchema(registrationPayload{})
				schema["required"] = []string{"service_id", "base_url"}
			}
			if strings.HasSuffix(path, "/heartbeat") {
				schema = map[string]any{"type": "object", "properties": map[string]any{"ttl_seconds": map[string]any{"type": "integer", "minimum": 30, "maximum": 3600, "default": 90}}}
			}
			if strings.HasSuffix(path, "/routing") {
				schema = payloadSchema(registry.RoutingPolicy{})
			}
			content := map[string]any{"application/json": map[string]any{"schema": schema}}
			if strings.Contains(path, "audio/trans") || strings.Contains(path, "images/edit") || strings.Contains(path, "images/variations") || path == "/v1/ml/predictions" || r.Auth == "manage" && strings.HasPrefix(path, "/v1/") {
				properties := map[string]any{"model": map[string]string{"type": "string"}, "file": map[string]string{"type": "string", "format": "binary"}, "image": map[string]string{"type": "string", "format": "binary"}}
				multipartSchema := map[string]any{"type": "object", "additionalProperties": true, "properties": properties}
				if strings.Contains(path, "audio/trans") {
					properties["model"] = map[string]any{"type": "string", "description": "V-Router qualified model ID, e.g. v-voice/qwen3-asr-0.6b"}
					properties["file"] = map[string]any{"type": "string", "format": "binary", "description": "Audio upload"}
					properties["language"] = map[string]any{"type": "string", "default": "", "description": "Input language code; empty lets the model decide"}
					properties["response_format"] = map[string]any{"type": "string", "enum": []string{"json", "verbose_json", "text", "srt", "vtt"}, "default": "json"}
					properties["hotwords"] = map[string]any{"type": "string", "description": "逗号或换行分隔的请求级热词；仅支持声明 hotwords capability 的模型"}
					properties["hotwords_set"] = map[string]any{"type": "array", "items": map[string]string{"type": "string"}, "description": "已保存热词集 ID，可重复提交；与 hotwords、hotwords_file 在服务端合并去重"}
					properties["hotwords_file"] = map[string]any{"type": "string", "format": "binary", "description": "UTF-8 文本，每行一个热词"}
					properties["hotwords_score"] = map[string]any{"type": "number", "minimum": 0, "maximum": 10, "description": "热词增强分数；仅支持 score 的模型使用"}
					properties["timestamp_granularities[]"] = map[string]any{"type": "array", "items": map[string]any{"type": "string", "enum": []string{"word", "segment"}}, "description": "可重复提交 word 和/或 segment；verbose_json 返回上游实际具备的时间戳"}
					multipartSchema["required"] = []string{"model", "file"}
				}
				if path == "/v1/ml/predictions" {
					properties = map[string]any{
						"model":          map[string]any{"type": "string", "description": "ml-server/ 前缀模型；可选 CLIP、buffalo_l、osnet_ain_x1_0、PP-OCRv5_server"},
						"image":          map[string]any{"type": "string", "format": "binary"},
						"min_score":      map[string]any{"type": "number", "minimum": 0, "maximum": 1},
						"max_resolution": map[string]any{"type": "integer", "minimum": 1, "default": 736, "description": "仅 OCR"},
						"score_mode":     map[string]any{"type": "string", "enum": []string{"fast", "slow"}, "description": "仅 OCR"},
					}
					multipartSchema["properties"] = properties
					multipartSchema["required"] = []string{"model", "image"}
					delete(content, "application/json")
				}
				content["multipart/form-data"] = map[string]any{"schema": multipartSchema}
				if strings.Contains(path, "audio/trans") {
					delete(content, "application/json")
				}
			}
			op["requestBody"] = map[string]any{"required": false, "content": content}
		}
		item[strings.ToLower(r.Method)] = op
	}
	w.Header().Set("Cache-Control", "no-store")
	writeDocumentation(w, req, map[string]any{"openapi": "3.1.0", "info": map[string]string{"title": "V-Router API", "version": s.version, "description": "模型路由、Responses/Anthropic Messages 到 Chat Completions 的无状态转换，以及显式上游资源代理。转换限制见端点说明；原生资源生命周期由上游决定。"}, "servers": []any{map[string]string{"url": gatewayAPIBase()}}, "paths": paths, "components": map[string]any{"securitySchemes": map[string]any{"bearerAuth": map[string]string{"type": "http", "scheme": "bearer", "description": "VOS OIDC token 或网关 Key"}, "apiKeyAuth": map[string]string{"type": "apiKey", "in": "header", "name": "x-api-key", "description": "Anthropic SDK 可使用网关 Key 或 VOS token；与 Bearer 同时提供时须相同"}}}})
}

// Management request properties follow the actual decoding type, so adding a
// JSON field updates the machine contract without a second field list.
func payloadSchema(v any) map[string]any {
	typ := reflect.TypeOf(v)
	props := map[string]any{}
	for i := 0; i < typ.NumField(); i++ {
		f := typ.Field(i)
		name := strings.Split(f.Tag.Get("json"), ",")[0]
		if name == "" || name == "-" {
			continue
		}
		kind := "string"
		switch f.Type.Kind() {
		case reflect.Bool:
			kind = "boolean"
		case reflect.Int, reflect.Int64:
			kind = "integer"
		case reflect.Slice:
			kind = "array"
		}
		props[name] = map[string]string{"type": kind}
	}
	return map[string]any{"type": "object", "properties": props, "additionalProperties": true}
}
