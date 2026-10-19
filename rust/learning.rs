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

#[cfg(test)]
mod tests;
