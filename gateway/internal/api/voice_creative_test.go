package api

import (
	"encoding/json"
	"github.com/ictrektech/v-router/internal/registry"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestCreativeSpeechExtensionsAndBinaryResponse(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	var received map[string]any
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/audio/speech" {
			t.Errorf("path=%s", r.URL.Path)
		}
		_ = json.NewDecoder(r.Body).Decode(&received)
		w.Header().Set("Content-Type", "audio/wav")
		_, _ = w.Write([]byte("RIFF-test-audio"))
	}))
	defer up.Close()
	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "app-v-voice", Source: registry.SourceVVoice, Kind: registry.KindCloud, Protocol: registry.ProtocolOpenAI, BaseURL: up.URL + "/v1", Healthy: true})
	reg.SetModels("app-v-voice", []registry.Model{{ID: "audiocpp-index-2.5-q8", Qualified: "v-voice/audiocpp-index-2.5-q8", Upstream: "app-v-voice", Kind: "tts", Endpoints: []string{"audio/speech"}, Capabilities: []string{"tts", "voice_cloning", "emotion_control"}}})
	handler := newServer(reg, "test").routes()
	body := `{"model":"v-voice/audiocpp-index-2.5-q8","input":"Hello","reference_audio":"UklGRg==","reference_text":"Reference","emotion_text":"calm","emotion_strength":0.4,"response_format":"wav"}`
	req := httptest.NewRequest("POST", "/v1/audio/speech", strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != 200 || w.Header().Get("Content-Type") != "audio/wav" || w.Body.String() != "RIFF-test-audio" {
		t.Fatalf("speech: %d %s", w.Code, w.Body.String())
	}
	if received["model"] != "audiocpp-index-2.5-q8" || received["reference_audio"] != "UklGRg==" || received["emotion_text"] != "calm" || received["emotion_strength"] != 0.4 {
		t.Fatalf("lost extension fields: %#v", received)
	}
	w = httptest.NewRecorder()
	handler.ServeHTTP(w, httptest.NewRequest("GET", "/api/v1/models", nil))
	if w.Code != 200 || !strings.Contains(w.Body.String(), "emotion_control") {
		t.Fatalf("capabilities not exposed: %s", w.Body.String())
	}
}
