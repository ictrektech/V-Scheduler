package registry

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func vMusicServer(t *testing.T, model string) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/v1/discovery" {
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
		_ = json.NewEncoder(w).Encode(map[string]any{
			"name": "V-Music", "protocol": "openai-extension", "visibility": "public",
			"models": []map[string]any{{
				"id": model, "owned_by": "v-music", "kind": "music",
				"capabilities": []string{"text-to-music", "instrumental"},
				"endpoints":    []string{"audio/music/generations"},
			}},
		})
	}))
}

func TestVMusicDiscoveryAndRouting(t *testing.T) {
	srv := vMusicServer(t, "stable-audio-3-small-music")
	defer srv.Close()
	r := New()
	p := NewVMusicPoller(r, []string{srv.URL}, time.Minute)
	p.pollOnce(context.Background())

	u, ok := r.Get(vMusicUpstreamID)
	if !ok || u.BaseURL != srv.URL+"/v1" || u.Source != SourceVMusic || u.Protocol != ProtocolOpenAI {
		t.Fatalf("wrong V-Music upstream: %#v, %v", u, ok)
	}
	models := r.ModelsOf(vMusicUpstreamID)
	if len(models) != 1 || models[0].Qualified != "v-music/stable-audio-3-small-music" || models[0].Kind != "music" {
		t.Fatalf("wrong models: %#v", models)
	}
	got, native, release, err := r.AcquireFor(models[0].Qualified, "/audio/music/generations", "", "", true)
	if err != nil || got.ID != vMusicUpstreamID || native != "stable-audio-3-small-music" {
		t.Fatalf("music route failed: upstream=%#v native=%q err=%v", got, native, err)
	}
	release()
}

func TestVMusicDiscoveryFallsBackAndRemovesMissingService(t *testing.T) {
	fallback := vMusicServer(t, "musicgen-small")
	r := New()
	p := NewVMusicPoller(r, []string{"http://127.0.0.1:1", fallback.URL}, time.Minute)
	p.pollOnce(context.Background())
	if _, ok := r.Get(vMusicUpstreamID); !ok {
		t.Fatal("fallback V-Music missing")
	}
	fallback.Close()
	p.pollOnce(context.Background())
	if _, ok := r.Get(vMusicUpstreamID); ok || len(r.ModelsOf(vMusicUpstreamID)) != 0 {
		t.Fatal("missing V-Music must leave no stale upstream or models")
	}
}
