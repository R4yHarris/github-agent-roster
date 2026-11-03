use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

use super::*;
use crate::cancel::CancellationToken;
use crate::identity::sha256_hex;
use crate::llm::control::{NoObserver, Observer};
use crate::paths::{self, FileIdentity};
use crate::planning::{self, lifecycle, task};
use crate::process::{self, FailureKind, Limits};
use crate::state::{files, layout, locks, redaction, secure};

#[derive(Debug, Clone)]
pub struct CargoApproval {
    pub executable: PathBuf,
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Host {
    pub git: PathBuf,
    pub node20: Option<PathBuf>,
    pub cargo: Option<CargoApproval>,
    pub contracts: Option<PathBuf>,
    pub environment: BTreeMap<String, String>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            git: PathBuf::from("git"),
            node20: Some(
                std::env::var_os("ROSTER_TEST_NODE")
                    .map_or_else(|| PathBuf::from("node"), PathBuf::from),
            ),
            cargo: None,
            contracts: std::env::var_os("GITHUB_AGENT_CONTRACTS").map(PathBuf::from),
            environment: std::env::vars()
                .filter(|(name, _)| environment_allowed(name))
                .collect(),
        }
    }
}

impl Host {
    pub(crate) fn command(
        &self,
        root: &Path,
        executable: &Path,
        args: &[String],
        extra: &BTreeMap<String, String>,
    ) -> Command {
        let mut command = Command::new(executable);
        command.current_dir(root).env_clear();
        for (name, value) in &self.environment {
            if environment_allowed(name) {
                command.env(name, value);
            }
        }
        command.envs(extra).args(args);
        command
    }

    pub(crate) fn git_command(
        &self,
        root: &Path,
        no_hooks: &Path,
        args: &[String],
        extra: &BTreeMap<String, String>,
    ) -> Command {
        let mut all = vec![
            "--no-pager".to_owned(),
            "-c".to_owned(),
            "core.fsmonitor=false".to_owned(),
            "-c".to_owned(),
            format!("core.hooksPath={}", no_hooks.display()),
        ];
        if args.first().is_none_or(|command| command != "check-ignore") {
            all.insert(1, "--literal-pathspecs".to_owned());
        }
        all.extend_from_slice(args);
        let mut environment = BTreeMap::from([
            ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
            ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
            (
                "GIT_CONFIG_GLOBAL".to_owned(),
                if cfg!(windows) { "NUL" } else { "/dev/null" }.to_owned(),
            ),
            ("GIT_OPTIONAL_LOCKS".to_owned(), "0".to_owned()),
            ("GIT_NO_LAZY_FETCH".to_owned(), "1".to_owned()),
            ("GIT_NO_REPLACE_OBJECTS".to_owned(), "1".to_owned()),
            ("GIT_ATTR_NOSYSTEM".to_owned(), "1".to_owned()),
        ]);
        environment.extend(extra.clone());
        self.command(root, &self.git, &all, &environment)
    }
}

fn environment_allowed(name: &str) -> bool {
    [
        "path",
        "pathext",
        "systemroot",
        "windir",
        "comspec",
        "temp",
        "tmp",
        "lang",
        "lc_all",
        "tz",
    ]
    .contains(&name.to_ascii_lowercase().as_str())
}

pub(crate) fn unverified_cleanup(failure: &process::Failure, descendants_possible: bool) -> bool {
    !failure.cleanup_complete
        || matches!(failure.kind, FailureKind::Exit(None))
        || failure.captured.pid.is_some()
            && descendants_possible
            && !matches!(failure.kind, FailureKind::Exit(Some(_)))
}

pub(crate) fn correctable_content(error: &Error) -> bool {
    error.name == "ToolAccessError"
        && error.code == Some("ROSTER_TOOL_DENIED")
        && error.message == "Tool writes must not contain credentials or private keys"
}

#[derive(Debug, Clone)]
pub struct Options {
    pub role: Role,
    pub scope_expansion: usize,
    pub initial_scope_files: Vec<String>,
    pub initial_repair_files: Vec<String>,
    pub required_reads: Vec<String>,
    pub slice_reads_only: bool,
    pub allow_run_test: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            role: Role::Coder,
            scope_expansion: 0,
            initial_scope_files: Vec::new(),
            initial_repair_files: Vec::new(),
            required_reads: Vec::new(),
            slice_reads_only: true,
            allow_run_test: true,
        }
    }
}

impl Options {
    pub fn from_config(config: &crate::config::Config, role: Role) -> Result<Self, Error> {
        let value = config.as_value();
        let scope = if role == Role::Coder {
            domain::safe_integer(
                value
                    .get("seat")
                    .and_then(|seat| seat.get("scope_expansion")),
            )
            .unwrap_or(3)
        } else {
            0
        };
        Ok(Self {
            role,
            scope_expansion: usize::try_from(scope).map_err(|_| {
                Error::type_error("Configured scope expansion exceeds this platform")
            })?,
            allow_run_test: role == Role::Coder
                && value.get("tools").and_then(|tools| tools.get("run_test"))
                    != Some(&Value::Boolean(false)),
            ..Self::default()
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub digest: String,
    pub mode: String,
    pub identity: FileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub files: BTreeMap<String, Product>,
}

impl Snapshot {
    pub fn digests(&self) -> Value {
        Value::Array(
            self.files
                .iter()
                .map(|(file, product)| {
                    Value::Array(vec![Value::string(file), Value::string(&product.digest)])
                })
                .collect(),
        )
    }

    pub fn digest(&self) -> String {
        sha256_hex(self.digests().json().as_bytes())
    }
}

#[derive(Clone)]
pub struct TestEvidence {
    pub(crate) result: Value,
    pub(crate) snapshot: Snapshot,
    pub(crate) authority: String,
    pub(crate) native: bool,
    pub(crate) validation: Option<Value>,
    pub(crate) audit: BTreeMap<String, String>,
    requirements: VerificationRequirements,
    coverage: EngineCoverage,
    observed_exit: i32,
    observed_skipped: bool,
    observed_commands: Vec<String>,
    observed_cargo_version: Option<String>,
    observation: std::sync::Arc<()>,
}

impl TestEvidence {
    pub fn result(&self) -> &Value {
        &self.result
    }

    pub(crate) fn require_current_observation(&self, boundary: &Boundary) -> Result<(), Error> {
        let _scope = boundary.span(super::timing::Label::ProofCompare);
        if self.authority != boundary.authority_digest()
            || self.snapshot != boundary.snapshot()?
            || self.audit != boundary.audit()?
            || self.requirements
                != boundary
                    .verification_requirements(&boundary.snapshot()?, self.requirements.full)?
        {
            return Err(denied(
                "Observed checks no longer match current authority, scope or worktree bytes",
            ));
        }
        Ok(())
    }

    pub(crate) fn observed_exit(&self) -> i32 {
        self.observed_exit
    }
    pub(crate) fn observed_skipped(&self) -> bool {
        self.observed_skipped
    }
    pub(crate) fn observed_commands(&self) -> &[String] {
        &self.observed_commands
    }
    pub(crate) fn observation_identity(&self) -> std::sync::Arc<()> {
        self.observation.clone()
    }
    pub(crate) fn full_exception_eligible(&self) -> bool {
        self.requirements.full
            && self.requirements.node
            && self.observed_exit != 0
            && !self.observed_skipped
            && (!self.requirements.native || self.coverage.cargo)
    }
    pub(crate) fn observation_contract(&self) -> Value {
        let mut required = Vec::new();
        if self.requirements.native {
            required.push("cargo".to_owned());
        }
        if self.requirements.node {
            required.push("node".to_owned());
        }
        let mut observed = Vec::new();
        if self
            .observed_commands
            .iter()
            .any(|command| command.starts_with("cargo "))
        {
            observed.push("cargo".to_owned());
        }
        if self
            .observed_commands
            .iter()
            .any(|command| command.starts_with("node --test"))
        {
            observed.push("node".to_owned());
        }
        Value::object(vec![
            ("required", strings(&required)),
            ("observed", strings(&observed)),
            (
                "commands_digest",
                Value::string(&sha256_hex(
                    strings(&self.observed_commands).json().as_bytes(),
                )),
            ),
            (
                "command_count",
                Value::Number(self.observed_commands.len() as f64),
            ),
            (
                "cargo_version",
                self.observed_cargo_version
                    .as_deref()
                    .map_or(Value::Null, Value::string),
            ),
        ])
    }

    pub(crate) fn require_current_coverage(&self, boundary: &Boundary) -> Result<(), Error> {
        let current =
            boundary.verification_requirements(&boundary.snapshot()?, self.requirements.full)?;
        if current != self.requirements
            || current.native && !self.coverage.cargo
            || current.node && !self.coverage.node
        {
            return Err(denied(
                "Green checkpoint requires actual passing checks for every effective Node/Cargo requirement",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerificationRequirements {
    files: Vec<String>,
    native: bool,
    node: bool,
    docs: bool,
    full: bool,
}

#[derive(Debug, Default, Clone)]
struct EngineCoverage {
    cargo: bool,
    node: bool,
}

pub(crate) struct FullVerification {
    latest: Option<TestEvidence>,
    failures: Option<(i32, i32)>,
    pending: bool,
}

impl FullVerification {
    pub(crate) fn latest(&self) -> Option<&TestEvidence> {
        self.latest.as_ref().filter(|_| !self.pending)
    }
    pub(crate) fn failure_exits(&self) -> Option<(i32, i32)> {
        self.failures
    }
    pub(crate) fn requires_resolution(&self) -> bool {
        self.pending || self.failures.is_some()
    }
}

pub(crate) struct ObservedProcess {
    pub exit: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

struct NodeChecks {
    output: ObservedProcess,
    skipped: bool,
    commands: Vec<String>,
    syntax_file: Option<String>,
}

enum Writer {
    Direct(locks::RepoLock),
    PrePlan(Box<backend::RootLease>),
}

impl Writer {
    fn check(&self) -> Result<(), Error> {
        match self {
            Self::Direct(lock) => lock.check(),
            Self::PrePlan(lease) => lease.storage_check(),
        }
    }

    async fn release_required(&mut self) -> Result<(), Error> {
        match self {
            Self::Direct(lock) => lock.release_required().await,
            Self::PrePlan(lease) => lease.close().await,
        }
    }
}

pub struct Boundary {
    root: PathBuf,
    root_identity: FileIdentity,
    git_identity: FileIdentity,
    git_marker: Option<secure::Snapshot>,
    task: Text,
    document: task::Document,
    assignment: lifecycle::Assignment,
    artifacts: BTreeMap<String, Option<secure::Snapshot>>,
    writer: Writer,
    writer_instance: std::sync::Arc<()>,
    token: CancellationToken,
    options: Options,
    initial_scope_expansion: usize,
    host: Host,
    redaction: redaction::Config,
    expanded: Vec<String>,
    scope_blocked: Vec<String>,
    repair: Vec<String>,
    written: BTreeSet<String>,
    seen: BTreeMap<String, secure::Snapshot>,
    searched: bool,
    checkpoint: Option<Value>,
    last_tests: Option<TestEvidence>,
    full_verification: Option<FullVerification>,
    restore_generation: u64,
    context_generation: u32,
    git_seal: Option<checkpoints::GitSeal>,
    timings: Option<super::timing::Timings>,
    fresh_after_restore: bool,
    recipe_tools: Option<Vec<String>>,
    no_hooks: PathBuf,
    cleanup_uncertain: Cell<bool>,
    owned_checks: bool,
    images: Vec<crate::host::Image>,
}

struct PendingCheck<'a> {
    boundary: &'a Boundary,
    settled: bool,
}

impl Drop for PendingCheck<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.boundary.quarantine_owned();
        }
    }
}

pub struct ReadOnly<'a> {
    boundary: &'a Boundary,
    tools: Vec<String>,
}

impl ReadOnly<'_> {
    pub async fn execute(&self, name: &str, args: &Value) -> Result<Value, Error> {
        self.boundary.check()?;
        if !["read_file", "search_text", "git_diff"].contains(&name)
            || !self.tools.iter().any(|tool| tool == name)
        {
            return Err(denied(
                "The reviewer is read-only; its recipe permits only read_file, search_text and git_diff requests",
            ));
        }
        let result = match name {
            "read_file" => self.boundary.read_file_value(args, true)?.2,
            "search_text" => self.search(args)?,
            "git_diff" => {
                fields(args, &[], &["path"], "Reviewer git_diff accepts only path")?;
                if let Some(path) = args.get("path") {
                    let path = self
                        .boundary
                        .normalized(&text(path, "Review path must be text")?, false)?;
                    let output = self
                        .boundary
                        .git(
                            &[
                                "diff",
                                "--no-ext-diff",
                                "--no-textconv",
                                "--no-renames",
                                "HEAD",
                                "--",
                                &path,
                            ]
                            .map(str::to_owned),
                            &BTreeMap::new(),
                        )
                        .await?;
                    if output.len() > evidence::MAX_DIFF_BYTES {
                        return Err(denied("Reviewer diff exceeds the bounded evidence limit"));
                    }
                    Value::String(planning::memory::redact(
                        &Text::from(bytes_text(&output)?.as_str()),
                        self.boundary.redaction(),
                    ))
                } else {
                    let changes = evidence::changes(self.boundary, None).await?;
                    Value::String(planning::memory::redact(
                        changes.diff(),
                        self.boundary.redaction(),
                    ))
                }
            }
            _ => return Err(denied("Reviewer request is unavailable")),
        };
        self.boundary.check()?;
        Ok(result)
    }

    fn search(&self, args: &Value) -> Result<Value, Error> {
        fields(
            args,
            &["query"],
            &["path"],
            "Reviewer search_text requires query and optional path",
        )?;
        let query = text(required(args, "query")?, "Reviewer query must be text")?;
        if query.is_empty() || query.len() > 500 || query.contains(['\0', '\r', '\n']) {
            return Err(usage(
                "Reviewer query must be a nonempty bounded single line",
            ));
        }
        let path = args
            .get("path")
            .map(|path| text(path, "Reviewer path must be text"))
            .transpose()?
            .unwrap_or_else(|| ".".to_owned());
        let path = self.boundary.normalized(&path, true)?;
        let mut queue = vec![path];
        let mut matches = Vec::new();
        let mut visited = 0;
        let mut bytes = 0;
        while let Some(path) = queue.pop() {
            self.boundary.check()?;
            visited += 1;
            if visited > MAX_FILES {
                return Err(usage("Reviewer search exceeds 2048 entries"));
            }
            let target = self.boundary.root.join(&path);
            secure::check_components(&target)?;
            let metadata = fs::symlink_metadata(&target)
                .map_err(|error| secure::io_error("Inspect review search path", &target, error))?;
            if secure::linked(&metadata) {
                return Err(denied("Reviewer search refuses linked paths"));
            }
            if metadata.is_dir() {
                let mut entries = fs::read_dir(&target)
                    .map_err(|error| {
                        secure::io_error("Read review search directory", &target, error)
                    })?
                    .map(|entry| {
                        entry.map_err(|error| secure::io_error("Read review entry", &target, error))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if entries.len() > MAX_FILES {
                    return Err(usage("Reviewer directory exceeds 2048 entries"));
                }
                entries.sort_by_key(|entry| entry.file_name());
                for entry in entries.into_iter().rev() {
                    let name = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| usage("Reviewer paths must be Unicode"))?;
                    let file = if path == "." {
                        name
                    } else {
                        format!("{path}/{name}")
                    };
                    if planning::forbidden_read(&file)
                        || private_path(&file)
                        || policy::managed(&file)
                    {
                        continue;
                    }
                    if self.boundary.normalized(&file, true).is_ok() {
                        queue.push(file);
                    }
                    if queue.len() + visited > MAX_FILES {
                        return Err(usage("Reviewer search exceeds 2048 entries"));
                    }
                }
            } else if metadata.is_file() {
                let (_, _, value) = match self
                    .boundary
                    .read_file_value(&Value::object(vec![("path", Value::string(&path))]), true)
                {
                    Ok(v) => v,
                    Err(err) if err.message.contains("UTF-8") => continue,
                    Err(err) => return Err(err),
                };
                let source = text(&value, "Reviewer search input must be text")?;
                bytes += source.len() as u64;
                if bytes > MAX_SNAPSHOT_BYTES {
                    return Err(usage("Reviewer search exceeds the byte budget"));
                }
                for (index, line) in source.replace("\r\n", "\n").split('\n').enumerate() {
                    if line.contains(&query) {
                        matches.push(Value::object(vec![
                            ("path", Value::string(&path)),
                            ("line", Value::Number((index + 1) as f64)),
                            ("text", Value::string(line)),
                        ]));
                        if matches.len() == 51 {
                            break;
                        }
                    }
                }
                if matches.len() == 51 {
                    break;
                }
            } else {
                return Err(denied("Reviewer search refuses special files"));
            }
        }
        let truncated = matches.len() > 50;
        matches.truncate(50);
        Ok(Value::object(vec![
            ("matches", Value::Array(matches)),
            ("truncated", Value::Boolean(truncated)),
        ]))
    }
}

fn key(file: &str) -> String {
    if cfg!(windows) {
        file.to_lowercase()
    } else {
        file.to_owned()
    }
}

fn private_path(file: &str) -> bool {
    file.split('/').any(|part| {
        [
            ".roster",
            ".roster-state",
            ".worktrees",
            "vendor",
            "node_modules",
        ]
        .contains(&part.to_lowercase().as_str())
    })
}

fn regular(root: &Path, file: &str, maximum: u64) -> Result<Option<secure::Snapshot>, Error> {
    let target = root.join(file);
    paths::ensure_local_path(&target, root)
        .map_err(|cause| denied("Tool path containment failed").caused_by(cause))?;
    let snapshot = secure::read(&target, maximum).map_err(|cause| {
        denied("Tool file must remain bounded, regular and unlinked").caused_by(cause)
    })?;
    if let Some(snapshot) = &snapshot {
        if paths::file_information(&target)?.links != 1 {
            return Err(denied("Tool files must be regular single-link files"));
        }
        if !secure::unchanged(&target, snapshot, maximum)? {
            return Err(denied("Tool file changed while reading"));
        }
    }
    Ok(snapshot)
}

#[derive(Debug, Clone)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

pub fn parse_calls(
    boundary: &Boundary,
    message: &Value,
    turn: u64,
    used_ids: &[String],
) -> Result<Vec<Call>, Error> {
    boundary.check()?;
    if domain::members(message).is_none() {
        return Err(Error::type_error("Coder returned no assistant message"));
    }
    let Some(calls) = message.get("tool_calls") else {
        return Ok(Vec::new());
    };
    let calls = domain::items(calls).ok_or_else(|| {
        Error::type_error("Native tool calls must be a structured function array")
    })?;
    if calls.len() > 8 || calls.iter().map(|call| call.json().len()).sum::<usize>() > 131_072 {
        return Err(Error::type_error(
            "Native tool calls exceed eight calls or 128 KiB",
        ));
    }
    let mut ids = used_ids.to_vec();
    let mut result = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        fields(
            call,
            &["function"],
            &["id", "type"],
            "Native tool call must be a function object",
        )?;
        if call
            .get("type")
            .is_some_and(|kind| !domain::is_text(Some(kind), "function"))
        {
            return Err(Error::type_error(
                "Native tool call must be a function object",
            ));
        }
        let definition = required(call, "function")?;
        fields(
            definition,
            &["name", "arguments"],
            &[],
            "Native function requires only name and arguments",
        )?;
        let name = text(
            required(definition, "name")?,
            "Native tool name must be text",
        )?;
        if !boundary.tool_names().contains(&name.as_str()) {
            return Err(denied("Tool is unavailable in this seat or recipe"));
        }
        let argument = required(definition, "arguments")?;
        let args = if let Some(source) = argument.as_string() {
            if source.utf8_len() > 131_072 {
                return Err(Error::type_error("Native tool arguments exceed 128 KiB"));
            }
            crate::json::parse(source)
                .map_err(|_| Error::type_error("Native tool arguments must be valid JSON"))?
        } else {
            argument.clone()
        };
        if domain::members(&args).is_none() {
            return Err(Error::type_error("Native tool arguments must be an object"));
        }
        let id = call
            .get("id")
            .map(|id| text(id, "Native tool call ID must be text"))
            .transpose()?
            .unwrap_or_else(|| format!("coder-{turn}-{index}"));
        if id.is_empty()
            || id.encode_utf16().count() > 128
            || id.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
            || ids.contains(&id)
        {
            return Err(Error::type_error(
                "Native tool call ID is invalid or duplicated",
            ));
        }
        ids.push(id.clone());
        result.push(Call { id, name, args });
    }
    boundary.check()?;
    Ok(result)
}

pub(crate) fn bytes_text(bytes: &[u8]) -> Result<String, Error> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| Error::type_error("Native tool output must be valid UTF-8"))
}

impl Boundary {
    pub fn read_only(&self) -> Result<ReadOnly<'_>, Error> {
        self.check()?;
        let snapshot = self
            .artifacts
            .get("RECIPE.yml")
            .and_then(Option::as_ref)
            .ok_or_else(|| denied("Read-only review requires the frozen recipe"))?;
        let recipe = crate::recipe::parse(&Text::from(bytes_text(&snapshot.bytes)?.as_str()))?;
        let recipe = crate::json::parse(&Text::from(recipe.json().as_str()))?;
        let reviewer = recipe
            .get("seats")
            .and_then(domain::items)
            .and_then(|seats| {
                seats
                    .iter()
                    .find(|seat| domain::is_text(seat.get("id"), "reviewer"))
            })
            .ok_or_else(|| denied("The frozen recipe has no reviewer seat"))?;
        if !domain::is_text(reviewer.get("principal"), "reviewer")
            || !domain::is_text(reviewer.get("worker"), "builtin")
        {
            return Err(denied(
                "Native review requires the frozen builtin reviewer principal, never coder or publishing authority",
            ));
        }
        let tools = match reviewer.get("tools") {
            Some(tools) => domain::items(tools)
                .ok_or_else(|| denied("Reviewer recipe tools must be an explicit list"))?
                .iter()
                .map(|tool| text(tool, "Reviewer tool must be text"))
                .collect::<Result<Vec<_>, _>>()?,
            None => ["read_file", "search_text", "git_diff"]
                .map(str::to_owned)
                .to_vec(),
        };
        if tools
            .iter()
            .any(|tool| !["read_file", "search_text", "git_diff"].contains(&tool.as_str()))
        {
            return Err(denied(
                "Reviewer recipe cannot grant mutation, shell or function tools",
            ));
        }
        Ok(ReadOnly {
            boundary: self,
            tools,
        })
    }
    pub async fn open(
        root: &Path,
        assignment: lifecycle::Assignment,
        options: Options,
        host: Host,
        redaction: redaction::Config,
        token: CancellationToken,
    ) -> Result<Self, Error> {
        Self::open_inner(root, assignment, options, host, redaction, token, &mut None).await
    }

    pub async fn from_root_lease(
        lease: backend::RootLease,
        assignment: lifecycle::Assignment,
        options: Options,
    ) -> Result<Self, Error> {
        let mut lease = Some(lease);
        let result = Self::borrow_root_lease(&mut lease, assignment, options).await;
        if let Some(mut lease) = lease {
            return locks::finish(result, lease.close().await);
        }
        result
    }

    pub async fn borrow_root_lease(
        lease: &mut Option<backend::RootLease>,
        assignment: lifecycle::Assignment,
        options: Options,
    ) -> Result<Self, Error> {
        let owner = lease
            .as_ref()
            .ok_or_else(|| denied("No pre-plan root lease is available for handoff"))?;
        let root = owner.root().to_path_buf();
        let host = owner.host().clone();
        let redaction = owner.redaction().clone();
        let token = owner.token().clone();
        Self::open_inner(&root, assignment, options, host, redaction, token, lease).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_inner(
        root: &Path,
        assignment: lifecycle::Assignment,
        options: Options,
        host: Host,
        redaction: redaction::Config,
        token: CancellationToken,
        lease: &mut Option<backend::RootLease>,
    ) -> Result<Self, Error> {
        token.check()?;
        assignment.validate()?;
        redaction.validate()?;
        for identifier in [&assignment.task, &assignment.session, &assignment.reference] {
            let value = Text::from(identifier.as_str());
            if planning::memory::redact(&value, &redaction) != value {
                return Err(denied(
                    "Execution identifiers must not contain secret material",
                ));
            }
        }
        if options.scope_expansion > 16
            || options.role == Role::Reviewer && options.scope_expansion != 0
        {
            return Err(Error::type_error(
                "Scope expansion requires a coder seat and a 0-16 file limit",
            ));
        }
        let recorded_limit = if options.role == Role::Coder {
            options.scope_expansion
        } else {
            16
        };
        if options.initial_scope_files.len() > recorded_limit {
            return Err(Error::type_error(
                "Recorded scope expansion exceeds its approved limit; no paths were discarded",
            ));
        }
        if options.initial_repair_files.len() > MAX_SCOPE_ENTRIES
            || options.required_reads.len() > 64
        {
            return Err(Error::type_error(
                "Native repair continuation and required reads are bounded to 64 paths; no paths were discarded",
            ));
        }
        let expanded_keys = options
            .initial_scope_files
            .iter()
            .map(|file| key(file))
            .collect::<BTreeSet<_>>();
        if expanded_keys.len() != options.initial_scope_files.len() {
            return Err(Error::type_error(
                "Recorded scope expansion paths must be distinct",
            ));
        }
        policy::task_and_repair_files(
            &[],
            &options.initial_repair_files,
            &options.initial_scope_files,
        )?;
        if options
            .initial_scope_files
            .iter()
            .chain(&options.initial_repair_files)
            .any(|file| private_path(file))
        {
            return Err(denied(
                "Recorded scope cannot grant private state, dependencies or another worktree",
            ));
        }
        for file in &options.required_reads {
            if !policy::exact_path(file) || planning::forbidden_read(file) || private_path(file) {
                return Err(denied(
                    "Required reads must be exact readable product files",
                ));
            }
        }
        let root = paths::absolute(root)?;
        secure::check_components(&root)?;
        let status = fs::symlink_metadata(&root)
            .map_err(|error| secure::io_error("Inspect worktree", &root, error))?;
        if secure::linked(&status) || !status.is_dir() {
            return Err(denied("Worktree must be a real directory, not a symlink"));
        }
        let root_identity = paths::file_information(&root)?.identity;
        let marker = root.join(".git");
        let marker_status = fs::symlink_metadata(&marker)
            .map_err(|error| secure::io_error("Inspect execution Git marker", &marker, error))?;
        if secure::linked(&marker_status) || !(marker_status.is_dir() || marker_status.is_file()) {
            return Err(denied("Execution requires a real Git worktree marker"));
        }
        let git_identity = paths::file_information(&marker)?.identity;
        let git_marker = if marker_status.is_file() {
            let snapshot = secure::read(&marker, 65_536)?
                .ok_or_else(|| denied("Execution Git marker disappeared"))?;
            if paths::file_information(&marker)?.links != 1 {
                return Err(denied("Execution Git marker must be single-link"));
            }
            Some(snapshot)
        } else {
            None
        };
        let mut artifacts = BTreeMap::new();
        for name in ["TASK.md", "RECIPE.yml", "ASSIGNMENT.md"] {
            artifacts.insert(name.to_owned(), regular(&root, name, 65_536)?);
        }
        let task_snapshot = artifacts
            .get("TASK.md")
            .and_then(Option::as_ref)
            .ok_or_else(|| denied("Execution requires a validated TASK.md"))?;
        let task = Text::from(bytes_text(&task_snapshot.bytes)?.as_str());
        let cached = lifecycle::cached_handoff(&root, &assignment, None)?.ok_or_else(|| {
            denied("Execution requires the unchanged strict planning handoff for this Ask")
        })?;
        if cached.normalized_recipe {
            return Err(denied(
                "Execution requires a finalized canonical RECIPE; the host must accept the validated planning handoff first",
            ));
        }
        if cached.plan.task.normalized_lines() != task.normalized_lines() {
            return Err(denied(
                "Planning normalization cannot replace execution authority",
            ));
        }
        let document = task::parse(
            &Value::String(task.clone()),
            &task::Authority {
                expected_ask: Some(assignment.ask.clone()),
                issue_title: assignment.title.clone(),
                issue_body: None,
            },
        )?;
        for file in document
            .files_allowed
            .iter()
            .chain(&options.initial_scope_files)
            .chain(&options.initial_repair_files)
            .chain(&options.required_reads)
        {
            let path = Text::from(file.as_str());
            if planning::memory::redact(&path, &redaction) != path {
                return Err(denied(
                    "Task and recorded read/write scope must not contain secret material",
                ));
            }
        }
        if policy::task_and_repair_files(
            &document.files_allowed,
            &options.initial_repair_files,
            &options.initial_scope_files,
        )?
        .len()
            > MAX_SCOPE_ENTRIES
        {
            return Err(unsupported(
                "More than 64 task/repair/expanded paths require explicit native re-scope integration; no paths were discarded",
            ));
        }
        for file in &document.files_allowed {
            if file.len() > 240 || private_path(file) || planning::forbidden_write(file) {
                return Err(denied(
                    "TASK.md scope cannot grant protected or truncated paths",
                ));
            }
        }
        let recipe = crate::json::parse(&Text::from(
            crate::recipe::parse(&cached.original_recipe)?
                .json()
                .as_str(),
        ))?;
        let seat = recipe
            .get("seats")
            .and_then(domain::items)
            .and_then(|seats| {
                seats
                    .iter()
                    .find(|seat| domain::is_text(seat.get("id"), options.role.as_str()))
            });
        let recipe_tools = seat
            .and_then(|seat| seat.get("tools"))
            .map(|tools| {
                domain::items(tools)
                    .ok_or_else(|| denied("Recipe tools must be an explicit tool list"))?
                    .iter()
                    .map(|tool| text(tool, "Recipe tool name must be text"))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let target = locks::target("run", &root, &root, None)?;
        let no_hooks = target
            .parent()
            .ok_or_else(|| denied("Execution lock requires a parent"))?
            .join("no-hooks");
        if !secure::entries(&no_hooks)?.is_empty() {
            return Err(denied("Native Git hook-disable directory must be empty"));
        }
        if let Some(lease) = lease.as_ref() {
            backend::Owner::PrePlan(lease)
                .git(
                    &root,
                    &[
                        "check-ignore",
                        "--no-index",
                        "--quiet",
                        "--",
                        ".roster/locks/run.lock",
                    ]
                    .map(str::to_owned),
                    false,
                )
                .await?;
        } else {
            let mut probe = host.git_command(
                &root,
                &no_hooks,
                &[
                    "check-ignore",
                    "--no-index",
                    "--quiet",
                    "--",
                    ".roster/locks/run.lock",
                ]
                .map(str::to_owned),
                &BTreeMap::new(),
            );
            if let Err(failure) = process::execute(
                &mut probe,
                Limits {
                    timeout: Some(Duration::from_secs(30)),
                    max_output_bytes: 16_384,
                },
                Some(&token),
            )
            .await
            {
                if unverified_cleanup(&failure, false) {
                    return Err(Error::new("OwnedProcessCleanupError", failure.to_string())
                        .coded("ROSTER_PROCESS_CLEANUP"));
                }
                if let Some(reason) = token.reason()? {
                    return Err(reason.error());
                }
                if matches!(failure.kind, FailureKind::Exit(Some(1))) {
                    return Err(denied(
                        "Native issue runtime locks must already be Git-ignored; no state was written.",
                    ));
                }
                let mut error = Error::new(
                    "Error",
                    format!(
                        "Native Git preflight failed: {failure}; configure an installed Git executable and a valid worktree."
                    ),
                );
                if let Some(code) = failure.code() {
                    error = error.coded(code);
                }
                return Err(error);
            }
        }
        token.check()?;
        secure::check_components(&root)?;
        if paths::file_information(&root)?.identity != root_identity
            || paths::file_information(&marker)?.identity != git_identity
        {
            return Err(denied(
                "Worktree or Git marker identity changed before lock acquisition",
            ));
        }
        let images = match lease.as_ref() {
            Some(lease) => {
                lease.check()?;
                if lease.root() != root {
                    return Err(denied(
                        "Planning writer handoff belongs to another worktree",
                    ));
                }
                lease.images().to_vec()
            }
            None => crate::host::pins(&host)?,
        };
        let writer_instance = lease.as_ref().map_or_else(
            || std::sync::Arc::new(()),
            backend::RootLease::writer_identity,
        );
        let mut writer = match lease.take() {
            Some(lease) => Writer::PrePlan(Box::new(lease)),
            None => Writer::Direct(
                locks::acquire_run(
                    &target,
                    &locks::Options {
                        run_id: Some(assignment.session.clone()),
                        ..locks::Options::default()
                    },
                    Some(&token),
                )
                .await?,
            ),
        };
        let prepared = secure::private_directory(&no_hooks, true);
        if let Err(error) = prepared {
            return locks::finish(Err(error), writer.release_required().await);
        }
        let mut boundary = Self {
            root,
            root_identity,
            git_identity,
            git_marker,
            task,
            document,
            assignment,
            artifacts,
            writer,
            writer_instance,
            token,
            expanded: options.initial_scope_files.clone(),
            scope_blocked: Vec::new(),
            repair: options.initial_repair_files.clone(),
            initial_scope_expansion: options.scope_expansion,
            options,
            host,
            redaction,
            written: BTreeSet::new(),
            seen: BTreeMap::new(),
            searched: false,
            checkpoint: None,
            last_tests: None,
            full_verification: None,
            restore_generation: 0,
            context_generation: 0,
            git_seal: None,
            timings: None,
            fresh_after_restore: false,
            recipe_tools,
            no_hooks,
            cleanup_uncertain: Cell::new(false),
            owned_checks: false,
            images,
        };
        if let Err(error) = boundary.check() {
            let mut boundary = boundary;
            return locks::finish(Err(error), boundary.writer.release_required().await);
        }
        match restore::resume_requirements(&boundary) {
            Ok(requirements) => {
                boundary.restore_generation = requirements.generation;
                boundary.fresh_after_restore = requirements.fresh;
                if let Some(failures) = requirements.failures {
                    boundary.full_verification = Some(FullVerification {
                        latest: None,
                        failures: Some(failures),
                        pending: true,
                    });
                }
            }
            Err(error) => {
                boundary.quarantine_owned();
                return Err(error);
            }
        }
        Ok(boundary)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn role(&self) -> Role {
        self.options.role
    }
    pub fn task(&self) -> &Text {
        &self.task
    }
    pub fn document(&self) -> &task::Document {
        &self.document
    }
    pub fn assignment(&self) -> &lifecycle::Assignment {
        &self.assignment
    }
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
    pub fn redaction(&self) -> &redaction::Config {
        &self.redaction
    }
    pub(crate) fn host(&self) -> &Host {
        &self.host
    }
    pub(crate) fn quarantine_owned(&self) {
        self.cleanup_uncertain.set(true);
    }
    pub(crate) fn owned_cleanup_certain(&self) -> bool {
        !self.cleanup_uncertain.get()
    }
    pub(crate) fn writer_identity(&self) -> std::sync::Arc<()> {
        self.writer_instance.clone()
    }
    pub(crate) fn requires_read(&self, file: &str) -> bool {
        self.options
            .required_reads
            .iter()
            .any(|path| key(path) == key(file))
    }
    pub fn scope_files(&self) -> &[String] {
        &self.expanded
    }
    pub fn repair_files(&self) -> &[String] {
        &self.repair
    }
    pub fn written_files(&self) -> Vec<String> {
        self.written.iter().cloned().collect()
    }
    pub(crate) fn scope_blocked(&self) -> &[String] {
        &self.scope_blocked
    }

    pub(crate) fn bind_timings(&mut self, timings: super::timing::Timings) {
        self.timings = Some(timings);
    }

    pub(crate) fn span(&self, label: super::timing::Label) -> Option<super::timing::Span> {
        self.timings.as_ref().map(|timings| timings.start(label))
    }

    pub(crate) async fn admit_git(&mut self) -> Result<(), Error> {
        if let Some(seal) = &self.git_seal {
            seal.require_current(self).await?;
        } else {
            self.git_seal = Some(checkpoints::GitSeal::capture(self).await?);
        }
        Ok(())
    }

    pub(crate) fn git_validation(&self) -> Result<&Value, Error> {
        self.git_seal
            .as_ref()
            .map(checkpoints::GitSeal::validation)
            .ok_or_else(|| denied("Coder has no original private Git/index seal"))
    }

    pub(crate) fn require_git<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + 'a>> {
        Box::pin(async move {
            self.git_seal
                .as_ref()
                .ok_or_else(|| denied("Coder has no original private Git/index seal"))?
                .require_current(self)
                .await
        })
    }

    pub(crate) fn next_scope_budget(&self) -> Result<Option<usize>, Error> {
        self.check()?;
        let current = self.options.scope_expansion;
        if current == 0 || current >= 16 || self.scope_blocked.is_empty() {
            return Ok(None);
        }
        for file in &self.scope_blocked {
            if self.checked_product_write_path(file)? != *file
                || !policy::scope_expansion_file(file)
                || planning::is_allowed(file, &self.scope()?)
            {
                return Err(denied(
                    "Rescoping requires actual unprotected blocked product paths",
                ));
            }
        }
        let needed = policy::task_and_repair_files(
            &self.document.files_allowed,
            &self.repair,
            &[self.expanded.clone(), self.scope_blocked.clone()].concat(),
        )?;
        if needed.len() > MAX_SCOPE_ENTRIES {
            return Ok(None);
        }
        Ok(Some(16.min(
            (current * 2).max(current + self.scope_blocked.len() + 1),
        )))
    }

    pub(crate) fn continue_context(&mut self, scope_budget: Option<usize>) -> Result<(), Error> {
        self.check()?;
        if self.options.role != Role::Coder
            || self.context_generation == u32::MAX
            || scope_budget.is_some() && scope_budget != self.next_scope_budget()?
        {
            return Err(denied(
                "Fresh context requires current host-owned continuation authority",
            ));
        }
        if let Some(budget) = scope_budget {
            self.options.scope_expansion = budget;
        }
        self.context_generation += 1;
        self.last_tests = None;
        if let Some(record) = &mut self.full_verification {
            record.latest = None;
            record.pending = record.pending || record.failures.is_some();
        }
        self.scope_blocked.clear();
        self.seen.clear();
        self.searched = false;
        Ok(())
    }
    pub fn first_checkpoint(&self) -> Option<&Value> {
        self.checkpoint.as_ref()
    }
    pub fn last_tests(&self) -> Option<&TestEvidence> {
        self.last_tests.as_ref()
    }
    pub(crate) fn full_verification(&self) -> Option<&FullVerification> {
        self.full_verification.as_ref()
    }
    pub(crate) fn require_restore_capacity(&self) -> Result<(), Error> {
        self.check()?;
        if self.restore_generation >= crate::MAX_SAFE_INTEGER {
            return Err(denied("Restore revocation generation is exhausted"));
        }
        Ok(())
    }
    pub(crate) fn clear_restore_evidence(&mut self) {
        self.last_tests = None;
        self.fresh_after_restore = true;
        if let Some(record) = &mut self.full_verification {
            record.latest = None;
            record.pending = record.failures.is_some();
        }
    }
    pub(crate) fn revoke_after_restore(&mut self) -> Result<(), Error> {
        self.clear_restore_evidence();
        self.restore_generation = self
            .restore_generation
            .checked_add(1)
            .filter(|generation| *generation <= crate::MAX_SAFE_INTEGER)
            .ok_or_else(|| denied("Restore revocation generation is exhausted"))?;
        self.writer_instance = std::sync::Arc::new(());
        self.seen.clear();
        self.checkpoint = None;
        Ok(())
    }
    pub(crate) fn requires_restored_checks(&self) -> bool {
        self.fresh_after_restore
    }
    pub(crate) fn restore_generation(&self) -> u64 {
        self.restore_generation
    }

    #[cfg(test)]
    pub(crate) fn discard_node_coverage(&mut self) -> Result<(), Error> {
        let evidence = self
            .last_tests
            .as_mut()
            .ok_or_else(|| Error::new("Error", "No observed verification evidence to revoke"))?;
        evidence.coverage.node = false;
        Ok(())
    }

    pub fn scope(&self) -> Result<Vec<String>, Error> {
        policy::task_and_repair_files(&self.document.files_allowed, &self.repair, &self.expanded)
    }

    pub fn authority_digest(&self) -> String {
        let mut entries = self
            .artifacts
            .values()
            .map(|snapshot| {
                snapshot.as_ref().map_or(Value::Null, |snapshot| {
                    Value::string(&sha256_hex(&snapshot.bytes))
                })
            })
            .collect::<Vec<_>>();
        if self.restore_generation != 0 {
            entries.push(Value::object(vec![(
                "restore_generation",
                Value::Number(self.restore_generation as f64),
            )]));
        }
        if self.owned_checks {
            entries.push(Value::object(vec![("owned_checks", Value::Boolean(true))]));
        }
        if self.context_generation != 0 {
            entries.push(Value::object(vec![(
                "context_generation",
                Value::Number(f64::from(self.context_generation)),
            )]));
        }
        sha256_hex(Value::Array(entries).json().as_bytes())
    }

    pub fn check(&self) -> Result<(), Error> {
        if self.cleanup_uncertain.get() {
            return Err(Error::new("OwnedProcessCleanupError", "Owned child cleanup is unverified; the writer lock must remain held for inspection.").coded("ROSTER_PROCESS_CLEANUP"));
        }
        self.token.check()?;
        self.check_diagnostic_authority()
    }

    pub(crate) fn check_cleanup_ownership(&self) -> Result<(), Error> {
        self.writer.check()?;
        for image in &self.images {
            let scope = self.span(super::timing::Label::ProofHostImage);
            let result = image.require_current();
            super::timing::finish(scope, result.as_ref().err());
            result?;
        }
        secure::check_components(&self.root)?;
        if paths::file_information(&self.root)?.identity != self.root_identity {
            return Err(denied("Worktree identity changed; execution is revoked"));
        }
        let marker = self.root.join(".git");
        secure::check_components(&marker)?;
        if paths::file_information(&marker)?.identity != self.git_identity {
            return Err(denied(
                "Git worktree marker identity changed; execution is revoked",
            ));
        }
        if let Some(expected) = &self.git_marker {
            if !secure::unchanged(&marker, expected, 65_536)?
                || paths::file_information(&marker)?.links != 1
            {
                return Err(denied("Git worktree marker changed; execution is revoked"));
            }
        }
        Ok(())
    }

    pub(crate) fn check_diagnostic_authority(&self) -> Result<(), Error> {
        self.check_cleanup_ownership()?;
        for (name, expected) in &self.artifacts {
            match (expected, regular(&self.root, name, 65_536)?) {
                (None, None) => {}
                (Some(expected), Some(current))
                    if expected.identity == current.identity && expected.bytes == current.bytes => {
                }
                _ => {
                    return Err(denied(
                        "TASK, recipe or assignment changed; tools and late results are revoked",
                    ));
                }
            }
        }
        if !secure::entries(&self.no_hooks)?.is_empty() {
            return Err(denied(
                "Native Git hook-disable directory changed; execution is refused",
            ));
        }
        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        if self.cleanup_uncertain.get() {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Writer release is refused while owned child cleanup is unverified.",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        self.writer.release_required().await
    }

    fn normalized(&self, input: &str, directory: bool) -> Result<String, Error> {
        self.check()?;
        let normalized = input.replace('\\', "/");
        if Path::new(input).is_absolute() {
            let root = self.root.display().to_string().replace('\\', "/");
            let root_key = key(&root);
            let input_key = key(&normalized);
            if input_key.starts_with(&format!("{root_key}/")) {
                return Err(usage(format!(
                    "Use a worktree-relative path: \"{}\" instead of an absolute path.",
                    normalized.get(root.len() + 1..).unwrap_or("")
                )));
            }
        }
        if normalized.starts_with('/')
            || normalized.as_bytes().get(1) == Some(&b':')
            || normalized.split('/').any(|part| part == "..")
            || normalized
                .split('/')
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case("vendor"))
        {
            return Err(
                Error::new("OutsideWorktreeError", "Refused: outside the worktree.")
                    .coded("ROSTER_TOOL_DENIED"),
            );
        }
        if normalized == "." && directory {
            return Ok(normalized);
        }
        if !policy::exact_path(&normalized) {
            return Err(usage(
                "Tool path must not contain ambiguous Windows components or alternate data streams",
            ));
        }
        if planning::memory::redact(&Text::from(normalized.as_str()), &self.redaction)
            != Text::from(normalized.as_str())
        {
            return Err(denied("Tool paths must not contain secret material"));
        }
        if planning::forbidden_read(&normalized) || private_path(&normalized) {
            return Err(denied(
                "Tool access to secrets, Git metadata, policy, workflows, contracts, or debug logs is refused",
            ));
        }
        let target = self.root.join(&normalized);
        paths::ensure_local_path(&target, &self.root)
            .map_err(|cause| denied("Tool path containment failed").caused_by(cause))?;
        secure::check_components(&target).map_err(|cause| {
            denied("Tool paths may not traverse links or junctions").caused_by(cause)
        })?;
        Ok(normalized)
    }

    pub(crate) fn checked_product_write_path(&self, input: &str) -> Result<String, Error> {
        let file = self.normalized(input, false)?;
        if planning::forbidden_write(&file) || private_path(&file) || policy::managed(&file) {
            return Err(denied(format!(
                "Writing {file} is not allowed by TASK.md or worktree policy"
            )));
        }
        Ok(file)
    }

    fn readable(&self, file: &str, directory: bool) -> Result<bool, Error> {
        if policy::readme_only(&self.document.files_allowed) && self.options.role == Role::Coder {
            return Ok(["TASK.md", "README.md"].contains(&file)
                || self.repair.iter().any(|path| path == file)
                || !directory
                    && !planning::forbidden_read(file)
                    && !private_path(file)
                    && self.task.to_utf8().is_ok_and(|t| t.contains(file)));
        }
        if self.options.role == Role::Reviewer
            || !self.options.slice_reads_only
            || file == "TASK.md"
        {
            return Ok(true);
        }
        let scope = self.scope()?;
        let is_report_slice = !scope.is_empty()
            && !policy::readme_only(&self.document.files_allowed)
            && scope.iter().all(|f| {
                let l = f.to_lowercase();
                l.ends_with(".html")
                    || (l.ends_with(".md")
                        && (l.contains("report")
                            || l.contains("audit")
                            || l.contains("summary")
                            || l.contains("evaluation")
                            || l.contains("inventory")))
            });
        if is_report_slice && !planning::forbidden_read(file) && !private_path(file) {
            return Ok(true);
        }
        Ok(planning::is_allowed(file, &scope)
            || self
                .options
                .required_reads
                .iter()
                .any(|path| key(path) == key(file))
            || directory
                && (file == "."
                    || scope
                        .iter()
                        .any(|allowed| allowed.starts_with(&format!("{file}/")))
                    || self
                        .task
                        .to_utf8()
                        .is_ok_and(|t| t.contains(&format!("{file}/"))))
            || self.imported_by_allowed(file)?
            || !directory && self.task.to_utf8().is_ok_and(|t| t.contains(file)))
    }

    fn imported_by_allowed(&self, file: &str) -> Result<bool, Error> {
        if !file.ends_with(".mjs") && !file.ends_with(".js") {
            return Ok(false);
        }
        let pattern = regex(r#"from\s+['"](\.[^'"]+)['"]"#)?;
        for allowed in self.scope()? {
            if allowed.contains('*') || planning::forbidden_read(&allowed) {
                continue;
            }
            let Some(snapshot) = regular(&self.root, &allowed, MAX_FILE_BYTES)? else {
                continue;
            };
            let source = bytes_text(&snapshot.bytes)?;
            for captures in pattern.captures_iter(&source) {
                let Some(relative) = captures.get(1) else {
                    continue;
                };
                let target = paths::resolve(
                    self.root.join(&allowed).parent().unwrap_or(&self.root),
                    Path::new(relative.as_str()),
                )?;
                if let Ok(relative) = target.strip_prefix(&self.root) {
                    let mut relative = relative
                        .to_str()
                        .ok_or_else(|| Error::type_error("Imported path must be Unicode"))?
                        .replace('\\', "/");
                    if !regex(r"\.[cm]?js$")?.is_match(&relative) {
                        relative.push_str(".mjs");
                    }
                    if key(&relative) == key(file) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn read_scope(&self, file: &str, directory: bool) -> Result<(), Error> {
        if !self.readable(file, directory)? {
            return Err(usage(format!(
                "Reading {file} is not allowed by TASK.md slice scope"
            )));
        }
        Ok(())
    }

    fn read_file(&mut self, args: &Value) -> Result<Value, Error> {
        let (file, snapshot, value) = self.read_file_value(args, false)?;
        self.seen.insert(key(&file), snapshot);
        Ok(value)
    }

    fn read_file_value(
        &self,
        args: &Value,
        reviewer: bool,
    ) -> Result<(String, secure::Snapshot, Value), Error> {
        fields(
            args,
            &["path"],
            &["max_lines", "offset", "start_line"],
            "Tool arguments must contain path",
        )?;
        let offset_arg = args.get("offset").or_else(|| args.get("start_line"));
        let mut counts = Vec::new();
        for (val_opt, message) in [
            (
                args.get("max_lines"),
                "read_file max_lines must be a positive safe integer",
            ),
            (
                offset_arg,
                "read_file offset must be a positive safe integer line number",
            ),
        ] {
            counts.push(if val_opt.is_some() {
                Some(
                    domain::safe_integer(val_opt)
                        .filter(|value| *value > 0)
                        .ok_or_else(|| Error::type_error(message))?,
                )
            } else {
                None
            });
        }
        let file = self.normalized(
            &text(required(args, "path")?, "Tool path must be text")?,
            false,
        )?;
        if !reviewer {
            self.read_scope(&file, false)?;
        }
        let target_path = self.root.join(&file);
        if let Ok(meta) = fs::symlink_metadata(&target_path) {
            if !secure::linked(&meta) && meta.is_dir() {
                return Err(usage(format!(
                    "read_file: {file} is a directory; use list_dir to inspect directories"
                )));
            }
        }
        let snapshot = regular(&self.root, &file, MAX_FILE_BYTES)?.ok_or_else(|| Error::new("Error", format!("read_file: {file} does not exist. Use list_dir or search_text to find the right path")).coded("ENOENT"))?;
        let source = bytes_text(&snapshot.bytes)?;
        self.check()?;
        let source =
            planning::memory::redact(&Text::from(source.as_str()), &self.redaction).to_utf8()?;
        if counts.iter().all(Option::is_none) {
            return Ok((file, snapshot, Value::string(&source)));
        }
        let offset = counts.get(1).copied().flatten().unwrap_or(1) - 1;
        let count = counts.first().copied().flatten().unwrap_or(u64::MAX);
        Ok((
            file,
            snapshot,
            Value::string(
                &source
                    .replace("\r\n", "\n")
                    .split('\n')
                    .skip(usize::try_from(offset).unwrap_or(usize::MAX))
                    .take(usize::try_from(count).unwrap_or(usize::MAX))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ))
    }

    fn list(&self, file: &str) -> Result<Vec<(String, String, bool)>, Error> {
        let target = if file == "." {
            self.root.clone()
        } else {
            self.root.join(file)
        };
        secure::check_components(&target)?;
        if !target.exists() {
            return Err(Error::new(
                "Error",
                format!(
                    "list_dir: {file} does not exist. Use list_dir on '.' to see existing directories"
                ),
            )
            .coded("ENOENT"));
        }
        if let Ok(meta) = fs::symlink_metadata(&target) {
            if !secure::linked(&meta) && meta.is_file() {
                return Err(usage(format!(
                    "list_dir: {file} is a regular file; use read_file instead"
                )));
            }
        }
        let mut result = Vec::new();
        for entry in fs::read_dir(&target)
            .map_err(|error| secure::io_error("List tool directory", &target, error))?
        {
            let entry = entry
                .map_err(|error| secure::io_error("Read tool directory entry", &target, error))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::type_error("Tool filenames must be Unicode"))?;
            let child = if file == "." {
                name.clone()
            } else {
                format!("{file}/{name}")
            };
            if planning::forbidden_read(&child)
                || private_path(&child)
                || name == "node_modules"
                || name == ".github"
            {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| secure::io_error("Inspect tool entry", &entry.path(), error))?;
            if secure::linked(&metadata) {
                continue;
            }
            if !self.readable(&child, metadata.is_dir())? {
                continue;
            }
            if !metadata.is_file() && !metadata.is_dir() {
                return Err(denied("Tool search refuses special filesystem entries"));
            }
            result.push((name, child, metadata.is_dir()));
            if result.len() > MAX_FILES {
                return Err(usage("Tool directory exceeds 2048 entries"));
            }
        }
        result.sort_by(|left, right| left.0.encode_utf16().cmp(right.0.encode_utf16()));
        Ok(result)
    }

    fn list_dir(&mut self, args: &Value) -> Result<Value, Error> {
        fields(
            args,
            &[],
            &["path"],
            "Tool arguments must contain no fields",
        )?;
        if policy::readme_only(&self.document.files_allowed) && self.options.role == Role::Coder {
            return Err(usage(
                "README-only docs task does not allow directory listing",
            ));
        }
        let file = args
            .get("path")
            .map(|value| text(value, "Tool path must be text"))
            .transpose()?
            .unwrap_or_else(|| ".".to_owned());
        let file = self.normalized(&file, true)?;
        self.read_scope(&file, true)?;
        self.searched = true;
        let value = Value::Array(
            self.list(&file)?
                .iter()
                .map(|(name, _, directory)| {
                    Value::object(vec![
                        ("name", Value::string(name)),
                        (
                            "type",
                            Value::string(if *directory { "directory" } else { "file" }),
                        ),
                    ])
                })
                .collect(),
        );
        self.check()?;
        Ok(value)
    }

    fn readable_files(&self, file: &str) -> Result<Vec<String>, Error> {
        let target = if file == "." {
            self.root.clone()
        } else {
            self.root.join(file)
        };
        secure::check_components(&target)?;
        let metadata = fs::symlink_metadata(&target)
            .map_err(|error| secure::io_error("Inspect search path", &target, error))?;
        self.read_scope(file, metadata.is_dir())?;
        if metadata.is_file() {
            return Ok(vec![file.to_owned()]);
        }
        let mut queue = vec![file.to_owned()];
        let mut files = Vec::new();
        let mut directories = 0;
        while let Some(directory) = queue.pop() {
            self.check()?;
            directories += 1;
            if directories > MAX_FILES {
                return Err(usage("Tool traversal exceeds 2048 directories"));
            }
            for (_, file, directory) in self.list(&directory)? {
                if directory {
                    queue.push(file);
                } else {
                    files.push(file);
                }
                if files.len() > MAX_FILES {
                    return Err(usage("Tool traversal exceeds 2048 files"));
                }
            }
        }
        files.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
        Ok(files)
    }

    fn search_text(&mut self, args: &Value) -> Result<Value, Error> {
        let mut normalized_search_args = None;
        if args.get("query").is_none() && args.get("pattern").is_some() {
            let mut cloned = args.clone();
            if let Some(pat) = args.get("pattern").cloned() {
                let _ = domain::put(&mut cloned, "query", pat);
            }
            let _ = domain::remove(&mut cloned, "pattern");
            normalized_search_args = Some(cloned);
        }
        let args = normalized_search_args.as_ref().unwrap_or(args);
        fields(
            args,
            &["query"],
            &["path"],
            "Tool arguments must contain query",
        )?;
        let query = text(
            required(args, "query")?,
            "search_text query must be a nonempty single-line string",
        )?;
        if query.is_empty() || query.contains(['\0', '\r', '\n']) {
            return Err(Error::type_error(
                "search_text query must be a nonempty single-line string",
            ));
        }
        if policy::readme_only(&self.document.files_allowed) && self.options.role == Role::Coder {
            return Err(usage(
                "README-only docs task does not allow repository search; read README.md directly",
            ));
        }
        let file = args
            .get("path")
            .map(|value| text(value, "Tool path must be text"))
            .transpose()?
            .unwrap_or_else(|| ".".to_owned());
        let file = self.normalized(&file, true)?;
        self.searched = true;
        let mut matches = Vec::new();
        let mut total = 0_u64;
        for file in self.readable_files(&file)? {
            let result = match self.read_file(&Value::object(vec![("path", Value::string(&file))]))
            {
                Ok(v) => v,
                Err(err) if err.message.contains("UTF-8") => continue,
                Err(err) => return Err(err),
            };
            let source = text(&result, "Search input must be text")?;
            total += source.len() as u64;
            if total > MAX_SNAPSHOT_BYTES {
                return Err(usage("Tool search exceeds 128 MiB"));
            }
            if source.contains('\0') {
                continue;
            }
            for (index, line) in source.replace("\r\n", "\n").split('\n').enumerate() {
                if line.contains(&query) {
                    matches.push(Value::object(vec![
                        ("path", Value::string(&file)),
                        ("line", Value::Number((index + 1) as f64)),
                        ("text", Value::string(line)),
                    ]));
                    if matches.len() > 50 {
                        break;
                    }
                }
            }
            if matches.len() > 50 {
                break;
            }
        }
        let truncated = matches.len() > 50;
        matches.truncate(50);
        self.check()?;
        Ok(Value::object(vec![
            ("matches", Value::Array(matches)),
            ("truncated", Value::Boolean(truncated)),
        ]))
    }

    fn glob_files(&mut self, args: &Value) -> Result<Value, Error> {
        fields(
            args,
            &["pattern"],
            &[],
            "Tool arguments must contain pattern",
        )?;
        let pattern = text(
            required(args, "pattern")?,
            "glob_files pattern must be a nonempty single-line string",
        )?;
        if pattern.is_empty() || pattern.contains(['\0', '\r', '\n']) {
            return Err(Error::type_error(
                "glob_files pattern must be a nonempty single-line string",
            ));
        }
        if policy::readme_only(&self.document.files_allowed) && self.options.role == Role::Coder {
            return Err(usage(
                "README-only docs task does not allow directory listing",
            ));
        }
        let expression = regex(&format!(
            "^{}$",
            pattern
                .split('*')
                .map(regex_lite::escape)
                .collect::<Vec<_>>()
                .join(".*")
        ))?;
        self.searched = true;
        let mut found = self
            .readable_files(".")?
            .into_iter()
            .filter(|file| expression.is_match(file))
            .take(101)
            .collect::<Vec<_>>();
        let truncated = found.len() > 100;
        found.truncate(100);
        self.check()?;
        Ok(Value::object(vec![
            ("paths", strings(&found)),
            ("truncated", Value::Boolean(truncated)),
        ]))
    }

    fn require_seen(
        &self,
        file: &str,
        previous: Option<&secure::Snapshot>,
        replacing: bool,
    ) -> Result<(), Error> {
        if let Some(previous) = previous {
            let requires_read = replacing && regex(r"\.(?:[cm]?[jt]sx?|rs)$")?.is_match(file);
            let seen = self.seen.get(&key(file));
            if requires_read
                && seen.is_none_or(|seen| {
                    seen.identity != previous.identity || seen.bytes != previous.bytes
                })
            {
                return Err(usage(format!(
                    "Read {file} with read_file before changing it; the last read must match current file identity and bytes"
                )));
            }
        } else if file.starts_with("src/") || file.starts_with("rust/") {
            if !self.searched
                && !self.options.slice_reads_only
                && self.document.files_allowed.len() > 1
            {
                return Err(usage(format!(
                    "Before creating {file}, use search_text, glob_files, or list_dir to find any existing module that already provides this behavior."
                )));
            }
            let unread = self
                .options
                .required_reads
                .iter()
                .filter(|required| !self.seen.contains_key(&key(required)))
                .cloned()
                .collect::<Vec<_>>();
            if !unread.is_empty() {
                return Err(usage(format!(
                    "Before creating {file}, read the modules earlier waves delivered: {}. Import and extend them instead of re-implementing their behavior.",
                    unread.join(", ")
                )));
            }
        }
        if replacing {
            self.check()?;
        }
        Ok(())
    }

    async fn prepare_write<O: Observer>(
        &mut self,
        file: &str,
        observer: &mut O,
    ) -> Result<(), Error> {
        self.check()?;
        self.require_ignored(".roster/locks/run.lock").await?;
        if self.options.role != Role::Coder {
            return Err(denied("The reviewer is read-only"));
        }
        if self.checked_product_write_path(file)? != file {
            return Err(denied(
                "Prepared product writes require exact normalized worktree paths",
            ));
        }
        if !planning::is_allowed(file, &self.scope()?) {
            if !policy::scope_expansion_file(file)
                || policy::readme_only(&self.document.files_allowed)
            {
                return Err(denied(format!(
                    "Writing {file} is not allowed by TASK.md or worktree policy"
                )));
            }
            if self.expanded.len() >= self.options.scope_expansion {
                if !self.scope_blocked.contains(&file.to_owned()) {
                    self.scope_blocked.push(file.to_owned());
                }
                return Err(usage(format!(
                    "Scope expansion limit ({} files outside TASK.md) reached; cannot also write {file}. Re-scope from recorded evidence.",
                    self.options.scope_expansion
                )));
            }
            let mut prospective = self.expanded.clone();
            prospective.push(file.to_owned());
            if policy::task_and_repair_files(
                &self.document.files_allowed,
                &self.repair,
                &prospective,
            )?
            .len()
                > MAX_SCOPE_ENTRIES
            {
                if !self.scope_blocked.contains(&file.to_owned()) {
                    self.scope_blocked.push(file.to_owned());
                }
                return Err(usage(
                    "Aggregate task/repair/expanded scope exceeds 64 entries; re-scope without discarding recorded paths",
                ));
            }
            self.expanded = prospective;
            observer
                .emit(Value::object(vec![
                    ("type", Value::string("scope-expansion")),
                    ("path", Value::string(file)),
                    ("count", Value::Number(self.expanded.len() as f64)),
                    ("limit", Value::Number(self.options.scope_expansion as f64)),
                ]))
                .await?;
            self.check()?;
        }
        if self.checkpoint.is_none() {
            self.checkpoint = Some(checkpoints::capture(self, false).await?);
        }
        self.check()?;
        self.last_tests = None;
        Ok(())
    }

    fn replace(
        &self,
        file: &str,
        previous: Option<&secure::Snapshot>,
        content: &[u8],
    ) -> Result<(), Error> {
        self.check()?;
        let target = self.root.join(file);
        paths::ensure_local_path(&target, &self.root)?;
        secure::check_components(&target)?;
        if content.len() as u64 > MAX_FILE_BYTES {
            return Err(usage("Tool file exceeds 16 MiB"));
        }
        let parent = target
            .parent()
            .ok_or_else(|| denied("Tool file requires a parent"))?;
        files::create_private_directories(parent)?;
        paths::ensure_local_path(&target, &self.root)?;
        secure::check_components(&target)?;
        self.check()?;
        if let Some(previous) = previous {
            let mut handle = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&target)
                .map_err(|error| secure::io_error("Open edit target", &target, error))?;
            let information = paths::handle_information(&handle)?;
            let mut bytes = Vec::new();
            (&mut handle)
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| secure::io_error("Verify edit target", &target, error))?;
            if information.links != 1
                || information.identity != previous.identity
                || bytes != previous.bytes
                || !secure::unchanged(&target, previous, MAX_FILE_BYTES)?
            {
                return Err(denied("Edit target changed before writing; re-read it"));
            }
            self.check()?;
            secure::check_components(&target)?;
            handle
                .seek(SeekFrom::Start(0))
                .and_then(|_| handle.write_all(content))
                .and_then(|()| handle.set_len(content.len() as u64))
                .and_then(|()| handle.sync_all())
                .map_err(|error| secure::io_error("Write edit target", &target, error))?;
            if paths::file_information(&target)?.identity != information.identity {
                return Err(denied(
                    "Edit target was replaced during writing; the result is unverified",
                ));
            }
        } else {
            secure::write_exclusive(&target, content)?;
        }
        self.check()?;
        Ok(())
    }

    fn expansion_note(&self, file: &str, result: &mut Value) -> Result<(), Error> {
        if self.expanded.iter().any(|entry| key(entry) == key(file)) {
            domain::put(result, "scope_expanded", Value::Boolean(true))?;
            domain::put(result, "scope_path", Value::string(file))?;
            domain::put(
                result,
                "note",
                Value::string(&format!(
                    "{file} is outside planned TASK.md scope ({} of {} expansions). Justify it in your summary; the reviewer judges it and publication lists it.",
                    self.expanded.len(),
                    self.options.scope_expansion
                )),
            )?;
        }
        Ok(())
    }

    async fn mutate<O: Observer>(
        &mut self,
        name: &str,
        args: &Value,
        observer: &mut O,
    ) -> Result<Value, Error> {
        let mut normalized_edit_args = None;
        if name == "edit_file"
            && args.get("old_string").is_none()
            && args.get("old_str").is_some()
            && args.get("new_str").is_some()
        {
            let mut cloned = args.clone();
            if let Some(os) = args.get("old_str").cloned() {
                let _ = domain::put(&mut cloned, "old_string", os);
            }
            if let Some(ns) = args.get("new_str").cloned() {
                let _ = domain::put(&mut cloned, "new_string", ns);
            }
            let _ = domain::remove(&mut cloned, "old_str");
            let _ = domain::remove(&mut cloned, "new_str");
            normalized_edit_args = Some(cloned);
        }
        let args = normalized_edit_args.as_ref().unwrap_or(args);
        let mandatory: &[&str] = match name {
            "write_file" => &["path", "content"],
            "edit_file" => &["path", "old_string", "new_string"],
            "delete_file" => &["path"],
            _ => return Err(usage("Unknown mutation tool")),
        };
        fields(
            args,
            mandatory,
            &[],
            &format!("Tool arguments must contain {}", mandatory.join(", ")),
        )?;
        let file = self.checked_product_write_path(&text(
            required(args, "path")?,
            "Tool path must be text",
        )?)?;
        let previous = regular(&self.root, &file, MAX_FILE_BYTES)?;
        self.require_seen(
            &file,
            previous.as_ref(),
            self.owned_checks || name == "write_file",
        )?;
        let mut content = None;
        match name {
            "write_file" => {
                let mut value = text(
                    required(args, "content")?,
                    "write_file content must be text",
                )?;
                if let Some(previous) = &previous {
                    let old = bytes_text(&previous.bytes)?;
                    if !file.ends_with(".md")
                        && old.encode_utf16().count() > 2000
                        && value.encode_utf16().count() * 10 < old.encode_utf16().count() * 6
                    {
                        return Err(usage(
                            "Refusing to replace an existing file with a much shorter rewrite. Use edit_file with the exact old text.",
                        ));
                    }
                    if !value.ends_with('\n') && old.ends_with('\n') {
                        value.push('\n');
                    }
                }
                content = Some(value);
            }
            "edit_file" => {
                let old = text(
                    required(args, "old_string")?,
                    "edit_file requires a nonempty old_string and a new_string",
                )?;
                let new = text(
                    required(args, "new_string")?,
                    "edit_file requires a nonempty old_string and a new_string",
                )?;
                if old.is_empty() {
                    return Err(Error::type_error(
                        "edit_file requires a nonempty old_string and a new_string",
                    ));
                }
                if old == new {
                    return Err(usage("edit_file old_string and new_string are identical"));
                }
                let source = bytes_text(
                    &previous
                        .as_ref()
                        .ok_or_else(|| usage("edit_file requires an existing regular file"))?
                        .bytes,
                )?;
                let mut count = source.matches(&old).count();
                if count == 1 {
                    content = Some(source.replacen(&old, &new, 1));
                } else {
                    let source = source.replace("\r\n", "\n");
                    let old = old.replace("\r\n", "\n");
                    let new = new.replace("\r\n", "\n");
                    count = source.matches(&old).count();
                    if count == 1 {
                        content = Some(source.replacen(&old, &new, 1));
                    }
                    if count == 0 {
                        let tolerant = policy::whitespace_edit(&source, &old, &new)?;
                        count = domain::safe_integer(tolerant.get("count")).unwrap_or(0) as usize;
                        content = tolerant
                            .get("content")
                            .map(|value| text(value, "Edit content requires text"))
                            .transpose()?;
                    }
                }
                if count != 1 {
                    return Err(usage(if count == 0 {
                        "edit_file old_string was not found. Read the file and copy the exact text."
                    } else {
                        "edit_file old_string matched more than once. Include more surrounding lines."
                    }));
                }
            }
            "delete_file" if previous.is_none() => {
                return Err(usage(format!("delete_file: {file} does not exist")));
            }
            _ => {}
        }
        if let Some(content) = &content {
            if content.len() as u64 > MAX_FILE_BYTES {
                return Err(usage("Tool file exceeds 16 MiB"));
            }
            if planning::memory::redact(&Text::from(content.as_str()), &self.redaction)
                != Text::from(content.as_str())
            {
                return Err(denied(
                    "Tool writes must not contain credentials or private keys",
                ));
            }
        }
        self.prepare_write(&file, observer).await?;
        self.check()?;
        let mut result = if name == "delete_file" {
            let previous = previous
                .as_ref()
                .ok_or_else(|| usage("delete_file requires a regular file"))?;
            if !secure::unchanged(&self.root.join(&file), previous, MAX_FILE_BYTES)?
                || paths::file_information(&self.root.join(&file))?.links != 1
            {
                return Err(denied("Delete target changed before removal"));
            }
            self.check()?;
            fs::remove_file(self.root.join(&file)).map_err(|error| {
                secure::io_error("Delete tool file", &self.root.join(&file), error)
            })?;
            Value::object(vec![
                ("path", Value::string(&file)),
                ("deleted", Value::Boolean(true)),
            ])
        } else {
            let content = content
                .ok_or_else(|| Error::type_error("Mutation did not produce file contents"))?;
            self.replace(&file, previous.as_ref(), content.as_bytes())?;
            if let Some(snapshot) = regular(&self.root, &file, MAX_FILE_BYTES)? {
                self.seen.insert(key(&file), snapshot);
            }
            if name == "edit_file" {
                Value::object(vec![
                    ("path", Value::string(&file)),
                    ("replacements", Value::Number(1.0)),
                ])
            } else {
                Value::object(vec![
                    ("path", Value::string(&file)),
                    ("bytes", Value::Number(content.len() as f64)),
                ])
            }
        };
        self.written.insert(file.clone());
        self.expansion_note(&file, &mut result)?;
        self.require_ignored(".roster/locks/run.lock").await?;
        self.check()?;
        Ok(result)
    }

    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let _scope = self.span(super::timing::Label::ProofSource);
        self.check()?;
        let scope = self.scope()?;
        let mut queue = vec![".".to_owned()];
        let mut products = BTreeMap::new();
        let mut total = 0_u64;
        let mut directories = 0;
        while let Some(directory) = queue.pop() {
            directories += 1;
            if directories > MAX_FILES {
                return Err(denied("Checkpoint traversal exceeds 2048 directories"));
            }
            let target = if directory == "." {
                self.root.clone()
            } else {
                self.root.join(&directory)
            };
            secure::check_components(&target)?;
            for entry in fs::read_dir(&target)
                .map_err(|error| secure::io_error("Read product scope", &target, error))?
            {
                let entry = entry
                    .map_err(|error| secure::io_error("Read product entry", &target, error))?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::type_error("Product filename must be Unicode"))?;
                let file = if directory == "." {
                    name.clone()
                } else {
                    format!("{directory}/{name}")
                };
                if planning::forbidden_read(&file)
                    || planning::forbidden_write(&file)
                    || private_path(&file)
                    || policy::managed(&file)
                    || ["node_modules", ".worktrees"].contains(&name.as_str())
                {
                    continue;
                }
                let entry = fs::symlink_metadata(entry.path()).map_err(|error| {
                    secure::io_error("Inspect product entry", &self.root.join(&file), error)
                })?;
                let allowed = planning::is_allowed(&file, &scope);
                if secure::linked(&entry) {
                    if allowed
                        || scope
                            .iter()
                            .any(|allowed| allowed.starts_with(&format!("{file}/")))
                    {
                        return Err(denied(
                            "Checkpoint product paths may not be symlinks or junctions",
                        ));
                    }
                    continue;
                }
                if entry.is_dir() {
                    if allowed
                        || scope.iter().any(|allowed| {
                            allowed == "**/*" || allowed.starts_with(&format!("{file}/"))
                        })
                    {
                        queue.push(file);
                    }
                } else if allowed {
                    if !entry.is_file() {
                        return Err(denied("Checkpoint product must be a regular file"));
                    }
                    let snapshot = regular(&self.root, &file, MAX_FILE_BYTES)?
                        .ok_or_else(|| denied("Checkpoint product disappeared"))?;
                    let source = bytes_text(&snapshot.bytes)?;
                    if planning::memory::redact(&Text::from(source.as_str()), &self.redaction)
                        != Text::from(source.as_str())
                    {
                        return Err(denied(
                            "Checkpoint refuses product files containing secret material",
                        ));
                    }
                    total += snapshot.bytes.len() as u64;
                    if total > MAX_SNAPSHOT_BYTES || products.len() == MAX_FILES {
                        return Err(denied(
                            "Checkpoint product scope exceeds its file or byte bound",
                        ));
                    }
                    #[cfg(unix)]
                    let executable = {
                        use std::os::unix::fs::PermissionsExt;
                        entry.permissions().mode() & 0o111 != 0
                    };
                    #[cfg(not(unix))]
                    let executable = false;
                    products.insert(
                        file,
                        Product {
                            digest: sha256_hex(&snapshot.bytes),
                            mode: if executable { "100755" } else { "100644" }.to_owned(),
                            identity: snapshot.identity,
                        },
                    );
                }
            }
        }
        self.check()?;
        Ok(Snapshot { files: products })
    }

    pub(crate) fn audit(&self) -> Result<BTreeMap<String, String>, Error> {
        let _scope = self.span(super::timing::Label::SourceAudit);
        self.check()?;
        let excluded_target = self
            .host
            .cargo
            .as_ref()
            .and_then(|approval| approval.environment.get("CARGO_TARGET_DIR"))
            .map(|target| paths::resolve(&self.root, Path::new(target)))
            .transpose()?;
        if excluded_target
            .as_ref()
            .is_some_and(|target| target == &self.root)
        {
            return Err(denied(
                "Cargo target output must not replace the worktree root",
            ));
        }
        if let Some(target) = &excluded_target {
            if let Ok(relative) = target.strip_prefix(&self.root) {
                let relative = relative
                    .to_str()
                    .ok_or_else(|| Error::type_error("Cargo target must be Unicode"))?
                    .replace('\\', "/");
                if private_path(&relative)
                    || planning::forbidden_write(&relative)
                    || self.scope()?.iter().any(|file| {
                        planning::is_allowed(&relative, std::slice::from_ref(file))
                            || file.starts_with(&format!("{relative}/"))
                    })
                {
                    return Err(denied(
                        "Cargo target output must not overlap product or protected scope",
                    ));
                }
            }
        }
        let mut queue = vec![self.root.clone()];
        let mut audit = BTreeMap::new();
        let mut visited = 0;
        let mut bytes = 0_u64;
        while let Some(directory) = queue.pop() {
            self.check()?;
            secure::check_components(&directory)?;
            for entry in fs::read_dir(&directory)
                .map_err(|error| secure::io_error("Audit worktree", &directory, error))?
            {
                let entry = entry
                    .map_err(|error| secure::io_error("Read audit entry", &directory, error))?;
                let target = entry.path();
                let relative = target
                    .strip_prefix(&self.root)
                    .map_err(|_| denied("Audit entry left the worktree"))?;
                let file = relative
                    .to_str()
                    .ok_or_else(|| Error::type_error("Audit filenames must be Unicode"))?
                    .replace('\\', "/");
                let first = file.split('/').next().unwrap_or("");
                if [".git", "node_modules", "target", ".worktrees"].contains(&first)
                    || [".roster/locks", ".roster/checkpoints"]
                        .iter()
                        .any(|prefix| file == *prefix || file.starts_with(&format!("{prefix}/")))
                    || policy::managed(&file)
                    || excluded_target
                        .as_ref()
                        .is_some_and(|excluded| target == *excluded)
                {
                    continue;
                }
                visited += 1;
                if visited > 8192 {
                    return Err(denied("Verification worktree audit exceeds 8192 entries"));
                }
                let metadata = fs::symlink_metadata(&target)
                    .map_err(|error| secure::io_error("Inspect audit entry", &target, error))?;
                let information = paths::file_information(&target)?;
                #[cfg(unix)]
                let stamp = {
                    use std::os::unix::fs::MetadataExt;
                    format!(
                        "{:?}:{}:{}:{}:{}:{}:{}",
                        information.identity,
                        metadata.len(),
                        metadata.mode(),
                        metadata.mtime(),
                        metadata.mtime_nsec(),
                        metadata.ctime(),
                        metadata.ctime_nsec()
                    )
                };
                #[cfg(windows)]
                let stamp = {
                    use std::os::windows::fs::MetadataExt;
                    format!(
                        "{:?}:{}:{}:{}:{}",
                        information.identity,
                        metadata.len(),
                        metadata.file_attributes(),
                        metadata.creation_time(),
                        metadata.last_write_time()
                    )
                };
                if secure::linked(&metadata) {
                    audit.insert(file, format!("linked:{stamp}"));
                } else if metadata.is_dir() {
                    if (planning::forbidden_read(&file) || private_path(&file))
                        && file != ".roster"
                        && file != ".roster-state"
                    {
                        audit.insert(file, format!("protected:{stamp}"));
                    }
                    queue.push(target);
                } else if planning::forbidden_read(&file) || private_path(&file) {
                    audit.insert(file, format!("protected:{stamp}"));
                } else if metadata.is_file() {
                    let snapshot = regular(&self.root, &file, MAX_FILE_BYTES)?
                        .ok_or_else(|| denied("Audit file disappeared"))?;
                    bytes += snapshot.bytes.len() as u64;
                    if bytes > MAX_SNAPSHOT_BYTES {
                        return Err(denied("Verification worktree audit exceeds 128 MiB"));
                    }
                    audit.insert(
                        file,
                        format!("{:?}:{}", snapshot.identity, sha256_hex(&snapshot.bytes)),
                    );
                } else {
                    audit.insert(file, format!("special:{stamp}"));
                }
            }
        }
        self.check()?;
        Ok(audit)
    }

    pub(crate) async fn process(
        &self,
        executable: &Path,
        args: &[String],
        extra_env: &BTreeMap<String, String>,
        timeout: u64,
        limit: usize,
    ) -> Result<ObservedProcess, Error> {
        self.check()?;
        let family = if self.host.node20.as_deref() == Some(executable) {
            Some(super::timing::Label::OwnedNode)
        } else if self
            .host
            .cargo
            .as_ref()
            .is_some_and(|approval| approval.executable == executable)
        {
            Some(super::timing::Label::OwnedCargo)
        } else {
            None
        };
        if self.owned_checks
            && (self.host.node20.as_deref() == Some(executable)
                || self
                    .host
                    .cargo
                    .as_ref()
                    .is_some_and(|approval| approval.executable == executable))
        {
            let mut pending = PendingCheck {
                boundary: self,
                settled: false,
            };
            let scope = family.and_then(|label| self.span(label));
            let run = backend::Owner::Execution(self)
                .run(&self.root, executable, args, extra_env, timeout, limit)
                .await;
            super::timing::finish(scope, run.as_ref().err());
            pending.settled = true;
            let run = run?;
            self.check()?;
            if run.timed_out {
                return Err(Error::new("OwnedCheckTimeout", "Owned matching verification timed out; its complete process tree was stopped and the attempt remains unverified").coded("ETIMEDOUT"));
            }
            if run.overflow {
                return Err(Error::new(
                    "OwnedCheckOverflow",
                    "Owned matching verification exceeded its output limit and remains unverified",
                )
                .coded("ERR_CHILD_PROCESS_STDIO_MAXBUFFER"));
            }
            let exit = run.exit.filter(|exit| (0..=255).contains(exit))
                .ok_or_else(|| Error::new("OwnedCheckExit", "Owned verification has no ordinary bounded exit; no passing check is available"))?;
            return Ok(ObservedProcess {
                exit,
                stdout: run.stdout,
                stderr: run.stderr,
            });
        }
        let mut command = self.host.command(&self.root, executable, args, extra_env);
        let scope = family.and_then(|label| self.span(label));
        let output = process::execute_timed(
            &mut command,
            Limits {
                timeout: Some(Duration::from_millis(timeout)),
                max_output_bytes: limit,
            },
            Some(&self.token),
            scope.as_ref(),
        )
        .await;
        if let Some(scope) = scope {
            scope.finish_status(output.is_ok());
        }
        let observed = match output {
            Ok(output) => ObservedProcess {
                exit: 0,
                stdout: output.captured.stdout,
                stderr: output.captured.stderr,
            },
            Err(failure) => {
                let descendants_possible = args.first().is_some_and(|arg| arg != "--version")
                    && (self.host.node20.as_deref() == Some(executable)
                        || self
                            .host
                            .cargo
                            .as_ref()
                            .is_some_and(|approval| approval.executable == executable));
                self.observe_process_failure(&failure, descendants_possible);
                if !failure.cleanup_complete || matches!(failure.kind, FailureKind::Exit(None)) {
                    return Err(Error::new("OwnedProcessCleanupError", failure.to_string())
                        .coded("ROSTER_PROCESS_CLEANUP"));
                }
                if let FailureKind::Exit(Some(exit)) = failure.kind {
                    if !(0..=255).contains(&exit) {
                        return Err(Error::new("Error", "Process returned an invalid exit code"));
                    }
                    ObservedProcess {
                        exit,
                        stdout: failure.captured.stdout,
                        stderr: failure.captured.stderr,
                    }
                } else {
                    let mut error = Error::new("Error", failure.to_string());
                    if let Some(code) = failure.code() {
                        error = error.coded(code);
                    }
                    return Err(error);
                }
            }
        };
        self.check()?;
        Ok(observed)
    }

    pub(crate) fn observe_process_failure(
        &self,
        failure: &process::Failure,
        descendants_possible: bool,
    ) {
        if unverified_cleanup(failure, descendants_possible) {
            self.cleanup_uncertain.set(true);
        }
    }

    pub(crate) async fn git(
        &self,
        args: &[String],
        extra_env: &BTreeMap<String, String>,
    ) -> Result<Vec<u8>, Error> {
        let label = match args.first().map(String::as_str) {
            _ if args
                .iter()
                .map(String::as_str)
                .eq(checkpoints::SUBJECT_ARGS) =>
            {
                super::timing::Label::GitSubject
            }
            Some("rev-parse") if args.iter().any(|arg| arg == "HEAD") => {
                super::timing::Label::GitHead
            }
            Some("branch") => super::timing::Label::GitBranch,
            _ => super::timing::Label::GitOther,
        };
        let _scope = self.span(label);
        self.check()?;
        let mut command = self
            .host
            .git_command(&self.root, &self.no_hooks, args, extra_env);
        let command_scope = self.span(super::timing::Label::OwnedGit);
        let output = process::execute_timed(
            &mut command,
            Limits {
                timeout: Some(Duration::from_secs(30)),
                max_output_bytes: MAX_FILE_BYTES as usize,
            },
            Some(&self.token),
            command_scope.as_ref(),
        )
        .await;
        if let Some(scope) = command_scope {
            scope.finish_status(output.is_ok());
        }
        match output {
            Ok(output) => {
                self.check()?;
                Ok(output.captured.stdout)
            }
            Err(failure) => {
                self.observe_process_failure(&failure, false);
                if !failure.cleanup_complete || matches!(failure.kind, FailureKind::Exit(None)) {
                    return Err(Error::new("OwnedProcessCleanupError", failure.to_string())
                        .coded("ROSTER_PROCESS_CLEANUP"));
                }
                self.token.check()?;
                match failure.kind {
                    FailureKind::Exit(Some(_)) => Err(Error::new("Error", "Scoped Git inspection or checkpoint failed; no product restoration was authorized.").coded("ROSTER_GIT_FAILED")),
                    _ => {
                        let mut error = Error::new("Error", failure.to_string());
                        if let Some(code) = failure.code() { error = error.coded(code); }
                        Err(error)
                    }
                }
            }
        }
    }

    pub(crate) async fn require_ignored(&self, file: &str) -> Result<(), Error> {
        self.git(&["check-ignore".to_owned(), "--no-index".to_owned(), "--quiet".to_owned(), "--".to_owned(), file.to_owned()], &BTreeMap::new()).await
            .map(|_| ())
            .map_err(|cause| if cause.code == Some("ROSTER_GIT_FAILED") {
                denied("Runtime state must remain Git-ignored; this attempt is unverified and no ignore rules were changed by the harness.").caused_by(cause)
            } else { cause })
    }

    async fn git_read(&self, name: &str, args: &Value) -> Result<Value, Error> {
        fields(
            args,
            &[],
            &["path"],
            "git_diff accepts only an optional readable path",
        )?;
        let file = args
            .get("path")
            .map(|value| text(value, "git_diff path must be text"))
            .transpose()?
            .map(|file| self.normalized(&file, false))
            .transpose()?;
        if let Some(file) = &file {
            self.read_scope(file, false)?;
        }
        let args = if name == "git_status" {
            vec!["status", "--short", "--untracked-files=normal"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        } else {
            let mut args = [
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "HEAD",
                "--",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
            if let Some(file) = &file {
                args.push(file.clone());
            } else {
                args.extend(self.scope()?.into_iter().filter(|file| !file.contains('*')));
                if args.len() == 6 {
                    return Err(usage(
                        "Scoped git_diff requires exact task paths or an explicit path",
                    ));
                }
            }
            args
        };
        let output = bytes_text(&self.git(&args, &BTreeMap::new()).await?)?;
        let output = planning::memory::redact(&Text::from(output.as_str()), &self.redaction);
        Ok(Value::String(Text(
            output.0.into_iter().take(20_000).collect(),
        )))
    }

    async fn node_version(&self) -> Result<PathBuf, Error> {
        let node = self.host.node20.as_ref().ok_or_else(|| {
            usage("Node 20 verification requires an explicitly configured executable")
        })?;
        let output = self
            .process(
                node,
                &["--version".to_owned()],
                &BTreeMap::new(),
                10_000,
                1024,
            )
            .await?;
        if output.exit != 0 || !bytes_text(&output.stdout)?.trim().starts_with("v20.") {
            return Err(usage(
                "Consumer verification requires Node 20; no runtime was silently substituted",
            ));
        }
        Ok(node.clone())
    }

    async fn node_checks(&self, scope: &[String], full: bool) -> Result<NodeChecks, Error> {
        let relevant = policy::relevant_tests(scope)?;
        let tests_root = self.root.join("tests");
        secure::check_components(&tests_root).map_err(|cause| {
            denied("Test paths may not traverse links or junctions").caused_by(cause)
        })?;
        let mut available = Vec::new();
        for file in &relevant {
            if regular(&self.root, file, MAX_FILE_BYTES)?.is_some() {
                available.push(file.clone());
            }
        }
        match fs::read_dir(&tests_root) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry
                        .map_err(|error| secure::io_error("Read test entry", &tests_root, error))?;
                    let name = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| Error::type_error("Test filename must be Unicode"))?;
                    let file = format!("tests/{name}");
                    let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                        secure::io_error("Inspect test entry", &entry.path(), error)
                    })?;
                    if secure::linked(&metadata) && (full || policy::covers_test(&relevant, &file)?)
                    {
                        return Err(denied("Selected test paths must not be links or junctions"));
                    }
                    if metadata.is_file() && !available.contains(&file) {
                        available.push(file);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(secure::io_error("Read tests directory", &tests_root, error)),
        }
        let tests = policy::expand_shards(&relevant, &available, scope)?;
        for file in &tests {
            regular(&self.root, file, MAX_FILE_BYTES)?
                .ok_or_else(|| denied("Selected test file disappeared"))?;
        }
        let mut result = NodeChecks {
            output: ObservedProcess {
                exit: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            skipped: false,
            commands: Vec::new(),
            syntax_file: None,
        };
        if !full && tests.is_empty() {
            result.skipped = true;
            result.output.stdout = b"no relevant tests exist for the changed files".to_vec();
            return Ok(result);
        }
        let node = self.node_version().await?;
        let mut environment = BTreeMap::from([("ROSTER_SEAT".to_owned(), "coder".to_owned())]);
        if let Some(modules) = regular(&self.root, ".gitmodules", 65_536)? {
            if regex(r"(?m)^[ \t]*path[ \t]*=[ \t]*vendor[\\/]github-agent-contracts[ \t]*$")?
                .is_match(&bytes_text(&modules.bytes)?)
            {
                let contracts = paths::resolve_contracts_path(
                    &self.root,
                    &self.root,
                    self.host.contracts.as_deref(),
                )?;
                let path = contracts.to_str().ok_or_else(|| {
                    Error::type_error("Contracts path must contain valid Unicode")
                })?;
                environment.insert("GITHUB_AGENT_CONTRACTS".to_owned(), path.to_owned());
            }
        }
        if full {
            let mut candidates = scope
                .iter()
                .chain(self.written.iter())
                .filter(|file| {
                    !file.contains('*')
                        && (file.ends_with(".mjs")
                            || file.ends_with(".cjs")
                            || file.ends_with(".js"))
                })
                .cloned()
                .collect::<Vec<_>>();
            candidates.sort();
            candidates.dedup();
            for file in candidates {
                if regular(&self.root, &file, MAX_FILE_BYTES)?.is_none() {
                    continue;
                }
                result.commands.push(format!("node --check {file}"));
                let output = self
                    .process(
                        &node,
                        &["--check".to_owned(), file.clone()],
                        &environment,
                        30_000,
                        1024 * 1024,
                    )
                    .await?;
                if output.exit != 0 {
                    result.output = output;
                    result.syntax_file = Some(file);
                    return Ok(result);
                }
            }
        }
        let jobs = policy::test_concurrency(
            std::thread::available_parallelism().map_or(1, |count| count.get() as u32),
        );
        let mut args = vec![
            "--test".to_owned(),
            "--test-concurrency".to_owned(),
            jobs.to_string(),
            format!("--test-timeout={}", policy::FULL_TEST_TIMEOUT_MS),
        ];
        if !full {
            args.extend(tests);
        }
        result.commands.push(format!("node {}", args.join(" ")));
        result.output = self
            .process(
                &node,
                &args,
                &environment,
                policy::FULL_TEST_TIMEOUT_MS,
                4 * 1024 * 1024,
            )
            .await?;
        Ok(result)
    }

    fn verification_requirements(
        &self,
        snapshot: &Snapshot,
        full: bool,
    ) -> Result<VerificationRequirements, Error> {
        let scope = self.scope()?;
        let mut effective = BTreeMap::new();
        for file in scope
            .iter()
            .filter(|file| !file.contains('*'))
            .chain(snapshot.files.keys())
            .chain(self.written.iter())
        {
            effective.entry(key(file)).or_insert_with(|| file.clone());
        }
        let files = effective.into_values().collect::<Vec<_>>();
        let mut native_paths = scope;
        native_paths.extend(files.iter().cloned());
        let native = policy::requires_native(&native_paths)?;
        let javascript = files.iter().any(|file| {
            [".mjs", ".cjs", ".js"]
                .iter()
                .any(|extension| file.ends_with(extension))
        });
        let docs = !native && !javascript && policy::docs_only(&files)?;
        let node =
            !docs && (!native || javascript || full && self.root.join("package.json").is_file());
        Ok(VerificationRequirements {
            files,
            native,
            node,
            docs,
            full,
        })
    }

    pub(crate) fn checks_required(&self) -> Result<bool, Error> {
        let required = self.verification_requirements(&self.snapshot()?, false)?;
        Ok(required.native || required.node && policy::tests_mode(&self.task)? != "none")
    }

    pub async fn verify(&mut self, full: bool) -> Result<Value, Error> {
        self.check()?;
        self.last_tests = None;
        if self.options.role != Role::Coder
            || !self.options.allow_run_test
            || !self.recipe_allows("run_test")
        {
            return Err(usage(
                "run_test is disabled by tools.run_test or seat authority",
            ));
        }
        if policy::readme_only(&self.document.files_allowed) && !self.written.contains("README.md")
        {
            let present = regular(&self.root, "README.md", MAX_FILE_BYTES)?
                .map(|snapshot| {
                    bytes_text(&snapshot.bytes)
                        .and_then(|source| policy::status_section_present(&source))
                })
                .transpose()?
                .unwrap_or(false);
            if !present {
                return Err(Error::new(
                    "Error",
                    "README-only docs task must write README.md before running tests or other tools",
                ));
            }
        }
        if full {
            self.full_verification
                .get_or_insert(FullVerification {
                    latest: None,
                    failures: None,
                    pending: true,
                })
                .pending = true;
        }
        let before = self.snapshot()?;
        let requirements = self.verification_requirements(&before, full)?;
        let native = requirements.native;
        if native && self.host.cargo.is_none() {
            return Err(Error::new("Error", policy::NATIVE_TEST_REQUIRED));
        }
        let docs = requirements.docs;
        let validation = if native || !docs {
            Some(checkpoints::validation(self).await?)
        } else {
            None
        };
        let audit = self.audit()?;
        let mut commands = Vec::new();
        let mut skipped = false;
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit = 0;
        let mut syntax_file = None;
        let mut cargo_version = None;
        let mut coverage = EngineCoverage::default();
        if native {
            let approved = self
                .host
                .cargo
                .as_ref()
                .ok_or_else(|| Error::new("Error", policy::NATIVE_TEST_REQUIRED))?;
            let mut environment = approved
                .environment
                .iter()
                .filter(|(name, _)| {
                    [
                        "CARGO_HOME",
                        "RUSTUP_HOME",
                        "CARGO_TARGET_DIR",
                        "RUSTFLAGS",
                        "RUSTUP_TOOLCHAIN",
                    ]
                    .contains(&name.as_str())
                })
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<BTreeMap<_, _>>();
            environment.insert("CARGO_NET_OFFLINE".to_owned(), "true".to_owned());
            environment.insert("RUSTUP_AUTO_INSTALL".to_owned(), "0".to_owned());
            environment.insert("CARGO_INCREMENTAL".to_owned(), "0".to_owned());
            let is_roster_self = self.root.join("rust").join("shell.rs").is_file();
            if is_roster_self && !environment.contains_key("CARGO_TARGET_DIR") {
                let manifest_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
                if manifest_target.is_dir() && !manifest_target.starts_with(&self.root) {
                    environment.insert(
                        "CARGO_TARGET_DIR".to_owned(),
                        manifest_target.display().to_string(),
                    );
                } else if let Ok(repo_paths) = paths::repository_paths(&self.root) {
                    if let Some(main_root) = repo_paths.common_dir.parent() {
                        let shared_target = main_root.join("target");
                        if shared_target.is_dir() && !shared_target.starts_with(&self.root) {
                            environment.insert(
                                "CARGO_TARGET_DIR".to_owned(),
                                shared_target.display().to_string(),
                            );
                        }
                    }
                }
            }
            let version = self
                .process(
                    &approved.executable,
                    &["--version".to_owned()],
                    &environment,
                    10_000,
                    1024,
                )
                .await?;
            if version.exit != 0
                || !regex(r"^cargo [0-9]+\.[0-9]+\.[0-9]+")?
                    .is_match(bytes_text(&version.stdout)?.trim())
            {
                return Err(denied(
                    "Native verification requires an actual host-approved Cargo executable",
                ));
            }
            cargo_version = Some(bytes_text(&version.stdout)?.trim().to_owned());
            let mut completed = 0;
            let has_integration_test = requirements
                .files
                .iter()
                .chain(self.written.iter())
                .any(|f| f.starts_with("tests/"));
            let (clippy_args, test_args) = if has_integration_test || !is_roster_self {
                (
                    vec![
                        "clippy".to_owned(),
                        "--all-targets".to_owned(),
                        "--locked".to_owned(),
                        "--offline".to_owned(),
                        "--".to_owned(),
                        "-D".to_owned(),
                        "warnings".to_owned(),
                    ],
                    vec![
                        "test".to_owned(),
                        "--all-targets".to_owned(),
                        "--locked".to_owned(),
                        "--offline".to_owned(),
                    ],
                )
            } else {
                let single_mod = if requirements.files.len() == 1 {
                    requirements.files[0]
                        .strip_prefix("rust/")
                        .and_then(|s| s.strip_suffix(".rs"))
                        .filter(|s| !s.contains('/') && *s != "lib" && *s != "main")
                        .map(|s| format!("{s}::tests"))
                } else {
                    None
                };
                let mut t_args = vec![
                    "test".to_owned(),
                    "--lib".to_owned(),
                    "--locked".to_owned(),
                    "--offline".to_owned(),
                ];
                if let Some(filter) = single_mod {
                    t_args.push(filter);
                }
                (
                    vec![
                        "clippy".to_owned(),
                        "--lib".to_owned(),
                        "--bins".to_owned(),
                        "--locked".to_owned(),
                        "--offline".to_owned(),
                        "--".to_owned(),
                        "-D".to_owned(),
                        "warnings".to_owned(),
                    ],
                    t_args,
                )
            };
            for args in [
                vec![
                    "fmt".to_owned(),
                    "--all".to_owned(),
                    "--".to_owned(),
                    "--check".to_owned(),
                ],
                clippy_args,
                test_args,
            ] {
                commands.push(format!("cargo {}", args.join(" ")));
                let output = self
                    .process(
                        &approved.executable,
                        &args,
                        &environment,
                        policy::FULL_TEST_TIMEOUT_MS,
                        4 * 1024 * 1024,
                    )
                    .await?;
                stdout.push_str(&bytes_text(&output.stdout)?);
                stderr.push_str(&bytes_text(&output.stderr)?);
                if output.exit != 0 {
                    exit = output.exit;
                    break;
                }
                completed += 1;
            }
            coverage.cargo = completed == 3 && exit == 0;
        } else if docs {
            skipped = true;
            stdout = "docs-only: tests skipped".to_owned();
        }
        if requirements.node && exit == 0 {
            let checks = self.node_checks(&requirements.files, full).await?;
            if native && checks.skipped {
                return Err(unsupported(
                    "Mixed Rust/JavaScript scope needs actual matching Node checks as well as Cargo evidence",
                ));
            }
            coverage.node =
                !checks.skipped && checks.output.exit == 0 && checks.syntax_file.is_none();
            skipped = checks.skipped;
            commands.extend(checks.commands);
            exit = checks.output.exit;
            stdout.push_str(&bytes_text(&checks.output.stdout)?);
            stderr.push_str(&bytes_text(&checks.output.stderr)?);
            syntax_file = checks.syntax_file;
        }
        self.check()?;
        if self.snapshot()? != before {
            return Err(denied(
                "Product identity or bytes changed during verification; late success is refused",
            ));
        }
        if self.audit()? != audit {
            return Err(denied(
                "Worktree product or protected metadata changed during verification; late success is refused",
            ));
        }
        if validation.is_some() && Some(checkpoints::validation(self).await?) != validation {
            return Err(denied(
                "Task branch or revision changed during verification; late success is refused",
            ));
        }
        let mut result = Value::object(vec![
            ("exit_code", Value::Number(f64::from(exit))),
            (
                "stdout",
                Value::String(planning::memory::redact(
                    &Text::from(stdout.as_str()),
                    &self.redaction,
                )),
            ),
            (
                "stderr",
                Value::String(planning::memory::redact(
                    &Text::from(stderr.as_str()),
                    &self.redaction,
                )),
            ),
        ]);
        if skipped {
            domain::put(&mut result, "skipped", Value::Boolean(true))?;
        }
        if let Some(file) = syntax_file {
            domain::put(&mut result, "syntax_check", Value::Boolean(true))?;
            domain::put(&mut result, "failing_files", strings(&[file]))?;
        }
        if native {
            domain::put(&mut result, "native", Value::Boolean(true))?;
            domain::put(&mut result, "commands", strings(&commands))?;
            if let Some(version) = &cargo_version {
                domain::put(&mut result, "cargo_version", Value::string(version))?;
            }
        }
        let fresh_pass = exit == 0
            && (requirements.docs
                || !skipped
                    && (!requirements.native || coverage.cargo)
                    && (!requirements.node || coverage.node));
        let observed = TestEvidence {
            result: result.clone(),
            snapshot: before,
            authority: self.authority_digest(),
            native,
            validation,
            audit,
            requirements,
            coverage,
            observed_exit: exit,
            observed_skipped: skipped,
            observed_commands: commands,
            observed_cargo_version: cargo_version,
            observation: std::sync::Arc::new(()),
        };
        if full {
            let previous = self
                .full_verification
                .as_ref()
                .and_then(|record| record.failures);
            let failures = if exit != 0 {
                Some((previous.map_or(exit, |pair| pair.0), exit))
            } else {
                previous
            };
            self.full_verification = Some(FullVerification {
                latest: Some(observed.clone()),
                failures,
                pending: false,
            });
        }
        self.last_tests = Some(observed);
        if fresh_pass {
            self.fresh_after_restore = false;
        }
        self.check()?;
        Ok(result)
    }

    fn recipe_allows(&self, name: &str) -> bool {
        self.recipe_tools.as_ref().is_none_or(|tools| {
            tools.iter().any(|tool| tool == name)
                || name == "delete_file"
                    && tools
                        .iter()
                        .any(|tool| tool == "write_file" || tool == "edit_file")
        })
    }

    pub(crate) fn recipe_permits(&self, name: &str) -> bool {
        self.recipe_allows(name)
    }

    pub(crate) fn require_coder_recipe(&self) -> Result<(), Error> {
        self.check()?;
        let snapshot = self
            .artifacts
            .get("RECIPE.yml")
            .and_then(Option::as_ref)
            .ok_or_else(|| denied("Coder requires its frozen recipe"))?;
        let recipe = crate::recipe::parse(&Text::from(bytes_text(&snapshot.bytes)?.as_str()))?;
        let recipe = crate::json::parse(&Text::from(recipe.json().as_str()))?;
        let seat = recipe
            .get("seats")
            .and_then(domain::items)
            .and_then(|seats| {
                seats
                    .iter()
                    .find(|seat| domain::is_text(seat.get("id"), "coder"))
            })
            .ok_or_else(|| denied("The frozen recipe has no coder seat"))?;
        if !domain::is_text(seat.get("principal"), "coder")
            || !domain::is_text(seat.get("worker"), "builtin")
        {
            return Err(denied(
                "Native product writes require the frozen builtin coder principal; prompt/model labels cannot grant another role",
            ));
        }
        Ok(())
    }

    pub fn tool_names(&self) -> Vec<&'static str> {
        let names: &[&str] = if self.options.role == Role::Reviewer {
            &["read_file", "search_text", "git_diff"]
        } else {
            &[
                "read_file",
                "write_file",
                "edit_file",
                "delete_file",
                "glob_files",
                "list_dir",
                "search_text",
                "run_test",
                "run_command",
                "git_diff",
            ]
        };
        names
            .iter()
            .copied()
            .filter(|name| self.recipe_allows(name))
            .collect()
    }

    pub(crate) fn verification_prerequisite(&self) -> Result<(), Error> {
        self.check()?;
        let required = self.verification_requirements(&self.snapshot()?, false)?;
        if (required.native || required.node && policy::tests_mode(self.task())? != "none")
            && (!self.options.allow_run_test || !self.recipe_allows("run_test"))
        {
            return Err(unsupported(
                "Required verification is disabled by the frozen host or recipe permissions",
            ));
        }

        if required.native && self.host.cargo.is_none() {
            return Err(unsupported(
                "Required Rust verification has no explicit host-approved Cargo executable and environment",
            ));
        }
        if required.node && policy::tests_mode(self.task())? != "none" && self.host.node20.is_none()
        {
            return Err(unsupported(
                "Required consumer verification has no host-approved Node 20 executable",
            ));
        }
        Ok(())
    }

    pub(crate) fn enable_owned_checks(&mut self) -> Result<(), Error> {
        self.check()?;
        if self.checks_required()? {
            if !cfg!(windows) && !cfg!(unix) {
                return Err(unsupported(
                    "Strict Unix verification process ownership is unavailable; no product writes or direct-child fallback are authorized",
                ));
            }
            let requirements = self.verification_requirements(&self.snapshot()?, false)?;
            for executable in [
                requirements
                    .native
                    .then(|| {
                        self.host
                            .cargo
                            .as_ref()
                            .map(|cargo| cargo.executable.as_path())
                    })
                    .flatten(),
                requirements
                    .node
                    .then_some(self.host.node20.as_deref())
                    .flatten(),
            ]
            .into_iter()
            .flatten()
            {
                if !executable.is_absolute() {
                    return Err(unsupported(
                        "Native inner seats require absolute, explicitly host-approved verification images, not a PATH fallback",
                    ));
                }
                let selected = paths::file_information(executable)?;
                let normalized = paths::strip_verbatim(executable.to_path_buf());
                secure::check_components(&normalized)?;
                if selected.identity != paths::file_information(&normalized)?.identity {
                    return Err(denied(
                        "Approved verification path and normalized path name different physical images",
                    ));
                }
                if !fs::metadata(&normalized)
                    .map_err(|error| {
                        secure::io_error("Inspect verification prerequisite", executable, error)
                    })?
                    .is_file()
                {
                    return Err(unsupported(
                        "Selected verification image is not an available regular file",
                    ));
                }
            }
        }
        if !self.owned_checks {
            self.owned_checks = true;
            self.last_tests = None;
            if let Some(full) = &mut self.full_verification {
                full.pending = true;
            }
        }
        Ok(())
    }

    pub(crate) fn scope_limit(&self) -> usize {
        self.options.scope_expansion
    }

    pub(crate) fn scope_budget_authorized(&self, configured: u64) -> Result<bool, Error> {
        self.check()?;
        let initial = u64::try_from(self.initial_scope_expansion)
            .map_err(|_| Error::type_error("Initial scope budget exceeds the native bound"))?;
        let current = u64::try_from(self.options.scope_expansion)
            .map_err(|_| Error::type_error("Current scope budget exceeds the native bound"))?;
        Ok(current <= configured
            || self.context_generation != 0 && initial <= configured && current <= 16)
    }

    pub(crate) fn require_mutation_capability(&self, file: &str) -> Result<(), Error> {
        if !self.owned_checks {
            return Ok(());
        }
        let native = policy::requires_native(&[file.to_owned()])?;
        let node = [".mjs", ".cjs", ".js"]
            .iter()
            .any(|extension| file.ends_with(extension))
            && (policy::tests_mode(self.task())? != "none"
                || policy::requires_native(&self.scope()?)?);
        if (native || node) && !cfg!(windows) && !cfg!(unix) {
            return Err(unsupported(
                "Selected strict Unix verification is unavailable before the proposed mutation",
            ));
        }
        if native && self.host.cargo.is_none() {
            return Err(unsupported(
                "Proposed Rust mutation has no host-approved Cargo verification capability",
            ));
        }
        if node && self.host.node20.is_none() {
            return Err(unsupported(
                "Proposed consumer mutation has no host-approved Node 20 verification capability",
            ));
        }
        Ok(())
    }

    pub fn definitions(&self) -> Value {
        Value::Array(self.tool_names().iter().map(|name| {
            let (description, required, optional): (&str, &[&str], &[&str]) = match *name {
                "read_file" => ("Read a bounded, regular, unlinked worktree file.", &["path"], &["max_lines", "offset"]),
                "write_file" => ("Write a task-scoped file after the required read and native checkpoint.", &["path", "content"], &[]),
                "edit_file" => ("Replace one exact, unique old_string in an existing task-scoped file.", &["path", "old_string", "new_string"], &[]),
                "delete_file" => ("Delete an authorized regular product file; Rust deletions still require native verification.", &["path"], &[]),
                "list_dir" => ("List readable worktree entries.", &[], &["path"]),
                "search_text" => ("Search readable regular files for literal text; returns at most 50 lines.", &["query"], &["path"]),
                "glob_files" => ("Find at most 100 readable worktree paths with a simple wildcard pattern.", &["pattern"], &[]),
                "run_test" => ("Run scoped Node 20 tests, or explicitly host-approved locked offline Cargo checks for Rust; no shell grant.", &[], &[]),
                "run_command" => ("Only git status, scoped git diff, or scoped node --test; no arbitrary flags, shell or network grant.", &["argv"], &[]),
                _ => ("Read a scoped Git diff without external diff or text conversion.", &[], &["path"]),
            };
            let mut properties = Value::object(Vec::new());
            for field in required.iter().chain(optional.iter()) {
                let shape = if *field == "argv" {
                    Value::object(vec![("type", Value::string("array")), ("items", Value::object(vec![("type", Value::string("string"))]))])
                } else if ["max_lines", "offset"].contains(field) {
                    Value::object(vec![("type", Value::string("integer")), ("minimum", Value::Number(1.0))])
                } else { Value::object(vec![("type", Value::string("string"))]) };
                if let Value::Object(entries) = &mut properties {
                    entries.push((Text::from(*field), shape));
                }
            }
            Value::object(vec![
                ("type", Value::string("function")),
                ("function", Value::object(vec![
                    ("name", Value::string(name)), ("description", Value::string(description)),
                    ("parameters", Value::object(vec![
                        ("type", Value::string("object")), ("properties", properties),
                        ("required", Value::Array(required.iter().map(|name| Value::string(name)).collect())),
                        ("additionalProperties", Value::Boolean(false)),
                    ])),
                ])),
            ])
        }).collect())
    }

    pub async fn execute(&mut self, name: &str, args: &Value) -> Result<Value, Error> {
        self.execute_with(name, args, &mut NoObserver).await
    }

    pub async fn execute_with<O: Observer>(
        &mut self,
        name: &str,
        args: &Value,
        observer: &mut O,
    ) -> Result<Value, Error> {
        self.execute_inner(name, args, observer)
            .await
            .map_err(|mut error| {
                let message = error
                    .message_text
                    .take()
                    .unwrap_or_else(|| Text::from(error.message.as_str()));
                let redacted = Error::from_text(
                    error.name,
                    planning::memory::redact(&message, &self.redaction),
                );
                error.message = redacted.message;
                error.message_text = redacted.message_text;
                error
            })
    }

    async fn execute_inner<O: Observer>(
        &mut self,
        name: &str,
        args: &Value,
        observer: &mut O,
    ) -> Result<Value, Error> {
        self.check()?;
        if !self.tool_names().contains(&name) {
            return Err(denied(if self.options.role == Role::Reviewer {
                "Refused: the reviewer is read-only; only read_file, search_text, git_diff are available."
            } else {
                "Tool is unavailable in this seat or recipe; no shell or network grant exists."
            }));
        }
        let result = match name {
            "read_file" => self.read_file(args)?,
            "list_dir" => self.list_dir(args)?,
            "search_text" => self.search_text(args)?,
            "glob_files" => self.glob_files(args)?,
            "write_file" | "edit_file" | "delete_file" => self.mutate(name, args, observer).await?,
            "run_test" => {
                fields(args, &[], &[], "Tool arguments must contain no fields")?;
                self.verify(false).await?
            }
            "git_diff" => self.git_read(name, args).await?,
            "run_command" => {
                fields(args, &["argv"], &[], "Tool arguments must contain argv")?;
                let argv = domain::items(required(args, "argv")?)
                    .ok_or_else(|| usage("run_command requires a scoped argument vector"))?;
                let argv = argv
                    .iter()
                    .map(|arg| text(arg, "run_command arguments must be text"))
                    .collect::<Result<Vec<_>, _>>()?;
                if argv == ["node", "--test"] {
                    self.verify(false).await?
                } else if argv == ["git", "status"] {
                    self.git_read("git_status", &Value::object(Vec::new()))
                        .await?
                } else if argv == ["git", "diff"] {
                    self.git_read("git_diff", &Value::object(Vec::new()))
                        .await?
                } else {
                    return Err(usage(
                        "run_command allows only git status, git diff, and scoped node --test; arbitrary flags and shell syntax are refused",
                    ));
                }
            }
            _ => return Err(unsupported(name)),
        };
        self.check()?;
        observer
            .emit(Value::object(vec![
                ("type", Value::string("tool-result")),
                ("name", Value::string(name)),
                ("status", Value::string("ok")),
            ]))
            .await?;
        self.check()?;
        Ok(result)
    }

    pub(crate) fn state_directory(&self, segments: &[&str]) -> Result<PathBuf, Error> {
        layout::state_path(
            layout::resolve_directory(
                layout::Scope::PerWorktree,
                &self.root,
                Some(&self.root),
                Some(".roster"),
            )?,
            segments,
        )
        .map(|path| path.path)
    }
}
