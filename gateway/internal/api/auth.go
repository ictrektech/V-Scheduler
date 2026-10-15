package api

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"regexp"
	"strings"
	"time"

	"github.com/ictrektech/v-router/internal/proxy"
)

// Identity is the verified caller of a request. VOS OIDC users come from the
// platform userinfo endpoint; the static gateway key authenticates an
// administrator; internal-network calls without credentials run as
// "internal" (inference only, never management).
type Identity struct {
	Sub      string `json:"sub,omitempty"`
	Username string `json:"username"`
	Email    string `json:"email,omitempty"`
	Admin    bool   `json:"admin"`
}

func (id Identity) ownerKey() string {
	if id.Sub != "" {
		return id.Sub
	}
	if id.Username != "" {
		return id.Username
	}
	if id.Admin {
		return "gateway-admin"
	}
	return ""
}

func (id Identity) label() string {
	switch {
	case id.Admin:
		return "gateway-admin"
	case id.Username != "":
		return id.Username
	default:
		return "internal"
	}
}

func (id Identity) authenticated() bool {
	return id.Admin || id.Username != ""
}

// userInfoResponse is the subset of VOS /v1000/oauth2/userinfo we consume.
type userInfoResponse struct {
	Sub               string `json:"sub"`
	PreferredUsername string `json:"preferred_username"`
	Name              string `json:"name"`
	Email             string `json:"email"`
}

type cachedIdentity struct {
	id      Identity
	expires time.Time
}

const identityCacheTTL = 60 * time.Second

// resolveUserInfoURL picks the VOS userinfo endpoint: explicit env first,
// then the platform-injected VOS_HOST_GW_IP (the gateway of this
// container's network — NOT necessarily the docker0 bridge), then the
// docker0 fallback.
func resolveUserInfoURL() string {
	if u := os.Getenv("VOS_OIDC_USERINFO_URL"); u != "" {
		return u
	}
	host := os.Getenv("VOS_HOST_GW_IP")
	if host == "" {
		host = "172.17.0.1"
	}
	return "http://" + host + ":8105/v1000/oauth2/userinfo"
}

// probeOIDC reports whether the userinfo endpoint answers at all (any HTTP
// status counts as reachable; only network errors do not). Cached briefly
// so the diagnostics endpoint cannot hammer VOS.
func (s *Server) probeOIDC() (bool, string) {
	s.idMu.RLock()
	if time.Now().Before(s.probeOKUntil) {
		err := s.lastUserInfoError
		s.idMu.RUnlock()
		return true, err
	}
	s.idMu.RUnlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, s.userInfoURL, nil)
	if err != nil {
		return false, err.Error()
	}
	resp, err := s.oidcClient.Do(req)
	if err != nil {
		msg := err.Error()
		s.idMu.Lock()
		s.lastUserInfoError = msg
		s.idMu.Unlock()
		return false, msg
	}
	_ = resp.Body.Close()
	s.idMu.Lock()
	s.probeOKUntil = time.Now().Add(30 * time.Second)
	s.lastUserInfoError = ""
	s.idMu.Unlock()
	return true, ""
}

// handleAuthProbe is a public, minimal diagnostics endpoint for the login
// 401 case: it tells the UI whether OIDC is configured and reachable.
func (s *Server) handleAuthProbe(w http.ResponseWriter, _ *http.Request) {
	reachable, errMsg := s.probeOIDC()
	writeJSON(w, http.StatusOK, map[string]any{
		"version":        s.version,
		"oidc_enabled":   s.userInfoURL != "" && !s.authDisabled,
		"oidc_reachable": reachable,
		"oidc_error":     errMsg,
	})
}

// verifyOIDC exchanges a bearer token for a VOS user identity, with a short
// cache to avoid a userinfo round-trip on every call.
func (s *Server) verifyOIDC(token string) (Identity, bool) {
	now := time.Now()
	s.idMu.RLock()
	if c, ok := s.identityCache[token]; ok && now.Before(c.expires) {
		s.idMu.RUnlock()
		return c.id, true
	}
	s.idMu.RUnlock()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, s.userInfoURL, nil)
	if err != nil {
		return Identity{}, false
	}
	req.Header.Set("Authorization", "Bearer "+token)
	resp, err := s.oidcClient.Do(req)
	if err != nil {
		s.idMu.Lock()
		s.lastUserInfoError = err.Error()
		s.idMu.Unlock()
		return Identity{}, false
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		s.idMu.Lock()
		s.lastUserInfoError = "userinfo returned status " + resp.Status
		s.idMu.Unlock()
		return Identity{}, false
	}
	var info userInfoResponse
	if err := json.NewDecoder(http.MaxBytesReader(nil, resp.Body, 1<<20)).Decode(&info); err != nil {
		return Identity{}, false
	}
	if info.Sub == "" && info.PreferredUsername == "" {
		return Identity{}, false
	}
	username := info.PreferredUsername
	if username == "" {
		username = info.Sub
	}
	id := Identity{Sub: info.Sub, Username: username, Email: info.Email}

	s.idMu.Lock()
	if len(s.identityCache) > 512 {
		s.identityCache = map[string]cachedIdentity{}
	}
	s.identityCache[token] = cachedIdentity{id, now.Add(identityCacheTTL)}
	s.idMu.Unlock()
	return id, true
}

// identityFor resolves the caller identity of a request.
func (s *Server) identityFor(r *http.Request) Identity {
	parts := strings.Fields(r.Header.Get("Authorization"))
	if key := r.Header.Get("X-Api-Key"); key != "" {
		if r.Header.Get("Authorization") == "" {
			parts = []string{"Bearer", key}
		} else if len(parts) != 2 || parts[1] != key {
			return Identity{}
		}
	}
	if len(parts) != 2 || !strings.EqualFold(parts[0], "Bearer") {
		return Identity{}
	}
	token := parts[1]
	if token == "" {
		return Identity{}
	}
	if s.apiKey != "" && token == s.apiKey {
		return Identity{Username: "gateway-admin", Admin: true}
	}
	if s.userInfoURL != "" {
		if id, ok := s.verifyOIDC(token); ok {
			s.vSubDiscovery.Refresh(r.Context(), token, id.ownerKey())
			return id
		}
	}
	return Identity{}
}

// requireIdentity guards management endpoints: a verified VOS user or the
// gateway admin key is required. V_ROUTER_AUTH_DISABLED=true is a standalone
// escape hatch and must stay off on VOS installs.
func (s *Server) requireIdentity(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if s.authDisabled {
			next(w, r)
			return
		}
		id := s.identityFor(r)
		if !id.authenticated() {
			w.Header().Set("WWW-Authenticate", "Bearer")
			writeRequestError(w, r, http.StatusUnauthorized, "management API requires a VOS user token or the gateway API key")
			return
		}
		next(w, withRequestIdentity(r, id))
	}
}

type identityKey struct{}

func withIdentity(ctx context.Context, id Identity) context.Context {
	return context.WithValue(ctx, identityKey{}, id)
}

var schedulerOwnerCharacters = regexp.MustCompile(`[^a-zA-Z0-9_.-]+`)

func withRequestIdentity(r *http.Request, id Identity) *http.Request {
	owner := schedulerOwnerCharacters.ReplaceAllString(strings.TrimSpace(id.ownerKey()), "-")
	if len(owner) > 80 {
		owner = owner[:80]
	}
	ctx := proxy.WithDelegation(withIdentity(r.Context(), id), r.Header.Get("X-VScheduler-Task-ID"), owner)
	return r.WithContext(ctx)
}

func identityFrom(r *http.Request) Identity {
	if id, ok := r.Context().Value(identityKey{}).(Identity); ok {
		return id
	}
	return Identity{}
}

// guardAPI is the inference-API policy: the gateway key authenticates when
// configured, an OIDC token attributes the call to a user, and internal
// anonymous calls pass with the "internal" identity.
func (s *Server) guardAPI(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if s.requiresKey() || r.Header.Get("Authorization") != "" || r.Header.Get("X-Api-Key") != "" {
			id := s.identityFor(r)
			if !id.authenticated() {
				w.Header().Set("WWW-Authenticate", "Bearer")
				writeRequestError(w, r, http.StatusUnauthorized, "invalid or missing gateway Key / VOS access token")
				return
			}
			next(w, withRequestIdentity(r, id))
			return
		}
		id := s.identityFor(r)
		if id.Username == "" {
			id.Username = "internal"
		}
		next(w, withRequestIdentity(r, id))
	}
}

func (s *Server) requiresKey() bool { return s.keyRequired || s.apiKey != "" }

func (s *Server) requestIdentity(r *http.Request) Identity {
	id := identityFrom(r)
	if s.authDisabled && !id.authenticated() {
		return Identity{Username: "auth-disabled", Admin: true}
	}
	return id
}

func (s *Server) handleMe(w http.ResponseWriter, r *http.Request) {
	id := identityFrom(r)
	method := "vos_oidc"
	if id.Admin {
		method = "gateway_key"
	}
	if s.authDisabled {
		method = "auth_disabled"
	}
	w.Header().Set("Cache-Control", "no-store")
	writeJSON(w, 200, map[string]any{"identity": id, "auth_method": method, "permissions": []string{"inference", "management", "upstream_resources"}, "resource_isolation": true, "registration_scopes": []string{"public", "private"}})
}
