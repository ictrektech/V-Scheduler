// Package registry keeps the upstream table and the unified model list, and
// resolves requested model names to a concrete upstream.
package registry

import (
	"errors"
	"sort"
	"strings"
	"sync"
	"time"
)

// Upstream kinds describe provenance; kindPriority only orders management lists.
const (
	KindLocal      = "local"
	KindManual     = "manual"
	KindCloud      = "cloud"
	KindRegistered = "registered"
)

const (
	VisibilityPublic  = "public"
	VisibilityPrivate = "private"
)

// Sources that create upstreams. Only sourceAPICreated upstreams are persisted.
const (
	SourceModelHub        = "model-hub"
	SourceSchedulerOllama = "v-scheduler"
	SourceImageStudio     = "image-studio"
	SourceVVoice          = "v-voice"
	SourceVSub            = "v-sub"
	SourceVMusic          = "v-music"
	SourceMLServer        = "ml-server"
	SourceAPI             = "api"
	SourceRegistered      = "registered"
)

// Upstream protocols describe how V-Router talks to a provider.
const (
	ProtocolOpenAI          = "openai"
	ProtocolChatCompletions = "chat_completions"
	ProtocolAnthropic       = "anthropic"
	ProtocolOfflineASR      = "offline_asr"
	ProtocolImageStudio     = "image_studio"
	ProtocolAsyncTask       = "async_task"
	ProtocolMLServer        = "ml_server"
)

// AsyncTaskConfig describes a provider-neutral create/poll/result workflow.
// Dot-separated paths address JSON fields; ResultJSONPath handles providers
// that return the final result as a JSON-encoded string.
type AsyncTaskConfig struct {
	CreatePath     string         `json:"create_path"`
	QueryPath      string         `json:"query_path"`
	QueryTaskParam string         `json:"query_task_param"`
	TaskIDPath     string         `json:"task_id_path"`
	StatusPath     string         `json:"status_path"`
	ResultPath     string         `json:"result_path"`
	ResultJSONPath string         `json:"result_json_path,omitempty"`
	InputPath      string         `json:"input_path,omitempty"`
	SuccessStates  []string       `json:"success_states"`
	FailureStates  []string       `json:"failure_states"`
	DefaultInput   map[string]any `json:"default_input,omitempty"`
	PollIntervalMS int            `json:"poll_interval_ms"`
	TimeoutSeconds int            `json:"timeout_seconds"`
}

// Role of an inference service, mapped from Model Hub registration data.
const (
	RoleGenerate       = "generate"
	RoleEmbed          = "embed"
	RoleEmbedding      = "embedding"
	ModelKindChat      = "chat"
	ModelKindEmbedding = "embedding"
)

var kindPriority = map[string]int{
	KindLocal:      0,
	KindManual:     1,
	KindRegistered: 1,
	KindCloud:      2,
}

// SchedulerPolicy declares generic resource coordination, independent of app name.
type SchedulerPolicy struct {
	Mode                string             `json:"mode"`
	ExecutorID          string             `json:"executor_id,omitempty"`
	Completion          string             `json:"completion,omitempty"`
	WebSocketCompletion string             `json:"websocket_completion,omitempty"`
	MaxInputBytes       int64              `json:"max_input_bytes,omitempty"`
	Limits              map[string]float64 `json:"limits,omitempty"`
	SecondaryModels     []string           `json:"secondary_models,omitempty"`
	UpstreamURL         string             `json:"upstream_url,omitempty"`
}

// Upstream is one OpenAI-compatible API provider behind the gateway.
type Upstream struct {
	Scheduler   *SchedulerPolicy `json:"scheduler,omitempty"`
	ID          string           `json:"id"`
	Name        string           `json:"name"`
	Kind        string           `json:"kind"`
	Role        string           `json:"role,omitempty"`
	BaseURL     string           `json:"base_url"`               // OpenAI-compatible root including /v1
	ModelPrefix string           `json:"model_prefix,omitempty"` // public model namespace, e.g. com.ictrek.ai
	APIKey      string           `json:"api_key,omitempty"`
	Protocol    string           `json:"protocol,omitempty"` // openai (legacy native), chat_completions, anthropic, offline_asr, image_studio
	AsyncTask   *AsyncTaskConfig `json:"async_task,omitempty"`
	Visibility  string           `json:"visibility,omitempty"`
	Owner       string           `json:"owner,omitempty"`
	Healthy     bool             `json:"healthy"`
	LastError   string           `json:"last_error,omitempty"`
	Source      string           `json:"source,omitempty"`
	CreatedAt   time.Time        `json:"created_at"`
	ExpiresAt   time.Time        `json:"expires_at,omitempty"`
}

func (u Upstream) ChatOnly() bool {
	return u.Protocol == ProtocolChatCompletions || (u.Protocol == "" && u.Source == SourceModelHub)
}

func (u Upstream) Scope() string {
	scope := strings.ToLower(strings.TrimSpace(u.Visibility))
	if scope == VisibilityPrivate {
		return VisibilityPrivate
	}
	return VisibilityPublic
}

func (u Upstream) VisibleTo(owner string, admin bool) bool {
	if u.Scope() != VisibilityPrivate {
		return true
	}
	return admin || strings.TrimSpace(u.Owner) != "" && strings.TrimSpace(u.Owner) == strings.TrimSpace(owner)
}

func (u Upstream) ManageableBy(owner string, admin bool) bool {
	return admin || strings.TrimSpace(u.Owner) == "" || strings.TrimSpace(u.Owner) == strings.TrimSpace(owner)
}

// NativeModelRef strips this upstream's public namespace from a secondary
// model reference (such as V-Voice's speaker_model), so prefixed gateway
// names resolve to the native id at the forwarding boundary. Values without
// the prefix pass through unchanged.
func (u Upstream) NativeModelRef(ref string) string {
	ref = strings.TrimSpace(ref)
	prefix := u.PublicPrefix() + "/"
	if u.Source == SourceVVoice && strings.HasPrefix(ref, prefix) {
		return strings.TrimSpace(strings.TrimPrefix(ref, prefix))
	}
	return ref
}

func (u Upstream) PublicPrefix() string {
	if strings.TrimSpace(u.ModelPrefix) != "" {
		return strings.TrimSpace(u.ModelPrefix)
	}
	if u.Source == SourceModelHub {
		return SourceModelHub
	}
	if u.Source == SourceImageStudio {
		return SourceImageStudio
	}
	if u.Source == SourceVVoice {
		return SourceVVoice
	}
	if u.Source == SourceVMusic {
		return SourceVMusic
	}
	if u.Source == SourceMLServer {
		return SourceMLServer
	}
	if u.Kind != "" {
		return u.Kind
	}
	return KindManual
}

// Model is one model entry exposed through the gateway.
type Model struct {
	ID           string   `json:"id"`        // upstream-native id, e.g. qwen3.5:2b
	Qualified    string   `json:"qualified"` // <kind>/<id>, may represent multiple replicas
	Upstream     string   `json:"upstream_id"`
	Kind         string   `json:"kind"` // chat, embedding or unknown fallback
	Capabilities []string `json:"capabilities,omitempty"`
	Endpoints    []string `json:"endpoints,omitempty"` // V-Router endpoint names this model declares
	OwnedBy      string   `json:"owned_by,omitempty"`
}

// Registry is safe for concurrent use.
type Registry struct {
	mu        sync.RWMutex
	persistMu sync.Mutex
	upstreams map[string]*Upstream
	models    map[string][]Model // upstream id -> models
	services  map[string]string  // discovered non-model service id -> base URL
	statePath string
	routing   map[string]RoutingPolicy
	inflight  map[string]int
	fair      map[string]map[string]int64
	breaker   map[string]*breakerState
	resources map[string]ResourceOwnership
}

func New() *Registry {
	return &Registry{
		upstreams: map[string]*Upstream{},
		models:    map[string][]Model{},
		services:  map[string]string{},
		routing:   map[string]RoutingPolicy{}, inflight: map[string]int{}, fair: map[string]map[string]int64{},
		breaker: map[string]*breakerState{}, resources: map[string]ResourceOwnership{},
	}
}

// Put inserts or replaces an upstream identified by ID. A fresh snapshot is
// authoritative for health, so any open breaker for the ID resets.
func (r *Registry) Put(u *Upstream) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if u.CreatedAt.IsZero() {
		u.CreatedAt = time.Now().UTC()
	}
	cp := *u
	r.upstreams[u.ID] = &cp
	delete(r.breaker, u.ID)
}

// SetModels replaces the model list of an upstream.
func (r *Registry) SetModels(upstreamID string, models []Model) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.models[upstreamID] = models
}

// ModelsOf returns copies of the models registered for one upstream.
func (r *Registry) ModelsOf(upstreamID string) []Model {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return append([]Model(nil), r.models[upstreamID]...)
}

// Delete removes an upstream, its models, its breaker state and any
// resource ownership recorded against it.
func (r *Registry) Delete(upstreamID string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	delete(r.upstreams, upstreamID)
	delete(r.models, upstreamID)
	delete(r.breaker, upstreamID)
	r.deleteResourcesOfUpstreamLocked(upstreamID)
}

// Get returns a copy of one upstream.
func (r *Registry) Get(upstreamID string) (Upstream, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	u, ok := r.upstreams[upstreamID]
	if !ok {
		return Upstream{}, false
	}
	cp := *u
	cp.Healthy = live(u)
	return cp, true
}

// List returns copies of all upstreams sorted by kind priority then id.
func (r *Registry) List() []Upstream {
	return r.ListFor("", true)
}

func (r *Registry) ListFor(owner string, admin bool) []Upstream {
	r.mu.RLock()
	defer r.mu.RUnlock()
	out := make([]Upstream, 0, len(r.upstreams))
	for _, u := range r.upstreams {
		if !u.VisibleTo(owner, admin) {
			continue
		}
		cp := *u
		cp.Healthy = live(u)
		out = append(out, cp)
	}
	sort.Slice(out, func(i, j int) bool {
		pi, pj := kindPriority[out[i].Kind], kindPriority[out[j].Kind]
		if pi != pj {
			return pi < pj
		}
		return out[i].ID < out[j].ID
	})
	return out
}

// DeleteBySource removes all upstreams created by the given source, e.g.
// before a Model Hub resync.
func (r *Registry) DeleteBySource(source string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for id, u := range r.upstreams {
		if u.Source == source {
			delete(r.upstreams, id)
			delete(r.models, id)
		}
	}
}

// ListModels returns the merged model list, deduplicated by qualified id.
// Model Hub Ollama instances share a model directory, so the same qualified
// model regularly appears on several local upstreams and must be listed once.
// Only qualified names are exposed; callers must choose an upstream namespace
// such as model-hub/, registered/ or a cloud domain prefix.
// Unhealthy upstreams are excluded.
func (r *Registry) ListModels() []Model {
	return r.ListModelsFor("", true)
}

func (r *Registry) ListModelsFor(owner string, admin bool) []Model {
	r.mu.RLock()
	defer r.mu.RUnlock()

	byQualified := map[string][]Model{}
	for _, u := range r.upstreams {
		if !u.VisibleTo(owner, admin) || !live(u) || r.routing[u.ID].Disabled {
			continue
		}
		for _, m := range r.models[u.ID] {
			byQualified[m.Qualified] = append(byQualified[m.Qualified], m)
		}
	}

	out := make([]Model, 0, len(byQualified))
	for _, variants := range byQualified {
		// Deterministic representative: a shared model directory lists the
		// same model on several upstreams with per-service kinds (a qa-role
		// instance labels everything chat, an embedding instance labels
		// everything embedding). Prefer the chat-kind row so dialogue
		// surfaces stay stable, then the lexicographically smallest
		// upstream — never depend on map iteration order.
		rep := variants[0]
		for _, m := range variants[1:] {
			if betterModelRepresentative(r.upstreams, m, rep) {
				rep = m
			}
		}
		out = append(out, rep)
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Qualified != out[j].Qualified {
			return out[i].Qualified < out[j].Qualified
		}
		return out[i].ID < out[j].ID
	})
	return out
}

func betterModelRepresentative(upstreams map[string]*Upstream, candidate, current Model) bool {
	candidateScore := modelRepresentativeScore(upstreams[candidate.Upstream], candidate)
	currentScore := modelRepresentativeScore(upstreams[current.Upstream], current)
	if candidateScore != currentScore {
		return candidateScore < currentScore
	}
	if candidate.Kind != current.Kind {
		return candidate.Kind == ModelKindChat
	}
	return candidate.Upstream < current.Upstream
}

func modelRepresentativeScore(u *Upstream, m Model) int {
	if u == nil {
		return 50
	}
	role := strings.ToLower(u.Role)
	switch m.Kind {
	case ModelKindChat:
		if role == RoleGenerate || role == "" {
			return 0
		}
		if role == RoleEmbed || role == RoleEmbedding {
			return 20
		}
	case ModelKindEmbedding:
		if role == RoleEmbed || role == RoleEmbedding {
			return 0
		}
		if role == RoleGenerate {
			return 20
		}
	}
	return 10
}

var (
	ErrUnknownModel = errors.New("unknown model")
	ErrUnhealthy    = errors.New("model upstream is unhealthy")
)

// DuplicateModels reports model ids served by more than one healthy upstream
// of the same kind, for UI disambiguation hints.
func (r *Registry) DuplicateModels() map[string][]string {
	r.mu.RLock()
	defer r.mu.RUnlock()
	owners := map[string][]string{}
	for _, u := range r.upstreams {
		if !live(u) || r.routing[u.ID].Disabled {
			continue
		}
		for _, m := range r.models[u.ID] {
			key := u.PublicPrefix() + "/" + m.ID
			owners[key] = append(owners[key], u.ID)
		}
	}
	dup := map[string][]string{}
	for k, ids := range owners {
		if len(ids) > 1 {
			sort.Strings(ids)
			dup[k] = ids
		}
	}
	return dup
}
