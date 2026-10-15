package api

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

// upstreamPayload is the API shape for creating and updating API-created upstreams.
type upstreamPayload struct {
	Name              string                    `json:"name"`
	Kind              string                    `json:"kind"`
	BaseURL           string                    `json:"base_url"`
	APIKey            string                    `json:"api_key"`
	Protocol          string                    `json:"protocol"`
	Visibility        string                    `json:"visibility"`
	TestModel         string                    `json:"test_model"`
	AllowProbeFailure bool                      `json:"allow_probe_failure"`
	Model             string                    `json:"model"`
	AsyncTask         *registry.AsyncTaskConfig `json:"async_task"`
}

type upstreamProbeView struct {
	OK          bool     `json:"ok"`
	BaseURL     string   `json:"base_url"`
	ModelPrefix string   `json:"model_prefix"`
	Protocol    string   `json:"protocol"`
	Models      []string `json:"models"`
	Error       string   `json:"error,omitempty"`
	TestOK      bool     `json:"test_ok,omitempty"`
	TestError   string   `json:"test_error,omitempty"`
}

// upstreamView never echoes upstream API keys back to clients.
type upstreamView struct {
	ModelIDs []string `json:"model_ids"`
	registry.Upstream
	APIKey     string                  `json:"api_key"`
	HasKey     bool                    `json:"has_key"`
	Models     int                     `json:"models"`
	Routing    registry.RoutingPolicy  `json:"routing"`
	Inflight   int                     `json:"inflight"`
	Status     registry.UpstreamStatus `json:"status"`
	Manageable bool                    `json:"manageable"`
}

var idSlug = regexp.MustCompile(`[^a-z0-9-]+`)

func (s *Server) handleStatus(w http.ResponseWriter, r *http.Request) {
	id := s.requestIdentity(r)
	owner, admin := id.ownerKey(), id.Admin
	upstreams := s.reg.ListFor(owner, admin)
	healthy := 0
	for _, u := range upstreams {
		if u.Healthy {
			healthy++
		}
	}
	modelCount := 0
	for _, m := range s.reg.ListModelsFor(owner, admin) {
		if m.Qualified != m.ID {
			modelCount++
		}
	}
	usage := s.usage.snapshot("all")
	writeJSON(w, http.StatusOK, map[string]any{
		"version":           s.version,
		"upstreams_total":   len(upstreams),
		"upstreams_healthy": healthy,
		"models":            modelCount,
		"requests_total":    usage.Totals.Requests,
		"usage_since":       usage.Since,
		"key_required":      s.requiresKey(),
		"oidc_enabled":      s.userInfoURL != "" && !s.authDisabled,
		"user":              id.Username,
		"is_admin":          id.Admin,
		"base_urls": map[string]string{
			"internal":    "http://v-router:8080/v1",
			"lan":         "http://<vos-host>:" + envOr("V_ROUTER_HOST_PORT", "22888") + "/v1",
			"same_origin": gatewayAPIBase() + "/v1",
		},
	})
}

func (s *Server) handleListUpstreams(w http.ResponseWriter, r *http.Request) {
	id := s.requestIdentity(r)
	var out []upstreamView
	for _, u := range s.reg.ListFor(id.ownerKey(), id.Admin) {
		out = append(out, toView(u, s.reg, id.ownerKey(), id.Admin))
	}
	out = append(out, videoStructureManagementUpstreams(s.reg)...)
	if out == nil {
		out = []upstreamView{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"upstreams": out})
}

func videoStructureManagementUpstreams(reg *registry.Registry) []upstreamView {
	specs := []struct{ id, name string }{
		{registry.ServiceVideoStructureDetect, "视频结构化 · 场景检测"},
		{registry.ServiceVideoStructureStructure, "视频结构化 · 结构化与描述"},
	}
	var out []upstreamView
	for _, spec := range specs {
		baseURL, ok := reg.Service(spec.id)
		if !ok {
			continue
		}
		count := 0
		for _, endpoint := range videoStructureInterfaces {
			if endpoint.service == spec.id {
				count++
			}
		}
		out = append(out, upstreamView{
			Upstream: registry.Upstream{ID: spec.id, Name: spec.name, Kind: registry.KindLocal, BaseURL: baseURL, Protocol: "business_api", Source: videoStructureSource, Healthy: true},
			Models:   count, Status: registry.UpstreamStatus{State: registry.StateHealthy}, Manageable: false,
		})
	}
	return out
}

func (s *Server) handleProbeUpstream(w http.ResponseWriter, r *http.Request) {
	var p upstreamPayload
	if err := decodeJSON(r, &p); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	normalized, err := normalizeUpstreamPayload(p)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if strings.TrimSpace(normalized.Name) == "" {
		normalized.Name = "probe"
	}
	if err := validateUpstreamPayload(normalized); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := s.guardProxyLoop(normalized.BaseURL); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	u := registry.Upstream{
		ID:          "probe",
		Name:        normalized.Name,
		Kind:        normalized.kind(),
		BaseURL:     normalized.BaseURL,
		ModelPrefix: modelPrefixForPayload(normalized),
		APIKey:      normalized.APIKey,
		Protocol:    normalized.Protocol,
		Source:      registry.SourceAPI,
		AsyncTask:   normalized.AsyncTask,
	}
	out := upstreamProbeView{BaseURL: u.BaseURL, ModelPrefix: u.PublicPrefix(), Protocol: u.Protocol}
	if normalized.Protocol == registry.ProtocolAsyncTask {
		out.OK = true
		out.Models = []string{normalized.Model}
		writeJSON(w, http.StatusOK, out)
		return
	}
	models, err := fetchUpstreamModels(u)
	if err != nil {
		out.Error = err.Error()
	} else {
		out.OK = true
		for _, m := range models {
			if m.ID != "" {
				out.Models = append(out.Models, m.ID)
			}
		}
	}
	if strings.TrimSpace(normalized.TestModel) != "" {
		if err := probeUpstreamModel(u, strings.TrimSpace(normalized.TestModel)); err != nil {
			out.TestError = err.Error()
		} else {
			out.TestOK = true
		}
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) handleCreateUpstream(w http.ResponseWriter, r *http.Request) {
	var p upstreamPayload
	if err := decodeJSON(r, &p); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	p, err := normalizeUpstreamPayload(p)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := validateUpstreamPayload(p); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := s.guardProxyLoop(p.BaseURL); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}

	id := s.uniqueID(p.Name)
	creator := s.requestIdentity(r)
	u := &registry.Upstream{
		ID:          id,
		Name:        p.Name,
		Kind:        p.kind(),
		BaseURL:     p.BaseURL,
		ModelPrefix: modelPrefixForPayload(p),
		APIKey:      p.APIKey,
		Protocol:    p.Protocol,
		Visibility:  visibilityOrDefault(p.Visibility),
		Owner:       creator.ownerKey(),
		Source:      registry.SourceAPI,
		CreatedAt:   time.Now().UTC(),
		AsyncTask:   p.AsyncTask,
	}
	if p.Protocol == registry.ProtocolAsyncTask {
		u.Healthy = true
		s.setAsyncTaskModel(u, p.Model)
	} else if err := s.probeAndFill(u); err != nil {
		if !p.AllowProbeFailure {
			writeError(w, http.StatusBadGateway, "probe upstream failed: "+err.Error())
			return
		}
		u.Healthy = true
		u.LastError = "model probe failed: " + err.Error()
		s.setFallbackModel(u, p.TestModel)
	}
	s.reg.Put(u)
	if err := s.reg.Persist(); err != nil {
		writeError(w, http.StatusInternalServerError, "persist state: "+err.Error())
		return
	}
	creatorKey, creatorAdmin := creator.ownerKey(), creator.Admin
	writeJSON(w, http.StatusCreated, toView(*u, s.reg, creatorKey, creatorAdmin))
}

func (s *Server) handleUpdateUpstream(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	current, ok := s.reg.Get(id)
	if !ok {
		writeError(w, http.StatusNotFound, "upstream not found: "+id)
		return
	}
	if current.Source != registry.SourceAPI {
		writeError(w, http.StatusConflict, "discovered upstreams cannot be edited; change them at the source")
		return
	}
	caller := s.requestIdentity(r)
	if !current.VisibleTo(caller.ownerKey(), caller.Admin) {
		writeError(w, http.StatusNotFound, "upstream not found: "+id)
		return
	}
	if !current.ManageableBy(caller.ownerKey(), caller.Admin) {
		writeError(w, http.StatusForbidden, "upstream is managed by its owner or an administrator")
		return
	}
	var p upstreamPayload
	if err := decodeJSON(r, &p); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	requestedKind := strings.TrimSpace(p.Kind)
	p, err := normalizeUpstreamPayload(p)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if requestedKind == "" {
		p.Kind = current.Kind
	}
	if err := validateUpstreamPayload(p); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := s.guardProxyLoop(p.BaseURL); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	current.Name = p.Name
	current.Kind = p.kind()
	if p.Protocol != "" {
		current.Protocol = p.Protocol
	}
	current.BaseURL = p.BaseURL
	current.ModelPrefix = modelPrefixForPayload(p)
	current.AsyncTask = p.AsyncTask
	if strings.TrimSpace(p.Visibility) != "" {
		current.Visibility = visibilityOrDefault(p.Visibility)
	}
	// An empty api_key on update keeps the stored one so the masked UI can
	// edit other fields without re-entering the secret.
	if p.APIKey != "" {
		current.APIKey = p.APIKey
	}
	if p.Protocol == registry.ProtocolAsyncTask {
		current.Healthy = true
		current.LastError = ""
		s.setAsyncTaskModel(&current, p.Model)
	} else if err := s.probeAndFill(&current); err != nil {
		if !p.AllowProbeFailure {
			writeError(w, http.StatusBadGateway, "probe upstream failed: "+err.Error())
			return
		}
		current.Healthy = true
		current.LastError = "model probe failed: " + err.Error()
		s.setFallbackModel(&current, p.TestModel)
	}
	s.reg.Put(&current)
	if err := s.reg.Persist(); err != nil {
		writeError(w, http.StatusInternalServerError, "persist state: "+err.Error())
		return
	}
	writeJSON(w, http.StatusOK, toView(current, s.reg, caller.ownerKey(), caller.Admin))
}

func (s *Server) handleRefreshUpstream(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	u, ok := s.reg.Get(id)
	if !ok {
		writeError(w, http.StatusNotFound, "upstream not found: "+id)
		return
	}
	if u.Source != registry.SourceAPI {
		writeError(w, http.StatusConflict, "only API-created upstreams can be refreshed here")
		return
	}
	caller := s.requestIdentity(r)
	if !u.VisibleTo(caller.ownerKey(), caller.Admin) {
		writeError(w, http.StatusNotFound, "upstream not found: "+id)
		return
	}
	if !u.ManageableBy(caller.ownerKey(), caller.Admin) {
		writeError(w, http.StatusForbidden, "upstream is managed by its owner or an administrator")
		return
	}
	if u.Protocol == registry.ProtocolAsyncTask {
		u.Healthy = true
		u.LastError = ""
		s.reg.Put(&u)
		writeJSON(w, http.StatusOK, toView(u, s.reg, caller.ownerKey(), caller.Admin))
		return
	}
	if err := s.probeAndFill(&u); err != nil {
		u.Healthy = false
		u.LastError = err.Error()
		s.reg.Put(&u)
		writeError(w, http.StatusBadGateway, "probe upstream failed: "+err.Error())
		return
	}
	s.reg.Put(&u)
	writeJSON(w, http.StatusOK, toView(u, s.reg, caller.ownerKey(), caller.Admin))
}

func (s *Server) handleDeleteUpstream(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	u, ok := s.reg.Get(id)
	if !ok {
		writeError(w, http.StatusNotFound, "upstream not found: "+id)
		return
	}
	if u.Source != registry.SourceAPI {
		writeError(w, http.StatusConflict, "discovered upstreams cannot be deleted; remove them at the source")
		return
	}
	caller := s.requestIdentity(r)
	if !u.VisibleTo(caller.ownerKey(), caller.Admin) {
		writeError(w, http.StatusNotFound, "upstream not found: "+id)
		return
	}
	if !u.ManageableBy(caller.ownerKey(), caller.Admin) {
		writeError(w, http.StatusForbidden, "upstream is managed by its owner or an administrator")
		return
	}
	s.reg.Delete(id)
	if err := s.reg.Persist(); err != nil {
		writeError(w, http.StatusInternalServerError, "persist state: "+err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// probeAndFill fetches GET base/models, marks health, and fills the model
// list of a manual upstream.
func (s *Server) probeAndFill(u *registry.Upstream) error {
	resp, err := fetchUpstreamModels(*u)
	if err != nil {
		u.Healthy = false
		u.LastError = err.Error()
		return err
	}
	u.Healthy = true
	u.LastError = ""
	models := make([]registry.Model, 0, len(resp))
	for _, m := range resp {
		if m.ID == "" {
			continue
		}
		kind := "unknown"
		if len(m.Capabilities) > 0 {
			kind = registry.ModelKindFromCapabilities(m.Capabilities, kind)
		}
		models = append(models, registry.Model{
			ID:           m.ID,
			Qualified:    u.PublicPrefix() + "/" + m.ID,
			Upstream:     u.ID,
			Kind:         kind,
			Capabilities: append([]string(nil), m.Capabilities...),
			Endpoints:    append([]string(nil), m.Endpoints...),
			OwnedBy:      m.OwnedBy,
		})
	}
	s.reg.SetModels(u.ID, models)
	return nil
}

func (s *Server) setFallbackModel(u *registry.Upstream, model string) {
	model = strings.TrimSpace(model)
	if model == "" {
		s.reg.SetModels(u.ID, nil)
		return
	}
	s.reg.SetModels(u.ID, []registry.Model{{
		ID:        model,
		Qualified: u.PublicPrefix() + "/" + model,
		Upstream:  u.ID,
		Kind:      "unknown",
	}})
}

func (s *Server) setAsyncTaskModel(u *registry.Upstream, model string) {
	model = strings.TrimSpace(model)
	s.reg.SetModels(u.ID, []registry.Model{{
		ID: model, Qualified: u.PublicPrefix() + "/" + model, Upstream: u.ID,
		Kind: "unknown", Capabilities: []string{"image_generation"}, Endpoints: []string{"images/generations"},
	}})
}

func decodeJSON(r *http.Request, v any) error {
	defer r.Body.Close()
	body, err := io.ReadAll(io.LimitReader(r.Body, 1<<20))
	if err != nil {
		return err
	}
	if err := json.Unmarshal(body, v); err != nil {
		return fmt.Errorf("invalid JSON body: %w", err)
	}
	return nil
}

func validateManualUpstream(p upstreamPayload) error {
	p.Kind = registry.KindManual
	normalized, err := normalizeUpstreamPayload(p)
	if err != nil {
		return err
	}
	return validateUpstreamPayload(normalized)
}

func (p upstreamPayload) kind() string {
	if p.Kind == registry.KindCloud {
		return registry.KindCloud
	}
	return registry.KindManual
}

func normalizeUpstreamPayload(p upstreamPayload) (upstreamPayload, error) {
	p.Name = strings.TrimSpace(p.Name)
	p.Kind = strings.TrimSpace(p.Kind)
	p.Protocol = strings.TrimSpace(p.Protocol)
	p.BaseURL = strings.TrimSpace(p.BaseURL)
	p.Model = strings.TrimSpace(p.Model)
	p.Visibility = strings.ToLower(strings.TrimSpace(p.Visibility))
	if p.Kind == "" {
		p.Kind = registry.KindManual
	}
	if p.Protocol == "" {
		p.Protocol = registry.ProtocolOpenAI
	}
	if p.BaseURL != "" && !strings.Contains(p.BaseURL, "://") {
		p.BaseURL = "https://" + p.BaseURL
	}
	parsed, err := url.Parse(p.BaseURL)
	if err != nil || parsed.Scheme == "" || parsed.Host == "" {
		return p, errors.New("base_url must be an absolute http(s) URL")
	}
	if parsed.Scheme != "http" && parsed.Scheme != "https" {
		return p, errors.New("base_url must use http or https")
	}
	if strings.Trim(parsed.Path, "/") == "" && p.Protocol != registry.ProtocolOfflineASR && p.Protocol != registry.ProtocolImageStudio && p.Protocol != registry.ProtocolAsyncTask {
		parsed.Path = "/v1"
	}
	parsed.RawQuery = ""
	parsed.Fragment = ""
	p.BaseURL = strings.TrimRight(parsed.String(), "/")
	return p, nil
}

func validateUpstreamPayload(p upstreamPayload) error {
	if p.Kind != "" && p.Kind != registry.KindManual && p.Kind != registry.KindCloud {
		return errors.New("kind must be manual or cloud")
	}
	if p.Visibility != "" && p.Visibility != registry.VisibilityPublic && p.Visibility != registry.VisibilityPrivate {
		return errors.New("visibility must be public or private")
	}
	if p.Protocol != "" && p.Protocol != registry.ProtocolOpenAI && p.Protocol != registry.ProtocolChatCompletions && p.Protocol != registry.ProtocolAnthropic && p.Protocol != registry.ProtocolOfflineASR && p.Protocol != registry.ProtocolImageStudio && p.Protocol != registry.ProtocolAsyncTask {
		return errors.New("protocol must be openai, chat_completions, anthropic, offline_asr, image_studio or async_task")
	}
	if strings.TrimSpace(p.Name) == "" {
		return errors.New("name is required")
	}
	parsed, err := url.Parse(p.BaseURL)
	if err != nil || parsed.Scheme == "" || parsed.Host == "" {
		return errors.New("base_url must be an absolute http(s) URL")
	}
	if p.Protocol == registry.ProtocolAsyncTask {
		if p.Model == "" {
			return errors.New("model is required for async_task")
		}
		if err := validateAsyncTaskConfig(p.AsyncTask); err != nil {
			return err
		}
	}
	return nil
}

func validateAsyncTaskConfig(c *registry.AsyncTaskConfig) error {
	if c == nil {
		return errors.New("async_task configuration is required")
	}
	required := []struct{ name, value string }{
		{"create_path", c.CreatePath}, {"query_path", c.QueryPath}, {"task_id_path", c.TaskIDPath},
		{"status_path", c.StatusPath}, {"result_path", c.ResultPath},
	}
	for _, field := range required {
		if strings.TrimSpace(field.value) == "" {
			return fmt.Errorf("async_task.%s is required", field.name)
		}
	}
	if len(c.SuccessStates) == 0 || len(c.FailureStates) == 0 {
		return errors.New("async_task success_states and failure_states are required")
	}
	if c.PollIntervalMS < 100 || c.PollIntervalMS > 60000 {
		return errors.New("async_task.poll_interval_ms must be 100..60000")
	}
	if c.TimeoutSeconds < 1 || c.TimeoutSeconds > 3600 {
		return errors.New("async_task.timeout_seconds must be 1..3600")
	}
	return nil
}

// visibilityOrDefault maps the optional payload field to the upstream scope;
// empty means the shared default so existing clients keep working.
func visibilityOrDefault(visibility string) string {
	if strings.ToLower(strings.TrimSpace(visibility)) == registry.VisibilityPrivate {
		return registry.VisibilityPrivate
	}
	return registry.VisibilityPublic
}

func modelPrefixForPayload(p upstreamPayload) string {
	if p.kind() != registry.KindCloud {
		return ""
	}
	parsed, err := url.Parse(p.BaseURL)
	if err != nil {
		return registry.KindCloud
	}
	host := strings.ToLower(parsed.Hostname())
	host = strings.TrimPrefix(host, "www.")
	parts := strings.Split(host, ".")
	clean := make([]string, 0, len(parts))
	for i := len(parts) - 1; i >= 0; i-- {
		part := idSlug.ReplaceAllString(parts[i], "-")
		part = strings.Trim(part, "-")
		if part != "" {
			clean = append(clean, part)
		}
	}
	if len(clean) == 0 {
		return registry.KindCloud
	}
	return strings.Join(clean, ".")
}

func probeUpstreamModel(u registry.Upstream, model string) error {
	var path string
	var payload any
	if u.Protocol == "anthropic" {
		path = "/messages"
		payload = map[string]any{"model": model, "max_tokens": 1, "messages": []map[string]string{{"role": "user", "content": "ping"}}}
	} else {
		path = "/chat/completions"
		payload = map[string]any{"model": model, "max_tokens": 1, "messages": []map[string]string{{"role": "user", "content": "ping"}}}
	}
	body, _ := json.Marshal(payload)
	req, err := http.NewRequest(http.MethodPost, u.BaseURL+path, bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	if u.APIKey != "" {
		if u.Protocol == "anthropic" {
			req.Header.Set("X-Api-Key", u.APIKey)
			req.Header.Set("Anthropic-Version", "2023-06-01")
		} else {
			req.Header.Set("Authorization", "Bearer "+u.APIKey)
		}
	}
	resp, err := probeClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		msg := strings.TrimSpace(string(body))
		if msg == "" {
			msg = resp.Status
		}
		return fmt.Errorf("model test: status %d: %s", resp.StatusCode, msg)
	}
	return nil
}

// guardProxyLoop rejects URLs that point back at this gateway or VOS internal
// services, preventing self-proxying loops.
func (s *Server) guardProxyLoop(rawBase string) error {
	if s.allowLoopbackUpstreams {
		return nil
	}
	parsed, err := url.Parse(rawBase)
	if err != nil {
		return errors.New("base_url is not a valid URL")
	}
	host := strings.ToLower(parsed.Hostname())
	self := []string{"v-router", "v-router-backend", "localhost", "127.0.0.1", "::1"}
	for _, h := range self {
		if host == h {
			return fmt.Errorf("base_url must not point at the gateway itself (%s)", host)
		}
	}
	if strings.HasSuffix(host, ".vos_default") || strings.HasPrefix(host, "model-hub-") {
		return fmt.Errorf("base_url must not target internal VOS services (%s); internal models are discovered automatically", host)
	}
	return nil
}

func (s *Server) uniqueID(name string) string {
	base := idSlug.ReplaceAllString(strings.ToLower(strings.TrimSpace(name)), "-")
	base = strings.Trim(base, "-")
	if base == "" {
		base = "upstream"
	}
	id, n := base, 2
	for {
		if _, exists := s.reg.Get(id); !exists {
			return id
		}
		id = fmt.Sprintf("%s-%d", base, n)
		n++
	}
}

func toView(u registry.Upstream, reg *registry.Registry, owner string, admin bool) upstreamView {
	u.Visibility = u.Scope()
	policy, inflight := reg.Routing(u.ID)
	status, _ := reg.StatusOf(u.ID)
	ids := []string{}
	for _, m := range reg.ModelsOf(u.ID) {
		ids = append(ids, m.ID)
	}
	hasKey := u.APIKey != ""
	u.APIKey = ""
	return upstreamView{
		Upstream: u, ModelIDs: ids,
		Routing: policy, Inflight: inflight,
		APIKey:     "",
		HasKey:     hasKey,
		Models:     len(reg.ModelsOf(u.ID)),
		Status:     status,
		Manageable: u.ManageableBy(owner, admin),
	}
}
