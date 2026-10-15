package api

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestRoutingHTTPStreamingCancellationAndPinnedLimit(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage"))
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		w.Header().Set("X-VRouter-Upstream", "forged")
		_, _ = w.Write([]byte("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n"))
		w.(http.Flusher).Flush()
		<-r.Context().Done()
	}))
	defer up.Close()
	reg := registry.New()
	for _, id := range []string{"a", "b"} {
		reg.Put(&registry.Upstream{ID: id, BaseURL: up.URL, Healthy: true, Kind: registry.KindManual})
		reg.SetModels(id, []registry.Model{{ID: "same", Qualified: "manual/same", Kind: registry.ModelKindChat}})
		_ = reg.SetRouting(id, registry.RoutingPolicy{MaxInflight: 1})
	}
	srv := httptest.NewServer(newServer(reg, "test").routes())
	defer srv.Close()
	open := func(pin string) (*http.Response, context.CancelFunc) {
		t.Helper()
		ctx, cancel := context.WithCancel(context.Background())
		req, _ := http.NewRequestWithContext(ctx, "POST", srv.URL+"/v1/chat/completions", strings.NewReader(`{"model":"manual/same","stream":true,"messages":[{"role":"user","content":"hello"}]}`))
		if pin != "" {
			req.Header.Set("X-VRouter-Upstream", pin)
		}
		res, e := http.DefaultClient.Do(req)
		if e != nil {
			cancel()
			t.Fatal(e)
		}
		return res, cancel
	}
	first, cancelFirst := open("")
	defer cancelFirst()
	defer first.Body.Close()
	if first.StatusCode != 200 || first.Header.Get("X-VRouter-Upstream") != "a" {
		t.Fatal(first.StatusCode, first.Header)
	}
	second, cancelSecond := open("")
	defer cancelSecond()
	defer second.Body.Close()
	if second.Header.Get("X-VRouter-Upstream") != "b" {
		t.Fatal(second.Header)
	}
	busy, cancelBusy := open("a")
	cancelBusy()
	busy.Body.Close()
	if busy.StatusCode != 429 || busy.Header.Get("Retry-After") == "" {
		t.Fatal(busy.StatusCode)
	}
	cancelFirst()
	first.Body.Close()
	cancelSecond()
	second.Body.Close()
	deadline := time.Now().Add(2 * time.Second)
	for {
		_, a := reg.Routing("a")
		_, b := reg.Routing("b")
		if a == 0 && b == 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("stream cancellation leaked reservation", a, b)
		}
		time.Sleep(time.Millisecond)
	}
	pinned, cancelPinned := open("b")
	if pinned.Header.Get("X-VRouter-Upstream") != "b" || pinned.Header.Get("X-VRouter-Routing") != "pinned" {
		t.Fatal(pinned.Header)
	}
	cancelPinned()
	pinned.Body.Close()
}
func TestRegistrationHTTPAuthUpdateExpiry(t *testing.T) {
	t.Setenv("V_ROUTER_REQUIRE_KEY", "true")
	t.Setenv("V_ROUTER_API_KEY", "registration-key")
	t.Setenv("V_ROUTER_AUTH_DISABLED", "false")
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage"))
	reg := registry.New()
	s := newServer(reg, "test")
	s.allowLoopbackUpstreams = true
	srv := httptest.NewServer(s.routes())
	defer srv.Close()
	up := fakeUpstream(t)
	call := func(method, path, body, key string) (int, string) {
		req, _ := http.NewRequest(method, srv.URL+path, strings.NewReader(body))
		if key != "" {
			req.Header.Set("Authorization", "Bearer "+key)
		}
		res, e := http.DefaultClient.Do(req)
		if e != nil {
			t.Fatal(e)
		}
		defer res.Body.Close()
		b, _ := io.ReadAll(res.Body)
		return res.StatusCode, string(b)
	}
	payload := `{"service_id":"vllm-1","base_url":"` + up.URL + `/v1","api_key":"provider-secret","protocol":"chat_completions","model_kind":"chat"}`
	if code, _ := call("POST", "/api/v1/registrations", payload, ""); code != 401 {
		t.Fatal(code)
	}
	for i := 0; i < 2; i++ {
		code, body := call("POST", "/api/v1/registrations", payload, "registration-key")
		if code != 200 || strings.Contains(body, "provider-secret") {
			t.Fatal(code, body)
		}
	}
	if len(reg.List()) != 1 {
		t.Fatal("registration not idempotent")
	}
	code, body := call("POST", "/v1/chat/completions", `{"model":"registered/gpt-4o","messages":[{"role":"user","content":"hi"}]}`, "registration-key")
	if code != 200 || !strings.Contains(body, "echo:gpt-4o") {
		t.Fatal(code, body)
	}
	u, _ := reg.Get("registered-vllm-1")
	u.ExpiresAt = time.Now().Add(-time.Second)
	_ = reg.Register(u, reg.ModelsOf(u.ID))
	if code, _ = call("POST", "/v1/chat/completions", `{"model":"registered/gpt-4o"}`, "registration-key"); code != 503 {
		t.Fatal(code)
	}
	if code, _ = call("POST", "/api/v1/registrations/registered-vllm-1/heartbeat", `{}`, "registration-key"); code != 404 {
		t.Fatal(code)
	}
	if code, _ = call("POST", "/api/v1/registrations", payload, "registration-key"); code != 200 {
		t.Fatal(code)
	}
	if code, _ = call("DELETE", "/api/v1/registrations/registered-vllm-1", "", "registration-key"); code != 204 {
		t.Fatal(code)
	}
	if len(reg.List()) != 0 {
		t.Fatal("unregister left upstream")
	}
}

func TestSingleModelRegistrationRequiresAndUsesEndpoints(t *testing.T) {
	t.Setenv("V_ROUTER_REQUIRE_KEY", "true")
	t.Setenv("V_ROUTER_API_KEY", "registration-key")
	t.Setenv("V_ROUTER_AUTH_DISABLED", "false")
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage"))
	reg := registry.New()
	s := newServer(reg, "test")
	s.allowLoopbackUpstreams = true
	srv := httptest.NewServer(s.routes())
	defer srv.Close()
	up := fakeUpstream(t)
	call := func(method, path, body string) (int, string) {
		req, _ := http.NewRequest(method, srv.URL+path, strings.NewReader(body))
		req.Header.Set("Authorization", "Bearer registration-key")
		res, e := http.DefaultClient.Do(req)
		if e != nil {
			t.Fatal(e)
		}
		defer res.Body.Close()
		b, _ := io.ReadAll(res.Body)
		return res.StatusCode, string(b)
	}
	missingEndpoints := `{"service_id":"single-1","base_url":"` + up.URL + `/v1","model":"gpt-4o"}`
	if code, body := call("POST", "/api/v1/registrations", missingEndpoints); code != http.StatusBadRequest || !strings.Contains(body, "must declare endpoints") {
		t.Fatalf("missing endpoints: %d %s", code, body)
	}
	payload := `{"service_id":"single-1","base_url":"` + up.URL + `/v1","model":"gpt-4o","endpoints":["embeddings"],"capabilities":["embedding"]}`
	if code, body := call("POST", "/api/v1/registrations", payload); code != http.StatusOK {
		t.Fatalf("single register: %d %s", code, body)
	}
	if code, _ := call("POST", "/v1/chat/completions", `{"model":"registered/gpt-4o","messages":[{"role":"user","content":"hi"}]}`); code != http.StatusBadRequest {
		t.Fatalf("chat should be unsupported for embedding-only registration: %d", code)
	}
	if code, body := call("POST", "/v1/embeddings", `{"model":"registered/gpt-4o","input":"hi"}`); code != http.StatusOK || !strings.Contains(body, `"path":"/v1/embeddings"`) {
		t.Fatalf("embeddings route: %d %s", code, body)
	}
}

func TestRegistrationVisibilityScopesModelsAndRouting(t *testing.T) {
	t.Setenv("V_ROUTER_REQUIRE_KEY", "false")
	srv := newAuthTestServer(t)
	up := fakeUpstream(t)
	payload := map[string]any{"service_id": "private-node", "visibility": "private", "base_url": up.URL + "/v1", "protocol": "chat_completions", "model_kind": "chat"}
	resp, body := doJSONAuth(t, "POST", srv.URL+"/api/v1/registrations", "good-token", payload)
	if resp.StatusCode != 200 {
		t.Fatalf("private register: %d %s", resp.StatusCode, body)
	}
	_, body = doJSONAuth(t, "GET", srv.URL+"/v1/models", "good-token", nil)
	if !strings.Contains(body, "registered/gpt-4o") {
		t.Fatalf("owner must see private model: %s", body)
	}
	_, body = doJSONAuth(t, "GET", srv.URL+"/v1/models", "bob-token", nil)
	if strings.Contains(body, "registered/gpt-4o") {
		t.Fatalf("other user saw private model: %s", body)
	}
	_, body = doJSON(t, "GET", srv.URL+"/v1/models", nil)
	if strings.Contains(body, "registered/gpt-4o") {
		t.Fatalf("anonymous saw private model: %s", body)
	}
	resp, _ = doJSONAuth(t, "POST", srv.URL+"/v1/chat/completions", "bob-token", map[string]any{"model": "registered/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hi"}}})
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("other user routed private model: %d", resp.StatusCode)
	}
	resp, body = doJSONAuth(t, "DELETE", srv.URL+"/api/v1/registrations/registered-private-node", "good-token", nil)
	if resp.StatusCode != http.StatusNoContent {
		t.Fatalf("delete private: %d %s", resp.StatusCode, body)
	}
	payload["visibility"] = "public"
	resp, body = doJSONAuth(t, "POST", srv.URL+"/api/v1/registrations", "good-token", payload)
	if resp.StatusCode != 200 {
		t.Fatalf("public register: %d %s", resp.StatusCode, body)
	}
	_, body = doJSONAuth(t, "GET", srv.URL+"/v1/models", "bob-token", nil)
	if !strings.Contains(body, "registered/gpt-4o") {
		t.Fatalf("other user must see public model: %s", body)
	}
	_, body = doJSON(t, "GET", srv.URL+"/v1/models", nil)
	if !strings.Contains(body, "registered/gpt-4o") {
		t.Fatalf("anonymous must see public model: %s", body)
	}
}

func TestModelListIncludesRequiredSDKFields(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "a", Healthy: true})
	reg.SetModels("a", []registry.Model{{ID: "m", Qualified: "local/m"}})
	srv := httptest.NewServer(newServer(reg, "test").routes())
	defer srv.Close()
	_, body := doJSON(t, "GET", srv.URL+"/v1/models", nil)
	if !strings.Contains(body, `"created":0`) || !strings.Contains(body, `"owned_by":""`) {
		t.Fatal(body)
	}
}
