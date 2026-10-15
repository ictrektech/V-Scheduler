package api

import (
	"encoding/json"
	"strings"
	"testing"
	"unicode"

	"github.com/ictrektech/v-router/internal/registry"
)

// The docs page is only trustworthy if /api/v1/apidoc covers every route the
// mux registers — which holds by construction since both come from spec().
// This test guards against someone bypassing spec() with a direct
// mux.HandleFunc call, or dropping entries from the doc groups.
func TestApidocListing(t *testing.T) {
	srv, _ := newTestServer(t)

	_, body := doJSON(t, "GET", srv.URL+"/api/v1/apidoc", nil)
	var doc struct {
		Version     string            `json:"version"`
		KeyRequired bool              `json:"key_required"`
		BaseURLs    map[string]string `json:"base_urls"`
		Groups      []struct {
			ID        string `json:"id"`
			Title     string `json:"title"`
			Endpoints []struct {
				Method  string `json:"method"`
				Path    string `json:"path"`
				Summary string `json:"summary"`
				Desc    string `json:"desc"`
				Example string `json:"example"`
			} `json:"endpoints"`
		} `json:"groups"`
	}
	if err := json.Unmarshal([]byte(body), &doc); err != nil {
		t.Fatal(err)
	}

	// Collect every documented method+path and compare against the spec.
	documented := map[string]bool{}
	total := 0
	for _, g := range doc.Groups {
		if g.Title == "" || len(g.Endpoints) == 0 {
			t.Fatalf("group %s has no title or endpoints", g.ID)
		}
		for _, e := range g.Endpoints {
			documented[e.Method+" "+e.Path] = true
			if e.Summary == "" || e.Desc == "" {
				t.Fatalf("endpoint %s %s missing summary or desc", e.Method, e.Path)
			}
			total++
		}
	}
	for _, r := range newServer(registry.New(), "test").spec() {
		if !documented[r.Method+" "+r.Path] {
			t.Fatalf("registered route %s %s is missing from /api/v1/apidoc", r.Method, r.Path)
		}
	}

	// The three headline endpoints must carry curl examples.
	for _, want := range []string{"POST /v1/chat/completions", "POST /v1/embeddings", "GET /v1/models", "POST /api/v1/upstreams"} {
		found := false
		for _, g := range doc.Groups {
			for _, e := range g.Endpoints {
				if e.Method+" "+e.Path == want && strings.HasPrefix(e.Example, "curl") {
					found = true
				}
			}
		}
		if !found {
			t.Fatalf("%s must ship a curl example", want)
		}
	}

	if doc.Version == "" || doc.BaseURLs["internal"] == "" || doc.BaseURLs["lan"] == "" {
		t.Fatalf("apidoc must include version and base urls: %s", body)
	}
}

func TestOpenAPIDocumentsVVoiceHotwordFields(t *testing.T) {
	srv, _ := newTestServer(t)
	_, body := doJSON(t, "GET", srv.URL+"/api/v1/openapi.json", nil)
	for _, field := range []string{"hotwords", "hotwords_file", "hotwords_score", "timestamp_granularities[]"} {
		if !strings.Contains(body, `"`+field+`"`) {
			t.Fatalf("OpenAPI is missing %s", field)
		}
	}
}

// English docs must cover the whole running contract, including dynamically
// assembled descriptions, resource families, integration notes and examples.
func TestEnglishDocumentation(t *testing.T) {
	srv, _ := newTestServer(t)
	for _, path := range []string{"/api/v1/apidoc", "/api/v1/openapi.json"} {
		t.Run(path, func(t *testing.T) {
			_, chinese := doJSON(t, "GET", srv.URL+path, nil)
			for _, lang := range []string{"en", "en-US"} {
				_, english := doJSON(t, "GET", srv.URL+path+"?lang="+lang, nil)
				for _, char := range english {
					if unicode.Is(unicode.Han, char) {
						t.Fatalf("English %s contains untranslated text near %q", path, char)
					}
				}
				for _, route := range newServer(registry.New(), "test").spec() {
					want := route.Path
					if strings.Contains(path, "openapi") {
						want = strings.ReplaceAll(want, "...}", "}")
					}
					if !strings.Contains(english, `"`+want+`"`) {
						t.Fatalf("English contract is missing %s", want)
					}
				}
				if english == chinese {
					t.Fatal("English documentation matches Chinese")
				}
			}
			_, fallback := doJSON(t, "GET", srv.URL+path+"?lang=zh-TW", nil)
			if fallback != chinese {
				t.Fatal("Chinese/default documentation changed")
			}
		})
	}
}

func TestDocumentationConfiguredLANPort(t *testing.T) {
	t.Setenv("V_ROUTER_HOST_PORT", "23999")
	srv, _ := newTestServer(t)
	for _, path := range []string{"/api/v1/status", "/api/v1/apidoc", "/api/v1/apidoc?lang=en"} {
		_, body := doJSON(t, "GET", srv.URL+path, nil)
		var doc struct {
			BaseURLs map[string]string `json:"base_urls"`
		}
		if err := json.Unmarshal([]byte(body), &doc); err != nil {
			t.Fatal(err)
		}
		if doc.BaseURLs["lan"] != "http://<vos-host>:23999/v1" {
			t.Fatalf("incorrect LAN URL: %s", body)
		}
	}
}
