package proxy

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"math"
	"mime"
	"mime/multipart"
	"net/http"
	"strconv"
	"strings"

	"github.com/ictrektech/v-router/internal/registry"
)

var resourceParameters = []string{
	"max_tokens", "max_output_tokens", "max_completion_tokens", "max_new_tokens",
	"n", "batch_size", "batch_count", "steps", "num_inference_steps", "video_frames", "fps", "width", "height", "duration", "duration_seconds", "num_frames",
	"options.num_ctx", "options.num_batch", "options.num_gpu", "options.num_thread", "context_length",
	"num_ctx", "num_batch", "num_gpu", "num_thread", "max_length", "max_model_len", "max_seq_length", "dimensions",
}

func validateInputBudget(r *http.Request, policy registry.SchedulerPolicy, strict bool) error {
	if policy.MaxInputBytes > 0 && r.ContentLength > policy.MaxInputBytes {
		return &AdmissionError{413, "request exceeds approved model input budget"}
	}
	if ref := r.URL.Query().Get("speaker_model"); ref != "" && !approvedSecondary(policy, ref) {
		return &AdmissionError{422, "secondary model resource budget not approved"}
	}
	if !strict {
		return nil
	}
	if policy.MaxInputBytes <= 0 {
		return &AdmissionError{503, "model policy requires a bounded input budget"}
	}
	if policy.MaxInputBytes > 32<<20 {
		return &AdmissionError{503, "approved input budget exceeds gateway limit"}
	}
	contentType := r.Header.Get("Content-Type")
	if r.Body == nil || !strings.Contains(contentType, "json") && !strings.HasPrefix(contentType, "multipart/") {
		return nil
	}
	data, err := io.ReadAll(io.LimitReader(r.Body, policy.MaxInputBytes+1))
	_ = r.Body.Close()
	if err != nil {
		return err
	}
	if int64(len(data)) > policy.MaxInputBytes {
		return &AdmissionError{413, "request exceeds approved model input budget"}
	}
	r.Body = io.NopCloser(bytes.NewReader(data))
	r.ContentLength = int64(len(data))
	r.GetBody = func() (io.ReadCloser, error) { return io.NopCloser(bytes.NewReader(data)), nil }
	var value any
	if strings.HasPrefix(contentType, "multipart/") {
		_, params, err := mime.ParseMediaType(contentType)
		if err != nil || params["boundary"] == "" {
			return &AdmissionError{422, "invalid multipart model input"}
		}
		fields := map[string]any{}
		reader := multipart.NewReader(bytes.NewReader(data), params["boundary"])
		for {
			part, err := reader.NextPart()
			if err == io.EOF {
				break
			}
			if err != nil {
				return &AdmissionError{422, "invalid multipart model input"}
			}
			name := part.FormName()
			relevant := name == "size" || name == "speaker_model"
			for _, field := range resourceParameters {
				if name == field {
					relevant = true
					break
				}
			}
			if !relevant {
				_ = part.Close()
				continue
			}
			if part.FileName() != "" {
				return &AdmissionError{422, "resource parameter cannot be a file"}
			}
			if _, duplicate := fields[name]; duplicate {
				return &AdmissionError{422, "duplicate resource parameter"}
			}
			raw, err := io.ReadAll(io.LimitReader(part, 4097))
			if err != nil || len(raw) > 4096 {
				return &AdmissionError{422, "resource parameter too large"}
			}
			fields[name] = string(raw)
			_ = part.Close()
		}
		value = fields
	} else if json.Unmarshal(data, &value) != nil {
		return &AdmissionError{422, "invalid model input"}
	}
	for _, field := range resourceParameters {
		v := valueAt(value, field)
		if v == nil {
			continue
		}
		limit, ok := policy.Limits[field]
		if !ok || limit < 0 {
			return &AdmissionError{422, "resource parameter requires an approved model limit: " + field}
		}
		n, numeric := v.(float64)
		if !numeric {
			if raw, ok := v.(string); ok {
				n, err = strconv.ParseFloat(raw, 64)
				numeric = err == nil
			}
		}
		if !numeric || math.IsNaN(n) || math.IsInf(n, 0) || n < 0 || n > limit {
			return &AdmissionError{422, "resource parameter exceeds approved model limit: " + field}
		}
	}
	if ref := stringAt(value, "speaker_model"); ref != "" && !approvedSecondary(policy, ref) {
		return &AdmissionError{422, "secondary model resource budget not approved"}
	}
	if size := stringAt(value, "size"); size != "" && size != "auto" {
		parts := strings.Split(strings.ToLower(size), "x")
		if len(parts) != 2 {
			return &AdmissionError{422, "image dimensions require an approved model limit"}
		}
		width, e1 := strconv.ParseFloat(parts[0], 64)
		height, e2 := strconv.ParseFloat(parts[1], 64)
		if e1 != nil || e2 != nil || width <= 0 || height <= 0 || policy.Limits["width"] < width || policy.Limits["height"] < height {
			return &AdmissionError{422, "image dimensions exceed approved model limit"}
		}
	}
	return nil
}
func approvedSecondary(policy registry.SchedulerPolicy, model string) bool {
	for _, approved := range policy.SecondaryModels {
		if approved == model {
			return true
		}
	}
	return false
}

type inputBudgetBody struct {
	io.ReadCloser
	remaining int64
}

func (b *inputBudgetBody) Read(data []byte) (int, error) {
	if int64(len(data)) > b.remaining+1 {
		data = data[:int(b.remaining+1)]
	}
	n, err := b.ReadCloser.Read(data)
	if int64(n) > b.remaining {
		b.remaining = 0
		return 0, errors.New("approved model input budget exceeded")
	}
	b.remaining -= int64(n)
	return n, err
}
