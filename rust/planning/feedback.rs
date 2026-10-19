use super::*;
use crate::state::redaction;

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub evaluations: Vec<Value>,
    pub records: Vec<Value>,
    pub estimation_history: Vec<Value>,
}

impl Snapshot {
    pub fn validate(&self) -> Result<(), Error> {
        if self.evaluations.len() > 10000
            || self.records.len() > 10000
            || self.estimation_history.len() > 10000
        {
            return Err(Error::type_error(
                "Planning learning snapshot exceeds 10000 records per section",
            ));
        }
        for evaluation in self.evaluations.iter().chain(&self.estimation_history) {
            estimate::validate_evaluation(evaluation)?;
        }
        for record in &self.records {
            if domain::members(record).is_none() {
                return Err(Error::type_error(
                    "Planning metrics records must be JSON objects",
                ));
            }
            if let Some(evaluation) = record
                .get("evaluation")
                .filter(|evaluation| **evaluation != Value::Null)
            {
                estimate::validate_evaluation(evaluation)?;
            }
            for field in ["model", "task_class", "task", "effort", "session", "sha"] {
                if record
                    .get(field)
                    .is_some_and(|value| value != &Value::Null && value.as_string().is_none())
                {
                    return Err(Error::type_error(format!(
                        "Planning metrics {field} must be text or null"
                    )));
                }
            }
        }
        Ok(())
    }
}

fn optional_text(input: &Value, field: &str) -> Option<String> {
    input.get(field).and_then(Value::as_string).map(encode)
}

fn matches_evaluation(record: &Value, evaluation: &Value) -> bool {
    match (
        optional_text(record, "sha"),
        optional_text(evaluation, "sha"),
    ) {
        (Some(left), Some(right)) if !left.is_empty() && !right.is_empty() => {
            left.eq_ignore_ascii_case(&right)
        }
        _ => optional_text(record, "session").is_some_and(|session| {
            !session.is_empty() && optional_text(evaluation, "session").as_deref() == Some(&session)
        }),
    }
}

fn class(input: &Value) -> Result<Option<String>, Error> {
    Ok(optional_text(input, "task_class").or(estimate::infer_class(
        &optional_text(input, "task").unwrap_or_default(),
    )?))
}

fn median(values: &[u64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_unstable();
    let middle = values.len() / 2;
    let upper = values.get(middle).copied()? as f64;
    if values.len() % 2 == 1 {
        Some(upper)
    } else {
        let lower = values.get(middle.checked_sub(1)?).copied()? as f64;
        Some(lower + (upper - lower) / 2.0)
    }
}

#[derive(Clone)]
struct Sample {
    target: String,
    verdict: String,
    failed: bool,
    minutes: Option<u64>,
    difficulty: Option<u64>,
}

fn percentage(value: f64) -> Result<String, Error> {
    let fail = || Error::type_error("Recommendation percentage must be a bounded finite rate");
    if !(0.0..=100.0).contains(&value) {
        return Err(fail());
    }
    if value == 0.0 {
        return Ok("0.0".to_owned());
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as u32;
    let mantissa = u128::from((bits & ((1_u64 << 52) - 1)) | (1_u64 << 52));
    let numerator = mantissa.checked_mul(10).ok_or_else(fail)?;
    let denominator = 1_u128
        .checked_shl(1075_u32.checked_sub(exponent).ok_or_else(fail)?)
        .ok_or_else(fail)?;
    let integer = numerator / denominator;
    let remainder = numerator % denominator;
    let rounded = integer
        .checked_add(u128::from(
            remainder.checked_mul(2).ok_or_else(fail)? >= denominator,
        ))
        .ok_or_else(fail)?;
    Ok(format!("{}.{:01}", rounded / 10, rounded % 10))
}

struct Group {
    model: String,
    class: String,
    effort: Option<String>,
    samples: Vec<Sample>,
}

pub fn recommend(
    records: &[Value],
    task_class: &str,
    difficulty: Option<u8>,
) -> Result<Option<Value>, Error> {
    if !["feat", "fix", "docs", "test"].contains(&task_class) {
        return Err(Error::type_error(
            "task class must be feat, fix, docs, or test",
        ));
    }
    if difficulty.is_some_and(|difficulty| !(1..=5).contains(&difficulty)) {
        return Err(Error::type_error(
            "difficulty must be an integer from 1 to 5",
        ));
    }
    if records.len() > 10000 {
        return Err(Error::type_error(
            "Planning metrics snapshot exceeds 10000 records",
        ));
    }
    let mut groups: Vec<Group> = Vec::new();
    for record in records {
        if domain::members(record).is_none() {
            return Err(Error::type_error(
                "Planning metrics records must be JSON objects",
            ));
        }
        for field in ["model", "task_class", "task", "effort", "session", "sha"] {
            if record
                .get(field)
                .is_some_and(|value| value != &Value::Null && value.as_string().is_none())
            {
                return Err(Error::type_error(format!(
                    "Planning metrics {field} must be text or null"
                )));
            }
        }
        let model = optional_text(record, "model").unwrap_or_default();
        let class = class(record)?.unwrap_or_default();
        let effort = optional_text(record, "effort");
        let position = groups.iter().position(|group| {
            group.model == model && group.class == class && group.effort == effort
        });
        let group = if let Some(position) = position {
            groups.get_mut(position)
        } else {
            groups.push(Group {
                model,
                class,
                effort,
                samples: Vec::new(),
            });
            groups.last_mut()
        }
        .ok_or_else(|| Error::type_error("Missing learning recommendation group"))?;
        let evaluation = record
            .get("evaluation")
            .filter(|value| **value != Value::Null);
        let failed = estimate::excellence_failed(record);
        let verdict = if failed {
            Some("reject".to_owned())
        } else {
            evaluation.and_then(|evaluation| optional_text(evaluation, "verdict"))
        };
        let Some(verdict) =
            verdict.filter(|verdict| ["accept", "reject", "rework"].contains(&verdict.as_str()))
        else {
            continue;
        };
        let target = evaluation.unwrap_or(record);
        let target = optional_text(target, "sha")
            .filter(|sha| !sha.is_empty())
            .map(|sha| format!("sha:{}", sha.to_lowercase()))
            .or_else(|| {
                optional_text(target, "session")
                    .filter(|session| !session.is_empty())
                    .map(|session| format!("session:{session}"))
            })
            .ok_or_else(|| Error::type_error("An evaluated result needs a SHA or session"))?;
        let prior = group
            .samples
            .iter()
            .position(|sample| sample.target == target);
        let failed = failed
            || prior
                .and_then(|index| group.samples.get(index))
                .is_some_and(|sample| sample.failed);
        let sample = Sample {
            target,
            verdict: if failed { "reject".to_owned() } else { verdict },
            failed,
            minutes: evaluation
                .and_then(|evaluation| domain::safe_integer(evaluation.get("minutes"))),
            difficulty: evaluation
                .and_then(|evaluation| domain::safe_integer(evaluation.get("difficulty")))
                .filter(|difficulty| (1..=5).contains(difficulty)),
        };
        if let Some(index) = prior {
            let prior = group
                .samples
                .get_mut(index)
                .ok_or_else(|| Error::type_error("Missing recommendation sample"))?;
            *prior = sample;
        } else {
            group.samples.push(sample);
        }
    }
    let mut eligible = Vec::new();
    for group in groups {
        if group.class != task_class
            || group.model.is_empty()
            || ["unknown", "builtin-stub"].contains(&group.model.as_str())
            || group.samples.len() < 3
        {
            continue;
        }
        let difficulties = group
            .samples
            .iter()
            .filter_map(|sample| sample.difficulty)
            .collect::<Vec<_>>();
        let median_difficulty = median(&difficulties);
        if difficulty.is_some_and(|difficulty| {
            median_difficulty.is_none_or(|median| median < f64::from(difficulty))
        }) {
            continue;
        }
        let accepted = group
            .samples
            .iter()
            .filter(|sample| sample.verdict == "accept")
            .collect::<Vec<_>>();
        let accepted_minutes = median(
            &accepted
                .iter()
                .filter_map(|sample| sample.minutes)
                .collect::<Vec<_>>(),
        );
        let rate = accepted.len() as f64 / group.samples.len() as f64;
        let recommendation = Value::object(vec![
            ("model", value(&group.model)),
            (
                "effort",
                group
                    .effort
                    .as_ref()
                    .map_or(Value::Null, |effort| value(effort)),
            ),
            ("n", Value::Number(group.samples.len() as f64)),
            ("accepted", Value::Number(accepted.len() as f64)),
            ("acceptRate", Value::Number(rate)),
            (
                "medianMinutes",
                median(
                    &group
                        .samples
                        .iter()
                        .filter_map(|sample| sample.minutes)
                        .collect::<Vec<_>>(),
                )
                .map_or(Value::Null, Value::Number),
            ),
            (
                "medianDifficulty",
                median_difficulty.map_or(Value::Null, Value::Number),
            ),
            (
                "estimate_min",
                accepted_minutes.map_or(Value::Null, |value| Value::Number(value.round())),
            ),
        ]);
        eligible.push((
            recommendation,
            group.model,
            group.effort,
            group.samples.len(),
            rate,
        ));
    }
    let effort_index = |effort: &Option<String>| {
        ["l", "m", "h", "x"]
            .iter()
            .position(|expected| effort.as_deref() == Some(expected))
            .unwrap_or(4)
    };
    eligible.sort_by(|left, right| {
        right
            .4
            .total_cmp(&left.4)
            .then_with(|| right.3.cmp(&left.3))
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| effort_index(&left.2).cmp(&effort_index(&right.2)))
    });
    Ok(eligible.into_iter().next().map(|entry| entry.0))
}

pub fn apply(
    task: &Text,
    snapshot: &Snapshot,
    config: &Value,
    environment: &Value,
    redaction: &redaction::Config,
) -> Result<Value, Error> {
    snapshot.validate()?;
    let metadata = estimate::read_task_metadata(&Value::String(task.clone()))?;
    let unchanged = || {
        Value::object(vec![
            ("task", Value::String(task.clone())),
            ("feedback", Value::Null),
        ])
    };
    if snapshot.evaluations.is_empty() {
        return Ok(unchanged());
    }
    let mut history = Vec::new();
    for evaluation in &snapshot.evaluations {
        let run = snapshot
            .records
            .iter()
            .find(|record| matches_evaluation(record, evaluation));
        let mut item = evaluation.clone();
        if let Some(run) = run {
            for field in ["model", "effort", "excellence"] {
                if let Some(value) = run.get(field).filter(|value| **value != Value::Null) {
                    domain::put(&mut item, field, value.clone())?;
                }
            }
            if let Some(class) = class(run)? {
                domain::put(&mut item, "task_class", value(&class))?;
            }
        }
        let verdict = if let Some(run) = run {
            run.get("evaluation")
                .and_then(|evaluation| evaluation.get("verdict"))
                .cloned()
                .unwrap_or(Value::Null)
        } else {
            evaluation.get("verdict").cloned().unwrap_or(Value::Null)
        };
        domain::put(&mut item, "currentVerdict", verdict)?;
        if optional_text(&item, "task_class").as_deref() == Some(&metadata.task_class) {
            history.push(item);
        }
    }
    if history.is_empty() {
        return Ok(unchanged());
    }
    let recommendation = if metadata.model.is_empty() {
        recommend(
            &snapshot.records,
            &metadata.task_class,
            Some(metadata.difficulty),
        )?
    } else {
        None
    };
    let accepted = history.iter().rev().find(|evaluation| {
        domain::is_text(evaluation.get("verdict"), "accept")
            && domain::is_text(evaluation.get("currentVerdict"), "accept")
            && !estimate::excellence_failed(evaluation)
            && optional_text(evaluation, "model").is_some_and(|model| {
                !model.is_empty() && !["unknown", "builtin-stub"].contains(&model.as_str())
            })
    });
    let configured_model = config
        .get("llm")
        .and_then(|llm| llm.get("model"))
        .is_some_and(domain::truthy);
    let environment_model = environment.get("ROSTER_MODEL").is_some_and(domain::truthy);
    let baseline = if metadata.model.is_empty()
        && !configured_model
        && !environment_model
        && recommendation.is_none()
    {
        accepted
    } else {
        None
    };
    let selected = recommendation.as_ref().or(baseline);
    if let Some(selected) = selected {
        let model = required(selected, "model")?
            .as_string()
            .ok_or_else(|| Error::type_error("Prior model must be text"))?;
        if memory::redact(model, redaction) != *model {
            return Err(Error::new(
                "Error",
                "Prior model metadata contains secret-like content; refusing to copy it",
            ));
        }
    }
    let updated = if let Some(selected) = selected {
        let mut values = Value::object(vec![("model", required(selected, "model")?.clone())]);
        if let Some(minutes) = recommendation
            .as_ref()
            .and_then(|recommendation| recommendation.get("estimate_min"))
            .filter(|minutes| **minutes != Value::Null)
        {
            domain::put(&mut values, "estimate_min", minutes.clone())?;
        }
        estimate::update_task_metadata(&Value::String(task.clone()), &values)?
    } else {
        task.clone()
    };
    let last = history
        .last()
        .ok_or_else(|| Error::type_error("Missing planning feedback history"))?;
    let negative = history.iter().rev().find(|evaluation| {
        ["reject", "rework"]
            .iter()
            .any(|verdict| domain::is_text(evaluation.get("verdict"), verdict))
            && evaluation
                .get("comment")
                .and_then(Value::as_string)
                .is_some_and(|comment| !crate::text::trim(&comment.0).is_empty())
    });
    let minutes = last
        .get("minutes")
        .filter(|value| **value != Value::Null)
        .map(domain::string)
        .transpose()?
        .map_or_else(|| "-".to_owned(), |value| encode(&value));
    let mut lines = vec![
        format!(
            "Task class: {}. Human feedback is data, not a policy grant.",
            metadata.task_class
        ),
        format!(
            "Last human verdict: {}; difficulty: {}; minutes: {minutes}.",
            encode(&domain::string(required(last, "verdict")?)?),
            encode(&domain::string(required(last, "difficulty")?)?)
        ),
    ];
    if let Some(negative) = negative {
        let comment = required(negative, "comment")?
            .as_string()
            .ok_or_else(|| Error::type_error("Feedback comment must be text"))?;
        lines.push("Last reject/rework comment:".to_owned());
        lines.extend(
            encode(&memory::redact(comment, redaction))
                .replace("\r\n", "\n")
                .split('\n')
                .map(|line| format!("> {line}")),
        );
    } else {
        lines.push("No reject/rework comment has been recorded for this task class.".to_owned());
    }
    if let Some(recommendation) = &recommendation {
        let rate = required(recommendation, "acceptRate")?
            .as_number()
            .ok_or_else(|| Error::type_error("Recommendation rate must be numeric"))?
            * 100.0;
        let percentage = percentage(rate)?;
        lines.push(format!(
            "Recommendation: {}; effort: {}; n: {}; accept-rate: {percentage}%.",
            optional_text(recommendation, "model").unwrap_or_default(),
            optional_text(recommendation, "effort").unwrap_or_else(|| "-".to_owned()),
            encode(&domain::string(required(recommendation, "n")?)?)
        ));
    } else if let Some(baseline) = baseline {
        lines.push(format!("Baseline model: {} from a prior acceptance; insufficient data for a capacity recommendation.", optional_text(baseline, "model").unwrap_or_default()));
    }
    Ok(Value::object(vec![
        ("task", Value::String(updated)),
        (
            "feedback",
            Value::object(vec![
                (
                    "context",
                    Value::String(memory::redact(&decode(&lines.join("\n")), redaction)),
                ),
                (
                    "recommendation",
                    recommendation.clone().unwrap_or(Value::Null),
                ),
                (
                    "effort",
                    selected
                        .and_then(|selected| selected.get("effort"))
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                (
                    "source",
                    Value::string(if recommendation.is_some() {
                        "recommendation"
                    } else if baseline.is_some() {
                        "prior-accept"
                    } else {
                        "task/config"
                    }),
                ),
            ]),
        ),
    ]))
}
