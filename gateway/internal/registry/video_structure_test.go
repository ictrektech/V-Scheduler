package registry

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func videoStructureHealthServer(t *testing.T, service string) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/health" {
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"code": 0, "data": map[string]string{"status": "ok", "service": service}})
	}))
}

func TestVideoStructureDiscoveryPrefersAliasesAndDoesNotCreateModels(t *testing.T) {
	detectAlias := videoStructureHealthServer(t, "scene-detection")
	defer detectAlias.Close()
	detectHost := videoStructureHealthServer(t, "scene-detection")
	defer detectHost.Close()
	structureAlias := videoStructureHealthServer(t, "structure")
	defer structureAlias.Close()

	r := New()
	p := NewVideoStructurePoller(r, []string{detectAlias.URL, detectHost.URL}, []string{structureAlias.URL}, time.Minute)
	p.pollOnce(context.Background())
	if got, ok := r.Service(ServiceVideoStructureDetect); !ok || got != detectAlias.URL {
		t.Fatalf("detect alias was not preferred: %q %v", got, ok)
	}
	if got, ok := r.Service(ServiceVideoStructureStructure); !ok || got != structureAlias.URL {
		t.Fatalf("structure alias missing: %q %v", got, ok)
	}
	if models := r.ListModels(); len(models) != 0 {
		t.Fatalf("business service must not enter the model pool: %#v", models)
	}
}

func TestVideoStructureDiscoveryFallsBackAndSilentlyClearsMissingServices(t *testing.T) {
	detectHost := videoStructureHealthServer(t, "scene-detection")
	structureHost := videoStructureHealthServer(t, "structure")
	r := New()
	p := NewVideoStructurePoller(r,
		[]string{"http://127.0.0.1:1", detectHost.URL},
		[]string{"http://127.0.0.1:1", structureHost.URL}, time.Minute)
	p.pollOnce(context.Background())
	if got, ok := r.Service(ServiceVideoStructureDetect); !ok || got != detectHost.URL {
		t.Fatalf("detect fallback missing: %q %v", got, ok)
	}
	if got, ok := r.Service(ServiceVideoStructureStructure); !ok || got != structureHost.URL {
		t.Fatalf("structure fallback missing: %q %v", got, ok)
	}
	detectHost.Close()
	structureHost.Close()
	p.pollOnce(context.Background())
	if _, ok := r.Service(ServiceVideoStructureDetect); ok {
		t.Fatal("stale detect service must be removed")
	}
	if _, ok := r.Service(ServiceVideoStructureStructure); ok {
		t.Fatal("stale structure service must be removed")
	}
}
