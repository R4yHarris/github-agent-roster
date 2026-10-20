use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::Instant;

use super::*;
use crate::llm::http::{Request, Response};

pub const MAX_REVIEW_REPAIRS: u32 = 2;
pub const MAX_RESCOPES: u32 = 2;
pub const MAX_PERSPECTIVES: u32 = 2;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub review_repairs: u32,
    pub rescopes: u32,
    pub perspectives: u32,
}

#[derive(Debug, Clone)]
pub struct SeatAttempt {
    pub seat: &'static str,
    pub index: u32,
    pub status: &'static str,
    pub run: Option<RunRecord>,
    pub response: Option<LastResponse>,
    pub self_review_response: Option<Value>,
    pub ask_audit: Option<Value>,
    pub handoff: Option<seats::Handoff>,
    pub failure: Option<Error>,
    pub continuation_failure: Option<Error>,
}

#[derive(Debug, Default)]
pub struct History {
    pub counts: Counts,
    pub attempts: Vec<SeatAttempt>,
    pub transitions: Vec<Value>,
    pub archives: Vec<PathBuf>,
}

pub(super) struct Budget {
    limit: Option<u32>,
    sent: Cell<u32>,
    started: Instant,
    last_started_ms: Cell<Option<u32>>,
    last_finished_ms: Cell<Option<u32>>,
    first_transport_error: RefCell<Option<Value>>,
    pending: Cell<bool>,
    timings: execution::timing::Timings,
}

pub(super) struct AuthorityFiles(Vec<(PathBuf, secure::Snapshot)>);

impl AuthorityFiles {
    pub(super) fn capture(boundary: &tools::Boundary) -> Result<Self, Error> {
        boundary.check()?;
        let files = ["TASK.md", "RECIPE.yml", "ESTIMATE.md"]
            .into_iter()
            .map(|name| {
                let path = boundary.root().join(name);
                let snapshot = secure::read(&path, 65_536)?.ok_or_else(|| {
                    execution::denied("Immutable local handoff artifact is missing")
                })?;
                Ok((path, snapshot))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let authority = Self(files);
        authority.check()?;
        Ok(authority)
    }

    pub(super) fn check(&self) -> Result<(), Error> {
        for (path, snapshot) in &self.0 {
            if !secure::unchanged(path, snapshot, 65_536)?
                || paths::file_information(path)?.links != 1
            {
                return Err(execution::denied(
                    "TASK, RECIPE or ESTIMATE identity/bytes changed; outer continuation is revoked",
                ));
            }
        }
        Ok(())
    }
}

impl Budget {
    pub(super) fn new(
        config: &config::Config,
        timings: execution::timing::Timings,
    ) -> Result<Self, Error> {
        let limit = config
            .as_value()
            .get("llm")
            .and_then(|llm| llm.get("max_requests"))
            .map(|value| {
                domain::safe_integer(Some(value))
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| (1..=10_000).contains(value))
                    .ok_or_else(|| {
                        Error::type_error("Invocation max_requests must be from 1 to 10000")
                    })
            })
            .transpose()?;
        Ok(Self {
            limit,
            sent: Cell::new(0),
            started: Instant::now(),
            last_started_ms: Cell::new(None),
            last_finished_ms: Cell::new(None),
            first_transport_error: RefCell::new(None),
            pending: Cell::new(false),
            timings,
        })
    }

    pub(super) fn timings(&self) -> execution::timing::Timings {
        self.timings.clone()
    }

    fn elapsed_ms(&self) -> Option<u32> {
        u32::try_from(self.started.elapsed().as_millis()).ok()
    }

    pub(super) fn snapshot(&self) -> Value {
        let milliseconds =
            |value: Option<u32>| value.map_or(Value::Null, |value| Value::Number(f64::from(value)));
        Value::object(vec![
            (
                "actual_transport_attempts",
                Value::Number(f64::from(self.sent.get())),
            ),
            ("invocation_elapsed_ms", milliseconds(self.elapsed_ms())),
            (
                "last_send_started_ms",
                milliseconds(self.last_started_ms.get()),
            ),
            (
                "last_send_finished_ms",
                milliseconds(self.last_finished_ms.get()),
            ),
            (
                "last_transport_phase",
                Value::string(if self.pending.get() {
                    "send-pending"
                } else if self.sent.get() == 0 {
                    "not-started"
                } else {
                    "headers-or-error-returned"
                }),
            ),
            (
                "first_transport_error",
                self.first_transport_error
                    .borrow()
                    .clone()
                    .unwrap_or(Value::Null),
            ),
        ])
    }

    pub(super) fn check(&self) -> Result<(), Error> {
        if self.limit.is_some_and(|limit| self.sent.get() >= limit) {
            return Err(Error::new("ModelRequestLimitError", format!(
                "Invocation model HTTP request budget ({}) exhausted; a fresh seat cannot reset it.",
                self.limit.ok_or_else(|| Error::type_error("Invocation request limit disappeared"))?
            )).coded("ROSTER_MODEL_REQUEST_LIMIT"));
        }
        Ok(())
    }
}

pub(super) struct TransportBudget<'a, T> {
    pub transport: &'a mut T,
    pub budget: &'a Budget,
}

impl<T: Transport> Transport for TransportBudget<'_, T> {
    type Body = T::Body;

    async fn send(&mut self, request: &Request) -> Result<Response<T::Body>, crate::llm::Failure> {
        self.budget.check()?;
        self.budget.sent.set(
            self.budget
                .sent
                .get()
                .checked_add(1)
                .ok_or_else(|| Error::type_error("Invocation HTTP attempt counter exhausted"))?,
        );
        self.budget.last_started_ms.set(self.budget.elapsed_ms());
        self.budget.pending.set(true);
        struct PendingGuard<'a>(&'a std::cell::Cell<bool>);
        impl Drop for PendingGuard<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let guard = PendingGuard(&self.budget.pending);
        let response = self.transport.send(request).await;
        drop(guard);
        self.budget.last_finished_ms.set(self.budget.elapsed_ms());
        if let Err(failure) = &response {
            let mut first = self.budget.first_transport_error.borrow_mut();
            if first.is_none() {
                *first = Some(Value::object(vec![
                    ("name", Value::string(failure.error.name)),
                    (
                        "code",
                        failure.error.code.map_or(Value::Null, Value::string),
                    ),
                    (
                        "category",
                        failure
                            .category
                            .map_or(Value::Null, |category| Value::string(category.as_str())),
                    ),
                    (
                        "at_ms",
                        self.budget
                            .elapsed_ms()
                            .map_or(Value::Null, |value| Value::Number(f64::from(value))),
                    ),
                ]));
            }
        }
        response
    }
}

pub(super) fn number(value: usize) -> Result<Value, Error> {
    Ok(Value::Number(f64::from(u32::try_from(value).map_err(
        |_| Error::type_error("Continuation count exceeds its native bound"),
    )?)))
}

pub(super) fn blocked_failure(coder: &coder::CoderResult, blocker: Error) -> Error {
    if let Some(first) = coder.failure() {
        let mut error = first.clone();
        error.message.push_str("\nContinuation blocked: ");
        error.message.push_str(&blocker.message);
        error.cause = Some(Box::new(blocker));
        error
    } else {
        blocker
    }
}

pub(super) fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(|value| Value::string(value)).collect())
}

pub(super) fn safe_line(value: &Text, boundary: &tools::Boundary) -> Text {
    let safe = planning::memory::redact(value, boundary.redaction());
    let mut bounded = Text(
        safe.0
            .into_iter()
            .map(|unit| {
                if matches!(unit, 0..=31 | 127) {
                    32
                } else {
                    unit
                }
            })
            .take(500)
            .collect(),
    );
    if bounded
        .0
        .last()
        .is_some_and(|unit| (0xd800..=0xdbff).contains(unit))
    {
        bounded.0.pop();
    }
    Text(crate::text::trim(&bounded.0).to_vec())
}

pub(super) fn findings(
    review: &reviewer::ReviewResult,
    boundary: &tools::Boundary,
) -> Result<(Vec<Text>, Vec<u64>), Error> {
    let report = review.report();
    let reasons = report
        .get("reasons")
        .and_then(domain::items)
        .ok_or_else(|| Error::type_error("Completed review has no findings array"))?;
    let reasons = reasons
        .iter()
        .map(|reason| {
            reason
                .as_string()
                .map(|reason| safe_line(reason, boundary))
                .filter(|reason| !crate::text::trim(&reason.0).is_empty())
                .ok_or_else(|| Error::type_error("Completed review finding is not concrete text"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if reasons.len() > 16 {
        return Err(Error::type_error(
            "Completed review findings exceed the bounded contract",
        ));
    }
    let checks = report
        .get("checks")
        .and_then(domain::items)
        .ok_or_else(|| Error::type_error("Completed review has no actual check mapping"))?;
    let unmet = checks
        .iter()
        .filter(|check| check.get("met") == Some(&Value::Boolean(false)))
        .map(|check| {
            domain::safe_integer(check.get("id"))
                .ok_or_else(|| Error::type_error("Completed review has an invalid check ID"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((reasons, unmet))
}

pub(super) fn instructions(boundary: &tools::Boundary) -> Result<&'static str, Error> {
    let scope = boundary.scope()?;
    if execution::policy::requires_native(&scope)? {
        Ok(
            "Run matching Cargo checks and the required locked/offline Cargo verification, format and strict Clippy. Node checks cannot verify Rust.",
        )
    } else if execution::policy::docs_only(&scope)? {
        Ok("Read and verify the requested document outcome; do not run an unrelated suite.")
    } else {
        Ok(
            "Run the approved matching checks for this consumer's language and the required final verification.",
        )
    }
}

pub(super) fn continuation(
    heading: &str,
    boundary: &tools::Boundary,
    evidence: &[Text],
    stalled: bool,
) -> Result<Text, Error> {
    let mut text = Text::from(format!(
        "{heading}. Correct work remains in this same worktree ({}). Re-read TASK.md, the Original Ask, current implementation and tests. Keep correct work; never weaken required checks, alternate assertion and implementation to chase a failure, or treat generated RESULT claims as evidence. RESULT.md and REVIEW.md are harness-written and read-only. {}{}",
        boundary.written_files().join(", "),
        if stalled { "The same checks stayed unmet: do not repeat that approach. Re-derive the required behavior and change strategy on this fixed model. " } else { "" },
        instructions(boundary)?
    ).as_str());
    text.push_str("\n\nActual bounded redacted findings:\n");
    let mut findings = Text::default();
    for finding in evidence {
        findings.push_str("- ");
        findings.0.extend(&safe_line(finding, boundary).0);
        findings.0.push(10);
    }
    findings.0.truncate(3000);
    if findings
        .0
        .last()
        .is_some_and(|unit| (0xd800..=0xdbff).contains(unit))
    {
        findings.0.pop();
    }
    text.0.extend(findings.0);
    Ok(text)
}
