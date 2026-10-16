package registry

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestSchedulerOllamaDirectRolesAndRecovery(t *testing.T) {
	calls := map[string]int{}
	failed := false
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls[r.URL.Path]++
		if strings.Contains(r.URL.Path, "inference-services") {
			t.Error("must not discover through model management")
		}
		if failed {
			w.WriteHeader(503)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		if strings.HasSuffix(r.URL.Path, "/api/tags") {
			json.NewEncoder(w).Encode(map[string]any{"models": []any{
				map[string]any{"name": "qwen:latest", "capabilities": []string{"completion"}},
				map[string]any{"name": "embed:latest", "capabilities": []string{"embedding"}},
				map[string]any{"name": "rank:latest", "capabilities": []string{"rerank"}},
			}})
			return
		}
		json.NewEncoder(w).Encode(map[string]any{"data": []any{map[string]any{"id": "qwen:latest"}, map[string]any{"id": "embed:latest"}, map[string]any{"id": "rank:latest"}}})
	}))
	defer backend.Close()
	reg := New()
	p := NewSchedulerOllama(reg, backend.URL+"/qa", backend.URL+"/embedding", backend.URL+"/rerank", time.Second)
	p.pollOnce(context.Background())
	for _, spec := range []struct{ role, model, path string }{{"qa", "qwen:latest", "/chat/completions"}, {"embedding", "embed:latest", "/embeddings"}, {"rerank", "rank:latest", "/rerank"}} {
		prefix := "v-scheduler/ollama-" + spec.role
		upstream, ok := reg.Get(prefix)
		if !ok || !upstream.Healthy || upstream.Name != prefix || upstream.Source != SourceSchedulerOllama {
			t.Fatalf("missing direct upstream: %+v", upstream)
		}
		got, _, release, err := reg.Acquire(prefix+"/"+spec.model, spec.path, "")
		if err != nil || got.ID != prefix {
			t.Fatalf("route %s: %+v %v", prefix, got, err)
		}
		release()
	}
	if _, _, _, err := reg.Acquire("v-scheduler/ollama-rerank/rank:latest", "/chat/completions", ""); err == nil {
		t.Fatal("rerank must not consume chat slots")
	}
	if _, _, _, err := reg.Acquire("v-scheduler/ollama-qa/embed:latest", "/embeddings", ""); err == nil {
		t.Fatal("QA must not consume embedding requests")
	}
	failed = true
	p.pollOnce(context.Background())
	if upstream, _ := reg.Get("v-scheduler/ollama-qa"); upstream.Healthy {
		t.Fatal("unreachable backend must be marked offline")
	}
	failed = false
	p.pollOnce(context.Background())
	if upstream, _ := reg.Get("v-scheduler/ollama-qa"); !upstream.Healthy {
		t.Fatal("backend must recover")
	}
}
