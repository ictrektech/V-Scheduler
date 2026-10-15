package registry

import (
	"errors"
	"path/filepath"
	"testing"
	"time"
)

func withFastBreaker(t *testing.T, cooldown time.Duration) {
	t.Helper()
	oldThreshold, oldBase, oldMax := breakerFailureThreshold, breakerBaseCooldown, breakerMaxCooldown
	breakerFailureThreshold, breakerBaseCooldown, breakerMaxCooldown = 3, cooldown, cooldown*4
	t.Cleanup(func() {
		breakerFailureThreshold, breakerBaseCooldown, breakerMaxCooldown = oldThreshold, oldBase, oldMax
	})
}

func breakerUpstream(id string) *Upstream {
	return &Upstream{ID: id, Name: id, Kind: KindManual, Source: SourceAPI, BaseURL: "http://127.0.0.1:9/v1", Healthy: true}
}

func breakerModel(id string) []Model {
	return []Model{{ID: "m", Qualified: "manual/m", Upstream: id, Kind: ModelKindChat}}
}

func TestBreakerTripsExcludesAndRecovers(t *testing.T) {
	withFastBreaker(t, 40*time.Millisecond)
	r := New()
	r.Put(breakerUpstream("a"))
	r.SetModels("a", breakerModel("a"))

	if r.CoolingDown("a") {
		t.Fatal("fresh upstream must not be cooling")
	}
	r.RecordResult("a", false)
	r.RecordResult("a", false)
	if r.CoolingDown("a") {
		t.Fatal("below threshold must not open the breaker")
	}
	r.RecordResult("a", false)
	if !r.CoolingDown("a") {
		t.Fatal("three consecutive transport failures must open the breaker")
	}
	status, _ := r.StatusOf("a")
	if status.State != StateCircuitOpen || status.Failures != 3 || status.OpenUntil.IsZero() {
		t.Fatalf("circuit_open status expected: %#v", status)
	}

	// A success outside the upstream closes the breaker (probe path).
	r.RecordResult("a", true)
	if r.CoolingDown("a") {
		t.Fatal("success must reset the breaker")
	}

	// Cooldown expiry re-allows selection without any bookkeeping.
	r.RecordResult("a", false)
	r.RecordResult("a", false)
	r.RecordResult("a", false)
	time.Sleep(60 * time.Millisecond)
	if r.CoolingDown("a") {
		t.Fatal("elapsed cooldown must re-allow the upstream")
	}
}

func TestBreakerBackoffGrowsAndIsBounded(t *testing.T) {
	withFastBreaker(t, 10*time.Millisecond)
	r := New()
	r.Put(breakerUpstream("a"))

	open := func() time.Time {
		r.RecordResult("a", false)
		r.RecordResult("a", false)
		r.RecordResult("a", false)
		status, _ := r.StatusOf("a")
		return status.OpenUntil
	}
	first := open()
	second := open()
	third := open()
	if !second.After(first) || !third.After(second) {
		t.Fatalf("cooldown must grow with repeated trips: %v %v %v", first, second, third)
	}
	fourth := open()
	fifth := open()
	if !fifth.After(fourth) {
		t.Fatalf("bounded growth expected to continue until cap: %v %v", fourth, fifth)
	}
}

func TestAcquireSkipsCoolingUpstream(t *testing.T) {
	withFastBreaker(t, 200*time.Millisecond)
	r := New()
	for _, id := range []string{"a", "b"} {
		r.Put(breakerUpstream(id))
		r.SetModels(id, breakerModel(id))
	}
	for i := 0; i < 3; i++ {
		r.RecordResult("a", false)
	}

	for i := 0; i < 5; i++ {
		u, _, release, err := r.AcquireFor("manual/m", "/chat/completions", "", "", true)
		if err != nil {
			t.Fatalf("healthy peer must serve: %v", err)
		}
		release()
		if u.ID != "b" {
			t.Fatalf("cooling upstream must be excluded, got %s", u.ID)
		}
	}

	// Pinned selection still reaches the cooling upstream.
	u, _, release, err := r.AcquireFor("m", "/chat/completions", "a", "", true)
	if err != nil || u.ID != "a" {
		t.Fatalf("pinned requests must bypass cooldown: %v %s", err, u.ID)
	}
	release()

	r.Delete("b")
	if _, _, _, err := r.AcquireFor("manual/m", "/chat/completions", "", "", true); !errors.Is(err, ErrUnhealthy) {
		t.Fatalf("only cooling candidates must yield 503 unhealthy, got %v", err)
	}
}

func TestPutAndHeartbeatResetBreaker(t *testing.T) {
	withFastBreaker(t, time.Hour)
	r := New()
	r.Put(breakerUpstream("a"))
	r.SetModels("a", breakerModel("a"))
	r.RecordResult("a", false)
	r.RecordResult("a", false)
	r.RecordResult("a", false)
	if !r.CoolingDown("a") {
		t.Fatal("breaker must be open")
	}

	fresh := breakerUpstream("a")
	r.Put(fresh)
	if r.CoolingDown("a") {
		t.Fatal("fresh snapshot must reset the breaker")
	}

	routed := &Upstream{ID: "reg", Kind: KindRegistered, Source: SourceRegistered, BaseURL: "http://x/v1", Healthy: true}
	r.Register(*routed, breakerModel("reg"))
	for i := 0; i < 3; i++ {
		r.RecordResult("reg", false)
	}
	if !r.CoolingDown("reg") {
		t.Fatal("breaker must be open for the registered upstream")
	}
	if _, err := r.Heartbeat("reg", time.Minute); err != nil {
		t.Fatalf("heartbeat: %v", err)
	}
	if r.CoolingDown("reg") {
		t.Fatal("heartbeat is a liveness signal and must reset the breaker")
	}
}

func TestUpstreamStatusClassification(t *testing.T) {
	r := New()
	expired := &Upstream{ID: "reg", Kind: KindRegistered, Source: SourceRegistered, BaseURL: "http://x/v1", Healthy: true, ExpiresAt: time.Now().Add(-time.Minute)}
	r.Put(expired)
	authFail := breakerUpstream("auth")
	authFail.Healthy = false
	authFail.LastError = "models probe: status 401"
	r.Put(authFail)
	discFail := breakerUpstream("disc")
	discFail.Healthy = false
	discFail.LastError = "models probe: status 503"
	r.Put(discFail)
	noModels := breakerUpstream("empty")
	r.Put(noModels)
	full := breakerUpstream("full")
	full.Healthy = true
	r.Put(full)
	r.SetModels("full", breakerModel("full"))
	if err := r.SetRouting("full", RoutingPolicy{Weight: 1, MaxInflight: 1}); err != nil {
		t.Fatal(err)
	}
	release := r.reserveLocked("full")
	paused := breakerUpstream("paused")
	r.Put(paused)
	r.SetModels("paused", breakerModel("paused"))
	_ = r.SetRouting("paused", RoutingPolicy{Weight: 1, Disabled: true})

	cases := map[string]string{
		"reg":     StateLeaseExpired,
		"auth":    StateAuthFailed,
		"disc":    StateDiscoveryFailed,
		"empty":   StateNoModels,
		"full":    StateFull,
		"paused":  StatePaused,
		"missing": "",
	}
	for id, want := range cases {
		status, ok := r.StatusOf(id)
		if want == "" {
			if ok {
				t.Fatalf("%s: unexpected status", id)
			}
			continue
		}
		if !ok || status.State != want {
			t.Fatalf("%s: want %s got %#v ok=%v", id, want, status, ok)
		}
	}
	release()
	status, _ := r.StatusOf("full")
	if status.State != StateHealthy {
		t.Fatalf("released inflight must return to healthy, got %#v", status)
	}
}

func TestResourceOwnershipPersistenceAndCleanup(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	r, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	r.Put(breakerUpstream("u"))
	if !r.RecordResource("u", "files", "file-1", "user-1") {
		t.Fatal("recording a valid resource must succeed")
	}
	if r.RecordResource("u", "files", "", "user-1") || r.RecordResource("", "files", "x", "user-1") {
		t.Fatal("incomplete records must be rejected")
	}
	if owner, ok := r.ResourceOwner("u", "files", "file-1"); !ok || owner != "user-1" {
		t.Fatalf("owner lookup failed: %q %v", owner, ok)
	}
	if _, ok := r.ResourceOwner("u", "batches", "file-1"); ok {
		t.Fatal("family must be part of the resource key")
	}
	if err := r.Persist(); err != nil {
		t.Fatal(err)
	}

	r2, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	if owner, ok := r2.ResourceOwner("u", "files", "file-1"); !ok || owner != "user-1" {
		t.Fatalf("ownership must survive restart: %q %v", owner, ok)
	}

	r2.Delete("u")
	if _, ok := r2.ResourceOwner("u", "files", "file-1"); ok {
		t.Fatal("deleting an upstream must drop its ownership records")
	}
}
