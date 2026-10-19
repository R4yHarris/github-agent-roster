use super::*;
use crate::cancel::{CancellationSource, Reason};
use std::future::Future;
use std::process::Command;
use std::task::Poll;

struct Fixture {
    container: PathBuf,
    root: PathBuf,
}

fn io(error: std::io::Error) -> Error {
    Error::new("BlobFixtureError", error.to_string())
}

impl Fixture {
    fn new(format: &str) -> Result<Self, Error> {
        let container = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".blob-batch-{}-{}",
            std::process::id(),
            crate::state::files::nonce()?
        ));
        let root = container.join("project");
        files::create_private_directories(&root)?;
        let fixture = Self { container, root };
        fixture.git(&["init", "--quiet", &format!("--object-format={format}")])?;
        fixture.write(".gitignore", b".roster/\n")?;
        fixture.write("a.bin", b"Owned blob.\n")?;
        fixture.write("empty", b"")?;
        fixture.write("repeat.bin", b"Owned blob.\n")?;
        fixture.write("run.sh", b"Owned executable bytes.\n")?;
        fixture.write("TASK.md", b"Immutable task.\n")?;
        fixture.write("RECIPE.yml", b"Immutable recipe.\n")?;
        fixture.git(&["add", "--all"])?;
        fixture.git(&["update-index", "--chmod=+x", "run.sh"])?;
        fixture.commit()?;
        Ok(fixture)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let path = self.root.join(name);
        files::create_private_directories(
            path.parent()
                .ok_or_else(|| denied("Fixture parent missing"))?,
        )?;
        fs::write(path, bytes).map_err(io)
    }

    fn git_bytes(&self, root: &Path, args: &[&str]) -> Result<Vec<u8>, Error> {
        let mut command = Command::new("git");
        command
            .current_dir(root)
            .env_clear()
            .envs(tools::Host::default().environment)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["--no-pager", "-c", "init.templateDir=", "-c"])
            .arg(format!(
                "core.hooksPath={}",
                self.container.join("no-hooks").display()
            ))
            .args(args);
        let output = command.output().map_err(io)?;
        if !output.status.success() {
            return Err(Error::new(
                "BlobFixtureError",
                "Owned fixture Git setup failed",
            ));
        }
        Ok(output.stdout)
    }

    fn git(&self, args: &[&str]) -> Result<String, Error> {
        tools::bytes_text(&self.git_bytes(&self.root, args)?)
    }

    fn commit(&self) -> Result<(), Error> {
        self.git(&[
            "-c",
            "user.name=BlobFixture",
            "-c",
            "user.email=blob@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "Owned immutable blob fixture",
        ])?;
        Ok(())
    }

    fn head(&self) -> Result<String, Error> {
        Ok(self
            .git(&["rev-parse", "--verify", "HEAD"])?
            .trim()
            .to_owned())
    }
    fn object(&self, name: &str) -> Result<blob::Object, Error> {
        let oid = self.git(&["rev-parse", &format!("HEAD:{name}")])?;
        let bytes = self.git_bytes(&self.root, &["cat-file", "blob", oid.trim()])?;
        blob::Object::new(
            oid.trim(),
            u64::try_from(bytes.len()).map_err(|_| denied("Fixture size overflow"))?,
        )
    }

    async fn lease(
        &self,
        root: &Path,
        relative: bool,
        source: &CancellationSource,
    ) -> Result<backend::RootLease, Error> {
        let git = if relative {
            PathBuf::from("git")
        } else {
            std::env::var_os("ROSTER_TEST_GIT")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("PATH").and_then(|value| {
                        std::env::split_paths(&value)
                            .map(|path| path.join("git.exe"))
                            .find(|path| path.is_file())
                    })
                })
                .ok_or_else(|| denied("Owned blob fixture requires installed Git"))?
        };
        let git = if relative {
            git
        } else {
            crate::host::Image::approve(&git)?.path().to_owned()
        };
        backend::RootLease::open(
            root,
            "blob-batch-fixture",
            tools::Host {
                git,
                ..tools::Host::default()
            },
            crate::state::redaction::Config::default(),
            source.token(),
        )
        .await
    }

    fn finish(self) -> Result<(), Error> {
        fs::remove_dir_all(&self.container).map_err(io)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.container.exists() {
            eprintln!("Owned blob fixture retained: {}", self.container.display());
        }
    }
}

async fn compare_materialized(
    fixture: &Fixture,
    root: &Path,
    relative: bool,
) -> Result<Vec<(String, String)>, Error> {
    let source = CancellationSource::default();
    let mut lease = fixture.lease(root, relative, &source).await?;
    let owner = backend::Owner::PrePlan(&lease);
    let scratch = Scratch::create(owner).await?;
    let result = async {
        let head = tools::bytes_text(
            &owner
                .git(
                    root,
                    &["rev-parse", "--verify", "HEAD"].map(str::to_owned),
                    false,
                )
                .await?,
        )?
        .trim()
        .to_owned();
        let index_path = paths::repository_paths(root)?.git_dir.join("index");
        let index = fs::read(&index_path).map_err(io)?;
        let config =
            fs::read(paths::repository_paths(root)?.common_dir.join("config")).map_err(io)?;
        let base = scratch.base(&head, &backend::Approval::default()).await?;
        let tree = tools::bytes_text(
            &owner
                .git(
                    root,
                    &["ls-tree", "-r", "-z", &head].map(str::to_owned),
                    false,
                )
                .await?,
        )?;
        let mut seen = Vec::new();
        for entry in tree.split('\0').filter(|entry| !entry.is_empty()) {
            let (metadata, name) = entry
                .split_once('\t')
                .ok_or_else(|| denied("Oracle tree framing"))?;
            let parts = metadata.split(' ').collect::<Vec<_>>();
            let [mode, kind, oid] = parts.as_slice() else {
                return Err(denied("Oracle tree metadata"));
            };
            if crate::planning::forbidden_read(name)
                || policy::managed(name)
                || name.starts_with(".roster/")
            {
                assert!(!base.join(name).exists());
                continue;
            }
            assert_eq!(*kind, "blob");
            let old = owner
                .git(root, &["cat-file", "blob", oid].map(str::to_owned), false)
                .await?;
            let materialized = fs::read(base.join(name)).map_err(io)?;
            assert!(old == materialized);
            let mode_line = tools::bytes_text(
                &owner
                    .git(
                        &base,
                        &["ls-files", "--stage", "--", name].map(str::to_owned),
                        false,
                    )
                    .await?,
            )?;
            assert!(mode_line.starts_with(mode));
            seen.push((name.to_owned(), crate::identity::sha256_hex(&materialized)));
        }
        assert_eq!(fs::read(index_path).map_err(io)?, index);
        assert_eq!(
            fs::read(paths::repository_paths(root)?.common_dir.join("config")).map_err(io)?,
            config
        );
        assert_eq!(
            fs::read(root.join("TASK.md")).map_err(io)?,
            b"Immutable task.\n"
        );
        assert_eq!(
            fs::read(root.join("RECIPE.yml")).map_err(io)?,
            b"Immutable recipe.\n"
        );
        assert_eq!(
            fixture.git_bytes(&base, &["rev-parse", "HEAD"])?,
            format!("{head}\n").as_bytes()
        );
        Ok(seen)
    }
    .await;
    let result = backend::finish_owned(owner, result, scratch.close().await);
    crate::state::locks::finish(result, lease.close().await)
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_actual_owned_batch_matches_per_blob_binary_duplicate_ordinals_both_hashes()
-> Result<(), Error> {
    for format in ["sha1", "sha256"] {
        let fixture = Fixture::new(format)?;
        fixture.write("binary", &[0, b'\n', 255, b'\r', 0])?;
        fixture.git(&["add", "binary"])?;
        fixture.commit()?;
        let source = CancellationSource::default();
        let mut lease = fixture.lease(&fixture.root, false, &source).await?;
        let owner = backend::Owner::PrePlan(&lease);
        let objects = [
            fixture.object("binary")?,
            fixture.object("empty")?,
            fixture.object("binary")?,
            fixture.object("a.bin")?,
        ];
        let output = owner.git_blob_chunk(&fixture.root, &objects).await?;
        for (index, object) in objects.iter().enumerate() {
            let legacy = owner
                .git(
                    &fixture.root,
                    &["cat-file", "blob", object.oid()].map(str::to_owned),
                    false,
                )
                .await?;
            assert!(output.body(index)? == legacy);
        }
        lease.close().await?;
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_materialization_matches_legacy_main_linked_detached_and_relative_compat_both_hashes()
-> Result<(), Error> {
    for format in ["sha1", "sha256"] {
        let fixture = Fixture::new(format)?;
        let native = compare_materialized(&fixture, &fixture.root, false).await?;
        let legacy = compare_materialized(&fixture, &fixture.root, true).await?;
        assert_eq!(native, legacy);
        let linked = fixture.container.join("linked 'quoted' 東京");
        let linked_name = linked
            .to_str()
            .ok_or_else(|| denied("Linked fixture path encoding"))?;
        fixture.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "owned-linked",
            linked_name,
            "HEAD",
        ])?;
        assert_eq!(
            compare_materialized(&fixture, &linked, false).await?,
            native
        );
        fixture.git(&["checkout", "--quiet", "--detach", "HEAD"])?;
        assert_eq!(
            compare_materialized(&fixture, &fixture.root, false).await?,
            native
        );
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_materialization_valid_full_128_mib_is_chunked_and_managed_oversize_does_not_count()
-> Result<(), Error> {
    let fixture = Fixture::new("sha1")?;
    fixture.git(&["rm", "--quiet", "a.bin", "empty", "repeat.bin", "run.sh"])?;
    fixture.write(".gitignore", b"")?;
    files::create_private_directories(&fixture.root.join(".git").join("info"))?;
    fs::write(
        fixture.root.join(".git").join("info").join("exclude"),
        b".roster/\n",
    )
    .map_err(io)?;
    let body = vec![0; 16 * 1024 * 1024];
    for index in 0..8 {
        fixture.write(&format!("part-{index}.bin"), &body)?;
    }
    fixture.write("CONTEXT.md", &vec![0; 17 * 1024 * 1024])?;
    fixture.git(&["add", "--all"])?;
    fixture.commit()?;
    let observed = compare_materialized(&fixture, &fixture.root, false).await?;
    assert_eq!(observed.len(), 9);
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_materialization_preserves_preclone_package_refusal_and_mode_before_managed_skip()
-> Result<(), Error> {
    let fixture = Fixture::new("sha1")?;
    fixture.write("package.json", br#"{"name":"github-agent-roster"}"#)?;
    fixture.git(&["add", "package.json"])?;
    fixture.commit()?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&fixture.root, false, &source).await?;
    let owner = backend::Owner::PrePlan(&lease);
    let scratch = Scratch::create(owner).await?;
    let error = scratch
        .base(&fixture.head()?, &backend::Approval::default())
        .await
        .err()
        .ok_or_else(|| denied("Retired package was materialized"))?;
    assert_eq!(error.code, Some("ROSTER_NATIVE_UNSUPPORTED"));
    assert!(!scratch.path.join("base").exists());
    scratch.close().await?;
    lease.close().await?;
    fixture.git(&["rm", "--quiet", "package.json"])?;
    let oid = fixture.git(&["rev-parse", "HEAD:a.bin"])?;
    fixture.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        "120000",
        oid.trim(),
        "CONTEXT.md",
    ])?;
    fixture.commit()?;
    let mut lease = fixture.lease(&fixture.root, false, &source).await?;
    let scratch = Scratch::create(backend::Owner::PrePlan(&lease)).await?;
    let error = scratch
        .base(&fixture.head()?, &backend::Approval::default())
        .await
        .err()
        .ok_or_else(|| denied("Special mode was silently skipped"))?;
    assert_eq!(error.code, Some("ROSTER_NATIVE_UNSUPPORTED"));
    scratch.close().await?;
    lease.close().await?;
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_materialization_approved_submodule_recursion_keeps_local_limits_and_depth()
-> Result<(), Error> {
    let fixture = Fixture::new("sha1")?;
    let module = Fixture::new("sha1")?;
    let module_head = module.head()?;
    fixture.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        "160000",
        &module_head,
        "module",
    ])?;
    fixture.commit()?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&fixture.root, false, &source).await?;
    let owner = backend::Owner::PrePlan(&lease);
    let scratch = Scratch::create(owner).await?;
    let approval = backend::Approval {
        submodules: vec![backend::LocalSubmodule {
            path: "module".to_owned(),
            source: module.root.clone(),
        }],
        ..Default::default()
    };
    let base = scratch.base(&fixture.head()?, &approval).await?;
    assert_eq!(
        fs::read(base.join("module").join("a.bin")).map_err(io)?,
        b"Owned blob.\n"
    );
    let refused = scratch.path.join("too-deep");
    assert!(
        scratch
            .materialize(
                &fixture.root,
                &fixture.head()?,
                &refused,
                &approval.submodules,
                5
            )
            .await
            .is_err()
    );
    assert!(!refused.exists());
    let at_four = scratch.path.join("at-four");
    scratch
        .materialize(&module.root, &module_head, &at_four, &[], 4)
        .await?;
    assert_eq!(
        fs::read(at_four.join("a.bin")).map_err(io)?,
        b"Owned blob.\n"
    );
    scratch.close().await?;
    lease.close().await?;
    fixture.finish()?;
    module.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_actual_owner_rejects_bad_frames_overflow_start_errors_and_relative_batch_route()
-> Result<(), Error> {
    let fixture = Fixture::new("sha1")?;
    fixture.write("large", &vec![0; 16 * 1024 * 1024])?;
    fixture.git(&["add", "large"])?;
    fixture.commit()?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&fixture.root, false, &source).await?;
    let owner = backend::Owner::PrePlan(&lease);
    let real = fixture.object("a.bin")?;
    assert!(
        owner
            .git_blob_chunk(&fixture.root, &[blob::Object::new(real.oid(), 0)?])
            .await
            .is_err()
    );
    assert!(
        owner
            .git_blob_chunk(&fixture.root, &[blob::Object::new(&"a".repeat(40), 0)?])
            .await
            .is_err()
    );
    assert!(
        owner
            .git_blob_chunk(&fixture.root, &[blob::Object::new(&fixture.head()?, 0)?])
            .await
            .is_err()
    );
    let large = fixture.object("large")?;
    let overflow = owner
        .git_blob_chunk(&fixture.root, &[blob::Object::new(large.oid(), 0)?])
        .await
        .err()
        .ok_or_else(|| denied("Actual Git output exceeded its size plan without refusal"))?;
    assert_eq!(overflow.code, Some("ERR_CHILD_PROCESS_STDIO_MAXBUFFER"));
    assert!(
        overflow
            .cause
            .is_some_and(|facts| facts.message.contains("submitted=true")
                && facts.message.contains("cleanup_complete=true"))
    );
    let missing = owner
        .git_blob_chunk(
            &fixture.container.join("missing"),
            std::slice::from_ref(&real),
        )
        .await
        .err()
        .ok_or_else(|| denied("Missing cwd unexpectedly ran"))?;
    assert_eq!(missing.code, Some("ROSTER_PROCESS_START"));
    assert!(owner.cleanup_certain());
    lease.close().await?;
    let mut relative = fixture.lease(&fixture.root, true, &source).await?;
    assert_eq!(
        backend::Owner::PrePlan(&relative)
            .git_blob_chunk(&fixture.root, std::slice::from_ref(&real))
            .await
            .err()
            .and_then(|error| error.code),
        Some("ROSTER_NATIVE_UNSUPPORTED")
    );
    relative.close().await?;
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_actual_owner_pending_cancellation_and_task_deadline_keep_known_cleanup()
-> Result<(), Error> {
    for timed in [false, true] {
        let fixture = Fixture::new("sha1")?;
        fixture.write("large", &vec![0; 16 * 1024 * 1024])?;
        fixture.git(&["add", "large"])?;
        fixture.commit()?;
        let object = fixture.object("large")?;
        let objects = [object];
        let source = CancellationSource::default();
        let mut lease = fixture.lease(&fixture.root, false, &source).await?;
        let owner = backend::Owner::PrePlan(&lease);
        let mut operation = Box::pin(owner.git_blob_chunk(&fixture.root, &objects));
        std::future::poll_fn(|context| match operation.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(Ok(())),
            Poll::Ready(_) => Poll::Ready(Err(denied(
                "Actual batch did not reach its pending I/O barrier",
            ))),
        })
        .await?;
        if timed {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            source.cancel(Reason::TaskTimeout(1))?;
        } else {
            source.cancel(Reason::Cancelled)?;
        }
        let error = operation
            .await
            .err()
            .ok_or_else(|| denied("Cancelled owned blob batch succeeded"))?;
        assert_eq!(
            error.code,
            Some(if timed {
                "ROSTER_TASK_TIMEOUT"
            } else {
                "ROSTER_CANCELLED"
            })
        );
        assert!(
            error
                .cause
                .is_some_and(|facts| facts.message.contains("submitted=true")
                    && facts.message.contains("cleanup_complete=true"))
        );
        assert!(owner.cleanup_certain());
        lease.close().await?;
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_owned_batch_keeps_source_index_ref_config_and_owner_current_obligations()
-> Result<(), Error> {
    for change in ["source", "index", "ref", "config"] {
        let fixture = Fixture::new("sha1")?;
        let source = CancellationSource::default();
        let mut lease = fixture.lease(&fixture.root, false, &source).await?;
        let owner = backend::Owner::PrePlan(&lease);
        let stamp = backend::Stamp::capture(owner).await?;
        let object = fixture.object("a.bin")?;
        owner
            .git_blob_chunk(&fixture.root, std::slice::from_ref(&object))
            .await?;
        match change {
            "source" => fixture.write("a.bin", b"Changed source.\n")?,
            "index" => {
                fixture.write("a.bin", b"Changed staged source.\n")?;
                fixture.git(&["add", "a.bin"])?;
            }
            "config" => {
                fixture.git(&["config", "--local", "core.longpaths", "true"])?;
            }
            _ => {
                let tree = fixture.git(&["rev-parse", "HEAD^{tree}"])?;
                let next = fixture.git(&[
                    "-c",
                    "user.name=BlobFixture",
                    "-c",
                    "user.email=blob@example.invalid",
                    "commit-tree",
                    tree.trim(),
                    "-p",
                    &fixture.head()?,
                    "-m",
                    "Changed fixture ref",
                ])?;
                fixture.git(&["update-ref", "HEAD", next.trim()])?;
            }
        }
        assert!(stamp.require_current(owner).await.is_err());
        lease.close().await?;
        fixture.finish()?;
    }
    let fixture = Fixture::new("sha1")?;
    let source = CancellationSource::default();
    let mut first = fixture.lease(&fixture.root, false, &source).await?;
    let stamp = backend::Stamp::capture(backend::Owner::PrePlan(&first)).await?;
    first.close().await?;
    drop(first);
    let mut second = fixture.lease(&fixture.root, false, &source).await?;
    assert!(
        stamp
            .require_current(backend::Owner::PrePlan(&second))
            .await
            .is_err()
    );
    second.close().await?;
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_whole_chunk_failure_and_cancellation_write_no_partial_materialization()
-> Result<(), Error> {
    let fixture = Fixture::new("sha1")?;
    fixture.write("large", &vec![0; 16 * 1024 * 1024])?;
    fixture.git(&["add", "large"])?;
    fixture.commit()?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&fixture.root, false, &source).await?;
    let owner = backend::Owner::PrePlan(&lease);
    let scratch = Scratch::create(owner).await?;
    let destination = scratch.path.join("chunk-failure");
    owner
        .git(
            owner.root(),
            &[
                "clone".to_owned(),
                "--quiet".to_owned(),
                "--shared".to_owned(),
                "--no-checkout".to_owned(),
                "--".to_owned(),
                fixture
                    .root
                    .to_str()
                    .ok_or_else(|| denied("Fixture path encoding"))?
                    .to_owned(),
                destination
                    .to_str()
                    .ok_or_else(|| denied("Fixture path encoding"))?
                    .to_owned(),
            ],
            false,
        )
        .await?;
    let good = fixture.object("a.bin")?;
    let large = fixture.object("large")?;
    for bad in [
        blob::Object::new(good.oid(), 0)?,
        blob::Object::new(&"a".repeat(40), 0)?,
        blob::Object::new(&fixture.head()?, 0)?,
        blob::Object::new(large.oid(), 0)?,
    ] {
        let selected = vec![
            ("first.bin".to_owned(), "100644".to_owned(), good.clone()),
            ("last.bin".to_owned(), "100644".to_owned(), bad),
        ];
        assert!(
            scratch
                .materialize_chunked(&destination, &selected)
                .await
                .is_err()
        );
        assert!(!destination.join("first.bin").exists() && !destination.join("last.bin").exists());
        assert!(owner.cleanup_certain());
    }
    let duplicate = vec![
        (
            "duplicate.bin".to_owned(),
            "100644".to_owned(),
            good.clone(),
        ),
        ("duplicate.bin".to_owned(), "100644".to_owned(), good),
    ];
    assert!(
        scratch
            .materialize_chunked(&destination, &duplicate)
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(destination.join("duplicate.bin")).map_err(io)?,
        b"Owned blob.\n"
    );
    fs::remove_file(destination.join("duplicate.bin")).map_err(io)?;
    let selected = vec![("cancelled.bin".to_owned(), "100644".to_owned(), large)];
    let mut operation = Box::pin(scratch.materialize_chunked(&destination, &selected));
    std::future::poll_fn(|context| match operation.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(Ok(())),
        Poll::Ready(_) => Poll::Ready(Err(denied(
            "Materialization did not reach pending owned batch",
        ))),
    })
    .await?;
    source.cancel(Reason::Cancelled)?;
    assert_eq!(
        operation.await.err().and_then(|error| error.code),
        Some("ROSTER_CANCELLED")
    );
    assert!(!destination.join("cancelled.bin").exists());
    assert!(owner.cleanup_certain());
    scratch.close().await?;
    lease.close().await?;
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_abandoned_pending_owned_batch_quarantines_actual_writer_and_scratch()
-> Result<(), Error> {
    let fixture = Fixture::new("sha1")?;
    fixture.write("large", &vec![0; 16 * 1024 * 1024])?;
    fixture.git(&["add", "large"])?;
    fixture.commit()?;
    let object = fixture.object("large")?;
    let objects = [object];
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&fixture.root, false, &source).await?;
    let owner = backend::Owner::PrePlan(&lease);
    let scratch = Scratch::create(owner).await?;
    let retained = scratch.path.clone();
    let mut operation = Box::pin(owner.git_blob_chunk(&fixture.root, &objects));
    std::future::poll_fn(|context| match operation.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(Ok(())),
        Poll::Ready(_) => Poll::Ready(Err(denied("Owned batch did not reach abandonment barrier"))),
    })
    .await?;
    drop(operation);
    assert!(!owner.cleanup_certain());
    assert!(scratch.close().await.is_err());
    assert!(retained.exists() && lease.close().await.is_err());
    drop(lease);
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn blob_native_future_frame_sizes_are_bounded_scalar_measurements() -> Result<(), Error> {
    let fixture = Fixture::new("sha1")?;
    let source = CancellationSource::default();
    let mut lease = fixture.lease(&fixture.root, false, &source).await?;
    let owner = backend::Owner::PrePlan(&lease);
    let objects = [fixture.object("a.bin")?];
    let batch = std::mem::size_of_val(&owner.git_blob_chunk(&fixture.root, &objects));
    let args = [
        "cat-file",
        "blob",
        objects
            .first()
            .ok_or_else(|| denied("Fixture object missing"))?
            .oid(),
    ]
    .map(str::to_owned);
    let legacy = std::mem::size_of_val(&owner.git(&fixture.root, &args, false));
    let scratch = Scratch::create(owner).await?;
    let head = fixture.head()?;
    let destination = scratch.path.join("measured");
    let materialize =
        std::mem::size_of_val(&scratch.materialize(&fixture.root, &head, &destination, &[], 0));
    eprintln!(
        "Native future frame bytes: blob_batch={batch}; legacy_git={legacy}; materialize={materialize}"
    );
    assert!(batch > 0 && legacy > 0 && materialize > 0);
    scratch.close().await?;
    lease.close().await?;
    fixture.finish()
}
