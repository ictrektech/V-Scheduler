package proxy

import (
	"context"
	"encoding/json"
	"github.com/ictrektech/v-router/internal/registry"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestSchedulerCloudBypassAndLocalReservation(t *testing.T) {
	var mu sync.Mutex
	var actions []string
	var stopped bool
	scheduler := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		defer mu.Unlock()
		actions = append(actions, r.URL.Path)
		if strings.HasSuffix(r.URL.Path, "release") {
			var body struct {
				Stopped bool `json:"stopped"`
			}
			_ = json.NewDecoder(r.Body).Decode(&body)
			stopped = body.Stopped
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"data":{"permit_id":"p1","lease_seconds":30}}`)
	}))
	defer scheduler.Close()
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = io.WriteString(w, `{"ok":true}`) }))
	defer upstream.Close()
	cfg := &SchedulerConfig{URL: scheduler.URL, Token: strings.Repeat("x", 32), DefaultLocal: registry.SchedulerPolicy{Mode: "permit", ExecutorID: "local.infer", Completion: "synchronous"}}
	send := func(kind string) {
		req, _ := http.NewRequestWithContext(WithUpstream(context.Background(), registry.Upstream{ID: "u", Kind: kind}), "POST", upstream.URL, strings.NewReader(`{}`))
		resp, err := (&http.Client{Transport: &scheduledTransport{base: http.DefaultTransport, config: cfg}}).Do(req)
		if err != nil {
			t.Fatal(err)
		}
		_, _ = io.ReadAll(resp.Body)
		_ = resp.Body.Close()
	}
	send(registry.KindCloud)
	if len(actions) != 0 {
		t.Fatal("cloud request contacted scheduler")
	}
	send(registry.KindLocal)
	mu.Lock()
	defer mu.Unlock()
	if len(actions) != 2 || actions[0] != "/permits" || actions[1] != "/permits/p1/release" || !stopped {
		t.Fatalf("unexpected lifecycle: %v stopped=%v", actions, stopped)
	}
}
func TestLocalAdmissionFailureDoesNotCallUpstream(t *testing.T) {
	scheduler := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(429) }))
	defer scheduler.Close()
	called := false
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { called = true }))
	defer up.Close()
	cfg := &SchedulerConfig{URL: scheduler.URL, Token: strings.Repeat("x", 32), DefaultLocal: registry.SchedulerPolicy{Mode: "permit", ExecutorID: "local", Completion: "synchronous"}}
	req, _ := http.NewRequestWithContext(WithUpstream(context.Background(), registry.Upstream{Kind: registry.KindLocal}), "POST", up.URL, nil)
	_, err := (&scheduledTransport{base: http.DefaultTransport, config: cfg}).RoundTrip(req)
	if AdmissionStatus(err) != 429 || called {
		t.Fatalf("err=%v called=%v", err, called)
	}
}

func TestBackendProofIsBoundToAdmissionAndForgedHeadersAreStripped(t *testing.T) {
	proof := make(chan string, 1)
	seen := make(chan http.Header, 2)
	scheduler := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/permits" {
			var body map[string]string
			_ = json.NewDecoder(r.Body).Decode(&body)
			if len(body["execution_token"]) != 64 {
				t.Error("missing private execution proof")
			}
			proof <- body["execution_token"]
		}
		_, _ = io.WriteString(w, `{"data":{"permit_id":"bound","lease_seconds":30}}`)
	}))
	defer scheduler.Close()
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		seen <- r.Header.Clone()
		_, _ = io.WriteString(w, "ok")
	}))
	defer up.Close()
	cfg := &SchedulerConfig{URL: scheduler.URL, Token: strings.Repeat("x", 32), DefaultLocal: registry.SchedulerPolicy{Mode: "permit", ExecutorID: "voice.infer", Completion: "synchronous"}}
	for _, kind := range []string{registry.KindCloud, registry.KindLocal} {
		ctx := WithUpstream(context.Background(), registry.Upstream{Kind: kind, BaseURL: up.URL})
		req, _ := http.NewRequestWithContext(ctx, "POST", up.URL, strings.NewReader(`{"model":"native"}`))
		for _, key := range []string{"X-VScheduler-Permit-ID", "X-VScheduler-Execution-Token", "X-VScheduler-Executor-ID"} {
			req.Header.Set(key, "forged")
		}
		resp, err := (&scheduledTransport{base: http.DefaultTransport, config: cfg}).RoundTrip(req)
		if err != nil {
			t.Fatal(err)
		}
		_, _ = io.ReadAll(resp.Body)
		_ = resp.Body.Close()
		h := <-seen
		if kind == registry.KindCloud {
			if h.Get("X-VScheduler-Permit-ID") != "" || h.Get("X-VScheduler-Execution-Token") != "" || h.Get("X-VScheduler-Executor-ID") != "" {
				t.Fatal("client forged proof reached cloud")
			}
		} else if h.Get("X-VScheduler-Permit-ID") != "bound" || h.Get("X-VScheduler-Executor-ID") != "voice.infer" || h.Get("X-VScheduler-Execution-Token") != <-proof {
			t.Fatal("backend proof did not match admitted budget")
		}
	}
}
func TestDelegatedUpstreamRequiresActiveTaskProof(t *testing.T) {
	calls := 0
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { calls++; _, _ = io.WriteString(w, "ok") }))
	defer up.Close()
	cfg := &SchedulerConfig{URL: "http://127.0.0.1:1", Token: strings.Repeat("x", 32)}
	u := registry.Upstream{Kind: registry.KindRegistered, Scheduler: &registry.SchedulerPolicy{Mode: "delegated"}}
	req, _ := http.NewRequestWithContext(WithUpstream(context.Background(), u), "POST", up.URL, nil)
	_, err := (&scheduledTransport{base: http.DefaultTransport, config: cfg}).RoundTrip(req)
	if AdmissionStatus(err) != 503 || calls != 0 {
		t.Fatalf("unverified delegation executed: err=%v calls=%d", err, calls)
	}
}
func TestVerifiedDelegationUsesLinkedLeaseAndNoSecondBudget(t *testing.T) {
	var mu sync.Mutex
	var actions []string
	scheduler := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		defer mu.Unlock()
		actions = append(actions, r.URL.Path)
		if r.URL.Path == "/delegations/verify" {
			var body map[string]string
			_ = json.NewDecoder(r.Body).Decode(&body)
			if body["owner_id"] != "owner" || body["task_id"] != "parent" || r.Header.Get("Idempotency-Key") == "" {
				t.Error("delegation lacked an authenticated owner or stable key")
			}
		}
		_, _ = io.WriteString(w, `{"data":{"permit_id":"linked","lease_seconds":30}}`)
	}))
	defer scheduler.Close()
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = io.WriteString(w, "ok") }))
	defer up.Close()
	cfg := &SchedulerConfig{URL: scheduler.URL, Token: strings.Repeat("x", 32)}
	u := registry.Upstream{ID: "u", Kind: registry.KindRegistered, BaseURL: up.URL, Scheduler: &registry.SchedulerPolicy{Mode: "delegated", ExecutorID: "infer", Completion: "synchronous"}}
	ctx := WithDelegation(WithUpstream(context.Background(), u), "parent", "owner")
	req, _ := http.NewRequestWithContext(ctx, "POST", up.URL, nil)
	resp, err := (&scheduledTransport{base: http.DefaultTransport, config: cfg}).RoundTrip(req)
	if err != nil {
		t.Fatal(err)
	}
	_, _ = io.ReadAll(resp.Body)
	_ = resp.Body.Close()
	mu.Lock()
	defer mu.Unlock()
	if len(actions) != 2 || actions[0] != "/delegations/verify" || actions[1] != "/permits/linked/release" {
		t.Fatalf("unexpected linked lifecycle: %v", actions)
	}
}
func TestPrematureStreamCloseRetainsSuspectPermit(t *testing.T) {
	released := make(chan bool, 1)
	scheduler := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasSuffix(r.URL.Path, "release") {
			var b struct {
				Stopped bool `json:"stopped"`
			}
			_ = json.NewDecoder(r.Body).Decode(&b)
			released <- b.Stopped
		}
		_, _ = io.WriteString(w, `{"data":{"permit_id":"p"}}`)
	}))
	defer scheduler.Close()
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: first\n\n")
		w.(http.Flusher).Flush()
		<-r.Context().Done()
	}))
	defer up.Close()
	ctx, cancel := context.WithCancel(context.Background())
	cfg := &SchedulerConfig{URL: scheduler.URL, Token: strings.Repeat("x", 32), DefaultLocal: registry.SchedulerPolicy{Mode: "permit", ExecutorID: "local", Completion: "synchronous"}}
	req, _ := http.NewRequestWithContext(WithUpstream(ctx, registry.Upstream{Kind: registry.KindLocal}), "POST", up.URL, nil)
	resp, err := (&scheduledTransport{base: http.DefaultTransport, config: cfg}).RoundTrip(req)
	if err != nil {
		t.Fatal(err)
	}
	buffer := make([]byte, 5)
	_, _ = resp.Body.Read(buffer)
	cancel()
	_ = resp.Body.Close()
	if <-released {
		t.Fatal("client disconnect was treated as confirmed compute stop")
	}
}
func TestCloudBypassesBrokenSchedulerConfig(t *testing.T) {
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = io.WriteString(w, "ok") }))
	defer up.Close()
	t.Setenv("V_ROUTER_SCHEDULER_CONFIG", "/nonexistent/config")
	t.Setenv("V_ROUTER_SCHEDULER_REQUIRED", "true")
	req, _ := http.NewRequestWithContext(WithUpstream(context.Background(), registry.Upstream{Kind: registry.KindCloud, BaseURL: up.URL, Scheduler: &registry.SchedulerPolicy{Mode: "cloud", UpstreamURL: up.URL}}), "POST", up.URL, nil)
	resp, err := NewScheduledTransport(http.DefaultTransport).RoundTrip(req)
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()
}

func TestLocalRejectsMissingSchedulerConfig(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("V_ROUTER_STATE_PATH", dir+"/state.json")
	t.Setenv("V_ROUTER_SCHEDULER_CONFIG", "")
	t.Setenv("V_ROUTER_SCHEDULER_REQUIRED", "true")
	req, _ := http.NewRequestWithContext(WithUpstream(context.Background(), registry.Upstream{Kind: registry.KindLocal}), "POST", "http://127.0.0.1:1", nil)
	_, err := NewScheduledTransport(http.DefaultTransport).RoundTrip(req)
	if AdmissionStatus(err) != 503 {
		t.Fatalf("missing scheduler config must block local execution, got %v", err)
	}
}

func TestDefaultSchedulerConfigUsesPrivateStateDirectory(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("V_ROUTER_STATE_PATH", dir+"/state.json")
	t.Setenv("V_ROUTER_SCHEDULER_CONFIG", "")
	transport := &scheduledTransport{}
	transport.load()
	if transport.config != nil || transport.err != nil {
		t.Fatal("missing optional config enabled scheduling")
	}
	data := `{"url":"http://scheduler/api/v1","token":"` + strings.Repeat("x", 32) + `"}`
	if err := os.WriteFile(dir+"/scheduler.local.json", []byte(data), 0600); err != nil {
		t.Fatal(err)
	}
	transport = &scheduledTransport{}
	transport.load()
	if transport.config == nil || transport.err != nil {
		t.Fatal("private scheduler configuration was not loaded")
	}
}

func TestBuiltinBusyAdmissionReturnsWithoutHiddenRetries(t *testing.T) {
	calls := 0
	scheduler := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { calls++; w.WriteHeader(429) }))
	defer scheduler.Close()
	cfg := &SchedulerConfig{URL: scheduler.URL, Token: strings.Repeat("x", 32), DefaultLocal: registry.SchedulerPolicy{Mode: "permit", ExecutorID: "local", Completion: "synchronous"}}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	u := registry.Upstream{Kind: registry.KindLocal, Source: registry.SourceSchedulerOllama}
	req, _ := http.NewRequestWithContext(ctx, "POST", "http://unused.invalid/v1/embeddings", nil)
	_, err := (&scheduledTransport{base: http.DefaultTransport}).withConfig(req, u, cfg)
	if AdmissionStatus(err) != 429 || calls != 1 || ctx.Err() != nil {
		t.Fatalf("busy admission was hidden: status=%d calls=%d context=%v", AdmissionStatus(err), calls, ctx.Err())
	}
}
func TestBuiltinMissingBindingReturnsBeforeClientTimeout(t *testing.T) {
	t.Setenv("V_ROUTER_SCHEDULER_REQUIRED", "true")
	t.Setenv("V_ROUTER_STATE_PATH", filepath.Join(t.TempDir(), "state.json"))
	u := registry.Upstream{ID: "v-scheduler/ollama-qa", Kind: registry.KindLocal, Source: registry.SourceSchedulerOllama, BaseURL: "http://unused.invalid/v1"}
	ctx, cancel := context.WithTimeout(WithModel(WithUpstream(context.Background(), u), "test"), time.Second)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, "POST", u.BaseURL+"/chat/completions", nil)
	_, err := (&scheduledTransport{base: http.DefaultTransport}).RoundTrip(req)
	if AdmissionStatus(err) != 503 || ctx.Err() != nil {
		t.Fatalf("missing binding was hidden: status=%d context=%v", AdmissionStatus(err), ctx.Err())
	}
}

func TestPendingStopHasDistinctAdmissionReason(t *testing.T) {
	scheduler := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(429)
		_, _ = io.WriteString(w, `{"code":"WAIT_STOP_CONFIRMATION"}`)
	}))
	defer scheduler.Close()
	_, err := schedulerCall(context.Background(), &SchedulerConfig{URL: scheduler.URL, Token: strings.Repeat("x", 32)}, "/permits", map[string]string{}, "fixture")
	if AdmissionStatus(err) != 429 || !strings.Contains(err.Error(), "native stop confirmation") {
		t.Fatalf("missing stop confirmation reason: %v", err)
	}
}
