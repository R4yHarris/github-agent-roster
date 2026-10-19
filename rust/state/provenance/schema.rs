use crate::Error;
use crate::domain;
use crate::json::Value;
use crate::text::{Text, ascii, trim};

pub const SCHEMA_VERSION: &str = "1.0.0";
pub const RECORD_VERSION: u32 = 1;
pub const KNOWN_METRICS: [&str; 6] = [
    "duration_ms",
    "cost_usd",
    "tokens_prompt",
    "tokens_completion",
    "tool_calls",
    "retry_count",
];

fn text(value: Option<&Value>) -> Value {
    value
        .and_then(Value::as_string)
        .cloned()
        .map_or_else(|| Value::string(""), Value::String)
}

fn id(value: Option<&Value>) -> Value {
    let value = match value {
        Some(Value::Object(_)) | Some(Value::Array(_)) => value.and_then(|value| value.get("id")),
        value => value,
    };
    value
        .and_then(Value::as_string)
        .filter(|text| !text.0.is_empty())
        .cloned()
        .map_or_else(|| Value::string("unknown"), Value::String)
}

pub fn create_record(input: &Value, now: Value) -> Result<Value, Error> {
    if input == &Value::Null {
        return Err(Error::type_error(
            "Cannot read properties of null (reading 'run')",
        ));
    }
    let metrics = input.get("metrics");
    let mut normalized = Value::Object(Vec::new());
    for name in KNOWN_METRICS {
        let value = metrics
            .and_then(|metrics| metrics.get(name))
            .and_then(Value::as_number)
            .filter(|number| number.is_finite())
            .map_or_else(|| Value::string("unknown"), Value::Number);
        domain::put(&mut normalized, name, value)?;
    }
    if let Some(metrics) =
        metrics.filter(|metrics| matches!(metrics, Value::Object(_) | Value::Array(_)))
    {
        if let Value::Object(entries) = domain::spread(metrics) {
            for (name, _) in entries {
                if !KNOWN_METRICS.iter().any(|known| ascii(&name.0, known)) {
                    let Value::Object(ref mut normalized) = normalized else {
                        return Err(Error::type_error("Metrics require an object"));
                    };
                    normalized.push((name, Value::string("unknown")));
                }
            }
        }
    }
    let fields = |source: &str, names: &[&str]| {
        Value::object(
            names
                .iter()
                .map(|name| {
                    (
                        *name,
                        text(input.get(source).and_then(|value| value.get(name))),
                    )
                })
                .collect(),
        )
    };
    Ok(Value::object(vec![
        ("schemaVersion", Value::string(SCHEMA_VERSION)),
        ("recordType", Value::string("provenance")),
        ("createdAt", now),
        ("runId", id(input.get("run"))),
        ("sessionId", id(input.get("session"))),
        ("repository", fields("repository", &["remote", "commit"])),
        ("issue", fields("issue", &["issue", "task"])),
        ("seat", fields("seat", &["name"])),
        ("route", fields("route", &["name"])),
        ("requestedModel", text(input.get("requestedModel"))),
        ("servedModel", text(input.get("servedModel"))),
        (
            "startedAt",
            input.get("startedAt").cloned().unwrap_or(Value::Null),
        ),
        (
            "endedAt",
            input.get("endedAt").cloned().unwrap_or(Value::Null),
        ),
        ("outcome", text(input.get("outcome"))),
        ("tools", fields("tools", &["name", "version"])),
        ("metrics", normalized),
        (
            "evidence",
            input
                .get("evidence")
                .filter(|value| matches!(value, Value::Object(_) | Value::Array(_)))
                .cloned()
                .unwrap_or(Value::Object(Vec::new())),
        ),
    ]))
}

fn forbidden(name: &Text) -> bool {
    let name = String::from_utf16_lossy(&name.0)
        .replace(['_', '-'], "")
        .to_lowercase();
    matches!(
        name.as_str(),
        "prompt"
            | "prompts"
            | "promptbody"
            | "promptbodies"
            | "promptsnapshot"
            | "promptsnapshots"
            | "rawprompt"
            | "systemprompt"
            | "userprompt"
            | "source"
            | "sources"
            | "sourcebody"
            | "sourcebodies"
            | "sourcesnapshot"
            | "sourcesnapshots"
            | "rawsource"
            | "sourcecode"
            | "filecontents"
            | "messages"
            | "transcript"
            | "transcripts"
    )
}

pub fn validate_data(value: &Value, source: &str) -> Result<(), Error> {
    validate_nested(value, source, 0)
}

fn validate_nested(value: &Value, source: &str, depth: usize) -> Result<(), Error> {
    if depth > 128 {
        return Err(Error::type_error(format!(
            "{source} exceeds 128 JSON levels"
        )));
    }
    match value {
        Value::Number(value) if !value.is_finite() => Err(Error::type_error(format!(
            "{source} must contain only JSON data (finite numbers, strings, booleans, null, arrays, and plain objects)"
        ))),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_nested(item, &format!("{source}.{index}"), depth + 1)?;
            }
            Ok(())
        }
        Value::Object(entries) => {
            for (name, nested) in entries {
                let field = format!("{source}.{}", String::from_utf16_lossy(&name.0));
                if forbidden(name) {
                    return Err(Error::type_error(format!(
                        "{field} is a prompt/source body snapshot; use a summary or reference instead"
                    )));
                }
                validate_nested(nested, &field, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn semver(version: &str) -> bool {
    let (main, build) = version
        .split_once('+')
        .map_or((version, None), |(main, build)| (main, Some(build)));
    let identifier = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    };
    if build.is_some_and(|build| !build.split('.').all(identifier)) {
        return false;
    }
    let (numbers, pre) = main
        .split_once('-')
        .map_or((main, None), |(main, pre)| (main, Some(pre)));
    if pre.is_some_and(|pre| {
        !pre.split('.').all(|part| {
            identifier(part)
                && (!part.bytes().all(|byte| byte.is_ascii_digit())
                    || part.len() == 1
                    || !part.starts_with('0'))
        })
    }) {
        return false;
    }
    let numbers = numbers.split('.').collect::<Vec<_>>();
    numbers.len() == 3
        && numbers.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    pub reason: Option<String>,
}

impl Validation {
    pub fn json(&self, value: &Value) -> Result<String, Error> {
        if let Some(reason) = &self.reason {
            return Ok(Value::object(vec![
                ("ok", Value::Boolean(false)),
                ("reason", Value::string(reason)),
            ])
            .json());
        }
        let mut body = value.clone();
        domain::remove(&mut body, "id")?;
        domain::remove(&mut body, "version")?;
        Ok(Value::object(vec![
            ("ok", Value::Boolean(true)),
            ("record", value.clone()),
            ("body", body),
        ])
        .json())
    }
}

pub fn validate_record(value: &Value) -> Validation {
    let reason = || -> Option<String> {
        if !matches!(value, Value::Object(_)) {
            return Some("record must be a JSON object".to_owned());
        }
        let Some(id) = value
            .get("id")
            .and_then(Value::as_string)
            .filter(|id| !trim(&id.0).is_empty())
        else {
            return Some("record.id must be a non-empty string".to_owned());
        };
        if id.0.len() > 256 || id.0.iter().any(|unit| matches!(*unit, 0 | 10 | 13)) {
            return Some("record.id is not a safe identifier".to_owned());
        }
        if value.get("version").and_then(Value::as_number) != Some(RECORD_VERSION as f64) {
            return Some(format!(
                "incompatible record version {} (expected {RECORD_VERSION})",
                value
                    .get("version")
                    .map_or_else(|| "undefined".to_owned(), Value::json)
            ));
        }
        if value.get("schemaVersion").is_some()
            || domain::is_text(value.get("recordType"), "provenance")
        {
            let Some(version) = value
                .get("schemaVersion")
                .and_then(Value::as_string)
                .and_then(|text| text.to_utf8().ok())
                .filter(|version| semver(version))
            else {
                return Some("record.schemaVersion must be a semver string".to_owned());
            };
            if version.split('.').next() != Some("1") {
                return Some(format!(
                    "incompatible record.schemaVersion {version} (expected major 1); migration required"
                ));
            }
        }
        validate_data(value, "record")
            .err()
            .map(|error| error.message)
    };
    Validation { reason: reason() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_cannot_promote_missing_or_named_unknowns_to_measurements() -> Result<(), Error> {
        let input = Value::object(vec![(
            "metrics",
            Value::object(vec![
                ("tokens_prompt", Value::Number(0.0)),
                ("future", Value::Number(10.0)),
            ]),
        )]);
        let record = create_record(&input, Value::Number(0.0))?;
        let metrics = record
            .get("metrics")
            .ok_or_else(|| Error::type_error("Missing metrics"))?;
        assert_eq!(metrics.get("tokens_prompt"), Some(&Value::Number(0.0)));
        assert_eq!(
            metrics.get("tokens_completion"),
            Some(&Value::string("unknown"))
        );
        assert_eq!(metrics.get("future"), Some(&Value::string("unknown")));
        Ok(())
    }

    #[test]
    fn snapshots_and_schema_drift_are_refused_recursively() {
        let value = Value::object(vec![
            ("id", Value::string("valid")),
            ("version", Value::Number(1.0)),
            ("payload", Value::object(vec![("source_body", Value::Null)])),
        ]);
        assert!(
            validate_record(&value)
                .reason
                .is_some_and(|reason| reason.contains("snapshot"))
        );
        for version in ["01.0.0", "1.0.0-01", "1.0.0+", "1.0", "1.0.0-a..b"] {
            assert!(!semver(version));
        }
        for version in ["1.0.0", "1.2.3-rc.1+fixture", "1.0.0-0"] {
            assert!(semver(version));
        }
    }
}
