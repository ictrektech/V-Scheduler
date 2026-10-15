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

// fakeModelHub mimics the Model Hub backend contract: a bare JSON array from
// GET /api/v1/inference-services, and OpenAI-format model lists per gateway.
type fakeModelHub struct {
	*httptest.Server
	includeBuiltinQueries []string
}

func newFakeModelHub(t *testing.T) *fakeModelHub {
	t.Helper()
	f := &fakeModelHub{}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/v1/inference-services", func(w http.ResponseWriter, r *http.Request) {
		f.includeBuiltinQueries = append(f.includeBuiltinQueries, r.URL.Query().Get("include_builtin"))
		services := []map[string]any{
			{
				"service_id":  "model-hub-ollama-qa",
				"name":        "QA Ollama",
				"kind":        "ollama",
				"role":        "generate",
				"base_url":    "http://ignored:11434",
				"gateway_url": f.URL + "/gw/qa",
			},
			{
				"service_id":  "model-hub-ollama-embedding",
				"name":        "Embedding Ollama",
				"kind":        "ollama",
				"role":        "embedding",
				"gateway_url": f.URL + "/gw/emb",
			},
			{
				"service_id":  "model-hub-ollama-rerank",
				"name":        "Rerank Ollama",
				"base_url":    f.URL + "/gw/rerank",
				"kind":        "ollama",
				"role":        "rerank",
				"gateway_url": f.URL + "/gw/rerank",
			},
			{
				"service_id": "vllm-extra",
				"name":       "vLLM",
				"kind":       "custom",
				"role":       nil,
			},
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(services)
	})
	mux.HandleFunc("/gw/", func(w http.ResponseWriter, r *http.Request) {
		// QA serves the chat model; the embedding gateway serves bge-m3 only,
		// matching real builtin Ollama role separation.
		if r.URL.Path == "/gw/rerank/api/tags" {
			_ = json.NewEncoder(w).Encode(map[string]any{"models": []map[string]any{{"name": "bge-reranker", "capabilities": []string{"rerank"}}}})
			return
		}
		data := []map[string]string{{"id": "qwen3.5:2b"}}
		if r.URL.Path == "/gw/emb/v1/models" {
			data = []map[string]string{{"id": "bge-m3"}}
		}
		if r.URL.Path == "/gw/rerank/v1/models" {
			data = []map[string]string{{"id": "bge-reranker"}}
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"object": "list", "data": data})
	})
	f.Server = httptest.NewServer(mux)
	t.Cleanup(f.Close)
	return f
}

func TestModelHubDiscovery(t *testing.T) {
	hub := newFakeModelHub(t)
	reg := New()
	NewModelHubPoller(reg, hub.URL, time.Minute).pollOnce(context.Background())

	if len(hub.includeBuiltinQueries) == 0 || hub.includeBuiltinQueries[0] != "true" {
		t.Fatalf("discovery must request include_builtin=true, got %v", hub.includeBuiltinQueries)
	}

	// Rerank is discovered and uses the dedicated gateway.
	rerank, ok := reg.Get("modelhub-model-hub-ollama-rerank")
	if !ok || !rerank.Healthy || rerank.Role != "rerank" || rerank.BaseURL != hub.URL+"/gw/rerank/v1" {
		t.Fatalf("rerank upstream missing or incorrect: %+v", rerank)
	}
	u, _, release, err := reg.Acquire("model-hub/bge-reranker", "/rerank", "")
	if err != nil || u.ID != rerank.ID {
		t.Fatalf("rerank must route to dedicated upstream: %+v %v", u, err)
	}
	release()

	qa, ok := reg.Get("modelhub-model-hub-ollama-qa")
	if !ok {
		t.Fatal("QA upstream missing after discovery")
	}
	if !qa.Healthy || qa.Role != RoleGenerate || qa.Source != SourceModelHub {
		t.Fatalf("QA upstream wrong: %+v", qa)
	}
	if qa.BaseURL != hub.URL+"/gw/qa/v1" {
		t.Fatalf("QA base URL must append /v1 to gateway_url, got %s", qa.BaseURL)
	}
	// Both QA and embedding gateways answered; QA carries the chat model.
	models := reg.ModelsOf("modelhub-model-hub-ollama-qa")
	if len(models) != 1 || models[0].Qualified != "model-hub/qwen3.5:2b" || models[0].Kind != ModelKindChat {
		t.Fatalf("QA models wrong: %+v", models)
	}

	emb, ok := reg.Get("modelhub-model-hub-ollama-embedding")
	if !ok || !emb.Healthy {
		t.Fatalf("embedding upstream missing or unhealthy: %+v", emb)
	}
	embModels := reg.ModelsOf("modelhub-model-hub-ollama-embedding")
	if len(embModels) != 1 || embModels[0].Kind != ModelKindEmbedding {
		t.Fatalf("embedding models must carry embedding kind: %+v", embModels)
	}

	// custom service without gateway_url is registered but unhealthy
	vllm, ok := reg.Get("modelhub-vllm-extra")
	if !ok || vllm.Healthy {
		t.Fatalf("custom service without gateway_url must exist and be unhealthy: %+v", vllm)
	}

	// Discovered local models must be addressed through their source prefix.
	u, _, err = reg.Resolve("model-hub/qwen3.5:2b")
	if err != nil || u.ID != "modelhub-model-hub-ollama-qa" {
		t.Fatalf("resolve local qualified name: (%v, %v)", u, err)
	}
	if _, _, err := reg.Resolve("qwen3.5:2b"); !errors.Is(err, ErrUnknownModel) {
		t.Fatalf("short local name must not route: %v", err)
	}
}

// A service that disappears from Model Hub must be dropped on the next poll.
func TestModelHubDiscoveryRemovesStale(t *testing.T) {
	hub := newFakeModelHub(t)
	reg := New()
	NewModelHubPoller(reg, hub.URL, time.Minute).pollOnce(context.Background())
	if _, ok := reg.Get("modelhub-model-hub-ollama-qa"); !ok {
		t.Fatal("setup: QA upstream missing")
	}

	empty := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode([]map[string]any{})
	}))
	defer empty.Close()
	NewModelHubPoller(reg, empty.URL, time.Minute).pollOnce(context.Background())
	if _, ok := reg.Get("modelhub-model-hub-ollama-qa"); ok {
		t.Fatal("stale discovered upstream must be removed after disappearing from Model Hub")
	}
}

func TestModelHubDiscoveryDeduplicatesRegisteredBaseURL(t *testing.T) {
	hub := newFakeModelHub(t)
	reg := New()
	poller := NewModelHubPoller(reg, hub.URL, time.Minute)
	poller.pollOnce(context.Background())
	if _, ok := reg.Get("modelhub-model-hub-ollama-qa"); !ok {
		t.Fatal("setup: model hub discovered QA upstream")
	}
	registered := Upstream{ID: "registered-qa", Name: "Registered QA", Kind: KindRegistered, Source: SourceRegistered, BaseURL: hub.URL + "/gw/qa/v1", Healthy: true}
	if err := reg.Register(registered, []Model{{ID: "qwen3.5:2b", Qualified: "registered/qwen3.5:2b", Upstream: registered.ID, Kind: ModelKindChat}}); err != nil {
		t.Fatal(err)
	}
	if _, ok := reg.Get("modelhub-model-hub-ollama-qa"); ok {
		t.Fatal("active registration must remove duplicate V-Router Model Hub discovery for the same base URL")
	}
	poller.pollOnce(context.Background())
	if _, ok := reg.Get("modelhub-model-hub-ollama-qa"); ok {
		t.Fatal("V-Router Model Hub polling must not re-add a service already registered with the same base URL")
	}
	if _, ok := reg.Get("modelhub-model-hub-ollama-embedding"); !ok {
		t.Fatal("dedupe must not remove different Model Hub services")
	}
}

func TestModelHubDiscoverySkipsExistingRegisteredBaseURL(t *testing.T) {
	hub := newFakeModelHub(t)
	reg := New()
	registered := Upstream{ID: "registered-qa", Name: "Registered QA", Kind: KindRegistered, Source: SourceRegistered, BaseURL: hub.URL + "/gw/qa/v1", Healthy: true}
	if err := reg.Register(registered, []Model{{ID: "qwen3.5:2b", Qualified: "registered/qwen3.5:2b", Upstream: registered.ID, Kind: ModelKindChat}}); err != nil {
		t.Fatal(err)
	}
	NewModelHubPoller(reg, hub.URL, time.Minute).pollOnce(context.Background())
	if _, ok := reg.Get("modelhub-model-hub-ollama-qa"); ok {
		t.Fatal("V-Router Model Hub discovery must skip a service already registered with the same base URL")
	}
	if _, ok := reg.Get("registered-qa"); !ok {
		t.Fatal("registered upstream must remain authoritative")
	}
}

func TestModelHubSharedDirectoryClassifiesByModelID(t *testing.T) {
	role := func(s string) *string { return &s }
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/v1/inference-services", func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode([]map[string]any{
			{"service_id": "model-hub-ollama-qa", "name": "QA", "role": "generate", "gateway_url": "http://example.invalid/qa"},
			{"service_id": "model-hub-ollama-embedding", "name": "Embedding", "role": "embedding", "gateway_url": "http://example.invalid/emb"},
		})
	})
	modelList := func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{"data": []map[string]string{{"id": "nomic-embed-text:latest"}, {"id": "qwen3:0.6b"}}})
	}
	tags := func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{"models": []map[string]any{
			{"name": "nomic-embed-text:latest", "model": "nomic-embed-text:latest", "capabilities": []string{"embedding"}},
			{"name": "qwen3:0.6b", "model": "qwen3:0.6b", "capabilities": []string{"completion", "tools", "thinking"}},
		}})
	}
	mux.HandleFunc("/qa/v1/models", modelList)
	mux.HandleFunc("/emb/v1/models", modelList)
	mux.HandleFunc("/qa/api/tags", tags)
	mux.HandleFunc("/emb/api/tags", tags)
	ts := httptest.NewServer(mux)
	defer ts.Close()

	services := []modelHubService{
		{ServiceID: "model-hub-ollama-qa", Name: "QA", Kind: "ollama", Role: role("generate"), BaseURL: ts.URL + "/qa", GatewayURL: ts.URL + "/qa"},
		{ServiceID: "model-hub-ollama-embedding", Name: "Embedding", Kind: "ollama", Role: role("embedding"), BaseURL: ts.URL + "/emb", GatewayURL: ts.URL + "/emb"},
	}
	reg := New()
	poller := NewModelHubPoller(reg, ts.URL, time.Minute)
	for _, svc := range services {
		models, err := poller.fetchModels(context.Background(), svc)
		if err != nil {
			t.Fatal(err)
		}
		reg.Put(&Upstream{ID: "modelhub-" + svc.ServiceID, Kind: KindLocal, Role: svc.roleString(), Healthy: true, Source: SourceModelHub})
		reg.SetModels("modelhub-"+svc.ServiceID, models)
	}

	if u, _, release, err := reg.Acquire("model-hub/qwen3:0.6b", "/chat/completions", ""); err != nil || u.ID != "modelhub-model-hub-ollama-qa" {
		t.Fatalf("qwen chat must route to QA, got %s %v", u.ID, err)
	} else {
		release()
	}
	if _, _, _, err := reg.Acquire("model-hub/nomic-embed-text:latest", "/chat/completions", ""); !errors.Is(err, ErrUnsupported) {
		t.Fatalf("embedding model must not support chat, got %v", err)
	}
	if u, _, release, err := reg.Acquire("model-hub/nomic-embed-text:latest", "/embeddings", ""); err != nil || u.ID != "modelhub-model-hub-ollama-embedding" {
		t.Fatalf("nomic embeddings must route to embedding gateway, got %s %v", u.ID, err)
	} else {
		release()
	}
	if _, _, _, err := reg.Acquire("model-hub/qwen3:0.6b", "/embeddings", ""); !errors.Is(err, ErrUnsupported) {
		t.Fatalf("chat model must not support embeddings, got %v", err)
	}
}
