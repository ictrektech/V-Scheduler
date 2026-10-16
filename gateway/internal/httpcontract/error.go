// Package httpcontract defines additive errors shared by gateway API and proxy paths.
package httpcontract

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"regexp"
)

var requestIDPattern = regexp.MustCompile(`^[a-zA-Z0-9_-]{1,96}$`)
var machineCode = regexp.MustCompile(`^[A-Z][A-Z0-9_]{0,95}$`)

func ValidCode(code string) bool { return machineCode.MatchString(code) }
func Code(status int) string {
	switch status {
	case 400, 422:
		return "INVALID_REQUEST"
	case 401:
		return "UNAUTHENTICATED"
	case 403:
		return "FORBIDDEN"
	case 404:
		return "NOT_FOUND"
	case 405:
		return "METHOD_NOT_ALLOWED"
	case 409:
		return "STATE_CONFLICT"
	case 413:
		return "REQUEST_TOO_LARGE"
	case 429:
		return "WAIT_RESOURCE"
	case 502:
		return "UPSTREAM_UNAVAILABLE"
	case 503, 504:
		return "TEMPORARILY_UNAVAILABLE"
	default:
		return "INTERNAL_ERROR"
	}
}
func WriteError(w http.ResponseWriter, status int, message, code, requestID, protocol string) {
	if !ValidCode(code) {
		code = Code(status)
	}
	if !requestIDPattern.MatchString(requestID) {
		var id [16]byte
		_, _ = rand.Read(id[:])
		requestID = hex.EncodeToString(id[:])
	}
	kind := "api_error"
	switch status {
	case 400, 422:
		kind = "invalid_request_error"
	case 401:
		kind = "authentication_error"
	case 403:
		kind = "permission_error"
	case 429:
		kind = "rate_limit_error"
	}
	value := map[string]any{"code": code, "category": "v-scheduler", "data": map[string]any{}, "msg": message, "version": "1", "request_id": requestID, "error": map[string]string{"code": code, "message": message, "type": kind}}
	if protocol == "messages" {
		value["type"] = "error"
	}
	if status == 401 {
		w.Header().Set("WWW-Authenticate", "Bearer")
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("X-Request-ID", requestID)
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}
