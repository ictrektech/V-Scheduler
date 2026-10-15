package proxy

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

// Async creation responses are receipts, not proof that GPU work stopped.
// Keep their permit durable until an approved task takes ownership or the
// upstream reports a terminal state. No business inputs or service token are
// copied into this journal; the caller credential needed for polling is private.
type asyncJob struct {
	PermitID    string                    `json:"permit_id"`
	UpstreamID  string                    `json:"upstream_id"`
	OwnerID     string                    `json:"owner_id"`
	Protocol    string                    `json:"protocol"`
	BaseURL     string                    `json:"base_url"`
	CreationURL string                    `json:"creation_url,omitempty"`
	JobID       string                    `json:"job_id"`
	PollURL     string                    `json:"poll_url"`
	TaskID      string                    `json:"task_id,omitempty"`
	StatusPath  string                    `json:"status_path"`
	Terminal    []string                  `json:"terminal"`
	Headers     http.Header               `json:"headers,omitempty"`
	Config      *registry.AsyncTaskConfig `json:"poll_contract,omitempty"`
	Latest      []byte                    `json:"latest,omitempty"`
	HTTPStatus  int                       `json:"http_status"`
	Completed   bool                      `json:"completed,omitempty"`
	Released    bool                      `json:"released,omitempty"`
	Consumed    bool                      `json:"consumed,omitempty"`
	Updated     time.Time                 `json:"updated"`
}

var asyncMu sync.Mutex
var asyncPollInterval = 2 * time.Second
var errAsyncNotCreated = errors.New("application rejected async creation")
var errAsyncCompleted = errors.New("synchronous workflow completed")

func asyncDir() string {
	state := os.Getenv("V_ROUTER_STATE_PATH")
	if state == "" {
		state = "/data/state.json"
	}
	return filepath.Join(filepath.Dir(state), "scheduler-jobs")
}
func asyncPath(id string) string { return filepath.Join(asyncDir(), id+".json") }
func saveAsync(job *asyncJob) error {
	asyncMu.Lock()
	defer asyncMu.Unlock()
	return saveAsyncLocked(job)
}
func saveAsyncLocked(job *asyncJob) error {
	if strings.ContainsAny(job.PermitID, "/\\") || job.PermitID == "" {
		return errors.New("invalid permit ID")
	}
	dir := asyncDir()
	if err := os.MkdirAll(dir, 0700); err != nil {
		return err
	}
	if err := os.Chmod(dir, 0700); err != nil {
		return err
	}
	data, err := json.Marshal(job)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(dir, ".pending-")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err = f.Write(data); err == nil {
		err = f.Sync()
	}
	if closeErr := f.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		return err
	}
	return os.Rename(f.Name(), asyncPath(job.PermitID))
}
func removeAsync(id string) {
	asyncMu.Lock()
	defer asyncMu.Unlock()
	_ = os.Remove(asyncPath(id))
}

func isAsyncCreation(r *http.Request, u registry.Upstream) bool {
	if r.Method != http.MethodPost {
		return false
	}
	switch u.Protocol {
	case registry.ProtocolImageStudio:
		return strings.HasSuffix(r.URL.Path, "/api/v1/generate")
	case registry.ProtocolMLServer:
		return strings.HasSuffix(r.URL.Path, "/api/v1/video/structure")
	case registry.ProtocolAsyncTask:
		return u.AsyncTask != nil && strings.TrimRight(r.URL.Path, "/") == strings.TrimRight(asyncURL(u.BaseURL, u.AsyncTask.CreatePath), "/")
	case "video_structure":
		return strings.HasSuffix(r.URL.Path, "/jobs")
	}
	background, _ := r.Context().Value(backgroundKey{}).(bool)
	return background && strings.HasSuffix(r.URL.Path, "/responses") || strings.HasSuffix(r.URL.Path, "/batches") || strings.HasSuffix(r.URL.Path, "/fine_tuning/jobs") || strings.HasSuffix(r.URL.Path, "/videos") || strings.HasSuffix(r.URL.Path, "/videos/generations")
}
func asyncURL(base, path string) string {
	parsed, err := url.Parse(strings.TrimRight(base, "/") + "/" + strings.TrimLeft(path, "/"))
	if err != nil {
		return ""
	}
	return parsed.Path
}
func newAsyncJob(r *http.Request, u registry.Upstream, id string) *asyncJob {
	owner, _ := r.Context().Value(delegationKey{}).(delegation)
	headers := make(http.Header)
	for _, key := range []string{"Authorization", "X-Api-Key", "Anthropic-Version"} {
		if value := r.Header.Get(key); value != "" {
			headers.Set(key, value)
		}
	}
	job := &asyncJob{PermitID: id, UpstreamID: u.ID, OwnerID: owner.OwnerID, Protocol: u.Protocol, BaseURL: u.BaseURL, CreationURL: r.URL.String(), Headers: headers, Updated: time.Now()}
	switch u.Protocol {
	case registry.ProtocolImageStudio:
		job.StatusPath, job.Terminal = "status", []string{"succeeded", "failed", "cancelled"}
	case registry.ProtocolMLServer:
		job.StatusPath, job.Terminal = "data.status", []string{"succeeded", "failed"}
	case "video_structure":
		job.StatusPath, job.Terminal = "data.status", []string{"succeeded", "failed"}
	case registry.ProtocolAsyncTask:
		contract := *u.AsyncTask
		contract.DefaultInput = nil
		job.Config = &contract
		job.StatusPath = contract.StatusPath
		job.Terminal = append(append([]string{}, contract.SuccessStates...), contract.FailureStates...)
	default:
		job.Protocol = "native"
		job.StatusPath, job.Terminal = "status", []string{"completed", "succeeded", "failed", "cancelled", "canceled", "expired", "incomplete"}
		if strings.HasSuffix(r.URL.Path, "/messages/batches") {
			job.StatusPath, job.Terminal = "processing_status", []string{"ended"}
		}
	}
	return job
}
func valueAt(value any, path string) any {
	for _, part := range strings.Split(path, ".") {
		object, ok := value.(map[string]any)
		if !ok {
			return nil
		}
		value = object[part]
	}
	return value
}
func stringAt(value any, path string) string {
	result, _ := valueAt(value, path).(string)
	return strings.TrimSpace(result)
}
func bindAsyncReceipt(job *asyncJob, response *http.Response) error {
	data, err := io.ReadAll(io.LimitReader(response.Body, (32<<20)+1))
	closeErr := response.Body.Close()
	if err != nil {
		return err
	}
	if closeErr != nil {
		return closeErr
	}
	if len(data) > 32<<20 {
		return errors.New("async receipt too large")
	}
	response.Body = io.NopCloser(bytes.NewReader(data))
	response.ContentLength = int64(len(data))
	var value any
	if json.Unmarshal(data, &value) != nil {
		return errors.New("invalid async receipt")
	}
	switch job.Protocol {
	case registry.ProtocolImageStudio:
		job.JobID = stringAt(value, "job_id")
		base := strings.TrimRight(job.BaseURL, "/")
		if !strings.HasSuffix(base, "/api/v1") {
			base += "/api/v1"
		}
		job.PollURL = base + "/jobs/" + url.PathEscape(job.JobID)
		job.TaskID = stringAt(value, "metadata.scheduler_task_id")
	case registry.ProtocolMLServer:
		job.JobID = stringAt(value, "data.task_id")
		job.PollURL = strings.TrimRight(job.BaseURL, "/") + "/api/v1/video/structure/tasks/" + url.PathEscape(job.JobID)
	case "video_structure":
		job.JobID = stringAt(value, "data.job_id")
		job.PollURL = strings.TrimRight(job.CreationURL, "/") + "/" + url.PathEscape(job.JobID)
	case registry.ProtocolAsyncTask:
		job.JobID = stringAt(value, job.Config.TaskIDPath)
		parsed, err := url.Parse(strings.TrimRight(job.BaseURL, "/") + "/" + strings.TrimLeft(job.Config.QueryPath, "/"))
		if err != nil {
			return err
		}
		parameter := job.Config.QueryTaskParam
		if parameter == "" {
			parameter = "taskId"
		}
		query := parsed.Query()
		query.Set(parameter, job.JobID)
		parsed.RawQuery = query.Encode()
		job.PollURL = parsed.String()
	case "native":
		job.JobID = stringAt(value, "id")
		base := strings.TrimRight(job.CreationURL, "/")
		if strings.HasSuffix(base, "/videos/generations") {
			base = strings.TrimSuffix(base, "/generations")
		}
		job.PollURL = base + "/" + url.PathEscape(job.JobID)
	}
	if job.JobID == "" {
		if job.Protocol == "native" && response.StatusCode != http.StatusAccepted {
			return errAsyncCompleted
		}
		if job.Protocol == "video_structure" {
			if code, ok := valueAt(value, "code").(float64); ok && code != 0 {
				return errAsyncNotCreated
			}
		}
		return errors.New("async job ID missing")
	}
	job.Latest, job.HTTPStatus = data, response.StatusCode
	for _, terminal := range job.Terminal {
		if strings.EqualFold(stringAt(value, job.StatusPath), terminal) {
			job.Completed = true
		}
	}
	job.Updated = time.Now()
	return saveAsync(job)
}

func tryAsyncHandoff(cfg *SchedulerConfig, job *asyncJob) bool {
	if job.TaskID == "" {
		return false
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	data, err := schedulerCall(ctx, cfg, "/permits/"+job.PermitID+"/delegate", map[string]string{"task_id": job.TaskID, "app_task_id": job.JobID}, "")
	var state, task string
	return err == nil && json.Unmarshal(data["state"], &state) == nil && state == "delegated" && json.Unmarshal(data["task_id"], &task) == nil && task == job.TaskID
}
func trackAsync(cfg *SchedulerConfig, lease *permitLease, job *asyncJob) {
	defer lease.stop()
	if job.JobID == "" {
		lease.finish(false, "async_creation_outcome_unknown")
		return
	}
	client := &http.Client{Transport: Transport, Timeout: 10 * time.Second}
	for {
		if tryAsyncHandoff(cfg, job) {
			lease.stop()
			removeAsync(job.PermitID)
			return
		}
		if job.Completed {
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			_, err := schedulerCall(ctx, cfg, "/permits/"+job.PermitID+"/release", map[string]any{"stopped": true, "evidence": "async_terminal_state_confirmed"}, "")
			cancel()
			if err == nil {
				job.Released = true
				asyncMu.Lock()
				// Preserve a consumption already made by a concurrent client.
				if data, readErr := os.ReadFile(asyncPath(job.PermitID)); readErr == nil {
					var current asyncJob
					if json.Unmarshal(data, &current) == nil {
						job.Consumed = current.Consumed
					}
				}
				if job.Protocol == registry.ProtocolImageStudio || job.Protocol == "video_structure" || job.Consumed {
					_ = os.Remove(asyncPath(job.PermitID))
				} else {
					_ = saveAsyncLocked(job)
				}
				asyncMu.Unlock()
				return
			}
		} else {
			request, err := http.NewRequest(http.MethodGet, job.PollURL, nil)
			if err == nil {
				request.Header = job.Headers.Clone()
				response, err := client.Do(request)
				if err == nil {
					data, readErr := io.ReadAll(io.LimitReader(response.Body, (32<<20)+1))
					_ = response.Body.Close()
					var value any
					if readErr == nil && len(data) <= 32<<20 && response.StatusCode >= 200 && response.StatusCode < 300 && json.Unmarshal(data, &value) == nil {
						state := strings.ToLower(stringAt(value, job.StatusPath))
						for _, terminal := range job.Terminal {
							if state == strings.ToLower(terminal) {
								job.Completed = true
							}
						}
						if job.Protocol == registry.ProtocolImageStudio {
							job.TaskID = stringAt(value, "metadata.scheduler_task_id")
						}
						job.Latest, job.HTTPStatus, job.Updated = data, response.StatusCode, time.Now()
						_ = saveAsync(job)
						if job.Completed {
							continue
						}
						if tryAsyncHandoff(cfg, job) {
							lease.stop()
							removeAsync(job.PermitID)
							return
						}
					}
					// Image Studio's DELETE waits for its process to stop before
					// removing the job. A previously recorded job becoming 404
					// is its documented stop contract, not a generic URL rule.
					if response.StatusCode == 404 && job.Protocol == registry.ProtocolImageStudio {
						job.Completed = true
						continue
					}
				}
			}
		}
		time.Sleep(asyncPollInterval)
	}
}

// ML Server terminal reads are consumptive. The tracker caches that response
// durably, so polling for stop evidence cannot steal the result from its owner.
func cachedAsyncResponse(r *http.Request, u registry.Upstream) (*http.Response, bool) {
	if r.Method != http.MethodGet {
		return nil, false
	}
	owner, _ := r.Context().Value(delegationKey{}).(delegation)
	asyncMu.Lock()
	defer asyncMu.Unlock()
	paths, _ := filepath.Glob(filepath.Join(asyncDir(), "*.json"))
	for _, path := range paths {
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		var job asyncJob
		if json.Unmarshal(data, &job) != nil || job.UpstreamID != u.ID || job.PollURL != r.URL.String() || job.Protocol == registry.ProtocolImageStudio || job.Protocol == "video_structure" {
			continue
		}
		if job.Released && time.Since(job.Updated) > time.Hour {
			_ = os.Remove(path)
			return &http.Response{StatusCode: 404, Header: http.Header{"Content-Type": []string{"application/json"}}, Body: io.NopCloser(strings.NewReader(`{"error":"task not found"}`)), Request: r}, true
		}
		status, body := job.HTTPStatus, job.Latest
		if owner.OwnerID != job.OwnerID || job.Consumed {
			status, body = 404, []byte(`{"error":"task not found"}`)
		} else if job.Completed && job.Protocol == registry.ProtocolMLServer {
			job.Consumed = true
			if job.Released {
				_ = os.Remove(path)
			} else {
				_ = saveAsyncLocked(&job)
			}
		}
		return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": []string{"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), ContentLength: int64(len(body)), Request: r}, true
	}
	return nil, false
}

// Called once at gateway startup. Recovery never starts the business job again.
func ResumeAsync() {
	transport := &scheduledTransport{}
	transport.load()
	if transport.err != nil || transport.config == nil {
		return
	}
	paths, _ := filepath.Glob(filepath.Join(asyncDir(), "*.json"))
	for _, path := range paths {
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		var job asyncJob
		if json.Unmarshal(data, &job) != nil || job.Released {
			continue
		}
		lease := resumePermit(transport.config, job.PermitID)
		go trackAsync(transport.config, lease, &job)
	}
}
