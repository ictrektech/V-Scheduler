package api

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

var probeClient = &http.Client{Timeout: 10 * time.Second}

// fetchUpstreamModels reads GET <base>/models from an OpenAI-compatible
// upstream and returns its model entries.
func fetchUpstreamModels(u registry.Upstream) ([]registry.Model, error) {
	if u.Protocol == registry.ProtocolImageStudio {
		return fetchImageStudioModels(u)
	}

	models := make([]registry.Model, 0)
	next := u.BaseURL + "/models"
	if u.Protocol == "anthropic" {
		next += "?limit=100"
	}
	for page := 0; page < 100; page++ {
		req, err := http.NewRequest(http.MethodGet, next, nil)
		if err != nil {
			return nil, err
		}
		if u.APIKey != "" {
			req.Header.Set("Authorization", "Bearer "+u.APIKey)
		}
		if u.Protocol == "anthropic" {
			req.Header.Del("Authorization")
			req.Header.Set("X-Api-Key", u.APIKey)
			req.Header.Set("Anthropic-Version", "2023-06-01")
		}
		resp, err := probeClient.Do(req)
		if err != nil {
			return nil, err
		}
		body, readErr := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
		resp.Body.Close()
		if readErr != nil {
			return nil, readErr
		}
		if resp.StatusCode != http.StatusOK {
			return nil, fmt.Errorf("models probe: status %d", resp.StatusCode)
		}
		var out struct {
			Data []struct {
				ID           string   `json:"id"`
				OwnedBy      string   `json:"owned_by"`
				Capabilities []string `json:"capabilities"`
				Endpoints    []string `json:"endpoints"`
			} `json:"data"`
			HasMore bool   `json:"has_more"`
			LastID  string `json:"last_id"`
		}
		if err := json.Unmarshal(body, &out); err != nil {
			return nil, fmt.Errorf("decode models response: %w", err)
		}
		for _, m := range out.Data {
			models = append(models, registry.Model{ID: m.ID, OwnedBy: m.OwnedBy, Capabilities: append([]string(nil), m.Capabilities...), Endpoints: registry.NormalizeEndpoints(m.Endpoints)})
		}
		if u.Protocol != "anthropic" || !out.HasMore {
			return models, nil
		}
		if len(out.Data) == 0 || out.LastID == "" {
			return nil, fmt.Errorf("invalid Anthropic models pagination")
		}
		candidate := u.BaseURL + "/models?limit=100&after_id=" + url.QueryEscape(out.LastID)
		if candidate == next {
			return nil, fmt.Errorf("Anthropic models pagination did not advance")
		}
		next = candidate
	}
	return nil, fmt.Errorf("Anthropic models pagination exceeded 100 pages")
}

func fetchImageStudioModels(u registry.Upstream) ([]registry.Model, error) {
	req, err := http.NewRequest(http.MethodGet, imageStudioAPIBase(u.BaseURL)+"/models", nil)
	if err != nil {
		return nil, err
	}
	if u.APIKey != "" {
		req.Header.Set("Authorization", "Bearer "+u.APIKey)
	}
	resp, err := probeClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("models probe: status %d", resp.StatusCode)
	}
	var rows []struct {
		Key          string   `json:"key"`
		Capabilities []string `json:"capabilities"`
		Status       string   `json:"status"`
		VRouter      struct {
			Model        string   `json:"model"`
			Status       string   `json:"status"`
			Endpoints    []string `json:"endpoints"`
			Capabilities []string `json:"capabilities"`
			ModelKind    string   `json:"model_kind"`
		} `json:"v_router"`
		Variants []struct {
			ID           string   `json:"id"`
			Key          string   `json:"key"`
			VariantID    string   `json:"variant_id"`
			Capabilities []string `json:"capabilities"`
			Status       string   `json:"status"`
			VRouter      struct {
				Model        string   `json:"model"`
				Status       string   `json:"status"`
				Endpoints    []string `json:"endpoints"`
				Capabilities []string `json:"capabilities"`
				ModelKind    string   `json:"model_kind"`
			} `json:"v_router"`
		} `json:"variants"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(&rows); err != nil {
		return nil, err
	}
	models := make([]registry.Model, 0, len(rows))
	for _, row := range rows {
		id := strings.TrimSpace(row.Key)
		if id == "" {
			continue
		}
		modelID, status, endpoints, caps := imageStudioProbeRoute(id, row.Status, row.Capabilities, row.VRouter.Model, row.VRouter.Status, row.VRouter.Endpoints, row.VRouter.Capabilities)
		if status == "ready" && len(endpoints) > 0 {
			models = append(models, registry.Model{ID: modelID, OwnedBy: registry.SourceImageStudio, Kind: row.VRouter.ModelKind, Capabilities: caps, Endpoints: endpoints})
		}
		for _, variant := range row.Variants {
			variantID := strings.TrimSpace(variant.ID)
			if variantID == "" {
				variantID = strings.TrimSpace(variant.VariantID)
			}
			if variantID == "" || variantID == id {
				continue
			}
			vcaps := variant.Capabilities
			if len(vcaps) == 0 {
				vcaps = row.Capabilities
			}
			modelID, status, endpoints, caps := imageStudioProbeRoute(id+"@"+variantID, variant.Status, vcaps, variant.VRouter.Model, variant.VRouter.Status, variant.VRouter.Endpoints, variant.VRouter.Capabilities)
			if status == "" {
				status = row.Status
			}
			if status == "ready" && len(endpoints) > 0 {
				models = append(models, registry.Model{ID: modelID, OwnedBy: registry.SourceImageStudio, Kind: variant.VRouter.ModelKind, Capabilities: caps, Endpoints: endpoints})
			}
		}
	}
	return models, nil
}

func imageStudioProbeRoute(fallbackID, fallbackStatus string, nativeCaps []string, metaModel, metaStatus string, metaEndpoints, metaCaps []string) (string, string, []string, []string) {
	modelID := strings.TrimSpace(metaModel)
	if modelID == "" {
		modelID = fallbackID
	}
	status := strings.TrimSpace(metaStatus)
	if status == "" {
		status = strings.TrimSpace(fallbackStatus)
	}
	endpoints := registry.NormalizeEndpoints(metaEndpoints)
	caps := registry.MergeCapabilities(metaCaps)
	if len(endpoints) == 0 {
		endpoints, caps = registry.ImageStudioCapabilities(nativeCaps)
	} else if len(caps) == 0 {
		caps = registry.CapabilitiesFromEndpoints(endpoints)
	}
	if len(caps) == 0 {
		caps = registry.CapabilitiesFromEndpoints(endpoints)
	}
	return modelID, status, endpoints, caps
}
