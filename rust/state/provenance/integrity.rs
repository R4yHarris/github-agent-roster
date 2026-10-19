use crate::Error;
use crate::domain;
use crate::identity::sha256_hex;
use crate::json::Value;

use super::{api, schema};

pub const SCHEMA: &str = "roster.history-provenance";
pub const VERSION: u32 = 1;

pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(entries) => {
            let mut entries = entries.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.0.cmp(&right.0));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(name, value)| format!("{}:{}", name.json(), canonical_json(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        _ => value.json(),
    }
}

pub fn digest(record: &Value) -> String {
    let source = Value::object(
        [
            "id",
            "runId",
            "sessionId",
            "repoIdentity",
            "issue",
            "seat",
            "servedModel",
            "requestedModel",
            "payload",
            "outcome",
            "event",
            "createdAt",
        ]
        .iter()
        .map(|name| (*name, record.get(name).cloned().unwrap_or(Value::Null)))
        .collect(),
    );
    sha256_hex(canonical_json(&source).as_bytes())
}

pub fn create(record: &Value, overrides: &Value) -> Result<Value, Error> {
    domain::merge(
        &Value::object(vec![
            ("schema", Value::string(SCHEMA)),
            ("version", Value::Number(VERSION as f64)),
            ("algorithm", Value::string("sha256")),
            ("digest", Value::string(&digest(record))),
        ]),
        &domain::spread(overrides),
    )
}

fn failure(message: impl Into<String>) -> Value {
    Value::object(vec![
        ("ok", Value::Boolean(false)),
        ("error", Value::string(&message.into())),
    ])
}

fn success(integrity: &str) -> Value {
    Value::object(vec![
        ("ok", Value::Boolean(true)),
        ("integrity", Value::string(integrity)),
    ])
}

pub fn validate(record: &Value) -> Result<Value, Error> {
    if !matches!(record, Value::Object(_)) {
        return Ok(failure("provenance record is not an object"));
    }
    if let Some(reason) = schema::validate_record(record).reason {
        return Ok(failure(format!("stored record is invalid: {reason}")));
    }
    let mut integrity = "unbound";
    if let Some(section) = record.get("section") {
        let event = if domain::is_text(Some(section), api::RAW_HISTORY) {
            record.get("event")
        } else if domain::is_text(Some(section), api::CURATED_MEMORY) {
            None
        } else {
            return Ok(failure(format!(
                "unknown provenance section {}",
                section.json()
            )));
        };
        let memory_event = Value::string("memory");
        let event = if domain::is_text(Some(section), api::CURATED_MEMORY) {
            Some(&memory_event)
        } else {
            event
        };
        let expected = api::store_record_id_values(&[
            record.get("repoIdentity"),
            record.get("runId"),
            record.get("sessionId"),
            event,
            Some(section),
        ])?;
        if !domain::is_text(record.get("id"), &expected) {
            return Ok(failure(
                "record id does not match its derived provenance identity",
            ));
        }
        integrity = "verified";
    }
    let Some(block) = record.get("provenance") else {
        return Ok(success(integrity));
    };
    if !matches!(block, Value::Object(_)) {
        return Ok(failure("provenance block is malformed"));
    }
    if !domain::is_text(block.get("schema"), SCHEMA) {
        return Ok(failure(format!("provenance schema must be {SCHEMA}")));
    }
    if block.get("version").and_then(Value::as_number) != Some(VERSION as f64) {
        return Ok(failure(format!("provenance version must be {VERSION}")));
    }
    let expected = digest(record);
    if !["digest", "hash", "checksum", "signature"]
        .iter()
        .any(|name| domain::is_text(block.get(name), &expected))
    {
        return Ok(failure("provenance integrity check failed"));
    }
    Ok(success("verified"))
}

pub fn validate_records(records: &Value) -> Result<Value, Error> {
    let Value::Array(records) = records else {
        return Ok(failure("provenance records must be an array"));
    };
    let mut quarantine = Vec::new();
    for (index, record) in records.iter().enumerate() {
        let verdict = validate(record)?;
        if verdict.get("ok") == Some(&Value::Boolean(false)) {
            quarantine.push(Value::object(vec![
                ("index", Value::Number(index as f64)),
                (
                    "error",
                    verdict.get("error").cloned().unwrap_or(Value::Null),
                ),
            ]));
        }
    }
    let mut result = Value::object(vec![("ok", Value::Boolean(quarantine.is_empty()))]);
    if let Some(first) = quarantine.first() {
        domain::put(
            &mut result,
            "index",
            first.get("index").cloned().unwrap_or(Value::Null),
        )?;
        domain::put(
            &mut result,
            "error",
            first.get("error").cloned().unwrap_or(Value::Null),
        )?;
    }
    domain::put(&mut result, "quarantine", Value::Array(quarantine))?;
    Ok(result)
}

pub fn scan_malformed(records: &Value) -> Result<Value, Error> {
    let Value::Array(records) = records else {
        return Err(Error::type_error("legacy history records must be an array"));
    };
    Ok(Value::Array(
        records
            .iter()
            .enumerate()
            .filter_map(|(index, record)| {
                let missing = ["id", "version", "runId", "sessionId"]
                    .iter()
                    .any(|name| record.get(name).is_none_or(|value| value == &Value::Null));
                (missing || schema::validate_record(record).reason.is_some())
                    .then_some(Value::Number(index as f64))
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Text;

    #[test]
    fn canonical_digest_is_stable_and_every_claimed_block_fails_closed() -> Result<(), Error> {
        let mut record = Value::object(vec![
            ("id", Value::string("record")),
            ("version", Value::Number(1.0)),
            ("runId", Value::string("r")),
            ("sessionId", Value::string("s")),
        ]);
        let block = create(&record, &Value::Object(Vec::new()))?;
        domain::put(&mut record, "provenance", block)?;
        assert_eq!(validate(&record)?, success("verified"));
        domain::put(&mut record, "servedModel", Value::string("forged"))?;
        assert_eq!(
            validate(&record)?,
            failure("provenance integrity check failed")
        );
        assert_eq!(
            canonical_json(&Value::object(vec![
                ("b", Value::Null),
                ("1", Value::String(Text(vec![0xd800]))),
                ("a", Value::Boolean(false))
            ])),
            "{\"1\":\"\\ud800\",\"a\":false,\"b\":null}"
        );
        Ok(())
    }
}
