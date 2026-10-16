package proxy

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"github.com/ictrektech/v-router/internal/registry"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
)

func BuiltinSchedulerPath(model string) string {
	state := os.Getenv("V_ROUTER_STATE_PATH")
	if state == "" {
		state = "/data/state.json"
	}
	digest := sha256.Sum256([]byte(model))
	return filepath.Join(filepath.Dir(state), "ollama-bindings", fmt.Sprintf("%x.json", digest))
}
func BuiltinSchedulerConfig(model string) (*SchedulerConfig, bool, error) {
	path := BuiltinSchedulerPath(model)
	info, err := os.Lstat(path)
	if os.IsNotExist(err) {
		return nil, false, nil
	}
	if err != nil {
		return nil, true, err
	}
	if !info.Mode().IsRegular() || info.Mode().Perm()&0077 != 0 || info.Size() > 262144 {
		return nil, true, fmt.Errorf("private model binding required")
	}
	file, err := os.Open(path)
	if err != nil {
		return nil, true, err
	}
	defer file.Close()
	var cfg SchedulerConfig
	decoder := json.NewDecoder(io.LimitReader(file, 262144))
	decoder.DisallowUnknownFields()
	if err = decoder.Decode(&cfg); err != nil {
		return nil, true, err
	}
	policy, ok := cfg.Models[model]
	if !ok || len(cfg.Models) != 1 || len(cfg.Token) < 32 || cfg.URL != "http://v-scheduler:38120/api/v1" || !cfg.RequireModelPolicy || policy.Mode != "permit" || !strings.HasPrefix(policy.ExecutorID, "builtin-ollama-") {
		return nil, true, fmt.Errorf("invalid scoped model binding")
	}
	return &cfg, true, nil
}

// The backend's installation defaults may exceed a per-model approval. Fill
// omitted inference parameters from that approval instead of inheriting them.
func boundBuiltinInput(r *http.Request, u registry.Upstream, policy registry.SchedulerPolicy) error {
	if u.Source != registry.SourceSchedulerOllama || !strings.HasPrefix(policy.ExecutorID, "builtin-ollama-") {
		return nil
	}
	if !strings.Contains(r.Header.Get("Content-Type"), "json") {
		return &AdmissionError{422, "built-in Ollama inference requires JSON"}
	}
	data, err := io.ReadAll(io.LimitReader(r.Body, policy.MaxInputBytes+1))
	if err != nil {
		return err
	}
	var value map[string]any
	if json.Unmarshal(data, &value) != nil || value == nil {
		return &AdmissionError{422, "invalid model input"}
	}
	if strings.HasPrefix(policy.ExecutorID, "builtin-ollama-auto-") {
		// Installation settings remain authoritative for our own runtimes.
		// Automatic enrollment must not invent smaller context/output limits.
		value["keep_alive"] = 0
		data, err = json.Marshal(value)
		if err != nil {
			return err
		}
		r.Body = io.NopCloser(bytes.NewReader(data))
		r.ContentLength = int64(len(data))
		r.GetBody = func() (io.ReadCloser, error) { return io.NopCloser(bytes.NewReader(data)), nil }
		return validateInputBudget(r, policy, false)
	}
	options, ok := value["options"].(map[string]any)
	if value["options"] != nil && !ok {
		return &AdmissionError{422, "invalid model options"}
	}
	if options == nil {
		options = map[string]any{}
	}
	if options["num_ctx"] == nil {
		options["num_ctx"] = policy.Limits["options.num_ctx"]
	}
	if u.Role == registry.RoleGenerate {
		if value["max_tokens"] == nil && value["max_output_tokens"] == nil {
			value["max_tokens"] = policy.Limits["max_tokens"]
		}
		if options["num_predict"] == nil {
			options["num_predict"] = policy.Limits["max_tokens"]
		}
	}
	value["options"] = options
	// These permits include the model's memory budget; do not leave an untracked
	// indefinitely resident allocation after the synchronous computation stops.
	value["keep_alive"] = 0
	data, err = json.Marshal(value)
	if err != nil {
		return err
	}
	r.Body = io.NopCloser(bytes.NewReader(data))
	r.ContentLength = int64(len(data))
	r.GetBody = func() (io.ReadCloser, error) { return io.NopCloser(bytes.NewReader(data)), nil }
	return validateInputBudget(r, policy, true)
}
