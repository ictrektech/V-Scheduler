package registry

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"
)

const mlServerUpstreamID = "app-ml-server"

// MLServerPoller discovers the same-VOS Immich ML Server. It deliberately
// validates the service identity and advertised OpenAPI paths instead of
// assuming that any service listening on port 3003 is compatible.
type MLServerPoller struct {
	reg        *Registry
	candidates []string
	interval   time.Duration
	client     *http.Client
}

type mlServerInfo struct {
	Service string `json:"service"`
	Version string `json:"version"`
	Status  string `json:"status"`
}

type mlServerOpenAPI struct {
	Paths map[string]json.RawMessage `json:"paths"`
}

func NewMLServerPoller(reg *Registry, candidates []string, interval time.Duration) *MLServerPoller {
	clean := make([]string, 0, len(candidates))
	for _, candidate := range candidates {
		if candidate = strings.TrimRight(strings.TrimSpace(candidate), "/"); candidate != "" {
			clean = append(clean, candidate)
		}
	}
	return &MLServerPoller{reg: reg, candidates: clean, interval: interval, client: &http.Client{Timeout: 5 * time.Second}}
}

func (p *MLServerPoller) Run(ctx context.Context) {
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

func (p *MLServerPoller) pollOnce(ctx context.Context) {
	for _, candidate := range p.candidates {
		info, paths, err := p.discover(ctx, candidate)
		if err != nil {
			continue
		}
		if p.reg.HasNonModelHubBaseURL(candidate, mlServerUpstreamID) {
			p.reg.Delete(mlServerUpstreamID)
			return
		}
		u := &Upstream{ID: mlServerUpstreamID, Name: "ML Server", Kind: KindLocal, Source: SourceMLServer, BaseURL: candidate, Protocol: ProtocolMLServer, Visibility: VisibilityPublic, ModelPrefix: SourceMLServer, Healthy: true}
		p.reg.Put(u)
		p.reg.SetModels(u.ID, mlServerModels(paths, info.Version))
		return
	}
	p.reg.Delete(mlServerUpstreamID)
}

func (p *MLServerPoller) discover(ctx context.Context, baseURL string) (mlServerInfo, map[string]json.RawMessage, error) {
	var info mlServerInfo
	if err := p.getJSON(ctx, baseURL+"/", &info); err != nil {
		return info, nil, err
	}
	if info.Service != "immich-ml-server" || !strings.EqualFold(info.Status, "running") {
		return info, nil, fmt.Errorf("unexpected service identity or status")
	}
	var spec mlServerOpenAPI
	if err := p.getJSON(ctx, baseURL+"/openapi.json", &spec); err != nil {
		return info, nil, err
	}
	return info, spec.Paths, nil
}

func (p *MLServerPoller) getJSON(ctx context.Context, target string, dst any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, target, nil)
	if err != nil {
		return err
	}
	resp, err := p.client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("probe %s: status %d", target, resp.StatusCode)
	}
	return json.NewDecoder(resp.Body).Decode(dst)
}

func mlServerModels(paths map[string]json.RawMessage, version string) []Model {
	var models []Model
	add := func(id, kind string, caps, endpoints []string) {
		models = append(models, Model{ID: id, Qualified: SourceMLServer + "/" + id, Upstream: mlServerUpstreamID, Kind: kind, Capabilities: caps, Endpoints: endpoints, OwnedBy: "immich-ml-server"})
	}
	if _, ok := paths["/predict"]; ok {
		add("qwen3-embedding-0.6b", ModelKindEmbedding, []string{"embedding"}, []string{"embeddings"})
		add("ViT-B-16-SigLIP2__webli", ModelKindEmbedding, []string{"embedding", "image_embedding", "vision"}, []string{"embeddings", "ml/predictions"})
		add("buffalo_l", "vision", []string{"face_recognition", "vision"}, []string{"ml/predictions"})
		add("osnet_ain_x1_0", "vision", []string{"person_reid", "vision"}, []string{"ml/predictions"})
		add("PP-OCRv5_server", "vision", []string{"ocr", "vision"}, []string{"ml/predictions"})
	}
	_, create := paths["/api/v1/video/structure"]
	_, query := paths["/api/v1/video/structure/tasks/{task_id}"]
	if create && query {
		add("video-structure", "video", []string{"video_structure"}, []string{"ml/video/structure", "ml/video/structure/tasks"})
	}
	return models
}
