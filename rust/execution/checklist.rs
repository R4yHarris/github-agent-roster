use std::path::Path;

use super::*;
use crate::identity::sha256_hex;
use crate::planning::task;
use crate::state::redaction;

const STATUSES: &[&str] = &["pending", "in_progress", "done", "blocked"];
const TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "delete_file",
    "run_test",
];

fn fail(reason: impl Into<String>) -> Error {
    Error::type_error(format!("Acceptance continuation: {}", reason.into()))
}

fn entries(checklist: &Value) -> Result<&[Value], Error> {
    checklist
        .get("items")
        .and_then(domain::items)
        .ok_or_else(|| Error::type_error("Checklist requires items"))
}

fn terminal(status: &str) -> bool {
    matches!(status, "done" | "blocked")
}

fn status(item: &Value) -> Result<String, Error> {
    text(required(item, "status")?, "Checklist requires a status")
}

fn id(item: &Value) -> Option<u64> {
    domain::safe_integer(item.get("id"))
}

fn evidence(input: Option<&Value>) -> Result<Text, Error> {
    Ok(one_line(
        &input
            .filter(|value| **value != Value::Null)
            .map(domain::string)
            .transpose()?
            .unwrap_or_default(),
        300,
    ))
}

pub fn create(checks: &[Text], exact: bool) -> Value {
    Value::object(vec![(
        "items",
        Value::Array(
            checks
                .iter()
                .enumerate()
                .map(|(index, check)| {
                    Value::object(vec![
                        ("id", Value::Number((index + 1) as f64)),
                        (
                            "check",
                            Value::String(if exact {
                                check.clone()
                            } else {
                                one_line(check, 300)
                            }),
                        ),
                        ("status", Value::string("pending")),
                        ("evidence", Value::string("")),
                    ])
                })
                .collect(),
        ),
    )])
}

pub fn authoritative_checks(source: &Text) -> Result<Vec<Text>, Error> {
    let document = task::parse(&Value::String(source.clone()), &task::Authority::default())?;
    let parts = task::sections(&Value::String(source.clone()))?;
    let section = parts
        .section("acceptance checks")
        .ok_or_else(|| fail("authoritative check count mismatch"))?;
    let bullet = regex(r"^(?:[-*+]|\d+[.)])[ \t]+")?;
    let mut checks: Vec<String> = Vec::new();
    for line in section
        .content
        .split('\n')
        .filter(|line| !js_trim(line).is_empty())
    {
        let value = bullet.replace(js_trim(line), "").into_owned();
        if line.chars().next().is_some_and(js_space) && !checks.is_empty() {
            if let Some(previous) = checks.last_mut() {
                previous.push(' ');
                previous.push_str(&value);
            }
        } else {
            checks.push(
                value
                    .strip_prefix('`')
                    .and_then(|value| value.strip_suffix('`'))
                    .filter(|value| !value.is_empty() && !value.contains('`'))
                    .unwrap_or(&value)
                    .to_owned(),
            );
        }
    }
    if checks.len() != document.acceptance_checks.len() {
        return Err(fail("authoritative check count mismatch"));
    }
    Ok(checks
        .iter()
        .map(|check| Text::from(check.as_str()))
        .collect())
}

pub fn render(checklist: &Value) -> Result<Text, Error> {
    let mut result = Text::default();
    for (index, item) in entries(checklist)?.iter().enumerate() {
        if index != 0 {
            result.0.push(10);
        }
        result.0.extend(
            Text::from(
                format!(
                    "{}. [{}] ",
                    id(item).ok_or_else(|| Error::type_error("Checklist requires an ID"))?,
                    status(item)?
                )
                .as_str(),
            )
            .0,
        );
        result.0.extend(domain::string(required(item, "check")?)?.0);
        let evidence = domain::string(required(item, "evidence")?)?;
        if !evidence.0.is_empty() {
            result.0.extend(Text::from(" (evidence: ").0);
            result.0.extend(evidence.0);
            result.0.push(41);
        }
    }
    Ok(result)
}

pub fn progress(checklist: &Value) -> Result<Value, Error> {
    let items = entries(checklist)?;
    Ok(Value::object(vec![
        (
            "done",
            Value::Number(
                items
                    .iter()
                    .filter(|item| domain::is_text(item.get("status"), "done"))
                    .count() as f64,
            ),
        ),
        ("total", Value::Number(items.len() as f64)),
    ]))
}

pub fn open(checklist: &Value) -> Result<Vec<Value>, Error> {
    Ok(entries(checklist)?
        .iter()
        .filter(|item| {
            domain::is_text(item.get("status"), "pending")
                || domain::is_text(item.get("status"), "in_progress")
        })
        .cloned()
        .collect())
}

pub fn update(checklist: &Value, args: &Value) -> Result<Value, Error> {
    let items = entries(checklist)?;
    let updates = args
        .get("items")
        .and_then(domain::items)
        .filter(|updates| !updates.is_empty() && updates.len() <= items.len())
        .ok_or_else(|| {
            Error::type_error(
                "update_checklist needs items: [{id, status, evidence}] for existing check numbers",
            )
        })?;
    let mut next = items.to_vec();
    for update in updates {
        let item = next
            .iter_mut()
            .find(|item| id(item).is_some() && id(item) == id(update))
            .ok_or_else(|| {
                Error::type_error(format!(
                    "update_checklist: no check {}; use 1-{}",
                    update
                        .get("id")
                        .map_or_else(|| "undefined".to_owned(), Value::json),
                    items.len()
                ))
            })?;
        let status = update
            .get("status")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_default();
        if !STATUSES.contains(&status.as_str()) {
            return Err(Error::type_error(format!(
                "update_checklist: status must be one of {}",
                STATUSES.join(", ")
            )));
        }
        let evidence = evidence(
            update
                .get("evidence")
                .filter(|value| **value != Value::Null),
        )?;
        if terminal(&status) && evidence.0.is_empty() {
            return Err(Error::type_error(format!(
                "update_checklist: check {} marked {status} needs evidence",
                id(item).unwrap_or(0)
            )));
        }
        domain::put(item, "status", Value::string(&status))?;
        if !evidence.0.is_empty() {
            domain::put(item, "evidence", Value::String(evidence))?;
        }
    }
    if next
        .iter()
        .filter(|item| domain::is_text(item.get("status"), "in_progress"))
        .count()
        > 1
    {
        return Err(Error::type_error(
            "update_checklist: work one item at a time; at most one item may be in_progress",
        ));
    }
    Ok(Value::object(vec![("items", Value::Array(next))]))
}

pub fn observation(input: &Value, config: &redaction::Config) -> Result<Value, Error> {
    let tool = text(required(input, "tool")?, "Observation requires a tool")?;
    if !TOOLS.contains(&tool.as_str()) {
        return Ok(Value::Null);
    }
    let result = input.get("result").unwrap_or(&Value::Null);
    let exit = result
        .get("exit_code")
        .and_then(|value| domain::safe_integer(Some(value)));
    if tool == "run_test" && result.get("native") == Some(&Value::Boolean(true)) {
        return Err(unsupported(
            "Version-1 acceptance continuation cannot label Cargo evidence as node --test",
        ));
    }
    if tool == "run_test" && result.get("skipped") == Some(&Value::Boolean(true)) {
        return Err(fail("skipped tests are not observed passing evidence"));
    }
    if tool == "run_test" && exit.is_none() {
        return Err(fail("run_test observation needs an observed exit code"));
    }
    let reference = if tool == "run_test" {
        Text::from("node --test")
    } else {
        let path = result
            .get("path")
            .filter(|value| **value != Value::Null)
            .or_else(|| input.get("args").and_then(|args| args.get("path")))
            .map(domain::string)
            .transpose()?
            .unwrap_or_default();
        redaction::evidence(
            &Text::from(path.to_utf8()?.replace('\\', "/").as_str()),
            config,
        )
    };
    if reference.0.is_empty() || reference.0.len() > 300 {
        return Err(fail("tool reference exceeds bounds"));
    }
    Ok(Value::object(vec![
        ("id", required(input, "id")?.clone()),
        ("tool", Value::string(&tool)),
        ("reference", Value::String(reference)),
        ("source", required(input, "source")?.clone()),
        (
            "verdict",
            Value::string(if tool == "run_test" {
                if exit == Some(0) { "pass" } else { "fail" }
            } else {
                "observed"
            }),
        ),
    ]))
}

pub fn reference_evidence(observation: &Value) -> Result<Text, Error> {
    Ok(Text::from(
        format!(
            "Observation {}: {} {} ({})",
            id(observation).ok_or_else(|| fail("invalid observation reference"))?,
            text(
                required(observation, "tool")?,
                "Observation requires a tool"
            )?,
            required(observation, "reference")?.json(),
            text(
                required(observation, "verdict")?,
                "Observation requires a verdict"
            )?,
        )
        .as_str(),
    ))
}

pub fn update_references(
    args: &Value,
    observations: &[Value],
    source: &str,
) -> Result<Value, Error> {
    let updates = args.get("items").and_then(domain::items).unwrap_or(&[]);
    let mut references = Vec::new();
    for update in updates {
        let state = update
            .get("status")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_default();
        if !terminal(&state) {
            references.push(Value::Null);
            continue;
        }
        let evidence = evidence(update.get("evidence"))?.to_utf8()?;
        let observed = observations
            .iter()
            .rev()
            .find(|entry| {
                domain::is_text(entry.get("source"), source)
                    && (entry
                        .get("reference")
                        .and_then(Value::as_string)
                        .and_then(|value| value.to_utf8().ok())
                        .is_some_and(|reference| evidence.contains(&reference))
                        || domain::is_text(entry.get("tool"), "run_test")
                            && evidence.contains("run_test"))
                    && if state == "done" {
                        !domain::is_text(entry.get("verdict"), "fail")
                    } else {
                        domain::is_text(entry.get("verdict"), "fail")
                    }
            })
            .ok_or_else(|| {
                fail(format!(
                    "check {} needs current observed {}",
                    id(update).unwrap_or(0),
                    if state == "done" {
                        "evidence"
                    } else {
                        "failure evidence"
                    }
                ))
            })?;
        references.push(required(observed, "id")?.clone());
    }
    Ok(Value::Array(references))
}

fn identity(task: &Text, worktree: &Path) -> Result<(String, String), Error> {
    let path = crate::paths::absolute(worktree)?;
    Ok((
        sha256_hex(task.to_utf8()?.as_bytes()),
        sha256_hex(
            path.to_str()
                .ok_or_else(|| Error::type_error("Worktree must be Unicode"))?
                .as_bytes(),
        ),
    ))
}

fn hash(input: Option<&Value>) -> bool {
    input.and_then(Value::as_string).is_some_and(|text| {
        text.0.len() == 64 && text.0.iter().all(|unit| matches!(unit, 48..=57 | 97..=102))
    })
}

fn keys(input: &Value, names: &[&str]) -> bool {
    fields(input, names, &[], "").is_ok()
}

pub fn export_continuation(input: &Value, task: &Text, worktree: &Path) -> Result<Value, Error> {
    let Some(checklist) = input
        .get("checklist")
        .filter(|value| **value != Value::Null)
    else {
        return Ok(Value::Null);
    };
    let source = text(required(input, "source")?, "Continuation requires source")?;
    let outcome = text(required(input, "outcome")?, "Continuation requires outcome")?;
    if !["verified", "failed", "cancelled"].contains(&outcome.as_str()) {
        return Err(fail("malformed capsule"));
    }
    let observed = required(input, "observations")?;
    let observed = domain::items(observed).ok_or_else(|| fail("invalid observation reference"))?;
    let mut retained = observed
        .iter()
        .filter(|entry| domain::is_text(entry.get("source"), &source))
        .rev()
        .take(32)
        .cloned()
        .collect::<Vec<_>>();
    retained.reverse();
    let mut notices = input
        .get("priorNotices")
        .and_then(domain::items)
        .unwrap_or(&[])
        .to_vec();
    let mut items = Vec::new();
    for item in entries(checklist)? {
        let mut state = status(item)?;
        let mut reference = item.get("evidenceRef").cloned().unwrap_or(Value::Null);
        let observed = retained
            .iter()
            .find(|entry| entry.get("id") == Some(&reference));
        if terminal(&state)
            && (observed.is_none()
                || state == "done"
                    && (outcome != "verified"
                        || observed
                            .is_some_and(|entry| domain::is_text(entry.get("verdict"), "fail")))
                || state == "blocked"
                    && !observed.is_some_and(|entry| domain::is_text(entry.get("verdict"), "fail")))
        {
            let notice = Value::string(&format!(
                "Check {}: {state} invalidated; evidence is missing, changed, or the attempt was not verified.",
                id(item).unwrap_or(0)
            ));
            if !notices.contains(&notice) {
                notices.push(notice);
            }
            state = "pending".to_owned();
        }
        let evidence = if terminal(&state) {
            reference_evidence(
                observed
                    .ok_or_else(|| fail("terminal obligation lacks valid observed evidence"))?,
            )?
        } else {
            reference = Value::Null;
            Text::from("")
        };
        items.push(Value::object(vec![
            ("id", required(item, "id")?.clone()),
            ("check", required(item, "check")?.clone()),
            ("status", Value::string(&state)),
            ("evidence", Value::String(evidence)),
            ("evidenceRef", reference),
        ]));
    }
    if notices.len() > items.len() {
        notices.drain(..notices.len() - items.len());
    }
    let (task, worktree) = identity(task, worktree)?;
    let capsule = Value::object(vec![
        ("version", Value::Number(1.0)),
        ("task", Value::string(&task)),
        ("worktree", Value::string(&worktree)),
        ("source", Value::string(&source)),
        ("outcome", Value::string(&outcome)),
        ("items", Value::Array(items)),
        ("observations", Value::Array(retained)),
        ("notices", Value::Array(notices)),
    ]);
    if capsule.json().encode_utf16().count() > 65_536 {
        return Err(fail("capsule exceeds 65536 characters"));
    }
    Ok(capsule)
}

pub fn import_continuation(
    capsule: &Value,
    task: &Text,
    worktree: &Path,
    checks: &[Text],
    source: &str,
    config: &redaction::Config,
) -> Result<Value, Error> {
    if !keys(
        capsule,
        &[
            "version",
            "task",
            "worktree",
            "source",
            "outcome",
            "items",
            "observations",
            "notices",
        ],
    ) || capsule.json().encode_utf16().count() > 65_536
        || capsule.get("version") != Some(&Value::Number(1.0))
        || !["verified", "failed", "cancelled"]
            .iter()
            .any(|outcome| domain::is_text(capsule.get("outcome"), outcome))
    {
        return Err(fail("malformed capsule"));
    }

    import_validated(capsule, task, worktree, checks, source, config)
}

pub fn close_from_summary(checklist: &Value, summary: &Text) -> Result<Value, Error> {
    let mut next = entries(checklist)?.to_vec();
    let pattern = regex(
        r"(?i)^(?:[-*]\s*)?(?:check\s*)?#?(\d+)\s*[.):-]?\s*\[?(done|met|blocked)\]?\s*[-:—]?\s*(.*)$",
    )?;
    for line in summary.0.split(|unit| *unit == 10) {
        let normalized = one_line(&Text(line.to_vec()), usize::MAX).to_utf8()?;
        let Some(captures) = pattern.captures(&normalized) else {
            continue;
        };
        let current = captures
            .get(1)
            .and_then(|value| value.as_str().parse::<u64>().ok());
        let Some(item) = next.iter_mut().find(|item| id(item) == current) else {
            continue;
        };
        if !["pending", "in_progress"]
            .iter()
            .any(|state| domain::is_text(item.get("status"), state))
        {
            continue;
        }
        let blocked = captures
            .get(2)
            .is_some_and(|state| state.as_str().eq_ignore_ascii_case("blocked"));
        let proof = captures.get(3).map_or("", |value| value.as_str());
        domain::put(
            item,
            "status",
            Value::string(if blocked { "blocked" } else { "done" }),
        )?;
        domain::put(
            item,
            "evidence",
            Value::String(if proof.is_empty() {
                Text::from("stated in the final summary")
            } else {
                one_line(&Text::from(proof), 300)
            }),
        )?;
    }
    Ok(Value::object(vec![("items", Value::Array(next))]))
}

pub fn correction(checklist: &Value) -> Result<Text, Error> {
    let mut output = Text::from("Your checklist still has open items:\n");
    output.0.extend(
        render(&Value::object(vec![(
            "items",
            Value::Array(open(checklist)?),
        )]))?
        .0,
    );
    output.push_str("\nFinish each one, then call update_checklist to mark it done with evidence (file and symbol or test), or blocked with the reason. Then summarize.");
    Ok(output)
}

fn table_cell(value: Option<&Value>) -> Result<Text, Error> {
    let value = value
        .filter(|value| domain::truthy(value))
        .map(domain::string)
        .transpose()?
        .unwrap_or_else(|| Text::from("-"));
    let mut output = Text::default();
    for unit in value.0 {
        if unit == 124 {
            output.0.push(92);
        }
        output.0.push(unit);
    }
    Ok(output)
}

pub fn table(checklist: &Value) -> Result<Text, Error> {
    let mut output = Text::from("| # | Check | Status | Evidence |\n|---|---|---|---|\n");
    for (index, item) in entries(checklist)?.iter().enumerate() {
        if index != 0 {
            output.0.push(10);
        }
        output.push_str(&format!(
            "| {} | ",
            id(item).ok_or_else(|| Error::type_error("Checklist item requires an ID"))?
        ));
        output.0.extend(table_cell(item.get("check"))?.0);
        output.push_str(" | ");
        output.0.extend(
            required(item, "status")?
                .as_string()
                .ok_or_else(|| Error::type_error("Checklist status must be text"))?
                .0
                .iter(),
        );
        output.push_str(" | ");
        output.0.extend(table_cell(item.get("evidence"))?.0);
        output.push_str(" |");
    }
    output.0.push(10);
    Ok(output)
}

pub fn render_acceptance(checklist: &Value, observations: &[Value]) -> Result<Text, Error> {
    let mut output = Text::default();
    for (index, item) in entries(checklist)?.iter().enumerate() {
        if index != 0 {
            output.0.push(10);
        }
        output.push_str(&format!(
            "{}. [{}] ",
            id(item).ok_or_else(|| fail("malformed obligation"))?,
            status(item)?
        ));
        output.0.extend(
            required(item, "check")?
                .as_string()
                .ok_or_else(|| fail("malformed obligation"))?
                .0
                .iter(),
        );
        if let Some(observed) = observations
            .iter()
            .find(|entry| entry.get("id") == item.get("evidenceRef"))
        {
            output.push_str(" (evidence: ");
            output.0.extend(reference_evidence(observed)?.0);
            output.0.push(41);
        }
    }
    Ok(output)
}

pub fn definition() -> Value {
    let statuses = Value::Array(
        STATUSES
            .iter()
            .map(|status| Value::string(status))
            .collect(),
    );
    Value::object(vec![
        ("type", Value::string("function")),
        (
            "function",
            Value::object(vec![
                ("name", Value::string("update_checklist")),
                (
                    "description",
                    Value::string(
                        "Update authoritative TASK checks with current observed evidence. At most one item may be in_progress.",
                    ),
                ),
                (
                    "parameters",
                    Value::object(vec![
                        ("type", Value::string("object")),
                        (
                            "properties",
                            Value::object(vec![(
                                "items",
                                Value::object(vec![
                                    ("type", Value::string("array")),
                                    (
                                        "items",
                                        Value::object(vec![
                                            ("type", Value::string("object")),
                                            (
                                                "properties",
                                                Value::object(vec![
                                                    (
                                                        "id",
                                                        Value::object(vec![(
                                                            "type",
                                                            Value::string("integer"),
                                                        )]),
                                                    ),
                                                    (
                                                        "status",
                                                        Value::object(vec![
                                                            ("type", Value::string("string")),
                                                            ("enum", statuses),
                                                        ]),
                                                    ),
                                                    (
                                                        "evidence",
                                                        Value::object(vec![(
                                                            "type",
                                                            Value::string("string"),
                                                        )]),
                                                    ),
                                                ]),
                                            ),
                                            (
                                                "required",
                                                Value::Array(vec![
                                                    Value::string("id"),
                                                    Value::string("status"),
                                                ]),
                                            ),
                                            ("additionalProperties", Value::Boolean(false)),
                                        ]),
                                    ),
                                ]),
                            )]),
                        ),
                        ("required", Value::Array(vec![Value::string("items")])),
                        ("additionalProperties", Value::Boolean(false)),
                    ]),
                ),
            ]),
        ),
    ])
}
fn import_validated(
    capsule: &Value,
    task: &Text,
    worktree: &Path,
    checks: &[Text],
    source: &str,
    config: &redaction::Config,
) -> Result<Value, Error> {
    let (task_id, worktree_id) = identity(task, worktree)?;
    if !domain::is_text(capsule.get("task"), &task_id)
        || !domain::is_text(capsule.get("worktree"), &worktree_id)
    {
        return Err(fail("TASK or worktree mismatch"));
    }
    if !hash(capsule.get("source")) || !domain::is_text(capsule.get("source"), source) {
        return Err(fail("stale source bytes; rerun checks"));
    }
    let items = capsule.get("items").and_then(domain::items);
    let observations = capsule.get("observations").and_then(domain::items);
    let notices = capsule.get("notices").and_then(domain::items);
    if items.is_none_or(|items| items.len() > checks.len())
        || observations.is_none_or(|items| items.len() > 32)
        || notices.is_none_or(|items| {
            items.len() > checks.len()
                || items
                    .iter()
                    .any(|notice| notice.as_string().is_none_or(|notice| notice.0.len() > 300))
        })
    {
        return Err(fail("invalid bounded references"));
    }
    let observations = observations.ok_or_else(|| fail("invalid bounded references"))?;
    let mut ids = Vec::new();
    for entry in observations {
        let tool = entry
            .get("tool")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_default();
        let reference = entry.get("reference").and_then(Value::as_string);
        let valid = if tool == "run_test" {
            domain::is_text(entry.get("reference"), "node --test")
                && ["pass", "fail"]
                    .iter()
                    .any(|verdict| domain::is_text(entry.get("verdict"), verdict))
        } else {
            domain::is_text(entry.get("verdict"), "observed")
        };
        let current_id = id(entry).filter(|id| *id > 0);
        if !keys(entry, &["id", "tool", "reference", "source", "verdict"])
            || current_id.is_none()
            || ids.contains(&current_id)
            || !TOOLS.contains(&tool.as_str())
            || !domain::is_text(entry.get("source"), source)
            || reference.is_none_or(|reference| reference.0.is_empty() || reference.0.len() > 300)
            || !valid
        {
            return Err(fail("invalid observation reference"));
        }
        ids.push(current_id);
    }
    let mut checklist = create(checks, true);
    let mut next = entries(&checklist)?.to_vec();
    let mut seen = Vec::new();
    for entry in items.ok_or_else(|| fail("invalid bounded references"))? {
        let item = next.iter_mut().find(|item| id(item) == id(entry));
        if !keys(entry, &["id", "check", "status", "evidence", "evidenceRef"])
            || item.is_none()
            || seen.contains(&id(entry))
            || !STATUSES
                .iter()
                .any(|state| domain::is_text(entry.get("status"), state))
            || entry
                .get("evidence")
                .and_then(Value::as_string)
                .is_none_or(|evidence| evidence.0.len() > 400)
        {
            return Err(fail("malformed or mismatched obligation"));
        }
        let item = item.ok_or_else(|| fail("malformed or mismatched obligation"))?;
        if item.get("check") != entry.get("check") {
            return Err(fail("malformed or mismatched obligation"));
        }
        let state = status(entry)?;
        if terminal(&state) {
            let observed = observations
                .iter()
                .find(|observation| observation.get("id") == entry.get("evidenceRef"))
                .ok_or_else(|| fail("terminal obligation lacks valid observed evidence"))?;
            if state == "done"
                && (!domain::is_text(capsule.get("outcome"), "verified")
                    || domain::is_text(observed.get("verdict"), "fail"))
                || state == "blocked" && !domain::is_text(observed.get("verdict"), "fail")
                || entry.get("evidence") != Some(&Value::String(reference_evidence(observed)?))
            {
                return Err(fail("terminal obligation lacks valid observed evidence"));
            }
        } else if entry.get("evidenceRef") != Some(&Value::Null)
            || !domain::is_text(entry.get("evidence"), "")
        {
            return Err(fail("open obligation has terminal evidence"));
        }
        seen.push(id(entry));
        *item = entry.clone();
        if terminal(&state) {
            let observed = observations
                .iter()
                .find(|observation| observation.get("id") == entry.get("evidenceRef"))
                .ok_or_else(|| fail("terminal obligation lacks valid observed evidence"))?;
            let mut observed = observed.clone();
            let reference = required(&observed, "reference")?
                .as_string()
                .ok_or_else(|| fail("invalid observation reference"))?;
            let reference = redaction::evidence(reference, config);
            domain::put(&mut observed, "reference", Value::String(reference))?;
            domain::put(
                item,
                "evidence",
                Value::String(reference_evidence(&observed)?),
            )?;
        }
    }
    if next
        .iter()
        .filter(|item| domain::is_text(item.get("status"), "in_progress"))
        .count()
        > 1
    {
        return Err(fail("multiple in_progress obligations"));
    }
    domain::put(&mut checklist, "items", Value::Array(next))?;
    let observations = observations
        .iter()
        .map(|entry| {
            let mut entry = entry.clone();
            let reference = required(&entry, "reference")?
                .as_string()
                .ok_or_else(|| fail("invalid observation reference"))?;
            let reference = redaction::evidence(reference, config);
            domain::put(&mut entry, "reference", Value::String(reference))?;
            Ok(entry)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let notices = notices
        .ok_or_else(|| fail("invalid bounded references"))?
        .iter()
        .map(|notice| {
            notice
                .as_string()
                .map(|notice| Value::String(redaction::evidence(notice, config)))
                .ok_or_else(|| fail("invalid bounded references"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::object(vec![
        ("checklist", checklist),
        ("observations", Value::Array(observations)),
        ("notices", Value::Array(notices)),
    ]))
}
