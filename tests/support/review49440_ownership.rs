use super::*;

fn phase_pids(path: &Path) -> Result<Vec<u32>, Error> {
    let snapshot =
        secure::read(path, 4096)?.ok_or_else(|| Error::type_error("Owned phase marker missing"))?;
    let value = github_agent_roster::json::parse(&Text::from(text(&snapshot.bytes)?))?;
    ["pid", "child"]
        .iter()
        .map(|name| {
            value
                .get(name)
                .and_then(Value::as_number)
                .filter(|pid| pid.fract() == 0.0 && (1.0..=f64::from(u32::MAX)).contains(pid))
                .map(|pid| pid as u32)
                .ok_or_else(|| Error::type_error("Owned phase process identity missing"))
        })
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_clone_and_checkout_keep_caller_lease_until_real_owned_outcomes_settle()
-> Result<(), Error> {
    use winsafe::{HPROCESS, co, prelude::*};
    for phase in ["clone", "checkout"] {
        for mode in ["success", "exit-fault", "cancel", "drop"] {
            let fixture = Fixture::new()?;
            let module = module(&fixture)?;
            let proxy = proxy(&fixture, phase)?;
            let loaded = fixture.loaded()?;
            let (mut host, mut approval) = fixture.host()?;
            host.git = proxy.image;
            approval.submodules = vec![LocalSubmodule {
                path: "vendor/owned".to_owned(),
                source: module,
            }];
            let paths = paths::repository_paths(&fixture.root)?;
            let git = start::Git::new(host.clone(), paths.common_dir.join("review49440-no-hooks"))?;
            let source = CancellationSource::default();
            let token = source.token();
            let repository = start::Repository::inspect(&fixture.root, &git, &token).await?;
            let selected = start::resolve(
                &repository,
                &git,
                &start::Options {
                    base: "current".to_owned(),
                    sync: start::Sync::Offline,
                },
                &start::FetchPolicy::Deny,
                &token,
            )
            .await?;
            let mut preparation = start::Preparation::default();
            let mut prepare = Box::pin(start::prepare_local(
                &repository,
                &git,
                &loaded.config,
                selected,
                &approval,
                Default::default(),
                token,
                &mut preparation,
            ));
            let marker = proxy.control.join(format!("{phase}.ready"));
            tokio::select! {
                result = ready(&marker) => { result?; }
                result = &mut prepare => return Err(Error::new("FixtureError", format!("Preparation ended before real {phase} {mode}: {:?}", result.err()))),
            }
            let handles = phase_pids(&marker)?
                .into_iter()
                .map(|pid| {
                    HPROCESS::OpenProcess(
                        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
                        false,
                        pid,
                    )
                    .map_err(|error| Error::new("FixtureError", error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            for handle in &handles {
                assert_eq!(
                    handle
                        .WaitForSingleObject(Some(0))
                        .map_err(|error| Error::new("FixtureError", error.to_string()))?,
                    co::WAIT::TIMEOUT
                );
            }
            let result = if mode == "drop" {
                source.cancel(github_agent_roster::cancel::Reason::Cancelled)?;
                drop(prepare);
                None
            } else {
                if mode == "cancel" {
                    source.cancel(github_agent_roster::cancel::Reason::Cancelled)?;
                } else {
                    if mode == "exit-fault" {
                        fs::write(
                            proxy.control.join(format!("{phase}.fault")),
                            "owned exit fault",
                        )
                        .map_err(io)?;
                    }
                    fs::write(
                        proxy.control.join(format!("{phase}.release")),
                        "owned phase release",
                    )
                    .map_err(io)?;
                }
                Some(prepare.await)
            };
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let mut stopped = true;
                    for handle in &handles {
                        stopped &= handle
                            .WaitForSingleObject(Some(0))
                            .map_err(|error| Error::new("FixtureError", error.to_string()))?
                            == co::WAIT::OBJECT_0;
                    }
                    if stopped {
                        return Ok::<_, Error>(());
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .map_err(|_| {
                Error::type_error("Real phase process/descendant cleanup was not observed")
            })??;
            let root = preparation.lease()?.root().to_path_buf();
            let lock = preparation.lease()?.ownership().path.clone();
            assert!(root.starts_with(fixture.root.join(".worktrees")));
            assert!(
                lock.is_file(),
                "Caller-owned run.lock disappeared before {phase} {mode} ownership settled"
            );
            match (mode, result) {
                ("success", Some(Ok(prepared))) => assert_eq!(prepared.worktree, root),
                ("exit-fault", Some(Err(error))) => {
                    assert_eq!(error.code, Some("ROSTER_GIT_FAILED"))
                }
                ("cancel", Some(Err(error))) => assert_eq!(error.code, Some("ROSTER_CANCELLED")),
                ("drop", None) => {}
                (_, result) => {
                    return Err(Error::new(
                        "FixtureError",
                        format!(
                            "Unexpected real phase outcome for {mode}: {:?}",
                            result.and_then(Result::err)
                        ),
                    ));
                }
            }
            if mode == "drop" {
                assert_eq!(
                    preparation.close().await.err().and_then(|error| error.code),
                    Some("ROSTER_PROCESS_CLEANUP")
                );
                assert!(
                    lock.is_file(),
                    "Kernel termination incorrectly cleared retained uncertainty"
                );
                let second = backend::RootLease::open(
                    &root,
                    "foreign-review49440",
                    host,
                    Default::default(),
                    CancellationSource::default().token(),
                )
                .await;
                assert_eq!(
                    second.err().and_then(|error| error.code),
                    Some("E_LOCK_HELD")
                );
            } else {
                preparation.close().await?;
                assert!(!lock.exists());
            }
        }
    }
    Ok(())
}
