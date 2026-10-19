use super::*;

fn git_at(root: &Path, git: &Path, args: &[&str]) -> Result<String, Error> {
    let output = std::process::Command::new(git)
        .current_dir(root)
        .env_clear()
        .envs(support::minimal()?)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["--no-pager", "-c", "core.fsmonitor=false"])
        .args(args)
        .output()
        .map_err(io)?;
    if !output.status.success() {
        return Err(Error::new(
            "OwnedGitControlError",
            text(&output.stderr)?.to_owned(),
        ));
    }
    text(&output.stdout).map(str::to_owned)
}

struct BranchSwitch {
    root: PathBuf,
    git: PathBuf,
    per_tool: bool,
    changed: bool,
}

impl Observer for BranchSwitch {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        let selected = if self.per_tool {
            event.get("type") == Some(&Value::string("seat-tool"))
                && event.get("name") == Some(&Value::string("edit_file"))
        } else {
            event.get("type") == Some(&Value::string("seat"))
                && event.get("status") == Some(&Value::string("finalizing"))
        };
        if !self.changed && selected {
            git_at(
                &self.root,
                &self.git,
                &["symbolic-ref", "HEAD", "refs/heads/owned-alternative"],
            )?;
            self.changed = true;
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn c5bc_finalizing_await_cannot_replace_original_branch_with_same_commit() -> Result<(), Error>
{
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    fixture.git(&["branch", "owned-alternative"])?;
    let original = fixture.git(&["rev-parse", "HEAD"])?;
    let server = support::Server::start_for(&fixture, support::docs_script(true))?;
    configure(&fixture, &server.endpoint, &[])?;
    let mut observer = BranchSwitch {
        root: fixture.root.clone(),
        git: fixture.git.clone(),
        per_tool: false,
        changed: false,
    };
    let outcome = Box::pin(local(
        &fixture,
        &mut observer,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    let requests = server.finish()?;
    eprintln!(
        "c5bc finalizing root={} observed={} status={} handoff={:?}",
        fixture.root.display(),
        requests.len(),
        outcome.status().as_str(),
        outcome.coder_handoff
    );
    assert!(observer.changed);
    assert_eq!(fixture.git(&["rev-parse", "HEAD"])?, original);
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::PolicyDenied));
    assert!(outcome.reviewer_run.is_none());
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert_eq!(requests.len(), 3);
    assert!(
        !fs::read_to_string(fixture.root.join("RESULT.md"))
            .map_err(io)?
            .contains("Checks: PASS")
    );
    assert!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md"))
            .map_err(io)?
            .contains("requested bounded outcome")
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn c5bc_per_tool_await_revokes_the_next_effect_in_the_same_batch() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    scope_task(&fixture)?;
    fixture.git(&["branch", "owned-alternative"])?;
    let script = vec![
        support::calls(vec![
            ("read-guide", "read_file", support::path("docs/guide.md")),
            (
                "read-overview",
                "read_file",
                support::path("docs/overview.md"),
            ),
        ]),
        support::calls(vec![
            (
                "first",
                "edit_file",
                support::edit("docs/guide.md", "Before.", "Correct first edit remains."),
            ),
            (
                "second",
                "edit_file",
                support::edit(
                    "docs/overview.md",
                    "Existing guide overview.",
                    "This stale second edit must never occur.",
                ),
            ),
        ]),
        support::summary(),
    ];
    let server = support::Server::start_for(&fixture, script)?;
    configure(&fixture, &server.endpoint, &[])?;
    let mut observer = BranchSwitch {
        root: fixture.root.clone(),
        git: fixture.git.clone(),
        per_tool: true,
        changed: false,
    };
    let outcome = Box::pin(local(
        &fixture,
        &mut observer,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    let requests = server.finish()?;
    eprintln!(
        "c5bc per-tool root={} observed={} status={} handoff={:?}",
        fixture.root.display(),
        requests.len(),
        outcome.status().as_str(),
        outcome.coder_handoff
    );
    assert!(observer.changed);
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::PolicyDenied));
    assert!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md"))
            .map_err(io)?
            .contains("Correct first edit remains.")
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("overview.md")).map_err(io)?,
        "# Overview\n\nExisting guide overview.\n"
    );
    assert_eq!(requests.len(), 2);
    assert_eq!(outcome.orchestration.counts, Counts::default());
    Ok(())
}

async fn index_only(linked: bool) -> Result<(), Error> {
    let mut fixture = Fixture::new()?;
    let main = fixture.root.clone();
    let original_main_index = fixture.git(&["ls-files", "--stage", "-z"])?;
    if linked {
        let worktree = fixture.container.join("linked-project");
        fixture.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "owned-linked",
            worktree
                .to_str()
                .ok_or_else(|| Error::type_error("Owned linked path"))?,
        ])?;
        fixture.root = worktree;
    }
    docs_task(&fixture)?;
    let blob = fixture
        .git(&["rev-parse", "HEAD:README.md"])?
        .trim()
        .to_owned();
    let staged = format!("100644,{blob},docs/guide.md");
    let root = fixture.root.clone();
    let git = fixture.git.clone();
    let callback_root = root.clone();
    let server = support::Server::observed_for(
        &fixture,
        vec![support::calls(vec![(
            "write",
            "write_file",
            write(
                "docs/guide.md",
                "# Guide\n\nThis pending write must be revoked by index drift.\n",
            ),
        )])],
        move |_, _| {
            git_at(
                &callback_root,
                &git,
                &["update-index", "--cacheinfo", &staged],
            )?;
            Ok(())
        },
    )?;
    if linked {
        let saved = fixture.root.clone();
        fixture.root = main.clone();
        configure(
            &fixture,
            &server.endpoint,
            &[
                ("turn_budget: 16", "turn_budget: 1"),
                ("llm:\n", "llm:\n  max_requests: 1\n"),
            ],
        )?;
        fixture.root = saved;
    } else {
        configure(
            &fixture,
            &server.endpoint,
            &[
                ("turn_budget: 16", "turn_budget: 1"),
                ("llm:\n", "llm:\n  max_requests: 1\n"),
            ],
        )?;
    }
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    let requests = server.finish()?;
    eprintln!(
        "c5bc index-only root={} linked={} observed={} status={} handoff={:?}",
        root.display(),
        linked,
        requests.len(),
        outcome.status().as_str(),
        outcome.coder_handoff
    );
    assert_eq!(fixture.git(&["rev-parse", ":docs/guide.md"])?.trim(), blob);
    if linked {
        assert_eq!(
            fixture.git_at(&main, &["ls-files", "--stage", "-z"])?,
            original_main_index
        );
    }
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::PolicyDenied));
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert!(outcome.orchestration.archives.is_empty());
    assert_eq!(
        fs::read_to_string(root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    assert_eq!(requests.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn c5bc_index_only_response_revokes_pending_write_before_budget_one() -> Result<(), Error> {
    Box::pin(index_only(false)).await
}

#[tokio::test(flavor = "current_thread")]
async fn c5bc_linked_worktree_index_is_bound_not_the_main_repository_index() -> Result<(), Error> {
    Box::pin(index_only(true)).await
}

#[tokio::test(flavor = "current_thread")]
async fn c5bc_native_readonly_git_does_not_refresh_the_original_index_stat_cache()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let root = fixture.root.clone();
    let git = fixture.git.clone();
    let server =
        support::Server::observed_for(&fixture, support::docs_script(true), move |_, _| {
            let index = git_at(&root, &git, &["rev-parse", "--git-path", "index"])?;
            let index = paths::resolve(&root, Path::new(index.trim()))?;
            let before = fs::read(&index).map_err(io)?;
            git_at(
                &root,
                &git,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            )?;
            assert_eq!(fs::read(&index).map_err(io)?, before);
            Ok(())
        })?;
    configure(&fixture, &server.endpoint, &[])?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Verified, "{:?}", outcome.failure);
    assert_eq!(server.finish()?.len(), 5);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn c5bc_main_repository_index_change_is_not_linked_worktree_index_drift() -> Result<(), Error>
{
    let mut fixture = Fixture::new()?;
    let main = fixture.root.clone();
    let linked = fixture.container.join("linked-project");
    fixture.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "owned-linked",
        linked
            .to_str()
            .ok_or_else(|| Error::type_error("Owned linked path"))?,
    ])?;
    let blob = fixture
        .git(&["rev-parse", "HEAD:README.md"])?
        .trim()
        .to_owned();
    let stage = format!("100644,{blob},docs/guide.md");
    let original_linked_index = fixture.git_at(&linked, &["ls-files", "--stage", "-z"])?;
    let git = fixture.git.clone();
    let original_main = main.clone();
    let server = support::Server::observed_for(
        &fixture,
        vec![support::calls(vec![(
            "write",
            "write_file",
            write(
                "docs/guide.md",
                "# Guide\n\nOwned linked worktree write remains legitimate.\n",
            ),
        )])],
        move |_, _| {
            git_at(
                &original_main,
                &git,
                &["update-index", "--cacheinfo", &stage],
            )?;
            Ok(())
        },
    )?;
    configure(
        &fixture,
        &server.endpoint,
        &[
            ("turn_budget: 16", "turn_budget: 1"),
            ("llm:\n", "llm:\n  max_requests: 1\n"),
        ],
    )?;
    fixture.root = linked;
    docs_task(&fixture)?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::TurnBudget));
    assert_eq!(
        fixture.git(&["ls-files", "--stage", "-z"])?,
        original_linked_index
    );
    assert_eq!(
        fixture
            .git_at(&main, &["rev-parse", ":docs/guide.md"])?
            .trim(),
        blob
    );
    assert!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md"))
            .map_err(io)?
            .contains("remains legitimate")
    );
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

struct HttpObserverFailure;

impl Observer for HttpObserverFailure {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if event.get("type") == Some(&Value::string("http"))
            && event.get("phase") == Some(&Value::string("ok"))
        {
            return Err(Error::new(
                "OwnedFirstObserver",
                "test-only-private-api-key must not enter diagnostics",
            )
            .coded("E_C5BC_OBSERVER"));
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn c5bc_diagnostics_preserve_observer_origin_before_network_flattening() -> Result<(), Error>
{
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let server = support::Server::start_for(
        &fixture,
        vec![support::calls(vec![(
            "write",
            "write_file",
            write(
                "docs/guide.md",
                "# Guide\n\nUnexecuted observer-failed write.\n",
            ),
        )])],
    )?;
    configure(&fixture, &server.endpoint, &[])?;
    let outcome = Box::pin(local(
        &fixture,
        &mut HttpObserverFailure,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::Failed));
    assert_eq!(server.finish()?.len(), 1);
    let path = fixture
        .diagnostics
        .output()
        .ok_or_else(|| Error::type_error("Owned diagnostic output missing"))?;
    let evidence = fs::read_to_string(&path).map_err(io)?;
    assert!(!evidence.contains("test-only-private-api-key"));
    assert!(!evidence.contains("Unexecuted observer-failed write"));
    let last = evidence
        .lines()
        .last()
        .ok_or_else(|| Error::type_error("Owned diagnostic snapshot missing"))?;
    let value = github_agent_roster::json::parse(&Text::from(last))?;
    assert_eq!(value.get("reason"), Some(&Value::string("client-return")));
    assert_eq!(value.get("client_state"), Some(&Value::string("returned")));
    assert_eq!(value.get("expected"), Some(&Value::Number(1.0)));
    assert_eq!(value.get("accepted"), Some(&Value::Number(1.0)));
    assert_eq!(value.get("responses_completed"), Some(&Value::Number(1.0)));
    assert_eq!(value.get("remaining_packets"), Some(&Value::Number(0.0)));
    assert_eq!(value.get("transport_attempts"), Some(&Value::Number(1.0)));
    assert_eq!(value.get("first_transport_error"), Some(&Value::Null));
    let timing = value
        .get("native_timings")
        .ok_or_else(|| Error::type_error("Passive native timing snapshot missing"))?;
    assert_eq!(
        timing.get("clock"),
        Some(&Value::string("native-ownership-std-instant"))
    );
    assert_eq!(timing.get("active"), Some(&Value::Array(Vec::new())));
    let aggregates = match timing.get("aggregates") {
        Some(Value::Array(items)) => items,
        _ => {
            return Err(Error::type_error(
                "Passive native aggregate timings missing",
            ));
        }
    };
    assert!(aggregates.iter().any(|aggregate| aggregate.get("label")
        == Some(&Value::string("model.await"))
        && aggregate.get("count") == Some(&Value::Number(1.0))));
    assert!(
        aggregates
            .iter()
            .any(|aggregate| aggregate.get("label") == Some(&Value::string("owned.git.await")))
    );
    let error = value
        .get("native_log")
        .and_then(|value| value.get("first_observer_error"))
        .and_then(|value| value.get("details"))
        .ok_or_else(|| Error::type_error("Pre-flatten observer error disappeared"))?;
    assert_eq!(
        error.get("origin"),
        Some(&Value::string("delivery-observer-before-flattening"))
    );
    assert_eq!(error.get("phase"), Some(&Value::string("delegate")));
    assert_eq!(
        error.get("name"),
        Some(&Value::string("OwnedFirstObserver"))
    );
    assert_eq!(error.get("code"), Some(&Value::string("E_C5BC_OBSERVER")));
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    Ok(())
}
