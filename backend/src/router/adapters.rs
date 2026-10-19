use super::*;
use axum::Json;
use proxy::Exchange;
fn ct(headers: &HeaderMap) -> &str {
    headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
fn form(fields: &[(&str, Vec<u8>, Option<String>, Option<String>)]) -> (Vec<u8>, String) {
    let boundary = format!("v-scheduler-{}", uuid::Uuid::new_v4().simple());
    let mut bytes = vec![];
    for (name, data, file, mime) in fields {
        bytes.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"{}\r\n",
                file.as_ref()
                    .map(|v| format!("; filename=\"{}\"", v.replace(['"', '\r', '\n'], "")))
                    .unwrap_or_default()
            )
            .as_bytes(),
        );
        if let Some(mime) = mime {
            bytes.extend_from_slice(format!("Content-Type: {mime}\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(data);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (bytes, format!("multipart/form-data; boundary={boundary}"))
}
pub(super) async fn json_request(
    exchange: &Exchange,
    method: axum::http::Method,
    url: String,
    headers: &HeaderMap,
    body: Option<Value>,
) -> Api<(StatusCode, Value)> {
    let mut builder = proxy::outgoing(
        &exchange.g,
        &exchange.u,
        method,
        url,
        headers,
        exchange.lease.as_ref(),
    );
    if exchange.u.source == "image-studio"
        && exchange.u.base_url == "http://image-studio-backend:5000"
    {
        if let Some(auth) = headers.get("authorization") {
            builder = builder.header("authorization", auth);
        }
    }
    if let Some(body) = body {
        builder = builder.json(&body);
    }
    if let Some(lease) = exchange.lease.as_ref() {
        lease.begin();
    }
    let response = lease::guard(exchange.lease.as_ref(), async {
        builder
            .send()
            .await
            .map_err(|_| Failure::new(502, "upstream connection failed"))
    })
    .await?;
    let status = response.status();
    let bytes = lease::guard(exchange.lease.as_ref(), read_limited(response, 32 << 20)).await?;
    if bytes.len() > 32 << 20 {
        return Err(Failure::new(502, "upstream response too large"));
    }
    if !status.is_success() {
        return Err(Failure::new(status.as_u16(), "upstream request failed"));
    }
    let value = serde_json::from_slice(&bytes)
        .map_err(|_| Failure::new(502, "invalid upstream JSON response"))?;
    Ok((status, value))
}
pub(super) async fn forward(
    g: Arc<Gateway>,
    mut exchange: Exchange,
    method: axum::http::Method,
    headers: HeaderMap,
    path: String,
    bytes: Vec<u8>,
    input: Value,
) -> Api<Response> {
    match exchange.u.protocol.as_str() {
        "offline_asr" => {
            if path != "/audio/transcriptions" {
                return Err(Failure::new(
                    400,
                    "offline_asr only supports audio/transcriptions",
                ));
            }
            let parts = proxy::multipart(&bytes, ct(&headers))?;
            let files = parts
                .iter()
                .filter(|p| ["file", "audio"].contains(&p.name.as_str()))
                .collect::<Vec<_>>();
            if files.len() != 1 || files[0].start == files[0].end {
                return Err(Failure::new(400, "multipart body requires one audio file"));
            }
            let language = parts
                .iter()
                .find(|p| ["language", "lang"].contains(&p.name.as_str()))
                .map(|p| String::from_utf8_lossy(&bytes[p.start..p.end]).into_owned())
                .unwrap_or_default();
            let mut url = reqwest::Url::parse(&format!(
                "{}/transcribe",
                exchange.u.base_url.trim_end_matches('/')
            ))
            .map_err(|_| Failure::new(502, "invalid upstream URL"))?;
            if !language.is_empty() {
                url.query_pairs_mut().append_pair("lang", &language);
            }
            if let Some(lease) = exchange.lease.as_ref() {
                lease.begin();
            }
            let response = proxy::outgoing(
                &g,
                &exchange.u,
                method,
                url.to_string(),
                &headers,
                exchange.lease.as_ref(),
            )
            .header("content-type", "application/octet-stream")
            .body(bytes[files[0].start..files[0].end].to_vec())
            .send()
            .await
            .map_err(|_| Failure::new(502, "upstream connection failed"))?;
            exchange.status = response.status().as_u16();
            let raw = read_limited(response, 32 << 20).await?;
            if exchange.status >= 300 {
                return Err(Failure::new(exchange.status, "offline ASR request failed"));
            }
            let value = serde_json::from_slice::<Value>(&raw)
                .unwrap_or(json!({"text":String::from_utf8_lossy(&raw)}));
            let out = if value["data"].is_object() {
                let data = &value["data"];
                let mut out = json!({"text":data["full_text"].as_str().unwrap_or("")});
                for key in ["segments", "speakers", "stats"] {
                    if let Some(v) = data.get(key) {
                        out[key] = v.clone();
                    }
                }
                out
            } else if value["text"].is_string() {
                json!({"text":value["text"]})
            } else {
                json!({"text":"","upstream":value})
            };
            if let Some(lease) = exchange.lease.as_mut() {
                lease.complete("synchronous_response_completed");
                lease.release_confirmed("synchronous_response_completed").await;
            }
            Ok(Json(out).into_response())
        }
        "ml_server" => ml(g, exchange, method, headers, path, bytes, input).await,
        "image_studio" => image(g, exchange, headers, path, bytes, input).await,
        "async_task" => async_task(exchange, headers, path, input).await,
        _ => Err(Failure::new(400, "unsupported model adapter")),
    }
}
async fn ml(
    g: Arc<Gateway>,
    mut e: Exchange,
    method: axum::http::Method,
    headers: HeaderMap,
    path: String,
    bytes: Vec<u8>,
    mut input: Value,
) -> Api<Response> {
    if path == "/embeddings" {
        let inputs = if let Some(text) = input["input"].as_str() {
            if text.is_empty() {
                return Err(Failure::new(400, "input must not be empty"));
            }
            vec![text.to_string()]
        } else {
            let rows = strings(&input["input"]);
            if rows.is_empty()
                || input["input"]
                    .as_array()
                    .is_none_or(|a| a.len() != rows.len())
            {
                return Err(Failure::new(
                    400,
                    "input must be text or a non-empty text array",
                ));
            }
            rows
        };
        let (field, entries) =
            if e.native.to_ascii_lowercase().contains("siglip") || e.native.starts_with("ViT-") {
                ("clip", json!({"clip":{"textual":{"modelName":e.native}}}))
            } else {
                (
                    "qwen3-embedding",
                    json!({"qwen3-embedding":{"modelName":e.native}}),
                )
            };
        let mut rows = vec![];
        for (index, text) in inputs.iter().enumerate() {
            let (body, mime) = form(&[
                ("entries", serde_json::to_vec(&entries).unwrap(), None, None),
                ("text", text.as_bytes().to_vec(), None, None),
            ]);
            if let Some(lease) = e.lease.as_ref() {
                lease.begin();
            }
            let response = proxy::outgoing(
                &g,
                &e.u,
                axum::http::Method::POST,
                format!("{}/predict", e.u.base_url.trim_end_matches('/')),
                &headers,
                e.lease.as_ref(),
            )
            .header("content-type", mime)
            .body(body)
            .send()
            .await
            .map_err(|_| Failure::new(502, "ML Server unavailable"))?;
            if !response.status().is_success() {
                return Err(Failure::new(502, "ML Server embedding failed"));
            }
            let value: Value = read_json(response, 32 << 20).await?;
            let vector: Value = serde_json::from_str(text_value(&value, field)?)
                .map_err(|_| Failure::new(502, "invalid embedding vector"))?;
            if !vector.is_array() {
                return Err(Failure::new(502, "invalid embedding vector"));
            }
            rows.push(json!({"object":"embedding","index":index,"embedding":vector}));
        }
        e.status = 200;
        if let Some(lease) = e.lease.as_mut() {
            lease.complete("synchronous_response_completed");
            lease.release_confirmed("synchronous_response_completed").await;
        }
        return Ok(Json(json!({"object":"list","model":e.native,"data":rows})).into_response());
    }
    if path == "/ml/predictions" {
        let parts = proxy::multipart(&bytes, ct(&headers))?;
        let value = |name: &str| {
            parts
                .iter()
                .find(|p| p.name == name)
                .map(|p| String::from_utf8_lossy(&bytes[p.start..p.end]).into_owned())
                .unwrap_or_default()
        };
        let file = parts
            .iter()
            .find(|p| p.name == "image")
            .ok_or_else(|| Failure::new(400, "image is required"))?;
        let min = value("min_score");
        let mut options = json!({});
        if !min.is_empty() {
            let n = min
                .parse::<f64>()
                .map_err(|_| Failure::new(400, "invalid min_score"))?;
            if !(0.0..=1.0).contains(&n) {
                return Err(Failure::new(400, "min_score must be 0..1"));
            }
            options["minScore"] = json!(n);
        }
        let model = &e.native;
        let entries = if model.to_ascii_lowercase().contains("siglip") || model.starts_with("ViT-")
        {
            json!({"clip":{"visual":{"modelName":model}}})
        } else if model.starts_with("buffalo_") || model == "antelopev2" {
            json!({"facial-recognition":{"detection":{"modelName":model,"options":options},"recognition":{"modelName":model}}})
        } else if model.starts_with("osnet_") {
            json!({"person-reid":{"modelName":model}})
        } else if model.contains("OCR") {
            let recognition = options.clone();
            let resolution = value("max_resolution");
            if !resolution.is_empty() {
                let n = resolution
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| Failure::new(400, "max_resolution must be positive"))?;
                options["maxResolution"] = json!(n);
            }
            let mode = value("score_mode");
            if !mode.is_empty() {
                if !["fast", "slow"].contains(&mode.as_str()) {
                    return Err(Failure::new(400, "score_mode must be fast or slow"));
                }
                options["scoreMode"] = json!(mode);
            }
            json!({"ocr":{"detection":{"modelName":model,"options":options},"recognition":{"modelName":model,"options":recognition}}})
        } else {
            return Err(Failure::new(
                400,
                "selected model does not support image prediction",
            ));
        };
        let (body, mime) = form(&[
            ("entries", serde_json::to_vec(&entries).unwrap(), None, None),
            (
                "image",
                bytes[file.start..file.end].to_vec(),
                Some("image".into()),
                Some("application/octet-stream".into()),
            ),
        ]);
        if let Some(lease) = e.lease.as_ref() {
            lease.begin();
        }
        let response = proxy::outgoing(
            &g,
            &e.u,
            method,
            format!("{}/predict", e.u.base_url.trim_end_matches('/')),
            &headers,
            e.lease.as_ref(),
        )
        .header("content-type", mime)
        .body(body)
        .send()
        .await
        .map_err(|_| Failure::new(502, "ML Server unavailable"))?;
        return proxy::finish_response(e, response, "").await;
    }
    if path.starts_with("/ml/video/structure") {
        let target = format!(
            "{}/api/v1/video/structure{}",
            e.u.base_url.trim_end_matches('/'),
            path.trim_start_matches("/ml/video/structure")
        );
        if method == axum::http::Method::GET {
            if let Some(response) = jobs::cached(&g, &e.id, &e.u, &target)? {
                e.status = response.status().as_u16();
                return Ok(response);
            }
        }
        if let Some(map) = input.as_object_mut() {
            map.remove("model");
        }
        if method == axum::http::Method::POST {
            e.async_job = jobs::prepare(&e, target.clone(), &headers)?;
        }
        let mut builder =
            proxy::outgoing(&g, &e.u, method.clone(), target, &headers, e.lease.as_ref());
        if method == axum::http::Method::POST {
            builder = builder.json(&input);
        }
        if let Some(lease) = e.lease.as_ref() {
            lease.begin();
        }
        let response = builder
            .send()
            .await
            .map_err(|_| Failure::new(502, "ML Server unavailable"))?;
        return proxy::finish_response(e, response, "").await;
    }
    Err(Failure::new(400, "unsupported ML Server endpoint"))
}
fn text_value<'a>(v: &'a Value, key: &str) -> Api<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| Failure::new(502, "missing upstream response field"))
}
async fn image(
    g: Arc<Gateway>,
    mut e: Exchange,
    headers: HeaderMap,
    path: String,
    bytes: Vec<u8>,
    input: Value,
) -> Api<Response> {
    if ![
        "/images/generations",
        "/images/edits",
        "/videos/generations",
    ]
    .contains(&path.as_str())
    {
        return Err(Failure::new(400, "unsupported Image Studio endpoint"));
    }
    let (model, variant) = e.native.split_once('@').unwrap_or((&e.native, ""));
    let mut payload = json!({"model_key":model});
    if !variant.is_empty() {
        payload["model_variant"] = json!(variant);
    }
    let base = e.u.base_url.trim_end_matches('/');
    let api = if base.ends_with("/api/v1") {
        base.to_string()
    } else {
        format!("{base}/api/v1")
    };
    if ct(&headers).starts_with("multipart/") {
        let parts = proxy::multipart(&bytes, ct(&headers))?;
        let files = parts
            .iter()
            .filter(|p| ["image", "file"].contains(&p.name.as_str()))
            .collect::<Vec<_>>();
        if files.len() > 1 {
            return Err(Failure::new(400, "duplicate image field"));
        }
        for part in &parts {
            let raw = String::from_utf8_lossy(&bytes[part.start..part.end]);
            match part.name.as_str() {
                "prompt" | "negative_prompt" | "model_variant" | "sampler" => {
                    payload[&part.name] = json!(raw)
                }
                "width" | "height" | "steps" | "seed" | "video_frames" | "fps" | "batch_count" => {
                    if let Ok(n) = raw.parse::<i64>() {
                        payload[&part.name] = json!(n);
                    }
                }
                "cfg_scale" | "strength" => {
                    if let Ok(n) = raw.parse::<f64>() {
                        if n.is_finite() {
                            payload[&part.name] = json!(n);
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(file) = files.first() {
            let (body, mime) = form(&[(
                "file",
                bytes[file.start..file.end].to_vec(),
                Some("input.png".into()),
                Some("application/octet-stream".into()),
            )]);
            let mut builder = proxy::outgoing(
                &g,
                &e.u,
                axum::http::Method::POST,
                format!("{api}/inputs"),
                &headers,
                None,
            )
            .header("content-type", mime)
            .body(body);
            if e.u.source == "image-studio" {
                if let Some(auth) = headers.get("authorization") {
                    builder = builder.header("authorization", auth);
                }
            }
            let response = builder
                .send()
                .await
                .map_err(|_| Failure::new(502, "Image Studio input upload failed"))?;
            if !response.status().is_success() {
                return Err(Failure::new(502, "Image Studio input upload rejected"));
            }
            let doc: Value = read_json(response, 1 << 20).await?;
            if text(&doc, "input_id").is_empty() {
                return Err(Failure::new(502, "input upload did not return an id"));
            }
            payload["init_input_id"] = doc["input_id"].clone();
        } else if path == "/images/edits" {
            return Err(Failure::new(400, "image edit requires image file"));
        }
    } else {
        for key in [
            "prompt",
            "negative_prompt",
            "model_variant",
            "steps",
            "cfg_scale",
            "strength",
            "seed",
            "video_frames",
            "fps",
            "width",
            "height",
        ] {
            if let Some(v) = input.get(key) {
                payload[key] = v.clone();
            }
        }
        if let Some(size) = input["size"].as_str() {
            if let Some((w, h)) = size.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.parse::<u64>(), h.parse::<u64>()) {
                    payload["width"] = json!(w);
                    payload["height"] = json!(h);
                }
            }
        }
        if let Some(n) = input.get("n") {
            payload["batch_count"] = n.clone();
        }
    }
    if path == "/videos/generations" {
        payload["batch_count"] = json!(1);
    }
    if text(&payload, "prompt").trim().is_empty() {
        return Err(Failure::new(400, "prompt is required"));
    }
    let journal = jobs::prepare(&e, format!("{api}/generate"), &headers)?;
    let (status, job) = json_request(
        &e,
        axum::http::Method::POST,
        format!("{api}/generate"),
        &headers,
        Some(payload),
    )
    .await?;
    e.status = status.as_u16();
    jobs::receipt(&mut e, journal, &job).await?;
    Ok((StatusCode::ACCEPTED,Json(json!({"id":job["job_id"],"object":"image_studio.generation.job","status":job["status"],"model":e.native,"image_studio_job":job}))).into_response())
}
fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    path.split('.')
        .filter(|s| !s.is_empty())
        .fold(v, |v, k| &v[k])
}
fn set(v: &mut Value, path: &str, value: Value) {
    let mut keys = path.split('.').peekable();
    let mut target = v;
    while let Some(key) = keys.next() {
        if keys.peek().is_none() {
            target[key] = value;
            break;
        }
        if !target[key].is_object() {
            target[key] = json!({});
        }
        target = &mut target[key];
    }
}
async fn async_task(
    mut e: Exchange,
    headers: HeaderMap,
    path: String,
    input: Value,
) -> Api<Response> {
    if path != "/images/generations" {
        return Err(Failure::new(
            400,
            "async_task only supports images/generations",
        ));
    }
    let contract =
        e.u.async_task
            .clone()
            .ok_or_else(|| Failure::new(502, "async task configuration is missing"))?;
    if text(&input, "prompt").trim().is_empty() {
        return Err(Failure::new(400, "prompt is required"));
    }
    let mut task = contract["default_input"]
        .as_object()
        .cloned()
        .map(Value::Object)
        .unwrap_or(json!({}));
    task["prompt"] = input["prompt"].clone();
    let mut payload = json!({"model":e.native});
    if text(&contract, "input_path").is_empty() {
        payload
            .as_object_mut()
            .unwrap()
            .extend(task.as_object().unwrap().clone());
    } else {
        set(&mut payload, text(&contract, "input_path"), task);
    }
    let base = e.u.base_url.trim_end_matches('/').to_string();
    let create = format!(
        "{base}/{}",
        text(&contract, "create_path").trim_start_matches('/')
    );
    let journal = jobs::prepare(&e, create.clone(), &headers)?;
    let (_, created) = json_request(
        &e,
        axum::http::Method::POST,
        create,
        &headers,
        Some(payload),
    )
    .await?;
    let task_id = at(&created, text(&contract, "task_id_path"))
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Failure::new(502, "task id missing in upstream response"))?
        .to_string();
    jobs::receipt(&mut e, journal, &created).await?;
    let mut query = reqwest::Url::parse(&format!(
        "{base}/{}",
        text(&contract, "query_path").trim_start_matches('/')
    ))
    .map_err(|_| Failure::new(502, "invalid async query URL"))?;
    query.query_pairs_mut().append_pair(
        if text(&contract, "query_task_param").is_empty() {
            "taskId"
        } else {
            text(&contract, "query_task_param")
        },
        &task_id,
    );
    let deadline =
        Instant::now() + Duration::from_secs(contract["timeout_seconds"].as_u64().unwrap_or(600));
    let interval = Duration::from_millis(
        contract["poll_interval_ms"]
            .as_u64()
            .unwrap_or(1000)
            .max(50),
    );
    loop {
        if Instant::now() > deadline {
            return Err(Failure::new(504, "async task timed out"));
        }
        tokio::time::sleep(interval).await;
        let (_, row) = json_request(
            &e,
            axum::http::Method::GET,
            query.to_string(),
            &headers,
            None,
        )
        .await?;
        let state = at(&row, text(&contract, "status_path"))
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase();
        if strings(&contract["failure_states"])
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&state))
        {
            return Err(Failure::new(502, "upstream task failed"));
        }
        if !strings(&contract["success_states"])
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&state))
        {
            continue;
        }
        let mut result = at(&row, text(&contract, "result_path")).clone();
        if let Some(encoded) = result.as_str() {
            if !text(&contract, "result_json_path").is_empty() {
                let decoded: Value = serde_json::from_str(encoded)
                    .map_err(|_| Failure::new(502, "invalid JSON-encoded task result"))?;
                result = at(&decoded, text(&contract, "result_json_path")).clone();
            }
        }
        let results = if let Some(url) = result.as_str() {
            vec![json!({"url":url})]
        } else {
            result
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().or(v["url"].as_str()).map(|s| json!({"url":s})))
                .collect::<Vec<_>>()
        };
        if results.is_empty() {
            return Err(Failure::new(502, "async result contains no URLs"));
        }
        e.status = 200;
        return Ok(Json(json!({"created":Utc::now().timestamp(),"data":results})).into_response());
    }
}
use std::time::Instant;
