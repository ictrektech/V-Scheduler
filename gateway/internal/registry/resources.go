package registry

import (
	"strings"
	"time"
)

// ResourceOwnership records who created a provider-side resource (file,
// batch, video task, vector store, ...) through this gateway, so later
// single-resource access can be restricted to the creator and admins.
// Resources created outside V-Router stay unrestricted, as before.
type ResourceOwnership struct {
	Upstream   string    `json:"upstream"`
	Family     string    `json:"family"`
	ResourceID string    `json:"resource_id"`
	Owner      string    `json:"owner"`
	CreatedAt  time.Time `json:"created_at"`
}

// maxResourceRecords bounds the persisted ownership table; the oldest
// entries are evicted first. 20k covers every realistic VOS install while
// keeping the state file small.
const maxResourceRecords = 20000

func resourceKey(upstreamID, family, resourceID string) string {
	return upstreamID + "\x00" + family + "\x00" + resourceID
}

// RecordResource stores the creator of one resource. Anonymous internal
// callers (empty owner) are not recorded: their resources keep the historic
// shared behavior.
func (r *Registry) RecordResource(upstreamID, family, resourceID, owner string) bool {
	upstreamID, family, resourceID, owner = strings.TrimSpace(upstreamID), strings.TrimSpace(family), strings.TrimSpace(resourceID), strings.TrimSpace(owner)
	if upstreamID == "" || family == "" || resourceID == "" || owner == "" {
		return false
	}
	r.mu.Lock()
	r.resources[resourceKey(upstreamID, family, resourceID)] = ResourceOwnership{
		Upstream: upstreamID, Family: family, ResourceID: resourceID, Owner: owner, CreatedAt: time.Now().UTC(),
	}
	if len(r.resources) > maxResourceRecords {
		var oldestKey string
		var oldest time.Time
		for key, rec := range r.resources {
			if oldest.IsZero() || rec.CreatedAt.Before(oldest) {
				oldest, oldestKey = rec.CreatedAt, key
			}
		}
		delete(r.resources, oldestKey)
	}
	r.mu.Unlock()
	return true
}

// ResourceOwner returns the recorded creator of one resource.
func (r *Registry) ResourceOwner(upstreamID, family, resourceID string) (string, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	rec, ok := r.resources[resourceKey(strings.TrimSpace(upstreamID), strings.TrimSpace(family), strings.TrimSpace(resourceID))]
	if !ok {
		return "", false
	}
	return rec.Owner, true
}

func (r *Registry) deleteResourcesOfUpstreamLocked(upstreamID string) {
	for key, rec := range r.resources {
		if rec.Upstream == upstreamID {
			delete(r.resources, key)
		}
	}
}
