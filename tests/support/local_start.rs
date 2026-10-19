use super::*;
use github_agent_roster::start::{FetchPolicy, Git, Options, Repository, Sync};

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn explicit_refs_are_unambiguous_complete_and_revalidated_before_any_local_claim()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let source = CancellationSource::default();
    let (host, approval) = fixture.host()?;
    let paths = paths::repository_paths(&fixture.root)?;
    let git = Git::new(host, paths.common_dir.join("owned-start-no-hooks"))?;
    fixture.git(&["tag", "main"])?;
    let repository = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    assert!(
        start::resolve(
            &repository,
            &git,
            &Options {
                base: "main".to_owned(),
                sync: Sync::Offline
            },
            &FetchPolicy::Deny,
            &source.token()
        )
        .await
        .is_err()
    );
    let full = start::resolve(
        &repository,
        &git,
        &Options {
            base: repository.head.to_ascii_uppercase(),
            sync: Sync::Offline,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await?;
    assert_eq!(full.commit, repository.head);
    let head = start::resolve(
        &repository,
        &git,
        &Options {
            base: "HEAD".to_owned(),
            sync: Sync::Offline,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await?;
    assert_eq!(head.commit, repository.head);
    fixture.git(&["branch", "human/selected", "HEAD"])?;
    let repository = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    let selected = start::resolve(
        &repository,
        &git,
        &Options {
            base: "refs/heads/human/selected".to_owned(),
            sync: Sync::Offline,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await?;
    let tree = fixture.git(&["rev-parse", "HEAD^{tree}"])?;
    let other = fixture.git(&[
        "commit-tree",
        tree.trim(),
        "-p",
        &repository.head,
        "-m",
        "Owned independent ref movement",
    ])?;
    fixture.git(&["update-ref", "refs/heads/human/selected", other.trim()])?;
    let loaded = fixture.loaded()?;
    assert_eq!(
        start::prepare_local(
            &repository,
            &git,
            &loaded.config,
            selected,
            &approval,
            Default::default(),
            source.token(),
            &mut start::Preparation::default()
        )
        .await
        .err()
        .and_then(|error| error.code),
        Some("ROSTER_STALE_SOURCE")
    );
    assert!(!fixture.root.join(".worktrees").exists());
    assert_eq!(fixture.git(&["rev-parse", "HEAD"])?.trim(), repository.head);
    Ok(())
}

#[test]
fn issue_names_and_worktree_inventory_preserve_exact_identity_and_refuse_namespace_collisions()
-> Result<(), Error> {
    assert_eq!(start::issue_branch(42, Some(16))?, "agents/42-a16");
    for bad in [
        "agents/0",
        "agents/01",
        "agents/42-a0",
        "agents/42-a17",
        "agents/local-model-task",
        "issue-42-a01",
    ] {
        assert!(start::parse_issue_branch(bad).is_none(), "{bad}");
    }
    let fixture = Fixture::new()?;
    let head = fixture.git(&["rev-parse", "HEAD"])?;
    let path = fixture
        .root
        .to_str()
        .ok_or_else(|| Error::type_error("Fixture root must be Unicode"))?;
    let data = format!(
        "worktree {path}\0HEAD {}\0branch refs/heads/agents/42\0\0",
        head.trim()
    );
    let entries = start::parse_inventory(data.as_bytes())?;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries.first().and_then(|entry| entry.branch.as_deref()),
        Some("agents/42")
    );
    let other = fixture.container.join("other");
    let duplicate = format!(
        "{data}worktree {}\0HEAD {}\0branch refs/heads/issue-42\0\0",
        other.display(),
        head.trim()
    );
    assert!(start::parse_inventory(duplicate.as_bytes()).is_err());
    let malformed = format!("worktree {path}\0HEAD short\0branch refs/heads/main\0\0");
    assert!(start::parse_inventory(malformed.as_bytes()).is_err());
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn real_local_bare_origin_start_fetches_only_approved_source_and_preserves_offline_staleness_and_drift()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let origin = fixture.container.join("origin.git");
    fixture.git_at(
        &fixture.container,
        &[
            "clone",
            "--bare",
            "--quiet",
            "--",
            fixture
                .root
                .to_str()
                .ok_or_else(|| Error::type_error("Root must be Unicode"))?,
            origin
                .to_str()
                .ok_or_else(|| Error::type_error("Origin must be Unicode"))?,
        ],
    )?;
    fixture.git(&[
        "remote",
        "add",
        "origin",
        origin
            .to_str()
            .ok_or_else(|| Error::type_error("Origin must be Unicode"))?,
    ])?;
    let source = CancellationSource::default();
    let (host, _) = fixture.host()?;
    let paths = paths::repository_paths(&fixture.root)?;
    let git = Git::new(host, paths.common_dir.join("owned-start-no-hooks"))?;
    let repo = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    let origin_head = fixture.git_at(&origin, &["rev-parse", "HEAD"])?;
    let approved = FetchPolicy::Local {
        source: origin.clone(),
        head: origin_head.trim().to_owned(),
    };
    let fresh = start::resolve(
        &repo,
        &git,
        &Options {
            base: "trunk".to_owned(),
            sync: Sync::Fetch,
        },
        &approved,
        &source.token(),
    )
    .await?;
    assert!(fresh.fetched);
    assert_eq!(fresh.mode, "trunk");
    assert_eq!(fresh.reference.as_deref(), Some("refs/remotes/origin/main"));
    assert_eq!(fresh.commit, origin_head.trim());
    fixture.write("docs/guide.md", "# New local source\n")?;
    fixture.git(&["add", "docs/guide.md"])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned source ahead of local origin",
    ])?;
    let repo = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    let offline = start::resolve(
        &repo,
        &git,
        &Options {
            base: "trunk".to_owned(),
            sync: Sync::Offline,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await?;
    assert!(!offline.fetched);
    assert_eq!(offline.commit, origin_head.trim());
    assert!(
        offline
            .notes
            .iter()
            .any(|note| note.contains("freshness unknown"))
    );
    assert_eq!(
        start::branch_drift(&repo, &git, "main", &offline, &source.token()).await?,
        Some(start::Drift {
            ahead: 1,
            behind: 0
        })
    );
    let current = start::resolve(
        &repo,
        &git,
        &Options {
            base: "current".to_owned(),
            sync: Sync::Offline,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await?;
    assert_eq!(current.commit, repo.head);
    let explicit = start::resolve(
        &repo,
        &git,
        &Options {
            base: origin_head.trim().to_owned(),
            sync: Sync::Offline,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await?;
    assert_eq!(explicit.commit, origin_head.trim());
    let unapproved = start::resolve(
        &repo,
        &git,
        &Options {
            base: "trunk".to_owned(),
            sync: Sync::Fetch,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await;
    assert_eq!(
        unapproved.err().and_then(|error| error.code),
        Some("ROSTER_NATIVE_UNSUPPORTED")
    );
    assert_eq!(
        fixture.git_at(&origin, &["rev-parse", "HEAD"])?,
        origin_head
    );
    fixture.git(&["branch", "human/main", "HEAD"])?;
    let human_head = fixture.git(&["rev-parse", "refs/heads/human/main"])?;
    let tree = fixture.git_at(&origin, &["rev-parse", "HEAD^{tree}"])?;
    let next_origin = fixture.git_at(
        &origin,
        &[
            "commit-tree",
            tree.trim(),
            "-p",
            origin_head.trim(),
            "-m",
            "Owned remote refspec control",
        ],
    )?;
    fixture.git_at(
        &origin,
        &["update-ref", "refs/heads/main", next_origin.trim()],
    )?;
    fixture.git(&[
        "config",
        "--replace-all",
        "remote.origin.fetch",
        "+refs/heads/*:refs/heads/human/*",
    ])?;
    let repo = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    let fetched = start::resolve(
        &repo,
        &git,
        &Options {
            base: "trunk".to_owned(),
            sync: Sync::Fetch,
        },
        &FetchPolicy::Local {
            source: origin.clone(),
            head: next_origin.trim().to_owned(),
        },
        &source.token(),
    )
    .await?;
    assert_eq!(fetched.commit, next_origin.trim());
    assert_eq!(
        fixture.git(&["rev-parse", "refs/heads/human/main"])?,
        human_head
    );
    assert_eq!(fixture.git(&["rev-parse", "HEAD"])?.trim(), repo.head);
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn local_preparation_preserves_human_dirty_worktrees_and_rejects_stale_refs_or_source()
-> Result<(), Error> {
    let mut fixture = Fixture::new()?;
    fixture.retain_on_drop = true;
    fixture.write("docs/guide.md", "# Human dirty guide\n")?;
    let human = fixture.container.join("human-tree");
    fixture.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "human/topic",
        human
            .to_str()
            .ok_or_else(|| Error::type_error("Human worktree must be Unicode"))?,
        "HEAD",
    ])?;
    fs::write(human.join("README.md"), "human other worktree bytes\n").map_err(io)?;
    let source = CancellationSource::default();
    let loaded = fixture.loaded()?;
    let (host, approval) = fixture.host()?;
    let paths = paths::repository_paths(&fixture.root)?;
    let git = Git::new(host, paths.common_dir.join("owned-start-no-hooks"))?;
    let repo = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    assert!(repo.dirty);
    let selected = start::resolve(
        &repo,
        &git,
        &Options {
            base: "current".to_owned(),
            sync: Sync::Offline,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await?;
    let mut preparation = start::Preparation::default();
    let prepared = start::prepare_local(
        &repo,
        &git,
        &loaded.config,
        selected,
        &approval,
        Default::default(),
        source.token(),
        &mut preparation,
    )
    .await?;
    assert_eq!(
        fs::read_to_string(prepared.worktree.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Human dirty guide\n"
    );
    assert_eq!(
        fs::read_to_string(human.join("README.md")).map_err(io)?,
        "human other worktree bytes\n"
    );
    assert_ne!(prepared.worktree, human);
    preparation.close().await?;
    let stale = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    fixture.write(
        "docs/guide.md",
        "# Same dirty-status different human bytes\n",
    )?;
    let stale_result = stale.require_current(&git, &source.token()).await;
    if let Err(error) = &stale_result {
        eprintln!(
            "Owned stale-source control: root={} error_name={} error_code={:?} failure_facts={:?}",
            fixture.root.display(),
            error.name,
            error.code,
            error.cause.as_ref().map(|cause| cause.message.as_str())
        );
    }
    assert_eq!(
        stale_result.err().and_then(|error| error.code),
        Some("ROSTER_STALE_SOURCE")
    );
    let current = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    for base in [
        "HEAD~1",
        "main^{tree}",
        "-bad",
        "main..other",
        "does-not-exist",
        current.head.get(..8).unwrap_or(""),
    ] {
        assert!(
            start::resolve(
                &current,
                &git,
                &Options {
                    base: base.to_owned(),
                    sync: Sync::Offline
                },
                &FetchPolicy::Deny,
                &source.token()
            )
            .await
            .is_err(),
            "{base}"
        );
    }
    assert_eq!(
        fixture
            .git_at(&human, &["branch", "--show-current"])?
            .trim(),
        "human/topic"
    );
    fixture.retain_on_drop = false;
    Ok(())
}

#[cfg(windows)]
#[test]
fn linked_worktree_reads_canonical_shared_private_config_not_a_forged_local_overlay()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let linked = fixture.container.join("linked");
    fixture.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "human/linked",
        linked
            .to_str()
            .ok_or_else(|| Error::type_error("Linked root must be Unicode"))?,
        "HEAD",
    ])?;
    secure::private_directory(&linked.join(".roster"), true)?;
    secure::write_exclusive(
        &linked.join(".roster").join("config.yml"),
        b"invalid local overlay\n",
    )?;
    let config = config::load(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &linked,
        &Value::object(Vec::new()),
    )?;
    assert_eq!(
        config.root,
        paths::repository_paths(&fixture.root)?.repository
    );
    assert_eq!(config.config, fixture.loaded()?.config);
    assert_eq!(
        fs::read_to_string(linked.join(".roster").join("config.yml")).map_err(io)?,
        "invalid local overlay\n"
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn declared_submodules_require_exact_approved_local_heads_and_never_use_their_declared_remote_url()
-> Result<(), Error> {
    let mut fixture = Fixture::new()?;
    fixture.retain_on_drop = true;
    let module = fixture.container.join("approved-module");
    secure::private_directory(&module, true)?;
    fixture.git_at(&module, &["init", "--quiet", "--initial-branch=main"])?;
    fs::write(
        module.join("value.txt"),
        "approved local dependency bytes\n",
    )
    .map_err(io)?;
    fixture.git_at(&module, &["add", "value.txt"])?;
    fixture.git_at(
        &module,
        &["commit", "--quiet", "-m", "Owned exact local submodule"],
    )?;
    fixture.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        "--",
        module
            .to_str()
            .ok_or_else(|| Error::type_error("Local module path must be Unicode"))?,
        "vendor/owned",
    ])?;
    fixture.git(&[
        "config",
        "--file",
        ".gitmodules",
        "submodule.vendor/owned.url",
        "https://unapproved.invalid/never-fetch.git",
    ])?;
    fixture.git(&["add", ".gitmodules", "vendor/owned"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned declarative submodule pin"])?;
    let original_modules = fs::read(fixture.root.join(".gitmodules")).map_err(io)?;
    let source = CancellationSource::default();
    let loaded = fixture.loaded()?;
    let (host, mut approval) = fixture.host()?;
    let paths = paths::repository_paths(&fixture.root)?;
    let git = Git::new(host, paths.common_dir.join("owned-start-no-hooks"))?;
    let repo = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    let selected = start::resolve(
        &repo,
        &git,
        &Options {
            base: "current".to_owned(),
            sync: Sync::Offline,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await?;
    let mut preparation = start::Preparation::default();
    let denied = start::prepare_local(
        &repo,
        &git,
        &loaded.config,
        selected.clone(),
        &approval,
        Default::default(),
        source.token(),
        &mut preparation,
    )
    .await;
    assert_eq!(
        denied.err().and_then(|error| error.code),
        Some("ROSTER_NATIVE_UNSUPPORTED")
    );
    assert!(!fixture.root.join(".worktrees").exists());
    approval.submodules.push(backend::LocalSubmodule {
        path: "vendor/owned".to_owned(),
        source: module.clone(),
    });
    let prepared = start::prepare_local(
        &repo,
        &git,
        &loaded.config,
        selected,
        &approval,
        Default::default(),
        source.token(),
        &mut preparation,
    )
    .await?;
    assert_eq!(
        fs::read_to_string(
            prepared
                .worktree
                .join("vendor")
                .join("owned")
                .join("value.txt")
        )
        .map_err(io)?,
        "approved local dependency bytes\n"
    );
    assert_eq!(
        fixture.git_at(
            &prepared.worktree.join("vendor").join("owned"),
            &["rev-parse", "HEAD"]
        )?,
        fixture.git_at(&module, &["rev-parse", "HEAD"])?
    );
    assert_eq!(
        fs::read(prepared.worktree.join(".gitmodules")).map_err(io)?,
        original_modules
    );
    assert_eq!(
        fs::read(fixture.root.join(".gitmodules")).map_err(io)?,
        original_modules
    );
    preparation.close().await?;
    fs::write(module.join("value.txt"), "new unapproved module revision\n").map_err(io)?;
    fixture.git_at(&module, &["add", "value.txt"])?;
    fixture.git_at(
        &module,
        &["commit", "--quiet", "-m", "Owned changed module revision"],
    )?;
    let repo = Repository::inspect(&fixture.root, &git, &source.token()).await?;
    let selected = start::resolve(
        &repo,
        &git,
        &Options {
            base: "current".to_owned(),
            sync: Sync::Offline,
        },
        &FetchPolicy::Deny,
        &source.token(),
    )
    .await?;
    assert!(
        start::prepare_local(
            &repo,
            &git,
            &loaded.config,
            selected,
            &approval,
            Default::default(),
            source.token(),
            &mut preparation
        )
        .await
        .is_err_and(|error| error.message.contains("HEAD must exactly equal"))
    );
    fixture.retain_on_drop = false;
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn git_source_configuration_cannot_select_filters_includes_or_credential_shells_for_native_preparation()
-> Result<(), Error> {
    for (key, value) in [
        ("filter.unsafe.smudge", "unapproved-command"),
        ("include.path", "unapproved-file"),
        ("core.askPass", "unapproved-command"),
        ("http.extraHeader", "unapproved-authentication"),
    ] {
        let fixture = Fixture::new()?;
        fixture.git(&["config", key, value])?;
        let source = CancellationSource::default();
        let (host, _) = fixture.host()?;
        let paths = paths::repository_paths(&fixture.root)?;
        let git = Git::new(host, paths.common_dir.join("owned-start-no-hooks"))?;
        assert_eq!(
            Repository::inspect(&fixture.root, &git, &source.token())
                .await
                .err()
                .and_then(|error| error.code),
            Some("ROSTER_GIT_CONFIG_DENIED")
        );
        assert!(!fixture.root.join(".worktrees").exists());
        assert!(!fixture.root.join("unapproved-command").exists());
        assert!(!fixture.root.join(".roster").join("locks").exists());
    }
    Ok(())
}
