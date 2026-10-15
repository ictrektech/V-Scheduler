package registry

import "strings"

const (
	ServiceVideoStructureDetect    = "video-structure-detect"
	ServiceVideoStructureStructure = "video-structure-structure"
)

// SetService records an ephemeral non-model service discovered on this VOS.
// An empty URL removes it; discovered services are intentionally not persisted.
func (r *Registry) SetService(id, baseURL string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	baseURL = strings.TrimRight(strings.TrimSpace(baseURL), "/")
	if baseURL == "" {
		delete(r.services, id)
		return
	}
	r.services[id] = baseURL
}

func (r *Registry) Service(id string) (string, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	baseURL, ok := r.services[id]
	return baseURL, ok
}
