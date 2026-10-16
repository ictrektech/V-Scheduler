package proxy

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"github.com/ictrektech/v-router/internal/registry"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestVSubBindingRequiresFullNamespaceAndVerifiedOwner(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("V_ROUTER_STATE_PATH", filepath.Join(dir, "state.json"))
	owner := "fixture-user"
	ownerHash := sha256.Sum256([]byte(owner))
	namespaceHash := sha256.Sum256([]byte("v-sub\x00" + owner))
	namespace := hex.EncodeToString(namespaceHash[:])
	id := "v-sub-" + namespace[:12] + "/builtin/tiny-q5_1"
	upstream := registry.Upstream{ID: "app-v-sub-" + hex.EncodeToString(ownerHash[:]), Owner: owner, Source: registry.SourceVSub, BaseURL: "http://v-sub-backend:7090/inference/" + namespace + "/v1"}
	if _, exists, _ := vsubSchedulerConfig(upstream); exists {
		t.Fatal("missing binding invented")
	}
	state := map[string]any{"protocol_version": 1, "app_id": "com.ictrek.v-sub", "namespaces": map[string]any{namespace: map[string]any{"url": "http://v-router:8080/scheduler/api/v1", "token": strings.Repeat("x", 64), "require_model_policy": true, "models": map[string]any{id: map[string]any{"mode": "permit", "executor_id": "app-gateway-v-sub-fixture", "completion": "synchronous", "upstream_url": upstream.BaseURL, "max_input_bytes": 1048576}}}}}
	data, _ := json.Marshal(state)
	path := filepath.Join(dir, "vsub-scheduler.local.json")
	if err := os.WriteFile(path, data, 0600); err != nil {
		t.Fatal(err)
	}
	cfg, exists, err := vsubSchedulerConfig(upstream)
	if !exists || err != nil {
		t.Fatalf("valid binding rejected: %v", err)
	}
	if cfg.Models[upstream.ID+"/"+id].ExecutorID != "app-gateway-v-sub-fixture" {
		t.Fatal("native model not bound to verified upstream")
	}
	other := upstream
	other.Owner = "other-user"
	if _, _, err := vsubSchedulerConfig(other); err == nil {
		t.Fatal("another owner borrowed private namespace")
	}
	other = upstream
	other.BaseURL = "http://v-sub-backend:7090/inference/" + namespace[:12] + "/v1"
	if _, _, err := vsubSchedulerConfig(other); err == nil {
		t.Fatal("display prefix used as security boundary")
	}
	other = upstream
	other.Source = registry.SourceAPI
	if _, exists, _ := vsubSchedulerConfig(other); exists {
		t.Fatal("manual upstream borrowed automatic app identity")
	}
	if err := os.Chmod(path, 0644); err != nil {
		t.Fatal(err)
	}
	if _, _, err := vsubSchedulerConfig(upstream); err == nil {
		t.Fatal("nonprivate credential file accepted")
	}
}
