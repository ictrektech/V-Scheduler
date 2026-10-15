package registry

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"time"
)

type VideoStructurePoller struct {
	reg                 *Registry
	detectCandidates    []string
	structureCandidates []string
	interval            time.Duration
	client              *http.Client
}

func NewVideoStructurePoller(reg *Registry, detectCandidates, structureCandidates []string, interval time.Duration) *VideoStructurePoller {
	return &VideoStructurePoller{
		reg:                 reg,
		detectCandidates:    cleanCandidates(detectCandidates),
		structureCandidates: cleanCandidates(structureCandidates),
		interval:            interval,
		client:              &http.Client{Timeout: 5 * time.Second},
	}
}

func cleanCandidates(candidates []string) []string {
	clean := make([]string, 0, len(candidates))
	for _, candidate := range candidates {
		if candidate = strings.TrimRight(strings.TrimSpace(candidate), "/"); candidate != "" {
			clean = append(clean, candidate)
		}
	}
	return clean
}

func (p *VideoStructurePoller) Run(ctx context.Context) {
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

func (p *VideoStructurePoller) pollOnce(ctx context.Context) {
	p.reg.SetService(ServiceVideoStructureDetect, p.firstHealthy(ctx, p.detectCandidates, "scene-detection"))
	p.reg.SetService(ServiceVideoStructureStructure, p.firstHealthy(ctx, p.structureCandidates, "structure"))
}

func (p *VideoStructurePoller) firstHealthy(ctx context.Context, candidates []string, wantService string) string {
	for _, candidate := range candidates {
		req, err := http.NewRequestWithContext(ctx, http.MethodGet, candidate+"/api/health", nil)
		if err != nil {
			continue
		}
		resp, err := p.client.Do(req)
		if err != nil {
			continue
		}
		var payload struct {
			Code int `json:"code"`
			Data struct {
				Status  string `json:"status"`
				Service string `json:"service"`
			} `json:"data"`
		}
		decodeErr := json.NewDecoder(resp.Body).Decode(&payload)
		_ = resp.Body.Close()
		if resp.StatusCode == http.StatusOK && decodeErr == nil && payload.Code == 0 && payload.Data.Status == "ok" && payload.Data.Service == wantService {
			return candidate
		}
	}
	return ""
}
