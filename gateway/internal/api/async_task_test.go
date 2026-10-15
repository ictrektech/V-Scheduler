package api

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestAsyncTaskImageGeneration(t *testing.T) {
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	var polls atomic.Int32
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if got := r.Header.Get("Authorization"); got != "Bearer provider-key" {
			t.Errorf("authorization = %q", got)
		}
		switch r.URL.Path {
		case "/api/v1/jobs/createTask":
			var body map[string]any
			_ = json.NewDecoder(r.Body).Decode(&body)
			if body["model"] != "nano-banana-2" {
				t.Errorf("model = %#v", body["model"])
			}
			input := body["input"].(map[string]any)
			if input["prompt"] != "a tiny fox" || input["resolution"] != "1K" {
				t.Errorf("input = %#v", input)
			}
			writeJSON(w, 200, map[string]any{"code": 200, "data": map[string]any{"taskId": "task-1"}})
		case "/api/v1/jobs/recordInfo":
			if r.URL.Query().Get("taskId") != "task-1" {
				t.Errorf("taskId = %q", r.URL.Query().Get("taskId"))
			}
			state := "generating"
			result := ""
			if polls.Add(1) > 1 {
				state = "success"
				result = `{"resultUrls":["https://cdn.example/result.png"]}`
			}
			writeJSON(w, 200, map[string]any{"data": map[string]any{"state": state, "resultJson": result}})
		default:
			http.NotFound(w, r)
		}
	}))
	defer upstream.Close()

	r := registry.New()
	r.Put(&registry.Upstream{ID: "kie", Name: "Kie", Kind: registry.KindCloud, Source: registry.SourceAPI, Protocol: registry.ProtocolAsyncTask, BaseURL: upstream.URL, APIKey: "provider-key", Healthy: true, AsyncTask: &registry.AsyncTaskConfig{
		CreatePath: "/api/v1/jobs/createTask", QueryPath: "/api/v1/jobs/recordInfo", QueryTaskParam: "taskId",
		TaskIDPath: "data.taskId", StatusPath: "data.state", ResultPath: "data.resultJson", ResultJSONPath: "resultUrls", InputPath: "input",
		SuccessStates: []string{"success"}, FailureStates: []string{"fail"}, DefaultInput: map[string]any{"resolution": "1K"}, PollIntervalMS: 10, TimeoutSeconds: 2,
	}})
	r.SetModels("kie", []registry.Model{{ID: "nano-banana-2", Qualified: "com.kie.api/nano-banana-2", Upstream: "kie", Endpoints: []string{"images/generations"}, Capabilities: []string{"image_generation"}}})
	h := newServer(r, "test").routes()
	req := httptest.NewRequest(http.MethodPost, "/v1/images/generations", strings.NewReader(`{"model":"com.kie.api/nano-banana-2","prompt":"a tiny fox"}`))
	rec := httptest.NewRecorder()
	req.Header.Set("Content-Type", "application/json")
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status=%d body=%s", rec.Code, rec.Body.String())
	}
	if !strings.Contains(rec.Body.String(), "https://cdn.example/result.png") {
		t.Fatalf("body=%s", rec.Body.String())
	}
}

func TestValidateAsyncTaskConfig(t *testing.T) {
	p := upstreamPayload{Name: "Kie", Kind: registry.KindCloud, BaseURL: "https://api.kie.ai", Protocol: registry.ProtocolAsyncTask, Model: "nano-banana-2", AsyncTask: &registry.AsyncTaskConfig{}}
	if err := validateUpstreamPayload(p); err == nil || !strings.Contains(err.Error(), "create_path") {
		t.Fatalf("error = %v", err)
	}
}

func TestCreatePersistentAsyncTaskUpstream(t *testing.T) {
	srv, _ := newTestServer(t)
	payload := upstreamPayload{
		Name: "Kie Async", Kind: registry.KindCloud, BaseURL: "https://api.kie.ai", Protocol: registry.ProtocolAsyncTask, Model: "nano-banana-2",
		AsyncTask: &registry.AsyncTaskConfig{
			CreatePath: "/api/v1/jobs/createTask", QueryPath: "/api/v1/jobs/recordInfo", QueryTaskParam: "taskId",
			TaskIDPath: "data.taskId", StatusPath: "data.state", ResultPath: "data.resultJson", ResultJSONPath: "resultUrls", InputPath: "input",
			SuccessStates: []string{"success"}, FailureStates: []string{"fail"}, PollIntervalMS: 1000, TimeoutSeconds: 300,
		},
	}
	resp, body := doJSON(t, http.MethodPost, srv.URL+"/api/v1/upstreams", payload)
	resp.Body.Close()
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("status=%d body=%s", resp.StatusCode, body)
	}
	if strings.Contains(body, "provider-secret") || !strings.Contains(body, `"protocol":"async_task"`) || !strings.Contains(body, `"model_ids":["nano-banana-2"]`) {
		t.Fatalf("unexpected body: %s", body)
	}
	models, modelBody := doJSON(t, http.MethodGet, srv.URL+"/v1/models", nil)
	models.Body.Close()
	if models.StatusCode != http.StatusOK || !strings.Contains(modelBody, "ai.kie.api/nano-banana-2") {
		t.Fatalf("models status=%d body=%s", models.StatusCode, modelBody)
	}
}
