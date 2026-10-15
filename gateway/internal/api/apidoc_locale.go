package api

import (
	_ "embed"
	"encoding/json"
	"net/http"
	"sort"
	"strings"
)

// Keep translations separate from routing so localization cannot alter handlers.
//go:embed apidoc_en.json
var apiDocEnglishJSON []byte

var apiDocEnglish = func() *strings.Replacer {
	var catalog map[string]string
	if err := json.Unmarshal(apiDocEnglishJSON, &catalog); err != nil {
		panic(err)
	}
	keys := make([]string, 0, len(catalog))
	for key := range catalog {
		keys = append(keys, key)
	}
	// Full descriptions take precedence over shared phrases and sample text.
	sort.Slice(keys, func(i, j int) bool { return len(keys[i]) > len(keys[j]) })
	pairs := make([]string, 0, len(keys)*2)
	for _, key := range keys {
		pairs = append(pairs, key, catalog[key])
	}
	return strings.NewReplacer(pairs...)
}()

func writeDocumentation(w http.ResponseWriter, r *http.Request, doc any) {
	locale := "zh-CN"
	lang := strings.ToLower(r.URL.Query().Get("lang"))
	if lang == "en" || strings.HasPrefix(lang, "en-") {
		locale = "en"
		// Walk values rather than serialized JSON, preserving escaping and keys.
		data, err := json.Marshal(doc)
		if err != nil {
			writeError(w, http.StatusInternalServerError, "cannot encode documentation")
			return
		}
		var value any
		if err := json.Unmarshal(data, &value); err != nil {
			writeError(w, http.StatusInternalServerError, "cannot decode documentation")
			return
		}
		doc = translateDocumentation(value)
	}
	w.Header().Set("Content-Language", locale)
	w.Header().Set("Cache-Control", "no-store")
	writeJSON(w, http.StatusOK, doc)
}

func translateDocumentation(value any) any {
	switch v := value.(type) {
	case string:
		return apiDocEnglish.Replace(v)
	case []any:
		for i := range v {
			v[i] = translateDocumentation(v[i])
		}
	case map[string]any:
		for key := range v {
			v[key] = translateDocumentation(v[key])
		}
	}
	return value
}
