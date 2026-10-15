package api

import (
	"bytes"
	"encoding/json"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

// startTestServer exposes one Server on an HTTP test server.
func startTestServer(t *testing.T, s *Server) string {
	t.Helper()
	srv := httptest.NewServer(s.routes())
	t.Cleanup(srv.Close)
	return srv.URL
}

// createUpstreamAs creates a manual upstream through the API as the given
// identity and returns its id.
func createUpstreamAs(t *testing.T, srv *httptestLike, token, name, visibility string) string {
	t.Helper()
	resp, body := doJSONAuth(t, "POST", srv.URL+"/api/v1/upstreams", token, map[string]string{
		"name": name, "base_url": srv.upBase, "api_key": "sk-shared", "visibility": visibility,
	})
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("create upstream as %s: %d %s", token, resp.StatusCode, body)
	}
	var created struct {
		ID         string `json:"id"`
		Owner      string `json:"owner"`
		Visibility string `json:"visibility"`
		Status     struct {
			State string `json:"state"`
		} `json:"status"`
		Manageable bool `json:"manageable"`
	}
	if err := json.Unmarshal([]byte(body), &created); err != nil {
		t.Fatal(err)
	}
	if created.Owner == "" || created.Visibility != visibility {
		t.Fatalf("creator ownership and visibility must be recorded: %#v", created)
	}
	if !created.Manageable {
		t.Fatal("creator must be able to manage the new upstream")
	}
	if created.Status.State != registry.StateHealthy {
		t.Fatalf("healthy upstream status expected, got %#v", created.Status)
	}
	return created.ID
}

// httptestLike carries the pieces the ownership tests need from a test server.
type httptestLike struct {
	URL    string
	upBase string
}

func newOwnershipTestServer(t *testing.T) *httptestLike {
	t.Helper()
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	t.Setenv("VOS_OIDC_USERINFO_URL", fakeUserinfo(t).URL)
	t.Setenv("V_ROUTER_AUTH_DISABLED", "")
	up := fakeUpstream(t)
	s := newServer(registry.New(), "test")
	s.allowLoopbackUpstreams = true
	srv := startTestServer(t, s)
	return &httptestLike{URL: srv, upBase: up.URL + "/v1"}
}

func TestUpstreamOwnerBoundaries(t *testing.T) {
	ts := newOwnershipTestServer(t)
	sharedID := createUpstreamAs(t, ts, "good-token", "Alice Shared", "public")
	privateID := createUpstreamAs(t, ts, "good-token", "Alice Private", "private")

	// Private upstream models are hidden from bob: his rich model list has no
	// row from the private upstream, the owner's list does.
	resp, body := doJSONAuth(t, "GET", ts.URL+"/api/v1/models", "bob-token", nil)
	if resp.StatusCode != http.StatusOK || jsonContains(body, privateID) {
		t.Fatalf("private upstream leaked to another user: %d %s", resp.StatusCode, body)
	}
	resp, body = doJSONAuth(t, "GET", ts.URL+"/api/v1/models", "good-token", nil)
	if resp.StatusCode != http.StatusOK || !jsonContains(body, privateID) {
		t.Fatalf("owner must see private upstream models: %d %s", resp.StatusCode, body)
	}

	// The private upstream disappears from bob's management list entirely.
	_, body = doJSONAuth(t, "GET", ts.URL+"/api/v1/upstreams", "bob-token", nil)
	if jsonContains(body, privateID) {
		t.Fatal("private upstream must not appear in another user's list")
	}
	_, body = doJSONAuth(t, "GET", ts.URL+"/v1/models", "bob-token", nil)
	if !containsModel(body, "manual/gpt-4o") {
		t.Fatalf("shared upstream model must stay visible to bob: %s", body)
	}

	// Bob cannot edit or delete the shared upstream: visible but forbidden.
	resp, body = doJSONAuth(t, "PUT", ts.URL+"/api/v1/upstreams/"+sharedID, "bob-token", map[string]string{"name": "Hijacked", "base_url": ts.upBase})
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("editing a shared upstream must be 403, got %d %s", resp.StatusCode, body)
	}
	resp, body = doJSONAuth(t, "DELETE", ts.URL+"/api/v1/upstreams/"+sharedID, "bob-token", nil)
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("deleting a shared upstream must be 403, got %d %s", resp.StatusCode, body)
	}
	resp, body = doJSONAuth(t, "POST", ts.URL+"/api/v1/upstreams/"+sharedID+"/refresh", "bob-token", nil)
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("refreshing a shared upstream must be 403, got %d %s", resp.StatusCode, body)
	}
	resp, body = doJSONAuth(t, "PUT", ts.URL+"/api/v1/upstreams/"+sharedID+"/routing", "bob-token", map[string]any{"weight": 99})
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("changing shared routing must be 403, got %d %s", resp.StatusCode, body)
	}

	// Bob cannot even probe the private upstream's routing or metadata.
	resp, _ = doJSONAuth(t, "PUT", ts.URL+"/api/v1/upstreams/"+privateID+"/routing", "bob-token", map[string]any{"weight": 5})
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("private upstream routing must be 404 for others, got %d", resp.StatusCode)
	}

	// The owner keeps full control.
	resp, _ = doJSONAuth(t, "PUT", ts.URL+"/api/v1/upstreams/"+privateID, "good-token", map[string]string{"name": "Alice Private", "base_url": ts.upBase, "visibility": "public"})
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("owner update failed: %d", resp.StatusCode)
	}
	resp, _ = doJSONAuth(t, "DELETE", ts.URL+"/api/v1/upstreams/"+privateID, "good-token", nil)
	if resp.StatusCode != http.StatusNoContent {
		t.Fatalf("owner delete failed: %d", resp.StatusCode)
	}
}

func TestLegacyOwnerlessUpstreamStaysShared(t *testing.T) {
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	t.Setenv("VOS_OIDC_USERINFO_URL", fakeUserinfo(t).URL)
	t.Setenv("V_ROUTER_AUTH_DISABLED", "")
	up := fakeUpstream(t)
	s := newServer(registry.New(), "test")
	s.allowLoopbackUpstreams = true
	reg := s.reg
	// Pre-upgrade snapshot: an upstream without owner or visibility.
	reg.Put(&registry.Upstream{ID: "legacy", Name: "Legacy", Kind: registry.KindManual, Source: registry.SourceAPI, BaseURL: up.URL + "/v1", Healthy: true})
	reg.SetModels("legacy", []registry.Model{{ID: "gpt-4o", Qualified: "manual/gpt-4o", Upstream: "legacy", Kind: registry.ModelKindChat}})
	srv := startTestServer(t, s)

	// Bob still sees and may manage it, preserving upgrade behavior.
	resp, body := doJSONAuth(t, "GET", srv+"/v1/models", "bob-token", nil)
	if resp.StatusCode != http.StatusOK || !containsModel(body, "manual/gpt-4o") {
		t.Fatalf("legacy upstream must stay visible: %d %s", resp.StatusCode, body)
	}
	resp, body = doJSONAuth(t, "PUT", srv+"/api/v1/upstreams/legacy", "bob-token", map[string]string{"name": "Legacy", "base_url": up.URL + "/v1"})
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("legacy upstream must stay manageable by any logged-in user: %d %s", resp.StatusCode, body)
	}
	var updated struct {
		Owner string `json:"owner"`
	}
	_ = json.Unmarshal([]byte(body), &updated)
	if updated.Owner != "" {
		t.Fatalf("legacy ownership must not be silently claimed: %q", updated.Owner)
	}
}

func TestResourceOwnershipEnforcement(t *testing.T) {
	ts := newOwnershipTestServer(t)
	upstreamID := createUpstreamAs(t, ts, "good-token", "Alice Shared", "public")
	header := map[string]string{}

	// Alice creates a file through V-Router; the id is recorded with her as owner.
	resp, body := doJSONAuthPinned(t, "POST", ts.URL+"/v1/files", "good-token", upstreamID, header)
	if resp.StatusCode != http.StatusOK || !jsonContains(body, "file-new") {
		t.Fatalf("file creation failed: %d %s", resp.StatusCode, body)
	}

	// Bob cannot read or delete Alice's recorded resource.
	resp, _ = doJSONAuthPinned(t, "GET", ts.URL+"/v1/files/file-new", "bob-token", upstreamID, header)
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("cross-account resource read must be 404, got %d", resp.StatusCode)
	}
	resp, _ = doJSONAuthPinned(t, "DELETE", ts.URL+"/v1/files/file-new", "bob-token", upstreamID, header)
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("cross-account resource delete must be 404, got %d", resp.StatusCode)
	}

	// Resources without a V-Router record stay shared (pre-upgrade behavior).
	resp, _ = doJSONAuthPinned(t, "GET", ts.URL+"/v1/files/file-legacy", "bob-token", upstreamID, header)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("unrecorded resource must stay accessible: %d", resp.StatusCode)
	}

	// List endpoints keep returning the upstream account's full contents.
	resp, _ = doJSONAuthPinned(t, "GET", ts.URL+"/v1/files", "bob-token", upstreamID, header)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("list endpoints must stay passthrough: %d", resp.StatusCode)
	}
}

func doJSONAuthPinned(t *testing.T, method, url, token, upstream string, _ map[string]string) (*http.Response, string) {
	t.Helper()
	req, err := http.NewRequest(method, url, nil)
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("X-VRouter-Upstream", upstream)
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

func containsModel(body, qualified string) bool {
	var doc struct {
		Data []struct {
			ID string `json:"id"`
		} `json:"data"`
	}
	if err := json.Unmarshal([]byte(body), &doc); err != nil {
		return false
	}
	for _, m := range doc.Data {
		if m.ID == qualified {
			return true
		}
	}
	return false
}

func jsonContains(body, needle string) bool {
	return strings.Contains(body, needle)
}

// TestSpeakerModelRefRestored verifies that V-Voice's secondary model
// reference (speaker_model) loses the gateway namespace at the forwarding
// boundary, in multipart form and streaming query parameters alike.
func TestSpeakerModelRefRestored(t *testing.T) {
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	var got formEcho
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got.form = map[string]string{}
		got.query = r.URL.Query()
		ct := r.Header.Get("Content-Type")
		if strings.HasPrefix(ct, "multipart/form-data") {
			if _, params, err := mime.ParseMediaType(ct); err == nil {
				mr := multipart.NewReader(r.Body, params["boundary"])
				for {
					part, err := mr.NextRawPart()
					if err != nil {
						break
					}
					v, _ := io.ReadAll(io.LimitReader(part, 4096))
					got.form[part.FormName()] = string(v)
				}
			}
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"text": "ok"})
	}))
	defer up.Close()

	s := newServer(registry.New(), "test")
	s.allowLoopbackUpstreams = true
	s.reg.Register(registry.Upstream{
		ID: "app-v-voice", Name: "V-Voice", Kind: registry.KindLocal, Source: registry.SourceVVoice,
		BaseURL: up.URL + "/v1", Protocol: registry.ProtocolOpenAI, ModelPrefix: registry.SourceVVoice, Healthy: true,
	}, []registry.Model{
		{ID: "asr-1", Qualified: "v-voice/asr-1", Upstream: "app-v-voice", Endpoints: registry.NormalizeEndpoints([]string{"audio/transcriptions"})},
		{ID: "stream-1", Qualified: "v-voice/stream-1", Upstream: "app-v-voice", Endpoints: registry.NormalizeEndpoints([]string{"audio/streaming/transcriptions"})},
	})
	srv := startTestServer(t, s)

	// Multipart whole-sentence transcription with a prefixed speaker_model.
	body := &bytes.Buffer{}
	mw := multipart.NewWriter(body)
	_ = mw.WriteField("model", "v-voice/asr-1")
	_ = mw.WriteField("speaker_model", "v-voice/stream-1")
	_ = mw.Close()
	req, err := http.NewRequest("POST", srv+"/v1/audio/transcriptions", body)
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Content-Type", mw.FormDataContentType())
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	_, _ = io.Copy(io.Discard, resp.Body)
	_ = resp.Body.Close()
	if resp.StatusCode != http.StatusOK || got.form["model"] != "asr-1" || got.form["speaker_model"] != "stream-1" {
		t.Fatalf("multipart refs not restored: status=%d form=%v", resp.StatusCode, got.form)
	}

	// Streaming endpoint with prefixed model and speaker_model query params.
	resp, err = http.Post(srv+"/v1/audio/streaming/transcriptions?model=v-voice/stream-1&speaker_model=v-voice/asr-1", "application/octet-stream", strings.NewReader("pcm-bytes"))
	if err != nil {
		t.Fatal(err)
	}
	_, _ = io.Copy(io.Discard, resp.Body)
	_ = resp.Body.Close()
	if resp.StatusCode != http.StatusOK || got.query.Get("model") != "stream-1" || got.query.Get("speaker_model") != "asr-1" {
		t.Fatalf("query refs not restored: status=%d query=%v", resp.StatusCode, got.query)
	}
}

type formEcho struct {
	form  map[string]string
	query url.Values
}
