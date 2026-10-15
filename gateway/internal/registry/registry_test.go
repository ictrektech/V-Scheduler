package registry

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

// Lookup-only convenience for tests; production holds the reservation to completion.
func (r *Registry) Resolve(m string) (Upstream, string, error) { return r.ResolveFor(m, ModelKindChat) }
func (r *Registry) ResolveFor(m, kind string) (Upstream, string, error) {
	path := "/chat/completions"
	if kind == ModelKindEmbedding {
		path = "/embeddings"
	}
	u, n, done, e := r.Acquire(m, path, "")
	if done != nil {
		done()
	}
	return u, n, e
}

func newTestRegistry() *Registry {
	r := New()
	r.Put(&Upstream{ID: "ollama-qa", Kind: KindLocal, Healthy: true, Source: SourceModelHub})
	r.SetModels("ollama-qa", []Model{
		{ID: "qwen3.5:2b", Qualified: "model-hub/qwen3.5:2b", Upstream: "ollama-qa", Kind: ModelKindChat},
	})
	r.Put(&Upstream{ID: "openai", Kind: KindManual, Healthy: true, Source: SourceAPI})
	r.SetModels("openai", []Model{
		{ID: "gpt-4o", Qualified: "manual/gpt-4o", Upstream: "openai", Kind: ModelKindChat},
		{ID: "qwen3.5:2b", Qualified: "manual/qwen3.5:2b", Upstream: "openai", Kind: ModelKindChat},
	})
	return r
}

func TestResolveQualified(t *testing.T) {
	r := newTestRegistry()
	u, model, err := r.Resolve("manual/gpt-4o")
	if err != nil || u.ID != "openai" || model != "gpt-4o" {
		t.Fatalf("qualified resolve got (%v, %q, %v)", u, model, err)
	}
}

func TestResolveShortNameDisabled(t *testing.T) {
	r := newTestRegistry()
	if _, _, err := r.Resolve("qwen3.5:2b"); !errors.Is(err, ErrUnknownModel) {
		t.Fatalf("short names must not route automatically, got %v", err)
	}
}

// Same qualified model on same namespace still rotates across equivalent upstreams.
func TestResolveSameQualifiedDuplicateRotates(t *testing.T) {
	r := newTestRegistry()
	r.Put(&Upstream{ID: "aaa-manual", Kind: KindManual, Healthy: true, Source: SourceAPI})
	r.SetModels("aaa-manual", []Model{
		{ID: "gpt-4o", Qualified: "manual/gpt-4o", Upstream: "aaa-manual", Kind: ModelKindChat},
	})
	for _, want := range []string{"aaa-manual", "openai"} {
		u, model, err := r.Resolve("manual/gpt-4o")
		if err != nil || u.ID != want || model != "gpt-4o" {
			t.Fatalf("resolve qualified duplicate got (%s, %q, %v)", u.ID, model, err)
		}
	}
}

// DuplicateModels lists ids served by several healthy same-kind upstreams.
func TestDuplicateModels(t *testing.T) {
	r := newTestRegistry()
	r.Put(&Upstream{ID: "ollama-extra", Kind: KindLocal, Healthy: true, Source: SourceModelHub})
	r.SetModels("ollama-extra", []Model{
		{ID: "qwen3.5:2b", Qualified: "model-hub/qwen3.5:2b", Upstream: "ollama-extra", Kind: ModelKindChat},
	})
	dup := r.DuplicateModels()
	if len(dup["model-hub/qwen3.5:2b"]) != 2 || len(dup["manual/gpt-4o"]) != 0 {
		t.Fatalf("duplicate map wrong: %+v", dup)
	}
	// And the duplicated qualified id still resolves deterministically.
	u, _, err := r.Resolve("model-hub/qwen3.5:2b")
	if err != nil || u.ID != "ollama-extra" {
		t.Fatalf("duplicate qualified resolve got (%s, %v)", u.ID, err)
	}
}

func TestResolveUnknown(t *testing.T) {
	r := newTestRegistry()
	if _, _, err := r.Resolve("nope"); !errors.Is(err, ErrUnknownModel) {
		t.Fatalf("expected unknown-model error, got %v", err)
	}
}

// Unhealthy upstreams must not serve traffic.
func TestResolveUnhealthy(t *testing.T) {
	r := newTestRegistry()
	u, _ := r.Get("ollama-qa")
	u.Healthy = false
	r.Put(&u)
	if _, _, err := r.Resolve("model-hub/qwen3.5:2b"); !errors.Is(err, ErrUnhealthy) {
		t.Fatalf("expected unhealthy error, got %v", err)
	}
}

func TestListModelsDoesNotExposeShortAliases(t *testing.T) {
	r := newTestRegistry()
	for _, m := range r.ListModels() {
		if m.Qualified == m.ID {
			t.Fatalf("short alias exposed: %+v", m)
		}
	}
}

func TestPersistRoundTripKeepsOnlyAPIUpstreams(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	r := newTestRegistry()
	r.statePath = path
	if err := r.Persist(); err != nil {
		t.Fatalf("persist: %v", err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("state file missing: %v", err)
	}
	loaded, err := Open(path)
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	if _, ok := loaded.Get("openai"); !ok {
		t.Fatal("manual upstream not restored")
	}
	if _, ok := loaded.Get("ollama-qa"); ok {
		t.Fatal("discovered upstream must not be persisted")
	}
}

// Shared model directories (Model Hub ollama instances) put the same model
// on several local upstreams: chat prefers the generate-role upstream and
// embeddings prefer the embedding-role one, deterministically.
func TestResolveForPrefersMatchingRole(t *testing.T) {
	r := New()
	r.Put(&Upstream{ID: "modelhub-model-hub-ollama-embedding", Kind: KindLocal, Role: "embedding", Healthy: true, Source: SourceModelHub})
	r.SetModels("modelhub-model-hub-ollama-embedding", []Model{
		{ID: "qwen3.5:2b", Qualified: "model-hub/qwen3.5:2b", Kind: ModelKindChat},
		{ID: "bge-m3:latest", Qualified: "model-hub/bge-m3:latest", Kind: ModelKindEmbedding},
	})
	r.Put(&Upstream{ID: "modelhub-model-hub-ollama-qa", Kind: KindLocal, Role: RoleGenerate, Healthy: true, Source: SourceModelHub})
	r.SetModels("modelhub-model-hub-ollama-qa", []Model{
		{ID: "qwen3.5:2b", Qualified: "model-hub/qwen3.5:2b", Kind: ModelKindChat},
		{ID: "bge-m3:latest", Qualified: "model-hub/bge-m3:latest", Kind: ModelKindEmbedding},
	})

	if u, _, err := r.ResolveFor("model-hub/qwen3.5:2b", ModelKindChat); err != nil || u.ID != "modelhub-model-hub-ollama-qa" {
		t.Fatalf("chat must pick the generate upstream, got %s (%v)", u.ID, err)
	}
	if u, _, err := r.ResolveFor("model-hub/bge-m3:latest", ModelKindEmbedding); err != nil || u.ID != "modelhub-model-hub-ollama-embedding" {
		t.Fatalf("embeddings must pick the embedding upstream, got %s (%v)", u.ID, err)
	}
	// Chat without a kind preference still resolves (generate preferred).
	if u, _, err := r.Resolve("model-hub/qwen3.5:2b"); err != nil || u.ID != "modelhub-model-hub-ollama-qa" {
		t.Fatalf("kindless chat resolve got %s (%v)", u.ID, err)
	}
}

// ListModels deduplicates identical qualified ids shared across upstreams.
func TestListModelsDedupesSharedModels(t *testing.T) {
	r := New()
	role := func(s string) *string { return &s }
	for _, spec := range []struct {
		id   string
		role *string
	}{
		{"modelhub-model-hub-ollama-embedding", role("embedding")},
		{"modelhub-model-hub-ollama-qa", role(RoleGenerate)},
	} {
		r.Put(&Upstream{ID: spec.id, Kind: KindLocal, Role: *spec.role, Healthy: true, Source: SourceModelHub})
		r.SetModels(spec.id, []Model{
			{ID: "qwen3.5:2b", Qualified: "model-hub/qwen3.5:2b", Kind: ModelKindChat, Upstream: spec.id},
			{ID: "bge-m3:latest", Qualified: "model-hub/bge-m3:latest", Kind: ModelKindEmbedding, Upstream: spec.id},
		})
	}
	models := r.ListModels()
	got := map[string]int{}
	for _, m := range models {
		got[m.Qualified]++
	}
	if got["model-hub/qwen3.5:2b"] != 1 || got["model-hub/bge-m3:latest"] != 1 {
		t.Fatalf("shared models must be listed once: %+v", got)
	}
	if got["qwen3.5:2b"] != 0 {
		t.Fatalf("short alias must stay hidden while duplicated: %+v", got)
	}
}

// The deduped representative kind must be deterministic across map
// iteration orders: chat-kind rows win so dialogue surfaces stay stable.
// Regression for the "(无可用对话模型)" incident where a shared model
// directory labeled qwen3.5:2b as embedding on the embedding upstream and
// random map order picked that row.
func TestListModelsRepresentativeKindIsStable(t *testing.T) {
	build := func() *Registry {
		r := New()
		for i := 0; i < 20; i++ { // many upstreams to expose ordering randomness
			id := "modelhub-model-hub-ollama-x" + string(rune('a'+i))
			r.Put(&Upstream{ID: id, Kind: KindLocal, Role: "embedding", Healthy: true, Source: SourceModelHub})
			r.SetModels(id, []Model{
				{ID: "qwen3.5:2b", Qualified: "model-hub/qwen3.5:2b", Kind: ModelKindEmbedding, Upstream: id},
			})
		}
		r.Put(&Upstream{ID: "modelhub-model-hub-ollama-qa", Kind: KindLocal, Role: RoleGenerate, Healthy: true, Source: SourceModelHub})
		r.SetModels("modelhub-model-hub-ollama-qa", []Model{
			{ID: "qwen3.5:2b", Qualified: "model-hub/qwen3.5:2b", Kind: ModelKindChat, Upstream: "modelhub-model-hub-ollama-qa"},
		})
		return r
	}
	for run := 0; run < 50; run++ {
		models := build().ListModels()
		for _, m := range models {
			if m.Qualified == "model-hub/qwen3.5:2b" && m.Kind != ModelKindChat {
				t.Fatalf("representative must be the chat-kind row, got %s (run %d)", m.Kind, run)
			}
		}
	}
}
