package proxy

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestVoiceBindingReloadsPrivateScopedSnapshot(t *testing.T) {
	directory := t.TempDir()
	t.Setenv("V_ROUTER_STATE_PATH", filepath.Join(directory, "state.json"))
	path := filepath.Join(directory, "voice-scheduler.local.json")
	if _, exists, _ := voiceSchedulerConfig(); exists {
		t.Fatal("missing binding must keep static compatibility")
	}
	write := func(token string) {
		data, _ := json.Marshal(SchedulerConfig{URL: "http://v-router:8080/scheduler/api/v1", Token: token, RequireModelPolicy: true})
		if err := os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	write(strings.Repeat("a", 32))
	first, exists, err := voiceSchedulerConfig()
	if err != nil || !exists || first.Token != strings.Repeat("a", 32) {
		t.Fatal("valid binding rejected")
	}
	write(strings.Repeat("b", 32))
	second, _, err := voiceSchedulerConfig()
	if err != nil || second.Token == first.Token {
		t.Fatal("rotation did not reload")
	}
	if err = os.Chmod(path, 0644); err != nil {
		t.Fatal(err)
	}
	if _, exists, err = voiceSchedulerConfig(); !exists || err == nil {
		t.Fatal("public binding must fail closed")
	}
}
