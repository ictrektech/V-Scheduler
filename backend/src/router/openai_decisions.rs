use super::*;

pub(super) fn request(value: &Value) -> Api<Value> {
    if ["assets", "images"].iter().any(|key| value.get(key).is_some()) {
        return Err(Failure::new(400, "use /v1/decisions for media input"));
    }
    let state = value.get("state").filter(|state| !state.is_null())
        .ok_or_else(|| Failure::new(400, "decision state is required"))?;
    let input = match state {
        Value::String(text) => text.clone(),
        _ => state.to_string(),
    };
    let questions = value["questions"].as_object().filter(|questions| !questions.is_empty())
        .ok_or_else(|| Failure::new(400, "decision questions are required"))?;
    let mut converted = Vec::with_capacity(questions.len());
    for (name, question) in questions {
        let instructions = question["instructions"].as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| Failure::new(400, "text decision instructions are required"))?;
        let criteria = question["criteria"].as_object()
            .filter(|criteria| criteria.len() >= 2)
            .ok_or_else(|| Failure::new(400, "choice requires at least two criteria"))?;
        if question["type"] != "choice" {
            return Err(Failure::new(400, "use /v1/decisions for question types other than choice"));
        }
        let choices = criteria.iter().map(|(choice, description)| {
            let description = description.as_str()
                .ok_or_else(|| Failure::new(400, "choice descriptions must be text"))?;
            Ok(json!({"value":choice,"description":description}))
        }).collect::<Api<Vec<_>>>()?;
        converted.push(json!({"type":"choice","name":name,"instructions":instructions,"choices":choices}));
    }
    Ok(json!({"model":value["model"],"input":input,"questions":converted}))
}

pub(super) fn systemone_response(mut value: Value) -> Api<Value> {
    let answers = value["answers"].as_array()
        .ok_or_else(|| Failure::new(502, "OpenAI Decisions returned invalid answers"))?;
    let mut by_name = serde_json::Map::new();
    for answer in answers {
        let name = text(answer, "name");
        if name.is_empty() || by_name.insert(name.into(), answer.clone()).is_some() {
            return Err(Failure::new(502, "OpenAI Decisions returned missing or duplicate answer names"));
        }
        if answer["type"] != "choice" || text(answer, "choice").is_empty()
            || !answer["confidence"].as_f64().is_some_and(|v| (0.0..=1.0).contains(&v))
        {
            return Err(Failure::new(502, "OpenAI Decisions did not return a valid choice"));
        }
    }
    value["answers"] = Value::Object(by_name);
    Ok(value)
}
