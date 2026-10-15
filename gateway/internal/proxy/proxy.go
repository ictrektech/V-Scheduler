// Package proxy forwards an inbound request to an upstream OpenAI-compatible
// endpoint, streaming the response back unchanged.
package proxy

import (
	"errors"
	"net/http"
	"net/http/httputil"
	"net/url"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

// Shared by native and converted requests; defaults allow only two idle
// connections per host, causing avoidable churn under parallel inference.
var Transport = func() *http.Transport {
	t := http.DefaultTransport.(*http.Transport).Clone()
	t.MaxIdleConns, t.MaxIdleConnsPerHost = 128, 64
	t.IdleConnTimeout = 90 * time.Second
	return t
}()

// To streams r to upstream.BaseURL+pathSuffix and writes the response to w.
// The request body already carries the upstream-native model id, so callers
// must rewrite it before invoking this.
// The optional onTransportError callbacks receive the error that made the
// upstream unreachable (dial failures, DNS, timeout); they are not invoked
// for HTTP error answers, which prove the upstream is alive.
func To(w http.ResponseWriter, r *http.Request, upstream registry.Upstream, pathSuffix string, onTransportError ...func(error)) {
	target, err := url.Parse(upstream.BaseURL)
	if err != nil {
		http.Error(w, "invalid upstream base_url: "+err.Error(), http.StatusBadGateway)
		return
	}

	rp := httputil.NewSingleHostReverseProxy(target)
	rp.Transport = NewScheduledTransport(Transport)
	r = r.WithContext(WithUpstream(r.Context(), upstream))
	rp.ModifyResponse = func(resp *http.Response) error {
		resp.Header.Del("X-VRouter-Upstream")
		resp.Header.Del("X-VRouter-Routing")
		return nil
	}
	// Flush immediately so SSE tokens reach the client as they arrive.
	rp.FlushInterval = -1
	rp.ErrorHandler = func(w http.ResponseWriter, r *http.Request, err error) {
		if errors.Is(err, ErrSchedulerAdmission) {
			for _, notify := range onTransportError {
				if notify != nil {
					notify(err)
				}
			}
			w.Header().Set("Retry-After", "1")
			http.Error(w, err.Error(), AdmissionStatus(err))
			return
		}
		for _, notify := range onTransportError {
			if notify != nil {
				notify(err)
			}
		}
		http.Error(w, "upstream "+upstream.ID+" unreachable: "+err.Error(), http.StatusBadGateway)
	}

	origDirector := rp.Director
	rp.Director = func(req *http.Request) {
		origDirector(req)
		req.URL.Path = singleJoiningSlash(target.Path, pathSuffix)
		req.Host = target.Host
		req.URL.RawPath = ""
		req.Header.Del("X-VRouter-Upstream")
		req.Header.Del("X-VRouter-Routing")
		req.Header.Del("X-VScheduler-Task-ID")
		req.Header.Del("Cookie")
		// Browser origin belongs to the gateway, not the server-to-server upstream.
		// Forwarding it can trigger upstream CORS rejection (for example Ollama).
		req.Header.Del("Origin")
		req.Header.Del("X-Api-Key")
		if upstream.APIKey != "" {
			req.Header.Set("Authorization", "Bearer "+upstream.APIKey)
		} else {
			req.Header.Del("Authorization")
		}
		if upstream.Protocol == "anthropic" {
			req.Header.Del("Authorization")
			if upstream.APIKey != "" {
				req.Header.Set("X-Api-Key", upstream.APIKey)
			}
			if req.Header.Get("Anthropic-Version") == "" {
				req.Header.Set("Anthropic-Version", "2023-06-01")
			}
		}
		req.Header.Del("Accept-Encoding")
	}
	rp.ServeHTTP(w, r)
}

func singleJoiningSlash(a, b string) string {
	aslash := len(a) > 0 && a[len(a)-1] == '/'
	bslash := len(b) > 0 && b[0] == '/'
	switch {
	case aslash && bslash:
		return a + b[1:]
	case !aslash && !bslash:
		return a + "/" + b
	}
	return a + b
}
