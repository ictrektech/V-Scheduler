package api

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"strings"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

// listModelsEntry is one row of the OpenAI GET /v1/models response.
type listModelsEntry struct {
	Created int64  `json:"created"` // Unknown provider creation time; required by the OpenAI model schema.
	ID      string `json:"id"`
	Object  string `json:"object"`
	OwnedBy string `json:"owned_by"`
}

func (s *Server) handleListModels(w http.ResponseWriter, r *http.Request) {
	if r.Header.Get("X-VRouter-Upstream") != "" {
		s.requireIdentity(s.handlePinned)(w, r)
		return
	}
	id := s.requestIdentity(r)
	models := s.reg.ListModelsFor(id.ownerKey(), id.Admin)
	entries := make([]listModelsEntry, 0, len(models))
	for _, m := range models {
		id := m.Qualified
		if id == m.ID { // short alias entry
			id = m.ID
		}
		entries = append(entries, listModelsEntry{ID: id, Object: "model", OwnedBy: m.OwnedBy})
	}
	writeJSON(w, http.StatusOK, map[string]any{"object": "list", "data": entries})
}

// modelView is the management-view model entry with upstream metadata.
type modelView struct {
	ID             string   `json:"id"`
	Qualified      string   `json:"qualified"`
	Short          bool     `json:"short"` // true when this row is a short alias
	Kind           string   `json:"kind"`
	Capabilities   []string `json:"capabilities,omitempty"`
	Endpoints      []string `json:"endpoints,omitempty"`
	UpstreamID     string   `json:"upstream_id"`
	UpstreamName   string   `json:"upstream_name"`
	UpstreamKind   string   `json:"upstream_kind"`
	UpstreamSource string   `json:"upstream_source,omitempty"`
	ModelPrefix    string   `json:"model_prefix,omitempty"`
	OwnedBy        string   `json:"owned_by,omitempty"`
}

func (s *Server) handleListModelsRich(w http.ResponseWriter, r *http.Request) {
	id := s.requestIdentity(r)
	owner, admin := id.ownerKey(), id.Admin
	upstreamNames := map[string]registry.Upstream{}
	for _, u := range s.reg.ListFor(owner, admin) {
		upstreamNames[u.ID] = u
	}
	var out []modelView
	for _, m := range s.reg.ListModelsFor(owner, admin) {
		short := m.Qualified == m.ID
		v := modelView{
			ID:           m.ID,
			Qualified:    m.Qualified,
			Short:        short,
			Kind:         m.Kind,
			Capabilities: append([]string(nil), m.Capabilities...),
			Endpoints:    append([]string(nil), m.Endpoints...),
			UpstreamID:   m.Upstream,
			OwnedBy:      m.OwnedBy,
		}
		if u, ok := upstreamNames[m.Upstream]; ok {
			v.UpstreamName = u.Name
			v.UpstreamKind = u.Kind
			v.UpstreamSource = u.Source
			v.ModelPrefix = u.PublicPrefix()
			if u.Source == registry.SourceAPI && v.Kind == registry.ModelKindChat {
				v.Kind = "unknown"
			}
		}
		out = append(out, v)
	}
	for _, endpoint := range videoStructureInterfaces {
		if _, ok := s.reg.Service(endpoint.service); !ok {
			continue
		}
		upstreamName := "视频结构化 · 结构化与描述"
		if endpoint.service == registry.ServiceVideoStructureDetect {
			upstreamName = "视频结构化 · 场景检测"
		}
		qualified := endpoint.method + " " + endpoint.path
		out = append(out, modelView{
			ID: qualified, Qualified: qualified, Kind: "service",
			Capabilities: []string{"video_structure"}, Endpoints: []string{endpoint.path},
			UpstreamID: endpoint.service, UpstreamName: upstreamName,
			UpstreamKind: registry.KindLocal, UpstreamSource: videoStructureSource,
		})
	}
	if out == nil {
		out = []modelView{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"models": out})
}

func (s *Server) handleUsage(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, s.usage.snapshot(normalizePeriod(r.URL.Query().Get("period"))))
}

// inferenceRequest is the minimal shape needed to route a chat or embedding
// call: the model field is rewritten to the upstream-native id before proxy.
type inferenceRequest struct {
	Model  string `json:"model"`
	Stream bool   `json:"stream"`
}

// forwardToModel resolves req.model, rewrites the body with the native id,
// and streams the call to pathSuffix (e.g. /chat/completions) upstream.
func (s *Server) forwardToModel(w http.ResponseWriter, r *http.Request, pathSuffix, endpoint string) {
	if r.Header.Get("X-VRouter-Upstream") != "" {
		s.requireIdentity(func(w http.ResponseWriter, r *http.Request) {
			id := s.requestIdentity(r)
			chosen, _, release, err := s.reg.AcquireFor("", pathSuffix, r.Header.Get("X-VRouter-Upstream"), id.ownerKey(), id.Admin)
			if err != nil {
				s.routingError(w, r, err)
				return
			}
			defer release()
			w.Header().Set("X-VRouter-Upstream", chosen.ID)
			w.Header().Set("X-VRouter-Routing", "pinned")
			if chosen.ChatOnly() && (pathSuffix == "/responses" || pathSuffix == "/messages") {
				s.forwardSelectedModel(w, r, pathSuffix, endpoint, &chosen)
			} else {
				s.forwardPinned(w, r, chosen)
			}
		})(w, r)
		return
	}
	s.forwardSelectedModel(w, r, pathSuffix, endpoint, nil)
}

func (s *Server) forwardSelectedModel(w http.ResponseWriter, r *http.Request, pathSuffix, endpoint string, selected *registry.Upstream) {
	body, model, stream, contentType, err := readModelBody(w, r)
	if err != nil {
		status := http.StatusBadRequest
		var tooLarge *http.MaxBytesError
		if errors.As(err, &tooLarge) {
			status = http.StatusRequestEntityTooLarge
		}
		writeRequestError(w, r, status, err.Error())
		return
	}
	req := inferenceRequest{Model: model, Stream: stream}

	var upstream registry.Upstream
	nativeID := req.Model
	var release func()
	if selected != nil {
		upstream = *selected
	} else {
		id := s.requestIdentity(r)
		upstream, nativeID, release, err = s.reg.AcquireFor(req.Model, pathSuffix, "", id.ownerKey(), id.Admin)
		if err == nil {
			defer release()
			w.Header().Set("X-VRouter-Upstream", upstream.ID)
			w.Header().Set("X-VRouter-Routing", "least-inflight")
		}
	}
	if err != nil {
		s.routingError(w, r, err)
		return
	}

	rewritten := body
	if !(upstream.ChatOnly() && (pathSuffix == "/responses" || pathSuffix == "/messages")) {
		rewritten, contentType, err = replaceModelBody(body, contentType, nativeID, upstream.NativeModelRef)
	}
	if err != nil {
		return
	}
	r.Body = io.NopCloser(bytes.NewReader(rewritten))
	r.ContentLength = int64(len(rewritten))
	r.Header.Set("Content-Type", contentType)

	rec := &statusRecorder{ResponseWriter: w, status: http.StatusOK}
	start := time.Now()
	s.forwardProtocol(rec, r, upstream, pathSuffix, rewritten, nativeID)
	s.recordModelCreatedResource(rec, r, upstream.ID, pathSuffix)
	latency := time.Since(start)
	tokens := rec.tokenUsage()

	s.usage.record(usageRecord{
		Time:            start.UTC(),
		User:            identityFrom(r).label(),
		Model:           req.Model,
		NativeID:        nativeID,
		UpstreamID:      upstream.ID,
		UpstreamName:    upstream.Name,
		Endpoint:        endpoint,
		Status:          rec.status,
		LatencyMS:       latency.Milliseconds(),
		Stream:          req.Stream,
		UsageReported:   tokens.Reported > 0,
		InputTokens:     tokens.Input,
		OutputTokens:    tokens.Output,
		TotalTokens:     tokens.Total,
		CachedTokens:    tokens.Cached,
		ReasoningTokens: tokens.Reasoning,
	})
}

func (s *Server) handleChatCompletions(w http.ResponseWriter, r *http.Request) {
	s.forwardToModel(w, r, "/chat/completions", "chat")
}

func (s *Server) handleEmbeddings(w http.ResponseWriter, r *http.Request) {
	s.forwardToModel(w, r, "/embeddings", "embeddings")
}

// rewriteModel replaces the model field of a JSON request body without
// disturbing any other keys.
func rewriteModel(body []byte, model string) ([]byte, error) {
	var doc map[string]json.RawMessage
	if err := json.Unmarshal(body, &doc); err != nil {
		return nil, err
	}
	encoded, err := json.Marshal(model)
	if err != nil {
		return nil, err
	}
	doc["model"] = encoded
	return json.Marshal(doc)
}

// Bound the entire upload, including files, before forwarding anything.
const maxInferenceBody = 32 << 20

func readModelBody(w http.ResponseWriter, r *http.Request) ([]byte, string, bool, string, error) {
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, maxInferenceBody))
	if err != nil {
		return nil, "", false, "", err
	}
	ct := r.Header.Get("Content-Type")
	media, params, _ := mime.ParseMediaType(ct)
	if media == "multipart/form-data" {
		mr := multipart.NewReader(bytes.NewReader(body), params["boundary"])
		model := ""
		for {
			part, e := mr.NextRawPart()
			if e == io.EOF {
				break
			}
			if e != nil {
				return nil, "", false, "", e
			}
			if part.FormName() == "model" {
				if model != "" {
					return nil, "", false, "", errors.New("duplicate model field")
				}
				v, e := io.ReadAll(part)
				if e != nil {
					return nil, "", false, "", e
				}
				model = string(v)
			}
		}
		if model == "" {
			return nil, "", false, "", errors.New("multipart body requires model; use X-VRouter-Upstream with native fields for provider defaults")
		}
		return body, model, false, ct, nil
	}
	var req inferenceRequest
	if err := json.Unmarshal(body, &req); err != nil || req.Model == "" {
		return nil, "", false, "", errors.New("JSON body requires model; use X-VRouter-Upstream with native fields for provider defaults")
	}
	return body, req.Model, req.Stream, "application/json", nil
}

// replaceModelBody rewrites the model field (and namespaced secondary model
// references such as speaker_model) of a multipart or JSON body without
// disturbing any other fields.
func replaceModelBody(body []byte, ct, model string, modelRef func(string) string) ([]byte, string, error) {
	if modelRef == nil {
		modelRef = func(s string) string { return s }
	}
	if !strings.HasPrefix(ct, "multipart/form-data") {
		b, e := rewriteModel(body, model)
		return b, ct, e
	}
	_, params, err := mime.ParseMediaType(ct)
	if err != nil {
		return nil, "", err
	}
	mr := multipart.NewReader(bytes.NewReader(body), params["boundary"])
	var out bytes.Buffer
	mw := multipart.NewWriter(&out)
	for {
		part, e := mr.NextRawPart()
		if e == io.EOF {
			break
		}
		if e != nil {
			return nil, "", e
		}
		dest, e := mw.CreatePart(part.Header)
		if e != nil {
			return nil, "", e
		}
		var value string
		switch part.FormName() {
		case "model":
			value = model
		case "speaker_model":
			v, e := io.ReadAll(io.LimitReader(part, 4096))
			if e != nil {
				return nil, "", e
			}
			value = modelRef(string(v))
		default:
			if _, e := io.Copy(dest, part); e != nil {
				return nil, "", e
			}
			continue
		}
		if _, e := io.WriteString(dest, value); e != nil {
			return nil, "", e
		}
	}
	err = mw.Close()
	return out.Bytes(), mw.FormDataContentType(), err
}
