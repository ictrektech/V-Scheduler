package api

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/ictrektech/v-router/internal/proxy"
	"github.com/ictrektech/v-router/internal/registry"
)

type mlEmbeddingRequest struct {
	Model string          `json:"model"`
	Input json.RawMessage `json:"input"`
}

func forwardMLServerEmbedding(w http.ResponseWriter, r *http.Request, u registry.Upstream, path string, body []byte, nativeID string, observe func(error)) {
	if path != "/embeddings" {
		writeProtocolError(w, strings.TrimPrefix(path, "/"), http.StatusBadRequest, "ml_server only supports embeddings on this OpenAI endpoint")
		return
	}
	var in mlEmbeddingRequest
	if json.Unmarshal(body, &in) != nil {
		writeProtocolError(w, "embeddings", http.StatusBadRequest, "invalid JSON request")
		return
	}
	inputs, err := embeddingInputs(in.Input)
	if err != nil {
		writeProtocolError(w, "embeddings", http.StatusBadRequest, err.Error())
		return
	}
	data := make([]map[string]any, 0, len(inputs))
	for i, text := range inputs {
		vector, err := callMLTextEmbedding(r, u, nativeID, text, observe)
		if err != nil {
			if status := proxy.AdmissionStatus(err); status != 0 {
				writeProtocolError(w, "embeddings", status, err.Error())
				return
			}
			writeProtocolError(w, "embeddings", http.StatusBadGateway, err.Error())
			return
		}
		data = append(data, map[string]any{"object": "embedding", "embedding": vector, "index": i})
	}
	// ML Server does not report tokenizer usage. Omitting usage is more honest
	// than returning fabricated zero token counts.
	writeJSON(w, http.StatusOK, map[string]any{"object": "list", "data": data, "model": in.Model})
}

func embeddingInputs(raw json.RawMessage) ([]string, error) {
	var one string
	if json.Unmarshal(raw, &one) == nil {
		if one == "" {
			return nil, fmt.Errorf("input must not be empty")
		}
		return []string{one}, nil
	}
	var many []string
	if json.Unmarshal(raw, &many) != nil || len(many) == 0 {
		return nil, fmt.Errorf("input must be a string or a non-empty string array")
	}
	return many, nil
}

func callMLTextEmbedding(r *http.Request, u registry.Upstream, model, text string, observe func(error)) ([]float64, error) {
	r = r.WithContext(proxy.WithModel(proxy.WithUpstream(r.Context(), u), model))
	entries := map[string]any{}
	field := "qwen3-embedding"
	if strings.Contains(strings.ToLower(model), "siglip") || strings.HasPrefix(model, "ViT-") {
		field = "clip"
		entries[field] = map[string]any{"textual": map[string]any{"modelName": model}}
	} else {
		entries[field] = map[string]any{"modelName": model}
	}
	encoded, _ := json.Marshal(entries)
	var buf bytes.Buffer
	mw := multipart.NewWriter(&buf)
	_ = mw.WriteField("entries", string(encoded))
	_ = mw.WriteField("text", text)
	_ = mw.Close()
	req, err := http.NewRequestWithContext(r.Context(), http.MethodPost, strings.TrimRight(u.BaseURL, "/")+"/predict", &buf)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", mw.FormDataContentType())
	resp, err := bridgeClient.Do(req)
	if err != nil {
		observe(err)
		if proxy.AdmissionStatus(err) != 0 {
			return nil, err
		}
		return nil, fmt.Errorf("upstream connection failed")
	}
	defer resp.Body.Close()
	result, err := io.ReadAll(io.LimitReader(resp.Body, maxInferenceBody+1))
	if err != nil {
		return nil, fmt.Errorf("upstream read failed")
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("upstream status %d: %s", resp.StatusCode, strings.TrimSpace(string(result)))
	}
	var out map[string]json.RawMessage
	if json.Unmarshal(result, &out) != nil {
		return nil, fmt.Errorf("invalid upstream response")
	}
	var serialized string
	if json.Unmarshal(out[field], &serialized) != nil {
		return nil, fmt.Errorf("upstream embedding missing")
	}
	var vector []float64
	if json.Unmarshal([]byte(serialized), &vector) != nil {
		return nil, fmt.Errorf("invalid upstream embedding")
	}
	return vector, nil
}

func (s *Server) handleMLPrediction(w http.ResponseWriter, r *http.Request) {
	r.Body = http.MaxBytesReader(w, r.Body, maxInferenceBody)
	if err := r.ParseMultipartForm(maxInferenceBody); err != nil {
		writeRequestError(w, r, http.StatusBadRequest, "multipart form with model and image is required")
		return
	}
	model := strings.TrimSpace(r.FormValue("model"))
	file, header, err := r.FormFile("image")
	if err != nil || model == "" {
		writeRequestError(w, r, http.StatusBadRequest, "model and image are required")
		return
	}
	defer file.Close()
	image, err := io.ReadAll(file)
	if err != nil {
		writeRequestError(w, r, http.StatusBadRequest, "cannot read image")
		return
	}
	id := s.requestIdentity(r)
	u, native, release, err := s.reg.AcquireFor(model, "/ml/predictions", "", id.ownerKey(), id.Admin)
	if err != nil {
		s.routingError(w, r, err)
		return
	}
	defer release()
	entries, err := mlImageEntries(native, r)
	if err != nil {
		writeRequestError(w, r, http.StatusBadRequest, err.Error())
		return
	}
	encoded, _ := json.Marshal(entries)
	var buf bytes.Buffer
	mw := multipart.NewWriter(&buf)
	_ = mw.WriteField("entries", string(encoded))
	part, _ := mw.CreateFormFile("image", header.Filename)
	_, _ = part.Write(image)
	_ = mw.Close()
	req, _ := http.NewRequestWithContext(r.Context(), http.MethodPost, strings.TrimRight(u.BaseURL, "/")+"/predict", &buf)
	req.Header.Set("Content-Type", mw.FormDataContentType())
	s.forwardMLRequest(w, r, u, model, native, "ml/predictions", req)
}

func mlImageEntries(model string, r *http.Request) (map[string]any, error) {
	opt := map[string]any{}
	if v := strings.TrimSpace(r.FormValue("min_score")); v != "" {
		n, err := strconv.ParseFloat(v, 64)
		if err != nil || n < 0 || n > 1 {
			return nil, fmt.Errorf("min_score must be between 0 and 1")
		}
		opt["minScore"] = n
	}
	switch {
	case strings.Contains(strings.ToLower(model), "siglip") || strings.HasPrefix(model, "ViT-"):
		return map[string]any{"clip": map[string]any{"visual": map[string]any{"modelName": model}}}, nil
	case strings.HasPrefix(model, "buffalo_") || model == "antelopev2":
		detection := map[string]any{"modelName": model}
		if len(opt) > 0 {
			detection["options"] = opt
		}
		return map[string]any{"facial-recognition": map[string]any{"detection": detection, "recognition": map[string]any{"modelName": model}}}, nil
	case strings.HasPrefix(model, "osnet_"):
		return map[string]any{"person-reid": map[string]any{"modelName": model}}, nil
	case strings.Contains(model, "OCR"):
		detection := map[string]any{"modelName": model}
		recognition := map[string]any{"modelName": model}
		detectionOptions := map[string]any{}
		recognitionOptions := map[string]any{}
		if v, ok := opt["minScore"]; ok {
			detectionOptions["minScore"] = v
			recognitionOptions["minScore"] = v
		}
		if v := strings.TrimSpace(r.FormValue("max_resolution")); v != "" {
			n, err := strconv.Atoi(v)
			if err != nil || n <= 0 {
				return nil, fmt.Errorf("max_resolution must be a positive integer")
			}
			detectionOptions["maxResolution"] = n
		}
		if v := strings.TrimSpace(r.FormValue("score_mode")); v != "" {
			if v != "fast" && v != "slow" {
				return nil, fmt.Errorf("score_mode must be fast or slow")
			}
			detectionOptions["scoreMode"] = v
		}
		if len(detectionOptions) > 0 {
			detection["options"] = detectionOptions
		}
		if len(recognitionOptions) > 0 {
			recognition["options"] = recognitionOptions
		}
		return map[string]any{"ocr": map[string]any{"detection": detection, "recognition": recognition}}, nil
	default:
		return nil, fmt.Errorf("selected model does not support image prediction")
	}
}

func (s *Server) handleMLVideoCreate(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(io.LimitReader(r.Body, maxInferenceBody+1))
	if err != nil || len(body) > maxInferenceBody {
		writeRequestError(w, r, http.StatusBadRequest, "invalid request body")
		return
	}
	var payload map[string]any
	if json.Unmarshal(body, &payload) != nil {
		writeRequestError(w, r, http.StatusBadRequest, "invalid JSON request")
		return
	}
	model, _ := payload["model"].(string)
	model = strings.TrimSpace(model)
	if model == "" {
		writeRequestError(w, r, http.StatusBadRequest, "model is required")
		return
	}
	delete(payload, "model")
	id := s.requestIdentity(r)
	u, native, release, err := s.reg.AcquireFor(model, "/ml/video/structure", "", id.ownerKey(), id.Admin)
	if err != nil {
		s.routingError(w, r, err)
		return
	}
	defer release()
	encoded, _ := json.Marshal(payload)
	req, _ := http.NewRequestWithContext(r.Context(), http.MethodPost, strings.TrimRight(u.BaseURL, "/")+"/api/v1/video/structure", bytes.NewReader(encoded))
	req.Header.Set("Content-Type", "application/json")
	s.forwardMLRequest(w, r, u, model, native, "ml/video/structure", req)
}

func (s *Server) handleMLVideoTask(w http.ResponseWriter, r *http.Request) {
	model := strings.TrimSpace(r.URL.Query().Get("model"))
	if model == "" {
		writeRequestError(w, r, http.StatusBadRequest, "model query parameter is required")
		return
	}
	taskID := strings.TrimSpace(r.PathValue("task_id"))
	if taskID == "" || strings.Contains(taskID, "/") {
		writeRequestError(w, r, http.StatusBadRequest, "invalid task_id")
		return
	}
	id := s.requestIdentity(r)
	u, native, release, err := s.reg.AcquireFor(model, "/ml/video/structure/tasks", "", id.ownerKey(), id.Admin)
	if err != nil {
		s.routingError(w, r, err)
		return
	}
	defer release()
	req, _ := http.NewRequestWithContext(r.Context(), http.MethodGet, strings.TrimRight(u.BaseURL, "/")+"/api/v1/video/structure/tasks/"+taskID, nil)
	s.forwardMLRequest(w, r, u, model, native, "ml/video/structure/tasks", req)
}

func (s *Server) forwardMLRequest(w http.ResponseWriter, original *http.Request, u registry.Upstream, model, native, endpoint string, req *http.Request) {
	req = req.WithContext(proxy.WithModel(proxy.WithUpstream(req.Context(), u), native))
	obs := s.newBreakerObserver(original, u.ID)
	defer obs.done()
	start := time.Now()
	resp, err := bridgeClient.Do(req)
	if err != nil {
		obs.observe(err)
		if status := proxy.AdmissionStatus(err); status != 0 {
			writeRequestError(w, original, status, err.Error())
			return
		}
		writeRequestError(w, original, http.StatusBadGateway, "upstream connection failed")
		return
	}
	defer resp.Body.Close()
	for _, h := range []string{"Content-Type", "Retry-After", "X-Request-Id"} {
		if v := resp.Header.Get(h); v != "" {
			w.Header().Set(h, v)
		}
	}
	w.Header().Set("X-VRouter-Upstream", u.ID)
	w.WriteHeader(resp.StatusCode)
	_, _ = io.Copy(w, io.LimitReader(resp.Body, maxInferenceBody+1))
	s.usage.record(usageRecord{Time: start.UTC(), User: identityFrom(original).label(), Model: model, NativeID: native, UpstreamID: u.ID, UpstreamName: u.Name, Endpoint: endpoint, Status: resp.StatusCode, LatencyMS: time.Since(start).Milliseconds()})
}
