package proxy

import "strings"

// A bounded SSE event recognizer. A marker inside JSON, a partial event or
// multiple data lines is not evidence that built-in Ollama has finished.
type ollamaSSECompletion struct {
	line      []byte
	overflow  bool
	dataLines int
	marker    bool
	invalid   bool
	done      bool
}

func (s *ollamaSSECompletion) feed(data []byte) bool {
	for _, ch := range data {
		if ch != '\n' {
			if len(s.line) < 1024 {
				s.line = append(s.line, ch)
			} else {
				s.overflow = true
			}
			continue
		}
		line := strings.TrimSuffix(string(s.line), "\r")
		if s.overflow {
			s.invalid = true
		}
		if line == "" && !s.overflow {
			if !s.invalid && s.dataLines == 1 && s.marker {
				s.done = true
			}
			s.dataLines, s.marker, s.invalid = 0, false, false
		} else if strings.HasPrefix(line, "data:") && !s.overflow {
			s.dataLines++
			s.marker = strings.TrimSpace(strings.TrimPrefix(line, "data:")) == "[DONE]"
		}
		s.line = s.line[:0]
		s.overflow = false
	}
	return s.done
}
