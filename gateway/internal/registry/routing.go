package registry

import (
	"errors"
	"fmt"
	"sort"
	"strings"
	"sync"
	"time"
)

var ErrBusy = errors.New("all matching upstreams are at their concurrency limit")
var ErrUnsupported = errors.New("no matching upstream supports this endpoint")

// RoutingPolicy applies to every source. Zero values preserve old state files.
// MaxInflight limits this gateway's concurrent requests, not GPU slots.
type RoutingPolicy struct {
	Weight      int  `json:"weight"`
	MaxInflight int  `json:"max_inflight"`
	Disabled    bool `json:"disabled"`
}

func (p RoutingPolicy) normalized() RoutingPolicy {
	if p.Weight == 0 {
		p.Weight = 1
	}
	return p
}
func (p RoutingPolicy) Validate() error {
	if p.Weight < 0 || p.Weight > 1000 || p.MaxInflight < 0 || p.MaxInflight > 100000 {
		return errors.New("weight must be 1..1000 (0 defaults to 1); max_inflight must be 0..100000")
	}
	return nil
}
func (r *Registry) SetRouting(id string, p RoutingPolicy) error {
	if err := p.Validate(); err != nil {
		return err
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, ok := r.upstreams[id]; !ok {
		return ErrUnknownModel
	}
	r.routing[id] = p.normalized()
	return nil
}
func (r *Registry) Routing(id string) (RoutingPolicy, int) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.routing[id].normalized(), r.inflight[id]
}
func live(u *Upstream) bool {
	return u.Healthy && (u.ExpiresAt.IsZero() || time.Now().Before(u.ExpiresAt))
}
func supports(u *Upstream, m Model, path string) bool {
	// Builtin Ollama instances share the model directory. Model availability
	// must not let rerank calls consume QA/embedding slots (or vice versa).
	if u.Source == SourceModelHub {
		role := strings.ToLower(strings.TrimSpace(u.Role))
		if role == "rerank" && path != "/rerank" {
			return false
		}
		if path == "/rerank" && (role == RoleGenerate || role == RoleEmbed || role == RoleEmbedding) {
			return false
		}
	}
	if path == "/messages" {
		if !u.ChatOnly() && u.Protocol != ProtocolAnthropic && u.Protocol != ProtocolOfflineASR {
			return false
		}
	} else if u.Protocol == ProtocolAnthropic {
		return false
	}
	switch path {
	case "/chat/completions", "/responses", "/messages", "/completions":
		if u.Source == SourceModelHub && (strings.EqualFold(u.Role, RoleEmbedding) || strings.EqualFold(u.Role, RoleEmbed)) {
			return false
		}
	case "/embeddings":
		if u.Source == SourceModelHub && strings.EqualFold(u.Role, RoleGenerate) {
			return false
		}
	}
	if len(m.Endpoints) > 0 {
		return endpointAllowsPath(u, m, path)
	}
	if HasKnownCapability(m.Capabilities) {
		return capabilityAllowsPath(m, path)
	}
	switch path {
	case "/chat/completions", "/responses", "/messages", "/completions":
		return m.Kind != ModelKindEmbedding
	case "/embeddings":
		return m.Kind != ModelKindChat
	default:
		return !u.ChatOnly()
	}
}

func endpointAllowsPath(u *Upstream, m Model, path string) bool {
	wanted := NormalizeEndpoint(path)
	for _, endpoint := range NormalizeEndpoints(m.Endpoints) {
		if endpoint == wanted {
			return true
		}
		if endpoint == "chat_completions" {
			switch path {
			case "/chat/completions":
				return true
			case "/responses", "/messages":
				if u.ChatOnly() {
					return true
				}
			}
		}
	}
	return false
}

func capabilityAllowsPath(m Model, path string) bool {
	switch path {
	case "/chat/completions", "/responses", "/messages", "/completions":
		return modelHasAnyCapability(m, "chat", "completion", "completions", "generate", "generation", "tools", "thinking")
	case "/embeddings":
		return modelHasAnyCapability(m, "embedding", "embeddings")
	case "/images/generations":
		return modelHasAnyCapability(m, "image", "images", "image_generation")
	case "/images/edits":
		return modelHasAnyCapability(m, "image_edit")
	case "/images/variations":
		return modelHasAnyCapability(m, "image_variation")
	case "/videos/generations":
		return modelHasAnyCapability(m, "video", "videos", "video_generation")
	case "/audio/speech":
		return modelHasAnyCapability(m, "tts", "speech")
	case "/audio/transcriptions":
		return modelHasAnyCapability(m, "asr", "transcription", "transcriptions")
	case "/audio/translations":
		return modelHasAnyCapability(m, "translation", "translations")
	case "/audio/music/generations":
		return modelHasAnyCapability(m, "music", "text-to-music", "instrumental")
	case "/moderations":
		return modelHasAnyCapability(m, "moderation", "moderations")
	case "/rerank":
		return modelHasAnyCapability(m, "rerank", "reranking")
	case "/ml/predictions":
		return modelHasAnyCapability(m, "vision", "image_embedding", "face_recognition", "person_reid", "ocr")
	case "/ml/video/structure", "/ml/video/structure/tasks":
		return modelHasAnyCapability(m, "video_structure")
	}
	return false
}

func modelHasAnyCapability(m Model, names ...string) bool {
	for _, cap := range m.Capabilities {
		normalized := strings.ToLower(strings.TrimSpace(cap))
		for _, name := range names {
			if normalized == name {
				return true
			}
		}
	}
	return false
}

// Acquire atomically chooses and reserves an upstream until release is called.
// Only automatic selection needs a discovered model: pinned native APIs may
// legitimately use provider-default or unlisted models.
func (r *Registry) Acquire(requested, path, pinned string) (Upstream, string, func(), error) {
	return r.AcquireFor(requested, path, pinned, "", true)
}

func (r *Registry) AcquireFor(requested, path, pinned, owner string, admin bool) (Upstream, string, func(), error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if pinned != "" {
		u, ok := r.upstreams[pinned]
		if !ok || !u.VisibleTo(owner, admin) {
			return Upstream{}, "", nil, ErrUnknownModel
		}
		if !live(u) || r.routing[pinned].Disabled {
			return Upstream{}, "", nil, ErrUnhealthy
		}
		p := r.routing[pinned]
		if p.MaxInflight > 0 && r.inflight[pinned] >= p.MaxInflight {
			return Upstream{}, "", nil, ErrBusy
		}
		return *u, requested, r.reserveLocked(pinned), nil
	}
	ids := make([]string, 0, len(r.upstreams))
	for id := range r.upstreams {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	type candidate struct {
		upstream *Upstream
		native   string
	}
	var choices []candidate
	found, healthy, compatible := false, false, false
	for _, id := range ids {
		u := r.upstreams[id]
		if !u.VisibleTo(owner, admin) {
			continue
		}
		p := r.routing[id].normalized()
		for _, m := range r.models[id] {
			if requested != m.Qualified {
				continue
			}
			found = true
			// A cooling upstream counts as unhealthy: it stays excluded
			// until its cooldown elapses, then re-enters and the request
			// itself acts as the recovery probe.
			if !live(u) || p.Disabled || r.coolingLocked(id) {
				continue
			}
			healthy = true
			if !supports(u, m, path) {
				continue
			}
			compatible = true
			if p.MaxInflight > 0 && r.inflight[id] >= p.MaxInflight {
				continue
			}
			comparison := 0
			if len(choices) > 0 {
				comparison = r.compareLoad(id, choices[0].upstream.ID)
			}
			if len(choices) == 0 || comparison < 0 {
				choices = []candidate{{u, m.ID}}
			} else if comparison == 0 {
				choices = append(choices, candidate{u, m.ID})
			}
			break
		}
	}
	if len(choices) > 0 {
		// Smooth weighted round-robin only breaks equal-load ties within this
		// request pool; pinned calls and unrelated models cannot bias it.
		pool := path + "\x00" + requested
		if r.fair[pool] == nil {
			r.fair[pool] = map[string]int64{}
		}
		scores := r.fair[pool]
		best := choices[0]
		total := int64(0)
		for _, c := range choices {
			weight := int64(r.routing[c.upstream.ID].normalized().Weight)
			scores[c.upstream.ID] += weight
			total += weight
		}
		for _, c := range choices {
			if scores[c.upstream.ID] > scores[best.upstream.ID] {
				best = c
			}
		}
		scores[best.upstream.ID] -= total
		return *best.upstream, best.native, r.reserveLocked(best.upstream.ID), nil
	}
	err := ErrUnknownModel
	if found {
		err = ErrUnhealthy
	}
	if healthy {
		err = ErrUnsupported
	}
	if compatible {
		err = ErrBusy
	}
	return Upstream{}, "", nil, fmt.Errorf("%w: %s", err, requested)
}
func (r *Registry) compareLoad(a, b string) int {
	wa, wb := int64(r.routing[a].normalized().Weight), int64(r.routing[b].normalized().Weight)
	la, lb := int64(r.inflight[a])*wb, int64(r.inflight[b])*wa
	if la < lb {
		return -1
	}
	if la > lb {
		return 1
	}
	return 0
}
func (r *Registry) reserveLocked(id string) func() {
	r.inflight[id]++
	var once sync.Once
	return func() { once.Do(func() { r.mu.Lock(); defer r.mu.Unlock(); r.inflight[id]-- }) }
}
