package proxy

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/ictrektech/v-router/internal/registry"
)

// These origins are the same fixed VOS applications accepted by the controller.
// A discovery response cannot select a credential file or controller URL.
func applicationSchedulerConfig(upstream registry.Upstream) (*SchedulerConfig, bool, error) {
	type application struct{ slug, origin, mode string }
	applications := map[string]application{
		"app-v-music":      {"v-music", "http://v-music-backend:7080/v1", "permit"},
		"app-desensitize":  {"desensitize", "http://desensitize-backend:5000/v1", "permit"},
		"app-image-studio": {"image-studio", "http://image-studio-backend:5000", "delegated"},
	}
	app, known := applications[upstream.ID]
	if !known {
		return nil, false, nil
	}
	state := os.Getenv("V_ROUTER_STATE_PATH")
	if state == "" {
		state = "/data/state.json"
	}
	path := filepath.Join(filepath.Dir(state), "app-bindings", app.slug+".json")
	info, err := os.Lstat(path)
	if os.IsNotExist(err) {
		return nil, false, nil
	}
	if err != nil {
		return nil, true, err
	}
	if strings.TrimRight(upstream.BaseURL, "/") != app.origin {
		return nil, true, errors.New("application origin no longer matches binding")
	}
	if !info.Mode().IsRegular() || info.Mode().Perm()&0077 != 0 || info.Size() > 262144 {
		return nil, true, errors.New("application binding must be private and bounded")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, true, err
	}
	var cfg SchedulerConfig
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if err = decoder.Decode(&cfg); err != nil {
		return nil, true, err
	}
	var extra any
	if decoder.Decode(&extra) != io.EOF {
		return nil, true, errors.New("trailing application binding data")
	}
	if cfg.URL != "http://v-router:8080/scheduler/api/v1" || len(cfg.Token) < 32 || !cfg.RequireModelPolicy || len(cfg.Upstreams) != 0 || cfg.DefaultLocal.Mode != "" {
		return nil, true, errors.New("invalid application credential scope")
	}
	for model, policy := range cfg.Models {
		if !strings.HasPrefix(model, upstream.ID+"/") || policy.Mode != app.mode || policy.Completion != "synchronous" || policy.UpstreamURL != app.origin || !strings.HasPrefix(policy.ExecutorID, "app-gateway-"+app.slug+"-") || policy.MaxInputBytes <= 0 || policy.MaxInputBytes > 33554432 {
			return nil, true, errors.New("invalid application model scope")
		}
	}
	return &cfg, true, nil
}
