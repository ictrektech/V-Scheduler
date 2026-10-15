package registry

import (
	"strings"
	"time"
)

// The upstream circuit breaker drops instances with consecutive
// transport-level failures from automatic selection for a bounded,
// exponentially growing cooldown. Upstream 4xx/5xx answers do not trip it:
// a peer that responds is alive. State is in-memory only and clears on
// restart; once a cooldown has elapsed the next request doubles as the
// recovery probe, so no background prober is needed.
var (
	breakerFailureThreshold = 3
	breakerBaseCooldown     = 30 * time.Second
	breakerMaxCooldown      = 5 * time.Minute
)

type breakerState struct {
	failures  int
	openUntil time.Time
}

// RecordResult feeds one transport-level round-trip outcome. ok=true resets
// the failure streak; ok=false opens the breaker once the streak reaches
// breakerFailureThreshold.
func (r *Registry) RecordResult(id string, ok bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.upstreams[id]; !exists {
		return
	}
	if ok {
		delete(r.breaker, id)
		return
	}
	b := r.breaker[id]
	if b == nil {
		b = &breakerState{}
		r.breaker[id] = b
	}
	b.failures++
	if b.failures < breakerFailureThreshold {
		return
	}
	shift := uint(b.failures - breakerFailureThreshold)
	if shift > 4 {
		shift = 4
	}
	backoff := breakerBaseCooldown << shift
	if backoff <= 0 || backoff > breakerMaxCooldown {
		backoff = breakerMaxCooldown
	}
	b.openUntil = time.Now().Add(backoff)
}

// CoolingDown reports whether automatic selection must currently skip the
// upstream. Pinned requests ignore this so operators can probe a suspect
// instance directly.
func (r *Registry) CoolingDown(id string) bool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.coolingLocked(id)
}

func (r *Registry) coolingLocked(id string) bool {
	b := r.breaker[id]
	return b != nil && time.Now().Before(b.openUntil)
}

func (r *Registry) breakerOfLocked(id string) breakerState {
	b := r.breaker[id]
	if b == nil {
		return breakerState{}
	}
	return *b
}

// Upstream states surfaced by the management API; healthy is the normal
// case and the others explain why an upstream is not serving traffic.
const (
	StateHealthy         = "healthy"
	StatePaused          = "paused"
	StateLeaseExpired    = "lease_expired"
	StateCircuitOpen     = "circuit_open"
	StateDiscoveryFailed = "discovery_failed"
	StateAuthFailed      = "auth_failed"
	StateNoModels        = "no_models"
	StateFull            = "full"
)

// UpstreamStatus classifies the current eligibility of one upstream for the
// management API and UI. State is a single label; Detail carries the reason.
type UpstreamStatus struct {
	State     string    `json:"state"`
	Detail    string    `json:"detail,omitempty"`
	Failures  int       `json:"failures,omitempty"`
	OpenUntil time.Time `json:"circuit_open_until,omitempty"`
}

// StatusOf computes the status label of one upstream.
func (r *Registry) StatusOf(id string) (UpstreamStatus, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	u, ok := r.upstreams[id]
	if !ok {
		return UpstreamStatus{}, false
	}
	return r.statusLocked(u), true
}

func (r *Registry) statusLocked(u *Upstream) UpstreamStatus {
	policy := r.routing[u.ID].normalized()
	if policy.Disabled {
		return UpstreamStatus{State: StatePaused, Detail: "调度已暂停，新请求不再分配到该上游"}
	}
	now := time.Now()
	if !u.ExpiresAt.IsZero() && !now.Before(u.ExpiresAt) {
		return UpstreamStatus{State: StateLeaseExpired, Detail: "注册租约已过期，等待服务重新注册"}
	}
	if b := r.breakerOfLocked(u.ID); b.openUntil.After(now) {
		return UpstreamStatus{State: StateCircuitOpen, Detail: "连续连接失败进入冷却；冷却结束后下一次请求自动探测恢复", Failures: b.failures, OpenUntil: b.openUntil}
	}
	if !u.Healthy {
		state, detail := classifyHealthError(u.LastError)
		return UpstreamStatus{State: state, Detail: detail}
	}
	if policy.MaxInflight > 0 && r.inflight[u.ID] >= policy.MaxInflight {
		return UpstreamStatus{State: StateFull, Detail: "进行中请求已达 max_inflight，新请求返回 429"}
	}
	if len(r.models[u.ID]) == 0 {
		return UpstreamStatus{State: StateNoModels, Detail: "上游可达但没有可用模型"}
	}
	return UpstreamStatus{State: StateHealthy}
}

// classifyHealthError sorts a persisted probe/discovery failure into auth
// versus other discovery problems using the well-known probe error shapes.
func classifyHealthError(lastError string) (string, string) {
	detail := strings.TrimSpace(lastError)
	if detail == "" {
		return StateDiscoveryFailed, "上游被来源标记为不健康"
	}
	lowered := strings.ToLower(detail)
	if strings.Contains(lowered, "status 401") || strings.Contains(lowered, "status 403") ||
		strings.Contains(lowered, "unauthorized") || strings.Contains(lowered, "invalid api key") ||
		strings.Contains(lowered, "invalid_api_key") || strings.Contains(lowered, "invalid x-api-key") {
		return StateAuthFailed, detail
	}
	return StateDiscoveryFailed, detail
}
