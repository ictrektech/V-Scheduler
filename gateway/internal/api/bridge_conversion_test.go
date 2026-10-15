package api

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

func testBridgeGateway(upstream string) http.Handler {
	r := registry.New()
	r.Put(&registry.Upstream{ID: "u", Protocol: "chat_completions", BaseURL: upstream + "/v1", Healthy: true, APIKey: "provider-secret"})
	r.SetModels("u", []registry.Model{{ID: "native", Qualified: "local/native", Upstream: "u"}})
	return newServer(r, "test").routes()
}

const chatTextResult = `{"id":"chat_test","choices":[{"index":0,"message":{"role":"assistant","content":"Hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}`
const chatToolResult = `{"id":"chat_test","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_test","type":"function","function":{"name":"weather","arguments":"{\"city\":\"Paris\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}`

func TestConvertedProtocolsAndToolRoundtrip(t *testing.T) {
	for _, protocol := range []string{"responses", "messages"} {
		t.Run(protocol, func(t *testing.T) {
			var received jsonObject
			up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path != "/v1/chat/completions" {
					t.Errorf("wrong path %s", r.URL.Path)
				}
				if r.Header.Get("Authorization") != "Bearer provider-secret" || r.Header.Get("Cookie") != "" || r.Header.Get("X-Api-Key") != "" {
					t.Error("wrong credentials")
				}
				_ = json.NewDecoder(r.Body).Decode(&received)
				result := chatToolResult
				for _, m := range received["messages"].([]any) {
					if m.(map[string]any)["role"] == "tool" {
						result = chatTextResult
					}
				}
				w.Header().Set("Content-Type", "application/json")
				_, _ = io.WriteString(w, result)
			}))
			defer up.Close()
			gateway := testBridgeGateway(up.URL)
			input := `{"model":"local/native","input":"weather?","tools":[{"type":"function","name":"weather","parameters":{"type":"object"}}]}`
			if protocol == "messages" {
				input = `{"model":"local/native","messages":[{"role":"user","content":"weather?"}],"max_tokens":20,"tools":[{"name":"weather","input_schema":{"type":"object"}}]}`
			}
			call := func(input string) jsonObject {
				req := httptest.NewRequest("POST", "/v1/"+protocol, strings.NewReader(input))
				req.Header.Set("Cookie", "private")
				w := httptest.NewRecorder()
				gateway.ServeHTTP(w, req)
				if w.Code != 200 {
					t.Fatalf("%d %s", w.Code, w.Body.String())
				}
				var out jsonObject
				_ = json.Unmarshal(w.Body.Bytes(), &out)
				return out
			}
			first := call(input)
			if received["model"] != "native" {
				t.Error("model rewrite failed")
			}
			if protocol == "responses" {
				item := first["output"].([]any)[0].(map[string]any)
				if item["type"] != "function_call" || item["call_id"] != "call_test" {
					t.Fatalf("invalid tool: %v", item)
				}
				second := call(`{"model":"local/native","input":[{"role":"user","content":"weather?"},{"type":"function_call","call_id":"call_test","name":"weather","arguments":"{\"city\":\"Paris\"}"},{"type":"function_call_output","call_id":"call_test","output":"sunny"}]}`)
				text := second["output"].([]any)[0].(map[string]any)["content"].([]any)[0].(map[string]any)["text"]
				if text != "Hello" {
					t.Error("tool result roundtrip lost text")
				}
			} else {
				item := first["content"].([]any)[0].(map[string]any)
				if item["type"] != "tool_use" || item["id"] != "call_test" || first["stop_reason"] != "tool_use" {
					t.Fatalf("invalid tool: %v", first)
				}
				second := call(`{"model":"local/native","max_tokens":20,"messages":[{"role":"user","content":"weather?"},{"role":"assistant","content":[{"type":"tool_use","id":"call_test","name":"weather","input":{"city":"Paris"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_test","content":"sunny"}]}]}`)
				if second["content"].([]any)[0].(map[string]any)["text"] != "Hello" {
					t.Error("tool result roundtrip lost text")
				}
			}
			last := received["messages"].([]any)
			if last[len(last)-1].(map[string]any)["tool_call_id"] != "call_test" {
				t.Error("tool identity lost")
			}
		})
	}
}

func TestConvertedStreamFirstTextAndCancel(t *testing.T) {
	for _, protocol := range []string{"responses", "messages"} {
		t.Run(protocol, func(t *testing.T) {
			cancelled := make(chan struct{})
			up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("Content-Type", "text/event-stream")
				_, _ = io.WriteString(w, "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n")
				w.(http.Flusher).Flush()
				<-r.Context().Done()
				close(cancelled)
			}))
			defer up.Close()
			gateway := httptest.NewServer(testBridgeGateway(up.URL))
			defer gateway.Close()
			body := `{"model":"local/native","input":"hi","stream":true}`
			if protocol == "messages" {
				body = `{"model":"local/native","messages":[{"role":"user","content":"hi"}],"max_tokens":20,"stream":true}`
			}
			client := &http.Client{Timeout: 2 * time.Second}
			resp, err := client.Post(gateway.URL+"/v1/"+protocol, "application/json", strings.NewReader(body))
			if err != nil {
				t.Fatal(err)
			}
			scanner := bufio.NewScanner(resp.Body)
			found := false
			for scanner.Scan() {
				if strings.Contains(scanner.Text(), `"Hello"`) {
					found = true
					break
				}
			}
			resp.Body.Close()
			if !found {
				t.Fatal("text buffered until upstream completion")
			}
			select {
			case <-cancelled:
			case <-time.After(time.Second):
				t.Fatal("client cancellation did not reach upstream")
			}
		})
	}
}

func TestConvertedStreamingToolsAndErrors(t *testing.T) {
	for _, protocol := range []string{"responses", "messages"} {
		for _, broken := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/broken=%v", protocol, broken), func(t *testing.T) {
				stream := "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_test\",\"function\":{\"name\":\"weather\",\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n" +
					"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Paris\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n"
				if broken {
					stream += "data: {invalid}\n\n"
				} else {
					stream += "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n"
				}
				w := httptest.NewRecorder()
				streamChat(w, httptest.NewRequest("POST", "/", nil), strings.NewReader(stream), protocol, "native")
				text := w.Body.String()
				if broken {
					if !strings.Contains(text, "event: error") || strings.Contains(text, "event: response.completed") || strings.Contains(text, "event: message_stop") {
						t.Fatal("failed stream reported success")
					}
					return
				}
				if !strings.Contains(text, "call_test") {
					t.Fatal("tool ID missing")
				}
				if protocol == "responses" {
					if !strings.Contains(text, "response.function_call_arguments.delta") || !strings.Contains(text, "response.completed") {
						t.Fatal(text)
					}
				} else {
					if !strings.Contains(text, "input_json_delta") || !strings.Contains(text, "message_stop") {
						t.Fatal(text)
					}
				}
			})
		}
	}
}

func TestConversionRejectsUnsupportedAndPreservesImages(t *testing.T) {
	for _, body := range []string{`{"model":"m","input":"hi","previous_response_id":"x"}`, `{"model":"m","input":"hi","store":true}`, `{"model":"m","input":"hi","tools":[{"type":"web_search"}]}`, `{"model":"m","input":[{"type":"reasoning","summary":[]}]}`} {
		if _, err := convertRequest([]byte(body), "responses"); err == nil {
			t.Errorf("silently accepted %s", body)
		}
	}
	out, err := convertRequest([]byte(`{"model":"m","input":[{"role":"user","content":[{"type":"input_image","image_url":"https://example.test/image.png","detail":"low"}]}],"text":{"format":{"type":"json_schema","name":"answer","schema":{"type":"object"},"strict":true}}}`), "responses")
	if err != nil {
		t.Fatal(err)
	}
	encoded, _ := json.Marshal(out)
	if !bytes.Contains(encoded, []byte(`"image_url":{"detail":"low","url":"https://example.test/image.png"}`)) || !bytes.Contains(encoded, []byte(`"json_schema"`)) {
		t.Fatal(string(encoded))
	}
}

func BenchmarkConvertRequest(b *testing.B) {
	for _, size := range []int{1024, 65536} {
		b.Run(fmt.Sprint(size), func(b *testing.B) {
			body, _ := json.Marshal(jsonObject{"model": "m", "input": strings.Repeat("x", size)})
			b.ReportAllocs()
			b.SetBytes(int64(len(body)))
			for i := 0; i < b.N; i++ {
				if _, err := convertRequest(body, "responses"); err != nil {
					b.Fatal(err)
				}
			}
		})
	}
}
