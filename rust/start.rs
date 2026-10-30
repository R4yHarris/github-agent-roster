use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Error;
use crate::cancel::CancellationToken;
use crate::config::Config;
use crate::execution::{backend, checkpoints::IndexSeal, owned, policy, tools};
use crate::host::Image;
use crate::json::Value;
use crate::paths::{self, FileIdentity, RepositoryPaths};
use crate::process::{FailureKind, Limits};
use crate::state::{files, locks, redaction, secure};
use crate::text::Text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sync {
    Fetch,
    Offline,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub base: String,
    pub sync: Sync,
}

impl Options {
    pub fn resolve(config: &Config, base: Option<&str>, sync: Option<Sync>) -> Result<Self, Error> {
        let start = config.as_value().get("start");
        let base = base
            .map(str::to_owned)
            .or_else(|| {
                start
                    .and_then(|value| value.get("base"))
                    .and_then(Value::as_string)
                    .and_then(|value| value.to_utf8().ok())
            })
            .unwrap_or_else(|| "trunk".to_owned());
        crate::config::validate_start_base(&Text::from(base.as_str()), "--base")?;
        let sync = sync.unwrap_or_else(|| {
            if start
                .and_then(|value| value.get("sync"))
                .and_then(Value::as_string)
                .is_some_and(|value| value == &Text::from("offline"))
            {
                Sync::Offline
            } else {
                Sync::Fetch
            }
        });
        Ok(Self { base, sync })
    }
}

#[derive(Debug, Clone, Default)]
pub enum FetchPolicy {
    #[default]
    Deny,
    Http,
    Local {
        source: PathBuf,
        head: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssueBranch {
    pub number: u64,
    pub attempt: Option<u8>,
}

pub fn parse_issue_number(value: &str) -> Result<u64, Error> {
    let number = value.parse::<u64>().ok().filter(|number| {
        !value.starts_with('0')
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && (1..=crate::MAX_SAFE_INTEGER).contains(number)
    });
    number.ok_or_else(|| Error::type_error("Issue number must be a positive safe decimal integer"))
}

pub fn issue_task(number: u64) -> Result<String, Error> {
    if number == 0 || number > crate::MAX_SAFE_INTEGER {
        return Err(Error::type_error(
            "Issue number must be a positive safe integer",
        ));
    }
    Ok(format!("issue-{number}"))
}

pub fn issue_branch(number: u64, attempt: Option<u8>) -> Result<String, Error> {
    issue_task(number)?;
    if attempt.is_some_and(|attempt| !(1..=16).contains(&attempt)) {
        return Err(Error::type_error("Attempt index must be between 1 and 16"));
    }
    Ok(format!(
        "agents/{number}{}",
        attempt.map_or_else(String::new, |attempt| format!("-a{attempt}"))
    ))
}

pub fn issue_branches(number: u64, attempt: Option<u8>) -> Result<[String; 2], Error> {
    let canonical = issue_branch(number, attempt)?;
    let task = issue_task(number)?;
    let suffix = attempt.map_or_else(String::new, |attempt| format!("-a{attempt}"));
    Ok([canonical, format!("{task}{suffix}")])
}

pub fn select_issue_branch(
    number: u64,
    branches: &[String],
    attempt: Option<u8>,
) -> Result<String, Error> {
    let candidates = issue_branches(number, attempt)?;
    let found = candidates
        .iter()
        .filter(|candidate| branches.iter().any(|branch| branch == *candidate))
        .collect::<Vec<_>>();
    if found.len() > 1 {
        return Err(Error::new(
            "Error",
            format!(
                "Ambiguous issue branch namespaces for #{number}; preserve both branches and resolve ownership explicitly"
            ),
        ));
    }
    Ok(found
        .first()
        .map_or_else(|| candidates[0].clone(), |branch| (*branch).clone()))
}

pub const DEFAULT_STALE_THRESHOLD_MS: u64 = 30 * 60 * 1000;

fn stranded_timestamp(value: Option<&Value>, label: &str) -> Result<f64, Error> {
    let fail = || Error::type_error(format!("{label} must be a valid timestamp"));
    let Some(value) = value else {
        return Err(fail());
    };
    match value {
        Value::Number(number) if number.is_finite() => Ok(*number),
        Value::String(text) => {
            let trimmed = crate::text::trim(&text.0);
            if trimmed.is_empty() {
                return Err(fail());
            }
            let normalized_raw = Text(trimmed.to_vec()).to_utf8().map_err(|_| fail())?;
            let normalized = if normalized_raw.len() == 20
                && normalized_raw.ends_with('Z')
                && normalized_raw.as_bytes().get(10) == Some(&b'T')
            {
                format!("{}.000Z", &normalized_raw[..19])
            } else if normalized_raw.len() == 10
                && normalized_raw.as_bytes().get(4) == Some(&b'-')
                && normalized_raw.as_bytes().get(7) == Some(&b'-')
            {
                format!("{normalized_raw}T00:00:00.000Z")
            } else {
                normalized_raw
            };
            crate::planning::estimate::timestamp(&Value::string(&normalized))
                .or_else(|| {
                    time::OffsetDateTime::parse(
                        &normalized,
                        &time::format_description::well_known::Rfc3339,
                    )
                    .ok()
                    .and_then(|parsed| {
                        i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).ok()
                    })
                    .filter(|millis| {
                        (-8_640_000_000_000_000..=8_640_000_000_000_000).contains(millis)
                    })
                })
                .map(|millis| millis as f64)
                .ok_or_else(fail)
        }
        _ => Err(fail()),
    }
}

pub fn classify_stranded_work(input: &Value) -> Result<Vec<Value>, Error> {
    if crate::domain::members(input).is_none() {
        return Err(Error::type_error(
            "Stranded-work detection requires issue, worktree, branch, and heartbeat arrays",
        ));
    }
    let Some(issues) = input.get("issues").and_then(crate::domain::items) else {
        return Err(Error::type_error(
            "Stranded-work detection requires issue, worktree, branch, and heartbeat arrays",
        ));
    };
    let empty = Vec::new();
    let worktrees = match input.get("worktrees") {
        None => &empty[..],
        Some(value) => crate::domain::items(value).ok_or_else(|| {
            Error::type_error(
                "Stranded-work detection requires issue, worktree, branch, and heartbeat arrays",
            )
        })?,
    };
    let branches = match input.get("branches") {
        None => &empty[..],
        Some(value) => crate::domain::items(value).ok_or_else(|| {
            Error::type_error(
                "Stranded-work detection requires issue, worktree, branch, and heartbeat arrays",
            )
        })?,
    };
    let heartbeats = match input.get("heartbeats") {
        None => &empty[..],
        Some(value) => crate::domain::items(value).ok_or_else(|| {
            Error::type_error(
                "Stranded-work detection requires issue, worktree, branch, and heartbeat arrays",
            )
        })?,
    };
    let current = stranded_timestamp(input.get("now"), "now")?;
    let threshold_ms = match input.get("thresholdMs") {
        None => DEFAULT_STALE_THRESHOLD_MS,
        Some(value) => crate::domain::safe_integer(Some(value))
            .filter(|threshold| *threshold > 0)
            .ok_or_else(|| {
                Error::type_error(
                    "Heartbeat threshold must be a positive safe integer in milliseconds",
                )
            })?,
    };
    let mut claims = BTreeSet::new();
    for branch in branches {
        let name = branch
            .as_string()
            .filter(|text| !text.0.is_empty())
            .ok_or_else(|| Error::type_error("Branches must be nonempty strings"))?
            .to_utf8()?;
        claims.insert(name);
    }
    for worktree in worktrees {
        let name = worktree
            .get("branch")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Worktree must name its branch"))?;
        if name.0.is_empty() {
            return Err(Error::type_error("Branches must be nonempty strings"));
        }
        claims.insert(name.to_utf8()?);
    }
    let claim_list = claims.iter().cloned().collect::<Vec<_>>();
    let mut latest = std::collections::BTreeMap::<u64, f64>::new();
    for heartbeat in heartbeats {
        let issue = crate::domain::safe_integer(heartbeat.get("issue"))
            .filter(|number| *number > 0)
            .ok_or_else(|| Error::type_error("Heartbeat must name a positive safe issue number"))?;
        let time = stranded_timestamp(heartbeat.get("timestamp"), "Heartbeat")?;
        let entry = latest.entry(issue).or_insert(f64::NEG_INFINITY);
        if time > *entry {
            *entry = time;
        }
    }
    let mut results = Vec::new();
    for issue in issues {
        let number = crate::domain::safe_integer(issue.get("number"))
            .filter(|number| *number > 0)
            .ok_or_else(|| {
                Error::type_error(
                    "Issue must have a positive safe number, open/closed state, and assignees",
                )
            })?;
        let state = issue
            .get("state")
            .and_then(Value::as_string)
            .and_then(|text| text.to_utf8().ok())
            .filter(|state| ["OPEN", "CLOSED", "open", "closed"].contains(&state.as_str()))
            .ok_or_else(|| {
                Error::type_error(
                    "Issue must have a positive safe number, open/closed state, and assignees",
                )
            })?;
        let assignees = issue
            .get("assignees")
            .and_then(crate::domain::items)
            .ok_or_else(|| {
                Error::type_error(
                    "Issue must have a positive safe number, open/closed state, and assignees",
                )
            })?;
        if !state.eq_ignore_ascii_case("open") || assignees.is_empty() {
            continue;
        }
        let branch = select_issue_branch(number, &claim_list, None)?;
        let last_heartbeat = latest.get(&number).copied();
        let has_claim = issue_branches(number, None)?
            .iter()
            .any(|candidate| claims.contains(candidate));
        let status = if !has_claim {
            "stranded"
        } else if last_heartbeat.is_none_or(|last| current - last > threshold_ms as f64) {
            "stale"
        } else {
            "healthy"
        };
        results.push(Value::object(vec![
            ("issue", issue.clone()),
            ("branch", Value::string(&branch)),
            ("status", Value::string(status)),
            (
                "lastHeartbeat",
                last_heartbeat.map_or(Value::Null, Value::Number),
            ),
        ]));
    }
    Ok(results)
}

pub fn issue_wave(issue: &Value) -> Result<Option<u64>, Error> {
    if crate::domain::members(issue).is_none() {
        return Err(Error::type_error("Issue must be a JSON object"));
    }
    let mut waves = Vec::new();
    if let Some(labels) = issue.get("labels") {
        let items = crate::domain::items(labels)
            .ok_or_else(|| Error::new("Error", "Issue wave labels must be an array"))?;
        for label in items {
            let name = if let Some(text) = label.as_string() {
                Some(text.to_utf8()?)
            } else {
                label
                    .get("name")
                    .and_then(Value::as_string)
                    .map(Text::to_utf8)
                    .transpose()?
            };
            if let Some(wave) = name
                .as_deref()
                .and_then(|name| name.strip_prefix("wave:"))
                .filter(|digits| digits.len() == 1)
                .and_then(|digits| digits.parse::<u64>().ok())
                .filter(|wave| (1..=8).contains(wave))
            {
                if !waves.contains(&wave) {
                    waves.push(wave);
                }
            }
        }
    }
    if let Some(body) = issue.get("body").and_then(Value::as_string) {
        let body = body.to_utf8()?;
        for marker in body.split("<!-- Roster-Wave: ").skip(1) {
            if let Some((digits, _)) = marker.split_once(" -->") {
                if digits.len() == 1 {
                    if let Some(wave) = digits
                        .parse::<u64>()
                        .ok()
                        .filter(|wave| (1..=8).contains(wave))
                    {
                        if !waves.contains(&wave) {
                            waves.push(wave);
                        }
                        break;
                    }
                }
            }
        }
    }
    if waves.len() > 1 {
        return Err(Error::new("Error", "Issue has conflicting wave metadata"));
    }
    Ok(waves.first().copied())
}

pub fn issue_dependencies(issue: &Value) -> Result<Option<Vec<u64>>, Error> {
    if crate::domain::members(issue).is_none() {
        return Err(Error::type_error("Issue must be a JSON object"));
    }
    let body = issue
        .get("body")
        .and_then(Value::as_string)
        .map(Text::to_utf8)
        .transpose()?
        .unwrap_or_default();
    let lines = body
        .replace("\r\n", "\n")
        .split('\n')
        .filter(|line| {
            line.get(..11)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("depends on:"))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Ok(None);
    }
    if lines.len() != 1 {
        return Err(Error::new(
            "Error",
            "Issue dependency links must be declared once.",
        ));
    }
    let value = lines[0]
        .get(11..)
        .map(|s| Text(crate::text::trim(&Text::from(s).0).to_vec()).to_utf8())
        .transpose()?
        .unwrap_or_default();
    if value == "none" {
        return Ok(Some(Vec::new()));
    }
    let pattern = regex_lite::Regex::new(r"^#[1-9]\d*(?:,\s*#[1-9]\d*)*$")
        .map_err(|error| Error::new("Error", error.to_string()))?;
    if !pattern.is_match(&value) {
        return Err(Error::new(
            "Error",
            "Issue dependency links must be comma-separated #numbers or none.",
        ));
    }
    let self_number = crate::domain::safe_integer(issue.get("number"));
    let mut numbers = Vec::new();
    let mut seen = BTreeSet::new();
    for part in value.split(',') {
        let trimmed = part.trim();
        let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
        let Some(num) = digits
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=crate::MAX_SAFE_INTEGER).contains(n))
        else {
            return Err(Error::new("Error", "Issue dependency links are invalid."));
        };
        if self_number == Some(num) || !seen.insert(num) {
            return Err(Error::new("Error", "Issue dependency links are invalid."));
        }
        numbers.push(num);
    }
    Ok(Some(numbers))
}

#[derive(Debug, Clone)]
pub struct WavePlan {
    pub source: String,
    pub key: String,
    pub plan: Value,
}

pub fn read_wave_plan(worktree: &Path) -> Result<WavePlan, Error> {
    let file = worktree.join("PLAN.md");
    paths::ensure_local_path(&file, worktree)?;
    let entry = std::fs::symlink_metadata(&file).map_err(|_| {
        Error::new(
            "Error",
            "Wave plan must be a bounded regular single-link file",
        )
    })?;
    let links = paths::file_information(&file).map_or(0, |info| info.links);
    if !entry.is_file() || secure::linked(&entry) || links != 1 || entry.len() > 65_536 {
        return Err(Error::new(
            "Error",
            "Wave plan must be a bounded regular single-link file",
        ));
    }
    let Some(snapshot) = secure::read(&file, 65_536)? else {
        return Err(Error::new(
            "Error",
            "Wave plan must be a bounded regular single-link file",
        ));
    };
    let source = String::from_utf8(snapshot.bytes).map_err(|_| {
        Error::new(
            "Error",
            "Wave plan must be a bounded regular single-link file",
        )
    })?;
    let normalized = source.replace("\r\n", "\n");
    let key = crate::identity::sha256_hex(normalized.as_bytes());
    let plan = crate::planning::outline::parse_document(&Value::string(&source))?;
    Ok(WavePlan { source, key, plan })
}

pub fn parallel_limit(value: Option<&Value>) -> Result<u64, Error> {
    let Some(value) = value else {
        return Ok(1);
    };
    let number = match value {
        Value::String(text) => {
            let s = text
                .to_utf8()
                .map_err(|_| Error::type_error("--parallel must be a positive safe integer"))?;
            if s.is_empty() || s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error::type_error(
                    "--parallel must be a positive safe integer",
                ));
            }
            s.parse::<u64>()
                .ok()
                .filter(|n| (1..=crate::MAX_SAFE_INTEGER).contains(n))
        }
        other => crate::domain::safe_integer(Some(other)).filter(|n| *n >= 1),
    };
    number.ok_or_else(|| Error::type_error("--parallel must be a positive safe integer"))
}

pub fn format_issue_summary(cached: &Value, env: Option<&Value>) -> Result<String, Error> {
    let issue = cached
        .get("issue")
        .ok_or_else(|| Error::type_error("Issue summary requires cached issue metadata"))?;
    let number = crate::domain::safe_integer(issue.get("number"))
        .filter(|n| *n >= 1)
        .ok_or_else(|| Error::type_error("Issue summary requires cached issue metadata"))?;
    let redaction_cfg = crate::state::redaction::Config {
        environment: env
            .cloned()
            .unwrap_or_else(crate::ask::environment_value_from_process),
        ..crate::state::redaction::Config::default()
    };
    redaction_cfg.validate()?;
    let safe = |raw: &str| -> Result<String, Error> {
        let redacted =
            crate::state::redaction::evidence(&Text::from(raw), &redaction_cfg).to_utf8()?;
        let stripped = crate::ask::strip_vt_and_controls(&redacted);
        Ok(stripped
            .chars()
            .map(|ch| {
                if matches!(ch, '\x00'..='\x1f' | '\x7f') {
                    '?'
                } else {
                    ch
                }
            })
            .collect())
    };
    let field_or = |val: Option<&Value>, default: &str| -> Result<String, Error> {
        match val.filter(|v| **v != Value::Null) {
            Some(Value::String(text)) => text.to_utf8(),
            Some(other) => Ok(other.json()),
            None => Ok(default.to_owned()),
        }
    };
    let title = safe(&field_or(issue.get("title"), "-")?)?;
    let state = safe(&field_or(issue.get("state"), "UNKNOWN")?)?;
    let default_branch = format!("agents/{number}");
    let branch = safe(&field_or(cached.get("branch"), &default_branch)?)?;
    let pr_raw = match cached.get("openPr") {
        Some(Value::Null) => "(none)".to_owned(),
        Some(open_pr) => field_or(open_pr.get("url"), "(not cached)")?,
        None => "(not cached)".to_owned(),
    };
    let pr = safe(&pr_raw)?;
    Ok(format!(
        "Issue: #{number} {title}\nState: {state}\nBranch: {branch}\nPR: {pr}\n"
    ))
}

pub fn attempt_limit(value: Option<&Value>, maximum: Option<&Value>) -> Result<u64, Error> {
    let max_val = match maximum {
        None => Some(3),
        Some(val) => crate::domain::safe_integer(Some(val)).filter(|n| (1..=16).contains(n)),
    };
    let shown_max = maximum.map_or_else(|| "3".to_owned(), Value::json);
    let invalid = || {
        Error::type_error(format!(
            "--attempts must be an integer from 1 to seat.max_attempts ({shown_max}); maximum is 16"
        ))
    };
    let max_val = max_val.ok_or_else(invalid)?;
    let num = match value {
        None => 1,
        Some(val) => crate::domain::safe_integer(Some(val))
            .filter(|n| (1..=max_val).contains(n))
            .ok_or_else(invalid)?,
    };
    Ok(num)
}

pub fn attempt_gates(result: Option<&Value>) -> Value {
    let tests = if result
        .and_then(|r| r.get("testsSkipped"))
        .is_some_and(crate::domain::truthy)
    {
        "skipped"
    } else if result
        .and_then(|r| r.get("tests"))
        .and_then(|t| crate::domain::safe_integer(t.get("exit_code")))
        == Some(0)
    {
        "pass"
    } else {
        "fail"
    };
    let excellence = if result
        .and_then(|r| r.get("excellence"))
        .and_then(|e| e.get("pass"))
        .is_some_and(crate::domain::truthy)
    {
        "pass"
    } else {
        "fail"
    };
    let red_green_obj = result
        .and_then(|r| r.get("redGreen"))
        .filter(|v| **v != Value::Null);
    let red_green = match red_green_obj {
        Some(rg) if crate::domain::is_text(rg.get("status"), "unavailable") => "fail",
        Some(rg) if crate::domain::is_text(rg.get("status"), "checked") => {
            if rg
                .get("notRed")
                .and_then(crate::domain::items)
                .is_some_and(|items| !items.is_empty())
            {
                "fail"
            } else {
                "pass"
            }
        }
        _ => "skipped",
    };
    let shadow_obj = result
        .and_then(|r| r.get("shadow"))
        .filter(|v| **v != Value::Null);
    let shadow = match shadow_obj {
        Some(sh) if crate::domain::is_text(sh.get("status"), "unavailable") => "fail",
        None => "skipped",
        Some(sh)
            if crate::domain::is_text(sh.get("status"), "skipped")
                || crate::domain::is_text(sh.get("status"), "none") =>
        {
            "skipped"
        }
        Some(sh) => {
            if sh
                .get("findings")
                .and_then(crate::domain::items)
                .is_some_and(|items| !items.is_empty())
            {
                "fail"
            } else {
                "pass"
            }
        }
    };
    Value::object(vec![
        ("tests", Value::string(tests)),
        ("excellence", Value::string(excellence)),
        ("red_green", Value::string(red_green)),
        ("shadow", Value::string(shadow)),
    ])
}

pub fn select_attempt(attempts: &[Value]) -> Option<Value> {
    let mut eligible = attempts
        .iter()
        .filter(|attempt| {
            let has_error = attempt.get("error").is_some_and(crate::domain::truthy);
            let completed = attempt.get("completed");
            let failed = completed
                .and_then(|c| c.get("failed"))
                .is_some_and(crate::domain::truthy);
            let llm_mode = completed
                .and_then(|c| c.get("result"))
                .is_some_and(|r| crate::domain::is_text(r.get("mode"), "llm"));
            let evidence = attempt.get("evidence");
            let gates_ok = evidence
                .and_then(|e| e.get("gates"))
                .and_then(crate::domain::members)
                .is_some_and(|entries| {
                    !entries.is_empty()
                        && entries
                            .iter()
                            .all(|(_, verdict)| !crate::domain::is_text(Some(verdict), "fail"))
                });
            let review_ok =
                evidence.is_some_and(|e| crate::domain::is_text(e.get("review"), "pass"));
            !has_error && !failed && llm_mode && gates_ok && review_ok
        })
        .cloned()
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| {
        let num = |val: &Value, key: &str| {
            val.get("evidence")
                .and_then(|e| crate::domain::safe_integer(e.get(key)))
                .unwrap_or(u64::MAX)
        };
        num(left, "changed_lines")
            .cmp(&num(right, "changed_lines"))
            .then_with(|| num(left, "duration_ms").cmp(&num(right, "duration_ms")))
            .then_with(|| num(left, "index").cmp(&num(right, "index")))
    });
    eligible.into_iter().next()
}

pub fn attempt_summary(run: &Value) -> Result<String, Error> {
    let winner = crate::domain::safe_integer(run.get("winner"))
        .ok_or_else(|| Error::type_error("Attempt summary requires winner"))?;
    let total = crate::domain::safe_integer(run.get("attempts"))
        .ok_or_else(|| Error::type_error("Attempt summary requires attempts"))?;
    let results = run
        .get("attemptResults")
        .and_then(crate::domain::items)
        .ok_or_else(|| Error::type_error("Attempt summary requires attemptResults"))?;
    let str_field = |obj: &Value, key: &str| -> Result<String, Error> {
        match obj.get(key) {
            Some(Value::String(text)) => text.to_utf8(),
            Some(Value::Number(n)) => Ok((*n as u64).to_string()),
            _ => Err(Error::type_error(format!("Attempt result requires {key}"))),
        }
    };
    let mut rows = Vec::new();
    for attempt in results {
        let index = str_field(attempt, "index")?;
        let profile = str_field(attempt, "profile")?;
        let hardware = str_field(attempt, "hardware")?;
        let review = str_field(attempt, "review")?;
        let changed_lines = str_field(attempt, "changed_lines")?;
        let duration_ms = str_field(attempt, "duration_ms")?;
        let gates = attempt
            .get("gates")
            .and_then(crate::domain::members)
            .ok_or_else(|| Error::type_error("Attempt result requires gates"))?
            .iter()
            .map(|(k, v)| {
                let k_str = k.to_utf8()?;
                let v_str = v
                    .as_string()
                    .ok_or_else(|| Error::type_error("Gate verdict must be text"))?
                    .to_utf8()?;
                Ok(format!("{k_str}:{v_str}"))
            })
            .collect::<Result<Vec<_>, Error>>()?
            .join(",");
        rows.push(format!(
            "- {index}: profile={profile}; hardware={hardware}; gates={gates}; review={review}; changed_lines={changed_lines}; duration_ms={duration_ms}"
        ));
    }
    Ok(format!(
        "\n\n## Attempts\n\nSelected attempt {winner} of {total}; only the winner is published.\n\n{}",
        rows.join("\n")
    ))
}

pub fn assert_isolated_issue_branches(worktrees: &[Worktree]) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    for entry in worktrees {
        let Some(branch) = entry.branch.as_deref() else {
            continue;
        };
        let Some(parsed) = parse_issue_branch(branch) else {
            continue;
        };
        let key = (parsed.number, parsed.attempt);
        if !seen.insert(key) {
            return Err(Error::new(
                "Error",
                "Two issues cannot share a worktree branch; isolation is refused.",
            ));
        }
    }
    Ok(())
}

pub fn provenance_opt_out(env: Option<&Value>) -> bool {
    let check_val = |val: &Value| -> bool {
        match val {
            Value::Boolean(b) => *b,
            Value::String(text) => text.to_utf8().ok().is_some_and(|s| {
                matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            }),
            _ => false,
        }
    };
    if let Some(env) = env {
        if let Some(val) = env
            .get("ROSTER_PROVENANCE_OPT_OUT")
            .filter(|v| **v != Value::Null)
            .or_else(|| {
                env.get("ROSTER_PROVENANCE_DISABLED")
                    .filter(|v| **v != Value::Null)
            })
        {
            return check_val(val);
        }
        return false;
    }
    for key in ["ROSTER_PROVENANCE_OPT_OUT", "ROSTER_PROVENANCE_DISABLED"] {
        if let Ok(raw) = std::env::var(key) {
            return matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }
    }
    false
}

fn read_recap_artifact(worktree_path: &Path, name: &str) -> Result<Option<String>, Error> {
    let file = worktree_path.join(name);
    paths::ensure_local_path(&file, worktree_path)?;
    let Some(snapshot) = secure::read(&file, 65_536)? else {
        return Ok(None);
    };
    if paths::file_information(&file)?.links != 1 {
        return Err(Error::type_error("Artifact must be a single-link file"));
    }
    let text = String::from_utf8(snapshot.bytes)
        .map_err(|error| Error::new("Error", error.to_string()))?;
    Ok(Some(text.replace("\r\n", "\n")))
}

pub fn format_run_recap(
    worktree_path: &Path,
    issue_title: Option<&str>,
    finish_reason: Option<&str>,
    env: Option<&Value>,
) -> Result<String, Error> {
    let redaction_cfg = crate::state::redaction::Config {
        environment: env
            .cloned()
            .unwrap_or_else(crate::ask::environment_value_from_process),
        ..crate::state::redaction::Config::default()
    };
    redaction_cfg.validate()?;
    let safe = |raw: Option<&str>| -> Result<String, Error> {
        let value = raw.filter(|s| !s.is_empty()).unwrap_or("-");
        let redacted =
            crate::state::redaction::evidence(&Text::from(value), &redaction_cfg).to_utf8()?;
        let stripped = crate::ask::strip_vt_and_controls(&redacted);
        Ok(stripped
            .chars()
            .map(|ch| {
                if matches!(ch, '\x00'..='\x1f' | '\x7f') {
                    '?'
                } else {
                    ch
                }
            })
            .collect())
    };
    let task_doc = read_recap_artifact(worktree_path, "TASK.md")?
        .map(|src| {
            crate::planning::task::parse(
                &Value::string(&src),
                &crate::planning::task::Authority::default(),
            )
        })
        .transpose()?;
    let doc_title = task_doc.as_ref().and_then(|d| d.title.to_utf8().ok());
    let outcome = safe(doc_title.as_deref().or(issue_title))?;
    let files_joined = task_doc.as_ref().map(|d| d.files_allowed.join(", "));
    let files_str = safe(files_joined.as_deref())?;
    let review_source = read_recap_artifact(worktree_path, "REVIEW.md")?.unwrap_or_default();
    let review_verdict = regex_lite::Regex::new(r"(?m)^Verdict: (pass|fail)$")
        .ok()
        .and_then(|re| re.captures(&review_source))
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_owned()));
    let result_source = read_recap_artifact(worktree_path, "RESULT.md")?.unwrap_or_default();
    let test_exit = regex_lite::Regex::new(r"- node --test exited (\d+)")
        .ok()
        .and_then(|re| re.captures(&result_source))
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_owned()));
    let last_test = match test_exit {
        Some(code) => format!("node --test (exit {code})"),
        None => "-".to_owned(),
    };
    let finish_str = safe(finish_reason)?;
    Ok(format!(
        "Outcome: {outcome} | Files: {files_str} | Last test: {last_test} | Review: {} | Finish reason: {finish_str}\n",
        review_verdict.as_deref().unwrap_or("-")
    ))
}

pub fn parse_open_issues_json(raw: &str) -> Result<Vec<Value>, Error> {
    let parsed = crate::json::parse(&Text::from(raw))
        .map_err(|_| Error::new("Error", "GitHub issue list returned invalid metadata JSON"))?;
    let Some(items) = crate::domain::items(&parsed) else {
        return Err(Error::new(
            "Error",
            "GitHub issue list returned invalid numbers or titles",
        ));
    };
    if items.len() > 100 {
        return Err(Error::new(
            "Error",
            "GitHub issue list returned invalid numbers or titles",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let Some(number) = crate::domain::safe_integer(item.get("number")).filter(|n| *n >= 1)
        else {
            return Err(Error::new(
                "Error",
                "GitHub issue list returned invalid numbers or titles",
            ));
        };
        let Some(title) = item
            .get("title")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .filter(|s| !s.trim().is_empty())
        else {
            return Err(Error::new(
                "Error",
                "GitHub issue list returned invalid numbers or titles",
            ));
        };
        if !seen.insert(number) {
            return Err(Error::new(
                "Error",
                "GitHub issue list returned invalid numbers or titles",
            ));
        }
        result.push(Value::object(vec![
            ("number", Value::Number(number as f64)),
            ("title", Value::string(&title)),
            ("state", Value::string("OPEN")),
        ]));
    }
    Ok(result)
}

pub fn parse_diff_names(raw: &str) -> Vec<String> {
    raw.split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn inspect_local_issue_worktree(number: u64, worktree_path: &Path) -> Result<Value, Error> {
    if !(1..=crate::MAX_SAFE_INTEGER).contains(&number) {
        return Err(Error::type_error(
            "Issue number must be a positive safe integer",
        ));
    }
    let assignment = read_recap_artifact(worktree_path, "ASSIGNMENT.md")?.ok_or_else(|| {
        Error::new(
            "Error",
            "Local assignment does not match the requested issue",
        )
    })?;
    let re = regex_lite::Regex::new(
        r"(?s)^# Assignment\n\n- Issue URL: (https://github\.com/[^\s]+/issues/([1-9]\d*))\n- Issue number: ([1-9]\d*)\n- Title: ([^\n]+)\n\n(?:- Wave: ([1-8])\n\n)?## Ask\n\n(.+)$",
    )
    .map_err(|error| Error::new("Error", error.to_string()))?;
    let Some(caps) = re.captures(&assignment) else {
        return Err(Error::new(
            "Error",
            "Local assignment does not match the requested issue",
        ));
    };
    let url = caps
        .get(1)
        .map(|m| m.as_str().to_owned())
        .unwrap_or_default();
    let url_num = caps.get(2).and_then(|m| m.as_str().parse::<u64>().ok());
    let num = caps.get(3).and_then(|m| m.as_str().parse::<u64>().ok());
    if url_num != Some(number) || num != Some(number) {
        return Err(Error::new(
            "Error",
            "Local assignment does not match the requested issue",
        ));
    }
    let title = caps
        .get(4)
        .map(|m| m.as_str().to_owned())
        .unwrap_or_default();
    let wave = caps.get(5).and_then(|m| m.as_str().parse::<u64>().ok());
    let raw_body = caps
        .get(6)
        .map(|m| m.as_str().strip_suffix('\n').unwrap_or(m.as_str()))
        .unwrap_or("");
    let parsed_body = crate::planning::intake::parse_issue_body(&Value::string(raw_body))?;
    let ask_str = parsed_body.ask.to_utf8()?;
    let classified = crate::planning::intake::classify(
        &Value::string(&ask_str),
        Some(&Text::from(title.as_str())),
        &[],
    )?;
    let ask_kind = classified
        .get("kind")
        .and_then(Value::as_string)
        .map(|t| t.to_utf8())
        .transpose()?
        .unwrap_or_else(|| "slice".to_owned());
    let review = read_recap_artifact(worktree_path, "REVIEW.md")?;
    let result = read_recap_artifact(worktree_path, "RESULT.md")?;
    let verdict = review.as_deref().and_then(|src| {
        regex_lite::Regex::new(r"(?m)^Verdict: (pass|fail)$")
            .ok()
            .and_then(|r| r.captures(src))
            .and_then(|c| c.get(1).map(|m| m.as_str().to_owned()))
    });
    let state = match verdict.as_deref() {
        Some("pass") => "passed",
        Some("fail") => "failed",
        _ if result
            .as_deref()
            .is_some_and(|s| s.contains("Checks: FAIL")) =>
        {
            "failed"
        }
        _ => "idle",
    };
    let has_plan = read_recap_artifact(worktree_path, "PLAN.md")?.is_some();
    let has_task = read_recap_artifact(worktree_path, "TASK.md")?.is_some();
    let has_recipe = read_recap_artifact(worktree_path, "RECIPE.yml")?.is_some();
    let plan_mode = ask_kind == "slice" && has_plan && !has_task && !has_recipe;
    let mut issue_fields = vec![
        ("number", Value::Number(number as f64)),
        ("title", Value::string(&title)),
        ("body", Value::string(raw_body)),
        ("url", Value::string(&url)),
        ("state", Value::string("UNKNOWN")),
    ];
    if let Some(w) = wave {
        issue_fields.push((
            "labels",
            Value::Array(vec![Value::object(vec![(
                "name",
                Value::string(&format!("wave:{w}")),
            )])]),
        ));
    }
    Ok(Value::object(vec![
        ("issue", Value::object(issue_fields)),
        ("ask", Value::string(&ask_str)),
        ("metadata", parsed_body.metadata.unwrap_or(Value::Null)),
        ("task", Value::string(&format!("issue-{number}"))),
        ("session", Value::string(&format!("roster-{number}-coder"))),
        ("reused", Value::Boolean(true)),
        (
            "state",
            Value::string(if plan_mode { "planning" } else { state }),
        ),
        (
            "reviewVerdict",
            verdict.map_or(Value::Null, |v| Value::string(&v)),
        ),
        ("askKind", Value::string(&ask_kind)),
        ("planMode", Value::Boolean(plan_mode)),
    ]))
}

fn issue_has_board_label(issue: &Value, name: &str) -> bool {
    let prefixed = format!("roster:{name}");
    issue
        .get("labels")
        .and_then(crate::domain::items)
        .is_some_and(|labels| {
            labels.iter().any(|l| {
                let label_str = l
                    .as_string()
                    .or_else(|| l.get("name").and_then(Value::as_string))
                    .and_then(|t| t.to_utf8().ok())
                    .unwrap_or_default();
                label_str == name || label_str == prefixed
            })
        })
}

pub fn compute_wave_board_rows(
    plan: &Value,
    linked_issues: &[Value],
    active_issue: Option<u64>,
    active_state: Option<&str>,
) -> Result<Vec<Value>, Error> {
    let Some(drafts) = plan.get("issues").and_then(crate::domain::items) else {
        return Err(Error::type_error("Wave plan requires issues"));
    };
    let draft_re = regex_lite::Regex::new(r"<!-- Roster-Draft: ([1-9]\d*) -->")
        .map_err(|e| Error::new("Error", e.to_string()))?;
    let mut matched = std::collections::BTreeMap::new();
    for issue in linked_issues {
        let body = issue
            .get("body")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_default();
        let draft_idx = draft_re
            .captures(&body)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .filter(|&idx| (1..=drafts.len()).contains(&idx))
            .ok_or_else(|| Error::new("Error", "Linked GitHub drafts conflict with PLAN.md"))?;
        let expected_wave = crate::domain::safe_integer(drafts[draft_idx - 1].get("wave"));
        if matched.contains_key(&draft_idx) || issue_wave(issue)? != expected_wave {
            return Err(Error::new(
                "Error",
                "Linked GitHub drafts conflict with PLAN.md",
            ));
        }
        matched.insert(draft_idx, issue.clone());
    }
    let mut rows = Vec::with_capacity(drafts.len());
    for (idx, draft) in drafts.iter().enumerate() {
        let wave_num = crate::domain::safe_integer(draft.get("wave")).unwrap_or(1);
        let title = draft
            .get("title")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_default();
        let issue_opt = matched.get(&(idx + 1));
        let mut deps_open = false;
        if let Some(issue) = issue_opt {
            if !crate::domain::is_text(issue.get("state"), "CLOSED") {
                match issue_dependencies(issue)? {
                    Some(dep_nums) => {
                        for dep_num in dep_nums {
                            let dep_closed = matched.values().any(|cand| {
                                crate::domain::safe_integer(cand.get("number")) == Some(dep_num)
                                    && crate::domain::is_text(cand.get("state"), "CLOSED")
                            });
                            if !dep_closed {
                                deps_open = true;
                            }
                        }
                    }
                    None => {
                        deps_open = matched.values().any(|earlier| {
                            issue_wave(earlier).ok().flatten().unwrap_or(wave_num) < wave_num
                                && crate::domain::is_text(earlier.get("state"), "OPEN")
                        });
                    }
                }
            }
        } else {
            deps_open = matched.values().any(|earlier| {
                issue_wave(earlier).ok().flatten().unwrap_or(wave_num) < wave_num
                    && crate::domain::is_text(earlier.get("state"), "OPEN")
            });
        }
        let issue_num = issue_opt.and_then(|iss| crate::domain::safe_integer(iss.get("number")));
        let state = match issue_opt {
            Some(iss) if crate::domain::is_text(iss.get("state"), "CLOSED") => "done",
            Some(iss) if deps_open || issue_has_board_label(iss, "blocked") => "blocked",
            Some(_)
                if issue_num.is_some()
                    && active_issue == issue_num
                    && matches!(active_state, Some("planning" | "drafting" | "testing")) =>
            {
                "running"
            }
            Some(iss)
                if (issue_num.is_some()
                    && active_issue == issue_num
                    && matches!(active_state, Some("reviewing" | "passed")))
                    || issue_has_board_label(iss, "review") =>
            {
                "review"
            }
            Some(iss) if issue_has_board_label(iss, "in-progress") => "running",
            None if deps_open => "blocked",
            _ => "todo",
        };
        rows.push(Value::object(vec![
            ("wave", Value::Number(wave_num as f64)),
            ("title", Value::string(&title)),
            ("state", Value::string(state)),
            (
                "issue",
                issue_num.map_or(Value::Null, |n| Value::Number(n as f64)),
            ),
        ]));
    }
    Ok(rows)
}

pub fn select_ready_wave_batch(
    waves: &[Value],
    attempted: &BTreeSet<u64>,
    parallel: Option<&Value>,
    capacity: Option<&Value>,
) -> Result<(u64, Vec<Value>), Error> {
    let limit = parallel_limit(parallel)?.min(parallel_limit(capacity)?);
    let ready = waves
        .iter()
        .filter(|row| {
            crate::domain::is_text(row.get("state"), "todo")
                && crate::domain::safe_integer(row.get("issue"))
                    .is_some_and(|n| !attempted.contains(&n))
        })
        .take(limit as usize)
        .cloned()
        .collect::<Vec<_>>();
    Ok((limit, ready))
}

pub fn parse_issue_branch(branch: &str) -> Option<IssueBranch> {
    let tail = branch
        .strip_prefix("agents/")
        .or_else(|| branch.strip_prefix("issue-"))?;
    let (number, attempt) = tail
        .split_once("-a")
        .map_or((tail, None), |(number, attempt)| (number, Some(attempt)));
    if number.starts_with('0') || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number = number.parse::<u64>().ok()?;
    let attempt = match attempt {
        Some(value) if value.starts_with('0') => return None,
        Some(value) => Some(value.parse::<u8>().ok()?),
        None => None,
    };
    issue_branch(number, attempt).ok()?;
    Some(IssueBranch { number, attempt })
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub locked: bool,
    pub prunable: bool,
}

fn sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn parse_inventory(bytes: &[u8]) -> Result<Vec<Worktree>, Error> {
    let source = std::str::from_utf8(bytes)
        .map_err(|_| Error::type_error("Git worktree inventory must be UTF-8"))?;
    let mut result = Vec::new();
    let mut issues = BTreeSet::new();
    let mut paths_seen = BTreeSet::new();
    for record in source.split("\0\0").filter(|record| !record.is_empty()) {
        let mut path = None;
        let mut head = None;
        let mut branch = None;
        let mut detached = false;
        let mut locked = false;
        let mut prunable = false;
        for field in record.split('\0').filter(|field| !field.is_empty()) {
            if let Some(value) = field.strip_prefix("worktree ") {
                if path.replace(PathBuf::from(value)).is_some() {
                    return Err(Error::type_error("Duplicate worktree path field"));
                }
            } else if let Some(value) = field.strip_prefix("HEAD ") {
                if !sha(value) || head.replace(value.to_owned()).is_some() {
                    return Err(Error::type_error(
                        "Worktree inventory has an invalid commit",
                    ));
                }
            } else if let Some(value) = field.strip_prefix("branch refs/heads/") {
                if branch.replace(value.to_owned()).is_some() || detached {
                    return Err(Error::type_error(
                        "Worktree inventory has ambiguous branch ownership",
                    ));
                }
            } else if field == "detached" && branch.is_none() {
                detached = true;
            } else if field == "locked" || field.starts_with("locked ") {
                locked = true;
            } else if field == "prunable" || field.starts_with("prunable ") {
                prunable = true;
            } else {
                return Err(Error::type_error(
                    "Unsupported Git worktree inventory field",
                ));
            }
        }
        let path = path
            .filter(|path| path.is_absolute())
            .ok_or_else(|| Error::type_error("Registered worktree has no absolute path"))?;
        let key = path.to_string_lossy().to_lowercase();
        if !paths_seen.insert(key) {
            return Err(Error::type_error(
                "Two Git worktree registrations name the same path",
            ));
        }
        if let Some(issue) = branch.as_deref().and_then(parse_issue_branch) {
            if !issues.insert((issue.number, issue.attempt)) {
                return Err(Error::new(
                    "WorktreeOwnershipError",
                    "Two issue branches have the same task ownership; preserve both and resolve their namespaces explicitly",
                ));
            }
        }
        result.push(Worktree {
            path,
            head: head.ok_or_else(|| Error::type_error("Registered worktree has no commit"))?,
            branch,
            locked,
            prunable,
        });
    }
    if result.is_empty() || result.len() > 256 {
        return Err(Error::type_error(
            "Git worktree inventory must contain 1-256 worktrees",
        ));
    }
    Ok(result)
}

pub struct Git {
    host: tools::Host,
    image: Image,
    no_hooks: PathBuf,
    uncertain: Cell<bool>,
    timings: Option<crate::execution::timing::Timings>,
}

struct Pending<'a> {
    uncertain: &'a Cell<bool>,
    settled: bool,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.uncertain.set(true);
        }
    }
}

fn git_failure_facts(root: &Path, args: &[&str], failure: &crate::process::Failure) -> Error {
    let operation = match args.first().copied() {
        Some("rev-parse") => "revision",
        Some("status") => "status",
        Some("config") => "configuration",
        Some("diff") => "diff",
        Some("worktree") => "worktree",
        Some("clone") => "clone",
        Some("submodule") => "submodule",
        _ => "other",
    };
    let cwd = match std::fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => "directory",
        Ok(_) => "not-directory",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "unavailable",
    };
    let os_code = failure
        .source
        .as_ref()
        .and_then(std::io::Error::raw_os_error);
    let io_kind = failure.source.as_ref().map(std::io::Error::kind);
    Error::new(
        "NativeGitFailureFacts",
        format!(
            "post-failure operation={operation}; os_code={os_code:?}; io_kind={io_kind:?}; cwd_state={cwd}; existing_image_checks=passed; existing_cancel_check=passed; cleanup_complete={}; child_pid_present={}",
            failure.cleanup_complete,
            failure.captured.pid.is_some()
        ),
    )
}

fn repository_paths_need_legacy(paths: &RepositoryPaths) -> bool {
    [&paths.worktree, &paths.common_dir, &paths.git_dir]
        .into_iter()
        .any(|path| path.as_os_str().as_encoded_bytes().contains(&b'\n'))
}

fn parse_repository_paths(bytes: &[u8]) -> Result<[String; 3], Error> {
    if !u64::try_from(bytes.len()).is_ok_and(|length| length <= crate::execution::MAX_FILE_BYTES) {
        return Err(Error::type_error("Git repository paths exceed 16 MiB"));
    }
    let source =
        std::str::from_utf8(bytes).map_err(|_| Error::type_error("Git metadata must be UTF-8"))?;
    let source = source
        .strip_suffix('\n')
        .ok_or_else(|| Error::type_error("Git repository paths require three complete lines"))?;
    let mut lines = source.split('\n');
    let [Some(root), Some(common), Some(git_dir)] = [lines.next(), lines.next(), lines.next()]
    else {
        return Err(Error::type_error(
            "Git repository paths require exactly three lines",
        ));
    };
    if lines.next().is_some() {
        return Err(Error::type_error(
            "Git repository paths require exactly three lines",
        ));
    }
    let result = [root, common, git_dir].map(|line| line.trim_end_matches('\r').to_owned());
    if result
        .iter()
        .any(|path| path.is_empty() || path.contains('\0'))
    {
        return Err(Error::type_error(
            "Git repository paths require three nonempty paths without NUL",
        ));
    }
    Ok(result)
}

impl Git {
    pub fn new(host: tools::Host, no_hooks: PathBuf) -> Result<Self, Error> {
        let image = Image::approve(&host.git)?;
        secure::check_components(&no_hooks)?;
        if !secure::entries(&no_hooks)?.is_empty() {
            return Err(Error::type_error(
                "The trusted Git hook-disable directory must be empty",
            ));
        }
        Ok(Self {
            host,
            image,
            no_hooks,
            uncertain: Cell::new(false),
            timings: None,
        })
    }

    pub(crate) fn bind_timings(&mut self, timings: crate::execution::timing::Timings) {
        self.timings = Some(timings);
    }

    pub fn cleanup_certain(&self) -> bool {
        !self.uncertain.get()
    }

    pub async fn run(
        &self,
        root: &Path,
        args: &[&str],
        token: &CancellationToken,
    ) -> Result<Vec<u8>, Error> {
        use crate::execution::timing::Label;
        let label = match args.first().copied() {
            Some("rev-parse")
                if args
                    == [
                        "rev-parse",
                        "--show-toplevel",
                        "--git-common-dir",
                        "--git-dir",
                    ] =>
            {
                Label::StartGitPaths
            }
            Some("rev-parse") if args.contains(&"--show-toplevel") => Label::StartGitRoot,
            Some("rev-parse") if args.contains(&"--git-common-dir") => Label::StartGitCommon,
            Some("rev-parse") if args.contains(&"--git-dir") => Label::StartGitDirectory,
            Some("rev-parse") => Label::StartGitRevision,
            Some("symbolic-ref") => Label::StartGitBranch,
            Some("config") => Label::StartGitConfig,
            Some("status") => Label::StartGitStatus,
            Some("worktree") => Label::StartGitWorktrees,
            Some("diff") => Label::StartGitSourceDiff,
            Some("ls-files") => Label::StartGitUntracked,
            Some("check-ignore") => Label::StartGitIgnore,
            _ => Label::StartGitOther,
        };
        let _scope = self.timings.as_ref().map(|timings| timings.start(label));
        token.check()?;
        let image_scope = _scope.as_ref().map(|span| span.child(Label::StartGitImage));
        let checked = self.image.require_current();
        if let Some(image_scope) = image_scope {
            image_scope.finish_status(checked.is_ok());
        }
        checked?;
        if self.uncertain.get() || !secure::entries(&self.no_hooks)?.is_empty() {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Git ownership or hook suppression is unverified; preserve the worktree and lock",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        let mut scoped = if args.first() == Some(&"check-ignore") {
            Vec::new()
        } else {
            [
                "-c",
                "core.longpaths=true",
                "-c",
                "submodule.recurse=false",
                "-c",
                "fetch.recurseSubmodules=false",
                "-c",
                "init.templateDir=",
                "-c",
                "protocol.allow=never",
            ]
            .map(str::to_owned)
            .to_vec()
        };
        scoped.extend(args.iter().map(|arg| (*arg).to_owned()));
        let command = self
            .host
            .git_command(root, &self.no_hooks, &scoped, &Default::default());
        let command_scope = self
            .timings
            .as_ref()
            .map(|timings| timings.start(Label::StartGitExecute));
        let mut pending = Pending {
            uncertain: &self.uncertain,
            settled: false,
        };
        let result = owned::execute_observed_timed(
            command,
            Limits {
                timeout: Some(Duration::from_secs(30)),
                max_output_bytes: 16 * 1024 * 1024,
            },
            Some(token),
            None,
            command_scope.as_ref(),
        )
        .await;
        pending.settled = true;
        if let Some(scope) = command_scope {
            scope.finish_status(result.is_ok());
        }
        if result.as_ref().is_err_and(|failure| {
            !failure.cleanup_complete || matches!(failure.kind, FailureKind::Exit(None))
        }) {
            self.uncertain.set(true);
        }
        if self.uncertain.get() {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Git process-tree shutdown is unverified; retain ownership for explicit recovery",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        let image_scope = _scope.as_ref().map(|span| span.child(Label::StartGitImage));
        let checked = self.image.require_current();
        if let Some(image_scope) = image_scope {
            image_scope.finish_status(checked.is_ok());
        }
        checked?;
        match result {
            Ok(output) => {
                token.check()?;
                Ok(output.captured.stdout)
            }
            Err(failure) => {
                if !failure.cleanup_complete || matches!(failure.kind, FailureKind::Exit(None)) {
                    self.uncertain.set(true);
                    return Err(Error::new(
                        "OwnedProcessCleanupError",
                        "Git process-tree shutdown is unverified; retain ownership for explicit recovery",
                    )
                    .coded("ROSTER_PROCESS_CLEANUP"));
                }
                token.check()?;
                let code = failure.code().unwrap_or("ROSTER_GIT_FAILED");
                let facts = git_failure_facts(root, args, &failure);
                Err(Error::new(
                    "NativeGitError",
                    format!(
                        "Approved Git operation failed: {failure}; verify the selected repository, ref and offline objects"
                    ),
                )
                .coded(code)
                .caused_by(facts))
            }
        }
    }

    pub async fn text(
        &self,
        root: &Path,
        args: &[&str],
        token: &CancellationToken,
    ) -> Result<String, Error> {
        String::from_utf8(self.run(root, args, token).await?)
            .map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
            .map_err(|_| Error::type_error("Git metadata must be UTF-8"))
    }

    pub(crate) async fn repository_paths(
        &self,
        paths: &RepositoryPaths,
        token: &CancellationToken,
    ) -> Result<[String; 3], Error> {
        if repository_paths_need_legacy(paths) {
            let root = self
                .text(&paths.worktree, &["rev-parse", "--show-toplevel"], token)
                .await?;
            let common = self
                .text(&paths.worktree, &["rev-parse", "--git-common-dir"], token)
                .await?;
            let git_dir = self
                .text(&paths.worktree, &["rev-parse", "--git-dir"], token)
                .await?;
            return Ok([root, common, git_dir]);
        }
        parse_repository_paths(
            &self
                .run(
                    &paths.worktree,
                    &[
                        "rev-parse",
                        "--show-toplevel",
                        "--git-common-dir",
                        "--git-dir",
                    ],
                    token,
                )
                .await?,
        )
    }

    async fn optional(
        &self,
        root: &Path,
        args: &[&str],
        token: &CancellationToken,
    ) -> Result<Option<String>, Error> {
        match self.text(root, args, token).await {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.code == Some("ROSTER_GIT_FAILED") && self.cleanup_certain() => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Repository {
    pub paths: RepositoryPaths,
    pub head: String,
    pub branch: Option<String>,
    pub dirty: bool,
    pub worktrees: Vec<Worktree>,
    root: FileIdentity,
    common: FileIdentity,
    git_dir: FileIdentity,
    status: Vec<u8>,
    config: Vec<u8>,
    source_digest: String,
    indexes: IndexSeal,
}

async fn source_digest(git: &Git, root: &Path, token: &CancellationToken) -> Result<String, Error> {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    digest.update(
        git.run(
            root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--binary",
                "HEAD",
                "--",
            ],
            token,
        )
        .await?,
    );
    let untracked = git
        .text(
            root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            token,
        )
        .await?;
    let mut bytes = 0_u64;
    for (index, path) in untracked
        .split('\0')
        .filter(|path| !path.is_empty())
        .enumerate()
    {
        if index >= crate::execution::MAX_FILES {
            return Err(Error::type_error(
                "Untracked start-source inventory exceeds 2048 paths",
            ));
        }
        if !policy::exact_path(path) {
            return Err(Error::type_error(
                "Untracked source inventory contains a non-exact path",
            ));
        }
        digest.update(path.as_bytes());
        if crate::planning::forbidden_read(path) || policy::managed(path) {
            continue;
        }
        let file = root.join(path);
        let snapshot = secure::read(&file, crate::execution::MAX_FILE_BYTES)?
            .ok_or_else(|| Error::type_error("Untracked source disappeared while preparing"))?;
        bytes = bytes
            .checked_add(snapshot.bytes.len() as u64)
            .filter(|bytes| *bytes <= crate::execution::MAX_SNAPSHOT_BYTES)
            .ok_or_else(|| Error::type_error("Untracked start-source snapshot exceeds 128 MiB"))?;
        if paths::file_information(&file)?.links != 1
            || !secure::unchanged(&file, &snapshot, crate::execution::MAX_FILE_BYTES)?
        {
            return Err(Error::type_error(
                "Untracked source identity changed while preparing",
            ));
        }
        digest.update(snapshot.bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn reject_git_execution_config(bytes: &[u8]) -> Result<(), Error> {
    let source = std::str::from_utf8(bytes)
        .map_err(|_| Error::type_error("Local Git configuration must be UTF-8"))?;
    for entry in source.split('\0').filter(|entry| !entry.is_empty()) {
        let key = entry.split('\n').next().unwrap_or("").to_ascii_lowercase();
        if key.starts_with("include.")
            || key.starts_with("includeif.")
            || key.starts_with("filter.")
            || key.starts_with("url.")
            || key.starts_with("credential.")
            || key.starts_with("http.")
            || key.starts_with("extensions.")
            || key == "core.sshcommand"
            || key == "core.askpass"
            || key == "core.gitproxy"
            || key == "core.alternaterefscommand"
            || key.starts_with("uploadpack.")
            || key.starts_with("diff.")
                && (key.ends_with(".command")
                    || key.ends_with(".textconv")
                    || key == "diff.external")
            || key.ends_with(".uploadpack")
            || key.ends_with(".receivepack")
            || key.ends_with(".vcs")
            || key.ends_with(".proxy")
            || key.starts_with("submodule.") && key.ends_with(".update")
        {
            return Err(Error::new(
                "GitConfigurationDenied",
                "Local Git configuration selects includes, filters, helpers or custom execution; remove those selections in a human-reviewed checkout before native local preparation",
            )
            .coded("ROSTER_GIT_CONFIG_DENIED"));
        }
    }
    Ok(())
}

fn require_index_seal(indexes: &IndexSeal) -> Result<(), Error> {
    indexes.require_current().map_err(|cause| {
        Error::new(
            "StaleRepository",
            "Original worktree Git index changed or cannot be verified; no stale preparation is accepted",
        )
        .coded("ROSTER_STALE_SOURCE")
        .caused_by(cause)
    })
}

impl Repository {
    pub async fn inspect(cwd: &Path, git: &Git, token: &CancellationToken) -> Result<Self, Error> {
        let paths = paths::repository_paths(cwd)?;
        let root = paths::file_information(&paths.worktree)?.identity;
        let common = paths::file_information(&paths.common_dir)?.identity;
        let git_dir = paths::file_information(&paths.git_dir)?.identity;
        let [actual, actual_common, actual_dir] = git.repository_paths(&paths, token).await?;
        if paths::file_information(Path::new(&actual))?.identity != root
            || paths::file_information(&paths::resolve(
                &paths.worktree,
                Path::new(&actual_common),
            )?)?
            .identity
                != common
            || paths::file_information(&paths::resolve(&paths.worktree, Path::new(&actual_dir))?)?
                .identity
                != git_dir
        {
            return Err(Error::type_error(
                "Git and filesystem repository ownership disagree",
            ));
        }
        let primary = git
            .text(
                &paths.worktree,
                &["rev-parse", "--git-path", "index"],
                token,
            )
            .await?;
        let mut indexes = IndexSeal::capture(&paths.worktree, &primary)?;
        let shared = git
            .text(
                &paths.worktree,
                &["rev-parse", "--shared-index-path"],
                token,
            )
            .await?;
        indexes.include_shared(&paths.worktree, &shared)?;
        token.check()?;
        require_index_seal(&indexes)?;
        let head = git
            .text(
                &paths.worktree,
                &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
                token,
            )
            .await?;
        if !sha(&head) {
            return Err(Error::type_error(
                "Local delivery requires a complete committed Git HEAD",
            ));
        }
        let branch = git
            .optional(
                &paths.worktree,
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
                token,
            )
            .await?;
        let config = git
            .run(
                &paths.worktree,
                &["config", "--local", "--no-includes", "--null", "--list"],
                token,
            )
            .await?;
        reject_git_execution_config(&config)?;
        let status = git
            .run(
                &paths.worktree,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                token,
            )
            .await?;
        let worktrees = parse_inventory(
            &git.run(
                &paths.worktree,
                &["worktree", "list", "--porcelain", "-z"],
                token,
            )
            .await?,
        )?;
        let source_digest = source_digest(git, &paths.worktree, token).await?;
        if worktrees
            .iter()
            .filter(|entry| {
                paths::file_information(&entry.path).is_ok_and(|entry| entry.identity == root)
            })
            .count()
            != 1
        {
            return Err(Error::type_error(
                "Current checkout has no unique registered worktree owner",
            ));
        }
        token.check()?;
        require_index_seal(&indexes)?;
        Ok(Self {
            paths,
            head,
            branch,
            dirty: !status.is_empty(),
            worktrees,
            root,
            common,
            git_dir,
            status,
            config,
            source_digest,
            indexes,
        })
    }

    fn require_indexes_current(&self) -> Result<(), Error> {
        require_index_seal(&self.indexes)
    }

    pub async fn require_current(&self, git: &Git, token: &CancellationToken) -> Result<(), Error> {
        token.check()?;
        let inspected = Self::inspect(&self.paths.worktree, git, token).await;
        token.check()?;
        self.require_indexes_current()?;
        let current = inspected?;
        token.check()?;
        self.require_indexes_current()?;
        if self.root != current.root
            || self.common != current.common
            || self.git_dir != current.git_dir
            || self.head != current.head
            || self.branch != current.branch
            || self.status != current.status
            || self.config != current.config
            || self.source_digest != current.source_digest
        {
            return Err(Error::new(
                "StaleRepository",
                "Repository identity, HEAD, branch, index, local Git configuration or source status changed; no stale preparation is accepted",
            )
            .coded("ROSTER_STALE_SOURCE"));
        }
        token.check()
    }
}

#[derive(Debug, Clone)]
pub struct Start {
    pub mode: &'static str,
    pub reference: Option<String>,
    pub commit: String,
    pub fetched: bool,
    pub notes: Vec<String>,
}

impl Start {
    pub fn format(&self) -> String {
        let location = match &self.reference {
            Some(reference) => format!(
                "{} {reference}@{}",
                self.mode,
                self.commit.get(..7).unwrap_or("invalid")
            ),
            None => "current HEAD".to_owned(),
        };
        let mut parts = vec![format!(
            "Start: {location} ({})",
            if self.fetched {
                "fetched"
            } else {
                "freshness unknown"
            }
        )];
        parts.extend(self.notes.iter().cloned());
        parts.join("; ")
    }
}

async fn commit_of(
    git: &Git,
    root: &Path,
    reference: &str,
    token: &CancellationToken,
) -> Result<String, Error> {
    let full_sha = matches!(reference.len(), 40 | 64)
        && reference.bytes().all(|byte| byte.is_ascii_hexdigit());
    let selected = if full_sha {
        reference.to_ascii_lowercase()
    } else if reference == "HEAD" {
        "HEAD".to_owned()
    } else {
        let candidates = if reference.starts_with("refs/") {
            vec![reference.to_owned()]
        } else {
            vec![
                format!("refs/heads/{reference}"),
                format!("refs/tags/{reference}"),
                format!("refs/remotes/{reference}"),
                format!("refs/remotes/{reference}/HEAD"),
            ]
        };
        let mut args = vec!["for-each-ref", "--format=%(refname)"];
        args.extend(candidates.iter().map(String::as_str));
        let output = git.text(root, &args, token).await?;
        let found = output
            .lines()
            .filter(|line| candidates.iter().any(|candidate| candidate == line))
            .collect::<Vec<_>>();
        let [selected] = found.as_slice() else {
            return Err(Error::new(
                "NativeGitRefError",
                "Start base is missing or ambiguous; choose an existing complete refs/heads/, refs/tags/ or refs/remotes/ name, or a complete commit SHA",
            ).coded("ROSTER_GIT_FAILED"));
        };
        if !["refs/heads/", "refs/remotes/", "refs/tags/"]
            .iter()
            .any(|prefix| selected.starts_with(prefix))
        {
            return Err(Error::type_error(
                "Start base must be a tracked branch, tag or remote ref",
            ));
        }
        (*selected).to_owned()
    };
    let commit = git
        .text(
            root,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{selected}^{{commit}}"),
            ],
            token,
        )
        .await?;
    if !sha(&commit) || full_sha && !commit.eq_ignore_ascii_case(reference) {
        return Err(Error::type_error(
            "Selected start base does not name the exact complete commit",
        ));
    }
    Ok(commit)
}

impl Start {
    pub async fn require_current(
        &self,
        git: &Git,
        root: &Path,
        token: &CancellationToken,
    ) -> Result<(), Error> {
        let actual = commit_of(
            git,
            root,
            self.reference.as_deref().unwrap_or("HEAD"),
            token,
        )
        .await?;
        if actual != self.commit {
            return Err(Error::new(
                "StaleStart",
                "Selected start ref changed after resolution; preserve existing worktrees and resolve the base again",
            ).coded("ROSTER_STALE_SOURCE"));
        }
        Ok(())
    }
}

pub async fn resolve(
    repository: &Repository,
    git: &Git,
    options: &Options,
    fetch: &FetchPolicy,
    token: &CancellationToken,
) -> Result<Start, Error> {
    crate::config::validate_start_base(&Text::from(options.base.as_str()), "--base")?;
    repository.require_current(git, token).await?;
    let root = &repository.paths.worktree;
    let origin = git
        .optional(root, &["remote", "get-url", "--all", "origin"], token)
        .await?;
    let mut fetched = false;
    let mut notes = Vec::new();
    if origin.is_none() {
        notes.push("no origin remote; freshness unknown".to_owned());
    } else if options.sync == Sync::Offline {
        notes.push("fetch skipped (offline); freshness unknown".to_owned());
    } else {
        let origin = origin
            .as_deref()
            .ok_or_else(|| Error::type_error("Missing origin"))?;
        if origin.contains(['\r', '\n']) {
            return Err(Error::type_error(
                "Approved fetch requires one unambiguous origin URL",
            ));
        }
        match fetch {
            FetchPolicy::Deny => {
                return Err(crate::execution::unsupported(
                    "Fetching requires explicit trusted-host approval; choose --no-fetch for offline work",
                ));
            }
            FetchPolicy::Http => {
                let remote = url::Url::parse(origin).map_err(|_| {
                    Error::type_error(
                        "Approved production fetch supports only credential-free HTTP(S) origins",
                    )
                })?;
                if !matches!(remote.scheme(), "http" | "https")
                    || !remote.username().is_empty()
                    || remote.password().is_some()
                    || remote.query().is_some()
                    || remote.fragment().is_some()
                {
                    return Err(Error::type_error(
                        "Approved production fetch requires a credential-free HTTP(S) origin",
                    ));
                }
                git.run(
                    root,
                    &[
                        "-c",
                        "protocol.http.allow=always",
                        "-c",
                        "protocol.https.allow=always",
                        "fetch",
                        "--prune",
                        "--quiet",
                        "--no-tags",
                        "--no-prune-tags",
                        "--no-recurse-submodules",
                        "--refmap=",
                        "origin",
                        "+refs/heads/*:refs/remotes/origin/*",
                    ],
                    token,
                )
                .await?;
            }
            FetchPolicy::Local { source, head } => {
                if !source.is_absolute() || !sha(head) || !Path::new(origin).is_absolute() {
                    return Err(Error::type_error(
                        "Fixture fetch needs an explicit absolute local origin and complete approved HEAD",
                    ));
                }
                secure::check_components(source)?;
                secure::check_components(Path::new(origin))?;
                if paths::file_information(source)?.identity
                    != paths::file_information(Path::new(origin))?.identity
                    || git
                        .text(
                            source,
                            &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
                            token,
                        )
                        .await?
                        != *head
                {
                    return Err(Error::type_error(
                        "Local fetch source identity or exact approved HEAD changed",
                    ));
                }
                reject_git_execution_config(
                    &git.run(
                        source,
                        &["config", "--local", "--no-includes", "--null", "--list"],
                        token,
                    )
                    .await?,
                )?;
                git.run(
                    root,
                    &[
                        "-c",
                        "protocol.file.allow=always",
                        "fetch",
                        "--prune",
                        "--quiet",
                        "--no-tags",
                        "--no-prune-tags",
                        "--no-recurse-submodules",
                        "--refmap=",
                        "origin",
                        "+refs/heads/*:refs/remotes/origin/*",
                    ],
                    token,
                )
                .await?;
            }
        }
        fetched = true;
    }
    if options.base == "current" {
        return Ok(Start {
            mode: "current",
            reference: None,
            commit: repository.head.clone(),
            fetched,
            notes,
        });
    }
    if options.base != "trunk" {
        return Ok(Start {
            mode: "ref",
            reference: Some(options.base.clone()),
            commit: commit_of(git, root, &options.base, token).await?,
            fetched,
            notes,
        });
    }
    let mut candidates = Vec::new();
    if origin.is_some() {
        if let Some(head) = git
            .optional(
                root,
                &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
                token,
            )
            .await?
        {
            if head.starts_with("refs/remotes/origin/") && !head.contains(['\r', '\n']) {
                candidates.push(head);
            }
        }
        let known = git
            .text(
                root,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/remotes/origin/main",
                    "refs/remotes/origin/master",
                ],
                token,
            )
            .await?;
        for name in ["refs/remotes/origin/main", "refs/remotes/origin/master"] {
            if known.lines().any(|line| line == name)
                && !candidates.iter().any(|value| value == name)
            {
                candidates.push(name.to_owned());
            }
        }
    }
    for candidate in candidates {
        match commit_of(git, root, &candidate, token).await {
            Ok(commit) => {
                return Ok(Start {
                    mode: "trunk",
                    reference: Some(candidate),
                    commit,
                    fetched,
                    notes,
                });
            }
            Err(error) if error.code == Some("ROSTER_GIT_FAILED") => {}
            Err(error) => return Err(error),
        }
    }
    notes.push("origin default branch not found; starting from current HEAD".to_owned());
    Ok(Start {
        mode: "current",
        reference: None,
        commit: repository.head.clone(),
        fetched,
        notes,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drift {
    pub ahead: u64,
    pub behind: u64,
}

pub async fn branch_drift(
    repository: &Repository,
    git: &Git,
    branch: &str,
    start: &Start,
    token: &CancellationToken,
) -> Result<Option<Drift>, Error> {
    crate::config::validate_start_base(&Text::from(branch), "branch")?;
    if start.reference.is_none() {
        return Ok(None);
    }
    let output = git
        .text(
            &repository.paths.worktree,
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("refs/heads/{branch}...{}", start.commit),
            ],
            token,
        )
        .await?;
    let values = output
        .split_whitespace()
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::type_error("Git branch drift must contain two bounded counts"))?;
    match values.as_slice() {
        [ahead, behind] => Ok(Some(Drift {
            ahead: *ahead,
            behind: *behind,
        })),
        _ => Err(Error::type_error(
            "Git branch drift must contain exactly two counts",
        )),
    }
}

#[derive(Debug, Clone)]
struct Module {
    parent: PathBuf,
    path: String,
    source: PathBuf,
    commit: String,
}

async fn modules(
    git: &Git,
    root: &Path,
    commit: &str,
    approvals: &[backend::LocalSubmodule],
    token: &CancellationToken,
) -> Result<Vec<Module>, Error> {
    let mut pending = vec![(
        root.to_path_buf(),
        commit.to_owned(),
        PathBuf::new(),
        String::new(),
        0,
    )];
    let mut selected = Vec::new();
    let mut checkout_bytes = 0_u64;
    while let Some((source, commit, parent, prefix, depth)) = pending.pop() {
        if depth > 4 {
            return Err(Error::type_error(
                "Approved local submodules exceed four levels",
            ));
        }
        let tree = git
            .text(&source, &["ls-tree", "-r", "-l", "-z", &commit], token)
            .await?;
        if tree.split('\0').filter(|entry| !entry.is_empty()).count() > 2048 {
            return Err(Error::type_error(
                "Selected Git base exceeds 2048 tracked entries",
            ));
        }
        for entry in tree.split('\0').filter(|entry| !entry.is_empty()) {
            let (metadata, path) = entry
                .split_once('\t')
                .ok_or_else(|| Error::type_error("Malformed base tree entry"))?;
            let parts = metadata.split_whitespace().collect::<Vec<_>>();
            if !policy::exact_path(path) || parts.len() != 4 {
                return Err(Error::type_error(
                    "Selected Git base contains a non-exact path",
                ));
            }
            if parts.first() == Some(&"120000") {
                return Err(crate::execution::unsupported(
                    "Linked files in the selected local base",
                ));
            }
            if parts.first() != Some(&"160000") {
                let size = parts.get(3).and_then(|size| size.parse::<u64>().ok())
                    .filter(|size| *size <= crate::execution::MAX_FILE_BYTES)
                    .ok_or_else(|| Error::type_error("Selected local base contains a blob exceeding the 16 MiB per-file bound"))?;
                checkout_bytes = checkout_bytes
                    .checked_add(size)
                    .filter(|size| *size <= crate::execution::MAX_SNAPSHOT_BYTES)
                    .ok_or_else(|| {
                        Error::type_error("Selected local checkout and submodules exceed 128 MiB")
                    })?;
            }
            if path == "package.json" {
                let package = git
                    .text(
                        &source,
                        &[
                            "cat-file",
                            "blob",
                            parts
                                .get(2)
                                .copied()
                                .ok_or_else(|| Error::type_error("Missing package object"))?,
                        ],
                        token,
                    )
                    .await?;
                let package = crate::json::parse(&Text::from(package.as_str()))?;
                if crate::domain::is_text(package.get("name"), "github-agent-roster") {
                    return Err(crate::execution::unsupported(
                        "Materializing retired Node Roster production as a local base",
                    ));
                }
            }
            if parts.first() != Some(&"160000") {
                continue;
            }
            let full = format!("{prefix}{path}");
            let approval = approvals.iter().find(|approval| approval.path == full).ok_or_else(|| {
                crate::execution::unsupported("A selected gitlink has no explicit approved local submodule source; no .gitmodules URL was fetched")
            })?;
            if !approval.source.is_absolute() {
                return Err(Error::type_error(
                    "Submodule source must be an absolute approved local directory",
                ));
            }
            secure::check_components(&approval.source)?;
            let expected = parts
                .get(2)
                .copied()
                .filter(|value| sha(value))
                .ok_or_else(|| Error::type_error("Invalid submodule commit"))?;
            let actual = git
                .text(
                    &approval.source,
                    &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
                    token,
                )
                .await?;
            if actual != expected {
                return Err(Error::type_error(
                    "Approved local submodule HEAD must exactly equal the selected gitlink commit",
                ));
            }
            reject_git_execution_config(
                &git.run(
                    &approval.source,
                    &["config", "--local", "--no-includes", "--null", "--list"],
                    token,
                )
                .await?,
            )?;
            let manifest = git
                .text(
                    &source,
                    &[
                        "config",
                        "--blob",
                        &format!("{commit}:.gitmodules"),
                        "--no-includes",
                        "--null",
                        "--get-regexp",
                        r"^submodule\..*\.path$",
                    ],
                    token,
                )
                .await?;
            let names = manifest
                .split('\0')
                .filter_map(|entry| {
                    let (name, value) = entry.split_once('\n')?;
                    (value == path)
                        .then(|| name.strip_prefix("submodule.")?.strip_suffix(".path"))
                        .flatten()
                })
                .collect::<Vec<_>>();
            let [name] = names.as_slice() else {
                return Err(Error::type_error(
                    "Selected submodule needs one unambiguous declared .gitmodules path",
                ));
            };
            if name.is_empty()
                || !name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/')
                })
            {
                return Err(Error::type_error(
                    "Submodule name is not a bounded declarative identifier",
                ));
            }
            selected.push(Module {
                parent: parent.clone(),
                path: path.to_owned(),
                source: approval.source.clone(),
                commit: actual.clone(),
            });
            pending.push((
                approval.source.clone(),
                actual,
                parent.join(path),
                format!("{full}/"),
                depth + 1,
            ));
        }
    }
    if selected.len() != approvals.len() || selected.len() > 16 {
        return Err(Error::type_error(
            "Submodule approvals must exactly cover the selected local gitlinks",
        ));
    }
    Ok(selected)
}

pub struct Prepared {
    pub worktree: PathBuf,
    pub task: String,
    pub session: String,
    pub branch: String,
    pub start: Start,
}

#[derive(Default)]
pub struct Preparation {
    lease: Option<backend::RootLease>,
    root: Option<PathBuf>,
    acquiring: bool,
}

impl Preparation {
    pub fn root(&self) -> Option<&Path> {
        self.lease
            .as_ref()
            .map(backend::RootLease::root)
            .or(self.root.as_deref())
    }

    pub fn lease(&self) -> Result<&backend::RootLease, Error> {
        self.lease.as_ref().ok_or_else(|| {
            Error::new(
                "RootOwnershipError",
                "Preparation has no admitted root lease",
            )
        })
    }

    pub(crate) fn lease_slot(&mut self) -> Result<&mut Option<backend::RootLease>, Error> {
        if self.acquiring {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Root admission remains unverified",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        Ok(&mut self.lease)
    }

    pub async fn acquire(
        &mut self,
        root: &Path,
        session: &str,
        host: tools::Host,
        redaction: redaction::Config,
        token: CancellationToken,
    ) -> Result<(), Error> {
        if self.root().is_some() {
            return Err(Error::type_error(
                "Caller already retains a preparation root",
            ));
        }
        self.root = Some(paths::absolute(root)?);
        self.acquiring = true;
        match backend::RootLease::open(root, session, host, redaction, token).await {
            Ok(lease) => {
                self.lease = Some(lease);
                self.acquiring = false;
                Ok(())
            }
            Err(error) => {
                if !matches!(
                    error.code,
                    Some("ROSTER_PROCESS_CLEANUP" | "E_LOCK_RELEASE" | "E_LOCK_LOST")
                ) {
                    self.acquiring = false;
                }
                Err(error)
            }
        }
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        if self.acquiring {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Preparation admission was interrupted; retain the root for explicit recovery",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        if let Some(lease) = &mut self.lease {
            lease.close().await?;
            self.lease = None;
        }
        self.root = None;
        Ok(())
    }
}

struct PendingMaterialization<'a> {
    lease: &'a backend::RootLease,
    git: &'a Git,
    settled: bool,
}

impl Drop for PendingMaterialization<'_> {
    fn drop(&mut self) {
        if !self.settled || !self.git.cleanup_certain() {
            backend::Owner::PrePlan(self.lease).quarantine();
        }
    }
}

pub struct Contracts {
    root: PathBuf,
    identity: FileIdentity,
    commit: String,
    publisher: secure::Snapshot,
}

impl Contracts {
    pub async fn open(
        installation: &Path,
        root: &Path,
        git: &Git,
        token: &CancellationToken,
    ) -> Result<Self, Error> {
        secure::check_components(root)?;
        let pin = git
            .text(
                installation,
                &["ls-tree", "HEAD", "--", "vendor/github-agent-contracts"],
                token,
            )
            .await?;
        let parts = pin.split_whitespace().collect::<Vec<_>>();
        let ["160000", "commit", commit, "vendor/github-agent-contracts"] = parts.as_slice() else {
            return Err(crate::execution::unsupported(
                "The installation has no reviewed contracts gitlink pin; use a complete native development installation",
            ));
        };
        if !sha(commit) {
            return Err(Error::type_error(
                "Reviewed contracts gitlink is not a complete commit",
            ));
        }
        let publisher = secure::read(&root.join("scripts").join("agent-pr.mjs"), 512 * 1024)?
            .ok_or_else(|| Error::type_error("Required contracts publisher marker disappeared"))?;
        let seal = Self {
            root: paths::absolute(root)?,
            identity: paths::file_information(root)?.identity,
            commit: (*commit).to_owned(),
            publisher,
        };
        seal.require_current(git, token).await?;
        Ok(seal)
    }

    pub async fn require_current(&self, git: &Git, token: &CancellationToken) -> Result<(), Error> {
        secure::check_components(&self.root)?;
        if paths::file_information(&self.root)?.identity != self.identity
            || !secure::unchanged(
                &self.root.join("scripts").join("agent-pr.mjs"),
                &self.publisher,
                512 * 1024,
            )?
            || paths::file_information(&self.root.join("scripts").join("agent-pr.mjs"))?.links != 1
        {
            return Err(Error::type_error(
                "Required contracts dependency identity changed; no source was adopted",
            ));
        }
        reject_git_execution_config(
            &git.run(
                &self.root,
                &["config", "--local", "--no-includes", "--null", "--list"],
                token,
            )
            .await?,
        )?;
        let worktree = git
            .text(&self.root, &["rev-parse", "--show-toplevel"], token)
            .await?;
        if paths::file_information(Path::new(&worktree))?.identity != self.identity {
            return Err(Error::type_error(
                "Contracts Git worktree ownership differs from the selected dependency root",
            ));
        }
        let actual = git
            .text(
                &self.root,
                &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
                token,
            )
            .await?;
        if actual != self.commit
            || !git
                .run(
                    &self.root,
                    &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                    token,
                )
                .await?
                .is_empty()
        {
            return Err(Error::type_error(
                "Contracts dependency must be clean at the installation's exact reviewed gitlink; initialize the pinned submodule explicitly, without editing vendor sources",
            ));
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_local(
    repository: &Repository,
    git: &Git,
    config: &Config,
    start: Start,
    approval: &backend::Approval,
    redaction: redaction::Config,
    token: CancellationToken,
    preparation: &mut Preparation,
) -> Result<Prepared, Error> {
    prepare_task(
        repository,
        git,
        config,
        start,
        approval,
        redaction,
        token,
        preparation,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_issue(
    repository: &Repository,
    git: &Git,
    config: &Config,
    start: Start,
    approval: &backend::Approval,
    redaction: redaction::Config,
    token: CancellationToken,
    preparation: &mut Preparation,
    number: u64,
) -> Result<Prepared, Error> {
    issue_branch(number, None)?;
    prepare_task(
        repository,
        git,
        config,
        start,
        approval,
        redaction,
        token,
        preparation,
        Some(number),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn prepare_task(
    repository: &Repository,
    git: &Git,
    config: &Config,
    start: Start,
    approval: &backend::Approval,
    redaction: redaction::Config,
    token: CancellationToken,
    preparation: &mut Preparation,
    issue: Option<u64>,
) -> Result<Prepared, Error> {
    token.check()?;
    approval.validate()?;
    if preparation.root().is_some() {
        return Err(Error::type_error(
            "Local preparation requires an empty caller-owned root slot",
        ));
    }
    repository.require_current(git, &token).await?;
    start
        .require_current(git, &repository.paths.worktree, &token)
        .await?;
    let modules = modules(
        git,
        &repository.paths.worktree,
        &start.commit,
        &approval.submodules,
        &token,
    )
    .await?;
    let (task, session, branch) = match issue {
        Some(number) => (
            format!("issue-{number}"),
            format!("roster-{number}-coder"),
            issue_branch(number, None)?,
        ),
        None => {
            let task = format!("local-{}", files::nonce()?);
            let session = format!("roster-{}", files::nonce()?);
            let branch = format!("agents/{task}");
            (task, session, branch)
        }
    };
    let worktrees = config
        .as_value()
        .get("paths")
        .and_then(|value| value.get("worktrees"))
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Local preparation needs paths.worktrees"))?
        .to_utf8()?;
    let worktree = repository.paths.repository.join(&worktrees).join(&task);
    paths::ensure_local_path(&worktree, &repository.paths.repository)?;
    if worktree.exists() {
        return Err(Error::type_error(
            "New local worktree path already exists; unknown ownership is preserved",
        ));
    }
    git.run(&repository.paths.worktree, &["check-ignore", "--no-index", "--quiet", "--", &format!("{worktrees}/{task}/")], &token).await
        .map_err(|error| Error::new("WorktreeOwnershipError", "Managed worktree paths must already be Git-ignored; update ignore rules as a human before local preparation").caused_by(error))?;
    let namespace = repository.paths.common_dir.join("roster-local");
    token.check()?;
    repository.require_indexes_current()?;
    secure::private_directory(&namespace, true)?;
    let target = namespace.join("prepare.lock");
    let mut lock = locks::acquire(
        "prepare",
        &target,
        &locks::Options {
            holder: session.clone(),
            run_id: Some(task.clone()),
            ..locks::Options::default()
        },
        Some(&token),
    )
    .await?;
    let result = async {
        lock.check()?;
        repository.require_current(git, &token).await?;
        start.require_current(git, &repository.paths.worktree, &token).await?;
        let mut refs = vec![
            "for-each-ref".to_owned(),
            "--format=%(refname)".to_owned(),
        ];
        if let Some(number) = issue {
            for prefix in ["refs/heads/", "refs/remotes/origin/"] {
                refs.push(format!("{prefix}agents/{number}*"));
                refs.push(format!("{prefix}issue-{number}*"));
            }
        } else {
            refs.push(format!("refs/heads/{branch}"));
        }
        let args = refs.iter().map(String::as_str).collect::<Vec<_>>();
        let branches = git.text(&repository.paths.worktree, &args, &token).await?;
        let claimed = issue.map_or(!branches.is_empty(), |number| {
            branches.lines().any(|name| {
                name.strip_prefix("refs/heads/")
                    .or_else(|| name.strip_prefix("refs/remotes/origin/"))
                    .and_then(parse_issue_branch)
                    .is_some_and(|claim| claim.number == number)
            })
        });
        if claimed || worktree.exists() {
            return Err(Error::type_error("Local branch/path ownership collision; nothing was adopted or replaced"));
        }
        let record = Value::object(vec![
            ("schema_version", Value::Number(1.0)),
            ("task", Value::string(&task)),
            ("session", Value::string(&session)),
            ("branch", Value::string(&branch)),
            ("base", Value::string(&start.commit)),
            ("worktree", Value::string(worktree.to_str().ok_or_else(|| Error::type_error("Worktree path must be Unicode"))?)),
            ("publication", Value::string("unknown")),
        ]);
        token.check()?;
        repository.require_indexes_current()?;
        secure::write_exclusive(&namespace.join(format!("{task}.json")), record.json().as_bytes())?;
        secure::private_directory(worktree.parent().ok_or_else(|| Error::type_error("Worktree needs a parent"))?, true)?;
        git.run(&repository.paths.worktree, &["worktree", "add", "--quiet", "-b", &branch, worktree.to_str().ok_or_else(|| Error::type_error("Worktree path must be Unicode"))?, &start.commit], &token).await?;
        let actual = Repository::inspect(&worktree, git, &token).await?;
        if actual.common != repository.common || actual.head != start.commit || actual.branch.as_deref() != Some(branch.as_str()) || actual.dirty {
            return Err(Error::type_error("New worktree identity, branch, base or cleanliness differs from its prepared claim"));
        }
        preparation.acquire(&worktree, &session, git.host.clone(), redaction.clone(), token.clone()).await?;
        let lease = preparation.lease()?;
        let mut pending = PendingMaterialization { lease, git, settled: false };
        let initialized = async {
            for module in &modules {
                let directory = worktree.join(&module.parent);
                let destination = directory.join(&module.path);
                git.run(&directory, &[
                    "-c", "protocol.file.allow=always",
                    "clone", "--quiet", "--no-checkout",                     "--shared", "--",
                    module.source.to_str().ok_or_else(|| Error::type_error("Approved submodule source must be Unicode"))?,
                    destination.to_str().ok_or_else(|| Error::type_error("Approved submodule destination must be Unicode"))?,
                ], &token).await?;
                git.run(&destination, &["checkout", "--quiet", "--detach", &module.commit], &token).await?;
                if git.text(&directory.join(&module.path), &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"], &token).await? != module.commit {
                    return Err(Error::type_error("Initialized local submodule differs from its exact approved gitlink"));
                }
                lease.check()?;
            }
            repository.require_current(git, &token).await?;
            start.require_current(git, &repository.paths.worktree, &token).await?;
            lease.check()
        }.await;
        pending.settled = true;
        drop(pending);
        initialized?;
        Ok(Prepared { worktree: worktree.clone(), task: task.clone(), session: session.clone(), branch, start })
    }.await;
    if !git.cleanup_certain() {
        return Err(Error::new(
            "OwnedProcessCleanupError",
            format!(
                "Preparation cleanup is unverified; preserve the claim, lock and worktree at {}",
                worktree.display()
            ),
        )
        .coded("ROSTER_PROCESS_CLEANUP"));
    }
    locks::finish(result, lock.release_required().await).map_err(|error| {
        Error::new(
            error.name,
            format!(
                "{}; local worktree location: {}",
                error.message,
                worktree.display()
            ),
        )
        .coded(error.code.unwrap_or("ROSTER_LOCAL_PREPARATION"))
        .caused_by(error)
    })
}

pub fn plan_green_checkpoint_resume(
    repo_root: &Path,
    issue_number: u64,
    green: bool,
    yes: bool,
) -> Result<Value, Error> {
    if issue_number == 0 || issue_number > crate::MAX_SAFE_INTEGER || !green || !yes {
        return Err(Error::type_error(
            "Use roster resume N --green --yes; explicit --yes authorizes product restore, not execution.",
        ));
    }
    let worktree = repo_root
        .join(".worktrees")
        .join(format!("issue-{issue_number}"));
    let info = inspect_local_issue_worktree(issue_number, &worktree)?;
    if info.get("planMode") == Some(&Value::Boolean(true)) {
        return Err(Error::new(
            "Error",
            "Green resume requires an executable TASK.md",
        ));
    }
    let task_path = worktree.join("TASK.md");
    if secure::read(&task_path, 65_536)?.is_none() {
        return Err(Error::new(
            "Error",
            "Green resume requires an executable TASK.md",
        ));
    }
    let task_id = format!("issue-{issue_number}");
    let mut checkpoints = Vec::new();
    for dir_name in [issue_number.to_string(), task_id.clone()] {
        let cp_dir = worktree.join(".roster").join("checkpoints").join(&dir_name);
        if cp_dir.is_dir() {
            for index in 1_u64..=1000 {
                let file = cp_dir.join(index.to_string());
                let Some(snap) = secure::read(&file, 65_536)? else {
                    break;
                };
                let text = String::from_utf8(snap.bytes)
                    .map_err(|e| Error::new("Error", e.to_string()))?;
                checkpoints.push(crate::json::parse(&Text::from(text.trim()))?);
            }
            if !checkpoints.is_empty() {
                break;
            }
        }
    }
    if checkpoints.is_empty() {
        let hashed_dir = worktree
            .join(".roster")
            .join("checkpoints")
            .join(&crate::identity::sha256_hex(task_id.as_bytes())[..16]);
        let records_file = hashed_dir.join("records.jsonl");
        if let Some(snap) = secure::read(&records_file, 65_536)? {
            let text =
                String::from_utf8(snap.bytes).map_err(|e| Error::new("Error", e.to_string()))?;
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                checkpoints.push(crate::json::parse(&Text::from(line))?);
            }
        }
    }
    let verified = checkpoints
        .iter()
        .rev()
        .find(|cp| crate::domain::is_text(cp.get("status"), "verified"))
        .ok_or_else(|| {
            Error::new(
                "Error",
                "No verified checkpoint is recorded for this issue worktree",
            )
        })?;
    let cp_number = crate::domain::safe_integer(verified.get("number"))
        .ok_or_else(|| Error::type_error("Verified checkpoint is missing its number"))?;
    let msg = format!(
        "Restored verified checkpoint {cp_number}; later edits preserved in a new checkpoint. Fresh checks/review required.\n"
    );
    Ok(Value::object(vec![
        ("issue", Value::Number(issue_number as f64)),
        ("checkpointNumber", Value::Number(cp_number as f64)),
        ("worktree", Value::string(&worktree.to_string_lossy())),
        ("message", Value::string(&msg)),
        ("pausedBeforeExecution", Value::Boolean(true)),
    ]))
}

pub fn orchestrate_wave_attempts(
    plan: &Value,
    linked_issues: &[Value],
    active_issues: &BTreeSet<u64>,
    parallel: Option<&Value>,
    concurrency: Option<&Value>,
    attempts_per_issue: Option<&Value>,
    completed_attempts: &std::collections::BTreeMap<u64, Vec<Value>>,
) -> Result<Value, Error> {
    let rows = compute_wave_board_rows(plan, linked_issues, None, None)?;
    let (batch_limit, ready_batch) =
        select_ready_wave_batch(&rows, active_issues, parallel, concurrency)?;
    let attempts_k = attempt_limit(attempts_per_issue, None)?;
    let mut winners = Vec::new();
    for (&issue_num, candidates) in completed_attempts {
        let selected = select_attempt(candidates).unwrap_or(Value::Null);
        winners.push(Value::object(vec![
            ("issue", Value::Number(issue_num as f64)),
            ("winner", selected),
        ]));
    }
    Ok(Value::object(vec![
        ("batchLimit", Value::Number(batch_limit as f64)),
        ("attemptsPerIssue", Value::Number(attempts_k as f64)),
        ("readyBatch", Value::Array(ready_batch)),
        ("boardRows", Value::Array(rows)),
        ("selectedWinners", Value::Array(winners)),
    ]))
}

#[cfg(test)]
mod repository_path_tests {
    use super::*;

    #[test]
    fn repository_path_frames_preserve_legacy_values_and_record_trimming() -> Result<(), Error> {
        for expected in [
            ["D:/owned main", ".git", ".git"],
            [
                "/owned 'quotes' \"double\" révision 東京",
                "../common/.git",
                "../common/.git/worktrees/linked",
            ],
            [
                "  /owned/edge  ",
                "relative\rinside",
                "/owned/\u{a0}edge\u{2003}",
            ],
        ] {
            let [root, common, git_dir] = expected;
            for ending in ["\n", "\r\n", "\r\r\n"] {
                let raw = format!("{root}{ending}{common}{ending}{git_dir}{ending}");
                assert_eq!(parse_repository_paths(raw.as_bytes())?, expected);
            }
        }
        Ok(())
    }

    #[test]
    fn repository_path_frames_refuse_missing_extra_empty_and_unterminated_records() {
        for raw in [
            "",
            "root\n",
            "root\ncommon\n",
            "root\ncommon\ndir",
            "root\ncommon\ndir\nextra\n",
            "root\ncommon\ndir\n\n",
            "\ncommon\ndir\n",
            "root\n\ndir\n",
            "root\ncommon\n\n",
            "root\ncommon\n\r\r\n",
            "root\ncommon\0extra\ndir\n",
        ] {
            assert!(parse_repository_paths(raw.as_bytes()).is_err());
        }
    }

    #[test]
    fn repository_path_frames_refuse_invalid_utf8_and_enforce_exact_byte_bound() -> Result<(), Error>
    {
        for raw in [
            b"\xff\ncommon\ndir\n".as_slice(),
            b"root\n\xff\ndir\n".as_slice(),
            b"root\ncommon\n\xff\n".as_slice(),
        ] {
            assert!(parse_repository_paths(raw).is_err());
        }
        let bound = usize::try_from(crate::execution::MAX_FILE_BYTES)
            .map_err(|_| Error::type_error("Repository path bound must fit usize"))?;
        let length = bound
            .checked_sub(5)
            .ok_or_else(|| Error::type_error("Repository path bound must fit framing"))?;
        let mut raw = vec![b'x'; length];
        raw.extend_from_slice(b"\nc\nd\n");
        let [root, common, git_dir] = parse_repository_paths(&raw)?;
        assert_eq!(root.len(), length);
        assert_eq!(common, "c");
        assert_eq!(git_dir, "d");
        raw.push(b'\n');
        assert!(parse_repository_paths(&raw).is_err());
        Ok(())
    }

    #[test]
    fn repository_path_compatibility_is_selected_from_expected_delimiters_only() {
        let plain = RepositoryPaths {
            worktree: PathBuf::from("owned 'quotes' \"double\" révision 東京"),
            common_dir: PathBuf::from("common\rinside"),
            git_dir: PathBuf::from("relative/.git"),
            repository: PathBuf::from("repository"),
        };
        assert!(!repository_paths_need_legacy(&plain));
        for field in 0..3 {
            let mut paths = plain.clone();
            match field {
                0 => paths.worktree.push("line\ninside"),
                1 => paths.common_dir.push("line\ninside"),
                _ => paths.git_dir.push("line\ninside"),
            }
            assert!(repository_paths_need_legacy(&paths));
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut paths = plain.clone();
            paths.common_dir = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff, b'\n']));
            assert!(repository_paths_need_legacy(&paths));
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let mut paths = plain;
            paths.common_dir =
                PathBuf::from(std::ffi::OsString::from_wide(&[0xd800, u16::from(b'\n')]));
            assert!(repository_paths_need_legacy(&paths));
        }
    }

    #[test]
    fn stranded_work_classification_and_wave_labels_preserve_legacy_contracts() -> Result<(), Error>
    {
        use crate::json::parse;

        let now = 1_791_144_000_000.0;
        let assigned = |number: u64, state: &str| {
            Value::object(vec![
                ("number", Value::Number(number as f64)),
                ("state", Value::string(state)),
                (
                    "assignees",
                    Value::Array(vec![Value::object(vec![(
                        "login",
                        Value::string("worker"),
                    )])]),
                ),
                (
                    "labels",
                    Value::Array(vec![Value::object(vec![(
                        "name",
                        Value::string("in-progress"),
                    )])]),
                ),
            ])
        };
        let mut unassigned = assigned(5, "OPEN");
        crate::domain::put(&mut unassigned, "assignees", Value::Array(Vec::new()))?;
        let issues = vec![
            assigned(1, "OPEN"),
            assigned(2, "OPEN"),
            assigned(3, "OPEN"),
            assigned(4, "CLOSED"),
            unassigned,
            assigned(6, "open"),
        ];
        let input = Value::object(vec![
            ("issues", Value::Array(issues)),
            (
                "branches",
                Value::Array(vec![Value::string("issue-2"), Value::string("issue-3")]),
            ),
            (
                "worktrees",
                Value::Array(vec![Value::object(vec![
                    ("branch", Value::string("issue-6")),
                    ("path", Value::string("unused")),
                ])]),
            ),
            (
                "heartbeats",
                Value::Array(vec![
                    Value::object(vec![
                        ("issue", Value::Number(3.0)),
                        ("timestamp", Value::Number(now)),
                    ]),
                    Value::object(vec![
                        ("issue", Value::Number(6.0)),
                        ("timestamp", Value::Number(now - 1000.0)),
                    ]),
                ]),
            ),
            ("now", Value::Number(now)),
            ("thresholdMs", Value::Number(1000.0)),
        ]);
        let results = classify_stranded_work(&input)?;
        let statuses = results
            .iter()
            .map(|entry| {
                (
                    entry
                        .get("branch")
                        .and_then(Value::as_string)
                        .and_then(|s| s.to_utf8().ok()),
                    entry
                        .get("status")
                        .and_then(Value::as_string)
                        .and_then(|s| s.to_utf8().ok()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec![
                (Some("agents/1".to_owned()), Some("stranded".to_owned())),
                (Some("issue-2".to_owned()), Some("stale".to_owned())),
                (Some("issue-3".to_owned()), Some("healthy".to_owned())),
                (Some("issue-6".to_owned()), Some("healthy".to_owned())),
            ]
        );
        assert!(
            select_issue_branch(42, &["agents/42".to_owned(), "issue-42".to_owned()], None)
                .is_err()
        );
        assert_eq!(
            issue_wave(&parse(&Text::from(
                r#"{"labels":[{"name":"wave:2"}],"body":"<!-- Roster-Wave: 2 -->"}"#
            ))?)?,
            Some(2)
        );
        assert!(
            issue_wave(&parse(&Text::from(
                r#"{"labels":["wave:1"],"body":"<!-- Roster-Wave: 2 -->"}"#
            ))?)
            .is_err()
        );
        assert!(issue_wave(&parse(&Text::from(r#"{"labels":null}"#))?).is_err());
        assert!(
            issue_wave(&parse(&Text::from(
                r#"{"labels":["wave:1"],"body":"Syntax: <!-- Roster-Wave: N -->\n<!-- Roster-Wave: 2 -->"}"#
            ))?)
            .is_err()
        );
        assert_eq!(
            issue_branches(0, None).err().map(|err| err.message),
            Some("Issue number must be a positive safe integer".to_owned())
        );
        assert_eq!(
            select_issue_branch(42, &[], Some(17))
                .err()
                .map(|err| err.message),
            Some("Attempt index must be between 1 and 16".to_owned())
        );
        let offset_input = parse(&Text::from(
            r#"{"issues":[{"number":1,"state":"OPEN","assignees":[{"login":"worker"}]}],"branches":["agents/1"],"heartbeats":[{"issue":1,"timestamp":"2026-10-04T16:00:00-04:00"}],"now":"2026-10-04T20:00:00+00:00"}"#,
        ))?;
        let offset_results = classify_stranded_work(&offset_input)?;
        assert_eq!(
            offset_results[0].get("status"),
            Some(&Value::string("healthy"))
        );
        assert_eq!(
            issue_dependencies(&parse(&Text::from(
                r#"{"number":5,"body":"Slice\n\nDepends on: #1, #2\n"}"#
            ))?)?,
            Some(vec![1, 2])
        );
        assert_eq!(
            issue_dependencies(&parse(&Text::from(
                r#"{"number":5,"body":"Slice\n\nDepends on: none\n"}"#
            ))?)?,
            Some(Vec::new())
        );
        assert!(
            issue_dependencies(&parse(&Text::from(
                r#"{"number":5,"body":"Depends on: #5"}"#
            ))?)
            .is_err()
        );
        assert_eq!(parallel_limit(Some(&Value::string("4")))?, 4);
        assert!(parallel_limit(Some(&Value::Number(0.0))).is_err());
        let summary = format_issue_summary(
            &parse(&Text::from(
                r#"{"issue":{"number":42,"title":"Add status","state":"OPEN"},"openPr":null}"#,
            ))?,
            Some(&Value::object(Vec::new())),
        )?;
        assert_eq!(
            summary,
            "Issue: #42 Add status\nState: OPEN\nBranch: agents/42\nPR: (none)\n"
        );
        let unterminated = format_issue_summary(
            &parse(&Text::from(
                r#"{"issue":{"number":42,"title":"Fix \u001b]0;unterminated","state":"OPEN"},"openPr":null}"#,
            ))?,
            Some(&Value::object(Vec::new())),
        )?;
        assert!(unterminated.contains("Issue: #42 Fix ?]0;unterminated\n"));

        assert_eq!(attempt_limit(None, None)?, 1);
        assert_eq!(attempt_limit(Some(&Value::Number(3.0)), None)?, 3);
        assert!(attempt_limit(Some(&Value::Number(4.0)), None).is_err());
        assert!(attempt_limit(Some(&Value::Number(2.0)), Some(&Value::Number(17.0))).is_err());
        let gates = attempt_gates(Some(&parse(&Text::from(
            r#"{"tests":{"exit_code":0},"excellence":{"pass":true}}"#,
        ))?));
        assert_eq!(gates.get("tests"), Some(&Value::string("pass")));
        assert_eq!(gates.get("excellence"), Some(&Value::string("pass")));
        assert_eq!(gates.get("red_green"), Some(&Value::string("skipped")));
        let candidate = |index: u64, lines: u64, duration: u64, review: &str| {
            Value::object(vec![
                (
                    "completed",
                    Value::object(vec![(
                        "result",
                        Value::object(vec![("mode", Value::string("llm"))]),
                    )]),
                ),
                (
                    "evidence",
                    Value::object(vec![
                        ("index", Value::Number(index as f64)),
                        ("changed_lines", Value::Number(lines as f64)),
                        ("duration_ms", Value::Number(duration as f64)),
                        ("gates", gates.clone()),
                        ("review", Value::string(review)),
                    ]),
                ),
            ])
        };
        let winner = select_attempt(&[
            candidate(4, 1, 1, "fail"),
            candidate(2, 4, 50, "pass"),
            candidate(3, 4, 20, "pass"),
            candidate(1, 4, 20, "pass"),
        ])
        .ok_or_else(|| Error::new("TestError", "expected winner"))?;
        assert_eq!(
            winner.get("evidence").and_then(|e| e.get("index")),
            Some(&Value::Number(1.0))
        );

        let wt1 = Worktree {
            path: PathBuf::from("/tmp/w1"),
            head: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            branch: Some("agents/42".to_owned()),
            locked: false,
            prunable: false,
        };
        let wt2 = Worktree {
            path: PathBuf::from("/tmp/w2"),
            head: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            branch: Some("issue-42".to_owned()),
            locked: false,
            prunable: false,
        };
        assert!(assert_isolated_issue_branches(std::slice::from_ref(&wt1)).is_ok());
        assert!(assert_isolated_issue_branches(&[wt1, wt2]).is_err());

        assert!(provenance_opt_out(Some(&Value::object(vec![(
            "ROSTER_PROVENANCE_OPT_OUT",
            Value::string("true"),
        )]))));
        assert!(provenance_opt_out(Some(&Value::object(vec![
            ("ROSTER_PROVENANCE_OPT_OUT", Value::Null),
            ("ROSTER_PROVENANCE_DISABLED", Value::string("1")),
        ]))));
        assert!(!provenance_opt_out(Some(&Value::object(vec![(
            "ROSTER_PROVENANCE_OPT_OUT",
            Value::string("0"),
        )]))));

        let recap_dir =
            std::env::temp_dir().join(format!("roster-recap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&recap_dir);
        std::fs::create_dir_all(&recap_dir).map_err(|e| Error::new("TestError", e.to_string()))?;
        let stub = crate::planning::artifacts::stub(
            &Value::string("Update README.md."),
            "issue:42",
            None,
            None,
        )?;
        std::fs::write(recap_dir.join("TASK.md"), stub.task.to_utf8()?)
            .map_err(|e| Error::new("TestError", e.to_string()))?;
        std::fs::write(recap_dir.join("REVIEW.md"), "# Review\r\nVerdict: pass\r\n")
            .map_err(|e| Error::new("TestError", e.to_string()))?;
        std::fs::write(
            recap_dir.join("RESULT.md"),
            "# Result\r\n- node --test exited 0\r\n",
        )
        .map_err(|e| Error::new("TestError", e.to_string()))?;
        let recap = format_run_recap(
            &recap_dir,
            Some("Fallback title"),
            Some("stop"),
            Some(&Value::object(Vec::new())),
        )?;
        let _ = std::fs::remove_dir_all(&recap_dir);
        assert!(recap.contains("Files: README.md"));
        assert!(recap.contains("Last test: node --test (exit 0)"));
        assert!(recap.contains("Review: pass"));
        assert!(recap.contains("Finish reason: stop"));
        let open_issues = parse_open_issues_json(r#"[{"number":42,"title":"Add status"}]"#)?;
        assert_eq!(open_issues.len(), 1);
        assert!(
            parse_open_issues_json(r#"[{"number":42,"title":"A"},{"number":42,"title":"B"}]"#)
                .is_err()
        );
        assert_eq!(
            parse_diff_names("README.md\0src/app.rs\0"),
            vec!["README.md".to_owned(), "src/app.rs".to_owned()]
        );
        let wave_plan_val = crate::json::parse(&Text::from(
            r#"{"issues":[{"wave":1,"title":"Slice 1"},{"wave":2,"title":"Slice 2"}]}"#,
        ))?;
        let linked = vec![
            crate::json::parse(&Text::from(
                r#"{"number":10,"state":"CLOSED","body":"<!-- Roster-Draft: 1 -->","labels":[{"name":"wave:1"}]}"#,
            ))?,
            crate::json::parse(&Text::from(
                r#"{"number":11,"state":"OPEN","body":"Depends on: #10\n<!-- Roster-Draft: 2 -->","labels":[{"name":"wave:2"}]}"#,
            ))?,
        ];
        let board_rows = compute_wave_board_rows(&wave_plan_val, &linked, None, None)?;
        assert_eq!(board_rows.len(), 2);
        assert_eq!(board_rows[0].get("state"), Some(&Value::string("done")));
        assert_eq!(board_rows[1].get("state"), Some(&Value::string("todo")));
        let (limit, ready) = select_ready_wave_batch(
            &board_rows,
            &BTreeSet::new(),
            Some(&Value::Number(2.0)),
            Some(&Value::Number(1.0)),
        )?;
        assert_eq!(limit, 1);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].get("issue"), Some(&Value::Number(11.0)));
        Ok(())
    }
}
