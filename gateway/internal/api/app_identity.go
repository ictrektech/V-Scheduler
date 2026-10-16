package api

import (
	"bytes"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
)

// This snapshot is private controller output, never a public registration API.
// The credential authenticates inference only; it grants no compute budget.
func applicationIdentity(token string) (Identity, bool) {
	if !strings.HasPrefix(token, "vs-app-") || len(token) != 71 {
		return Identity{}, false
	}
	state := os.Getenv("V_ROUTER_STATE_PATH")
	if state == "" {
		state = "/data/state.json"
	}
	path := filepath.Join(filepath.Dir(state), "app-identities", "hybrag.json")
	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() || info.Mode().Perm()&0077 != 0 || info.Size() > 4096 {
		return Identity{}, false
	}
	data, err := os.ReadFile(path)
	if err != nil || len(data) > 4096 {
		return Identity{}, false
	}
	var snapshot struct {
		ProtocolVersion int    `json:"protocol_version"`
		BindingRevision uint64 `json:"binding_revision"`
		AppID           string `json:"app_id"`
		Scope           string `json:"scope"`
		Enabled         bool   `json:"enabled"`
		TokenHash       string `json:"token_hash"`
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if decoder.Decode(&snapshot) != nil || decoder.Decode(new(any)) != io.EOF || snapshot.ProtocolVersion != 1 || snapshot.BindingRevision < 1 || snapshot.AppID != "com.ictrek.hybrag" || snapshot.Scope != "gateway-inference-v1" || !snapshot.Enabled {
		return Identity{}, false
	}
	expected, err := hex.DecodeString(snapshot.TokenHash)
	actual := sha256.Sum256([]byte(token))
	if err != nil || len(expected) != sha256.Size || subtle.ConstantTimeCompare(expected, actual[:]) != 1 {
		return Identity{}, false
	}
	return Identity{Sub: "app:com.ictrek.hybrag", Username: "hybrag-service", ServiceApp: "com.ictrek.hybrag"}, true
}

func applicationInferenceAllowed(id Identity, r *http.Request) bool {
	if id.ServiceApp == "" {
		return true
	}
	if id.ServiceApp != "com.ictrek.hybrag" {
		return false
	}
	if r.Method == http.MethodGet && r.URL.Path == "/v1/models" {
		return true
	}
	if r.Method != http.MethodPost {
		return false
	}
	switch r.URL.Path {
	case "/v1/chat/completions", "/v1/embeddings", "/v1/rerank":
		return true
	default:
		return false
	}
}
