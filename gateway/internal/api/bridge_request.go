package api

import (
	"encoding/json"
	"fmt"
	"strings"
)

type jsonObject = map[string]any

func object(v any) (jsonObject, error) {
	m, ok := v.(map[string]any)
	if !ok {
		return nil, fmt.Errorf("expected object")
	}
	return m, nil
}

func allowFields(m jsonObject, allowed string) error {
	for key := range m {
		if !strings.Contains(" "+allowed+" ", " "+key+" ") {
			return fmt.Errorf("unsupported parameter %q for chat conversion; use a native upstream", key)
		}
	}
	return nil
}

// convertRequest admits only fields with a defined mapping. It must never
// silently discard provider features such as storage, caching or hosted tools.
func convertRequest(body []byte, protocol string) (jsonObject, error) {
	var in jsonObject
	if json.Unmarshal(body, &in) != nil || in == nil {
		return nil, fmt.Errorf("invalid JSON request")
	}
	out := jsonObject{"model": in["model"]}
	for _, key := range []string{"stream", "temperature", "top_p"} {
		if value, ok := in[key]; ok {
			out[key] = value
		}
	}
	var messages []any
	if protocol == "responses" {
		if err := allowFields(in, "model input instructions stream max_output_tokens temperature top_p tools tool_choice parallel_tool_calls store text"); err != nil {
			return nil, err
		}
		if value, ok := in["store"]; ok && value != false {
			return nil, fmt.Errorf("store must be false for stateless chat conversion")
		}
		if value, ok := in["instructions"]; ok {
			if _, ok := value.(string); !ok {
				return nil, fmt.Errorf("instructions must be text")
			}
			messages = append(messages, jsonObject{"role": "system", "content": value})
		}
		if value, ok := in["max_output_tokens"]; ok {
			out["max_tokens"] = value
		}
		if value, ok := in["parallel_tool_calls"]; ok {
			out["parallel_tool_calls"] = value
		}
		switch input := in["input"].(type) {
		case string:
			messages = append(messages, jsonObject{"role": "user", "content": input})
		case []any:
			for _, value := range input {
				item, err := object(value)
				if err != nil {
					return nil, err
				}
				switch item["type"] {
				case "function_call":
					if err := allowFields(item, "type id call_id name arguments status"); err != nil {
						return nil, err
					}
					messages = append(messages, jsonObject{"role": "assistant", "content": nil, "tool_calls": []any{jsonObject{"id": item["call_id"], "type": "function", "function": jsonObject{"name": item["name"], "arguments": item["arguments"]}}}})
				case "function_call_output":
					if err := allowFields(item, "type id call_id output status"); err != nil {
						return nil, err
					}
					if _, ok := item["output"].(string); !ok {
						return nil, fmt.Errorf("function_call_output.output must be text")
					}
					messages = append(messages, jsonObject{"role": "tool", "tool_call_id": item["call_id"], "content": item["output"]})
				case nil, "message":
					if err := allowFields(item, "type id role content status"); err != nil {
						return nil, err
					}
					content, err := convertContent(item["content"], "responses")
					if err != nil {
						return nil, err
					}
					messages = append(messages, jsonObject{"role": item["role"], "content": content})
				default:
					return nil, fmt.Errorf("unsupported Responses input item")
				}
			}
		default:
			return nil, fmt.Errorf("input must be text or an array")
		}
		if text, ok := in["text"]; ok {
			settings, err := object(text)
			if err != nil {
				return nil, err
			}
			if err := allowFields(settings, "format"); err != nil {
				return nil, err
			}
			format, err := object(settings["format"])
			if err != nil {
				return nil, err
			}
			switch format["type"] {
			case "text", "json_object":
				if err := allowFields(format, "type"); err != nil {
					return nil, err
				}
				out["response_format"] = format
			case "json_schema":
				if err := allowFields(format, "type name description schema strict"); err != nil {
					return nil, err
				}
				schema := jsonObject{}
				for k, v := range format {
					if k != "type" {
						schema[k] = v
					}
				}
				out["response_format"] = jsonObject{"type": "json_schema", "json_schema": schema}
			default:
				return nil, fmt.Errorf("unsupported text.format")
			}
		}
	} else {
		if err := allowFields(in, "model messages system max_tokens stream temperature top_p stop_sequences tools tool_choice"); err != nil {
			return nil, err
		}
		if in["max_tokens"] == nil {
			return nil, fmt.Errorf("max_tokens is required")
		}
		out["max_tokens"] = in["max_tokens"]
		if value, ok := in["stop_sequences"]; ok {
			out["stop"] = value
		}
		if value, ok := in["system"]; ok {
			content, err := convertContent(value, "messages")
			if err != nil {
				return nil, err
			}
			messages = append(messages, jsonObject{"role": "system", "content": content})
		}
		items, ok := in["messages"].([]any)
		if !ok {
			return nil, fmt.Errorf("messages must be an array")
		}
		for _, value := range items {
			item, err := object(value)
			if err != nil {
				return nil, err
			}
			if err := allowFields(item, "role content"); err != nil {
				return nil, err
			}
			if blocks, ok := item["content"].([]any); ok {
				var content, calls []any
				for _, value := range blocks {
					block, err := object(value)
					if err != nil {
						return nil, err
					}
					switch block["type"] {
					case "tool_use":
						if item["role"] != "assistant" {
							return nil, fmt.Errorf("tool_use requires assistant role")
						}
						if err := allowFields(block, "type id name input"); err != nil {
							return nil, err
						}
						args, err := json.Marshal(block["input"])
						if err != nil {
							return nil, err
						}
						calls = append(calls, jsonObject{"id": block["id"], "type": "function", "function": jsonObject{"name": block["name"], "arguments": string(args)}})
					case "tool_result":
						if item["role"] != "user" {
							return nil, fmt.Errorf("tool_result requires user role")
						}
						if err := allowFields(block, "type tool_use_id content"); err != nil {
							return nil, err
						}
						result, err := convertContent(block["content"], "messages")
						if err != nil {
							return nil, err
						}
						messages = append(messages, jsonObject{"role": "tool", "tool_call_id": block["tool_use_id"], "content": result})
					default:
						converted, err := convertContent([]any{block}, "messages")
						if err != nil {
							return nil, err
						}
						content = append(content, converted.([]any)...)
					}
				}
				if len(content) > 0 || len(calls) > 0 {
					message := jsonObject{"role": item["role"], "content": content}
					if len(calls) > 0 {
						message["tool_calls"] = calls
					}
					messages = append(messages, message)
				}
			} else {
				content, err := convertContent(item["content"], "messages")
				if err != nil {
					return nil, err
				}
				messages = append(messages, jsonObject{"role": item["role"], "content": content})
			}
		}
	}
	if len(messages) == 0 {
		return nil, fmt.Errorf("at least one message is required")
	}
	out["messages"] = messages
	if raw, ok := in["tools"]; ok {
		tools, ok := raw.([]any)
		if !ok {
			return nil, fmt.Errorf("tools must be an array")
		}
		converted := make([]any, 0, len(tools))
		for _, raw := range tools {
			tool, err := object(raw)
			if err != nil {
				return nil, err
			}
			function := jsonObject{}
			if protocol == "responses" {
				if tool["type"] != "function" {
					return nil, fmt.Errorf("only function tools support chat conversion")
				}
				if err := allowFields(tool, "type name description parameters strict"); err != nil {
					return nil, err
				}
				for k, v := range tool {
					if k != "type" {
						function[k] = v
					}
				}
			} else {
				if err := allowFields(tool, "name description input_schema"); err != nil {
					return nil, err
				}
				function["name"], function["parameters"] = tool["name"], tool["input_schema"]
				if v, ok := tool["description"]; ok {
					function["description"] = v
				}
			}
			converted = append(converted, jsonObject{"type": "function", "function": function})
		}
		out["tools"] = converted
	}
	if choice, ok := in["tool_choice"]; ok {
		if text, ok := choice.(string); ok && protocol == "responses" {
			if text != "auto" && text != "required" && text != "none" {
				return nil, fmt.Errorf("unsupported tool_choice")
			}
			out["tool_choice"] = text
		} else {
			m, err := object(choice)
			if err != nil {
				return nil, err
			}
			if err := allowFields(m, "type name disable_parallel_tool_use"); err != nil {
				return nil, err
			}
			switch m["type"] {
			case "auto", "none":
				out["tool_choice"] = m["type"]
			case "any":
				out["tool_choice"] = "required"
			case "tool", "function":
				out["tool_choice"] = jsonObject{"type": "function", "function": jsonObject{"name": m["name"]}}
			default:
				return nil, fmt.Errorf("unsupported tool_choice type")
			}
			if disabled, ok := m["disable_parallel_tool_use"].(bool); ok {
				out["parallel_tool_calls"] = !disabled
			}
		}
	}
	if out["stream"] == true {
		out["stream_options"] = jsonObject{"include_usage": true}
	}
	return out, nil
}

func convertContent(raw any, protocol string) (any, error) {
	if text, ok := raw.(string); ok {
		return text, nil
	}
	blocks, ok := raw.([]any)
	if !ok {
		return nil, fmt.Errorf("content must be text or an array")
	}
	out := make([]any, 0, len(blocks))
	for _, raw := range blocks {
		block, err := object(raw)
		if err != nil {
			return nil, err
		}
		switch block["type"] {
		case "text", "input_text", "output_text":
			if err := allowFields(block, "type text annotations"); err != nil {
				return nil, err
			}
			if annotations, ok := block["annotations"].([]any); ok && len(annotations) > 0 {
				return nil, fmt.Errorf("annotated content requires native upstream")
			}
			if _, ok := block["text"].(string); !ok {
				return nil, fmt.Errorf("text block requires text")
			}
			out = append(out, jsonObject{"type": "text", "text": block["text"]})
		case "input_image":
			if protocol != "responses" {
				return nil, fmt.Errorf("unsupported content type")
			}
			if err := allowFields(block, "type image_url detail"); err != nil {
				return nil, err
			}
			url, ok := block["image_url"].(string)
			if !ok {
				return nil, fmt.Errorf("image_url is required; file IDs need native upstream")
			}
			image := jsonObject{"url": url}
			if detail, ok := block["detail"]; ok {
				image["detail"] = detail
			}
			out = append(out, jsonObject{"type": "image_url", "image_url": image})
		case "image":
			if protocol != "messages" {
				return nil, fmt.Errorf("unsupported content type")
			}
			if err := allowFields(block, "type source"); err != nil {
				return nil, err
			}
			source, err := object(block["source"])
			if err != nil {
				return nil, err
			}
			var url string
			switch source["type"] {
			case "url":
				if err := allowFields(source, "type url"); err != nil {
					return nil, err
				}
				url, _ = source["url"].(string)
			case "base64":
				if err := allowFields(source, "type media_type data"); err != nil {
					return nil, err
				}
				mime, _ := source["media_type"].(string)
				data, _ := source["data"].(string)
				if mime != "" && data != "" {
					url = "data:" + mime + ";base64," + data
				}
			default:
				return nil, fmt.Errorf("unsupported image source")
			}
			if url == "" {
				return nil, fmt.Errorf("empty image source")
			}
			out = append(out, jsonObject{"type": "image_url", "image_url": jsonObject{"url": url}})
		default:
			return nil, fmt.Errorf("unsupported content type; use native upstream")
		}
	}
	return out, nil
}
