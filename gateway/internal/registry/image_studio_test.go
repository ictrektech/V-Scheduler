package registry

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestImageStudioDiscoveryExposesOnlyReadyModelsAndVariants(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/v1/models" {
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
		_ = json.NewEncoder(w).Encode([]map[string]any{
			{"key": "sd-turbo", "status": "ready", "capabilities": []string{"txt2img", "img2img"}},
			{"key": "wan21", "status": "missing", "capabilities": []string{"video"}},
			{"key": "sd15", "status": "missing", "capabilities": []string{"txt2img", "img2img"}, "variants": []map[string]any{
				{"id": "q8_0", "status": "ready"},
				{"id": "q4_0", "status": "missing"},
			}},
		})
	}))
	defer srv.Close()

	r := New()
	NewImageStudioPoller(r, srv.URL, time.Minute).pollOnce(context.Background())

	models := r.ListModels()
	got := map[string]Model{}
	for _, m := range models {
		got[m.Qualified] = m
	}
	if _, ok := got["image-studio/sd-turbo"]; !ok {
		t.Fatalf("ready base model missing: %#v", models)
	}
	if _, ok := got["image-studio/sd15@q8_0"]; !ok {
		t.Fatalf("ready variant missing: %#v", models)
	}
	if _, ok := got["image-studio/wan21"]; ok {
		t.Fatalf("missing video model should not be exposed: %#v", models)
	}
	if got["image-studio/sd-turbo"].Endpoints[0] != "images/generations" {
		t.Fatalf("wrong endpoints: %#v", got["image-studio/sd-turbo"].Endpoints)
	}
}
