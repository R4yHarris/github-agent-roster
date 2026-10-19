use std::fmt;
use std::io;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use crate::execution::timing::{self, Label, ProcessTrace, Span};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

use crate::Error;
use crate::cancel::{CancellationToken, Reason};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Option<Duration>,
    pub max_output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: None,
            max_output_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Default)]
pub struct Captured {
    pub pid: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub captured: Captured,
}

#[derive(Debug)]
pub enum FailureKind {
    Spawn,
    Io,
    OutputLimit,
    Exit(Option<i32>),
    Timeout,
    Cancelled(Reason),
    Control(Box<Error>),
    Cleanup,
}

#[derive(Debug)]
pub struct Failure {
    pub kind: FailureKind,
    pub captured: Captured,
    pub cleanup_complete: bool,
    pub source: Option<io::Error>,
}

impl Failure {
    pub fn code(&self) -> Option<&'static str> {
        match &self.kind {
            FailureKind::Spawn => Some("ROSTER_PROCESS_START"),
            FailureKind::Io => Some("ROSTER_PROCESS_IO"),
            FailureKind::OutputLimit => Some("ERR_CHILD_PROCESS_STDIO_MAXBUFFER"),
            FailureKind::Timeout => Some("ETIMEDOUT"),
            FailureKind::Cancelled(Reason::Cancelled) => Some("ROSTER_CANCELLED"),
            FailureKind::Cancelled(Reason::TaskTimeout(_)) => Some("ROSTER_TASK_TIMEOUT"),
            FailureKind::Control(error) => error.code,
            FailureKind::Cleanup => Some("ROSTER_PROCESS_CLEANUP"),
            FailureKind::Exit(_) => None,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            FailureKind::Spawn => formatter.write_str("Child process could not start"),
            FailureKind::Io => formatter.write_str("Child process I/O failed"),
            FailureKind::OutputLimit => {
                formatter.write_str("Child process exceeded its output limit")
            }
            FailureKind::Exit(Some(code)) => {
                write!(formatter, "Child process exited with code {code}")
            }
            FailureKind::Exit(None) => {
                formatter.write_str("Child process ended without an exit code")
            }
            FailureKind::Timeout => formatter.write_str("Child process timed out"),
            FailureKind::Cancelled(reason) => reason.error().fmt(formatter),
            FailureKind::Control(error) => error.fmt(formatter),
            FailureKind::Cleanup => write!(
                formatter,
                "Owned child cleanup could not be verified (pid={:?}); retain ownership and inspect the process",
                self.captured.pid,
            ),
        }
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|error| error as &dyn std::error::Error)
    }
}

enum ReadFailure {
    Io(io::Error),
    Limit,
}

async fn read_limited(
    reader: impl AsyncRead + Unpin,
    output: &mut Vec<u8>,
    maximum: usize,
) -> Result<(), ReadFailure> {
    reader
        .take(maximum as u64 + 1)
        .read_to_end(output)
        .await
        .map_err(ReadFailure::Io)?;
    if output.len() > maximum {
        output.truncate(maximum);
        return Err(ReadFailure::Limit);
    }
    Ok(())
}

async fn wait_cancel(token: Option<&CancellationToken>) -> Result<Reason, Error> {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn wait_timeout(timeout: Option<Duration>) {
    match timeout {
        Some(timeout) => tokio::time::sleep(timeout).await,
        None => std::future::pending().await,
    }
}

async fn stop_owned(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    match child.kill().await {
        Ok(()) => Ok(()),
        Err(error) => match child.try_wait()? {
            Some(_) => Ok(()),
            None => Err(error),
        },
    }
}

enum End {
    Exited(Result<ExitStatus, ReadFailure>),
    Cancelled(Result<Reason, Error>),
    Timeout,
}

pub async fn execute(
    command: &mut Command,
    limits: Limits,
    token: Option<&CancellationToken>,
) -> Result<Output, Failure> {
    execute_core(command, limits, token, None, None).await
}

pub(crate) fn execute_timed<'a>(
    command: &'a mut Command,
    limits: Limits,
    token: Option<&'a CancellationToken>,
    parent: Option<&Span>,
) -> impl std::future::Future<Output = Result<Output, Failure>> + 'a {
    let diagnostics = parent.map(|parent| ProcessTrace::new(parent, Label::NoInputDirect));
    let dispatched = diagnostics
        .as_ref()
        .map(|trace| trace.scope(Label::NoInputDispatch));
    execute_core(command, limits, token, diagnostics, dispatched)
}

async fn execute_core(
    command: &mut Command,
    limits: Limits,
    token: Option<&CancellationToken>,
    diagnostics: Option<Arc<ProcessTrace>>,
    dispatched: Option<Span>,
) -> Result<Output, Failure> {
    timing::finish(dispatched, None);
    let mut preflight = diagnostics
        .as_ref()
        .map(|trace| trace.scope(Label::NoInputPreflight));
    let initial = |kind| Failure {
        kind,
        captured: Captured::default(),
        cleanup_complete: true,
        source: None,
    };
    if limits.max_output_bytes > 32 * 1024 * 1024 {
        if let Some(preflight) = preflight.take() {
            preflight.finish_status(false);
        }
        return Err(initial(FailureKind::Control(Box::new(Error::type_error(
            "Process output limit must not exceed 32 MiB",
        )))));
    }
    if let Some(token) = token {
        let checked = token.check();
        if checked.is_err() {
            if let Some(preflight) = preflight.take() {
                preflight.finish_status(false);
            }
        }
        checked.map_err(|error| initial(FailureKind::Control(Box::new(error))))?;
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    timing::finish(preflight, None);
    let spawning = diagnostics
        .as_ref()
        .map(|trace| trace.scope(Label::NoInputSpawn));
    let observed = command.spawn().map_err(|error| Failure {
        kind: FailureKind::Spawn,
        captured: Captured::default(),
        cleanup_complete: true,
        source: Some(error),
    });
    if let Some(spawning) = spawning {
        spawning.finish_status(observed.is_ok());
    }
    let mut child = observed?;
    let mut captured = Captured {
        pid: child.id(),
        ..Captured::default()
    };
    let pipes = (child.stdout.take(), child.stderr.take());
    let (Some(stdout), Some(stderr)) = pipes else {
        let cleanup = stop_owned(&mut child).await;
        return Err(Failure {
            kind: if cleanup.is_ok() {
                FailureKind::Io
            } else {
                FailureKind::Cleanup
            },
            captured,
            cleanup_complete: cleanup.is_ok(),
            source: cleanup.err(),
        });
    };
    let end = {
        let work = async {
            let wait = async {
                let waiting = diagnostics
                    .as_ref()
                    .map(|trace| trace.scope(Label::NoInputWait));
                let leader = waiting
                    .as_ref()
                    .map(|span| span.child(Label::NoInputLeaderWindow));
                let observed = child.wait().await;
                if observed.is_ok() {
                    if let Some(trace) = &diagnostics {
                        trace.leader();
                    }
                }
                if let Some(leader) = leader {
                    leader.finish_status(observed.is_ok());
                }
                if let Some(waiting) = waiting {
                    waiting.finish_status(observed.is_ok());
                }
                observed.map_err(ReadFailure::Io)
            };
            let (status, (), ()) = tokio::try_join!(
                wait,
                read_limited(
                    timing::ReadTrace::new(stdout, diagnostics.as_ref(), true),
                    &mut captured.stdout,
                    limits.max_output_bytes
                ),
                read_limited(
                    timing::ReadTrace::new(stderr, diagnostics.as_ref(), false),
                    &mut captured.stderr,
                    limits.max_output_bytes
                ),
            )?;
            Ok(status)
        };
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => End::Exited(result),
            reason = wait_cancel(token) => End::Cancelled(reason),
            () = wait_timeout(limits.timeout) => End::Timeout,
        }
    };
    captured.stdout.truncate(limits.max_output_bytes);
    captured.stderr.truncate(limits.max_output_bytes);
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
        End::Exited(Err(ReadFailure::Io(error))) => (FailureKind::Io, Some(error)),
        End::Exited(Err(ReadFailure::Limit)) => (FailureKind::OutputLimit, None),
        End::Cancelled(Ok(reason)) => (FailureKind::Cancelled(reason), None),
        End::Cancelled(Err(error)) => (FailureKind::Control(Box::new(error)), None),
        End::Timeout => (FailureKind::Timeout, None),
    };
    if let Err(error) = stop_owned(&mut child).await {
        return Err(Failure {
            kind: FailureKind::Cleanup,
            captured,
            cleanup_complete: false,
            source: Some(error),
        });
    }
    Err(Failure {
        kind,
        captured,
        cleanup_complete: true,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::CancellationSource;
    use std::ffi::OsString;

    fn node(script: &str) -> Command {
        let program =
            std::env::var_os("ROSTER_TEST_NODE").unwrap_or_else(|| OsString::from("node"));
        let mut command = Command::new(program);
        command.args(["-e", script]);
        command
    }

    #[tokio::test(flavor = "current_thread")]
    async fn closes_stdin_and_collects_both_streams_without_node_runtime_delegation()
    -> Result<(), Failure> {
        let mut command = node(
            "require('node:fs').readFileSync(0); process.stdout.write('eof'); process.stderr.write('diagnostic');",
        );
        let output = execute(
            &mut command,
            Limits {
                timeout: Some(Duration::from_secs(10)),
                ..Limits::default()
            },
            None,
        )
        .await?;
        assert_eq!(output.captured.stdout, b"eof");
        assert_eq!(output.captured.stderr, b"diagnostic");
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn preserves_real_nonzero_exit_and_output() {
        let failure = execute(
            &mut node(
                "process.stdout.write('out'); process.stderr.write('err'); process.exitCode=7;",
            ),
            Limits {
                timeout: Some(Duration::from_secs(10)),
                ..Limits::default()
            },
            None,
        )
        .await;
        assert!(failure.is_err_and(|failure| {
            matches!(failure.kind, FailureKind::Exit(Some(7)))
                && failure.captured.stdout == b"out"
                && failure.captured.stderr == b"err"
                && failure.cleanup_complete
        }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn enforces_byte_limits_and_reaps_an_output_flooding_child() {
        let failure = execute(
            &mut node("process.stdout.write('€'.repeat(1000)); setInterval(()=>{},1000);"),
            Limits {
                timeout: Some(Duration::from_secs(10)),
                max_output_bytes: 16,
            },
            None,
        )
        .await;
        assert!(failure.is_err_and(|failure| {
            matches!(failure.kind, FailureKind::OutputLimit)
                && failure.captured.stdout.len() == 16
                && failure.cleanup_complete
                && failure.captured.pid.is_some()
        }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn timeout_and_cancellation_return_only_after_owned_child_cleanup() -> Result<(), Error> {
        let timed = execute(
            &mut node("setInterval(()=>{},1000);"),
            Limits {
                timeout: Some(Duration::from_millis(50)),
                ..Limits::default()
            },
            None,
        )
        .await;
        assert!(timed.is_err_and(
            |failure| matches!(failure.kind, FailureKind::Timeout) && failure.cleanup_complete
        ));
        let source = CancellationSource::default();
        let token = source.token();
        let cancelled = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            source.cancel(Reason::TaskTimeout(100))
        };
        let mut command = node("setInterval(()=>{},1000);");
        let (result, cancel) = tokio::join!(
            execute(
                &mut command,
                Limits {
                    timeout: Some(Duration::from_secs(10)),
                    ..Limits::default()
                },
                Some(&token)
            ),
            cancelled,
        );
        cancel?;
        assert!(result.is_err_and(|failure| {
            matches!(
                failure.kind,
                FailureKind::Cancelled(Reason::TaskTimeout(100))
            ) && failure.cleanup_complete
                && failure.code() == Some("ROSTER_TASK_TIMEOUT")
        }));
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn already_cancelled_work_never_spawns_a_child() -> Result<(), Error> {
        let source = CancellationSource::default();
        source.cancel(Reason::Cancelled)?;
        let result = execute(
            &mut node("process.exitCode=0;"),
            Limits::default(),
            Some(&source.token()),
        )
        .await;
        assert!(
            result.is_err_and(|failure| failure.captured.pid.is_none()
                && failure.code() == Some("ROSTER_CANCELLED"))
        );
        Ok(())
    }
}
