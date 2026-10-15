package registry

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"strings"
	"time"
)

const imageStudioUpstreamID = "app-image-studio"

// ImageStudioPoller mirrors an installed same-VOS Image Studio app into the
// registry. It intentionally uses the app's native API instead of pretending it
// is OpenAI compatible; the API layer adapts image/video generation calls.
type ImageStudioPoller struct {
	reg      *Registry
	baseURL  string
	interval time.Duration
	client   *http.Client
}

type imageStudioModel struct {
	Key          string               `json:"key"`
	Name         string               `json:"name"`
	Capabilities []string             `json:"capabilities"`
	Status       string               `json:"status"`
	VRouter      imageStudioVRouter   `json:"v_router"`
	Variants     []imageStudioVariant `json:"variants"`
}

type imageStudioVariant struct {
	ID           string             `json:"id"`
	Key          string             `json:"key"`
	VariantID    string             `json:"variant_id"`
	Capabilities []string           `json:"capabilities"`
	Status       string             `json:"status"`
	VRouter      imageStudioVRouter `json:"v_router"`
}

type imageStudioVRouter struct {
	Model        string   `json:"model"`
	Qualified    string   `json:"qualified"`
	Status       string   `json:"status"`
	Endpoints    []string `json:"endpoints"`
	Capabilities []string `json:"capabilities"`
	ModelKind    string   `json:"model_kind"`
}

func NewImageStudioPoller(reg *Registry, baseURL string, interval time.Duration) *ImageStudioPoller {
	return &ImageStudioPoller{reg: reg, baseURL: strings.TrimRight(baseURL, "/"), interval: interval, client: &http.Client{Timeout: 10 * time.Second}}
}

func (p *ImageStudioPoller) Run(ctx context.Context) {
	ticker := time.NewTicker(p.interval)
	defer ticker.Stop()
	for {
		p.pollOnce(ctx)
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

func (p *ImageStudioPoller) pollOnce(ctx context.Context) {
	if p.baseURL == "" {
		return
	}
	if p.reg.HasNonModelHubBaseURL(p.baseURL, imageStudioUpstreamID) {
		p.reg.Delete(imageStudioUpstreamID)
		return
	}
	models, err := p.fetchModels(ctx)
	u := &Upstream{ID: imageStudioUpstreamID, Name: "Image Studio", Kind: KindLocal, Source: SourceImageStudio, BaseURL: p.baseURL, Protocol: ProtocolImageStudio, ModelPrefix: SourceImageStudio, Healthy: err == nil}
	if err != nil {
		u.LastError = err.Error()
	}
	p.reg.Put(u)
	if err == nil {
		p.reg.SetModels(u.ID, models)
	} else {
		log.Printf("image studio discovery failed: %v", err)
	}
}

func (p *ImageStudioPoller) fetchModels(ctx context.Context) ([]Model, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, imageStudioAPIBase(p.baseURL)+"/models", nil)
	if err != nil {
		return nil, err
	}
	resp, err := p.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("models probe: status %d", resp.StatusCode)
	}
	var rows []imageStudioModel
	if err := json.NewDecoder(resp.Body).Decode(&rows); err != nil {
		return nil, err
	}
	models := make([]Model, 0, len(rows))
	for _, row := range rows {
		id := strings.TrimSpace(row.Key)
		if id == "" {
			continue
		}
		modelID, status, kind, endpoints, caps := imageStudioModelRoute(id, row.Status, row.Capabilities, row.VRouter)
		if status == "ready" && len(endpoints) > 0 {
			models = append(models, Model{ID: modelID, Qualified: SourceImageStudio + "/" + modelID, Upstream: imageStudioUpstreamID, Kind: kind, Capabilities: caps, Endpoints: endpoints, OwnedBy: "image-studio"})
		}
		for _, variant := range row.Variants {
			variantID := variantID(variant)
			if variantID == "" {
				continue
			}
			vcaps := append([]string(nil), variant.Capabilities...)
			if len(vcaps) == 0 {
				vcaps = row.Capabilities
			}
			modelID, status, kind, endpoints, caps := imageStudioModelRoute(id+"@"+variantID, variant.Status, vcaps, variant.VRouter)
			if status == "" {
				status = row.Status
			}
			if status == "ready" && len(endpoints) > 0 {
				models = append(models, Model{ID: modelID, Qualified: SourceImageStudio + "/" + modelID, Upstream: imageStudioUpstreamID, Kind: kind, Capabilities: caps, Endpoints: endpoints, OwnedBy: "image-studio"})
			}
		}
	}
	return models, nil
}

func imageStudioModelRoute(fallbackID, fallbackStatus string, nativeCaps []string, meta imageStudioVRouter) (string, string, string, []string, []string) {
	modelID := strings.TrimSpace(meta.Model)
	if modelID == "" {
		modelID = fallbackID
	}
	status := strings.TrimSpace(meta.Status)
	if status == "" {
		status = strings.TrimSpace(fallbackStatus)
	}
	kind := strings.TrimSpace(meta.ModelKind)
	if kind == "" {
		kind = "unknown"
	}
	endpoints := NormalizeEndpoints(meta.Endpoints)
	caps := MergeCapabilities(meta.Capabilities)
	if len(endpoints) == 0 {
		endpoints, caps = ImageStudioCapabilities(nativeCaps)
	} else if len(caps) == 0 {
		caps = CapabilitiesFromEndpoints(endpoints)
	}
	if len(caps) == 0 {
		caps = CapabilitiesFromEndpoints(endpoints)
	}
	return modelID, status, kind, endpoints, caps
}

func variantID(v imageStudioVariant) string {
	for _, candidate := range []string{v.ID, v.VariantID} {
		if strings.TrimSpace(candidate) != "" && strings.TrimSpace(candidate) != strings.TrimSpace(v.Key) {
			return strings.TrimSpace(candidate)
		}
	}
	return ""
}

func imageStudioAPIBase(base string) string {
	base = strings.TrimRight(strings.TrimSpace(base), "/")
	if strings.HasSuffix(base, "/api/v1") {
		return base
	}
	return base + "/api/v1"
}

// ImageStudioCapabilities maps Image Studio's native capability labels to
// V-Router model-routed endpoints.
func ImageStudioCapabilities(in []string) ([]string, []string) {
	endpointSeen := map[string]bool{}
	capSeen := map[string]bool{}
	var endpoints []string
	var caps []string
	addEndpoint := func(v string) {
		v = NormalizeEndpoint(v)
		if v != "" && !endpointSeen[v] {
			endpointSeen[v] = true
			endpoints = append(endpoints, v)
		}
	}
	addCap := func(v string) {
		v = strings.ToLower(strings.TrimSpace(v))
		if v != "" && !capSeen[v] {
			capSeen[v] = true
			caps = append(caps, v)
		}
	}
	for _, raw := range in {
		switch strings.ToLower(strings.TrimSpace(raw)) {
		case "txt2img", "text2image", "text_to_image", "image_generation":
			addEndpoint("images/generations")
			addCap("image_generation")
		case "img2img", "image2image", "image_to_image", "edit", "image_edit":
			addEndpoint("images/edits")
			addCap("image_edit")
		case "video", "txt2video", "text2video", "text_to_video", "img2video", "image2video", "image_to_video", "video_generation":
			addEndpoint("videos/generations")
			addCap("video_generation")
		}
	}
	return endpoints, caps
}
