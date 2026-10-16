package proxy

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestApplicationBindingIsPrivateAndFixedToApplicationOrigin(t *testing.T) {
	directory := t.TempDir()
	t.Setenv("V_ROUTER_STATE_PATH", filepath.Join(directory, "state.json"))
	upstream := registry.Upstream{ID: "app-v-music", BaseURL: "http://v-music-backend:7080/v1"}
	if _, exists, _ := applicationSchedulerConfig(upstream); exists {
		t.Fatal("missing binding was invented")
	}
	if err := os.Mkdir(filepath.Join(directory, "app-bindings"), 0700); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(directory, "app-bindings/v-music.json")
	data := `{"url":"http://v-router:8080/scheduler/api/v1","token":"` + strings.Repeat("x", 64) + `","require_model_policy":true,"models":{"app-v-music/musicgen-small":{"mode":"permit","executor_id":"app-gateway-v-music-fixture","completion":"synchronous","upstream_url":"http://v-music-backend:7080/v1","max_input_bytes":8192}}}`
	if err := os.WriteFile(path, []byte(data), 0600); err != nil {
		t.Fatal(err)
	}
	if cfg, exists, err := applicationSchedulerConfig(upstream); err != nil || !exists || cfg.Token == "" {
		t.Fatalf("binding: %v %v", exists, err)
	}
	upstream.BaseURL = "http://untrusted/v1"
	if _, exists, err := applicationSchedulerConfig(upstream); !exists || err == nil {
		t.Fatal("changed origin accepted")
	}
	upstream.BaseURL = "http://v-music-backend:7080/v1"
	if err := os.Chmod(path, 0644); err != nil {
		t.Fatal(err)
	}
	if _, exists, err := applicationSchedulerConfig(upstream); !exists || err == nil {
		t.Fatal("public credentials accepted")
	}
	if err := os.Chmod(path, 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(strings.ReplaceAll(data, "app-v-music/musicgen-small", "app-image-studio/sd-turbo")), 0600); err != nil {
		t.Fatal(err)
	}
	if _, exists, err := applicationSchedulerConfig(upstream); !exists || err == nil {
		t.Fatal("another application model accepted")
	}
}
