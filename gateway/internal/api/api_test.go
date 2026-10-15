package api

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

// fakeUpstream answers OpenAI-format model lists and chat completions.
func fakeUpstream(t *testing.T) *httptest.Server {
	t.Helper()
	mux := http.NewServeMux()
	mux.HandleFunc("GET /v1/models", func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{
			"object": "list",
			"data":   []map[string]string{{"id": "gpt-4o", "owned_by": "fake"}},
		})
	})
	// Generic catch-all so resource-passthrough and multimodal endpoints can
	// be contract-tested against a live upstream.
	mux.HandleFunc("/v1/", func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{"echo": "ok", "path": r.URL.Path, "method": r.Method})
	})
	// A file-creation endpoint returning a provider resource id, so resource
	// ownership tests can record and enforce a creator.
	mux.HandleFunc("POST /v1/files", func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{"id": "file-new", "object": "file"})
	})
	mux.HandleFunc("POST /v1/chat/completions", func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model  string `json:"model"`
			Stream bool   `json:"stream"`
		}
		_ = json.NewDecoder(r.Body).Decode(&req)
		if req.Model != "gpt-4o" {
			w.WriteHeader(http.StatusBadRequest)
			return
		}
		if req.Stream {
			w.Header().Set("Content-Type", "text/event-stream")
			for _, tok := range []string{"你", "好"} {
				chunk := map[string]any{"object": "chat.completion.chunk", "choices": []map[string]any{{"delta": map[string]string{"content": tok}}}}
				line, _ := json.Marshal(chunk)
				_, _ = w.Write([]byte("data: " + string(line) + "\n\n"))
			}
			usage, _ := json.Marshal(map[string]any{
				"choices": []map[string]any{{"index": 0, "delta": map[string]string{}, "finish_reason": "stop"}},
				"usage":   map[string]int{"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
			})
			_, _ = w.Write([]byte("data: " + string(usage) + "\n\n"))
			_, _ = w.Write([]byte("data: [DONE]\n\n"))
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{
			"object":  "chat.completion",
			"choices": []map[string]any{{"index": 0, "finish_reason": "stop", "message": map[string]string{"role": "assistant", "content": "echo:" + req.Model}}},
			"usage":   map[string]int{"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
		})
	})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	return srv
}

// newTestServer wires a gateway API server against a fake upstream and
// returns the API server and the fake upstream's base URL (including /v1).
func newTestServer(t *testing.T) (*httptest.Server, string) {
	t.Helper()
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true") // lifecycle test focuses on CRUD, not auth
	up := fakeUpstream(t)
	s := newServer(registry.New(), "test")
	s.allowLoopbackUpstreams = true
	srv := httptest.NewServer(s.routes())
	t.Cleanup(srv.Close)
	return srv, up.URL + "/v1"
}

func doJSON(t *testing.T, method, url string, body any) (*http.Response, string) {
	t.Helper()
	var rd io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		rd = strings.NewReader(string(b))
	}
	req, err := http.NewRequest(method, url, rd)
	if err != nil {
		t.Fatal(err)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	out, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()
	return resp, string(out)
}

func TestManualUpstreamLifecycleAndUsage(t *testing.T) {
	srv, upBase := newTestServer(t)

	// 1. Create: probe succeeds, key is masked in the response.
	resp, body := doJSON(t, "POST", srv.URL+"/api/v1/upstreams",
		map[string]string{"name": "Fake", "base_url": upBase, "api_key": "sk-x"})
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("create upstream: %d %s", resp.StatusCode, body)
	}
	var created struct {
		ID     string `json:"id"`
		APIKey string `json:"api_key"`
		HasKey bool   `json:"has_key"`
		Models int    `json:"models"`
	}
	if err := json.Unmarshal([]byte(body), &created); err != nil {
		t.Fatal(err)
	}
	if created.APIKey != "" || !created.HasKey || created.Models != 1 {
		t.Fatalf("created upstream must mask key and list 1 model: %+v", created)
	}

	// 2. OpenAI model list exposes only qualified ids.
	_, body = doJSON(t, "GET", srv.URL+"/v1/models", nil)
	var list struct {
		Data []struct{ ID string } `json:"data"`
	}
	if err := json.Unmarshal([]byte(body), &list); err != nil {
		t.Fatal(err)
	}
	ids := map[string]bool{}
	for _, m := range list.Data {
		ids[m.ID] = true
	}
	if !ids["manual/gpt-4o"] || ids["gpt-4o"] {
		t.Fatalf("model list must include only qualified ids: %v", ids)
	}

	// 3. Rich management model list carries upstream metadata.
	_, body = doJSON(t, "GET", srv.URL+"/api/v1/models", nil)
	var rich struct {
		Models []struct {
			ID           string `json:"id"`
			Short        bool   `json:"short"`
			UpstreamName string `json:"upstream_name"`
			UpstreamKind string `json:"upstream_kind"`
			ModelPrefix  string `json:"model_prefix"`
		} `json:"models"`
	}
	if err := json.Unmarshal([]byte(body), &rich); err != nil {
		t.Fatal(err)
	}
	if len(rich.Models) != 1 {
		t.Fatalf("rich models: %+v", rich.Models)
	}
	if rich.Models[0].UpstreamName != "Fake" || rich.Models[0].UpstreamKind != "manual" || rich.Models[0].ModelPrefix != "manual" {
		t.Fatalf("rich models metadata wrong: %+v", rich.Models[0])
	}

	// 4. Streaming chat passes through and is recorded in usage.
	resp, body = doJSON(t, "POST", srv.URL+"/v1/chat/completions",
		map[string]any{"model": "manual/gpt-4o", "stream": true, "messages": []map[string]string{{"role": "user", "content": "hi"}}})
	if resp.StatusCode != http.StatusOK || !strings.Contains(body, "data: ") || !strings.Contains(body, "你") {
		t.Fatalf("streaming chat failed: %d %s", resp.StatusCode, body)
	}

	// 5. Non-streaming chat works via the qualified id.
	_, body = doJSON(t, "POST", srv.URL+"/v1/chat/completions",
		map[string]any{"model": "manual/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hi"}}})
	if !strings.Contains(body, "echo:gpt-4o") {
		t.Fatalf("chat passthrough failed: %s", body)
	}

	// 6. Usage snapshot: two requests, one streaming, both on the same model.
	_, body = doJSON(t, "GET", srv.URL+"/api/v1/usage", nil)
	var usage struct {
		Totals struct {
			Requests      int64 `json:"requests"`
			Errors        int64 `json:"errors"`
			InputTokens   int64 `json:"input_tokens"`
			OutputTokens  int64 `json:"output_tokens"`
			TotalTokens   int64 `json:"total_tokens"`
			UsageRequests int64 `json:"usage_requests"`
		} `json:"totals"`
		PerModel map[string]struct {
			Requests int64 `json:"requests"`
		} `json:"per_model"`
		Recent []struct {
			Stream bool   `json:"stream"`
			Model  string `json:"model"`
			Status int    `json:"status"`
		} `json:"recent"`
	}
	if err := json.Unmarshal([]byte(body), &usage); err != nil {
		t.Fatal(err)
	}
	if usage.Totals.Requests != 2 || usage.Totals.Errors != 0 {
		t.Fatalf("usage totals wrong: %+v", usage.Totals)
	}
	if usage.Totals.InputTokens != 6 || usage.Totals.OutputTokens != 4 || usage.Totals.TotalTokens != 10 || usage.Totals.UsageRequests != 2 {
		t.Fatalf("usage token totals wrong: %+v", usage.Totals)
	}
	if len(usage.Recent) != 2 || usage.Recent[0].Stream || !usage.Recent[1].Stream {
		t.Fatalf("usage recent records wrong (newest first, stream flags): %+v", usage.Recent)
	}
	if usage.PerModel["fake/gpt-4o"].Requests != 2 {
		t.Fatalf("per-model aggregation wrong: %+v", usage.PerModel)
	}

	// 7. Status aggregates from usage.
	_, body = doJSON(t, "GET", srv.URL+"/api/v1/status", nil)
	if !strings.Contains(body, `"requests_total":2`) {
		t.Fatalf("status must report 2 requests: %s", body)
	}

	// 8. Delete removes the upstream everywhere.
	resp, _ = doJSON(t, "DELETE", srv.URL+"/api/v1/upstreams/fake", nil)
	if resp.StatusCode != http.StatusNoContent {
		t.Fatalf("delete upstream: %d", resp.StatusCode)
	}
	_, body = doJSON(t, "GET", srv.URL+"/v1/models", nil)
	if strings.Contains(body, "gpt-4o") {
		t.Fatalf("models must be gone after delete: %s", body)
	}
}

// A failing upstream probe must reject creation with a gateway error instead
// of registering a dead upstream silently.
func TestCreateUpstreamProbeFailure(t *testing.T) {
	srv, _ := newTestServer(t)
	resp, body := doJSON(t, "POST", srv.URL+"/api/v1/upstreams",
		map[string]string{"name": "Dead", "base_url": "http://127.0.0.1:1/v1"})
	if resp.StatusCode != http.StatusBadGateway {
		t.Fatalf("expected 502 for unreachable upstream, got %d %s", resp.StatusCode, body)
	}
}

func TestCloudUpstreamUsesDomainPrefixAndCanSkipProbeFailure(t *testing.T) {
	if got := modelPrefixForPayload(upstreamPayload{Kind: registry.KindCloud, BaseURL: "https://ai.ictrek.com/v1"}); got != "com.ictrek.ai" {
		t.Fatalf("ictrek cloud prefix = %q", got)
	}
	srv, upBase := newTestServer(t)

	resp, body := doJSON(t, "POST", srv.URL+"/api/v1/upstreams",
		map[string]any{"kind": "cloud", "name": "ICTrek AI", "base_url": upBase, "api_key": "sk-x"})
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("create cloud upstream: %d %s", resp.StatusCode, body)
	}
	if !strings.Contains(body, `"kind":"cloud"`) || !strings.Contains(body, `"model_prefix":"`) {
		t.Fatalf("cloud view missing kind/prefix: %s", body)
	}

	_, body = doJSON(t, "GET", srv.URL+"/v1/models", nil)
	if !strings.Contains(body, "1.0.0.127/gpt-4o") {
		t.Fatalf("cloud qualified model missing: %s", body)
	}

	resp, body = doJSON(t, "POST", srv.URL+"/api/v1/upstreams", map[string]any{
		"kind": "cloud", "name": "Saved Without Probe", "base_url": "http://127.0.0.1:1/v1", "test_model": "known-model", "allow_probe_failure": true,
	})
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("skip probe create: %d %s", resp.StatusCode, body)
	}
	_, body = doJSON(t, "GET", srv.URL+"/v1/models", nil)
	if !strings.Contains(body, "1.0.0.127/known-model") {
		t.Fatalf("fallback model missing: %s", body)
	}
}
