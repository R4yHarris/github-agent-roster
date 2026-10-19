use super::*;

async fn legacy_signature(owner: backend::Owner<'_>, directory: &Path) -> Result<Value, Error> {
    let mut parts = Vec::new();
    for args in [
        vec!["rev-parse", "--verify", "HEAD"],
        vec![
            "diff",
            "--cached",
            "--raw",
            "--no-ext-diff",
            "--no-textconv",
            "-z",
        ],
        vec!["show-ref", "--head"],
        vec!["config", "--local", "--null", "--list"],
    ] {
        let bytes = owner
            .git(
                directory,
                &args.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                false,
            )
            .await?;
        parts.push(Value::string(&crate::identity::sha256_hex(&bytes)));
    }
    Ok(Value::Array(parts))
}

async fn assert_signature_parity(fixture: &Fixture, directory: &Path) -> Result<(), Error> {
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let result =
        async {
            let owner = backend::Owner::Execution(&boundary);
            let index_path = tools::bytes_text(
                &owner
                    .git(
                        directory,
                        &["rev-parse", "--git-path", "index"].map(str::to_owned),
                        false,
                    )
                    .await?,
            )?;
            let index_path = crate::paths::resolve(directory, Path::new(index_path.trim()))?;
            let config_path = tools::bytes_text(
                &owner
                    .git(
                        directory,
                        &["rev-parse", "--git-path", "config"].map(str::to_owned),
                        false,
                    )
                    .await?,
            )?;
            let config_path = crate::paths::resolve(directory, Path::new(config_path.trim()))?;
            let index = fs::read(&index_path).map_err(io)?;
            let config = fs::read(&config_path).map_err(io)?;
            let task = fs::read(fixture.root.join("TASK.md")).map_err(io)?;
            let recipe = fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?;
            let expected = legacy_signature(owner, directory).await?;
            let timings = timing::Timings::default();
            boundary.bind_timings(timings.clone());
            let owner = backend::Owner::Execution(&boundary);
            assert_eq!(backend::git_signature(owner, directory).await?, expected);
            assert_eq!(backend::git_signature(owner, directory).await?, expected);
            let snapshot = timings.snapshot();
            let aggregates = match snapshot.get("aggregates") {
                Some(Value::Array(aggregates)) => aggregates,
                _ => return Err(Error::type_error("Owned signature timings missing")),
            };
            for (label, count) in [
                ("proof.head.from-refs", 2.0),
                ("proof.git.index", 2.0),
                ("proof.git.refs", 2.0),
                ("proof.git.config", 2.0),
                ("proof.git.execute.await", 6.0),
            ] {
                assert!(aggregates.iter().any(|aggregate| {
                    aggregate.get("label") == Some(&Value::string(label))
                        && aggregate.get("count") == Some(&Value::Number(count))
                }));
            }
            assert!(!aggregates.iter().any(|aggregate| {
                aggregate.get("label") == Some(&Value::string("proof.git.head"))
            }));
            assert_eq!(fs::read(index_path).map_err(io)?, index);
            assert_eq!(fs::read(config_path).map_err(io)?, config);
            assert_eq!(fs::read(fixture.root.join("TASK.md")).map_err(io)?, task);
            assert_eq!(
                fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?,
                recipe
            );
            Ok(())
        }
        .await;
    crate::state::locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn signature_real_git_matches_four_components_main_linked_and_detached_for_both_hashes()
-> Result<(), Error> {
    for format in ["sha1", "sha256"] {
        let mut fixture = committed_query_fixture(format)?;
        let head = fixture
            .git(&["rev-parse", "--verify", "HEAD"])?
            .trim()
            .to_owned();
        fixture.git(&["update-ref", "refs/tags/HEAD", &head])?;
        fixture.git(&["update-ref", "refs/heads/HEAD", &head])?;
        fixture.git(&["update-ref", "refs/tags/owned-query-subject", &head])?;
        assert_signature_parity(&fixture, &fixture.root).await?;
        let linked = fixture.root.join("signature-linked");
        let linked_name = linked
            .to_str()
            .ok_or_else(|| Error::type_error("Owned signature path must be Unicode"))?;
        fixture.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "signature-linked",
            linked_name,
            &head,
        ])?;
        assert_signature_parity(&fixture, &linked).await?;
        let base = fixture.root.join("signature-base");
        let base_name = base
            .to_str()
            .ok_or_else(|| Error::type_error("Owned baseline path must be Unicode"))?;
        fixture.git(&[
            "clone",
            "--quiet",
            "--shared",
            "--no-checkout",
            "--",
            ".",
            base_name,
        ])?;
        for args in [
            vec!["update-ref", "--no-deref", "HEAD", head.as_str()],
            vec!["read-tree", head.as_str()],
        ] {
            let output = fixture
                .git_command()
                .current_dir(&base)
                .args(args)
                .output()
                .map_err(io)?;
            if !output.status.success() {
                return Err(Error::new("Error", "Owned detached baseline setup failed"));
            }
        }
        assert_signature_parity(&fixture, &base).await?;
        fixture.retain_on_drop = false;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn signature_real_git_keeps_non_utf8_refs_and_local_config_byte_exact() -> Result<(), Error> {
    let mut fixture = committed_query_fixture("sha1")?;
    let head = fixture
        .git(&["rev-parse", "--verify", "HEAD"])?
        .trim()
        .to_owned();
    fixture.git(&["pack-refs", "--all"])?;
    let packed_path = fixture.root.join(".git").join("packed-refs");
    let mut packed = fs::read(&packed_path).map_err(io)?;
    packed.extend_from_slice(format!("{head} refs/tags/").as_bytes());
    packed.extend_from_slice(&[0xff, 0xfe, b'\n']);
    fs::write(&packed_path, &packed).map_err(io)?;
    let config_path = fixture.root.join(".git").join("config");
    let mut config = fs::read(&config_path).map_err(io)?;
    config.extend_from_slice(b"\n[signature]\n\tbytes = opaque");
    config.extend_from_slice(&[0xff, 0xfe, b'\n']);
    fs::write(&config_path, &config).map_err(io)?;
    let refs = fixture
        .git_command()
        .args(["show-ref", "--head"])
        .output()
        .map_err(io)?;
    assert!(refs.status.success());
    assert!(std::str::from_utf8(&refs.stdout).is_err());
    let local = fixture
        .git_command()
        .args(["config", "--local", "--null", "--list"])
        .output()
        .map_err(io)?;
    assert!(local.status.success());
    assert!(std::str::from_utf8(&local.stdout).is_err());
    assert_signature_parity(&fixture, &fixture.root).await?;
    fixture.retain_on_drop = false;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn signature_actual_stamp_rejects_each_source_index_ref_and_config_drift() -> Result<(), Error>
{
    for variant in ["source", "index", "refs", "config"] {
        let mut fixture = committed_query_fixture("sha1")?;
        let original_product = fs::read(fixture.root.join("docs").join("query.md")).map_err(io)?;
        let head = fixture
            .git(&["rev-parse", "--verify", "HEAD"])?
            .trim()
            .to_owned();
        let alternate = fixture
            .git(&["rev-parse", "HEAD:TASK.md"])?
            .trim()
            .to_owned();
        let mut boundary = fixture
            .open(
                tools::Options::default(),
                tools::Host::default(),
                &CancellationSource::default(),
            )
            .await?;
        let result = async {
            let owner = backend::Owner::Execution(&boundary);
            let stamp = backend::Stamp::capture(owner).await?;
            stamp.require_current(owner).await?;
            match variant {
                "source" => fixture.write("docs/query.md", "Owned changed signature source.\n")?,
                "index" => {
                    fixture.git(&[
                        "update-index",
                        "--cacheinfo",
                        "100644",
                        &alternate,
                        "docs/query.md",
                    ])?;
                    assert_eq!(
                        fs::read(fixture.root.join("docs").join("query.md")).map_err(io)?,
                        original_product
                    );
                }
                "refs" => {
                    fixture.git(&["update-ref", "refs/tags/signature-changed", &head])?;
                }
                "config" => {
                    fixture.write(
                        ".git/config",
                        &format!(
                            "{}\n[signature]\n\tchanged = true\n",
                            fs::read_to_string(fixture.root.join(".git").join("config"))
                                .map_err(io)?
                        ),
                    )?;
                }
                _ => return Err(Error::type_error("Unknown owned signature drift case")),
            }
            assert!(stamp.require_current(owner).await.is_err_and(|error| {
                error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
            }));
            Ok(())
        }
        .await;
        crate::state::locks::finish(result, boundary.close().await)?;
        fixture.retain_on_drop = false;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn signature_precancel_and_real_missing_head_errors_never_return_cached_state()
-> Result<(), Error> {
    let mut fixture = committed_query_fixture("sha1")?;
    let source = CancellationSource::default();
    let mut boundary = fixture
        .open(tools::Options::default(), tools::Host::default(), &source)
        .await?;
    let timings = timing::Timings::default();
    boundary.bind_timings(timings.clone());
    source.cancel(Reason::Cancelled)?;
    let result = backend::git_signature(backend::Owner::Execution(&boundary), &fixture.root).await;
    boundary.close().await?;
    assert!(result.is_err_and(|error| error.code == Some("ROSTER_CANCELLED")));
    let snapshot = timings.snapshot();
    let aggregates = match snapshot.get("aggregates") {
        Some(Value::Array(aggregates)) => aggregates,
        _ => {
            return Err(Error::type_error(
                "Owned cancelled signature timings missing",
            ));
        }
    };
    assert!(!aggregates.iter().any(|aggregate| {
        aggregate.get("label") == Some(&Value::string("proof.git.execute.await"))
    }));
    fixture.retain_on_drop = false;
    let mut missing = committed_query_fixture("sha1")?;
    let mut boundary = missing
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let result = async {
        missing.git(&["update-ref", "-d", "refs/heads/owned-query-subject"])?;
        let owner = backend::Owner::Execution(&boundary);
        let legacy = legacy_signature(owner, &missing.root).await;
        let current = backend::git_signature(owner, &missing.root).await;
        let legacy = legacy
            .err()
            .ok_or_else(|| Error::type_error("Missing old HEAD must fail"))?;
        let current = current
            .err()
            .ok_or_else(|| Error::type_error("Missing new HEAD must fail"))?;
        assert_eq!(legacy.name, "NativeGitError");
        assert_eq!(current.name, legacy.name);
        assert_eq!(current.code, legacy.code);
        Ok(())
    }
    .await;
    crate::state::locks::finish(result, boundary.close().await)?;
    missing.retain_on_drop = false;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn signature_pending_real_git_cancellation_settles_before_owner_release() -> Result<(), Error>
{
    use std::future::Future;
    use std::task::Poll;

    let mut fixture = committed_query_fixture("sha1")?;
    let source = CancellationSource::default();
    let mut boundary = fixture
        .open(tools::Options::default(), tools::Host::default(), &source)
        .await?;
    let result = {
        let mut work = Box::pin(backend::git_signature(
            backend::Owner::Execution(&boundary),
            &fixture.root,
        ));
        std::future::poll_fn(|context| match work.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(Ok(())),
            Poll::Ready(_) => Poll::Ready(Err(Error::new(
                "Error",
                "Owned signature completed before the cancellation barrier",
            ))),
        })
        .await?;
        source.cancel(Reason::Cancelled)?;
        work.await
    };
    boundary.close().await?;
    assert!(result.is_err_and(|error| error.code == Some("ROSTER_CANCELLED")));
    fixture.retain_on_drop = false;
    Ok(())
}
