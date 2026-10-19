use super::*;
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::Poll;
use std::time::Duration;

#[tokio::test(flavor = "current_thread")]
async fn dropping_pending_native_metadata_keeps_git_uncertainty_in_the_callers_ownership()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    let loaded = fixture.loaded()?;
    let (host, approval) = fixture.host()?;
    let mut transport = support::Scripted::new(Vec::new());
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let admission = Admission::default();
    let source = CancellationSource::default();
    let mut ownership = delivery::Ownership::default();
    let mut driver = delivery::Driver {
        transport: &mut transport,
        observer: &mut observer,
        credentials: &credentials,
        admission: &admission,
    };
    let mut work = Box::pin(driver.run_local(
        delivery::Options {
            cwd: &fixture.root,
            installation: Path::new(env!("CARGO_MANIFEST_DIR")),
            loaded: &loaded,
            input: Input::ExistingTask,
            host,
            approval,
            start: start::Options::resolve(&loaded.config, None, None)?,
            fetch: start::FetchPolicy::Deny,
            skip_review: false,
        },
        source.token(),
        &mut ownership,
    ));
    std::future::poll_fn(|context| match work.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(Ok(())),
        Poll::Ready(result) => Poll::Ready(Err(Error::new(
            "FixtureError",
            format!(
                "Metadata completed without a pending owned operation: {:?}",
                result.err()
            ),
        ))),
    })
    .await?;
    source.cancel(github_agent_roster::cancel::Reason::Cancelled)?;
    drop(work);
    assert!(ownership.root().is_some());
    assert_eq!(
        ownership.close().await.err().and_then(|error| error.code),
        Some("ROSTER_PROCESS_CLEANUP")
    );
    assert_eq!(transport.requests.len(), 0);
    assert!(!fixture.root.join("RESULT.md").exists());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn no_awaited_dependency_window_follows_the_final_private_review_and_completion_event()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.configure(Some("http://127.0.0.1:1/v1"))?;
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    let loaded = fixture.loaded()?;
    let (host, approval) = fixture.host()?;
    let mut transport = support::Scripted::new(support::docs_script(true));
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let admission = Admission::default();
    let source = CancellationSource::default();
    let complete = Arc::new(AtomicBool::new(false));
    let finished = complete.clone();
    let root = fixture.root.clone();
    let watcher = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(600), async {
            loop {
                if finished.load(Ordering::SeqCst) {
                    return Ok::<_, Error>(false);
                }
                if let Ok(entries) = fs::read_dir(root.join(".roster").join("runs")) {
                    for entry in entries {
                        let entry = entry.map_err(io)?;
                        if let Some(snapshot) = secure::read(&entry.path(), 2 * 1024 * 1024)? {
                            let body = text(&snapshot.bytes)?;
                            if body.contains("\"event\":\"invocation-end\"")
                                && body.contains("\"status\":\"verified\"")
                            {
                                if finished.load(Ordering::SeqCst) {
                                    return Ok(false);
                                }
                                fs::write(
                                    root.join("docs").join("guide.md"),
                                    "human source in an awaited post-completion gap\n",
                                )
                                .map_err(io)?;
                                return Ok(true);
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .map_err(|_| {
            Error::type_error("Owned completion-window observer exceeded its workflow guard")
        })?
    });
    let mut ownership = delivery::Ownership::default();
    let mut driver = delivery::Driver {
        transport: &mut transport,
        observer: &mut observer,
        credentials: &credentials,
        admission: &admission,
    };
    let result = Box::pin(driver.run_local(
        delivery::Options {
            cwd: &fixture.root,
            installation: Path::new(env!("CARGO_MANIFEST_DIR")),
            loaded: &loaded,
            input: Input::ExistingTask,
            host,
            approval,
            start: start::Options::resolve(&loaded.config, None, None)?,
            fetch: start::FetchPolicy::Deny,
            skip_review: false,
        },
        source.token(),
        &mut ownership,
    ))
    .await;
    complete.store(true, Ordering::SeqCst);
    let changed_before_completion = watcher
        .await
        .map_err(|_| Error::type_error("Owned source-window observer failed"))??;
    let result = result?;
    assert_eq!(result.status(), Status::Verified, "{:?}", result.failure);
    assert!(
        !changed_before_completion,
        "Source arrived after the declared terminal gate but before the invocation actually completed"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nThe requested bounded outcome is documented.\n"
    );
    assert!(ownership.root().is_none());
    Ok(())
}
