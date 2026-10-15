package registry

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"regexp"
	"strings"
	"sync"
	"time"
)

// V-Sub models belong to individual VOS users. Pull with an already verified
// caller token, cache briefly, and never retain that token or expose models to
// anonymous clients. Absent/old/unreachable V-Sub is a quiet empty snapshot.
type VSubDiscovery struct {
	reg        *Registry
	candidates []string
	client     *http.Client
	mu         sync.Mutex
	next       map[string]time.Time
}

func NewVSubDiscovery(reg *Registry, candidates []string) *VSubDiscovery {
	return &VSubDiscovery{reg: reg, candidates: candidates, client: &http.Client{Timeout: 3 * time.Second}, next: map[string]time.Time{}}
}

var vSubBasePath = regexp.MustCompile(`^/inference/[a-f0-9]{64}/v1$`)

func (p *VSubDiscovery) Refresh(ctx context.Context, token, owner string) {
	if token == "" || owner == "" {
		return
	}
	p.mu.Lock()
	if time.Now().Before(p.next[owner]) {
		p.mu.Unlock()
		return
	}
	p.next[owner] = time.Now().Add(30 * time.Second)
	p.mu.Unlock()
	hash := sha256.Sum256([]byte(owner))
	id := "app-v-sub-" + hex.EncodeToString(hash[:])
	for _, candidate := range p.candidates {
		candidate = strings.TrimRight(strings.TrimSpace(candidate), "/")
		if candidate == "" {
			continue
		}
		req, err := http.NewRequestWithContext(ctx, http.MethodGet, candidate+"/api/v1/discovery", nil)
		if err != nil {
			continue
		}
		req.Header.Set("Authorization", "Bearer "+token)
		resp, err := p.client.Do(req)
		if err != nil {
			continue
		}
		var doc struct {
			Owner    string        `json:"owner"`
			BasePath string        `json:"base_path"`
			APIKey   string        `json:"api_key"`
			Models   []vVoiceModel `json:"models"`
		}
		err = json.NewDecoder(io.LimitReader(resp.Body, 4<<20)).Decode(&doc)
		resp.Body.Close()
		if resp.StatusCode != http.StatusOK || err != nil || doc.Owner != owner {
			continue
		}
		if len(doc.Models) == 0 {
			p.reg.Delete(id)
			return
		}
		if !vSubBasePath.MatchString(doc.BasePath) || doc.APIKey == "" {
			continue
		}
		u := Upstream{ID: id, Name: "V-Sub local models", Source: SourceVSub, Kind: KindLocal, Owner: owner, Visibility: VisibilityPrivate, Protocol: ProtocolOpenAI, ModelPrefix: SourceVSub, BaseURL: candidate + doc.BasePath, APIKey: doc.APIKey, Healthy: true, ExpiresAt: time.Now().Add(90 * time.Second)}
		models := vVoiceModels(doc.Models)
		for i := range models {
			models[i].Upstream = id
			models[i].Qualified = SourceVSub + "/" + models[i].ID
		}
		p.reg.SetVSubDiscovery(u, models)
		return
	}
	p.reg.Delete(id)
}
