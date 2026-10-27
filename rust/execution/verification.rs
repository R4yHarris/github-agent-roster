use std::path::Path;

use super::*;
use crate::cancel::CancellationToken;
use crate::identity::sha256_hex;
use crate::planning;
use crate::state::secure;

pub(crate) fn tool_free_assistant(
    completion: &crate::llm::openai::Completion,
) -> Result<(), Error> {
    if domain::members(&completion.message).is_none()
        || !domain::is_text(completion.message.get("role"), "assistant")
        || completion
            .finish_reason
            .as_ref()
            .is_some_and(|reason| *reason != Value::Null && !domain::is_text(Some(reason), "stop"))
        || completion.message.get("tool_calls").is_some()
        || completion.message.get("function_call").is_some()
    {
        return Err(Error::new("ToolFreeResponseAuthority", "Tool-free review requires an assistant response with an ordinary stop and no function or tool requests").coded("ROSTER_TOOL_FREE_AUTHORITY"));
    }
    Ok(())
}

pub fn file_digest(worktree: &Path, name: &str, optional: bool) -> Result<Value, Error> {
    let maximum = if matches!(name, "TASK.md" | "RESULT.md" | "REVIEW.md") {
        65_536
    } else {
        MAX_FILE_BYTES
    };
    let file = worktree.join(name);
    crate::paths::ensure_local_path(&file, worktree)?;
    let snapshot = secure::read(&file, maximum)?;
    let Some(snapshot) = snapshot else {
        return if optional {
            Ok(Value::Null)
        } else {
            Err(
                Error::new("Error", format!("Independent verification requires {name}"))
                    .coded("ENOENT"),
            )
        };
    };
    if crate::paths::file_information(&file)?.links != 1 {
        return Err(Error::new(
            "Error",
            format!(
                "Independent verification requires {name} to be a single-link regular file of at most 64 KiB"
            ),
        ));
    }
    if !secure::unchanged(&file, &snapshot, maximum)? {
        return Err(Error::new(
            "Error",
            "Independent verification subject changed during capture",
        ));
    }
    Ok(Value::string(&sha256_hex(&snapshot.bytes)))
}

pub fn capture_subject(worktree: &Path, files: &[String]) -> Result<Value, Error> {
    capture_subject_result(worktree, files, None)
}

pub(crate) fn capture_subject_result(
    worktree: &Path,
    files: &[String],
    result: Option<&[u8]>,
) -> Result<Value, Error> {
    if result.is_some_and(|bytes| bytes.len() > 65_536) {
        return Err(denied(
            "Independent verification candidate RESULT exceeds 64 KiB",
        ));
    }
    if files.is_empty() || files.len() > 64 {
        return Err(Error::type_error(
            "Independent verification requires a worktree and 1-64 exact product file paths",
        ));
    }
    let mut names = Vec::new();
    for name in files {
        if name.is_empty()
            || name.encode_utf16().count() > 500
            || name.starts_with(['/', '\\'])
            || name.as_bytes().get(1) == Some(&b':')
            || name
                .chars()
                .any(|c| c <= '\u{1f}' || c == '\u{7f}' || c == '*' || c == '?')
        {
            return Err(Error::type_error(
                "Independent verification file paths must be exact relative paths",
            ));
        }
        let name = name.replace('\\', "/");
        if name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
            || ["TASK.md", "RESULT.md", "REVIEW.md"].contains(&name.as_str())
            || planning::forbidden_read(&name)
        {
            return Err(Error::new(
                "Error",
                "Independent verification file paths must name readable product files",
            ));
        }
        names.push(name);
    }
    names.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
    if names.windows(2).any(|pair| pair.first() == pair.get(1)) {
        return Err(Error::new(
            "Error",
            "Independent verification file paths must be unique",
        ));
    }
    let mut subject = Value::object(vec![
        ("task_digest", file_digest(worktree, "TASK.md", false)?),
        (
            "result_digest",
            if let Some(bytes) = result {
                Value::string(&sha256_hex(bytes))
            } else {
                file_digest(worktree, "RESULT.md", false)?
            },
        ),
        (
            "files",
            Value::Array(
                names
                    .iter()
                    .map(|name| {
                        Ok(Value::object(vec![
                            ("path", Value::string(name)),
                            ("digest", file_digest(worktree, name, true)?),
                        ]))
                    })
                    .collect::<Result<Vec<_>, Error>>()?,
            ),
        ),
    ]);
    let digest = sha256_hex(subject.json().as_bytes());
    domain::put(&mut subject, "digest", Value::string(&digest))?;
    Ok(subject)
}

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn short_output(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_string)
        .is_some_and(|value| value.0.len() <= 2000 && !value.0.contains(&0))
}

fn exit(value: Option<&Value>) -> bool {
    domain::safe_integer(value).is_some_and(|value| value <= 255)
}

pub fn judge_receipt(
    receipt: &Value,
    subject_digest: &str,
    check_count: u64,
) -> Result<Value, Error> {
    if !hash(subject_digest) || !(1..=16).contains(&check_count) {
        return Err(Error::type_error(
            "Independent verification requires a subject digest and 1-16 acceptance checks",
        ));
    }
    let observations = receipt.get("observations").and_then(domain::items);
    if fields(
        receipt,
        &["observations", "schema_version", "subject_digest"],
        &[],
        "",
    )
    .is_err()
        || receipt.get("schema_version") != Some(&Value::Number(1.0))
        || !domain::is_text(receipt.get("subject_digest"), subject_digest)
        || observations.is_none_or(|items| items.is_empty() || items.len() > 64)
    {
        return Err(Error::new(
            "Error",
            "Independent verification receipt is missing, malformed, or bound to different product bytes",
        ));
    }
    let mut ids = Vec::new();
    let mut checks = Vec::new();
    let mut results = Vec::new();
    let mut reasons = Vec::new();
    for entry in observations
        .ok_or_else(|| Error::type_error("Independent verification requires observations"))?
    {
        let id = domain::safe_integer(entry.get("id"));
        let check = domain::safe_integer(entry.get("check_id"));
        let evidence = entry.get("evidence").and_then(Value::as_string);
        if fields(
            entry,
            &[
                "id",
                "check_id",
                "evidence",
                "actual_exit",
                "actual_output",
                "expected_exit",
                "expected_output",
            ],
            &[],
            "",
        )
        .is_err()
            || id.is_none_or(|id| !(1..=64).contains(&id))
            || ids.contains(&id)
            || check.is_none_or(|check| !(1..=check_count).contains(&check))
            || !short_output(entry.get("expected_output"))
            || !short_output(entry.get("actual_output"))
            || !exit(entry.get("expected_exit"))
            || !exit(entry.get("actual_exit"))
            || evidence.is_none_or(|evidence| {
                evidence.0.is_empty()
                    || evidence.0.len() > 500
                    || crate::text::trim(&evidence.0) != evidence.0
                    || evidence.0.iter().any(|unit| matches!(unit, 0..=31 | 127))
            })
        {
            return Err(Error::new(
                "Error",
                "Independent verification observations require unique IDs, valid checks, bounded outputs, exits and evidence",
            ));
        }
        ids.push(id);
        if !checks.contains(&check) {
            checks.push(check);
        }
        let met = entry.get("expected_output") == entry.get("actual_output")
            && entry.get("expected_exit") == entry.get("actual_exit");
        let mut entry = entry.clone();
        domain::put(&mut entry, "met", Value::Boolean(met))?;
        if !met {
            reasons.push(Value::string(&format!(
                "Independent verification observation {}, check {} failed: {}",
                id.unwrap_or(0),
                check.unwrap_or(0),
                evidence
                    .ok_or_else(|| Error::type_error("Missing evidence"))?
                    .to_utf8()?
            )));
        }
        results.push(entry);
    }
    if checks.len() as u64 != check_count {
        return Err(Error::new(
            "Error",
            "Independent verification receipt must cover every acceptance check",
        ));
    }
    Ok(Value::object(vec![
        (
            "status",
            Value::string(if reasons.is_empty() { "pass" } else { "fail" }),
        ),
        ("subjectDigest", Value::string(subject_digest)),
        ("observations", Value::Array(results)),
        ("reasons", Value::Array(reasons)),
    ]))
}

pub fn judge_current_receipt(
    receipt: &Value,
    subject: &Value,
    worktree: &Path,
    files: &[String],
    check_count: u64,
    token: &CancellationToken,
) -> Result<Value, Error> {
    token.check()?;
    let digest = text(required(subject, "digest")?, "Subject requires a digest")?;
    let result = judge_receipt(receipt, &digest, check_count)?;
    if capture_subject(worktree, files)? != *subject {
        return Err(Error::new(
            "Error",
            "Independent verification product changed after capture; receipt is stale",
        ));
    }
    token.check()?;
    Ok(result)
}

pub fn parse_review_requests(content: &Text) -> Result<Option<Value>, Error> {
    let value = match crate::json::parse(content) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if fields(&value, &["requests"], &[], "").is_err() {
        return Ok(None);
    }
    let requests = value
        .get("requests")
        .and_then(domain::items)
        .filter(|requests| !requests.is_empty() && requests.len() <= 4)
        .ok_or_else(|| Error::new("Error", "Reviewer requests must list 1-4 reads"))?;
    let mut result = Vec::new();
    for request in requests {
        let members = domain::members(request)
            .filter(|_| request.get("tool").and_then(Value::as_string).is_some())
            .ok_or_else(|| {
                Error::new(
                    "Error",
                    "Each reviewer request must be {tool, path} or {tool, query, path?}",
                )
            })?;
        let mut args = Value::object(Vec::new());
        for (key, value) in members {
            let key = key.to_utf8()?;
            if key == "tool" {
                continue;
            }
            if value.as_string().is_none_or(|value| value.0.len() > 500) {
                return Err(Error::new(
                    "Error",
                    "Reviewer request arguments must be short strings",
                ));
            }
            domain::put(&mut args, &key, value.clone())?;
        }
        result.push(Value::object(vec![
            ("tool", required(request, "tool")?.clone()),
            ("args", args),
        ]));
    }
    Ok(Some(Value::Array(result)))
}

pub fn parse_self_review(content: &Text, check_count: u64) -> Result<Value, Error> {
    let report = crate::json::parse(content)
        .map_err(|_| Error::new("Error", "Self-review did not return valid JSON"))?;
    let line = |value: &Value| {
        value.as_string().is_some_and(|value| {
            !crate::text::trim(&value.0).is_empty()
                && value.0.len() <= 300
                && !value.0.iter().any(|unit| matches!(unit, 0..=31 | 127))
        })
    };
    let findings = report.get("findings").and_then(domain::items);
    let checks = report.get("checks").and_then(domain::items);
    let mut ids = Vec::new();
    let valid = checks.is_some_and(|checks| {
        checks.len() as u64 == check_count
            && checks.iter().all(|check| {
                let id = domain::safe_integer(check.get("id"));
                let valid = fields(check, &["id", "met", "evidence"], &[], "").is_ok()
                    && id.is_some_and(|id| (1..=check_count).contains(&id))
                    && !ids.contains(&id)
                    && matches!(check.get("met"), Some(Value::Boolean(_)))
                    && check.get("evidence").is_some_and(line);
                ids.push(id);
                valid
            })
    });
    if fields(&report, &["checks", "findings"], &[], "").is_err()
        || findings.is_none_or(|findings| findings.len() > 8 || !findings.iter().all(line))
        || !valid
    {
        return Err(Error::new(
            "Error",
            format!(
                "Self-review must return checks (one per acceptance check 1-{check_count}) and findings"
            ),
        ));
    }
    let mut checks = checks
        .ok_or_else(|| Error::type_error("Self-review requires checks"))?
        .to_vec();
    checks.sort_by_key(|entry| domain::safe_integer(entry.get("id")));
    let findings = findings
        .ok_or_else(|| Error::type_error("Self-review requires findings"))?
        .iter()
        .map(|finding| {
            finding
                .as_string()
                .map(|finding| Value::String(Text(crate::text::trim(&finding.0).to_vec())))
                .ok_or_else(|| Error::type_error("Self-review requires finding text"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::object(vec![
        ("checks", Value::Array(checks)),
        ("findings", Value::Array(findings)),
    ]))
}

pub fn require_docker_executor(enabled: bool, image: &str) -> Result<(), Error> {
    if !enabled {
        return Err(Error::new(
            "Error",
            "Docker verification requires explicit host opt-in",
        ));
    }
    if !image.strip_prefix("sha256:").is_some_and(hash) {
        return Err(Error::type_error(
            "Docker verification requires an immutable locally acquired sha256 image ID",
        ));
    }
    Err(Error::new(
        "NativeIntegrationError",
        "A flag and image ID cannot grant Docker execution; use the explicit host-approved native Docker backend",
    ).coded("ROSTER_NATIVE_UNSUPPORTED"))
}
