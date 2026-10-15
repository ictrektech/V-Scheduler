package registry

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"
)

const vVoiceUpstreamID = "app-v-voice"

// VVoicePoller discovers the same-VOS V-Voice service. Candidates are tried in
// order on every poll so the vos_default alias always wins over the host port.
// Absence is normal: no unhealthy placeholder or recurring error is emitted.
type VVoicePoller struct {
	reg        *Registry
	candidates []string
	interval   time.Duration
	client     *http.Client
}

type vVoiceDiscovery struct {
	Name       string        `json:"name"`
	Protocol   string        `json:"protocol"`
	Visibility string        `json:"visibility"`
	Models     []vVoiceModel `json:"models"`
}

type vVoiceModel struct {
	ID           string   `json:"id"`
	OwnedBy      string   `json:"owned_by"`
	Kind         string   `json:"kind"`
	Capabilities []string `json:"capabilities"`
	Endpoints    []string `json:"endpoints"`
}

func NewVVoicePoller(reg *Registry, candidates []string, interval time.Duration) *VVoicePoller {
	clean := make([]string, 0, len(candidates))
	for _, candidate := range candidates {
		if candidate = strings.TrimRight(strings.TrimSpace(candidate), "/"); candidate != "" {
			clean = append(clean, candidate)
		}
	}
	return &VVoicePoller{reg: reg, candidates: clean, interval: interval, client: &http.Client{Timeout: 5 * time.Second}}
}

func (p *VVoicePoller) Run(ctx context.Context) {
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

func (p *VVoicePoller) pollOnce(ctx context.Context) {
	for _, candidate := range p.candidates {
		discovery, err := p.fetchDiscovery(ctx, candidate)
		if err != nil {
			continue
		}
		baseURL := candidate + "/v1"
		if p.reg.HasNonModelHubBaseURL(baseURL, vVoiceUpstreamID) {
			p.reg.Delete(vVoiceUpstreamID)
			return
		}
		name := strings.TrimSpace(discovery.Name)
		if name == "" {
			name = "V-Voice"
		}
		u := &Upstream{ID: vVoiceUpstreamID, Name: name, Kind: KindLocal, Source: SourceVVoice, BaseURL: baseURL, Protocol: vVoiceProtocol(discovery.Protocol), Visibility: vVoiceVisibility(discovery.Visibility), ModelPrefix: SourceVVoice, Healthy: true}
		p.reg.Put(u)
		p.reg.SetModels(u.ID, vVoiceModels(discovery.Models))
		return
	}
	p.reg.Delete(vVoiceUpstreamID)
}

// vVoiceProtocol honors a declared protocol the gateway can serve; anything
// else falls back to the documented OpenAI-compatible default.
func vVoiceProtocol(protocol string) string {
	if strings.ToLower(strings.TrimSpace(protocol)) == ProtocolChatCompletions {
		return ProtocolChatCompletions
	}
	return ProtocolOpenAI
}

// vVoiceVisibility honors a service-declared scope. V-Voice documents
// public shared models and receives no user identity, so a private
// declaration simply hides the service from non-admin callers.
func vVoiceVisibility(visibility string) string {
	if strings.ToLower(strings.TrimSpace(visibility)) == VisibilityPrivate {
		return VisibilityPrivate
	}
	return VisibilityPublic
}

func (p *VVoicePoller) fetchDiscovery(ctx context.Context, baseURL string) (vVoiceDiscovery, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, baseURL+"/api/v1/discovery", nil)
	if err != nil {
		return vVoiceDiscovery{}, err
	}
	resp, err := p.client.Do(req)
	if err != nil {
		return vVoiceDiscovery{}, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return vVoiceDiscovery{}, fmt.Errorf("discovery probe: status %d", resp.StatusCode)
	}
	var discovery vVoiceDiscovery
	if err := json.NewDecoder(resp.Body).Decode(&discovery); err != nil {
		return vVoiceDiscovery{}, err
	}
	return discovery, nil
}

func vVoiceModels(rows []vVoiceModel) []Model {
	models := make([]Model, 0, len(rows))
	for _, row := range rows {
		id := strings.TrimSpace(row.ID)
		endpoints := NormalizeEndpoints(row.Endpoints)
		if id == "" || len(endpoints) == 0 {
			continue
		}
		caps := MergeCapabilities(row.Capabilities, CapabilitiesFromEndpoints(endpoints))
		kind := strings.TrimSpace(row.Kind)
		if kind == "" {
			kind = "unknown"
		}
		models = append(models, Model{ID: id, Qualified: SourceVVoice + "/" + id, Upstream: vVoiceUpstreamID, Kind: kind, Capabilities: caps, Endpoints: endpoints, OwnedBy: row.OwnedBy})
	}
	return models
}
