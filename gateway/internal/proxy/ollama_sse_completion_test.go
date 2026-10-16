package proxy

import (
	"context"
	"io"
	"strings"
	"testing"
)

func TestOllamaCompletionRequiresWholeTerminalEvent(t *testing.T) {
	for _, raw := range []string{`data: {"content":"data: [DONE]"}` + "\n\n", "data: [DONE]\n", "data: x\ndata: [DONE]\n\n", "data: " + strings.Repeat("x", 2048) + "\ndata: [DONE]\n\n"} {
		s := &ollamaSSECompletion{}
		if s.feed([]byte(raw)) {
			t.Fatalf("accepted nonterminal event %q", raw[:20])
		}
	}
	s := &ollamaSSECompletion{}
	for _, fragment := range []string{"data: {\"content\":\"hi\"}\n\n", "data:", " [DO", "NE]\r\n", "\r"} {
		if s.feed([]byte(fragment)) {
			t.Fatal("accepted unfinished terminal frame")
		}
	}
	if !s.feed([]byte("\n")) {
		t.Fatal("lost fragmented terminal event")
	}
}

func TestOllamaTerminalEventSurvivesClientCancelBeforeTransportEOF(t *testing.T) {
	for _, terminal := range []bool{false, true} {
		ctx, cancel := context.WithCancel(context.Background())
		raw := "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n"
		if terminal {
			raw += "data: [DONE]\n\n"
		}
		var stopped bool
		body := &permitBody{ReadCloser: io.NopCloser(strings.NewReader(raw)), ctx: ctx, cancel: cancel, remaining: -1, synchronous: true, ollamaCompletion: &ollamaSSECompletion{}, finish: func(value bool) { stopped = value }}
		buf := make([]byte, len(raw))
		if n, err := body.Read(buf); n != len(raw) || err != nil {
			t.Fatalf("read: %d %v", n, err)
		}
		cancel() // Client closes after reading the final frame, without reading EOF.
		_ = body.Close()
		if stopped != terminal {
			t.Fatalf("terminal=%v stopped=%v", terminal, stopped)
		}
	}
}
