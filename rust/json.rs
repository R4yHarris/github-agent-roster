use std::collections::HashMap;

use serde_json::value::RawValue;

use crate::Error;
use crate::text::{Text, parse_json_string};

pub const MAX_JSON_BYTES: usize = 32 * 1024 * 1024;
const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Boolean(bool),
    Number(f64),
    String(Text),
    Array(Vec<Value>),
    Object(Vec<(Text, Value)>),
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new("SyntaxError", message)
}

fn parse_raw(raw: &RawValue, depth: usize) -> Result<Value, Error> {
    if depth > MAX_DEPTH {
        return Err(invalid("JSON nesting exceeds 128 levels"));
    }
    let text = raw.get();
    match text.as_bytes().first() {
        Some(b'n') => Ok(Value::Null),
        Some(b't') => Ok(Value::Boolean(true)),
        Some(b'f') => Ok(Value::Boolean(false)),
        Some(b'"') => parse_json_string(&Text::from(text).0)
            .map(Value::String)
            .map_err(|error| invalid(error.message)),
        Some(b'[') => {
            let items: Vec<&RawValue> =
                serde_json::from_str(text).map_err(|error| invalid(error.to_string()))?;
            items
                .into_iter()
                .map(|item| parse_raw(item, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Some(b'{') => parse_object(text, depth),
        _ => text
            .parse::<f64>()
            .map(Value::Number)
            .map_err(|_| invalid("Invalid JSON number")),
    }
}

fn skip_space(bytes: &[u8], position: &mut usize) {
    while bytes
        .get(*position)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    {
        *position += 1;
    }
}

fn string_end(bytes: &[u8], start: usize) -> Result<usize, Error> {
    if bytes.get(start) != Some(&b'"') {
        return Err(invalid("JSON object key must be a string"));
    }
    let mut position = start + 1;
    while let Some(&byte) = bytes.get(position) {
        if byte == b'"' {
            return Ok(position + 1);
        }
        position += if byte == b'\\' { 2 } else { 1 };
    }
    Err(invalid("Unterminated JSON object key"))
}

fn parse_object(text: &str, depth: usize) -> Result<Value, Error> {
    let bytes = text.as_bytes();
    let mut position = 1;
    let mut entries: Vec<(Text, Value)> = Vec::new();
    let mut positions = HashMap::new();
    loop {
        skip_space(bytes, &mut position);
        if bytes.get(position) == Some(&b'}') {
            return Ok(Value::Object(entries));
        }
        let end = string_end(bytes, position)?;
        let key = parse_json_string(&Text::from(&text[position..end]).0)
            .map_err(|error| invalid(error.message))?;
        position = end;
        skip_space(bytes, &mut position);
        if bytes.get(position) != Some(&b':') {
            return Err(invalid("JSON object key must be followed by a colon"));
        }
        position += 1;
        let mut stream =
            serde_json::Deserializer::from_str(&text[position..]).into_iter::<&RawValue>();
        let raw = stream
            .next()
            .ok_or_else(|| invalid("Missing JSON object value"))?
            .map_err(|error| invalid(error.to_string()))?;
        let value = parse_raw(raw, depth + 1)?;
        position += stream.byte_offset();
        if let Some(&index) = positions.get(&key) {
            let (_, prior): &mut (Text, Value) = &mut entries[index];
            *prior = value;
        } else {
            positions.insert(key.clone(), entries.len());
            entries.push((key, value));
        }
        skip_space(bytes, &mut position);
        match bytes.get(position) {
            Some(b',') => position += 1,
            Some(b'}') => return Ok(Value::Object(entries)),
            _ => return Err(invalid("JSON object needs a comma or closing brace")),
        }
    }
}

fn utf8_json_source(source: &Text) -> Result<String, Error> {
    let mut output = String::with_capacity(source.utf8_len());
    let mut quoted = false;
    let mut escaped = false;
    for unit in char::decode_utf16(source.0.iter().copied()) {
        match unit {
            Ok(character) => {
                output.push(character);
                if quoted {
                    if escaped {
                        escaped = false;
                    } else if character == '\\' {
                        escaped = true;
                    } else if character == '"' {
                        quoted = false;
                    }
                } else if character == '"' {
                    quoted = true;
                }
            }
            Err(error) if quoted && !escaped => {
                let encoded = Text(vec![error.unpaired_surrogate()]).json();
                output.push_str(&encoded[1..encoded.len() - 1]);
            }
            Err(_) => {
                return Err(invalid(
                    "Invalid Unicode outside a JSON string or in an escape",
                ));
            }
        }
    }
    Ok(output)
}

pub fn parse(source: &Text) -> Result<Value, Error> {
    if source.utf8_len() > MAX_JSON_BYTES {
        return Err(invalid("JSON input exceeds 32 MiB"));
    }
    let source = utf8_json_source(source)?;
    let raw: &RawValue =
        serde_json::from_str(&source).map_err(|error| invalid(error.to_string()))?;
    parse_raw(raw, 0)
}

fn array_index(key: &Text) -> Option<u32> {
    if key.0.is_empty() || key.0.len() > 10 || key.0.len() > 1 && key.0.first() == Some(&48) {
        return None;
    }
    let mut result = 0_u32;
    for &unit in &key.0 {
        if !(48..=57).contains(&unit) {
            return None;
        }
        result = result.checked_mul(10)?.checked_add((unit - 48) as u32)?;
    }
    (result != u32::MAX).then_some(result)
}

impl Value {
    pub fn string(value: &str) -> Self {
        Self::String(Text::from(value))
    }

    pub fn object(entries: Vec<(&str, Value)>) -> Self {
        Self::Object(
            entries
                .into_iter()
                .map(|(name, value)| (Text::from(name), value))
                .collect(),
        )
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        let Self::Object(entries) = self else {
            return None;
        };
        entries
            .iter()
            .find(|(name, _)| name.0.iter().copied().eq(key.encode_utf16()))
            .map(|(_, value)| value)
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&Text> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn json(&self) -> String {
        self.render(false, 0)
    }

    pub fn pretty(&self) -> String {
        self.render(true, 0)
    }

    fn render(&self, pretty: bool, depth: usize) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(value) if !value.is_finite() => "null".to_owned(),
            Self::Number(value) => ryu_js::Buffer::new().format_finite(*value).to_owned(),
            Self::String(value) => value.json(),
            Self::Array(values) => {
                if values.is_empty() {
                    return "[]".to_owned();
                }
                let body = values
                    .iter()
                    .map(|value| value.render(pretty, depth + 1))
                    .collect::<Vec<_>>();
                if pretty {
                    let indent = "  ".repeat(depth + 1);
                    format!(
                        "[\n{indent}{}\n{}]",
                        body.join(&format!(",\n{indent}")),
                        "  ".repeat(depth)
                    )
                } else {
                    format!("[{}]", body.join(","))
                }
            }
            Self::Object(entries) => {
                let mut ordered = entries.iter().enumerate().collect::<Vec<_>>();
                ordered.sort_by_key(|(position, (key, _))| match array_index(key) {
                    Some(index) => (0, index as usize),
                    None => (1, *position),
                });
                if entries.is_empty() {
                    return "{}".to_owned();
                }
                let body = ordered
                    .into_iter()
                    .map(|(_, (key, value))| {
                        format!(
                            "{}:{}{}",
                            key.json(),
                            if pretty { " " } else { "" },
                            value.render(pretty, depth + 1)
                        )
                    })
                    .collect::<Vec<_>>();
                if pretty {
                    let indent = "  ".repeat(depth + 1);
                    format!(
                        "{{\n{indent}{}\n{}}}",
                        body.join(&format!(",\n{indent}")),
                        "  ".repeat(depth)
                    )
                } else {
                    format!("{{{}}}", body.join(","))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_raw_surrogates_and_numeric_semantics_without_an_ad_hoc_json_parser()
    -> Result<(), Error> {
        let value = parse(&Text::from("{\"\\ud800\":\"\\udfff\",\"number\":1e400}"))?;
        assert!(
            value
                .get("number")
                .and_then(Value::as_number)
                .is_some_and(f64::is_infinite)
        );
        assert_eq!(value.json(), "{\"\\ud800\":\"\\udfff\",\"number\":null}");
        Ok(())
    }

    #[test]
    fn preserves_duplicate_overwrite_order_and_ecmascript_number_formatting() -> Result<(), Error> {
        let value = parse(&Text::from(
            "{\"z\":1,\"2\":2,\"1\":3,\"z\":4,\"small\":1e-7,\"large\":1e20}",
        ))?;
        assert_eq!(
            value.json(),
            "{\"1\":3,\"2\":2,\"z\":4,\"small\":1e-7,\"large\":100000000000000000000}"
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_json_and_preserves_literal_surrogates_only_in_strings() -> Result<(), Error>
    {
        let source = Text(vec![34, 0xd800, 34]);
        assert_eq!(parse(&source)?.json(), "\"\\ud800\"");
        assert!(parse(&Text(vec![0xd800])).is_err());
        for source in [
            "",
            "{\"a\":}",
            "[1,]",
            "NaN",
            "01",
            "\"\\x00\"",
            "{\"a\":1,}",
        ] {
            assert!(parse(&Text::from(source)).is_err(), "{source}");
        }
        Ok(())
    }
}
