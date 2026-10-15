package api

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

type chatTool struct {
	Index    int    `json:"index"`
	ID       string `json:"id"`
	Type     string `json:"type"`
	Function struct {
		Name      string `json:"name"`
		Arguments string `json:"arguments"`
	} `json:"function"`
}
type chatMessage struct {
	Content   *string    `json:"content"`
	ToolCalls []chatTool `json:"tool_calls"`
	Refusal   *string    `json:"refusal"`
}
type chatUsage struct {
	Input   int `json:"prompt_tokens"`
	Output  int `json:"completion_tokens"`
	Details struct {
		Cached     int `json:"cached_tokens"`
		CacheWrite int `json:"cache_write_tokens"`
	} `json:"prompt_tokens_details"`
	OutputDetails struct {
		Reasoning int `json:"reasoning_tokens"`
	} `json:"completion_tokens_details"`
}
type chatResult struct {
	Choices []struct {
		Index   int         `json:"index"`
		Message chatMessage `json:"message"`
		Delta   chatMessage `json:"delta"`
		Finish  string      `json:"finish_reason"`
	} `json:"choices"`
	Usage *chatUsage      `json:"usage"`
	Error json.RawMessage `json:"error"`
}

type outputPart struct {
	ID, CallID, Name string
	Tool             bool
	Text             strings.Builder
	closed           bool
}
type outputState struct {
	protocol, model, id, finish string
	created                     int64
	parts                       []*outputPart
	tools                       map[int]int
	textIndex                   int
	usage                       chatUsage
	bytes                       int
	request                     jsonObject
}

func newOutputState(protocol, model string, request ...jsonObject) *outputState {
	var nonce [12]byte
	_, _ = rand.Read(nonce[:])
	prefix := "resp_"
	if protocol == "messages" {
		prefix = "msg_"
	}
	s := &outputState{protocol: protocol, model: model, id: prefix + hex.EncodeToString(nonce[:]), created: time.Now().Unix(), tools: map[int]int{}, textIndex: -1}
	if len(request) > 0 {
		s.request = request[0]
	}
	return s
}

func (s *outputState) addPart(tool bool) *outputPart {
	p := &outputPart{ID: fmt.Sprintf("%s_%d", s.id, len(s.parts)), Tool: tool}
	s.parts = append(s.parts, p)
	return p
}

func (s *outputState) complete(result chatResult) error {
	choice := result.Choices[0]
	s.finish = choice.Finish
	if !validFinish(s.finish) {
		return fmt.Errorf("unsupported upstream finish reason")
	}
	if result.Usage != nil {
		s.usage = *result.Usage
	}
	if choice.Message.Refusal != nil {
		return fmt.Errorf("upstream refusal cannot be represented by this conversion")
	}
	if choice.Message.Content != nil {
		p := s.addPart(false)
		p.Text.WriteString(*choice.Message.Content)
	}
	for _, tool := range choice.Message.ToolCalls {
		p := s.addPart(true)
		p.CallID = tool.ID
		p.Name = tool.Function.Name
		p.Text.WriteString(tool.Function.Arguments)
		if !json.Valid([]byte(p.Text.String())) {
			return fmt.Errorf("upstream returned invalid tool arguments")
		}
	}
	if len(s.parts) == 0 && s.finish != "length" {
		return fmt.Errorf("upstream returned no content or tool calls")
	}
	return nil
}

func (s *outputState) part(index int, done bool) jsonObject {
	p := s.parts[index]
	status := "in_progress"
	if done {
		status = "completed"
	}
	if s.protocol == "messages" {
		if p.Tool {
			var input any = jsonObject{}
			if done {
				_ = json.Unmarshal([]byte(p.Text.String()), &input)
			}
			return jsonObject{"type": "tool_use", "id": p.CallID, "name": p.Name, "input": input}
		}
		text := ""
		if done {
			text = p.Text.String()
		}
		return jsonObject{"type": "text", "text": text}
	}
	if p.Tool {
		args := ""
		if done {
			args = p.Text.String()
		}
		return jsonObject{"type": "function_call", "id": p.ID, "call_id": p.CallID, "name": p.Name, "arguments": args, "status": status}
	}
	content := []any{}
	if done {
		content = append(content, jsonObject{"type": "output_text", "text": p.Text.String(), "annotations": []any{}, "logprobs": []any{}})
	}
	return jsonObject{"type": "message", "id": p.ID, "role": "assistant", "status": status, "content": content}
}

func (s *outputState) result() jsonObject {
	items := make([]any, 0, len(s.parts))
	for i := range s.parts {
		items = append(items, s.part(i, true))
	}
	if s.protocol == "messages" {
		stop := "end_turn"
		if s.finish == "length" {
			stop = "max_tokens"
		}
		if s.finish == "tool_calls" {
			stop = "tool_use"
		}
		input := s.usage.Input - s.usage.Details.Cached - s.usage.Details.CacheWrite
		if input < 0 {
			input = 0
		}
		return jsonObject{"id": s.id, "type": "message", "role": "assistant", "model": s.model, "content": items, "stop_reason": stop, "stop_sequence": nil, "usage": jsonObject{"input_tokens": input, "output_tokens": s.usage.Output, "cache_read_input_tokens": s.usage.Details.Cached, "cache_creation_input_tokens": s.usage.Details.CacheWrite}}
	}
	status := "completed"
	var incomplete any
	if s.finish == "length" {
		status = "incomplete"
		incomplete = jsonObject{"reason": "max_output_tokens"}
	}
	tools := []any{}
	if configured, ok := s.request["tools"].([]any); ok {
		for _, raw := range configured {
			tool, _ := object(raw)
			function, _ := object(tool["function"])
			item := jsonObject{"type": "function"}
			for k, v := range function {
				item[k] = v
			}
			tools = append(tools, item)
		}
	}
	var choice any = "auto"
	if value, ok := s.request["tool_choice"]; ok {
		choice = value
		if m, ok := value.(map[string]any); ok {
			function, _ := object(m["function"])
			choice = jsonObject{"type": "function", "name": function["name"]}
		}
	}
	var parallel any = true
	if value, ok := s.request["parallel_tool_calls"]; ok {
		parallel = value
	}
	return jsonObject{"id": s.id, "object": "response", "created_at": s.created, "status": status, "model": s.model, "output": items, "error": nil, "incomplete_details": incomplete, "store": false, "parallel_tool_calls": parallel, "tool_choice": choice, "tools": tools, "usage": jsonObject{"input_tokens": s.usage.Input, "output_tokens": s.usage.Output, "total_tokens": s.usage.Input + s.usage.Output, "input_tokens_details": jsonObject{"cached_tokens": s.usage.Details.Cached, "cache_write_tokens": s.usage.Details.CacheWrite}, "output_tokens_details": jsonObject{"reasoning_tokens": s.usage.OutputDetails.Reasoning}}}
}

func validFinish(reason string) bool {
	return reason == "stop" || reason == "length" || reason == "tool_calls"
}
