package proxy

import (
	"encoding/json"
	"github.com/ictrektech/v-router/internal/registry"
	"io"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestBuiltinBindingPrivateScopedAndReloaded(t *testing.T) {
	t.Setenv("V_ROUTER_STATE_PATH", filepath.Join(t.TempDir(), "state.json"))
	model := "v-scheduler/ollama-qa/model:latest"
	path := BuiltinSchedulerPath(model)
	cfg := SchedulerConfig{URL: "http://v-scheduler:38120/api/v1", Token: strings.Repeat("a", 64), RequireModelPolicy: true, Models: map[string]registry.SchedulerPolicy{model: {Mode: "permit", ExecutorID: "builtin-ollama-one"}}}
	os.MkdirAll(filepath.Dir(path), 0700)
	data, _ := json.Marshal(cfg)
	os.WriteFile(path, data, 0600)
	got, exists, err := BuiltinSchedulerConfig(model)
	if err != nil || !exists || got.Token != cfg.Token {
		t.Fatal("valid binding missing", err)
	}
	cfg.Token = strings.Repeat("b", 64)
	data, _ = json.Marshal(cfg)
	os.WriteFile(path, data, 0600)
	got, _, _ = BuiltinSchedulerConfig(model)
	if got.Token != cfg.Token {
		t.Fatal("binding changes require restart")
	}
	os.Chmod(path, 0644)
	if _, _, err = BuiltinSchedulerConfig(model); err == nil {
		t.Fatal("public credential accepted")
	}
	os.Remove(path)
	os.Symlink("/etc/passwd", path)
	if _, _, err = BuiltinSchedulerConfig(model); err == nil {
		t.Fatal("symlink accepted")
	}
	if _, exists, _ := BuiltinSchedulerConfig(model + "/other"); exists {
		t.Fatal("model approval leaked")
	}
}
func TestBuiltinApprovalBoundsBackendDefaults(t *testing.T) {
	policy := registry.SchedulerPolicy{ExecutorID: "builtin-ollama-test", MaxInputBytes: 2048, Limits: map[string]float64{"max_tokens": 8, "options.num_ctx": 512, "options.num_predict": 8}}
	upstream := registry.Upstream{Source: registry.SourceSchedulerOllama, Role: registry.RoleGenerate}
	request := httptest.NewRequest("POST", "http://backend/v1/chat/completions", strings.NewReader(`{"model":"test","messages":[],"keep_alive":-1}`))
	request.Header.Set("Content-Type", "application/json")
	if err := boundBuiltinInput(request, upstream, policy); err != nil {
		t.Fatal(err)
	}
	raw, _ := io.ReadAll(request.Body)
	var value map[string]any
	json.Unmarshal(raw, &value)
	if value["max_tokens"] != float64(8) || value["keep_alive"] != float64(0) || value["options"].(map[string]any)["num_ctx"] != float64(512) {
		t.Fatalf("backend defaults escaped approval: %s", raw)
	}
	request = httptest.NewRequest("POST", "http://backend", strings.NewReader(`{"max_tokens":100}`))
	request.Header.Set("Content-Type", "application/json")
	if err := boundBuiltinInput(request, upstream, policy); AdmissionStatus(err) != 422 {
		t.Fatal("oversized generation admitted", err)
	}
}

func TestAutomaticBuiltinRetainsInstalledInferenceParameters(t *testing.T) {
	policy := registry.SchedulerPolicy{ExecutorID: "builtin-ollama-auto-test", MaxInputBytes: 32 << 20}
	upstream := registry.Upstream{Source: registry.SourceSchedulerOllama, Role: registry.RoleGenerate}
	request := httptest.NewRequest("POST", "http://backend/v1/chat/completions", strings.NewReader(`{"max_tokens":4096,"options":{"num_ctx":24576},"keep_alive":-1}`))
	request.Header.Set("Content-Type", "application/json")
	if err := boundBuiltinInput(request, upstream, policy); err != nil {
		t.Fatal(err)
	}
	var value map[string]any
	json.NewDecoder(request.Body).Decode(&value)
	if value["max_tokens"] != float64(4096) || value["options"].(map[string]any)["num_ctx"] != float64(24576) || value["keep_alive"] != float64(0) {
		t.Fatal("automatic registration changed caller's inference parameters")
	}
	request = httptest.NewRequest("POST", "http://backend", strings.NewReader(`{"messages":[]}`))
	request.Header.Set("Content-Type", "application/json")
	if err := boundBuiltinInput(request, upstream, policy); err != nil {
		t.Fatal(err)
	}
	value = nil
	json.NewDecoder(request.Body).Decode(&value)
	if _, ok := value["max_tokens"]; ok {
		t.Fatal("automatic binding inserted an output limit")
	}
	if _, ok := value["options"]; ok {
		t.Fatal("automatic binding inserted a context limit")
	}
}
