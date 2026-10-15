package api

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"mime/multipart"
	"net"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestExtendedRouting(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	var gotPath, gotModel, gotQuery, gotKey, gotPin, gotCookie, gotHotwords, gotHotwordsScore string
	var gotTimestampGranularities []string
	var gotFile, gotHotwordsFile []byte
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath = r.URL.Path
		gotQuery = r.URL.RawQuery
		gotKey = r.Header.Get("Authorization")
		gotPin = r.Header.Get("X-VRouter-Upstream")
		gotCookie = r.Header.Get("Cookie")
		if strings.HasPrefix(r.Header.Get("Content-Type"), "multipart/") {
			if err := r.ParseMultipartForm(1 << 20); err != nil {
				t.Error(err)
			}
			gotModel = r.FormValue("model")
			gotHotwords = r.FormValue("hotwords")
			gotHotwordsScore = r.FormValue("hotwords_score")
			gotTimestampGranularities = r.MultipartForm.Value["timestamp_granularities[]"]
			f, _, err := r.FormFile("file")
			if err != nil {
				t.Error(err)
			} else {
				gotFile, _ = io.ReadAll(f)
				f.Close()
			}
			if f, _, err = r.FormFile("hotwords_file"); err == nil {
				gotHotwordsFile, _ = io.ReadAll(f)
				f.Close()
			}
		} else {
			var body map[string]any
			_ = json.NewDecoder(r.Body).Decode(&body)
			gotModel, _ = body["model"].(string)
		}
		w.Header().Set("Content-Type", "audio/mpeg")
		w.WriteHeader(201)
		_, _ = w.Write([]byte{0, 1, 255, 4})
	}))
	defer upstream.Close()
	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "u", Name: "test", BaseURL: upstream.URL + "/v1", Healthy: true, APIKey: "provider-key"})
	reg.SetModels("u", []registry.Model{{ID: "native", Qualified: "manual/native", Upstream: "u"}})
	handler := newServer(reg, "test").routes()
	for _, path := range []string{"/responses", "/completions", "/embeddings", "/images/generations", "/videos/generations", "/audio/speech", "/moderations", "/rerank"} {
		req := httptest.NewRequest("POST", "/v1"+path, strings.NewReader(`{"model":"manual/native","input":"hello","extra":{"preserve":true}}`))
		req.Header.Set("Content-Type", "application/json")
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, req)
		if w.Code != 201 || gotPath != "/v1"+path || gotModel != "native" || !bytes.Equal(w.Body.Bytes(), []byte{0, 1, 255, 4}) {
			t.Fatalf("%s: %d %s %s", path, w.Code, gotPath, gotModel)
		}
	}
	for _, path := range []string{"/audio/transcriptions", "/audio/translations", "/images/edits"} {
		var buf bytes.Buffer
		mw := multipart.NewWriter(&buf)
		_ = mw.WriteField("model", "manual/native")
		if strings.HasPrefix(path, "/audio/trans") {
			_ = mw.WriteField("hotwords", "V-Voice,sherpa-onnx")
			_ = mw.WriteField("hotwords_score", "1.5")
			_ = mw.WriteField("timestamp_granularities[]", "word")
			_ = mw.WriteField("timestamp_granularities[]", "segment")
			h, _ := mw.CreateFormFile("hotwords_file", "hotwords.txt")
			_, _ = h.Write([]byte("OpenAI\nVOS\n"))
		}
		f, _ := mw.CreateFormFile("file", "test.wav")
		_, _ = f.Write([]byte{0, 255, 13, 10})
		_ = mw.Close()
		req := httptest.NewRequest("POST", "/v1"+path, &buf)
		req.Header.Set("Content-Type", mw.FormDataContentType())
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, req)
		if w.Code != 201 || gotModel != "native" || !bytes.Equal(gotFile, []byte{0, 255, 13, 10}) {
			t.Fatalf("multipart %s: %d %s", path, w.Code, w.Body.String())
		}
		if strings.HasPrefix(path, "/audio/trans") && (gotHotwords != "V-Voice,sherpa-onnx" || gotHotwordsScore != "1.5" || !bytes.Equal(gotHotwordsFile, []byte("OpenAI\nVOS\n"))) {
			t.Fatalf("hotwords were not preserved for %s: %q %q %q", path, gotHotwords, gotHotwordsScore, gotHotwordsFile)
		}
		if strings.HasPrefix(path, "/audio/trans") && !reflect.DeepEqual(gotTimestampGranularities, []string{"word", "segment"}) {
			t.Fatalf("timestamp granularities were not preserved for %s: %q", path, gotTimestampGranularities)
		}
	}
	req := httptest.NewRequest("GET", "/v1/files/file123/content?after=a&limit=2", nil)
	req.Header.Set("X-VRouter-Upstream", "u")
	req.Header.Set("Cookie", "vos=private")
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != 201 || gotQuery != "after=a&limit=2" || gotKey != "Bearer provider-key" || gotPin != "" || gotCookie != "" {
		t.Fatalf("pinned: %d %s %s %s %s", w.Code, gotQuery, gotKey, gotPin, gotCookie)
	}
	for _, tc := range []struct {
		path, pin string
		code      int
	}{{"/v1/files", "", 400}, {"/v1/files", "missing", 404}, {"/v1/organization/users", "u", 404}} {
		req := httptest.NewRequest("GET", tc.path, nil)
		req.Header.Set("X-VRouter-Upstream", tc.pin)
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, req)
		if w.Code != tc.code {
			t.Fatalf("%s: %d", tc.path, w.Code)
		}
	}
}

func TestPinnedRequiresIdentity(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "false")
	s := newServer(registry.New(), "test")
	h := s.routes()
	for _, path := range []string{"/v1/files", "/v1/audio/speech"} {
		r := httptest.NewRequest("POST", path, strings.NewReader(`{}`))
		r.Header.Set("X-VRouter-Upstream", "u")
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if w.Code != 401 {
			t.Fatalf("%s: %d", path, w.Code)
		}
	}
}

func TestPinnedInferenceRecordsModelStreamAndUsage(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	up := fakeUpstream(t)
	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "u", Name: "Pinned", BaseURL: up.URL + "/v1", Healthy: true})
	handler := newServer(reg, "test").routes()

	req := httptest.NewRequest(http.MethodPost, "/v1/chat/completions", strings.NewReader(`{"model":"gpt-4o","stream":true,"messages":[{"role":"user","content":"hi"}]}`))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("X-VRouter-Upstream", "u")
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("pinned inference: %d %s", w.Code, w.Body.String())
	}

	w = httptest.NewRecorder()
	handler.ServeHTTP(w, httptest.NewRequest(http.MethodGet, "/api/v1/usage", nil))
	var usage usageSnapshot
	if err := json.Unmarshal(w.Body.Bytes(), &usage); err != nil {
		t.Fatal(err)
	}
	if usage.Totals.UsageRequests != 1 || usage.Totals.InputTokens != 3 || usage.Totals.OutputTokens != 2 || usage.Totals.TotalTokens != 5 {
		t.Fatalf("pinned totals: %+v", usage.Totals)
	}
	if len(usage.Recent) != 1 || usage.Recent[0].Model != "gpt-4o" || usage.Recent[0].NativeID != "gpt-4o" || !usage.Recent[0].Stream {
		t.Fatalf("pinned record: %+v", usage.Recent)
	}
}

func TestBodyLimitAndMultipartModel(t *testing.T) {
	r := httptest.NewRequest("POST", "/v1/audio/speech", strings.NewReader(strings.Repeat(" ", maxInferenceBody+1)))
	w := httptest.NewRecorder()
	if _, _, _, _, err := readModelBody(w, r); err == nil {
		t.Fatal("oversized body accepted")
	}
	var b bytes.Buffer
	mw := multipart.NewWriter(&b)
	_ = mw.WriteField("model", "a")
	_ = mw.WriteField("model", "b")
	_ = mw.Close()
	r = httptest.NewRequest("POST", "/v1/audio/transcriptions", &b)
	r.Header.Set("Content-Type", mw.FormDataContentType())
	if _, _, _, _, err := readModelBody(w, r); err == nil {
		t.Fatal("duplicate model accepted")
	}
}

func TestOpenAPIContract(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	s := newServer(registry.New(), "test-version")
	w := httptest.NewRecorder()
	s.routes().ServeHTTP(w, httptest.NewRequest("GET", "/api/v1/openapi.json", nil))
	var doc struct {
		OpenAPI string                    `json:"openapi"`
		Paths   map[string]map[string]any `json:"paths"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &doc); err != nil {
		t.Fatal(err)
	}
	if doc.OpenAPI != "3.1.0" {
		t.Fatal(doc.OpenAPI)
	}
	for _, r := range s.spec() {
		path := strings.ReplaceAll(r.Path, "...}", "}")
		if doc.Paths[path][strings.ToLower(r.Method)] == nil {
			t.Fatalf("missing %s %s", r.Method, path)
		}
	}
}

func TestRealtimeUpgrade(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("model") != "native" || r.Header.Get("Authorization") != "Bearer provider" {
			t.Error("upgrade routing changed")
		}
		conn, buf, err := w.(http.Hijacker).Hijack()
		if err != nil {
			t.Error(err)
			return
		}
		defer conn.Close()
		_, _ = buf.WriteString("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n")
		_ = buf.Flush()
		b := make([]byte, 4)
		if _, err = io.ReadFull(buf, b); err == nil {
			_, _ = conn.Write(b)
		}
	}))
	defer up.Close()
	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "u", Healthy: true, BaseURL: up.URL + "/v1", APIKey: "provider"})
	srv := httptest.NewServer(newServer(reg, "test").routes())
	defer srv.Close()
	conn, err := net.DialTimeout("tcp", strings.TrimPrefix(srv.URL, "http://"), time.Second)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(3 * time.Second))
	_, _ = fmt.Fprintf(conn, "GET /v1/realtime?model=native HTTP/1.1\r\nHost: gateway\r\nX-VRouter-Upstream: u\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n")
	br := bufio.NewReader(conn)
	resp, err := http.ReadResponse(br, nil)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 101 {
		t.Fatalf("upgrade status %d", resp.StatusCode)
	}
	_, _ = conn.Write([]byte("ping"))
	b := make([]byte, 4)
	if _, err = io.ReadFull(br, b); err != nil || string(b) != "ping" {
		t.Fatalf("duplex forwarding: %q %v", b, err)
	}
}

func TestResponsesFlushAndUpstreamErrors(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	release := make(chan struct{})
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("fail") == "1" {
			w.Header().Set("Retry-After", "3")
			w.WriteHeader(429)
			_, _ = io.WriteString(w, `{"error":{"message":"quota"}}`)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: first\n\n")
		w.(http.Flusher).Flush()
		<-release
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer up.Close()
	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "u", Healthy: true, BaseURL: up.URL + "/v1"})
	reg.SetModels("u", []registry.Model{{ID: "native", Qualified: "manual/native", Upstream: "u"}})
	srv := httptest.NewServer(newServer(reg, "test").routes())
	defer func() { close(release); srv.Close() }()
	client := &http.Client{Timeout: 2 * time.Second}
	resp, err := client.Post(srv.URL+"/v1/responses?fail=1", "application/json", strings.NewReader(`{"model":"manual/native"}`))
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != 429 || resp.Header.Get("Retry-After") != "3" || !strings.Contains(string(body), "quota") {
		t.Fatal("upstream error not preserved")
	}
	resp, err = client.Post(srv.URL+"/v1/responses", "application/json", strings.NewReader(`{"model":"manual/native","stream":true}`))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	line, err := bufio.NewReader(resp.Body).ReadString('\n')
	if err != nil || line != "data: first\n" {
		t.Fatalf("first SSE must arrive before upstream finishes: %q %v", line, err)
	}
}

func TestOfflineASRProtocolAdaptsTranscriptions(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	var gotPath, gotQuery, gotType, gotAuth string
	var gotBody []byte
	asr := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath = r.URL.Path
		gotQuery = r.URL.RawQuery
		gotType = r.Header.Get("Content-Type")
		gotAuth = r.Header.Get("Authorization")
		gotBody, _ = io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"code":0,"msg":"success","data":{"full_text":"识别成功","segments":[{"text":"识别成功"}],"stats":{"duration_s":1}}}`)
	}))
	defer asr.Close()

	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "registered-asr", Name: "ASR", Kind: registry.KindRegistered, Source: registry.SourceRegistered, Protocol: registry.ProtocolOfflineASR, BaseURL: asr.URL, APIKey: "provider", Healthy: true})
	reg.SetModels("registered-asr", []registry.Model{{ID: "offline-asr", Qualified: "registered/offline-asr", Upstream: "registered-asr", Kind: "unknown", Capabilities: []string{"asr"}, Endpoints: []string{"audio/transcriptions"}}})
	handler := newServer(reg, "test").routes()

	var buf bytes.Buffer
	mw := multipart.NewWriter(&buf)
	_ = mw.WriteField("model", "registered/offline-asr")
	_ = mw.WriteField("language", "zh")
	f, _ := mw.CreateFormFile("file", "test.wav")
	_, _ = f.Write([]byte("RIFFdata"))
	_ = mw.Close()
	req := httptest.NewRequest("POST", "/v1/audio/transcriptions", &buf)
	req.Header.Set("Content-Type", mw.FormDataContentType())
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("transcription status: %d %s", w.Code, w.Body.String())
	}
	if gotPath != "/transcribe" || gotQuery != "lang=zh" || gotType != "application/octet-stream" || gotAuth != "Bearer provider" || !bytes.Equal(gotBody, []byte("RIFFdata")) {
		t.Fatalf("wrong upstream call path=%s query=%s type=%s auth=%s body=%q", gotPath, gotQuery, gotType, gotAuth, gotBody)
	}
	if !strings.Contains(w.Body.String(), `"text":"识别成功"`) || !strings.Contains(w.Body.String(), `"segments"`) {
		t.Fatalf("unexpected response: %s", w.Body.String())
	}

	req = httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"registered/offline-asr","messages":[{"role":"user","content":"hi"}]}`))
	req.Header.Set("Content-Type", "application/json")
	w = httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("chat should be unsupported for ASR-only model: %d %s", w.Code, w.Body.String())
	}
}

func TestAudioVoicesRoutesQualifiedModelAndPreservesResponse(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	var gotModel string
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet || r.URL.Path != "/v1/audio/voices" {
			t.Fatalf("unexpected request %s %s", r.Method, r.URL.Path)
		}
		gotModel = r.URL.Query().Get("model")
		w.Header().Set("X-Voice-Version", "1")
		w.WriteHeader(http.StatusOK)
		_, _ = io.WriteString(w, `{"voices":[{"id":"zf_xiaoxiao","name":"zf_xiaoxiao"}]}`)
	}))
	defer upstream.Close()

	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "app-v-voice", Name: "V-Voice", Kind: registry.KindLocal, Source: registry.SourceVVoice, Protocol: registry.ProtocolOpenAI, BaseURL: upstream.URL + "/v1", Healthy: true})
	reg.SetModels("app-v-voice", []registry.Model{{ID: "kokoro", Qualified: "v-voice/kokoro", Upstream: "app-v-voice", Kind: "tts", Endpoints: []string{"audio/speech"}}})
	handler := newServer(reg, "test").routes()

	req := httptest.NewRequest(http.MethodGet, "/v1/audio/voices?model=v-voice%2Fkokoro", nil)
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusOK || gotModel != "kokoro" || w.Header().Get("X-Voice-Version") != "1" || !strings.Contains(w.Body.String(), "zf_xiaoxiao") {
		t.Fatalf("voices proxy failed: status=%d model=%q headers=%v body=%s", w.Code, gotModel, w.Header(), w.Body.String())
	}
}

func TestMusicGenerationRoutesQualifiedModelAndStreamsAudio(t *testing.T) {
	var gotModel string
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/v1/audio/music/generations" {
			t.Fatalf("unexpected upstream request %s %s", r.Method, r.URL.Path)
		}
		var body map[string]any
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		gotModel, _ = body["model"].(string)
		w.Header().Set("Content-Type", "audio/wav")
		w.Header().Set("Content-Disposition", `inline; filename="music.wav"`)
		_, _ = w.Write([]byte("RIFFmusic"))
	}))
	defer upstream.Close()

	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "app-v-music", Name: "V-Music", Kind: registry.KindLocal, Source: registry.SourceVMusic, Protocol: registry.ProtocolOpenAI, ModelPrefix: registry.SourceVMusic, BaseURL: upstream.URL + "/v1", Healthy: true})
	reg.SetModels("app-v-music", []registry.Model{{ID: "stable-audio-3-small-music", Qualified: "v-music/stable-audio-3-small-music", Upstream: "app-v-music", Kind: "music", Endpoints: []string{"audio/music/generations"}}})
	s := newServer(reg, "test")
	req := httptest.NewRequest(http.MethodPost, "/v1/audio/music/generations", strings.NewReader(`{"model":"v-music/stable-audio-3-small-music","prompt":"warm ambient","duration":10}`))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	s.routes().ServeHTTP(w, req)

	if w.Code != http.StatusOK || gotModel != "stable-audio-3-small-music" || w.Header().Get("Content-Type") != "audio/wav" || w.Body.String() != "RIFFmusic" {
		t.Fatalf("music proxy failed: status=%d model=%q headers=%v body=%q", w.Code, gotModel, w.Header(), w.Body.String())
	}
}
