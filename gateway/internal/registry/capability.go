package registry

import "strings"

func HasKnownCapability(caps []string) bool {
	for _, cap := range caps {
		switch strings.ToLower(strings.TrimSpace(cap)) {
		case "embedding", "embeddings", "chat", "completion", "completions", "generate", "generation", "tools", "thinking", "asr", "transcription", "transcriptions", "tts", "speech", "music", "text-to-music", "instrumental", "image", "images", "image_generation", "image_edit", "image_variation", "moderation", "moderations", "rerank", "reranking", "translation", "translations", "video", "videos", "video_generation":
			return true
		}
	}
	return false
}

func ModelKindFromCapabilities(caps []string, fallback string) string {
	chat, embedding := false, false
	for _, cap := range caps {
		switch strings.ToLower(strings.TrimSpace(cap)) {
		case "embedding", "embeddings":
			embedding = true
		case "chat", "completion", "completions", "generate", "generation", "tools", "thinking":
			chat = true
		}
	}
	switch {
	case embedding && !chat:
		return ModelKindEmbedding
	case chat && !embedding:
		return ModelKindChat
	case fallback != "":
		return fallback
	default:
		return "unknown"
	}
}

func NormalizeEndpoint(endpoint string) string {
	e := strings.ToLower(strings.TrimSpace(endpoint))
	e = strings.TrimPrefix(e, "/")
	e = strings.TrimPrefix(e, "v1/")
	e = strings.ReplaceAll(e, "-", "_")
	e = strings.ReplaceAll(e, " ", "_")
	switch e {
	case "chat", "chat_completions", "chat/completions":
		return "chat_completions"
	case "response", "responses":
		return "responses"
	case "message", "messages", "anthropic_messages", "anthropic/messages":
		return "messages"
	case "completion", "completions":
		return "completions"
	case "embedding", "embeddings":
		return "embeddings"
	case "image", "images", "images/generations", "image_generation":
		return "images/generations"
	case "images/edits", "image_edit":
		return "images/edits"
	case "images/variations", "image_variation":
		return "images/variations"
	case "video", "videos", "videos/generations", "video_generation":
		return "videos/generations"
	case "audio/speech", "speech", "tts":
		return "audio/speech"
	case "audio/transcriptions", "transcriptions", "asr":
		return "audio/transcriptions"
	case "audio/translations", "translations":
		return "audio/translations"
	case "audio/music/generations", "music", "music_generation", "text_to_music":
		return "audio/music/generations"
	case "moderation", "moderations":
		return "moderations"
	case "ml/predictions", "ml_prediction", "ml_predictions":
		return "ml/predictions"
	case "ml/video/structure", "ml_video_structure":
		return "ml/video/structure"
	case "ml/video/structure/tasks", "ml_video_structure_tasks":
		return "ml/video/structure/tasks"
	case "rerank", "reranking":
		return "rerank"
	}
	return e
}

func NormalizeEndpoints(endpoints []string) []string {
	seen := map[string]bool{}
	out := make([]string, 0, len(endpoints))
	for _, endpoint := range endpoints {
		normalized := NormalizeEndpoint(endpoint)
		if normalized == "" || seen[normalized] {
			continue
		}
		seen[normalized] = true
		out = append(out, normalized)
	}
	return out
}

func CapabilitiesFromEndpoints(endpoints []string) []string {
	seen := map[string]bool{}
	var out []string
	add := func(cap string) {
		if cap != "" && !seen[cap] {
			seen[cap] = true
			out = append(out, cap)
		}
	}
	for _, endpoint := range NormalizeEndpoints(endpoints) {
		switch endpoint {
		case "chat_completions", "responses", "messages", "completions":
			add("completion")
		case "embeddings":
			add("embedding")
		case "images/generations":
			add("image_generation")
		case "images/edits":
			add("image_edit")
		case "images/variations":
			add("image_variation")
		case "videos/generations":
			add("video_generation")
		case "audio/speech":
			add("tts")
		case "audio/transcriptions":
			add("asr")
		case "audio/translations":
			add("translation")
		case "audio/music/generations":
			add("text-to-music")
		case "moderations":
			add("moderation")
		case "rerank":
			add("rerank")
		case "ml/predictions":
			add("vision")
		case "ml/video/structure", "ml/video/structure/tasks":
			add("video_structure")
		}
	}
	return out
}

func MergeCapabilities(groups ...[]string) []string {
	seen := map[string]bool{}
	var out []string
	for _, group := range groups {
		for _, cap := range group {
			normalized := strings.ToLower(strings.TrimSpace(cap))
			if normalized == "" || seen[normalized] {
				continue
			}
			seen[normalized] = true
			out = append(out, normalized)
		}
	}
	return out
}
