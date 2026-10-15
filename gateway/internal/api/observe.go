package api

import (
	"bytes"
	"encoding/json"
	"net/http"
	"strings"

	"errors"
	"github.com/ictrektech/v-router/internal/proxy"
	"github.com/ictrektech/v-router/internal/registry"
)

// breakerObserver tracks one upstream round trip: observe receives
// transport-level errors from the proxy layer, done records success when no
// error was seen. Client-side cancellations count neither way: the upstream
// outcome is unknown, and aborting requests must not trip the breaker.
type breakerObserver struct {
	reg      *registry.Registry
	id       string
	canceled func() error
	failed   bool
	skipped  bool
}

func (s *Server) newBreakerObserver(r *http.Request, upstreamID string) *breakerObserver {
	return &breakerObserver{reg: s.reg, id: upstreamID, canceled: r.Context().Err}
}

func (b *breakerObserver) observe(err error) {
	if errors.Is(err, proxy.ErrSchedulerAdmission) {
		b.skipped = true
		return
	}
	if err == nil || b.failed || b.canceled() != nil {
		return
	}
	b.failed = true
	b.reg.RecordResult(b.id, false)
}

func (b *breakerObserver) done() {
	if b.skipped || b.failed || b.canceled() != nil {
		return
	}
	b.reg.RecordResult(b.id, true)
}

// resourceFamilies are the pinned provider API families whose single-resource
// access is ownership-checked when the resource was created through V-Router.
var resourceFamilies = map[string]bool{
	"files": true, "uploads": true, "batches": true, "videos": true,
	"fine_tuning": true, "vector_stores": true, "containers": true,
	"assistants": true, "threads": true, "conversations": true, "responses": true, "skills": true,
}

type resourceRef struct{ family, id string }

// resourceRefOf decomposes /v1/<family>/<resource-id>[/...]; the bare family
// path (no id segment) is a create, not a resource access.
func resourceRefOf(path string) (resourceRef, bool) {
	rest := strings.TrimPrefix(path, "/v1/")
	if rest == path {
		return resourceRef{}, false
	}
	segs := strings.Split(rest, "/")
	if len(segs) < 2 || segs[0] == "" || segs[1] == "" {
		return resourceRef{}, false
	}
	if !resourceFamilies[segs[0]] {
		return resourceRef{}, false
	}
	return resourceRef{family: segs[0], id: segs[1]}, true
}

// enforceResourceOwner rejects single-resource access with 404 when the
// resource was created through V-Router by a different non-admin caller.
// Returns true when the request may proceed.
func (s *Server) enforceResourceOwner(w http.ResponseWriter, r *http.Request, u registry.Upstream) bool {
	ref, ok := resourceRefOf(r.URL.Path)
	if !ok {
		return true
	}
	owner, recorded := s.reg.ResourceOwner(u.ID, ref.family, ref.id)
	if !recorded {
		return true
	}
	ident := identityFrom(r)
	if ident.Admin || owner == "" || owner == ident.ownerKey() {
		return true
	}
	writeRequestError(w, r, http.StatusNotFound, "unknown resource")
	return false
}

// recordCreatedResource parses a 2xx JSON response to a bare /v1/<family>
// POST and stores the creator of the returned id, protecting later
// cross-account access to that resource.
func (s *Server) recordCreatedResource(rec *statusRecorder, r *http.Request, u registry.Upstream) {
	if r.Method != http.MethodPost || rec.status < 200 || rec.status >= 300 {
		return
	}
	rest := strings.TrimPrefix(r.URL.Path, "/v1/")
	if rest == "" || strings.Contains(rest, "/") || !resourceFamilies[rest] {
		return
	}
	if id := parseResourceID(rec.observer.tail); id != "" {
		s.storeResourceOwner(r, u.ID, rest, id)
	}
}

// recordModelCreatedResource does the same for model-routed creates that
// return a task id, currently POST /v1/videos/generations.
func (s *Server) recordModelCreatedResource(rec *statusRecorder, r *http.Request, upstreamID, pathSuffix string) {
	if pathSuffix != "/videos/generations" || rec.status < 200 || rec.status >= 300 {
		return
	}
	if id := parseResourceID(rec.observer.tail); id != "" {
		s.storeResourceOwner(r, upstreamID, "videos", id)
	}
}

func (s *Server) storeResourceOwner(r *http.Request, upstreamID, family, id string) {
	owner := identityFrom(r).ownerKey()
	if !s.reg.RecordResource(upstreamID, family, id, owner) {
		return
	}
	if err := s.reg.Persist(); err != nil {
		// Ownership is best-effort persistence: the in-memory record already
		// guards this process; the next successful create rewrites the file.
		s.reg.RecordResource(upstreamID, family, id, owner)
	}
}

// parseResourceID extracts a top-level string id from a captured JSON
// response tail. SSE bodies do not parse and yield no id, so streamed
// creates stay unrecorded.
func parseResourceID(tail []byte) string {
	var doc struct {
		ID string `json:"id"`
	}
	if err := json.Unmarshal(tail, &doc); err == nil && doc.ID != "" {
		return doc.ID
	}
	for _, line := range bytes.Split(tail, []byte{'\n'}) {
		line = bytes.TrimSpace(line)
		if !bytes.HasPrefix(line, []byte("data:")) {
			continue
		}
		line = bytes.TrimSpace(bytes.TrimPrefix(line, []byte("data:")))
		if json.Unmarshal(line, &doc) == nil && doc.ID != "" {
			return doc.ID
		}
	}
	return ""
}
