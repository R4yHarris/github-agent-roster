use super::*;

#[path = "signature_tests.rs"]
mod signature_tests;

#[cfg(windows)]
fn approved_start_host() -> Result<tools::Host, Error> {
    let git = std::env::var_os("ROSTER_TEST_GIT")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH").and_then(|value| {
                std::env::split_paths(&value)
                    .map(|path| path.join("git.exe"))
                    .find(|path| path.is_file())
            })
        })
        .ok_or_else(|| Error::type_error("Owned start diagnostics require installed Git"))?;
    Ok(tools::Host {
        git: crate::host::Image::approve(&git)?.path().to_owned(),
        ..tools::Host::default()
    })
}

#[cfg(windows)]
fn start_path_git(fixture: &Fixture) -> Result<crate::start::Git, Error> {
    crate::start::Git::new(
        approved_start_host()?,
        fixture.root.join(".roster").join("path-query-no-hooks"),
    )
}

#[cfg(windows)]
async fn assert_start_path_parity(fixture: &Fixture, root: &Path) -> Result<(), Error> {
    let source = CancellationSource::default();
    let mut git = start_path_git(fixture)?;
    let paths = crate::paths::repository_paths(root)?;
    let index = fs::read(paths.git_dir.join("index")).map_err(io)?;
    let config = fs::read(paths.common_dir.join("config")).map_err(io)?;
    let task = fs::read(root.join("TASK.md")).map_err(io)?;
    let recipe = fs::read(root.join("RECIPE.yml")).map_err(io)?;
    let legacy_timings = timing::Timings::default();
    git.bind_timings(legacy_timings.clone());
    let legacy = [
        git.run(root, &["rev-parse", "--show-toplevel"], &source.token())
            .await?,
        git.run(root, &["rev-parse", "--git-common-dir"], &source.token())
            .await?,
        git.run(root, &["rev-parse", "--git-dir"], &source.token())
            .await?,
    ];
    let legacy_snapshot = legacy_timings.snapshot();
    let Some(Value::Array(legacy_aggregates)) = legacy_snapshot.get("aggregates") else {
        return Err(Error::type_error("Owned legacy path timings missing"));
    };
    for (label, count) in [
        ("start.git.root", 1.0),
        ("start.git.common", 1.0),
        ("start.git.directory", 1.0),
        ("start.git.execute.await", 3.0),
    ] {
        assert!(legacy_aggregates.iter().any(|aggregate| {
            aggregate.get("label") == Some(&Value::string(label))
                && aggregate.get("count") == Some(&Value::Number(count))
        }));
    }
    let raw = legacy.concat();
    let [expected_root, expected_common, expected_dir] = legacy.map(|bytes| {
        String::from_utf8(bytes)
            .map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
            .map_err(|_| Error::type_error("Owned legacy Git paths must be UTF-8"))
    });
    let expected = [expected_root?, expected_common?, expected_dir?];
    let timings = timing::Timings::default();
    git.bind_timings(timings.clone());
    assert_eq!(
        git.run(
            root,
            &[
                "rev-parse",
                "--show-toplevel",
                "--git-common-dir",
                "--git-dir"
            ],
            &source.token(),
        )
        .await?,
        raw
    );
    for _ in 0..2 {
        let current = git.repository_paths(&paths, &source.token()).await?;
        assert_eq!(current, expected);
        let [actual, common, git_dir] = current;
        assert_eq!(
            crate::paths::file_information(Path::new(&actual))?.identity,
            crate::paths::file_information(&paths.worktree)?.identity
        );
        assert_eq!(
            crate::paths::file_information(&crate::paths::resolve(root, Path::new(&common))?)?
                .identity,
            crate::paths::file_information(&paths.common_dir)?.identity
        );
        assert_eq!(
            crate::paths::file_information(&crate::paths::resolve(root, Path::new(&git_dir))?)?
                .identity,
            crate::paths::file_information(&paths.git_dir)?.identity
        );
    }
    let snapshot = timings.snapshot();
    let Some(Value::Array(aggregates)) = snapshot.get("aggregates") else {
        return Err(Error::type_error("Owned repository path timings missing"));
    };
    for label in ["start.git.paths", "start.git.execute.await"] {
        assert!(aggregates.iter().any(|aggregate| {
            aggregate.get("label") == Some(&Value::string(label))
                && aggregate.get("count") == Some(&Value::Number(3.0))
        }));
    }
    assert!(!aggregates.iter().any(|aggregate| {
        matches!(
            aggregate.get("label").and_then(Value::as_string),
            Some(label) if ["start.git.root", "start.git.common", "start.git.directory"].iter().any(|expected| label == &Text::from(*expected))
        )
    }));
    assert!(
        !snapshot
            .json()
            .contains(&fixture.root.display().to_string())
    );
    assert_eq!(fs::read(paths.git_dir.join("index")).map_err(io)?, index);
    assert_eq!(
        fs::read(paths.common_dir.join("config")).map_err(io)?,
        config
    );
    assert_eq!(fs::read(root.join("TASK.md")).map_err(io)?, task);
    assert_eq!(fs::read(root.join("RECIPE.yml")).map_err(io)?, recipe);
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn start_repository_path_query_matches_raw_values_and_identities_main_linked_both_hashes()
-> Result<(), Error> {
    for format in ["sha1", "sha256"] {
        let mut fixture = committed_query_fixture(format)?;
        let moved = fixture.root.with_extension("owned 'quoted' révision 東京");
        fs::rename(&fixture.root, &moved).map_err(io)?;
        fixture.root = moved;
        assert_start_path_parity(&fixture, &fixture.root).await?;
        let linked = fixture.root.join("linked 'quoted' révision 東京");
        let linked_name = linked
            .to_str()
            .ok_or_else(|| Error::type_error("Owned linked path must be Unicode"))?;
        fixture.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "owned-linked-path-query",
            linked_name,
            "HEAD",
        ])?;
        assert_start_path_parity(&fixture, &linked).await?;
        fixture.retain_on_drop = false;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn start_repository_path_query_preserves_detached_unborn_and_existing_config_refusals()
-> Result<(), Error> {
    for format in ["sha1", "sha256"] {
        let mut fixture = committed_query_fixture(format)?;
        fixture.git(&["checkout", "--quiet", "--detach", "HEAD"])?;
        assert_start_path_parity(&fixture, &fixture.root).await?;
        let git = start_path_git(&fixture)?;
        let source = CancellationSource::default();
        let observed =
            crate::start::Repository::inspect(&fixture.root, &git, &source.token()).await;
        if format == "sha256" {
            assert_eq!(
                fixture
                    .git(&["config", "--local", "--get", "extensions.objectformat"])?
                    .trim(),
                "sha256"
            );
            assert!(observed.is_err_and(|error| {
                error.name == "GitConfigurationDenied"
                    && error.code == Some("ROSTER_GIT_CONFIG_DENIED")
            }));
        } else {
            let repository = observed?;
            assert!(repository.branch.is_none());
            assert_eq!(
                repository.head,
                fixture.git(&["rev-parse", "--verify", "HEAD"])?.trim()
            );
        }
        fixture.retain_on_drop = false;
        let mut unborn = Fixture::with_object_format(&["docs/query.md"], false, Some(format))?;
        unborn.retain_on_drop = true;
        let git = start_path_git(&unborn)?;
        let paths = crate::paths::repository_paths(&unborn.root)?;
        let [actual, _, _] = git.repository_paths(&paths, &source.token()).await?;
        assert_eq!(
            crate::paths::file_information(Path::new(&actual))?.identity,
            crate::paths::file_information(&paths.worktree)?.identity
        );
        let error = crate::start::Repository::inspect(&unborn.root, &git, &source.token())
            .await
            .err()
            .ok_or_else(|| Error::type_error("Unborn inspection must remain an explicit error"))?;
        assert_eq!(error.name, "NativeGitError");
        assert_eq!(error.code, Some("ROSTER_GIT_FAILED"));
        unborn.retain_on_drop = false;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn start_repository_path_query_propagates_actual_errors_and_cancellation_without_fallback()
-> Result<(), Error> {
    let mut fixture = committed_query_fixture("sha1")?;
    let source = CancellationSource::default();
    let mut git = start_path_git(&fixture)?;
    let original = crate::paths::repository_paths(&fixture.root)?;
    let outside = fixture.root.join("not-a-repository");
    fs::create_dir(&outside).map_err(io)?;
    fs::write(outside.join(".git"), "gitdir: missing-directory\n").map_err(io)?;
    let mut absent = original.clone();
    absent.worktree = outside;
    let mut missing = original.clone();
    missing.worktree = fixture.root.join("missing-directory");
    let timings = timing::Timings::default();
    for (paths, code) in [
        (absent, "ROSTER_GIT_FAILED"),
        (missing, "ROSTER_PROCESS_START"),
    ] {
        let legacy = git
            .text(
                &paths.worktree,
                &["rev-parse", "--show-toplevel"],
                &source.token(),
            )
            .await
            .err()
            .ok_or_else(|| Error::type_error("Legacy invalid repository must fail"))?;
        assert_eq!(legacy.code, Some(code));
        git.bind_timings(timings.clone());
        let error = git
            .repository_paths(&paths, &source.token())
            .await
            .err()
            .ok_or_else(|| Error::type_error("Invalid repository query must fail"))?;
        assert_eq!(error.name, legacy.name);
        assert_eq!(error.code, legacy.code);
    }
    source.cancel(Reason::Cancelled)?;
    let error = git
        .repository_paths(&original, &source.token())
        .await
        .err()
        .ok_or_else(|| Error::type_error("Cancelled repository query must fail"))?;
    assert_eq!(error.code, Some("ROSTER_CANCELLED"));
    assert!(git.cleanup_certain());
    let snapshot = timings.snapshot();
    let Some(Value::Array(aggregates)) = snapshot.get("aggregates") else {
        return Err(Error::type_error("Owned repository error timings missing"));
    };
    for (label, count) in [
        ("start.git.paths", 3.0),
        ("start.git.root", 1.0),
        ("start.git.execute.await", 3.0),
    ] {
        assert!(aggregates.iter().any(|aggregate| {
            aggregate.get("label") == Some(&Value::string(label))
                && aggregate.get("count") == Some(&Value::Number(count))
        }));
    }
    assert!(!aggregates.iter().any(|aggregate| {
        aggregate.get("label") == Some(&Value::string("start.git.common"))
            || aggregate.get("label") == Some(&Value::string("start.git.directory"))
    }));
    fixture.retain_on_drop = false;
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn start_repository_path_query_keeps_original_source_config_index_and_ref_refusals()
-> Result<(), Error> {
    for change in ["source", "config", "index", "ref"] {
        let mut fixture = committed_query_fixture("sha1")?;
        let source = CancellationSource::default();
        let mut git = start_path_git(&fixture)?;
        let timings = timing::Timings::default();
        git.bind_timings(timings.clone());
        let repository =
            crate::start::Repository::inspect(&fixture.root, &git, &source.token()).await?;
        match change {
            "source" => fixture.write("docs/query.md", "Owned changed current source.\n")?,
            "config" => {
                fixture.git(&["config", "--local", "core.longpaths", "true"])?;
            }
            "index" => {
                fixture.write("docs/query.md", "Owned changed staged source.\n")?;
                fixture.git(&["add", "--", "docs/query.md"])?;
            }
            _ => {
                let tree = fixture.git(&["rev-parse", "HEAD^{tree}"])?;
                let commit = fixture.git(&[
                    "-c",
                    "user.name=OwnedQueryFixture",
                    "-c",
                    "user.email=owned-query@example.invalid",
                    "commit-tree",
                    tree.trim(),
                    "-p",
                    &repository.head,
                    "-m",
                    "Owned changed current ref",
                ])?;
                fixture.git(&[
                    "update-ref",
                    "refs/heads/owned-query-subject",
                    commit.trim(),
                ])?;
            }
        }
        let error = repository
            .require_current(&git, &source.token())
            .await
            .err()
            .ok_or_else(|| Error::type_error("Original current repository proof must revoke"))?;
        assert_eq!(error.code, Some("ROSTER_STALE_SOURCE"));
        let snapshot = timings.snapshot();
        let Some(Value::Array(aggregates)) = snapshot.get("aggregates") else {
            return Err(Error::type_error("Current repository timings missing"));
        };
        assert!(aggregates.iter().any(|aggregate| {
            aggregate.get("label") == Some(&Value::string("start.git.paths"))
                && aggregate.get("count") == Some(&Value::Number(2.0))
        }));
        fixture.retain_on_drop = false;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn start_repository_path_query_retains_marker_hardlink_refusal_before_owned_submission()
-> Result<(), Error> {
    let mut fixture = committed_query_fixture("sha1")?;
    let linked = fixture.root.join("linked-marker");
    let linked_name = linked
        .to_str()
        .ok_or_else(|| Error::type_error("Owned linked marker path must be Unicode"))?;
    fixture.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "owned-linked-marker",
        linked_name,
        "HEAD",
    ])?;
    crate::paths::repository_paths(&linked)?;
    fs::hard_link(linked.join(".git"), fixture.root.join("marker-alias")).map_err(io)?;
    assert_eq!(
        crate::paths::file_information(&linked.join(".git"))?.links,
        2
    );
    let expected = crate::paths::repository_paths(&linked)
        .err()
        .ok_or_else(|| Error::type_error("Hard-linked Git marker must fail native validation"))?;
    let mut git = start_path_git(&fixture)?;
    let timings = timing::Timings::default();
    git.bind_timings(timings.clone());
    let error =
        crate::start::Repository::inspect(&linked, &git, &CancellationSource::default().token())
            .await
            .err()
            .ok_or_else(|| Error::type_error("Linked Git marker must fail before submission"))?;
    assert_eq!(error.name, expected.name);
    assert_eq!(error.code, expected.code);
    assert_eq!(error.message, expected.message);
    let snapshot = timings.snapshot();
    assert_eq!(snapshot.get("aggregates"), Some(&Value::Array(Vec::new())));
    fixture.retain_on_drop = false;
    Ok(())
}

#[cfg(unix)]
#[test]
fn start_repository_path_query_keeps_actual_symlink_marker_refusal() -> Result<(), Error> {
    let mut fixture = Fixture::new(&["docs/query.md"], false)?;
    fixture.retain_on_drop = true;
    let linked = fixture.root.join("symlink-marker");
    fs::create_dir(&linked).map_err(io)?;
    std::os::unix::fs::symlink(fixture.root.join(".git"), linked.join(".git")).map_err(io)?;
    let error = crate::paths::repository_paths(&linked)
        .err()
        .ok_or_else(|| Error::type_error("Symlink marker must fail before a Git query"))?;
    assert!(
        error
            .message
            .contains("Git worktree marker must be a regular file or directory")
    );
    fs::remove_file(linked.join(".git")).map_err(io)?;
    fixture.retain_on_drop = false;
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn runtime_ignore_batch_checks_every_legacy_path_and_refuses_partial_coverage()
-> Result<(), Error> {
    let mut fixture = committed_query_fixture("sha1")?;
    let names = [
        ".roster/locks/run.lock",
        ".roster/runs/session.log",
        "ASSIGNMENT.md",
        "TASK.md",
        "RECIPE.yml",
        "ESTIMATE.md",
        "CONTEXT.md",
        "RESULT.md",
        "REVIEW.md",
        "PLAN.md",
        "RESEARCH.md",
    ];
    fixture.write(".gitignore", &format!("{}\n", names.join("\n")))?;
    let source = CancellationSource::default();
    let mut git = crate::start::Git::new(
        approved_start_host()?,
        fixture.root.join(".roster").join("batch-no-hooks"),
    )?;
    for name in names {
        git.run(
            &fixture.root,
            &["check-ignore", "--no-index", "--quiet", "--", name],
            &source.token(),
        )
        .await?;
    }
    let timings = timing::Timings::default();
    git.bind_timings(timings.clone());
    crate::delivery::ignored(&git, &fixture.root, &source.token()).await?;
    let snapshot = timings.snapshot();
    let aggregates = match snapshot.get("aggregates") {
        Some(Value::Array(aggregates)) => aggregates,
        _ => return Err(Error::type_error("Runtime ignore batch timings missing")),
    };
    assert!(aggregates.iter().any(|aggregate| {
        aggregate.get("label") == Some(&Value::string("start.git.execute.await"))
            && aggregate.get("count") == Some(&Value::Number(1.0))
    }));
    let partial = names
        .iter()
        .filter(|name| **name != "REVIEW.md")
        .copied()
        .collect::<Vec<_>>();
    fixture.write(".gitignore", &format!("{}\n", partial.join("\n")))?;
    let error = crate::delivery::ignored(&git, &fixture.root, &source.token())
        .await
        .err()
        .ok_or_else(|| Error::type_error("Partial runtime ignore coverage must fail"))?;
    assert_eq!(error.code, Some("ROSTER_LOCAL_IGNORE"));
    let mixed = git
        .run(
            &fixture.root,
            &["check-ignore", "--no-index", "--", "TASK.md", "REVIEW.md"],
            &source.token(),
        )
        .await?;
    assert_eq!(mixed, b"TASK.md\n");
    fixture.retain_on_drop = false;
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn start_git_failure_facts_distinguish_real_missing_and_nondirectory_cwd() -> Result<(), Error>
{
    let mut fixture = committed_query_fixture("sha1")?;
    let source = CancellationSource::default();
    let git = crate::start::Git::new(
        approved_start_host()?,
        fixture.root.join(".roster").join("diagnostic-no-hooks"),
    )?;
    let missing = fixture.root.join("missing-cwd");
    let file = fixture.root.join("file-cwd");
    fs::write(&file, "Owned non-directory current path.\n").map_err(io)?;
    for (cwd, expected) in [(&missing, "missing"), (&file, "not-directory")] {
        let error = git
            .run(cwd, &["status", "--porcelain=v1"], &source.token())
            .await
            .err()
            .ok_or_else(|| Error::type_error("Invalid owned cwd must not start Git"))?;
        assert_eq!(error.name, "NativeGitError");
        assert_eq!(error.code, Some("ROSTER_PROCESS_START"));
        let facts = error
            .cause
            .ok_or_else(|| Error::type_error("Actual spawn OS facts must be retained"))?;
        assert_eq!(facts.name, "NativeGitFailureFacts");
        assert!(facts.message.contains(&format!("cwd_state={expected}")));
        assert!(facts.message.contains("os_code=Some("));
        assert!(facts.message.contains("existing_image_checks=passed"));
        assert!(facts.message.contains("existing_cancel_check=passed"));
        assert!(!facts.message.contains(&fixture.root.display().to_string()));
        eprintln!("Owned native Git failure facts: {}", facts.message);
    }
    fixture.retain_on_drop = false;
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn start_git_cancelled_source_keeps_reason_without_spawn_or_stale_relabel()
-> Result<(), Error> {
    let mut fixture = committed_query_fixture("sha1")?;
    let source = CancellationSource::default();
    let git = crate::start::Git::new(
        approved_start_host()?,
        fixture.root.join(".roster").join("cancelled-no-hooks"),
    )?;
    source.cancel(Reason::Cancelled)?;
    let error = git
        .run(
            &fixture.root,
            &["status", "--porcelain=v1"],
            &source.token(),
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Cancelled Git must not start"))?;
    assert_eq!(error.code, Some("ROSTER_CANCELLED"));
    assert!(error.cause.is_none());
    assert!(git.cleanup_certain());
    fixture.retain_on_drop = false;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn deep_proof_spans_share_owner_clock_without_replacing_actual_current_checks()
-> Result<(), Error> {
    let mut fixture = committed_query_fixture("sha1")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let result = async {
        let timings = timing::Timings::default();
        boundary.bind_timings(timings.clone());
        let owner = backend::Owner::Execution(&boundary);
        let stamp = backend::Stamp::capture(owner).await?;
        stamp.require_current(owner).await?;
        fixture.write("docs/query.md", "Owned changed proof subject.\n")?;
        assert!(stamp.require_current(owner).await.is_err_and(|error| {
            error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
        }));
        let snapshot = timings.snapshot();
        assert_eq!(snapshot.get("active"), Some(&Value::Array(Vec::new())));
        let aggregates = match snapshot.get("aggregates") {
            Some(Value::Array(aggregates)) => aggregates,
            _ => return Err(Error::type_error("Owned proof aggregate timings missing")),
        };
        for (label, count) in [
            ("proof.stamp.capture", 3.0),
            ("proof.stamp.current", 2.0),
            ("proof.source.capture", 3.0),
            ("proof.head.from-refs", 3.0),
            ("proof.git.index", 3.0),
            ("proof.git.refs", 3.0),
            ("proof.git.config", 3.0),
            ("proof.git.execute.await", 9.0),
        ] {
            assert!(aggregates.iter().any(|aggregate| {
                aggregate.get("label") == Some(&Value::string(label))
                    && aggregate.get("count") == Some(&Value::Number(count))
            }));
        }
        let encoded = snapshot.json();
        assert!(encoded.len() <= 64 * 1024);
        assert!(!encoded.contains(&fixture.root.display().to_string()));
        assert!(!encoded.contains("docs/query.md"));
        assert!(!encoded.contains("Owned changed proof subject"));
        Ok(())
    }
    .await;
    crate::state::locks::finish(result, boundary.close().await)?;
    fixture.retain_on_drop = false;
    Ok(())
}

fn committed_query_fixture(format: &str) -> Result<Fixture, Error> {
    let mut fixture = Fixture::with_object_format(&["docs/query.md"], false, Some(format))?;
    fixture.retain_on_drop = true;
    fixture.write("docs/query.md", "Owned committed query subject.\n")?;
    fixture.git(&["symbolic-ref", "HEAD", "refs/heads/owned-query-subject"])?;
    fixture.git(&["add", "--all"])?;
    fixture.git(&[
        "-c",
        "user.name=OwnedQueryFixture",
        "-c",
        "user.email=owned-query@example.invalid",
        "commit",
        "--quiet",
        "-m",
        "Owned committed query subject",
    ])?;
    Ok(fixture)
}

async fn assert_query_display(fixture: &Fixture, root: &Path) -> Result<(), Error> {
    let source = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        root,
        fixture.assignment.clone(),
        tools::Options::default(),
        tools::Host::default(),
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let result = async {
        let old_head = tools::bytes_text(
            &boundary
                .git(
                    &["rev-parse", "--verify", "HEAD"].map(str::to_owned),
                    &BTreeMap::new(),
                )
                .await?,
        )?
        .trim()
        .to_owned();
        let old_branch = tools::bytes_text(
            &boundary
                .git(
                    &["branch", "--show-current"].map(str::to_owned),
                    &BTreeMap::new(),
                )
                .await?,
        )?
        .trim()
        .to_owned();
        let index_path = tools::bytes_text(
            &boundary
                .git(
                    &["rev-parse", "--git-path", "index"].map(str::to_owned),
                    &BTreeMap::new(),
                )
                .await?,
        )?;
        let index_path = crate::paths::resolve(root, Path::new(index_path.trim()))?;
        if root != fixture.root {
            assert_ne!(index_path, fixture.root.join(".git").join("index"));
        }
        let index = fs::read(&index_path).map_err(io)?;
        let config = fs::read(fixture.root.join(".git").join("config")).map_err(io)?;
        let task = fs::read(root.join("TASK.md")).map_err(io)?;
        let recipe = fs::read(root.join("RECIPE.yml")).map_err(io)?;
        let timings = timing::Timings::default();
        boundary.bind_timings(timings.clone());
        for _ in 0..2 {
            let subject = checkpoints::validation(&boundary).await?;
            assert_eq!(subject.get("head"), Some(&Value::string(&old_head)));
            assert_eq!(subject.get("branch"), Some(&Value::string(&old_branch)));
        }
        let snapshot = timings.snapshot();
        let aggregates = match snapshot.get("aggregates") {
            Some(Value::Array(aggregates)) => aggregates,
            _ => return Err(Error::type_error("Owned query aggregate timings missing")),
        };
        assert!(aggregates.iter().any(|aggregate| {
            aggregate.get("label") == Some(&Value::string("git.head-branch"))
                && aggregate.get("count") == Some(&Value::Number(2.0))
        }));
        assert!(aggregates.iter().any(|aggregate| {
            aggregate.get("label") == Some(&Value::string("owned.git.await"))
                && aggregate.get("count") == Some(&Value::Number(2.0))
        }));
        assert!(!aggregates.iter().any(|aggregate| {
            aggregate.get("label") == Some(&Value::string("git.head"))
                || aggregate.get("label") == Some(&Value::string("git.branch"))
        }));
        assert_eq!(fs::read(index_path).map_err(io)?, index);
        assert_eq!(
            fs::read(fixture.root.join(".git").join("config")).map_err(io)?,
            config
        );
        assert_eq!(fs::read(root.join("TASK.md")).map_err(io)?, task);
        assert_eq!(fs::read(root.join("RECIPE.yml")).map_err(io)?, recipe);
        Ok(())
    }
    .await;
    crate::state::locks::finish(result, boundary.close().await)
}

async fn assert_valid_displays(format: &str) -> Result<(), Error> {
    let mut fixture = committed_query_fixture(format)?;
    let prior = fixture
        .git(&["rev-parse", "--verify", "HEAD"])?
        .trim()
        .to_owned();
    fixture.git(&[
        "-c",
        "user.name=OwnedQueryFixture",
        "-c",
        "user.email=owned-query@example.invalid",
        "commit",
        "--quiet",
        "--allow-empty",
        "-m",
        "Owned current commit distinct from HEAD tag",
    ])?;
    let head = fixture
        .git(&["rev-parse", "--verify", "HEAD"])?
        .trim()
        .to_owned();
    fixture.git(&["update-ref", "refs/tags/HEAD", &prior])?;
    for tag in ["topic", "heads/topic", "refs/heads/topic"] {
        fixture.git(&["update-ref", &format!("refs/tags/{tag}"), &prior])?;
    }
    for branch in [
        "owned-query-subject",
        "(detached)",
        "topic",
        "heads/topic",
        "refs/heads/topic",
        "révision/東京",
        "\u{a0}owned-edge\u{2003}",
        "HEAD",
    ] {
        let reference = format!("refs/heads/{branch}");
        fixture.git(&["update-ref", &reference, &head])?;
        fixture.git(&["symbolic-ref", "HEAD", &reference])?;
        assert_query_display(&fixture, &fixture.root).await?;
    }
    fixture.retain_on_drop = false;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn query_subject_valid_sha1_displays_match_actual_branch_command() -> Result<(), Error> {
    assert_valid_displays("sha1").await
}

#[tokio::test(flavor = "current_thread")]
async fn query_subject_valid_sha256_displays_match_actual_branch_command() -> Result<(), Error> {
    assert_valid_displays("sha256").await
}

#[tokio::test(flavor = "current_thread")]
async fn query_subject_linked_worktrees_bind_current_branch_and_actual_index_for_both_hashes()
-> Result<(), Error> {
    for format in ["sha1", "sha256"] {
        let mut fixture = committed_query_fixture(format)?;
        let head = fixture
            .git(&["rev-parse", "--verify", "HEAD"])?
            .trim()
            .to_owned();
        fixture.git(&["update-ref", "refs/tags/HEAD", &head])?;
        let linked = fixture.root.join("linked-query");
        let linked_name = linked
            .to_str()
            .ok_or_else(|| Error::type_error("Owned linked path must be Unicode"))?;
        fixture.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "owned-linked-query",
            linked_name,
            &head,
        ])?;
        assert_query_display(&fixture, &linked).await?;
        fixture.retain_on_drop = false;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn query_subject_detached_and_unborn_states_fail_closed_for_both_hashes() -> Result<(), Error>
{
    for format in ["sha1", "sha256"] {
        let mut detached = committed_query_fixture(format)?;
        let head = detached
            .git(&["rev-parse", "--verify", "HEAD"])?
            .trim()
            .to_owned();
        detached.git(&["update-ref", "--no-deref", "HEAD", &head])?;
        let mut boundary = detached
            .open(
                tools::Options::default(),
                tools::Host::default(),
                &CancellationSource::default(),
            )
            .await?;
        let subject = checkpoints::validation(&boundary).await;
        boundary.close().await?;
        assert!(subject.is_err_and(|error| {
            error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
        }));
        detached.retain_on_drop = false;
        let mut unborn = Fixture::with_object_format(&["docs/query.md"], false, Some(format))?;
        unborn.retain_on_drop = true;
        let mut boundary = unborn
            .open(
                tools::Options::default(),
                tools::Host::default(),
                &CancellationSource::default(),
            )
            .await?;
        let subject = checkpoints::validation(&boundary).await;
        boundary.close().await?;
        assert!(subject.is_err_and(|error| {
            error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
        }));
        unborn.retain_on_drop = false;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn query_subject_original_seal_refuses_every_other_head_named_fallback() -> Result<(), Error>
{
    for reference in ["refs/heads/HEAD", "refs/remotes/HEAD", "refs/HEAD"] {
        let mut fixture = committed_query_fixture("sha1")?;
        let head = fixture
            .git(&["rev-parse", "--verify", "HEAD"])?
            .trim()
            .to_owned();
        fixture.git(&["update-ref", reference, &head])?;
        let task = fs::read(fixture.root.join("TASK.md")).map_err(io)?;
        let recipe = fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?;
        let index = fs::read(fixture.root.join(".git").join("index")).map_err(io)?;
        let mut boundary = fixture
            .open(
                tools::Options::default(),
                tools::Host::default(),
                &CancellationSource::default(),
            )
            .await?;
        let result = async {
            boundary.admit_git().await?;
            let original = boundary.git_validation()?.clone();
            boundary.require_git().await?;
            fixture.git(&["update-ref", "-d", "refs/heads/owned-query-subject"])?;
            assert!(
                checkpoints::validation(&boundary)
                    .await
                    .is_err_and(|error| {
                        error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
                    })
            );
            assert!(boundary.require_git().await.is_err_and(|error| {
                error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
            }));
            assert_eq!(boundary.git_validation()?, &original);
            assert_eq!(fs::read(fixture.root.join("TASK.md")).map_err(io)?, task);
            assert_eq!(
                fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?,
                recipe
            );
            assert_eq!(
                fs::read(fixture.root.join(".git").join("index")).map_err(io)?,
                index
            );
            assert!(boundary.first_checkpoint().is_none());
            Ok(())
        }
        .await;
        crate::state::locks::finish(result, boundary.close().await)?;
        fixture.retain_on_drop = false;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn query_subject_owned_git_start_error_never_becomes_a_subject() -> Result<(), Error> {
    let mut fixture = committed_query_fixture("sha1")?;
    let host = tools::Host {
        git: fixture.root.join("owned-missing-git.exe"),
        ..tools::Host::default()
    };
    let opened = fixture
        .open(
            tools::Options::default(),
            host,
            &CancellationSource::default(),
        )
        .await;
    match opened {
        Err(error) => {
            assert_eq!(error.name, "Error");
            assert_eq!(error.code, Some("ROSTER_PROCESS_START"));
        }
        Ok(mut boundary) => {
            boundary.close().await?;
            return Err(Error::new(
                "Error",
                "Missing owned Git image must fail native preflight",
            ));
        }
    }
    fixture.retain_on_drop = false;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn query_subject_original_seal_refuses_deleted_branch_masked_by_head_named_tag()
-> Result<(), Error> {
    let mut fixture = Fixture::new(&["docs/query.md"], false)?;
    fixture.retain_on_drop = true;
    fixture.write("docs/query.md", "Owned query subject.\n")?;
    fixture.git(&["symbolic-ref", "HEAD", "refs/heads/owned-query-subject"])?;
    fixture.git(&["add", "--all"])?;
    fixture.git(&[
        "-c",
        "user.name=OwnedQueryFixture",
        "-c",
        "user.email=owned-query@example.invalid",
        "commit",
        "--quiet",
        "-m",
        "Owned query subject",
    ])?;
    let original_head = fixture
        .git(&["rev-parse", "--verify", "HEAD"])?
        .trim()
        .to_owned();
    fixture.git(&["update-ref", "refs/tags/HEAD", &original_head])?;
    let task = fs::read(fixture.root.join("TASK.md")).map_err(io)?;
    let recipe = fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?;
    let index = fs::read(fixture.root.join(".git").join("index")).map_err(io)?;
    let product = fs::read(fixture.root.join("docs").join("query.md")).map_err(io)?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let result = async {
        boundary.admit_git().await?;
        let original = boundary.git_validation()?.clone();
        assert_eq!(original.get("head"), Some(&Value::string(&original_head)));
        assert_eq!(
            original.get("branch"),
            Some(&Value::string("owned-query-subject"))
        );
        boundary.require_git().await?;
        fixture.git(&["update-ref", "-d", "refs/heads/owned-query-subject"])?;
        let actual_branch = boundary
            .git(
                &["rev-parse", "--verify", "refs/heads/owned-query-subject"]
                    .map(str::to_owned),
                &BTreeMap::new(),
            )
            .await;
        assert!(
            actual_branch.is_err_and(|error| error.code == Some("ROSTER_GIT_FAILED"))
        );
        let validation = checkpoints::validation(&boundary).await;
        assert!(validation.is_err_and(|error| {
            error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
        }));
        assert_eq!(boundary.git_validation()?, &original);
        assert_eq!(
            fs::read(fixture.root.join("TASK.md")).map_err(io)?,
            task
        );
        assert_eq!(
            fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?,
            recipe
        );
        assert_eq!(
            fs::read(fixture.root.join(".git").join("index")).map_err(io)?,
            index
        );
        assert_eq!(
            fs::read(fixture.root.join("docs").join("query.md")).map_err(io)?,
            product
        );
        assert!(boundary.first_checkpoint().is_none());
        assert!(!fixture.root.join("RESULT.md").exists());
        assert!(!fixture.root.join("REVIEW.md").exists());
        let current = boundary.require_git().await;
        eprintln!(
            "Owned Git-query original-seal check: root={} actual_branch_present=false validation_refused=true original_seal_current={}",
            fixture.root.display(),
            current.is_ok()
        );
        Ok(current)
    }
    .await;
    let current = crate::state::locks::finish(result, boundary.close().await)?;
    assert!(
        current.is_err_and(|error| {
            error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
        }),
        "An absent original branch masked by a HEAD-named tag must revoke the original private Git seal"
    );
    fixture.retain_on_drop = false;
    Ok(())
}
