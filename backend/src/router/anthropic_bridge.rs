//! Chat Completions clients calling native Anthropic Messages upstreams.
use super::*;

fn invalid(message: &str) -> Failure {
    Failure::new(400, message)
}

fn blocks(value: &Value) -> Api<Vec<Value>> {
    match value {
        Value::Null => Ok(vec![]),
        Value::String(text) => Ok(vec![json!({"type":"text","text":text})]),
        Value::Array(parts) => parts.iter().map(|part| {
            match text(part, "type") {
                "text" if part["text"].is_string() => Ok(json!({"type":"text","text":part["text"]})),
                "image_url" => {
                    let url = text(&part["image_url"], "url");
                    if let Some(data) = url.strip_prefix("data:") {
                        let (media_type, data) = data.split_once(";base64,")
                            .ok_or_else(|| invalid("invalid base64 image URL"))?;
                        return Ok(json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}}));
                    }
                    if !url.starts_with("https://") && !url.starts_with("http://") {
                        return Err(invalid("image_url must be HTTP(S) or a base64 data URL"));
                    }
                    Ok(json!({"type":"image","source":{"type":"url","url":url}}))
                }
                _ => Err(invalid("unsupported Chat Completions content block for Messages conversion")),
            }
        }).collect(),
        _ => Err(invalid("message content must be text or an array")),
    }
}

pub(super) fn request(chat: &Value) -> Api<Value> {
    bridge::fields(
        chat,
        "model messages max_tokens max_completion_tokens stream stream_options temperature top_p stop tools tool_choice parallel_tool_calls n",
    )?;
    if chat.get("n").is_some_and(|n| n != 1) {
        return Err(invalid("Messages conversion supports n=1"));
    }
    if chat.get("max_tokens").is_some() && chat.get("max_completion_tokens").is_some() {
        return Err(invalid("use one output token limit"));
    }
    let limit = chat
        .get("max_tokens")
        .or_else(|| chat.get("max_completion_tokens"))
        .map_or(Some(1024), Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("output limit must be a positive integer"))?;
    let input = chat["messages"]
        .as_array()
        .ok_or_else(|| invalid("messages must be an array"))?;
    let mut messages = Vec::new();
    let mut system = Vec::new();
    for message in input {
        bridge::fields(message, "role content tool_calls tool_call_id")?;
        let mut content = blocks(&message["content"])?;
        match text(message, "role") {
            "system" | "developer" => {
                if !messages.is_empty() || content.iter().any(|block| block["type"] != "text") {
                    return Err(invalid("Messages conversion requires leading text system messages"));
                }
                system.extend(content);
            }
            "user" => messages.push(json!({"role":"user","content":content})),
            "tool" => messages.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":message["tool_call_id"],"content":content}]})),
            "assistant" => {
                if let Some(calls) = message.get("tool_calls") {
                    for call in calls.as_array().ok_or_else(|| invalid("tool_calls must be an array"))? {
                        if call["type"] != "function" {
                            return Err(invalid("only function tool calls can be converted"));
                        }
                        let arguments: Value = serde_json::from_str(text(&call["function"], "arguments"))
                            .map_err(|_| invalid("invalid function arguments"))?;
                        if !arguments.is_object() {
                            return Err(invalid("function arguments must be an object"));
                        }
                        content.push(json!({"type":"tool_use","id":call["id"],"name":call["function"]["name"],"input":arguments}));
                    }
                }
                messages.push(json!({"role":"assistant","content":content}));
            }
            _ => return Err(invalid("unsupported message role for Messages conversion")),
        }
    }
    if messages.is_empty() {
        return Err(invalid("messages must include a conversation"));
    }
    let mut result = json!({"model":chat["model"],"messages":messages,"max_tokens":limit});
    if !system.is_empty() {
        result["system"] = json!(system);
    }
    for key in ["stream", "temperature", "top_p"] {
        if let Some(value) = chat.get(key) {
            result[key] = value.clone();
        }
    }
    if let Some(stop) = chat.get("stop") {
        result["stop_sequences"] = match stop {
            Value::String(_) => json!([stop]),
            Value::Array(items) if items.iter().all(Value::is_string) => stop.clone(),
            _ => return Err(invalid("stop must be text or an array of strings")),
        };
    }
    if let Some(tools) = chat.get("tools") {
        let tools = tools
            .as_array()
            .ok_or_else(|| invalid("tools must be an array"))?;
        let mut converted = Vec::new();
        for tool in tools {
            if tool["type"] != "function" {
                return Err(invalid("only function tools can be converted"));
            }
            let f = &tool["function"];
            bridge::fields(f, "name description parameters")?;
            let mut entry = json!({"name":f["name"],"input_schema":f.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object"}))});
            if let Some(description) = f.get("description") {
                entry["description"] = description.clone();
            }
            converted.push(entry);
        }
        if !converted.is_empty() {
            result["tools"] = json!(converted);
        }
    }
    if let Some(choice) = chat.get("tool_choice") {
        result["tool_choice"] = match choice.as_str() {
            Some("none" | "auto") => json!({"type":choice}),
            Some("required") => json!({"type":"any"}),
            _ if choice["type"] == "function" && choice["function"]["name"].is_string() => {
                json!({"type":"tool","name":choice["function"]["name"]})
            }
            _ => return Err(invalid("unsupported tool_choice for Messages conversion")),
        };
    }
    if chat["parallel_tool_calls"] == false && result.get("tools").is_some() {
        if result.get("tool_choice").is_none() {
            result["tool_choice"] = json!({"type":"auto"});
        }
        if result["tool_choice"]["type"] != "none" {
            result["tool_choice"]["disable_parallel_tool_use"] = json!(true);
        }
    }
    Ok(result)
}

pub(super) fn finish_reason(reason: &str) -> Option<&'static str> {
    match reason {
        "end_turn" | "stop_sequence" => Some("stop"),
        "max_tokens" | "model_context_window_exceeded" => Some("length"),
        "tool_use" => Some("tool_calls"),
        "refusal" => Some("content_filter"),
        _ => None,
    }
}

pub(super) fn usage(value: &Value) -> Value {
    let cached = value["cache_read_input_tokens"].as_u64().unwrap_or(0);
    let input = value["input_tokens"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(cached)
        .saturating_add(value["cache_creation_input_tokens"].as_u64().unwrap_or(0));
    let output = value["output_tokens"].as_u64().unwrap_or(0);
    json!({"prompt_tokens":input,"completion_tokens":output,"total_tokens":input.saturating_add(output),
        "prompt_tokens_details":{"cached_tokens":cached}})
}

pub(super) fn response(message: &Value, model: &str) -> Api<Value> {
    let invalid = || Failure::new(502, "invalid upstream Anthropic Messages response");
    let reason = finish_reason(text(message, "stop_reason")).ok_or_else(invalid)?;
    let mut content = String::new();
    let mut tools = Vec::new();
    for block in message["content"].as_array().ok_or_else(invalid)? {
        match text(block, "type") {
            "text" => content.push_str(block["text"].as_str().ok_or_else(invalid)?),
            "tool_use" if block["id"].is_string() && block["name"].is_string() && block["input"].is_object() =>
                tools.push(json!({"id":block["id"],"type":"function","function":{"name":block["name"],"arguments":block["input"].to_string()}})),
            "thinking" | "redacted_thinking" => {}
            _ => return Err(invalid()),
        }
    }
    let mut assistant = json!({"role":"assistant","content":content});
    if !tools.is_empty() {
        assistant["tool_calls"] = json!(tools);
        if content.is_empty() {
            assistant["content"] = Value::Null;
        }
    }
    Ok(
        json!({"id":message["id"],"object":"chat.completion","created":Utc::now().timestamp(),"model":model,
        "choices":[{"index":0,"message":assistant,"finish_reason":reason}],"usage":usage(&message["usage"])}),
    )
}
