package registry

import (
	"errors"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

func pool() *Registry {
	r := New()
	for _, id := range []string{"a", "b"} {
		r.Put(&Upstream{ID: id, Kind: KindRegistered, Protocol: "chat_completions", Source: SourceRegistered, Healthy: true})
		r.SetModels(id, []Model{{ID: "same", Qualified: "registered/same", Upstream: id, Kind: ModelKindChat}})
	}
	return r
}
func TestWeightedSchedulingAndCapacity(t *testing.T) {
	r := pool()
	_ = r.SetRouting("a", RoutingPolicy{Weight: 2, MaxInflight: 1})
	_ = r.SetRouting("b", RoutingPolicy{Weight: 1, MaxInflight: 1})
	counts := map[string]int{}
	for i := 0; i < 60; i++ {
		u, _, done, e := r.Acquire("registered/same", "/chat/completions", "")
		if e != nil {
			t.Fatal(e)
		}
		counts[u.ID]++
		done()
	}
	if counts["a"] != 40 || counts["b"] != 20 {
		t.Fatal(counts)
	}
	_, _, a, e := r.Acquire("registered/same", "/chat/completions", "a")
	if e != nil {
		t.Fatal(e)
	}
	u, _, b, e := r.Acquire("registered/same", "/chat/completions", "")
	if e != nil || u.ID != "b" {
		t.Fatalf("must skip busy a: %s %v", u.ID, e)
	}
	if _, _, _, e = r.Acquire("registered/same", "/chat/completions", ""); !errors.Is(e, ErrBusy) {
		t.Fatal(e)
	}
	if _, _, _, e = r.Acquire("registered/same", "/chat/completions", "a"); !errors.Is(e, ErrBusy) {
		t.Fatal("pin must not fall back", e)
	}
	a()
	a()
	b()
	for _, id := range []string{"a", "b"} {
		_, n := r.Routing(id)
		if n != 0 {
			t.Fatal(n)
		}
	}
}
func TestAtomicReservationUnderConcurrency(t *testing.T) {
	r := pool()
	for _, id := range []string{"a", "b"} {
		_ = r.SetRouting(id, RoutingPolicy{MaxInflight: 1})
	}
	var wg sync.WaitGroup
	var mu sync.Mutex
	var releases []func()
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, _, done, e := r.Acquire("registered/same", "/chat/completions", "")
			if e == nil {
				mu.Lock()
				releases = append(releases, done)
				mu.Unlock()
			} else if !errors.Is(e, ErrBusy) {
				t.Error(e)
			}
		}()
	}
	wg.Wait()
	if len(releases) != 2 {
		t.Fatalf("oversubscribed: %d", len(releases))
	}
	for _, done := range releases {
		done()
	}
}
func TestRegistrationLeaseAndSourceBoundary(t *testing.T) {
	r := pool()
	a, _ := r.Get("a")
	a.ExpiresAt = time.Now().Add(-time.Second)
	if e := r.Register(a, r.ModelsOf("a")); e != nil {
		t.Fatal(e)
	}
	if _, e := r.Heartbeat("a", time.Minute); e == nil {
		t.Fatal("expired heartbeat revived service")
	}
	u, _, done, e := r.Acquire("registered/same", "/chat/completions", "")
	if e != nil || u.ID != "b" {
		t.Fatal(u, e)
	}
	done()
	a.ExpiresAt = time.Now().Add(time.Minute)
	if e = r.Register(a, r.ModelsOf("a")); e != nil {
		t.Fatal(e)
	}
	if _, e = r.Heartbeat("a", time.Minute); e != nil {
		t.Fatal(e)
	}
	if e = r.Unregister("a"); e != nil {
		t.Fatal(e)
	}
	r.Put(&Upstream{ID: "a", Source: SourceAPI})
	if e = r.Register(a, nil); !errors.Is(e, ErrRegistrationConflict) {
		t.Fatal(e)
	}
	if e = r.Unregister("a"); !errors.Is(e, ErrRegistrationConflict) {
		t.Fatal(e)
	}
}
func TestPolicySurvivesRediscoveryAndRestart(t *testing.T) {
	r := pool()
	r.statePath = filepath.Join(t.TempDir(), "state.json")
	_ = r.SetRouting("a", RoutingPolicy{Weight: 3, MaxInflight: 2})
	_, _, release, _ := r.Acquire("registered/same", "/chat/completions", "a")
	a, _ := r.Get("a")
	models := r.ModelsOf("a")
	r.DeleteBySource(SourceRegistered)
	_ = r.Register(a, models)
	p, n := r.Routing("a")
	if p.Weight != 3 || n != 1 {
		t.Fatal(p, n)
	}
	release()
	if e := r.Persist(); e != nil {
		t.Fatal(e)
	}
	restored, e := Open(r.statePath)
	if e != nil {
		t.Fatal(e)
	}
	p, n = restored.Routing("a")
	if p.Weight != 3 || n != 0 {
		t.Fatal(p, n)
	}
	if _, ok := restored.Get("a"); ok {
		t.Fatal("leased registration restored without re-probe")
	}
}
func TestEligibilityAndQualifiedScope(t *testing.T) {
	r := pool()
	a, _ := r.Get("a")
	a.Kind = KindLocal
	a.Protocol = "openai"
	r.Put(&a)
	r.SetModels("a", []Model{{ID: "same", Qualified: "model-hub/same", Kind: ModelKindChat}})
	u, _, done, e := r.Acquire("registered/same", "/messages", "")
	if e != nil || u.ID != "b" {
		t.Fatal(u, e)
	}
	done()
	if _, _, _, e = r.Acquire("model-hub/same", "/messages", ""); !errors.Is(e, ErrUnsupported) {
		t.Fatal(e)
	}
	u, _, done, e = r.Acquire("model-hub/same", "/chat/completions", "")
	if e != nil || u.ID != "a" {
		t.Fatal(u, e)
	}
	done()
	_ = r.SetRouting("a", RoutingPolicy{Disabled: true})
	if _, _, _, e = r.Acquire("model-hub/same", "/chat/completions", ""); !errors.Is(e, ErrUnhealthy) {
		t.Fatal(e)
	}
	for _, m := range r.ListModels() {
		if m.Qualified == "model-hub/same" {
			t.Fatal("disabled model advertised", m)
		}
	}
}

func TestPinnedAndOtherScopesDoNotBiasRoundRobin(t *testing.T) {
	r := pool()
	for i := 0; i < 20; i++ {
		_, _, done, e := r.Acquire("registered/same", "/chat/completions", "a")
		if e != nil {
			t.Fatal(e)
		}
		done()
	}
	// Qualified and short names are separate automatic request pools.
	_, _, done, e := r.Acquire("registered/same", "/chat/completions", "")
	if e != nil {
		t.Fatal(e)
	}
	done()
	ids := []string{}
	for i := 0; i < 4; i++ {
		u, _, release, e := r.Acquire("registered/same", "/chat/completions", "")
		if e != nil {
			t.Fatal(e)
		}
		ids = append(ids, u.ID)
		release()
	}
	for i := 1; i < len(ids); i++ {
		if ids[i] == ids[i-1] {
			t.Fatal(ids)
		}
	}
}

func TestCapabilityOnlyRoutingCoversNonChatEndpoints(t *testing.T) {
	r := New()
	r.Put(&Upstream{ID: "u", Kind: KindRegistered, Protocol: ProtocolOpenAI, Source: SourceRegistered, Healthy: true})
	r.SetModels("u", []Model{
		{ID: "img", Qualified: "registered/img", Upstream: "u", Kind: "unknown", Capabilities: []string{"image_generation"}},
		{ID: "video", Qualified: "registered/video", Upstream: "u", Kind: "unknown", Capabilities: []string{"video_generation"}},
		{ID: "tts", Qualified: "registered/tts", Upstream: "u", Kind: "unknown", Capabilities: []string{"tts"}},
		{ID: "rerank", Qualified: "registered/rerank", Upstream: "u", Kind: "unknown", Capabilities: []string{"rerank"}},
	})
	for _, tc := range []struct {
		model string
		ok    string
		bad   string
	}{
		{"registered/img", "/images/generations", "/chat/completions"},
		{"registered/video", "/videos/generations", "/chat/completions"},
		{"registered/tts", "/audio/speech", "/embeddings"},
		{"registered/rerank", "/rerank", "/chat/completions"},
	} {
		_, _, release, err := r.Acquire(tc.model, tc.ok, "")
		if err != nil {
			t.Fatalf("%s should support %s: %v", tc.model, tc.ok, err)
		}
		release()
		if _, _, _, err = r.Acquire(tc.model, tc.bad, ""); !errors.Is(err, ErrUnsupported) {
			t.Fatalf("%s should reject %s: %v", tc.model, tc.bad, err)
		}
	}
}

func TestModelHubSharedModelsUseDedicatedRerankSlots(t *testing.T) {
	r := New()
	for _, role := range []string{RoleGenerate, RoleEmbedding, "rerank"} {
		r.Put(&Upstream{ID: role, Kind: KindLocal, Source: SourceModelHub, Role: role, Healthy: true})
		r.SetModels(role, []Model{{ID: "shared", Qualified: "model-hub/shared", Upstream: role, Capabilities: []string{"completion", "embedding", "rerank"}}})
	}
	_ = r.SetRouting("rerank", RoutingPolicy{MaxInflight: 2})
	releases := []func(){}
	for i := 0; i < 2; i++ {
		u, _, release, err := r.Acquire("model-hub/shared", "/rerank", "")
		if err != nil || u.ID != "rerank" {
			t.Fatalf("dedicated rerank reservation: %+v %v", u, err)
		}
		releases = append(releases, release)
	}
	if _, _, _, err := r.Acquire("model-hub/shared", "/rerank", ""); !errors.Is(err, ErrBusy) {
		t.Fatalf("must not overflow into QA/embedding: %v", err)
	}
	for _, role := range []string{RoleGenerate, RoleEmbedding} {
		u, _ := r.Get(role)
		if supports(&u, r.ModelsOf(role)[0], "/rerank") {
			t.Fatalf("automatic %s must reject rerank", role)
		}
	}
	for _, path := range []string{"/chat/completions", "/responses", "/completions", "/embeddings"} {
		u, _ := r.Get("rerank")
		if supports(&u, r.ModelsOf("rerank")[0], path) {
			t.Fatalf("automatic rerank instance must reject %s", path)
		}
	}
	for _, release := range releases {
		release()
	}
	if _, n := r.Routing("rerank"); n != 0 {
		t.Fatalf("rerank reservations leaked: %d", n)
	}
}
