package api

import (
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
)

// Protect the VOS gateway itself before reading request bodies. Scheduler
// permits cover model execution, not concurrent JSON/multipart buffers here.
type gatewayCapacity struct {
	mu        sync.Mutex
	active    int
	bytes     int64
	maxActive int
	maxBytes  int64
}

func boundedSetting(name string, fallback, maximum int64) int64 {
	value, err := strconv.ParseInt(os.Getenv(name), 10, 64)
	if err != nil || value <= 0 || value > maximum {
		return fallback
	}
	return value
}
func protectGateway(next http.Handler) http.Handler {
	capacity := &gatewayCapacity{maxActive: int(boundedSetting("V_ROUTER_MAX_ACTIVE_REQUESTS", 8, 128)), maxBytes: boundedSetting("V_ROUTER_MAX_BUFFERED_INPUT_MIB", 64, 512) << 20}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		compute := strings.HasPrefix(r.URL.Path, "/v1/") && r.URL.Path != "/v1/models" || strings.HasPrefix(r.URL.Path, "/api/v2/") || r.URL.Path == "/api/v1/scenes/detect"
		if !compute {
			next.ServeHTTP(w, r)
			return
		}
		weight := r.ContentLength
		if weight < 0 {
			weight = maxInferenceBody
		}
		if weight < 1024 {
			weight = 1024
		}
		if weight > maxInferenceBody {
			writeRequestError(w, r, 413, "gateway request body limit exceeded")
			return
		}
		capacity.mu.Lock()
		allowed := capacity.active < capacity.maxActive && capacity.bytes <= capacity.maxBytes-weight
		if allowed {
			capacity.active++
			capacity.bytes += weight
		}
		capacity.mu.Unlock()
		if !allowed {
			w.Header().Set("Retry-After", "1")
			writeRequestError(w, r, 429, "gateway input capacity full; retry later")
			return
		}
		defer func() { capacity.mu.Lock(); capacity.active--; capacity.bytes -= weight; capacity.mu.Unlock() }()
		next.ServeHTTP(w, r)
	})
}
