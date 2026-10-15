package api

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestGatewayCapacityProtectsControlPlaneAndRecovers(t *testing.T) {
	t.Setenv("V_ROUTER_MAX_ACTIVE_REQUESTS", "1")
	t.Setenv("V_ROUTER_MAX_BUFFERED_INPUT_MIB", "1")
	entered, release, done := make(chan struct{}), make(chan struct{}), make(chan struct{})
	handler := protectGateway(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/v1/chat/completions" {
			close(entered)
			<-release
		}
		w.WriteHeader(200)
	}))
	go func() {
		defer close(done)
		handler.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader("{}")))
	}()
	<-entered
	busy := httptest.NewRecorder()
	handler.ServeHTTP(busy, httptest.NewRequest("POST", "/v1/images/generations", strings.NewReader("{}")))
	if busy.Code != 429 {
		t.Fatalf("overflow request reached model: %d", busy.Code)
	}
	health := httptest.NewRecorder()
	handler.ServeHTTP(health, httptest.NewRequest("GET", "/health", nil))
	if health.Code != 200 {
		t.Fatal("model saturation blocked control plane")
	}
	close(release)
	<-done
	recovered := httptest.NewRecorder()
	handler.ServeHTTP(recovered, httptest.NewRequest("POST", "/v1/images/generations", strings.NewReader("{}")))
	if recovered.Code != 200 {
		t.Fatal("completed request did not return capacity")
	}
	unknown := httptest.NewRequest("POST", "/v1/images/generations", strings.NewReader("{}"))
	unknown.ContentLength = -1
	bounded := httptest.NewRecorder()
	handler.ServeHTTP(bounded, unknown)
	if bounded.Code != 429 {
		t.Fatal("unknown-length input evaded memory budget")
	}
}
