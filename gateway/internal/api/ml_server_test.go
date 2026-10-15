package api

import (
	"bytes"
	"encoding/json"
	"io"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ictrektech/v-router/internal/registry"
)

func TestMLServerAdapters(t *testing.T) {
	t.Setenv("V_ROUTER_USAGE_PATH", filepath.Join(t.TempDir(), "usage.jsonl"))
	t.Setenv("V_ROUTER_AUTH_DISABLED", "true")
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/predict":
			if err := r.ParseMultipartForm(1 << 20); err != nil {
				t.Errorf("parse multipart: %v", err)
			}
			entries := r.FormValue("entries")
			switch {
			case strings.Contains(entries, "qwen3-embedding"):
				_ = json.NewEncoder(w).Encode(map[string]string{"qwen3-embedding": "[0.25,0.75]"})
			case strings.Contains(entries, "ocr"):
				_ = json.NewEncoder(w).Encode(map[string]any{"ocr": map[string]any{"text": []string{"hello"}}, "imageHeight": 1, "imageWidth": 1})
			default:
				http.Error(w, "unexpected entries", 422)
			}
		case "/api/v1/video/structure":
			body, _ := io.ReadAll(r.Body)
			if bytes.Contains(body, []byte(`"model"`)) {
				t.Error("gateway model must not reach upstream video API")
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"code": 0, "data": map[string]string{"task_id": "task-1", "status": "queued"}})
		case "/api/v1/video/structure/tasks/task-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"code": 0, "data": map[string]string{"task_id": "task-1", "status": "running"}})
		default:
			http.NotFound(w, r)
		}
	}))
	defer up.Close()
	r := registry.New()
	r.Put(&registry.Upstream{ID: "app-ml-server", Name: "ML Server", Kind: registry.KindLocal, Source: registry.SourceMLServer, Protocol: registry.ProtocolMLServer, ModelPrefix: registry.SourceMLServer, BaseURL: up.URL, Healthy: true})
	r.SetModels("app-ml-server", []registry.Model{
		{ID: "qwen3-embedding-0.6b", Qualified: "ml-server/qwen3-embedding-0.6b", Upstream: "app-ml-server", Kind: registry.ModelKindEmbedding, Endpoints: []string{"embeddings"}},
		{ID: "PP-OCRv5_server", Qualified: "ml-server/PP-OCRv5_server", Upstream: "app-ml-server", Kind: "vision", Endpoints: []string{"ml/predictions"}},
		{ID: "video-structure", Qualified: "ml-server/video-structure", Upstream: "app-ml-server", Kind: "video", Endpoints: []string{"ml/video/structure", "ml/video/structure/tasks"}},
	})
	s := newServer(r, "test")
	s.allowLoopbackUpstreams = true
	srv := httptest.NewServer(s.routes())
	defer srv.Close()

	resp, body := doJSON(t, "POST", srv.URL+"/v1/embeddings", map[string]any{"model": "ml-server/qwen3-embedding-0.6b", "input": []string{"a", "b"}})
	if resp.StatusCode != 200 || !strings.Contains(body, `"embedding":[0.25,0.75]`) || strings.Contains(body, `"usage"`) {
		t.Fatalf("embeddings: %d %s", resp.StatusCode, body)
	}

	var form bytes.Buffer
	mw := multipart.NewWriter(&form)
	_ = mw.WriteField("model", "ml-server/PP-OCRv5_server")
	part, _ := mw.CreateFormFile("image", "x.png")
	_, _ = part.Write([]byte("image"))
	_ = mw.Close()
	req, _ := http.NewRequest(http.MethodPost, srv.URL+"/v1/ml/predictions", &form)
	req.Header.Set("Content-Type", mw.FormDataContentType())
	pred, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	predBody, _ := io.ReadAll(pred.Body)
	_ = pred.Body.Close()
	if pred.StatusCode != 200 || !bytes.Contains(predBody, []byte(`"hello"`)) {
		t.Fatalf("prediction: %d %s", pred.StatusCode, predBody)
	}

	resp, body = doJSON(t, "POST", srv.URL+"/v1/ml/video/structure", map[string]any{"model": "ml-server/video-structure", "asset_id": "asset", "video_path": "video/a.mp4"})
	if resp.StatusCode != 200 || !strings.Contains(body, "task-1") {
		t.Fatalf("video create: %d %s", resp.StatusCode, body)
	}
	get, err := http.Get(srv.URL + "/v1/ml/video/structure/tasks/task-1?model=ml-server%2Fvideo-structure")
	if err != nil {
		t.Fatal(err)
	}
	getBody, _ := io.ReadAll(get.Body)
	_ = get.Body.Close()
	if get.StatusCode != 200 || !bytes.Contains(getBody, []byte(`"running"`)) {
		t.Fatalf("video get: %d %s", get.StatusCode, getBody)
	}
}
