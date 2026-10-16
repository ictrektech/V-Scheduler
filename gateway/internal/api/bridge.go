package api

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"github.com/ictrektech/v-router/internal/httpcontract"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"net/textproto"
	"net/url"
	"strings"

	"github.com/ictrektech/v-router/internal/proxy"
	"github.com/ictrektech/v-router/internal/registry"
)

var bridgeClient = &http.Client{Transport: proxy.NewScheduledTransport(proxy.Transport)}

// Select the protocol before issuing any generation. Native calls retain the
// existing reverse-proxy path; converted calls reuse a process-wide pool.
// Every branch feeds transport-level outcomes into the upstream breaker.
func (s *Server) forwardProtocol(w http.ResponseWriter, r *http.Request, u registry.Upstream, path string, body []byte, nativeID string) {
	r = r.WithContext(proxy.WithModel(proxy.WithUpstream(r.Context(), u), nativeID))
	if path == "/responses" {
		var request map[string]any
		if json.Unmarshal(body, &request) == nil && request["background"] == true {
			r = r.WithContext(proxy.WithBackground(r.Context()))
		}
	}
	obs := s.newBreakerObserver(r, u.ID)
	defer obs.done()
	if u.Protocol == registry.ProtocolOfflineASR {
		forwardOfflineASR(w, r, u, path, body, obs.observe)
		return
	}
	if u.Protocol == registry.ProtocolImageStudio {
		forwardImageStudio(w, r, u, path, body, nativeID, obs.observe)
		return
	}
	if u.Protocol == registry.ProtocolAsyncTask {
		forwardAsyncTask(w, r, u, path, body, nativeID, obs.observe)
		return
	}
	if u.Protocol == registry.ProtocolMLServer {
		forwardMLServerEmbedding(w, r, u, path, body, nativeID, obs.observe)
		return
	}
	if (path == "/responses" || path == "/messages") && u.ChatOnly() {
		protocol := strings.TrimPrefix(path, "/")
		converted, err := convertRequest(body, protocol)
		if err != nil {
			writeProtocolError(w, protocol, 400, err.Error())
			return
		}
		converted["model"] = nativeID
		data, err := json.Marshal(converted)
		if err != nil {
			writeProtocolError(w, protocol, 400, "invalid request")
			return
		}
		req, err := http.NewRequestWithContext(r.Context(), "POST", strings.TrimRight(u.BaseURL, "/")+"/chat/completions", bytes.NewReader(data))
		if err != nil {
			writeProtocolError(w, protocol, 502, "invalid upstream URL")
			return
		}
		req.Header.Set("Content-Type", "application/json")
		if u.APIKey != "" {
			req.Header.Set("Authorization", "Bearer "+u.APIKey)
		}
		resp, err := bridgeClient.Do(req)
		if err != nil {
			if status := proxy.AdmissionStatus(err); status != 0 {
				obs.observe(err)
				writeTransportError(w, protocol, status, err)
				return
			}
			obs.observe(err)
			if r.Context().Err() == nil {
				writeProtocolError(w, protocol, 502, "upstream connection failed")
			}
			return
		}
		defer resp.Body.Close()
		for _, key := range []string{"Retry-After", "X-Request-Id", "Request-Id"} {
			if value := resp.Header.Get(key); value != "" {
				w.Header().Set(key, value)
			}
		}
		if resp.StatusCode < 200 || resp.StatusCode >= 300 {
			writeProtocolError(w, protocol, resp.StatusCode, "upstream request failed")
			return
		}
		model, _ := converted["model"].(string)
		if converted["stream"] == true {
			streamChat(w, r, resp.Body, protocol, model, converted)
			return
		}
		var result chatResult
		decoder := json.NewDecoder(io.LimitReader(resp.Body, maxInferenceBody+1))
		if decoder.Decode(&result) != nil || len(result.Choices) != 1 {
			writeProtocolError(w, protocol, 502, "invalid upstream Chat Completions response")
			return
		}
		state := newOutputState(protocol, model, converted)
		if err := state.complete(result); err != nil {
			writeProtocolError(w, protocol, 502, err.Error())
			return
		}
		writeJSON(w, 200, state.result())
		return
	}
	if path == "/messages" && u.Protocol != registry.ProtocolAnthropic {
		writeProtocolError(w, "messages", 400, "select an anthropic or chat_completions upstream")
		return
	}
	proxy.To(w, r, u, path, obs.observe)
}

func forwardOfflineASR(w http.ResponseWriter, r *http.Request, u registry.Upstream, path string, body []byte, observe func(error)) {
	if path != "/audio/transcriptions" {
		writeProtocolError(w, strings.TrimPrefix(path, "/"), http.StatusBadRequest, "offline_asr only supports audio/transcriptions")
		return
	}
	ct := r.Header.Get("Content-Type")
	media, params, err := mime.ParseMediaType(ct)
	if err != nil || media != "multipart/form-data" || params["boundary"] == "" {
		writeProtocolError(w, "audio/transcriptions", http.StatusBadRequest, "offline_asr requires multipart/form-data with file")
		return
	}
	audio, language, err := extractAudioFile(body, params["boundary"])
	if err != nil {
		writeProtocolError(w, "audio/transcriptions", http.StatusBadRequest, err.Error())
		return
	}
	target, err := offlineASRURL(u.BaseURL, language)
	if err != nil {
		writeProtocolError(w, "audio/transcriptions", http.StatusBadGateway, "invalid upstream URL")
		return
	}
	req, err := http.NewRequestWithContext(r.Context(), http.MethodPost, target, bytes.NewReader(audio))
	if err != nil {
		writeProtocolError(w, "audio/transcriptions", http.StatusBadGateway, "invalid upstream URL")
		return
	}
	req.Header.Set("Content-Type", "application/octet-stream")
	if u.APIKey != "" {
		req.Header.Set("Authorization", "Bearer "+u.APIKey)
	}
	resp, err := bridgeClient.Do(req)
	if err != nil {
		observe(err)
		if status := proxy.AdmissionStatus(err); status != 0 {
			writeProtocolError(w, strings.TrimPrefix(path, "/"), status, err.Error())
			return
		}
		if r.Context().Err() == nil {
			writeProtocolError(w, "audio/transcriptions", http.StatusBadGateway, "upstream connection failed")
		}
		return
	}
	defer resp.Body.Close()
	for _, key := range []string{"Retry-After", "X-Request-Id", "Request-Id"} {
		if value := resp.Header.Get(key); value != "" {
			w.Header().Set(key, value)
		}
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxInferenceBody+1))
	if err != nil {
		writeProtocolError(w, "audio/transcriptions", http.StatusBadGateway, "upstream read failed")
		return
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		writeProtocolError(w, "audio/transcriptions", resp.StatusCode, string(data))
		return
	}
	out := normalizeOfflineASRResponse(data)
	writeJSON(w, http.StatusOK, out)
}

func extractAudioFile(body []byte, boundary string) ([]byte, string, error) {
	mr := multipart.NewReader(bytes.NewReader(body), boundary)
	var audio []byte
	language := ""
	for {
		part, err := mr.NextRawPart()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, "", err
		}
		name := part.FormName()
		switch name {
		case "file", "audio":
			if audio != nil {
				return nil, "", errors.New("duplicate file field")
			}
			audio, err = io.ReadAll(part)
			if err != nil {
				return nil, "", err
			}
		case "language", "lang":
			v, err := io.ReadAll(io.LimitReader(part, 128))
			if err != nil {
				return nil, "", err
			}
			language = strings.TrimSpace(string(v))
		}
	}
	if len(audio) == 0 {
		return nil, "", errors.New("multipart body requires file")
	}
	return audio, language, nil
}

func offlineASRURL(base, language string) (string, error) {
	u, err := url.Parse(strings.TrimRight(base, "/"))
	if err != nil || u.Scheme == "" || u.Host == "" {
		return "", errors.New("invalid upstream URL")
	}
	u.Path = strings.TrimRight(u.Path, "/") + "/transcribe"
	q := u.Query()
	if strings.TrimSpace(language) != "" {
		q.Set("lang", strings.TrimSpace(language))
	}
	u.RawQuery = q.Encode()
	return u.String(), nil
}

func normalizeOfflineASRResponse(data []byte) jsonObject {
	var raw map[string]any
	if err := json.Unmarshal(data, &raw); err != nil {
		return jsonObject{"text": string(data)}
	}
	if d, ok := raw["data"].(map[string]any); ok {
		text, _ := d["full_text"].(string)
		out := jsonObject{"text": text}
		for _, key := range []string{"segments", "speakers", "stats"} {
			if value, ok := d[key]; ok {
				out[key] = value
			}
		}
		return out
	}
	if text, ok := raw["text"].(string); ok {
		return jsonObject{"text": text}
	}
	return jsonObject{"text": "", "upstream": raw}
}

func writeProtocolError(w http.ResponseWriter, protocol string, status int, message string) {
	httpcontract.WriteError(w, status, message, "", "", protocol)
}

func writeTransportError(w http.ResponseWriter, protocol string, status int, err error) {
	code, requestID := proxy.AdmissionDetails(err)
	httpcontract.WriteError(w, status, err.Error(), code, requestID, protocol)
}
func writeRequestFailure(w http.ResponseWriter, r *http.Request, status int, err error) {
	protocol := ""
	if strings.HasPrefix(r.URL.Path, "/v1/messages") {
		protocol = "messages"
	}
	writeTransportError(w, protocol, status, err)
}

func writeRequestError(w http.ResponseWriter, r *http.Request, status int, message string) {
	if strings.HasPrefix(r.URL.Path, "/v1/messages") {
		writeProtocolError(w, "messages", status, message)
		return
	}
	writeError(w, status, message)
}

func (s *Server) handleMessages(w http.ResponseWriter, r *http.Request) {
	s.forwardToModel(w, r, "/messages", "messages")
}

func forwardImageStudio(w http.ResponseWriter, r *http.Request, u registry.Upstream, path string, body []byte, nativeID string, observe func(error)) {
	if path != "/images/generations" && path != "/images/edits" && path != "/videos/generations" {
		writeProtocolError(w, strings.TrimPrefix(path, "/"), http.StatusBadRequest, "image_studio only supports images/generations, images/edits and videos/generations")
		return
	}
	payload, err := imageStudioPayload(r, u, path, body, nativeID)
	if err != nil {
		writeProtocolError(w, strings.TrimPrefix(path, "/"), http.StatusBadRequest, err.Error())
		return
	}
	data, err := json.Marshal(payload)
	if err != nil {
		writeProtocolError(w, strings.TrimPrefix(path, "/"), http.StatusBadRequest, "invalid request")
		return
	}
	req, err := http.NewRequestWithContext(r.Context(), http.MethodPost, imageStudioAPIBase(u.BaseURL)+"/generate", bytes.NewReader(data))
	if err != nil {
		writeProtocolError(w, strings.TrimPrefix(path, "/"), http.StatusBadGateway, "invalid upstream URL")
		return
	}
	req.Header.Set("Content-Type", "application/json")
	copyImageStudioAuth(req, r, u)
	resp, err := bridgeClient.Do(req)
	if err != nil {
		observe(err)
		if status := proxy.AdmissionStatus(err); status != 0 {
			writeProtocolError(w, strings.TrimPrefix(path, "/"), status, err.Error())
			return
		}
		if r.Context().Err() == nil {
			writeProtocolError(w, strings.TrimPrefix(path, "/"), http.StatusBadGateway, "upstream connection failed")
		}
		return
	}
	defer resp.Body.Close()
	data, err = io.ReadAll(io.LimitReader(resp.Body, maxInferenceBody+1))
	if err != nil {
		writeProtocolError(w, strings.TrimPrefix(path, "/"), http.StatusBadGateway, "upstream read failed")
		return
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		writeProtocolError(w, strings.TrimPrefix(path, "/"), resp.StatusCode, string(data))
		return
	}
	var job map[string]any
	if err := json.Unmarshal(data, &job); err != nil {
		writeProtocolError(w, strings.TrimPrefix(path, "/"), http.StatusBadGateway, "invalid Image Studio response")
		return
	}
	writeJSON(w, http.StatusAccepted, map[string]any{
		"id":               job["job_id"],
		"object":           "image_studio.generation.job",
		"status":           job["status"],
		"model":            nativeID,
		"image_studio_job": job,
	})
}

func imageStudioPayload(r *http.Request, u registry.Upstream, path string, body []byte, nativeID string) (map[string]any, error) {
	if strings.HasPrefix(r.Header.Get("Content-Type"), "multipart/") {
		return imageStudioMultipartPayload(r, u, path, body, nativeID)
	}
	var in map[string]any
	if err := json.Unmarshal(body, &in); err != nil {
		return nil, err
	}
	out := imageStudioBasePayload(nativeID)
	copyStringField(out, in, "prompt", "prompt")
	copyStringField(out, in, "negative_prompt", "negative_prompt")
	copyStringField(out, in, "model_variant", "model_variant")
	copyNumberField(out, in, "steps", "steps")
	copyNumberField(out, in, "cfg_scale", "cfg_scale")
	copyNumberField(out, in, "strength", "strength")
	copyNumberField(out, in, "seed", "seed")
	copyNumberField(out, in, "video_frames", "video_frames")
	copyNumberField(out, in, "fps", "fps")
	copySize(out, in)
	if n, ok := in["n"].(float64); ok && path != "/videos/generations" {
		out["batch_count"] = int(n)
	}
	if path == "/videos/generations" {
		out["batch_count"] = 1
	}
	if strings.TrimSpace(fmt.Sprint(out["prompt"])) == "" {
		return nil, errors.New("prompt is required")
	}
	return out, nil
}

func imageStudioMultipartPayload(r *http.Request, u registry.Upstream, path string, body []byte, nativeID string) (map[string]any, error) {
	_, params, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
	if err != nil || params["boundary"] == "" {
		return nil, errors.New("multipart boundary is required")
	}
	mr := multipart.NewReader(bytes.NewReader(body), params["boundary"])
	out := imageStudioBasePayload(nativeID)
	var image []byte
	filename := "input.png"
	contentType := "application/octet-stream"
	for {
		part, err := mr.NextRawPart()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, err
		}
		name := part.FormName()
		switch name {
		case "image", "file":
			if image != nil {
				return nil, errors.New("duplicate image field")
			}
			image, err = io.ReadAll(part)
			if err != nil {
				return nil, err
			}
			if part.FileName() != "" {
				filename = part.FileName()
			}
			if ct := part.Header.Get("Content-Type"); ct != "" {
				contentType = ct
			}
		case "prompt", "negative_prompt", "model_variant", "sampler":
			v, err := io.ReadAll(part)
			if err != nil {
				return nil, err
			}
			out[name] = string(v)
		case "width", "height", "steps", "seed", "video_frames", "fps", "batch_count":
			v, err := io.ReadAll(part)
			if err != nil {
				return nil, err
			}
			if n, ok := parseJSONNumber(string(v)); ok {
				out[name] = n
			}
		case "cfg_scale", "strength":
			v, err := io.ReadAll(part)
			if err != nil {
				return nil, err
			}
			if n, ok := parseJSONFloat(string(v)); ok {
				out[name] = n
			}
		}
	}
	if path == "/images/edits" || image != nil {
		if image == nil {
			return nil, errors.New("image edit requires image file")
		}
		inputID, err := uploadImageStudioInput(r, u, filename, contentType, image)
		if err != nil {
			return nil, err
		}
		out["init_input_id"] = inputID
	}
	if strings.TrimSpace(fmt.Sprint(out["prompt"])) == "" {
		return nil, errors.New("prompt is required")
	}
	return out, nil
}

func imageStudioBasePayload(nativeID string) map[string]any {
	modelKey, variant := splitImageStudioModel(nativeID)
	out := map[string]any{"model_key": modelKey}
	if variant != "" {
		out["model_variant"] = variant
	}
	return out
}

func splitImageStudioModel(nativeID string) (string, string) {
	parts := strings.SplitN(nativeID, "@", 2)
	if len(parts) == 2 {
		return parts[0], parts[1]
	}
	return nativeID, ""
}

func uploadImageStudioInput(r *http.Request, u registry.Upstream, filename, contentType string, image []byte) (string, error) {
	var buf bytes.Buffer
	mw := multipart.NewWriter(&buf)
	h := make(textproto.MIMEHeader)
	h.Set("Content-Disposition", fmt.Sprintf(`form-data; name="file"; filename="%s"`, strings.ReplaceAll(filename, `"`, "")))
	h.Set("Content-Type", contentType)
	part, err := mw.CreatePart(h)
	if err != nil {
		return "", err
	}
	if _, err := part.Write(image); err != nil {
		return "", err
	}
	if err := mw.Close(); err != nil {
		return "", err
	}
	req, err := http.NewRequestWithContext(r.Context(), http.MethodPost, imageStudioAPIBase(u.BaseURL)+"/inputs", &buf)
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", mw.FormDataContentType())
	copyImageStudioAuth(req, r, u)
	resp, err := bridgeClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return "", err
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return "", fmt.Errorf("input upload: status %d: %s", resp.StatusCode, strings.TrimSpace(string(data)))
	}
	var out struct {
		InputID string `json:"input_id"`
	}
	if err := json.Unmarshal(data, &out); err != nil || out.InputID == "" {
		return "", errors.New("invalid input upload response")
	}
	return out.InputID, nil
}

func imageStudioAPIBase(base string) string {
	base = strings.TrimRight(strings.TrimSpace(base), "/")
	if strings.HasSuffix(base, "/api/v1") {
		return base
	}
	return base + "/api/v1"
}

func copyImageStudioAuth(dst *http.Request, src *http.Request, u registry.Upstream) {
	if auth := src.Header.Get("Authorization"); auth != "" {
		dst.Header.Set("Authorization", auth)
	} else if u.APIKey != "" {
		dst.Header.Set("Authorization", "Bearer "+u.APIKey)
	}
}

func copyStringField(out, in map[string]any, dst, src string) {
	if v, ok := in[src].(string); ok && strings.TrimSpace(v) != "" {
		out[dst] = v
	}
}

func copyNumberField(out, in map[string]any, dst, src string) {
	if v, ok := in[src].(float64); ok {
		out[dst] = v
	}
}

func copySize(out, in map[string]any) {
	if size, ok := in["size"].(string); ok {
		parts := strings.Split(strings.ToLower(size), "x")
		if len(parts) == 2 {
			if w, ok := parseJSONNumber(parts[0]); ok {
				out["width"] = w
			}
			if h, ok := parseJSONNumber(parts[1]); ok {
				out["height"] = h
			}
		}
	}
	copyNumberField(out, in, "width", "width")
	copyNumberField(out, in, "height", "height")
}

func parseJSONNumber(s string) (int, bool) {
	var n int
	if _, err := fmt.Sscanf(strings.TrimSpace(s), "%d", &n); err == nil {
		return n, true
	}
	return 0, false
}

func parseJSONFloat(s string) (float64, bool) {
	var n float64
	if _, err := fmt.Sscanf(strings.TrimSpace(s), "%f", &n); err == nil {
		return n, true
	}
	return 0, false
}
