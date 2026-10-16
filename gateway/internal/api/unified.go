package api

import (
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"strings"
)

func gatewayAPIBase() string {
	if os.Getenv("V_ROUTER_CONTROLLER_URL") != "" {
		return "/api/com.ictrek.v-scheduler"
	}
	return "/api/com.ictrek.v-router"
}

// The combined VOS package keeps the gateway and controller in separate
// processes. Controller authentication remains authoritative on this proxy.
func unifiedController(mux *http.ServeMux) {
	raw := os.Getenv("V_ROUTER_CONTROLLER_URL")
	if raw == "" {
		return
	}
	target, err := url.Parse(raw)
	if err != nil || target.Host == "" || (target.Scheme != "http" && target.Scheme != "https") || target.User != nil || target.RawQuery != "" || target.Fragment != "" || (target.Path != "" && target.Path != "/") {
		mux.HandleFunc("/scheduler/", func(w http.ResponseWriter, _ *http.Request) {
			http.Error(w, "controller endpoint unavailable", http.StatusServiceUnavailable)
		})
		return
	}
	proxy := httputil.NewSingleHostReverseProxy(target)
	proxy.ErrorHandler = func(w http.ResponseWriter, _ *http.Request, _ error) {
		http.Error(w, "controller unavailable", http.StatusServiceUnavailable)
	}
	mux.Handle("/scheduler/", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		forward := r.Clone(r.Context())
		forward.URL.Path = strings.TrimPrefix(r.URL.Path, "/scheduler")
		forward.URL.RawPath = ""
		proxy.ServeHTTP(w, forward)
	}))
	// The unified UI reaches model storage through a distinct prefix. This
	// forwards management operations only; inference uses the direct Ollama upstreams.
	storageURL := os.Getenv("V_SCHEDULER_MODEL_MANAGEMENT_URL")
	if storageURL == "" {
		storageURL = "http://v-scheduler-model-hub:6005"
	}
	storageTarget, storageErr := url.Parse(storageURL)
	if storageErr == nil && storageTarget.Host != "" && (storageTarget.Scheme == "http" || storageTarget.Scheme == "https") && storageTarget.User == nil && storageTarget.RawQuery == "" && storageTarget.Fragment == "" && (storageTarget.Path == "" || storageTarget.Path == "/") {
		storageProxy := httputil.NewSingleHostReverseProxy(storageTarget)
		storageProxy.FlushInterval = -1
		storageProxy.ErrorHandler = func(w http.ResponseWriter, _ *http.Request, _ error) {
			http.Error(w, "model management unavailable", http.StatusServiceUnavailable)
		}
		mux.Handle("/model-hub/", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			forward := r.Clone(r.Context())
			forward.URL.Path = strings.TrimPrefix(r.URL.Path, "/model-hub")
			forward.URL.RawPath = ""
			storageProxy.ServeHTTP(w, forward)
		}))
	}
	// LAN clients can open the same UI from the existing gateway host port.
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if strings.HasPrefix(r.URL.Path, "/api/") || strings.HasPrefix(r.URL.Path, "/v1/") {
			http.NotFound(w, r)
			return
		}
		proxy.ServeHTTP(w, r)
	})
}
