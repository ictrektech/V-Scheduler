package registry

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// ModelHubPoller periodically reads the inference-service table from the
// Model Hub backend and mirrors local services and their models into the
// registry as kind=local upstreams.
type ModelHubPoller struct {
	reg      *Registry
	baseURL  string
	interval time.Duration
	client   *http.Client
}

// modelHubService is one entry of Model Hub GET /api/v1/inference-services.
// The endpoint returns a bare JSON array. kind is one of ollama |
// openai_compatible | custom; role is a free-form string (generate |
// embedding | rerank in practice) and may be null.
type modelHubService struct {
	ServiceID  string  `json:"service_id"`
	Name       string  `json:"name"`
	Kind       string  `json:"kind"`
	Role       *string `json:"role"`
	BaseURL    string  `json:"base_url"`
	GatewayURL string  `json:"gateway_url"`
}

type openAIModelsResponse struct {
	Data []struct {
		ID      string `json:"id"`
		OwnedBy string `json:"owned_by"`
	} `json:"data"`
}

type ollamaTagsResponse struct {
	Models []struct {
		Name         string   `json:"name"`
		Model        string   `json:"model"`
		Capabilities []string `json:"capabilities"`
	} `json:"models"`
}

func fallbackModelKindForHubRole(role string) string {
	switch role {
	case RoleEmbed, RoleEmbedding:
		return ModelKindEmbedding
	case RoleGenerate, "":
		return ModelKindChat
	default:
		return "unknown"
	}
}

func NewModelHubPoller(reg *Registry, baseURL string, interval time.Duration) *ModelHubPoller {
	return &ModelHubPoller{
		reg:      reg,
		baseURL:  strings.TrimRight(baseURL, "/"),
		interval: interval,
		client:   &http.Client{Timeout: 10 * time.Second},
	}
}

// Run blocks until ctx is done, polling once immediately and then every
// interval.
func (p *ModelHubPoller) Run(ctx context.Context) {
	ticker := time.NewTicker(p.interval)
	defer ticker.Stop()
	for {
		p.pollOnce(ctx)
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

func (p *ModelHubPoller) pollOnce(ctx context.Context) {
	services, err := p.listServices(ctx)
	if err != nil {
		log.Printf("model hub discovery failed: %v", err)
		return
	}

	// Rebuild discovered upstreams from scratch so removed services disappear.
	kept := make(map[string]bool, len(services))
	for _, svc := range services {
		role := svc.roleString()
		upstreamID := "modelhub-" + svc.ServiceID
		base := svc.openAIBaseURL()
		if base != "" && p.reg.HasNonModelHubBaseURL(base, upstreamID) {
			continue
		}
		kept[upstreamID] = true
		models, err := p.fetchModels(ctx, svc)
		u := &Upstream{
			ID:     upstreamID,
			Name:   svc.Name,
			Kind:   KindLocal,
			Role:   role,
			Source: SourceModelHub,
		}
		if base != "" {
			u.BaseURL = base
			u.Healthy = err == nil
			if err != nil {
				u.LastError = err.Error()
			}
		} else {
			u.Healthy = false
			u.LastError = "no gateway_url registered"
		}
		p.reg.Put(u)
		if err == nil {
			p.reg.SetModels(upstreamID, models)
		}
	}
	for _, u := range p.reg.List() {
		if u.Source == SourceModelHub && !kept[u.ID] {
			p.reg.Delete(u.ID)
		}
	}
}

// roleString normalizes the nullable role field; empty means chat.
func (s modelHubService) roleString() string {
	if s.Role == nil {
		return ""
	}
	return strings.ToLower(strings.TrimSpace(*s.Role))
}

// openAIBaseURL returns the OpenAI-compatible root (including /v1) that the
// gateway should call. Calls must go through the Model Hub gateway (11535)
// rather than the native Ollama port so slot and speed metrics keep working.
func (s modelHubService) openAIBaseURL() string {
	raw := s.GatewayURL
	if raw == "" {
		return ""
	}
	return strings.TrimRight(raw, "/") + "/v1"
}

func (p *ModelHubPoller) listServices(ctx context.Context) ([]modelHubService, error) {
	// include_builtin=true is required: Model Hub filters its built-in Ollama
	// instances (QA / embedding / rerank) out of the default listing, and they
	// are exactly where locally deployed models run.
	req, err := http.NewRequestWithContext(ctx, http.MethodGet,
		p.baseURL+"/api/v1/inference-services?include_builtin=true", nil)
	if err != nil {
		return nil, err
	}
	resp, err := p.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("list inference-services: status %d", resp.StatusCode)
	}
	var services []modelHubService
	if err := json.NewDecoder(resp.Body).Decode(&services); err != nil {
		return nil, err
	}
	return services, nil
}

func (p *ModelHubPoller) fetchOllamaCapabilities(ctx context.Context, svc modelHubService) map[string][]string {
	if svc.Kind != "ollama" || strings.TrimSpace(svc.BaseURL) == "" {
		return nil
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(svc.BaseURL, "/")+"/api/tags", nil)
	if err != nil {
		return nil
	}
	resp, err := p.client.Do(req)
	if err != nil {
		return nil
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return nil
	}
	var out ollamaTagsResponse
	if err := json.Unmarshal(body, &out); err != nil {
		return nil
	}
	models := make(map[string][]string, len(out.Models))
	for _, m := range out.Models {
		name := m.Model
		if name == "" {
			name = m.Name
		}
		if name != "" && len(m.Capabilities) > 0 {
			models[name] = append([]string(nil), m.Capabilities...)
		}
	}
	return models
}

// fetchModels reads the model list of a service. Both Ollama gateways and
// OpenAI-compatible custom servers answer GET <base>/models.
func (p *ModelHubPoller) fetchModels(ctx context.Context, svc modelHubService) ([]Model, error) {
	base := svc.openAIBaseURL()
	if base == "" {
		return nil, fmt.Errorf("service %s has no gateway_url", svc.ServiceID)
	}
	if _, err := url.Parse(base); err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, base+"/models", nil)
	if err != nil {
		return nil, err
	}
	resp, err := p.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return nil, err
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("fetch models of %s: status %d", svc.ServiceID, resp.StatusCode)
	}
	var out openAIModelsResponse
	if err := json.Unmarshal(body, &out); err != nil {
		return nil, err
	}

	capabilities := p.fetchOllamaCapabilities(ctx, svc)
	fallbackKind := fallbackModelKindForHubRole(svc.roleString())
	models := make([]Model, 0, len(out.Data))
	for _, m := range out.Data {
		if m.ID == "" {
			continue
		}
		caps := append([]string(nil), capabilities[m.ID]...)
		models = append(models, Model{
			ID:           m.ID,
			Qualified:    SourceModelHub + "/" + m.ID,
			Upstream:     "modelhub-" + svc.ServiceID,
			Kind:         ModelKindFromCapabilities(caps, fallbackKind),
			Capabilities: caps,
			OwnedBy:      m.OwnedBy,
		})
	}
	return models, nil
}
