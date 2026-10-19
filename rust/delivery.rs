use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::cancel::CancellationToken;
use crate::config::{self, Loaded};
use crate::domain;
use crate::execution::{self, backend, evidence, finalization, hooks, tools};
use crate::json::Value;
use crate::llm::admission::Admission;
use crate::llm::control::Observer;
use crate::llm::http::Transport;
use crate::llm::openai::{Credentials, LastResponse};
use crate::paths;
use crate::planning::{self, artifacts, estimate, intake, lifecycle, task};
use crate::run::RunRecord;
use crate::seats::{self, coder, reviewer};
use crate::start;
use crate::state::{files, layout, locks, redaction, secure};
use crate::text::Text;

mod continuation;

pub use continuation::{
    Counts, History, MAX_PERSPECTIVES, MAX_RESCOPES, MAX_REVIEW_REPAIRS, SeatAttempt,
};

#[derive(Debug, Clone)]
pub enum Input {
    ExistingTask,
    AskFile(PathBuf),
}

pub struct Options<'a> {
    pub cwd: &'a Path,
    pub installation: &'a Path,
    pub loaded: &'a Loaded,
    pub input: Input,
    pub host: tools::Host,
    pub approval: backend::Approval,
    pub start: start::Options,
    pub fetch: start::FetchPolicy,
    pub skip_review: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Verified,
    Stub,
    PlanningOnly,
    PlanningEscalation,
    ReviewSkipped,
    Unverified,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Stub => "stub",
            Self::PlanningOnly => "planning-only",
            Self::PlanningEscalation => "planning-escalation",
            Self::ReviewSkipped => "review-skipped",
            Self::Unverified => "unverified",
        }
    }
}

pub struct Outcome {
    status: Status,
    pub worktree: PathBuf,
    pub task: String,
    pub session: String,
    pub ask_kind: intake::Kind,
    pub artifacts: Vec<(&'static str, PathBuf)>,
    pub log: PathBuf,
    pub planner_run: Option<RunRecord>,
    pub coder_run: Option<RunRecord>,
    pub reviewer_run: Option<RunRecord>,
    pub planner_response: Option<LastResponse>,
    pub planner_critic: Vec<Value>,
    pub coder_response: Option<LastResponse>,
    pub reviewer_response: Option<LastResponse>,
    pub coder_result: Option<Value>,
    pub reviewer_report: Option<Value>,
    pub coder_handoff: Option<seats::Handoff>,
    pub continuation: Option<Value>,
    pub orchestration: History,
    pub failure: Option<Error>,
    pub start: Option<start::Start>,
}

impl Outcome {
    pub fn status(&self) -> Status {
        self.status
    }

    pub fn succeeded(&self) -> bool {
        matches!(
            self.status,
            Status::Verified | Status::Stub | Status::PlanningOnly | Status::ReviewSkipped
        ) && self.failure.is_none()
    }

    pub fn format(&self) -> String {
        let mut output = format!(
            "Ask kind: {}\nWorktree: {}\n",
            self.ask_kind.as_str(),
            self.worktree.display()
        );
        if let Some(start) = &self.start {
            output.push_str(&format!("{}\n", start.format()));
        }
        for (name, path) in &self.artifacts {
            output.push_str(&format!("{name}: {}\n", path.display()));
        }
        output.push_str(&format!(
            "Log: {}\nMode: {}\nRecording: session-only; durable history was not recorded\nPublication: unknown; no publisher ran\n",
            self.log.display(),
            self.status.as_str()
        ));
        match self.status {
            Status::Stub => output.push_str("Offline stub: no model calls, product edits or tests; no passing review.\n"),
            Status::PlanningOnly => output.push_str("Planning only: no coder, tests or delivery verdict.\n"),
            Status::PlanningEscalation => output.push_str("Scope needs a new human/planner handoff; immutable TASK checks were not implemented or weakened. No automatic replanning or source restore ran.\n"),
            Status::ReviewSkipped => output.push_str("Review skipped by explicit invocation and private configuration; not publishable.\n"),
            Status::Unverified => output.push_str("Delivery is unverified; retain the worktree and artifacts for human review.\n"),
            Status::Verified => output.push_str("Current owned coder checks and independent read-only review passed at completion.\n"),
        }
        let counts = self.orchestration.counts;
        if counts != Counts::default() {
            output.push_str(&format!(
                "Continuations: {} rescope(s), {} perspective(s), {} review repair(s); fixed profile, same worktree and writer lease.\n",
                counts.rescopes, counts.perspectives, counts.review_repairs
            ));
        }
        if let Some(Value::Array(files)) = self
            .coder_result
            .as_ref()
            .and_then(|result| result.get("scopeFiles"))
        {
            if !files.is_empty() {
                output.push_str(&format!(
                    "Recorded scope expansions: {}\n",
                    Value::Array(files.clone()).json()
                ));
            }
        }
        for archive in &self.orchestration.archives {
            output.push_str(&format!("Artifact archive: {}\n", archive.display()));
        }
        for attempt in &self.orchestration.attempts {
            if let Some(error) = &attempt.continuation_failure {
                output.push_str(&format!("Continuation blocked by {} [{}]; the original seat failure and redacted notification details remain in RESULT.md.\n", error.name, error.code.unwrap_or("unclassified")));
            }
        }
        output
    }
}

pub struct Driver<'a, T: Transport, O: Observer> {
    pub transport: &'a mut T,
    pub observer: &'a mut O,
    pub credentials: &'a Credentials,
    pub admission: &'a Admission,
}

#[derive(Default)]
pub struct Ownership {
    preparation: start::Preparation,
    boundary: Option<tools::Boundary>,
    git: Option<start::Git>,
    bootstrap_root: Option<PathBuf>,
    handoff_pending: bool,
    timings: execution::timing::Timings,
}

impl Ownership {
    pub fn timings(&self) -> execution::timing::Timings {
        self.timings.clone()
    }

    pub fn root(&self) -> Option<&Path> {
        self.boundary
            .as_ref()
            .map(tools::Boundary::root)
            .or_else(|| self.preparation.root())
            .or(self.bootstrap_root.as_deref())
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        let _scope = self.timings.start(execution::timing::Label::Cleanup);
        if self.handoff_pending || self.git.as_ref().is_some_and(|git| !git.cleanup_certain()) {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Invocation Git or writer handoff cleanup is unverified; retain its writer/claim for explicit recovery",
            ).coded("ROSTER_PROCESS_CLEANUP"));
        }
        if let Some(boundary) = &mut self.boundary {
            boundary.close().await?;
            self.boundary = None;
        }
        self.preparation.close().await?;
        self.git = None;
        self.bootstrap_root = None;
        Ok(())
    }
}

fn selected_key_name(config: &config::Config) -> Option<&Text> {
    let llm = config.as_value().get("llm");
    if llm
        .and_then(|value| value.get("base_url"))
        .and_then(Value::as_string)
        .is_some_and(|value| !value.0.is_empty())
    {
        return llm
            .and_then(|value| value.get("api_key_env"))
            .and_then(Value::as_string);
    }
    None
}

pub fn credentials(environment: &Value, config: &config::Config) -> Result<Credentials, Error> {
    let mut selected = Value::object(Vec::new());
    if let Some(name) = selected_key_name(config) {
        let name = name.to_utf8()?;
        if let Some(value) = config::environment_entry(environment, &name)? {
            let value = value
                .as_string()
                .filter(|value| !crate::text::trim(&value.0).is_empty())
                .ok_or_else(|| {
                    Error::type_error(format!("{name} must be nonblank text when set"))
                })?;
            domain::put(&mut selected, &name, Value::String(value.clone()))?;
        }
    }
    Ok(Credentials {
        environment: Some(selected),
        vault_entries: None,
    })
}

pub fn invocation_credentials(environment: &Value, loaded: &Loaded) -> Result<Credentials, Error> {
    loaded.require_current()?;
    let mut credentials = credentials(environment, &loaded.config)?;
    if let Some(profile) = loaded.critic_profile() {
        let effective = loaded.config.with_profile(profile)?;
        let critic = self::credentials(environment, &effective)?;
        credentials.environment = Some(domain::merge(
            credentials.environment.as_ref().ok_or_else(|| {
                Error::type_error("Invocation credentials have no typed environment")
            })?,
            critic
                .environment
                .as_ref()
                .ok_or_else(|| Error::type_error("Critic credentials have no typed environment"))?,
        )?);
    }
    Ok(credentials)
}

pub fn invocation_credentials_with_vault(
    environment: &Value,
    loaded: &Loaded,
    get: &mut impl FnMut(&str) -> Result<Option<Value>, Error>,
) -> Result<Credentials, Error> {
    loaded.require_current()?;
    let mut configurations = vec![loaded.config.clone()];
    if let Some(profile) = loaded.critic_profile() {
        configurations.push(loaded.config.with_profile(profile)?);
    }
    let mut names = Vec::new();
    for configuration in &configurations {
        if let Some(name) = selected_key_name(configuration) {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    let mut selected = Value::object(Vec::new());
    let mut stored = Value::object(Vec::new());
    let mut has_stored = false;
    for name in names {
        let utf8 = name
            .to_utf8()
            .map_err(|_| Error::from(crate::vault::VaultError::Name))?;
        let value = crate::secrets::resolve_secret(&name, environment, get)?;
        if let Some(value) = value {
            let from_environment = config::environment_entry(environment, &utf8)?
                .and_then(Value::as_string)
                .is_some_and(|value| !value.0.is_empty());
            if from_environment {
                domain::put(&mut selected, &utf8, Value::String(value))?;
            } else {
                domain::put(&mut stored, &utf8, Value::String(value))?;
                has_stored = true;
            }
        }
    }
    loaded.require_current()?;
    Ok(Credentials {
        environment: Some(selected),
        vault_entries: has_stored.then_some(stored),
    })
}

pub fn native_invocation_credentials(
    environment: &Value,
    loaded: &Loaded,
) -> Result<Credentials, Error> {
    let mut vault = None;
    invocation_credentials_with_vault(environment, loaded, &mut |name| {
        if vault.is_none() {
            vault = Some(crate::vault::FileVault::from_environment(environment)?);
        }
        vault
            .as_ref()
            .ok_or_else(|| Error::from(crate::vault::VaultError::Directory))?
            .get(name)
            .map(|value| value.map(|value| Value::string(value.as_str())))
            .map_err(Error::from)
    })
}

fn redaction(
    config: &config::Config,
    credentials: &Credentials,
) -> Result<redaction::Config, Error> {
    let mut redaction = redaction::Config {
        environment: credentials
            .environment
            .clone()
            .unwrap_or_else(|| Value::object(Vec::new())),
        ..redaction::Config::default()
    };
    if let Some(llm) = config.as_value().get("llm") {
        if let Some(name) = llm.get("api_key_env").and_then(Value::as_string) {
            redaction.api_key_env = name.clone();
        }
        if let Some(environment) = &credentials.environment {
            redaction.add_selected_credentials(environment)?;
        }
        if let Some(entries) = &credentials.vault_entries {
            redaction.add_selected_credentials(entries)?;
        }
        for name in ["base_url", "model"] {
            if let Some(value) = llm
                .get(name)
                .and_then(Value::as_string)
                .filter(|value| !value.0.is_empty())
            {
                redaction.values.push(value.clone());
            }
        }
    }
    redaction.endpoint_values = endpoint_forms(config)?;
    redaction.validate()?;
    Ok(redaction)
}

fn invocation_redaction(
    loaded: &Loaded,
    credentials: &Credentials,
) -> Result<redaction::Config, Error> {
    let mut selected = redaction(&loaded.config, credentials)?;
    if let Some(profile) = loaded.critic_profile() {
        let effective = loaded.config.with_profile(profile)?;
        let critic = redaction(&effective, credentials)?;
        selected.values.extend(critic.values);
        selected.endpoint_values.extend(critic.endpoint_values);
    }
    Ok(selected)
}

fn endpoint_forms(config: &config::Config) -> Result<Vec<Text>, Error> {
    let Some(base) = config
        .as_value()
        .get("llm")
        .and_then(|llm| llm.get("base_url"))
        .and_then(Value::as_string)
        .filter(|base| !base.0.is_empty())
    else {
        return Ok(Vec::new());
    };
    let raw = base.to_utf8()?;
    let selected = config::base_url(base, "llm.base_url")?;
    let endpoint = crate::llm::openai::endpoint_for(base)?;
    let endpoint = url::Url::parse(&endpoint)
        .map_err(|_| Error::type_error("Cannot normalize selected endpoint diagnostics"))?;
    let mut forms = vec![Text::from(raw.as_str())];
    if let Some((_, rest)) = raw.split_once("://") {
        if let Some(authority) = rest
            .split(['/', '?', '#'])
            .next()
            .filter(|value| !value.is_empty())
        {
            forms.push(Text::from(authority));
        }
    }
    for url in [&selected, &endpoint] {
        forms.push(Text::from(url.as_str()));
        forms.push(Text::from(
            &url[url::Position::BeforeHost..url::Position::AfterPort],
        ));
        let host = match url.host() {
            Some(url::Host::Domain(domain)) => domain.to_owned(),
            Some(url::Host::Ipv4(address)) => address.to_string(),
            Some(url::Host::Ipv6(address)) => {
                forms.push(Text::from(address.to_string().as_str()));
                format!("[{address}]")
            }
            None => {
                return Err(Error::type_error(
                    "Selected endpoint has no normalized hostname",
                ));
            }
        };
        forms.push(Text::from(host.as_str()));
        if let Some(port) = url.port_or_known_default() {
            forms.push(Text::from(format!("{host}:{port}").as_str()));
        }
    }
    forms.sort_by_key(|form| std::cmp::Reverse(form.0.len()));
    forms.dedup();
    Ok(forms)
}

pub fn safe_error(
    error: &Error,
    config: &config::Config,
    credentials: &Credentials,
) -> Result<Error, Error> {
    let mut redaction = redaction(config, credentials)?;
    for field in ["base_url", "model"] {
        if let Some(value) = config
            .as_value()
            .get("llm")
            .and_then(|value| value.get(field))
            .and_then(Value::as_string)
            .filter(|value| !value.0.is_empty())
        {
            redaction.values.push(value.clone());
        }
    }
    let mut message = error.message.clone();
    let mut cause = error.cause.as_deref();
    for _ in 0..32 {
        let Some(current) = cause else {
            break;
        };
        if !message.contains("\nWorktree: ") {
            if let Some(position) = current.message.find("\nWorktree: ") {
                message.push_str(
                    current
                        .message
                        .get(position..)
                        .ok_or_else(|| Error::type_error("Invalid diagnostic location"))?,
                );
            }
        }
        cause = current.cause.as_deref();
    }
    let message = planning::memory::redact(&Text::from(message.as_str()), &redaction);
    let mut message = redaction::configured_string(&message, &redaction);
    for form in endpoint_forms(config)? {
        message = redaction::replace_ascii_case_insensitive(&message, &form, "[REDACTED]");
    }
    let message = message.to_utf8()?;
    let mut safe = Error::new(error.name, message);
    safe.code = error.code;
    Ok(safe)
}

pub fn safe_invocation_error(
    error: &Error,
    loaded: &Loaded,
    credentials: &Credentials,
) -> Result<Error, Error> {
    let error = safe_error(error, &loaded.config, credentials)?;
    let Some(profile) = loaded.critic_profile() else {
        return Ok(error);
    };
    let effective = loaded.config.with_profile(profile)?;
    safe_error(&error, &effective, credentials)
}

struct Log {
    path: PathBuf,
    file: File,
    snapshot: secure::Snapshot,
    count: u64,
    seat: &'static str,
}

impl Log {
    fn create(lease: &backend::RootLease, session: &str) -> Result<Self, Error> {
        lease.check()?;
        let directory = layout::resolve_directory(
            layout::Scope::PerWorktree,
            lease.root(),
            Some(lease.root()),
            Some(".roster"),
        )?;
        let path = layout::state_path(directory, &["runs", &format!("{session}.log")])?.path;
        secure::private_directory(
            path.parent()
                .ok_or_else(|| Error::type_error("Run log needs a parent"))?,
            true,
        )?;
        let identity = secure::write_exclusive(&path, b"")?;
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|error| secure::io_error("Open owned session log", &path, error))?;
        if paths::handle_information(&file)?.identity != identity {
            return Err(Error::type_error(
                "Session log identity changed while opening",
            ));
        }
        let mut log = Self {
            path,
            file,
            snapshot: secure::Snapshot {
                bytes: Vec::new(),
                identity,
            },
            count: 0,
            seat: "host",
        };
        log.write("invocation", "session-only", None)?;
        Ok(log)
    }

    fn write(
        &mut self,
        event: &'static str,
        status: &'static str,
        run: Option<&RunRecord>,
    ) -> Result<(), Error> {
        self.write_details(event, status, run, None)
    }

    fn write_details(
        &mut self,
        event: &'static str,
        status: &'static str,
        run: Option<&RunRecord>,
        details: Option<Value>,
    ) -> Result<(), Error> {
        if !secure::unchanged(&self.path, &self.snapshot, 2 * 1024 * 1024)?
            || paths::file_information(&self.path)?.links != 1
            || paths::handle_information(&self.file)?.identity != self.snapshot.identity
        {
            return Err(Error::new(
                "RunLogError",
                "Session log was externally replaced; preserve human bytes",
            )
            .coded("ROSTER_RUN_LOG"));
        }
        let mut record = Value::object(vec![
            ("schema_version", Value::Number(1.0)),
            ("sequence", Value::Number(self.count as f64)),
            ("event", Value::string(event)),
            ("seat", Value::string(self.seat)),
            ("status", Value::string(status)),
        ]);
        if let Some(run) = run {
            let usage = run.metrics().usage;
            domain::put(
                &mut record,
                "usage",
                Value::object(vec![
                    (
                        "prompt_tokens",
                        usage
                            .prompt_tokens
                            .map_or(Value::Null, |value| Value::Number(value as f64)),
                    ),
                    (
                        "completion_tokens",
                        usage
                            .completion_tokens
                            .map_or(Value::Null, |value| Value::Number(value as f64)),
                    ),
                ]),
            )?;
            domain::put(&mut record, "response_backed", Value::Boolean(true))?;
        }
        if let Some(details) = details {
            domain::put(&mut record, "details", details)?;
        }
        let bytes = format!("{}\n", record.json()).into_bytes();
        if self.snapshot.bytes.len() + bytes.len() > 2 * 1024 * 1024 {
            return Err(
                Error::new("RunLogError", "Session log reached its bounded 2 MiB limit")
                    .coded("ROSTER_RUN_LOG"),
            );
        }
        self.file
            .write_all(&bytes)
            .and_then(|()| self.file.sync_data())
            .map_err(|error| secure::io_error("Append owned session log", &self.path, error))?;
        self.snapshot.bytes.extend(bytes);
        self.count += 1;
        if !secure::unchanged(&self.path, &self.snapshot, 2 * 1024 * 1024)?
            || paths::file_information(&self.path)?.links != 1
        {
            return Err(Error::new("RunLogError", "Session log identity or bytes changed during append; no completed recording is current").coded("ROSTER_RUN_LOG"));
        }
        Ok(())
    }
}

struct Observations<'a, O: Observer> {
    log: &'a mut Log,
    observer: &'a mut O,
    loaded: &'a Loaded,
    redaction: redaction::Config,
    authority: Option<continuation::AuthorityFiles>,
    budget: &'a continuation::Budget,
    observer_error_recorded: bool,
}

impl<O: Observer> Observer for Observations<'_, O> {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        let mut phase = "pre-configuration";
        let result = async {
            self.loaded.require_current()?;
            phase = "pre-authority";
            if let Some(authority) = &self.authority {
                authority.check()?;
            }
            let transition = match event
                .get("type")
                .and_then(Value::as_string)
                .map(Text::to_utf8)
                .transpose()?
                .as_deref()
            {
                Some("rescope") => Some("rescope"),
                Some("perspective-escalation") => Some("perspective-escalation"),
                Some("review-repair") => Some("review-repair"),
                Some("scope-expansion") => Some("scope-expansion"),
                _ => None,
            };
            phase = "session-log";
            if let Some(name) = transition {
                self.log.write_details(
                    name,
                    "observed",
                    None,
                    Some(redaction::record(&event, &self.redaction)?),
                )?;
            } else {
                self.log.write("observation", "observed", None)?;
            }
            phase = "delegate";
            self.observer.emit(event).await?;
            phase = "post-authority";
            if let Some(authority) = &self.authority {
                authority.check()?;
            }
            phase = "post-configuration";
            self.loaded.require_current()
        }
        .await;
        if let Err(error) = &result {
            if !self.observer_error_recorded {
                self.observer_error_recorded = true;
                let record = Value::object(vec![
                    (
                        "origin",
                        Value::string("delivery-observer-before-flattening"),
                    ),
                    ("phase", Value::string(phase)),
                    ("name", Value::string(error.name)),
                    ("code", error.code.map_or(Value::Null, Value::string)),
                    ("transport", self.budget.snapshot()),
                ]);
                if let Err(diagnostic) =
                    self.log
                        .write_details("observer-error", "failed", None, Some(record))
                {
                    eprintln!(
                        "Observer diagnostic unavailable: {} [{}]; original error {} [{}]",
                        diagnostic.name,
                        diagnostic.code.unwrap_or("unclassified"),
                        error.name,
                        error.code.unwrap_or("unclassified")
                    );
                }
            }
        }
        result
    }
}

fn read_text(path: &Path, maximum: u64, label: &str) -> Result<Text, Error> {
    let snapshot = secure::read(path, maximum)?.ok_or_else(|| {
        Error::new("Error", format!("{label} is required: {}", path.display())).coded("ENOENT")
    })?;
    if paths::file_information(path)?.links != 1 || !secure::unchanged(path, &snapshot, maximum)? {
        return Err(Error::type_error(format!(
            "{label} must be a stable single-link regular file"
        )));
    }
    let source = String::from_utf8(snapshot.bytes)
        .map_err(|_| Error::type_error(format!("{label} must be UTF-8")))?;
    Ok(Text::from(source.as_str()))
}

fn no_foreign_artifacts(root: &Path, existing_task: bool) -> Result<(), Error> {
    for name in [
        "ASSIGNMENT.md",
        "CONTEXT.md",
        "ESTIMATE.md",
        "RESULT.md",
        "REVIEW.md",
        "PLAN.md",
        "RESEARCH.md",
    ]
    .into_iter()
    .chain((!existing_task).then_some("TASK.md"))
    .chain((!existing_task).then_some("RECIPE.yml"))
    {
        let file = root.join(name);
        secure::check_components(&file)?;
        match fs::symlink_metadata(&file) {
            Ok(_) => return Err(Error::new("ForeignArtifactError", format!(
                "Existing {name} is not owned by this invocation; preserve it and use a fresh worktree. Automatic resume/archive is not implemented."
            )).coded("ROSTER_FOREIGN_ARTIFACT")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(secure::io_error("Inspect local delivery artifact", &file, error)),
        }
    }
    Ok(())
}

pub(crate) async fn ignored(
    git: &start::Git,
    root: &Path,
    token: &CancellationToken,
) -> Result<(), Error> {
    let names = [
        ".roster/locks/run.lock",
        ".roster/runs/session.log",
        "ASSIGNMENT.md",
        "TASK.md",
        "RECIPE.yml",
        "ESTIMATE.md",
        "CONTEXT.md",
        "RESULT.md",
        "REVIEW.md",
        "PLAN.md",
        "RESEARCH.md",
    ];
    let mut args = vec!["check-ignore", "--no-index", "--"];
    args.extend(names);
    let observed = git.run(root, &args, token).await.map_err(|error| {
        Error::new(
            "LocalStateIgnoreError",
            "Runtime state paths must already be Git-ignored; no policy or ignore file is changed automatically",
        )
        .coded("ROSTER_LOCAL_IGNORE")
        .caused_by(error)
    })?;
    let expected = format!("{}\n", names.join("\n"));
    if observed != expected.as_bytes() {
        return Err(Error::new(
            "LocalStateIgnoreError",
            "Every required runtime state path must already be Git-ignored; partial or malformed ignore coverage is refused",
        )
        .coded("ROSTER_LOCAL_IGNORE"));
    }
    Ok(())
}

fn reference(recipe: &Text) -> Result<String, Error> {
    let value = crate::recipe::parse(recipe)?;
    let value = crate::json::parse(&Text::from(value.json().as_str()))?;
    value
        .get("ask")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Existing RECIPE.yml must declare its Ask reference"))?
        .to_utf8()
}

fn local_assignment(
    assignment: &lifecycle::Assignment,
    redaction: &redaction::Config,
) -> Result<Text, Error> {
    let source = Text::from(format!(
        "# Local assignment\n\nTask: {}\nSession: {}\nReference: {}\nPublication: unknown\n\n## Ask\n\n{}\n",
        assignment.task, assignment.session, assignment.reference, assignment.ask.to_utf8()?
    ).as_str());
    if planning::memory::redact(&source, redaction) != source {
        return Err(Error::type_error(
            "Local assignment must not contain secrets or private keys",
        ));
    }
    Ok(source)
}

fn present_artifacts(
    root: &Path,
    names: &[(&'static str, &str)],
) -> Result<Vec<(&'static str, PathBuf)>, Error> {
    let mut paths = Vec::new();
    for (label, name) in names {
        if artifacts::read(root, name)?.is_some() {
            paths.push((*label, root.join(name)));
        }
    }
    Ok(paths)
}

impl<T: Transport, O: Observer> Driver<'_, T, O> {
    pub fn run_local<'a>(
        &'a mut self,
        options: Options<'a>,
        token: CancellationToken,
        ownership: &'a mut Ownership,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Outcome, Error>> + 'a>> {
        Box::pin(self.run_local_inner(options, token, ownership))
    }

    async fn run_local_inner(
        &mut self,
        options: Options<'_>,
        token: CancellationToken,
        ownership: &mut Ownership,
    ) -> Result<Outcome, Error> {
        token.check()?;
        if ownership.root().is_some() {
            return Err(Error::type_error(
                "This invocation still owns local work; settle cleanup before another task",
            ));
        }
        let _scope = ownership.timings.start(execution::timing::Label::Local);
        let budget = continuation::Budget::new(&options.loaded.config, ownership.timings())?;
        let mut transport = continuation::TransportBudget {
            transport: self.transport,
            budget: &budget,
        };
        let mut driver = Driver {
            transport: &mut transport,
            observer: self.observer,
            credentials: self.credentials,
            admission: self.admission,
        };
        let result = Box::pin(driver.run_local_owned(options, token, ownership, &budget)).await;
        let root = ownership.root().map(Path::to_path_buf);
        locks::finish(result, ownership.close().await).map_err(|error| {
            if let Some(root) = root {
                if !error.message.contains("\nWorktree: ") {
                    return Error::new(error.name, format!("{}\nWorktree: {}\nNo publisher ran; preserve owned state on unverified cleanup.", error.message, root.display()))
                        .coded(error.code.unwrap_or("ROSTER_LOCAL_DELIVERY")).caused_by(error);
                }
            }
            error
        })
    }

    async fn run_local_owned(
        &mut self,
        options: Options<'_>,
        token: CancellationToken,
        ownership: &mut Ownership,
        budget: &continuation::Budget,
    ) -> Result<Outcome, Error> {
        let preparation_scope = budget
            .timings()
            .start(execution::timing::Label::Preparation);
        token.check()?;
        if ownership.root().is_some() {
            return Err(Error::type_error(
                "This invocation still owns a local writer; settle its cleanup before starting another task",
            ));
        }
        options.loaded.require_current()?;
        options.approval.validate()?;
        if options.skip_review && config::is_review_required(options.loaded.config.as_value())? {
            return Err(Error::type_error(
                "--skip-review requires an explicit private review.required: false policy; the required review gate cannot be waived by a flag alone",
            ));
        }
        crate::config::validate_start_base(&Text::from(options.start.base.as_str()), "--base")?;
        let redaction = invocation_redaction(options.loaded, self.credentials)?;
        let contracts = paths::resolve_contracts_path(
            options.installation,
            options.cwd,
            options.host.contracts.as_deref(),
        )?;
        let paths = paths::repository_paths(options.cwd)?;
        ownership.bootstrap_root = Some(paths.worktree.clone());
        let mut start_git = start::Git::new(
            options.host.clone(),
            paths.common_dir.join("roster-local").join("no-hooks"),
        )?;
        start_git.bind_timings(budget.timings());
        ownership.git = Some(start_git);
        let git = ownership
            .git
            .as_ref()
            .ok_or_else(|| Error::type_error("Invocation has no owned Git host"))?;
        let contracts =
            start::Contracts::open(options.installation, &contracts, git, &token).await?;
        let repository = start::Repository::inspect(options.cwd, git, &token).await?;
        let existing = matches!(options.input, Input::ExistingTask);
        let (ask, original_task, original_recipe) = match &options.input {
            Input::ExistingTask => {
                no_foreign_artifacts(&paths.worktree, true)?;
                let source = read_text(
                    &paths.worktree.join("TASK.md"),
                    65_536,
                    "An existing TASK.md",
                )?;
                let document =
                    task::parse(&Value::String(source.clone()), &task::Authority::default())?;
                let recipe = artifacts::read(&paths.worktree, "RECIPE.yml")?;
                (document.ask, Some(source), recipe)
            }
            Input::AskFile(path) => {
                let path = paths::resolve(options.cwd, path)?;
                paths::ensure_local_path(&path, &paths.worktree)?;
                let source = read_text(&path, planning::MAX_ASK_BYTES as u64, "Ask file")?;
                let ask = intake::clean_ask(&Value::String(source))?;
                if ask.contains("{{ASK}}") {
                    return Err(Error::type_error(
                        "Ask file contains an unresolved {{ASK}} placeholder",
                    ));
                }
                (Text::from(ask.as_str()), None, None)
            }
        };
        let configured = options
            .loaded
            .config
            .as_value()
            .get("llm")
            .and_then(|value| value.get("base_url"))
            .is_some_and(domain::truthy);
        let files = original_task
            .as_ref()
            .map(|source| task::files_allowed(&Value::String(source.clone())))
            .transpose()?
            .unwrap_or_default();
        let classified = intake::classify(&Value::String(ask.clone()), None, &files)?;
        let kind = intake::Kind::parse(
            &classified
                .get("kind")
                .and_then(Value::as_string)
                .ok_or_else(|| Error::type_error("Ask classification requires a kind"))?
                .to_utf8()?,
        )?;
        if kind == intake::Kind::Clarify {
            return Err(Error::type_error(intake::CLARIFICATION_HINT));
        }
        let original_reference = original_recipe.as_ref().map(reference).transpose()?;
        let (worktree, task_id, session, selected_start) = if existing {
            if repository.dirty {
                return Err(Error::new("DirtyWorktree", "Existing-TASK execution requires clean product source; preserve or commit human changes before running this seat").coded("ROSTER_DIRTY_WORKTREE"));
            }
            ignored(git, &paths.worktree, &token).await?;
            let task = format!("local-{}", files::nonce()?);
            let session = format!("roster-{}", files::nonce()?);
            ownership
                .preparation
                .acquire(
                    &paths.worktree,
                    &session,
                    options.host.clone(),
                    redaction.clone(),
                    token.clone(),
                )
                .await?;
            (paths.worktree.clone(), task, session, None)
        } else {
            let selected =
                start::resolve(&repository, git, &options.start, &options.fetch, &token).await?;
            let prepared = start::prepare_local(
                &repository,
                git,
                &options.loaded.config,
                selected,
                &options.approval,
                redaction.clone(),
                token.clone(),
                &mut ownership.preparation,
            )
            .await?;
            (
                prepared.worktree,
                prepared.task,
                prepared.session,
                Some(prepared.start),
            )
        };
        let assignment = lifecycle::Assignment {
            ask,
            title: None,
            reference: original_reference.unwrap_or_else(|| format!("local:{task_id}")),
            task: task_id.clone(),
            session: session.clone(),
        };
        ownership.bootstrap_root = Some(worktree.clone());
        preparation_scope.finish(None);
        let mut result = Box::pin(self.run_prepared(
            &options,
            git,
            ownership.preparation.lease_slot()?,
            &mut ownership.boundary,
            &mut ownership.handoff_pending,
            &contracts,
            &assignment,
            kind,
            configured,
            original_task.as_ref(),
            original_recipe.as_ref(),
            budget,
        ))
        .await;
        if let Ok(outcome) = &mut result {
            outcome.start = selected_start;
        }
        result.map_err(|error| {
            let mut message = planning::memory::redact(&Text::from(error.message.as_str()), &redaction);
            message.push_str(&format!("\nWorktree: {}\nInspect TASK.md, RESULT.md, REVIEW.md and .roster\\runs; no publisher ran.", worktree.display()));
            Error::new(error.name, message.to_utf8().unwrap_or_else(|_| "Local delivery failed; preserve its worktree".to_owned()))
                .coded(error.code.unwrap_or("ROSTER_LOCAL_DELIVERY")).caused_by(error)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn run_prepared<'a>(
        &'a mut self,
        options: &'a Options<'_>,
        git: &'a start::Git,
        lease: &'a mut Option<backend::RootLease>,
        boundary_slot: &'a mut Option<tools::Boundary>,
        handoff_pending: &'a mut bool,
        contracts: &'a start::Contracts,
        assignment: &'a lifecycle::Assignment,
        kind: intake::Kind,
        configured: bool,
        original_task: Option<&'a Text>,
        original_recipe: Option<&'a Text>,
        budget: &'a continuation::Budget,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Outcome, Error>> + 'a>> {
        Box::pin(self.run_prepared_inner(
            options,
            git,
            lease,
            boundary_slot,
            handoff_pending,
            contracts,
            assignment,
            kind,
            configured,
            original_task,
            original_recipe,
            budget,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_prepared_inner(
        &mut self,
        options: &Options<'_>,
        git: &start::Git,
        lease: &mut Option<backend::RootLease>,
        boundary_slot: &mut Option<tools::Boundary>,
        handoff_pending: &mut bool,
        contracts: &start::Contracts,
        assignment: &lifecycle::Assignment,
        kind: intake::Kind,
        configured: bool,
        original_task: Option<&Text>,
        original_recipe: Option<&Text>,
        budget: &continuation::Budget,
    ) -> Result<Outcome, Error> {
        let owner = lease
            .as_ref()
            .ok_or_else(|| Error::type_error("Prepared local task has no root lease"))?;
        owner.check()?;
        assignment.validate()?;
        no_foreign_artifacts(owner.root(), original_task.is_some())?;
        ignored(git, owner.root(), owner.token()).await?;
        if let Some(task) = original_task {
            if artifacts::read(owner.root(), "TASK.md")?.as_ref() != Some(task)
                || artifacts::read(owner.root(), "RECIPE.yml")?.as_ref() != original_recipe
            {
                return Err(Error::new(
                    "StaleTask",
                    "Existing TASK/RECIPE changed before writer admission",
                )
                .coded("ROSTER_STALE_SOURCE"));
            }
        }
        let worktree = owner.root().to_path_buf();
        let token = owner.token().clone();
        let mut log = Log::create(owner, &assignment.session)?;
        let log_path = log.path.clone();
        let source = start::Repository::inspect(owner.root(), git, owner.token()).await?;
        let pre_plan = hooks::run_pre_plan(owner, &options.approval).await?;
        pre_plan.require_current(owner.root()).await?;
        let assignment_text = local_assignment(assignment, owner.redaction())?;
        secure::write_exclusive(
            &owner.root().join("ASSIGNMENT.md"),
            assignment_text.to_utf8()?.as_bytes(),
        )?;
        let mut observed = Observations {
            log: &mut log,
            observer: self.observer,
            loaded: options.loaded,
            redaction: owner.redaction().clone(),
            authority: None,
            budget,
            observer_error_recorded: false,
        };
        let mut planner_run = None;
        let mut planner_response = None;
        let mut planner_critic = Vec::new();
        if original_task.is_none() || kind != intake::Kind::Slice {
            observed.log.seat = "planner";
            observed.log.write("seat-start", "running", None)?;
            let mut driver = lifecycle::Driver {
                transport: self.transport,
                observer: &mut observed,
                credentials: self.credentials,
                admission: self.admission,
                signal: Some(pre_plan.token()),
                redaction: owner.redaction().clone(),
                last_response: None,
                critic_evidence: Vec::new(),
            };
            let inventory = git
                .text(owner.root(), &["ls-files", "-z"], owner.token())
                .await?;
            let inventory = inventory
                .split('\0')
                .filter(|file| {
                    !file.is_empty()
                        && !planning::forbidden_write(file)
                        && !file
                            .split('/')
                            .any(|part| part.eq_ignore_ascii_case("vendor"))
                })
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if inventory.len() > 1500 {
                return Err(Error::type_error(
                    "Local planning inventory exceeds 1500 files; a bounded host inventory is required",
                ));
            }
            let planning = Box::pin(driver.run_planner_session_owned_after_pre_plan(
                &lifecycle::RunOptions {
                    worktree: owner.root(),
                    repo_root: &options.loaded.root,
                    assignment,
                    config: options.loaded.config.as_value(),
                    metadata: None,
                    locked_model: None,
                    kind: Some(kind),
                    plan_mode: false,
                    repository_files: Some(&inventory),
                    evaluations: None,
                    learning: None,
                },
                &pre_plan,
            ))
            .await;
            planner_response = driver.last_response.clone();
            planner_critic = driver.critic_evidence.clone();
            let plan = match planning {
                Ok(plan) => plan,
                Err(failure) => {
                    let run = failure.run.map(|run| *run);
                    let critic = failure.critic_evidence;
                    observed.log.write("seat-end", "unverified", run.as_ref())?;
                    observed.log.seat = "host";
                    observed.log.write("invocation-end", "unverified", None)?;
                    return Ok(Outcome {
                        status: Status::Unverified,
                        worktree: worktree.clone(),
                        task: assignment.task.clone(),
                        session: assignment.session.clone(),
                        ask_kind: kind,
                        artifacts: present_artifacts(
                            &worktree,
                            &[
                                ("RECIPE", "RECIPE.yml"),
                                ("TASK", "TASK.md"),
                                ("ESTIMATE", "ESTIMATE.md"),
                                ("PLAN", "PLAN.md"),
                            ],
                        )?,
                        log: log_path,
                        planner_run: run,
                        coder_run: None,
                        reviewer_run: None,
                        planner_response,
                        planner_critic: critic,
                        coder_response: None,
                        reviewer_response: None,
                        coder_result: None,
                        reviewer_report: None,
                        coder_handoff: None,
                        continuation: None,
                        orchestration: History::default(),
                        failure: Some(*failure.error),
                        start: None,
                    });
                }
            };
            planner_run = lifecycle::response_run(
                options.loaded.config.as_value(),
                driver.last_response.as_ref(),
                assignment,
            )?;
            observed.log.write(
                "seat-end",
                if plan.get("error").is_some()
                    || configured
                        && kind == intake::Kind::Slice
                        && plan.get("handoffReady") != Some(&Value::Boolean(true))
                {
                    "unverified"
                } else if configured {
                    "planned"
                } else {
                    "stub"
                },
                planner_run.as_ref(),
            )?;
            if configured
                && kind == intake::Kind::Slice
                && plan.get("handoffReady") != Some(&Value::Boolean(true))
            {
                observed.log.seat = "host";
                observed.log.write("invocation-end", "unverified", None)?;
                return Ok(Outcome {
                    status: Status::Unverified, worktree: worktree.clone(),
                    task: assignment.task.clone(), session: assignment.session.clone(), ask_kind: kind,
                    artifacts: present_artifacts(&worktree, &[("RECIPE", "RECIPE.yml"), ("TASK", "TASK.md"), ("ESTIMATE", "ESTIMATE.md")])?,
                    log: log_path, planner_run, coder_run: None, reviewer_run: None,
                    planner_response, planner_critic, coder_response: None, reviewer_response: None,
                    coder_result: None, reviewer_report: None, coder_handoff: None, continuation: None,
                    orchestration: History::default(),
                    failure: Some(Error::new("UnverifiedPlan", "Configured planning did not produce a verified native handoff; inspect TASK/ESTIMATE and retained critic findings").coded("ROSTER_PLAN_UNVERIFIED")),
                    start: None,
                });
            }
        } else {
            pre_plan.require_current(owner.root()).await?;
            if original_recipe.is_none() {
                let recipe = artifacts::runtime_recipe(&assignment.reference)?;
                let mut writer = artifacts::Writer::new(
                    owner.root(),
                    false,
                    owner.redaction().clone(),
                    Some(token.clone()),
                )?;
                writer.write("RECIPE.yml", &recipe)?;
            }
            let cached = lifecycle::cached_handoff(owner.root(), assignment, None)?.ok_or_else(|| Error::type_error(
                "Existing TASK/RECIPE is not a strict native handoff for its unchanged Original Ask",
            ))?;
            if cached.normalized_recipe {
                return Err(execution::unsupported(
                    "Automatic replacement of an existing noncanonical RECIPE",
                ));
            }
            let default_model = options
                .loaded
                .config
                .as_value()
                .get("llm")
                .and_then(|value| value.get("model"))
                .and_then(Value::as_string)
                .map(Text::to_utf8)
                .transpose()?
                .unwrap_or_default();
            lifecycle::prepare_handoff(
                owner.root(),
                &cached,
                &default_model,
                &[],
                owner.redaction().clone(),
                Some(token.clone()),
            )?;
            estimate::read_task_metadata(&Value::String(cached.plan.task))?;
        }
        source.require_current(git, owner.token()).await?;
        if kind != intake::Kind::Slice {
            contracts.require_current(git, &token).await?;
            options.loaded.require_current()?;
            source.require_current(git, owner.token()).await?;
            observed.log.seat = "host";
            observed
                .log
                .write("invocation-end", "planning-only", None)?;
            return Ok(Outcome {
                status: Status::PlanningOnly,
                worktree: worktree.clone(),
                task: assignment.task.clone(),
                session: assignment.session.clone(),
                ask_kind: kind,
                artifacts: vec![("PLAN", worktree.join("PLAN.md"))],
                log: log_path,
                planner_run,
                coder_run: None,
                reviewer_run: None,
                failure: None,
                start: None,
                planner_response,
                planner_critic,
                coder_response: None,
                reviewer_response: None,
                coder_result: None,
                reviewer_report: None,
                coder_handoff: None,
                continuation: None,
                orchestration: History::default(),
            });
        }
        owner.check()?;
        options.loaded.require_current()?;
        drop(pre_plan);
        let boundary_options =
            tools::Options::from_config(&options.loaded.config, execution::Role::Coder)?;
        *handoff_pending = true;
        let handed =
            tools::Boundary::borrow_root_lease(lease, assignment.clone(), boundary_options).await;
        match handed {
            Ok(boundary) => {
                *boundary_slot = Some(boundary);
                *handoff_pending = false;
            }
            Err(error) => {
                if lease.is_some() {
                    *handoff_pending = false;
                }
                return Err(error);
            }
        }
        let boundary = boundary_slot.as_mut().ok_or_else(|| {
            Error::type_error("Root lease handoff produced no execution boundary")
        })?;
        boundary.bind_timings(budget.timings());
        observed.authority = Some(continuation::AuthorityFiles::capture(boundary)?);
        async {
            let mut seats = seats::Driver {
                transport: self.transport,
                observer: &mut observed,
                credentials: self.credentials,
                admission: self.admission,
                signal: Some(&token),
            };
            let projection = evidence::Projection::default();
            let baseline = evidence::capture(boundary.root(), None, &projection)?;
            let mut orchestration = History::default();
            let mut capsule = None;
            let mut guidance = None;
            let mut previous_findings = Vec::new();
            let mut previous_unmet: Option<Vec<u64>> = None;
            let mut coder_index = 0_u32;
            let mut reviewer_index = 0_u32;
            let (coder, review, status, failure) = loop {
                token.check()?;
                options.loaded.require_current()?;
                boundary.check()?;
                coder_index += 1;
                budget.timings().seat("coder", coder_index);
                seats.observer.log.seat = "coder";
                seats.observer.log.write("seat-start", "running", None)?;
                let scope = boundary.span(execution::timing::Label::Coder);
                let coder = Box::pin(seats.run_coder(
                    boundary,
                    options.installation,
                    &options.loaded.config,
                    coder::Options {
                        approval: Some(options.approval.clone()),
                        initial_baseline: Some(&baseline),
                        capsule: capsule.as_ref(),
                        continuation: guidance.as_ref(),
                        ..coder::Options::default()
                    },
                )).await;
                execution::timing::finish(scope, coder.as_ref().err());
                let coder = coder?;
                options.loaded.require_current()?;
                seats.observer.log.write("seat-end", coder.status().as_str(), coder.run())?;
                orchestration.attempts.push(SeatAttempt {
                    seat: "coder", index: coder_index, status: coder.status().as_str(),
                    run: coder.run().cloned(), response: coder.response().cloned(),
                    self_review_response: coder.result().get("selfReviewResponse").map(|response| redaction::record(response, boundary.redaction())).transpose()?,
                    ask_audit: None,
                    handoff: coder.handoff(), failure: coder.failure().cloned(),
                    continuation_failure: coder.continuation_failure().cloned(),
                });
                if let Some(error) = coder.continuation_failure().cloned() {
                    let failure = coder.failure().cloned().or(Some(error));
                    break (coder, None, Status::Unverified, failure);
                }
                let handoff = coder.handoff();
                let scope_blocked = handoff == Some(seats::Handoff::ScopeBlocked)
                    && !boundary.scope_blocked().is_empty();
                let raised = if scope_blocked && orchestration.counts.rescopes < MAX_RESCOPES {
                    boundary.next_scope_budget()?
                } else { None };
                let perspective = coder.perspective_reason()
                    .filter(|_| orchestration.counts.perspectives < MAX_PERSPECTIVES);
                if scope_blocked && raised.is_none() {
                    let error = Error::new("PlanningEscalation", format!(
                        "Actual unprotected scope requests ({}) exceeded the fixed rescope policy (budget {}, {} of {MAX_RESCOPES} rescopings). Preserve correct work and obtain a new planning handoff; required checks remain unmet.",
                        boundary.scope_blocked().join(", "), boundary.scope_limit(), orchestration.counts.rescopes,
                    )).coded("ROSTER_PLANNING_ESCALATION");
                    break (coder, None, Status::PlanningEscalation, Some(error));
                }
                let mut continuation_blocked = None;
                if raised.is_some() || perspective.is_some() {
                    if let Err(error) = budget.check() {
                        continuation_blocked = Some(continuation::blocked_failure(&coder, error));
                    } else {
                    let count = if raised.is_some() {
                        orchestration.counts.rescopes + 1
                    } else { orchestration.counts.perspectives + 1 };
                    let kind = if raised.is_some() { "rescope" } else { "perspective-escalation" };
                    let mut event = Value::object(vec![
                        ("type", Value::string(kind)), ("attempt", Value::Number(f64::from(count))),
                        ("model", coder.response().map_or(Value::Null, |response| Value::String(response.model.clone()))),
                        ("model_source", Value::string("previous-coder-response")),
                        ("selected_model", options.loaded.config.as_value().get("llm").and_then(|llm| llm.get("model")).cloned().unwrap_or(Value::Null)),
                        ("changedFiles", continuation::strings(&boundary.written_files())),
                        ("scopeFiles", continuation::strings(boundary.scope_files())),
                        ("repairFiles", continuation::strings(boundary.repair_files())),
                    ]);
                    let heading = if let Some(to) = raised {
                        domain::put(&mut event, "from", continuation::number(boundary.scope_limit())?)?;
                        domain::put(&mut event, "to", continuation::number(to)?)?;
                        domain::put(&mut event, "files", continuation::strings(boundary.scope_blocked()))?;
                        format!("Re-scoped {count}: the previous {}-file expansion budget blocked actual requests {}; the budget is now {to}",
                            boundary.scope_limit(), boundary.scope_blocked().join(", "))
                    } else {
                        let reason = perspective.ok_or_else(|| Error::type_error("Perspective reason disappeared"))?;
                        domain::put(&mut event, "reason", Value::string(reason))?;
                        domain::put(&mut event, "same_selected_model", Value::Boolean(true))?;
                        format!("Fresh perspective {count}: the previous coder {reason}. This fixed-profile continuation uses the same selected model, not an invented alternate route. Use a materially different approach; do not repeat earlier failed moves")
                    };
                    let facts = orchestration.attempts.iter().filter(|attempt| attempt.seat == "coder")
                        .filter_map(|attempt| attempt.failure.as_ref().map(|error| {
                            let model = attempt.response.as_ref().map(|response| response.model.to_utf8()).transpose()?;
                            Ok(continuation::safe_line(&Text::from(format!(
                                "Coder context {} on {} stopped as {}: {}",
                                attempt.index, model.as_deref().unwrap_or("unknown actual model"),
                                attempt.handoff.map_or("failed", seats::Handoff::as_str), error.message,
                            ).as_str()), boundary))
                        })).collect::<Result<Vec<_>, Error>>()?;
                    guidance = Some(continuation::continuation(&heading, boundary, &facts, false)?);
                    capsule = execution::continuity::next_context(boundary, &options.loaded.config, coder.capsule(), &[], &projection)?;
                    orchestration.archives.push(coder.archive_continuation(boundary, None, &projection).await?);
                    options.loaded.require_current()?;
                    boundary.continue_context(raised)?;
                    if raised.is_some() { orchestration.counts.rescopes = count; }
                    else { orchestration.counts.perspectives = count; }
                    let event = redaction::record(&event, boundary.redaction())?;
                    orchestration.transitions.push(event.clone());
                    seats.observe_continuation(boundary, event, &projection).await?;
                    boundary.require_git().await?;
                    continue;
                    }
                }
                if options.skip_review {
                    let status = if coder.status() == finalization::Status::Stub { Status::Stub }
                    else if coder.status() == finalization::Status::Pass {
                        contracts.require_current(git, &token).await?;
                        options.loaded.require_current()?;
                        coder.require_current(boundary, &projection).await?;
                        Status::ReviewSkipped
                    } else { Status::Unverified };
                    let failure = continuation_blocked.or_else(|| coder.failure().cloned());
                    break (coder, None, status, failure);
                }
                reviewer_index += 1;
                budget.timings().seat("reviewer", reviewer_index);
                seats.observer.log.seat = "reviewer";
                seats.observer.log.write("seat-start", "running", None)?;
                let scope = boundary.span(execution::timing::Label::Reviewer);
                let review = Box::pin(seats.run_reviewer(
                    boundary, &coder, options.installation, &options.loaded.config,
                    reviewer::Options {
                        approval: options.approval.clone(),
                        previous_findings: &previous_findings,
                        ..reviewer::Options::default()
                    },
                )).await;
                execution::timing::finish(scope, review.as_ref().err());
                let review = review?;
                options.loaded.require_current()?;
                seats.observer.log.write("seat-end", review.status().as_str(), review.run())?;
                orchestration.attempts.push(SeatAttempt {
                    seat: "reviewer", index: reviewer_index, status: review.status().as_str(),
                    run: review.run().cloned(), response: review.response().cloned(),
                    self_review_response: None,
                    ask_audit: review.ask_audit().map(|audit| redaction::record(audit, boundary.redaction())).transpose()?,
                    handoff: None, failure: review.failure().cloned(),
                    continuation_failure: None,
                });
                if configured && coder.status() == finalization::Status::Pass
                    && review.status() == reviewer::Status::Fail && review.failure().is_none()
                    && orchestration.counts.review_repairs < MAX_REVIEW_REPAIRS
                {
                    if let Err(error) = budget.check() {
                        break (coder, Some(review), Status::Unverified, Some(error));
                    }
                    let (findings, unmet) = continuation::findings(&review, boundary)?;
                    let stalled = previous_unmet.as_ref().is_some_and(|previous|
                        !unmet.is_empty() && unmet.iter().all(|id| previous.contains(id)));
                    let count = orchestration.counts.review_repairs + 1;
                    guidance = Some(continuation::continuation(
                        &format!("Review repair {count}: the independent reviewer failed the previous result (unmet acceptance checks: {})",
                            unmet.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")),
                        boundary, &findings, stalled,
                    )?);
                    capsule = execution::continuity::next_context(boundary, &options.loaded.config, coder.capsule(), &unmet, &projection)?;
                    let event = redaction::record(&Value::object(vec![
                        ("type", Value::string("review-repair")), ("attempt", Value::Number(f64::from(count))),
                        ("unmetChecks", Value::Array(unmet.iter().map(|id| u32::try_from(*id)
                            .map(|id| Value::Number(f64::from(id)))
                            .map_err(|_| Error::type_error("Unmet check ID exceeds the native bound"))).collect::<Result<Vec<_>, _>>()?)),
                        ("changed_perspective", Value::Boolean(stalled)),
                        ("model", coder.response().map_or(Value::Null, |response| Value::String(response.model.clone()))),
                        ("model_source", Value::string("previous-coder-response")),
                        ("findings", Value::Array(findings.iter().cloned().map(Value::String).collect())),
                        ("changedFiles", continuation::strings(&boundary.written_files())),
                        ("scopeFiles", continuation::strings(boundary.scope_files())),
                        ("repairFiles", continuation::strings(boundary.repair_files())),
                    ]), boundary.redaction())?;
                    orchestration.archives.push(coder.archive_continuation(boundary, Some(&review), &projection).await?);
                    options.loaded.require_current()?;
                    boundary.continue_context(None)?;
                    orchestration.counts.review_repairs = count;
                    orchestration.transitions.push(event.clone());
                    previous_unmet = Some(unmet);
                    previous_findings = findings;
                    seats.observe_continuation(boundary, event, &projection).await?;
                    boundary.require_git().await?;
                    continue;
                }
                contracts.require_current(git, &token).await?;
                options.loaded.require_current()?;
                let mut failure = continuation_blocked.or_else(|| coder.failure().cloned());
                let status = if coder.status() == finalization::Status::Stub {
                    if review.status() == reviewer::Status::Pass {
                        return Err(Error::type_error("Offline review cannot claim a passing verdict"));
                    }
                    Status::Stub
                } else if review.status() == reviewer::Status::Fail && review.failure().is_none() && failure.is_none() {
                    review.require_completed_failure(boundary, &coder, &projection).await?;
                    let (findings, _) = continuation::findings(&review, boundary)?;
                    let findings = findings.iter().map(Text::to_utf8).collect::<Result<Vec<_>, _>>()?.join("; ");
                    failure = Some(Error::new("ReviewFailed", format!(
                        "Completed independent review remains failed after {} of {MAX_REVIEW_REPAIRS} repairs: {findings}",
                        orchestration.counts.review_repairs,
                    )).coded("ROSTER_REVIEW_FAILED"));
                    Status::Unverified
                } else {
                    match review.require_passing(boundary, &coder, &projection).await {
                        Ok(()) => Status::Verified,
                        Err(error) => {
                            if failure.is_none() {
                                failure = Some(review.failure().cloned().unwrap_or(error));
                            }
                            Status::Unverified
                        }
                    }
                };
                break (coder, Some(review), status, failure);
            };
            let coder_run = coder.run().cloned();
            let coder_response = coder.response().cloned();
            let coder_result = Some(coder.result().clone());
            let coder_handoff = coder.handoff();
            let continuation = coder.capsule().cloned();
            let reviewer_run = review.as_ref().and_then(|review| review.run().cloned());
            let reviewer_response = review.as_ref().and_then(|review| review.response().cloned());
            let reviewer_report = review.as_ref().map(|review| review.report().clone());
            let mut artifacts = vec![
                ("RECIPE", worktree.join("RECIPE.yml")),
                ("TASK", worktree.join("TASK.md")),
                ("ESTIMATE", worktree.join("ESTIMATE.md")),
                ("CONTEXT", worktree.join("CONTEXT.md")),
                ("RESULT", coder.result_path(boundary)?),
            ];
            if let Some(review) = &review {
                artifacts.push(("REVIEW", review.review_path(boundary)?));
            }
            token.check()?;
            options.loaded.require_current()?;
            seats.observer.authority.as_ref().ok_or_else(|| Error::type_error("Immutable delivery authority disappeared"))?.check()?;
            boundary.check()?;
            seats.observer.log.seat = "host";
            seats
                .observer
                .log
                .write_details("invocation-end", status.as_str(), None, Some(budget.snapshot()))?;
            Ok(Outcome {
                status,
                worktree: worktree.clone(),
                task: assignment.task.clone(),
                session: assignment.session.clone(),
                ask_kind: kind,
                artifacts,
                log: log_path,
                planner_run,
                coder_run,
                reviewer_run,
                failure,
                start: None,
                planner_response,
                planner_critic,
                coder_response,
                reviewer_response,
                coder_result,
                reviewer_report,
                coder_handoff,
                continuation,
                orchestration,
            })
        }
        .await
    }
}
