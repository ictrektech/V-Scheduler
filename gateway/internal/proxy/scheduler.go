package proxy

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"github.com/ictrektech/v-router/internal/registry"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

var ErrSchedulerAdmission = errors.New("scheduler admission unavailable")

type AdmissionError struct {
	Status  int
	Message string
}

func (e *AdmissionError) Error() string { return e.Message }
func (e *AdmissionError) Unwrap() error { return ErrSchedulerAdmission }
func AdmissionStatus(err error) int {
	var e *AdmissionError
	if errors.As(err, &e) {
		return e.Status
	}
	return 0
}

type schedulerKey struct{}
type modelKey struct{}
type backgroundKey struct{}
type delegationKey struct{}
type delegation struct{ TaskID, OwnerID string }

func WithModel(ctx context.Context, model string) context.Context {
	return context.WithValue(ctx, modelKey{}, model)
}
func WithBackground(ctx context.Context) context.Context {
	return context.WithValue(ctx, backgroundKey{}, true)
}
func WithDelegation(ctx context.Context, task, owner string) context.Context {
	return context.WithValue(ctx, delegationKey{}, delegation{task, owner})
}

func WithUpstream(ctx context.Context, u registry.Upstream) context.Context {
	return context.WithValue(ctx, schedulerKey{}, u)
}

type SchedulerConfig struct {
	URL                string                              `json:"url"`
	Token              string                              `json:"token"`
	DefaultLocal       registry.SchedulerPolicy            `json:"default_local"`
	Upstreams          map[string]registry.SchedulerPolicy `json:"upstreams"`
	Models             map[string]registry.SchedulerPolicy `json:"models,omitempty"`
	RequireModelPolicy bool                                `json:"require_model_policy,omitempty"`
}
type scheduledTransport struct {
	base   http.RoundTripper
	once   sync.Once
	config *SchedulerConfig
	err    error
}

func NewScheduledTransport(base http.RoundTripper) http.RoundTripper {
	return &scheduledTransport{base: base}
}
func (t *scheduledTransport) load() {
	path := os.Getenv("V_ROUTER_SCHEDULER_CONFIG")
	if path == "" {
		state := os.Getenv("V_ROUTER_STATE_PATH")
		if state == "" {
			state = "/data/state.json"
		}
		path = filepath.Join(filepath.Dir(state), "scheduler.local.json")
		if _, err := os.Stat(path); os.IsNotExist(err) {
			return
		}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.err = err
		return
	}
	var cfg SchedulerConfig
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if decoder.Decode(&cfg) != nil || len(cfg.Token) < 32 || cfg.URL == "" {
		t.err = errors.New("invalid scheduler configuration")
		return
	}
	t.config = &cfg
	if os.Getenv("V_ROUTER_SCHEDULER_REQUIRED") == "true" {
		t.config.RequireModelPolicy = true
	}
}
func schedulerCall(ctx context.Context, cfg *SchedulerConfig, path string, body any, key string) (map[string]json.RawMessage, error) {
	data, _ := json.Marshal(body)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, strings.TrimRight(cfg.URL, "/")+path, bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+cfg.Token)
	req.Header.Set("Content-Type", "application/json")
	if key != "" {
		req.Header.Set("Idempotency-Key", key)
	}
	response, err := (&http.Client{Timeout: 3 * time.Second}).Do(req)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusTooManyRequests {
		return nil, &AdmissionError{429, "local resources busy; retry later"}
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return nil, &AdmissionError{503, "scheduler did not authorize local execution"}
	}
	var value struct {
		Data map[string]json.RawMessage `json:"data"`
	}
	if json.NewDecoder(io.LimitReader(response.Body, 65536)).Decode(&value) != nil {
		return nil, errors.New("invalid scheduler response")
	}
	return value.Data, nil
}
func (t *scheduledTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	// 准入凭据只能由网关产生；客户端提供的同名头在所有分支均清除。
	r = r.Clone(r.Context())
	for _, key := range []string{"X-VScheduler-Permit-ID", "X-VScheduler-Execution-Token", "X-VScheduler-Executor-ID"} {
		r.Header.Del(key)
	}
	u, known := r.Context().Value(schedulerKey{}).(registry.Upstream)
	// Cloud bypass precedes config loading: a broken/offline scheduler never blocks cloud.
	if !known {
		if os.Getenv("V_ROUTER_SCHEDULER_REQUIRED") == "true" {
			return nil, &AdmissionError{503, "model upstream resource scope missing"}
		}
		return t.base.RoundTrip(r)
	}
	if u.Kind == registry.KindCloud && (u.Scheduler == nil || u.Scheduler.Mode == "cloud") || (u.Scheduler != nil && u.Scheduler.Mode == "cloud") {
		if os.Getenv("V_ROUTER_SCHEDULER_REQUIRED") == "true" && u.Scheduler != nil && u.Scheduler.Mode == "cloud" && (strings.TrimRight(u.Scheduler.UpstreamURL, "/") != strings.TrimRight(u.BaseURL, "/") || !requestInUpstream(r, u)) {
			return nil, &AdmissionError{503, "cloud origin approval no longer matches upstream"}
		}
		private := os.Getenv("V_ROUTER_SCHEDULER_REQUIRED") == "true" && privateTarget(r.Context(), r.URL)
		approved := u.Scheduler != nil && u.Scheduler.Mode == "cloud" && strings.TrimRight(u.Scheduler.UpstreamURL, "/") == strings.TrimRight(u.BaseURL, "/") && requestInUpstream(r, u)
		if !private || approved {
			return t.base.RoundTrip(r)
		}
		u.Kind = registry.KindManual
	}
	if os.Getenv("V_ROUTER_SCHEDULER_REQUIRED") == "true" && !requestInUpstream(r, u) {
		return nil, &AdmissionError{503, "model redirect left its approved resource endpoint"}
	}
	if u.Source == registry.SourceSchedulerOllama && os.Getenv("V_ROUTER_SCHEDULER_REQUIRED") == "true" {
		model, _ := r.Context().Value(modelKey{}).(string)
		ready := time.NewTimer(time.Minute)
		defer ready.Stop()
		for {
			if cfg, exists, err := BuiltinSchedulerConfig(u.ID + "/" + model); exists {
				if err != nil {
					return nil, &AdmissionError{503, "built-in Ollama scheduler binding invalid"}
				}
				return t.withConfig(r, u, cfg)
			}
			select {
			case <-r.Context().Done():
				return nil, r.Context().Err()
			case <-ready.C:
				return nil, &AdmissionError{503, "built-in Ollama automatic registration is not ready"}
			case <-time.After(250 * time.Millisecond):
			}
		}
	}
	t.once.Do(t.load)
	// 自动绑定使用独立且受限的网关身份，逐请求读原子快照，使重试/轮换无需重启。
	if u.ID == "app-v-voice" {
		if cfg, exists, err := voiceSchedulerConfig(); exists {
			if err != nil {
				return nil, &AdmissionError{503, "V-Voice scheduler binding invalid"}
			}
			return t.withConfig(r, u, cfg)
		}
	}
	if t.err != nil {
		return nil, &AdmissionError{503, "local scheduler configuration invalid"}
	}
	if t.config == nil {
		if os.Getenv("V_ROUTER_SCHEDULER_REQUIRED") == "true" {
			return nil, &AdmissionError{503, "local scheduler configuration missing"}
		}
		return t.base.RoundTrip(r)
	}
	return t.withConfig(r, u, t.config)
}

func voiceSchedulerConfig() (*SchedulerConfig, bool, error) {
	state := os.Getenv("V_ROUTER_STATE_PATH")
	if state == "" {
		state = "/data/state.json"
	}
	path := filepath.Join(filepath.Dir(state), "voice-scheduler.local.json")
	info, err := os.Lstat(path)
	if os.IsNotExist(err) {
		return nil, false, nil
	}
	if err != nil {
		return nil, true, err
	}
	if !info.Mode().IsRegular() || info.Mode().Perm()&0077 != 0 || info.Size() > 262144 {
		return nil, true, errors.New("binding file must be private and bounded")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, true, err
	}
	var cfg SchedulerConfig
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if err = decoder.Decode(&cfg); err != nil {
		return nil, true, err
	}
	if len(cfg.Token) < 32 || cfg.URL == "" || !cfg.RequireModelPolicy {
		return nil, true, errors.New("invalid scoped binding")
	}
	return &cfg, true, nil
}

func (t *scheduledTransport) withConfig(r *http.Request, u registry.Upstream, cfg *SchedulerConfig) (*http.Response, error) {
	model, _ := r.Context().Value(modelKey{}).(string)
	policy, configured := cfg.Upstreams[u.ID]
	if model != "" {
		if approved, ok := cfg.Models[u.ID+"/"+model]; ok {
			policy, configured = approved, true
		} else if cfg.RequireModelPolicy && (r.Method != http.MethodGet || strings.EqualFold(r.Header.Get("Upgrade"), "websocket")) {
			return nil, &AdmissionError{503, "local model resource policy not approved"}
		}
	}
	if !configured && u.Scheduler != nil {
		policy = *u.Scheduler
		configured = true
	}
	if !configured && u.Kind == registry.KindLocal {
		policy = cfg.DefaultLocal
		configured = true
	}
	if !configured {
		return nil, &AdmissionError{503, "declare scheduler policy for registered/manual upstream"}
	}
	if response, ok := cachedAsyncResponse(r, u); ok {
		return response, nil
	}
	if r.Method == http.MethodGet && !strings.EqualFold(r.Header.Get("Upgrade"), "websocket") || r.Method == http.MethodDelete || strings.HasSuffix(r.URL.Path, "/cancel") || u.Protocol == registry.ProtocolImageStudio && strings.HasSuffix(r.URL.Path, "/api/v1/inputs") {
		return t.base.RoundTrip(r)
	}
	strictInput := cfg.RequireModelPolicy && !(u.Source == registry.SourceSchedulerOllama && strings.HasPrefix(policy.ExecutorID, "builtin-ollama-auto-"))
	if err := validateInputBudget(r, policy, strictInput); err != nil {
		return nil, err
	}
	if err := boundBuiltinInput(r, u, policy); err != nil {
		return nil, err
	}
	var delegatedProof *delegation
	switch policy.Mode {
	case "cloud":
		if os.Getenv("V_ROUTER_SCHEDULER_REQUIRED") == "true" && strings.TrimRight(policy.UpstreamURL, "/") != strings.TrimRight(u.BaseURL, "/") {
			return nil, &AdmissionError{503, "cloud resource ownership requires an origin-bound approval"}
		}
		return t.base.RoundTrip(r)
	case "delegated":
		if policy.ExecutorID == "" {
			policy.ExecutorID = cfg.DefaultLocal.ExecutorID
		}
		if proof, ok := r.Context().Value(delegationKey{}).(delegation); ok && proof.TaskID != "" {
			if proof.OwnerID == "" || policy.ExecutorID == "" {
				return nil, &AdmissionError{503, "delegated execution requires verified owner and executor"}
			}
			delegatedProof = &proof
			break
		}
		// An asynchronous adapter holds a bridge permit until the scheduler
		// verifies ownership transfer. A declaration alone never bypasses it.
		if !isAsyncCreation(r, u) {
			return nil, &AdmissionError{503, "delegated execution requires an active task proof"}
		}
	case "permit":
	default:
		return nil, &AdmissionError{503, "local upstream scheduler policy missing"}
	}
	websocket := strings.EqualFold(r.Header.Get("Upgrade"), "websocket")
	if websocket && policy.WebSocketCompletion != "server_close" {
		return nil, &AdmissionError{503, "WebSocket requires an approved server-close completion contract"}
	}
	if r.Method == http.MethodGet && !websocket || r.Method == http.MethodDelete {
		return t.base.RoundTrip(r)
	}
	if policy.ExecutorID == "" || policy.Completion != "synchronous" && !isAsyncCreation(r, u) {
		return nil, &AdmissionError{503, "local permit requires synchronous execution contract; async services must delegate"}
	}
	var lease *permitLease
	var err error
	if delegatedProof != nil {
		lease, err = acquireDelegation(r.Context(), cfg, policy.ExecutorID, u.BaseURL, model, *delegatedProof)
	} else {
		lease, err = acquirePermit(r.Context(), cfg, policy.ExecutorID, u.BaseURL, model)
		// Own backends need no manual enrollment. Wait for their compute turn
		// instead of turning ordinary contention into an immediate failed chat.
		if u.Source == registry.SourceSchedulerOllama && AdmissionStatus(err) == 429 {
			waiting := time.NewTimer(5 * time.Minute)
			defer waiting.Stop()
		waitForBuiltin:
			for AdmissionStatus(err) == 429 {
				select {
				case <-r.Context().Done():
					return nil, r.Context().Err()
				case <-waiting.C:
					break waitForBuiltin
				case <-time.After(500 * time.Millisecond):
					lease, err = acquirePermit(r.Context(), cfg, policy.ExecutorID, u.BaseURL, model)
				}
			}
		}
	}
	if err != nil {
		if errors.Is(err, ErrSchedulerAdmission) {
			return nil, err
		}
		return nil, &AdmissionError{503, "local scheduler unavailable"}
	}
	ctx, cancel := context.WithCancel(r.Context())
	unlink := context.AfterFunc(lease.ctx, cancel)
	finish := func(stopped bool) {
		unlink()
		cancel()
		evidence := "transport_closed_without_stop_confirmation"
		if stopped {
			evidence = "synchronous_response_completed"
		}
		lease.finish(stopped, evidence)
	}
	var job *asyncJob
	if isAsyncCreation(r, u) {
		job = newAsyncJob(r, u, lease.id)
		if saveAsync(job) != nil {
			lease.finish(true, "async_creation_not_started")
			unlink()
			cancel()
			return nil, &AdmissionError{503, "cannot persist async admission"}
		}
	}
	outgoing := r.Clone(ctx)
	if lease.proof != "" {
		outgoing.Header.Set("X-VScheduler-Permit-ID", lease.id)
		outgoing.Header.Set("X-VScheduler-Execution-Token", lease.proof)
		outgoing.Header.Set("X-VScheduler-Executor-ID", policy.ExecutorID)
	}
	if outgoing.Body != nil && outgoing.ContentLength < 0 && policy.MaxInputBytes > 0 {
		outgoing.Body = &inputBudgetBody{ReadCloser: outgoing.Body, remaining: policy.MaxInputBytes}
	}
	response, err := t.base.RoundTrip(outgoing)
	if err != nil {
		renewalFailed := lease.ctx.Err() != nil && r.Context().Err() == nil
		finish(false)
		if renewalFailed {
			return nil, &AdmissionError{503, "local resource lease could not be renewed"}
		}
		return nil, err
	}
	if job != nil && response.StatusCode >= 200 && response.StatusCode < 300 {
		if err := bindAsyncReceipt(job, response); err != nil {
			if errors.Is(err, errAsyncNotCreated) || errors.Is(err, errAsyncCompleted) {
				removeAsync(job.PermitID)
				finish(true)
				return response, nil
			}
			_ = response.Body.Close()
			finish(false)
			return nil, &AdmissionError{503, "async execution receipt cannot be reconciled"}
		}
		unlink()
		cancel()
		go trackAsync(cfg, lease, job)
		return response, nil
	}
	// An asynchronous endpoint may create work before returning an error.
	// Keep its unresolved journal and reservation; HTTP completion alone is
	// stop evidence only for the approved synchronous contract.
	if websocket && response.StatusCode == http.StatusSwitchingProtocols {
		duplex, ok := response.Body.(io.ReadWriteCloser)
		if !ok {
			_ = response.Body.Close()
			finish(false)
			return nil, &AdmissionError{503, "upstream upgrade did not provide a duplex connection"}
		}
		response.Body = newPermitDuplex(duplex, ctx, policy.MaxInputBytes, func(stopped bool) {
			unlink()
			cancel()
			evidence := "websocket_closed_without_stop_confirmation"
			if stopped {
				evidence = "approved_server_close_after_compute_stopped"
			}
			lease.finish(stopped, evidence)
		}, func() { unlink(); go lease.finish(true, "approved_server_close_after_compute_stopped") })
		return response, nil
	}
	body := &permitBody{ReadCloser: response.Body, ctx: ctx, cancel: cancel, finish: finish, remaining: response.ContentLength, synchronous: job == nil && response.StatusCode != 202}
	// Our Ollama emits [DONE] only after synchronous generation has stopped.
	// A client may close immediately after that event, before transport EOF.
	// Trust this completion contract only for the built-in, fixed-origin backend.
	if body.synchronous && u.Source == registry.SourceSchedulerOllama && response.StatusCode == http.StatusOK && strings.HasPrefix(strings.ToLower(response.Header.Get("Content-Type")), "text/event-stream") {
		body.ollamaCompletion = &ollamaSSECompletion{}
	}
	response.Body = body
	return response, nil
}

type permitBody struct {
	io.ReadCloser
	ctx              context.Context
	cancel           context.CancelFunc
	finish           func(bool)
	remaining        int64
	synchronous      bool
	complete         bool
	ollamaCompletion *ollamaSSECompletion
}

func (b *permitBody) Read(p []byte) (int, error) {
	n, err := b.ReadCloser.Read(p)
	if b.ollamaCompletion != nil && b.ollamaCompletion.feed(p[:n]) {
		b.complete = true
	}
	if b.remaining >= 0 {
		b.remaining -= int64(n)
	}
	if b.ctx.Err() == nil && b.synchronous && (err == io.EOF || b.remaining == 0) {
		b.complete = true
	}
	return n, err
}
func (b *permitBody) Close() error {
	// Converted JSON/SSE consumers may stop before reading the transport EOF.
	// A bounded drain can establish EOF; timeout/partial response stays suspect.
	if !b.complete && b.ctx.Err() == nil && b.synchronous {
		timer := time.AfterFunc(2*time.Second, b.cancel)
		_, _ = io.Copy(io.Discard, io.LimitReader(b, 65536))
		timer.Stop()
	}
	err := b.ReadCloser.Close()
	b.finish(b.complete)
	return err
}
