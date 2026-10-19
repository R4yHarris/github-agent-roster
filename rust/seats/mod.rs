pub mod coder;
mod preparation;
pub mod reviewer;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::Error;
use crate::cancel::CancellationToken;
use crate::domain;
use crate::execution::{self, archive, evidence, tools};
use crate::json::Value;
use crate::llm::admission::Admission;
use crate::llm::builtin::BuiltinChat;
use crate::llm::control::Observer;
use crate::llm::http::Transport;
use crate::llm::openai::{CallOptions, Completion, Credentials, LastResponse};
use crate::planning;
use crate::state::{provenance, secure};
use crate::text::Text;

pub struct Driver<'a, T: Transport, O: Observer> {
    pub transport: &'a mut T,
    pub observer: &'a mut O,
    pub credentials: &'a Credentials,
    pub admission: &'a Admission,
    pub signal: Option<&'a CancellationToken>,
}

#[derive(Default)]
pub struct Recording<'a> {
    pub history: Option<&'a mut provenance::api::ProvenanceStore>,
    pub promote_summary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handoff {
    TurnBudget,
    ContextBudget,
    RepairBudget,
    RepeatedFailure,
    NoProgress,
    ScopeBlocked,
    PolicyDenied,
    Unavailable,
    Cancelled,
    Failed,
    Rescope,
}

impl Handoff {
    pub(crate) fn perspective_reason(self) -> Option<&'static str> {
        match self {
            Self::TurnBudget => Some("exhausted its turn budget"),
            Self::ContextBudget => Some("reached its measured context budget"),
            Self::RepairBudget => Some("exhausted its test repair budget"),
            Self::RepeatedFailure => Some("repeated an earlier actual test failure"),
            Self::NoProgress => Some("stopped making progress after bounded corrections"),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TurnBudget => "turn-budget",
            Self::ContextBudget => "context-budget",
            Self::RepairBudget => "repair-budget",
            Self::RepeatedFailure => "repeated-failure",
            Self::NoProgress => "no-progress",
            Self::ScopeBlocked => "scope-blocked",
            Self::PolicyDenied => "policy-denied",
            Self::Unavailable => "unavailable",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Rescope => "rescope-required",
        }
    }

    fn error(self, message: impl Into<String>) -> Error {
        Error::new("SeatHandoff", message).coded(match self {
            Self::TurnBudget => "ROSTER_SEAT_TURN_BUDGET",
            Self::ContextBudget => "ROSTER_SEAT_CONTEXT_BUDGET",
            Self::RepairBudget => "ROSTER_SEAT_REPAIR_BUDGET",
            Self::RepeatedFailure => "ROSTER_SEAT_REPEATED_FAILURE",
            Self::NoProgress => "ROSTER_SEAT_NO_PROGRESS",
            Self::ScopeBlocked => "ROSTER_SEAT_SCOPE_BLOCKED",
            Self::PolicyDenied => "ROSTER_TOOL_DENIED",
            Self::Unavailable => "ROSTER_NATIVE_UNSUPPORTED",
            Self::Cancelled => "ROSTER_CANCELLED",
            Self::Failed => "ROSTER_SEAT_FAILED",
            Self::Rescope => "ROSTER_SEAT_RESCOPE",
        })
    }

    fn from_error(error: &Error) -> Self {
        match error.code {
            Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT") => Self::Cancelled,
            Some("ROSTER_SEAT_TURN_BUDGET") => Self::TurnBudget,
            Some("ROSTER_SEAT_CONTEXT_BUDGET") => Self::ContextBudget,
            Some("ROSTER_SEAT_REPAIR_BUDGET") => Self::RepairBudget,
            Some("ROSTER_SEAT_REPEATED_FAILURE") => Self::RepeatedFailure,
            Some("ROSTER_SEAT_NO_PROGRESS") => Self::NoProgress,
            Some("ROSTER_SEAT_RESCOPE") => Self::Rescope,
            Some("ROSTER_SEAT_SCOPE_BLOCKED") => Self::ScopeBlocked,
            Some("ROSTER_TOOL_DENIED") => Self::PolicyDenied,
            Some("ROSTER_NATIVE_UNSUPPORTED" | "ENOENT") => Self::Unavailable,
            _ => Self::Failed,
        }
    }
}

struct Artifact {
    name: &'static str,
    root: crate::paths::FileIdentity,
    writer: Arc<()>,
    authority: String,
    snapshot: secure::Snapshot,
}

impl Artifact {
    fn from_written(
        boundary: &tools::Boundary,
        name: &'static str,
        snapshot: secure::Snapshot,
    ) -> Result<Self, Error> {
        boundary.check_diagnostic_authority()?;
        let path = boundary.root().join(name);
        crate::paths::ensure_local_path(&path, boundary.root())?;
        if crate::paths::file_information(&path)?.links != 1 {
            return Err(execution::denied(
                "Seat artifact must be a single-link regular file",
            ));
        }
        let artifact = Self {
            name,
            root: crate::paths::file_information(boundary.root())?.identity,
            writer: boundary.writer_identity(),
            authority: boundary.authority_digest(),
            snapshot,
        };
        artifact.require_current(boundary, true)?;
        Ok(artifact)
    }

    fn create(
        boundary: &tools::Boundary,
        name: &'static str,
        body: &Text,
        diagnostic: bool,
    ) -> Result<Self, Error> {
        if diagnostic {
            boundary.check_diagnostic_authority()?;
        } else {
            boundary.check()?;
        }
        let path = boundary.root().join(name);
        crate::paths::ensure_local_path(&path, boundary.root())?;
        let bytes = body.to_utf8()?.into_bytes();
        if bytes.len() as u64 > execution::MAX_FILE_BYTES {
            return Err(Error::type_error(
                "Seat artifact exceeds the native byte bound",
            ));
        }
        let identity = secure::write_exclusive(&path, &bytes)?;
        let artifact = Self {
            name,
            root: crate::paths::file_information(boundary.root())?.identity,
            writer: boundary.writer_identity(),
            authority: boundary.authority_digest(),
            snapshot: secure::Snapshot { identity, bytes },
        };
        let checked = artifact
            .require_current(boundary, diagnostic)
            .and_then(|()| if diagnostic { Ok(()) } else { boundary.check() });
        if let Err(error) = checked {
            return match secure::remove_matching(
                &path,
                &artifact.snapshot,
                execution::MAX_FILE_BYTES,
            ) {
                Ok(true) => Err(error),
                Ok(false) => Err(execution::denied(
                    "Failed artifact creation no longer owns its bytes; preserve human replacement",
                )
                .caused_by(error)),
                Err(cleanup) => Err(cleanup.caused_by(error)),
            };
        }
        Ok(artifact)
    }

    fn require_current(&self, boundary: &tools::Boundary, diagnostic: bool) -> Result<(), Error> {
        if diagnostic {
            boundary.check_diagnostic_authority()?;
        } else {
            boundary.check()?;
        }
        let path = boundary.root().join(self.name);
        if self.authority != boundary.authority_digest()
            || self.root != crate::paths::file_information(boundary.root())?.identity
            || !Arc::ptr_eq(&self.writer, &boundary.writer_identity())
            || !secure::unchanged(&path, &self.snapshot, execution::MAX_FILE_BYTES)?
            || crate::paths::file_information(&path)?.links != 1
        {
            return Err(execution::denied(
                "Owned seat artifact changed; human bytes are preserved and the receipt is revoked",
            ));
        }
        Ok(())
    }

    async fn replace_context(
        &mut self,
        boundary: &tools::Boundary,
        body: &Text,
    ) -> Result<(), Error> {
        self.require_current(boundary, false)?;
        let plan = archive::prepare_owned(boundary, self.name, &self.snapshot).await?;
        self.require_current(boundary, false)?;
        let archived = archive::execute(plan, boundary).await?.ok_or_else(|| {
            execution::denied("Owned context rotation did not archive the current context")
        })?;
        let next = Self::create(boundary, "CONTEXT.md", body, false);
        drop(archived);
        *self = next?;
        Ok(())
    }

    async fn archive_owned(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.require_current(boundary, true)?;
        let plan = archive::prepare_owned(boundary, self.name, &self.snapshot).await?;
        self.require_current(boundary, true)?;
        archive::execute(plan, boundary)
            .await?
            .ok_or_else(|| execution::denied("Owned seat artifact was not archived"))?;
        Ok(())
    }
}

fn message(role: &str, content: Text) -> Value {
    Value::object(vec![
        ("role", Value::string(role)),
        ("content", Value::String(content)),
    ])
}

fn request(messages: &[Value], tools: Option<Value>, structured: bool) -> Value {
    let mut fields = vec![("messages", Value::Array(messages.to_vec()))];
    if let Some(tools) = tools {
        fields.push(("tools", tools));
    }
    if structured {
        fields.push((
            "response_format",
            Value::object(vec![("type", Value::string("json_object"))]),
        ));
    }
    Value::object(fields)
}

fn text_value(value: &Value, message: &str) -> Result<Text, Error> {
    value
        .as_string()
        .cloned()
        .ok_or_else(|| Error::type_error(message))
}

fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(|value| Value::string(value)).collect())
}

fn bounded_request(request: &Value, budget: usize) -> Result<(), Error> {
    let messages = request
        .get("messages")
        .and_then(domain::items)
        .ok_or_else(|| Error::type_error("Seat request requires messages"))?;
    let raw = messages.iter().try_fold(0usize, |sum, message| {
        sum.checked_add(
            message
                .get("content")
                .and_then(Value::as_string)
                .map_or(0, |text| text.0.len()),
        )
        .ok_or_else(|| Handoff::ContextBudget.error("Native transcript size overflow"))
    })?;
    if raw > budget {
        return Err(Handoff::ContextBudget.error("Actual tool results exceed the bounded native seat context; retain edits for a host handoff"));
    }
    let length = crate::llm::baseline::with_engineering_baseline(&Value::Array(messages.to_vec()))?
        .json()
        .encode_utf16()
        .count();
    let tools = request
        .get("tools")
        .map_or(0, |value| value.json().encode_utf16().count());
    if length
        .checked_add(tools)
        .is_none_or(|length| length > budget)
    {
        return Err(Handoff::ContextBudget.error("Required seat evidence and current transcript exceed seat.context_chars; retain edits for a fresh host context"));
    }
    Ok(())
}

fn budget(config: &crate::config::Config) -> Result<usize, Error> {
    usize::try_from(
        domain::safe_integer(
            config
                .as_value()
                .get("seat")
                .and_then(|seat| seat.get("context_chars")),
        )
        .filter(|budget| *budget > 0)
        .ok_or_else(|| Error::type_error("Seat context budget must be positive"))?,
    )
    .map(|budget| budget.min(4 * 1024 * 1024))
    .map_err(|_| Error::type_error("Seat context budget exceeds this platform"))
}

async fn cancellation(token: Option<&CancellationToken>) -> Result<crate::cancel::Reason, Error> {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

impl<T: Transport, O: Observer> Driver<'_, T, O> {
    async fn notify_stop(
        &mut self,
        boundary: &tools::Boundary,
        seat: &str,
        status: &str,
    ) -> Result<(), Error> {
        let result = async {
            boundary.check_diagnostic_authority()?;
            tokio::time::timeout(
                Duration::from_secs(1),
                self.observer.emit(Value::object(vec![
                    ("type", Value::string("seat-stopped")),
                    ("seat", Value::string(seat)),
                    ("status", Value::string(status)),
                ])),
            )
            .await
            .map_err(|_| {
                Error::new(
                    "SeatNotificationTimeout",
                    "Failure notification did not settle within its owned deadline",
                )
                .coded("ROSTER_RUN_LOG")
            })??;
            boundary.check_diagnostic_authority()
        }
        .await;
        result.map_err(|error| crate::state::redaction::error(&error, boundary.redaction()))
    }

    fn check(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        if let Some(signal) = self.signal {
            signal.check()?;
        }
        boundary.check()
    }

    pub(crate) fn observe_continuation<'a>(
        &'a mut self,
        boundary: &'a tools::Boundary,
        event: Value,
        projection: &'a evidence::Projection,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + 'a>> {
        Box::pin(async move {
            let _scope = boundary.span(execution::timing::Label::Transition);
            let source = evidence::capture(boundary.root(), None, projection)?;
            let audit = boundary.audit()?;
            let validation = execution::checkpoints::validation(boundary).await?;
            self.emit(boundary, event).await?;
            if !source.same_source(&evidence::capture(boundary.root(), None, projection)?)
                || audit != boundary.audit()?
                || validation != execution::checkpoints::validation(boundary).await?
            {
                return Err(execution::denied(
                    "Source or Git state changed during continuation; no new coder is authorized",
                ));
            }
            self.check(boundary)
        })
    }

    async fn emit(&mut self, boundary: &tools::Boundary, event: Value) -> Result<(), Error> {
        let result = async {
            self.check(boundary)?;
            let result = tokio::select! {
                biased;
                reason = cancellation(self.signal) => Err(reason?.error()),
                reason = boundary.token().cancelled() => Err(reason?.error()),
                result = tokio::time::timeout(Duration::from_secs(30), self.observer.emit(event)) =>
                    result.map_err(|_| Error::new("SeatObserverTimeout", "Seat observer did not settle within its bounded deadline").coded("ROSTER_RUN_LOG"))?,
            };
            result?;
            self.check(boundary)
        }
        .await;
        result.map_err(|error| crate::state::redaction::error(&error, boundary.redaction()))
    }

    fn complete<'a>(
        &'a mut self,
        boundary: &'a tools::Boundary,
        chat: &'a mut BuiltinChat<'_>,
        request: &'a Value,
        budget: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Completion, Error>> + 'a>> {
        Box::pin(async move {
            self.check(boundary)?;
            bounded_request(request, budget)?;
            let scope = boundary.span(execution::timing::Label::Model);
            let result = chat
                .complete(
                    request,
                    self.transport,
                    self.observer,
                    self.credentials,
                    CallOptions {
                        parent: self.signal,
                        signal: Some(boundary.token()),
                        retry_command: None,
                    },
                )
                .await;
            if let Some(scope) = scope {
                scope.finish_status(result.is_ok());
            }
            self.check(boundary)?;
            let completion = result.map_err(|failure| {
                crate::state::redaction::error(&failure.error, boundary.redaction())
            })?;
            let model = completion.model.to_utf8()?;
            if !crate::run::is_model_id(&model) || model == "builtin-stub" {
                return Err(Error::new("SeatModelUnavailable", "Configured seat response has no valid actual model identity; no declaration or stub can replace it").coded("ROSTER_SEAT_MODEL"));
            }
            Ok(completion)
        })
    }
}

async fn record(
    boundary: &tools::Boundary,
    seat: &str,
    outcome: &str,
    response: Option<&LastResponse>,
    stages: &[String],
    failure: Option<&Error>,
    recording: &mut Recording<'_>,
) -> Result<(), Error> {
    let assignment = boundary.assignment();
    if let Some(history) = recording.history.as_mut() {
        let normal = |path: &Path| -> Result<PathBuf, Error> {
            let absolute = crate::paths::strip_verbatim(crate::paths::absolute(path)?);
            let path = absolute
                .to_str()
                .ok_or_else(|| Error::type_error("History roots must be Unicode"))?;
            Ok(PathBuf::from(if cfg!(windows) {
                path.to_lowercase()
            } else {
                path.to_owned()
            }))
        };
        let store = normal(&history.underlying.root)?;
        let root = normal(boundary.root())?;
        if store.starts_with(&root) || root.starts_with(&store) {
            return Err(execution::denied(
                "Machine history and curated memory must not overlap the task worktree; no repository fallback was used",
            ));
        }
        let identity = crate::identity::resolve_repo_identity(boundary.root(), None).await?;
        if history.identity(None).await? != identity {
            return Err(execution::denied(
                "Requested machine history store belongs to a different repository",
            ));
        }
        let mut payload = Value::object(vec![
            ("seat", Value::object(vec![("name", Value::string(seat))])),
            ("outcome", Value::string(outcome)),
            (
                "evidence",
                Value::Array(stages.iter().map(|stage| Value::string(stage)).collect()),
            ),
        ]);
        if let Some(response) = response {
            domain::put(
                &mut payload,
                "servedModel",
                Value::String(response.model.clone()),
            )?;
            if let Some(usage) = response.usage {
                domain::put(
                    &mut payload,
                    "metrics",
                    Value::object(vec![
                        (
                            "tokens_prompt",
                            usage.prompt_tokens.map_or_else(
                                || Value::string("unknown"),
                                |count| Value::Number(count as f64),
                            ),
                        ),
                        (
                            "tokens_completion",
                            usage.completion_tokens.map_or_else(
                                || Value::string("unknown"),
                                |count| Value::Number(count as f64),
                            ),
                        ),
                    ]),
                )?;
            }
        }
        if let Some(error) = failure {
            let mut causes = Vec::new();
            let mut current = Some(error);
            while let Some(error) = current {
                causes.push(Value::object(vec![
                    ("name", Value::string(error.name)),
                    ("code", error.code.map_or(Value::Null, Value::string)),
                ]));
                if causes.len() == 32 {
                    break;
                }
                current = error.cause.as_deref();
            }
            domain::put(&mut payload, "failure", Value::Array(causes))?;
        }
        let payload = crate::state::redaction::record(&payload, boundary.redaction())?;
        history
            .record_event(
                &Value::object(vec![
                    (
                        "runId",
                        Value::string(&format!("{}-{seat}", assignment.task)),
                    ),
                    ("sessionId", Value::string(&assignment.session)),
                    (
                        "event",
                        Value::string(if outcome == "pass" || outcome == "stub" {
                            "completed"
                        } else {
                            "failure"
                        }),
                    ),
                    ("payload", payload),
                ]),
                crate::clock::now_milliseconds()?,
                None,
            )
            .await?;
        if recording.promote_summary {
            let model = response
                .map(|response| response.model.to_utf8())
                .transpose()?
                .unwrap_or_else(|| "unavailable".to_owned());
            let summary = planning::memory::redact(
                &Text::from(
                    format!(
                        "{seat} task {} finished {outcome}; observed model {model}; stages {}.",
                        assignment.task,
                        stages.join(" -> "),
                    )
                    .as_str(),
                ),
                boundary.redaction(),
            );
            history
                .record_memory(
                    &Value::object(vec![
                        (
                            "runId",
                            Value::string(&format!("{}-{seat}", assignment.task)),
                        ),
                        ("sessionId", Value::string(&assignment.session)),
                        ("memory", Value::String(summary)),
                        (
                            "payload",
                            Value::object(vec![
                                ("seat", Value::string(seat)),
                                ("outcome", Value::string(outcome)),
                            ]),
                        ),
                    ]),
                    None,
                )
                .await?;
        }
    } else if recording.promote_summary {
        return Err(execution::denied(
            "Explicit summary promotion needs a repository-bound machine store; no notebook or raw-history fallback was used",
        ));
    }
    Ok(())
}

fn result_path(boundary: &tools::Boundary, name: &str) -> PathBuf {
    boundary.root().join(name)
}
