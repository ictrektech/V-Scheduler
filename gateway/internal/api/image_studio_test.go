package api

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestImageStudioGenerationBridge(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	var gotPath string
	var gotAuth string
	var gotBody map[string]any
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath = r.URL.Path
		gotAuth = r.Header.Get("Authorization")
		if r.URL.Path != "/api/v1/generate" {
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
		if err := json.NewDecoder(r.Body).Decode(&gotBody); err != nil {
			t.Fatal(err)
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"job_id": "job-1", "status": "queued", "outputs": []string{}})
	}))
	defer up.Close()

	r := registry.New()
	r.Put(&registry.Upstream{ID: "app-image-studio", Name: "Image Studio", Kind: registry.KindLocal, Source: registry.SourceImageStudio, Protocol: registry.ProtocolImageStudio, ModelPrefix: registry.SourceImageStudio, BaseURL: up.URL, APIKey: "provider-key", Healthy: true})
	r.SetModels("app-image-studio", []registry.Model{{ID: "sd15@q8_0", Qualified: "image-studio/sd15@q8_0", Upstream: "app-image-studio", Capabilities: []string{"image_generation"}, Endpoints: []string{"images/generations"}}})
	h := newServer(r, "test").routes()

	req := httptest.NewRequest("POST", "/v1/images/generations", strings.NewReader(`{"model":"image-studio/sd15@q8_0","prompt":"cat","size":"512x768","n":2}`))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	h.ServeHTTP(w, req)

	if w.Code != http.StatusAccepted {
		t.Fatalf("status %d body %s", w.Code, w.Body.String())
	}
	if gotPath != "/api/v1/generate" || gotAuth != "Bearer provider-key" {
		t.Fatalf("wrong upstream call path=%s auth=%s", gotPath, gotAuth)
	}
	if gotBody["model_key"] != "sd15" || gotBody["model_variant"] != "q8_0" || gotBody["prompt"] != "cat" || gotBody["width"].(float64) != 512 || gotBody["height"].(float64) != 768 || gotBody["batch_count"].(float64) != 2 {
		t.Fatalf("wrong image studio payload: %#v", gotBody)
	}
	if !strings.Contains(w.Body.String(), "job-1") {
		t.Fatalf("missing job response: %s", w.Body.String())
	}
}
