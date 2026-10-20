use std::collections::{HashMap, HashSet};

use crate::Error;
use crate::domain;
use crate::json::{self, Value};
use crate::planning::{estimate, feedback, memory};
use crate::state::redaction;
use crate::text::{Text, ascii, trim};

pub const MAX_ROWS: usize = 10_000;
pub const MAX_JSONL_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: usize = 2 * MAX_JSONL_BYTES;
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const TASK_CLASSES: [&str; 4] = ["feat", "fix", "docs", "test"];
pub const DELIVERY_GATES: [&str; 4] = ["red-green", "shadow", "self-review", "verifying-reviewer"];
const RUN_FIELDS: [&str; 17] = [
    "sha",
    "session",
    "task",
    "seat",
    "task_class",
    "provider",
    "model",
    "effort",
    "prompt_tokens",
    "completion_tokens",
    "context_used",
    "context_max",
    "context_out",
    "excellence",
    "defects",
    "delivery",
    "attempt",
];

pub(crate) fn error(code: &'static str, message: impl Into<String>) -> Error {
    Error::new("LearningError", message).coded(code)
}

pub(crate) fn optional<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    value.get(field).filter(|value| **value != Value::Null)
}

fn text<'a>(value: &'a Value, field: &str) -> Option<&'a Text> {
    optional(value, field).and_then(Value::as_string)
}

pub fn valid_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|unit| unit.is_ascii_hexdigit())
}

pub fn validate_run(value: &Value) -> Result<(), Error> {
    estimate::validate_run(value)
}

pub fn validate_delivery(value: &Value, source: &str) -> Result<(), Error> {
    estimate::validate_delivery_with_source(value, source)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonlKind {
    Run,
    HumanEvaluation,
    LegacyEvaluation,
    Exported,
}

fn validate(value: &Value, kind: JsonlKind) -> Result<(), Error> {
    let fail = |message| error("ROSTER_ANALYTICS_INPUT", message);
    if domain::members(value).is_none() {
        return Err(fail("expected a JSON object"));
    }
    match kind {
        JsonlKind::Run => validate_run(value),
        JsonlKind::HumanEvaluation => estimate::validate_evaluation(value),
        JsonlKind::LegacyEvaluation | JsonlKind::Exported => {
            if !text(value, "sha").is_some_and(|sha| sha.to_utf8().is_ok_and(|sha| valid_sha(&sha)))
            {
                return Err(fail(
                    "sha must be a full 40- or 64-character hexadecimal Git SHA",
                ));
            }
            if kind == JsonlKind::LegacyEvaluation {
                if !text(value, "verdict")
                    .is_some_and(|verdict| !verdict.0.is_empty() && trim(&verdict.0) == verdict.0)
                {
                    return Err(fail(
                        "verdict must be a nonempty string without surrounding whitespace",
                    ));
                }
                if !value
                    .get("difficulty")
                    .and_then(Value::as_number)
                    .is_some_and(|number| number.is_finite() && number >= 0.0)
                {
                    return Err(fail("difficulty must be a finite, nonnegative number"));
                }
                if !matches!(value.get("again"), Some(Value::Boolean(_))) {
                    return Err(fail("again must be a boolean"));
                }
            } else {
                if value.get("schema") != Some(&Value::Number(1.0)) {
                    return Err(fail("expected AI-Run schema 1"));
                }
                if !text(value, "model").is_some_and(|model| {
                    !model.0.is_empty() && !model.0.iter().any(|unit| crate::text::is_space(*unit))
                }) {
                    return Err(fail("model must be a nonempty name without whitespace"));
                }
                if value.get("effort") != Some(&Value::Null)
                    && !["l", "m", "h", "x"]
                        .iter()
                        .any(|effort| domain::is_text(value.get("effort"), effort))
                {
                    return Err(fail("effort must be l, m, h, x, or null"));
                }
                validate_run(value)?;
            }
            Ok(())
        }
    }
}

pub fn parse_jsonl(source: &Text, label: &str, kind: JsonlKind) -> Result<Vec<Value>, Error> {
    if source.utf8_len() > MAX_JSONL_BYTES {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            format!("{label}: JSONL exceeds 16 MiB"),
        ));
    }
    if source.0.is_empty() {
        return Ok(Vec::new());
    }
    let mut lines = source.0.split(|unit| *unit == 10).peekable();
    let mut records = Vec::new();
    let mut seen = HashSet::new();
    while let Some(line) = lines.next() {
        if line.is_empty() && lines.peek().is_none() {
            break;
        }
        let line = Text(line.strip_suffix(&[13]).unwrap_or(line).to_vec());
        let location = format!("{label}:{}", records.len() + 1);
        if records.len() == MAX_ROWS || line.utf8_len() > MAX_LINE_BYTES {
            return Err(error(
                "ROSTER_ANALYTICS_SIZE",
                format!("{location}: JSONL exceeds 10000 rows or a 1 MiB line"),
            ));
        }
        if trim(&line.0).is_empty() {
            return Err(error(
                "ROSTER_ANALYTICS_INPUT",
                format!("{location}: blank JSONL line"),
            ));
        }
        let record = json::parse(&line).map_err(|_| {
            error(
                "ROSTER_ANALYTICS_INPUT",
                format!("{location}: invalid JSON"),
            )
            .caused_by(Error::new(
                "SyntaxError",
                "Malformed JSONL bytes were preserved",
            ))
        })?;
        crate::state::provenance::schema::validate_data(&record, "learning record").map_err(
            |_| {
                error(
                    "ROSTER_ANALYTICS_INPUT",
                    format!(
                        "{location}: raw snapshots or excessive nesting are not learning evidence"
                    ),
                )
            },
        )?;
        validate(&record, kind).map_err(|cause| {
            error(
                "ROSTER_ANALYTICS_INPUT",
                format!(
                    "{location}: {}",
                    cause
                        .message
                        .strip_prefix("Estimation history: ")
                        .unwrap_or(&cause.message)
                ),
            )
        })?;
        if matches!(kind, JsonlKind::LegacyEvaluation | JsonlKind::Exported) {
            let sha = text(&record, "sha")
                .ok_or_else(|| error("ROSTER_ANALYTICS_INPUT", format!("{location}: missing SHA")))?
                .to_utf8()?
                .to_ascii_lowercase();
            if !seen.insert(sha) {
                return Err(error(
                    "ROSTER_ANALYTICS_INPUT",
                    format!("{location}: duplicate sha"),
                ));
            }
        }
        records.push(record);
    }
    Ok(records)
}

fn known(record: &Value, fields_only: bool) -> Result<Value, Error> {
    let entries = domain::members(record)
        .ok_or_else(|| error("ROSTER_ANALYTICS_INPUT", "Expected a learning object"))?;
    Ok(Value::Object(
        entries
            .iter()
            .filter(|(name, value)| {
                *value != Value::Null
                    && !(ascii(&name.0, "model") && domain::is_text(Some(value), "unknown"))
                    && (!fields_only || RUN_FIELDS.iter().any(|field| ascii(&name.0, field)))
            })
            .cloned()
            .collect(),
    ))
}

fn report_failed(record: &Value) -> bool {
    domain::is_text(record.get("excellence"), "fail")
        || record.get("excellence").and_then(|value| value.get("pass"))
            == Some(&Value::Boolean(false))
}

fn merge_record(base: &Value, overlay: &Value) -> Result<Value, Error> {
    let failed = if report_failed(overlay) {
        overlay.get("excellence")
    } else if report_failed(base) {
        base.get("excellence")
    } else {
        None
    };
    let mut defects = Vec::new();
    for record in [base, overlay] {
        if let Some(values) = optional(record, "defects") {
            for defect in domain::items(values)
                .ok_or_else(|| error("ROSTER_ANALYTICS_INPUT", "defects must be an array"))?
            {
                if !defects.contains(defect) {
                    defects.push(defect.clone());
                }
            }
        }
    }
    let mut merged = domain::merge(base, &known(overlay, false)?)?;
    if let Some(failed) = failed {
        domain::put(&mut merged, "excellence", failed.clone())?;
    }
    if !defects.is_empty() {
        domain::put(&mut merged, "defects", Value::Array(defects))?;
    }
    Ok(merged)
}

fn same_sha(left: &Value, right: &Value) -> Option<bool> {
    Some(
        text(left, "sha")?
            .0
            .iter()
            .map(|unit| {
                if (65..=70).contains(unit) {
                    unit + 32
                } else {
                    *unit
                }
            })
            .eq(text(right, "sha")?.0.iter().map(|unit| {
                if (65..=70).contains(unit) {
                    unit + 32
                } else {
                    *unit
                }
            })),
    )
}

fn matches_run(left: &Value, right: &Value) -> bool {
    same_sha(left, right).unwrap_or_else(|| {
        text(left, "session").is_some_and(|session| Some(session) == text(right, "session"))
            && (text(left, "task").is_none()
                || text(right, "task").is_none()
                || text(left, "task") == text(right, "task"))
    })
}

pub fn matches_evaluation(record: &Value, evaluation: &Value) -> bool {
    same_sha(record, evaluation).unwrap_or_else(|| {
        text(record, "session").is_some_and(|session| Some(session) == text(evaluation, "session"))
    })
}

pub(crate) fn unambiguous(records: &[&Value]) -> Result<(), Error> {
    for field in ["model", "effort", "task", "task_class", "seat"] {
        let mut binding = None;
        for record in records {
            if let Some(value) = text(record, field).filter(|value| !ascii(&value.0, "unknown")) {
                if binding.is_some_and(|prior| prior != value) {
                    return Err(error(
                        "ROSTER_ANALYTICS_JOIN",
                        "Ambiguous session attribution; use a full SHA or distinct session/task evidence. History was preserved.",
                    ));
                }
                binding = Some(value);
            }
        }
    }
    Ok(())
}

struct Entry {
    record: Value,
    git: Option<Value>,
    evaluation_only: bool,
    bytes: usize,
}

fn replace_learning_bytes(
    total: &mut usize,
    previous: usize,
    record: &Value,
    byte_limit: usize,
) -> Result<usize, Error> {
    let exceeded = || {
        error(
            "ROSTER_ANALYTICS_SIZE",
            format!(
                "Learning data exceed the {} MiB processing budget; select a narrower history",
                byte_limit / (1024 * 1024)
            ),
        )
    };
    let bytes = record.json().len().checked_add(1).ok_or_else(exceeded)?;
    let next = total
        .checked_sub(previous)
        .and_then(|total| total.checked_add(bytes))
        .filter(|total| *total <= byte_limit)
        .ok_or_else(exceeded)?;
    *total = next;
    Ok(bytes)
}

pub fn join_learning(
    exported: &[Value],
    local: &[Value],
    evaluations: &[Value],
    include_unpublished: bool,
) -> Result<Vec<Value>, Error> {
    if exported
        .len()
        .saturating_add(local.len())
        .saturating_add(evaluations.len())
        > MAX_ROWS
    {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            "Learning inputs exceed 10000 rows",
        ));
    }
    let mut input_bytes = 0;
    for record in exported.iter().chain(local).chain(evaluations) {
        if domain::members(record).is_none() {
            return Err(error(
                "ROSTER_ANALYTICS_INPUT",
                "Expected learning JSON objects",
            ));
        }
        crate::state::provenance::schema::validate_data(record, "learning record").map_err(
            |_| {
                error(
                    "ROSTER_ANALYTICS_INPUT",
                    "Raw snapshots, nonfinite data or excessive nesting are not learning evidence",
                )
            },
        )?;
        replace_learning_bytes(&mut input_bytes, 0, record, MAX_SOURCE_BYTES)?;
    }
    let mut joined_bytes = 0;
    let mut entries = exported
        .iter()
        .map(|record| {
            let bytes = replace_learning_bytes(&mut joined_bytes, 0, record, MAX_JSONL_BYTES)?;
            Ok(Entry {
                record: record.clone(),
                git: Some(record.clone()),
                evaluation_only: false,
                bytes,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    for run in local {
        let matches = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches_run(&entry.record, run))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if text(run, "sha").is_none() {
            unambiguous(
                &matches
                    .iter()
                    .filter_map(|index| entries.get(*index).map(|entry| &entry.record))
                    .collect::<Vec<_>>(),
            )?;
        }
        if matches.is_empty() && include_unpublished {
            let bytes = replace_learning_bytes(&mut joined_bytes, 0, run, MAX_JSONL_BYTES)?;
            entries.push(Entry {
                record: run.clone(),
                git: None,
                evaluation_only: false,
                bytes,
            });
        }
        for index in matches {
            let entry = entries
                .get_mut(index)
                .ok_or_else(|| error("ROSTER_ANALYTICS_JOIN", "Learning match changed"))?;
            let vllm = domain::is_text(run.get("provider"), "vllm")
                || domain::is_text(entry.record.get("provider"), "vllm");
            let mut merged = merge_record(&entry.record, run)?;
            if let Some(git) = &entry.git {
                merged = merge_record(&merged, git)?;
            }
            if vllm
                && entry.git.as_ref().is_none_or(|git| {
                    optional(git, "provider").is_none()
                        || domain::is_text(git.get("provider"), "local")
                })
            {
                domain::put(&mut merged, "provider", Value::string("vllm"))?;
            }
            entry.bytes =
                replace_learning_bytes(&mut joined_bytes, entry.bytes, &merged, MAX_JSONL_BYTES)?;
            entry.record = merged;
        }
    }
    let mut by_sha = HashMap::new();
    let mut by_session = HashMap::new();
    for (order, evaluation) in evaluations.iter().enumerate() {
        if let Some(sha) = text(evaluation, "sha") {
            by_sha.insert(sha.to_utf8()?.to_ascii_lowercase(), (order, evaluation));
        } else if let Some(session) = text(evaluation, "session") {
            by_session.insert(session.clone(), (order, evaluation));
        }
        let metadata = known(evaluation, true)?;
        let matches = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches_evaluation(&entry.record, evaluation))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if text(evaluation, "sha").is_none() {
            unambiguous(
                &matches
                    .iter()
                    .filter_map(|index| entries.get(*index).map(|entry| &entry.record))
                    .collect::<Vec<_>>(),
            )?;
        }
        if matches.is_empty()
            && include_unpublished
            && text(&metadata, "model").is_some()
            && text(&metadata, "task_class").is_some()
        {
            let bytes = replace_learning_bytes(&mut joined_bytes, 0, &metadata, MAX_JSONL_BYTES)?;
            entries.push(Entry {
                record: metadata.clone(),
                git: None,
                evaluation_only: true,
                bytes,
            });
        }
        for index in matches {
            let entry = entries
                .get_mut(index)
                .ok_or_else(|| error("ROSTER_ANALYTICS_JOIN", "Evaluation match changed"))?;
            let merged = if entry.evaluation_only {
                merge_record(&entry.record, &metadata)?
            } else {
                merge_record(&metadata, &entry.record)?
            };
            entry.bytes =
                replace_learning_bytes(&mut joined_bytes, entry.bytes, &merged, MAX_JSONL_BYTES)?;
            entry.record = merged;
        }
    }
    entries
        .into_iter()
        .map(|entry| {
            let mut record = entry.record;
            let sha_evaluation = text(&record, "sha")
                .and_then(|sha| sha.to_utf8().ok())
                .and_then(|sha| by_sha.get(&sha.to_ascii_lowercase()).copied());
            let session_evaluation = text(&record, "session")
                .and_then(|session| by_session.get(session).copied())
                .filter(|(_, evaluation)| matches_evaluation(&record, evaluation));
            let evaluation = sha_evaluation.or(session_evaluation);
            domain::put(
                &mut record,
                "evaluation",
                evaluation
                    .map(|(_, evaluation)| evaluation.clone())
                    .unwrap_or(Value::Null),
            )?;
            domain::put(
                &mut record,
                "evaluation_order",
                evaluation
                    .map(|(order, _)| Value::Number(order as f64))
                    .unwrap_or(Value::Null),
            )?;
            replace_learning_bytes(&mut joined_bytes, entry.bytes, &record, MAX_JSONL_BYTES)?;
            Ok(record)
        })
        .collect()
}

pub fn join_delivery_learning(
    exported: &[Value],
    local: &[Value],
    evaluations: &[Value],
    include_unpublished: bool,
) -> Result<Vec<Value>, Error> {
    if exported
        .len()
        .saturating_add(local.len())
        .saturating_add(evaluations.len())
        > MAX_ROWS
    {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            "Learning inputs exceed 10000 rows",
        ));
    }
    let mut input_bytes = 0;
    for record in exported.iter().chain(local).chain(evaluations) {
        if domain::members(record).is_none() {
            return Err(error(
                "ROSTER_ANALYTICS_INPUT",
                "Expected learning JSON objects",
            ));
        }
        crate::state::provenance::schema::validate_data(record, "learning record").map_err(
            |_| {
                error(
                    "ROSTER_ANALYTICS_INPUT",
                    "Raw snapshots, nonfinite data or excessive nesting are not learning evidence",
                )
            },
        )?;
        replace_learning_bytes(&mut input_bytes, 0, record, MAX_SOURCE_BYTES)?;
    }
    let mut represented = HashSet::new();
    let mut attempts = Vec::new();
    let mut non_delivery = Vec::new();
    for record in local {
        let Some(delivery) = optional(record, "delivery") else {
            non_delivery.push(record.clone());
            continue;
        };
        validate_delivery(delivery, "Delivery evidence")?;
        let matching = exported
            .iter()
            .filter(|git| matches_run(git, record))
            .collect::<Vec<_>>();
        if text(record, "sha").is_none() {
            unambiguous(&matching)?;
        }
        if !include_unpublished && matching.is_empty() {
            continue;
        }
        let mut attempt = record.clone();
        if let Some(git) = matching.first() {
            if let Some(sha) = text(git, "sha") {
                represented.insert(sha.to_utf8()?.to_ascii_lowercase());
                domain::put(&mut attempt, "sha", Value::String(sha.clone()))?;
            }
        }
        let matching_evaluations = evaluations
            .iter()
            .enumerate()
            .filter(|(_, evaluation)| matches_evaluation(&attempt, evaluation))
            .collect::<Vec<_>>();
        let eval_slice = matching_evaluations
            .iter()
            .map(|(_, evaluation)| (*evaluation).clone())
            .collect::<Vec<_>>();
        let joined = join_learning(&[], std::slice::from_ref(&attempt), &eval_slice, true)?;
        if let Some(mut first) = joined.into_iter().next() {
            if let Some(local_order) = first
                .get("evaluation_order")
                .and_then(|value| domain::safe_integer(Some(value)))
            {
                if let Some((original_order, _)) = matching_evaluations.get(local_order as usize) {
                    domain::put(
                        &mut first,
                        "evaluation_order",
                        Value::Number(*original_order as f64),
                    )?;
                }
            }
            attempts.push(first);
        }
    }
    let unrepresented = exported
        .iter()
        .filter(|record| {
            text(record, "sha")
                .and_then(|sha| sha.to_utf8().ok())
                .is_none_or(|sha| !represented.contains(&sha.to_ascii_lowercase()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let remaining_non_delivery = non_delivery
        .into_iter()
        .filter(|record| !attempts.iter().any(|attempt| matches_run(attempt, record)))
        .collect::<Vec<_>>();
    let remaining_evaluations = evaluations
        .iter()
        .filter(|evaluation| {
            unrepresented
                .iter()
                .chain(&remaining_non_delivery)
                .any(|record| matches_evaluation(record, evaluation))
                || !attempts
                    .iter()
                    .any(|attempt| matches_evaluation(attempt, evaluation))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut combined = join_learning(
        &unrepresented,
        &remaining_non_delivery,
        &remaining_evaluations,
        include_unpublished,
    )?;
    combined.extend(attempts);
    if combined.len() > MAX_ROWS {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            "Learning snapshot exceeds 10000 records",
        ));
    }
    let mut joined_bytes = 0;
    for record in &combined {
        replace_learning_bytes(&mut joined_bytes, 0, record, MAX_JSONL_BYTES)?;
    }
    Ok(combined)
}

pub fn median(values: &[f64]) -> Result<Option<f64>, Error> {
    if values.len() > MAX_ROWS || values.iter().any(|value| !value.is_finite()) {
        return Err(Error::type_error(
            "Median requires at most 10000 finite numeric samples",
        ));
    }
    if values.is_empty() {
        return Ok(None);
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    let upper = *values
        .get(middle)
        .ok_or_else(|| Error::type_error("Missing median sample"))?;
    let result = if values.len() % 2 == 1 {
        upper
    } else {
        let lower = *values
            .get(middle - 1)
            .ok_or_else(|| Error::type_error("Missing lower median sample"))?;
        lower + (upper - lower) / 2.0
    };
    if !result.is_finite() {
        return Err(Error::type_error("Median exceeds finite numeric range"));
    }
    Ok(Some(result))
}

pub(crate) fn collation_key(text: &Text) -> Result<(Vec<u8>, Vec<u8>), Error> {
    crate::execution::evidence::name_key(&text.to_utf8()?).map_err(|_| {
        error("ROSTER_ANALYTICS_COLLATION_UNPROVEN", "Learning collation is verified only for the en-US ASCII profile; non-ASCII locale ordering requires independent platform verification.")
    })
}

fn task_class(record: &Value) -> Result<Option<Text>, Error> {
    match text(record, "task_class") {
        Some(class) => Ok(Some(class.clone())),
        None => text(record, "task")
            .map(|task| task.to_utf8().and_then(|task| estimate::infer_class(&task)))
            .transpose()
            .map(|class| class.flatten().map(|class| Text::from(class.as_str()))),
    }
}

fn target(record: &Value) -> Result<Text, Error> {
    if let Some(sha) = text(record, "sha") {
        let sha = sha.to_utf8()?;
        if !valid_sha(&sha) {
            return Err(error(
                "ROSTER_ANALYTICS_INPUT",
                "An evaluated result needs a full SHA or session",
            ));
        }
        return Ok(Text::from(
            format!("sha:{}", sha.to_ascii_lowercase()).as_str(),
        ));
    }
    let session = text(record, "session")
        .filter(|session| !session.0.is_empty())
        .ok_or_else(|| {
            error(
                "ROSTER_ANALYTICS_INPUT",
                "An evaluated result needs a SHA or session",
            )
        })?;
    let mut target = Text::from("session:");
    target.0.extend_from_slice(&session.0);
    Ok(target)
}

fn validate_joined(record: &Value) -> Result<(), Error> {
    if domain::members(record).is_none() {
        return Err(Error::type_error("Expected joined metrics records"));
    }
    for field in [
        "model",
        "task_class",
        "task",
        "effort",
        "session",
        "sha",
        "seat",
    ] {
        if optional(record, field).is_some_and(|value| value.as_string().is_none()) {
            return Err(Error::type_error(format!(
                "Joined metrics {field} must be text or null"
            )));
        }
    }
    if optional(record, "evaluation").is_some_and(|value| domain::members(value).is_none()) {
        return Err(Error::type_error("Expected a joined human evaluation"));
    }
    if optional(record, "evaluation_order").is_some_and(|value| {
        domain::safe_integer(Some(value)).is_none_or(|order| order >= MAX_ROWS as u64)
    }) {
        return Err(Error::type_error(
            "Joined evaluation order must be a bounded nonnegative integer or null",
        ));
    }
    Ok(())
}

fn effort_index(effort: Option<&Text>) -> usize {
    ["l", "m", "h", "x"]
        .iter()
        .position(|expected| effort.is_some_and(|effort| ascii(&effort.0, expected)))
        .unwrap_or(4)
}

struct Sample {
    id: Text,
    accepted: bool,
    failed: bool,
    minutes: Option<f64>,
    difficulty: Option<f64>,
}

struct Group {
    model: Option<Text>,
    class: Option<Text>,
    effort: Option<Text>,
    runs: usize,
    evaluated: usize,
    samples: Vec<Sample>,
}

fn null_text(text: &Option<Text>) -> Value {
    text.clone().map_or(Value::Null, Value::String)
}

pub fn summarize_learning(records: &[Value]) -> Result<Vec<Value>, Error> {
    if records.len() > MAX_ROWS {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            "Learning snapshot exceeds 10000 records",
        ));
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut positions = HashMap::new();
    for record in records {
        validate_joined(record)?;
        let key = (
            text(record, "model").cloned(),
            task_class(record)?,
            text(record, "effort").cloned(),
        );
        let index = if let Some(index) = positions.get(&key) {
            *index
        } else {
            let index = groups.len();
            groups.push(Group {
                model: key.0.clone(),
                class: key.1.clone(),
                effort: key.2.clone(),
                runs: 0,
                evaluated: 0,
                samples: Vec::new(),
            });
            positions.insert(key, index);
            index
        };
        let group = groups
            .get_mut(index)
            .ok_or_else(|| Error::type_error("Missing learning group"))?;
        group.runs += 1;
        let evaluation = optional(record, "evaluation");
        group.evaluated += usize::from(evaluation.is_some());
        let failed = estimate::excellence_failed(record);
        let verdict = if failed {
            Some("reject".to_owned())
        } else {
            evaluation
                .and_then(|value| text(value, "verdict"))
                .map(Text::to_utf8)
                .transpose()?
        };
        let Some(verdict) =
            verdict.filter(|verdict| ["accept", "reject", "rework"].contains(&verdict.as_str()))
        else {
            continue;
        };
        let id = target(evaluation.unwrap_or(record))?;
        let prior = group.samples.iter().position(|sample| sample.id == id);
        let failed = failed
            || prior
                .and_then(|index| group.samples.get(index))
                .is_some_and(|sample| sample.failed);
        let sample = Sample {
            id,
            accepted: verdict == "accept" && !failed,
            failed,
            minutes: evaluation
                .and_then(|evaluation| domain::safe_integer(evaluation.get("minutes")))
                .map(|value| value as f64),
            difficulty: evaluation
                .and_then(|evaluation| domain::safe_integer(evaluation.get("difficulty")))
                .filter(|value| (1..=5).contains(value))
                .map(|value| value as f64),
        };
        if let Some(index) = prior {
            *group
                .samples
                .get_mut(index)
                .ok_or_else(|| Error::type_error("Missing learning sample"))? = sample;
        } else {
            group.samples.push(sample);
        }
    }
    let mut output = Vec::new();
    for group in groups {
        let model_key = collation_key(group.model.as_ref().unwrap_or(&Text::default()))?;
        let class_key = collation_key(group.class.as_ref().unwrap_or(&Text::default()))?;
        let effort_key = effort_index(group.effort.as_ref());
        let accepted = group
            .samples
            .iter()
            .filter(|sample| sample.accepted)
            .count();
        let accepted_minutes = median(
            &group
                .samples
                .iter()
                .filter(|sample| sample.accepted)
                .filter_map(|sample| sample.minutes)
                .collect::<Vec<_>>(),
        )?;
        let value = Value::object(vec![
            ("model", null_text(&group.model)),
            ("task_class", null_text(&group.class)),
            ("effort", null_text(&group.effort)),
            ("runs", Value::Number(group.runs as f64)),
            ("evaluated", Value::Number(group.evaluated as f64)),
            ("n", Value::Number(group.samples.len() as f64)),
            ("accepted", Value::Number(accepted as f64)),
            (
                "acceptRate",
                if group.samples.is_empty() {
                    Value::Null
                } else {
                    Value::Number(accepted as f64 / group.samples.len() as f64)
                },
            ),
            (
                "medianMinutes",
                median(
                    &group
                        .samples
                        .iter()
                        .filter_map(|sample| sample.minutes)
                        .collect::<Vec<_>>(),
                )?
                .map_or(Value::Null, Value::Number),
            ),
            (
                "medianDifficulty",
                median(
                    &group
                        .samples
                        .iter()
                        .filter_map(|sample| sample.difficulty)
                        .collect::<Vec<_>>(),
                )?
                .map_or(Value::Null, Value::Number),
            ),
            (
                "estimate_min",
                accepted_minutes.map_or(Value::Null, |value| Value::Number(value.round())),
            ),
        ]);
        output.push(((model_key, class_key, effort_key), value));
    }
    output.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(output.into_iter().map(|(_, value)| value).collect())
}

pub fn recommend(
    records: &[Value],
    task_class: &str,
    difficulty: Option<u8>,
) -> Result<Option<Value>, Error> {
    if !TASK_CLASSES.contains(&task_class) {
        return Err(Error::type_error(
            "task class must be feat, fix, docs, or test",
        ));
    }
    if difficulty.is_some_and(|difficulty| !(1..=5).contains(&difficulty)) {
        return Err(Error::type_error(
            "difficulty must be an integer from 1 to 5",
        ));
    }
    let groups = summarize_learning(records)?;
    let mut eligible = Vec::new();
    for group in groups {
        if !domain::is_text(group.get("task_class"), task_class)
            || text(&group, "model").is_none_or(|model| {
                ["unknown", "builtin-stub", ""]
                    .iter()
                    .any(|unknown| ascii(&model.0, unknown))
            })
            || domain::safe_integer(group.get("n")).is_none_or(|n| n < 3)
            || difficulty.is_some_and(|difficulty| {
                group
                    .get("medianDifficulty")
                    .and_then(Value::as_number)
                    .is_none_or(|median| median < f64::from(difficulty))
            })
        {
            continue;
        }
        let model = text(&group, "model")
            .ok_or_else(|| Error::type_error("Missing recommendation model"))?;
        let key = collation_key(model)?;
        let effort = effort_index(text(&group, "effort"));
        let rate = group
            .get("acceptRate")
            .and_then(Value::as_number)
            .ok_or_else(|| Error::type_error("Missing evaluated acceptance rate"))?;
        let n = domain::safe_integer(group.get("n"))
            .ok_or_else(|| Error::type_error("Missing evaluated sample count"))?;
        eligible.push((group, key, effort, rate, n));
    }
    eligible.sort_by(|left, right| {
        right
            .3
            .total_cmp(&left.3)
            .then_with(|| right.4.cmp(&left.4))
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    eligible
        .into_iter()
        .next()
        .map(|(group, _, _, _, _)| {
            Ok(Value::object(
                [
                    "model",
                    "effort",
                    "n",
                    "accepted",
                    "acceptRate",
                    "medianMinutes",
                    "medianDifficulty",
                    "estimate_min",
                ]
                .into_iter()
                .map(|name| {
                    group
                        .get(name)
                        .cloned()
                        .map(|value| (name, value))
                        .ok_or_else(|| Error::type_error("Incomplete recommendation aggregate"))
                })
                .collect::<Result<Vec<_>, Error>>()?,
            ))
        })
        .transpose()
}

pub fn learning_seat(record: &Value) -> Result<Text, Error> {
    let evaluation = optional(record, "evaluation");
    for source in [Some(record), evaluation].into_iter().flatten() {
        if optional(source, "seat").is_some_and(|seat| seat.as_string().is_none()) {
            return Err(Error::type_error("Invalid learning seat"));
        }
    }
    let seat = text(record, "seat")
        .or_else(|| evaluation.and_then(|evaluation| text(evaluation, "seat")))
        .cloned()
        .or_else(|| {
            let session = text(record, "session")
                .or_else(|| evaluation.and_then(|evaluation| text(evaluation, "session")))?;
            ["planner", "coder", "reviewer"]
                .iter()
                .find(|seat| {
                    session
                        .0
                        .ends_with(&Text::from(format!("-{seat}").as_str()).0)
                })
                .map(|seat| Text::from(*seat))
        })
        .unwrap_or_else(|| Text::from("coder"));
    if !(1..=64).contains(&seat.0.len())
        || ascii(&seat.0, "-")
        || !seat
            .0
            .iter()
            .all(|unit| matches!(unit, 45 | 46 | 48..=57 | 65..=90 | 95 | 97..=122))
    {
        return Err(Error::type_error("Invalid learning seat"));
    }
    Ok(seat)
}

fn human_timestamp(value: &Value) -> Option<i64> {
    estimate::timestamp(value).or_else(|| {
        let source = value.as_string()?.to_utf8().ok()?;
        let parsed =
            time::OffsetDateTime::parse(&source, &time::format_description::well_known::Rfc3339)
                .ok()?;
        i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).ok()
    })
}

pub struct EvidenceScope<'a> {
    pub model: &'a Text,
    pub task_class: &'a str,
    pub seat: &'a Text,
    pub difficulty: u8,
    pub effort: Option<&'a Value>,
    pub now_ms: f64,
}

pub fn explain_route_evidence(
    records: &[Value],
    scope: EvidenceScope<'_>,
) -> Result<Vec<Value>, Error> {
    if !scope.now_ms.is_finite() {
        return Err(Error::type_error(
            "Routing evidence clock must be finite milliseconds",
        ));
    }
    if !TASK_CLASSES.contains(&scope.task_class)
        || scope.model.0.is_empty()
        || !(1..=5).contains(&scope.difficulty)
        || scope.effort.is_some_and(|effort| {
            effort != &Value::Null
                && !["l", "m", "h", "x"]
                    .iter()
                    .any(|name| domain::is_text(Some(effort), name))
        })
        || learning_seat(&Value::object(vec![(
            "seat",
            Value::String(scope.seat.clone()),
        )]))
        .is_err()
    {
        return Err(Error::type_error("Invalid routing evidence scope"));
    }
    if records.len() > MAX_ROWS {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            "Routing evidence exceeds 10000 records",
        ));
    }
    let mut human = Vec::new();
    for record in records {
        validate_joined(record)?;
        if text(record, "model") == Some(scope.model)
            && optional(record, "evaluation").is_some()
            && learning_seat(record)? == *scope.seat
        {
            human.push(record.clone());
        }
    }
    let mut groups = summarize_learning(&human)?
        .into_iter()
        .filter(|group| {
            domain::is_text(group.get("task_class"), scope.task_class)
                && scope
                    .effort
                    .is_none_or(|effort| group.get("effort") == Some(effort))
        })
        .collect::<Vec<_>>();
    if groups.is_empty() {
        groups.push(Value::object(vec![
            ("effort", scope.effort.cloned().unwrap_or(Value::Null)),
            ("n", Value::Number(0.0)),
            ("accepted", Value::Number(0.0)),
            ("acceptRate", Value::Null),
            ("medianDifficulty", Value::Null),
        ]));
    }
    groups
        .into_iter()
        .map(|group| {
            let mut latest = None;
            for record in &human {
                let evaluation = optional(record, "evaluation")
                    .ok_or_else(|| Error::type_error("Missing human evidence"))?;
                if task_class(record)?.as_ref() != Some(&Text::from(scope.task_class))
                    || optional(record, "effort") != optional(&group, "effort")
                    || !["accept", "reject"]
                        .iter()
                        .any(|verdict| domain::is_text(evaluation.get("verdict"), verdict))
                {
                    continue;
                }
                if let Some(at) = evaluation
                    .get("at")
                    .and_then(human_timestamp)
                    .filter(|at| *at as f64 <= scope.now_ms)
                {
                    latest = Some(latest.map_or(at, |prior: i64| prior.max(at)));
                }
            }
            let n = domain::safe_integer(group.get("n"))
                .ok_or_else(|| Error::type_error("Missing evidence count"))?;
            let accepted = domain::safe_integer(group.get("accepted"))
                .ok_or_else(|| Error::type_error("Missing accepted count"))?;
            let difficulty = group.get("medianDifficulty").and_then(Value::as_number);
            Ok(Value::object(vec![
                ("model", Value::String(scope.model.clone())),
                ("seat", Value::String(scope.seat.clone())),
                ("task_class", Value::string(scope.task_class)),
                (
                    "effort",
                    group.get("effort").cloned().unwrap_or(Value::Null),
                ),
                ("samples", Value::Number(n as f64)),
                ("accepted", Value::Number(accepted as f64)),
                ("rejected", Value::Number(n.saturating_sub(accepted) as f64)),
                (
                    "acceptRate",
                    group.get("acceptRate").cloned().unwrap_or(Value::Null),
                ),
                (
                    "medianDifficulty",
                    difficulty.map_or(Value::Null, Value::Number),
                ),
                (
                    "latestEvaluation",
                    latest
                        .map(|at| crate::clock::iso_timestamp(at).map(|at| Value::string(&at)))
                        .transpose()?
                        .unwrap_or(Value::Null),
                ),
                (
                    "ageDays",
                    latest.map_or(Value::Null, |at| {
                        Value::Number(((scope.now_ms - at as f64) / 86_400_000.0).floor())
                    }),
                ),
                (
                    "sufficient",
                    Value::Boolean(
                        n >= 3
                            && difficulty.is_some_and(|difficulty| {
                                difficulty >= f64::from(scope.difficulty)
                            }),
                    ),
                ),
            ]))
        })
        .collect()
}

struct CeilingSample {
    id: Text,
    evaluation: Value,
    failed: bool,
    order: u64,
}

struct CeilingGroup {
    seat: Text,
    model: Text,
    samples: Vec<CeilingSample>,
}

pub fn derive_difficulty_ceilings(
    records: &[Value],
    redaction: &redaction::Config,
) -> Result<Vec<Value>, Error> {
    if records.len() > MAX_ROWS {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            "Learning snapshot exceeds 10000 records",
        ));
    }
    let mut groups: Vec<CeilingGroup> = Vec::new();
    for (order, record) in records.iter().enumerate() {
        validate_joined(record)?;
        let Some(evaluation) = optional(record, "evaluation") else {
            continue;
        };
        let Some(model) = text(record, "model").filter(|model| {
            !model.0.is_empty()
                && !["unknown", "builtin-stub"]
                    .iter()
                    .any(|unknown| ascii(&model.0, unknown))
        }) else {
            continue;
        };
        if !["accept", "reject", "rework"]
            .iter()
            .any(|verdict| domain::is_text(evaluation.get("verdict"), verdict))
        {
            continue;
        }
        let seat = learning_seat(record)?;
        let index = groups
            .iter()
            .position(|group| group.seat == seat && &group.model == model)
            .unwrap_or(groups.len());
        if index == groups.len() {
            groups.push(CeilingGroup {
                seat,
                model: model.clone(),
                samples: Vec::new(),
            });
        }
        let samples = &mut groups
            .get_mut(index)
            .ok_or_else(|| Error::type_error("Missing ceiling group"))?
            .samples;
        let id = target(evaluation)?;
        let prior = samples.iter().position(|sample| sample.id == id);
        let failed = estimate::excellence_failed(record)
            || prior
                .and_then(|index| samples.get(index))
                .is_some_and(|sample| sample.failed);
        let sample = CeilingSample {
            id,
            evaluation: evaluation.clone(),
            failed,
            order: domain::safe_integer(record.get("evaluation_order")).unwrap_or(order as u64),
        };
        if let Some(index) = prior {
            *samples
                .get_mut(index)
                .ok_or_else(|| Error::type_error("Missing ceiling sample"))? = sample;
        } else {
            samples.push(sample);
        }
    }
    let mut output = Vec::new();
    for CeilingGroup {
        seat,
        model,
        mut samples,
    } in groups
    {
        let timestamped = samples.iter().all(|sample| {
            sample
                .evaluation
                .get("at")
                .and_then(human_timestamp)
                .is_some()
        });
        samples.sort_by_key(|sample| {
            (
                if timestamped {
                    sample.evaluation.get("at").and_then(human_timestamp)
                } else {
                    None
                },
                sample.order,
            )
        });
        let mut ceiling = 2_u8;
        let mut streak = 0_u8;
        let mut notes = Vec::new();
        for sample in &samples {
            let evaluation = &sample.evaluation;
            if sample.failed || domain::is_text(evaluation.get("verdict"), "reject") {
                ceiling = ceiling.saturating_sub(1).max(1);
                streak = 0;
                let mut note =
                    Text::from("Rejected slice: human feedback is data, not a policy grant.");
                if let Some(comment) =
                    text(evaluation, "comment").filter(|comment| !trim(&comment.0).is_empty())
                {
                    let safe = redaction::diagnostic(
                        &memory::redact(&Text(trim(&comment.0).to_vec()), redaction),
                        redaction,
                    )
                    .normalized_lines();
                    if safe.utf8_len() > 4096 {
                        return Err(error(
                            "ROSTER_ANALYTICS_SIZE",
                            "Ceiling skill note exceeds 4 KiB",
                        ));
                    }
                    for line in safe.0.split(|unit| *unit == 10) {
                        note.push_str("\n> ");
                        note.0.extend_from_slice(line);
                    }
                } else {
                    note.push_str("\nNo human comment recorded.");
                }
                notes.push(Value::String(note));
            } else if domain::is_text(evaluation.get("verdict"), "accept")
                && domain::safe_integer(evaluation.get("difficulty")) == Some(u64::from(ceiling))
            {
                streak += 1;
                if streak == 3 {
                    ceiling = ceiling.saturating_add(1).min(5);
                    streak = 0;
                }
            } else {
                streak = 0;
            }
        }
        let key = (collation_key(&seat)?, collation_key(&model)?);
        output.push((
            key,
            Value::object(vec![
                ("seat", Value::String(seat)),
                ("model", Value::String(model)),
                ("ceiling", Value::Number(f64::from(ceiling))),
                ("n", Value::Number(samples.len() as f64)),
                ("acceptStreak", Value::Number(f64::from(streak))),
                ("skillNotes", Value::Array(notes)),
            ]),
        ));
    }
    output.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(output.into_iter().map(|(_, value)| value).collect())
}

pub fn bounded_output(output: String) -> Result<String, Error> {
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            "Analytics output exceeds 4 MiB; select a narrower --ref",
        ));
    }
    Ok(output)
}

fn display(value: Option<&Value>) -> Result<Text, Error> {
    value
        .filter(|value| **value != Value::Null)
        .map_or_else(|| Ok(Text::from("-")), domain::string)
}

pub fn format_metrics(groups: &[Value]) -> Result<String, Error> {
    if groups.is_empty() {
        return Ok("No AI-Run records found.\n".to_owned());
    }
    let mut rows = vec![
        [
            "MODEL",
            "EFFORT",
            "RUNS",
            "EVALS",
            "TASK_CLASS",
            "N",
            "ACCEPT",
            "MEDIAN_MIN",
            "MEDIAN_DIFFICULTY",
        ]
        .iter()
        .map(|value| Text::from(*value))
        .collect::<Vec<_>>(),
    ];
    for group in groups {
        let mut row = ["model", "effort", "runs", "evaluated", "task_class", "n"]
            .iter()
            .map(|name| display(group.get(name)))
            .collect::<Result<Vec<_>, _>>()?;
        row.push(match group.get("acceptRate").and_then(Value::as_number) {
            Some(rate) => Text::from(format!("{}%", feedback::percentage(rate * 100.0)?).as_str()),
            None => Text::from("-"),
        });
        row.push(display(group.get("medianMinutes"))?);
        row.push(display(group.get("medianDifficulty"))?);
        rows.push(row);
    }
    let widths = (0..9)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.0.len())
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let mut output = Text::default();
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            output.0.extend_from_slice(&cell.0);
            if column < 8 {
                let width = widths
                    .get(column)
                    .copied()
                    .ok_or_else(|| Error::type_error("Missing metrics column width"))?;
                output.push_str(&" ".repeat(width.saturating_sub(cell.0.len()) + 2));
            }
        }
        output.push_str("\n");
    }
    bounded_output(output.to_utf8()?)
}

fn delivery_label(value: Option<&Value>) -> Result<Text, Error> {
    match value.filter(|value| **value != Value::Null) {
        None => Ok(Text::from("unknown")),
        Some(Value::String(text)) => {
            if !text.0.is_empty() && !ascii(&text.0, "-") {
                Ok(text.clone())
            } else {
                Ok(Text::from("unknown"))
            }
        }
        Some(_) => Err(Error::type_error(
            "Expected text or null delivery dimension",
        )),
    }
}

fn map_set<K: PartialEq, V>(entries: &mut Vec<(K, V)>, key: K, value: V) {
    if let Some((_, existing)) = entries.iter_mut().find(|(existing, _)| *existing == key) {
        *existing = value;
    } else {
        entries.push((key, value));
    }
}

pub fn delivery_metrics(records: &[Value]) -> Result<Vec<Value>, Error> {
    if records.len() > MAX_ROWS {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            "Delivery snapshot exceeds 10000 records",
        ));
    }
    struct DeliveryGroup {
        key_units: Vec<u16>,
        dimensions: [Text; 3],
        records: Vec<(String, Value)>,
    }
    let mut groups: Vec<DeliveryGroup> = Vec::new();
    for record in records {
        validate_joined(record)?;
        if let Some(evaluation) = optional(record, "evaluation") {
            target(evaluation)?;
        }
        let delivery = optional(record, "delivery");
        if let Some(delivery) = delivery {
            validate_delivery(delivery, "Delivery evidence")?;
        }
        let dimensions = [
            delivery_label(record.get("model"))?,
            delivery_label(delivery.and_then(|value| value.get("hardware")))?,
            delivery_label(record.get("seat"))?,
        ];
        let key_json = Value::Array(dimensions.iter().cloned().map(Value::String).collect()).json();
        let key_units = Text::from(key_json.as_str()).0;
        let group =
            if let Some(group) = groups.iter_mut().find(|group| group.key_units == key_units) {
                group
            } else {
                groups.push(DeliveryGroup {
                    key_units,
                    dimensions,
                    records: Vec::new(),
                });
                groups
                    .last_mut()
                    .ok_or_else(|| Error::type_error("Missing delivery group"))?
            };
        let identity = if let Some(delivery) = delivery {
            Value::Array(vec![
                delivery.get("id").cloned().unwrap_or(Value::Null),
                delivery.get("attempt").cloned().unwrap_or(Value::Null),
            ])
            .json()
        } else if let Some(sha) = text(record, "sha").filter(|sha| !sha.0.is_empty()) {
            format!("sha:{}", sha.to_utf8()?.to_ascii_lowercase())
        } else {
            format!("row:{}", group.records.len())
        };
        map_set(&mut group.records, identity, record.clone());
    }
    groups.sort_by(|left, right| left.key_units.cmp(&right.key_units));
    let mut output = Vec::with_capacity(groups.len());
    for group in groups {
        let [model, hardware, seat] = group.dimensions;
        let is_coder = ascii(&seat.0, "coder");
        let mut lead: Vec<(Text, f64)> = Vec::new();
        let mut cycles: Vec<(Text, f64)> = Vec::new();
        let mut reviews: Vec<(Text, (u64, Text))> = Vec::new();
        let mut evaluations: Vec<(Text, Value)> = Vec::new();
        let mut estimates: Vec<(Text, f64)> = Vec::new();
        let mut repairs = 0_u64;
        let mut repair_samples = 0_usize;
        let mut gate_checks = [0_u64; 4];
        let mut gate_failures = [0_u64; 4];
        for (_, record) in &group.records {
            let evidence = optional(record, "delivery");
            if let Some(evidence) = evidence {
                let id = text(evidence, "id")
                    .cloned()
                    .ok_or_else(|| Error::type_error("Delivery evidence requires an id"))?;
                if let (Some(created), Some(merged)) = (
                    optional(evidence, "ask_created_at").and_then(estimate::timestamp),
                    optional(evidence, "pr_merged_at").and_then(estimate::timestamp),
                ) {
                    map_set(
                        &mut lead,
                        id.clone(),
                        (merged.saturating_sub(created)) as f64 / 60_000.0,
                    );
                }
                if is_coder {
                    if let Some(duration) =
                        optional(evidence, "duration_ms").and_then(Value::as_number)
                    {
                        let next = cycles
                            .iter()
                            .find(|(existing, _)| *existing == id)
                            .map_or(0.0, |(_, current)| *current)
                            + duration / 60_000.0;
                        map_set(&mut cycles, id.clone(), next);
                    }
                }
                if let Some(verdict) = text(evidence, "review_verdict") {
                    let attempt = domain::safe_integer(evidence.get("attempt")).unwrap_or(1);
                    let replace = reviews
                        .iter()
                        .find(|(existing, _)| *existing == id)
                        .is_none_or(|(_, (previous, _))| attempt < *previous);
                    if replace {
                        map_set(&mut reviews, id.clone(), (attempt, verdict.clone()));
                    }
                }
                if let Some(count) =
                    optional(evidence, "review_repairs").and_then(|v| domain::safe_integer(Some(v)))
                {
                    repairs = repairs.saturating_add(count);
                    repair_samples += 1;
                }
                if let Some(gates) = optional(evidence, "gates") {
                    for (index, gate) in DELIVERY_GATES.iter().enumerate() {
                        if let Some(counts) = optional(gates, gate) {
                            gate_checks[index] = gate_checks[index].saturating_add(
                                domain::safe_integer(counts.get("checks")).unwrap_or(0),
                            );
                            gate_failures[index] = gate_failures[index].saturating_add(
                                domain::safe_integer(counts.get("failures")).unwrap_or(0),
                            );
                        }
                    }
                }
            }
            if let Some(evaluation) = optional(record, "evaluation") {
                let eval_key =
                    if let Some(sha) = text(evaluation, "sha").filter(|sha| !sha.0.is_empty()) {
                        Some(Text::from(sha.to_utf8()?.to_ascii_lowercase().as_str()))
                    } else {
                        text(evaluation, "session")
                            .filter(|session| !session.0.is_empty())
                            .cloned()
                    };
                if let Some(key) = eval_key {
                    map_set(&mut evaluations, key.clone(), evaluation.clone());
                    if let (Some(estimated), Some(actual)) = (
                        evidence
                            .and_then(|e| optional(e, "estimate_min"))
                            .and_then(Value::as_number),
                        optional(evaluation, "minutes")
                            .and_then(Value::as_number)
                            .filter(|minutes| minutes.is_finite() && *minutes >= 0.0),
                    ) {
                        if !estimates.iter().any(|(existing, _)| *existing == key) {
                            estimates.push((key, actual - estimated));
                        }
                    }
                }
            }
        }
        let rework_evals = evaluations
            .iter()
            .filter(|(_, evaluation)| domain::is_text(evaluation.get("verdict"), "rework"))
            .count() as u64;
        let lead_values = lead.iter().map(|(_, value)| *value).collect::<Vec<_>>();
        let cycle_values = cycles.iter().map(|(_, value)| *value).collect::<Vec<_>>();
        let estimate_values = estimates
            .iter()
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        let first_pass = if reviews.is_empty() {
            Value::string("unknown")
        } else {
            let passed = reviews
                .iter()
                .filter(|(_, (attempt, verdict))| *attempt == 1 && ascii(&verdict.0, "pass"))
                .count();
            Value::Number(passed as f64 / reviews.len() as f64)
        };
        let rework = if repair_samples > 0 || !evaluations.is_empty() {
            Value::Number((repairs.saturating_add(rework_evals)) as f64)
        } else {
            Value::string("unknown")
        };
        let gate_failures_obj = Value::object(
            DELIVERY_GATES
                .iter()
                .enumerate()
                .map(|(index, gate)| {
                    (
                        *gate,
                        if gate_checks[index] > 0 {
                            Value::Number(gate_failures[index] as f64)
                        } else {
                            Value::string("unknown")
                        },
                    )
                })
                .collect(),
        );
        let gate_checks_obj = Value::object(
            DELIVERY_GATES
                .iter()
                .enumerate()
                .map(|(index, gate)| (*gate, Value::Number(gate_checks[index] as f64)))
                .collect(),
        );
        output.push(Value::object(vec![
            ("model", Value::String(model)),
            ("hardware", Value::String(hardware)),
            ("seat", Value::String(seat)),
            ("runs", Value::Number(group.records.len() as f64)),
            (
                "leadMinutes",
                median(&lead_values)?.map_or_else(|| Value::string("unknown"), Value::Number),
            ),
            (
                "coderMinutes",
                median(&cycle_values)?.map_or_else(|| Value::string("unknown"), Value::Number),
            ),
            ("firstPassYield", first_pass),
            ("rework", rework),
            ("gateFailures", gate_failures_obj),
            (
                "estimateErrorMinutes",
                median(&estimate_values)?.map_or_else(|| Value::string("unknown"), Value::Number),
            ),
            (
                "samples",
                Value::object(vec![
                    ("lead", Value::Number(lead.len() as f64)),
                    ("cycle", Value::Number(cycles.len() as f64)),
                    ("review", Value::Number(reviews.len() as f64)),
                    ("repairs", Value::Number(repair_samples as f64)),
                    ("evaluations", Value::Number(evaluations.len() as f64)),
                    ("estimate", Value::Number(estimates.len() as f64)),
                ]),
            ),
            ("gateChecks", gate_checks_obj),
        ]));
    }
    Ok(output)
}

fn delivery_number(value: Option<&Value>) -> Result<Text, Error> {
    match value {
        Some(Value::Number(number)) if number.is_finite() => {
            let mut rounded = (number * 1000.0).round() / 1000.0;
            if rounded == 0.0 {
                rounded = 0.0;
            }
            Ok(Text::from(Value::Number(rounded).json().as_str()))
        }
        Some(value) if *value != Value::Null => domain::string(value),
        _ => Ok(Text::from("unknown")),
    }
}

pub fn format_delivery_metrics(groups: &[Value]) -> Result<String, Error> {
    if groups.is_empty() {
        return Ok("No delivery records found.\n".to_owned());
    }
    let headers = [
        "MODEL",
        "HARDWARE",
        "SEAT",
        "RUNS",
        "LEAD_MIN",
        "CODER_MIN",
        "FIRST_PASS",
        "REWORK",
        "ESTIMATE_ERROR_MIN",
        "red-green",
        "shadow",
        "self-review",
        "verifying-reviewer",
    ];
    let mut rows = vec![
        headers
            .iter()
            .map(|header| Text::from(*header))
            .collect::<Vec<_>>(),
    ];
    for group in groups {
        if domain::members(group).is_none() {
            return Err(Error::type_error("Expected delivery metrics group objects"));
        }
        let mut row = vec![
            delivery_label(group.get("model"))?,
            delivery_label(group.get("hardware"))?,
            delivery_label(group.get("seat"))?,
            domain::string(
                group
                    .get("runs")
                    .ok_or_else(|| Error::type_error("Delivery group requires runs"))?,
            )?,
            delivery_number(group.get("leadMinutes"))?,
            delivery_number(group.get("coderMinutes"))?,
            match group.get("firstPassYield").and_then(Value::as_number) {
                Some(rate) => {
                    Text::from(format!("{}%", feedback::percentage(rate * 100.0)?).as_str())
                }
                None => Text::from("unknown"),
            },
            delivery_number(group.get("rework"))?,
            delivery_number(group.get("estimateErrorMinutes"))?,
        ];
        let failures = group
            .get("gateFailures")
            .ok_or_else(|| Error::type_error("Delivery group requires gateFailures"))?;
        for gate in DELIVERY_GATES {
            row.push(delivery_number(failures.get(gate))?);
        }
        rows.push(row);
    }
    let columns = headers.len();
    let widths = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.0.len())
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let mut output = Text::default();
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            output.0.extend_from_slice(&cell.0);
            if column + 1 < columns {
                let width = widths
                    .get(column)
                    .copied()
                    .ok_or_else(|| Error::type_error("Missing delivery column width"))?;
                output.push_str(&" ".repeat(width.saturating_sub(cell.0.len()) + 2));
            }
        }
        output.push_str("\n");
    }
    bounded_output(output.to_utf8()?)
}

fn proposal_regex(pattern: &str) -> Result<regex_lite::Regex, Error> {
    regex_lite::Regex::new(pattern).map_err(|_| Error::new("Error", "Invalid proposal pattern"))
}

fn failure_signature(reason: &str) -> Result<(String, String), Error> {
    let prefix = proposal_regex(r"^([A-Za-z][A-Za-z /-]{1,40}):\s*")?;
    let whitespace = proposal_regex(r"\s+")?;
    let (gate, rest) = if let Some(captures) = prefix.captures(reason) {
        let full = captures.get(0).map_or(0, |entry| entry.end());
        let raw_gate = captures
            .get(1)
            .map_or("review", |entry| entry.as_str())
            .to_ascii_lowercase();
        (
            whitespace.replace_all(&raw_gate, "-").into_owned(),
            reason.get(full..).unwrap_or(""),
        )
    } else {
        ("review".to_owned(), reason)
    };
    let quoted = proposal_regex(r#"`[^`]+`|'[^']+'|"[^"]+""#)?;
    let path = proposal_regex(
        r"(?:[A-Za-z]:[\\/]|\.{0,2}[\\/])?[A-Za-z0-9_@.-]+(?:[\\/][A-Za-z0-9_@.-]+)+",
    )?;
    let filename = proposal_regex(r"(?i)\b[A-Za-z0-9_.-]+\.(?:[cm]?[jt]sx?|md|ya?ml|json)\b")?;
    let verb_ident = proposal_regex(r"\b(adds|exports|extend|role of)\s+[$A-Za-z_][$A-Za-z0-9_]*")?;
    let ident = proposal_regex(r"\b[a-z]+(?:[A-Z][A-Za-z0-9]*)+\b|\b[a-z]+(?:_[a-z0-9]+)+\b")?;
    let numbers = proposal_regex(r"\b\d+(?:\.\d+)?\b")?;
    let normalized = quoted.replace_all(rest, "<identifier>");
    let normalized = path.replace_all(&normalized, "<path>");
    let normalized = filename.replace_all(&normalized, "<path>");
    let normalized = verb_ident.replace_all(&normalized, "$1 <identifier>");
    let normalized = ident.replace_all(&normalized, "<identifier>");
    let normalized = numbers.replace_all(&normalized, "<number>");
    let normalized = normalized.to_lowercase();
    let normalized = whitespace.replace_all(&normalized, " ");
    Ok((gate, normalized.trim().to_owned()))
}

type NumericKey = (Vec<(u8, usize, Vec<u8>)>, Vec<u8>);

fn numeric_collation_key(source: &str) -> Result<NumericKey, Error> {
    let mut primary = Vec::new();
    let mut tertiary = Vec::new();
    let mut index = 0;
    let bytes = source.as_bytes();
    while index < bytes.len() {
        let is_digit = bytes[index].is_ascii_digit();
        let start = index;
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_digit() == is_digit {
            index += 1;
        }
        let slice = source
            .get(start..index)
            .ok_or_else(|| Error::type_error("Invalid numeric collation slice"))?;
        if is_digit {
            let trimmed = slice.trim_start_matches('0');
            let significant = if trimmed.is_empty() { "0" } else { trimmed };
            primary.push((40_u8, significant.len(), significant.as_bytes().to_vec()));
            tertiary.push(0_u8);
        } else {
            let (slice_primary, slice_tertiary) = collation_key(&Text::from(slice))?;
            for weight in slice_primary {
                primary.push((weight, 0_usize, Vec::new()));
            }
            tertiary.extend(slice_tertiary);
        }
    }
    Ok((primary, tertiary))
}

pub fn failure_signatures(records: &[Value]) -> Result<Vec<Value>, Error> {
    if records.len() > MAX_ROWS {
        return Err(error(
            "ROSTER_ANALYTICS_SIZE",
            "Failure signature history exceeds 10000 records",
        ));
    }
    struct Group {
        key: String,
        gate: String,
        reason: String,
        occurrences: Vec<(String, Value)>,
    }
    let mut groups: Vec<Group> = Vec::new();
    for record in records {
        if domain::members(record).is_none() {
            return Err(Error::type_error("Failure history requires object records"));
        }
        let evaluation = optional(record, "evaluation");
        let eval_failed = evaluation.is_some_and(|eval| {
            domain::is_text(eval.get("verdict"), "reject")
                || domain::is_text(eval.get("verdict"), "rework")
        });
        if !estimate::excellence_failed(record) && !eval_failed {
            continue;
        }
        let mut reasons = Vec::new();
        for source in [
            optional(record, "defects"),
            evaluation.and_then(|eval| optional(eval, "defects")),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(items) = domain::items(source) {
                for item in items {
                    if let Some(reason) = item.as_string() {
                        reasons.push(reason.to_utf8()?);
                    }
                }
            }
        }
        if eval_failed {
            if let Some(comment) = evaluation
                .and_then(|eval| text(eval, "comment"))
                .filter(|comment| !trim(&comment.0).is_empty())
            {
                reasons.push(comment.to_utf8()?);
            }
        }
        let seat = learning_seat(record)?.to_utf8()?;
        let identity = if let Some(task) = text(record, "task").filter(|task| !task.0.is_empty()) {
            Some(format!("task:{}", task.to_utf8()?))
        } else if let Some(session) =
            text(record, "session").filter(|session| !session.0.is_empty())
        {
            Some(format!("session:{}", session.to_utf8()?))
        } else if let Some(sha) = text(record, "sha").filter(|sha| !sha.0.is_empty()) {
            Some(format!("sha:{}", sha.to_utf8()?.to_ascii_lowercase()))
        } else {
            None
        };
        let Some(identity) = identity else {
            continue;
        };
        let task_text = text(record, "task").map(Text::to_utf8).transpose()?;
        let issue = task_text
            .as_deref()
            .and_then(|task| task.strip_prefix("issue-"))
            .filter(|digits| {
                !digits.is_empty()
                    && !digits.starts_with('0')
                    && digits.bytes().all(|byte| byte.is_ascii_digit())
            })
            .map(str::to_owned);
        let occurrence_key = format!("{identity}:{seat}");
        for reason in reasons {
            if reason.trim().is_empty() {
                continue;
            }
            let (gate, normalized) = failure_signature(&reason)?;
            if normalized.is_empty() {
                continue;
            }
            let key = format!("{gate}:{normalized}");
            let group = if let Some(existing) = groups.iter_mut().find(|group| group.key == key) {
                existing
            } else {
                groups.push(Group {
                    key,
                    gate,
                    reason: normalized,
                    occurrences: Vec::new(),
                });
                groups
                    .last_mut()
                    .ok_or_else(|| Error::type_error("Missing failure signature group"))?
            };
            if !group
                .occurrences
                .iter()
                .any(|(existing, _)| *existing == occurrence_key)
            {
                let occurrence = Value::object(vec![
                    ("issue", issue.as_deref().map_or(Value::Null, Value::string)),
                    (
                        "task",
                        task_text.as_deref().map_or(Value::Null, Value::string),
                    ),
                    (
                        "session",
                        text(record, "session")
                            .cloned()
                            .map_or(Value::Null, Value::String),
                    ),
                    (
                        "model",
                        text(record, "model")
                            .cloned()
                            .map_or_else(|| Value::string("unknown"), Value::String),
                    ),
                    ("seat", Value::string(&seat)),
                ]);
                group.occurrences.push((occurrence_key.clone(), occurrence));
            }
        }
    }
    let principal_pattern = proposal_regex(r"policy|scope|secret|permission")?;
    let mut output = Vec::with_capacity(groups.len());
    for group in groups {
        let slug_prefix: String = group
            .gate
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .take(40)
            .collect();
        let digest = crate::identity::sha256_hex(group.key.as_bytes());
        let slug = format!(
            "{slug_prefix}-{}",
            digest.get(..16).unwrap_or(digest.as_str())
        );
        let target = if principal_pattern.is_match(&format!("{} {}", group.gate, group.reason)) {
            "principal"
        } else {
            "skill"
        };
        let mut occurrences = Vec::with_capacity(group.occurrences.len());
        for (_, occurrence) in group.occurrences {
            let primary = text(&occurrence, "issue")
                .or_else(|| text(&occurrence, "task"))
                .or_else(|| text(&occurrence, "session"))
                .map(Text::to_utf8)
                .transpose()?
                .unwrap_or_default();
            let seat = text(&occurrence, "seat")
                .map(Text::to_utf8)
                .transpose()?
                .unwrap_or_default();
            let key = numeric_collation_key(&format!("{primary}:{seat}"))?;
            occurrences.push((key, occurrence));
        }
        occurrences.sort_by(|left, right| left.0.cmp(&right.0));
        output.push((
            slug.clone(),
            Value::object(vec![
                ("gate", Value::string(&group.gate)),
                ("reason", Value::string(&group.reason)),
                ("slug", Value::string(&slug)),
                ("target", Value::string(target)),
                (
                    "occurrences",
                    Value::Array(occurrences.into_iter().map(|(_, value)| value).collect()),
                ),
            ]),
        ));
    }
    output.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(output.into_iter().map(|(_, value)| value).collect())
}

fn safe_proposal_text(
    value: Option<&Value>,
    redaction: &redaction::Config,
) -> Result<String, Error> {
    let raw = match value.filter(|value| **value != Value::Null) {
        Some(value) => domain::string(value)?,
        None => Text::from("unknown"),
    };
    let redacted = redaction::diagnostic(&memory::redact(&raw, redaction), redaction).to_utf8()?;
    let mut escaped = String::with_capacity(redacted.len());
    for character in redacted.chars() {
        let character = if matches!(character, '\x00'..='\x1f' | '\x7f') {
            ' '
        } else {
            character
        };
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '<' | '>' | '#' | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    Ok(escaped)
}

pub fn render_failure_proposal(
    proposal: &Value,
    redaction: &redaction::Config,
) -> Result<String, Error> {
    if domain::members(proposal).is_none() {
        return Err(Error::type_error("Proposal must be a JSON object"));
    }
    let target = match proposal.get("target").and_then(Value::as_string) {
        Some(target) if ascii(&target.0, "principal") => "principal",
        Some(target) if ascii(&target.0, "skill") => "skill",
        _ => {
            return Err(Error::type_error(
                "Proposal target must be skill or principal",
            ));
        }
    };
    let gate = safe_proposal_text(proposal.get("gate"), redaction)?;
    let reason = safe_proposal_text(proposal.get("reason"), redaction)?;
    let occurrences = proposal
        .get("occurrences")
        .and_then(domain::items)
        .ok_or_else(|| Error::type_error("Proposal occurrences must be an array"))?;
    let mut lines = Vec::with_capacity(occurrences.len());
    for entry in occurrences {
        if domain::members(entry).is_none() {
            return Err(Error::type_error("Proposal occurrence must be an object"));
        }
        let issue = match optional(entry, "issue") {
            Some(issue) => format!("#{}", safe_proposal_text(Some(issue), redaction)?),
            None => "unknown".to_owned(),
        };
        lines.push(format!(
            "- Issue: {issue}; task: {}; session: {}; model: {}; seat: {}",
            safe_proposal_text(entry.get("task"), redaction)?,
            safe_proposal_text(entry.get("session"), redaction)?,
            safe_proposal_text(entry.get("model"), redaction)?,
            safe_proposal_text(entry.get("seat"), redaction)?
        ));
    }
    bounded_output(format!(
        "# Proposed {target}: {gate}\n\nStatus: draft; human review required.\nTarget: {target}\nOccurrences: {} distinct slice/seat identities.\n\n## Failure signature\n\n{gate}: {reason}\n\n## Evidence\n\n{}\n\n## Proposed rule\n\nBefore completing this seat, verify that the {gate} gate has evidence resolving the recurring condition \"{reason}\". If it remains unmet, report the concrete blocker and retain the existing scope and policy boundaries rather than claiming success.\n\n## Human promotion\n\nReview the cited outcomes, revise this draft into an actionable rule, and choose the appropriate repository skill packet or principal note. Promotion is a human-owned move/edit; this command never installs a rule, grants capabilities, modifies routing, or changes active skills/principals. After promotion or rejection, move this draft out of the proposals directory.\n",
        occurrences.len(),
        lines.join("\n")
    ))
}

pub fn format_failure_proposals(names: &[String]) -> Result<String, Error> {
    if names.is_empty() {
        return Ok(String::new());
    }
    bounded_output(format!(
        "Open improvement proposals ({}):\n{}\n",
        names.len(),
        names
            .iter()
            .map(|name| format!("- {name}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

pub fn format_recommendation(
    recommendation: Option<&Value>,
    task_class: &str,
    config: &Value,
    environment: &Value,
) -> Result<String, Error> {
    let Some(recommendation) = recommendation else {
        let llm = config.get("llm");
        let model = llm
            .and_then(|llm| llm.get("model"))
            .filter(|model| domain::truthy(model))
            .or_else(|| {
                environment
                    .get("ROSTER_MODEL")
                    .filter(|model| domain::truthy(model))
            });
        let model = model
            .map_or_else(|| Ok(Text::from("(unset)")), domain::string)?
            .to_utf8()?;
        if model != "(unset)"
            && (!crate::run::is_model_id(&model)
                || model.contains("://")
                || std::path::Path::new(&model).is_absolute())
        {
            return Err(error(
                "ROSTER_ANALYTICS_INPUT",
                "Config default model must be a model identifier, not a private path or endpoint",
            ));
        }
        let effort = display(llm.and_then(|llm| llm.get("effort")))?.to_utf8()?;
        return bounded_output(format!(
            "insufficient data; config default: {model} effort={effort}\n"
        ));
    };
    let field = |name| display(recommendation.get(name)).and_then(|text| text.to_utf8());
    let rate = recommendation
        .get("acceptRate")
        .and_then(Value::as_number)
        .ok_or_else(|| Error::type_error("Recommendation rate must be numeric"))?;
    bounded_output(format!(
        "{task_class}: {} effort={} accept-rate={}% n={} median-min={} median-difficulty={}\n",
        field("model")?,
        field("effort")?,
        feedback::percentage(rate * 100.0)?,
        field("n")?,
        field("medianMinutes")?,
        field("medianDifficulty")?
    ))
}

pub mod pilot;

#[cfg(test)]
mod tests;
