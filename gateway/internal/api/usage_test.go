package api

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestUsageObserverReadsCompatibleFormats(t *testing.T) {
	tests := []struct {
		name string
		body string
		want tokenUsage
	}{
		{
			name: "openai chat",
			body: `{"choices":[],"usage":{"prompt_tokens":86,"completion_tokens":13,"total_tokens":99,"prompt_tokens_details":{"cached_tokens":3},"completion_tokens_details":{"reasoning_tokens":11}}}`,
			want: tokenUsage{Reported: 1, Input: 86, Output: 13, Total: 99, Cached: 3, Reasoning: 11},
		},
		{
			name: "ictrek terminal stream chunk with choices",
			body: "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":86,\"completion_tokens\":30,\"total_tokens\":116}}\n\ndata: [DONE]\n\n",
			want: tokenUsage{Reported: 1, Input: 86, Output: 30, Total: 116},
		},
		{
			name: "responses event",
			body: "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":4,\"total_tokens\":16,\"output_tokens_details\":{\"reasoning_tokens\":2}}}}\n\n",
			want: tokenUsage{Reported: 1, Input: 12, Output: 4, Total: 16, Reasoning: 2},
		},
		{
			name: "ollama native",
			body: `{"done":true,"prompt_eval_count":7,"eval_count":5}`,
			want: tokenUsage{Reported: 1, Input: 7, Output: 5, Total: 12},
		},
		{
			name: "anthropic nonstream with prompt caching",
			body: `{"type":"message","usage":{"input_tokens":80,"cache_creation_input_tokens":10,"cache_read_input_tokens":20,"output_tokens":7}}`,
			want: tokenUsage{Reported: 1, Input: 110, Output: 7, Total: 117, Cached: 30},
		},
		{
			name: "anthropic stream merges message start and delta",
			body: "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":80,\"cache_creation_input_tokens\":10,\"cache_read_input_tokens\":20,\"output_tokens\":1}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n",
			want: tokenUsage{Reported: 1, Input: 110, Output: 7, Total: 117, Cached: 30},
		},
		{name: "missing usage is optional", body: `{"choices":[{"message":{"content":"ok"}}]}`, want: tokenUsage{}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var observer usageObserver
			// Split the body to verify observation is independent of HTTP writes.
			middle := len(tt.body) / 2
			observer.Write([]byte(tt.body[:middle]))
			observer.Write([]byte(tt.body[middle:]))
			if got := observer.usage(); got != tt.want {
				t.Fatalf("usage = %+v, want %+v", got, tt.want)
			}
		})
	}
}

func TestUsageLogAggregatesPeriodsAndReloads(t *testing.T) {
	path := filepath.Join(t.TempDir(), "usage.jsonl")
	now := time.Now()
	log := newUsageLog(path)
	log.record(usageRecord{Time: now, User: "alice", Model: "model-hub/qwen", NativeID: "qwen", UpstreamID: "ollama", UpstreamName: "Ollama", Status: 200, UsageReported: true, InputTokens: 10, OutputTokens: 5, TotalTokens: 15})
	log.record(usageRecord{Time: now, User: "bob", Model: "cloud/deepseek", NativeID: "deepseek", UpstreamID: "ictrek", UpstreamName: "ICTrek", Status: 200})
	log.close()

	reloaded := newUsageLog(path)
	defer reloaded.close()
	for _, period := range []string{"day", "week", "month", "all"} {
		snapshot := reloaded.snapshot(period)
		if snapshot.Totals.Requests != 2 || snapshot.Totals.TotalTokens != 15 || snapshot.Totals.UsageRequests != 1 {
			t.Fatalf("%s totals = %+v", period, snapshot.Totals)
		}
		if snapshot.PerModel["ollama/qwen"].InputTokens != 10 {
			t.Fatalf("%s model stats = %+v", period, snapshot.PerModel)
		}
		if snapshot.PerUpstream["ictrek"].Name != "ICTrek" || snapshot.PerUpstream["ictrek"].UsageRequests != 0 {
			t.Fatalf("%s upstream stats = %+v", period, snapshot.PerUpstream)
		}
	}
	files, err := filepath.Glob(filepath.Join(strings.TrimSuffix(path, ".jsonl"), "users", "*.jsonl"))
	if err != nil || len(files) != 2 {
		t.Fatalf("per-user files = %v, err = %v", files, err)
	}
}

func TestUsageLogMigratesLegacyFileByUser(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "usage.jsonl")
	now := time.Now().UTC()
	legacy := []usageRecord{
		{Time: now, User: "alice", UpstreamID: "one", Status: 200},
		{Time: now.Add(time.Second), User: "bob/example", UpstreamID: "two", Status: 200},
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	for _, record := range legacy {
		line, _ := json.Marshal(record)
		_, _ = f.Write(append(line, '\n'))
	}
	_ = f.Close()

	log := newUsageLog(path)
	defer log.close()
	if got := log.snapshot("all").Totals.Requests; got != 2 {
		t.Fatalf("migrated requests = %d", got)
	}
	root := strings.TrimSuffix(path, ".jsonl")
	files, err := filepath.Glob(filepath.Join(root, "users", "*.jsonl"))
	if err != nil || len(files) != 2 {
		t.Fatalf("migrated user files = %v, err = %v", files, err)
	}
	if _, err := os.Stat(filepath.Join(root, "legacy.jsonl")); err != nil {
		t.Fatalf("legacy archive: %v", err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatalf("legacy source still active: %v", err)
	}
}

func TestUsageUserFileNameIsSafeAndStable(t *testing.T) {
	name := usageUserFileName("../../张三")
	if name != usageUserFileName("../../张三") || strings.Contains(name, "/") || strings.HasPrefix(name, ".") || !strings.HasSuffix(name, ".jsonl") {
		t.Fatalf("unsafe user file name: %q", name)
	}
	if name == usageUserFileName("../../李四") {
		t.Fatalf("different users share file name: %q", name)
	}
}
