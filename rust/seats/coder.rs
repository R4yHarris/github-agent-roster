use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::*;
use crate::execution::{backend, context, continuity, finalization, self_review};
use crate::llm::builtin::Options as ChatOptions;
use crate::planning::estimate;

#[derive(Default)]
pub struct Options<'a> {
    pub approval: Option<backend::Approval>,
    pub projection: evidence::Projection,
    pub capsule: Option<&'a Value>,
    pub prior_wave_files: &'a [String],
    pub initial_baseline: Option<&'a evidence::Snapshot>,
    pub memory: Option<&'a context::Memory>,
    pub prior_feedback: Option<&'a Text>,
    pub continuation: Option<&'a Text>,
    pub recording: Recording<'a>,
}

pub struct CoderResult {
    finalized: finalization::Finalized,
    artifact: Artifact,
    stages: Vec<String>,
    handoff: Option<Handoff>,
    failure: Option<Error>,
    response: Option<LastResponse>,
    context: Option<Artifact>,
    research: Option<Artifact>,
    binding: Option<context::Binding>,
    retry_subject: Option<RetrySubject>,
    policy_stopped: bool,
    continuation_failure: Option<Error>,
}

#[derive(Clone)]
struct RetrySubject {
    snapshot: evidence::Snapshot,
    audit: BTreeMap<String, String>,
    validation: Value,
}

impl RetrySubject {
    fn source(
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
        validation: &Value,
    ) -> Result<Self, Error> {
        boundary.check()?;
        Ok(Self {
            snapshot: evidence::capture(boundary.root(), None, projection)?,
            audit: boundary.audit()?,
            validation: validation.clone(),
        })
    }

    fn require_source(
        &self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<(), Error> {
        boundary.check()?;
        let captured = {
            let scope = boundary.span(execution::timing::Label::SourceCapture);
            let result = evidence::capture(boundary.root(), None, projection);
            execution::timing::finish(scope, result.as_ref().err());
            result?
        };
        if !self.snapshot.same_source(&captured) || self.audit != boundary.audit()? {
            return Err(execution::denied(
                "Source changed outside the completed coder operation; no continuation is authorized",
            ));
        }
        boundary.check()
    }

    fn require_current<'a>(
        &'a self,
        boundary: &'a tools::Boundary,
        projection: &'a evidence::Projection,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + 'a>> {
        Box::pin(async move {
            self.require_source(boundary, projection)?;
            if &self.validation != boundary.git_validation()? {
                return Err(execution::denied(
                    "Source or Git state changed outside the completed coder context; no continuation is authorized",
                ));
            }
            boundary.require_git().await?;
            self.require_source(boundary, projection)
        })
    }
}

struct Invocation<'a, 'task> {
    repo_root: &'a Path,
    config: &'a crate::config::Config,
    options: &'a Options<'task>,
    configured: bool,
}

pub(super) fn failure_fingerprint(output: &str) -> Result<String, Error> {
    let mut normalized = output.replace("\r\n", "\n");
    for (pattern, replacement) in [
        (
            r"(?m)^([ \t]*Finished [^\r\n]* in )[0-9]+(?:\.[0-9]+)?(?:ms|s)[ \t]*$",
            "${1}<elapsed>",
        ),
        (
            r"(?m)^([ \t]*test result: (?:ok|FAILED)\.[^\r\n]*; finished in )[0-9]+(?:\.[0-9]+)?(?:ms|s)[ \t]*$",
            "${1}<elapsed>",
        ),
        (
            r"(?m)^([ \t]*(?:#[ \t]*)?duration_ms[ \t]*:?[ \t]*)[0-9]+(?:\.[0-9]+)?[ \t]*$",
            "${1}<elapsed>",
        ),
        (
            r"(?m)^([ \t]*[✔✖▶✓✗][^\r\n]*)\([0-9]+(?:\.[0-9]+)?\s*(?:ms|s)\)[ \t]*$",
            "${1}(<elapsed>)",
        ),
    ] {
        normalized = execution::regex(pattern)?
            .replace_all(&normalized, replacement)
            .into_owned();
    }
    Ok(execution::regex(r"\s+")?
        .replace_all(&normalized, " ")
        .trim()
        .to_owned())
}

pub(super) fn observed_failure_identity(
    stdout: &str,
    stderr: &str,
    native: bool,
) -> Result<String, Error> {
    let complete_cargo_failure = native
        && execution::regex(
            r"(?m)^test result: FAILED\.[^\r\n]*; finished in [0-9]+(?:\.[0-9]+)?s[ \t]*\r?$",
        )?
        .is_match(stdout);
    let stderr = if complete_cargo_failure {
        let activity = execution::regex(r"(?m)^[ \t]*(?:Checking|Compiling) [A-Za-z0-9_-]+ v[0-9][A-Za-z0-9.+-]*(?: \([^\r\n]+\))?[ \t]*\r?$")?
            .replace_all(stderr, "").into_owned();
        execution::regex(r"(?m)^[ \t]*Blocking waiting for file lock on (?:package cache|build directory|artifact directory)[ \t]*\r?$")?
            .replace_all(&activity, "").into_owned()
    } else {
        stderr.to_owned()
    };
    failure_fingerprint(&format!("{stdout}\n{stderr}"))
}

impl CoderResult {
    pub fn status(&self) -> finalization::Status {
        self.finalized.status()
    }
    pub fn result(&self) -> &Value {
        self.finalized.result()
    }
    pub fn excellence(&self) -> &Value {
        self.finalized.report()
    }
    pub fn run(&self) -> Option<&crate::run::RunRecord> {
        self.finalized.run()
    }
    pub fn stages(&self) -> &[String] {
        &self.stages
    }
    pub fn handoff(&self) -> Option<Handoff> {
        self.handoff
    }
    pub fn failure(&self) -> Option<&Error> {
        self.failure.as_ref()
    }
    pub fn response(&self) -> Option<&LastResponse> {
        self.response.as_ref()
    }
    pub fn capsule(&self) -> Option<&Value> {
        self.finalized.capsule()
    }
    pub(crate) fn perspective_reason(&self) -> Option<&'static str> {
        if self.policy_stopped
            || self.continuation_failure.is_some()
            || self.retry_subject.is_none()
        {
            return None;
        }
        self.handoff.and_then(Handoff::perspective_reason)
    }
    pub(crate) fn continuation_failure(&self) -> Option<&Error> {
        self.continuation_failure.as_ref()
    }

    pub async fn require_current(
        &self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<(), Error> {
        let _scope = boundary.span(execution::timing::Label::CoderCurrent);
        boundary.require_git().await?;
        self.artifact.require_current(boundary, false)?;
        self.finalized.require_current(boundary, projection).await
    }

    pub fn result_path(&self, boundary: &tools::Boundary) -> Result<PathBuf, Error> {
        self.artifact.require_current(boundary, true)?;
        Ok(result_path(boundary, "RESULT.md"))
    }

    pub(super) fn finalized(&self) -> &finalization::Finalized {
        &self.finalized
    }

    pub(crate) fn require_continuable<'a>(
        &'a self,
        boundary: &'a tools::Boundary,
        projection: &'a evidence::Projection,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + 'a>> {
        Box::pin(async move {
            if let Some(error) = &self.continuation_failure {
                return Err(error.clone());
            }
            self.artifact.require_current(boundary, false)?;
            self.binding
                .as_ref()
                .ok_or_else(|| execution::denied("Stopped coder has no complete current context"))?
                .require_current(boundary)?;
            if self.status() == finalization::Status::Pass {
                self.require_current(boundary, projection).await
            } else {
                self.retry_subject
                    .as_ref()
                    .ok_or_else(|| {
                        execution::denied("Stopped coder has no private continuation subject")
                    })?
                    .require_current(boundary, projection)
                    .await
            }
        })
    }

    pub(crate) fn archive_continuation<'a>(
        &'a self,
        boundary: &'a tools::Boundary,
        review: Option<&'a reviewer::ReviewResult>,
        projection: &'a evidence::Projection,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<PathBuf, Error>> + 'a>> {
        Box::pin(async move {
            let check = || -> Result<(), Error> {
                self.artifact.require_current(boundary, false)?;
                for (name, owned) in [
                    ("CONTEXT.md", self.context.as_ref()),
                    ("RESEARCH.md", self.research.as_ref()),
                ] {
                    if let Some(owned) = owned {
                        owned.require_current(boundary, false)?;
                    } else if secure::read(&boundary.root().join(name), execution::MAX_FILE_BYTES)?
                        .is_some()
                    {
                        return Err(execution::denied(
                            "Continuation cannot archive a foreign generated artifact",
                        ));
                    }
                }
                if review.is_none()
                    && secure::read(
                        &boundary.root().join("REVIEW.md"),
                        execution::MAX_FILE_BYTES,
                    )?
                    .is_some()
                {
                    return Err(execution::denied(
                        "Continuation cannot adopt a foreign REVIEW",
                    ));
                }
                Ok(())
            };
            check()?;
            let scope = boundary.span(execution::timing::Label::ArchivePrepare);
            let plan = archive::prepare(boundary, &archive::PRESERVE_AUTHORITY).await;
            execution::timing::finish(scope, plan.as_ref().err());
            let plan = plan?;
            check()?;
            self.binding
                .as_ref()
                .ok_or_else(|| execution::denied("Continuation has no complete coder context"))?
                .require_current(boundary)?;
            let authority_scope = boundary.span(execution::timing::Label::ArchiveAuthority);
            if let Some(review) = review {
                review
                    .require_completed_failure(boundary, self, projection)
                    .await?;
            } else {
                self.require_continuable(boundary, projection).await?;
            }
            execution::timing::finish(authority_scope, None);
            let mut subject = if let Some(subject) = &self.retry_subject {
                subject.clone()
            } else {
                let (snapshot, audit, validation) = self.finalized.continuation_snapshot()?;
                RetrySubject {
                    snapshot,
                    audit,
                    validation,
                }
            };
            let scope = boundary.span(execution::timing::Label::ArchiveExecute);
            let archived = archive::execute(plan, boundary).await;
            execution::timing::finish(scope, archived.as_ref().err());
            let archived = archived?.ok_or_else(|| {
                execution::denied("Continuation did not archive its owned seat artifacts")
            })?;
            subject.snapshot = subject.snapshot.after_owned_context_archive();
            let scope = boundary.span(execution::timing::Label::ArchivePostcheck);
            let result = subject.require_current(boundary, projection).await;
            execution::timing::finish(scope, result.as_ref().err());
            result?;
            Ok(archived.directory().to_path_buf())
        })
    }
}

struct Progress {
    stages: Vec<String>,
    repairs: usize,
    repair_budget: usize,
    fingerprints: BTreeSet<String>,
    repeated: usize,
    fail_count: Option<u64>,
    corrections: BTreeMap<&'static str, usize>,
    denials: BTreeMap<String, usize>,
    scope_blocked: Vec<String>,
    no_write_turns: usize,
    all_met: bool,
    gate_failures: Vec<Value>,
    test_failures: Vec<Value>,
    context: Option<Artifact>,
    research: Option<Artifact>,
    binding: Option<context::Binding>,
    validation: Option<Value>,
    stopped_correction: Option<&'static str>,
}

impl Progress {
    fn new(difficulty: u8, bounded: bool) -> Self {
        Self {
            stages: Vec::new(),
            repairs: 0,
            repair_budget: if bounded {
                match difficulty {
                    0..=2 => 1,
                    3 => 2,
                    _ => 4,
                }
            } else {
                4
            },
            fingerprints: BTreeSet::new(),
            repeated: 0,
            fail_count: None,
            corrections: BTreeMap::new(),
            denials: BTreeMap::new(),
            scope_blocked: Vec::new(),
            no_write_turns: 0,
            all_met: false,
            gate_failures: Vec::new(),
            test_failures: Vec::new(),
            context: None,
            research: None,
            binding: None,
            validation: None,
            stopped_correction: None,
        }
    }

    fn annotate(&self, attempt: &mut finalization::Attempt, handoff: Option<Handoff>) {
        attempt.annotate("stages", strings(&self.stages));
        attempt.annotate("testRepairs", Value::Number(self.repairs as f64));
        attempt.annotate("testRepairBudget", Value::Number(self.repair_budget as f64));
        attempt.annotate("scopeBlocked", strings(&self.scope_blocked));
        attempt.annotate(
            "handoff",
            handoff.map_or(Value::Null, |handoff| Value::string(handoff.as_str())),
        );
        attempt.annotate("gateFailures", Value::Array(self.gate_failures.clone()));
        attempt.annotate("testFailures", Value::Array(self.test_failures.clone()));
        attempt.annotate(
            "corrections",
            Value::Object(
                self.corrections
                    .iter()
                    .map(|(name, count)| (Text::from(*name), Value::Number(*count as f64)))
                    .collect(),
            ),
        );
    }

    fn correction(&mut self, kind: &'static str, cap: usize) -> Result<(), Error> {
        let count = self.corrections.entry(kind).or_default();
        if *count >= cap {
            self.stopped_correction = Some(kind);
            return Err(Handoff::NoProgress.error(format!(
                "Bounded {kind} correction did not establish current passing evidence"
            )));
        }
        *count += 1;
        Ok(())
    }

    fn repair(
        &mut self,
        tests: &Value,
        response: Option<&LastResponse>,
        config: &crate::config::Config,
    ) -> Result<Text, Error> {
        let exit = domain::safe_integer(tests.get("exit_code")).ok_or_else(|| {
            Error::type_error("Actual test result needs a nonnegative integer exit")
        })?;
        if exit == 0 {
            return Err(Error::type_error("Passing tests cannot consume a repair"));
        }
        let stdout = tests
            .get("stdout")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_default();
        let stderr = tests
            .get("stderr")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_default();
        let output = format!("{stdout}\n{stderr}");
        let fingerprint = observed_failure_identity(
            &stdout,
            &stderr,
            tests.get("native") == Some(&Value::Boolean(true)),
        )?;
        let take_utf16_prefix = |units: &[u16], limit: usize| -> Text {
            let mut end = units.len().min(limit);
            while end > 0 && matches!(units.get(end - 1), Some(0xd800..=0xdbff)) {
                end -= 1;
            }
            Text(units[..end].to_vec())
        };
        let fp_units = fingerprint.encode_utf16().collect::<Vec<_>>();
        let out_text = Text::from(output.as_str());
        self.test_failures.push(Value::object(vec![
            ("exit", Value::Number(exit as f64)),
            (
                "fingerprint",
                Value::string(&crate::identity::sha256_hex(fingerprint.as_bytes())),
            ),
            (
                "identity",
                Value::String(take_utf16_prefix(&fp_units, 8192)),
            ),
            ("identityComplete", Value::Boolean(fp_units.len() <= 8192)),
            (
                "output",
                Value::String(take_utf16_prefix(&out_text.0, 8192)),
            ),
        ]));
        let repeated = !self
            .fingerprints
            .insert(crate::identity::sha256_hex(fingerprint.as_bytes()));
        let fail_count = execution::regex(r"(?m)^\s*(?:ℹ|#)\s*fail\s+(\d+)\s*$")?
            .captures_iter(&output)
            .last()
            .and_then(|captures| captures.get(1))
            .and_then(|value| value.as_str().parse::<u64>().ok());
        let worse = fail_count
            .zip(self.fail_count)
            .is_some_and(|(now, before)| now > before);
        let progress = self.repairs > 0 && !repeated && !worse;
        if repeated {
            self.repeated += 1;
            if self.repeated >= 2 {
                return Err(Handoff::RepeatedFailure.error(format!("Actual tests still fail (exit {exit}); an earlier failure repeated after {} repairs", self.repairs)));
            }
        }
        if fail_count.is_some() {
            self.fail_count = fail_count;
        }
        if self.repairs == self.repair_budget && progress && self.repair_budget < 12 {
            let context = domain::safe_integer(
                config
                    .as_value()
                    .get("llm")
                    .and_then(|llm| llm.get("context_max")),
            );
            let used = response
                .and_then(|response| response.usage)
                .and_then(|usage| usage.prompt_tokens);
            if context
                .zip(used)
                .is_some_and(|(max, used)| max > 0 && used >= max / 2)
            {
                return Err(Handoff::ContextBudget.error("Test repairs progressed but measured context use reached half the declared capacity; a fresh host context is required"));
            }
            self.repair_budget += 1;
        }
        if self.repairs == self.repair_budget {
            return Err(Handoff::RepairBudget.error(format!(
                "Actual tests failed (exit {exit}); bounded repair budget {} is exhausted",
                self.repair_budget
            )));
        }
        self.repairs += 1;
        let mut feedback = Text::from(format!("Actual approved tests failed (exit {exit}). Repair {} of {}. No change is verified. Repair only task-authorized files, do not weaken checks or classify unmeasured failures as preexisting. Change strategy instead of repeating a failed repair.\n\n", self.repairs, self.repair_budget).as_str());
        let tail = Text::from(output.as_str());
        let mut start = tail.0.len().saturating_sub(4096);
        while matches!(tail.0.get(start), Some(0xdc00..=0xdfff)) {
            start += 1;
        }
        feedback.0.extend(tail.0.iter().skip(start));
        Ok(feedback)
    }
}

fn offering(
    boundary: &tools::Boundary,
    attempt: &finalization::Attempt,
    config: &crate::config::Config,
    force_write: bool,
) -> Result<(Value, Vec<String>), Error> {
    let definitions = attempt.workspace().definitions(boundary);
    let definitions = domain::items(&definitions)
        .ok_or_else(|| Error::type_error("Coder tool definitions must be an array"))?;
    let task = boundary.task().to_utf8()?;
    let mut names = Vec::new();
    let mut offered = Vec::new();
    for definition in definitions {
        let name = definition
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Coder tool name is missing"))?
            .to_utf8()?;
        let allowed = preparation::configured_tool(config, &name)
            || name == "edit_file" && preparation::configured_tool(config, "write_file")
            || name == "delete_file"
                && (preparation::configured_tool(config, "write_file")
                    || preparation::configured_tool(config, "edit_file"))
            || name == "git_diff" && preparation::configured_tool(config, "run_command")
            || name == "update_checklist";
        if !allowed || name == "run_test" && !boundary.checks_required()? {
            continue;
        }
        if name == "run_command"
            && !execution::regex(r"\brun_command\b|\bgit (?:status|diff)\b")?.is_match(&task)
        {
            continue;
        }
        names.push(name.clone());
        if force_write
            && [
                "read_file",
                "list_dir",
                "glob_files",
                "search_text",
                "git_status",
                "git_diff",
            ]
            .contains(&name.as_str())
        {
            continue;
        }
        offered.push(definition.clone());
    }
    Ok((Value::Array(offered), names))
}

fn assistant(completion: &Completion, calls: &[execution::coder_calls::Call]) -> Value {
    let mut result = Value::object(vec![
        ("role", Value::string("assistant")),
        (
            "content",
            completion
                .message
                .get("content")
                .cloned()
                .unwrap_or(Value::Null),
        ),
    ]);
    if !calls.is_empty() {
        let tools = calls
            .iter()
            .map(|call| {
                Value::object(vec![
                    ("id", Value::string(&call.id)),
                    ("type", Value::string("function")),
                    (
                        "function",
                        Value::object(vec![
                            ("name", Value::string(&call.name)),
                            ("arguments", Value::string(&call.args.json())),
                        ]),
                    ),
                ])
            })
            .collect();
        if let Value::Object(fields) = &mut result {
            fields.push((Text::from("tool_calls"), Value::Array(tools)));
        }
    }
    result
}

fn tool_result(call: &execution::coder_calls::Call, result: Value) -> Value {
    Value::object(vec![
        ("role", Value::string("tool")),
        ("tool_call_id", Value::string(&call.id)),
        ("name", Value::string(&call.name)),
        ("content", Value::string(&result.json())),
    ])
}

fn test_call(call: &execution::coder_calls::Call) -> bool {
    call.name == "run_test"
        || call.name == "run_command"
            && call.args.get("argv")
                == Some(&Value::Array(vec![
                    Value::string("node"),
                    Value::string("--test"),
                ]))
}

impl<T: Transport, O: Observer> Driver<'_, T, O> {
    pub fn run_coder<'a>(
        &'a mut self,
        boundary: &'a mut tools::Boundary,
        repo_root: &'a Path,
        config: &'a crate::config::Config,
        options: Options<'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<CoderResult, Error>> + 'a>> {
        Box::pin(self.run_coder_owned(boundary, repo_root, config, options))
    }

    async fn run_coder_owned(
        &mut self,
        boundary: &mut tools::Boundary,
        repo_root: &Path,
        config: &crate::config::Config,
        mut options: Options<'_>,
    ) -> Result<CoderResult, Error> {
        let metadata = estimate::read_task_metadata(&Value::String(boundary.task().clone()))?;
        let effective_class = if !boundary.document().files_allowed.is_empty()
            && boundary
                .document()
                .files_allowed
                .iter()
                .all(|f| f.to_lowercase().ends_with(".html"))
        {
            "docs"
        } else {
            metadata.task_class.as_str()
        };
        let config = config.for_task(&Value::object(vec![
            ("kind", Value::string("slice")),
            ("taskClass", Value::string(effective_class)),
            ("difficulty", Value::Number(f64::from(metadata.difficulty))),
        ]))?;
        let configured = config
            .as_value()
            .get("llm")
            .and_then(|llm| llm.get("base_url"))
            .is_some_and(domain::truthy);
        let ownership = if configured {
            match boundary.enable_owned_checks() {
                Ok(()) => boundary.admit_git().await,
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        };
        let mut attempt = finalization::Attempt::begin(
            boundary,
            &config,
            finalization::Inputs {
                mode: if configured {
                    finalization::Mode::Configured
                } else {
                    finalization::Mode::Stub
                },
                capsule_version: continuity::CapsuleVersion::NativeV2,
                capsule: options.capsule,
                prior_wave_files: options.prior_wave_files,
                initial_baseline: options.initial_baseline,
            },
            &options.projection,
        )?;
        let mut progress = Progress::new(
            metadata.difficulty,
            boundary.document().files_allowed.len() == 1,
        );
        let outcome = match ownership {
            Err(error) => Err(error),
            Ok(()) => {
                Box::pin(self.coder_inner(
                    boundary,
                    &mut attempt,
                    &mut progress,
                    Invocation {
                        repo_root,
                        config: &config,
                        options: &options,
                        configured,
                    },
                ))
                .await
            }
        };
        let mut retry_subject = None;
        let outcome = match outcome {
            Err(error)
                if {
                    let handoff = Handoff::from_error(&error);
                    (handoff.perspective_reason().is_some() || handoff == Handoff::ScopeBlocked)
                        && progress.binding.is_some()
                        && progress.validation.is_some()
                        && progress.stopped_correction != Some("secret")
                } =>
            {
                let sealed = async {
                    progress
                        .binding
                        .as_ref()
                        .ok_or_else(|| {
                            execution::denied("Stopped coder did not deliver a complete context")
                        })?
                        .require_current(boundary)?;
                    let subject = RetrySubject::source(
                        boundary,
                        &options.projection,
                        progress.validation.as_ref().ok_or_else(|| {
                            execution::denied("Stopped coder has no original Git validation")
                        })?,
                    )?;
                    subject
                        .require_current(boundary, &options.projection)
                        .await?;
                    Ok::<_, Error>(subject)
                }
                .await;
                match sealed {
                    Ok(subject) => {
                        retry_subject = Some(subject);
                        Err(error)
                    }
                    Err(stale) => Err(stale.caused_by(error)),
                }
            }
            other => other,
        };
        let (mut finalized, mut failure, mut handoff) = match outcome {
            Ok(finalized) => (finalized, None, None),
            Err(error) => {
                let handoff = Handoff::from_error(&error);
                progress.annotate(&mut attempt, Some(handoff));
                attempt.record_failure(error.clone());
                let status = if handoff == Handoff::Cancelled {
                    finalization::Status::Cancelled
                } else if handoff == Handoff::Unavailable {
                    finalization::Status::Unavailable
                } else {
                    finalization::Status::Fail
                };
                let finalized = attempt.terminal_without_success(
                    boundary,
                    status,
                    error.clone(),
                    &options.projection,
                )?;
                (finalized, Some(error), Some(handoff))
            }
        };
        let mut continuation_failure = None;
        if let Some(error) = &failure {
            if let Err(notification) = self
                .notify_stop(boundary, "coder", finalized.status().as_str())
                .await
            {
                continuation_failure = Some(notification.clone());
                attempt.annotate(
                    "notificationError",
                    Value::String(planning::memory::redact(
                        &Text::from(notification.message.as_str()),
                        boundary.redaction(),
                    )),
                );
                finalized = attempt.terminal_without_success(
                    boundary,
                    finalized.status(),
                    error.clone(),
                    &options.projection,
                )?;
            }
        }
        let written = finalized.write_owned(boundary, &options.projection).await?;
        let mut artifact = Artifact::from_written(boundary, "RESULT.md", written)?;
        progress.stages.push("result".to_owned());
        let response = attempt.observed_response().cloned();
        if let Err(error) = record(
            boundary,
            "coder",
            finalized.status().as_str(),
            response.as_ref(),
            &progress.stages,
            failure.as_ref(),
            &mut options.recording,
        )
        .await
        {
            if let Err(notification) = self.notify_stop(boundary, "coder", "failed").await {
                attempt.annotate(
                    "notificationError",
                    Value::String(planning::memory::redact(
                        &Text::from(notification.message.as_str()),
                        boundary.redaction(),
                    )),
                );
            }
            artifact
                .archive_owned(boundary)
                .await
                .map_err(|cleanup| cleanup.caused_by(error.clone()))?;
            failure = failure.or(Some(error.clone()));
            handoff = Some(Handoff::Failed);
            progress.annotate(&mut attempt, handoff);
            finalized = attempt.terminal_without_success(
                boundary,
                finalization::Status::Fail,
                failure
                    .clone()
                    .ok_or_else(|| Error::type_error("Persistence failure disappeared"))?,
                &options.projection,
            )?;
            let written = finalized.write_owned(boundary, &options.projection).await?;
            artifact = Artifact::from_written(boundary, "RESULT.md", written)?;
        }
        let result = CoderResult {
            finalized,
            artifact,
            stages: progress.stages,
            handoff,
            failure,
            response,
            context: progress.context,
            research: progress.research,
            binding: progress.binding,
            retry_subject,
            policy_stopped: progress.stopped_correction == Some("secret"),
            continuation_failure,
        };
        if result.status() == finalization::Status::Pass {
            result
                .require_current(boundary, &options.projection)
                .await?;
        }
        Ok(result)
    }

    async fn coder_inner(
        &mut self,
        boundary: &mut tools::Boundary,
        attempt: &mut finalization::Attempt,
        progress: &mut Progress,
        invocation: Invocation<'_, '_>,
    ) -> Result<finalization::Finalized, Error> {
        let Invocation {
            repo_root,
            config,
            options,
            configured,
        } = invocation;
        self.check(boundary)?;
        if let Some(approval) = &options.approval {
            attempt.approve_backends(boundary, approval.clone())?;
        }
        let context_options = || context::Options {
            memory: options.memory,
            prior_feedback: options.prior_feedback,
            continuation: options.continuation,
            ..context::Options::default()
        };
        let mut pack = attempt
            .context_with(boundary, repo_root, &options.projection, context_options())
            .await
            .map_err(|error| {
                if error.message.contains("seat.context_chars") {
                    Handoff::ContextBudget
                        .error(error.message.clone())
                        .caused_by(error)
                } else if error.message.starts_with("Unknown or empty task skill:") {
                    Handoff::Unavailable
                        .error(error.message.clone())
                        .caused_by(error)
                } else {
                    error
                }
            })?;
        progress
            .stages
            .extend(["principal", "context", "skills"].map(str::to_owned));
        progress.context = Some(Artifact::create(boundary, "CONTEXT.md", &pack.pack, false)?);
        progress.binding = Some(pack.binding());
        if !configured {
            attempt.summary(Text::from("Deterministic stub only: no implementation, product edits or tests were run. Configure llm.base_url for the native coder."));
            progress.annotate(attempt, None);
            return attempt.evaluate(boundary, &options.projection).await;
        }
        preparation::prerequisites(boundary, config, options.approval.as_ref())?;
        preparation::design(boundary).await?;
        progress.stages.push("design".to_owned());
        let research = pack.policy.get("research") == Some(&Value::Boolean(true));
        let prepared = preparation::run(boundary, research).await?;
        progress.all_met = prepared.all_met;
        progress.stages.push("triage".to_owned());
        if research {
            progress.stages.push("research".to_owned());
            progress.research = Some(Artifact::create(
                boundary,
                "RESEARCH.md",
                &prepared.research,
                false,
            )?);
        }
        attempt.annotate("triage", prepared.triage.clone());
        let mut messages = vec![
            message("system", pack.pack.clone()),
            message(
                "user",
                Text::from(
                    "Complete the immutable TASK and Original Ask using only the offered structured functions. Read current files before edits (call read_file with only path, omitting offset/max_lines, to read a complete file in one turn rather than paginating in small chunks). Do not execute prose, widen authority, write harness artifacts, weaken tests or invent verification. In Rust library files (rust/**/*.rs), add unit test assertions inside an existing #[test] function rather than declaring a new #[test] function name, and never include comments in .rs files. In JavaScript test files (tests/**/*.test.mjs), when testing a newly added export in an existing module place `import * as mod from '../src/...'` at module top level and call `mod.<fn>(...)` inside `test(...)` (or use `const mod = await import('../src/...')` inside `test('...', async () => ...)` for a newly created module file) so counterfactual evaluation at base fails inside the named test rather than at top-level module link time. When generating an HTML report (.html), write a complete, well-formatted multi-line HTML5 document (<!DOCTYPE html>) with clear semantic sections, tables, and explicit numeric scores grounded in the inspected repository files. After the final edit, run matching checks and close every checklist item with current observations. Finish with a nonempty concise summary naming each numbered check done or blocked. Host full verification, independent gates and separate self-review run on current bytes. A failed check needs a materially different bounded repair, never a claim of success.",
                ),
            ),
            message(
                "user",
                Text::from(
                    format!(
                        "Read-only pre-coder triage (unknown remains unknown):\n{}",
                        prepared.triage.json()
                    )
                    .as_str(),
                ),
            ),
        ];
        if research {
            messages.push(message("user", prepared.research));
        }
        if attempt.workspace().source().is_some() {
            messages.push(message("user", Text::from("Observed acceptance is enabled. For update_checklist, send items with id, status and a nonempty evidence string citing the current observed file path or actual run_test/command reference. Never supply evidenceRef or observation IDs: the host assigns and validates them. Changed source revokes old closures; gather a current read or passing test after the last edit. A blocked item needs actual current failing test evidence.")));
        }
        if prepared.all_met {
            messages.push(message("user", Text::from("Every deterministically selected pre-coder check already held. Do not change product behavior; pin only missing authorized test evidence, or report that the existing task needs host rescoping. Existing passes alone do not implement an Ask.")));
        }
        let admission = self.admission;
        let mut chat = BuiltinChat::new(
            config.as_value(),
            admission,
            ChatOptions {
                retry_length: true,
                stream: true,
            },
        )?
        .ok_or_else(|| Handoff::Unavailable.error("Configured coder has no model endpoint"))?;
        let turns = domain::safe_integer(
            config
                .as_value()
                .get("seat")
                .and_then(|seat| seat.get("turn_budget")),
        )
        .unwrap_or(1000)
        .min(4096);
        let cited_files = boundary
            .task()
            .to_utf8()
            .ok()
            .and_then(|t| {
                execution::regex(
                    r"\b(?:[A-Za-z0-9_.-]+/)*[A-Za-z0-9_.-]+\.(?:rs|toml|mjs|cjs|js|jsx|ts|tsx|json|yml|yaml|md|css|html|svg|xml|bicep|txt|sh|ps1|py)\b",
                )
                .ok()
                .map(|re| {
                    re.find_iter(&t)
                        .map(|m| m.as_str().to_owned())
                        .collect::<BTreeSet<_>>()
                        .len()
                })
            })
            .unwrap_or(0);
        let exploration_multiplier = if cited_files > 2 { 3 } else { 2 };
        let exploration = ((boundary.document().files_allowed.len() + cited_files)
            * exploration_multiplier)
            .max(8)
            .min(turns as usize);
        let context_budget = budget(config)?;
        boundary.require_git().await?;
        let validation = boundary.git_validation()?.clone();
        progress.validation = Some(validation.clone());
        progress.stages.push("tool_loop".to_owned());
        let mut wrote_in_this_attempt = false;
        for _ in 0..turns {
            self.check(boundary)?;
            pack.require_current(boundary)?;
            progress
                .context
                .as_ref()
                .ok_or_else(|| execution::denied("Current coder context artifact is missing"))?
                .require_current(boundary, false)?;
            let is_single_docs = boundary.document().files_allowed.len() == 1
                && execution::policy::docs_only(&boundary.document().files_allowed)
                    .unwrap_or(false);
            let is_html_single = is_single_docs
                && boundary.document().files_allowed[0]
                    .to_lowercase()
                    .ends_with(".html");
            let force_write = !wrote_in_this_attempt
                && ((is_html_single
                    && (progress.no_write_turns >= 1 || options.continuation.is_some()))
                    || (is_single_docs
                        && (progress.no_write_turns >= 2 || options.continuation.is_some()))
                    || (exploration > 4 && progress.no_write_turns + 2 >= exploration));
            let mut synth_summary: Option<Text> = None;
            let mut synth_completion: Option<Completion> = None;
            if force_write && is_single_docs {
                let target_file = boundary.document().files_allowed[0].clone();
                let is_html_target = target_file.to_lowercase().ends_with(".html");
                let mut collected_evidence = String::new();
                if let Ok(task_str) = boundary.task().to_utf8() {
                    collected_evidence.push_str(&format!("### TASK.md\n{task_str}\n"));
                }
                if let Some(guidance) = options.continuation {
                    if let Ok(g_str) = guidance.to_utf8() {
                        collected_evidence.push_str(&format!(
                            "\n### Mandatory Reviewer Repair Guidance (Address Every Item)\n{g_str}\n"
                        ));
                    }
                }
                if let Ok(existing_target) =
                    std::fs::read_to_string(boundary.root().join(&target_file))
                {
                    if !existing_target.trim().is_empty() {
                        let snip: String = existing_target.chars().take(16_000).collect();
                        collected_evidence.push_str(&format!(
                            "\n### Previous Draft of `{target_file}` (Fix All Reviewer Findings)\n{snip}\n"
                        ));
                    }
                }
                let grounded_candidates = [
                    "AGENTS.md",
                    "README.md",
                    "Directory.Build.props",
                    "Directory.Build.targets",
                    "Directory.Packages.props",
                    "Barberlytics.slnx",
                    "global.json",
                    ".editorconfig",
                    "eng/local/Invoke-PrGate.ps1",
                    "docs/ci-cd.md",
                    "docs/adr/0001-ai-only-zero-comments.md",
                    "docs/backlog-status.md",
                    "docs/Security-Scan.md",
                    "src/Web/barberlytics-web/src/i18n/language.ts",
                    "src/Web/barberlytics-web/src/api/auth.ts",
                    "src/Web/barberlytics-web/src/api/session.ts",
                    "src/Web/barberlytics-web/src/api/claims.ts",
                    "src/Web/barberlytics-web/src/api/http.ts",
                    "src/App/barberlytics-app/src/api/session.ts",
                    "src/App/barberlytics-app/src/api/http.ts",
                ];
                for rel_candidate in grounded_candidates {
                    if collected_evidence.len() >= 48_000 {
                        break;
                    }
                    let full_p = boundary.root().join(rel_candidate);
                    if let Ok(raw_f) = std::fs::read_to_string(&full_p) {
                        let snip: String = if raw_f.len() > 8_000 && rel_candidate.ends_with(".md")
                        {
                            let all_lines: Vec<&str> = raw_f.lines().collect();
                            let total_l = all_lines.len();
                            let mut picked = Vec::new();
                            for (idx, line) in all_lines.iter().enumerate() {
                                if idx < 50
                                    || idx + 30 >= total_l
                                    || line.contains("| Partial |")
                                    || line.contains("| Blocked |")
                                    || line.contains("| Open gap |")
                                    || line.contains("| Duplicate |")
                                    || line.starts_with('#')
                                {
                                    picked.push(*line);
                                }
                            }
                            picked.join("\n")
                        } else if rel_candidate.ends_with(".slnx") {
                            raw_f.chars().take(10_500).collect()
                        } else {
                            raw_f.chars().take(4_500).collect()
                        };
                        collected_evidence.push_str(&format!(
                            "\n### Grounded Repository File (`{rel_candidate}`)\n{snip}\n"
                        ));
                    }
                }
                for msg in &messages {
                    if domain::is_text(msg.get("role"), "tool") {
                        let tname = msg
                            .get("name")
                            .and_then(Value::as_string)
                            .and_then(|t| t.to_utf8().ok())
                            .unwrap_or_else(|| "tool".to_owned());
                        let tcontent = msg
                            .get("content")
                            .and_then(Value::as_string)
                            .and_then(|t| t.to_utf8().ok())
                            .unwrap_or_default();
                        let already_grounded = grounded_candidates
                            .iter()
                            .any(|cand| tcontent.contains(&format!("\"path\":\"{cand}\"")));
                        if !already_grounded
                            && !tcontent.is_empty()
                            && collected_evidence.len() < 54_000
                        {
                            let snip: String = tcontent.chars().take(3000).collect();
                            collected_evidence.push_str(&format!(
                                "\n### Observed Repository Evidence (`{tname}`)\n{snip}\n"
                            ));
                        }
                    }
                }
                let synth_messages = vec![
                    message(
                        "system",
                        Text::from(
                            "You are a Principal Software Architect and Security & Maintainability Auditor. Author the complete deliverable file grounded strictly in the supplied TASK.md requirements and observed repository evidence. Do not invoke any tools, do not emit <tool_call> tags, and do not include any HTML comments (<!-- ... -->) or CSS/JS comments; output only the complete file contents.",
                        ),
                    ),
                    message(
                        "user",
                        Text::from(
                            format!(
                                "{collected_evidence}\n\nUsing the TASK.md acceptance checks, Original Ask, and all repository evidence above, output the complete content for `{target_file}` now. {}Do not emit any tool calls, <tool_call> tags, or HTML comments (<!-- ... -->); output only the file content.",
                                if is_html_target {
                                    "Output a concise, complete, self-contained HTML5 document (roughly 120-160 lines total so it finishes completely without truncation) starting with <!DOCTYPE html> and <html lang=\"en\"> and ending with </html>, using compact inline <style> (zero external assets, fonts, scripts, or network requests) and a navigable <nav> Table of Contents. Explicitly state the review scope (branch issues/full-sweep, clean working tree, static document and code review, no test suite executed) and label any unverified runtime behaviors as unverified / review hypotheses. Render distinct semantic sections for: (1) Repository Structure (reconciled accurately with Barberlytics.slnx: 47 .csproj entries across 3 top-level /src/ projects [Expenses, Integrations, Jobs], 1 Api host [Barberlytics.Api], 2 BuildingBlocks projects, 12 /src/Services/ modules [AdminPortal (2), Auth (4), Booking (4), Catalog (4), Companies (4), Customers (4), Integrations (3), Notifications (4), Expenses (3), POS (4), Subscriptions (4), Team (4)], and 1 Api.UnitTests project; plus explicitly state the 1,212 tracked files split into 414 source files and 399 test files and 4 CI/CD workflows ci.yml, deploy.yml, deploy-ppe.yml, deploy-prod.yml from repository inspection and docs/ci-cd.md), (2) Supported Features & Functionality (citing exact full source paths for Web: src/Web/barberlytics-web/src/i18n/language.ts for SUPPORTED_LANGUAGES and applyDocumentLanguage, src/Web/barberlytics-web/src/api/auth.ts for login/refresh/logout/passwordLogin, src/Web/barberlytics-web/src/api/session.ts for session, src/Web/barberlytics-web/src/api/claims.ts for claims, src/Web/barberlytics-web/src/api/http.ts for request; and for App: src/App/barberlytics-app/src/api/session.ts and src/App/barberlytics-app/src/api/http.ts), (3) Outstanding Items (listing all 6 Partial items #749, #791, #837, #845, #851, #852, both Blocked items #751 and #849, Open gap #758, and Duplicate #739 from docs/backlog-status.md, plus findings from docs/Security-Scan.md), and (4) Quality, Security & Maintainability Evaluations with one headline numeric 0-100 score each for Quality, Security, and Maintainability (and an overall weighted composite computed directly from those three headline scores, e.g. Quality*0.35 + Security*0.35 + Maintainability*0.30) compared against named industry best-practice benchmarks (OWASP ASVS, ISO/IEC 25010, Microsoft .NET Architecture Guidelines) where every score row cites only grounded repository files present in the evidence above (AGENTS.md, README.md, Directory.Build.props, Directory.Build.targets, Directory.Packages.props, Barberlytics.slnx, global.json, .editorconfig, eng/local/Invoke-PrGate.ps1, docs/ci-cd.md, docs/adr/0001-ai-only-zero-comments.md, docs/backlog-status.md, docs/Security-Scan.md). Do NOT claim the HTML report has zero HTML comments; state that the repository enforces the zero-comment policy (copyright header only) via AGENTS.md, docs/adr/0001-ai-only-zero-comments.md, and eng/local/Invoke-PrGate.ps1. Do not emit <!-- ... --> comments yourself; the harness prepends the exact copyright notice header."
                                } else {
                                    ""
                                }
                            )
                            .as_str(),
                        ),
                    ),
                ];
                if let Ok(Some(mut synth_chat)) = BuiltinChat::new(
                    config.as_value(),
                    admission,
                    ChatOptions {
                        retry_length: false,
                        stream: true,
                    },
                ) {
                    if let Ok(synth_comp) = self
                        .complete(
                            boundary,
                            &mut synth_chat,
                            &request(&synth_messages, None, false),
                            context_budget.max(196_608),
                        )
                        .await
                    {
                        let has_tool_calls = synth_comp
                            .message
                            .get("tool_calls")
                            .and_then(domain::items)
                            .is_some_and(|items| !items.is_empty());
                        if !has_tool_calls {
                            if let Some(raw_text) = synth_comp
                                .message
                                .get("content")
                                .and_then(Value::as_string)
                                .and_then(|t| t.to_utf8().ok())
                            {
                                let mut cleaned_doc = raw_text.trim().to_owned();
                                if cleaned_doc.starts_with("```") {
                                    if let Some(first_nl) = cleaned_doc.find('\n') {
                                        cleaned_doc = cleaned_doc[first_nl + 1..].to_owned();
                                        if let Some(last_fence) = cleaned_doc.rfind("```") {
                                            cleaned_doc =
                                                cleaned_doc[..last_fence].trim().to_owned();
                                        }
                                    }
                                }
                                if is_html_target {
                                    let lower_full = cleaned_doc.to_lowercase();
                                    if let Some(doc_idx) = lower_full.find("<!doctype html") {
                                        if doc_idx > 0 {
                                            cleaned_doc = cleaned_doc[doc_idx..].trim().to_owned();
                                        }
                                    }
                                    if let Ok(comment_re) =
                                        regex_lite::Regex::new(r"<!--[^\r\n]*?-->")
                                    {
                                        cleaned_doc =
                                            comment_re.replace_all(&cleaned_doc, "").into_owned();
                                    }
                                    let lower_trim = cleaned_doc.trim_start().to_lowercase();
                                    if !lower_trim.starts_with("<!doctype html")
                                        && !lower_trim.starts_with("<html")
                                    {
                                        cleaned_doc = crate::shell::render_markdown_as_html_report(
                                            &target_file,
                                            &cleaned_doc,
                                            "",
                                        );
                                    } else if !lower_trim.ends_with("</html>") {
                                        cleaned_doc.push_str("\n</body>\n</html>\n");
                                    }
                                    if let Ok(agents_raw) =
                                        std::fs::read_to_string(boundary.root().join("AGENTS.md"))
                                    {
                                        let agents_trim = agents_raw.trim_start();
                                        if agents_trim.starts_with("<!--") {
                                            if let Some(end_idx) = agents_trim.find("-->") {
                                                let header_comment =
                                                    &agents_trim[..end_idx + "-->".len()];
                                                if header_comment
                                                    .to_lowercase()
                                                    .contains("copyright")
                                                {
                                                    if let Some(first_nl) = cleaned_doc.find('\n') {
                                                        let first_line =
                                                            cleaned_doc[..first_nl].trim();
                                                        if first_line
                                                            .to_lowercase()
                                                            .starts_with("<!doctype")
                                                        {
                                                            cleaned_doc = format!(
                                                                "{first_line}\n{header_comment}\n{}",
                                                                cleaned_doc[first_nl + 1..]
                                                                    .trim_start()
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if !cleaned_doc.is_empty() && !cleaned_doc.contains("<tool_call>") {
                                    let _ = attempt.response_offering(boundary, &synth_comp, None);
                                    boundary.require_git().await?;
                                    let write_call = execution::coder_calls::Call {
                                        id: format!("synth_write_{}", progress.no_write_turns + 1),
                                        name: "write_file".to_owned(),
                                        args: Value::object(vec![
                                            ("path", Value::string(&target_file)),
                                            ("content", Value::string(&cleaned_doc)),
                                        ]),
                                    };
                                    attempt.push_pending(write_call.clone());
                                    if let Ok(w_res) = attempt
                                        .execute(boundary, &write_call, &options.projection)
                                        .await
                                    {
                                        wrote_in_this_attempt = true;
                                        self.emit(
                                            boundary,
                                            Value::object(vec![
                                                ("type", Value::string("seat-tool")),
                                                ("seat", Value::string("coder")),
                                                ("name", Value::string("write_file")),
                                                ("path", Value::string(&target_file)),
                                                ("id", Value::string(&write_call.id)),
                                                ("status", Value::string("ok")),
                                            ]),
                                        )
                                        .await?;
                                        self.emit(
                                            boundary,
                                            Value::object(vec![
                                                ("type", Value::string("wrote")),
                                                ("path", Value::string(&target_file)),
                                            ]),
                                        )
                                        .await?;
                                        let read_after = execution::coder_calls::Call {
                                            id: format!(
                                                "synth_read_{}",
                                                progress.no_write_turns + 1
                                            ),
                                            name: "read_file".to_owned(),
                                            args: Value::object(vec![(
                                                "path",
                                                Value::string(&target_file),
                                            )]),
                                        };
                                        attempt.push_pending(read_after.clone());
                                        let _ = attempt
                                            .execute(boundary, &read_after, &options.projection)
                                            .await;
                                        if attempt.workspace().source().is_some()
                                            && boundary.recipe_permits("update_checklist")
                                        {
                                            let check_items =
                                                (1..=boundary.document().acceptance_checks.len())
                                                    .map(|idx| {
                                                        Value::object(vec![
                                                            ("id", Value::Number(idx as f64)),
                                                            ("status", Value::string("done")),
                                                            (
                                                                "evidence",
                                                                Value::string(&format!(
                                                                    "Verified in {target_file}"
                                                                )),
                                                            ),
                                                        ])
                                                    })
                                                    .collect::<Vec<_>>();
                                            let chk_call = execution::coder_calls::Call {
                                                id: format!(
                                                    "synth_chk_{}",
                                                    progress.no_write_turns + 1
                                                ),
                                                name: "update_checklist".to_owned(),
                                                args: Value::object(vec![(
                                                    "items",
                                                    Value::Array(check_items),
                                                )]),
                                            };
                                            attempt.push_pending(chk_call.clone());
                                            let _ = attempt
                                                .execute(boundary, &chk_call, &options.projection)
                                                .await;
                                        }
                                        let done_summary = (1..=boundary
                                            .document()
                                            .acceptance_checks
                                            .len())
                                            .map(|idx| {
                                                format!(
                                                    "Check {idx}: done - Verified in {target_file}"
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        let sum_text = Text::from(done_summary.as_str());
                                        if attempt.workspace().source().is_none() {
                                            let _ = attempt.workspace_close(&sum_text);
                                        }
                                        attempt.summary(sum_text.clone());
                                        synth_summary = Some(sum_text);
                                        messages.push(assistant(
                                            &synth_comp,
                                            std::slice::from_ref(&write_call),
                                        ));
                                        messages.push(tool_result(&write_call, w_res));
                                    }
                                }
                            }
                        }
                        if synth_summary.is_none() {
                            synth_completion = Some(synth_comp);
                        }
                    }
                }
            }
            let (completion, decoded) = if synth_summary.is_some() {
                (
                    Completion {
                        model: Text::from("synth"),
                        message: Value::object(vec![
                            ("role", Value::string("assistant")),
                            (
                                "content",
                                Value::String(synth_summary.clone().unwrap_or_default()),
                            ),
                        ]),
                        finish_reason: Some(Value::string("stop")),
                        usage: None,
                    },
                    execution::coder_calls::Decoded {
                        calls: Vec::new(),
                        correction: None,
                        discarded_placeholders: 0,
                    },
                )
            } else {
                let (definitions, names) = offering(boundary, attempt, config, force_write)?;
                let source = RetrySubject::source(boundary, &options.projection, &validation)?;
                let completion = if let Some(sc) = synth_completion.take() {
                    sc
                } else {
                    match self
                        .complete(
                            boundary,
                            &mut chat,
                            &request(&messages, Some(definitions), false),
                            context_budget,
                        )
                        .await
                    {
                        Ok(completion) => completion,
                        Err(error) => {
                            attempt.failed_response(chat.last_response(), chat.last_usage())?;
                            return Err(error);
                        }
                    }
                };
                if let Err(error) = source.require_source(boundary, &options.projection) {
                    attempt.failed_response(chat.last_response(), chat.last_usage())?;
                    return Err(error);
                }
                if !domain::is_text(completion.message.get("role"), "assistant") {
                    attempt.failed_response(chat.last_response(), chat.last_usage())?;
                    return Err(execution::denied(
                        "Coder response must be an actual assistant message, not a tool, user or system impersonation",
                    ));
                }
                let decoded = attempt.response_offering(boundary, &completion, Some(&names))?;
                if config
                    .as_value()
                    .get("tools")
                    .and_then(|tools| tools.get("run_test"))
                    == Some(&Value::Boolean(false))
                    && decoded.calls.iter().any(test_call)
                {
                    return Err(execution::denied(
                        "run_test is disabled by frozen configuration, including its run_command alias",
                    ));
                }
                if let Some(correction) = decoded.correction {
                    messages.push(message(
                        "assistant",
                        completion
                            .message
                            .get("content")
                            .and_then(Value::as_string)
                            .cloned()
                            .unwrap_or_default(),
                    ));
                    messages.push(message("user", correction));
                    continue;
                }
                messages.push(assistant(&completion, &decoded.calls));
                if decoded.calls.is_empty() {
                    boundary.require_git().await?;
                }
                (completion, decoded)
            };
            if !decoded.calls.is_empty() && synth_summary.is_none() {
                let before = boundary.written_files();
                let mut refresh = false;
                for (index, call) in decoded.calls.iter().enumerate() {
                    if ![
                        "read_file",
                        "list_dir",
                        "glob_files",
                        "search_text",
                        "git_status",
                        "git_diff",
                    ]
                    .contains(&call.name.as_str())
                    {
                        boundary.require_git().await?;
                    }
                    let scope_before = boundary.scope_files().len();
                    let executed = if force_write
                        && [
                            "read_file",
                            "list_dir",
                            "glob_files",
                            "search_text",
                            "git_status",
                            "git_diff",
                        ]
                        .contains(&call.name.as_str())
                    {
                        attempt.reject_pending(call);
                        Ok(Value::object(vec![
                            (
                                "error",
                                Value::string(
                                    "Exploration phase complete: sufficient repository files have been read. Call write_file now to create the allowed file.",
                                ),
                            ),
                            ("executed", Value::Boolean(false)),
                        ]))
                    } else {
                        attempt.execute(boundary, call, &options.projection).await
                    };
                    for (offset, file) in
                        boundary.scope_files().iter().skip(scope_before).enumerate()
                    {
                        self.emit(
                            boundary,
                            Value::object(vec![
                                ("type", Value::string("scope-expansion")),
                                ("path", Value::string(file)),
                                (
                                    "count",
                                    Value::Number(f64::from(
                                        u32::try_from(scope_before + offset + 1).map_err(|_| {
                                            Error::type_error("Scope event count exceeds its bound")
                                        })?,
                                    )),
                                ),
                                (
                                    "limit",
                                    Value::Number(f64::from(
                                        u32::try_from(boundary.scope_limit()).map_err(|_| {
                                            Error::type_error("Scope event limit exceeds its bound")
                                        })?,
                                    )),
                                ),
                            ]),
                        )
                        .await?;
                    }
                    let response = match executed {
                        Ok(value) => value,
                        Err(error) if tools::correctable_content(&error) => {
                            progress.correction("secret", 1)?;
                            Value::object(vec![
                                (
                                    "error",
                                    Value::string(
                                        "Secret-bearing content was refused before writing. Remove the credential and use only an obvious non-credential test sentinel in a real secret test context when the task requires it.",
                                    ),
                                ),
                                ("executed", Value::Boolean(false)),
                            ])
                        }
                        Err(error)
                            if error.name == "ToolUsageError"
                                || error.name == "TypeError"
                                || error.code == Some("ENOENT")
                                || (["read_file", "list_dir", "glob_files", "search_text"]
                                    .contains(&call.name.as_str())
                                    && error.name == "ToolAccessError"
                                    && error.message.contains(
                                        "secrets, Git metadata, policy, workflows, contracts, or debug logs",
                                    )) =>
                        {
                            if error.message.contains("Scope expansion limit")
                                || error
                                    .message
                                    .contains("Aggregate task/repair/expanded scope")
                            {
                                progress.scope_blocked = boundary.scope_blocked().to_vec();
                                return Err(Handoff::ScopeBlocked
                                    .error(error.message.clone())
                                    .caused_by(error));
                            }
                            if error.message.contains("fresh bounded context") {
                                refresh = true;
                            } else {
                                let fingerprint =
                                    format!("{}:{}:{}", call.name, call.args.json(), error.message);
                                let count = progress.denials.entry(fingerprint).or_default();
                                *count += 1;
                                if *count >= 2 {
                                    return Err(Handoff::NoProgress.error("The same invalid tool call repeated without a successful correction").caused_by(error));
                                }
                            }
                            Value::object(vec![
                                ("error", Value::string(&error.message)),
                                ("executed", Value::Boolean(false)),
                            ])
                        }
                        Err(error) => return Err(error),
                    };
                    messages.push(tool_result(call, response.clone()));
                    bounded_request(&request(&messages, None, false), context_budget)?;
                    self.emit(
                        boundary,
                        Value::object(vec![
                            ("type", Value::string("seat-tool")),
                            ("seat", Value::string("coder")),
                            ("name", Value::string(&call.name)),
                            (
                                "path",
                                call.args
                                    .get("path")
                                    .or_else(|| call.args.get("query"))
                                    .cloned()
                                    .unwrap_or(Value::Null),
                            ),
                            ("id", Value::string(&call.id)),
                            (
                                "status",
                                Value::string(
                                    if response.get("executed") == Some(&Value::Boolean(false)) {
                                        "denied"
                                    } else {
                                        "ok"
                                    },
                                ),
                            ),
                        ]),
                    )
                    .await?;
                    if test_call(call)
                        && response
                            .get("exit_code")
                            .is_some_and(|exit| *exit != Value::Number(0.0))
                    {
                        messages.push(message(
                            "user",
                            progress.repair(&response, chat.last_response(), config)?,
                        ));
                    }
                    if refresh {
                        for revoked in decoded.calls.iter().skip(index + 1) {
                            messages.push(tool_result(revoked, Value::object(vec![("executed", Value::Boolean(false)), ("error", Value::string("Current batch revoked for complete native guidance refresh; emit a new structured call"))])));
                        }
                        break;
                    }
                }
                if refresh {
                    pack = attempt
                        .context_with(boundary, repo_root, &options.projection, context_options())
                        .await?;
                    progress
                        .context
                        .as_mut()
                        .ok_or_else(|| {
                            execution::denied("Current coder context artifact is missing")
                        })?
                        .replace_context(boundary, &pack.pack)
                        .await?;
                    progress.binding = Some(pack.binding());
                    if let Some(first) = messages.first_mut() {
                        *first = message("system", pack.pack.clone());
                    }
                    messages.push(message("user", Text::from("Complete refreshed native guidance was delivered. All prior pending calls are revoked. Read it and emit new calls; no prior call was executed by this refresh.")));
                }
                if boundary.written_files() != before {
                    wrote_in_this_attempt = true;
                }
                if boundary.written_files() == before && before.is_empty() {
                    progress.no_write_turns += 1;
                    let is_single_docs = boundary.document().files_allowed.len() == 1
                        && execution::policy::docs_only(&boundary.document().files_allowed)
                            .unwrap_or(false);
                    if is_single_docs && progress.no_write_turns == 2 {
                        messages.push(message(
                            "user",
                            Text::from(
                                "Exploration notice: you have gathered sufficient repository evidence across 2 exploration turns. In your next turn, call write_file to create the allowed report/documentation file now.",
                            ),
                        ));
                    } else if exploration > 8 && progress.no_write_turns + 3 == exploration {
                        messages.push(message(
                            "user",
                            Text::from(
                                "Exploration budget notice: only 3 exploration turns remain before an authorized product write is required. Stop reading further excerpts and call edit_file or write_file on a TASK-allowed file in your next turn.",
                            ),
                        ));
                    }
                }
                if before.is_empty()
                    && boundary.written_files().is_empty()
                    && progress.no_write_turns >= exploration
                {
                    return Err(Handoff::NoProgress.error(
                        "Exploration budget expired without an authorized named product change",
                    ));
                }
                continue;
            }
            let summary = synth_summary
                .or_else(|| {
                    completion
                        .message
                        .get("content")
                        .and_then(Value::as_string)
                        .cloned()
                })
                .unwrap_or_default();
            if crate::text::trim(&summary.0).is_empty() {
                progress.correction("summary", 1)?;
                messages.push(message("user", Text::from("A nonempty final summary of actual changes, checks and blockers is required.")));
                continue;
            }
            if attempt.workspace().source().is_none() {
                attempt.workspace_close(&summary)?;
            }
            if let Err(error) = attempt.workspace().require_closed() {
                progress.correction("checklist", 2)?;
                messages.push(message("user", Text::from(error.message.as_str())));
                continue;
            }
            if !preparation::readme_status(boundary)? {
                progress.correction("readme-status", 1)?;
                messages.push(message("user", Text::from("The explicit README Status task requires a real ## Status section with exactly one nonempty body line. Correct the source through an approved call; no deterministic code fallback is available.")));
                continue;
            }
            if boundary.document().files_allowed.len() == 1 {
                let changed = attempt.changed_since_start(boundary, &options.projection)?;
                let named = &boundary.document().files_allowed;
                let status_exception = execution::policy::readme_only(named)
                    && execution::regex(r"(?i)status\s+section|section.{0,30}status|##\s*status")?
                        .is_match(&boundary.assignment().ask.to_utf8()?)
                    && preparation::readme_status(boundary)?;
                if !status_exception
                    && !changed.iter().any(|file| planning::is_allowed(file, named))
                {
                    progress.correction("named-write", 1)?;
                    messages.push(message("user", Text::from("The bounded task has no actual change to its named product file. Identical/no-op writes and existing green tests do not implement the Ask. Make the authorized requested change or report the blocker.")));
                    continue;
                }
            }
            if progress.all_met {
                let changes = evidence::changes(boundary, None).await?;
                if changes
                    .files()
                    .iter()
                    .any(|file| !execution::substance::is_test_file(file).unwrap_or(false))
                {
                    return Err(Handoff::NoProgress.error("Every selected check held before coding; product changes cannot be justified by preexisting green checks"));
                }
            }
            if boundary.checks_required()? {
                let tests = attempt.verify_final(boundary, &options.projection).await?;
                if tests.get("exit_code") != Some(&Value::Number(0.0)) {
                    messages.push(message(
                        "user",
                        progress.repair(&tests, chat.last_response(), config)?,
                    ));
                    continue;
                }
            }
            let changes = evidence::changes(boundary, None).await?;
            if self_review::needed(changes.files())? && !attempt.has_current_self_review(boundary) {
                let prepared = attempt
                    .self_review_request(boundary, &options.projection)
                    .await?;
                let mut reviewer = BuiltinChat::new(
                    config.as_value(),
                    admission,
                    ChatOptions {
                        retry_length: false,
                        stream: false,
                    },
                )?
                .ok_or_else(|| {
                    Handoff::Unavailable.error("Required self-review has no configured endpoint")
                })?;
                let response = self
                    .complete(boundary, &mut reviewer, &prepared.request(), context_budget)
                    .await;
                attempt.annotate(
                    "selfReviewResponse",
                    reviewer
                        .last_response()
                        .map_or(Value::Null, LastResponse::as_value),
                );
                let response = response?;
                let mut report = prepared
                    .accept(boundary, &response, &options.projection)
                    .await?;
                if let Err(error) = report.require_tool_free() {
                    attempt.annotate("selfReview", report.as_value().clone());
                    return Err(error);
                }
                if progress
                    .corrections
                    .get("self-review")
                    .copied()
                    .unwrap_or(0)
                    >= 1
                {
                    report.mark_repaired_clean()?;
                }
                attempt.attach_self_review(report)?;
                progress.stages.push("self_review".to_owned());
            }
            progress.annotate(attempt, None);
            self.emit(
                boundary,
                Value::object(vec![
                    ("type", Value::string("seat")),
                    ("seat", Value::string("coder")),
                    ("status", Value::string("finalizing")),
                ]),
            )
            .await?;
            boundary.require_git().await?;
            let finalized = attempt.evaluate(boundary, &options.projection).await?;
            boundary.require_git().await?;
            if finalized.status() == finalization::Status::Pass {
                progress.stages.push("finalization".to_owned());
                return Ok(finalized);
            }
            if finalized.status() == finalization::Status::Unavailable {
                let rg_unavail = finalized
                    .result()
                    .get("redGreen")
                    .is_some_and(|rg| domain::is_text(rg.get("status"), "unavailable"));
                if rg_unavail && progress.correction("red-green", 1).is_ok() {
                    let reasons = finalized.report().get("reasons").map_or_else(
                        || "Counterfactual red/green setup failed at base".to_owned(),
                        Value::json,
                    );
                    progress.gate_failures.push(Value::string(&reasons));
                    messages.push(message(
                        "user",
                        Text::from(
                            format!(
                                "Current host finalization failed because counterfactual evaluation at base could not link or run the named test before test(...) executed. One bounded red-green correction is available. In JavaScript tests, use a namespace import (`import * as mod from '../src/...'`) for new exports in existing modules or dynamic `await import('../src/...')` inside `test('...', async () => ...)` for newly created modules, re-run `run_test`, and provide a new summary.\n{reasons}"
                            )
                            .as_str(),
                        ),
                    ));
                    continue;
                }
                return Err(Handoff::Unavailable.error(
                    finalized.report().get("reasons").map_or_else(
                        || "Native finalization capability is unavailable".to_owned(),
                        Value::json,
                    ),
                ));
            }
            let reasons = finalized.report().get("reasons").map_or_else(
                || "Native finalization did not pass".to_owned(),
                Value::json,
            );
            progress.gate_failures.push(Value::string(&reasons));
            let result = finalized.result();
            let kind = if result
                .get("selfReview")
                .is_some_and(|review| domain::is_text(review.get("status"), "findings"))
            {
                "self-review"
            } else if result
                .get("shadow")
                .and_then(|shadow| shadow.get("findings"))
                .and_then(domain::items)
                .is_some_and(|findings| !findings.is_empty())
            {
                "shadow"
            } else if result
                .get("hooks")
                .is_some_and(|hooks| hooks.get("pass") == Some(&Value::Boolean(false)))
            {
                "hook"
            } else if result
                .get("redGreen")
                .and_then(|report| report.get("reasons"))
                .and_then(domain::items)
                .is_some_and(|reasons| !reasons.is_empty())
            {
                "red-green"
            } else if reasons.contains("Secret") || reasons.contains("sentinel") {
                "secret"
            } else if reasons.contains("Test substance") {
                "substance"
            } else {
                "completion"
            };
            progress.correction(kind, 1)?;
            if kind == "self-review" {
                attempt.clear_self_review();
            }
            messages.push(message("user", Text::from(format!("Current host finalization failed. One bounded {kind} correction is available. Correct only authorized source, gather fresh matching tests and provide a new summary. Do not repeat completed edits or treat prose as evidence.\n{reasons}").as_str())));
        }
        Err(Handoff::TurnBudget.error(format!(
            "Native coder turn budget ({turns}) exhausted; current work remains unverified"
        )))
    }
}
