use super::*;
use crate::cancel::CancellationSource;

fn failure(result: Result<Output, Failure>) -> Result<Failure, Error> {
    match result {
        Err(error) => Ok(error),
        Ok(_) => Err(Error::new(
            "Error",
            "A nonexecuting native submission control succeeded",
        )),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn only_native_spawn_entry_marks_possible_submission_and_missing_pid_never_proves_absence()
-> Result<(), Error> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        ".docker-owned-missing-{}.exe",
        crate::state::files::nonce()?
    ));
    let invalid = Submission::default();
    let rejected = failure(
        execute_observed(
            Command::new(&path),
            Limits {
                timeout: Some(Duration::from_secs(1)),
                max_output_bytes: 32 * 1024 * 1024 + 1,
            },
            None,
            Some(&invalid),
        )
        .await,
    )?;
    assert!(!invalid.attempted());
    assert!(rejected.captured.pid.is_none());
    assert!(rejected.cleanup_complete);
    let source = CancellationSource::default();
    source.cancel(crate::cancel::Reason::Cancelled)?;
    let cancelled = Submission::default();
    let stopped = failure(
        execute_observed(
            Command::new(&path),
            Limits {
                timeout: Some(Duration::from_secs(1)),
                max_output_bytes: 1024,
            },
            Some(&source.token()),
            Some(&cancelled),
        )
        .await,
    )?;
    assert!(!cancelled.attempted());
    assert!(matches!(
        stopped.kind,
        FailureKind::Control(_) | FailureKind::Cancelled(_)
    ));
    let attempted = Submission::default();
    let spawned = failure(
        execute_observed(
            Command::new(&path),
            Limits {
                timeout: Some(Duration::from_secs(1)),
                max_output_bytes: 1024,
            },
            None,
            Some(&attempted),
        )
        .await,
    )?;
    assert!(matches!(spawned.kind, FailureKind::Spawn));
    assert!(attempted.attempted());
    assert!(attempted.pid.get().is_none());
    assert_eq!(
        attempted.snapshot(std::time::Instant::now()).get("state"),
        Some(&crate::json::Value::string("may-have-executed"))
    );
    Ok(())
}
