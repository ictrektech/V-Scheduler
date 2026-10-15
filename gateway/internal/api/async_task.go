package api

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/ictrektech/v-router/internal/proxy"
	"github.com/ictrektech/v-router/internal/registry"
)

func forwardAsyncTask(w http.ResponseWriter, r *http.Request, u registry.Upstream, path string, body []byte, nativeID string, observe func(error)) {
	protocol := strings.TrimPrefix(path, "/")
	if path != "/images/generations" {
		writeProtocolError(w, protocol, http.StatusBadRequest, "async_task only supports images/generations")
		return
	}
	c := u.AsyncTask
	if c == nil {
		writeProtocolError(w, protocol, http.StatusBadGateway, "async task configuration is missing")
		return
	}
	var input map[string]any
	if err := json.Unmarshal(body, &input); err != nil {
		writeProtocolError(w, protocol, http.StatusBadRequest, "invalid JSON request")
		return
	}
	prompt, _ := input["prompt"].(string)
	if strings.TrimSpace(prompt) == "" {
		writeProtocolError(w, protocol, http.StatusBadRequest, "prompt is required")
		return
	}
	taskInput := cloneMap(c.DefaultInput)
	taskInput["prompt"] = prompt
	create := map[string]any{"model": nativeID}
	if strings.TrimSpace(c.InputPath) == "" {
		for k, v := range taskInput {
			create[k] = v
		}
	} else if err := setJSONPath(create, c.InputPath, taskInput); err != nil {
		writeProtocolError(w, protocol, http.StatusBadGateway, err.Error())
		return
	}
	created, status, err := asyncTaskRequest(r, u, http.MethodPost, c.CreatePath, "", create, observe)
	if err != nil {
		writeProtocolError(w, protocol, status, err.Error())
		return
	}
	taskID := strings.TrimSpace(fmt.Sprint(jsonPath(created, c.TaskIDPath)))
	if taskID == "" || taskID == "<nil>" {
		writeProtocolError(w, protocol, http.StatusBadGateway, "task id missing in upstream response")
		return
	}

	deadline := time.NewTimer(time.Duration(c.TimeoutSeconds) * time.Second)
	defer deadline.Stop()
	ticker := time.NewTicker(time.Duration(c.PollIntervalMS) * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-r.Context().Done():
			return
		case <-deadline.C:
			writeProtocolError(w, protocol, http.StatusGatewayTimeout, "async task timed out")
			return
		case <-ticker.C:
			row, pollStatus, err := asyncTaskRequest(r, u, http.MethodGet, c.QueryPath, taskID, nil, observe)
			if err != nil {
				writeProtocolError(w, protocol, pollStatus, err.Error())
				return
			}
			state := strings.ToLower(strings.TrimSpace(fmt.Sprint(jsonPath(row, c.StatusPath))))
			if stringIn(state, c.FailureStates) {
				writeProtocolError(w, protocol, http.StatusBadGateway, "upstream task failed: "+state)
				return
			}
			if !stringIn(state, c.SuccessStates) {
				continue
			}
			urls, err := asyncResultURLs(row, *c)
			if err != nil {
				writeProtocolError(w, protocol, http.StatusBadGateway, err.Error())
				return
			}
			data := make([]map[string]string, 0, len(urls))
			for _, item := range urls {
				data = append(data, map[string]string{"url": item})
			}
			writeJSON(w, http.StatusOK, map[string]any{"created": time.Now().Unix(), "data": data})
			return
		}
	}
}

func asyncTaskRequest(r *http.Request, u registry.Upstream, method, requestPath, taskID string, payload map[string]any, observe func(error)) (map[string]any, int, error) {
	target, err := url.Parse(strings.TrimRight(u.BaseURL, "/") + "/" + strings.TrimLeft(requestPath, "/"))
	if err != nil {
		return nil, http.StatusBadGateway, errors.New("invalid upstream URL")
	}
	if taskID != "" {
		param := u.AsyncTask.QueryTaskParam
		if strings.TrimSpace(param) == "" {
			param = "taskId"
		}
		q := target.Query()
		q.Set(param, taskID)
		target.RawQuery = q.Encode()
	}
	var reader io.Reader
	if payload != nil {
		data, _ := json.Marshal(payload)
		reader = bytes.NewReader(data)
	}
	req, err := http.NewRequestWithContext(r.Context(), method, target.String(), reader)
	if err != nil {
		return nil, http.StatusBadGateway, errors.New("invalid upstream request")
	}
	if payload != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if u.APIKey != "" {
		req.Header.Set("Authorization", "Bearer "+u.APIKey)
	}
	resp, err := bridgeClient.Do(req)
	if err != nil {
		observe(err)
		if status := proxy.AdmissionStatus(err); status != 0 {
			return nil, status, err
		}
		return nil, http.StatusBadGateway, errors.New("upstream connection failed")
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxInferenceBody+1))
	if err != nil {
		return nil, http.StatusBadGateway, errors.New("upstream read failed")
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, resp.StatusCode, fmt.Errorf("upstream request failed: %s", strings.TrimSpace(string(data)))
	}
	var out map[string]any
	if json.Unmarshal(data, &out) != nil {
		return nil, http.StatusBadGateway, errors.New("invalid upstream JSON response")
	}
	return out, http.StatusOK, nil
}

func asyncResultURLs(row map[string]any, c registry.AsyncTaskConfig) ([]string, error) {
	value := jsonPath(row, c.ResultPath)
	if encoded, ok := value.(string); ok && strings.TrimSpace(c.ResultJSONPath) != "" {
		var decoded any
		if json.Unmarshal([]byte(encoded), &decoded) != nil {
			return nil, errors.New("invalid JSON-encoded task result")
		}
		value = jsonPath(decoded, c.ResultJSONPath)
	} else if strings.TrimSpace(c.ResultJSONPath) != "" {
		value = jsonPath(value, c.ResultJSONPath)
	}
	items, ok := value.([]any)
	if !ok {
		return nil, errors.New("result URL list missing in upstream response")
	}
	urls := make([]string, 0, len(items))
	for _, item := range items {
		if s, ok := item.(string); ok && strings.TrimSpace(s) != "" {
			urls = append(urls, s)
		}
	}
	if len(urls) == 0 {
		return nil, errors.New("upstream returned no result URLs")
	}
	return urls, nil
}

func jsonPath(value any, path string) any {
	for _, part := range strings.Split(strings.TrimSpace(path), ".") {
		if part == "" {
			continue
		}
		obj, ok := value.(map[string]any)
		if !ok {
			return nil
		}
		value = obj[part]
	}
	return value
}

func setJSONPath(root map[string]any, path string, value any) error {
	parts := strings.Split(strings.Trim(path, "."), ".")
	if len(parts) == 0 || parts[0] == "" {
		return errors.New("invalid async task input_path")
	}
	cur := root
	for _, part := range parts[:len(parts)-1] {
		next := map[string]any{}
		cur[part] = next
		cur = next
	}
	cur[parts[len(parts)-1]] = value
	return nil
}

func cloneMap(in map[string]any) map[string]any {
	out := map[string]any{}
	for k, v := range in {
		out[k] = v
	}
	return out
}
func stringIn(value string, list []string) bool {
	for _, item := range list {
		if value == strings.ToLower(strings.TrimSpace(item)) {
			return true
		}
	}
	return false
}
