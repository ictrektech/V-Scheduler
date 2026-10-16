package proxy

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/ictrektech/v-router/internal/registry"
)

var vsubPrivateOrigin = regexp.MustCompile(`^http://v-sub-backend:7090/inference/([a-f0-9]{64})/v1$`)

func vsubSchedulerConfig(upstream registry.Upstream) (*SchedulerConfig, bool, error) {
	if upstream.Source != registry.SourceVSub {
		return nil, false, nil
	}
	state := os.Getenv("V_ROUTER_STATE_PATH")
	if state == "" {
		state = "/data/state.json"
	}
	path := filepath.Join(filepath.Dir(state), "vsub-scheduler.local.json")
	info, err := os.Lstat(path)
	if os.IsNotExist(err) {
		return nil, false, nil
	}
	if err != nil {
		return nil, true, err
	}
	if !info.Mode().IsRegular() || info.Mode().Perm()&0077 != 0 || info.Size() > 262144 {
		return nil, true, errors.New("private V-Sub binding must be bounded")
	}
	match := vsubPrivateOrigin.FindStringSubmatch(upstream.BaseURL)
	if match == nil {
		return nil, true, errors.New("V-Sub upstream must use a full fixed namespace")
	}
	data, err := os.ReadFile(path)
	if err != nil || len(data) > 262144 {
		return nil, true, errors.New("private V-Sub binding unavailable")
	}
	var stateFile struct {
		ProtocolVersion int                        `json:"protocol_version"`
		AppID           string                     `json:"app_id"`
		Namespaces      map[string]SchedulerConfig `json:"namespaces"`
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if decoder.Decode(&stateFile) != nil || decoder.Decode(new(any)) != io.EOF || stateFile.ProtocolVersion != 1 || stateFile.AppID != "com.ictrek.v-sub" {
		return nil, true, errors.New("invalid private V-Sub binding")
	}
	owner := sha256.Sum256([]byte(upstream.Owner))
	expectedNamespace := sha256.Sum256([]byte("v-sub\x00" + upstream.Owner))
	if upstream.Owner == "" || upstream.ID != "app-v-sub-"+hex.EncodeToString(owner[:]) || match[1] != hex.EncodeToString(expectedNamespace[:]) {
		return nil, true, errors.New("private V-Sub namespace does not belong to verified owner")
	}
	namespace := match[1]
	cfg, exists := stateFile.Namespaces[namespace]
	if !exists {
		return nil, true, errors.New("private namespace is not calibrated")
	}
	if cfg.URL != "http://v-router:8080/scheduler/api/v1" || len(cfg.Token) < 32 || !cfg.RequireModelPolicy || len(cfg.Upstreams) != 0 || cfg.DefaultLocal.Mode != "" {
		return nil, true, errors.New("invalid V-Sub credential scope")
	}
	models := make(map[string]registry.SchedulerPolicy, len(cfg.Models))
	for model, policy := range cfg.Models {
		if !strings.HasPrefix(model, "v-sub-"+namespace[:12]+"/") || policy.Mode != "permit" || policy.Completion != "synchronous" || policy.UpstreamURL != upstream.BaseURL || !strings.HasPrefix(policy.ExecutorID, "app-gateway-v-sub-") || policy.MaxInputBytes <= 0 || policy.MaxInputBytes > 33554432 {
			return nil, true, errors.New("invalid private V-Sub model scope")
		}
		models[upstream.ID+"/"+model] = policy
	}
	cfg.Models = models
	return &cfg, true, nil
}
