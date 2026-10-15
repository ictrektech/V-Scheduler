package api

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// Opt-in loopback benchmark, excluding model inference. Run without -race.
// JSON lines report complete HTTP roundtrips, including client and mock costs.
func TestProtocolPerformance(t *testing.T) {
	if os.Getenv("V_ROUTER_PERF") != "1" {
		t.Skip("set V_ROUTER_PERF=1 for loopback benchmark")
	}
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body jsonObject
		_ = json.NewDecoder(r.Body).Decode(&body)
		if body["stream"] != true {
			w.Header().Set("Content-Type", "application/json")
			_, _ = io.WriteString(w, chatTextResult)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		for i := 0; i < 8; i++ {
			_, _ = io.WriteString(w, "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"token\"}}]}\n\n")
			w.(http.Flusher).Flush()
		}
		_, _ = io.WriteString(w, "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
	}))
	defer up.Close()
	gateway := httptest.NewServer(testBridgeGateway(up.URL))
	defer gateway.Close()
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.MaxIdleConnsPerHost = 64
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 10 * time.Second}
	for _, size := range []int{1024, 65536} {
		for _, stream := range []bool{false, true} {
			for _, concurrency := range []int{1, 32} {
				for _, protocol := range []string{"chat/completions", "responses", "messages"} {
					input := strings.Repeat("x", size)
					body := jsonObject{"model": "local/native", "stream": stream}
					if protocol == "responses" {
						body["input"] = input
						body["store"] = false
					} else {
						body["messages"] = []any{jsonObject{"role": "user", "content": input}}
						if protocol == "messages" {
							body["max_tokens"] = 32
						}
					}
					data, _ := json.Marshal(body)
					request := func() (float64, float64, int64, error) {
						start := time.Now()
						resp, err := client.Post(gateway.URL+"/v1/"+protocol, "application/json", bytes.NewReader(data))
						if err != nil {
							return 0, 0, 0, err
						}
						defer resp.Body.Close()
						if resp.StatusCode != 200 {
							return 0, 0, 0, fmt.Errorf("status %d", resp.StatusCode)
						}
						var first float64
						var n int64
						if stream {
							scanner := bufio.NewScanner(resp.Body)
							scanner.Buffer(make([]byte, 4096), 1<<20)
							for scanner.Scan() {
								line := scanner.Bytes()
								n += int64(len(line))
								if first == 0 && bytes.Contains(line, []byte(`"token"`)) {
									first = float64(time.Since(start).Microseconds()) / 1000
								}
							}
							if err := scanner.Err(); err != nil {
								return 0, 0, 0, err
							}
						} else {
							n, err = io.Copy(io.Discard, resp.Body)
						}
						return float64(time.Since(start).Microseconds()) / 1000, first, n, err
					}
					for i := 0; i < 64; i++ {
						if _, _, _, err := request(); err != nil {
							t.Fatal(err)
						}
					}
					const count = 1000
					latencies := make([]float64, count)
					firsts := make([]float64, count)
					var next atomic.Int64
					var bytesTotal atomic.Int64
					var failures atomic.Int64
					var group sync.WaitGroup
					start := time.Now()
					for worker := 0; worker < concurrency; worker++ {
						group.Add(1)
						go func() {
							defer group.Done()
							for {
								i := int(next.Add(1)) - 1
								if i >= count {
									return
								}
								latency, first, n, err := request()
								if err != nil {
									failures.Add(1)
								}
								latencies[i], firsts[i] = latency, first
								bytesTotal.Add(n)
							}
						}()
					}
					group.Wait()
					elapsed := time.Since(start).Seconds()
					if failures.Load() != 0 {
						t.Fatal("benchmark request failed")
					}
					sort.Float64s(latencies)
					sort.Float64s(firsts)
					row := jsonObject{"protocol": protocol, "input_bytes": size, "stream": stream, "concurrency": concurrency, "requests": count, "rps": float64(count) / elapsed, "p50_ms": latencies[count/2], "p95_ms": latencies[count*95/100], "first_text_p95_ms": firsts[count*95/100], "response_bytes": bytesTotal.Load() / count}
					encoded, _ := json.Marshal(row)
					t.Log(string(encoded))
				}
			}
		}
	}
}
