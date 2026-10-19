use std::cell::Cell;
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
#[cfg(windows)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::timing::{self, Label, ProcessTrace, Span};
use crate::Error;
use crate::cancel::CancellationToken;
#[cfg(windows)]
use crate::cancel::Reason;
use crate::process::{Captured, Failure, FailureKind, Limits, Output};

const MAX_INPUT_BYTES: usize = 256 * 1024;

pub(crate) struct FiniteInput {
    bytes: Vec<u8>,
    #[cfg(all(test, windows))]
    trace: Option<std::rc::Rc<InputTrace>>,
}

impl TryFrom<Vec<u8>> for FiniteInput {
    type Error = Error;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(Error::type_error(
                "Owned finite input must not exceed 256 KiB",
            ));
        }
        Ok(Self {
            bytes,
            #[cfg(all(test, windows))]
            trace: None,
        })
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum InputState {
    #[default]
    Writing,
    Joining,
    AbortLatched,
    KillingAndJoining,
    Closed {
        delivered: bool,
    },
    Unknown,
}

#[cfg(all(test, windows))]
#[derive(Default)]
struct InputTrace {
    state: Cell<InputState>,
    accepted: Cell<usize>,
    flush_terminal: Cell<bool>,
    flush_error: Cell<bool>,
    idle_original: Cell<bool>,
    disposed: Cell<bool>,
    known_cleanup: Cell<bool>,
}

#[cfg(windows)]
struct JoinedInput {
    terminal_error: Option<io::Error>,
}

#[cfg(windows)]
struct InputController {
    stdin: Option<tokio::process::ChildStdin>,
    input: FiniteInput,
    position: usize,
    state: InputState,
}

#[cfg(windows)]
impl InputController {
    fn new(stdin: tokio::process::ChildStdin, input: FiniteInput) -> Self {
        Self {
            stdin: Some(stdin),
            input,
            position: 0,
            state: InputState::Writing,
        }
    }

    fn transition(&mut self, state: InputState) {
        self.state = state;
        #[cfg(test)]
        if let Some(trace) = &self.input.trace {
            trace.state.set(state);
        }
    }

    fn delivered(&self) -> bool {
        self.state == InputState::Closed { delivered: true }
    }

    fn close_idle(&mut self, delivered: bool) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;

        let stdin = self
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Owned input handle is missing"))?;
        let original = stdin.as_raw_handle();
        let handle = stdin.into_owned_handle()?;
        let idle_original = handle.as_raw_handle() == original;
        drop(handle);
        #[cfg(test)]
        if let Some(trace) = &self.input.trace {
            trace.idle_original.set(idle_original);
            trace.disposed.set(idle_original);
        }
        if !idle_original {
            self.transition(InputState::Unknown);
            return Err(io::Error::other(
                "Owned input worker still retains its original handle",
            ));
        }
        self.transition(InputState::Closed { delivered });
        Ok(())
    }

    async fn complete(&mut self) -> io::Result<()> {
        if !matches!(self.state, InputState::Writing | InputState::Joining) {
            return Err(io::Error::other(
                "Aborted owned input cannot resume writing",
            ));
        }
        while self.position < self.input.bytes.len() {
            let remaining = self
                .input
                .bytes
                .get(self.position..)
                .ok_or_else(|| io::Error::other("Owned input cursor is invalid"))?;
            let written = self
                .stdin
                .as_mut()
                .ok_or_else(|| io::Error::other("Owned input pipe is missing"))?
                .write(remaining)
                .await?;
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Owned input write made no progress",
                ));
            }
            self.position = self
                .position
                .checked_add(written)
                .filter(|position| *position <= self.input.bytes.len())
                .ok_or_else(|| io::Error::other("Owned input cursor exceeds its bound"))?;
            #[cfg(test)]
            if let Some(trace) = &self.input.trace {
                trace.accepted.set(self.position);
            }
        }
        self.transition(InputState::Joining);
        let flushed = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("Owned input pipe is missing"))?
            .flush()
            .await;
        #[cfg(test)]
        if let Some(trace) = &self.input.trace {
            trace.flush_terminal.set(true);
            trace
                .flush_error
                .set(trace.flush_error.get() || flushed.is_err());
        }
        flushed?;
        self.close_idle(true)
    }

    fn latch_abort(&mut self) {
        if !matches!(self.state, InputState::Closed { .. } | InputState::Unknown) {
            self.transition(InputState::AbortLatched);
        }
    }

    async fn join_aborted(&mut self) -> Result<JoinedInput, Error> {
        if matches!(self.state, InputState::Closed { .. }) {
            return Ok(JoinedInput {
                terminal_error: None,
            });
        }
        if self.state != InputState::AbortLatched {
            return Err(Error::new(
                "OwnedInputCleanupError",
                "Owned input abort is not latched for terminal join",
            ));
        }
        self.transition(InputState::KillingAndJoining);
        let terminal = self
            .stdin
            .as_mut()
            .ok_or_else(|| Error::new("OwnedInputCleanupError", "Owned input pipe is missing"))?
            .flush()
            .await;
        #[cfg(test)]
        if let Some(trace) = &self.input.trace {
            trace.flush_terminal.set(true);
            trace
                .flush_error
                .set(trace.flush_error.get() || terminal.is_err());
        }
        self.close_idle(false).map_err(|_| {
            Error::new(
                "OwnedInputCleanupError",
                "Owned input original handle is not disposed after terminal join",
            )
        })?;
        Ok(JoinedInput {
            terminal_error: terminal.err(),
        })
    }
}

#[cfg(windows)]
impl Drop for InputController {
    fn drop(&mut self) {
        if !matches!(self.state, InputState::Closed { .. }) {
            self.transition(InputState::Unknown);
        }
    }
}

#[derive(Default)]
pub(crate) struct Submission {
    attempted_at: Cell<Option<std::time::Instant>>,
    spawned_at: Cell<Option<std::time::Instant>>,
    pid: Cell<Option<u32>>,
}

impl Submission {
    pub(crate) fn attempted(&self) -> bool {
        self.attempted_at.get().is_some()
    }

    pub(crate) fn snapshot(&self, origin: std::time::Instant) -> crate::json::Value {
        use crate::json::Value;
        let elapsed = |value: Option<std::time::Instant>| {
            value
                .and_then(|value| value.checked_duration_since(origin))
                .map_or(Value::Null, |duration| {
                    Value::Number(duration.as_secs_f64() * 1000.0)
                })
        };
        Value::object(vec![
            ("schema_version", Value::Number(1.0)),
            (
                "state",
                Value::string(if self.attempted() {
                    "may-have-executed"
                } else {
                    "not-attempted"
                }),
            ),
            (
                "native_pid",
                self.pid
                    .get()
                    .map_or(Value::Null, |pid| Value::Number(f64::from(pid))),
            ),
            ("attempted_elapsed_ms", elapsed(self.attempted_at.get())),
            ("spawned_elapsed_ms", elapsed(self.spawned_at.get())),
        ])
    }
}

#[cfg(windows)]
enum ReadError {
    Io(io::Error),
    Limit,
    Input(io::Error),
    EarlyLeaderExit(ExitStatus),
}

#[cfg(windows)]
async fn collect(
    mut pipe: impl AsyncRead + Unpin,
    output: &mut Vec<u8>,
    budget: Arc<AtomicUsize>,
    maximum: usize,
) -> Result<(), ReadError> {
    let mut buffer = [0_u8; 4096];
    loop {
        let read = pipe.read(&mut buffer).await.map_err(ReadError::Io)?;
        if read == 0 {
            return Ok(());
        }
        let previous = budget.fetch_add(read, Ordering::AcqRel);
        let available = maximum.saturating_sub(previous);
        output.extend(buffer.iter().take(read.min(available)));
        if read > available {
            return Err(ReadError::Limit);
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicU32;

    use process_wrap::tokio::{
        CreationFlags, JobObject, KillOnDrop, TokioChildWrapper, TokioCommandWrap,
        TokioCommandWrapper,
    };
    use tokio::process::Child as TokioChild;
    use win32job::{ExtendedLimitInfo, Job};
    use winsafe::{HPROCESS, co, guard::CloseHandleGuard, prelude::*};

    use super::*;

    #[derive(Debug)]
    struct Assign {
        job: Arc<Job>,
        pid: Arc<AtomicU32>,
    }

    impl TokioCommandWrapper for Assign {
        fn post_spawn(&mut self, child: &mut TokioChild, _: &TokioCommandWrap) -> io::Result<()> {
            let pid = child
                .id()
                .ok_or_else(|| io::Error::other("Owned child has no PID"))?;
            self.pid.store(pid, Ordering::Release);
            let handle = child
                .raw_handle()
                .ok_or_else(|| io::Error::other("Owned child has no process handle"))?;
            self.job
                .assign_process(handle as isize)
                .map_err(io::Error::other)
        }
    }

    pub(super) struct Child {
        inner: Box<dyn TokioChildWrapper>,
        job: Arc<Job>,
        known: BTreeMap<u32, CloseHandleGuard<HPROCESS>>,
        diagnostics: Option<Arc<ProcessTrace>>,
    }

    impl Child {
        pub(super) fn spawn(command: Command) -> Result<Self, Failure> {
            let mut limits = ExtendedLimitInfo::new();
            limits.limit_kill_on_job_close();
            let job = Arc::new(
                Job::create_with_limit_info(&limits).map_err(|error| Failure {
                    kind: FailureKind::Control(Box::new(Error::new(
                        "OwnedProcessError",
                        format!("Create owned job: {error}"),
                    ))),
                    captured: Captured::default(),
                    cleanup_complete: true,
                    source: None,
                })?,
            );
            let pid = Arc::new(AtomicU32::new(0));
            let mut wrapped = TokioCommandWrap::from(command);
            wrapped
                .wrap(CreationFlags(Default::default()))
                .wrap(KillOnDrop)
                .wrap(Assign {
                    job: job.clone(),
                    pid: pid.clone(),
                })
                .wrap(JobObject);
            let inner = wrapped.spawn().map_err(|error| {
                let pid = pid.load(Ordering::Acquire);
                Failure {
                    kind: FailureKind::Spawn,
                    captured: Captured {
                        pid: (pid != 0).then_some(pid),
                        ..Captured::default()
                    },
                    cleanup_complete: pid == 0,
                    source: Some(error),
                }
            })?;
            Ok(Self {
                inner,
                job,
                known: BTreeMap::new(),
                diagnostics: None,
            })
        }

        pub(super) fn inner_mut(&mut self) -> &mut TokioChild {
            self.inner.inner_mut()
        }

        pub(super) fn id(&self) -> Option<u32> {
            self.inner.id()
        }

        pub(super) fn diagnostics(&mut self, diagnostics: Option<Arc<ProcessTrace>>) {
            self.diagnostics = diagnostics;
        }

        fn empty(&mut self, parent: Option<&Span>) -> io::Result<bool> {
            let diagnostics = self.diagnostics.clone();
            let scope = |label| {
                diagnostics.as_ref().map(|trace| match parent {
                    Some(parent) => parent.child(label),
                    None => trace.scope(label),
                })
            };
            let query = scope(Label::NoInputJobQuery);
            let observed = self.job.query_process_id_list().map_err(io::Error::other);
            if let Some(query) = query {
                query.finish_status(observed.is_ok());
            }
            let pids = observed?;
            for pid in &pids {
                let id = u32::try_from(*pid).map_err(io::Error::other)?;
                if self.known.contains_key(&id) {
                    continue;
                }
                let opening = scope(Label::NoInputMemberOpen);
                let observed = HPROCESS::OpenProcess(
                    co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
                    false,
                    id,
                );
                if let Some(opening) = opening {
                    opening.finish_status(observed.is_ok());
                }
                let handle = match observed {
                    Ok(handle) => handle,
                    Err(error) if error == co::ERROR::INVALID_PARAMETER => continue,
                    Err(error) => return Err(io::Error::other(error)),
                };
                let query = scope(Label::NoInputJobQuery);
                let observed = self.job.query_process_id_list().map_err(io::Error::other);
                if let Some(query) = query {
                    query.finish_status(observed.is_ok());
                }
                if observed?.contains(pid) {
                    self.known.insert(id, handle);
                }
            }
            if !pids.is_empty() {
                return Ok(false);
            }
            if let Some(trace) = &diagnostics {
                trace.job_zero();
            }
            for handle in self.known.values() {
                let waiting = scope(Label::NoInputMemberWait);
                let observed = handle
                    .WaitForSingleObject(Some(0))
                    .map_err(io::Error::other);
                if let Some(waiting) = waiting {
                    waiting.finish_status(observed.is_ok());
                }
                match observed? {
                    result if result == co::WAIT::OBJECT_0 => {}
                    result if result == co::WAIT::TIMEOUT => return Ok(false),
                    _ => {
                        return Err(io::Error::other(
                            "Owned process wait returned an unknown state",
                        ));
                    }
                }
            }
            if let Some(trace) = &diagnostics {
                trace.members();
            }
            Ok(true)
        }

        pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
            let diagnostics = self.diagnostics.clone();
            let waiting = diagnostics
                .as_ref()
                .map(|trace| trace.scope(Label::NoInputWait));
            let mut leader = diagnostics
                .as_ref()
                .filter(|trace| !trace.leader_seen())
                .and_then(|_| {
                    waiting
                        .as_ref()
                        .map(|span| span.child(Label::NoInputLeaderWindow))
                });
            let mut group = waiting
                .as_ref()
                .map(|span| span.child(Label::NoInputGroupWindow));
            loop {
                let observing = waiting
                    .as_ref()
                    .map(|span| span.child(Label::NoInputTryWait));
                let observed = self.inner.inner_mut().try_wait();
                if let Some(observing) = observing {
                    observing.finish_status(observed.is_ok());
                }
                let status = observed?;
                if status.is_some() {
                    if let Some(trace) = &diagnostics {
                        trace.leader();
                    }
                    timing::finish(leader.take(), None);
                }
                if self.empty(group.as_ref().or(waiting.as_ref()))? {
                    timing::finish(group.take(), None);
                    let observed = match status {
                        Some(status) => Ok(status),
                        None => self.inner.inner_mut().wait().await,
                    };
                    if observed.is_ok() {
                        if let Some(trace) = &diagnostics {
                            trace.leader();
                        }
                        timing::finish(leader.take(), None);
                    }
                    if let Some(waiting) = waiting {
                        waiting.finish_status(observed.is_ok());
                    }
                    return observed;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }

        pub(super) async fn wait_leader(&mut self) -> io::Result<ExitStatus> {
            self.inner.inner_mut().wait().await
        }

        pub(super) async fn stop(&mut self) -> io::Result<()> {
            let _cleanup = self
                .diagnostics
                .as_ref()
                .map(|trace| trace.scope(Label::NoInputCleanup));
            if !self.empty(None)? {
                self.inner.start_kill()?;
            }
            self.wait().await?;
            if !self.empty(None)? {
                return Err(io::Error::other(
                    "Owned job still contains processes after shutdown",
                ));
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
async fn cancellation(token: Option<&CancellationToken>) -> Result<Reason, Error> {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

#[cfg(windows)]
async fn deadline(timeout: Option<Duration>) {
    match timeout {
        Some(timeout) => tokio::time::sleep(timeout).await,
        None => std::future::pending().await,
    }
}

#[cfg(windows)]
enum End {
    Exited(Result<ExitStatus, ReadError>),
    Cancelled(Result<Reason, Error>),
    Timeout,
}

#[cfg(windows)]
async fn input_then_wait(
    child: &mut windows::Child,
    input: &mut InputController,
) -> Result<ExitStatus, ReadError> {
    {
        let writing = input.complete();
        tokio::pin!(writing);
        tokio::select! {
            biased;
            result = &mut writing => result.map_err(ReadError::Input)?,
            status = child.wait_leader() => {
                return Err(ReadError::EarlyLeaderExit(status.map_err(ReadError::Io)?));
            }
        }
    }
    child.wait().await.map_err(ReadError::Io)
}

#[cfg(windows)]
fn input_cleanup_result(
    kind: FailureKind,
    source: Option<io::Error>,
    captured: Captured,
    cleanup: Result<(io::Result<()>, Result<JoinedInput, Error>), ()>,
) -> Failure {
    match cleanup {
        Ok((Ok(()), Ok(joined))) => Failure {
            kind,
            captured,
            cleanup_complete: true,
            source: source.or(joined.terminal_error),
        },
        _ => {
            let initiating = Failure {
                kind,
                captured: Captured::default(),
                cleanup_complete: true,
                source: None,
            }
            .code()
            .unwrap_or("ROSTER_PROCESS_FAILURE");
            Failure {
                kind: FailureKind::Cleanup,
                captured,
                cleanup_complete: false,
                source: Some(io::Error::other(format!(
                    "Owned input cleanup unverified; initiating={initiating}; first_os_code={:?}",
                    source.as_ref().and_then(io::Error::raw_os_error)
                ))),
            }
        }
    }
}

#[cfg(windows)]
async fn cleanup_input(
    child: &mut windows::Child,
    input: &mut InputController,
    kind: FailureKind,
    source: Option<io::Error>,
    captured: Captured,
) -> Failure {
    input.latch_abort();
    let cleanup = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(biased; child.stop(), input.join_aborted())
    })
    .await;
    let failure = input_cleanup_result(kind, source, captured, cleanup.map_err(|_| ()));
    if !failure.cleanup_complete {
        input.transition(InputState::Unknown);
    }
    #[cfg(test)]
    if let Some(trace) = &input.input.trace {
        trace.known_cleanup.set(failure.cleanup_complete);
    }
    failure
}

pub fn execute(
    command: Command,
    limits: Limits,
    token: Option<&CancellationToken>,
) -> impl std::future::Future<Output = Result<Output, Failure>> + '_ {
    execute_observed(command, limits, token, None)
}

pub(crate) async fn execute_observed(
    command: Command,
    limits: Limits,
    token: Option<&CancellationToken>,
    submission: Option<&Submission>,
) -> Result<Output, Failure> {
    execute_with_input(command, limits, token, submission, None).await
}

pub(crate) async fn execute_with_input(
    command: Command,
    limits: Limits,
    token: Option<&CancellationToken>,
    submission: Option<&Submission>,
    input: Option<FiniteInput>,
) -> Result<Output, Failure> {
    execute_core(command, limits, token, submission, input, None, None).await
}

pub(crate) fn execute_observed_timed<'a>(
    command: Command,
    limits: Limits,
    token: Option<&'a CancellationToken>,
    submission: Option<&'a Submission>,
    parent: Option<&Span>,
) -> impl std::future::Future<Output = Result<Output, Failure>> + 'a {
    let diagnostics = parent.map(|parent| ProcessTrace::new(parent, Label::NoInputOwned));
    let dispatched = diagnostics
        .as_ref()
        .map(|trace| trace.scope(Label::NoInputDispatch));
    execute_core(
        command,
        limits,
        token,
        submission,
        None,
        diagnostics,
        dispatched,
    )
}

async fn execute_core(
    mut command: Command,
    limits: Limits,
    token: Option<&CancellationToken>,
    submission: Option<&Submission>,
    input: Option<FiniteInput>,
    diagnostics: Option<Arc<ProcessTrace>>,
    dispatched: Option<Span>,
) -> Result<Output, Failure> {
    timing::finish(dispatched, None);
    let mut preflight = diagnostics
        .as_ref()
        .map(|trace| trace.scope(Label::NoInputPreflight));
    let initial = |error: Error| Failure {
        kind: FailureKind::Control(Box::new(error)),
        captured: Captured::default(),
        cleanup_complete: true,
        source: None,
    };
    if limits.max_output_bytes > 32 * 1024 * 1024 {
        if let Some(preflight) = preflight.take() {
            preflight.finish_status(false);
        }
        return Err(initial(Error::type_error(
            "Owned output limit must not exceed 32 MiB",
        )));
    }
    if input
        .as_ref()
        .is_some_and(|input| input.bytes.len() > MAX_INPUT_BYTES)
    {
        if let Some(preflight) = preflight.take() {
            preflight.finish_status(false);
        }
        return Err(initial(Error::type_error(
            "Owned finite input must not exceed 256 KiB",
        )));
    }
    if let Some(token) = token {
        let checked = token.check();
        if checked.is_err() {
            if let Some(preflight) = preflight.take() {
                preflight.finish_status(false);
            }
        }
        checked.map_err(initial)?;
    }
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    timing::finish(preflight, None);
    #[cfg(not(windows))]
    {
        let _ = command;
        let _ = submission;
        let _ = input;
        let _ = diagnostics;
        Err(initial(super::unsupported(
            "Strict native job ownership on this platform requires an approved containment provider",
        )))
    }
    #[cfg(windows)]
    {
        if let Some(submission) = submission {
            submission.attempted_at.set(Some(std::time::Instant::now()));
        }
        let spawn = diagnostics
            .as_ref()
            .map(|trace| trace.scope(Label::NoInputSpawn));
        let observed = windows::Child::spawn(command);
        if let Some(spawn) = spawn {
            spawn.finish_status(observed.is_ok());
        }
        let mut child = observed?;
        child.diagnostics(diagnostics.clone());
        if let Some(submission) = submission {
            submission.pid.set(child.id());
            submission.spawned_at.set(Some(std::time::Instant::now()));
        }
        let mut captured = Captured {
            pid: child.id(),
            ..Captured::default()
        };
        let mut input = match input {
            Some(input) => match child.inner_mut().stdin.take() {
                Some(stdin) => Some(InputController::new(stdin, input)),
                None => {
                    let cleanup = tokio::time::timeout(Duration::from_secs(5), child.stop()).await;
                    return Err(Failure {
                        kind: FailureKind::Control(Box::new(
                            Error::new("OwnedInputError", "Owned finite input pipe is missing")
                                .coded("ROSTER_PROCESS_INPUT"),
                        )),
                        captured,
                        cleanup_complete: matches!(cleanup, Ok(Ok(()))),
                        source: None,
                    });
                }
            },
            None => None,
        };
        let stdout = child.inner_mut().stdout.take();
        let stderr = child.inner_mut().stderr.take();
        let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
            if let Some(input) = input.as_mut() {
                return Err(
                    cleanup_input(&mut child, input, FailureKind::Io, None, captured).await,
                );
            }
            let cleanup = tokio::time::timeout(Duration::from_secs(5), child.stop()).await;
            return Err(Failure {
                kind: FailureKind::Io,
                captured,
                cleanup_complete: matches!(cleanup, Ok(Ok(()))),
                source: None,
            });
        };
        let budget = Arc::new(AtomicUsize::new(0));
        let end = if let Some(input) = input.as_mut() {
            let work = async {
                let ((), (), status) = tokio::try_join!(
                    biased;
                    collect(stdout, &mut captured.stdout, budget.clone(), limits.max_output_bytes),
                    collect(stderr, &mut captured.stderr, budget.clone(), limits.max_output_bytes),
                    input_then_wait(&mut child, input),
                )?;
                Ok(status)
            };
            tokio::pin!(work);
            tokio::select! {
                biased;
                reason = cancellation(token) => End::Cancelled(reason),
                () = deadline(limits.timeout) => End::Timeout,
                status = &mut work => End::Exited(status),
            }
        } else {
            let work = async {
                let (status, (), ()) = tokio::try_join!(
                    async { child.wait().await.map_err(ReadError::Io) },
                    collect(
                        timing::ReadTrace::new(stdout, diagnostics.as_ref(), true),
                        &mut captured.stdout,
                        budget.clone(),
                        limits.max_output_bytes
                    ),
                    collect(
                        timing::ReadTrace::new(stderr, diagnostics.as_ref(), false),
                        &mut captured.stderr,
                        budget.clone(),
                        limits.max_output_bytes
                    ),
                )?;
                Ok(status)
            };
            tokio::pin!(work);
            tokio::select! {
                status = &mut work => End::Exited(status),
                reason = cancellation(token) => End::Cancelled(reason),
                () = deadline(limits.timeout) => End::Timeout,
            }
        };
        let end = match end {
            End::Exited(Ok(_)) if input.as_ref().is_some_and(|input| !input.delivered()) => {
                End::Exited(Err(ReadError::Input(io::Error::other(
                    "Owned input delivery and EOF are incomplete",
                ))))
            }
            other => other,
        };
        let (kind, source) = match end {
            End::Exited(Ok(status)) => {
                if let Some(token) = token {
                    match token.reason() {
                        Ok(Some(reason)) => {
                            return Err(Failure {
                                kind: FailureKind::Cancelled(reason),
                                captured,
                                cleanup_complete: true,
                                source: None,
                            });
                        }
                        Err(error) => {
                            return Err(Failure {
                                kind: FailureKind::Control(Box::new(error)),
                                captured,
                                cleanup_complete: true,
                                source: None,
                            });
                        }
                        Ok(None) => {}
                    }
                }
                if status.success() {
                    return Ok(Output { status, captured });
                }
                return Err(Failure {
                    kind: FailureKind::Exit(status.code()),
                    captured,
                    cleanup_complete: true,
                    source: None,
                });
            }
            End::Exited(Err(ReadError::Io(error))) => (FailureKind::Io, Some(error)),
            End::Exited(Err(ReadError::Limit)) => (FailureKind::OutputLimit, None),
            End::Exited(Err(ReadError::Input(error))) => (
                FailureKind::Control(Box::new(
                    Error::new("OwnedInputError", "Owned finite input delivery failed")
                        .coded("ROSTER_PROCESS_INPUT"),
                )),
                Some(error),
            ),
            End::Exited(Err(ReadError::EarlyLeaderExit(status))) => (
                FailureKind::Control(Box::new(
                    Error::new("OwnedInputError", format!(
                        "Owned leader exited before finite input delivery and EOF; exit_code={:?}",
                        status.code()
                    )).coded("ROSTER_PROCESS_INPUT"),
                )),
                None,
            ),
            End::Cancelled(Ok(reason)) => (FailureKind::Cancelled(reason), None),
            End::Cancelled(Err(error)) => (FailureKind::Control(Box::new(error)), None),
            End::Timeout => (FailureKind::Timeout, None),
        };
        if let Some(input) = input.as_mut() {
            return Err(cleanup_input(&mut child, input, kind, source, captured).await);
        }
        match tokio::time::timeout(Duration::from_secs(5), child.stop()).await {
            Ok(Ok(())) => Err(Failure {
                kind,
                captured,
                cleanup_complete: true,
                source,
            }),
            Ok(Err(error)) => Err(Failure {
                kind: FailureKind::Cleanup,
                captured,
                cleanup_complete: false,
                source: Some(error),
            }),
            Err(_) => Err(Failure {
                kind: FailureKind::Cleanup,
                captured,
                cleanup_complete: false,
                source: Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Owned job shutdown deadline expired",
                )),
            }),
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::cancel::CancellationSource;
    use crate::execution::tools;
    use crate::state::locks::{Liveness, probe_pid};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicU64;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Marker(PathBuf);

    impl Drop for Marker {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn captured_pid(bytes: &[u8]) -> Result<u64, Error> {
        std::str::from_utf8(bytes)
            .map_err(|error| Error::new("Error", error.to_string()))?
            .trim()
            .parse::<u64>()
            .map_err(|error| Error::new("Error", error.to_string()))
    }

    fn node(source: &str) -> Result<Command, Error> {
        let executable = std::env::var_os("ROSTER_TEST_NODE").ok_or_else(|| {
            Error::new("Error", "Owned process fixture requires approved Node 20")
        })?;
        let mut command = Command::new(executable);
        command
            .env_clear()
            .envs(tools::Host::default().environment)
            .args(["-e", source]);
        Ok(command)
    }

    fn failed(result: Result<Output, Failure>) -> Result<Failure, Error> {
        match result {
            Ok(_) => Err(Error::new(
                "Error",
                "Owned failure control unexpectedly passed",
            )),
            Err(failure) => Ok(failure),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn owned_job_must_wait_for_descendants_after_root_exit() -> Result<(), Error> {
        let command = node(
            "const cp=require('node:child_process');const child=cp.spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore',detached:true});process.stdout.write(String(child.pid));child.unref();",
        )?;
        let failure = failed(
            execute(
                command,
                Limits {
                    timeout: Some(Duration::from_secs(2)),
                    max_output_bytes: 4096,
                },
                None,
            )
            .await,
        )?;
        assert!(matches!(failure.kind, FailureKind::Timeout));
        assert!(failure.cleanup_complete, "{failure}");
        let pid = captured_pid(&failure.captured.stdout)?;
        assert_eq!(probe_pid(pid)?, Liveness::Dead);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn owned_job_success_waits_for_natural_descendant_exit() -> Result<(), Error> {
        let output = execute(node("const child=require('node:child_process').spawn(process.execPath,['-e','setTimeout(()=>{},250)'],{stdio:'ignore',detached:true});process.stdout.write(String(child.pid));child.unref();")?, Limits {timeout: Some(Duration::from_secs(10)), max_output_bytes: 4096}, None).await.map_err(|error| Error::new("Error", error.to_string()))?;
        assert!(output.status.success());
        assert_eq!(
            probe_pid(captured_pid(&output.captured.stdout)?)?,
            Liveness::Dead
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn owned_job_overflow_reaps_a_detached_descendant() -> Result<(), Error> {
        let failure = failed(execute(node("const child=require('node:child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore',detached:true});process.stdout.write(String(child.pid));setTimeout(()=>process.stderr.write('x'.repeat(65536)),250);setInterval(()=>{},1000);")?, Limits {timeout: Some(Duration::from_secs(10)), max_output_bytes: 64}, None).await)?;
        assert!(matches!(failure.kind, FailureKind::OutputLimit));
        assert!(failure.cleanup_complete, "{failure}");
        assert_eq!(
            failure.captured.stdout.len() + failure.captured.stderr.len(),
            64
        );
        assert_eq!(
            probe_pid(captured_pid(&failure.captured.stdout)?)?,
            Liveness::Dead
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn owned_job_collects_real_exit_and_aggregate_output() -> Result<(), Error> {
        let output = execute(
            node("process.stdout.write('out');process.stderr.write('err');")?,
            Limits {
                timeout: Some(Duration::from_secs(10)),
                max_output_bytes: 6,
            },
            None,
        )
        .await
        .map_err(|error| Error::new("Error", error.to_string()))?;
        assert_eq!(output.captured.stdout, b"out");
        assert_eq!(output.captured.stderr, b"err");
        let failure = failed(execute(node("process.stdout.write('out');process.stderr.write('err');setInterval(()=>{},1000);")?, Limits {timeout: Some(Duration::from_secs(10)), max_output_bytes: 5}, None).await)?;
        assert!(matches!(failure.kind, FailureKind::OutputLimit));
        assert!(failure.cleanup_complete);
        assert_eq!(
            failure.captured.stdout.len() + failure.captured.stderr.len(),
            5
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn owned_job_cancel_and_spawn_failures_are_not_success() -> Result<(), Error> {
        let source = CancellationSource::default();
        let token = source.token();
        let marker = Marker(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".owned-ready-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )));
        let path = crate::json::Value::string(marker.0.to_string_lossy().as_ref()).json();
        let cancellation = async {
            for _ in 0..1000 {
                match std::fs::read(&marker.0) {
                    Ok(bytes) if !bytes.is_empty() => {
                        let pid = captured_pid(&bytes)?;
                        assert_eq!(probe_pid(pid)?, Liveness::Alive);
                        source.cancel(Reason::Cancelled)?;
                        return Ok(pid);
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(Error::new("Error", error.to_string())),
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(Error::new(
                "Error",
                "Owned descendant never reached the cancellation readiness barrier",
            ))
        };
        let command = node(&format!(
            "const child=require('node:child_process').spawn(process.execPath,['-e','setInterval(()=>{{}},1000)'],{{stdio:'ignore',detached:true}});require('node:fs').writeFileSync({path},String(child.pid),{{flag:'wx',mode:0o600}});setInterval(()=>{{}},1000);"
        ))?;
        let (result, cancelled) = tokio::join!(
            execute(
                command,
                Limits {
                    timeout: Some(Duration::from_secs(10)),
                    max_output_bytes: 4096
                },
                Some(&token)
            ),
            cancellation
        );
        let pid = cancelled?;
        let failure = failed(result)?;
        assert!(matches!(
            failure.kind,
            FailureKind::Cancelled(Reason::Cancelled)
        ));
        assert!(failure.cleanup_complete);
        assert_eq!(probe_pid(pid)?, Liveness::Dead);
        let command = Command::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".owned-program-missing.exe"),
        );
        let missing = failed(execute(command, Limits::default(), None).await)?;
        assert!(matches!(missing.kind, FailureKind::Spawn));
        assert!(missing.cleanup_complete);
        assert!(missing.captured.pid.is_none());
        Ok(())
    }
}

#[cfg(all(test, windows))]
#[path = "owned/submission_tests.rs"]
mod submission_tests;

#[cfg(all(test, windows))]
#[path = "owned/input_tests.rs"]
mod input_tests;
