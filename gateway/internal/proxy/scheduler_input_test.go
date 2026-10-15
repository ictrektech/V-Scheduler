package proxy

import (
	"bytes"
	"io"
	"mime/multipart"
	"net/http/httptest"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestMultipartResourceBudgetCannotBypassModelLimits(t *testing.T) {
	policy := registry.SchedulerPolicy{MaxInputBytes: 1 << 20, Limits: map[string]float64{"n": 1, "width": 256, "height": 256}}
	for _, test := range []struct {
		name, value string
		status      int
	}{{"n", "1", 0}, {"n", "1000", 422}, {"size", "4096x4096", 422}, {"max_tokens", "100000", 422}} {
		var body bytes.Buffer
		form := multipart.NewWriter(&body)
		file, _ := form.CreateFormFile("image", "input.png")
		_, _ = file.Write([]byte("fixture-image"))
		_ = form.WriteField(test.name, test.value)
		_ = form.Close()
		original := append([]byte(nil), body.Bytes()...)
		request := httptest.NewRequest("POST", "http://model.test/v1/images/edits", bytes.NewReader(original))
		request.Header.Set("Content-Type", form.FormDataContentType())
		err := validateInputBudget(request, policy, true)
		if AdmissionStatus(err) != test.status {
			t.Fatalf("%s=%s: got %v", test.name, test.value, err)
		}
		if test.status == 0 {
			restored, _ := io.ReadAll(request.Body)
			if !bytes.Equal(restored, original) {
				t.Fatal("admission altered the native multipart payload")
			}
		}
	}
}
