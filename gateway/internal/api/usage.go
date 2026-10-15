package api

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

type usageRecord struct {
	Time            time.Time `json:"time"`
	User            string    `json:"user"`
	Model           string    `json:"model"`
	NativeID        string    `json:"native_id"`
	UpstreamID      string    `json:"upstream_id"`
	UpstreamName    string    `json:"upstream_name"`
	Endpoint        string    `json:"endpoint"`
	Status          int       `json:"status"`
	LatencyMS       int64     `json:"latency_ms"`
	Stream          bool      `json:"stream"`
	UsageReported   bool      `json:"usage_reported,omitempty"`
	InputTokens     int64     `json:"input_tokens,omitempty"`
	OutputTokens    int64     `json:"output_tokens,omitempty"`
	TotalTokens     int64     `json:"total_tokens,omitempty"`
	CachedTokens    int64     `json:"cached_tokens,omitempty"`
	ReasoningTokens int64     `json:"reasoning_tokens,omitempty"`
}

type tokenUsage struct{ Reported, Input, Output, Total, Cached, Reasoning int64 }

type modelStats struct {
	Requests, Errors, InputTokens, OutputTokens, TotalTokens int64
	CachedTokens, ReasoningTokens, UsageRequests             int64
	LatencySum, LatencySumN                                  int64
	Name                                                     string
}

func (m *modelStats) avgLatencyMS() int64 {
	if m.LatencySumN == 0 {
		return 0
	}
	return m.LatencySum / m.LatencySumN
}

type statsView struct {
	Requests        int64  `json:"requests"`
	Errors          int64  `json:"errors"`
	LatencyMSAvg    int64  `json:"latency_ms_avg"`
	InputTokens     int64  `json:"input_tokens"`
	OutputTokens    int64  `json:"output_tokens"`
	TotalTokens     int64  `json:"total_tokens"`
	CachedTokens    int64  `json:"cached_tokens"`
	ReasoningTokens int64  `json:"reasoning_tokens"`
	UsageRequests   int64  `json:"usage_requests"`
	Name            string `json:"name,omitempty"`
}

type usageAggregate struct {
	totals      modelStats
	perModel    map[string]*modelStats
	perUpstream map[string]*modelStats
	perUser     map[string]*modelStats
}

func newUsageAggregate() *usageAggregate {
	return &usageAggregate{perModel: map[string]*modelStats{}, perUpstream: map[string]*modelStats{}, perUser: map[string]*modelStats{}}
}

func (a *usageAggregate) add(r usageRecord) {
	add := func(m *modelStats) {
		m.Requests++
		if r.Status >= 400 {
			m.Errors++
		}
		if r.LatencyMS >= 0 {
			m.LatencySum += r.LatencyMS
			m.LatencySumN++
		}
		if r.UsageReported {
			m.UsageRequests++
			m.InputTokens += r.InputTokens
			m.OutputTokens += r.OutputTokens
			m.TotalTokens += r.TotalTokens
			m.CachedTokens += r.CachedTokens
			m.ReasoningTokens += r.ReasoningTokens
		}
	}
	add(&a.totals)
	modelKey := r.UpstreamID + "/" + r.NativeID
	if a.perModel[modelKey] == nil {
		a.perModel[modelKey] = &modelStats{}
	}
	add(a.perModel[modelKey])
	if a.perUpstream[r.UpstreamID] == nil {
		a.perUpstream[r.UpstreamID] = &modelStats{Name: r.UpstreamName}
	}
	if a.perUpstream[r.UpstreamID].Name == "" {
		a.perUpstream[r.UpstreamID].Name = r.UpstreamName
	}
	add(a.perUpstream[r.UpstreamID])
	if a.perUser[r.User] == nil {
		a.perUser[r.User] = &modelStats{}
	}
	add(a.perUser[r.User])
}

type usageLog struct {
	mu      sync.Mutex
	userDir string
	files   map[string]*os.File
	since   time.Time
	buckets map[string]*usageAggregate
	recent  []usageRecord
}

const recentLimit = 200

func newUsageLog(path string) *usageLog {
	u := &usageLog{since: time.Now(), buckets: map[string]*usageAggregate{"all": newUsageAggregate()}, files: map[string]*os.File{}}
	if path == "" {
		return u
	}
	root := strings.TrimSuffix(path, filepath.Ext(path))
	if root == path {
		root += ".d"
	}
	u.userDir = filepath.Join(root, "users")
	if err := prepareUserUsageDir(path, u.userDir); err != nil {
		u.loadFiles([]string{path})
		return u
	}
	entries, _ := filepath.Glob(filepath.Join(u.userDir, "*.jsonl"))
	u.loadFiles(entries)
	return u
}

func (u *usageLog) loadFiles(paths []string) {
	var records []usageRecord
	for _, path := range paths {
		f, err := os.Open(path)
		if err != nil {
			continue
		}
		scanner := bufio.NewScanner(f)
		scanner.Buffer(make([]byte, 64<<10), 1<<20)
		for scanner.Scan() {
			var r usageRecord
			if json.Unmarshal(scanner.Bytes(), &r) == nil && !r.Time.IsZero() {
				records = append(records, r)
			}
		}
		_ = f.Close()
	}
	sort.Slice(records, func(i, j int) bool { return records[i].Time.Before(records[j].Time) })
	for _, r := range records {
		u.add(r)
		if r.Time.Before(u.since) {
			u.since = r.Time
		}
	}
}

func prepareUserUsageDir(legacyPath, userDir string) error {
	if _, err := os.Stat(userDir); err == nil {
		return nil
	} else if !os.IsNotExist(err) {
		return err
	}
	if _, err := os.Stat(legacyPath); err == nil {
		root := filepath.Dir(userDir)
		if err := os.MkdirAll(root, 0o700); err != nil {
			return err
		}
		stage, err := os.MkdirTemp(root, ".users-migrating-")
		if err != nil {
			return err
		}
		ok := false
		defer func() {
			if !ok {
				_ = os.RemoveAll(stage)
			}
		}()
		if err := splitLegacyUsage(legacyPath, stage); err != nil {
			return err
		}
		if err := os.Rename(stage, userDir); err != nil {
			return err
		}
		ok = true
		_ = os.Rename(legacyPath, filepath.Join(root, "legacy.jsonl"))
		return nil
	} else if !os.IsNotExist(err) {
		return err
	}
	return os.MkdirAll(userDir, 0o700)
}

func splitLegacyUsage(legacyPath, userDir string) error {
	f, err := os.Open(legacyPath)
	if err != nil {
		return err
	}
	defer f.Close()
	files := map[string]*os.File{}
	defer func() {
		for _, out := range files {
			_ = out.Close()
		}
	}()
	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 64<<10), 1<<20)
	for scanner.Scan() {
		var r usageRecord
		if json.Unmarshal(scanner.Bytes(), &r) != nil || r.Time.IsZero() {
			continue
		}
		name := usageUserFileName(r.User)
		out := files[name]
		if out == nil {
			out, err = os.OpenFile(filepath.Join(userDir, name), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
			if err != nil {
				return err
			}
			files[name] = out
		}
		if _, err = out.Write(append(append([]byte(nil), scanner.Bytes()...), '\n')); err != nil {
			return err
		}
	}
	return scanner.Err()
}

func usageUserFileName(user string) string {
	label := strings.ToLower(strings.TrimSpace(user))
	if label == "" {
		label = "unknown"
	}
	var safe strings.Builder
	for _, r := range label {
		if (r >= 'a' && r <= 'z') || (r >= '0' && r <= '9') || r == '-' || r == '_' || r == '.' {
			safe.WriteRune(r)
		} else {
			safe.WriteByte('_')
		}
		if safe.Len() >= 48 {
			break
		}
	}
	sum := sha256.Sum256([]byte(user))
	prefix := strings.Trim(safe.String(), ".")
	if prefix == "" {
		prefix = "user"
	}
	return fmt.Sprintf("%s-%x.jsonl", prefix, sum[:6])
}

func (u *usageLog) fileForUser(user string) *os.File {
	if u.userDir == "" {
		return nil
	}
	name := usageUserFileName(user)
	if u.files[name] == nil {
		f, err := os.OpenFile(filepath.Join(u.userDir, name), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
		if err != nil {
			return nil
		}
		u.files[name] = f
	}
	return u.files[name]
}

func (u *usageLog) close() {
	for _, f := range u.files {
		_ = f.Close()
	}
}

func periodStart(t time.Time, period string) time.Time {
	t = t.Local()
	day := time.Date(t.Year(), t.Month(), t.Day(), 0, 0, 0, 0, t.Location())
	switch period {
	case "week":
		return day.AddDate(0, 0, -(int(day.Weekday())+6)%7)
	case "month":
		return time.Date(t.Year(), t.Month(), 1, 0, 0, 0, 0, t.Location())
	default:
		return day
	}
}

func periodKey(t time.Time, period string) string {
	return period + ":" + periodStart(t, period).Format("2006-01-02")
}

func (u *usageLog) add(r usageRecord) {
	for _, key := range []string{"all", periodKey(r.Time, "day"), periodKey(r.Time, "week"), periodKey(r.Time, "month")} {
		if u.buckets[key] == nil {
			u.buckets[key] = newUsageAggregate()
		}
		u.buckets[key].add(r)
	}
	u.recent = append(u.recent, r)
	if len(u.recent) > recentLimit {
		u.recent = u.recent[len(u.recent)-recentLimit:]
	}
}

func (u *usageLog) record(r usageRecord) {
	if u == nil {
		return
	}
	u.mu.Lock()
	defer u.mu.Unlock()
	if r.Time.IsZero() {
		r.Time = time.Now()
	}
	u.add(r)
	if f := u.fileForUser(r.User); f != nil {
		if line, err := json.Marshal(r); err == nil {
			_, _ = f.Write(append(line, '\n'))
		}
	}
}

type usageSnapshot struct {
	Period      string               `json:"period"`
	Since       string               `json:"since"`
	Totals      statsView            `json:"totals"`
	PerModel    map[string]statsView `json:"per_model"`
	PerUpstream map[string]statsView `json:"per_upstream"`
	PerUser     map[string]statsView `json:"per_user"`
	Recent      []usageRecord        `json:"recent"`
}

func stats(m *modelStats) statsView {
	if m == nil {
		return statsView{}
	}
	return statsView{Requests: m.Requests, Errors: m.Errors, LatencyMSAvg: m.avgLatencyMS(), InputTokens: m.InputTokens, OutputTokens: m.OutputTokens, TotalTokens: m.TotalTokens, CachedTokens: m.CachedTokens, ReasoningTokens: m.ReasoningTokens, UsageRequests: m.UsageRequests, Name: m.Name}
}

func statsMap(in map[string]*modelStats) map[string]statsView {
	out := make(map[string]statsView, len(in))
	for key, value := range in {
		out[key] = stats(value)
	}
	return out
}

func (u *usageLog) snapshot(period string) usageSnapshot {
	if period != "day" && period != "week" && period != "month" && period != "all" {
		period = "day"
	}
	if u == nil {
		return usageSnapshot{Period: period, PerModel: map[string]statsView{}, PerUpstream: map[string]statsView{}, PerUser: map[string]statsView{}, Recent: []usageRecord{}}
	}
	u.mu.Lock()
	defer u.mu.Unlock()
	key, since := "all", u.since
	if period != "all" {
		since, key = periodStart(time.Now(), period), periodKey(time.Now(), period)
	}
	a := u.buckets[key]
	if a == nil {
		a = newUsageAggregate()
	}
	out := usageSnapshot{Period: period, Since: since.Format(time.RFC3339), Totals: stats(&a.totals), PerModel: statsMap(a.perModel), PerUpstream: statsMap(a.perUpstream), PerUser: statsMap(a.perUser)}
	for i := len(u.recent) - 1; i >= 0; i-- {
		if period == "all" || !u.recent[i].Time.Before(since) {
			out.Recent = append(out.Recent, u.recent[i])
		}
	}
	return out
}

// Keep only the response tail: usage is at the end of OpenAI JSON responses
// and in terminal SSE events. Observation does not delay proxy streaming.
type usageObserver struct{ tail []byte }

const usageTailLimit = 256 << 10

func (o *usageObserver) Write(p []byte) {
	o.tail = append(o.tail, p...)
	if len(o.tail) > usageTailLimit {
		o.tail = append([]byte(nil), o.tail[len(o.tail)-usageTailLimit:]...)
	}
}

func (o *usageObserver) usage() tokenUsage {
	var best tokenUsage
	consider := func(raw []byte) {
		var doc map[string]any
		if json.Unmarshal(raw, &doc) == nil {
			if got := usageFromObject(doc); got.Reported > 0 {
				best = mergeTokenUsage(best, got)
			}
		}
	}
	consider(o.tail)
	for _, line := range bytes.Split(o.tail, []byte{'\n'}) {
		line = bytes.TrimSpace(line)
		if bytes.HasPrefix(line, []byte("data:")) {
			consider(bytes.TrimSpace(bytes.TrimPrefix(line, []byte("data:"))))
		}
	}
	for offset := 0; ; {
		i := bytes.Index(o.tail[offset:], []byte(`"usage"`))
		if i < 0 {
			break
		}
		offset += i + len(`"usage"`)
		rest := bytes.TrimSpace(o.tail[offset:])
		if len(rest) == 0 || rest[0] != ':' {
			continue
		}
		var value map[string]any
		if json.NewDecoder(bytes.NewReader(rest[1:])).Decode(&value) == nil {
			if got := usageFields(value); got.Reported > 0 {
				best = mergeTokenUsage(best, got)
			}
		}
	}
	return best
}

func usageFromObject(doc map[string]any) tokenUsage {
	if value, ok := doc["usage"].(map[string]any); ok {
		return usageFields(value)
	}
	for _, key := range []string{"response", "message"} {
		if nested, ok := doc[key].(map[string]any); ok {
			if value, ok := nested["usage"].(map[string]any); ok {
				return usageFields(value)
			}
		}
	}
	if _, ok := doc["prompt_eval_count"]; ok {
		return usageFields(doc)
	}
	return tokenUsage{}
}

func usageFields(value map[string]any) tokenUsage {
	num := func(keys ...string) int64 {
		for _, key := range keys {
			if n := number(value[key]); n != 0 {
				return n
			}
			if _, ok := value[key]; ok {
				return 0
			}
		}
		return 0
	}
	input := num("prompt_tokens", "input_tokens", "prompt_eval_count")
	output := num("completion_tokens", "output_tokens", "eval_count")
	cacheRead := num("cache_read_input_tokens")
	cacheCreation := num("cache_creation_input_tokens")
	// Anthropic reports uncached, cache-read and cache-creation input tokens as
	// separate top-level fields. Normalize Input to their billable sum; OpenAI
	// prompt_tokens already includes cached tokens and needs no adjustment.
	if _, anthropic := value["input_tokens"]; anthropic {
		input += cacheRead + cacheCreation
	}
	total := num("total_tokens")
	if total == 0 {
		total = input + output
	}
	cached, reasoning := cacheRead+cacheCreation, int64(0)
	for _, key := range []string{"prompt_tokens_details", "input_tokens_details"} {
		if details, ok := value[key].(map[string]any); ok {
			cached += number(details["cached_tokens"])
		}
	}
	for _, key := range []string{"completion_tokens_details", "output_tokens_details"} {
		if details, ok := value[key].(map[string]any); ok {
			reasoning = number(details["reasoning_tokens"])
		}
	}
	has := false
	for _, key := range []string{"prompt_tokens", "input_tokens", "prompt_eval_count", "completion_tokens", "output_tokens", "eval_count", "total_tokens"} {
		if _, ok := value[key]; ok {
			has = true
			break
		}
	}
	if !has {
		return tokenUsage{}
	}
	return tokenUsage{Reported: 1, Input: input, Output: output, Total: total, Cached: cached, Reasoning: reasoning}
}

func mergeTokenUsage(current, next tokenUsage) tokenUsage {
	if current.Reported == 0 {
		return next
	}
	// Anthropic streaming splits usage across message_start (input/cache) and
	// message_delta (cumulative output). Preserve fields omitted by later events.
	if next.Input == 0 {
		next.Input = current.Input
	}
	if next.Output == 0 {
		next.Output = current.Output
	}
	if next.Cached == 0 {
		next.Cached = current.Cached
	}
	if next.Reasoning == 0 {
		next.Reasoning = current.Reasoning
	}
	next.Total = next.Input + next.Output
	next.Reported = 1
	return next
}

func number(value any) int64 {
	switch v := value.(type) {
	case float64:
		return int64(v)
	case json.Number:
		n, _ := v.Int64()
		return n
	}
	return 0
}

func normalizePeriod(value string) string { return strings.ToLower(strings.TrimSpace(value)) }
