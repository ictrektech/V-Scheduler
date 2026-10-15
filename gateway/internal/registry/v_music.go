package registry

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"
)

const vMusicUpstreamID = "app-v-music"

// VMusicPoller discovers the same-VOS V-Music service. The vos_default alias
// is tried before the host-port fallback. An absent app is a normal state.
type VMusicPoller struct {
	reg        *Registry
	candidates []string
	interval   time.Duration
	client     *http.Client
}

type vMusicDiscovery struct {
	Name       string        `json:"name"`
	Visibility string        `json:"visibility"`
	Models     []vMusicModel `json:"models"`
}

type vMusicModel struct {
	ID           string   `json:"id"`
	OwnedBy      string   `json:"owned_by"`
	Kind         string   `json:"kind"`
	Capabilities []string `json:"capabilities"`
	Endpoints    []string `json:"endpoints"`
}

func NewVMusicPoller(reg *Registry, candidates []string, interval time.Duration) *VMusicPoller {
	clean := make([]string, 0, len(candidates))
	for _, candidate := range candidates {
		if candidate = strings.TrimRight(strings.TrimSpace(candidate), "/"); candidate != "" {
			clean = append(clean, candidate)
		}
	}
	return &VMusicPoller{reg: reg, candidates: clean, interval: interval, client: &http.Client{Timeout: 5 * time.Second}}
}

func (p *VMusicPoller) Run(ctx context.Context) {
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

func (p *VMusicPoller) pollOnce(ctx context.Context) {
	for _, candidate := range p.candidates {
		discovery, err := p.fetchDiscovery(ctx, candidate)
		if err != nil {
			continue
		}
		baseURL := candidate + "/v1"
		if p.reg.HasNonModelHubBaseURL(baseURL, vMusicUpstreamID) {
			p.reg.Delete(vMusicUpstreamID)
			return
		}
		name := strings.TrimSpace(discovery.Name)
		if name == "" {
			name = "V-Music"
		}
		u := &Upstream{ID: vMusicUpstreamID, Name: name, Kind: KindLocal, Source: SourceVMusic, BaseURL: baseURL, Protocol: ProtocolOpenAI, Visibility: vMusicVisibility(discovery.Visibility), ModelPrefix: SourceVMusic, Healthy: true}
		p.reg.Put(u)
		p.reg.SetModels(u.ID, vMusicModels(discovery.Models))
		return
	}
	p.reg.Delete(vMusicUpstreamID)
}

func vMusicVisibility(visibility string) string {
	if strings.EqualFold(strings.TrimSpace(visibility), VisibilityPrivate) {
		return VisibilityPrivate
	}
	return VisibilityPublic
}

func (p *VMusicPoller) fetchDiscovery(ctx context.Context, baseURL string) (vMusicDiscovery, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, baseURL+"/api/v1/discovery", nil)
	if err != nil {
		return vMusicDiscovery{}, err
	}
	resp, err := p.client.Do(req)
	if err != nil {
		return vMusicDiscovery{}, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return vMusicDiscovery{}, fmt.Errorf("discovery probe: status %d", resp.StatusCode)
	}
	var discovery vMusicDiscovery
	if err := json.NewDecoder(resp.Body).Decode(&discovery); err != nil {
		return vMusicDiscovery{}, err
	}
	return discovery, nil
}

func vMusicModels(rows []vMusicModel) []Model {
	models := make([]Model, 0, len(rows))
	for _, row := range rows {
		id := strings.TrimSpace(row.ID)
		endpoints := NormalizeEndpoints(row.Endpoints)
		if id == "" || len(endpoints) == 0 {
			continue
		}
		kind := strings.TrimSpace(row.Kind)
		if kind == "" {
			kind = "music"
		}
		models = append(models, Model{
			ID: id, Qualified: SourceVMusic + "/" + id, Upstream: vMusicUpstreamID,
			Kind: kind, Capabilities: MergeCapabilities(row.Capabilities, CapabilitiesFromEndpoints(endpoints)),
			Endpoints: endpoints, OwnedBy: row.OwnedBy,
		})
	}
	return models
}
