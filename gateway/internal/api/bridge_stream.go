package api

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
)

type eventWriter struct {
	buffer     *bufio.Writer
	controller *http.ResponseController
	sequence   int
	protocol   string
	err        error
	failed     bool
}

func (e *eventWriter) emit(kind string, data jsonObject) {
	if e.err != nil {
		return
	}
	data["type"] = kind
	if e.protocol == "responses" {
		data["sequence_number"] = e.sequence
		e.sequence++
	}
	encoded, err := json.Marshal(data)
	if err != nil {
		e.err = err
		return
	}
	_, e.err = fmt.Fprintf(e.buffer, "event: %s\ndata: %s\n\n", kind, encoded)
}

func (e *eventWriter) flush() bool {
	if e.err == nil {
		e.err = e.buffer.Flush()
	}
	if e.err == nil {
		e.err = e.controller.Flush()
	}
	return e.err == nil
}

func (e *eventWriter) fail() {
	e.failed = true
	if e.protocol == "messages" {
		e.emit("error", jsonObject{"error": jsonObject{"type": "api_error", "message": "upstream stream failed"}})
	} else {
		e.emit("error", jsonObject{"code": "upstream_stream_error", "message": "upstream stream failed", "param": nil})
	}
	e.flush()
}

func streamChat(w http.ResponseWriter, r *http.Request, reader io.Reader, protocol, model string, request ...jsonObject) {
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("X-Accel-Buffering", "no")
	e := &eventWriter{buffer: bufio.NewWriterSize(w, 16<<10), controller: http.NewResponseController(w), protocol: protocol}
	s := newOutputState(protocol, model, request...)
	if protocol == "messages" {
		message := s.result()
		message["stop_reason"] = nil
		e.emit("message_start", jsonObject{"message": message})
	} else {
		response := s.result()
		response["status"] = "in_progress"
		response["usage"] = nil
		e.emit("response.created", jsonObject{"response": response})
		e.emit("response.in_progress", jsonObject{"response": response})
	}
	if !e.flush() {
		return
	}
	scanner := bufio.NewScanner(reader)
	scanner.Buffer(make([]byte, 16<<10), maxInferenceBody)
	var data strings.Builder
	ended := false
	process := func() bool {
		payload := data.String()
		data.Reset()
		if payload == "" {
			return true
		}
		if payload == "[DONE]" {
			ended = true
			return false
		}
		var chunk chatResult
		if json.Unmarshal([]byte(payload), &chunk) != nil || len(chunk.Error) > 0 {
			e.fail()
			return false
		}
		if chunk.Usage != nil {
			s.usage = *chunk.Usage
		}
		if len(chunk.Choices) > 1 {
			e.fail()
			return false
		}
		for _, choice := range chunk.Choices {
			if choice.Index != 0 || choice.Delta.Refusal != nil {
				e.fail()
				return false
			}
			if choice.Delta.Content != nil && *choice.Delta.Content != "" {
				if s.textIndex < 0 {
					s.textIndex = len(s.parts)
					s.addPart(false)
					s.startPart(e, s.textIndex)
				}
				if !s.appendDelta(e, s.textIndex, *choice.Delta.Content) {
					return false
				}
			}
			for _, tool := range choice.Delta.ToolCalls {
				index, ok := s.tools[tool.Index]
				if !ok {
					if tool.ID == "" || tool.Function.Name == "" {
						e.fail()
						return false
					}
					index = len(s.parts)
					s.tools[tool.Index] = index
					p := s.addPart(true)
					p.CallID = tool.ID
					p.Name = tool.Function.Name
					s.startPart(e, index)
				}
				if !s.appendDelta(e, index, tool.Function.Arguments) {
					return false
				}
			}
			if choice.Finish != "" {
				s.finish = choice.Finish
			}
		}
		return e.flush()
	}
	for scanner.Scan() {
		if r.Context().Err() != nil {
			return
		}
		line := scanner.Text()
		if line == "" {
			if !process() {
				break
			}
			continue
		}
		if strings.HasPrefix(line, "data:") {
			if data.Len() > 0 {
				data.WriteByte('\n')
			}
			data.WriteString(strings.TrimPrefix(strings.TrimPrefix(line, "data:"), " "))
			if data.Len() > maxInferenceBody {
				e.fail()
				return
			}
		}
	}
	if !ended && data.Len() > 0 {
		if !process() && !ended {
			return
		}
	}
	if e.err != nil || e.failed || r.Context().Err() != nil {
		return
	}
	if scanner.Err() != nil || !validFinish(s.finish) {
		e.fail()
		return
	}
	// EOF is accepted after a terminal finish_reason, as some compatible
	// providers omit [DONE]. A truncated generation must never look successful.
	for i, p := range s.parts {
		if p.Tool && !json.Valid([]byte(p.Text.String())) {
			e.fail()
			return
		}
		s.endPart(e, i)
	}
	if protocol == "messages" {
		result := s.result()
		e.emit("message_delta", jsonObject{"delta": jsonObject{"stop_reason": result["stop_reason"], "stop_sequence": nil}, "usage": result["usage"]})
		e.emit("message_stop", jsonObject{})
	} else {
		result := s.result()
		kind := "response.completed"
		if result["status"] == "incomplete" {
			kind = "response.incomplete"
		}
		e.emit(kind, jsonObject{"response": result})
	}
	e.flush()
}

func (s *outputState) startPart(e *eventWriter, index int) {
	if s.protocol == "messages" {
		e.emit("content_block_start", jsonObject{"index": index, "content_block": s.part(index, false)})
		return
	}
	e.emit("response.output_item.added", jsonObject{"output_index": index, "item": s.part(index, false)})
	if !s.parts[index].Tool {
		e.emit("response.content_part.added", jsonObject{"item_id": s.parts[index].ID, "output_index": index, "content_index": 0, "part": jsonObject{"type": "output_text", "text": "", "annotations": []any{}, "logprobs": []any{}}})
	}
}

func (s *outputState) appendDelta(e *eventWriter, index int, delta string) bool {
	if delta == "" {
		return true
	}
	s.bytes += len(delta)
	if s.bytes > maxInferenceBody {
		e.fail()
		return false
	}
	p := s.parts[index]
	p.Text.WriteString(delta)
	if s.protocol == "messages" {
		block := jsonObject{"type": "text_delta", "text": delta}
		if p.Tool {
			block = jsonObject{"type": "input_json_delta", "partial_json": delta}
		}
		e.emit("content_block_delta", jsonObject{"index": index, "delta": block})
	} else {
		kind := "response.output_text.delta"
		data := jsonObject{"item_id": p.ID, "output_index": index, "delta": delta}
		if p.Tool {
			kind = "response.function_call_arguments.delta"
		} else {
			data["content_index"] = 0
			data["logprobs"] = []any{}
		}
		e.emit(kind, data)
	}
	return e.err == nil
}

func (s *outputState) endPart(e *eventWriter, index int) {
	p := s.parts[index]
	if p.closed {
		return
	}
	p.closed = true
	if s.protocol == "messages" {
		e.emit("content_block_stop", jsonObject{"index": index})
		return
	}
	data := jsonObject{"item_id": p.ID, "output_index": index}
	if p.Tool {
		data["arguments"] = p.Text.String()
		data["name"] = p.Name
		e.emit("response.function_call_arguments.done", data)
	} else {
		data["content_index"] = 0
		data["text"] = p.Text.String()
		data["logprobs"] = []any{}
		e.emit("response.output_text.done", data)
		e.emit("response.content_part.done", jsonObject{"item_id": p.ID, "output_index": index, "content_index": 0, "part": jsonObject{"type": "output_text", "text": p.Text.String(), "annotations": []any{}, "logprobs": []any{}}})
	}
	e.emit("response.output_item.done", jsonObject{"output_index": index, "item": s.part(index, true)})
}
