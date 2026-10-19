use std::path::PathBuf;

use super::*;
use crate::llm::openai::{Completion, LastResponse};
use crate::run::{RunRecord, UsageSample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Configured,
    Stub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Cancelled,
    Stub,
    Unavailable,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Cancelled => "cancelled",
            Self::Stub => "stub",
            Self::Unavailable => "unavailable",
        }
    }
}

pub struct Inputs<'a> {
    pub mode: Mode,
    pub capsule_version: continuity::CapsuleVersion,
    pub capsule: Option<&'a Value>,
    pub prior_wave_files: &'a [String],
    pub initial_baseline: Option<&'a evidence::Snapshot>,
}

pub struct Attempt {
    authority: String,
    task: Text,
    root: crate::paths::FileIdentity,
    writer: std::sync::Arc<()>,
    config: crate::config::Config,
    mode: Mode,
    baseline: evidence::Snapshot,
    baseline_audit: std::collections::BTreeMap<String, String>,
    workspace: continuity::Workspace,
    decoder: coder_calls::Decoder,
    pending: Vec<coder_calls::Call>,
    summary: Option<Text>,
    responses: Vec<UsageSample>,
    last_response: Option<LastResponse>,
    had_usage: bool,
    failure: Option<Error>,
    self_review: Option<self_review::Report>,
    prior_wave_files: Vec<String>,
    context_binding: Option<context::Binding>,
    native_hints: Vec<String>,
    backend_approval: Option<backend::Approval>,
    classification: Option<classification::Report>,
    annotations: Vec<(&'static str, Value)>,
}

pub struct Finalized {
    authority: String,
    root: crate::paths::FileIdentity,
    writer: std::sync::Arc<()>,
    snapshot: evidence::Snapshot,
    audit: std::collections::BTreeMap<String, String>,
    status: Status,
    report: Value,
    result: Value,
    run: Option<RunRecord>,
    body: Text,
    capsule: Option<Value>,
    validation: Option<Value>,
    independent: Option<docker::Report>,
}

fn values(reasons: &[Text]) -> Value {
    Value::Array(reasons.iter().cloned().map(Value::String).collect())
}
fn reason(reasons: &mut Vec<Text>, value: impl AsRef<str>) {
    reasons.push(Text::from(value.as_ref()));
}

fn append_backend_reasons(report: &Value, key: &str, target: &mut Vec<Text>) -> Result<(), Error> {
    let entries = domain::items(required(report, key)?)
        .ok_or_else(|| Error::type_error("Backend receipt reasons must be an array"))?;
    for entry in entries {
        target.push(
            entry
                .as_string()
                .cloned()
                .ok_or_else(|| Error::type_error("Backend receipt reason must be text"))?,
        );
    }
    Ok(())
}

fn full_verification_value(
    record: &tools::FullVerification,
    current: bool,
    resolution: &str,
    clean: bool,
) -> Value {
    let failures = record.failure_exits();
    let observed = record.latest();
    Value::object(vec![
        (
            "original_exit",
            failures.map_or(Value::Null, |pair| Value::Number(f64::from(pair.0))),
        ),
        (
            "last_failed_exit",
            failures.map_or(Value::Null, |pair| Value::Number(f64::from(pair.1))),
        ),
        (
            "latest_full_exit",
            observed.map_or(Value::Null, |proof| {
                Value::Number(f64::from(proof.observed_exit()))
            }),
        ),
        ("latest_full_completed", Value::Boolean(observed.is_some())),
        ("latest_full_current", Value::Boolean(current)),
        ("clean_full_suite", Value::Boolean(clean)),
        ("resolution", Value::string(resolution)),
    ])
}

fn backend_issue(
    error: Error,
    reasons: &mut Vec<Text>,
    unavailable: &mut Vec<Text>,
) -> Result<(), Error> {
    if crate::cancel::is_run_cancelled(&error)
        || ["ROSTER_TASK_TIMEOUT", "ROSTER_PROCESS_CLEANUP"].contains(&error.code.unwrap_or(""))
    {
        return Err(error);
    }
    let target = if error.code == Some("ROSTER_TOOL_DENIED") {
        reasons
    } else {
        unavailable
    };
    target.push(Text::from(error.message.as_str()));
    Ok(())
}

async fn needs_counterfactual(
    boundary: &tools::Boundary,
    files: &[String],
    changes: &evidence::Changes,
) -> Result<bool, Error> {
    for file in files {
        if !regex(r"(?:^|/)[^/]+\.test\.[cm]?js$|\.rs$")?.is_match(file)
            || crate::planning::forbidden_read(file)
        {
            continue;
        }
        let Some(candidate) = evidence::read_product(boundary.root(), file, MAX_FILE_BYTES)? else {
            continue;
        };
        let names = substance::counterfactual_names(file, &candidate)?;
        if names.is_empty() {
            continue;
        }
        let previous = changes
            .head_product(boundary, file)
            .await?
            .unwrap_or_default();
        let old_names = substance::counterfactual_names(file, &previous)?;
        if names.iter().any(|name| !old_names.contains(name)) {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Attempt {
    pub fn begin(
        boundary: &tools::Boundary,
        config: &crate::config::Config,
        inputs: Inputs<'_>,
        projection: &evidence::Projection,
    ) -> Result<Self, Error> {
        let Inputs {
            mode,
            capsule_version: version,
            capsule,
            prior_wave_files,
            initial_baseline,
        } = inputs;
        boundary.check()?;
        if boundary.role() != Role::Coder {
            return Err(denied(
                "Execution finalization requires the coder host boundary, never reviewer write authority",
            ));
        }
        if prior_wave_files.len() > MAX_SCOPE_ENTRIES {
            return Err(denied(
                "Prior-wave context exceeds the bounded required-read contract",
            ));
        }
        for file in prior_wave_files {
            if !policy::exact_path(file) || crate::planning::forbidden_read(file) {
                return Err(denied(
                    "Prior-wave evidence must name exact readable product files",
                ));
            }
            if !boundary.requires_read(file) {
                return Err(denied(
                    "Prior-wave files must already be registered as boundary required_reads before the attempt begins",
                ));
            }
        }
        if mode == Mode::Configured
            && config
                .as_value()
                .get("llm")
                .and_then(|llm| llm.get("base_url"))
                .and_then(Value::as_string)
                .is_none_or(|url| url.0.is_empty())
        {
            return Err(Error::type_error(
                "Configured execution requires an explicitly configured model route",
            ));
        }
        let captured = evidence::capture(boundary.root(), None, projection)?;
        let baseline = initial_baseline.cloned().unwrap_or(captured.clone());
        baseline.changed(&captured)?;
        if initial_baseline.is_some()
            && baseline.entries().get("TASK.md") != captured.entries().get("TASK.md")
        {
            return Err(denied(
                "Retained attempt baseline belongs to different authoritative TASK bytes",
            ));
        }
        Ok(Self {
            authority: boundary.authority_digest(),
            task: boundary.task().clone(),
            root: crate::paths::file_information(boundary.root())?.identity,
            writer: boundary.writer_identity(),
            config: config.clone(),
            mode,
            baseline,
            baseline_audit: boundary.audit()?,
            workspace: continuity::Workspace::new(boundary, config, version, capsule, projection)?,
            decoder: coder_calls::Decoder::default(),
            pending: Vec::new(),
            summary: None,
            responses: Vec::new(),
            last_response: None,
            had_usage: false,
            failure: None,
            self_review: None,
            prior_wave_files: prior_wave_files.to_vec(),
            context_binding: None,
            native_hints: Vec::new(),
            backend_approval: None,
            classification: None,
            annotations: Vec::new(),
        })
    }

    fn check(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        boundary.check()?;
        if self.authority != boundary.authority_digest()
            || !std::sync::Arc::ptr_eq(&self.writer, &boundary.writer_identity())
            || self.task != *boundary.task()
            || self.root != crate::paths::file_information(boundary.root())?.identity
        {
            return Err(denied(
                "Frozen execution attempt does not match current authoritative handoff or worktree",
            ));
        }
        Ok(())
    }
    pub fn workspace(&self) -> &continuity::Workspace {
        &self.workspace
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn approve_backends(
        &mut self,
        boundary: &tools::Boundary,
        approval: backend::Approval,
    ) -> Result<(), Error> {
        self.check(boundary)?;
        approval.validate()?;
        if self.backend_approval.is_some()
            || self.context_binding.is_some()
            || !self.responses.is_empty()
            || !self.pending.is_empty()
        {
            return Err(denied(
                "Backend host approval must be frozen once before context/model/tool execution",
            ));
        }
        self.backend_approval = Some(approval);
        Ok(())
    }

    pub async fn context(
        &mut self,
        boundary: &tools::Boundary,
        repo_root: &std::path::Path,
        projection: &evidence::Projection,
    ) -> Result<context::Context, Error> {
        self.context_with(boundary, repo_root, projection, context::Options::default())
            .await
    }

    pub(crate) async fn context_with(
        &mut self,
        boundary: &tools::Boundary,
        repo_root: &std::path::Path,
        projection: &evidence::Projection,
        options: context::Options<'_>,
    ) -> Result<context::Context, Error> {
        self.check(boundary)?;
        self.workspace.refresh(boundary, projection)?;
        let mut anticipated = self.native_hints.clone();
        for call in &self.pending {
            if !["write_file", "edit_file", "delete_file"].contains(&call.name.as_str()) {
                continue;
            }
            let Some(file) = call
                .args
                .get("path")
                .and_then(Value::as_string)
                .map(Text::to_utf8)
                .transpose()?
            else {
                continue;
            };
            let file = file.replace('\\', "/");
            if policy::exact_path(&file)
                && !crate::planning::forbidden_write(&file)
                && policy::requires_native(std::slice::from_ref(&file))?
                && !anticipated.contains(&file)
            {
                anticipated.push(file);
            }
        }
        if anticipated.len() > MAX_SCOPE_ENTRIES {
            return Err(denied(
                "Anticipated native context paths exceed the bounded context contract",
            ));
        }
        let context = context::load(
            boundary,
            repo_root,
            &self.config,
            context::Options {
                acceptance: self.workspace.source().map(|_| &self.workspace),
                projection: Some(projection),
                prior_wave_files: &self.prior_wave_files,
                anticipated_files: &anticipated,
                ..options
            },
        )
        .await?;
        self.context_binding = Some(context.binding());
        self.native_hints = anticipated;
        self.pending.clear();
        self.summary = None;
        Ok(context)
    }

    pub fn response(
        &mut self,
        boundary: &tools::Boundary,
        completion: &Completion,
    ) -> Result<coder_calls::Decoded, Error> {
        self.response_offering(boundary, completion, None)
    }

    pub(crate) fn response_offering(
        &mut self,
        boundary: &tools::Boundary,
        completion: &Completion,
        offered_subset: Option<&[String]>,
    ) -> Result<coder_calls::Decoded, Error> {
        self.check(boundary)?;
        self.context_binding
            .as_ref()
            .ok_or_else(|| {
                denied("Required bounded context has not been delivered for this attempt")
            })?
            .require_current(boundary)?;
        if self.mode == Mode::Stub {
            return Err(denied(
                "Deterministic stub cannot invoke a model, product tools or tests",
            ));
        }
        if self.failure.is_some() {
            return Err(denied(
                "Failed execution attempt cannot accept a late model response",
            ));
        }
        if !self.pending.is_empty() {
            return Err(Error::new(
                "Error",
                "Execute or reject the existing structured batch before another response",
            ));
        }
        let mut offered = match self.workspace.definitions(boundary) {
            Value::Array(definitions) => definitions
                .iter()
                .filter_map(|definition| {
                    definition
                        .get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_string)
                        .and_then(|name| name.to_utf8().ok())
                })
                .collect::<Vec<_>>(),
            _ => {
                return Err(Error::type_error(
                    "Execution tool definitions are malformed",
                ));
            }
        };
        if let Some(subset) = offered_subset {
            if subset.iter().any(|name| !offered.contains(name)) {
                return Err(denied(
                    "The host offering is not a subset of current seat authority",
                ));
            }
            offered.retain(|name| subset.contains(name));
        }
        let turn_budget = domain::safe_integer(
            self.config
                .as_value()
                .get("seat")
                .and_then(|seat| seat.get("turn_budget")),
        )
        .unwrap_or(1000)
        .min(4096);
        if self.responses.len() as u64 >= turn_budget {
            let error = Error::type_error("Frozen execution response budget is exhausted");
            self.failure = Some(error.clone());
            return Err(error);
        }
        let sample = UsageSample {
            prompt_tokens: completion
                .usage
                .as_ref()
                .and_then(|usage| usage.get("prompt_tokens"))
                .and_then(Value::as_number),
            completion_tokens: completion
                .usage
                .as_ref()
                .and_then(|usage| usage.get("completion_tokens"))
                .and_then(Value::as_number),
        };
        self.responses.push(sample);
        self.had_usage |= completion.usage.is_some();
        let usage = crate::run::merge_usage(&self.responses)?;
        self.last_response = Some(LastResponse {
            model: completion.model.clone(),
            usage: self.had_usage.then_some(usage),
        });
        self.summary = None;
        let decoded = match self.decoder.decode(completion, &offered) {
            Ok(decoded) => decoded,
            Err(error) => {
                self.failure = Some(error.clone());
                return Err(error);
            }
        };
        if decoded.correction.is_none() {
            self.pending = decoded.calls.clone();
            if decoded.calls.is_empty() {
                self.summary = completion
                    .message
                    .get("content")
                    .and_then(Value::as_string)
                    .cloned();
            }
        }
        Ok(decoded)
    }

    pub async fn execute(
        &mut self,
        boundary: &mut tools::Boundary,
        call: &coder_calls::Call,
        projection: &evidence::Projection,
    ) -> Result<Value, Error> {
        self.check(boundary)?;
        self.context_binding
            .as_ref()
            .ok_or_else(|| {
                denied("Required bounded context has not been delivered for this attempt")
            })?
            .require_current(boundary)?;
        if self.mode == Mode::Stub || self.failure.is_some() {
            return Err(denied(
                "Stub or failed attempt cannot execute product tools",
            ));
        }
        if ["write_file", "edit_file", "delete_file"].contains(&call.name.as_str()) {
            if let Some(file) = call.args.get("path").and_then(Value::as_string) {
                let file = boundary.checked_product_write_path(&file.to_utf8()?)?;
                boundary.require_mutation_capability(&file)?;
                self.context_binding
                    .as_ref()
                    .ok_or_else(|| {
                        denied("Required bounded context has not been delivered for this attempt")
                    })?
                    .require_guidance_for(&file)?;
            }
        }
        let index = self
            .pending
            .iter()
            .position(|pending| {
                pending.id == call.id && pending.name == call.name && pending.args == call.args
            })
            .ok_or_else(|| {
                denied(
                    "Execution call was not authorized by the current strict structured response",
                )
            })?;
        self.pending.remove(index);
        if ["write_file", "edit_file", "delete_file"].contains(&call.name.as_str()) {
            self.classification = None;
            self.self_review = None;
        }
        let result = self
            .workspace
            .execute(boundary, &call.name, &call.args, projection)
            .await;
        if let Err(error) = &result {
            if error.name != "ToolUsageError"
                && error.name != "TypeError"
                && !tools::correctable_content(error)
            {
                self.failure = Some(error.clone());
            }
        }
        result
    }

    pub fn record_failure(&mut self, error: Error) {
        self.failure = self.failure.take().or(Some(error));
    }

    pub(crate) fn failed_response(
        &mut self,
        response: Option<&LastResponse>,
        usage: Option<&Value>,
    ) -> Result<(), Error> {
        self.responses.push(UsageSample {
            prompt_tokens: usage
                .and_then(|value| value.get("prompt_tokens"))
                .and_then(Value::as_number),
            completion_tokens: usage
                .and_then(|value| value.get("completion_tokens"))
                .and_then(Value::as_number),
        });
        self.had_usage |= usage.is_some();
        if let Some(response) = response {
            self.last_response = Some(LastResponse {
                model: response.model.clone(),
                usage: self
                    .had_usage
                    .then(|| crate::run::merge_usage(&self.responses))
                    .transpose()?,
            });
        } else if let Some(response) = &mut self.last_response {
            response.usage = self
                .had_usage
                .then(|| crate::run::merge_usage(&self.responses))
                .transpose()?;
        }
        Ok(())
    }

    pub(crate) fn annotate(&mut self, name: &'static str, value: Value) {
        self.annotations.retain(|(key, _)| *key != name);
        self.annotations.push((name, value));
    }

    pub(crate) fn summary(&mut self, summary: Text) {
        self.summary = Some(summary);
    }

    pub(crate) fn observed_response(&self) -> Option<&LastResponse> {
        self.last_response.as_ref()
    }

    pub(crate) fn workspace_close(&mut self, summary: &Text) -> Result<(), Error> {
        self.workspace.close_summary(summary)
    }

    pub(crate) fn changed_since_start(
        &self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<Vec<String>, Error> {
        self.check(boundary)?;
        self.baseline
            .changed(&evidence::capture(boundary.root(), None, projection)?)
    }

    pub(crate) async fn verify_final(
        &mut self,
        boundary: &mut tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<Value, Error> {
        self.check(boundary)?;
        let validation = checkpoints::validation(boundary).await?;
        if let Some(observed) = boundary
            .full_verification()
            .and_then(tools::FullVerification::latest)
        {
            let current = observed.require_current_observation(boundary).is_ok()
                && observed.validation.as_ref() == Some(&validation);
            if current
                && observed.observed_exit() == 0
                && !observed.observed_skipped()
                && observed.require_current_coverage(boundary).is_ok()
            {
                return Ok(observed.result().clone());
            }
            if current && observed.full_exception_eligible() {
                if let Some(report) = &self.classification {
                    if report.all_outside_proven()
                        && report.matches_observation(observed)
                        && report.require_current(boundary).await.is_ok()
                    {
                        if let Some(scoped) = boundary.last_tests() {
                            if scoped.observed_exit() == 0
                                && !scoped.observed_skipped()
                                && scoped.require_current_observation(boundary).is_ok()
                                && scoped.require_current_coverage(boundary).is_ok()
                            {
                                return Ok(scoped.result().clone());
                            }
                        }
                    }
                }
            }
        }
        self.verify_full(boundary, projection).await
    }

    pub(crate) fn has_current_self_review(&self, boundary: &tools::Boundary) -> bool {
        self.self_review.is_some() && self.check(boundary).is_ok()
    }

    pub async fn restore_checkpoint<P: restore::PublicationCheck>(
        &mut self,
        boundary: &mut tools::Boundary,
        approval: restore::Approval,
        publication: &mut P,
    ) -> Result<restore::Report, Error> {
        self.check(boundary)?;
        let result = restore::execute(boundary, approval, publication).await;
        if self.authority != boundary.authority_digest()
            || !std::sync::Arc::ptr_eq(&self.writer, &boundary.writer_identity())
        {
            self.pending.clear();
            self.context_binding = None;
            self.classification = None;
            self.self_review = None;
            self.summary = None;
        }
        result
    }

    pub async fn verify_full(
        &mut self,
        boundary: &mut tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<Value, Error> {
        let _scope = boundary.span(super::timing::Label::FullVerification);
        self.check(boundary)?;
        self.context_binding
            .as_ref()
            .ok_or_else(|| {
                denied("Required bounded context has not been delivered for this attempt")
            })?
            .require_current(boundary)?;
        if self.mode == Mode::Stub || self.failure.is_some() || !self.pending.is_empty() {
            return Err(denied(
                "Host full verification requires a current configured attempt with no failed or pending calls",
            ));
        }
        let result = self.workspace.verify_full(boundary, projection).await?;
        if result.get("exit_code") != Some(&Value::Number(0.0)) {
            if let Some(approval) = self
                .backend_approval
                .as_ref()
                .filter(|approval| approval.classifications)
            {
                let scope = boundary.span(super::timing::Label::Classification);
                let report = classification::run(boundary, approval).await;
                super::timing::finish(scope, report.as_ref().err());
                let report = report?;
                let proven = report.all_outside_proven();
                self.annotate("fullClassification", report.as_value().clone());
                self.classification = Some(report);
                if proven {
                    return self
                        .workspace
                        .execute(boundary, "run_test", &Value::object(Vec::new()), projection)
                        .await;
                }
            }
        }
        Ok(result)
    }

    pub async fn self_review_request(
        &self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<self_review::Request, Error> {
        self.check(boundary)?;
        let changes = evidence::changes(boundary, None).await?;
        let budget = domain::safe_integer(
            self.config
                .as_value()
                .get("seat")
                .and_then(|seat| seat.get("context_chars")),
        )
        .unwrap_or(8000);
        self_review::Request::prepare(
            boundary,
            &changes,
            usize::try_from(budget)
                .map_err(|_| Error::type_error("Self-review context exceeds this platform"))?,
            projection,
        )
    }
    pub fn attach_self_review(&mut self, report: self_review::Report) -> Result<(), Error> {
        if self.self_review.is_some() {
            return Err(Error::new(
                "Error",
                "Self-review adapter accepts one report per frozen attempt",
            ));
        }
        self.self_review = Some(report);
        Ok(())
    }

    pub async fn finish(
        mut self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<Finalized, Error> {
        self.evaluate(boundary, projection).await
    }

    pub(crate) fn evaluate<'a>(
        &'a mut self,
        boundary: &'a tools::Boundary,
        projection: &'a evidence::Projection,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Finalized, Error>> + 'a>> {
        Box::pin(self.evaluate_inner(boundary, projection))
    }

    async fn evaluate_inner(
        &mut self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<Finalized, Error> {
        let _scope = boundary.span(super::timing::Label::Finalization);
        if let Some(cancelled) = boundary.token().reason()? {
            return self.terminal_without_success(
                boundary,
                Status::Cancelled,
                cancelled.error(),
                projection,
            );
        }
        self.check(boundary)?;
        self.workspace.refresh(boundary, projection)?;
        let mut current = evidence::capture(boundary.root(), None, projection)?;
        let mut validation = checkpoints::validation(boundary).await?;
        let mut audit = boundary.audit()?;
        let changes = evidence::changes(boundary, None).await?;
        let baseline_changed = self.baseline.changed(&current)?;
        let mut files = baseline_changed.clone();
        files.extend(changes.files.iter().cloned());
        files.retain(|file| {
            !policy::managed(file)
                && !evidence::ignored_notebook(file, None)
                && file
                    .split('/')
                    .next()
                    .is_none_or(|part| !part.eq_ignore_ascii_case("vendor"))
        });
        files.sort();
        files.dedup();
        let mut reasons = Vec::new();
        let mut unavailable = Vec::new();
        if let Some(binding) = &self.context_binding {
            if let Err(error) = binding.require_current(boundary) {
                unavailable.push(Text::from(error.message.as_str()));
            }
        } else {
            unavailable.push(Text::from(
                "Required bounded context was not prepared/delivered for this attempt.",
            ));
        }
        if self.mode == Mode::Stub {
            reason(
                &mut reasons,
                "Deterministic stub: implementation and acceptance checks were not executed.",
            );
        }
        if let Some(error) = &self.failure {
            reasons.push(crate::planning::memory::redact(
                error
                    .message_text
                    .as_ref()
                    .unwrap_or(&Text::from(error.message.as_str())),
                boundary.redaction(),
            ));
        }
        if let Some(error) = current
            .projection_failure()
            .filter(|_| self.workspace.source().is_some())
        {
            unavailable.push(Text::from(error.message.as_str()));
        }
        if !self.pending.is_empty() {
            reason(
                &mut reasons,
                "Structured tool calls remain unexecuted; the attempt is incomplete.",
            );
        }
        let model = self
            .last_response
            .as_ref()
            .map(|response| response.model.to_utf8())
            .transpose()?
            .unwrap_or_else(|| "builtin-stub".to_owned());
        if self.mode == Mode::Configured
            && (self.responses.is_empty()
                || !crate::run::is_model_id(&model)
                || ["unknown", "builtin-stub"].contains(&model.as_str()))
        {
            reason(
                &mut reasons,
                "Coder model ID or actual response-backed turn count is missing or invalid.",
            );
        }
        let summary = self.summary.clone().unwrap_or_default();
        if crate::text::trim(&summary.0).is_empty() {
            reason(&mut reasons, "Coder result summary is missing.");
        }
        if self.workspace.source().is_none() {
            self.workspace.close_summary(&summary)?;
        }
        if let Err(error) = self.workspace.require_closed() {
            reasons.push(Text::from(error.message.as_str()));
        }
        let scope = boundary.scope()?;
        let tests_mode = policy::tests_mode(boundary.task())?;
        if boundary.requires_restored_checks() {
            unavailable.push(Text::from("A restored checkpoint is only a candidate; fresh current-scope checks are required before any passing finalization."));
        }
        let mut effective = scope.clone();
        effective.extend(boundary.written_files());
        effective.extend(files.iter().cloned());
        let native = policy::requires_native(&effective)?;
        let docs = policy::docs_only(&effective)?;
        match boundary.last_tests() {
            Some(proof) => {
                if let Err(error) = proof.require_current_observation(boundary) {
                    reasons.push(Text::from(error.message.as_str()));
                }
                if proof
                    .validation
                    .as_ref()
                    .is_some_and(|observed| *observed != validation)
                {
                    reason(
                        &mut reasons,
                        "Actual check evidence no longer matches current Git, task or checkpoint identity.",
                    );
                }
                if proof.observed_exit() != 0 {
                    reason(
                        &mut reasons,
                        format!(
                            "{} checks failed (exit {}).",
                            if native {
                                "Native/scoped"
                            } else {
                                "node --test"
                            },
                            proof.observed_exit()
                        ),
                    );
                }
                if native || !docs && tests_mode != "none" {
                    if proof.observed_skipped() {
                        reason(
                            &mut reasons,
                            "Required checks were skipped; skipped evidence is not passing verification.",
                        );
                    }
                    if let Err(error) = proof.require_current_coverage(boundary) {
                        reasons.push(Text::from(error.message.as_str()));
                    }
                }
            }
            None if native || !docs && tests_mode != "none" => reason(
                &mut reasons,
                "Required checks were not executed; no privately sealed actual verification evidence exists.",
            ),
            None => {}
        }
        let app_diff = baseline_changed
            .iter()
            .any(|file| crate::planning::is_allowed(file, &boundary.document().files_allowed));
        let original = &boundary.document().files_allowed;
        if self.mode == Mode::Configured
            && original.len() == 1
            && !original.first().is_some_and(|file| file.ends_with(".md"))
            && !app_diff
        {
            reason(
                &mut reasons,
                format!(
                    "Bounded task must produce an application diff in {}; passing existing tests or rewriting identical content does not implement the Ask.",
                    original.first().map_or("", String::as_str)
                ),
            );
        }
        if self.mode == Mode::Configured
            && original.len() == 1
            && !boundary
                .written_files()
                .iter()
                .any(|file| crate::planning::is_allowed(file, original))
        {
            if policy::readme_only(original) {
                let existing =
                    evidence::read_product(boundary.root(), "README.md", MAX_FILE_BYTES)?;
                if !existing.as_ref().is_some_and(|source| {
                    source.to_utf8().is_ok_and(|source| {
                        policy::status_section_present(&source).unwrap_or(false)
                    })
                }) {
                    reason(
                        &mut reasons,
                        "README-only docs task must write README.md before finishing",
                    );
                }
            } else {
                reason(
                    &mut reasons,
                    format!(
                        "Bounded task must write {} before finishing",
                        original.first().map_or("", String::as_str)
                    ),
                );
            }
        }
        let added = substance::added_lines(&changes.diff)?;
        let mut test_additions = Text::default();
        let mut has_test_evidence = false;
        let mut sentinels = Vec::new();
        let mut app_added = Vec::new();
        let mut product_changed = false;
        for file in &files {
            if crate::planning::forbidden_write(file) {
                reason(
                    &mut reasons,
                    format!("Diff path is protected or outside TASK.md allowed paths: {file}"),
                );
                continue;
            }
            if !crate::planning::is_allowed(file, &scope) {
                reason(
                    &mut reasons,
                    format!("Diff path is outside TASK.md allowed paths: {file}"),
                );
                continue;
            }
            let is_test = substance::is_test_file(file)?;
            if !is_test {
                product_changed |= regex(r"\.[cm]?[jt]sx?$|\.rs$|\.py$")?.is_match(file);
            }
            let Some(source) = evidence::read_product(boundary.root(), file, MAX_FILE_BYTES)?
            else {
                continue;
            };
            let secret_lines =
                crate::state::redaction::secret_material_lines(&source, boundary.redaction());
            if !secret_lines.is_empty() {
                reason(
                    &mut reasons,
                    format!(
                        "Secret material detected in changed file: {file} (line {})",
                        secret_lines
                            .iter()
                            .take(8)
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
            let additions = added
                .iter()
                .find(|(name, _)| name == file)
                .map_or(&source, |(_, text)| text);
            if is_test {
                has_test_evidence |= substance::adds_test_evidence(additions)?;
                test_additions.0.extend(&additions.0);
                test_additions.0.push(10);
                reasons.extend(substance::analyze(
                    &Text::from(file.as_str()),
                    &source,
                    additions,
                )?);
                sentinels.extend(substance::sentinel_values(additions)?);
            } else {
                app_added.push((Text::from(file.as_str()), additions.clone()));
            }
        }
        reasons.extend(substance::sentinel_special_cases(&app_added, &sentinels));
        if !has_test_evidence
            && !crate::text::trim(&test_additions.0).is_empty()
            && !files.iter().any(|file| {
                !substance::is_test_file(file).unwrap_or(false)
                    && crate::planning::is_allowed(file, &scope)
            })
            && (crate::planning::estimate::read_task_metadata(&Value::String(
                boundary.task().clone(),
            ))
            .is_ok_and(|metadata| metadata.task_class == "test")
                || substance::adds_only_imports(&test_additions)?)
        {
            reason(
                &mut reasons,
                "Test substance: the diff changes only test files but adds no new test block or assertion; imports, comments, or fixtures alone do not implement the Ask.",
            );
        }
        if !crate::state::redaction::secret_material_lines(&changes.diff, boundary.redaction())
            .is_empty()
        {
            reason(&mut reasons, "Secret material detected in the Git diff.");
        }
        let shadow = if self.mode == Mode::Configured {
            shadow::check(
                boundary,
                &files,
                &changes,
                &self.prior_wave_files,
                boundary.task(),
            )
            .await?
        } else {
            Value::object(vec![
                ("status", Value::string("none")),
                ("findings", Value::Array(Vec::new())),
            ])
        };
        for finding in shadow
            .get("findings")
            .and_then(domain::items)
            .unwrap_or(&[])
        {
            reasons.push(
                required(finding, "reason")?
                    .as_string()
                    .cloned()
                    .ok_or_else(|| Error::type_error("Shadow finding requires text"))?,
            );
        }
        let needs_self_review = self.mode == Mode::Configured && self_review::needed(&files)?;
        if needs_self_review {
            match &self.self_review {
                Some(report) => {
                    if let Err(error) = report.require_current(boundary, projection).await { unavailable.push(Text::from(error.message.as_str())); }
                    if domain::is_text(report.as_value().get("status"), "findings") { reasons.extend(self_review::reasons(report.as_value())?); }
                }
                None => unavailable.push(Text::from("Applicable tool-free self-review adapter has not returned a response or an explicit advisory failure.")),
            }
        }
        let mut pending = Vec::new();
        let mut red_green = None;
        let mut hook_report = None;
        let mut end_to_end = None;
        let mut counterfactual_required = false;
        if tests_mode == "required" && product_changed {
            match needs_counterfactual(boundary, &files, &changes).await {
                Ok(true) => counterfactual_required = true,
                Ok(false) => {}
                Err(error) => unavailable.push(Text::from(
                    format!(
                        "Counterfactual applicability could not read independent base evidence: {}",
                        error.message
                    )
                    .as_str(),
                )),
            }
        }
        let mut full_verification = None;
        if let Some(record) = boundary
            .full_verification()
            .filter(|record| record.requires_resolution())
        {
            let observed = record.latest();
            let failures = record.failure_exits();
            let current_full = observed.is_some_and(|proof| {
                proof.require_current_observation(boundary).is_ok()
                    && proof.validation.as_ref() == Some(&validation)
            });
            let full_pass = current_full
                && observed.is_some_and(|proof| {
                    proof.observed_exit() == 0
                        && !proof.observed_skipped()
                        && proof.require_current_coverage(boundary).is_ok()
                });
            if full_pass {
                self.classification = None;
            }
            let outside = if let Some(observed) =
                observed.filter(|proof| current_full && proof.full_exception_eligible())
            {
                if let Some(report) = &self.classification {
                    report.all_outside_proven()
                        && report.matches_observation(observed)
                        && report.require_current(boundary).await.is_ok()
                } else {
                    false
                }
            } else {
                false
            };
            if !full_pass && !outside {
                if let Some((_, failed)) = failures {
                    reason(
                        &mut reasons,
                        format!(
                            "Previous actual full verification failed (exit {failed}); scoped checks, source changes and stale or incomplete classifications cannot discharge that obligation."
                        ),
                    );
                } else {
                    unavailable.push(Text::from("Requested full verification has no completed actual receipt; a scoped pass cannot replace unavailable full evidence."));
                }
            }
            full_verification = Some(full_verification_value(
                record,
                current_full,
                if full_pass {
                    "full-pass"
                } else if outside {
                    "outside-exception"
                } else {
                    "unresolved"
                },
                full_pass,
            ));
        }
        if let Some(report) = &self.classification {
            if let Err(error) = report.require_current(boundary).await {
                reasons.push(Text::from(error.message.as_str()));
            }
        }
        if self.mode == Mode::Configured && reasons.is_empty() {
            if let Some(approval) = &self.backend_approval {
                if counterfactual_required {
                    match counterfactual::run(boundary, approval, &changes, &files).await {
                        Ok(report) => {
                            if !report.passed() {
                                let target = if domain::is_text(
                                    report.as_value().get("status"),
                                    "unavailable",
                                ) {
                                    &mut unavailable
                                } else {
                                    &mut reasons
                                };
                                append_backend_reasons(report.as_value(), "reasons", target)?;
                                if target.is_empty() {
                                    target.push(Text::from("Counterfactual gate did not establish named red/green evidence."));
                                }
                            }
                            red_green = Some(report);
                        }
                        Err(error) => backend_issue(error, &mut reasons, &mut unavailable)?,
                    }
                }
                if reasons.is_empty() {
                    match hooks::run(boundary, "post-coder", approval).await {
                        Ok(report) => {
                            if !report.passed() {
                                append_backend_reasons(report.as_value(), "reasons", &mut reasons)?;
                            }
                            hook_report = Some(report);
                        }
                        Err(error) => backend_issue(error, &mut reasons, &mut unavailable)?,
                    }
                }
                if reasons.is_empty() {
                    match e2e::run(boundary, approval).await {
                        Ok(report) => {
                            if !report.passed() {
                                let target = if domain::is_text(
                                    report.as_value().get("status"),
                                    "unavailable",
                                ) {
                                    &mut unavailable
                                } else {
                                    &mut reasons
                                };
                                append_backend_reasons(report.as_value(), "failures", target)?;
                            }
                            end_to_end = Some(report);
                        }
                        Err(error) => backend_issue(error, &mut reasons, &mut unavailable)?,
                    }
                }
            } else if counterfactual_required {
                pending.push("independent red/green execution".to_owned());
            }
        }
        let hook_file = boundary.root().join(".roster").join("hooks.yml");
        if self.backend_approval.is_none() {
            if let Some(hooks) = crate::state::secure::read(&hook_file, 65_536)? {
                if crate::paths::file_information(&hook_file)?.links != 1 {
                    return Err(denied("Lifecycle hook manifest must remain single-link"));
                }
                let hooks =
                    policy::parse_hooks(&Text::from(tools::bytes_text(&hooks.bytes)?.as_str()))?;
                if hooks.get("event").is_some()
                    || domain::items(&hooks).is_some_and(|hooks| {
                        hooks
                            .iter()
                            .any(|hook| domain::is_text(hook.get("event"), "post-coder"))
                    })
                {
                    pending.push("trusted post-coder hook execution and containment".to_owned());
                }
            }
            match e2e::selected_commands(&boundary.document().acceptance_checks) {
                Ok(commands)
                    if domain::items(&commands).is_some_and(|commands| !commands.is_empty()) =>
                {
                    pending.push("read-only end-to-end CLI verification".to_owned())
                }
                Ok(_) => {}
                Err(error) => unavailable.push(Text::from(error.message.as_str())),
            }
        }
        for required in &pending {
            unavailable.push(Text::from(format!("Required {required} belongs to a deferred executor; no successful gate was assumed.").as_str()));
        }
        let after_validation = checkpoints::validation(boundary).await?;
        let after = evidence::capture(boundary.root(), None, projection)?;
        let after_audit = boundary.audit()?;
        if !current.same_source(&after) || audit != after_audit || validation != after_validation {
            reasons.push(Text::from("Source or Git identity changed during backend verification; current work remains unverified."));
            self.workspace.refresh(boundary, projection)?;
            current = after;
            audit = after_audit;
            validation = after_validation;
        }
        let run = match crate::planning::lifecycle::response_run(
            self.config.as_value(),
            self.last_response.as_ref(),
            boundary.assignment(),
        ) {
            Ok(run) => run,
            Err(error) => {
                reason(
                    &mut reasons,
                    format!("AI-Run metadata failed: {}", error.message),
                );
                None
            }
        };
        if self.mode == Mode::Configured && run.is_none() {
            reason(
                &mut reasons,
                "Response-backed AI-Run metadata is unavailable.",
            );
        }
        let mut status = if self.mode == Mode::Stub {
            Status::Stub
        } else if !reasons.is_empty() {
            Status::Fail
        } else if !unavailable.is_empty() {
            Status::Unavailable
        } else {
            Status::Pass
        };
        reasons.extend(unavailable);
        let mut report = Value::object(vec![
            ("pass", Value::Boolean(status == Status::Pass)),
            ("status", Value::string(status.as_str())),
            ("reasons", values(&reasons)),
            ("files", strings(&files)),
            ("model", Value::string(&model)),
            ("turns", Value::Number(self.responses.len() as f64)),
            ("snapshot", current.as_value()),
            (
                "acceptanceSourceIdentity",
                current.source().map_or(Value::Null, Value::string),
            ),
            (
                "acceptanceProjectionUnavailable",
                Value::Boolean(current.projection_failure().is_some()),
            ),
            ("pending", strings(&pending)),
        ]);
        let mut result = Value::object(vec![
            (
                "mode",
                Value::string(if self.mode == Mode::Stub {
                    "stub"
                } else {
                    "llm"
                }),
            ),
            ("model", Value::string(&model)),
            ("turns", Value::Number(self.responses.len() as f64)),
            ("summary", Value::String(summary)),
            ("scopeFiles", strings(boundary.scope_files())),
            ("repairFiles", strings(boundary.repair_files())),
            ("shadow", shadow),
            (
                "checklist",
                required(self.workspace.checklist(), "items")?.clone(),
            ),
            (
                "testsSkipped",
                Value::Boolean(docs || tests_mode == "none" && !native),
            ),
        ]);
        if let Some(tests) = boundary.last_tests() {
            let mut value = tests.result().clone();
            if let Some(classification) = &self.classification {
                domain::put(
                    &mut value,
                    "full_suite_exit_code",
                    required(classification.as_value(), "original_exit")?.clone(),
                )?;
                for key in [
                    "preexisting_files",
                    "transient_files",
                    "unclassified_files",
                    "regression_files",
                ] {
                    domain::put(
                        &mut value,
                        key,
                        required(classification.as_value(), key)?.clone(),
                    )?;
                }
                domain::put(
                    &mut result,
                    "classification",
                    classification.as_value().clone(),
                )?;
            }
            domain::put(&mut result, "tests", value)?;
        }
        if let Some(report) = red_green {
            domain::put(&mut result, "redGreen", report.as_value().clone())?;
        }
        if let Some(report) = hook_report {
            domain::put(&mut result, "hooks", report.as_value().clone())?;
        }
        if let Some(report) = end_to_end {
            domain::put(&mut result, "endToEnd", report.as_value().clone())?;
        }
        if let Some(full) = full_verification {
            if let Some(mut tests) = result.get("tests").cloned() {
                domain::put(
                    &mut tests,
                    "full_suite_exit_code",
                    required(&full, "latest_full_exit")?.clone(),
                )?;
                domain::put(
                    &mut tests,
                    "full_suite_clean",
                    required(&full, "clean_full_suite")?.clone(),
                )?;
                domain::put(&mut result, "tests", tests)?;
            }
            domain::put(&mut result, "fullVerification", full)?;
        }
        if let Some(self_review) = &self.self_review {
            domain::put(&mut result, "selfReview", self_review.as_value().clone())?;
        }
        for (name, value) in &self.annotations {
            domain::put(&mut result, name, value.clone())?;
        }
        let mut body = crate::execution::result::render(
            boundary.root(),
            &result,
            &report,
            run.as_ref(),
            boundary.redaction(),
        )?;
        let mut independent = None;
        if let Some(selection) = self
            .backend_approval
            .as_ref()
            .map(|approval| &approval.independent)
            .filter(|selection| !matches!(selection, docker::Selection::None))
        {
            if status == Status::Pass && self.mode == Mode::Configured {
                let approved_files = match selection {
                    docker::Selection::Docker(Some(approval)) => Some(&approval.files),
                    _ => None,
                };
                let complete = approved_files.is_none_or(|approved| {
                    let mut approved = approved.clone();
                    approved.sort();
                    approved == files
                });
                let verified = if complete {
                    body.push_str("\nIndependent verification consumes these frozen RESULT bytes; its final outcome is carried by the separate sealed independent receipt, not recursively embedded in this subject.\n");
                    let candidate = body.to_utf8()?;
                    docker::run_candidate(boundary, selection, Some(candidate.as_bytes())).await
                } else {
                    Err(denied(
                        "Independent host plan does not cover the complete current changed product set",
                    ))
                };
                match verified {
                    Ok(verified) => {
                        if !verified.passed() {
                            status = if domain::is_text(verified.as_value().get("status"), "fail") {
                                Status::Fail
                            } else {
                                Status::Unavailable
                            };
                            append_backend_reasons(verified.as_value(), "reasons", &mut reasons)?;
                            if reasons.is_empty() {
                                reasons.push(Text::from(
                                    "Selected independent Docker verification did not pass",
                                ));
                            }
                        }
                        domain::put(&mut result, "independent", verified.as_value().clone())?;
                        domain::put(&mut report, "independent", verified.as_value().clone())?;
                        independent = Some(verified);
                    }
                    Err(error) => {
                        let mut deferred = Vec::new();
                        backend_issue(error, &mut reasons, &mut deferred)?;
                        status = if deferred.is_empty() {
                            Status::Fail
                        } else {
                            Status::Unavailable
                        };
                        reasons.extend(deferred);
                    }
                }
                domain::put(&mut report, "status", Value::string(status.as_str()))?;
                domain::put(&mut report, "pass", Value::Boolean(status == Status::Pass))?;
                domain::put(&mut report, "reasons", values(&reasons))?;
                if status != Status::Pass {
                    body = crate::execution::result::render(
                        boundary.root(),
                        &result,
                        &report,
                        run.as_ref(),
                        boundary.redaction(),
                    )?;
                }
            } else {
                domain::put(
                    &mut result,
                    "independent",
                    Value::object(vec![
                        ("status", Value::string("blocked")),
                        (
                            "reasons",
                            Value::Array(vec![Value::string(
                                "Prior native gates did not pass; independent execution was not started",
                            )]),
                        ),
                    ]),
                )?;
            }
        }
        let capsule = if self.workspace.source().is_some() {
            Some(self.workspace.export(
                boundary,
                if status == Status::Pass {
                    "verified"
                } else {
                    "failed"
                },
                projection,
            )?)
        } else {
            None
        };
        self.check(boundary)?;
        if validation != checkpoints::validation(boundary).await?
            || !current.same_source(&evidence::capture(boundary.root(), None, projection)?)
            || audit != boundary.audit()?
        {
            return Err(denied(
                "Worktree changed during final gate composition; no late success was accepted",
            ));
        }
        self.check(boundary)?;
        Ok(Finalized {
            authority: self.authority.clone(),
            root: self.root,
            writer: self.writer.clone(),
            snapshot: current,
            audit,
            status,
            report,
            result,
            run,
            body,
            capsule,
            validation: Some(validation),
            independent,
        })
    }

    pub(crate) fn terminal_without_success(
        &mut self,
        boundary: &tools::Boundary,
        status: Status,
        error: Error,
        projection: &evidence::Projection,
    ) -> Result<Finalized, Error> {
        let model = self
            .last_response
            .as_ref()
            .map(|response| response.model.to_utf8())
            .transpose()?
            .unwrap_or_else(|| {
                if self.mode == Mode::Stub {
                    "builtin-stub"
                } else {
                    "unavailable"
                }
                .to_owned()
            });
        let safe = crate::planning::memory::redact(
            &Text::from(error.message.as_str()),
            boundary.redaction(),
        );
        let report = Value::object(vec![
            ("pass", Value::Boolean(false)),
            ("status", Value::string(status.as_str())),
            ("reasons", values(&[safe])),
            ("files", strings(&boundary.written_files())),
            ("model", Value::string(&model)),
            ("turns", Value::Number(self.responses.len() as f64)),
        ]);
        let mut result = Value::object(vec![
            (
                "mode",
                Value::string(if self.mode == Mode::Stub {
                    "stub"
                } else {
                    "llm"
                }),
            ),
            ("model", Value::string(&model)),
            ("turns", Value::Number(self.responses.len() as f64)),
            (
                "summary",
                Value::String(self.summary.clone().unwrap_or_else(|| {
                    Text::from(if status == Status::Cancelled {
                        "Execution cancelled; current work remains unverified."
                    } else {
                        "Coder stopped before verified completion; the retained failure and observations remain authoritative."
                    })
                })),
            ),
            ("cancelled", Value::Boolean(status == Status::Cancelled)),
            ("unverifiedObservation", Value::Boolean(true)),
            ("scopeFiles", strings(boundary.scope_files())),
            ("repairFiles", strings(boundary.repair_files())),
            ("writtenFiles", strings(&boundary.written_files())),
        ]);
        if let Some(response) = &self.last_response {
            domain::put(&mut result, "response", response.as_value())?;
            domain::put(
                &mut result,
                "usage",
                response.usage.map_or(Ok(Value::Null), |usage| {
                    crate::json::parse(&Text::from(usage.json().as_str()))
                })?,
            )?;
        }
        if let Some(tests) = boundary.last_tests() {
            domain::put(&mut result, "tests", tests.result().clone())?;
        }
        for (name, value) in &self.annotations {
            domain::put(&mut result, name, value.clone())?;
        }
        domain::put(
            &mut result,
            "error",
            Value::object(vec![
                ("name", Value::string(error.name)),
                ("code", error.code.map_or(Value::Null, Value::string)),
                (
                    "message",
                    Value::String(crate::planning::memory::redact(
                        &Text::from(error.message.as_str()),
                        boundary.redaction(),
                    )),
                ),
            ]),
        )?;
        if let Some(record) = boundary
            .full_verification()
            .filter(|record| record.requires_resolution())
        {
            domain::put(
                &mut result,
                "fullVerification",
                full_verification_value(record, false, status.as_str(), false),
            )?;
        }
        let run = crate::planning::lifecycle::response_run(
            self.config.as_value(),
            self.last_response.as_ref(),
            boundary.assignment(),
        );
        let run = match run {
            Ok(run) => run,
            Err(metadata) => {
                domain::put(
                    &mut result,
                    "metadataError",
                    Value::string(&metadata.message),
                )?;
                None
            }
        };
        let capsule = if self.workspace.source().is_some() && status != Status::Cancelled {
            match self
                .workspace
                .refresh(boundary, projection)
                .and_then(|()| self.workspace.export(boundary, "failed", projection))
            {
                Ok(capsule) => Some(capsule),
                Err(cause) => {
                    domain::put(
                        &mut result,
                        "acceptanceUnavailable",
                        Value::String(crate::planning::memory::redact(
                            &Text::from(cause.message.as_str()),
                            boundary.redaction(),
                        )),
                    )?;
                    None
                }
            }
        } else {
            None
        };
        let body = crate::execution::result::render(
            boundary.root(),
            &result,
            &report,
            run.as_ref(),
            boundary.redaction(),
        )?;
        Ok(Finalized {
            authority: self.authority.clone(),
            root: self.root,
            writer: self.writer.clone(),
            snapshot: self.baseline.clone(),
            audit: self.baseline_audit.clone(),
            status,
            report,
            result,
            run,
            body,
            capsule,
            validation: None,
            independent: None,
        })
    }
}

impl Finalized {
    pub(crate) fn continuation_snapshot(
        &self,
    ) -> Result<
        (
            evidence::Snapshot,
            std::collections::BTreeMap<String, String>,
            Value,
        ),
        Error,
    > {
        if self.status != Status::Pass {
            return Err(denied(
                "Only a private passing coder receipt has this continuation snapshot",
            ));
        }
        Ok((
            self.snapshot.clone(),
            self.audit.clone(),
            self.validation.clone().ok_or_else(|| {
                denied("Passing coder continuation has no original Git validation")
            })?,
        ))
    }

    pub(crate) async fn require_current(
        &self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<(), Error> {
        let _scope = boundary.span(super::timing::Label::ResultCurrent);
        boundary.check()?;
        if self.authority != boundary.authority_digest()
            || !std::sync::Arc::ptr_eq(&self.writer, &boundary.writer_identity())
            || self.root != crate::paths::file_information(boundary.root())?.identity
            || self.validation != Some(checkpoints::validation(boundary).await?)
            || !self.snapshot.same_source(&{
                let scope = boundary.span(super::timing::Label::SourceCapture);
                let captured = evidence::capture(boundary.root(), None, projection);
                super::timing::finish(scope, captured.as_ref().err());
                captured?
            })
            || self.audit != boundary.audit()?
        {
            return Err(denied(
                "Coder receipt is stale for the current writer, handoff, source or Git state",
            ));
        }
        let path = boundary.root().join("RESULT.md");
        let result = crate::state::secure::read(&path, MAX_FILE_BYTES)?
            .ok_or_else(|| denied("The observed coder RESULT is missing"))?;
        if result.bytes != self.body.to_utf8()?.as_bytes()
            || crate::paths::file_information(&path)?.links != 1
        {
            return Err(denied(
                "The actual RESULT no longer matches the private coder receipt",
            ));
        }
        if let Some(independent) = &self.independent {
            independent.require_current(boundary).await?;
        }
        boundary.check()
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn report(&self) -> &Value {
        &self.report
    }
    pub fn result(&self) -> &Value {
        &self.result
    }
    pub fn run(&self) -> Option<&RunRecord> {
        self.run.as_ref()
    }
    pub fn body(&self) -> &Text {
        &self.body
    }
    pub fn capsule(&self) -> Option<&Value> {
        self.capsule.as_ref()
    }
    pub fn independent(&self) -> Option<&docker::Report> {
        self.independent.as_ref()
    }
    pub async fn write(
        &self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<PathBuf, Error> {
        self.write_owned(boundary, projection).await?;
        Ok(boundary.root().join("RESULT.md"))
    }

    pub(crate) async fn write_owned(
        &self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<crate::state::secure::Snapshot, Error> {
        if self.authority != boundary.authority_digest()
            || !std::sync::Arc::ptr_eq(&self.writer, &boundary.writer_identity())
            || self.root != crate::paths::file_information(boundary.root())?.identity
        {
            return Err(denied(
                "Sealed finalization belongs to another authoritative attempt",
            ));
        }
        if self.validation.is_none() && self.status != Status::Pass {
            boundary.check_diagnostic_authority()?;
        } else {
            boundary.check()?;
            if self.status == Status::Pass {
                if let Some(independent) = &self.independent {
                    independent.require_staged_current(boundary).await?;
                }
            }
            if self.authority != boundary.authority_digest()
                || !self.snapshot.same_source(&evidence::capture(
                    boundary.root(),
                    None,
                    projection,
                )?)
                || self.audit != boundary.audit()?
                || self.validation != Some(checkpoints::validation(boundary).await?)
            {
                return Err(denied(
                    "Finalization changed after its sealed verdict; result publication is refused",
                ));
            }
        }
        let path = boundary.root().join("RESULT.md");
        crate::paths::ensure_local_path(&path, boundary.root())?;
        let body = self.body.to_utf8()?;
        let identity = crate::state::secure::write_exclusive(&path, body.as_bytes())?;
        let owned = crate::state::secure::Snapshot {
            identity,
            bytes: body.into_bytes(),
        };
        let checked = async {
            boundary.check_diagnostic_authority()?;
            if self.status == Status::Pass {
                boundary.check()?;
                if let Some(independent) = &self.independent {independent.require_current(boundary).await?;}
                if self.validation != Some(checkpoints::validation(boundary).await?)
                    || !self.snapshot.same_source(&evidence::capture(boundary.root(), None, projection)?)
                    || self.audit != boundary.audit()?
                {
                    return Err(denied("Worktree changed during RESULT write; no late successful finalization is accepted"));
                }
            }
            if !crate::state::secure::unchanged(&path, &owned, MAX_FILE_BYTES)? {
                return Err(denied("RESULT identity or bytes changed before write completion"));
            }
            if self.status != Status::Cancelled { boundary.check()?; }
            Ok(())
        }.await;
        if let Err(error) = checked {
            return match crate::state::secure::remove_matching(&path, &owned, MAX_FILE_BYTES) {
                Ok(true) => Err(error),
                Ok(false) => Err(Error::new(
                    "OwnedArtifactCleanupError",
                    "Failed RESULT write no longer owns its artifact; preserve it as unverified",
                )
                .coded("ROSTER_ARTIFACT_CLEANUP")
                .caused_by(error)),
                Err(cleanup) => Err(cleanup.caused_by(error)),
            };
        }
        Ok(owned)
    }
}
