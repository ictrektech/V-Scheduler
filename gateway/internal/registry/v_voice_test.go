package registry

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"slices"
	"testing"
	"time"
)

func TestVVoiceCreativeModelsRefreshWithoutHardcodedList(t *testing.T) {
	rows := []vVoiceModel{{ID: "audiocpp-new-model", Kind: "tts", Capabilities: []string{"voice_cloning", "voice_design"}, Endpoints: []string{"audio/speech"}}}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(vVoiceDiscovery{Name: "V-Voice", Models: rows})
	}))
	defer srv.Close()
	reg := New()
	p := NewVVoicePoller(reg, []string{srv.URL}, time.Minute)
	p.pollOnce(context.Background())
	models := reg.ModelsOf(vVoiceUpstreamID)
	if len(models) != 1 || models[0].Qualified != "v-voice/audiocpp-new-model" || !slices.Contains(models[0].Capabilities, "voice_cloning") {
		t.Fatalf("creative model discovery lost capabilities: %#v", models)
	}
	_, native, release, err := reg.AcquireFor(models[0].Qualified, "/audio/speech", "", "", true)
	if err != nil || native != rows[0].ID {
		t.Fatalf("creative routing: %q %v", native, err)
	}
	release()
	rows = nil
	p.pollOnce(context.Background())
	if len(reg.ModelsOf(vVoiceUpstreamID)) != 0 {
		t.Fatal("removed models remained routable")
	}
}

func vVoiceServer(t *testing.T, model string) *httptest.Server {
	t.Helper()
	return vVoiceServerWithContract(t, model, "", "")
}

func vVoiceServerWithContract(t *testing.T, model, protocol, visibility string) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/v1/discovery" {
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
		_ = json.NewEncoder(w).Encode(map[string]any{
			"name":         "V-Voice",
			"protocol":     protocol,
			"visibility":   visibility,
			"multi_tenant": false,
			"models":       []map[string]any{{"id": model, "owned_by": "v-voice", "kind": "tts", "capabilities": []string{"tts", "speech"}, "endpoints": []string{"audio/speech"}}},
		})
	}))
}

func TestVVoiceDiscoveryPrefersFirstReachableCandidate(t *testing.T) {
	preferred := vVoiceServer(t, "preferred")
	defer preferred.Close()
	fallback := vVoiceServer(t, "fallback")
	defer fallback.Close()
	r := New()
	p := NewVVoicePoller(r, []string{preferred.URL, fallback.URL}, time.Minute)
	p.pollOnce(context.Background())

	u, ok := r.Get(vVoiceUpstreamID)
	if !ok || u.BaseURL != preferred.URL+"/v1" || u.Source != SourceVVoice {
		t.Fatalf("wrong preferred upstream: %#v, %v", u, ok)
	}
	models := r.ModelsOf(vVoiceUpstreamID)
	if len(models) != 1 || models[0].Qualified != "v-voice/preferred" || models[0].Kind != "tts" {
		t.Fatalf("wrong models: %#v", models)
	}
}

func TestVVoiceDiscoveryFallsBackAndSilentlyRemovesMissingService(t *testing.T) {
	fallback := vVoiceServer(t, "kokoro")
	r := New()
	p := NewVVoicePoller(r, []string{"http://127.0.0.1:1", fallback.URL}, time.Minute)
	p.pollOnce(context.Background())
	if u, ok := r.Get(vVoiceUpstreamID); !ok || u.BaseURL != fallback.URL+"/v1" {
		t.Fatalf("fallback not selected: %#v, %v", u, ok)
	}
	fallback.Close()
	p.pollOnce(context.Background())
	if _, ok := r.Get(vVoiceUpstreamID); ok || len(r.ModelsOf(vVoiceUpstreamID)) != 0 {
		t.Fatal("missing V-Voice must leave no stale upstream or models")
	}
}

func TestVVoiceDiscoveryHonorsDeclaredProtocolAndVisibility(t *testing.T) {
	srv := vVoiceServerWithContract(t, "kokoro", "chat_completions", "private")
	defer srv.Close()
	r := New()
	p := NewVVoicePoller(r, []string{srv.URL}, time.Minute)
	p.pollOnce(context.Background())

	u, ok := r.Get(vVoiceUpstreamID)
	if !ok {
		t.Fatal("upstream missing")
	}
	if u.Protocol != ProtocolChatCompletions {
		t.Fatalf("declared protocol not honored: %q", u.Protocol)
	}
	if u.Visibility != VisibilityPrivate || u.Scope() != VisibilityPrivate {
		t.Fatalf("declared visibility not honored: %q", u.Visibility)
	}
	if u.VisibleTo("someone-else", false) {
		t.Fatal("private V-Voice must be hidden from other callers")
	}
}

func TestVVoiceDiscoveryDefaultsProtocolAndVisibility(t *testing.T) {
	srv := vVoiceServerWithContract(t, "kokoro", "unexpected", "")
	defer srv.Close()
	r := New()
	p := NewVVoicePoller(r, []string{srv.URL}, time.Minute)
	p.pollOnce(context.Background())

	u, ok := r.Get(vVoiceUpstreamID)
	if !ok {
		t.Fatal("upstream missing")
	}
	if u.Protocol != ProtocolOpenAI {
		t.Fatalf("unknown protocol must fall back to openai, got %q", u.Protocol)
	}
	if u.Scope() != VisibilityPublic {
		t.Fatalf("undocumented visibility must default to public, got %q", u.Visibility)
	}
}

func TestVVoiceStreamingAndSpeakerEndpointRouting(t *testing.T) {
	r := New()
	streaming := &Upstream{ID: "vv-stream", Kind: KindLocal, Source: SourceVVoice, BaseURL: "http://v/v1", Protocol: ProtocolOpenAI, ModelPrefix: SourceVVoice, Healthy: true}
	r.Put(streaming)
	r.SetModels("vv-stream", []Model{{
		ID: "zipformer-zh-streaming", Qualified: "v-voice/zipformer-zh-streaming", Upstream: "vv-stream",
		Capabilities: MergeCapabilities([]string{"asr", "streaming", "hotwords"}),
		Endpoints:    NormalizeEndpoints([]string{"audio/transcriptions", "audio/streaming/transcriptions", "audio/streaming/ws"}),
	}})
	speaker := &Upstream{ID: "vv-spk", Kind: KindLocal, Source: SourceVVoice, BaseURL: "http://v/v1", Protocol: ProtocolOpenAI, ModelPrefix: SourceVVoice, Healthy: true}
	r.Put(speaker)
	r.SetModels("vv-spk", []Model{{
		ID: "speaker-campplus-zh-en", Qualified: "v-voice/speaker-campplus-zh-en", Upstream: "vv-spk",
		Capabilities: MergeCapabilities([]string{"speaker", "speaker-identification", "speaker-verification"}),
		Endpoints:    NormalizeEndpoints([]string{"audio/speakers", "audio/speakers/identify", "audio/speakers/verify"}),
	}})

	for _, path := range []string{"/audio/streaming/transcriptions", "/audio/streaming/ws", "/audio/transcriptions"} {
		u, _, release, err := r.AcquireFor("v-voice/zipformer-zh-streaming", path, "", "", true)
		if err != nil || u.ID != "vv-stream" {
			t.Fatalf("streaming model must serve %s: %v %s", path, err, u.ID)
		}
		release()
	}
	for _, path := range []string{"/audio/speakers", "/audio/speakers/identify", "/audio/speakers/verify"} {
		u, _, release, err := r.AcquireFor("v-voice/speaker-campplus-zh-en", path, "", "", true)
		if err != nil || u.ID != "vv-spk" {
			t.Fatalf("speaker model must serve %s: %v %s", path, err, u.ID)
		}
		release()
	}
	// A non-streaming model must not be routed to the streaming endpoints.
	whole := &Upstream{ID: "vv-whole", Kind: KindLocal, Source: SourceVVoice, BaseURL: "http://v/v1", Protocol: ProtocolOpenAI, ModelPrefix: SourceVVoice, Healthy: true}
	r.Put(whole)
	r.SetModels("vv-whole", []Model{{
		ID: "qwen3-asr-0.6b", Qualified: "v-voice/qwen3-asr-0.6b", Upstream: "vv-whole",
		Endpoints: NormalizeEndpoints([]string{"audio/transcriptions"}),
	}})
	if _, _, _, err := r.AcquireFor("v-voice/qwen3-asr-0.6b", "/audio/streaming/transcriptions", "", "", true); !errors.Is(err, ErrUnsupported) {
		t.Fatalf("non-streaming model must be refused on streaming endpoints, got %v", err)
	}
}
