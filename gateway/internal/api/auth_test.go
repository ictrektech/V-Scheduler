package api

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

// fakeUserinfo mimics VOS /v1000/oauth2/userinfo: only the magic token is
// valid and maps to user alice.
func fakeUserinfo(t *testing.T) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Header.Get("Authorization") {
		case "Bearer good-token":
			_ = json.NewEncoder(w).Encode(map[string]string{
				"sub":                "user-123",
				"preferred_username": "alice",
				"email":              "alice@example.com",
			})
		case "Bearer bob-token":
			_ = json.NewEncoder(w).Encode(map[string]string{
				"sub":                "user-456",
				"preferred_username": "bob",
				"email":              "bob@example.com",
			})
		default:
			w.WriteHeader(http.StatusUnauthorized)
		}
	}))
	t.Cleanup(srv.Close)
	return srv
}

func newAuthTestServer(t *testing.T) *httptest.Server {
	t.Helper()
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	t.Setenv("VOS_OIDC_USERINFO_URL", fakeUserinfo(t).URL)
	t.Setenv("V_ROUTER_AUTH_DISABLED", "")
	s := newServer(registry.New(), "test")
	s.allowLoopbackUpstreams = true
	srv := httptest.NewServer(s.routes())
	t.Cleanup(srv.Close)
	return srv
}

func TestManagementRequiresIdentity(t *testing.T) {
	srv := newAuthTestServer(t)

	resp, body := doJSON(t, "GET", srv.URL+"/api/v1/status", nil)
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("anonymous management call must be 401, got %d %s", resp.StatusCode, body)
	}
}

// doJSONAuth issues a request with an explicit bearer token.
func doJSONAuth(t *testing.T, method, url, token string, body any) (*http.Response, string) {
	t.Helper()
	b, err := json.Marshal(body)
	if err != nil {
		t.Fatal(err)
	}
	req, err := http.NewRequest(method, url, strings.NewReader(string(b)))
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+token)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	out, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()
	return resp, string(out)
}

func TestOIDCIdentityFlow(t *testing.T) {
	srv := newAuthTestServer(t)

	// Bad token rejected on management API.
	resp, _ := doJSONAuth(t, "GET", srv.URL+"/api/v1/status", "bad-token", nil)
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("bad token must be 401, got %d", resp.StatusCode)
	}

	// Valid OIDC token passes and identifies the user.
	resp, body := doJSONAuth(t, "GET", srv.URL+"/api/v1/status", "good-token", nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("good token must pass, got %d %s", resp.StatusCode, body)
	}
	var st struct {
		User        string `json:"user"`
		OIDCEnabled bool   `json:"oidc_enabled"`
		IsAdmin     bool   `json:"is_admin"`
	}
	if err := json.Unmarshal([]byte(body), &st); err != nil {
		t.Fatal(err)
	}
	if st.User != "alice" || !st.OIDCEnabled || st.IsAdmin {
		t.Fatalf("status must reflect alice via OIDC: %+v", st)
	}

	// Inference calls attribute usage per user.
	_, upBase := newUpstreamForAuth(t, srv)
	_ = upBase
	_, body = doJSONAuth(t, "POST", srv.URL+"/v1/chat/completions", "good-token",
		map[string]any{"model": "manual/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hi"}}})
	if !strings.Contains(body, "echo:gpt-4o") {
		t.Fatalf("chat via oidc user failed: %s", body)
	}
	// Anonymous inference still works and is attributed to internal.
	_, body = doJSON(t, "POST", srv.URL+"/v1/chat/completions",
		map[string]any{"model": "manual/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hi"}}})
	if !strings.Contains(body, "echo:gpt-4o") {
		t.Fatalf("anonymous chat failed: %s", body)
	}

	_, body = doJSONAuth(t, "GET", srv.URL+"/api/v1/usage", "good-token", nil)
	var usage struct {
		PerUser map[string]struct {
			Requests int64 `json:"requests"`
		} `json:"per_user"`
		Recent []struct {
			User string `json:"user"`
		} `json:"recent"`
	}
	if err := json.Unmarshal([]byte(body), &usage); err != nil {
		t.Fatal(err)
	}
	if usage.PerUser["alice"].Requests != 1 || usage.PerUser["internal"].Requests != 1 {
		t.Fatalf("per-user usage wrong: %+v", usage.PerUser)
	}
	if len(usage.Recent) != 2 || usage.Recent[0].User != "internal" || usage.Recent[1].User != "alice" {
		t.Fatalf("recent user attribution wrong: %+v", usage.Recent)
	}
}

// newUpstreamForAuth registers the shared fake upstream through the
// authenticated management API and returns the server plus its base URL.
func newUpstreamForAuth(t *testing.T, srv *httptest.Server) (*httptest.Server, string) {
	t.Helper()
	up := fakeUpstream(t)
	resp, body := doJSONAuth(t, "POST", srv.URL+"/api/v1/upstreams", "good-token",
		map[string]string{"name": "Fake", "base_url": up.URL + "/v1"})
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("create upstream as alice failed: %d %s", resp.StatusCode, body)
	}
	return srv, up.URL + "/v1"
}

func TestGatewayKeyAdmin(t *testing.T) {
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	t.Setenv("V_ROUTER_AUTH_DISABLED", "")
	t.Setenv("V_ROUTER_REQUIRE_KEY", "true")
	t.Setenv("V_ROUTER_API_KEY", "sk-admin")
	s := newServer(registry.New(), "test")
	s.allowLoopbackUpstreams = true
	s.userInfoURL = fakeUserinfo(t).URL
	srv := httptest.NewServer(s.routes())
	t.Cleanup(srv.Close)

	// Key-less inference is rejected when the key gate is on.
	resp, _ := doJSON(t, "GET", srv.URL+"/v1/models", nil)
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("key-less inference must be 401 under key mode, got %d", resp.StatusCode)
	}

	// The gateway key authenticates as admin on management.
	resp, body := doJSONAuth(t, "GET", srv.URL+"/api/v1/status", "sk-admin", nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("admin key must pass management, got %d %s", resp.StatusCode, body)
	}
	if !strings.Contains(body, `"is_admin":true`) {
		t.Fatalf("key identity must be admin: %s", body)
	}

	// An OIDC user token also works when the key gate is on.
	resp, _ = doJSONAuth(t, "GET", srv.URL+"/api/v1/status", "good-token", nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("oidc user must pass management under key mode, got %d", resp.StatusCode)
	}
}

// auth-probe is public and distinguishes an unreachable userinfo endpoint
// from an invalid token.
func TestAuthProbe(t *testing.T) {
	srv := newAuthTestServer(t)

	// The fake userinfo answers (401 for tokenless GET still counts as reachable).
	resp, body := doJSON(t, "GET", srv.URL+"/api/v1/auth-probe", nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("auth-probe must be public, got %d", resp.StatusCode)
	}
	var probe struct {
		Version       string `json:"version"`
		OIDCEnabled   bool   `json:"oidc_enabled"`
		OIDCReachable bool   `json:"oidc_reachable"`
	}
	if err := json.Unmarshal([]byte(body), &probe); err != nil {
		t.Fatal(err)
	}
	if !probe.OIDCEnabled || !probe.OIDCReachable {
		t.Fatalf("fake userinfo must probe reachable: %+v", probe)
	}
	if probe.Version != "test" {
		t.Fatalf("auth-probe must report version: %+v", probe)
	}
}

func TestMeAndInvalidTokenNeverAnonymous(t *testing.T) {
	t.Setenv("V_ROUTER_REQUIRE_KEY", "false")
	srv := newAuthTestServer(t)
	resp, body := doJSONAuth(t, "GET", srv.URL+"/api/v1/auth/me", "good-token", nil)
	if resp.StatusCode != 200 || !strings.Contains(body, `"username":"alice"`) || !strings.Contains(body, `"resource_isolation":true`) {
		t.Fatalf("me: %d %s", resp.StatusCode, body)
	}
	resp, _ = doJSONAuth(t, "GET", srv.URL+"/v1/models", "bad-token", nil)
	if resp.StatusCode != 401 {
		t.Fatal("invalid token downgraded to anonymous")
	}
	resp, _ = doJSON(t, "GET", srv.URL+"/v1/models", nil)
	if resp.StatusCode != 200 {
		t.Fatal("anonymous policy changed")
	}
}

func TestRequiredEmptyKeyFailsClosed(t *testing.T) {
	t.Setenv("V_ROUTER_REQUIRE_KEY", "true")
	t.Setenv("V_ROUTER_API_KEY", "")
	srv := newAuthTestServer(t)
	resp, _ := doJSON(t, "GET", srv.URL+"/v1/models", nil)
	if resp.StatusCode != 401 {
		t.Fatal("required empty key allowed anonymous request")
	}
	resp, _ = doJSONAuth(t, "GET", srv.URL+"/v1/models", "good-token", nil)
	if resp.StatusCode != 200 {
		t.Fatal("valid OIDC token must work when key is required")
	}
}
