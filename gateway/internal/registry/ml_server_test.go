package registry

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestMLServerDiscoveryAndCapabilityRouting(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/":
			_ = json.NewEncoder(w).Encode(map[string]string{"service": "immich-ml-server", "version": "1.6.1", "status": "running"})
		case "/openapi.json":
			_ = json.NewEncoder(w).Encode(map[string]any{"paths": map[string]any{"/predict": map[string]any{}, "/api/v1/video/structure": map[string]any{}, "/api/v1/video/structure/tasks/{task_id}": map[string]any{}}})
		default:
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()
	r := New()
	p := NewMLServerPoller(r, []string{"http://127.0.0.1:1", srv.URL}, time.Minute)
	p.pollOnce(context.Background())
	u, ok := r.Get(mlServerUpstreamID)
	if !ok || u.Source != SourceMLServer || u.Protocol != ProtocolMLServer || u.BaseURL != srv.URL {
		t.Fatalf("unexpected discovered upstream: %+v %v", u, ok)
	}
	if got := len(r.ModelsOf(mlServerUpstreamID)); got != 6 {
		t.Fatalf("models=%d want 6", got)
	}
	for _, tc := range []struct{ model, path string }{
		{"ml-server/qwen3-embedding-0.6b", "/embeddings"},
		{"ml-server/ViT-B-16-SigLIP2__webli", "/ml/predictions"},
		{"ml-server/video-structure", "/ml/video/structure/tasks"},
	} {
		u, _, release, err := r.Acquire(tc.model, tc.path, "")
		if err != nil || u.ID != mlServerUpstreamID {
			t.Fatalf("route %s %s: %+v %v", tc.model, tc.path, u, err)
		}
		release()
	}
	if _, _, _, err := r.Acquire("ml-server/qwen3-embedding-0.6b", "/ml/predictions", ""); !errors.Is(err, ErrUnsupported) {
		t.Fatalf("qwen image route should be unsupported, got %v", err)
	}
}

func TestMLServerDiscoveryRejectsWrongService(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{"service": "other", "status": "running"})
	}))
	defer srv.Close()
	r := New()
	p := NewMLServerPoller(r, []string{srv.URL}, time.Minute)
	p.pollOnce(context.Background())
	if _, ok := r.Get(mlServerUpstreamID); ok {
		t.Fatal("wrong service must not be registered")
	}
}
