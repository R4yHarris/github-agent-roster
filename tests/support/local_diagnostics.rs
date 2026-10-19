use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use github_agent_roster::Error;
use github_agent_roster::delivery;
use github_agent_roster::json::Value;
use github_agent_roster::llm::control::Observer;
use github_agent_roster::llm::http::{Request, Response, Transport};
use github_agent_roster::state::secure;
use github_agent_roster::text::Text;

#[derive(Clone)]
pub struct Diagnostics {
    state: Arc<Mutex<State>>,
    failed: Arc<AtomicBool>,
}

struct State {
    started: Instant,
    root: PathBuf,
    home: PathBuf,
    output: PathBuf,
    expected: Option<usize>,
    accepted: usize,
    completed: usize,
    remaining: Option<usize>,
    attempts: Option<usize>,
    transport_returns: Option<usize>,
    server_started_ms: Option<u32>,
    server_phase: &'static str,
    client_state: &'static str,
    client_started_ms: Option<u32>,
    client_returned_ms: Option<u32>,
    client_result: Value,
    first_transport_error: Value,
    last_request: Value,
    first_server_error: Value,
    events: VecDeque<Value>,
    timings: Option<github_agent_roster::execution::timing::Timings>,
}

fn number(value: usize) -> Value {
    u32::try_from(value).map_or(Value::Null, |value| Value::Number(f64::from(value)))
}

fn elapsed(started: Instant) -> Value {
    u32::try_from(started.elapsed().as_millis())
        .map_or(Value::Null, |value| Value::Number(f64::from(value)))
}

fn error_value(error: &Error) -> Value {
    Value::object(vec![
        ("name", Value::string(error.name)),
        ("code", error.code.map_or(Value::Null, Value::string)),
    ])
}

fn items(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(values)) => values,
        _ => &[],
    }
}

fn event_value(event: &Value) -> Value {
    let mut output = Value::object(Vec::new());
    for (field, allowed) in [
        (
            "type",
            &[
                "http",
                "waiting",
                "seat",
                "seat-tool",
                "seat-stopped",
                "review-repair",
                "rescope",
                "perspective-escalation",
                "model",
                "usage",
                "scope-expansion",
            ][..],
        ),
        ("phase", &["start", "ok", "error"][..]),
        ("seat", &["coder", "reviewer", "planner"][..]),
        (
            "status",
            &[
                "running",
                "finalizing",
                "pass",
                "fail",
                "unavailable",
                "cancelled",
                "ok",
                "denied",
            ][..],
        ),
        (
            "name",
            &[
                "read_file",
                "write_file",
                "edit_file",
                "delete_file",
                "run_test",
                "run_command",
                "update_checklist",
                "git_diff",
                "git_status",
                "search_text",
                "list_dir",
                "glob_files",
            ][..],
        ),
    ] {
        if let Some(value) = event.get(field).filter(|value| {
            allowed
                .iter()
                .any(|allowed| *value == &Value::string(allowed))
        }) {
            super::put(&mut output, field, value.clone());
        }
    }
    output
}

fn request_value(request: &Value) -> Value {
    let messages = items(request.get("messages"));
    let has_tools = !items(request.get("tools")).is_empty();
    let system = messages
        .first()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_string);
    let needle = "Perform a separate Original Ask audit"
        .encode_utf16()
        .collect::<Vec<_>>();
    let audit = system.is_some_and(|system| {
        system
            .0
            .iter()
            .take(8192)
            .copied()
            .collect::<Vec<_>>()
            .windows(needle.len())
            .any(|part| part == needle)
    });
    Value::object(vec![
        (
            "phase",
            Value::string(if has_tools {
                "coder-tool-chat"
            } else if audit {
                "original-ask-audit"
            } else {
                "tool-free-review-or-author"
            }),
        ),
        ("messages", number(messages.len())),
        ("tools", number(items(request.get("tools")).len())),
        (
            "stream",
            Value::Boolean(request.get("stream") == Some(&Value::Boolean(true))),
        ),
        (
            "last_role",
            messages
                .last()
                .and_then(|message| message.get("role"))
                .filter(|role| {
                    ["system", "assistant", "user", "tool"]
                        .iter()
                        .any(|allowed| *role == &Value::string(allowed))
                })
                .cloned()
                .unwrap_or(Value::Null),
        ),
    ])
}

impl Diagnostics {
    pub fn new(root: &Path, home: &Path, container: &Path) -> Self {
        let name = container.file_name().map_or_else(
            || "owned-local".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        Self {
            state: Arc::new(Mutex::new(State {
                started: Instant::now(),
                root: root.to_path_buf(),
                home: home.to_path_buf(),
                output: Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(".roster")
                    .join("outer-local-milestone")
                    .join("c5bc-diagnostics")
                    .join(format!("{name}.jsonl")),
                expected: None,
                accepted: 0,
                completed: 0,
                remaining: None,
                attempts: None,
                transport_returns: None,
                server_started_ms: None,
                server_phase: "not-attached",
                client_state: "not-started",
                client_started_ms: None,
                client_returned_ms: None,
                client_result: Value::Null,
                first_transport_error: Value::Null,
                first_server_error: Value::Null,
                last_request: Value::Null,
                events: VecDeque::new(),
                timings: None,
            })),
            failed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn update(&self, operation: impl FnOnce(&mut State)) {
        match self.state.lock() {
            Ok(mut state) => operation(&mut state),
            Err(_) => {
                eprintln!("Owned diagnostics state unavailable; no runtime result was changed")
            }
        }
    }

    pub fn root(&self, root: &Path) {
        self.update(|state| state.root = root.to_path_buf());
    }

    pub fn server_start(&self, started: Instant, expected: usize) {
        self.update(|state| {
            state.server_started_ms =
                u32::try_from(started.saturating_duration_since(state.started).as_millis()).ok();
            state.expected = Some(expected);
            state.remaining = Some(expected);
            state.server_phase = "accept";
        });
    }

    pub fn server_phase(&self, phase: &'static str) {
        self.update(|state| state.server_phase = phase);
    }

    pub fn server_error(&self, error: &Error) {
        self.update(|state| {
            if state.first_server_error == Value::Null {
                let mut value = error_value(error);
                super::put(&mut value, "at_ms", elapsed(state.started));
                state.first_server_error = value;
            }
        });
    }

    pub fn output(&self) -> Option<PathBuf> {
        self.state.lock().ok().map(|state| state.output.clone())
    }

    pub fn accepted(&self, request: &Value) {
        self.update(|state| {
            state.accepted += 1;
            state.last_request = request_value(request);
            state.server_phase = "decoded";
        });
    }

    pub fn popped(&self, remaining: usize) {
        self.update(|state| {
            state.remaining = Some(remaining);
            state.server_phase = "responding";
        });
    }

    pub fn responded(&self) {
        self.update(|state| {
            state.completed += 1;
            state.server_phase = "accept";
        });
    }

    pub fn client_started(&self, root: &Path) {
        self.update(|state| {
            state.root = root.to_path_buf();
            state.client_state = "pending";
            state.client_started_ms = u32::try_from(state.started.elapsed().as_millis()).ok();
        });
    }

    pub fn counting_transport(&self) {
        self.update(|state| {
            state.attempts = Some(0);
            state.transport_returns = Some(0);
        });
    }

    pub fn native_timings(&self, timings: github_agent_roster::execution::timing::Timings) {
        self.update(|state| state.timings = Some(timings));
    }

    pub fn client_returned(&self, result: &Result<delivery::Outcome, Error>) {
        self.update(|state| {
            state.client_state = "returned";
            state.client_returned_ms = u32::try_from(state.started.elapsed().as_millis()).ok();
            state.client_result = match result {
                Err(error) => Value::object(vec![("outer_error", error_value(error))]),
                Ok(outcome) => Value::object(vec![
                    ("status", Value::string(outcome.status().as_str())),
                    (
                        "first_error",
                        outcome.failure.as_ref().map_or(Value::Null, error_value),
                    ),
                    (
                        "handoff",
                        outcome
                            .coder_handoff
                            .map_or(Value::Null, |handoff| Value::string(handoff.as_str())),
                    ),
                    (
                        "review_repairs",
                        Value::Number(f64::from(outcome.orchestration.counts.review_repairs)),
                    ),
                    (
                        "rescopes",
                        Value::Number(f64::from(outcome.orchestration.counts.rescopes)),
                    ),
                    (
                        "perspectives",
                        Value::Number(f64::from(outcome.orchestration.counts.perspectives)),
                    ),
                    (
                        "seat_attempts",
                        number(outcome.orchestration.attempts.len()),
                    ),
                ]),
            };
        });
        if result.is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
        self.persist("client-return");
    }

    pub fn cli_returned(&self, result: &Result<std::process::Output, std::io::Error>) {
        self.update(|state| {
            state.client_state = "returned";
            state.client_returned_ms = u32::try_from(state.started.elapsed().as_millis()).ok();
            state.client_result = match result {
                Err(_) => Value::object(vec![("outer_error", Value::string("cli-io-error"))]),
                Ok(output) => {
                    let stdout = std::str::from_utf8(&output.stdout).ok();
                    let mode = [
                        "verified",
                        "unverified",
                        "planning-only",
                        "planning-escalation",
                        "stub",
                        "review-skipped",
                    ]
                    .into_iter()
                    .find(|mode| {
                        stdout.is_some_and(|stdout| {
                            stdout.lines().any(|line| line == format!("Mode: {mode}"))
                        })
                    });
                    Value::object(vec![
                        ("cli_success", Value::Boolean(output.status.success())),
                        ("mode", mode.map_or(Value::Null, Value::string)),
                    ])
                }
            };
        });
        if result.is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
        self.persist("client-return");
    }

    pub fn event(&self, event: &Value) {
        self.update(|state| {
            let mut value = event_value(event);
            super::put(&mut value, "at_ms", elapsed(state.started));
            state.events.push_back(value);
            if state.events.len() > 32 {
                state.events.pop_front();
            }
        });
    }

    pub fn retain(&self, reason: &'static str) {
        self.failed.store(true, Ordering::SeqCst);
        self.persist(reason);
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    fn native_log(root: &Path) -> Value {
        let directory = root.join(".roster").join("runs");
        if secure::check_components(&directory).is_err() {
            return Value::Null;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => return Value::Null,
        };
        let mut last = Value::Null;
        let mut observer_error = Value::Null;
        for entry in entries.take(4).flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "log") {
                continue;
            }
            let snapshot = match secure::read(&path, 2 * 1024 * 1024) {
                Ok(Some(snapshot)) => snapshot,
                _ => continue,
            };
            let source = match std::str::from_utf8(&snapshot.bytes) {
                Ok(source) => source,
                Err(_) => continue,
            };
            for line in source.lines() {
                let record = match github_agent_roster::json::parse(&Text::from(line)) {
                    Ok(record) => record,
                    Err(_) => continue,
                };
                let mut safe = Value::object(Vec::new());
                for field in ["sequence", "event", "seat", "status"] {
                    if let Some(value) = record.get(field) {
                        super::put(&mut safe, field, value.clone());
                    }
                }
                if record.get("event") == Some(&Value::string("observer-error"))
                    && observer_error == Value::Null
                {
                    super::put(
                        &mut safe,
                        "details",
                        record.get("details").cloned().unwrap_or(Value::Null),
                    );
                    observer_error = safe.clone();
                }
                if record.get("event") == Some(&Value::string("invocation-end")) {
                    super::put(
                        &mut safe,
                        "transport",
                        record.get("details").cloned().unwrap_or(Value::Null),
                    );
                }
                last = safe;
            }
        }
        Value::object(vec![
            ("last", last),
            ("first_observer_error", observer_error),
        ])
    }

    pub fn persist(&self, reason: &'static str) {
        let snapshot = match self.state.lock() {
            Ok(state) => {
                let time = |value: Option<u32>| {
                    value.map_or(Value::Null, |value| Value::Number(f64::from(value)))
                };
                let value = Value::object(vec![
                    ("reason", Value::string(reason)),
                    ("elapsed_ms", elapsed(state.started)),
                    ("root", Value::string(&state.root.display().to_string())),
                    ("home", Value::string(&state.home.display().to_string())),
                    ("expected", state.expected.map_or(Value::Null, number)),
                    ("accepted", number(state.accepted)),
                    ("responses_completed", number(state.completed)),
                    (
                        "remaining_packets",
                        state.remaining.map_or(Value::Null, number),
                    ),
                    (
                        "transport_attempts",
                        state.attempts.map_or(Value::Null, number),
                    ),
                    (
                        "transport_header_returns",
                        state.transport_returns.map_or(Value::Null, number),
                    ),
                    ("server_started_ms", time(state.server_started_ms)),
                    ("first_transport_error", state.first_transport_error.clone()),
                    ("server_phase", Value::string(state.server_phase)),
                    ("first_server_error", state.first_server_error.clone()),
                    ("client_state", Value::string(state.client_state)),
                    ("client_started_ms", time(state.client_started_ms)),
                    ("client_returned_ms", time(state.client_returned_ms)),
                    ("client_result", state.client_result.clone()),
                    ("last_decoded_request", state.last_request.clone()),
                    (
                        "last_events",
                        Value::Array(state.events.iter().cloned().collect()),
                    ),
                ]);
                (
                    state.output.clone(),
                    state.root.clone(),
                    value,
                    state.timings.clone(),
                    state.started,
                )
            }
            Err(_) => {
                eprintln!("Owned diagnostic snapshot unavailable");
                return;
            }
        };
        let (path, root, mut value, timings, epoch) = snapshot;
        let sampled_start = elapsed(epoch);
        let native_timings = timings.map_or(Value::Null, |timings| timings.snapshot());
        let sampled_end = elapsed(epoch);
        super::put(&mut value, "native_timings", native_timings);
        super::put(&mut value, "timings_capture_start_ms", sampled_start);
        super::put(&mut value, "timings_capture_end_ms", sampled_end);
        super::put(&mut value, "native_log", Self::native_log(&root));
        let bytes = format!("{}\n", value.json()).into_bytes();
        if bytes.len() > 64 * 1024 {
            eprintln!("Owned diagnostic snapshot exceeded its bound");
            return;
        }
        let saved = (|| -> Result<(), Error> {
            secure::private_directory(
                path.parent()
                    .ok_or_else(|| Error::type_error("Owned diagnostic parent"))?,
                true,
            )?;
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(super::io)?;
            file.write_all(&bytes).map_err(super::io)?;
            file.sync_data().map_err(super::io)
        })();
        if saved.is_err() {
            eprintln!("Owned diagnostic persistence failed; no runtime result was changed");
        }
    }
}

pub struct Counting<T> {
    pub inner: T,
    pub diagnostics: Diagnostics,
}

impl<T: Transport> Transport for Counting<T> {
    type Body = T::Body;

    async fn send(
        &mut self,
        request: &Request,
    ) -> Result<Response<T::Body>, github_agent_roster::llm::Failure> {
        self.diagnostics.update(|state| {
            state.attempts = state.attempts.and_then(|count| count.checked_add(1));
        });
        let result = self.inner.send(request).await;
        self.diagnostics.update(|state| {
            state.transport_returns = state
                .transport_returns
                .and_then(|count| count.checked_add(1));
            if let Err(failure) = &result {
                if state.first_transport_error == Value::Null {
                    let mut error = error_value(&failure.error);
                    super::put(&mut error, "at_ms", elapsed(state.started));
                    state.first_transport_error = error;
                }
            }
        });
        result
    }
}

pub struct Observed<'a, O> {
    pub inner: &'a mut O,
    pub diagnostics: Diagnostics,
}

impl<O: Observer> Observer for Observed<'_, O> {
    fn enabled(&self) -> bool {
        self.inner.enabled()
    }

    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        self.diagnostics.event(&event);
        self.inner.emit(event).await
    }
}
