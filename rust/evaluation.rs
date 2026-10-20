use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::cancel::CancellationToken;
use crate::json::Value;
use crate::planning::estimate;
use crate::state::{files, layout, locks, redaction, secure};
use crate::text::Text;
use crate::{Error, clock, domain, learning, metrics, paths};

mod github;

const USAGE: &str = "Use eval TARGET accept|reject|rework 1-5 y|n --local|--post [--minutes N] [--comment TEXT]. Posting requires an approved human GitHub CLI and explicit human credential.";

#[derive(Debug, Clone)]
enum Target {
    Sha(String),
    Abbreviation(String),
    Session(String),
}

#[derive(Debug, Clone)]
pub struct Decision {
    target: Target,
    verdict: &'static str,
    difficulty: u8,
    again: bool,
    minutes: Option<u64>,
    comment: Option<Text>,
    post: bool,
}

fn error(code: &'static str, message: &str) -> Error {
    Error::new("EvaluationError", message).coded(code)
}

fn usage() -> Error {
    error("ROSTER_EVALUATION_INPUT", USAGE)
}

impl Decision {
    pub fn parse(args: &[String]) -> Result<Self, Error> {
        let [target, verdict, difficulty, again, flags @ ..] = args else {
            return Err(usage());
        };
        let target = if learning::valid_sha(target) {
            Target::Sha(target.to_ascii_lowercase())
        } else if (4..=63).contains(&target.len())
            && target.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            Target::Abbreviation(target.clone())
        } else if (1..=64).contains(&target.len())
            && !target.starts_with('-')
            && target
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            Target::Session(target.clone())
        } else {
            return Err(usage());
        };
        let verdict = match verdict.as_str() {
            "accept" => "accept",
            "reject" => "reject",
            "rework" => "rework",
            _ => return Err(usage()),
        };
        let difficulty = match difficulty.as_bytes() {
            [value @ b'1'..=b'5'] => value - b'0',
            _ => return Err(usage()),
        };
        let again = if again.eq_ignore_ascii_case("y") {
            true
        } else if again.eq_ignore_ascii_case("n") {
            false
        } else {
            return Err(usage());
        };
        let mut minutes = None;
        let mut comment = None;
        let mut post = None;
        let mut seen = BTreeSet::new();
        let mut position = 0;
        while let Some(flag) = flags.get(position) {
            if !seen.insert(flag) {
                return Err(usage());
            }
            match flag.as_str() {
                "--local" | "--post" => {
                    if post.replace(flag == "--post").is_some() {
                        return Err(usage());
                    }
                }
                "--minutes" | "--comment" => {
                    position += 1;
                    let value = flags.get(position).ok_or_else(usage)?;
                    if flag == "--minutes" {
                        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                            return Err(usage());
                        }
                        minutes = Some(
                            value
                                .parse::<u64>()
                                .ok()
                                .filter(|value| *value <= crate::MAX_SAFE_INTEGER)
                                .ok_or_else(usage)?,
                        );
                    } else {
                        if value.encode_utf16().count() > 4096 {
                            return Err(usage());
                        }
                        comment = Some(Text::from(value.as_str()));
                    }
                }
                _ => return Err(usage()),
            }
            position += 1;
        }
        let post = post.ok_or_else(usage)?;
        Ok(Self {
            target,
            verdict,
            difficulty,
            again,
            minutes,
            comment,
            post,
        })
    }
}

pub struct Recorded {
    pub metadata_known: bool,
    posting: github::Status,
}

impl Recorded {
    pub fn format(&self) -> String {
        let metadata = if self.metadata_known {
            "Matched observed run metadata."
        } else {
            "No matching observed run metadata; unknown fields remain unknown."
        };
        format!(
            "Recorded the explicit human decision in the repository-scoped evaluation ledger.\n{metadata}\n{}",
            self.posting.format()
        )
    }
}

fn require_human_invocation(environment: &Value) -> Result<(), Error> {
    for (name, _) in domain::members(environment)
        .ok_or_else(|| Error::type_error("Evaluation environment must be an object"))?
    {
        let name = name.to_utf8()?.to_ascii_uppercase();
        if name.starts_with("AI_")
            || [
                "GITHUB_APP_ID",
                "GITHUB_APP_PRIVATE_KEY_PATH",
                "ROSTER_AGENT_SEAT",
            ]
            .contains(&name.as_str())
        {
            return Err(error(
                "ROSTER_EVALUATION_HUMAN",
                "Human evaluation is unavailable in an App or agent-declared invocation. Use a separate human session; no decision was recorded.",
            ));
        }
    }
    Ok(())
}

fn read_ledger(path: &Path) -> Result<(Option<secure::Snapshot>, Vec<Value>), Error> {
    secure::check_components(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file()
                || secure::linked(&metadata)
                || paths::file_information(path)?.links != 1
            {
                return Err(error(
                    "ROSTER_EVALUATION_INPUT",
                    "The evaluation ledger must be a real single-link file; its bytes were preserved",
                ));
            }
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            return Ok((None, Vec::new()));
        }
        Err(cause) => {
            return Err(secure::io_error(
                "Inspect human evaluation ledger",
                path,
                cause,
            ));
        }
    }
    let snapshot = secure::read(path, learning::MAX_JSONL_BYTES as u64)?;
    let Some(current) = &snapshot else {
        return Ok((None, Vec::new()));
    };
    if paths::file_information(path)?.links != 1
        || !secure::unchanged(path, current, learning::MAX_JSONL_BYTES as u64)?
    {
        return Err(error(
            "ROSTER_EVALUATION_CHANGED",
            "The evaluation ledger is linked or changed; preserve it rather than recording a partial decision",
        ));
    }
    let source = std::str::from_utf8(&current.bytes).map_err(|_| {
        error(
            "ROSTER_EVALUATION_INPUT",
            "The existing evaluation ledger must be UTF-8; its bytes were preserved",
        )
    })?;
    let rows = learning::parse_jsonl(
        &Text::from(source),
        "existing human evaluations",
        learning::JsonlKind::HumanEvaluation,
    )?;
    Ok((snapshot, rows))
}

fn safe_error(cause: Error) -> Error {
    if ["EvaluationError", "LearningError", "HumanFeedbackError"].contains(&cause.name)
        || matches!(
            cause.code,
            Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT" | "ROSTER_PROCESS_CLEANUP")
        )
    {
        return cause;
    }
    let mut safe = Error::new(cause.name, "Checked provider details were omitted");
    safe.code = cause.code;
    error(
        "ROSTER_EVALUATION_PROVIDER",
        "Human evaluation could not be recorded; verify bounded repository history, state scope, file permissions and lock ownership. Existing ledger bytes were preserved unless the atomic replacement had already completed.",
    )
    .caused_by(safe)
}

pub async fn record_local(
    cwd: &Path,
    environment: &Value,
    decision: &Decision,
    token: &CancellationToken,
) -> Result<Recorded, Error> {
    if decision.post {
        return Err(usage());
    }
    record(cwd, environment, decision, token).await
}

pub async fn record(
    cwd: &Path,
    environment: &Value,
    decision: &Decision,
    token: &CancellationToken,
) -> Result<Recorded, Error> {
    record_selected(cwd, environment, decision, token)
        .await
        .map_err(safe_error)
}

async fn record_selected(
    cwd: &Path,
    environment: &Value,
    decision: &Decision,
    token: &CancellationToken,
) -> Result<Recorded, Error> {
    require_human_invocation(environment)?;
    token.check()?;
    let paths = paths::repository_paths(cwd)?;
    let target = match &decision.target {
        Target::Sha(sha) => Value::object(vec![("sha", Value::string(sha))]),
        Target::Session(session) => Value::object(vec![("session", Value::string(session))]),
        Target::Abbreviation(abbreviation) => Value::object(vec![(
            "sha",
            Value::string(&metrics::evaluation_sha(cwd, environment, abbreviation, token).await?),
        )]),
    };
    let directory =
        layout::resolve_directory(layout::Scope::Shared, &paths.repository, None, None)?;
    secure::check_components(&directory.dir)?;
    secure::check_components(&directory.legacy_root)?;
    let ledger = layout::state_path(directory.clone(), &["evals.jsonl"])?;
    let lock_path = ledger.path.with_file_name("evals.lock");
    secure::check_components(&lock_path)?;
    let mut lock = locks::acquire(
        "evaluations",
        &lock_path,
        &locks::Options {
            holder: format!("human-evaluation-{}", std::process::id()),
            wait: Duration::from_secs(10),
            ..Default::default()
        },
        Some(token),
    )
    .await?;
    let uncertain = Cell::new(false);
    let operation = async {
        lock.check()?;
        token.check()?;
        if layout::state_path(directory.clone(), &["evals.jsonl"])?.path != ledger.path {
            return Err(error(
                "ROSTER_EVALUATION_CHANGED",
                "Evaluation state selection changed; preserve both layouts before retrying",
            ));
        }
        let (before, rows) = read_ledger(&ledger.path)?;
        if rows.len() == learning::MAX_ROWS {
            return Err(error(
                "ROSTER_EVALUATION_SIZE",
                "The evaluation ledger already contains 10000 rows; preserve or explicitly retain history before appending",
            ));
        }
        let metadata = metrics::evaluation_metadata(cwd, environment, &target, &rows, token).await?;
        token.check()?;
        lock.check()?;
        let mut record = Value::object(vec![
            ("sha", target.get("sha").cloned().unwrap_or(Value::Null)),
            ("session", target.get("session").cloned().unwrap_or(Value::Null)),
            ("model", Value::Null),
            ("task_class", Value::Null),
            ("verdict", Value::string(decision.verdict)),
            ("difficulty", Value::Number(f64::from(decision.difficulty))),
            ("again", Value::Boolean(decision.again)),
            (
                "minutes",
                decision.minutes.map_or(Value::Null, |value| Value::Number(value as f64)),
            ),
            (
                "comment",
                decision.comment.clone().map_or(Value::Null, Value::String),
            ),
            ("at", Value::string(&clock::now_iso()?)),
        ]);
        if let Some(metadata) = &metadata {
            for field in [
                "sha", "session", "model", "task", "task_class", "seat", "effort", "provider",
                "provenance",
            ] {
                if let Some(value) = metadata.get(field).filter(|value| **value != Value::Null) {
                    if matches!(field, "sha" | "session")
                        && record.get(field).is_some_and(|value| *value != Value::Null)
                        && !(field == "session" && metadata.get("provenance").is_some())
                    {
                        continue;
                    }
                    domain::put(&mut record, field, value.clone())?;
                }
            }
        }
        let mut redaction = redaction::Config {
            environment: environment.clone(),
            ..Default::default()
        };
        for path in [&paths.repository, &paths.worktree, &paths.common_dir, &paths.git_dir] {
            redaction.paths.push(Text::from(
                path.to_str()
                    .ok_or_else(|| Error::type_error("Evaluation scope must be Unicode"))?,
            ));
        }
        for name in ["HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "ROSTER_STATE_HOME"] {
            if let Some(value) = environment.get(name).and_then(Value::as_string) {
                redaction.paths.push(value.clone());
            }
        }
        let safe = redaction::history_record(&record, &redaction)?;
        for field in ["sha", "session", "model", "task_class", "provenance"] {
            if safe.get(field) != record.get(field) {
                return Err(error(
                    "ROSTER_EVALUATION_INPUT",
                    "Evaluation attribution required redaction; no guessed or redacted identity was recorded",
                ));
            }
        }
        estimate::validate_evaluation(&safe)?;
        let line = format!("{}\n", safe.json()).into_bytes();
        if line.len() > learning::MAX_LINE_BYTES {
            return Err(error("ROSTER_EVALUATION_SIZE", "An evaluation row exceeds 1 MiB"));
        }
        let mut bytes = before.as_ref().map_or_else(Vec::new, |before| before.bytes.clone());
        if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
            bytes.push(b'\n');
        }
        if bytes.len().checked_add(line.len()).is_none_or(|size| size > learning::MAX_JSONL_BYTES) {
            return Err(error("ROSTER_EVALUATION_SIZE", "The evaluation ledger would exceed 16 MiB"));
        }
        bytes.extend_from_slice(&line);
        let posting = if decision.post {
            github::prepare(&paths, environment, &safe, &uncertain, token).await?
        } else {
            github::Plan::Local
        };
        lock.check()?;
        token.check()?;
        let unchanged = match &before {
            Some(before) => secure::unchanged(&ledger.path, before, learning::MAX_JSONL_BYTES as u64)?,
            None => secure::read(&ledger.path, learning::MAX_JSONL_BYTES as u64)?.is_none(),
        };
        if !unchanged || layout::state_path(directory, &["evals.jsonl"])?.path != ledger.path {
            return Err(error("ROSTER_EVALUATION_CHANGED", "The evaluation ledger changed before recording; existing bytes were preserved"));
        }
        files::atomic_write_bytes(&ledger.path, &bytes, learning::MAX_JSONL_BYTES as u64)?;
        if let Err(mut cause) = token.check() {
            cause.message.push_str(" The evaluation may already have been appended; inspect the ledger before retrying. Cancellation is not rollback.");
            return Err(cause);
        }
        let posting = posting.execute(&paths.repository, &uncertain, token).await.map_err(|cause| {
            let mut failure = error(
                "ROSTER_HUMAN_FEEDBACK_UNKNOWN",
                "The local human decision was saved, but GitHub feedback did not complete. The remote outcome may be uncertain; inspect the pull request before retrying. No automatic retry was performed.",
            );
            if matches!(cause.code, Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT" | "ROSTER_PROCESS_CLEANUP")) {
                failure.code = cause.code;
            }
            failure.caused_by(cause)
        })?;
        token.check()?;
        Ok(Recorded { metadata_known: metadata.is_some(), posting })
    }
    .await;
    finish_record(operation, &mut lock, uncertain.get()).await
}

async fn finish_record(
    operation: Result<Recorded, Error>,
    lock: &mut locks::RepoLock,
    uncertain: bool,
) -> Result<Recorded, Error> {
    if uncertain
        || operation
            .as_ref()
            .err()
            .is_some_and(|failure| failure.code == Some("ROSTER_PROCESS_CLEANUP"))
    {
        let failure = error(
            "ROSTER_PROCESS_CLEANUP",
            "Evaluation provider cleanup is unverified; the evaluation lock is retained. Preserve local and remote evidence before retrying.",
        );
        return Err(match operation {
            Ok(_) => failure,
            Err(cause) => failure.caused_by(cause),
        });
    }
    locks::finish(operation, lock.release_required().await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::{CancellationSource, Reason};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn explicit_human_input_keeps_zero_unknowns_and_comment_bytes() -> Result<(), Error> {
        let decision = Decision::parse(&args(&[
            "human-session",
            "accept",
            "5",
            "N",
            "--local",
            "--minutes",
            "0",
            "--comment",
            "  Explicit human text.\r\nNot executable policy.  ",
        ]))?;
        assert!(matches!(decision.target, Target::Session(ref value) if value == "human-session"));
        assert_eq!(decision.verdict, "accept");
        assert_eq!(decision.difficulty, 5);
        assert!(!decision.again);
        assert_eq!(decision.minutes, Some(0));
        assert_eq!(
            decision.comment,
            Some(Text::from(
                "  Explicit human text.\r\nNot executable policy.  "
            ))
        );
        let unknown = Decision::parse(&args(&["human-session", "rework", "1", "y", "--local"]))?;
        assert_eq!(unknown.minutes, None);
        assert_eq!(unknown.comment, None);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_is_explicit_and_the_local_only_api_never_performs_network_work()
    -> Result<(), Error> {
        let decision = Decision::parse(&args(&["human-session", "accept", "2", "y", "--post"]))?;
        assert!(decision.post);
        let failure = record_local(
            Path::new("nonexistent-owned-feedback-fixture"),
            &Value::object(Vec::new()),
            &decision,
            &CancellationSource::default().token(),
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("The local-only API adopted a posting decision"))?;
        assert_eq!(failure.code, Some("ROSTER_EVALUATION_INPUT"));
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn git_cleanup_failure_retains_the_actual_writer_lock_without_a_client_uncertainty_flag()
    -> Result<(), Error> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".roster")
            .join(format!(
                "evaluation-cleanup-{}-{}",
                std::process::id(),
                files::nonce()?
            ));
        files::create_private_directories(&directory)?;
        let ordinary_path = directory.join("ordinary.lock");
        let mut ordinary = locks::acquire(
            "evaluations",
            &ordinary_path,
            &locks::Options::default(),
            None,
        )
        .await?;
        let ordinary_failure = finish_record(
            Err(Error::new(
                "GitExecutionError",
                "Owned fixture provider refused before cleanup was uncertain",
            )
            .coded("ROSTER_HUMAN_FEEDBACK_PROVIDER")),
            &mut ordinary,
            false,
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Provider refusal reported success"))?;
        assert_eq!(
            ordinary_failure.code,
            Some("ROSTER_HUMAN_FEEDBACK_PROVIDER")
        );
        assert!(
            !ordinary_path.exists(),
            "Known-settled provider failure retained writer authority"
        );
        let path = directory.join("writer.lock");
        let mut lock =
            locks::acquire("evaluations", &path, &locks::Options::default(), None).await?;
        let failure = finish_record(
            Err(Error::new(
                "GitExecutionError",
                "Owned fixture Git cleanup is unverified",
            )
            .coded("ROSTER_PROCESS_CLEANUP")),
            &mut lock,
            false,
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Unverified Git cleanup reported success"))?;
        assert_eq!(failure.code, Some("ROSTER_PROCESS_CLEANUP"));
        assert!(
            path.is_file(),
            "Unverified Git cleanup released the actual writer lock"
        );
        eprintln!(
            "EVALUATION_CLEANUP_FIXTURE_RETAINED pid={} root={}",
            std::process::id(),
            directory.display()
        );
        Ok(())
    }

    #[test]
    fn human_arguments_reject_unknown_flags_duplicates_and_exact_numeric_overflow()
    -> Result<(), Error> {
        for values in [
            args(&[]),
            args(&["human-session", "accept", "2", "n"]),
            args(&["human-session", "pass", "2", "n", "--local"]),
            args(&["human-session", "accept", "0", "n", "--local"]),
            args(&["human-session", "accept", "6", "n", "--local"]),
            args(&["human-session", "accept", "2.0", "n", "--local"]),
            args(&["human-session", "accept", "2", "true", "--local"]),
            args(&["../private", "accept", "2", "n", "--local"]),
            args(&["-session", "accept", "2", "n", "--local"]),
            args(&["human-session", "accept", "2", "n", "--local", "--local"]),
            args(&["human-session", "accept", "2", "n", "--local", "--post"]),
            args(&[
                "human-session",
                "accept",
                "2",
                "n",
                "--local",
                "--minutes",
                "-1",
            ]),
            args(&[
                "human-session",
                "accept",
                "2",
                "n",
                "--local",
                "--minutes",
                "0.5",
            ]),
            args(&[
                "human-session",
                "accept",
                "2",
                "n",
                "--local",
                "--minutes",
                "9007199254740992",
            ]),
            args(&["human-session", "accept", "2", "n", "--local", "--comment"]),
        ] {
            assert!(Decision::parse(&values).is_err());
        }
        let maximum = Decision::parse(&args(&[
            "human-session",
            "reject",
            "2",
            "y",
            "--local",
            "--minutes",
            "9007199254740991",
        ]))?;
        assert_eq!(maximum.minutes, Some(crate::MAX_SAFE_INTEGER));
        let mut exact = args(&["human-session", "accept", "2", "n", "--local", "--comment"]);
        exact.push("a".repeat(4096));
        assert!(Decision::parse(&exact).is_ok());
        exact.last_mut().ok_or_else(usage)?.push('a');
        assert!(Decision::parse(&exact).is_err());
        Ok(())
    }

    #[test]
    fn full_hashes_abbreviations_and_opaque_sessions_stay_distinct() -> Result<(), Error> {
        for length in [40, 64] {
            let mut input = args(&["unused", "accept", "2", "y", "--local"]);
            input[0] = "A".repeat(length);
            assert!(
                matches!(Decision::parse(&input)?.target, Target::Sha(value) if value == "a".repeat(length))
            );
        }
        for length in [4, 39, 41, 63] {
            let mut input = args(&["unused", "accept", "2", "y", "--local"]);
            input[0] = "a".repeat(length);
            assert!(
                matches!(Decision::parse(&input)?.target, Target::Abbreviation(value) if value.len() == length)
            );
        }
        let input = args(&["123", "accept", "2", "y", "--local"]);
        assert!(matches!(
            Decision::parse(&input)?.target,
            Target::Session(_)
        ));
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn app_agent_and_precancelled_invocations_refuse_before_any_repository_access()
    -> Result<(), Error> {
        let decision = Decision::parse(&args(&["human-session", "accept", "2", "n", "--local"]))?;
        for key in [
            "GITHUB_APP_ID",
            "github_app_private_key_path",
            "AI_MODEL",
            "ai_provider",
            "ROSTER_AGENT_SEAT",
        ] {
            let environment = Value::object(vec![(key, Value::string(""))]);
            let failure = record_local(
                Path::new("nonexistent-evaluation-fixture"),
                &environment,
                &decision,
                &CancellationSource::default().token(),
            )
            .await
            .err()
            .ok_or_else(|| Error::type_error("Agent context recorded human feedback"))?;
            assert_eq!(failure.code, Some("ROSTER_EVALUATION_HUMAN"));
        }
        let source = CancellationSource::default();
        source.cancel(Reason::Cancelled)?;
        let failure = record_local(
            Path::new("nonexistent-evaluation-fixture"),
            &Value::object(Vec::new()),
            &decision,
            &source.token(),
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Cancelled evaluation performed discovery"))?;
        assert_eq!(failure.code, Some("ROSTER_CANCELLED"));
        Ok(())
    }
}
