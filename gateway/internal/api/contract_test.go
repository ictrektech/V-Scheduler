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

// Every endpoint listed in the API docs must actually work. This walks the
// same spec() the docs are generated from and exercises each route over real
// HTTP against a live fake upstream. A route added to the docs without a
// working implementation fails here, and so does a regression — docs, routes
// and tests cannot drift apart.
func TestEveryDocumentedEndpointIsUsable(t *testing.T) {
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true") // exercise management paths without OIDC

	up := fakeUpstream(t)
	business := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{"echo": "business", "path": r.URL.Path, "method": r.Method})
	}))
	t.Cleanup(business.Close)
	reg := registry.New()
	reg.SetService(registry.ServiceVideoStructureDetect, business.URL)
	reg.SetService(registry.ServiceVideoStructureStructure, business.URL)
	s := newServer(reg, "contract")
	s.allowLoopbackUpstreams = true
	srv := httptest.NewServer(s.routes())
	t.Cleanup(srv.Close)

	resp, body := doJSON(t, "POST", srv.URL+"/api/v1/upstreams",
		map[string]string{"name": "Fake", "base_url": up.URL + "/v1"})
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("setup upstream failed: %d %s", resp.StatusCode, body)
	}

	// PUT must update using the real upstream URL, then refresh must re-probe.
	resp, body = doJSON(t, "PUT", srv.URL+"/api/v1/upstreams/fake",
		map[string]string{"name": "Fake", "base_url": up.URL + "/v1"})
	if resp.StatusCode != http.StatusOK || !strings.Contains(body, "fake") {
		t.Fatalf("PUT upstream: %d %s", resp.StatusCode, body)
	}
	resp, body = doJSON(t, "POST", srv.URL+"/api/v1/upstreams/fake/refresh", nil)
	if resp.StatusCode != http.StatusOK || !strings.Contains(body, "fake") {
		t.Fatalf("refresh upstream: %d %s", resp.StatusCode, body)
	}

	callAndCheck := func(method, path string, wantStatus int, bodyMust string) {
		t.Helper()
		r, b := callEndpoint(t, srv.URL, method, path)
		if r.StatusCode != wantStatus {
			t.Errorf("%s %s: status %d, want %d (body: %.160s)", method, path, r.StatusCode, wantStatus, b)
		} else if bodyMust != "" && !strings.Contains(b, bodyMust) {
			t.Errorf("%s %s: body missing %q (body: %.160s)", method, path, bodyMust, b)
		}
	}

	for _, r := range s.spec() {
		key := r.Method + " " + r.Path
		if r.Group == "video-structure" {
			if key == "GET /api/v1/video-structure/status" {
				callAndCheck(r.Method, r.Path, http.StatusOK, `"available":true`)
			} else {
				real := strings.ReplaceAll(r.Path, "{job_id}", "job-1")
				callAndCheck(r.Method, real, http.StatusOK, `"echo":"business"`)
			}
			continue
		}
		switch key {
		// Inference + multimodal: JSON bodies routed by model.
		case "POST /v1/chat/completions":
			for i := 0; i < 6; i++ {
				r2, b2 := callEndpoint(t, srv.URL, r.Method, r.Path)
				t.Logf("probe chat #%d: %d", i, r2.StatusCode)
				if r2.StatusCode != http.StatusOK {
					t.Errorf("chat #%d failed: %s", i, b2)
					break
				}
			}
		case "POST /v1/completions", "POST /v1/responses", "POST /v1/responses/compact",
			"POST /v1/embeddings", "POST /v1/moderations", "POST /v1/rerank",
			"POST /v1/images/generations", "POST /v1/images/edits", "POST /v1/images/variations",
			"POST /v1/videos/generations",
			"POST /v1/audio/transcriptions", "POST /v1/audio/translations", "POST /v1/audio/speech", "POST /v1/audio/music/generations",
			"POST /v1/audio/speakers/identify", "POST /v1/audio/speakers/verify":
			callAndCheck(r.Method, r.Path, http.StatusOK, `"echo":"ok"`)
		case "POST /v1/audio/streaming/transcriptions", "GET /v1/audio/speakers", "GET /v1/audio/streaming/ws":
			// Query-routed audio endpoints take ?model= and stream/echo.
			callAndCheck(r.Method, r.Path+"?model=manual%2Fgpt-4o", http.StatusOK, `"echo":"ok"`)
		case "POST /v1/messages":
			// Documented contract: against an openai-protocol upstream the
			// Messages bridge must refuse with a clear error. The happy
			// path is covered in TestMessagesAgainstChatCompletionsUpstream.
			callAndCheck(r.Method, r.Path, http.StatusBadRequest, "no matching upstream supports")
		case "POST /v1/ml/predictions":
			callAndCheck(r.Method, r.Path, http.StatusBadRequest, "model")
		case "POST /v1/ml/video/structure":
			callAndCheck(r.Method, r.Path, http.StatusBadRequest, "invalid JSON request")
		case "GET /v1/ml/video/structure/tasks/{task_id}":
			callAndCheck(r.Method, strings.ReplaceAll(r.Path, "{task_id}", "task-1"), http.StatusBadRequest, "model")

		// Management read/write endpoints.
		case "PUT /api/v1/upstreams/{id}/routing":
			resp, body := doJSON(t, "PUT", srv.URL+"/api/v1/upstreams/fake/routing", map[string]int{"weight": 2})
			if resp.StatusCode != 200 || !strings.Contains(body, `"weight":2`) {
				t.Fatalf("routing: %d %s", resp.StatusCode, body)
			}
		case "POST /api/v1/registrations":
			resp, body := doJSON(t, "POST", srv.URL+"/api/v1/registrations", map[string]string{"service_id": "replica-1", "base_url": up.URL + "/v1"})
			if resp.StatusCode != 200 || !strings.Contains(body, "registered-replica-1") {
				t.Fatalf("register: %d %s", resp.StatusCode, body)
			}
		case "POST /api/v1/registrations/{id}/heartbeat":
			resp, body := doJSON(t, "POST", srv.URL+"/api/v1/registrations/registered-replica-1/heartbeat", map[string]int{"ttl_seconds": 90})
			if resp.StatusCode != 200 || !strings.Contains(body, "expires_at") {
				t.Fatalf("heartbeat: %d %s", resp.StatusCode, body)
			}
		case "DELETE /api/v1/registrations/{id}":
			resp, body := doJSON(t, "DELETE", srv.URL+"/api/v1/registrations/registered-replica-1", nil)
			if resp.StatusCode != 204 {
				t.Fatalf("unregister: %d %s", resp.StatusCode, body)
			}
		case "GET /healthz":
			callAndCheck(r.Method, r.Path, http.StatusOK, "ok")
		case "GET /api/v1/auth-probe":
			callAndCheck(r.Method, r.Path, http.StatusOK, "oidc_enabled")
		case "GET /api/v1/auth/me":
			callAndCheck(r.Method, r.Path, http.StatusOK, "identity")
		case "GET /api/v1/openapi.json":
			callAndCheck(r.Method, r.Path, http.StatusOK, "openapi")
		case "GET /api/v1/status":
			callAndCheck(r.Method, r.Path, http.StatusOK, "version")
		case "GET /api/v1/models":
			callAndCheck(r.Method, r.Path, http.StatusOK, "upstream_name")
		case "GET /api/v1/usage":
			callAndCheck(r.Method, r.Path, http.StatusOK, "per_model")
		case "GET /api/v1/upstreams":
			callAndCheck(r.Method, r.Path, http.StatusOK, "fake")
		case "POST /api/v1/upstreams/probe":
			resp, body := doJSON(t, "POST", srv.URL+"/api/v1/upstreams/probe", map[string]string{"kind": "cloud", "name": "Cloud", "base_url": up.URL + "/v1"})
			if resp.StatusCode != 200 || !strings.Contains(body, "models") || !strings.Contains(body, "0.0.1") {
				t.Fatalf("probe upstream: %d %s", resp.StatusCode, body)
			}
		case "PUT /api/v1/upstreams/{id}", "POST /api/v1/upstreams/{id}/refresh", "DELETE /api/v1/upstreams/{id}", "POST /api/v1/upstreams":
			continue // exercised explicitly below with the real upstream URL
		case "GET /api/v1/apidoc":
			callAndCheck(r.Method, r.Path, http.StatusOK, "groups")
		case "GET /v1/models":
			callAndCheck(r.Method, r.Path, http.StatusOK, "manual/gpt-4o")
		case "GET /v1/audio/voices":
			callAndCheck(r.Method, r.Path+"?model=manual%2Fgpt-4o", http.StatusOK, `"echo":"ok"`)

		default:
			if r.Group == "resource" {
				continue // swept below
			}
			t.Errorf("documented endpoint %s has no contract check", key)
		}
	}

	// Resource-passthrough sweep: every documented resource route must
	// forward to the upstream when pinned, and reject without a pin.
	for _, r := range s.spec() {
		if r.Group != "resource" {
			continue
		}
		real := strings.ReplaceAll(r.Path, "{resource_path...}", "")
		req, err := http.NewRequest(r.Method, srv.URL+real, nil)
		if err != nil {
			t.Fatal(err)
		}
		req.Header.Set("X-VRouter-Upstream", "fake")
		pResp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		pBody, _ := io.ReadAll(pResp.Body)
		_ = pResp.Body.Close()
		want := `"echo":"ok"`
		if r.Method == "POST" && r.Path == "/v1/files" {
			// The fake upstream answers bare POST /v1/files with a creation
			// body so the gateway records resource ownership.
			want = `"id":"file-new"`
		}
		if pResp.StatusCode != http.StatusOK || !strings.Contains(string(pBody), want) {
			t.Errorf("pinned %s %s: status %d body %.120s", r.Method, r.Path, pResp.StatusCode, pBody)
		}

		uResp, uBody := doJSON(t, r.Method, srv.URL+real, nil)
		if uResp.StatusCode != http.StatusBadRequest {
			t.Errorf("unpinned %s %s: status %d, want 400 (%.80s)", r.Method, r.Path, uResp.StatusCode, uBody)
		}
	}

	// DELETE last: removes the upstream and must report 204.
	callAndCheck("DELETE", "/api/v1/upstreams/fake", http.StatusNoContent, "")
}

func callEndpoint(t *testing.T, base, method, path string) (*http.Response, string) {
	t.Helper()
	real := strings.ReplaceAll(path, "{id}", "fake")
	var body any
	switch method + " " + path {
	case "POST /v1/chat/completions":
		body = map[string]any{"model": "manual/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hi"}}}
	case "POST /v1/completions":
		body = map[string]any{"model": "manual/gpt-4o", "prompt": "hi"}
	case "POST /v1/embeddings":
		body = map[string]any{"model": "manual/gpt-4o", "input": "hi"}
	case "POST /v1/responses", "POST /v1/responses/compact":
		body = map[string]any{"model": "manual/gpt-4o", "input": "hi"}
	case "POST /v1/messages":
		body = map[string]any{"model": "manual/gpt-4o", "max_tokens": 16, "messages": []map[string]any{{"role": "user", "content": "hi"}}}
	case "POST /v1/images/generations", "POST /v1/images/edits", "POST /v1/images/variations", "POST /v1/videos/generations":
		body = map[string]any{"model": "manual/gpt-4o", "prompt": "a mountain"}
	case "POST /v1/audio/transcriptions", "POST /v1/audio/translations",
		"POST /v1/audio/speakers/identify", "POST /v1/audio/speakers/verify":
		body = map[string]any{"model": "manual/gpt-4o"}
	case "POST /v1/audio/speech":
		body = map[string]any{"model": "manual/gpt-4o", "input": "hi", "voice": "alloy"}
	case "POST /v1/audio/music/generations":
		body = map[string]any{"model": "manual/gpt-4o", "prompt": "warm ambient", "duration": 10}
	case "POST /v1/moderations":
		body = map[string]any{"model": "manual/gpt-4o", "input": "hi"}
	case "POST /v1/rerank":
		body = map[string]any{"model": "manual/gpt-4o", "query": "q", "documents": []string{"d"}}
	}
	return doJSON(t, method, base+real, body)
}

// The Messages bridge must work against an upstream that declares the
// chat_completions protocol — the case Ollama Model Hub upstreams hit.
func TestMessagesAgainstChatCompletionsUpstream(t *testing.T) {
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	s := newServer(registry.New(), "contract")
	s.allowLoopbackUpstreams = true
	srv := httptest.NewServer(s.routes())
	t.Cleanup(srv.Close)

	// "aaa" sorts before "fake" so chat models resolve to this upstream.
	cc := fakeChatCompletionsUpstream(t)
	resp, body := doJSON(t, "POST", srv.URL+"/api/v1/upstreams",
		map[string]string{"name": "CC", "base_url": cc.URL + "/v1", "protocol": "chat_completions"})
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("setup cc upstream: %d %s", resp.StatusCode, body)
	}
	resp, body = doJSON(t, "POST", srv.URL+"/v1/messages",
		map[string]any{"model": "manual/gpt-4o", "max_tokens": 16,
			"messages": []map[string]any{{"role": "user", "content": "hi"}}})
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("messages via chat_completions upstream: %d %s", resp.StatusCode, body)
	}
	if !strings.Contains(body, "content") {
		t.Errorf("anthropic-style response expected, got %.160s", body)
	}
}

// fakeChatCompletionsUpstream answers with a valid Chat Completions payload,
// as a real chat_completions-protocol upstream (e.g. Ollama) would.
func fakeChatCompletionsUpstream(t *testing.T) *httptest.Server {
	t.Helper()
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/chat/completions", func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id": "chatcmpl-cc", "object": "chat.completion",
			"choices": []map[string]any{{
				"index": 0, "finish_reason": "stop",
				"message": map[string]string{"role": "assistant", "content": "hi there"},
			}},
			"usage": map[string]int{"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3},
		})
	})
	mux.HandleFunc("GET /v1/models", func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{
			"object": "list",
			"data":   []map[string]string{{"id": "gpt-4o", "owned_by": "fake"}},
		})
	})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	return srv
}
