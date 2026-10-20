use std::collections::BTreeMap;

use super::*;

pub fn infer_class(title: &str) -> Result<Option<String>, Error> {
    Ok(regex(r"^(feat|fix|docs|test)(?:$|[-_.:]|\([^)]+\)!?:|!:)")?
        .captures(title)
        .and_then(|captures| captures.get(1).map(|entry| entry.as_str().to_owned())))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Estimate {
    pub difficulty: u8,
    pub minutes: u64,
    pub task_class: String,
    pub model: String,
    pub source: String,
    pub samples: usize,
    pub accepted: usize,
    pub confidence: &'static str,
}

impl Estimate {
    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("difficulty", Value::Number(f64::from(self.difficulty))),
            ("estimate_min", Value::Number(self.minutes as f64)),
            ("task_class", value(&self.task_class)),
            ("model", value(&self.model)),
            ("source", value(&self.source)),
            ("n", Value::Number(self.samples as f64)),
            ("accepted", Value::Number(self.accepted as f64)),
            ("confidence", Value::string(self.confidence)),
        ])
    }

    pub fn metadata_text(&self) -> String {
        format!(
            "difficulty: {}\nestimate_min: {}\ntask_class: {}\nmodel: {}",
            self.difficulty, self.minutes, self.task_class, self.model
        )
    }

    pub fn render(&self) -> Text {
        let evidence = if self.source == "recommendation" {
            format!(
                "Recommendation samples: {}\nAccepted samples: {}\n",
                self.samples, self.accepted
            )
        } else {
            format!(
                "Matching timed evaluations: {}\nAccepted timed evaluations: {}\n",
                self.samples, self.accepted
            )
        };
        decode(&format!(
            "# Estimate\n\n{}\nconfidence: {}\n\nSource: {}\n{evidence}\nA story-point style estimate, not a delivery promise. Compare with human-reported actuals.\n",
            self.metadata_text(),
            self.confidence,
            self.source
        ))
    }
}

fn default_number(input: &Value, name: &str, fallback: u64) -> Option<u64> {
    if input.get(name).is_none() {
        Some(fallback)
    } else {
        domain::safe_integer(input.get(name))
    }
}

fn optional_text(input: &Value, field: &str) -> Option<String> {
    input.get(field).and_then(Value::as_string).map(encode)
}

fn confidence(samples: usize, accepted: usize) -> &'static str {
    if samples < 3 || accepted < 1 {
        "low"
    } else if accepted >= 5 {
        "high"
    } else {
        "medium"
    }
}

pub fn excellence_failed(input: &Value) -> bool {
    domain::is_text(input.get("excellence"), "fail")
        || input
            .get("excellence")
            .and_then(|report| report.get("pass"))
            == Some(&Value::Boolean(false))
        || input
            .get("defects")
            .and_then(domain::items)
            .is_some_and(|items| !items.is_empty())
}

pub fn validate_evaluation(input: &Value) -> Result<(), Error> {
    validate_history(input, true)
}

pub(crate) fn validate_run(input: &Value) -> Result<(), Error> {
    validate_history(input, false)
}

fn validate_history(input: &Value, evaluation: bool) -> Result<(), Error> {
    let fail = |message: &str| Error::new("Error", format!("Estimation history: {message}"));
    if domain::members(input).is_none() {
        return Err(fail("expected a JSON object"));
    }
    if input.get("sha").is_none_or(|value| value == &Value::Null)
        && input
            .get("session")
            .is_none_or(|value| value == &Value::Null)
    {
        return Err(fail(if evaluation {
            "an evaluation needs a sha or session"
        } else {
            "a run needs a sha or session"
        }));
    }
    if let Some(sha) = input.get("sha").filter(|value| **value != Value::Null) {
        if !sha.as_string().is_some_and(|value| {
            let text = encode(value);
            matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Err(fail(
                "sha must be a full 40- or 64-character hexadecimal Git SHA",
            ));
        }
    }
    for field in ["session", "task", "seat"] {
        if let Some(value) = input.get(field).filter(|value| **value != Value::Null) {
            let pattern = regex(r"^[A-Za-z0-9._-]{1,64}$")?;
            if !value
                .as_string()
                .is_some_and(|value| encode(value) != "-" && pattern.is_match(&encode(value)))
            {
                return Err(fail(&format!(
                    "{field} must be an opaque 1-64 character identifier"
                )));
            }
        }
    }
    if evaluation {
        if !["accept", "reject", "rework"]
            .iter()
            .any(|expected| domain::is_text(input.get("verdict"), expected))
        {
            return Err(fail("verdict must be accept, reject, or rework"));
        }
        if !domain::safe_integer(input.get("difficulty"))
            .is_some_and(|number| (1..=5).contains(&number))
        {
            return Err(fail("difficulty must be an integer from 1 to 5"));
        }
        if !matches!(input.get("again"), Some(Value::Boolean(_))) {
            return Err(fail("again must be a boolean"));
        }
    }
    if let Some(value) = input.get("model").filter(|value| **value != Value::Null) {
        if !value
            .as_string()
            .is_some_and(|value| !value.0.is_empty() && !value.0.iter().any(|unit| is_space(*unit)))
        {
            return Err(fail("model must be a nonempty name without whitespace"));
        }
    }
    if let Some(value) = input.get("provider").filter(|value| **value != Value::Null) {
        if ![
            "vllm",
            "github-copilot",
            "anthropic",
            "openai",
            "local",
            "other",
        ]
        .iter()
        .any(|name| domain::is_text(Some(value), name))
        {
            return Err(fail("provider must name a supported model backend"));
        }
    }
    for (field, options, message) in [
        (
            "task_class",
            &["feat", "fix", "docs", "test"][..],
            "task_class must be feat, fix, docs, or test",
        ),
        (
            "effort",
            &["l", "m", "h", "x"][..],
            "effort must be l, m, h, x, or null",
        ),
    ] {
        if input.get(field).is_some_and(|value| {
            value != &Value::Null
                && !options
                    .iter()
                    .any(|name| domain::is_text(Some(value), name))
        }) {
            return Err(fail(message));
        }
    }
    for field in ["minutes", "prompt_tokens", "completion_tokens"] {
        if input.get(field).is_some_and(|value| {
            value != &Value::Null && domain::safe_integer(Some(value)).is_none()
        }) {
            let message = if field == "minutes" {
                "minutes must be a nonnegative integer".to_owned()
            } else {
                format!("{field} must be a reported nonnegative safe integer")
            };
            return Err(fail(&message));
        }
    }
    for (field, alias) in [
        ("context_used", Some("prompt_tokens")),
        ("context_max", None),
        ("context_out", Some("completion_tokens")),
    ] {
        if let Some(value) = input.get(field).filter(|value| **value != Value::Null) {
            if domain::safe_integer(Some(value)).is_none()
                && !value.as_string().is_some_and(|value| {
                    !value.0.is_empty() && value.0.iter().all(|unit| (48..=57).contains(unit))
                })
            {
                return Err(fail(&format!(
                    "{field} must be a nonnegative integer or decimal string"
                )));
            }
            if let Some(other) = alias
                .and_then(|alias| input.get(alias))
                .filter(|value| **value != Value::Null)
            {
                if domain::string(value)? != domain::string(other)? {
                    return Err(fail("token aliases must match reported usage"));
                }
            }
        }
    }
    if let Some(value) = input
        .get("excellence")
        .filter(|value| **value != Value::Null)
    {
        if !domain::is_text(Some(value), "pass")
            && !domain::is_text(Some(value), "fail")
            && !matches!(value.get("pass"), Some(Value::Boolean(_)))
        {
            return Err(fail(
                "excellence must be pass, fail, or a report with boolean pass",
            ));
        }
    }
    if let Some(value) = input.get("defects").filter(|value| **value != Value::Null) {
        if domain::items(value).is_none_or(|items| {
            items.iter().any(|value| {
                value.as_string().is_none_or(|value| {
                    crate::text::trim(&value.0).is_empty()
                        || value
                            .0
                            .iter()
                            .any(|unit| matches!(unit, 0..=8 | 11 | 12 | 14..=31 | 127))
                })
            })
        }) {
            return Err(fail(
                "defects must be an array of nonempty failure reasons without control characters",
            ));
        }
    }
    if let Some(comment) = input.get("comment").filter(|value| **value != Value::Null) {
        if comment.as_string().is_none_or(|value| {
            value.utf8_len() > 4096
                || value
                    .0
                    .iter()
                    .any(|unit| matches!(unit, 0..=8 | 11 | 12 | 14..=31 | 127))
        }) {
            return Err(fail(
                "comment must be text of at most 4 KiB without control characters",
            ));
        }
    }
    if let Some(at) = input.get("at").filter(|value| **value != Value::Null) {
        if timestamp(at).is_none() {
            return Err(fail("at must be an ISO timestamp"));
        }
    }
    if let Some(attempt) = input.get("attempt") {
        validate_attempt(attempt)?;
    }
    if let Some(delivery) = input
        .get("delivery")
        .filter(|delivery| **delivery != Value::Null)
    {
        validate_delivery(delivery)?;
    }
    Ok(())
}

fn validate_attempt(input: &Value) -> Result<(), Error> {
    let fail = || Error::type_error("Invalid bounded attempt evidence");
    if fields(
        input,
        &[],
        &[
            "batch",
            "count",
            "index",
            "profile",
            "hardware",
            "gates",
            "review",
            "changed_lines",
            "duration_ms",
            "winner",
        ],
        "",
    )
    .is_err()
    {
        return Err(fail());
    }
    let count = domain::safe_integer(input.get("count"))
        .filter(|count| (2..=16).contains(count))
        .ok_or_else(fail)?;
    if !input
        .get("batch")
        .and_then(Value::as_string)
        .is_some_and(|batch| {
            batch.0.len() == 16
                && batch
                    .0
                    .iter()
                    .all(|unit| (48..=57).contains(unit) || (97..=102).contains(unit))
        })
    {
        return Err(fail());
    }
    if !domain::safe_integer(input.get("index")).is_some_and(|index| index >= 1 && index <= count) {
        return Err(fail());
    }
    if !input
        .get("profile")
        .and_then(Value::as_string)
        .is_some_and(|profile| {
            let profile = encode(profile);
            profile.len() <= 64
                && profile
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && profile
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
    {
        return Err(fail());
    }
    if input
        .get("hardware")
        .and_then(Value::as_string)
        .is_none_or(|hardware| {
            hardware.0.is_empty()
                || hardware.0.len() > 120
                || hardware.0.iter().any(|unit| matches!(unit, 0..=31 | 127))
        })
    {
        return Err(fail());
    }
    let gates = input.get("gates").ok_or_else(fail)?;
    if fields(
        gates,
        &["excellence", "red_green", "shadow", "tests"],
        &[],
        "",
    )
    .is_err()
        || domain::members(gates).is_none_or(|entries| {
            entries.iter().any(|(_, value)| {
                !["pass", "fail", "skipped"]
                    .iter()
                    .any(|gate| domain::is_text(Some(value), gate))
            })
        })
    {
        return Err(fail());
    }
    if !["pass", "fail", "escalate", "unavailable"]
        .iter()
        .any(|review| domain::is_text(input.get("review"), review))
        || domain::safe_integer(input.get("changed_lines")).is_none()
        || domain::safe_integer(input.get("duration_ms")).is_none()
        || domain::safe_integer(input.get("winner")).is_none_or(|winner| winner > count)
    {
        return Err(fail());
    }
    Ok(())
}

pub(crate) fn timestamp(input: &Value) -> Option<i64> {
    let source = input.as_string()?.to_utf8().ok()?;
    let year_length = match source.len() {
        24 => 4,
        27 if source.starts_with(['+', '-']) => 7,
        _ => return None,
    };
    let digits = |start: usize, end: usize| -> Option<i64> {
        let text = source.get(start..end)?;
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    let mut year = if year_length == 4 {
        digits(0, 4)?
    } else {
        digits(1, 7)?.checked_mul(if source.starts_with('-') { -1 } else { 1 })?
    };
    if year_length == 7 && (0..=9999).contains(&year) {
        return None;
    }
    let tail = year_length;
    for (offset, separator) in [
        (0, "-"),
        (3, "-"),
        (6, "T"),
        (9, ":"),
        (12, ":"),
        (15, "."),
        (19, "Z"),
    ] {
        if source.get(tail + offset..tail + offset + 1) != Some(separator) {
            return None;
        }
    }
    let month = digits(tail + 1, tail + 3)?;
    let day = digits(tail + 4, tail + 6)?;
    let hour = digits(tail + 7, tail + 9)?;
    let minute = digits(tail + 10, tail + 12)?;
    let second = digits(tail + 13, tail + 15)?;
    let fraction = digits(tail + 16, tail + 19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = if month == 2 && leap {
        29
    } else {
        *[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31].get(usize::try_from(month - 1).ok()?)?
    };
    if day > month_days {
        return None;
    }
    if month <= 2 {
        year = year.checked_sub(1)?;
    }
    let era = year.div_euclid(400);
    let year_of_era = year.checked_sub(era.checked_mul(400)?)?;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era
        .checked_mul(146097)?
        .checked_add(day_of_era)?
        .checked_sub(719468)?;
    let millis = days
        .checked_mul(86_400_000)?
        .checked_add(hour * 3_600_000)?
        .checked_add(minute * 60_000)?
        .checked_add(second * 1000)?
        .checked_add(fraction)?;
    if !(-8_640_000_000_000_000..=8_640_000_000_000_000).contains(&millis) {
        return None;
    }
    Some(millis)
}

fn validate_delivery(input: &Value) -> Result<(), Error> {
    validate_delivery_with_source(input, "Estimation history")
}

pub(crate) fn validate_delivery_with_source(input: &Value, source: &str) -> Result<(), Error> {
    let fail = |message: &str| Error::type_error(format!("{source}: {message}"));
    let entries = domain::members(input).ok_or_else(|| fail("delivery must be an object"))?;
    for (name, _) in entries {
        let name = encode(name);
        if ![
            "id",
            "attempt",
            "hardware",
            "duration_ms",
            "estimate_min",
            "ask_created_at",
            "pr_merged_at",
            "review_verdict",
            "review_repairs",
            "gates",
        ]
        .contains(&name.as_str())
        {
            return Err(text_error(
                "TypeError",
                &format!("{source}: unsupported delivery field {name}"),
            ));
        }
    }
    let id = regex(r"^[A-Za-z0-9._-]{1,64}$")?;
    if !input
        .get("id")
        .and_then(Value::as_string)
        .is_some_and(|value| id.is_match(&encode(value)))
    {
        return Err(fail("delivery id must be an opaque identifier"));
    }
    if domain::safe_integer(input.get("attempt")).is_none_or(|value| value < 1) {
        return Err(fail("delivery attempt must be a positive integer"));
    }
    if input.get("hardware").is_some_and(|value| {
        value != &Value::Null
            && value.as_string().is_none_or(|value| {
                crate::text::trim(&value.0).is_empty()
                    || value.0.len() > 120
                    || value.0.iter().any(|unit| matches!(unit, 0..=31 | 127))
            })
    }) {
        return Err(fail("delivery hardware must be bounded plain text"));
    }
    for field in ["duration_ms", "estimate_min", "review_repairs"] {
        if let Some(value) = input.get(field).filter(|value| **value != Value::Null) {
            if value.as_number().is_none_or(|number| {
                !number.is_finite()
                    || number < 0.0
                    || field != "duration_ms" && domain::safe_integer(Some(value)).is_none()
            }) {
                return Err(fail(&format!("delivery {field} must be nonnegative")));
            }
        }
    }
    for field in ["ask_created_at", "pr_merged_at"] {
        if input
            .get(field)
            .is_some_and(|value| value != &Value::Null && timestamp(value).is_none())
        {
            return Err(fail(&format!("delivery {field} must be an ISO timestamp")));
        }
    }
    if let (Some(created), Some(merged)) = (
        input.get("ask_created_at").and_then(timestamp),
        input.get("pr_merged_at").and_then(timestamp),
    ) {
        if merged < created {
            return Err(fail("PR merge precedes ask creation"));
        }
    }
    if input.get("review_verdict").is_some_and(|value| {
        value != &Value::Null
            && !["pass", "fail", "escalate"]
                .iter()
                .any(|verdict| domain::is_text(Some(value), verdict))
    }) {
        return Err(fail(
            "delivery review_verdict must be pass, fail, or escalate",
        ));
    }
    if let Some(gates) = input.get("gates").filter(|value| **value != Value::Null) {
        let entries =
            domain::members(gates).ok_or_else(|| fail("delivery gates must be an object"))?;
        for (name, counts) in entries {
            let name = encode(name);
            let checks = domain::safe_integer(counts.get("checks"));
            let failures = domain::safe_integer(counts.get("failures"));
            if !["red-green", "shadow", "self-review", "verifying-reviewer"]
                .contains(&name.as_str())
                || fields(counts, &["checks", "failures"], &[], "").is_err()
                || checks.is_none_or(|checks| checks < 1)
                || failures.is_none()
                || failures > checks
            {
                return Err(fail("invalid delivery gate counts"));
            }
        }
    }
    Ok(())
}

pub fn estimate(
    metadata: &Value,
    evaluations: &[Value],
    default_model: &str,
) -> Result<Estimate, Error> {
    let difficulty = default_number(metadata, "difficulty", 2)
        .filter(|value| (1..=5).contains(value))
        .ok_or_else(|| Error::type_error("Task difficulty must be an integer from 1 to 5"))?;
    let minutes = default_number(metadata, "estimate_min", 15)
        .ok_or_else(|| Error::type_error("Task estimate_min must be a nonnegative integer"))?;
    let class = if metadata.get("task_class").is_none() {
        "feat".to_owned()
    } else {
        optional_text(metadata, "task_class").unwrap_or_default()
    };
    if !["feat", "fix", "docs", "test"].contains(&class.as_str()) {
        return Err(Error::type_error(
            "Task task_class must be feat, fix, docs, or test",
        ));
    }
    let raw_model = match metadata.get("model") {
        None => String::new(),
        Some(Value::String(value)) => encode(value),
        _ => {
            return Err(Error::type_error(
                "Task model must be a served model id or empty",
            ));
        }
    };
    let model = if raw_model.is_empty() {
        default_model.to_owned()
    } else {
        raw_model
    };
    if !model.is_empty()
        && (!matches(r"^[A-Za-z0-9._:/-]+$", &model)?
            || ["unknown", "builtin-stub"].contains(&model.as_str()))
    {
        return Err(Error::type_error(
            "Task model must be a served model id or empty",
        ));
    }
    let mut session_shas = BTreeMap::new();
    for evaluation in evaluations {
        validate_evaluation(evaluation)?;
        if let (Some(sha), Some(session)) = (
            optional_text(evaluation, "sha"),
            optional_text(evaluation, "session"),
        ) {
            session_shas.insert(session, sha.to_lowercase());
        }
    }
    let mut latest: Vec<(String, &Value)> = Vec::new();
    for evaluation in evaluations {
        let session = optional_text(evaluation, "session");
        let sha = optional_text(evaluation, "sha")
            .map(|sha| sha.to_lowercase())
            .or_else(|| {
                session
                    .as_ref()
                    .and_then(|session| session_shas.get(session).cloned())
            });
        let target = sha.map_or_else(
            || format!("session:{}", session.as_deref().unwrap_or("undefined")),
            |sha| format!("sha:{sha}"),
        );
        if let Some((_, prior)) = latest.iter_mut().find(|(name, _)| name == &target) {
            *prior = evaluation;
        } else {
            latest.push((target, evaluation));
        }
    }
    let samples = latest
        .iter()
        .map(|(_, evaluation)| *evaluation)
        .filter(|evaluation| {
            !model.is_empty()
                && optional_text(evaluation, "model").as_deref() == Some(&model)
                && optional_text(evaluation, "task_class").as_deref() == Some(&class)
                && evaluation
                    .get("minutes")
                    .is_some_and(|value| value != &Value::Null)
        })
        .collect::<Vec<_>>();
    let mut accepted = samples
        .iter()
        .filter(|evaluation| {
            domain::is_text(evaluation.get("verdict"), "accept") && !excellence_failed(evaluation)
        })
        .map(|evaluation| {
            domain::safe_integer(evaluation.get("minutes"))
                .ok_or_else(|| Error::type_error("Invalid timed evaluation"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    accepted.sort_unstable();
    let history = samples.len() >= 3 && !accepted.is_empty();
    let minutes = if history {
        let middle = accepted.len() / 2;
        let upper = *accepted
            .get(middle)
            .ok_or_else(|| Error::type_error("Missing median sample"))?;
        if accepted.len() % 2 == 1 {
            upper
        } else {
            let lower = *accepted
                .get(middle - 1)
                .ok_or_else(|| Error::type_error("Missing median sample"))?;
            (lower as f64 + (upper - lower) as f64 / 2.0).round() as u64
        }
    } else {
        minutes
    };
    Ok(Estimate {
        difficulty: difficulty as u8,
        minutes,
        task_class: class,
        model,
        source: if history { "history" } else { "task/default" }.to_owned(),
        samples: samples.len(),
        accepted: accepted.len(),
        confidence: confidence(samples.len(), accepted.len()),
    })
}

fn read_metadata(header: &str) -> Result<Value, Error> {
    let mut metadata = Value::object(Vec::new());
    let pattern =
        regex(r"(?i)^(?:-[ \t]+)?(difficulty|estimate_min|task_class|model):[ \t]*(.*)$")?;
    for line in header.split('\n') {
        let Some(captures) = pattern.captures(trim(line)) else {
            continue;
        };
        let field = captures
            .get(1)
            .map_or("", |entry| entry.as_str())
            .to_lowercase();
        let raw = captures.get(2).map_or("", |entry| entry.as_str());
        if metadata.get(&field).is_some() {
            return Err(Error::type_error(format!("TASK.md has duplicate {field}")));
        }
        let parsed = if ["difficulty", "estimate_min"].contains(&field.as_str()) {
            if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(Error::type_error(format!(
                    "Task {field} must be an integer"
                )));
            }
            Value::Number(
                raw.parse::<f64>()
                    .map_err(|_| Error::type_error(format!("Task {field} must be an integer")))?,
            )
        } else {
            value(raw)
        };
        domain::put(&mut metadata, &field, parsed)?;
    }
    if metadata.get("task_class").is_none() {
        let first = header
            .strip_prefix("# Task: ")
            .unwrap_or(header)
            .split('\n')
            .next()
            .unwrap_or("");
        domain::put(
            &mut metadata,
            "task_class",
            Value::string(infer_class(first)?.as_deref().unwrap_or("feat")),
        )?;
    }
    Ok(metadata)
}

fn metadata_from_parts(parts: &task::Parts) -> Result<Value, Error> {
    let header = parts
        .header
        .split('\n')
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n");
    read_metadata(&format!(
        "# Task: {}\n{header}\n{}",
        parts.title,
        parts
            .section("metadata")
            .map_or("", |section| section.content.as_str())
    ))
}

pub fn read_task_metadata(input: &Value) -> Result<Estimate, Error> {
    estimate(&metadata_from_parts(&task::sections(input)?)?, &[], "")
}

pub fn update_task_metadata(input: &Value, metadata: &Value) -> Result<Text, Error> {
    let parts = task::sections(input)?;
    let values = estimate(
        &domain::merge(&metadata_from_parts(&parts)?, metadata)?,
        &[],
        "",
    )?;
    let remove =
        regex(r"(?im)^(?:-[ \t]+)?(?:difficulty|estimate_min|task_class|model):[^\n]*\n?")?;
    let header = remove.replace_all(&parts.header, "");
    let mut body = String::new();
    for section in &parts.sections {
        if section.name == "metadata" {
            body.push_str(&remove.replace_all(&section.source, ""));
        } else {
            body.push_str(&section.source);
        }
    }
    Ok(decode(&format!(
        "{}{}\n\n{}\n\n{body}",
        parts.frontmatter,
        trim_end(&header),
        values.metadata_text()
    )))
}

pub fn needs_reestimate(input: Option<&Value>, current: &Value) -> Result<bool, Error> {
    let parse = |input: Option<&Value>| -> Result<Vec<String>, Error> {
        let items = input.and_then(domain::items).ok_or_else(|| {
            Error::type_error("Re-estimation files must be arrays of nonempty file paths")
        })?;
        items
            .iter()
            .map(|value| {
                let file = text(
                    value,
                    "Re-estimation files must be arrays of nonempty file paths",
                )?;
                if trim(&file).is_empty() {
                    return Err(Error::type_error(
                        "Re-estimation files must be arrays of nonempty file paths",
                    ));
                }
                Ok(file.replace('\\', "/"))
            })
            .collect()
    };
    let previous = parse(input.and_then(|input| input.get("files")))?;
    Ok(parse(Some(current))?
        .iter()
        .any(|file| !previous.contains(file)))
}

pub fn with_recommendation(
    mut estimate: Estimate,
    recommendation: Option<&Value>,
) -> Result<Estimate, Error> {
    let Some(recommendation) = recommendation.filter(|value| {
        value
            .get("estimate_min")
            .is_some_and(|value| value != &Value::Null)
    }) else {
        return Ok(estimate);
    };
    let n = domain::safe_integer(recommendation.get("n"));
    if optional_text(recommendation, "model").as_deref() != Some(&estimate.model)
        || n.is_none_or(|n| n < 3)
    {
        return Err(Error::new(
            "Error",
            "Recommended estimate must match the selected model with at least three samples",
        ));
    }
    let accepted = domain::safe_integer(recommendation.get("accepted"));
    if accepted.is_none() || accepted > n {
        return Err(Error::type_error(
            "Recommended estimate accepted samples must be an integer from 0 to n",
        ));
    }
    let minutes = recommendation
        .get("estimate_min")
        .cloned()
        .ok_or_else(|| Error::type_error("Missing recommended minutes"))?;
    let validated = self::estimate(
        &domain::merge(
            &estimate.as_value(),
            &Value::object(vec![("estimate_min", minutes)]),
        )?,
        &[],
        "",
    )?;
    estimate.minutes = validated.minutes;
    estimate.source = "recommendation".to_owned();
    estimate.samples = n
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| Error::type_error("Recommendation sample count exceeds native range"))?;
    estimate.accepted = accepted
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| Error::type_error("Recommendation accepted count exceeds native range"))?;
    estimate.confidence = confidence(estimate.samples, estimate.accepted);
    Ok(estimate)
}
