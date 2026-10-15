package registry

import (
	"errors"
	"net/url"
	"strings"
	"time"
)

var ErrRegistrationConflict = errors.New("upstream ID belongs to another source")

// Register atomically replaces a service snapshot; discovery adapters can use
// the same upstream/model representation without affecting routing counters.
func (r *Registry) Register(u Upstream, models []Model) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	if old, ok := r.upstreams[u.ID]; ok {
		if old.Source != SourceRegistered {
			return ErrRegistrationConflict
		}
		u.CreatedAt = old.CreatedAt
	}
	if u.CreatedAt.IsZero() {
		u.CreatedAt = time.Now().UTC()
	}
	if u.Source == SourceRegistered {
		for _, discovered := range r.upstreams {
			if discovered.Source == SourceVSub && live(discovered) && discovered.Owner == u.Owner && canonicalBaseURL(discovered.BaseURL) == canonicalBaseURL(u.BaseURL) {
				return nil // V-Router discovery wins; accept legacy registrations without duplicating models.
			}
		}
		r.deleteDiscoveredDuplicatesLocked(u.BaseURL)
	}
	r.upstreams[u.ID] = &u
	r.models[u.ID] = append([]Model(nil), models...)
	return nil
}
func (r *Registry) Heartbeat(id string, ttl time.Duration) (Upstream, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	u, ok := r.upstreams[id]
	if !ok || u.Source != SourceRegistered || !live(u) {
		return Upstream{}, ErrUnhealthy
	}
	u.ExpiresAt = time.Now().UTC().Add(ttl)
	delete(r.breaker, id)
	return *u, nil
}
func (r *Registry) Unregister(id string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	u, ok := r.upstreams[id]
	if !ok {
		return ErrUnknownModel
	}
	if u.Source != SourceRegistered {
		return ErrRegistrationConflict
	}
	delete(r.upstreams, id)
	delete(r.models, id)
	return nil
}

func canonicalBaseURL(raw string) string {
	raw = strings.TrimRight(strings.TrimSpace(raw), "/")
	if raw == "" {
		return ""
	}
	parsed, err := url.Parse(raw)
	if err != nil || parsed.Scheme == "" || parsed.Host == "" {
		return strings.ToLower(raw)
	}
	parsed.Scheme = strings.ToLower(parsed.Scheme)
	parsed.Host = strings.ToLower(parsed.Host)
	parsed.Path = strings.TrimRight(parsed.Path, "/")
	parsed.RawQuery = ""
	parsed.Fragment = ""
	return parsed.String()
}

func (r *Registry) HasNonModelHubBaseURL(baseURL, excludeID string) bool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	wanted := canonicalBaseURL(baseURL)
	if wanted == "" {
		return false
	}
	for id, u := range r.upstreams {
		if id == excludeID || isDiscoveredSource(u.Source) {
			continue
		}
		if canonicalBaseURL(u.BaseURL) == wanted {
			return true
		}
	}
	return false
}

func (r *Registry) deleteDiscoveredDuplicatesLocked(baseURL string) {
	wanted := canonicalBaseURL(baseURL)
	if wanted == "" {
		return
	}
	for id, u := range r.upstreams {
		if isDiscoveredSource(u.Source) && canonicalBaseURL(u.BaseURL) == wanted {
			delete(r.upstreams, id)
			delete(r.models, id)
		}
	}
}

func isDiscoveredSource(source string) bool {
	return source == SourceModelHub || source == SourceImageStudio || source == SourceVVoice || source == SourceVMusic || source == SourceMLServer
}

// SetVSubDiscovery atomically replaces one user's private snapshot and removes
// only legacy registrations for that same owner and exact inference URL.
func (r *Registry) SetVSubDiscovery(u Upstream, models []Model) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for id, old := range r.upstreams {
		if old.Source == SourceRegistered && old.Owner == u.Owner && canonicalBaseURL(old.BaseURL) == canonicalBaseURL(u.BaseURL) {
			delete(r.upstreams, id)
			delete(r.models, id)
		}
	}
	r.upstreams[u.ID] = &u
	r.models[u.ID] = models
}
