package proxy

import (
	"bytes"
	"context"
	"encoding/json"
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

	"github.com/ictrektech/v-router/internal/registry"
)

type transportFunc func(*http.Request) (*http.Response, error)

func (f transportFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }
func waitCondition(t *testing.T, predicate func() bool) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if predicate() {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("timed out waiting for admission lifecycle")
}
func lifecycleScheduler(t *testing.T) (*SchedulerConfig, chan bool, *atomic.Int32) {
	t.Helper()
	released := make(chan bool, 10)
	acquired := &atomic.Int32{}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/permits" {
			acquired.Add(1)
		}
		if strings.HasSuffix(r.URL.Path, "/release") {
			var body struct {
				Stopped bool `json:"stopped"`
			}
			_ = json.NewDecoder(r.Body).Decode(&body)
			released <- body.Stopped
		}
		_, _ = io.WriteString(w, `{"data":{"permit_id":"permit-test","lease_seconds":30,"state":"released"}}`)
	}))
	t.Cleanup(server.Close)
	return &SchedulerConfig{URL: server.URL, Token: strings.Repeat("x", 32), DefaultLocal: registry.SchedulerPolicy{Mode: "permit", ExecutorID: "infer", Completion: "synchronous"}}, released, acquired
}
func expectRelease(t *testing.T, released <-chan bool, want bool) {
	t.Helper()
	select {
	case stopped := <-released:
		if stopped != want {
			t.Fatalf("stopped=%v, want %v", stopped, want)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("permit was not reconciled")
	}
}
func privateJournal(t *testing.T) {
	t.Helper()
	t.Setenv("V_ROUTER_STATE_PATH", filepath.Join(t.TempDir(), "state.json"))
}

func TestAsyncReceiptRetainsPermitAfterClientReturnsAndCloses(t *testing.T) {
	privateJournal(t)
	cfg, released, _ := lifecycleScheduler(t)
	terminal := &atomic.Bool{}
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if r.Method == "POST" {
			_, _ = io.WriteString(w, `{"job_id":"j1","status":"queued"}`)
			return
		}
		state := "running"
		if terminal.Load() {
			state = "succeeded"
		}
		_ = json.NewEncoder(w).Encode(map[string]string{"job_id": "j1", "status": state})
	}))
	defer up.Close()
	u := registry.Upstream{ID: "image", Kind: registry.KindLocal, Protocol: registry.ProtocolImageStudio, BaseURL: up.URL}
	ctx, cancel := context.WithCancel(WithUpstream(context.Background(), u))
	req, _ := http.NewRequestWithContext(ctx, "POST", up.URL+"/api/v1/generate", strings.NewReader(`{}`))
	response, err := (&scheduledTransport{base: http.DefaultTransport, config: cfg}).RoundTrip(req)
	if err != nil {
		t.Fatal(err)
	}
	_, _ = io.ReadAll(response.Body)
	_ = response.Body.Close()
	cancel()
	select {
	case <-released:
		t.Fatal("creation receipt released live async compute")
	case <-time.After(50 * time.Millisecond):
	}
	paths, _ := filepath.Glob(filepath.Join(asyncDir(), "*.json"))
	if len(paths) != 1 {
		t.Fatal("async permit was not durable")
	}
	mode, _ := os.Stat(paths[0])
	if mode.Mode().Perm() != 0600 {
		t.Fatal("async credential journal is not private")
	}
	terminal.Store(true)
	expectRelease(t, released, true)
	waitCondition(t, func() bool { _, err := os.Stat(paths[0]); return os.IsNotExist(err) })
}

func TestMLTerminalPollDoesNotStealResultAndIsOwnerScoped(t *testing.T) {
	privateJournal(t)
	cfg, released, _ := lifecycleScheduler(t)
	reads := &atomic.Int32{}
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if r.Method == "POST" {
			_, _ = io.WriteString(w, `{"code":0,"data":{"task_id":"m1","status":"queued"}}`)
			return
		}
		if reads.Add(1) > 1 {
			http.NotFound(w, r)
			return
		}
		_, _ = io.WriteString(w, `{"code":0,"data":{"task_id":"m1","status":"succeeded","result":{"scenes":[1,2]}}}`)
	}))
	defer up.Close()
	u := registry.Upstream{ID: "ml", Kind: registry.KindLocal, Protocol: registry.ProtocolMLServer, BaseURL: up.URL}
	transport := &scheduledTransport{base: http.DefaultTransport, config: cfg}
	owned := func(owner string) context.Context {
		return WithDelegation(WithUpstream(context.Background(), u), "", owner)
	}
	req, _ := http.NewRequestWithContext(owned("alice"), "POST", up.URL+"/api/v1/video/structure", strings.NewReader(`{}`))
	response, err := transport.RoundTrip(req)
	if err != nil {
		t.Fatal(err)
	}
	_ = response.Body.Close()
	expectRelease(t, released, true)
	waitCondition(t, func() bool {
		data, err := os.ReadFile(asyncPath("permit-test"))
		var job asyncJob
		return err == nil && json.Unmarshal(data, &job) == nil && job.Released
	})
	poll := func(owner string) (int, string) {
		r, _ := http.NewRequestWithContext(owned(owner), "GET", up.URL+"/api/v1/video/structure/tasks/m1", nil)
		res, err := transport.RoundTrip(r)
		if err != nil {
			t.Fatal(err)
		}
		defer res.Body.Close()
		data, _ := io.ReadAll(res.Body)
		return res.StatusCode, string(data)
	}
	if status, _ := poll("bob"); status != 404 {
		t.Fatal("another owner read cached output")
	}
	if status, body := poll("alice"); status != 200 || !strings.Contains(body, `"scenes":[1,2]`) {
		t.Fatalf("terminal output lost: status=%d", status)
	}
	if status, _ := poll("alice"); status != 404 {
		t.Fatal("consumptive result was read twice")
	}
}

func TestAsyncRecoveryDoesNotRestartBusinessJob(t *testing.T) {
	privateJournal(t)
	cfg, released, _ := lifecycleScheduler(t)
	posts := &atomic.Int32{}
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "GET" {
			posts.Add(1)
		}
		_, _ = io.WriteString(w, `{"job_id":"existing","status":"succeeded"}`)
	}))
	defer up.Close()
	configPath := filepath.Join(filepath.Dir(asyncDir()), "scheduler.local.json")
	data, _ := json.Marshal(cfg)
	if err := os.WriteFile(configPath, data, 0600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("V_ROUTER_SCHEDULER_CONFIG", configPath)
	job := &asyncJob{PermitID: "old", UpstreamID: "image", Protocol: registry.ProtocolImageStudio, JobID: "existing", PollURL: up.URL + "/api/v1/jobs/existing", StatusPath: "status", Terminal: []string{"succeeded"}}
	if err := saveAsync(job); err != nil {
		t.Fatal(err)
	}
	ResumeAsync()
	expectRelease(t, released, true)
	waitCondition(t, func() bool { _, err := os.Stat(asyncPath("old")); return os.IsNotExist(err) })
	if posts.Load() != 0 {
		t.Fatal("gateway recovery replayed creation")
	}
}

func TestWebSocketPreservesDuplexAndRequiresServerStopFrame(t *testing.T) {
	for _, normal := range []bool{true, false} {
		t.Run(map[bool]string{true: "server_finish", false: "client_disconnect"}[normal], func(t *testing.T) {
			cfg, released, _ := lifecycleScheduler(t)
			cfg.DefaultLocal.WebSocketCompletion = "server_close"
			cfg.DefaultLocal.MaxInputBytes = 1024
			client, server := net.Pipe()
			defer server.Close()
			upstream := transportFunc(func(r *http.Request) (*http.Response, error) {
				return &http.Response{StatusCode: 101, Header: http.Header{"Upgrade": []string{"websocket"}}, Body: client, ContentLength: -1, Request: r}, nil
			})
			u := registry.Upstream{Kind: registry.KindLocal, BaseURL: "http://stream/v1"}
			req, _ := http.NewRequestWithContext(WithUpstream(context.Background(), u), "GET", u.BaseURL+"/audio/streaming/ws", nil)
			req.Header.Set("Upgrade", "websocket")
			response, err := (&scheduledTransport{base: upstream, config: cfg}).RoundTrip(req)
			if err != nil {
				t.Fatal(err)
			}
			duplex, ok := response.Body.(io.ReadWriteCloser)
			if !ok {
				t.Fatal("permit removed duplex capability")
			}
			if normal {
				done := make(chan struct{})
				go func() {
					defer close(done)
					data := make([]byte, 1)
					_, _ = server.Read(data)
					_, _ = server.Write([]byte{0x88})
					_, _ = server.Write([]byte{2, 3, 0xe8})
				}()
				if _, err := duplex.Write([]byte{1}); err != nil {
					t.Fatal(err)
				}
				observed := make([]byte, 4)
				if _, err := io.ReadFull(duplex, observed); err != nil {
					t.Fatal(err)
				}
				<-done
			}
			_ = duplex.Close()
			expectRelease(t, released, normal)
		})
	}
}

func TestCloseBytesInsideDataCannotReleaseWebSocket(t *testing.T) {
	var normal atomic.Bool
	observer := serverCloseObserver{normal: &normal}
	// A binary payload containing a close frame is data, not stop evidence.
	for _, b := range []byte{0x82, 4, 0x88, 2, 3, 0xe8} {
		observer.feed([]byte{b})
	}
	if normal.Load() {
		t.Fatal("payload was mistaken for a close control frame")
	}
	for _, b := range []byte{0x88, 2, 3, 0xe8} {
		observer.feed([]byte{b})
	}
	if !normal.Load() {
		t.Fatal("fragmented server-close proof was not recognized")
	}
}

func TestModelInputBudgetRejectsUnapprovedGrowthBeforeCompute(t *testing.T) {
	policy := registry.SchedulerPolicy{MaxInputBytes: 128, Limits: map[string]float64{"max_tokens": 64, "options.num_ctx": 2048}}
	for _, body := range []string{`{"max_tokens":65}`, `{"n":8}`, `{"options":{"num_ctx":65536}}`, `{"max_tokens":"NaN"}`} {
		req, _ := http.NewRequest("POST", "http://model/v1/chat/completions", strings.NewReader(body))
		req.Header.Set("Content-Type", "application/json")
		if err := validateInputBudget(req, policy, true); AdmissionStatus(err) != 422 {
			t.Fatalf("unsafe growth was not rejected: %v", err)
		}
	}
	req, _ := http.NewRequest("POST", "http://model/v1/chat/completions", strings.NewReader(`{"max_tokens":64}`))
	req.Header.Set("Content-Type", "application/json")
	if err := validateInputBudget(req, policy, true); err != nil {
		t.Fatal(err)
	}
	data, _ := io.ReadAll(req.Body)
	if !bytes.Contains(data, []byte(`64`)) {
		t.Fatal("budget validation changed input")
	}
	body := &inputBudgetBody{ReadCloser: io.NopCloser(strings.NewReader("12345")), remaining: 4}
	if _, err := io.ReadAll(body); err == nil {
		t.Fatal("unknown-length input escaped its budget")
	}
}

func TestAsyncCreationHTTPErrorDoesNotProveComputeStopped(t *testing.T) {
	for _, status := range []int{408, 500} {
		t.Run(http.StatusText(status), func(t *testing.T) {
			privateJournal(t)
			cfg, released, _ := lifecycleScheduler(t)
			transport := &scheduledTransport{config: cfg, base: transportFunc(func(r *http.Request) (*http.Response, error) {
				return &http.Response{StatusCode: status, Body: io.NopCloser(strings.NewReader("provider error after creation")), ContentLength: 29, Header: make(http.Header), Request: r}, nil
			})}
			transport.once.Do(func() {})
			upstream := registry.Upstream{ID: "async", Kind: registry.KindLocal, Protocol: registry.ProtocolImageStudio, BaseURL: "http://model.test"}
			request := httptest.NewRequest("POST", "http://model.test/api/v1/generate", strings.NewReader(`{}`)).WithContext(WithUpstream(context.Background(), upstream))
			response, err := transport.RoundTrip(request)
			if err != nil {
				t.Fatal(err)
			}
			_, _ = io.ReadAll(response.Body)
			_ = response.Body.Close()
			expectRelease(t, released, false)
			if _, err := os.Stat(asyncPath("permit-test")); err != nil {
				t.Fatal("unknown creation lost its recovery journal")
			}
		})
	}
}
