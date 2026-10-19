use crate::domain;
use crate::json::Value;

pub const CONTEXT_FIELDS: [&str; 6] = [
    "max_model_len",
    "max_context_length",
    "max_context_len",
    "context_length",
    "context_window",
    "context_max",
];

pub fn reported_context_max(entry: &Value) -> Option<u64> {
    CONTEXT_FIELDS
        .iter()
        .filter_map(|field| domain::safe_integer(entry.get(field)))
        .find(|value| *value > 0)
}

pub fn context_max_for_model(models: &Value, model: &Value, fallback: &Value) -> Value {
    domain::items(models)
        .and_then(|models| {
            models
                .iter()
                .find(|candidate| candidate.get("id") == Some(model))
        })
        .and_then(|entry| domain::safe_integer(entry.get("context_max")))
        .filter(|value| *value > 0)
        .map_or_else(|| fallback.clone(), |value| Value::Number(value as f64))
}
