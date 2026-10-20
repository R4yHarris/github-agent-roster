use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::Error;
use crate::clock;
use crate::config;
use crate::domain;
use crate::identity::sha256_hex;
use crate::json::{self, Value};
use crate::llm::redact::Redactor;
use crate::paths;
use crate::state::{files, layout, redaction, secure};
use crate::text::Text;

pub const BENCH_FIELDS: &[&str] = &[
    "process_start_ms",
    "config_load_ms",
    "command_dispatch_ms",
    "git_worktree_add_ms",
    "submodule_init_ms",
    "mocked_model_round_trip_ms",
];

const DEBUG_SEATS: &[&str] = &["planner", "coder", "reviewer"];
const DEBUG_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "delete_file",
    "glob_files",
    "list_dir",
    "run_test",
    "run_command",
    "search_text",
    "web_search",
    "web_fetch",
    "update_checklist",
];
const DEBUG_PATH_CLASSES: &[&str] = &[
    "root", "outside", "secret", "git", "policy", "workflow", "vendor", "managed", "tests",
    "source", "docs", "other",
];
const DEBUG_PHASES: &[&str] = &[
    "seat-start",
    "seat-end",
    "seat-error",
    "model",
    "http-start",
    "http-ok",
    "http-error",
    "waiting",
    "timeout",
    "timeout-retry",
    "stall",
    "served-model",
    "tool-start",
    "tool-ok",
    "tool-error",
    "tool-denied",
    "tool-refused",
    "finish-reason",
    "finish-retry",
    "completion",
    "steering",
    "test-repair",
    "contracts-uninitialized",
    "wrote",
    "implementation",
    "usage",
    "checklist",
];
const DEBUG_FIELDS: &[&str] = &[
    "time",
    "issue",
    "seat",
    "phase",
    "tool_name",
    "path_class",
    "finish_reason",
    "test_name",
    "exit_code",
    "repair",
    "elapsed_ms",
];
const DEBUG_FINISH_REASONS: &[&str] = &[
    "stop",
    "tool_calls",
    "length",
    "content_filter",
    "function_call",
    "eos_token",
    "error",
    "cancelled",
    "abort",
    "redacted",
    "unsupported",
];

static DIAG_SEQ: AtomicU64 = AtomicU64::new(1);

fn round_ms(ms: f64) -> f64 {
    ((ms.max(0.0)) * 1000.0).round() / 1000.0
}

fn elapsed_ms(started: Instant) -> f64 {
    round_ms(started.elapsed().as_secs_f64() * 1000.0)
}

fn git_executable() -> PathBuf {
    std::env::var_os("ROSTER_TEST_GIT").map_or_else(|| PathBuf::from("git"), PathBuf::from)
}

fn bench_git_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for name in [
        "PATH",
        "PATHEXT",
        "SYSTEMROOT",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "HOME",
        "USERPROFILE",
    ] {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                env.insert(name.to_owned(), value);
            }
        }
    }
    env.insert("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned());
    env.insert("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned());
    env
}

fn run_git_sync(cwd: &Path, args: &[&str], env: &BTreeMap<String, String>) -> Result<(), Error> {
    let output = StdCommand::new(git_executable())
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .args(args)
        .output()
        .map_err(|error| Error::new("Error", format!("git {}: {error}", args.join(" "))))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(
            "Error",
            format!("git {} failed: {}", args.join(" "), stderr.trim()),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct BenchReport {
    pub process_start_ms: f64,
    pub config_load_ms: f64,
    pub command_dispatch_ms: f64,
    pub git_worktree_add_ms: f64,
    pub submodule_init_ms: f64,
    pub mocked_model_round_trip_ms: f64,
}

impl BenchReport {
    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("process_start_ms", Value::Number(self.process_start_ms)),
            ("config_load_ms", Value::Number(self.config_load_ms)),
            (
                "command_dispatch_ms",
                Value::Number(self.command_dispatch_ms),
            ),
            (
                "git_worktree_add_ms",
                Value::Number(self.git_worktree_add_ms),
            ),
            ("submodule_init_ms", Value::Number(self.submodule_init_ms)),
            (
                "mocked_model_round_trip_ms",
                Value::Number(self.mocked_model_round_trip_ms),
            ),
        ])
    }
}

pub fn run_bench(
    cwd: &Path,
    repo_root: &Path,
    environment: &Value,
    command_dispatch_ms: f64,
) -> Result<BenchReport, Error> {
    if cwd.as_os_str().is_empty()
        || repo_root.as_os_str().is_empty()
        || !command_dispatch_ms.is_finite()
        || command_dispatch_ms < 0.0
    {
        return Err(Error::type_error(
            "Benchmark requires paths and a nonnegative command dispatch duration",
        ));
    }
    let process_start_ms = round_ms(command_dispatch_ms);
    let config_started = Instant::now();
    config::load(repo_root, cwd, environment)?;
    let config_load_ms = elapsed_ms(config_started);
    let command_dispatch_ms = round_ms(command_dispatch_ms);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let seq = DIAG_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp_root =
        std::env::temp_dir().join(format!("roster-bench-{}-{nanos}-{seq}", std::process::id()));
    files::create_private_directories(&temp_root)?;

    let timing_result = (|| -> Result<(f64, f64, f64), Error> {
        let repository = temp_root.join("repository");
        let contracts = temp_root.join("contracts");
        let worktree = temp_root.join("worktree");
        let env = bench_git_env();
        fs::create_dir_all(&repository).map_err(|error| Error::new("Error", error.to_string()))?;
        fs::create_dir_all(&contracts).map_err(|error| Error::new("Error", error.to_string()))?;
        run_git_sync(&repository, &["init", "--quiet"], &env)?;
        run_git_sync(&contracts, &["init", "--quiet"], &env)?;
        fs::write(repository.join("README.md"), "# benchmark\n")
            .map_err(|error| Error::new("Error", error.to_string()))?;
        fs::write(contracts.join("agent-pr.mjs"), "export {};\n")
            .map_err(|error| Error::new("Error", error.to_string()))?;
        run_git_sync(&repository, &["add", "README.md"], &env)?;
        run_git_sync(&contracts, &["add", "agent-pr.mjs"], &env)?;
        run_git_sync(
            &contracts,
            &[
                "-c",
                "user.name=Roster Bench",
                "-c",
                "user.email=bench@localhost",
                "commit",
                "--quiet",
                "-m",
                "contracts fixture",
            ],
            &env,
        )?;
        let contracts_arg = contracts.display().to_string();
        run_git_sync(
            &repository,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                &contracts_arg,
                "vendor/github-agent-contracts",
            ],
            &env,
        )?;
        run_git_sync(
            &repository,
            &["add", ".gitmodules", "vendor/github-agent-contracts"],
            &env,
        )?;
        run_git_sync(
            &repository,
            &[
                "-c",
                "user.name=Roster Bench",
                "-c",
                "user.email=bench@localhost",
                "commit",
                "--quiet",
                "-m",
                "benchmark",
            ],
            &env,
        )?;

        let worktree_arg = worktree.display().to_string();
        let wt_started = Instant::now();
        run_git_sync(
            &repository,
            &["worktree", "add", "--quiet", "--detach", &worktree_arg],
            &env,
        )?;
        let git_worktree_add_ms = elapsed_ms(wt_started);

        let sub_started = Instant::now();
        run_git_sync(
            &worktree,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "--recursive",
            ],
            &env,
        )?;
        let submodule_init_ms = elapsed_ms(sub_started);

        let model_started = Instant::now();
        let _mock = Value::object(vec![(
            "choices",
            Value::Array(vec![Value::object(vec![(
                "message",
                Value::object(vec![("content", Value::string("benchmark"))]),
            )])]),
        )]);
        let mocked_model_round_trip_ms = elapsed_ms(model_started);
        Ok((
            git_worktree_add_ms,
            submodule_init_ms,
            mocked_model_round_trip_ms,
        ))
    })();
    let _ = fs::remove_dir_all(&temp_root);
    let (git_worktree_add_ms, submodule_init_ms, mocked_model_round_trip_ms) = timing_result?;

    let report = BenchReport {
        process_start_ms,
        config_load_ms,
        command_dispatch_ms,
        git_worktree_add_ms,
        submodule_init_ms,
        mocked_model_round_trip_ms,
    };
    for value in [
        report.process_start_ms,
        report.config_load_ms,
        report.command_dispatch_ms,
        report.git_worktree_add_ms,
        report.submodule_init_ms,
        report.mocked_model_round_trip_ms,
    ] {
        if !value.is_finite() || value < 0.0 {
            return Err(Error::new(
                "Error",
                "Benchmark produced an invalid timing report",
            ));
        }
    }
    let roster_dir = cwd.join(".roster");
    files::create_private_directories(&roster_dir)?;
    let report_path = roster_dir.join("bench.json");
    secure::check_components(&report_path)?;
    if let Ok(meta) = fs::symlink_metadata(&report_path) {
        if secure::linked(&meta) || !meta.is_file() {
            return Err(Error::new(
                "Error",
                "Benchmark report target must be a regular unlinked file",
            ));
        }
    }
    let serialized = format!("{}\n", report.as_value().pretty());
    files::atomic_write_file(&report_path, &Text::from(serialized.as_str()))?;
    Ok(report)
}

pub fn debug_log_error(message: impl Into<String>) -> Error {
    Error::new("DebugLogError", message).coded("ROSTER_RUN_LOG")
}

pub fn debug_path_class(value: Option<&str>) -> Option<&'static str> {
    let raw = value?;
    let file = raw.replace('\\', "/").to_lowercase();
    let parts = file.split('/').collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| *part == ".env" || part.starts_with(".env.") || part.ends_with(".pem"))
        || file.contains(".roster/vault")
    {
        return Some("secret");
    }
    let is_abs = file.starts_with('/')
        || (file.len() >= 3
            && file.as_bytes()[0].is_ascii_alphabetic()
            && file.as_bytes()[1] == b':'
            && file.as_bytes()[2] == b'/');
    if is_abs || parts.contains(&"..") {
        return Some("outside");
    }
    if parts.contains(&".git") {
        return Some("git");
    }
    if parts.contains(&"agent-policy.yml") {
        return Some("policy");
    }
    if file.contains(".github/workflows") {
        return Some("workflow");
    }
    if parts.contains(&"vendor") {
        return Some("vendor");
    }
    if file.starts_with(".roster/")
        || [
            "assignment.md",
            "task.md",
            "recipe.yml",
            "plan.md",
            "context.md",
            "research.md",
            "result.md",
            "review.md",
            "estimate.md",
        ]
        .contains(&file.as_str())
    {
        return Some("managed");
    }
    if parts.contains(&"tests")
        || parts.contains(&"test")
        || parts.contains(&"fixtures")
        || regex_lite::Regex::new(r"[._-]test\.[cm]?js$").is_ok_and(|re| re.is_match(&file))
    {
        return Some("tests");
    }
    if parts.first() == Some(&"src") {
        return Some("source");
    }
    if parts.first() == Some(&"docs") || file.ends_with(".md") {
        return Some("docs");
    }
    if matches!(file.as_str(), "" | "." | "./") {
        Some("root")
    } else {
        Some("other")
    }
}

fn valid_debug_row(row: &Value) -> bool {
    let Some(entries) = domain::members(row) else {
        return false;
    };
    if entries.len() != DEBUG_FIELDS.len()
        || DEBUG_FIELDS.iter().any(|field| row.get(field).is_none())
    {
        return false;
    }
    let Some(time) = row
        .get("time")
        .and_then(Value::as_string)
        .and_then(|text| text.to_utf8().ok())
    else {
        return false;
    };
    if !regex_lite::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$")
        .is_ok_and(|re| re.is_match(&time))
    {
        return false;
    }
    let valid_issue = match row.get("issue") {
        Some(Value::Null) => true,
        Some(value) => domain::safe_integer(Some(value)).is_some_and(|issue| issue > 0),
        None => false,
    };
    let valid_seat = row
        .get("seat")
        .and_then(Value::as_string)
        .and_then(|text| text.to_utf8().ok())
        .is_some_and(|seat| DEBUG_SEATS.contains(&seat.as_str()));
    let valid_phase = row
        .get("phase")
        .and_then(Value::as_string)
        .and_then(|text| text.to_utf8().ok())
        .is_some_and(|phase| DEBUG_PHASES.contains(&phase.as_str()));
    let valid_tool = match row.get("tool_name") {
        Some(Value::Null) => true,
        Some(value) => value
            .as_string()
            .and_then(|text| text.to_utf8().ok())
            .is_some_and(|tool| DEBUG_TOOLS.contains(&tool.as_str())),
        None => false,
    };
    let valid_path_class = match row.get("path_class") {
        Some(Value::Null) => true,
        Some(value) => value
            .as_string()
            .and_then(|text| text.to_utf8().ok())
            .is_some_and(|class| DEBUG_PATH_CLASSES.contains(&class.as_str())),
        None => false,
    };
    let valid_reason = match row.get("finish_reason") {
        Some(Value::Null) => true,
        Some(value) => value
            .as_string()
            .and_then(|text| text.to_utf8().ok())
            .is_some_and(|reason| DEBUG_FINISH_REASONS.contains(&reason.as_str())),
        None => false,
    };
    let valid_test = match row.get("test_name") {
        Some(Value::Null) => true,
        Some(value) => domain::is_text(Some(value), "node --test"),
        None => false,
    };
    let valid_exit = match row.get("exit_code") {
        Some(Value::Null) => true,
        Some(value) => domain::safe_integer(Some(value)).is_some(),
        None => false,
    };
    let valid_repair = match row.get("repair") {
        Some(Value::Null) => true,
        Some(repair) => {
            domain::members(repair).is_some_and(|fields| fields.len() == 2)
                && match (
                    domain::safe_integer(repair.get("n")),
                    domain::safe_integer(repair.get("of")),
                ) {
                    (Some(n), Some(of)) => n >= 1 && n <= of && [1, 2, 4].contains(&of),
                    _ => false,
                }
        }
        None => false,
    };
    let valid_elapsed = domain::safe_integer(row.get("elapsed_ms")).is_some();
    valid_issue
        && valid_seat
        && valid_phase
        && valid_tool
        && valid_path_class
        && valid_reason
        && valid_test
        && valid_exit
        && valid_repair
        && valid_elapsed
}

pub struct DebugLog {
    env: Value,
    redaction: redaction::Config,
    enabled: bool,
    session: String,
    started: Instant,
    latest_path: Option<PathBuf>,
    latest_root: Option<PathBuf>,
    checked_roots: BTreeSet<PathBuf>,
}

impl DebugLog {
    pub fn new(
        env: Option<&Value>,
        enabled: Option<bool>,
        session: Option<&str>,
    ) -> Result<Self, Error> {
        let env_value = env
            .cloned()
            .unwrap_or_else(crate::ask::environment_value_from_process);
        let active = enabled.unwrap_or_else(|| domain::is_text(env_value.get("ROSTER_DEBUG"), "1"));
        let session_id = match session {
            Some(value) => value.to_owned(),
            None => {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_nanos());
                let seq = DIAG_SEQ.fetch_add(1, Ordering::Relaxed);
                let hex =
                    sha256_hex(format!("debug-{}-{nanos}-{seq}", std::process::id()).as_bytes());
                format!("roster-{}", &hex[..16])
            }
        };
        if session_id.is_empty()
            || session_id.len() > 64
            || !session_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(Error::type_error("Invalid process debug log settings"));
        }
        let redaction = redaction::Config {
            environment: env_value.clone(),
            ..redaction::Config::default()
        };
        redaction.validate()?;
        let log = Self {
            env: env_value,
            redaction,
            enabled: active,
            session: session_id,
            started: Instant::now(),
            latest_path: None,
            latest_root: None,
            checked_roots: BTreeSet::new(),
        };
        if log.enabled && !log.session_is_safe()? {
            return Err(Error::type_error(
                "Debug session must not contain environment values",
            ));
        }
        Ok(log)
    }

    fn session_is_safe(&self) -> Result<bool, Error> {
        if let Some(entries) = domain::members(&self.env) {
            for (_, value) in entries {
                if let Some(text) = value.as_string().and_then(|text| text.to_utf8().ok()) {
                    if text == self.session {
                        return Ok(false);
                    }
                }
            }
        }
        let redactor = Redactor::new(&self.env, None);
        let redacted = redactor
            .redact(&Text::from(self.session.as_str()), None)
            .to_utf8()?;
        let evidence_redacted =
            redaction::evidence(&Text::from(self.session.as_str()), &self.redaction).to_utf8()?;
        Ok(redacted == self.session && evidence_redacted == self.session)
    }

    fn safe_reason(&self, value: Option<&Value>) -> Result<Value, Error> {
        let Some(value) = value.filter(|value| **value != Value::Null) else {
            return Ok(Value::Null);
        };
        let Some(reason) = value.as_string().and_then(|text| text.to_utf8().ok()) else {
            return Ok(Value::string("unsupported"));
        };
        if reason.is_empty()
            || reason.len() > 64
            || !reason
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Ok(Value::string("unsupported"));
        }
        if let Some(entries) = domain::members(&self.env) {
            for (_, entry) in entries {
                if let Some(text) = entry.as_string().and_then(|text| text.to_utf8().ok()) {
                    if !text.is_empty() && reason.contains(&text) {
                        return Ok(Value::string("redacted"));
                    }
                }
            }
        }
        let redactor = Redactor::new(&self.env, None);
        let redacted = redactor
            .redact(&Text::from(reason.as_str()), None)
            .to_utf8()?;
        let evidence_redacted =
            redaction::evidence(&Text::from(reason.as_str()), &self.redaction).to_utf8()?;
        if redacted != reason || evidence_redacted != reason {
            return Ok(Value::string("redacted"));
        }
        if DEBUG_FINISH_REASONS.contains(&reason.as_str()) {
            Ok(Value::string(&reason))
        } else {
            Ok(Value::string("unsupported"))
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn path(&self) -> Option<&Path> {
        self.latest_path.as_deref()
    }

    pub fn set_enabled(&mut self, value: bool) -> Result<(), Error> {
        if value && !self.session_is_safe()? {
            return Err(Error::type_error(
                "Debug session must not contain environment values",
            ));
        }
        self.enabled = value;
        Ok(())
    }

    pub fn record(
        &mut self,
        repo_root: &Path,
        issue: Option<u64>,
        seat: &str,
        event: &Value,
    ) -> Result<(), Error> {
        if !self.enabled {
            return Ok(());
        }
        let event_type = event
            .get("type")
            .and_then(Value::as_string)
            .and_then(|text| text.to_utf8().ok())
            .unwrap_or_default();
        let phase = match event_type.as_str() {
            "http" => format!(
                "http-{}",
                event
                    .get("phase")
                    .and_then(Value::as_string)
                    .and_then(|text| text.to_utf8().ok())
                    .unwrap_or_default()
            ),
            "tool" => "tool-start".to_owned(),
            "tool-result" => format!(
                "tool-{}",
                event
                    .get("status")
                    .and_then(Value::as_string)
                    .and_then(|text| text.to_utf8().ok())
                    .unwrap_or_default()
            ),
            "finish-reason" if event.get("retry").is_some_and(domain::truthy) => {
                "finish-retry".to_owned()
            }
            other => other.to_owned(),
        };
        let timestamp =
            clock::now_iso().map_err(|_| debug_log_error("Invalid debug event time"))?;
        let tool_name = if ["tool", "tool-result", "tool-refused"].contains(&event_type.as_str()) {
            event.get("name").cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let path_class = if ["tool", "tool-result", "wrote"].contains(&event_type.as_str()) {
            let path_str = event
                .get("path")
                .and_then(Value::as_string)
                .and_then(|text| text.to_utf8().ok());
            debug_path_class(path_str.as_deref()).map_or(Value::Null, Value::string)
        } else if event_type == "tool-refused" {
            if domain::is_text(event.get("reason"), "absolute-inside") {
                Value::Null
            } else {
                Value::string("outside")
            }
        } else {
            Value::Null
        };
        let finish_reason = if ["finish-reason", "completion"].contains(&event_type.as_str()) {
            self.safe_reason(event.get("reason"))?
        } else {
            Value::Null
        };
        let test_name = if ["tool", "tool-result"].contains(&event_type.as_str())
            && domain::is_text(event.get("name"), "run_test")
        {
            Value::string("node --test")
        } else {
            Value::Null
        };
        let exit_code = if event_type == "tool-result" {
            event.get("exit_code").cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let repair = if event_type == "test-repair" {
            Value::object(vec![
                ("n", event.get("attempt").cloned().unwrap_or(Value::Null)),
                ("of", event.get("budget").cloned().unwrap_or(Value::Null)),
            ])
        } else {
            Value::Null
        };
        let elapsed = self.started.elapsed().as_millis() as u64;
        let row = Value::object(vec![
            ("time", Value::string(&timestamp)),
            (
                "issue",
                issue.map_or(Value::Null, |number| Value::Number(number as f64)),
            ),
            ("seat", Value::string(seat)),
            ("phase", Value::string(&phase)),
            ("tool_name", tool_name),
            ("path_class", path_class),
            ("finish_reason", finish_reason),
            ("test_name", test_name),
            ("exit_code", exit_code),
            ("repair", repair),
            ("elapsed_ms", Value::Number(elapsed as f64)),
        ]);
        if !valid_debug_row(&row) {
            return Ok(());
        }
        let write_result = (|| -> Result<(), Error> {
            let file = repo_root
                .join(".roster")
                .join("logs")
                .join(format!("debug-{}.jsonl", self.session));
            paths::ensure_local_path(&file, repo_root)?;
            if !self.checked_roots.contains(repo_root) {
                let has_git = match fs::symlink_metadata(repo_root.join(".git")) {
                    Ok(_) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => return Err(Error::new("Error", error.to_string())),
                };
                if has_git {
                    let rel = format!(".roster/logs/debug-{}.jsonl", self.session);
                    let env = bench_git_env();
                    let ignore_status = StdCommand::new(git_executable())
                        .current_dir(repo_root)
                        .env_clear()
                        .envs(&env)
                        .args(["check-ignore", "--quiet", "--", &rel])
                        .status()
                        .map_err(|error| Error::new("Error", error.to_string()))?;
                    if !ignore_status.success() {
                        if ignore_status.code() == Some(1) {
                            return Err(debug_log_error(
                                "Debug logs must be untracked and gitignored; ignore .roster/logs/ before enabling",
                            ));
                        }
                        return Err(Error::new("Error", "git check-ignore failed"));
                    }
                    let ls_out = StdCommand::new(git_executable())
                        .current_dir(repo_root)
                        .env_clear()
                        .envs(&env)
                        .args(["ls-files", "--", &rel])
                        .output()
                        .map_err(|error| Error::new("Error", error.to_string()))?;
                    if !ls_out.status.success() {
                        return Err(Error::new("Error", "git ls-files failed"));
                    }
                    if !String::from_utf8_lossy(&ls_out.stdout).trim().is_empty() {
                        return Err(debug_log_error(
                            "Debug logs must be untracked and gitignored; ignore .roster/logs/ before enabling",
                        ));
                    }
                }
                self.checked_roots.insert(repo_root.to_path_buf());
            }
            if let Some(parent) = file.parent() {
                files::create_private_directories(parent)?;
            }
            paths::ensure_local_path(&file, repo_root)?;
            let before_info = match fs::symlink_metadata(&file) {
                Ok(meta) => {
                    if secure::linked(&meta) || !meta.is_file() {
                        return Err(debug_log_error(
                            "Debug log must be a regular, single-link file",
                        ));
                    }
                    Some(paths::file_information(&file)?)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(Error::new("Error", error.to_string())),
            };
            let mut open = OpenOptions::new();
            open.write(true).append(true).create(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open.mode(0o600);
            }
            let mut handle = open
                .open(&file)
                .map_err(|error| Error::new("Error", error.to_string()))?;
            let info = paths::handle_information(&handle)?;
            let after_meta = fs::symlink_metadata(&file)
                .map_err(|error| Error::new("Error", error.to_string()))?;
            let after_info = paths::file_information(&file)?;
            let meta = handle
                .metadata()
                .map_err(|error| Error::new("Error", error.to_string()))?;
            if secure::linked(&after_meta)
                || info.identity != after_info.identity
                || before_info.is_some_and(|before| before.identity != info.identity)
                || !meta.is_file()
                || info.links != 1
            {
                return Err(debug_log_error(
                    "Debug log must be a regular, single-link file",
                ));
            }
            handle
                .write_all(format!("{}\n", row.json()).as_bytes())
                .map_err(|error| Error::new("Error", error.to_string()))?;
            self.latest_path = Some(file);
            self.latest_root = Some(repo_root.to_path_buf());
            Ok(())
        })();
        write_result.map_err(|error| {
            if error.code == Some("ROSTER_RUN_LOG") {
                error
            } else {
                debug_log_error("Could not write debug log").caused_by(error)
            }
        })
    }

    pub fn tail(&self, limit: usize) -> Result<Option<(PathBuf, Vec<String>)>, Error> {
        if !(1..=200).contains(&limit) {
            return Err(Error::type_error("Debug tail limit must be 1-200"));
        }
        let (Some(latest_path), Some(latest_root)) = (&self.latest_path, &self.latest_root) else {
            return Ok(None);
        };
        paths::ensure_local_path(latest_path, latest_root)?;
        let meta = fs::symlink_metadata(latest_path)
            .map_err(|_| debug_log_error("Debug log must be a regular, single-link file"))?;
        if secure::linked(&meta) || !meta.is_file() {
            return Err(debug_log_error(
                "Debug log must be a regular, single-link file",
            ));
        }
        let before_info = paths::file_information(latest_path)
            .map_err(|_| debug_log_error("Debug log must be a regular, single-link file"))?;
        let mut handle = fs::File::open(latest_path)
            .map_err(|_| debug_log_error("Debug log must be a regular, single-link file"))?;
        let info = paths::handle_information(&handle)
            .map_err(|_| debug_log_error("Debug log must be a regular, single-link file"))?;
        let after_meta = fs::symlink_metadata(latest_path)
            .map_err(|_| debug_log_error("Debug log must be a regular, single-link file"))?;
        let handle_meta = handle
            .metadata()
            .map_err(|_| debug_log_error("Debug log must be a regular, single-link file"))?;
        if secure::linked(&after_meta)
            || info.identity != before_info.identity
            || !handle_meta.is_file()
            || info.links != 1
        {
            return Err(debug_log_error(
                "Debug log must be a regular, single-link file",
            ));
        }
        let total_size = handle_meta.len();
        let read_size = total_size.min(262_144);
        let offset = total_size.saturating_sub(read_size);
        handle
            .seek(SeekFrom::Start(offset))
            .map_err(|_| debug_log_error("Invalid debug log metadata"))?;
        let mut bytes = Vec::with_capacity(read_size as usize);
        handle
            .take(read_size)
            .read_to_end(&mut bytes)
            .map_err(|_| debug_log_error("Invalid debug log metadata"))?;
        let text =
            String::from_utf8(bytes).map_err(|_| debug_log_error("Invalid debug log metadata"))?;
        let mut lines = text.split('\n').map(str::to_owned).collect::<Vec<_>>();
        let _ = lines.pop();
        if total_size > read_size && !lines.is_empty() {
            lines.remove(0);
        }
        let start = lines.len().saturating_sub(limit);
        let selected = lines[start..].to_vec();
        for line in &selected {
            let row = json::parse(&Text::from(line.as_str()))
                .map_err(|_| debug_log_error("Invalid debug log metadata"))?;
            if !valid_debug_row(&row) {
                return Err(debug_log_error("Invalid debug log metadata"));
            }
            if let Some(reason) = row.get("finish_reason").filter(|v| **v != Value::Null) {
                if self.safe_reason(Some(reason))? != *reason {
                    return Err(debug_log_error("Invalid debug log metadata"));
                }
            }
        }
        Ok(Some((latest_path.clone(), selected)))
    }
}

pub fn read_diff_status(cwd: &Path, untracked: bool) -> Result<String, Error> {
    if cwd.as_os_str().is_empty() {
        return Err(Error::type_error(
            "Diff status requires a working directory, boolean untracked option, and command runner",
        ));
    }
    let env = bench_git_env();
    let mode = if untracked {
        "--untracked-files=all"
    } else {
        "--untracked-files=no"
    };
    let output = StdCommand::new(git_executable())
        .current_dir(cwd)
        .env_clear()
        .envs(&env)
        .args(["status", "--short", "--porcelain=v1", mode])
        .output()
        .map_err(|error| Error::new("Error", error.to_string()))?;
    if !output.status.success() {
        return Err(Error::new(
            "Error",
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| Error::new("Error", error.to_string()))
}

pub fn ensure_managed_ignored(worktree: &Path, pattern: &str) -> Result<(), Error> {
    if ![".roster/checkpoints/", ".roster/map.md"].contains(&pattern) {
        return Err(Error::type_error("Unsupported managed ignore pattern"));
    }
    let has_git = match fs::symlink_metadata(worktree.join(".git")) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(Error::new("Error", error.to_string())),
    };
    if !has_git {
        return Ok(());
    }
    let env = bench_git_env();
    let out = StdCommand::new(git_executable())
        .current_dir(worktree)
        .env_clear()
        .envs(&env)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .output()
        .map_err(|error| Error::new("Error", error.to_string()))?;
    if !out.status.success() {
        return Err(Error::new(
            "Error",
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    let rel_exclude =
        String::from_utf8(out.stdout).map_err(|error| Error::new("Error", error.to_string()))?;
    let file = paths::resolve(worktree, Path::new(rel_exclude.trim()))?;
    let existing_meta = match fs::symlink_metadata(&file) {
        Ok(entry) => Some(entry),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(Error::new("Error", error.to_string())),
    };
    if let Some(entry) = &existing_meta {
        let links = paths::file_information(&file).map_or(0, |info| info.links);
        if !entry.is_file() || secure::linked(entry) || links != 1 {
            return Err(Error::new(
                "Error",
                "Git exclude must be a regular single-link file before storing managed artifacts",
            ));
        }
    }
    let source = if existing_meta.is_some() {
        fs::read_to_string(&file).map_err(|error| Error::new("Error", error.to_string()))?
    } else {
        String::new()
    };
    if !source
        .replace("\r\n", "\n")
        .split('\n')
        .any(|line| line == pattern)
    {
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|error| Error::new("Error", error.to_string()))?;
        }
        let mut open = OpenOptions::new();
        open.write(true).append(true).create(true);
        let mut handle = open
            .open(&file)
            .map_err(|error| Error::new("Error", error.to_string()))?;
        let prefix = if source.is_empty() || source.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        handle
            .write_all(format!("{prefix}{pattern}\n").as_bytes())
            .map_err(|error| Error::new("Error", error.to_string()))?;
    }
    Ok(())
}

pub fn write_repo_map(
    worktree: &Path,
    env: Option<&Value>,
    api_key_env: Option<&str>,
) -> Result<(PathBuf, usize), Error> {
    let Some(task_source) = crate::planning::artifacts::read(worktree, "TASK.md")? else {
        return Err(Error::new(
            "Error",
            "A current TASK.md is required before /map.",
        ));
    };
    let named = crate::planning::task::files_allowed(&Value::String(task_source))?;
    let redaction_cfg = redaction::Config {
        environment: env
            .cloned()
            .unwrap_or_else(crate::ask::environment_value_from_process),
        api_key_env: api_key_env.map_or_else(|| Text::from("ROSTER_API_KEY"), Text::from),
        ..redaction::Config::default()
    };
    redaction_cfg.validate()?;
    let mut rows = vec![
        "# Repository map (filenames only)".to_owned(),
        String::new(),
        "## TASK paths".to_owned(),
    ];
    for file in &named {
        rows.push(format!("- {file}"));
    }
    rows.push(String::new());
    rows.push("## Top two directory levels".to_owned());
    let mut truncated = false;

    fn visit(
        worktree: &Path,
        directory: &str,
        depth: usize,
        redaction_cfg: &redaction::Config,
        rows: &mut Vec<String>,
        truncated: &mut bool,
    ) -> Result<(), Error> {
        let dir_path = if directory.is_empty() {
            worktree.to_path_buf()
        } else {
            worktree.join(directory)
        };
        let mut entries = Vec::new();
        for item in
            fs::read_dir(&dir_path).map_err(|error| Error::new("Error", error.to_string()))?
        {
            let item = item.map_err(|error| Error::new("Error", error.to_string()))?;
            let Ok(name) = item.file_name().into_string() else {
                continue;
            };
            let Ok(meta) = fs::symlink_metadata(dir_path.join(&name)) else {
                continue;
            };
            if secure::linked(&meta) || (!meta.is_file() && !meta.is_dir()) {
                continue;
            }
            entries.push((name, meta.is_dir()));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, is_dir) in entries {
            let file = if directory.is_empty() {
                name.clone()
            } else {
                format!("{directory}/{name}")
            };
            let first_seg = file.split('/').next().unwrap_or("");
            let redacted = redaction::evidence(&Text::from(file.as_str()), redaction_cfg)
                .to_utf8()
                .unwrap_or_default();
            if crate::planning::forbidden_read(&file)
                || crate::execution::policy::managed(&file)
                || matches!(first_seg, ".roster" | ".worktrees" | "node_modules")
                || redacted != file
            {
                continue;
            }
            let line = format!("- {file}{}", if is_dir { "/" } else { "" });
            let candidate_bytes = rows.iter().map(|r| r.len() + 1).sum::<usize>() + line.len() + 1;
            if rows.len() == 79 || candidate_bytes > 32_000 {
                *truncated = true;
                return Ok(());
            }
            rows.push(line);
            if is_dir && depth < 1 {
                visit(worktree, &file, depth + 1, redaction_cfg, rows, truncated)?;
            }
            if *truncated {
                return Ok(());
            }
        }
        Ok(())
    }

    visit(worktree, "", 0, &redaction_cfg, &mut rows, &mut truncated)?;
    if truncated {
        rows.push("- (additional filenames omitted at the 80-line cap)".to_owned());
    }
    let content = format!("{}\n", rows.join("\n"));
    let file = worktree.join(".roster").join("map.md");
    paths::ensure_local_path(&file, worktree)?;
    ensure_managed_ignored(worktree, ".roster/map.md")?;
    if let Some(parent) = file.parent() {
        files::create_private_directories(parent)?;
    }
    let before_info = match fs::symlink_metadata(&file) {
        Ok(entry) => {
            let links = paths::file_information(&file).map_or(0, |info| info.links);
            if !entry.is_file() || secure::linked(&entry) || links != 1 {
                return Err(Error::new(
                    "Error",
                    "Repo map must be a regular single-link file",
                ));
            }
            Some(paths::file_information(&file)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(Error::new("Error", error.to_string())),
    };
    let mut open = OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    let mut handle = open
        .open(&file)
        .map_err(|error| Error::new("Error", error.to_string()))?;
    let info = paths::handle_information(&handle)?;
    let after_meta =
        fs::symlink_metadata(&file).map_err(|error| Error::new("Error", error.to_string()))?;
    if secure::linked(&after_meta)
        || !after_meta.is_file()
        || info.links != 1
        || before_info.is_some_and(|before| before.identity != info.identity)
    {
        return Err(Error::new(
            "Error",
            "Repo map must be a regular single-link file",
        ));
    }
    handle
        .write_all(content.as_bytes())
        .and_then(|()| handle.sync_all())
        .map_err(|error| Error::new("Error", error.to_string()))?;
    Ok((file, rows.len()))
}

pub fn read_repo_map(
    worktree: &Path,
    env: Option<&Value>,
    api_key_env: Option<&str>,
) -> Result<Option<String>, Error> {
    let file = worktree.join(".roster").join("map.md");
    paths::ensure_local_path(&file, worktree)?;
    let entry = match fs::symlink_metadata(&file) {
        Ok(entry) => entry,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::new("Error", error.to_string())),
    };
    let links = paths::file_information(&file).map_or(0, |info| info.links);
    if !entry.is_file() || secure::linked(&entry) || links != 1 || entry.len() > 32_768 {
        return Err(Error::new(
            "Error",
            "Repo map must be a bounded regular single-link file",
        ));
    }
    let Some(snapshot) = secure::read(&file, 32_768)? else {
        return Ok(None);
    };
    let content = String::from_utf8(snapshot.bytes)
        .map_err(|_| Error::new("Error", "Repo map metadata is invalid"))?;
    let redaction_cfg = redaction::Config {
        environment: env
            .cloned()
            .unwrap_or_else(crate::ask::environment_value_from_process),
        api_key_env: api_key_env.map_or_else(|| Text::from("ROSTER_API_KEY"), Text::from),
        ..redaction::Config::default()
    };
    redaction_cfg.validate()?;
    let redacted = redaction::evidence(&Text::from(content.as_str()), &redaction_cfg).to_utf8()?;
    let trimmed_lines = content.trim_end().split('\n').collect::<Vec<_>>();
    if trimmed_lines.len() > 80
        || !content.starts_with("# Repository map (filenames only)\n")
        || redacted != content
    {
        return Err(Error::new("Error", "Repo map metadata is invalid"));
    }
    if trimmed_lines.iter().any(|line| {
        !line.is_empty()
            && ![
                "# Repository map (filenames only)",
                "## TASK paths",
                "## Top two directory levels",
            ]
            .contains(line)
            && !line.starts_with("- ")
    }) {
        return Err(Error::new(
            "Error",
            "Repo map may contain only filename metadata",
        ));
    }
    Ok(Some(content))
}

pub const RUNTIME_STATE_PATTERNS: &[&str] = &[
    ".roster/runs/",
    ".roster/logs/",
    ".roster/history",
    ".roster/history.*",
    ".roster/checkpoints/",
    ".roster/locks/",
    ".roster/map.md",
    ".roster/evals.jsonl",
    ".roster/memory/",
    ".roster/asks/",
    ".roster-state/",
    "/evals.jsonl",
];

pub const PRIVATE_SETUP_PATTERNS: &[&str] = &[
    ".roster/config.yml",
    ".roster/config.yml.*",
    ".roster/fleet.yml",
    ".roster/fleet.yml.*",
    ".roster/capabilities.yml",
    ".roster/capabilities.yml.*",
];

pub fn check_gitignore_coverage(repo_root: &Path) -> Result<Vec<&'static str>, Error> {
    let file = repo_root.join(".gitignore");
    let source = fs::read_to_string(&file).map_err(|_| {
        Error::new(
            "RosterScopeError",
            format!(
                "missing .gitignore at {}; runtime state paths must be ignored before first write.",
                file.display()
            ),
        )
        .coded("E_ROSTER_SCOPE")
    })?;
    let lines = source
        .replace("\r\n", "\n")
        .split('\n')
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let all_patterns = RUNTIME_STATE_PATTERNS
        .iter()
        .chain(PRIVATE_SETUP_PATTERNS.iter())
        .copied()
        .collect::<Vec<_>>();
    let mut missing = Vec::new();
    for pattern in &all_patterns {
        if lines.contains(*pattern) {
            continue;
        }
        let parent = pattern
            .trim_end_matches('/')
            .rsplit_once('/')
            .map_or(".", |(dir, _)| dir);
        if parent == "." || !lines.contains(&format!("{parent}/")) {
            missing.push(*pattern);
        }
    }
    if !missing.is_empty() {
        return Err(Error::new(
            "RosterScopeError",
            format!(
                ".gitignore does not cover runtime state paths: {}. Add them so runtime state is never committed.",
                missing.join(", ")
            ),
        )
        .coded("E_ROSTER_SCOPE"));
    }
    Ok(all_patterns)
}

pub fn check_staged_runtime_state(repo_root: &Path) -> Result<usize, Error> {
    let env = bench_git_env();
    let out = StdCommand::new(git_executable())
        .current_dir(repo_root)
        .env_clear()
        .envs(&env)
        .args(["--no-pager", "ls-files", "-z"])
        .output()
        .map_err(|error| Error::new("Error", error.to_string()))?;
    if !out.status.success() {
        return Err(Error::new(
            "Error",
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    let tracked =
        String::from_utf8(out.stdout).map_err(|error| Error::new("Error", error.to_string()))?;
    let mut candidates = Vec::new();
    for item in tracked.split('\0').filter(|s| !s.is_empty()) {
        if !candidates.contains(&item) {
            candidates.push(item);
        }
    }
    let mut offenders = Vec::new();
    for file in &candidates {
        let normalized = file.replace('\\', "/");
        let is_runtime = RUNTIME_STATE_PATTERNS.iter().any(|pattern| {
            let p = pattern.strip_prefix('/').unwrap_or(pattern);
            if p.ends_with('/') {
                normalized.starts_with(p)
            } else if let Some((base, _)) = p.split_once('*') {
                normalized == p || normalized.starts_with(base)
            } else {
                normalized == p
            }
        });
        if is_runtime {
            offenders.push(normalized);
        }
    }
    if !offenders.is_empty() {
        return Err(Error::new(
            "RosterScopeError",
            format!(
                "runtime state must never be committed; untrack: {}. Remove it from the index with `git rm --cached` and keep the .gitignore rules.",
                offenders.join(", ")
            ),
        )
        .coded("E_ROSTER_SCOPE"));
    }
    Ok(candidates.len())
}

pub fn check_roster_scope(repo_root: &Path) -> Result<(Vec<&'static str>, usize), Error> {
    let patterns = check_gitignore_coverage(repo_root)?;
    let checked = check_staged_runtime_state(repo_root)?;
    Ok((patterns, checked))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLogSnapshot {
    pub path: PathBuf,
    pub session: String,
    pub last_seat: String,
    pub last_line: String,
    pub last_error_class: Option<String>,
    pub lines: Vec<String>,
}

fn valid_run_log_metadata(value: &str) -> bool {
    if value == "steering coder" || value == "timeout retry 1/1" {
        return true;
    }
    let patterns = [
        r#"^(?:session=[A-Za-z0-9._\[\]-]{1,128}|model="(?:[^"\\]|\\.)*" host="(?:[^"\\]|\\.)*"|mode (?:stub|llm)|waiting host=[A-Za-z0-9.:\[\]-]{1,255} elapsed=\d+s(?: cold-start up to 15m)?(?: queued depth=\d+)?|The LLM request timed out after \d+(?:\.\d+)?s at host=[A-Za-z0-9.:\[\]-]{1,255}\. (?:Cold-start: the host may still be warming; Spark/SGLang can take up to 15m\. )?This is an endpoint timeout, not a bad TASK\. (?:Retry: (?:roster run --issue [1-9]\d*(?: --auto-model)?|roster run --seat coder --runtime builtin|roster doctor --warm)|Retry the same request\.)|implementation (?:model|deterministic-readme)|http chat\.completions (?:start|ok status=2\d\d|error(?: status=[1-5]\d\d)? class=(?:authentication|network|timeout|http|response|abort))|tool (?:read_file|write_file|list_dir|run_test|search_text)(?: path="(?:[^"\\]|\\.)*")?|wrote (?:RECIPE\.yml|TASK\.md|PLAN\.md|ESTIMATE\.md|RESULT\.md|REVIEW\.md)|error class=(?:Error|TypeError|RangeError|AbortError|RunLogError)|elapsed_ms=\d+ mode=(?:stub|llm))$"#,
        r#"^stall host=[A-Za-z0-9.:\[\]-]{1,255} idle=\d+(?:\.\d+)?s retry=(?:true|false)$"#,
        r#"^served-model host=[A-Za-z0-9.:\[\]-]{1,255} requested=[A-Za-z0-9._:/@+-]{1,128} served=[A-Za-z0-9._:/@+-]{1,128}$"#,
        r#"^tool refused (?:read_file|write_file|edit_file|delete_file|list_dir|run_test|search_text) (?:outside-worktree|absolute-inside)$"#,
        r#"^toolset (?:-|(?:read_file|write_file|edit_file|delete_file|glob_files|list_dir|run_test|run_command|search_text|web_search|web_fetch|update_checklist)(?:,(?:read_file|write_file|edit_file|delete_file|glob_files|list_dir|run_test|run_command|search_text|web_search|web_fetch|update_checklist))*)$"#,
        r#"^completion finish_reason=(?:null|"stop"|"tool_calls")$"#,
        r#"^checklist \d+/\d+(?: open-at-finish)?$"#,
        r#"^red-green (?:checked|none|exempt|skipped|unavailable) tests=\d+ not-red=\d+$"#,
        r#"^shadow-modules (?:none|checked|flagged|unavailable) findings=\d+$"#,
        r#"^lifecycle-hook (?:pre-plan|post-coder|pre-publish) \.roster/hooks/[A-Za-z0-9_-]+\.[cm]?js (?:pass|fail|timeout|output-limit) ms=\d+$"#,
        r#"^review-e2e (?:pass|fail) commands=\d+ failures=\d+$"#,
        r#"^review-reads count=\d+ refused=\d+$"#,
        r#"^self-review (?:clean|findings|unavailable) unmet=\d+ findings=\d+ ms=\d+ in=\d+ out=\d+$"#,
    ];
    patterns
        .iter()
        .any(|pat| regex_lite::Regex::new(pat).is_ok_and(|re| re.is_match(value)))
}

pub fn read_last_run_log(
    repo_root: &Path,
    session: &str,
    env: Option<&Value>,
    api_key_env: Option<&str>,
    limit: usize,
) -> Result<Option<RunLogSnapshot>, Error> {
    if !(1..=200).contains(&limit) {
        return Err(Error::type_error("Run log tail limit must be 1-200"));
    }
    let valid_session = regex_lite::Regex::new(r"^[A-Za-z0-9._-]{1,64}$")
        .map_err(|error| Error::new("Error", error.to_string()))?
        .is_match(session);
    if !valid_session {
        return Err(Error::type_error(
            "Run log session must be an opaque identifier of at most 64 characters",
        ));
    }
    let runs_dir = layout::state_path(
        layout::resolve_directory(
            layout::Scope::PerWorktree,
            repo_root,
            Some(repo_root),
            Some(".roster"),
        )?,
        &["runs"],
    )?
    .path;
    let file = runs_dir.join(format!("{session}.log"));
    paths::ensure_local_path(&file, repo_root)?;
    let meta = match fs::symlink_metadata(&file) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(
                Error::new("RunLogError", "Could not read live run log").coded("ROSTER_RUN_LOG")
            );
        }
    };
    if !meta.is_file() || secure::linked(&meta) {
        return Err(
            Error::new("RunLogError", "Run log must be a regular, single-link file")
                .coded("ROSTER_RUN_LOG"),
        );
    }
    let before_info = paths::file_information(&file).map_err(|_| {
        Error::new("RunLogError", "Could not read live run log").coded("ROSTER_RUN_LOG")
    })?;
    if before_info.links != 1 {
        return Err(
            Error::new("RunLogError", "Run log must be a regular, single-link file")
                .coded("ROSTER_RUN_LOG"),
        );
    }
    let mut handle = fs::File::open(&file).map_err(|_| {
        Error::new("RunLogError", "Could not read live run log").coded("ROSTER_RUN_LOG")
    })?;
    let handle_info = paths::handle_information(&handle).map_err(|_| {
        Error::new("RunLogError", "Could not read live run log").coded("ROSTER_RUN_LOG")
    })?;
    let after_meta = handle.metadata().map_err(|_| {
        Error::new("RunLogError", "Could not read live run log").coded("ROSTER_RUN_LOG")
    })?;
    if !after_meta.is_file()
        || secure::linked(&after_meta)
        || handle_info.links != 1
        || handle_info.identity != before_info.identity
    {
        return Err(
            Error::new("RunLogError", "Run log must be a regular, single-link file")
                .coded("ROSTER_RUN_LOG"),
        );
    }
    let file_size = after_meta.len();
    let read_size = file_size.min(65_536);
    handle
        .seek(SeekFrom::Start(file_size - read_size))
        .map_err(|_| {
            Error::new("RunLogError", "Could not read live run log").coded("ROSTER_RUN_LOG")
        })?;
    let mut buf = vec![0_u8; read_size as usize];
    let bytes_read = handle.read(&mut buf).map_err(|_| {
        Error::new("RunLogError", "Could not read live run log").coded("ROSTER_RUN_LOG")
    })?;
    let text = String::from_utf8_lossy(&buf[..bytes_read]);
    let mut lines = text.split('\n').collect::<Vec<_>>();
    lines.pop();
    let Some(&last_line) = lines.last() else {
        return Ok(None);
    };
    if last_line.is_empty() {
        return Ok(None);
    }
    let line_re = regex_lite::Regex::new(
        r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z (?:start seat|seat) (planner|coder|reviewer) (.+)$",
    )
    .map_err(|error| Error::new("Error", error.to_string()))?;
    let parse_valid = |line: &str| -> Option<(String, String)> {
        if line.len() > 2048
            || line
                .chars()
                .any(|ch| matches!(ch, '\x00'..='\x1f' | '\x7f'))
        {
            return None;
        }
        let caps = line_re.captures(line)?;
        let seat = caps.get(1)?.as_str().to_owned();
        let meta_str = caps.get(2)?.as_str().to_owned();
        if !valid_run_log_metadata(&meta_str) {
            return None;
        }
        Some((seat, meta_str))
    };
    let Some((last_seat, _)) = parse_valid(last_line) else {
        return Err(
            Error::new("RunLogError", "Last live run log line has invalid metadata")
                .coded("ROSTER_RUN_LOG"),
        );
    };
    let valid_lines = lines
        .into_iter()
        .filter(|line| parse_valid(line).is_some())
        .collect::<Vec<_>>();
    let class_re = regex_lite::Regex::new(r"\bclass=([A-Za-z]+)")
        .map_err(|error| Error::new("Error", error.to_string()))?;
    let last_error_class = valid_lines.iter().rev().find_map(|line| {
        class_re
            .captures(line)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_owned()))
    });
    let env_value = env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    let key_text = api_key_env.map(Text::from);
    let redactor = Redactor::new(&env_value, key_text.as_ref());
    let start_idx = valid_lines.len().saturating_sub(limit);
    let mut tail_lines = Vec::with_capacity(valid_lines.len() - start_idx);
    for line in &valid_lines[start_idx..] {
        tail_lines.push(redactor.redact(&Text::from(*line), None).to_utf8()?);
    }
    let redacted_last = redactor.redact(&Text::from(last_line), None).to_utf8()?;
    Ok(Some(RunLogSnapshot {
        path: file,
        session: session.to_owned(),
        last_seat,
        last_line: redacted_last,
        last_error_class,
        lines: tail_lines,
    }))
}

pub fn read_issue_logs(
    repo_root: &Path,
    issue: u64,
    env: Option<&Value>,
    api_key_env: Option<&str>,
    limit: usize,
) -> Result<Vec<RunLogSnapshot>, Error> {
    if !(1..=crate::MAX_SAFE_INTEGER).contains(&issue) {
        return Err(Error::type_error(
            "Issue log requires a positive issue number",
        ));
    }
    if !(1..=200).contains(&limit) {
        return Err(Error::type_error("Run log tail limit must be 1-200"));
    }
    let runs_dir = layout::state_path(
        layout::resolve_directory(
            layout::Scope::PerWorktree,
            repo_root,
            Some(repo_root),
            Some(".roster"),
        )?,
        &["runs"],
    )?
    .path;
    paths::ensure_local_path(&runs_dir, repo_root)?;
    let read_dir = match fs::read_dir(&runs_dir) {
        Ok(rd) => rd,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::new("Error", error.to_string())),
    };
    let pat = regex_lite::Regex::new(&format!(r"^roster-{issue}-[A-Za-z0-9._-]+\.log$"))
        .map_err(|error| Error::new("Error", error.to_string()))?;
    let mut matching = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|error| Error::new("Error", error.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if pat.is_match(&name) {
            let meta = fs::symlink_metadata(entry.path())
                .map_err(|error| Error::new("Error", error.to_string()))?;
            if !meta.is_file() || secure::linked(&meta) {
                return Err(
                    Error::new("RunLogError", "Issue log must be a regular file")
                        .coded("ROSTER_RUN_LOG"),
                );
            }
            matching.push(name);
        }
    }
    matching.sort();
    let mut logs = Vec::new();
    for name in matching {
        let session = &name[..name.len() - 4];
        if let Some(log) = read_last_run_log(repo_root, session, env, api_key_env, limit)? {
            logs.push(log);
        }
    }
    Ok(logs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_err(message: impl Into<String>) -> Error {
        Error::new("TestError", message)
    }

    fn temp_dir(prefix: &str) -> Result<PathBuf, Error> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let seq = DIAG_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "roster-diag-test-{prefix}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        files::create_private_directories(&dir).map_err(|error| test_err(error.message))?;
        Ok(dir)
    }

    struct TempCleanup(PathBuf);

    impl Drop for TempCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn bench_writes_six_numeric_secret_free_timings() -> Result<(), Error> {
        let dir = temp_dir("bench")?;
        let _cleanup = TempCleanup(dir.clone());
        let env = Value::object(vec![
            ("ROSTER_API_KEY", Value::string("bench-secret-token")),
            (
                "GITHUB_APP_PRIVATE_KEY_PATH",
                Value::string("private-key-path.pem"),
            ),
        ]);
        let report = run_bench(&dir, &dir, &env, 4.25)?;
        assert_eq!(report.command_dispatch_ms, 4.25);
        let raw = fs::read_to_string(dir.join(".roster").join("bench.json"))
            .map_err(|error| test_err(error.to_string()))?;
        assert!(!raw.contains("bench-secret-token"));
        assert!(!raw.contains("private-key-path"));
        let parsed = json::parse(&Text::from(raw.as_str()))?;
        let entries = domain::members(&parsed).ok_or_else(|| test_err("object"))?;
        let keys = entries
            .iter()
            .map(|(k, _)| k.to_utf8())
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(keys, BENCH_FIELDS);
        for (_, val) in entries {
            let num = val.as_number().ok_or_else(|| test_err("number"))?;
            assert!(num.is_finite() && num >= 0.0);
        }
        Ok(())
    }

    #[test]
    fn debug_log_redacts_secrets_verifies_git_ignore_and_rejects_tampering() -> Result<(), Error> {
        assert_eq!(debug_path_class(Some("/etc/passwd")), Some("outside"));
        assert_eq!(
            debug_path_class(Some("/outside/README.md")),
            Some("outside")
        );
        assert_eq!(debug_path_class(Some("C:foo")), Some("other"));
        let sk_err = DebugLog::new(None, Some(true), Some("sk-12345678901234567890"))
            .err()
            .ok_or_else(|| test_err("20-char sk- session must fail"))?;
        assert!(
            sk_err
                .message
                .contains("Debug session must not contain environment values")
        );

        let root = temp_dir("debug")?;
        let _cleanup = TempCleanup(root.clone());
        let env = Value::object(vec![
            ("TEST_VALUE", Value::string("ENV_VALUE_DO_NOT_LOG")),
            (
                "GITHUB_APP_PRIVATE_KEY_PATH",
                Value::string("C:\\private\\secret.pem"),
            ),
        ]);
        let mut debug = DebugLog::new(Some(&env), Some(true), Some("privacy-test"))?;
        debug.record(
            &root,
            None,
            "coder",
            &Value::object(vec![
                ("type", Value::string("tool")),
                ("name", Value::string("read_file")),
                ("path", Value::string("C:\\private\\secret.pem")),
                ("prompt", Value::string("PRIVATE_PROMPT")),
            ]),
        )?;
        debug.record(
            &root,
            None,
            "coder",
            &Value::object(vec![
                ("type", Value::string("finish-reason")),
                ("reason", Value::string("ENV_VALUE_DO_NOT_LOG")),
                ("retry", Value::Boolean(false)),
            ]),
        )?;
        debug.record(
            &root,
            None,
            "coder",
            &Value::object(vec![
                ("type", Value::string("tool-result")),
                ("name", Value::string("run_test")),
                ("status", Value::string("ok")),
                ("exit_code", Value::Number(1.0)),
                ("stdout", Value::string("PRIVATE_TEST_OUTPUT")),
            ]),
        )?;
        debug.record(
            &root,
            None,
            "coder",
            &Value::object(vec![
                ("type", Value::string("test-repair")),
                ("attempt", Value::Number(1.0)),
                ("budget", Value::Number(4.0)),
            ]),
        )?;
        let path = debug.path().ok_or_else(|| test_err("path"))?.to_path_buf();
        let text = fs::read_to_string(&path).map_err(|error| test_err(error.to_string()))?;
        assert!(!text.contains("PRIVATE_"));
        assert!(!text.contains("ENV_VALUE_DO_NOT_LOG"));
        assert!(!text.contains("secret.pem"));
        let (_, tail_lines) = debug.tail(50)?.ok_or_else(|| test_err("tail"))?;
        assert_eq!(tail_lines.len(), 4);

        let mut tampered = json::parse(&Text::from(tail_lines[0].as_str()))?;
        domain::put(&mut tampered, "prompt", Value::string("PRIVATE_BODY"))?;
        fs::write(&path, format!("{}\n", tampered.json()))
            .map_err(|error| test_err(error.to_string()))?;
        let err = debug
            .tail(50)
            .err()
            .ok_or_else(|| test_err("tampered must fail"))?;
        assert_eq!(err.code, Some("ROSTER_RUN_LOG"));
        assert!(!err.message.contains("PRIVATE_BODY"));

        let tracked_root = temp_dir("tracked-debug")?;
        let _cleanup_tracked = TempCleanup(tracked_root.clone());
        let git_env = bench_git_env();
        run_git_sync(&tracked_root, &["init", "--quiet"], &git_env)?;
        fs::write(tracked_root.join(".gitignore"), ".roster/logs/\n")
            .map_err(|error| test_err(error.to_string()))?;
        let logs = tracked_root.join(".roster").join("logs");
        files::create_private_directories(&logs)?;
        let tracked_file = logs.join("debug-tracked-test.jsonl");
        fs::write(&tracked_file, "{}\n").map_err(|error| test_err(error.to_string()))?;
        run_git_sync(
            &tracked_root,
            &[
                "add",
                "--force",
                "--",
                ".roster/logs/debug-tracked-test.jsonl",
            ],
            &git_env,
        )?;
        let mut tracked_debug = DebugLog::new(None, Some(true), Some("tracked-test"))?;
        let err = tracked_debug
            .record(
                &tracked_root,
                None,
                "coder",
                &Value::object(vec![("type", Value::string("seat-start"))]),
            )
            .err()
            .ok_or_else(|| test_err("tracked debug file must fail"))?;
        assert!(err.message.contains("untracked and gitignored"));

        let map_root = temp_dir("repo-map")?;
        let _cleanup_map = TempCleanup(map_root.clone());
        run_git_sync(&map_root, &["init", "--quiet"], &git_env)?;
        let stub = crate::planning::artifacts::stub(
            &Value::string("Update README.md."),
            "issue:42",
            None,
            None,
        )?;
        fs::write(map_root.join("TASK.md"), stub.task.to_utf8()?)
            .map_err(|error| test_err(error.to_string()))?;
        fs::write(map_root.join("README.md"), "# hello\n")
            .map_err(|error| test_err(error.to_string()))?;
        fs::write(map_root.join(".env"), "SECRET=1\n")
            .map_err(|error| test_err(error.to_string()))?;
        let (map_path, line_count) =
            write_repo_map(&map_root, Some(&Value::object(Vec::new())), None)?;
        assert!(map_path.exists());
        assert!(line_count >= 5);
        let exclude_content =
            fs::read_to_string(map_root.join(".git").join("info").join("exclude"))
                .map_err(|error| test_err(error.to_string()))?;
        assert!(exclude_content.contains(".roster/map.md"));
        let map_content = read_repo_map(&map_root, Some(&Value::object(Vec::new())), None)?
            .ok_or_else(|| test_err("expected repo map"))?;
        assert!(map_content.contains("- README.md"));
        assert!(!map_content.contains(".env"));

        let runs_dir = map_root.join(".roster").join("runs");
        fs::create_dir_all(&runs_dir).map_err(|error| test_err(error.to_string()))?;
        fs::write(
            runs_dir.join("roster-42-coder.log"),
            "2026-10-19T12:00:00.000Z start seat coder session=roster-42-coder\n2026-10-19T12:00:01.000Z seat coder error class=TypeError\n2026-10-19T12:00:02.000Z seat coder lifecycle-hook post-coder .roster/hooks/check.mjs pass ms=12\n",
        )
        .map_err(|error| test_err(error.to_string()))?;
        let snap = read_last_run_log(
            &map_root,
            "roster-42-coder",
            Some(&Value::object(Vec::new())),
            None,
            2,
        )?
        .ok_or_else(|| test_err("expected run log snapshot"))?;
        assert_eq!(snap.last_seat, "coder");
        assert_eq!(snap.last_error_class.as_deref(), Some("TypeError"));
        assert_eq!(snap.lines.len(), 2);
        let issue_logs = read_issue_logs(&map_root, 42, Some(&Value::object(Vec::new())), None, 5)?;
        assert_eq!(issue_logs.len(), 1);
        Ok(())
    }
}
