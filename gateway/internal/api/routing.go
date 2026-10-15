package api

import (
	"errors"
	"fmt"
	"net/http"
	"regexp"
	"strings"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

func (s *Server) routingError(w http.ResponseWriter, r *http.Request, err error) {
	status := http.StatusServiceUnavailable
	switch {
	case errors.Is(err, registry.ErrUnknownModel):
		status = 404
	case errors.Is(err, registry.ErrUnsupported):
		status = 400
	case errors.Is(err, registry.ErrBusy):
		status = 429
		w.Header().Set("Retry-After", "1")
	}
	writeRequestError(w, r, status, err.Error())
}
func (s *Server) handleRouting(w http.ResponseWriter, r *http.Request) {
	id := s.requestIdentity(r)
	u, ok := s.reg.Get(r.PathValue("id"))
	if !ok {
		writeError(w, http.StatusNotFound, "upstream not found")
		return
	}
	if !u.VisibleTo(id.ownerKey(), id.Admin) {
		writeError(w, http.StatusNotFound, "upstream not found")
		return
	}
	if !u.ManageableBy(id.ownerKey(), id.Admin) {
		writeError(w, http.StatusForbidden, "upstream is managed by its owner or an administrator")
		return
	}
	var p registry.RoutingPolicy
	if err := decodeJSON(r, &p); err != nil {
		writeError(w, 400, err.Error())
		return
	}
	if err := p.Validate(); err != nil {
		writeError(w, 400, err.Error())
		return
	}
	if err := s.reg.SetRouting(r.PathValue("id"), p); err != nil {
		writeError(w, 404, "upstream not found")
		return
	}
	if err := s.reg.Persist(); err != nil {
		writeError(w, 500, "persist routing: "+err.Error())
		return
	}
	p, n := s.reg.Routing(r.PathValue("id"))
	writeJSON(w, 200, map[string]any{"routing": p, "inflight": n})
}

type registrationPayload struct {
	Scheduler         *registry.SchedulerPolicy `json:"scheduler,omitempty"`
	ServiceID         string                    `json:"service_id"`
	Name              string                    `json:"name"`
	BaseURL           string                    `json:"base_url"`
	APIKey            string                    `json:"api_key"`
	Protocol          string                    `json:"protocol"`
	Visibility        string                    `json:"visibility"`
	Model             string                    `json:"model"`
	Models            []registeredModel         `json:"models"`
	ModelKind         string                    `json:"model_kind"`
	Capabilities      []string                  `json:"capabilities"`
	Endpoints         []string                  `json:"endpoints"`
	ModelCapabilities map[string][]string       `json:"model_capabilities"`
	ModelEndpoints    map[string][]string       `json:"model_endpoints"`
	TTLSeconds        int                       `json:"ttl_seconds"`
}

type registeredModel struct {
	ID           string   `json:"id"`
	Model        string   `json:"model"`
	OwnedBy      string   `json:"owned_by"`
	ModelKind    string   `json:"model_kind"`
	Capabilities []string `json:"capabilities"`
	Endpoints    []string `json:"endpoints"`
}

func (m registeredModel) modelID() string {
	if strings.TrimSpace(m.ID) != "" {
		return strings.TrimSpace(m.ID)
	}
	return strings.TrimSpace(m.Model)
}

var serviceIDPattern = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,95}$`)

func leaseTTL(seconds int) (time.Duration, bool) {
	if seconds == 0 {
		seconds = 90
	}
	return time.Duration(seconds) * time.Second, seconds >= 30 && seconds <= 3600
}
func (s *Server) handleRegister(w http.ResponseWriter, r *http.Request) {
	var p registrationPayload
	if err := decodeJSON(r, &p); err != nil {
		writeError(w, 400, err.Error())
		return
	}
	ttl, ok := leaseTTL(p.TTLSeconds)
	if !ok || !serviceIDPattern.MatchString(p.ServiceID) {
		writeError(w, 400, "service_id must be 1..96 alphanumeric/._- characters; ttl_seconds must be 30..3600 (default 90)")
		return
	}
	if p.Name == "" {
		p.Name = p.ServiceID
	}
	if p.ModelKind == "" {
		p.ModelKind = "unknown"
	}
	if !validModelKind(p.ModelKind) {
		writeError(w, 400, "model_kind must be chat, embedding or unknown")
		return
	}
	visibility, err := normalizeRegistrationVisibility(p.Visibility)
	if err != nil {
		writeError(w, 400, err.Error())
		return
	}
	id := s.requestIdentity(r)
	owner := id.ownerKey()
	if p.Scheduler != nil {
		if !id.Admin {
			writeError(w, 403, "scheduler policy requires administrator approval")
			return
		}
		mode := p.Scheduler.Mode
		if mode != "cloud" && mode != "delegated" && mode != "permit" {
			writeError(w, 400, "invalid scheduler mode")
			return
		}
		if mode == "permit" && (p.Scheduler.ExecutorID == "" || p.Scheduler.Completion != "synchronous") {
			writeError(w, 400, "permit policy requires executor_id and synchronous completion")
			return
		}
	}
	if visibility == registry.VisibilityPrivate && owner == "" {
		writeError(w, 401, "private registration requires a VOS user token or gateway key")
		return
	}
	normalized, err := normalizeUpstreamPayload(upstreamPayload{Name: p.Name, BaseURL: p.BaseURL, Protocol: p.Protocol})
	if err != nil {
		writeError(w, 400, err.Error())
		return
	}
	if err := s.guardProxyLoop(normalized.BaseURL); err != nil {
		writeError(w, 400, err.Error())
		return
	}
	p.Name, p.BaseURL, p.Protocol = normalized.Name, normalized.BaseURL, normalized.Protocol
	if p.Scheduler != nil {
		p.Scheduler.UpstreamURL = p.BaseURL
	}
	upstreamID := "registered-" + p.ServiceID
	if current, ok := s.reg.Get(upstreamID); ok && !current.ManageableBy(owner, id.Admin) {
		writeError(w, 409, "service_id is already registered by another owner")
		return
	}
	u := registry.Upstream{Scheduler: p.Scheduler, ID: upstreamID, Name: p.Name, Kind: registry.KindRegistered, Source: registry.SourceRegistered, BaseURL: p.BaseURL, APIKey: p.APIKey, Protocol: p.Protocol, Visibility: visibility, Owner: owner, Healthy: true}

	models, err := registeredModelsFromPayload(u, p)
	if err != nil {
		writeError(w, 400, err.Error())
		return
	}
	if len(models) == 0 {
		rows, err := fetchUpstreamModels(u)
		if err != nil {
			writeError(w, 502, "registration probe failed: "+err.Error())
			return
		}
		models, err = registeredModelsFromProbe(u, p, rows)
		if err != nil {
			writeError(w, 400, err.Error())
			return
		}
	}
	u.ExpiresAt = time.Now().UTC().Add(ttl)
	if err := s.reg.Register(u, models); err != nil {
		writeError(w, 409, err.Error())
		return
	}
	u, _ = s.reg.Get(u.ID)
	writeJSON(w, 200, map[string]any{"upstream": toView(u, s.reg, owner, id.Admin), "heartbeat_after_seconds": int(ttl.Seconds()) / 3})
}

func normalizeRegistrationVisibility(scope string) (string, error) {
	scope = strings.ToLower(strings.TrimSpace(scope))
	if scope == "" {
		return registry.VisibilityPublic, nil
	}
	if scope == registry.VisibilityPublic || scope == registry.VisibilityPrivate {
		return scope, nil
	}
	return "", errors.New("visibility must be public or private")
}

func validModelKind(kind string) bool {
	return kind == "chat" || kind == "embedding" || kind == "unknown"
}

func registeredModelsFromPayload(u registry.Upstream, p registrationPayload) ([]registry.Model, error) {
	items := append([]registeredModel(nil), p.Models...)
	if strings.TrimSpace(p.Model) != "" {
		items = append(items, registeredModel{ID: p.Model, ModelKind: p.ModelKind, Capabilities: p.Capabilities, Endpoints: p.Endpoints})
	}
	if len(items) == 0 {
		return nil, nil
	}
	models := make([]registry.Model, 0, len(items))
	for _, item := range items {
		id := item.modelID()
		if id == "" {
			return nil, errors.New("registered model id is required")
		}
		caps := registry.MergeCapabilities(p.Capabilities, item.Capabilities, registry.CapabilitiesFromEndpoints(p.Endpoints), registry.CapabilitiesFromEndpoints(item.Endpoints))
		endpoints := registry.NormalizeEndpoints(append(append([]string{}, p.Endpoints...), item.Endpoints...))
		if perModel := p.ModelCapabilities[id]; len(perModel) > 0 {
			caps = registry.MergeCapabilities(caps, perModel)
		}
		if perModel := p.ModelEndpoints[id]; len(perModel) > 0 {
			endpoints = registry.NormalizeEndpoints(append(endpoints, perModel...))
			caps = registry.MergeCapabilities(caps, registry.CapabilitiesFromEndpoints(perModel))
		}
		if len(endpoints) == 0 {
			return nil, fmt.Errorf("registered model %q must declare endpoints", id)
		}
		kind := item.ModelKind
		if kind == "" {
			kind = p.ModelKind
		}
		if kind == "" {
			kind = "unknown"
		}
		if !validModelKind(kind) {
			return nil, fmt.Errorf("model %q model_kind must be chat, embedding or unknown", id)
		}
		kind = registry.ModelKindFromCapabilities(caps, kind)
		models = append(models, registry.Model{ID: id, Qualified: u.PublicPrefix() + "/" + id, Upstream: u.ID, Kind: kind, Capabilities: caps, Endpoints: endpoints, OwnedBy: item.OwnedBy})
	}
	return models, nil
}

func registeredModelsFromProbe(u registry.Upstream, p registrationPayload, rows []registry.Model) ([]registry.Model, error) {
	models := make([]registry.Model, 0, len(rows))
	defaultEndpoints := registry.NormalizeEndpoints(p.Endpoints)
	defaultCaps := registry.MergeCapabilities(p.Capabilities, registry.CapabilitiesFromEndpoints(defaultEndpoints))
	for _, m := range rows {
		if m.ID == "" {
			continue
		}
		caps := registry.MergeCapabilities(defaultCaps, m.Capabilities)
		if perModel := p.ModelCapabilities[m.ID]; len(perModel) > 0 {
			caps = registry.MergeCapabilities(caps, perModel)
		}
		endpoints := append([]string(nil), defaultEndpoints...)
		if perModel := p.ModelEndpoints[m.ID]; len(perModel) > 0 {
			endpoints = registry.NormalizeEndpoints(append(endpoints, perModel...))
			caps = registry.MergeCapabilities(caps, registry.CapabilitiesFromEndpoints(perModel))
		}
		kind := p.ModelKind
		if len(caps) > 0 {
			kind = registry.ModelKindFromCapabilities(caps, kind)
		}
		models = append(models, registry.Model{ID: m.ID, Qualified: u.PublicPrefix() + "/" + m.ID, Upstream: u.ID, Kind: kind, Capabilities: caps, Endpoints: endpoints, OwnedBy: m.OwnedBy})
	}
	return models, nil
}
func (s *Server) handleHeartbeat(w http.ResponseWriter, r *http.Request) {
	if u, ok := s.reg.Get(r.PathValue("id")); !ok || !u.ManageableBy(s.requestIdentity(r).ownerKey(), s.requestIdentity(r).Admin) {
		writeError(w, 404, "registration missing or expired; register again")
		return
	}
	var p struct {
		TTLSeconds int `json:"ttl_seconds"`
	}
	if err := decodeJSON(r, &p); err != nil {
		writeError(w, 400, err.Error())
		return
	}
	ttl, ok := leaseTTL(p.TTLSeconds)
	if !ok {
		writeError(w, 400, "ttl_seconds must be 30..3600")
		return
	}
	u, err := s.reg.Heartbeat(r.PathValue("id"), ttl)
	if err != nil {
		writeError(w, 404, "registration missing or expired; register again")
		return
	}
	writeJSON(w, 200, map[string]any{"upstream_id": u.ID, "expires_at": u.ExpiresAt, "heartbeat_after_seconds": int(ttl.Seconds()) / 3})
}
func (s *Server) handleUnregister(w http.ResponseWriter, r *http.Request) {
	if u, ok := s.reg.Get(r.PathValue("id")); !ok || !u.ManageableBy(s.requestIdentity(r).ownerKey(), s.requestIdentity(r).Admin) {
		writeError(w, 404, "registration missing or expired; register again")
		return
	}
	err := s.reg.Unregister(r.PathValue("id"))
	if err != nil {
		status := 404
		if errors.Is(err, registry.ErrRegistrationConflict) {
			status = 409
		}
		writeError(w, status, err.Error())
		return
	}
	w.WriteHeader(204)
}
func (s *Server) routingRoutes() []apiRouteSpec {
	return []apiRouteSpec{
		{Method: "PUT", Path: "/api/v1/upstreams/{id}/routing", Group: "manage", Auth: "manage", Summary: "配置上游自动调度", Desc: "适用于手动、Model Hub 和主动注册后端。weight 默认 1（1..1000），max_inflight 默认 0 不限并发，disabled 默认 false。该上限仅统计本网关进行中请求；SSE 到结束或取消才释放。配置持久化。仅 owner 和管理员可调用（403），他人 private 上游返回 404。", Handler: s.handleRouting},
		{Method: "POST", Path: "/api/v1/registrations", Group: "manage", Auth: "manage", Summary: "推理后端主动注册 / 更新", Desc: "需网关 Key 或 VOS 身份。稳定 service_id 生成 registered-<service_id>；相同 ID 更新同一实例，副本须不同 ID。可注册 public 或 private 服务；private 只对注册者身份可见，public 对所有调用者可见。可注册 base_url/models 模型 API，也可提交 model 或 models 显式注册单个/多个模型；显式模型必须声明 endpoints，capabilities/model_kind 用于接口能力过滤；model_endpoints 和 model_capabilities 可逐模型覆盖；ttl_seconds 默认90。", Handler: s.handleRegister},
		{Method: "POST", Path: "/api/v1/registrations/{id}/heartbeat", Group: "manage", Auth: "manage", Summary: "续期注册租约", Desc: "提交 JSON {ttl_seconds:90}，建议每30秒续期。404 表示租约过期或网关重启，须重新注册。心跳不刷新模型目录；模型变动请重新注册。", Handler: s.handleHeartbeat},
		{Method: "DELETE", Path: "/api/v1/registrations/{id}", Group: "manage", Auth: "manage", Summary: "注销推理后端", Desc: "立即退出新请求分配，已开始的请求继续完成。只允许注销主动注册来源。", Handler: s.handleUnregister},
	}
}
