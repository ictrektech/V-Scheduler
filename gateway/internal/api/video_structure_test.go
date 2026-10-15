package api

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestVideoStructureDiscoveryAppearsInManagementListsOnly(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	reg := registry.New()
	reg.SetService(registry.ServiceVideoStructureDetect, "http://video-structure-detect:8910")
	reg.SetService(registry.ServiceVideoStructureStructure, "http://video-structure:8911")
	handler := newServer(reg, "test").routes()

	request := func(path string, target any) {
		t.Helper()
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, httptest.NewRequest(http.MethodGet, path, nil))
		if w.Code != http.StatusOK {
			t.Fatalf("GET %s: status=%d body=%s", path, w.Code, w.Body.String())
		}
		if err := json.Unmarshal(w.Body.Bytes(), target); err != nil {
			t.Fatalf("GET %s: decode: %v", path, err)
		}
	}

	var upstreams struct {
		Upstreams []struct {
			ID     string `json:"id"`
			Source string `json:"source"`
		} `json:"upstreams"`
	}
	request("/api/v1/upstreams", &upstreams)
	upstreamIDs := map[string]bool{}
	for _, upstream := range upstreams.Upstreams {
		if upstream.Source == "video-structure" {
			upstreamIDs[upstream.ID] = true
		}
	}
	if !upstreamIDs[registry.ServiceVideoStructureDetect] || !upstreamIDs[registry.ServiceVideoStructureStructure] {
		t.Fatalf("video structure upstreams missing from management list: %+v", upstreams.Upstreams)
	}

	var models struct {
		Models []struct {
			Qualified string `json:"qualified"`
			Kind      string `json:"kind"`
		} `json:"models"`
	}
	request("/api/v1/models", &models)
	wantInterfaces := map[string]bool{
		"POST /api/v1/scenes/detect":                         false,
		"POST /api/v2/scenes/detect/jobs":                    false,
		"GET /api/v2/scenes/detect/jobs/{job_id}":            false,
		"POST /api/v2/video/structure/jobs":                  false,
		"GET /api/v2/video/structure/jobs/{job_id}":          false,
		"GET /api/v2/video/structure/jobs/{job_id}/artifact": false,
		"DELETE /api/v2/video/structure/jobs/{job_id}":       false,
		"POST /api/v2/video/describe/jobs":                   false,
		"GET /api/v2/video/describe/jobs/{job_id}":           false,
	}
	for _, model := range models.Models {
		if model.Kind == "service" {
			if _, ok := wantInterfaces[model.Qualified]; ok {
				wantInterfaces[model.Qualified] = true
			}
		}
	}
	for endpoint, found := range wantInterfaces {
		if !found {
			t.Errorf("video structure interface missing from management model list: %s", endpoint)
		}
	}

	var publicModels struct {
		Data []struct {
			ID string `json:"id"`
		} `json:"data"`
	}
	request("/v1/models", &publicModels)
	for _, model := range publicModels.Data {
		if strings.Contains(model.ID, "video") || strings.Contains(model.ID, "/api/") {
			t.Fatalf("business interface leaked into OpenAI model routing: %+v", publicModels.Data)
		}
	}
}

func TestVideoStructureAPIsProxyWithoutEnteringModelRouting(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	t.Setenv("V_ROUTER_REQUIRE_KEY", "true")
	t.Setenv("V_ROUTER_API_KEY", "gateway-token")
	var gotPath, gotAuth, gotBody string
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath, gotAuth = r.URL.Path, r.Header.Get("Authorization")
		body, _ := io.ReadAll(r.Body)
		gotBody = string(body)
		if strings.HasSuffix(r.URL.Path, "/artifact") {
			w.Header().Set("Content-Type", "application/x-tar")
			w.Header().Set("X-Artifact-Describe-Status", "succeeded")
			_, _ = w.Write([]byte{0, 1, 255, 4})
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"code":0,"data":{"job_id":"job-1","poll_url":"/api/v2/video/structure/jobs/job-1"}}`)
	}))
	defer upstream.Close()

	reg := registry.New()
	reg.SetService(registry.ServiceVideoStructureDetect, upstream.URL)
	reg.SetService(registry.ServiceVideoStructureStructure, upstream.URL)
	handler := newServer(reg, "test").routes()

	req := httptest.NewRequest(http.MethodPost, "/api/v2/video/structure/jobs", strings.NewReader(`{"video_path":"https://example/video.mp4","model_cfg":{"base_url":"http://v-router:8080/v1","model":"model-hub/vlm"}}`))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer gateway-token")
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusOK || gotPath != "/api/v2/video/structure/jobs" || gotAuth != "" || !strings.Contains(gotBody, `"base_url":"http://v-router:8080/v1"`) || !strings.Contains(w.Body.String(), `"poll_url":"/api/v2/video/structure/jobs/job-1"`) {
		t.Fatalf("structure proxy mismatch: status=%d path=%q auth=%q request=%s response=%s", w.Code, gotPath, gotAuth, gotBody, w.Body.String())
	}
	if len(reg.ListModels()) != 0 {
		t.Fatal("video structure must not be registered as a model upstream")
	}

	req = httptest.NewRequest(http.MethodGet, "/api/v2/video/structure/jobs/job-1/artifact", nil)
	req.Header.Set("Authorization", "Bearer gateway-token")
	w = httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusOK || w.Header().Get("Content-Type") != "application/x-tar" || !bytes.Equal(w.Body.Bytes(), []byte{0, 1, 255, 4}) {
		t.Fatalf("artifact proxy mismatch: status=%d headers=%v body=%v", w.Code, w.Header(), w.Body.Bytes())
	}
}

func TestVideoStructureUnavailableIsAQuietDiscoveryState(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	handler := newServer(registry.New(), "test").routes()
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, httptest.NewRequest(http.MethodGet, "/api/v1/video-structure/status", nil))
	if w.Code != http.StatusOK || w.Body.String() != `{"available":false,"services":{"scene_detect":false,"structure":false}}`+"\n" {
		t.Fatalf("unexpected status: %d %s", w.Code, w.Body.String())
	}
	w = httptest.NewRecorder()
	handler.ServeHTTP(w, httptest.NewRequest(http.MethodPost, "/api/v2/scenes/detect/jobs", strings.NewReader(`{}`)))
	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("missing service request: %d %s", w.Code, w.Body.String())
	}
}
