package api

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestAnthropicNativeAndCredentials(t *testing.T) {
	var calls int
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		if r.Header.Get("X-Api-Key") != "provider-secret" || r.Header.Get("Authorization") != "" || r.Header.Get("Cookie") != "" || r.Header.Get("X-VRouter-Upstream") != "" {
			t.Error("gateway credentials leaked or provider credentials missing")
		}
		if r.Header.Get("Anthropic-Version") != "2023-06-01" {
			t.Error("missing API version")
		}
		if r.URL.Path == "/v1/messages" {
			var body map[string]any
			_ = json.NewDecoder(r.Body).Decode(&body)
			if body["model"] != "native" {
				t.Error("model not rewritten")
			}
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"type":"message","content":[{"type":"text","text":"hello"}]}`))
	}))
	defer up.Close()
	r := registry.New()
	r.Put(&registry.Upstream{ID: "u", Protocol: "anthropic", APIKey: "provider-secret", BaseURL: up.URL + "/v1", Healthy: true})
	r.SetModels("u", []registry.Model{{ID: "native", Qualified: "manual/native", Upstream: "u"}})
	s := newServer(r, "test")
	s.apiKey, s.keyRequired = "gateway-secret", true
	h := s.routes()
	for _, conflict := range []bool{false, true} {
		req := httptest.NewRequest("POST", "/v1/messages", strings.NewReader(`{"model":"manual/native","messages":[{"role":"user","content":"hi"}],"max_tokens":5}`))
		req.Header.Set("X-Api-Key", "gateway-secret")
		req.Header.Set("Cookie", "private-cookie")
		if conflict {
			req.Header.Set("Authorization", "Bearer different")
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, req)
		want := 200
		if conflict {
			want = 401
		}
		if w.Code != want {
			t.Fatalf("status %d: %s", w.Code, w.Body.String())
		}
	}
	if calls != 1 {
		t.Fatalf("unexpected upstream calls: %d", calls)
	}
}

func TestChatBridgeRejectsStatefulRequestsBeforeCallingUpstream(t *testing.T) {
	u := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { t.Error("must not call upstream") }))
	defer u.Close()
	r := registry.New()
	r.Put(&registry.Upstream{ID: "u", Source: registry.SourceModelHub, BaseURL: u.URL + "/v1", Healthy: true})
	r.SetModels("u", []registry.Model{{ID: "native", Qualified: "local/native", Upstream: "u"}})
	req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(`{"model":"local/native","input":"hi","store":true}`))
	w := httptest.NewRecorder()
	newServer(r, "test").routes().ServeHTTP(w, req)
	if w.Code != 400 {
		t.Fatalf("status %d", w.Code)
	}
}

func TestAnthropicModelPagination(t *testing.T) {
	pages := 0
	u := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		pages++
		if r.Header.Get("X-Api-Key") != "provider-secret" || r.Header.Get("Authorization") != "" {
			t.Error("incorrect probe authentication")
		}
		if r.URL.Query().Get("after_id") == "" {
			_, _ = w.Write([]byte(`{"data":[{"id":"a"}],"has_more":true,"last_id":"a"}`))
		} else {
			_, _ = w.Write([]byte(`{"data":[{"id":"b"}],"has_more":false,"last_id":"b"}`))
		}
	}))
	defer u.Close()
	models, err := fetchUpstreamModels(registry.Upstream{Protocol: "anthropic", APIKey: "provider-secret", BaseURL: u.URL + "/v1"})
	if err != nil || pages != 2 || len(models) != 2 {
		t.Fatalf("pagination: %v %d %v", err, pages, models)
	}
}

func TestPinnedChatUpstreamUsesConversion(t *testing.T) {
	u := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/chat/completions" {
			t.Error("wrong protocol")
		}
		var body jsonObject
		_ = json.NewDecoder(r.Body).Decode(&body)
		if body["model"] != "unlisted-native" {
			t.Error("wrong native model")
		}
		_, _ = w.Write([]byte(chatTextResult))
	}))
	defer u.Close()
	r := registry.New()
	r.Put(&registry.Upstream{ID: "u", Protocol: "chat_completions", BaseURL: u.URL + "/v1", Healthy: true})
	s := newServer(r, "test")
	s.apiKey = "gateway-secret"
	req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(`{"model":"unlisted-native","input":"hi"}`))
	req.Header.Set("Authorization", "Bearer gateway-secret")
	req.Header.Set("X-VRouter-Upstream", "u")
	w := httptest.NewRecorder()
	s.routes().ServeHTTP(w, req)
	if w.Code != 200 || !strings.Contains(w.Body.String(), `"object":"response"`) {
		t.Fatalf("pinned conversion: %d %s", w.Code, w.Body.String())
	}
}
