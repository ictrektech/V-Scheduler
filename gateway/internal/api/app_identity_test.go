package api

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestApplicationCredentialIsInferenceOnlyAndRevokesWithoutCache(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("V_ROUTER_STATE_PATH", filepath.Join(dir, "state.json"))
	t.Setenv("V_ROUTER_REQUIRE_KEY", "true")
	t.Setenv("V_ROUTER_API_KEY", "")
	token := "vs-app-" + strings.Repeat("a", 64)
	hash := sha256.Sum256([]byte(token))
	snapshot := map[string]any{"protocol_version": 1, "binding_revision": 1, "app_id": "com.ictrek.hybrag", "scope": "gateway-inference-v1", "enabled": true, "token_hash": hex.EncodeToString(hash[:])}
	path := filepath.Join(dir, "app-identities", "hybrag.json")
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		t.Fatal(err)
	}
	save := func() {
		data, _ := json.Marshal(snapshot)
		if err := os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	save()
	srv := newAuthTestServer(t)
	for _, tc := range []struct {
		method, path string
		status       int
	}{
		{"GET", "/v1/models", 200}, {"GET", "/api/v1/status", 403}, {"POST", "/v1/images/generations", 403},
	} {
		res, _ := doJSONAuth(t, tc.method, srv.URL+tc.path, token, map[string]any{"model": "unknown"})
		if res.StatusCode != tc.status {
			t.Fatalf("%s %s: got %d want %d", tc.method, tc.path, res.StatusCode, tc.status)
		}
	}
	for _, path := range []string{"/v1/chat/completions", "/v1/embeddings", "/v1/rerank"} {
		req, _ := http.NewRequest("POST", path, nil)
		if !applicationInferenceAllowed(Identity{ServiceApp: "com.ictrek.hybrag"}, req) {
			t.Fatal("allowed inference path denied")
		}
	}
	if id, ok := applicationIdentity(token); !ok || id.Admin {
		t.Fatal("application identity missing or elevated")
	}
	snapshot["enabled"] = false
	save()
	res, _ := doJSONAuth(t, "GET", srv.URL+"/v1/models", token, nil)
	if res.StatusCode != 401 {
		t.Fatal("revoked credential survived request boundary")
	}
	snapshot["enabled"] = true
	save()
	if err := os.Chmod(path, 0644); err != nil {
		t.Fatal(err)
	}
	if _, ok := applicationIdentity(token); ok {
		t.Fatal("nonprivate identity trusted")
	}
	if err := os.Chmod(path, 0600); err != nil {
		t.Fatal(err)
	}
	snapshot["app_id"] = "other-app"
	save()
	if _, ok := applicationIdentity(token); ok {
		t.Fatal("wrong application trusted")
	}
	snapshot["app_id"] = "com.ictrek.hybrag"
	snapshot["unknown"] = true
	save()
	if _, ok := applicationIdentity(token); ok {
		t.Fatal("unknown identity fields trusted")
	}
}
