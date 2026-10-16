package httpcontract

import (
	"encoding/json"
	"net/http/httptest"
	"os"
	"strconv"
	"testing"
)

func TestErrorContractMatchesSharedSpecification(t *testing.T) {
	data, err := os.ReadFile("../../../api/error-contract.json")
	if err != nil {
		t.Fatal(err)
	}
	var spec struct {
		Required []string          `json:"required_error_fields"`
		Codes    map[string]string `json:"http_codes"`
	}
	if err = json.Unmarshal(data, &spec); err != nil {
		t.Fatal(err)
	}
	for status, want := range spec.Codes {
		n, _ := strconv.Atoi(status)
		w := httptest.NewRecorder()
		WriteError(w, n, "safe message", "", "", "messages")
		var value map[string]any
		if json.Unmarshal(w.Body.Bytes(), &value) != nil {
			t.Fatal("invalid JSON")
		}
		for _, key := range spec.Required {
			if _, ok := value[key]; !ok {
				t.Errorf("missing %s", key)
			}
		}
		if value["code"] != want || value["request_id"] != w.Header().Get("X-Request-ID") || value["type"] != "error" {
			t.Errorf("%d: %v", n, value)
		}
	}
}
