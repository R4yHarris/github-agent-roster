use std::path::{Path, PathBuf};

use crate::cancel::CancellationToken;
use crate::domain;
use crate::execution::tools;
use crate::host;
use crate::identity;
use crate::json::Value;
use crate::learning::{self, JsonlKind, optional};
use crate::paths;
use crate::run;
use crate::start::Git;
use crate::state::provenance::api::{self, ProvenanceStore};
use crate::state::{layout, redaction, secure};
use crate::text::{Text, trim};
use crate::{Error, config};

pub use crate::learning::MAX_SOURCE_BYTES;
pub const MAX_DIRECTORY_ENTRIES: usize = 256;

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub reference: Option<String>,
    pub evals_path: Option<PathBuf>,
    pub delivery: bool,
}

enum ReadScope<'a> {
    Analytics(Option<&'a ProvenanceStore>),
    HumanAttribution,
}

impl Options {
    pub fn validate(&self) -> Result<(), Error> {
        validate_reference(self.reference.as_deref().unwrap_or("HEAD"))?;
        if let Some(path) = &self.evals_path {
            let value = path
                .to_str()
                .ok_or_else(|| Error::type_error("evalsPath must be a Unicode local file path"))?;
            if value.is_empty()
                || value.starts_with('-')
                || trim(&Text::from(value).0).is_empty()
                || value.contains(['\0', '\r', '\n'])
                || value.len() > 4096
            {
                return Err(Error::type_error(
                    "evalsPath must be a nonempty local file path",
                ));
            }
        }
        Ok(())
    }
}

type ReferenceParts<'a> = (&'a str, Option<(&'static str, &'a str)>);

fn reference_parts(reference: &str) -> Result<ReferenceParts<'_>, Error> {
    let range = reference
        .split_once("...")
        .map(|(left, right)| (left, "...", right))
        .or_else(|| {
            reference
                .split_once("..")
                .map(|(left, right)| (left, "..", right))
        });
    let (first, rest) = range.map_or((reference, None), |(left, separator, right)| {
        (left, Some((separator, right)))
    });
    for endpoint in [Some(first), rest.map(|(_, right)| right)]
        .into_iter()
        .flatten()
    {
        if endpoint.starts_with('-')
            || endpoint.contains("..")
            || endpoint.bytes().any(|unit| {
                !unit.is_ascii_alphanumeric()
                    && !matches!(
                        unit,
                        b'/' | b'_' | b'.' | b'-' | b'~' | b'^' | b'@' | b'{' | b'}'
                    )
            })
        {
            return Err(Error::type_error(
                "ref must be a nonempty Git revision or range, not an option or path selector",
            ));
        }
    }
    Ok((first, rest))
}

pub fn validate_reference(reference: &str) -> Result<(), Error> {
    if reference.is_empty() || reference.len() > 512 || reference_parts(reference).is_err() {
        return Err(Error::type_error(
            "ref must be a nonempty bounded Git revision or range, not an option or path selector",
        ));
    }
    Ok(())
}

fn provider_error(action: &str, cause: Error) -> Error {
    if matches!(
        cause.code,
        Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT" | "ROSTER_PROCESS_CLEANUP")
    ) {
        return cause;
    }
    let mut safe = Error::new(
        cause.name,
        "The native read-only provider failed; external command details were omitted",
    );
    safe.code = cause.code;
    learning::error("ROSTER_ANALYTICS_GIT", format!("Could not {action}; verify the repository, revision, approved Git image and available offline objects"))
        .caused_by(safe)
}

async fn revision(
    git: &Git,
    root: &Path,
    reference: &str,
    token: &CancellationToken,
) -> Result<String, Error> {
    let (first, rest) = reference_parts(reference)?;
    let resolve = |endpoint: &str| {
        if endpoint.is_empty() {
            "HEAD".to_owned()
        } else {
            endpoint.to_owned()
        }
    };
    let first = git
        .text(
            root,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{}^{{commit}}", resolve(first)),
            ],
            token,
        )
        .await
        .map_err(|error| provider_error("resolve the selected Git revision", error))?;
    if !learning::valid_sha(&first) {
        return Err(learning::error(
            "ROSTER_ANALYTICS_GIT",
            "Git returned an invalid resolved commit object ID",
        ));
    }
    let Some((separator, last)) = rest else {
        return Ok(first);
    };
    let last = git
        .text(
            root,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{}^{{commit}}", resolve(last)),
            ],
            token,
        )
        .await
        .map_err(|error| provider_error("resolve the selected Git range", error))?;
    if !learning::valid_sha(&last) {
        return Err(learning::error(
            "ROSTER_ANALYTICS_GIT",
            "Git returned an invalid range commit object ID",
        ));
    }
    Ok(format!("{first}{separator}{last}"))
}

pub fn parse_git_history(bytes: &[u8]) -> Result<Vec<Value>, Error> {
    if bytes.len() > learning::MAX_JSONL_BYTES {
        return Err(learning::error(
            "ROSTER_ANALYTICS_SIZE",
            "Git history exceeds 16 MiB; select a narrower --ref",
        ));
    }
    let history = std::str::from_utf8(bytes)
        .map_err(|_| learning::error("ROSTER_ANALYTICS_INPUT", "Git history must be UTF-8"))?;
    let fields = history.split('\0').collect::<Vec<_>>();
    if fields.len() % 2 != 1
        || fields
            .last()
            .is_none_or(|tail| !trim(&Text::from(*tail).0).is_empty())
    {
        return Err(learning::error(
            "ROSTER_ANALYTICS_INPUT",
            "Git returned an invalid commit stream; no metrics were exported",
        ));
    }
    if fields.len() / 2 > learning::MAX_ROWS {
        return Err(learning::error(
            "ROSTER_ANALYTICS_SIZE",
            "Git history exceeds 10000 commits; select a narrower --ref",
        ));
    }
    let mut records = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for pair in fields.chunks_exact(2) {
        let [sha, message] = pair else {
            return Err(learning::error(
                "ROSTER_ANALYTICS_INPUT",
                "Invalid Git commit frame",
            ));
        };
        let sha = Text::from(*sha);
        let sha = Text(trim(&sha.0).to_vec()).to_utf8()?;
        if !learning::valid_sha(&sha) || !seen.insert(sha.to_ascii_lowercase()) {
            return Err(learning::error(
                "ROSTER_ANALYTICS_INPUT",
                "Git returned an invalid or duplicate commit SHA",
            ));
        }
        if let Some(record) = run::parse_message(&Text::from(*message), &sha).map_err(|cause| {
            learning::error(
                "ROSTER_ANALYTICS_INPUT",
                format!("Invalid AI-Run in commit {sha}; no metrics were exported"),
            )
            .caused_by(cause)
        })? {
            records.push(record);
        }
    }
    Ok(records)
}

#[derive(Default)]
struct Budget {
    bytes: usize,
    rows: usize,
}

impl Budget {
    fn add(&mut self, bytes: usize, rows: usize) -> Result<(), Error> {
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(|| {
            learning::error("ROSTER_ANALYTICS_SIZE", "Analytics source size overflow")
        })?;
        self.rows = self.rows.checked_add(rows).ok_or_else(|| {
            learning::error("ROSTER_ANALYTICS_SIZE", "Analytics record count overflow")
        })?;
        if self.bytes > MAX_SOURCE_BYTES || self.rows > learning::MAX_ROWS {
            return Err(learning::error(
                "ROSTER_ANALYTICS_SIZE",
                "Analytics sources exceed 32 MiB or 10000 rows",
            ));
        }
        Ok(())
    }
}

fn read_jsonl(
    path: &Path,
    label: &str,
    kind: JsonlKind,
    required: bool,
    budget: &mut Budget,
) -> Result<Vec<Value>, Error> {
    let mut read = || -> Result<Vec<Value>, Error> {
        let Some(snapshot) = secure::read(path, learning::MAX_JSONL_BYTES as u64)? else {
            if required {
                return Err(learning::error(
                    "ROSTER_ANALYTICS_MISSING",
                    format!("{label}: explicit AI-Eval file is missing"),
                ));
            }
            return Ok(Vec::new());
        };
        if paths::file_information(path)?.links != 1
            || !secure::unchanged(path, &snapshot, learning::MAX_JSONL_BYTES as u64)?
        {
            return Err(learning::error(
                "ROSTER_ANALYTICS_INPUT",
                format!("{label}: source changed or is hard-linked; preserve its bytes"),
            ));
        }
        let source = std::str::from_utf8(&snapshot.bytes).map_err(|_| {
            learning::error(
                "ROSTER_ANALYTICS_INPUT",
                format!("{label}: expected UTF-8 JSONL"),
            )
        })?;
        let records = learning::parse_jsonl(&Text::from(source), label, kind)?;
        budget.add(snapshot.bytes.len(), records.len())?;
        Ok(records)
    };
    read().map_err(|cause| {
        if cause.name == "LearningError" { cause } else {
            let mut error = learning::error(cause.code.unwrap_or("ROSTER_ANALYTICS_INPUT"), format!("{label}: read refused; verify permissions, real single-link files and the declared scope. Bytes were preserved."));
            error.cause = Some(Box::new(Error::new(cause.name, "The checked file provider refused this read")));
            error
        }
    })
}

fn learning_files(
    repository: &Path,
    budget: &mut Budget,
    include_evaluations: bool,
) -> Result<(Vec<Value>, Vec<Value>), Error> {
    let directory = layout::resolve_directory(layout::Scope::Shared, repository, None, None)?;
    secure::check_components(&directory.dir)?;
    secure::check_components(&directory.legacy_root)?;
    let runs = layout::state_path(directory.clone(), &["runs"])?;
    let evals = layout::state_path(directory, &["evals.jsonl"])?;
    let mut names = Vec::new();
    if secure::private_directory_if_present(&runs.path)? {
        for name in secure::entries_bounded(&runs.path, MAX_DIRECTORY_ENTRIES).map_err(|_| {
            learning::error(
                "ROSTER_ANALYTICS_SIZE",
                "Run history is unreadable or exceeds 256 directory entries",
            )
        })? {
            names.push((learning::collation_key(&Text::from(name.as_str()))?, name));
        }
    }
    names.sort_by(|left, right| left.0.cmp(&right.0));
    let mut records = Vec::new();
    for (index, (_, name)) in names
        .iter()
        .enumerate()
        .filter(|(_, (_, name))| name.ends_with(".jsonl"))
    {
        records.extend(read_jsonl(
            &runs.path.join(name),
            &format!("local runs JSONL {}", index + 1),
            JsonlKind::Run,
            true,
            budget,
        )?);
    }
    let evaluations = if include_evaluations {
        read_jsonl(
            &evals.path,
            "local AI-Eval",
            JsonlKind::HumanEvaluation,
            false,
            budget,
        )?
    } else {
        Vec::new()
    };
    Ok((records, evaluations))
}

async fn repository_identity(
    git: &Git,
    root: &Path,
    token: &CancellationToken,
) -> Result<String, Error> {
    let origin = git
        .text(root, &["config", "--get", "remote.origin.url"], token)
        .await
        .map_err(|error| {
            provider_error(
                "read the current repository identity for explicit machine history",
                error,
            )
        })?;
    let source = Text::from(origin.as_str());
    if trim(&source.0).is_empty() {
        return Err(learning::error(
            "ROSTER_ANALYTICS_IDENTITY",
            "Machine history requires an unambiguous current repository identity",
        ));
    }
    let local = !origin.contains("://")
        && origin
            .split_once(':')
            .is_none_or(|(host, _)| host.contains(['/', '\\']));
    identity::identity_hash(&if local {
        Text::from(
            paths::resolve(root, Path::new(&origin))?
                .to_str()
                .ok_or_else(|| Error::type_error("Repository origin path must be Unicode"))?,
        )
    } else {
        source
    })
}

async fn require_local_identity(
    git: &Git,
    root: &Path,
    token: &CancellationToken,
) -> Result<(), Error> {
    let directory = layout::resolve_directory(layout::Scope::Shared, root, None, None)?;
    let marker = layout::state_path(directory, &["identity"])?;
    let Some(snapshot) = secure::read(&marker.path, 1024)? else {
        return Ok(());
    };
    if paths::file_information(&marker.path)?.links != 1
        || !secure::unchanged(&marker.path, &snapshot, 1024)?
    {
        return Err(learning::error(
            "ROSTER_ANALYTICS_IDENTITY",
            "Local learning identity marker changed or is hard-linked",
        ));
    }
    let recorded = std::str::from_utf8(&snapshot.bytes).map_err(|_| {
        learning::error(
            "ROSTER_ANALYTICS_IDENTITY",
            "Local learning identity must be UTF-8",
        )
    })?;
    let recorded = Text::from(recorded);
    let recorded = Text(trim(&recorded.0).to_vec()).to_utf8()?;
    let current = repository_identity(git, root, token).await?;
    if !matches!(
        identity::compare_identity(Some(&recorded), &current),
        identity::Comparison::Match
    ) {
        return Err(learning::error(
            "ROSTER_ANALYTICS_IDENTITY",
            "Local learning belongs to a different repository identity; no records were read or reassigned",
        ));
    }
    Ok(())
}

fn copy_known(target: &mut Value, source: &Value, from: &str, to: &str) -> Result<(), Error> {
    if let Some(value) = optional(source, from).filter(|value| {
        !domain::is_text(Some(value), "unknown")
            && !value.as_string().is_some_and(|value| value.0.is_empty())
    }) {
        domain::put(target, to, value.clone())?;
    }
    Ok(())
}

fn project_provenance(record: &Value) -> Result<Option<Value>, Error> {
    if !domain::is_text(record.get("section"), api::RAW_HISTORY) {
        return Ok(None);
    }
    api::validate_typed_record(record, "analytics provenance")?;
    let payload = record.get("payload");
    let seat = record
        .get("seat")
        .and_then(|seat| seat.get("name"))
        .and_then(Value::as_string);
    if !(domain::is_text(record.get("event"), "session")
        || seat.is_some_and(|seat| !seat.0.is_empty())
            && ["completed", "failure"]
                .iter()
                .any(|event| domain::is_text(record.get("event"), event)))
    {
        return Ok(None);
    }
    let mut result = Value::object(Vec::new());
    copy_known(&mut result, record, "sessionId", "session")?;
    copy_known(&mut result, record, "servedModel", "model")?;
    if let Some(seat) = seat.filter(|seat| !seat.0.is_empty()) {
        domain::put(&mut result, "seat", Value::String(seat.clone()))?;
    }
    if let Some(issue) = record.get("issue") {
        copy_known(&mut result, issue, "task", "task")?;
    }
    if let Some(payload) = payload {
        for field in [
            "task",
            "task_class",
            "provider",
            "effort",
            "context_max",
            "excellence",
            "defects",
        ] {
            if optional(&result, field).is_none() {
                copy_known(&mut result, payload, field, field)?;
            }
        }
        if optional(&result, "model").is_none() && record.get("servedModel").is_none() {
            copy_known(&mut result, payload, "model", "model")?;
        }
    }
    if let Some(metrics) = record.get("metrics") {
        for (source, field, alias) in [
            ("tokens_prompt", "prompt_tokens", "context_used"),
            ("tokens_completion", "completion_tokens", "context_out"),
        ] {
            if let Some(value) =
                optional(metrics, source).filter(|value| !domain::is_text(Some(value), "unknown"))
            {
                if domain::safe_integer(Some(value)).is_none() {
                    return Err(learning::error(
                        "ROSTER_ANALYTICS_INPUT",
                        "Provenance usage must be a reported nonnegative safe integer or unknown",
                    ));
                }
                domain::put(&mut result, field, value.clone())?;
                domain::put(&mut result, alias, value.clone())?;
            }
        }
    }
    learning::validate_run(&result)?;
    Ok(Some(result))
}

async fn read_scoped_provenance(
    store: &ProvenanceStore,
    expected: &str,
    budget: &mut Budget,
    token: &CancellationToken,
) -> Result<Vec<Value>, Error> {
    token.check()?;
    if store.identity(Some(token)).await? != expected
        || store
            .underlying
            .root
            .file_name()
            .is_none_or(|name| name != expected)
    {
        return Err(learning::error(
            "ROSTER_ANALYTICS_IDENTITY",
            "Refusing foreign or unscoped machine history before reading records",
        ));
    }
    let mut loaded = store.underlying.read_all_bounded().map_err(|_| learning::error("ROSTER_ANALYTICS_INPUT", "Explicit same-identity provenance could not be read within its existing byte and record bounds; preserve it for diagnostics"))?;
    if !loaded.skipped.is_empty() {
        return Err(learning::error(
            "ROSTER_ANALYTICS_INPUT",
            "Machine history contains malformed or incompatible records; bytes were preserved, no repair was attempted",
        ));
    }
    if loaded
        .records
        .iter()
        .all(|record| domain::safe_integer(record.get("seq")).is_some())
    {
        loaded
            .records
            .sort_by_key(|record| domain::safe_integer(record.get("seq")));
    }
    for record in &loaded.records {
        token.check()?;
        if !domain::is_text(record.get("repoIdentity"), expected) {
            return Err(learning::error(
                "ROSTER_ANALYTICS_IDENTITY",
                "Machine history identity does not match its declared repository scope",
            ));
        }
        budget.add(record.json().len(), 1)?;
    }
    Ok(loaded.records)
}

async fn read_provenance(
    store: &ProvenanceStore,
    expected: &str,
    budget: &mut Budget,
    token: &CancellationToken,
) -> Result<Vec<Value>, Error> {
    let mut records = Vec::new();
    for record in read_scoped_provenance(store, expected, budget, token).await? {
        if let Some(projected) = project_provenance(&record)? {
            records.push(projected);
        }
    }
    Ok(records)
}

pub struct Analytics {
    records: Vec<Value>,
    redaction: redaction::Config,
}

fn evaluation_projection(record: &Value) -> Result<Value, Error> {
    let mut metadata = Value::object(Vec::new());
    for field in [
        "sha",
        "session",
        "model",
        "task",
        "task_class",
        "seat",
        "effort",
        "provider",
    ] {
        copy_known(&mut metadata, record, field, field)?;
    }
    if optional(&metadata, "task_class").is_none() {
        if let Some(task) = metadata.get("task").and_then(Value::as_string) {
            if let Some(class) = crate::planning::estimate::infer_class(&task.to_utf8()?)? {
                domain::put(&mut metadata, "task_class", Value::string(&class))?;
            }
        }
    }
    Ok(metadata)
}

impl Analytics {
    fn evaluation_metadata(&self, target: &Value) -> Result<Option<Value>, Error> {
        let matches = self
            .records
            .iter()
            .filter(|record| learning::matches_evaluation(record, target))
            .collect::<Vec<_>>();
        if optional(target, "sha").is_none() {
            learning::unambiguous(&matches)?;
        }
        let selected = matches
            .iter()
            .find(|record| optional(record, "sha").is_some())
            .copied()
            .or_else(|| matches.last().copied());
        selected.map(evaluation_projection).transpose()
    }

    pub fn groups(&self) -> Result<Vec<Value>, Error> {
        learning::summarize_learning(&self.records)?
            .iter()
            .map(|value| redaction::history_record(value, &self.redaction))
            .collect()
    }

    pub fn delivery_groups(&self) -> Result<Vec<Value>, Error> {
        learning::delivery_metrics(&self.records)?
            .iter()
            .map(|value| redaction::history_record(value, &self.redaction))
            .collect()
    }

    pub fn recommendation(
        &self,
        task_class: &str,
        difficulty: Option<u8>,
    ) -> Result<Option<Value>, Error> {
        let difficulty = difficulty.unwrap_or(2);
        let ceilings = self.ceilings()?;
        let mut human = Vec::new();
        for record in &self.records {
            if optional(record, "evaluation").is_some()
                && learning::learning_seat(record)? == Text::from("coder")
                && ceilings.iter().any(|ceiling| {
                    ceiling.get("model") == record.get("model")
                        && domain::is_text(ceiling.get("seat"), "coder")
                        && number_at_least(ceiling.get("ceiling"), difficulty)
                })
            {
                human.push(record.clone());
            }
        }
        learning::recommend(&human, task_class, Some(difficulty))?.map(|value| {
            let safe = redaction::history_record(&value, &self.redaction)?;
            if safe.get("model") != value.get("model") {
                return Err(learning::error("ROSTER_ANALYTICS_INPUT", "Recommendation model metadata required redaction; no usable model was recommended"));
            }
            Ok(safe)
        }).transpose()
    }

    pub fn ceilings(&self) -> Result<Vec<Value>, Error> {
        learning::derive_difficulty_ceilings(&self.records, &self.redaction)
    }

    pub fn route_evidence(&self, scope: learning::EvidenceScope<'_>) -> Result<Vec<Value>, Error> {
        learning::explain_route_evidence(&self.records, scope)?
            .iter()
            .map(|value| redaction::history_record(value, &self.redaction))
            .collect()
    }
}

fn number_at_least(value: Option<&Value>, minimum: u8) -> bool {
    domain::safe_integer(value).is_some_and(|value| value >= u64::from(minimum))
}
fn redaction_config(environment: &Value, paths: &paths::RepositoryPaths) -> redaction::Config {
    let mut redaction = redaction::Config {
        environment: environment.clone(),
        ..Default::default()
    };
    for path in [
        &paths.worktree,
        &paths.repository,
        &paths.common_dir,
        &paths.git_dir,
    ] {
        if let Some(path) = path.to_str() {
            redaction.paths.push(Text::from(path));
        }
    }
    for name in ["HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
        if let Some(value) = environment
            .get(name)
            .and_then(Value::as_string)
            .filter(|value| !value.0.is_empty())
        {
            redaction.paths.push(value.clone());
        }
    }
    redaction
}

fn safe_models(records: &[Value], redaction: &redaction::Config) -> Result<(), Error> {
    for record in records {
        if let Some(model) = record.get("model").and_then(Value::as_string) {
            let source = model.to_utf8().map_err(|_| {
                learning::error(
                    "ROSTER_ANALYTICS_COLLATION_UNPROVEN",
                    "Unpaired UTF-16 model ordering is unproven",
                )
            })?;
            if redaction::diagnostic(model, redaction) != *model {
                return Err(learning::error(
                    "ROSTER_ANALYTICS_INPUT",
                    "Model metadata contains private or secret-shaped material; no aggregate was emitted",
                ));
            }
            if !source.is_ascii() {
                learning::collation_key(model)?;
            }
            if !(run::is_model_id(&source)
                || ["unknown", "builtin-stub"].contains(&source.as_str()))
                || source.contains("://")
                || Path::new(&source).is_absolute()
            {
                return Err(learning::error(
                    "ROSTER_ANALYTICS_INPUT",
                    "Model metadata must be a redacted model identifier, not a path or endpoint",
                ));
            }
        }
        if let Some(hardware) = optional(record, "delivery")
            .and_then(|delivery| optional(delivery, "hardware"))
            .and_then(Value::as_string)
        {
            let source = hardware.to_utf8().map_err(|_| {
                learning::error(
                    "ROSTER_ANALYTICS_COLLATION_UNPROVEN",
                    "Unpaired UTF-16 hardware ordering is unproven",
                )
            })?;
            if redaction::diagnostic(hardware, redaction) != *hardware {
                return Err(learning::error(
                    "ROSTER_ANALYTICS_INPUT",
                    "Delivery hardware metadata contains private or secret-shaped material; no aggregate was emitted",
                ));
            }
            if !source.is_ascii() {
                learning::collation_key(hardware)?;
            }
        }
    }
    Ok(())
}

pub async fn load_with_store(
    git: &Git,
    cwd: &Path,
    options: &Options,
    environment: &Value,
    store: Option<&ProvenanceStore>,
    token: &CancellationToken,
) -> Result<Analytics, Error> {
    load_selected(
        git,
        cwd,
        options,
        environment,
        ReadScope::Analytics(store),
        token,
    )
    .await
    .map_err(safe_error)
}

fn safe_error(cause: Error) -> Error {
    safe_error_at(cause, 0)
}

fn safe_error_at(mut cause: Error, depth: usize) -> Error {
    if !["LearningError", "AgentRunError", "TypeError"].contains(&cause.name)
        && !matches!(
            cause.code,
            Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT" | "ROSTER_PROCESS_CLEANUP")
        )
    {
        let mut safe = Error::new(
            cause.name,
            "The checked analytics provider refused a read; verify its declared scope and permissions. Existing bytes were preserved.",
        );
        safe.code = cause.code;
        cause = safe;
    }
    if let Some(nested) = cause.cause.take() {
        cause.cause = Some(Box::new(if depth < 32 {
            safe_error_at(*nested, depth + 1)
        } else {
            Error::new(
                "AnalyticsProviderError",
                "Further provider details were omitted",
            )
        }));
    }
    cause
}

async fn load_selected(
    git: &Git,
    cwd: &Path,
    options: &Options,
    environment: &Value,
    scope: ReadScope<'_>,
    token: &CancellationToken,
) -> Result<Analytics, Error> {
    let (store, include_evaluations) = match scope {
        ReadScope::Analytics(store) => (store, true),
        ReadScope::HumanAttribution => (None, false),
    };
    options.validate()?;
    token.check()?;
    let paths = paths::repository_paths(cwd)
        .map_err(|cause| provider_error("find the current Git repository", cause))?;
    let observed = git
        .text(&paths.worktree, &["rev-parse", "--show-toplevel"], token)
        .await
        .map_err(|cause| provider_error("find the current Git repository", cause))?;
    if paths::file_information(&PathBuf::from(observed))?.identity
        != paths::file_information(&paths.worktree)?.identity
    {
        return Err(learning::error(
            "ROSTER_ANALYTICS_GIT",
            "Git resolved a different repository; no history was read",
        ));
    }
    let selected = revision(
        git,
        &paths.worktree,
        options.reference.as_deref().unwrap_or("HEAD"),
        token,
    )
    .await?;
    let history = git
        .run(
            &paths.worktree,
            &[
                "-c",
                "log.showSignature=false",
                "-c",
                "credential.helper=",
                "-c",
                "core.pager=",
                "log",
                "--no-show-signature",
                "--no-color",
                "--no-decorate",
                "--no-notes",
                "--no-ext-diff",
                "--no-textconv",
                "--reverse",
                "--format=%H%x00%B%x00",
                "--end-of-options",
                &selected,
                "--",
            ],
            token,
        )
        .await
        .map_err(|cause| provider_error("read local Git history", cause))?;
    let exported = parse_git_history(&history)?;
    let mut budget = Budget { bytes: 0, rows: 0 };
    budget.add(history.len(), exported.len())?;
    require_local_identity(git, &paths.repository, token).await?;
    let (mut local, mut evaluations) = learning_files(&paths.repository, &mut budget, include_evaluations).map_err(|cause| {
        if cause.name == "LearningError" { cause } else { learning::error("ROSTER_ANALYTICS_INPUT", "Local learning read refused; verify bounded real files and private directories. Existing bytes were preserved.") }
    })?;
    if let Some(store) = store {
        let expected = repository_identity(git, &paths.repository, token).await?;
        local.extend(read_provenance(store, &expected, &mut budget, token).await?);
    }
    if let Some(file) = &options.evals_path {
        let file = paths::resolve(cwd, file)?;
        for mut evaluation in read_jsonl(
            &file,
            "explicit AI-Eval",
            JsonlKind::LegacyEvaluation,
            true,
            &mut budget,
        )? {
            domain::remove(&mut evaluation, "session")?;
            if exported.iter().chain(&local).any(|record| {
                optional(record, "sha").is_some()
                    && learning::matches_evaluation(record, &evaluation)
            }) {
                evaluations.push(evaluation);
            }
        }
    }
    let records = if options.delivery {
        learning::join_delivery_learning(
            &exported,
            &local,
            &evaluations,
            options.reference.is_none(),
        )?
    } else {
        learning::join_learning(&exported, &local, &evaluations, options.reference.is_none())?
    };
    let redaction = redaction_config(environment, &paths);
    safe_models(&records, &redaction)?;
    tokio::task::yield_now().await;
    token.check()?;
    Ok(Analytics { records, redaction })
}

pub async fn load(
    cwd: &Path,
    environment: &Value,
    options: &Options,
    token: &CancellationToken,
) -> Result<Analytics, Error> {
    load_invocation(cwd, environment, options, token)
        .await
        .map_err(safe_error)
}

async fn load_invocation(
    cwd: &Path,
    environment: &Value,
    options: &Options,
    token: &CancellationToken,
) -> Result<Analytics, Error> {
    options.validate()?;
    token.check()?;
    let (git, paths) = invocation_git(cwd, environment)?;
    let store = invocation_store(&git, &paths, environment, token).await?;
    load_with_store(&git, cwd, options, environment, store.as_ref(), token).await
}

fn invocation_git(cwd: &Path, environment: &Value) -> Result<(Git, paths::RepositoryPaths), Error> {
    let git_path = host::selected_path(environment, "ROSTER_GIT")?.ok_or_else(|| {
        learning::error(
            "ROSTER_ANALYTICS_HOST",
            "Read-only analytics requires ROSTER_GIT naming an absolute approved Git image",
        )
    })?;
    let paths = paths::repository_paths(cwd)
        .map_err(|cause| provider_error("find the current Git repository", cause))?;
    let host = tools::Host {
        git: git_path,
        node20: None,
        cargo: None,
        contracts: None,
        environment: host::minimal_environment(environment)?,
    };
    let git = Git::new(host, paths.git_dir.join("roster-analytics-disabled-hooks"))
        .map_err(|cause| provider_error("approve read-only Git", cause))?;
    Ok((git, paths))
}

async fn invocation_store(
    git: &Git,
    paths: &paths::RepositoryPaths,
    environment: &Value,
    token: &CancellationToken,
) -> Result<Option<ProvenanceStore>, Error> {
    let opt_out = config::environment_value(environment, "ROSTER_PROVENANCE_OPT_OUT")?.or(
        config::environment_value(environment, "ROSTER_PROVENANCE_DISABLED")?,
    );
    let opt_out = opt_out
        .map(
            |value| match value.to_utf8()?.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                _ => Err(Error::type_error(
                    "Provenance opt-out must be a boolean flag",
                )),
            },
        )
        .transpose()?
        .unwrap_or(false);
    let state = host::selected_path(environment, "ROSTER_STATE_HOME")?;
    if let Some(state) = state.filter(|_| !opt_out) {
        secure::private_directory_if_present(&state)?;
        let history = state.join("history");
        if secure::private_directory_if_present(&history)? {
            let identity = repository_identity(git, &paths.repository, token).await?;
            Ok(Some(ProvenanceStore::open(
                &history.join(&identity),
                api::Identity::Explicit(identity),
            )?))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

pub(crate) async fn evaluation_sha(
    cwd: &Path,
    environment: &Value,
    abbreviation: &str,
    token: &CancellationToken,
) -> Result<String, Error> {
    if !(4..=63).contains(&abbreviation.len())
        || !abbreviation.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(learning::error(
            "ROSTER_ANALYTICS_INPUT",
            "Commit abbreviation must contain 4-63 hexadecimal characters",
        ));
    }
    token.check()?;
    let (git, paths) = invocation_git(cwd, environment)?;
    let revision = format!("{abbreviation}^{{commit}}");
    let resolved = git
        .text(
            &paths.repository,
            &["rev-parse", "--verify", "--end-of-options", &revision],
            token,
        )
        .await
        .map_err(|cause| provider_error("resolve the evaluation commit", cause))?;
    let resolved = resolved.trim();
    if !learning::valid_sha(resolved) {
        return Err(learning::error(
            "ROSTER_ANALYTICS_INPUT",
            "Evaluation commit did not resolve to a full Git SHA",
        ));
    }
    token.check()?;
    Ok(resolved.to_ascii_lowercase())
}

pub(crate) async fn evaluation_metadata(
    cwd: &Path,
    environment: &Value,
    target: &Value,
    prior_evaluations: &[Value],
    token: &CancellationToken,
) -> Result<Option<Value>, Error> {
    token.check()?;
    let (git, paths) = invocation_git(cwd, environment)?;
    let local = load_selected(
        &git,
        cwd,
        &Options::default(),
        environment,
        ReadScope::HumanAttribution,
        token,
    )
    .await?;
    if let Some(metadata) = local.evaluation_metadata(target)? {
        return Ok(Some(metadata));
    }
    if let Some(prior) = prior_evaluations.iter().rev().find(|record| {
        learning::matches_evaluation(record, target)
            && !domain::is_text(record.get("model"), "unknown")
            && !domain::is_text(record.get("model"), "builtin-stub")
            && ["model", "task_class"]
                .iter()
                .all(|field| optional(record, field).is_some())
    }) {
        let metadata = evaluation_projection(prior)?;
        safe_models(
            std::slice::from_ref(&metadata),
            &redaction_config(environment, &paths),
        )?;
        return Ok(Some(metadata));
    }
    let Some(session) = target.get("session").and_then(Value::as_string) else {
        return Ok(None);
    };
    let origin = git
        .text(
            &paths.repository,
            &["config", "--default", "", "--get", "remote.origin.url"],
            token,
        )
        .await
        .map_err(|cause| provider_error("inspect the evaluation repository identity", cause))?;
    if origin.trim().is_empty() {
        return Ok(None);
    }
    let Some(store) = invocation_store(&git, &paths, environment, token).await? else {
        return Ok(None);
    };
    let identity = repository_identity(&git, &paths.repository, token).await?;
    let records = read_scoped_provenance(&store, &identity, &mut Budget::default(), token).await?;
    let matches = records
        .iter()
        .filter(|record| {
            domain::is_text(record.get("event"), "session")
                && domain::is_text(record.get("section"), api::RAW_HISTORY)
                && record
                    .get("seat")
                    .and_then(|seat| seat.get("name"))
                    .and_then(Value::as_string)
                    .is_some_and(|name| !name.0.is_empty())
                && (record.get("sessionId") == Some(&Value::String(session.clone()))
                    || record
                        .get("evidence")
                        .and_then(|evidence| evidence.get("attempt"))
                        .and_then(|attempt| attempt.get("sessionId"))
                        == Some(&Value::String(session.clone())))
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(learning::error(
            "ROSTER_ANALYTICS_JOIN",
            "Multiple durable session records match; use a specific attempt session",
        ));
    }
    let Some(record) = matches.first() else {
        return Ok(None);
    };
    let projected = project_provenance(record)?.ok_or_else(|| {
        learning::error(
            "ROSTER_ANALYTICS_INPUT",
            "Durable evaluation source is not a seat session",
        )
    })?;
    let mut metadata = evaluation_projection(&projected)?;
    domain::remove(&mut metadata, "model")?;
    copy_known(&mut metadata, record, "servedModel", "model")?;
    safe_models(
        std::slice::from_ref(&metadata),
        &redaction_config(environment, &paths),
    )?;
    let reference = |field| {
        optional(record, field)
            .and_then(Value::as_string)
            .filter(|value| {
                (1..=if field == "id" { 256 } else { 64 }).contains(&value.0.len())
                    && *value != &Text::from("-")
                    && value
                        .0
                        .iter()
                        .all(|unit| matches!(*unit, 45 | 46 | 48..=57 | 65..=90 | 95 | 97..=122))
            })
            .cloned()
            .ok_or_else(|| {
                learning::error(
                    "ROSTER_ANALYTICS_INPUT",
                    "Durable evaluation source reference is invalid",
                )
            })
    };
    domain::put(
        &mut metadata,
        "provenance",
        Value::object(vec![
            ("recordId", Value::String(reference("id")?)),
            ("runId", Value::String(reference("runId")?)),
            ("sessionId", Value::String(reference("sessionId")?)),
        ]),
    )?;
    token.check()?;
    Ok(Some(metadata))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProposalOutcome {
    pub created: Vec<String>,
    pub existing: Vec<String>,
    pub signatures: Vec<Value>,
}

fn valid_proposal_draft_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".md") else {
        return false;
    };
    let Some((prefix, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && hash.len() == 16
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn proposal_directory(cwd: &Path, create: bool) -> Result<(PathBuf, PathBuf), Error> {
    let root = config::shared_config_root(cwd)?;
    secure::check_components(&root)?;
    let directory = layout::resolve_directory(layout::Scope::Shared, &root, None, None)?;
    secure::check_components(&directory.dir)?;
    secure::check_components(&directory.legacy_root)?;
    let proposals = layout::state_path(directory, &["proposals"])?.path;
    paths::ensure_local_path(&proposals, &root)?;
    secure::check_components(&proposals)?;
    if create {
        secure::private_directory(&proposals, true)?;
        paths::ensure_local_path(&proposals, &root)?;
    }
    Ok((root, proposals))
}

pub fn list_failure_proposals(cwd: &Path) -> Result<Vec<String>, Error> {
    list_failure_proposals_inner(cwd).map_err(safe_error)
}

fn list_failure_proposals_inner(cwd: &Path) -> Result<Vec<String>, Error> {
    let (root, directory) = proposal_directory(cwd, false)?;
    if !secure::private_directory_if_present(&directory)? {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for name in secure::entries_bounded(&directory, MAX_DIRECTORY_ENTRIES).map_err(|_| {
        learning::error(
            "ROSTER_ANALYTICS_SIZE",
            "Proposal directory is unreadable or exceeds 256 directory entries",
        )
    })? {
        if !name.ends_with(".md") {
            continue;
        }
        let target = directory.join(&name);
        paths::ensure_local_path(&target, &root)?;
        secure::check_components(&target)?;
        let metadata = std::fs::symlink_metadata(&target)
            .map_err(|cause| secure::io_error("Inspect proposal draft", &target, cause))?;
        if !metadata.is_file() || secure::linked(&metadata) {
            return Err(learning::error(
                "ROSTER_ANALYTICS_INPUT",
                "Proposal drafts must be regular files",
            ));
        }
        if !valid_proposal_draft_name(&name) {
            return Err(learning::error(
                "ROSTER_ANALYTICS_INPUT",
                "Proposal draft names must use the generated signature slug",
            ));
        }
        if paths::file_information(&target)?.links != 1 {
            return Err(learning::error(
                "ROSTER_ANALYTICS_INPUT",
                "Proposal drafts must be single-link files",
            ));
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}

pub fn propose_recurring_failures(
    installation: &Path,
    cwd: &Path,
    environment: &Value,
) -> Result<ProposalOutcome, Error> {
    propose_recurring_failures_inner(installation, cwd, environment).map_err(safe_error)
}

fn add_declared_credential(
    redaction: &mut redaction::Config,
    environment: &Value,
    name: Option<&Value>,
) -> Result<(), Error> {
    let Some(name) = name
        .and_then(Value::as_string)
        .filter(|name| !name.0.is_empty())
    else {
        return Ok(());
    };
    if let Some(value) = config::environment_entry(environment, &name.to_utf8()?)? {
        let value = value.as_string().ok_or_else(|| {
            learning::error(
                "ROSTER_ANALYTICS_INPUT",
                "Selected credential declaration must be text",
            )
        })?;
        if !value.0.is_empty() {
            redaction.credential_values.push(value.clone());
        }
    }
    Ok(())
}

fn propose_recurring_failures_inner(
    installation: &Path,
    cwd: &Path,
    environment: &Value,
) -> Result<ProposalOutcome, Error> {
    if !matches!(environment, Value::Object(_)) {
        return Err(Error::type_error(
            "Recurring failure proposal environment must be a typed mapping",
        ));
    }
    let root = config::shared_config_root(cwd)?;
    secure::check_components(&root)?;
    paths::ensure_local_path(&root.join(".roster").join("runs"), &root)?;
    paths::ensure_local_path(&root.join(".roster").join("evals.jsonl"), &root)?;
    let mut redaction = if let Ok(repo_paths) = paths::repository_paths(cwd) {
        redaction_config(environment, &repo_paths)
    } else {
        let mut config = redaction::Config {
            environment: environment.clone(),
            ..Default::default()
        };
        if let Some(path) = root.to_str() {
            config.paths.push(Text::from(path));
        }
        for name in ["HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
            if let Some(value) = environment
                .get(name)
                .and_then(Value::as_string)
                .filter(|value| !value.0.is_empty())
            {
                config.paths.push(value.clone());
            }
        }
        config
    };
    let loaded = config::load(installation, &root, environment).map_err(|cause| {
        let mut safe = Error::new(
            cause.name,
            "Configuration diagnostics were omitted; existing bytes were preserved.",
        );
        safe.code = cause.code;
        learning::error(
            "ROSTER_ANALYTICS_CONFIG",
            "Could not load recurring-proposal configuration; verify checked configuration files and permissions",
        )
        .caused_by(safe)
    })?;
    if let Some(name) = loaded
        .config
        .as_value()
        .get("llm")
        .and_then(|llm| llm.get("api_key_env"))
        .and_then(Value::as_string)
    {
        redaction.api_key_env = name.clone();
    }
    add_declared_credential(
        &mut redaction,
        environment,
        loaded
            .config
            .as_value()
            .get("llm")
            .and_then(|llm| llm.get("api_key_env")),
    )?;
    let catalog = crate::fleet::load(&root).map_err(|cause| {
        let mut safe = Error::new(cause.name, "Private fleet provider details were omitted");
        safe.code = cause.code;
        learning::error(
            "ROSTER_ANALYTICS_CONFIG",
            "Could not read recurring-proposal fleet settings; private settings were preserved",
        )
        .caused_by(safe)
    })?;
    for profile in catalog.profiles() {
        add_declared_credential(
            &mut redaction,
            environment,
            profile.as_value().get("api_key_env"),
        )?;
    }
    let mut budget = Budget::default();
    let (runs, evaluations) = learning_files(&root, &mut budget, true).map_err(|cause| {
        if cause.name == "LearningError" {
            cause
        } else {
            learning::error(
                "ROSTER_ANALYTICS_INPUT",
                "Local learning read refused; verify bounded real files and private directories. Existing bytes were preserved.",
            )
        }
    })?;
    let mut records = learning::join_learning(&[], &runs, &evaluations, true)?;
    for (order, evaluation) in evaluations.iter().enumerate() {
        if !records
            .iter()
            .any(|record| learning::matches_evaluation(record, evaluation))
        {
            let latest = evaluations
                .iter()
                .enumerate()
                .rfind(|(_, entry)| learning::matches_evaluation(entry, evaluation))
                .map(|(index, _)| index);
            if latest == Some(order) {
                let mut row = evaluation.clone();
                domain::put(&mut row, "evaluation", evaluation.clone())?;
                domain::put(&mut row, "evaluation_order", Value::Number(order as f64))?;
                records.push(row);
            }
        }
    }
    safe_models(&records, &redaction)?;
    let recurring = learning::failure_signatures(&records)?
        .into_iter()
        .filter(|signature| {
            signature
                .get("occurrences")
                .and_then(domain::items)
                .is_some_and(|occurrences| occurrences.len() >= 3)
        })
        .collect::<Vec<_>>();
    if recurring.is_empty() {
        return Ok(ProposalOutcome::default());
    }
    let (root, directory) = proposal_directory(&root, true)?;
    let mut created = Vec::new();
    let mut existing = Vec::new();
    for proposal in &recurring {
        let slug = proposal
            .get("slug")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Proposal requires a slug"))?
            .to_utf8()?;
        let name = format!("{slug}.md");
        if !valid_proposal_draft_name(&name) {
            return Err(learning::error(
                "ROSTER_ANALYTICS_INPUT",
                "Proposal draft names must use the generated signature slug",
            ));
        }
        let file = directory.join(&name);
        paths::ensure_local_path(&file, &root)?;
        secure::check_components(&file)?;
        let content = learning::render_failure_proposal(proposal, &redaction)?;
        match secure::write_exclusive(&file, content.as_bytes()) {
            Ok(_) => created.push(name),
            Err(error) if error.code == Some("EEXIST") => {
                let metadata = std::fs::symlink_metadata(&file).map_err(|cause| {
                    secure::io_error("Inspect existing proposal draft", &file, cause)
                })?;
                if !metadata.is_file()
                    || secure::linked(&metadata)
                    || paths::file_information(&file)?.links != 1
                {
                    return Err(learning::error(
                        "ROSTER_ANALYTICS_INPUT",
                        "Existing proposal must be a regular single-link draft",
                    ));
                }
                existing.push(name);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(ProposalOutcome {
        created,
        existing,
        signatures: recurring,
    })
}

#[cfg(test)]
mod tests;
