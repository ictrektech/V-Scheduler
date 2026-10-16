package api

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestUnifiedControllerPreservesContractAndAuthentication(t *testing.T) {
	controller := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/assets/app.js" {
			w.Write([]byte("unified-ui"))
			return
		}
		if r.URL.RequestURI() != "/api/v1/tasks?cursor=7" || r.Method != "POST" || r.Header.Get("Authorization") != "Bearer caller" || r.Header.Get("Idempotency-Key") != "stable" {
			t.Errorf("controller contract changed: %s %s", r.Method, r.URL.RequestURI())
		}
		body, _ := io.ReadAll(r.Body)
		if string(body) != `{"app_task_id":"job"}` {
			t.Error("request body changed")
		}
		w.Header().Set("X-Controller", "true")
		w.WriteHeader(http.StatusAccepted)
		w.Write([]byte(`{"status":"queued"}`))
	}))
	defer controller.Close()
	t.Setenv("V_ROUTER_CONTROLLER_URL", controller.URL)
	mux := http.NewServeMux()
	mux.HandleFunc("GET /v1/models", func(w http.ResponseWriter, _ *http.Request) { w.Write([]byte("gateway-models")) })
	unifiedController(mux)
	req := httptest.NewRequest("POST", "/scheduler/api/v1/tasks?cursor=7", strings.NewReader(`{"app_task_id":"job"}`))
	req.Header.Set("Authorization", "Bearer caller")
	req.Header.Set("Idempotency-Key", "stable")
	response := httptest.NewRecorder()
	mux.ServeHTTP(response, req)
	if response.Code != 202 || response.Header().Get("X-Controller") != "true" || response.Body.String() != `{"status":"queued"}` {
		t.Fatalf("controller response changed: %d %s", response.Code, response.Body.String())
	}
	for path, expected := range map[string]string{"/v1/models": "gateway-models", "/assets/app.js": "unified-ui"} {
		r := httptest.NewRecorder()
		mux.ServeHTTP(r, httptest.NewRequest("GET", path, nil))
		if r.Code != 200 || r.Body.String() != expected {
			t.Fatalf("%s: %d %s", path, r.Code, r.Body.String())
		}
	}
}

func TestUnifiedControllerFailureDoesNotBlockGateway(t *testing.T) {
	t.Setenv("V_ROUTER_CONTROLLER_URL", "http://127.0.0.1:1")
	mux := http.NewServeMux()
	mux.HandleFunc("GET /v1/models", func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(200) })
	unifiedController(mux)
	for path, expected := range map[string]int{"/scheduler/api/v1/nodes": 503, "/v1/models": 200} {
		r := httptest.NewRecorder()
		mux.ServeHTTP(r, httptest.NewRequest("GET", path, nil))
		if r.Code != expected {
			t.Fatalf("%s: %d", path, r.Code)
		}
	}
	r := httptest.NewRecorder()
	mux.ServeHTTP(r, httptest.NewRequest("GET", "/api/v1/tasks", nil))
	if r.Code != 404 {
		t.Fatalf("controller API leaked outside prefix: %d", r.Code)
	}
}

func TestUnifiedModelManagementHasSeparateTaskNamespace(t *testing.T) {
	storage := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.RequestURI() != "/api/v1/tasks?x=7" {
			t.Errorf("changed model management URL: %s", r.URL.RequestURI())
		}
		if r.Header.Get("Authorization") != "Bearer caller" {
			t.Error("header lost")
		}
		w.Write([]byte(`[{"task_id":"download-1"}]`))
	}))
	defer storage.Close()
	t.Setenv("V_ROUTER_CONTROLLER_URL", "http://127.0.0.1:1")
	t.Setenv("V_SCHEDULER_MODEL_MANAGEMENT_URL", storage.URL)
	mux := http.NewServeMux()
	unifiedController(mux)
	req := httptest.NewRequest("GET", "/model-hub/api/v1/tasks?x=7", nil)
	req.Header.Set("Authorization", "Bearer caller")
	response := httptest.NewRecorder()
	mux.ServeHTTP(response, req)
	if response.Code != 200 || !strings.Contains(response.Body.String(), "download-1") {
		t.Fatalf("model management proxy: %d %s", response.Code, response.Body.String())
	}
}
