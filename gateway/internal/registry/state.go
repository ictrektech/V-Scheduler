package registry

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
)

// stateFile is the on-disk representation. Only manually added upstreams are
// persisted; discovered upstreams are rebuilt by polling on startup.
// Resource ownership rows cover every upstream and survive restarts.
type stateFile struct {
	Routing   map[string]RoutingPolicy `json:"routing,omitempty"`
	Version   int                      `json:"version"`
	Upstreams []Upstream               `json:"upstreams"`
	Models    map[string][]Model       `json:"models"`
	Resources []ResourceOwnership      `json:"resources,omitempty"`
}

// Open loads persisted state from path and returns a registry. An empty or
// missing file yields an empty registry.
func Open(path string) (*Registry, error) {
	r := New()
	r.statePath = path
	data, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return r, nil
		}
		return nil, err
	}
	var st stateFile
	if err := json.Unmarshal(data, &st); err != nil {
		return nil, err
	}
	for id, p := range st.Routing {
		if err := p.Validate(); err != nil {
			return nil, err
		}
		r.routing[id] = p.normalized()
	}
	for i := range st.Upstreams {
		u := st.Upstreams[i]
		r.upstreams[u.ID] = &u
	}
	for id, models := range st.Models {
		r.models[id] = models
	}
	for _, rec := range st.Resources {
		if rec.Upstream == "" || rec.Family == "" || rec.ResourceID == "" || rec.Owner == "" {
			continue
		}
		r.resources[resourceKey(rec.Upstream, rec.Family, rec.ResourceID)] = rec
	}
	return r, nil
}

// Persist writes API-created upstreams and their models to disk atomically.
// Discovered upstreams are intentionally skipped.
func (r *Registry) Persist() error {
	r.persistMu.Lock()
	defer r.persistMu.Unlock()
	if r.statePath == "" {
		return nil
	}
	r.mu.RLock()
	st := stateFile{Version: 2, Models: map[string][]Model{}, Routing: map[string]RoutingPolicy{}}
	for id, p := range r.routing {
		st.Routing[id] = p
	}
	for _, rec := range r.resources {
		st.Resources = append(st.Resources, rec)
	}
	for _, u := range r.upstreams {
		if u.Source == SourceAPI {
			st.Upstreams = append(st.Upstreams, *u)
			if models := r.models[u.ID]; len(models) > 0 {
				st.Models[u.ID] = models
			}
		}
	}
	r.mu.RUnlock()

	data, err := json.MarshalIndent(&st, "", "  ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(r.statePath), 0o755); err != nil {
		return err
	}
	tmp := r.statePath + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, r.statePath)
}
