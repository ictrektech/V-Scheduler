package registry

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestVSubDiscoveryPrivateAndPreferredOverRegistration(t *testing.T) {
	path := "/inference/" + strings.Repeat("a", 64) + "/v1"
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/v1/discovery" || r.Header.Get("Authorization") != "Bearer user-token" {
			w.WriteHeader(401)
			return
		}
		json.NewEncoder(w).Encode(map[string]any{"owner": "alice", "base_path": path, "api_key": "worker-secret", "models": []vVoiceModel{{ID: "local/asr", Kind: "asr", Endpoints: []string{"audio/transcriptions"}}}})
	}))
	defer srv.Close()
	r := New()
	legacy := Upstream{ID: "registered-v-sub-local", Source: SourceRegistered, Owner: "alice", Visibility: VisibilityPrivate, BaseURL: srv.URL + path, Healthy: true}
	if err := r.Register(legacy, []Model{{ID: "local/asr", Qualified: "registered/local/asr", Upstream: legacy.ID}}); err != nil {
		t.Fatal(err)
	}
	p := NewVSubDiscovery(r, []string{srv.URL})
	p.Refresh(context.Background(), "user-token", "alice")
	if _, ok := r.Get(legacy.ID); ok {
		t.Fatal("legacy registration duplicated discovered upstream")
	}
	if err := r.Register(legacy, nil); err != nil {
		t.Fatal(err)
	}
	if _, ok := r.Get(legacy.ID); ok {
		t.Fatal("registration replaced active discovery")
	}
	var discovered *Upstream
	for _, u := range r.List() {
		if u.Source == SourceVSub {
			copy := u
			discovered = &copy
		}
	}
	if discovered == nil || discovered.VisibleTo("bob", false) || discovered.VisibleTo("", false) || !discovered.VisibleTo("alice", false) {
		t.Fatalf("private visibility: %#v", discovered)
	}
	models := r.ModelsOf(discovered.ID)
	if len(models) != 1 || models[0].Qualified != "v-sub/local/asr" || discovered.APIKey != "worker-secret" {
		t.Fatal("model or inference credential lost")
	}
	// Missing installations, malformed responses and wrong-owner claims are quiet.
	p = NewVSubDiscovery(r, []string{srv.URL + "/missing"})
	p.Refresh(context.Background(), "user-token", "alice")
	if _, ok := r.Get(discovered.ID); ok {
		t.Fatal("stale discovery remained")
	}
	p = NewVSubDiscovery(r, []string{srv.URL})
	p.Refresh(context.Background(), "user-token", "bob")
	for _, u := range r.List() {
		if u.Source == SourceVSub {
			t.Fatal("accepted another user's snapshot")
		}
	}
}
