package api

import (
	"bufio"
	"crypto/sha1"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/ictrektech/v-router/internal/proxy"
	"github.com/ictrektech/v-router/internal/registry"
)

func schedulerGateway(t *testing.T, up *httptest.Server, protocol string) (*httptest.Server, *atomic.Int32, *atomic.Int32, *atomic.Bool) {
	t.Helper()
	acquired := &atomic.Int32{}
	released := &atomic.Int32{}
	busy := &atomic.Bool{}
	scheduler := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/permits" {
			if busy.Load() {
				w.WriteHeader(429)
				return
			}
			var body map[string]string
			_ = json.NewDecoder(r.Body).Decode(&body)
			if body["model_id"] != "gpt-4o" || body["upstream_url"] != up.URL+"/v1" || body["executor_id"] != "infer" {
				t.Error("gateway lost model or execution-endpoint scope")
			}
			acquired.Add(1)
		}
		if strings.HasSuffix(r.URL.Path, "/release") {
			var body struct {
				Stopped bool `json:"stopped"`
			}
			_ = json.NewDecoder(r.Body).Decode(&body)
			if !body.Stopped {
				t.Error("fixture's synchronous completion was not reconciled")
			}
			released.Add(1)
		}
		_, _ = io.WriteString(w, `{"data":{"permit_id":"fixture-permit","lease_seconds":30}}`)
	}))
	t.Cleanup(scheduler.Close)
	dir := t.TempDir()
	t.Setenv("V_ROUTER_STATE_PATH", filepath.Join(dir, "state.json"))
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(dir, "usage.jsonl"))
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	t.Setenv("V_ROUTER_SCHEDULER_REQUIRED", "true")
	policy := registry.SchedulerPolicy{Mode: "permit", ExecutorID: "infer", Completion: "synchronous", WebSocketCompletion: "server_close", MaxInputBytes: 1 << 20, Limits: map[string]float64{"max_tokens": 64, "max_output_tokens": 64, "n": 1, "width": 512, "height": 512, "duration": 5}}
	cfg := proxy.SchedulerConfig{URL: scheduler.URL, Token: strings.Repeat("x", 32), DefaultLocal: policy, Models: map[string]registry.SchedulerPolicy{"local/gpt-4o": policy}, RequireModelPolicy: true}
	data, _ := json.Marshal(cfg)
	configPath := filepath.Join(dir, "scheduler.local.json")
	if err := os.WriteFile(configPath, data, 0600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("V_ROUTER_SCHEDULER_CONFIG", configPath)
	previous := bridgeClient
	bridgeClient = &http.Client{Transport: proxy.NewScheduledTransport(proxy.Transport)}
	t.Cleanup(func() { bridgeClient = previous })
	reg := registry.New()
	reg.Put(&registry.Upstream{ID: "local", Kind: registry.KindLocal, BaseURL: up.URL + "/v1", Protocol: protocol, ModelPrefix: "fixture", Healthy: true})
	endpoints := []string{"chat_completions", "responses", "messages", "completions", "embeddings", "images/generations", "images/edits", "videos/generations", "audio/speech", "audio/transcriptions", "audio/music/generations", "audio/streaming/transcriptions", "audio/streaming/ws"}
	reg.SetModels("local", []registry.Model{{ID: "gpt-4o", Qualified: "fixture/gpt-4o", Upstream: "local", Kind: registry.ModelKindChat, Endpoints: endpoints}, {ID: "unapproved", Qualified: "fixture/unapproved", Upstream: "local", Kind: registry.ModelKindChat, Endpoints: []string{"chat_completions"}}})
	s := newServer(reg, "conformance")
	s.allowLoopbackUpstreams = true
	gateway := httptest.NewServer(s.routes())
	t.Cleanup(gateway.Close)
	return gateway, acquired, released, busy
}

func TestStrictSchedulerCoversNativeAndConvertedModelHTTP(t *testing.T) {
	for _, protocol := range []string{registry.ProtocolOpenAI, registry.ProtocolChatCompletions, registry.ProtocolAnthropic} {
		t.Run(protocol, func(t *testing.T) {
			upstream := fakeUpstream(t)
			if protocol == registry.ProtocolAnthropic {
				upstream = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.Header().Set("Content-Type", "application/json")
					_, _ = io.WriteString(w, `{"id":"msg-1","type":"message","role":"assistant","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn"}`)
				}))
				t.Cleanup(upstream.Close)
			}
			gateway, acquired, released, _ := schedulerGateway(t, upstream, protocol)
			cases := []struct {
				path string
				body map[string]any
			}{
				{"chat/completions", map[string]any{"model": "fixture/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hello"}}}},
				{"chat/completions", map[string]any{"model": "fixture/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hello"}}, "stream": true}},
				{"responses", map[string]any{"model": "fixture/gpt-4o", "input": "hello", "store": false}},
				{"messages", map[string]any{"model": "fixture/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hello"}}, "max_tokens": 16}},
				{"completions", map[string]any{"model": "fixture/gpt-4o", "prompt": "hello"}},
				{"embeddings", map[string]any{"model": "fixture/gpt-4o", "input": "hello"}},
				{"images/generations", map[string]any{"model": "fixture/gpt-4o", "prompt": "hello", "size": "256x256"}},
				{"videos/generations", map[string]any{"model": "fixture/gpt-4o", "prompt": "hello", "duration": 1}},
				{"audio/speech", map[string]any{"model": "fixture/gpt-4o", "input": "hello"}},
				{"audio/music/generations", map[string]any{"model": "fixture/gpt-4o", "prompt": "hello"}},
			}
			for _, test := range cases {
				if protocol == registry.ProtocolOpenAI && test.path == "messages" || protocol == registry.ProtocolAnthropic && test.path != "messages" {
					continue
				}
				t.Run(test.path, func(t *testing.T) {
					before := acquired.Load()
					response, body := doJSON(t, "POST", gateway.URL+"/v1/"+test.path, test.body)
					if response.StatusCode != 200 {
						t.Fatalf("scheduled endpoint failed: status=%d body=%s", response.StatusCode, body)
					}
					if acquired.Load() != before+1 {
						t.Fatal("model endpoint did not acquire exactly one permit")
					}
				})
			}
			deadline := time.Now().Add(5 * time.Second)
			for released.Load() != acquired.Load() && time.Now().Before(deadline) {
				time.Sleep(10 * time.Millisecond)
			}
			if released.Load() != acquired.Load() {
				t.Fatal("completed model requests leaked permits")
			}
		})
	}
}

func TestStrictSchedulerRejectsUnknownModelsInputGrowthAndQuota(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { t.Error("denied request reached compute") }))
	defer upstream.Close()
	gateway, acquired, _, busy := schedulerGateway(t, upstream, registry.ProtocolOpenAI)
	for _, test := range []struct {
		body   map[string]any
		status int
	}{
		{map[string]any{"model": "fixture/unapproved", "messages": []map[string]string{{"role": "user", "content": "hello"}}}, 503},
		{map[string]any{"model": "fixture/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hello"}}, "max_tokens": 100000}, 422},
		{map[string]any{"model": "fixture/gpt-4o", "messages": []map[string]string{{"role": "user", "content": strings.Repeat("x", (1<<20)+1)}}}, 413},
	} {
		response, _ := doJSON(t, "POST", gateway.URL+"/v1/chat/completions", test.body)
		if response.StatusCode != test.status {
			t.Fatalf("unsafe input status=%d, want %d", response.StatusCode, test.status)
		}
	}
	if acquired.Load() != 0 {
		t.Fatal("unapproved input reached compute admission")
	}
	busy.Store(true)
	for _, path := range []string{"chat/completions", "responses", "images/generations", "audio/speech"} {
		body := map[string]any{"model": "fixture/gpt-4o", "messages": []map[string]string{{"role": "user", "content": "hello"}}, "input": "hello", "prompt": "hello"}
		response, _ := doJSON(t, "POST", gateway.URL+"/v1/"+path, body)
		if response.StatusCode != 429 {
			t.Fatalf("quota denial was not preserved for %s: %d", path, response.StatusCode)
		}
	}
	if acquired.Load() != 0 {
		t.Fatal("quota-denied execution acquired compute")
	}
}

func TestStrictSchedulerWebSocketThroughGateway(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("model") != "gpt-4o" {
			t.Error("streaming model identity was not restored")
		}
		connection, buffer, err := w.(http.Hijacker).Hijack()
		if err != nil {
			t.Error(err)
			return
		}
		defer connection.Close()
		hash := sha1.Sum([]byte(r.Header.Get("Sec-WebSocket-Key") + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
		_, _ = fmt.Fprintf(buffer, "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: %s\r\n\r\n", base64.StdEncoding.EncodeToString(hash[:]))
		_ = buffer.Flush()
		data := make([]byte, 7)
		if _, err := io.ReadFull(buffer, data); err != nil {
			t.Error(err)
			return
		}
		if data[6]^data[2] != 'x' {
			t.Error("client frame was changed")
		}
		_, _ = connection.Write([]byte{0x82, 1, 'x', 0x88, 2, 3, 0xe8})
	}))
	defer upstream.Close()
	gateway, acquired, released, _ := schedulerGateway(t, upstream, registry.ProtocolOpenAI)
	address := strings.TrimPrefix(gateway.URL, "http://")
	connection, err := net.DialTimeout("tcp", address, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	defer connection.Close()
	_ = connection.SetDeadline(time.Now().Add(10 * time.Second))
	_, _ = fmt.Fprintf(connection, "GET /v1/audio/streaming/ws?model=fixture%%2Fgpt-4o HTTP/1.1\r\nHost: gateway\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")
	reader := bufio.NewReader(connection)
	response, err := http.ReadResponse(reader, &http.Request{Method: "GET"})
	if err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != 101 {
		t.Fatalf("scheduled upgrade status=%d", response.StatusCode)
	}
	_, _ = connection.Write([]byte{0x82, 0x81, 1, 2, 3, 4, 'x' ^ 1})
	frames := make([]byte, 7)
	if _, err := io.ReadFull(reader, frames); err != nil {
		t.Fatal(err)
	}
	if frames[2] != 'x' {
		t.Fatal("scheduled duplex response was corrupted")
	}
	deadline := time.Now().Add(5 * time.Second)
	for released.Load() != 1 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if acquired.Load() != 1 || released.Load() != 1 {
		t.Fatal("WebSocket did not keep and reconcile one permit")
	}
}
