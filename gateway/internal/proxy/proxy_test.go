package proxy

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestBrowserOriginDoesNotReachUpstream(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Origin") != "" {
			w.WriteHeader(http.StatusForbidden)
			return
		}
		if r.URL.Path != "/v1/chat/completions" || r.Header.Get("Authorization") != "Bearer provider-key" {
			t.Errorf("incorrect upstream routing or credential")
		}
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = w.Write([]byte("data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n"))
	}))
	defer upstream.Close()
	req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"test"}`))
	req.Header.Set("Origin", "http://vos.example")
	req.Header.Set("Authorization", "Bearer vos-user-token")
	out := httptest.NewRecorder()
	To(out, req, registry.Upstream{BaseURL: upstream.URL + "/v1", APIKey: "provider-key"}, "/chat/completions")
	if out.Code != http.StatusOK || !strings.Contains(out.Body.String(), "hello") || !strings.Contains(out.Body.String(), "[DONE]") {
		t.Fatalf("browser inference failed: %d %s", out.Code, out.Body.String())
	}
}
