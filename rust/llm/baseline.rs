use crate::Error;
use crate::domain;
use crate::json::Value;
use crate::text::{Text, trim};

pub fn normalize(source: &Text) -> Result<Text, Error> {
    let source = source.normalized_lines();
    let source = trim(&source.0);
    if !(1..=8192).contains(&source.len()) {
        return Err(Error::new(
            "Error",
            "Engineering baseline must contain 1-8192 characters",
        ));
    }
    Ok(Text(source.to_vec()))
}

pub fn read() -> Result<Text, Error> {
    normalize(&Text::from(include_str!("../../principals/engineering.md")))
}

pub fn with_engineering_baseline(messages: &Value) -> Result<Value, Error> {
    with_baseline(messages, &read()?)
}

fn with_baseline(messages: &Value, baseline: &Text) -> Result<Value, Error> {
    let messages = domain::items(messages)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| Error::type_error("Engineering baseline requires nonempty chat messages"))?;
    let mut result = messages.to_vec();
    if let Some(first) = result.first_mut().filter(|first| {
        domain::is_text(first.get("role"), "system")
            && first.get("content").and_then(Value::as_string).is_some()
    }) {
        let content = first
            .get("content")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("System message requires content"))?;
        if content
            .0
            .windows(baseline.0.len())
            .any(|part| part == baseline.0)
        {
            return Ok(Value::Array(result));
        }
        let mut content = content.clone();
        content.push_str("\n\n");
        content.0.extend_from_slice(&baseline.0);
        domain::put(first, "content", Value::String(content))?;
    } else {
        result.insert(
            0,
            Value::object(vec![
                ("role", Value::string("system")),
                ("content", Value::String(baseline.clone())),
            ]),
        );
    }
    Ok(Value::Array(result))
}
