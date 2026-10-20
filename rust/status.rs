use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::cancel::CancellationToken;
use crate::execution::tools;
use crate::json::{self, Value};
use crate::state::{layout, redaction, secure};
use crate::text::Text;
use crate::{Error, domain, host, paths, start};

const MAX_LOG_BYTES: usize = 2 * 1024 * 1024;

fn error(message: &str) -> Error {
    Error::new("StatusError", message).coded("ROSTER_STATUS_INPUT")
}

#[derive(Debug, Clone)]
pub struct Options {
    pub issue: u64,
    pub offline: bool,
    pub json: bool,
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self, Error> {
        let usage = || {
            error(
                "Use status --issue N [--offline] [--json]. Online status requires GitHub App authentication.",
            )
        };
        let mut issue = None;
        let mut offline = false;
        let mut json = false;
        let mut seen = std::collections::BTreeSet::new();
        let mut position = 0;
        while let Some(flag) = args.get(position) {
            if !seen.insert(flag) {
                return Err(usage());
            }
            match flag.as_str() {
                "--offline" => offline = true,
                "--json" => json = true,
                "--issue" => {
                    position += 1;
                    let value = args.get(position).ok_or_else(usage)?;
                    issue = Some(start::parse_issue_number(value).map_err(|_| usage())?);
                }
                _ => return Err(usage()),
            }
            position += 1;
        }
        Ok(Self {
            issue: issue.ok_or_else(usage)?,
            offline,
            json,
        })
    }
}

pub struct Snapshot {
    value: Value,
}

impl Snapshot {
    pub fn as_value(&self) -> &Value {
        &self.value
    }

    pub fn format(&self, json: bool) -> Result<String, Error> {
        if json {
            return Ok(format!("{}\n", self.value.pretty()));
        }
        let display = |field| -> Result<String, Error> {
            self.value
                .get(field)
                .filter(|value| **value != Value::Null)
                .map_or_else(|| Ok(Text::from("unknown")), domain::string)
                .and_then(|value| value.to_utf8())
        };
        let mut output = format!(
            "Issue: {}\nLocal log: {}\nLast seat: {}\nLast complete event: {}\nObserved state: {}\nEvidence: local metadata only; worker liveness, verification, publication and acceptance remain unknown.\n",
            display("issue")?,
            display("log")?,
            display("seat")?,
            display("event")?,
            display("state")?
        );
        if let Some(issue) = self.value.get("github_issue") {
            let field = |name| -> Result<String, Error> {
                domain::string(
                    issue
                        .get(name)
                        .ok_or_else(|| error("GitHub observation omitted a metadata field"))?,
                )?
                .to_utf8()
            };
            output.push_str(&format!(
                "GitHub issue: #{} {} ({}) {}\nGitHub App: {}\nOpen PR: unknown (not queried)\n",
                field("number")?,
                field("title")?,
                field("state")?,
                field("url")?,
                field("app")?
            ));
        }
        Ok(output)
    }
}

fn word(value: Option<&Value>) -> Result<Text, Error> {
    value
        .and_then(Value::as_string)
        .filter(|value| {
            (1..=64).contains(&value.0.len())
                && value
                    .0
                    .iter()
                    .all(|unit| matches!(*unit, 45 | 46 | 48..=57 | 65..=90 | 95 | 97..=122))
        })
        .cloned()
        .ok_or_else(|| error("Session log metadata labels are invalid"))
}

fn log_path(root: &Path, issue: u64) -> Result<PathBuf, Error> {
    let directory = layout::resolve_directory(
        layout::Scope::PerWorktree,
        root,
        Some(root),
        Some(".roster"),
    )?;
    Ok(layout::state_path(directory, &["runs", &format!("roster-{issue}-coder.log")])?.path)
}

fn read_log(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    secure::check_components(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(error("The selected session log could not be inspected")),
    };
    if !metadata.is_file() || secure::linked(&metadata) {
        return Err(error("Session status requires a real regular log file"));
    }
    let information = paths::file_information(path)?;
    if information.links != 1 {
        return Err(error("Session status refuses a hard-linked log"));
    }
    let mut file =
        fs::File::open(path).map_err(|_| error("The selected session log could not be read"))?;
    if paths::handle_information(&file)?.identity != information.identity {
        return Err(error("The selected session log changed while opening"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take((MAX_LOG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| error("The selected session log could not be read"))?;
    if bytes.len() > MAX_LOG_BYTES {
        return Err(error("Session log exceeds 2 MiB"));
    }
    secure::check_components(path)?;
    let current = paths::file_information(path)?;
    if current.identity != information.identity || current.links != 1 {
        return Err(error("The selected session log was replaced while reading"));
    }
    Ok(Some(bytes))
}

fn project(issue: u64, bytes: Option<&[u8]>) -> Result<Value, Error> {
    let mut value = Value::object(vec![
        ("issue", Value::Number(issue as f64)),
        (
            "log",
            Value::string(if bytes.is_some() {
                "observed"
            } else {
                "missing"
            }),
        ),
        ("seat", Value::Null),
        ("event", Value::Null),
        ("state", Value::Null),
        ("last_complete_record", Value::Null),
        ("partial_line_ignored", Value::Boolean(false)),
        ("authority", Value::string("local-observation-only")),
    ]);
    let Some(bytes) = bytes else {
        return Ok(value);
    };
    if bytes.len() > MAX_LOG_BYTES {
        return Err(error("Session log exceeds 2 MiB"));
    }
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |last| last + 1);
    domain::put(
        &mut value,
        "partial_line_ignored",
        Value::Boolean(complete < bytes.len()),
    )?;
    let source = std::str::from_utf8(
        bytes
            .get(..complete)
            .ok_or_else(|| error("Invalid log prefix"))?,
    )
    .map_err(|_| error("Complete session log records must be UTF-8"))?;
    let mut last = None;
    for (sequence, line) in source.split_terminator('\n').enumerate() {
        if line.trim().is_empty() {
            return Err(error("Session log framing is invalid"));
        }
        let record = json::parse(&Text::from(line))
            .map_err(|_| error("Session log contains malformed complete JSON"))?;
        if record.get("schema_version") != Some(&Value::Number(1.0))
            || domain::safe_integer(record.get("sequence")) != Some(sequence as u64)
        {
            return Err(error("Session log schema or sequence is invalid"));
        }
        let seat = word(record.get("seat"))?;
        if !["host", "planner", "coder", "reviewer"]
            .iter()
            .any(|name| seat == Text::from(*name))
        {
            return Err(error("Session log names an unknown seat"));
        }
        if seat != Text::from("host") {
            domain::put(&mut value, "seat", Value::String(seat.clone()))?;
        }
        let event = word(record.get("event"))?;
        let state = word(record.get("status"))?;
        domain::put(&mut value, "event", Value::String(event.clone()))?;
        domain::put(&mut value, "state", Value::String(state.clone()))?;
        last = Some(Value::object(vec![
            ("sequence", Value::Number(sequence as f64)),
            ("seat", Value::String(seat)),
            ("event", Value::String(event)),
            ("status", Value::String(state)),
        ]));
    }
    domain::put(
        &mut value,
        "last_complete_record",
        last.unwrap_or(Value::Null),
    )?;
    Ok(value)
}

pub async fn observe(
    cwd: &Path,
    environment: &Value,
    options: &Options,
    token: &CancellationToken,
) -> Result<Snapshot, Error> {
    let outcome = async {
        if !options.offline {
            return Err(error("The local-only status API requires explicit offline mode; no authentication was attempted"));
        }
        if options.issue == 0 || options.issue > crate::MAX_SAFE_INTEGER {
            return Err(error("Status requires a positive safe issue number"));
        }
        token.check()?;
        let paths = paths::repository_paths(cwd)?;
        let image = host::selected_path(environment, "ROSTER_GIT")?
            .ok_or_else(|| error("Offline status requires an absolute approved ROSTER_GIT"))?;
        let git = start::Git::new(
            tools::Host {
                git: image,
                environment: host::minimal_environment(environment)?,
                ..tools::Host::default()
            },
            paths.git_dir.join("roster-status-disabled-hooks"),
        )?;
        let inventory = git
            .run(
                &paths.worktree,
                &["worktree", "list", "--porcelain", "-z"],
                token,
            )
            .await?;
        let inventory = start::parse_inventory(&inventory)?;
        let matching = inventory
            .iter()
            .filter(|worktree| {
                worktree
                    .branch
                    .as_deref()
                    .and_then(start::parse_issue_branch)
                    .is_some_and(|branch| {
                        branch.number == options.issue && branch.attempt.is_none()
                    })
            })
            .collect::<Vec<_>>();
        if matching.len() > 1 {
            return Err(error(
                "Offline status found ambiguous issue worktree ownership",
            ));
        }
        let selected =
            if let Some(worktree) = matching.first().filter(|worktree| !worktree.prunable) {
                let selected = paths::repository_paths(&worktree.path)?;
                if paths::file_information(&selected.common_dir)?.identity
                    != paths::file_information(&paths.common_dir)?.identity
                {
                    return Err(error("Issue worktree status belongs to another repository"));
                }
                log_path(&selected.worktree, options.issue)?
            } else {
                log_path(&paths.repository, options.issue)?
            };
        let mut bytes = read_log(&selected)?;
        if bytes.is_none() && selected != log_path(&paths.repository, options.issue)? {
            bytes = read_log(&log_path(&paths.repository, options.issue)?)?;
        }
        let value = project(options.issue, bytes.as_deref())?;
        let mut redaction = redaction::Config {
            environment: environment.clone(),
            ..Default::default()
        };
        for path in [
            &paths.worktree,
            &paths.repository,
            &paths.git_dir,
            &paths.common_dir,
        ] {
            redaction.paths.push(Text::from(
                path.to_str()
                    .ok_or_else(|| error("Status scope must be Unicode"))?,
            ));
        }
        let value = redaction::history_record(&value, &redaction)?;
        token.check()?;
        Ok(Snapshot { value })
    }
    .await;
    outcome.map_err(|cause: Error| {
        if cause.name == "StatusError"
            || matches!(
                cause.code,
                Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT" | "ROSTER_PROCESS_CLEANUP")
            )
        {
            cause
        } else {
            let mut safe = Error::new(cause.name, "Private status provider details were omitted");
            safe.code = cause.code;
            error("Offline status could not be read within its checked repository scope")
                .caused_by(safe)
        }
    })
}

pub async fn read<T: crate::llm::http::Transport>(
    transport: &mut T,
    installation: &Path,
    cwd: &Path,
    environment: &Value,
    options: &Options,
    token: &CancellationToken,
) -> Result<Snapshot, Error> {
    if options.offline {
        return observe(cwd, environment, options, token).await;
    }
    let issue = crate::github::read_issue(
        transport,
        installation,
        cwd,
        environment,
        options.issue,
        token,
    )
    .await?;
    let mut observed = observe(
        cwd,
        environment,
        &Options {
            offline: true,
            ..options.clone()
        },
        token,
    )
    .await?;
    issue.require_current(token).await?;
    let redaction = redaction::Config {
        environment: environment.clone(),
        ..Default::default()
    };
    redaction.validate()?;
    let redacted_issue = redaction::record(&issue.metadata(), &redaction)?;
    domain::put(&mut observed.value, "github_issue", redacted_issue)?;
    token.check()?;
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn offline_issue_status_is_explicit_and_validates_numbers_and_flags_before_discovery()
    -> Result<(), Error> {
        let options = Options::parse(&args(&["--issue", "42", "--offline", "--json"]))?;
        assert_eq!(options.issue, 42);
        assert!(options.offline);
        assert!(options.json);
        assert!(!Options::parse(&args(&["--issue", "42"]))?.offline);
        for input in [
            args(&[]),
            args(&["--issue", "0", "--offline"]),
            args(&["--issue", "042", "--offline"]),
            args(&["--issue", "9007199254740992", "--offline"]),
            args(&["--issue", "-1", "--offline"]),
            args(&["--issue", "1.0", "--offline"]),
            args(&["--issue", "42", "--offline", "--offline"]),
            args(&["--issue", "42", "--offline", "--repo", "outside"]),
        ] {
            assert!(Options::parse(&input).is_err());
        }
        assert_eq!(
            Options::parse(&args(&["--issue", "9007199254740991", "--offline"]))?.issue,
            crate::MAX_SAFE_INTEGER
        );
        Ok(())
    }

    #[test]
    fn missing_or_unfinished_status_is_unknown_and_never_a_verification_receipt()
    -> Result<(), Error> {
        let missing = project(42, None)?;
        assert_eq!(missing.get("log"), Some(&Value::string("missing")));
        assert_eq!(missing.get("seat"), Some(&Value::Null));
        assert_eq!(
            missing.get("authority"),
            Some(&Value::string("local-observation-only"))
        );
        let partial = project(42, Some(b"{unfinished-private-tail"))?;
        assert_eq!(partial.get("last_complete_record"), Some(&Value::Null));
        assert_eq!(
            partial.get("partial_line_ignored"),
            Some(&Value::Boolean(true))
        );
        assert!(!partial.json().contains("private-tail"));
        Ok(())
    }

    #[test]
    fn status_uses_only_complete_typed_metadata_and_never_raw_details() -> Result<(), Error> {
        let source = concat!(
            "{\"schema_version\":1,\"sequence\":0,\"event\":\"invocation\",\"seat\":\"host\",\"status\":\"session-only\"}\n",
            "{\"schema_version\":1,\"sequence\":1,\"event\":\"seat-end\",\"seat\":\"coder\",\"status\":\"pass\",\"details\":{\"body\":\"test-only-private-source\",\"key\":\"test-only-private-token\"}}\n",
            "{\"schema_version\":1,\"sequence\":2,\"event\":\"invocation-end\",\"seat\":\"host\",\"status\":\"verified\"}\n",
            "{\"partial\":\"test-only-private-tail"
        );
        let observed = project(42, Some(source.as_bytes()))?;
        assert_eq!(observed.get("seat"), Some(&Value::string("coder")));
        assert_eq!(
            observed.get("event"),
            Some(&Value::string("invocation-end"))
        );
        assert_eq!(observed.get("state"), Some(&Value::string("verified")));
        assert_eq!(
            observed.get("partial_line_ignored"),
            Some(&Value::Boolean(true))
        );
        assert!(!observed.json().contains("test-only-private"));
        Ok(())
    }

    #[test]
    fn corrupt_complete_records_fail_instead_of_becoming_missing_or_complete() {
        for source in [
            "not-json\n",
            "{\"schema_version\":2,\"sequence\":0,\"event\":\"seat-end\",\"seat\":\"coder\",\"status\":\"pass\"}\n",
            "{\"schema_version\":1,\"sequence\":1,\"event\":\"seat-end\",\"seat\":\"coder\",\"status\":\"pass\"}\n",
            "{\"schema_version\":1,\"sequence\":0,\"event\":\"seat-end\",\"seat\":\"foreign\",\"status\":\"pass\"}\n",
            "{\"schema_version\":1,\"sequence\":0,\"event\":\"seat-end\",\"seat\":\"coder\",\"status\":null}\n",
            "\n",
        ] {
            assert!(project(42, Some(source.as_bytes())).is_err());
        }
        assert!(
            project(
                42,
                Some(format!("{}\n", "x".repeat(MAX_LOG_BYTES + 1)).as_bytes())
            )
            .is_err()
        );
    }

    #[test]
    fn every_bounded_native_writer_record_and_sequence_remains_readable() -> Result<(), Error> {
        use std::fmt::Write;

        let prefix = "{\"schema_version\":1,\"sequence\":0,\"event\":\"seat-end\",\"seat\":\"coder\",\"status\":\"pass\",\"details\":{\"ignored\":\"";
        let suffix = "\"}}\n";
        let source = format!(
            "{prefix}{}{suffix}",
            "a".repeat(MAX_LOG_BYTES - prefix.len() - suffix.len())
        );
        assert_eq!(source.len(), MAX_LOG_BYTES);
        assert_eq!(
            project(42, Some(source.as_bytes()))?.get("state"),
            Some(&Value::string("pass"))
        );
        let mut source = String::new();
        for sequence in 0..10_001 {
            writeln!(&mut source, "{{\"schema_version\":1,\"sequence\":{sequence},\"event\":\"tool\",\"seat\":\"coder\",\"status\":\"observed\"}}")
                .map_err(|_| Error::type_error("Could not format owned status records"))?;
        }
        assert!(source.len() <= MAX_LOG_BYTES);
        assert_eq!(
            project(42, Some(source.as_bytes()))?
                .get("last_complete_record")
                .and_then(|record| record.get("sequence")),
            Some(&Value::Number(10_000.0))
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_status_refuses_before_repository_or_tool_discovery() -> Result<(), Error> {
        let cancellation = crate::cancel::CancellationSource::default();
        cancellation.cancel(crate::cancel::Reason::Cancelled)?;
        let result = observe(
            Path::new("missing-owned-status-root"),
            &Value::object(Vec::<(&str, Value)>::new()),
            &Options {
                issue: 42,
                offline: true,
                json: true,
            },
            &cancellation.token(),
        )
        .await;
        assert!(matches!(result, Err(cause) if cause.code == Some("ROSTER_CANCELLED")));
        Ok(())
    }
}
