use super::*;
use crate::cancel::CancellationSource;
use crate::execution::tools;
use crate::state::locks::{Liveness, probe_pid};
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct Fixture {
    root: PathBuf,
    image: PathBuf,
}

impl Fixture {
    async fn new() -> Result<Self, Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".retained-input-{}-{}",
            std::process::id(),
            crate::state::files::nonce()?
        ));
        let project = root.join("provider");
        crate::state::files::create_private_directories(&project.join("src"))?;
        let fixture = Self {
            image: project
                .join("target")
                .join("debug")
                .join("retained_input_fixture.exe"),
            root,
        };
        std::fs::write(project.join("Cargo.toml"), "[package]\nname=\"retained_input_fixture\"\nversion=\"0.0.0\"\nedition=\"2024\"\nrust-version=\"1.85\"\n[lints.rust]\nunsafe_code=\"forbid\"\n[lints.clippy]\nunwrap_used=\"deny\"\nexpect_used=\"deny\"\npanic=\"deny\"\n").map_err(io_error)?;
        std::fs::write(
            project.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"retained_input_fixture\"\nversion = \"0.0.0\"\n",
        )
        .map_err(io_error)?;
        std::fs::write(
            project.join("src").join("main.rs"),
            include_str!("../../../tests/fixtures/retained_input.rs"),
        )
        .map_err(io_error)?;
        let cargo = std::env::var_os("CARGO")
            .ok_or_else(|| Error::type_error("Retained input fixture needs invoking Cargo"))?;
        for args in [
            vec![
                "clippy",
                "--all-targets",
                "--locked",
                "--offline",
                "--",
                "-D",
                "warnings",
            ],
            vec!["build", "--locked", "--offline"],
        ] {
            let mut command = Command::new(&cargo);
            command
                .current_dir(&project)
                .env_clear()
                .envs(tools::Host::default().environment)
                .env("CARGO_TARGET_DIR", project.join("target"))
                .env("CARGO_NET_OFFLINE", "true")
                .env("RUSTUP_AUTO_INSTALL", "0")
                .args(args);
            for name in ["CARGO_HOME", "RUSTUP_HOME", "RUSTFLAGS", "PATH"] {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
            if let Some(value) = std::env::var_os("ROSTER_TEST_CARGO_TOOLCHAIN") {
                command.env("RUSTUP_TOOLCHAIN", value);
            }
            execute(
                command,
                Limits {
                    timeout: Some(Duration::from_secs(90)),
                    max_output_bytes: 1024 * 1024,
                },
                None,
            )
            .await
            .map_err(|failure| Error::new("RetainedInputFixtureError", failure.to_string()))?;
        }
        Ok(fixture)
    }

    fn command(&self, mode: &str) -> Command {
        let mut command = Command::new(&self.image);
        command
            .current_dir(&self.root)
            .env_clear()
            .envs(tools::Host::default().environment)
            .arg(mode)
            .arg(&self.root);
        command
    }

    fn release(&self, name: &str) -> Result<(), Error> {
        std::fs::write(self.root.join(name), b"release").map_err(io_error)
    }

    async fn ready(&self, name: &str) -> Result<u64, Error> {
        let started = std::time::Instant::now();
        loop {
            match std::fs::read_to_string(self.root.join(name)) {
                Ok(value) => {
                    return value
                        .parse()
                        .map_err(|_| Error::type_error("Retained fixture PID must be a scalar"));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
            if started.elapsed() > Duration::from_secs(10) {
                return Err(Error::type_error(
                    "Retained input readiness barrier expired",
                ));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn finish(self) -> Result<(), Error> {
        std::fs::remove_dir_all(&self.root).map_err(io_error)
    }

    async fn lease(
        &self,
        source: &CancellationSource,
    ) -> Result<super::super::backend::RootLease, Error> {
        let git = std::env::var_os("ROSTER_TEST_GIT")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("PATH").and_then(|value| {
                    std::env::split_paths(&value)
                        .map(|path| path.join("git.exe"))
                        .find(|path| path.is_file())
                })
            })
            .ok_or_else(|| Error::type_error("Retained input lease needs installed Git"))?;
        let host = tools::Host {
            git: crate::host::Image::approve(&git)?.path().to_owned(),
            ..tools::Host::default()
        };
        std::fs::write(self.root.join(".gitignore"), ".roster/\nprovider/target/\n")
            .map_err(io_error)?;
        let command = host.git_command(
            &self.root,
            &self.root.join("lease-no-hooks"),
            &["init".to_owned(), "--quiet".to_owned()],
            &Default::default(),
        );
        output(execute(command, limits(), None).await)?;
        super::super::backend::RootLease::open(
            &self.root,
            "retained-input-proof",
            host,
            crate::state::redaction::Config::default(),
            source.token(),
        )
        .await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.exists() {
            eprintln!("Retained-input fixture preserved: {}", self.root.display());
        }
    }
}

fn io_error(error: io::Error) -> Error {
    Error::new("RetainedInputFixtureError", error.to_string())
}

fn limits() -> Limits {
    Limits {
        timeout: Some(Duration::from_secs(10)),
        max_output_bytes: 1024 * 1024,
    }
}

fn payload() -> Vec<u8> {
    (0_u8..=255).cycle().take(MAX_INPUT_BYTES).collect()
}

fn input(bytes: Vec<u8>) -> Result<(FiniteInput, Rc<InputTrace>), Error> {
    let mut input = FiniteInput::try_from(bytes)?;
    let trace = Rc::new(InputTrace::default());
    input.trace = Some(trace.clone());
    Ok((input, trace))
}

fn output(result: Result<Output, Failure>) -> Result<Output, Error> {
    result.map_err(|error| Error::new("RetainedInputControlError", error.to_string()))
}

fn failure(result: Result<Output, Failure>) -> Result<Failure, Error> {
    result
        .err()
        .ok_or_else(|| Error::type_error("Retained input control unexpectedly succeeded"))
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_heap_bound_and_precancellation_precede_native_submission()
-> Result<(), Error> {
    assert!(FiniteInput::try_from(vec![0; MAX_INPUT_BYTES]).is_ok());
    assert!(FiniteInput::try_from(vec![0; MAX_INPUT_BYTES + 1]).is_err());
    let submission = Submission::default();
    let invalid = FiniteInput {
        bytes: vec![0; MAX_INPUT_BYTES + 1],
        trace: None,
    };
    let observed = failure(
        execute_with_input(
            Command::new("retained-input-must-not-spawn.exe"),
            limits(),
            None,
            Some(&submission),
            Some(invalid),
        )
        .await,
    )?;
    assert!(
        !submission.attempted() && observed.captured.pid.is_none() && observed.cleanup_complete
    );
    let oversized = failure(
        execute_with_input(
            Command::new("retained-input-must-not-spawn.exe"),
            Limits {
                max_output_bytes: 32 * 1024 * 1024 + 1,
                ..limits()
            },
            None,
            Some(&submission),
            Some(FiniteInput::try_from(Vec::new())?),
        )
        .await,
    )?;
    assert!(!submission.attempted() && oversized.cleanup_complete);
    let source = CancellationSource::default();
    source.cancel(Reason::Cancelled)?;
    let stopped = failure(
        execute_with_input(
            Command::new("retained-input-must-not-spawn.exe"),
            limits(),
            Some(&source.token()),
            Some(&submission),
            Some(FiniteInput::try_from(Vec::new())?),
        )
        .await,
    )?;
    assert!(!submission.attempted() && stopped.cleanup_complete);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_after_kill_terminal_join_reclaims_original_handle_before_known_cleanup()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let token = source.token();
    let mut lease = fixture.lease(&source).await?;
    let (input, trace) = input(payload())?;
    let cancellation = async {
        let root = fixture.ready("ready").await?;
        let child = fixture.ready("descendant").await?;
        assert_eq!(trace.state.get(), InputState::Joining);
        assert_eq!(trace.accepted.get(), MAX_INPUT_BYTES);
        assert!(!trace.flush_terminal.get() && !trace.disposed.get());
        source.cancel(Reason::Cancelled)?;
        Ok::<_, Error>((root, child))
    };
    let (observed, pids) = tokio::join!(
        execute_with_input(
            fixture.command("hold"),
            limits(),
            Some(&token),
            None,
            Some(input)
        ),
        cancellation
    );
    let (root, child) = pids?;
    let observed = failure(observed)?;
    assert!(matches!(
        observed.kind,
        FailureKind::Cancelled(Reason::Cancelled)
    ));
    assert!(observed.cleanup_complete && trace.known_cleanup.get());
    assert_eq!(trace.state.get(), InputState::Closed { delivered: false });
    assert!(trace.flush_terminal.get() && trace.idle_original.get() && trace.disposed.get());
    assert!(trace.flush_error.get());
    assert_eq!(probe_pid(root)?, Liveness::Dead);
    assert_eq!(probe_pid(child)?, Liveness::Dead);
    lease.close().await?;
    drop(lease);
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_binary_maximum_and_empty_close_idle_original_for_eof() -> Result<(), Error>
{
    let fixture = Fixture::new().await?;
    for body in [Vec::new(), payload(), vec![0, 255, 1, 0, b'\n']] {
        let (input, trace) = input(body.clone())?;
        let observed = output(
            execute_with_input(fixture.command("binary"), limits(), None, None, Some(input)).await,
        )?;
        assert!(observed.captured.stdout == body && observed.captured.stderr == b"eof");
        assert_eq!(trace.accepted.get(), body.len());
        assert_eq!(trace.state.get(), InputState::Closed { delivered: true });
        assert!(trace.flush_terminal.get() && trace.idle_original.get() && trace.disposed.get());
    }
    let none = output(execute(fixture.command("binary"), limits(), None).await)?;
    assert!(none.captured.stdout.is_empty() && none.captured.stderr == b"eof");
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_full_duplex_drains_stdout_and_stderr_before_large_input_read()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let body = payload();
    let (input, trace) = input(body.clone())?;
    let observed = output(
        execute_with_input(
            fixture.command("output-first"),
            limits(),
            None,
            None,
            Some(input),
        )
        .await,
    )?;
    let mut expected = vec![b'O'; 128 * 1024];
    expected.extend_from_slice(&body);
    assert!(
        observed.captured.stdout == expected && observed.captured.stderr == vec![b'E'; 128 * 1024]
    );
    assert_eq!(trace.state.get(), InputState::Closed { delivered: true });
    assert!(trace.idle_original.get() && trace.disposed.get());
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_pending_deadline_kills_and_joins_without_replacing_first_reason()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&source).await?;
    let (input, trace) = input(payload())?;
    let observed = failure(
        execute_with_input(
            fixture.command("hold"),
            limits(),
            Some(lease.token()),
            None,
            Some(input),
        )
        .await,
    )?;
    assert!(matches!(observed.kind, FailureKind::Timeout));
    assert!(observed.cleanup_complete && trace.known_cleanup.get());
    assert_eq!(trace.accepted.get(), MAX_INPUT_BYTES);
    assert_eq!(trace.state.get(), InputState::Closed { delivered: false });
    assert!(trace.flush_terminal.get() && trace.idle_original.get() && trace.disposed.get());
    for marker in ["ready", "descendant"] {
        assert_eq!(probe_pid(fixture.ready(marker).await?)?, Liveness::Dead);
    }
    lease.close().await?;
    drop(lease);
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_overflow_keeps_first_reason_while_joining_busy_writer() -> Result<(), Error>
{
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&source).await?;
    let (input, trace) = input(payload())?;
    let release = async {
        fixture.ready("ready").await?;
        assert_eq!(trace.state.get(), InputState::Joining);
        assert!(!trace.flush_terminal.get());
        fixture.release("release")
    };
    let (observed, released) = tokio::join!(
        execute_with_input(
            fixture.command("overflow"),
            Limits {
                max_output_bytes: 64,
                ..limits()
            },
            Some(lease.token()),
            None,
            Some(input)
        ),
        release
    );
    released?;
    let observed = failure(observed)?;
    assert!(matches!(observed.kind, FailureKind::OutputLimit));
    assert_eq!(
        observed.captured.stdout.len() + observed.captured.stderr.len(),
        64
    );
    assert!(observed.cleanup_complete && trace.known_cleanup.get());
    assert!(trace.flush_terminal.get() && trace.idle_original.get() && trace.disposed.get());
    assert_eq!(trace.state.get(), InputState::Closed { delivered: false });
    for marker in ["ready", "descendant"] {
        assert_eq!(probe_pid(fixture.ready(marker).await?)?, Liveness::Dead);
    }
    lease.close().await?;
    drop(lease);
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_pinned_leader_exit_precedes_job_settlement_when_descendant_holds_pipes()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let (input, trace) = input(payload())?;
    let release = async {
        fixture.ready("ready").await?;
        assert_eq!(trace.state.get(), InputState::Joining);
        assert!(!trace.flush_terminal.get());
        fixture.release("release")
    };
    let (observed, released) = tokio::join!(
        execute_with_input(fixture.command("early"), limits(), None, None, Some(input)),
        release
    );
    released?;
    let observed = failure(observed)?;
    assert_eq!(observed.code(), Some("ROSTER_PROCESS_INPUT"));
    assert!(observed.cleanup_complete && trace.known_cleanup.get());
    assert!(trace.flush_terminal.get() && trace.idle_original.get() && trace.disposed.get());
    assert_eq!(trace.state.get(), InputState::Closed { delivered: false });
    for marker in ["ready", "descendant"] {
        assert_eq!(probe_pid(fixture.ready(marker).await?)?, Liveness::Dead);
    }
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_early_exit_zero_is_not_complete_input_success() -> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let (input, trace) = input(payload())?;
    let observed = failure(
        execute_with_input(fixture.command("exit"), limits(), None, None, Some(input)).await,
    )?;
    assert_eq!(observed.code(), Some("ROSTER_PROCESS_INPUT"));
    assert!(observed.cleanup_complete && trace.known_cleanup.get());
    assert_eq!(trace.state.get(), InputState::Closed { delivered: false });
    assert!(trace.flush_terminal.get() && trace.idle_original.get() && trace.disposed.get());
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_completed_eof_and_output_still_require_actual_job_settlement()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let body = vec![0, 1, 255, 0];
    let (input, trace) = input(body.clone())?;
    let mut operation = Box::pin(execute_with_input(
        fixture.command("consume-descendant"),
        limits(),
        None,
        None,
        Some(input),
    ));
    let root = tokio::select! {
        _=&mut operation=>return Err(Error::type_error("Input returned before descendant readiness")),
        ready=fixture.ready("ready")=>ready?
    };
    let child = fixture.ready("descendant").await?;
    let exited = async {
        let start = std::time::Instant::now();
        while probe_pid(root)? != Liveness::Dead {
            if start.elapsed() > Duration::from_secs(5) {
                return Err(Error::type_error("Original leader did not exit"));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok::<_, Error>(())
    };
    tokio::select! {
        _=&mut operation=>return Err(Error::type_error("Input accepted before actual descendant settlement")),
        result=exited=>result?
    }
    assert_eq!(trace.state.get(), InputState::Closed { delivered: true });
    assert!(trace.idle_original.get() && trace.disposed.get());
    assert_eq!(probe_pid(child)?, Liveness::Alive);
    fixture.release("finish-child")?;
    let observed = output(operation.await)?;
    assert!(observed.captured.stdout == body);
    assert_eq!(probe_pid(child)?, Liveness::Dead);
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_late_success_cannot_replace_latched_cancellation() -> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let token = source.token();
    let (input, trace) = input(vec![0, 1, 0])?;
    let cancel = async {
        fixture.ready("ready").await?;
        assert_eq!(trace.state.get(), InputState::Closed { delivered: true });
        source.cancel(Reason::Cancelled)?;
        fixture.release("release")
    };
    let (observed, cancelled) = tokio::join!(
        execute_with_input(
            fixture.command("late"),
            limits(),
            Some(&token),
            None,
            Some(input)
        ),
        cancel
    );
    cancelled?;
    let observed = failure(observed)?;
    assert!(matches!(
        observed.kind,
        FailureKind::Cancelled(Reason::Cancelled)
    ));
    assert!(observed.cleanup_complete && trace.known_cleanup.get() && trace.disposed.get());
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_known_cleanup_allows_actual_writer_lease_release() -> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&source).await?;
    let (input, trace) = input(payload())?;
    let observed = output(
        execute_with_input(
            fixture.command("binary"),
            limits(),
            Some(lease.token()),
            None,
            Some(input),
        )
        .await,
    )?;
    assert!(observed.status.success() && trace.disposed.get() && trace.idle_original.get());
    lease.check()?;
    lease.close().await?;
    drop(lease);
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_pending_future_drop_quarantines_real_writer_even_after_child_death()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let token = source.token();
    let mut lease = fixture.lease(&source).await?;
    let guard = super::super::backend::input_test_pending_owner(
        super::super::backend::Owner::PrePlan(&lease),
    );
    let submission = Submission::default();
    let (input, trace) = input(payload())?;
    let mut operation = Box::pin(execute_with_input(
        fixture.command("hold"),
        limits(),
        Some(&token),
        Some(&submission),
        Some(input),
    ));
    let root = tokio::select! {
        _=&mut operation=>return Err(Error::type_error("Pending input returned before abandonment")),
        ready=fixture.ready("ready")=>ready?
    };
    let child = fixture.ready("descendant").await?;
    assert_eq!(trace.state.get(), InputState::Joining);
    assert!(submission.attempted());
    source.cancel(Reason::Cancelled)?;
    drop(operation);
    drop(guard);
    assert_eq!(trace.state.get(), InputState::Unknown);
    assert!(!trace.known_cleanup.get() && !trace.disposed.get());
    assert!(lease.check().is_err() && lease.close().await.is_err());
    let start = std::time::Instant::now();
    while [root, child]
        .into_iter()
        .any(|pid| probe_pid(pid).ok() != Some(Liveness::Dead))
    {
        if start.elapsed() > Duration::from_secs(5) {
            return Err(Error::type_error("Abandoned input job has not stopped"));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(lease.close().await.is_err());
    assert_eq!(trace.state.get(), InputState::Unknown);
    drop(lease);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_unpolled_future_has_no_submission_but_no_writer_release()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&source).await?;
    let guard = super::super::backend::input_test_pending_owner(
        super::super::backend::Owner::PrePlan(&lease),
    );
    let submission = Submission::default();
    let operation = execute_with_input(
        fixture.command("binary"),
        limits(),
        None,
        Some(&submission),
        Some(FiniteInput::try_from(Vec::new())?),
    );
    drop(operation);
    drop(guard);
    assert!(!submission.attempted());
    assert!(lease.close().await.is_err());
    drop(lease);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_drop_after_eof_before_job_exit_still_quarantines_actual_writer()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&source).await?;
    let guard = super::super::backend::input_test_pending_owner(
        super::super::backend::Owner::PrePlan(&lease),
    );
    let (input, trace) = input(vec![0, 255, 0])?;
    let mut operation = Box::pin(execute_with_input(
        fixture.command("consume-descendant"),
        limits(),
        None,
        None,
        Some(input),
    ));
    tokio::select! {
        _ = &mut operation => return Err(Error::type_error("Input returned before abandonment barrier")),
        ready = fixture.ready("ready") => { ready?; }
    }
    let child = fixture.ready("descendant").await?;
    assert_eq!(trace.state.get(), InputState::Closed { delivered: true });
    assert!(trace.disposed.get() && trace.idle_original.get());
    assert_eq!(probe_pid(child)?, Liveness::Alive);
    drop(operation);
    drop(guard);
    assert!(!trace.known_cleanup.get());
    assert!(lease.close().await.is_err());
    let start = std::time::Instant::now();
    while probe_pid(child)? != Liveness::Dead {
        if start.elapsed() > Duration::from_secs(5) {
            return Err(Error::type_error(
                "Abandoned post-EOF descendant did not stop",
            ));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(lease.close().await.is_err());
    drop(lease);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn retained_input_late_terminal_join_does_not_unquarantine_an_abandoned_real_writer()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&source).await?;
    let guard = super::super::backend::input_test_pending_owner(
        super::super::backend::Owner::PrePlan(&lease),
    );
    let mut command = fixture.command("hold");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = windows::Child::spawn(command)
        .map_err(|error| Error::new("RetainedInputControlError", error.to_string()))?;
    let stdin = child
        .inner_mut()
        .stdin
        .take()
        .ok_or_else(|| Error::type_error("Original input pipe missing"))?;
    let (input, trace) = input(payload())?;
    let mut controller = InputController::new(stdin, input);
    let mut operation = Box::pin(controller.complete());
    tokio::select! {
        _=&mut operation=>return Err(Error::type_error("Pending writer joined before abandonment")),
        ready=fixture.ready("ready")=>{ready?;}
    }
    drop(operation);
    drop(guard);
    assert!(lease.close().await.is_err());
    controller.latch_abort();
    let (job, joined) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(biased;child.stop(),controller.join_aborted())
    })
    .await
    .map_err(|_| Error::type_error("Late input join cleanup expired"))?;
    job.map_err(io_error)?;
    joined?;
    assert!(trace.flush_terminal.get() && trace.idle_original.get() && trace.disposed.get());
    assert_eq!(trace.state.get(), InputState::Closed { delivered: false });
    assert!(lease.close().await.is_err());
    drop(child);
    drop(controller);
    drop(lease);
    Ok(())
}

#[test]
fn retained_input_unknown_cleanup_and_terminal_error_preserve_initiating_reason() {
    for cleanup in [
        Err(()),
        Ok((
            Err(io::Error::from_raw_os_error(5)),
            Ok(JoinedInput {
                terminal_error: None,
            }),
        )),
        Ok((Ok(()), Err(Error::new("OwnedInputCleanupError", "unknown")))),
    ] {
        let observed = input_cleanup_result(
            FailureKind::Cancelled(Reason::Cancelled),
            None,
            Captured::default(),
            cleanup,
        );
        assert!(matches!(observed.kind, FailureKind::Cleanup));
        assert!(!observed.cleanup_complete);
        assert!(
            observed
                .source
                .is_some_and(|error| error.to_string().contains("ROSTER_CANCELLED")
                    && error.to_string().len() < 256)
        );
    }

    for (kind, code) in [
        (FailureKind::Timeout, "ETIMEDOUT"),
        (
            FailureKind::OutputLimit,
            "ERR_CHILD_PROCESS_STDIO_MAXBUFFER",
        ),
    ] {
        let observed = input_cleanup_result(
            kind,
            None,
            Captured::default(),
            Ok((
                Ok(()),
                Ok(JoinedInput {
                    terminal_error: Some(io::Error::from_raw_os_error(232)),
                }),
            )),
        );
        assert_eq!(observed.code(), Some(code));
        assert!(observed.cleanup_complete);
        assert_eq!(
            observed.source.and_then(|error| error.raw_os_error()),
            Some(232)
        );
    }
}

fn measured_count(snapshot: &crate::json::Value, label: &str) -> Option<f64> {
    match snapshot.get("aggregates") {
        Some(crate::json::Value::Array(values)) => values
            .iter()
            .find(|value| value.get("label") == Some(&crate::json::Value::string(label)))
            .and_then(|value| value.get("count"))
            .and_then(crate::json::Value::as_number),
        _ => None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn passive_owned_noinput_observes_real_spawn_drain_and_existing_kernel_settlement()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let timings = timing::Timings::default();
    let parent = timings.start(Label::Local);
    let observed = output(
        execute_observed_timed(
            fixture.command("output-first"),
            limits(),
            None,
            None,
            Some(&parent),
        )
        .await,
    )?;
    assert!(observed.captured.stdout == vec![b'O'; 128 * 1024]);
    assert!(observed.captured.stderr == vec![b'E'; 128 * 1024]);
    parent.finish(None);
    let snapshot = timings.snapshot();
    for label in [
        "owned.noinput.execute",
        "noinput.caller-dispatch",
        "noinput.preflight",
        "noinput.spawn.sync",
        "noinput.leader-observed",
        "noinput.job-zero-observed",
        "noinput.members-settled-observed",
        "noinput.stdout.drain",
        "noinput.stderr.drain",
        "noinput.stdout.first-byte-observed",
        "noinput.stderr.first-byte-observed",
    ] {
        assert_eq!(measured_count(&snapshot, label), Some(1.0), "{label}");
    }
    assert!(
        !snapshot
            .json()
            .contains(&fixture.root.display().to_string())
    );
    let unmeasured = output(execute(fixture.command("output-first"), limits(), None).await)?;
    assert!(observed.captured.stdout == unmeasured.captured.stdout);
    assert!(observed.captured.stderr == unmeasured.captured.stderr);
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn passive_owned_noinput_does_not_turn_leader_observation_into_job_completion()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let timings = timing::Timings::default();
    let parent = timings.start(Label::Local);
    let mut operation = Box::pin(execute_observed_timed(
        fixture.command("consume-descendant"),
        limits(),
        None,
        None,
        Some(&parent),
    ));
    let child = tokio::select! {
        _ = &mut operation => return Err(Error::type_error("Observed input-free job returned before its barrier")),
        ready = fixture.ready("ready") => { ready?; fixture.ready("descendant").await? }
    };
    let started = std::time::Instant::now();
    while measured_count(&timings.snapshot(), "noinput.leader-observed") != Some(1.0) {
        tokio::select! {
            _ = &mut operation => return Err(Error::type_error("Observed input-free job accepted a live descendant")),
            () = tokio::time::sleep(Duration::from_millis(5)) => {}
        }
        if started.elapsed() > Duration::from_secs(5) {
            return Err(Error::type_error(
                "Existing leader observation did not arrive",
            ));
        }
    }
    assert_eq!(probe_pid(child)?, Liveness::Alive);
    assert_eq!(
        measured_count(&timings.snapshot(), "noinput.members-settled-observed"),
        None
    );
    fixture.release("finish-child")?;
    output(operation.await)?;
    parent.finish(None);
    assert_eq!(
        measured_count(&timings.snapshot(), "noinput.members-settled-observed"),
        Some(1.0)
    );
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn passive_direct_noinput_observes_existing_wait_without_inventing_job_authority()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let timings = timing::Timings::default();
    let parent = timings.start(Label::Local);
    let mut command = fixture.command("output-first");
    let observed =
        output(crate::process::execute_timed(&mut command, limits(), None, Some(&parent)).await)?;
    assert!(observed.captured.stdout == vec![b'O'; 128 * 1024]);
    assert!(observed.captured.stderr == vec![b'E'; 128 * 1024]);
    parent.finish(None);
    let snapshot = timings.snapshot();
    assert_eq!(
        measured_count(&snapshot, "direct.noinput.execute"),
        Some(1.0)
    );
    assert_eq!(measured_count(&snapshot, "noinput.spawn.sync"), Some(1.0));
    assert_eq!(
        measured_count(&snapshot, "noinput.leader-observed"),
        Some(1.0)
    );
    assert_eq!(measured_count(&snapshot, "noinput.job-zero-observed"), None);
    assert_eq!(
        measured_count(&snapshot, "noinput.members-settled-observed"),
        None
    );
    fixture.finish()
}
