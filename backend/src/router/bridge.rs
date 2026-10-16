use super::*;
fn fields(v: &Value, allowed: &str) -> Api<()> {
    let object = v
        .as_object()
        .ok_or_else(|| Failure::new(400, "expected an object"))?;
    for key in object.keys() {
        if !allowed.split_whitespace().any(|a| a == key) {
            return Err(Failure::new(
                400,
                format!("unsupported parameter {key:?} for chat conversion; use a native upstream"),
            ));
        }
    }
    Ok(())
}
fn content(v: &Value, protocol: &str) -> Api<Value> {
    if v.is_string() || v.is_null() {
        return Ok(v.clone());
    }
    let mut out = vec![];
    for item in v
        .as_array()
        .ok_or_else(|| Failure::new(400, "content must be text or an array"))?
    {
        match text(item, "type") {
            "input_text" | "output_text" | "text" => {
                fields(item, "type text")?;
                out.push(json!({"type":"text","text":item["text"]}));
            }
            "input_image" => {
                fields(item, "type image_url detail")?;
                out.push(json!({"type":"image_url","image_url":{"url":item["image_url"],"detail":item.get("detail").cloned().unwrap_or(json!("auto"))}}));
            }
            "image" if protocol == "messages" => {
                fields(item, "type source")?;
                let source = &item["source"];
                let url = match text(source, "type") {
                    "base64" => format!(
                        "data:{};base64,{}",
                        text(source, "media_type"),
                        text(source, "data")
                    ),
                    "url" => text(source, "url").into(),
                    _ => return Err(Failure::new(400, "unsupported image source")),
                };
                out.push(json!({"type":"image_url","image_url":{"url":url}}));
            }
            _ => {
                return Err(Failure::new(
                    400,
                    "unsupported content block for chat conversion",
                ));
            }
        }
    }
    Ok(json!(out))
}
pub(super) fn request(input: &Value, protocol: &str) -> Api<Value> {
    fields(
        input,
        if protocol == "responses" {
            "model input instructions stream max_output_tokens temperature top_p tools tool_choice parallel_tool_calls store text"
        } else {
            "model messages system max_tokens stream temperature top_p stop_sequences tools tool_choice"
        },
    )?;
    let mut out = json!({"model":input["model"]});
    let mut messages = vec![];
    for key in ["stream", "temperature", "top_p"] {
        if let Some(v) = input.get(key) {
            out[key] = v.clone();
        }
    }
    if protocol == "responses" {
        if input.get("store").is_some_and(|v| v != false) {
            return Err(Failure::new(
                400,
                "store must be false for stateless chat conversion",
            ));
        }
        if let Some(instructions) = input.get("instructions") {
            if !instructions.is_string() {
                return Err(Failure::new(400, "instructions must be text"));
            }
            messages.push(json!({"role":"system","content":instructions}));
        }
        if let Some(n) = input.get("max_output_tokens") {
            out["max_tokens"] = n.clone();
        }
        if let Some(n) = input.get("parallel_tool_calls") {
            out["parallel_tool_calls"] = n.clone();
        }
        match &input["input"] {
            Value::String(s) => messages.push(json!({"role":"user","content":s})),
            Value::Array(items) => {
                for item in items {
                    match text(item, "type") {
                        "function_call" => {
                            fields(item, "type id call_id name arguments status")?;
                            messages.push(json!({"role":"assistant","content":null,"tool_calls":[{"id":item["call_id"],"type":"function","function":{"name":item["name"],"arguments":item["arguments"]}}]}));
                        }
                        "function_call_output" => {
                            fields(item, "type id call_id output status")?;
                            if !item["output"].is_string() {
                                return Err(Failure::new(
                                    400,
                                    "function_call_output.output must be text",
                                ));
                            }
                            messages.push(json!({"role":"tool","tool_call_id":item["call_id"],"content":item["output"]}));
                        }
                        "" | "message" => {
                            fields(item, "type id role content status")?;
                            messages.push(json!({"role":item["role"],"content":content(&item["content"],protocol)?}));
                        }
                        _ => return Err(Failure::new(400, "unsupported Responses input item")),
                    }
                }
            }
            _ => return Err(Failure::new(400, "input must be text or an array")),
        }
        if let Some(settings) = input.get("text") {
            fields(settings, "format")?;
            let format = &settings["format"];
            out["response_format"] = match text(format, "type") {
                "text" | "json_object" => {
                    fields(format, "type")?;
                    format.clone()
                }
                "json_schema" => {
                    fields(format, "type name description schema strict")?;
                    let mut schema = format.clone();
                    schema.as_object_mut().unwrap().remove("type");
                    json!({"type":"json_schema","json_schema":schema})
                }
                _ => return Err(Failure::new(400, "unsupported text.format")),
            };
        }
    } else {
        if input.get("max_tokens").is_none() {
            return Err(Failure::new(400, "max_tokens is required"));
        }
        out["max_tokens"] = input["max_tokens"].clone();
        if let Some(stop) = input.get("stop_sequences") {
            out["stop"] = stop.clone();
        }
        if let Some(system) = input.get("system") {
            messages.push(json!({"role":"system","content":content(system,protocol)?}));
        }
        for item in input["messages"]
            .as_array()
            .ok_or_else(|| Failure::new(400, "messages must be an array"))?
        {
            fields(item, "role content")?;
            if let Some(blocks) = item["content"].as_array() {
                let mut normal = vec![];
                let mut calls = vec![];
                for block in blocks {
                    match text(block, "type") {
                        "tool_use" => {
                            if item["role"] != "assistant" {
                                return Err(Failure::new(400, "tool_use requires assistant role"));
                            }
                            fields(block, "type id name input")?;
                            calls.push(json!({"id":block["id"],"type":"function","function":{"name":block["name"],"arguments":serde_json::to_string(&block["input"]).unwrap()}}));
                        }
                        "tool_result" => {
                            if item["role"] != "user" {
                                return Err(Failure::new(400, "tool_result requires user role"));
                            }
                            fields(block, "type tool_use_id content")?;
                            messages.push(json!({"role":"tool","tool_call_id":block["tool_use_id"],"content":content(&block["content"],protocol)?}));
                        }
                        _ => normal.push(block.clone()),
                    }
                }
                if !normal.is_empty() || !calls.is_empty() {
                    let mut m = json!({"role":item["role"],"content":if normal.is_empty(){Value::Null}else{content(&json!(normal),protocol)?}});
                    if !calls.is_empty() {
                        m["tool_calls"] = json!(calls);
                    }
                    messages.push(m);
                }
            } else {
                messages.push(
                    json!({"role":item["role"],"content":content(&item["content"],protocol)?}),
                );
            }
        }
    }
    if let Some(tools) = input.get("tools") {
        let mut converted = vec![];
        for tool in tools
            .as_array()
            .ok_or_else(|| Failure::new(400, "tools must be an array"))?
        {
            let function = if protocol == "responses" {
                fields(tool, "type name description parameters strict")?;
                if tool["type"] != "function" {
                    return Err(Failure::new(400, "only function tools can be converted"));
                }
                let mut f = tool.clone();
                f.as_object_mut().unwrap().remove("type");
                f
            } else {
                fields(tool, "name description input_schema")?;
                let mut f = json!({"name":tool["name"],"parameters":tool["input_schema"]});
                if let Some(d) = tool.get("description") {
                    f["description"] = d.clone();
                }
                f
            };
            converted.push(json!({"type":"function","function":function}));
        }
        out["tools"] = json!(converted);
    }
    if let Some(choice) = input.get("tool_choice") {
        out["tool_choice"] = if protocol == "responses" {
            if choice.is_string() {
                choice.clone()
            } else {
                fields(choice, "type name")?;
                if choice["type"] != "function" {
                    return Err(Failure::new(400, "unsupported tool_choice"));
                }
                json!({"type":"function","function":{"name":choice["name"]}})
            }
        } else {
            match text(choice, "type") {
                "auto" => json!("auto"),
                "any" => json!("required"),
                "none" => json!("none"),
                "tool" => json!({"type":"function","function":{"name":choice["name"]}}),
                _ => return Err(Failure::new(400, "unsupported tool_choice")),
            }
        };
    }
    out["messages"] = json!(messages);
    Ok(out)
}
fn stop(reason: &str) -> &str {
    match reason {
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        "stop" => "end_turn",
        _ => "end_turn",
    }
}
pub(super) fn validate_response(chat: &Value) -> Api<()> {
    if chat["choices"].as_array().is_none_or(|v| v.len() != 1)
        || !["stop", "length", "tool_calls"].contains(&text(&chat["choices"][0], "finish_reason"))
        || !chat["choices"][0]["message"]["refusal"].is_null()
    {
        return Err(Failure::new(
            502,
            "invalid upstream Chat Completions response",
        ));
    }
    let msg = &chat["choices"][0]["message"];
    if !msg["content"].is_null() && !msg["content"].is_string() {
        return Err(Failure::new(502, "unsupported upstream chat content"));
    }
    for call in msg["tool_calls"].as_array().into_iter().flatten() {
        if call["type"] != "function"
            || text(call, "id").is_empty()
            || text(&call["function"], "name").is_empty()
            || serde_json::from_str::<Value>(text(&call["function"], "arguments")).is_err()
        {
            return Err(Failure::new(502, "invalid upstream tool call"));
        }
    }
    Ok(())
}
pub(super) fn context(request: &Value) -> Value {
    let mut result = json!({});
    for key in ["tools", "tool_choice", "parallel_tool_calls"] {
        if let Some(v) = request.get(key) {
            result[key] = v.clone();
        }
    }
    result
}
pub(super) fn response(chat: &Value, protocol: &str, request: &Value) -> Value {
    let msg = &chat["choices"][0]["message"];
    let finish = text(&chat["choices"][0], "finish_reason");
    let usage = &chat["usage"];
    let input = usage["prompt_tokens"].as_u64().unwrap_or(0);
    let output = usage["completion_tokens"].as_u64().unwrap_or(0);
    let mut result = if protocol == "messages" {
        let mut blocks = vec![];
        if let Some(s) = msg["content"].as_str().filter(|s| !s.is_empty()) {
            blocks.push(json!({"type":"text","text":s}));
        }
        for call in msg["tool_calls"].as_array().into_iter().flatten() {
            blocks.push(json!({"type":"tool_use","id":call["id"],"name":call["function"]["name"],"input":serde_json::from_str::<Value>(text(&call["function"],"arguments")).unwrap_or(json!({}))}));
        }
        json!({"id":chat["id"],"type":"message","role":"assistant","model":chat["model"],"content":blocks,"stop_reason":stop(finish),"stop_sequence":null,"usage":{"input_tokens":input,"output_tokens":output}})
    } else {
        let mut items = vec![];
        if let Some(s) = msg["content"].as_str().filter(|s| !s.is_empty()) {
            items.push(json!({"id":format!("msg_{}",uuid::Uuid::new_v4().simple()),"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":s,"annotations":[]}]}));
        }
        for call in msg["tool_calls"].as_array().into_iter().flatten() {
            items.push(json!({"id":format!("fc_{}",uuid::Uuid::new_v4().simple()),"type":"function_call","call_id":call["id"],"name":call["function"]["name"],"arguments":call["function"]["arguments"],"status":"completed"}));
        }
        json!({"id":chat["id"],"object":"response","created_at":chat["created"],"status":if finish=="length"{"incomplete"}else{"completed"},"model":chat["model"],"output":items,"output_text":msg["content"].as_str().unwrap_or(""),"error":null,"incomplete_details":if finish=="length"{json!({"reason":"max_output_tokens"})}else{Value::Null},"usage":{"input_tokens":input,"output_tokens":output,"total_tokens":input+output}})
    };
    let cached = usage["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0);
    let write = usage["prompt_tokens_details"]["cache_write_tokens"]
        .as_u64()
        .unwrap_or(0);
    if protocol == "messages" {
        result["usage"]["input_tokens"] = json!(input.saturating_sub(cached + write));
        result["usage"]["cache_read_input_tokens"] = json!(cached);
        result["usage"]["cache_creation_input_tokens"] = json!(write);
    } else {
        result["store"] = json!(false);
        result["parallel_tool_calls"] = request
            .get("parallel_tool_calls")
            .cloned()
            .unwrap_or(json!(true));
        result["tools"] = json!(
            request["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|t| {
                    let mut function = t["function"].clone();
                    function["type"] = json!("function");
                    function
                })
                .collect::<Vec<_>>()
        );
        let choice = request.get("tool_choice").cloned().unwrap_or(json!("auto"));
        result["tool_choice"] = if choice.is_object() {
            json!({"type":"function","name":choice["function"]["name"]})
        } else {
            choice
        };
        result["usage"]["input_tokens_details"] =
            json!({"cached_tokens":cached,"cache_write_tokens":write});
        result["usage"]["output_tokens_details"] = json!({"reasoning_tokens":usage["completion_tokens_details"]["reasoning_tokens"].as_u64().unwrap_or(0)});
    }
    result
}

pub(super) struct Stream {
    protocol: String,
    buffer: Vec<u8>,
    data: Vec<u8>,
    pub terminal: bool,
    pub failed: bool,
    pub usage: Value,
    id: String,
    model: String,
    finish_reason: String,
    request: Value,
    started: bool,
    text: String,
    tools: std::collections::BTreeMap<usize, Value>,
    tool_order: std::collections::BTreeMap<usize, usize>,
    text_index: Option<usize>,
    next_index: usize,
    sequence: u64,
    created: i64,
    output_bytes: usize,
}
impl Stream {
    pub fn new(protocol: &str, model: &str) -> Self {
        Self {
            protocol: protocol.into(),
            buffer: vec![],
            data: vec![],
            terminal: false,
            failed: false,
            usage: Value::Null,
            finish_reason: String::new(),
            id: format!("resp_{}", uuid::Uuid::new_v4().simple()),
            model: model.into(),
            request: Value::Null,
            started: false,
            text: String::new(),
            tools: Default::default(),
            tool_order: Default::default(),
            text_index: None,
            next_index: 0,
            sequence: 0,
            created: Utc::now().timestamp(),
            output_bytes: 0,
        }
    }
    pub fn with_request(mut self, request: Value) -> Self {
        self.request = request;
        self
    }
    fn event(&mut self, name: &str, mut value: Value) -> Vec<u8> {
        value["type"] = json!(name);
        if self.protocol == "responses" {
            value["sequence_number"] = json!(self.sequence);
            self.sequence += 1;
        }
        format!(
            "event: {name}\ndata: {}\n\n",
            serde_json::to_string(&value).unwrap()
        )
        .into_bytes()
    }
    fn fail(&mut self) -> Vec<u8> {
        self.failed = true;
        self.event("error", json!({"error":{"type":"api_error","message":"upstream chat stream failed","code":"UPSTREAM_UNAVAILABLE"}}))
    }
    fn native_finish(&self) -> bool {
        ["stop", "length", "tool_calls"].contains(&self.finish_reason.as_str())
    }
    fn valid_finish(&self) -> bool {
        self.native_finish()
            && self.tools.values().all(|tool| {
                serde_json::from_str::<Value>(text(&tool["function"], "arguments")).is_ok()
            })
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Api<Vec<u8>> {
        if self.terminal || self.failed {
            return Ok(if self.protocol.is_empty() {
                bytes.to_vec()
            } else {
                vec![]
            });
        }
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > 32 << 20 {
            return Err(Failure::new(502, "upstream SSE frame exceeds 32 MiB"));
        }
        let mut output = vec![];
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line = self.buffer.drain(..=end).collect::<Vec<_>>();
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if !line.is_empty() {
                if let Some(data) = line
                    .strip_prefix(b"data:")
                    .map(|s| s.strip_prefix(b" ").unwrap_or(s))
                {
                    if !self.data.is_empty() {
                        self.data.push(b'\n');
                    }
                    self.data.extend_from_slice(data);
                    if self.data.len() > 32 << 20 {
                        return Err(Failure::new(502, "upstream SSE frame exceeds 32 MiB"));
                    }
                }
                continue;
            }
            let frame = std::mem::take(&mut self.data);
            if frame.is_empty() {
                continue;
            }
            if frame == b"[DONE]" {
                self.terminal = true;
                if !self.protocol.is_empty() {
                    if self.valid_finish() {
                        output.extend(self.finish());
                    } else {
                        output.extend(self.fail());
                    }
                }
                break;
            }
            let chunk: Value = match serde_json::from_slice(&frame) {
                Ok(value) => value,
                Err(_) if self.protocol.is_empty() => continue,
                Err(_) => {
                    output.extend(self.fail());
                    break;
                }
            };
            if [
                "message_stop",
                "response.completed",
                "response.incomplete",
                "response.failed",
                "error",
            ]
            .contains(&text(&chunk, "type"))
                || chunk["error"].is_object()
            {
                self.terminal = true;
            }
            if let Some(usage) = chunk.get("usage").filter(|u| !u.is_null()) {
                self.usage = usage.clone();
            }
            if let Some(reason) = chunk["choices"][0]["finish_reason"].as_str() {
                self.finish_reason = reason.into();
            }
            if self.protocol.is_empty() {
                continue;
            }
            if chunk.get("error").is_some_and(|v| !v.is_null())
                || chunk["choices"].as_array().is_some_and(|a| a.len() > 1)
                || chunk["choices"][0]["index"]
                    .as_u64()
                    .is_some_and(|i| i != 0)
                || !chunk["choices"][0]["delta"]["refusal"].is_null()
            {
                output.extend(self.fail());
                break;
            }
            if !self.started {
                self.started = true;
                if self.protocol == "messages" {
                    output.extend(self.event("message_start", json!({"message":{"id":self.id,"type":"message","role":"assistant","model":self.model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}})));
                } else {
                    let response = json!({"id":self.id,"object":"response","created_at":self.created,"status":"in_progress","model":self.model,"output":[],"usage":null});
                    output.extend(self.event("response.created", json!({"response":response})));
                    output.extend(self.event("response.in_progress", json!({"response":response})));
                }
            }
            let delta = &chunk["choices"][0]["delta"];
            if let Some(fragment) = delta["content"].as_str().filter(|s| !s.is_empty()) {
                let index = match self.text_index {
                    Some(index) => index,
                    None => {
                        let index = self.next_index;
                        self.next_index += 1;
                        self.text_index = Some(index);
                        if self.protocol == "messages" {
                            output.extend(self.event(
                                "content_block_start",
                                json!({"index":index,"content_block":{"type":"text","text":""}}),
                            ));
                        } else {
                            output.extend(self.event("response.output_item.added", json!({"output_index":index,"item":{"id":format!("msg_{}",self.id),"type":"message","role":"assistant","status":"in_progress","content":[]}})));
                            output.extend(self.event("response.content_part.added", json!({"item_id":format!("msg_{}",self.id),"output_index":index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}})));
                        }
                        index
                    }
                };
                self.output_bytes += fragment.len();
                if self.output_bytes > 32 << 20 {
                    output.extend(self.fail());
                    break;
                }
                self.text.push_str(fragment);
                output.extend(if self.protocol == "messages" {
                    self.event("content_block_delta", json!({"index":index,"delta":{"type":"text_delta","text":fragment}}))
                } else {
                    self.event("response.output_text.delta", json!({"item_id":format!("msg_{}",self.id),"output_index":index,"content_index":0,"delta":fragment}))
                });
            }
            for tool in delta["tool_calls"].as_array().into_iter().flatten() {
                let native_index = tool["index"].as_u64().unwrap_or(0) as usize;
                let fresh = !self.tools.contains_key(&native_index);
                if fresh
                    && (text(tool, "id").is_empty() || text(&tool["function"], "name").is_empty())
                {
                    output.extend(self.fail());
                    return Ok(output);
                }
                if fresh {
                    self.tool_order.insert(native_index, self.next_index);
                    self.next_index += 1;
                }
                let index = self.tool_order[&native_index];
                let entry = self.tools.entry(native_index).or_insert(json!({"id":tool["id"],"type":"function","function":{"name":tool["function"]["name"],"arguments":""}}));
                let args = tool["function"]["arguments"].as_str().unwrap_or("");
                entry["function"]["arguments"] =
                    json!(format!("{}{}", text(&entry["function"], "arguments"), args));
                let entry = entry.clone();
                self.output_bytes += args.len();
                if self.output_bytes > 32 << 20 {
                    output.extend(self.fail());
                    return Ok(output);
                }
                if fresh {
                    output.extend(if self.protocol == "messages" {
                        self.event("content_block_start", json!({"index":index,"content_block":{"type":"tool_use","id":entry["id"],"name":entry["function"]["name"],"input":{}}}))
                    } else {
                        self.event("response.output_item.added", json!({"output_index":index,"item":{"type":"function_call","id":format!("fc_{}_{native_index}",self.id),"call_id":entry["id"],"name":entry["function"]["name"],"arguments":"","status":"in_progress"}}))
                    });
                }
                if !args.is_empty() {
                    output.extend(if self.protocol == "messages" {
                        self.event("content_block_delta", json!({"index":index,"delta":{"type":"input_json_delta","partial_json":args}}))
                    } else {
                        self.event("response.function_call_arguments.delta", json!({"item_id":format!("fc_{}_{native_index}",self.id),"output_index":index,"delta":args}))
                    });
                }
            }
        }
        Ok(if self.protocol.is_empty() {
            bytes.to_vec()
        } else {
            output
        })
    }
    pub fn eof(&mut self) -> Vec<u8> {
        if self.terminal || self.failed {
            return vec![];
        }
        let mut output = vec![];
        if !self.buffer.is_empty() || !self.data.is_empty() {
            match self.feed(b"\n\n") {
                Ok(events) if !self.protocol.is_empty() => output.extend(events),
                Err(_) => output.extend(self.fail()),
                _ => {}
            }
        }
        if self.terminal || self.failed {
            return output;
        }
        self.terminal = self.native_finish();
        if self.protocol.is_empty() {
            if self.terminal {
                output.extend_from_slice(b"\n\ndata: [DONE]\n\n");
            }
        } else if self.valid_finish() {
            output.extend(self.finish());
        } else {
            output.extend(self.fail());
        }
        output
    }
    fn finish(&mut self) -> Vec<u8> {
        let chat = json!({"id":self.id,"model":self.model,"created":self.created,"choices":[{"message":{"content":self.text,"tool_calls":self.tools.values().cloned().collect::<Vec<_>>()},"finish_reason":self.finish_reason}],"usage":self.usage});
        let mut completed = response(&chat, &self.protocol, &self.request);
        let mut parts = std::collections::BTreeMap::new();
        if let Some(index) = self.text_index {
            let part = if self.protocol == "messages" {
                json!({"type":"text","text":self.text})
            } else {
                json!({"id":format!("msg_{}",self.id),"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":self.text,"annotations":[]}]})
            };
            parts.insert(index, part);
        }
        for (native_index, tool) in &self.tools {
            let part = if self.protocol == "messages" {
                json!({"type":"tool_use","id":tool["id"],"name":tool["function"]["name"],"input":serde_json::from_str::<Value>(text(&tool["function"],"arguments")).unwrap()})
            } else {
                json!({"type":"function_call","id":format!("fc_{}_{native_index}",self.id),"call_id":tool["id"],"name":tool["function"]["name"],"arguments":tool["function"]["arguments"],"status":"completed"})
            };
            parts.insert(self.tool_order[native_index], part);
        }
        let mut output = vec![];
        for (index, part) in &parts {
            if self.protocol == "messages" {
                output.extend(self.event("content_block_stop", json!({"index":index})));
            } else {
                if part["type"] == "message" {
                    output.extend(self.event("response.output_text.done", json!({"item_id":part["id"],"output_index":index,"content_index":0,"text":self.text})));
                    output.extend(self.event("response.content_part.done", json!({"item_id":part["id"],"output_index":index,"content_index":0,"part":part["content"][0]})));
                } else {
                    output.extend(self.event("response.function_call_arguments.done", json!({"item_id":part["id"],"output_index":index,"arguments":part["arguments"]})));
                }
                output.extend(self.event(
                    "response.output_item.done",
                    json!({"output_index":index,"item":part}),
                ));
            }
        }
        if self.protocol == "messages" {
            completed["content"] = json!(parts.into_values().collect::<Vec<_>>());
            output.extend(self.event("message_delta", json!({"delta":{"stop_reason":completed["stop_reason"],"stop_sequence":null},"usage":completed["usage"]})));
            output.extend(self.event("message_stop", json!({})));
        } else {
            completed["output"] = json!(parts.into_values().collect::<Vec<_>>());
            output.extend(self.event(
                if self.finish_reason == "length" {
                    "response.incomplete"
                } else {
                    "response.completed"
                },
                json!({"response":completed}),
            ));
        }
        output
    }
}
