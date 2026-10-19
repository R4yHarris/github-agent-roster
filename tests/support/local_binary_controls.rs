use super::*;
use std::process::Stdio;
use std::time::{Duration, Instant};

#[cfg(windows)]
#[test]
fn actual_cli_whole_task_deadline_uses_native_cancellation_and_never_returns_a_passing_stub()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    let mut environment = fixture.environment()?;
    put(
        &mut environment,
        "ROSTER_TASK_TIMEOUT_MS",
        Value::string("1"),
    );
    let started = Instant::now();
    let result = fixture.invoke_with(
        &["run", "--seat", "coder", "--runtime", "builtin"],
        &environment,
        &fixture.root,
    )?;
    assert_eq!(result.status.code(), Some(124), "{}", text(&result.stderr)?);
    assert!(result.stdout.is_empty());
    assert!(text(&result.stderr)?.contains("wall-clock budget exhausted after 1 ms"));
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!fixture.root.join("RESULT.md").exists());
    assert!(!fixture.root.join("REVIEW.md").exists());
    assert!(
        !fixture
            .root
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn actual_cli_crash_retains_partial_artifacts_and_only_a_dead_process_proof_reclaims_its_writer()
-> Result<(), Error> {
    use github_agent_roster::state::locks::{self, Liveness};
    use winsafe::{HPROCESS, co, prelude::*};

    let fixture = Fixture::new()?;
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    let server = support::Server::pending()?;
    fixture.configure(Some(&server.endpoint))?;
    let mut command = fixture.command(
        &["run", "--seat", "coder", "--runtime", "builtin"],
        &fixture.environment()?,
        &fixture.root,
    )?;
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = support::OwnedCli(Some(command.spawn().map_err(io)?));
    let pid = child
        .0
        .as_ref()
        .ok_or_else(|| Error::type_error("Owned CLI did not spawn"))?
        .id();
    let handle = HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    )
    .map_err(|error| Error::new("FixtureError", error.to_string()))?;
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let ready = !server
                .requests
                .lock()
                .map_err(|_| Error::type_error("Loopback readiness lock poisoned"))?
                .is_empty();
            if ready {
                return Ok::<_, Error>(());
            }
            if let Some(status) = child
                .0
                .as_mut()
                .ok_or_else(|| Error::type_error("Owned CLI vanished"))?
                .try_wait()
                .map_err(io)?
            {
                return Err(Error::new(
                    "FixtureError",
                    format!("CLI exited before actual model readiness: {status}"),
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| Error::type_error("Actual native CLI did not reach model-body readiness"))??;
    assert_eq!(locks::probe_pid(u64::from(pid))?, Liveness::Alive);
    let lock = fixture.root.join(".roster").join("locks").join("run.lock");
    assert!(lock.is_file());
    let context = fs::read(fixture.root.join("CONTEXT.md")).map_err(io)?;
    child
        .0
        .as_mut()
        .ok_or_else(|| Error::type_error("Owned CLI vanished"))?
        .kill()
        .map_err(io)?;
    let output = child
        .0
        .take()
        .ok_or_else(|| Error::type_error("Owned CLI handle vanished"))?
        .wait_with_output()
        .map_err(io)?;
    assert!(!output.status.success());
    assert_eq!(
        handle
            .WaitForSingleObject(Some(0))
            .map_err(|error| Error::new("FixtureError", error.to_string()))?,
        co::WAIT::OBJECT_0
    );
    assert_eq!(locks::probe_pid(u64::from(pid))?, Liveness::Dead);
    assert!(lock.is_file());
    assert!(!fixture.root.join("RESULT.md").exists());
    assert!(!fixture.root.join("REVIEW.md").exists());
    let source = CancellationSource::default();
    let (host, _) = fixture.host()?;
    let mut replacement = backend::RootLease::open(
        &fixture.root,
        "owned-after-real-crash",
        host,
        Default::default(),
        source.token(),
    )
    .await?;
    assert!(lock.is_file());
    assert_eq!(
        fs::read(fixture.root.join("CONTEXT.md")).map_err(io)?,
        context
    );
    replacement.close().await?;
    assert!(!lock.exists());
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[cfg(windows)]
#[test]
fn local_ask_stub_is_real_offline_worktree_preparation_not_a_demo_copy_or_a_fake_passing_review()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.write("ASK.md", "Update the guide with the requested bounded outcome.\n\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide documents the requested bounded outcome.\n")?;
    let result = fixture.invoke(&[
        "run",
        "--ask-file",
        "ASK.md",
        "--runtime",
        "builtin",
        "--no-fetch",
    ])?;
    assert!(
        result.status.success(),
        "{}\n{}",
        text(&result.stdout)?,
        text(&result.stderr)?
    );
    let output = text(&result.stdout)?;
    assert!(output.contains("Mode: stub\n") && output.contains("no passing review"));
    let worktree = Path::new(
        output
            .lines()
            .find_map(|line| line.strip_prefix("Worktree: "))
            .ok_or_else(|| Error::type_error("Offline Ask returned no worktree"))?,
    );
    assert!(worktree.starts_with(fixture.root.join(".worktrees")));
    assert!(worktree.join(".git").is_file());
    assert_eq!(
        fs::read_to_string(worktree.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    assert!(
        fs::read_to_string(worktree.join("RESULT.md"))
            .map_err(io)?
            .contains("stub")
    );
    assert!(
        fs::read_to_string(worktree.join("RESULT.md"))
            .map_err(io)?
            .contains("AI-Run: not emitted for a deterministic stub.\n")
    );
    assert!(
        !fs::read_to_string(worktree.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: pass")
    );
    assert!(!fixture.root.join(".roster").join("memory").exists());
    assert!(
        !worktree
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn local_start_never_materializes_retired_node_roster_or_treats_a_missing_approved_image_as_path_authority()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.write("package.json", "{\"name\":\"github-agent-roster\"}\n")?;
    fixture.git(&["add", "package.json"])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned retired-package guard fixture",
    ])?;
    fixture.write("ASK.md", "Update docs/guide.md.\n\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide documents the requested bounded outcome.\n")?;
    let result = fixture.invoke(&[
        "run",
        "--ask-file",
        "ASK.md",
        "--runtime",
        "builtin",
        "--no-fetch",
    ])?;
    assert!(!result.status.success());
    assert!(text(&result.stderr)?.contains("retired Node Roster"));
    assert!(!fixture.root.join(".worktrees").exists());
    let mut environment = fixture.environment()?;
    if let Value::Object(values) = &mut environment {
        values.retain(|(key, _)| key != &Text::from("ROSTER_GIT"));
    }
    let result = fixture.invoke_with(
        &["run", "--seat", "coder", "--runtime", "builtin"],
        &environment,
        &fixture.root,
    )?;
    assert!(!result.status.success());
    assert!(text(&result.stderr)?.contains("ROSTER_GIT"));
    assert!(!fixture.root.join(".roster").join("locks").exists());
    Ok(())
}

#[test]
fn explicit_local_submodule_host_configuration_is_bounded_typed_and_never_an_implicit_url_grant()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let mut environment = fixture.environment()?;
    let source = fixture
        .container
        .to_str()
        .ok_or_else(|| Error::type_error("Fixture source must be Unicode"))?;
    let selected = Value::Array(vec![Value::object(vec![
        ("path", Value::string("vendor/owned")),
        ("source", Value::string(source)),
    ])]);
    put(
        &mut environment,
        "ROSTER_LOCAL_SUBMODULES",
        Value::string(&selected.json()),
    );
    let (_, approval) = github_agent_roster::host::invocation_host(
        &environment,
        Path::new(env!("CARGO_BIN_EXE_roster")),
    )?;
    assert_eq!(approval.submodules.len(), 1);
    assert_eq!(
        approval
            .submodules
            .first()
            .map(|module| module.source.as_path()),
        Some(fixture.container.as_path())
    );
    for invalid in [
        " ",
        "not-json",
        "[{\"path\":\"vendor/owned\",\"source\":\"https://unapproved.invalid\"}]",
        "[{\"path\":\"../outside\",\"source\":\"relative\"}]",
        "[{\"path\":\"vendor/owned\",\"path\":\"vendor/other\"}]",
    ] {
        put(
            &mut environment,
            "ROSTER_LOCAL_SUBMODULES",
            Value::string(invalid),
        );
        assert!(
            github_agent_roster::host::invocation_host(
                &environment,
                Path::new(env!("CARGO_BIN_EXE_roster"))
            )
            .is_err()
        );
    }
    assert!(!fixture.root.join(".worktrees").exists());
    assert!(!fixture.root.join(".roster").join("locks").exists());
    Ok(())
}
