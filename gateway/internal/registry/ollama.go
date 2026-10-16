package registry

import (
	"context"
	"net/http"
	"strings"
	"time"
)

// SchedulerOllama connects the three built-in backends directly. It never
// queries Model Hub or depends on a service-registration round trip.
type SchedulerOllama struct {
	reg      *Registry
	services []modelHubService
	interval time.Duration
	client   *http.Client
}

func NewSchedulerOllama(reg *Registry, qa, embedding, rerank string, interval time.Duration) *SchedulerOllama {
	p := &SchedulerOllama{reg: reg, interval: interval, client: &http.Client{Timeout: 10 * time.Second}}
	for _, entry := range []struct{ role, url string }{{"qa", qa}, {"embedding", embedding}, {"rerank", rerank}} {
		base := strings.TrimRight(strings.TrimSpace(entry.url), "/")
		if base == "" {
			continue
		}
		role := entry.role
		if role == "qa" {
			role = RoleGenerate
		}
		p.services = append(p.services, modelHubService{ServiceID: entry.role, Name: "v-scheduler/ollama-" + entry.role, Kind: "ollama", Role: &role, BaseURL: base, GatewayURL: base})
	}
	return p
}
func (p *SchedulerOllama) Run(ctx context.Context) {
	if p.interval <= 0 {
		p.interval = 30 * time.Second
	}
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
func (p *SchedulerOllama) pollOnce(ctx context.Context) {
	fetcher := &ModelHubPoller{client: p.client}
	for _, svc := range p.services {
		id := svc.Name
		models, err := fetcher.fetchModels(ctx, svc)
		protocol := ProtocolOpenAI
		if svc.roleString() == RoleGenerate {
			protocol = ProtocolChatCompletions
		}
		upstream := &Upstream{ID: id, Name: id, Kind: KindLocal, Role: svc.roleString(), Source: SourceSchedulerOllama, BaseURL: svc.openAIBaseURL(), ModelPrefix: id, Protocol: protocol, Healthy: err == nil}
		if err != nil {
			upstream.LastError = "built-in Ollama unavailable"
		}
		p.reg.Put(upstream)
		if err == nil {
			path := "/chat/completions"
			if svc.roleString() == RoleEmbedding {
				path = "/embeddings"
			} else if svc.roleString() == "rerank" {
				path = "/rerank"
			}
			compatible := models[:0]
			for _, model := range models {
				if !HasKnownCapability(model.Capabilities) {
					continue
				}
				if svc.roleString() != "rerank" && modelHasAnyCapability(model, "rerank", "reranking") {
					continue
				}
				if supports(upstream, model, path) {
					compatible = append(compatible, model)
				}
			}
			models = compatible
			for i := range models {
				models[i].Upstream = id
				models[i].Qualified = id + "/" + models[i].ID
			}
			p.reg.SetModels(id, models)
		}
	}
}
