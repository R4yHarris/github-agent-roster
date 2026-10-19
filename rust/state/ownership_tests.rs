use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::Error;
use crate::cancel::{CancellationSource, Reason};
use crate::json::Value;
use crate::state::{files, locks, migrate, provenance};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixture() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("roster-native-state-{}", files::nonce()?));
    let mut directory = fs::DirBuilder::new();
    directory.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(&root)?;
    Ok(root)
}

fn target(root: &Path, name: &str) -> Result<PathBuf, Error> {
    locks::target(name, root, root, Some(root))
}

#[cfg(windows)]
struct RestrictedOwner {
    root: PathBuf,
    path: PathBuf,
    handle: fs::File,
    bytes: Vec<u8>,
}

#[cfg(windows)]
fn restricted_owner(name: &str) -> Result<RestrictedOwner, Box<dyn std::error::Error>> {
    use std::os::windows::fs::OpenOptionsExt;

    let root = fixture()?;
    let path = target(&root, name)?;
    files::create_private_directories(
        path.parent()
            .ok_or_else(|| Error::type_error("Missing parent"))?,
    )?;
    let bytes = format!(
        "{{\"holder\":\"restricted-owner\",\"pid\":{}}}\n",
        std::process::id()
    )
    .into_bytes();
    fs::write(&path, &bytes)?;
    let handle = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&path)?;
    let reading = crate::state::secure::read(&path, 16 * 1024);
    assert!(
        reading
            .as_ref()
            .is_err_and(|error| error.code == Some("EBUSY")),
        "{reading:?}"
    );
    Ok(RestrictedOwner {
        root,
        path,
        handle,
        bytes,
    })
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn windows_lock_wait_requires_exclusive_creation_after_restricted_owner_release() -> TestResult
{
    let RestrictedOwner {
        root, path, handle, ..
    } = restricted_owner("restricted-release")?;
    let options = locks::Options {
        wait: Duration::from_secs(2),
        poll: Duration::from_millis(1),
        ..locks::Options::default()
    };
    let started = Instant::now();
    let (acquired, released) = tokio::join!(
        async {
            let result = locks::acquire("restricted-release", &path, &options, None).await;
            (started.elapsed(), result)
        },
        async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            drop(handle);
            fs::remove_file(&path)
        }
    );
    released?;
    let (elapsed, result) = acquired;
    let result = match result {
        Ok(mut owner) => {
            assert!(elapsed >= Duration::from_millis(25));
            owner.release_required().await
        }
        Err(error) => Err(error),
    };
    fs::remove_dir_all(root)?;
    result?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn windows_lock_wait_deadline_preserves_inaccessible_owner_and_actual_error() -> TestResult {
    for wait in [Duration::ZERO, Duration::from_millis(10)] {
        let RestrictedOwner {
            root,
            path,
            handle,
            bytes,
        } = restricted_owner("restricted-deadline")?;
        let options = locks::Options {
            wait,
            poll: Duration::from_millis(1),
            ..locks::Options::default()
        };
        let started = Instant::now();
        let result = locks::acquire("restricted-deadline", &path, &options, None).await;
        let elapsed = started.elapsed();
        let code = result.err().and_then(|error| error.code);
        drop(handle);
        let preserved = fs::read(&path)?;
        fs::remove_dir_all(root)?;
        assert_eq!(code, Some("EBUSY"));
        assert_eq!(preserved, bytes);
        assert!(elapsed >= wait);
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn windows_lock_wait_cancellation_preserves_inaccessible_owner() -> TestResult {
    let RestrictedOwner {
        root,
        path,
        handle,
        bytes,
    } = restricted_owner("restricted-cancellation")?;
    let options = locks::Options {
        wait: Duration::from_secs(2),
        poll: Duration::from_millis(1),
        ..locks::Options::default()
    };
    let source = CancellationSource::default();
    let token = source.token();
    let (result, cancelled) = tokio::join!(
        locks::acquire("restricted-cancellation", &path, &options, Some(&token)),
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            source.cancel(Reason::Cancelled)
        }
    );
    let code = result.err().and_then(|error| error.code);
    drop(handle);
    let preserved = fs::read(&path)?;
    fs::remove_dir_all(root)?;
    assert!(cancelled?);
    assert_eq!(code, Some("ROSTER_CANCELLED"));
    assert_eq!(preserved, bytes);
    Ok(())
}

fn record(id: &str) -> Value {
    Value::object(vec![
        ("id", Value::string(id)),
        ("version", Value::Number(1.0)),
        ("runId", Value::string("run")),
        ("sessionId", Value::string("session")),
        ("event", Value::string("started")),
        (
            "metrics",
            Value::object(vec![
                ("tokens_prompt", Value::Number(0.0)),
                ("tokens_completion", Value::string("unknown")),
            ]),
        ),
    ])
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_and_unknown_lock_owners_survive_without_a_death_probe() -> TestResult {
    let root = fixture()?;
    let path = target(&root, "unknown")?;
    files::create_private_directories(
        path.parent()
            .ok_or_else(|| Error::type_error("Missing parent"))?,
    )?;
    for source in [
        r#"{"holder":"owner","pid":0}"#,
        r#"{"holder":"owner","pid":1.5}"#,
        r#"{"holder":"owner","pid":"12"}"#,
        "null",
        "{}",
        "{broken",
    ] {
        fs::write(&path, source)?;
        let probes = Cell::new(0);
        let result =
            locks::acquire_with_probe("unknown", &path, &locks::Options::default(), None, |_| {
                probes.set(probes.get() + 1);
                Ok(locks::Liveness::Dead)
            })
            .await;
        assert!(result.is_err_and(|error| error.code == Some("E_LOCK_HELD")));
        assert_eq!(probes.get(), 0);
        assert_eq!(fs::read(&path)?, source.as_bytes());
    }
    fs::remove_dir_all(root)?;
    Ok(())
}

async fn serialized_lock_rejection(options: locks::Options, run: bool) -> TestResult {
    let root = fixture()?;
    let path = target(&root, "serialized-bound")?;
    let result = if run {
        locks::acquire_run(&path, &options, None).await
    } else {
        locks::acquire("serialized-bound", &path, &options, None).await
    };
    let accepted = result.is_ok();
    let code = result.err().and_then(|error| error.code);
    let owner_exists = path.exists();
    let entries = fs::read_dir(&root)?.count();
    fs::remove_dir_all(root)?;
    assert!(!accepted, "oversized serialized metadata was accepted");
    assert_eq!(code, Some("E_LOCK_SIZE"));
    assert!(!owner_exists, "rejected metadata created an owner marker");
    assert_eq!(entries, 0, "metadata rejection changed the lock root");
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn serialized_lock_holder_bound_accounts_for_json_escaping() -> TestResult {
    serialized_lock_rejection(
        locks::Options {
            holder: "\0".repeat(4096),
            ..locks::Options::default()
        },
        false,
    )
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn serialized_lock_run_id_bound_accounts_for_all_record_bytes() -> TestResult {
    for run_id in ["r".repeat(16 * 1024), "\0".repeat(4096)] {
        serialized_lock_rejection(
            locks::Options {
                run_id: Some(run_id),
                ..locks::Options::default()
            },
            true,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn serialized_lock_limit_includes_newline_and_accepted_records_release() -> TestResult {
    for run in [false, true] {
        let mut options = locks::Options::default();
        let stamp = crate::clock::now_iso()?;
        let empty = if run {
            Value::object(vec![
                ("pid", Value::Number(std::process::id() as f64)),
                ("runId", Value::string("")),
                ("acquiredAt", Value::string(&stamp)),
            ])
        } else {
            Value::object(vec![
                ("holder", Value::string("")),
                ("pid", Value::Number(std::process::id() as f64)),
                ("acquiredAt", Value::string(&stamp)),
            ])
        };
        let remaining = (16_usize * 1024)
            .checked_sub(empty.json().len() + 1)
            .ok_or_else(|| Error::type_error("Lock fixture overhead exceeds its bound"))?;
        if run {
            options.run_id = Some("r".repeat(remaining));
        } else {
            options.holder = format!(
                "{}{}",
                "\0".repeat(remaining / 6),
                "x".repeat(remaining % 6)
            );
            assert!(options.holder.encode_utf16().count() <= 4096);
        }
        let root = fixture()?;
        let path = target(&root, "exact-bound")?;
        let mut lock = if run {
            locks::acquire_run(&path, &options, None).await?
        } else {
            locks::acquire("exact-bound", &path, &options, None).await?
        };
        assert_eq!(fs::read(&path)?.len(), 16 * 1024);
        lock.check()?;
        lock.release_required().await?;
        assert!(!path.exists());
        fs::remove_dir_all(root)?;
        if run {
            options
                .run_id
                .as_mut()
                .ok_or_else(|| Error::type_error("Missing run fixture"))?
                .push('x');
        } else {
            options.holder.push('x');
        }
        serialized_lock_rejection(options, run).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn serialized_lock_preflight_preserves_existing_owners_without_probing() -> TestResult {
    for run in [false, true] {
        let root = fixture()?;
        let path = target(&root, "preflight")?;
        files::create_private_directories(
            path.parent()
                .ok_or_else(|| Error::type_error("Missing lock parent"))?,
        )?;
        let bytes = b" {\"holder\":\"prior-owner\",\"pid\":123,\"acquiredAt\":\"old\"}\r\n";
        fs::write(&path, bytes)?;
        let options = if run {
            locks::Options {
                run_id: Some("\0".repeat(4096)),
                format: locks::RecordFormat::Run,
                ..locks::Options::default()
            }
        } else {
            locks::Options {
                holder: "\0".repeat(4096),
                ..locks::Options::default()
            }
        };
        let probes = Cell::new(0);
        let result = locks::acquire_with_probe("preflight", &path, &options, None, |_| {
            probes.set(probes.get() + 1);
            Ok(locks::Liveness::Dead)
        })
        .await;
        assert!(result.is_err_and(|error| error.code == Some("E_LOCK_SIZE")));
        assert_eq!(probes.get(), 0);
        assert_eq!(fs::read(&path)?, bytes);
        assert_eq!(
            fs::read_dir(
                path.parent()
                    .ok_or_else(|| Error::type_error("Missing lock parent"))?
            )?
            .count(),
            1
        );
        fs::remove_dir_all(root)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn stale_probe_cannot_unlink_a_replacement_actor() -> TestResult {
    let root = fixture()?;
    let path = target(&root, "race")?;
    files::create_private_directories(
        path.parent()
            .ok_or_else(|| Error::type_error("Missing parent"))?,
    )?;
    fs::write(&path, r#"{"holder":"reused","pid":123,"acquiredAt":"old"}"#)?;
    let replacement = format!(
        r#"{{"holder":"reused","pid":{},"acquiredAt":"new"}}"#,
        std::process::id()
    );
    let probes = Cell::new(0);
    let result =
        locks::acquire_with_probe("race", &path, &locks::Options::default(), None, |pid| {
            probes.set(probes.get() + 1);
            if pid == 123 {
                fs::write(&path, &replacement)
                    .map_err(|error| Error::new("Error", error.to_string()))?;
                Ok(locks::Liveness::Dead)
            } else {
                Ok(locks::Liveness::Alive)
            }
        })
        .await;
    assert!(result.is_err_and(|error| error.code == Some("E_LOCK_HELD")));
    assert_eq!(probes.get(), 2);
    assert_eq!(fs::read_to_string(&path)?, replacement);
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn release_is_bound_to_file_ownership_not_a_reused_pid_or_holder() -> TestResult {
    let root = fixture()?;
    let path = target(&root, "release")?;
    let options = locks::Options {
        holder: "same-holder".to_owned(),
        ..locks::Options::default()
    };
    let mut first = locks::acquire("release", &path, &options, None).await?;
    fs::remove_file(&path)?;
    let mut second = locks::acquire("release", &path, &options, None).await?;
    assert!(!first.release().await?);
    second.check()?;
    assert!(second.release().await?);
    assert!(!second.release().await?);
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn lock_wait_is_cancellable_without_releasing_the_live_owner() -> TestResult {
    let root = fixture()?;
    let path = target(&root, "cancel")?;
    let mut owner = locks::acquire("cancel", &path, &locks::Options::default(), None).await?;
    let source = CancellationSource::default();
    let token = source.token();
    let options = locks::Options {
        wait: Duration::from_secs(10),
        ..locks::Options::default()
    };
    let start = Instant::now();
    let (result, cancelled) = tokio::join!(
        locks::acquire("cancel", &path, &options, Some(&token)),
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            source.cancel(Reason::Cancelled)
        }
    );
    assert!(cancelled?);
    assert!(result.is_err_and(|error| error.code == Some("ROSTER_CANCELLED")));
    assert!(start.elapsed() < Duration::from_secs(1));
    owner.check()?;
    owner.release_required().await?;
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn probe_errors_and_unknown_liveness_preserve_the_original_bytes() -> TestResult {
    let root = fixture()?;
    let path = target(&root, "probe")?;
    files::create_private_directories(
        path.parent()
            .ok_or_else(|| Error::type_error("Missing parent"))?,
    )?;
    let bytes = b" {\"holder\":\"owner\",\"pid\":123,\"acquiredAt\":\"1900-01-01\"}\r\n";
    fs::write(&path, bytes)?;
    let failed =
        locks::acquire_with_probe("probe", &path, &locks::Options::default(), None, |_| {
            Err(Error::new("Error", "access probe unavailable"))
        })
        .await;
    assert!(failed.is_err_and(|error| error.code == Some("E_LOCK_PROBE")));
    let unknown =
        locks::acquire_with_probe("probe", &path, &locks::Options::default(), None, |_| {
            Ok(locks::Liveness::Unknown)
        })
        .await;
    assert!(unknown.is_err_and(|error| error.code == Some("E_LOCK_HELD")));
    assert_eq!(fs::read(&path)?, bytes);
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn actual_owned_child_wait_proves_death_for_the_specific_recorded_pid() -> TestResult {
    use std::process::{Command, Stdio};
    let node = std::env::var_os("ROSTER_TEST_NODE").ok_or_else(|| {
        Error::new(
            "Error",
            "ROSTER_TEST_NODE is required for owned death-proof fixtures",
        )
    })?;
    let mut child = Command::new(node)
        .args(["-e", "process.stdin.resume()"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let pid = child.id() as u64;
    let alive = locks::probe_pid(pid);
    child.kill()?;
    child.wait()?;
    assert_eq!(alive?, locks::Liveness::Alive);
    assert_eq!(locks::probe_pid(pid)?, locks::Liveness::Dead);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn unrepresentable_record_ids_are_refused_before_they_can_become_unreadable_commits()
-> TestResult {
    let root = fixture()?;
    let store = provenance::Store::open(&root)?;
    for id in [".hidden", "../escaped", "contains space", "unicode-雪"] {
        let failed = store.append_record(&record(id), None).await;
        assert!(failed.is_err_and(|error| error.code == Some("E_INVALID_RECORD")));
        assert_eq!(fs::read_dir(&root)?.count(), 0);
    }
    store.append_record(&record("valid.json"), None).await?;
    assert_eq!(store.read_all()?.records, [record("valid.json")]);
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn redaction_configuration_failure_cannot_initialize_or_write_the_store() -> TestResult {
    let root = fixture()?;
    let mut store = provenance::Store::open(&root)?;
    store.redaction.environment = Value::Null;
    let failed = store.append_record(&record("refused"), None).await;
    assert!(failed.is_err_and(|error| error.name == "TypeError"));
    assert_eq!(fs::read_dir(&root)?.count(), 0);
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn interrupted_record_and_committed_record_have_different_recovery_evidence() -> TestResult {
    let root = fixture()?;
    let store = provenance::Store::open(&root)?;
    let failed = store
        .append_record_with(&record("torn"), None, |stage, _| {
            if stage == provenance::store::AppendStage::RecordSynced {
                Err(Error::new("Error", "owned pre-commit failure"))
            } else {
                Ok(())
            }
        })
        .await;
    assert!(failed.is_err());
    assert!(store.read_all()?.records.is_empty());
    let recovery = store.recover(None).await?;
    assert!(
        recovery
            .json()
            .contains("\"recordId\":\"torn\",\"incomplete\":true")
    );
    assert!(!root.join("log").join("torn.json").exists());
    let failed = store
        .append_record_with(&record("committed"), None, |stage, _| {
            if stage == provenance::store::AppendStage::RecordCommitted {
                Err(Error::new("Error", "owned post-commit failure"))
            } else {
                Ok(())
            }
        })
        .await;
    assert!(failed.is_err_and(|error| error.code == Some("E_RECORD_COMMITTED")));
    assert_eq!(store.read_all()?.records, [record("committed")]);
    store.repair(None).await?;
    assert!(fs::read_to_string(root.join("provenance.log"))?.contains("\"id\":\"committed\""));
    assert!(
        store
            .append_record(&record("committed"), None)
            .await
            .is_err_and(|error| error.code == Some("E_DUPLICATE_RECORD"))
    );
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_revokes_late_append_success_without_discarding_committed_evidence()
-> TestResult {
    for stage in [
        provenance::store::AppendStage::RecordSynced,
        provenance::store::AppendStage::RecordCommitted,
        provenance::store::AppendStage::IndexSynced,
    ] {
        let root = fixture()?;
        let store = provenance::Store::open(&root)?;
        let source = CancellationSource::default();
        let token = source.token();
        let failed = store
            .append_record_with(&record("cancelled"), Some(&token), |observed, _| {
                if observed == stage {
                    source.cancel(Reason::TaskTimeout(10))?;
                }
                Ok(())
            })
            .await;
        let error = failed
            .err()
            .ok_or_else(|| Error::new("Error", "Late cancellation was accepted"))?;
        assert_eq!(error.code, Some("ROSTER_TASK_TIMEOUT"));
        if stage == provenance::store::AppendStage::RecordSynced {
            assert!(store.read_all()?.records.is_empty());
        } else {
            assert_eq!(store.read_all()?.records, [record("cancelled")]);
            assert!(
                error
                    .cause
                    .as_ref()
                    .is_some_and(|cause| cause.code == Some("E_RECORD_COMMITTED"))
            );
        }
        store.append_record(&record("next"), None).await?;
        fs::remove_dir_all(root)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn lost_writer_ownership_cannot_be_reported_as_a_successful_append() -> TestResult {
    let root = fixture()?;
    let store = provenance::Store::open(&root)?;
    let marker = root.join("locks").join("provenance-store.lock");
    let failed = store
        .append_record_with(&record("refused"), None, |stage, _| {
            if stage == provenance::store::AppendStage::RecordSynced {
                fs::write(&marker, b"{\"holder\":\"independent\",\"pid\":null}")
                    .map_err(|error| Error::new("Error", error.to_string()))?;
            }
            Ok(())
        })
        .await;
    assert!(
        failed.is_err_and(|error| error.code == Some("E_LOCK_RELEASE")
            && error.message.contains("no longer owned"))
    );
    assert!(!root.join("log").join("refused.json").exists());
    assert!(fs::read_to_string(marker)?.contains("independent"));
    fs::remove_dir_all(root)?;
    Ok(())
}

fn legacy(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    files::create_private_directories(&root.join(".roster").join("runs"))?;
    fs::write(
        root.join(".roster").join("runs").join("a.json"),
        b"{\"run\":1}\n",
    )?;
    fs::write(
        root.join(".roster").join("runs").join("b.json"),
        b"{\"run\":2}\n",
    )?;
    fs::write(
        root.join(".roster").join("config.yml"),
        b"api_key: test-only-private-api-key\n",
    )?;
    Ok(())
}

async fn synthetic_pending_refresh()
-> Result<(PathBuf, migrate::Layout, Value), Box<dyn std::error::Error>> {
    let root = fixture()?;
    legacy(&root)?;
    migrate::migrate(&root, false).await?;
    let layout = migrate::Layout::resolve(&root)?;
    let mut receipt = migrate::read_record(&root)?
        .ok_or_else(|| Error::type_error("Missing completed migration receipt"))?;
    let mut entries = Vec::new();
    for (name, bytes) in [
        ("a.json", b"{\"run\":3}\n".as_slice()),
        ("b.json", b"{\"run\":4}\n".as_slice()),
    ] {
        let old = fs::read(layout.destination.join("runs").join(name))?;
        fs::write(layout.source.join("runs").join(name), bytes)?;
        if name == "a.json" {
            fs::write(layout.destination.join("runs").join(name), bytes)?;
        }
        entries.push(Value::object(vec![
            ("path", Value::string(&format!("runs/{name}"))),
            ("sha256", Value::string(&crate::identity::sha256_hex(bytes))),
            (
                "previousSha256",
                Value::string(&crate::identity::sha256_hex(&old)),
            ),
        ]));
    }
    crate::domain::put(&mut receipt, "status", Value::string("in-progress"))?;
    crate::domain::put(&mut receipt, "files", Value::Array(entries))?;
    files::atomic_write_file(
        &layout.record,
        &crate::text::Text::from(format!("{}\n", receipt.pretty()).as_str()),
    )?;
    Ok((root, layout, receipt))
}

#[tokio::test(flavor = "current_thread")]
async fn pending_refresh_rollback_restores_prior_bytes_and_exact_pending_receipt() -> TestResult {
    let (root, layout, _) = synthetic_pending_refresh().await?;
    let receipt = fs::read(&layout.record)?;
    let pending_bytes = fs::read(layout.destination.join("runs").join("b.json"))?;
    let failed = migrate::migrate_with(&root, false, "fixture-time", locks::probe_pid, |_, _| {
        Err(Error::new("Error", "refresh rollback fixture"))
    })
    .await;
    assert!(failed.is_err_and(|error| error.code == Some("E_ROLLBACK")));
    assert_eq!(fs::read(&layout.record)?, receipt);
    assert_eq!(
        fs::read(layout.destination.join("runs").join("b.json"))?,
        pending_bytes
    );
    let dry_run = migrate::migrate(&root, true).await?;
    assert_eq!(
        dry_run
            .get("planned")
            .and_then(|planned| planned.get("repairs")),
        Some(&Value::Number(1.0))
    );
    migrate::migrate(&root, false).await?;
    assert_eq!(
        fs::read(layout.destination.join("runs").join("b.json"))?,
        b"{\"run\":4}\n"
    );
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn pending_refresh_previous_checksums_are_validated_without_authorizing_complete_receipts()
-> TestResult {
    let (root, layout, receipt) = synthetic_pending_refresh().await?;
    let entries = match receipt.get("files") {
        Some(Value::Array(entries)) => entries.clone(),
        _ => return Err(Error::type_error("Missing receipt fixture files").into()),
    };
    for invalid in [
        Value::Null,
        Value::Number(0.0),
        Value::string(&"a".repeat(63)),
        Value::string(&"G".repeat(64)),
        Value::Array(Vec::new()),
    ] {
        let mut entries = entries.clone();
        crate::domain::put(
            entries
                .get_mut(1)
                .ok_or_else(|| Error::type_error("Missing pending file"))?,
            "previousSha256",
            invalid,
        )?;
        let mut invalid_receipt = receipt.clone();
        crate::domain::put(&mut invalid_receipt, "files", Value::Array(entries))?;
        files::atomic_write_file(
            &layout.record,
            &crate::text::Text::from(invalid_receipt.json().as_str()),
        )?;
        let bytes = fs::read(&layout.record)?;
        assert!(
            migrate::read_record(&root).is_err_and(|error| error.code == Some("E_CORRUPT_STATE"))
        );
        assert_eq!(fs::read(&layout.record)?, bytes);
    }
    let mut completed = receipt;
    crate::domain::put(&mut completed, "status", Value::string("complete"))?;
    files::atomic_write_file(
        &layout.record,
        &crate::text::Text::from(completed.json().as_str()),
    )?;
    assert!(
        migrate::migrate(&root, true)
            .await
            .is_err_and(|error| error.code == Some("E_CONFLICT"))
    );
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_pending_refresh_without_previous_proof_remains_blocked() -> TestResult {
    let (root, layout, mut receipt) = synthetic_pending_refresh().await?;
    let mut entries = match receipt.get("files") {
        Some(Value::Array(entries)) => entries.clone(),
        _ => return Err(Error::type_error("Missing receipt fixture files").into()),
    };
    for entry in &mut entries {
        crate::domain::remove(entry, "previousSha256")?;
    }
    crate::domain::put(&mut receipt, "files", Value::Array(entries))?;
    files::atomic_write_file(
        &layout.record,
        &crate::text::Text::from(receipt.json().as_str()),
    )?;
    let bytes = fs::read(&layout.record)?;
    assert!(
        migrate::migrate(&root, true)
            .await
            .is_err_and(|error| error.code == Some("E_CONFLICT"))
    );
    assert_eq!(fs::read(&layout.record)?, bytes);
    assert_eq!(
        fs::read(layout.destination.join("runs").join("b.json"))?,
        b"{\"run\":2}\n"
    );
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn failed_migration_rolls_back_owned_copies_and_never_changes_legacy() -> TestResult {
    let root = fixture()?;
    legacy(&root)?;
    let layout = migrate::Layout::resolve(&root)?;
    let failed = migrate::migrate_with(&root, false, "fixture-time", locks::probe_pid, |_, _| {
        Err(Error::new("Error", "owned copy fault"))
    })
    .await;
    assert!(failed.is_err_and(|error| error.code == Some("E_ROLLBACK")));
    assert!(!layout.destination.join("runs").join("a.json").exists());
    assert_eq!(
        fs::read(root.join(".roster").join("runs").join("a.json"))?,
        b"{\"run\":1}\n"
    );
    assert_eq!(migrate::read_record(&root)?, None);
    assert!(layout.backup.is_file());
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rollback_preserves_an_independent_destination_edit_and_reports_incomplete_recovery()
-> TestResult {
    let root = fixture()?;
    legacy(&root)?;
    let layout = migrate::Layout::resolve(&root)?;
    let failed =
        migrate::migrate_with(&root, false, "fixture-time", locks::probe_pid, |_, path| {
            fs::write(path, b"independent destination edit")
                .map_err(|error| Error::new("Error", error.to_string()))?;
            Err(Error::new(
                "Error",
                "copy observer failed after independent edit",
            ))
        })
        .await;
    assert!(failed.is_err_and(|error| error.code == Some("E_ROLLBACK_FAILED")));
    assert_eq!(
        fs::read(layout.destination.join("runs").join("a.json"))?,
        b"independent destination edit"
    );
    assert!(
        migrate::read_record(&root)?
            .is_some_and(|record| record.get("status") == Some(&Value::string("in-progress")))
    );
    assert_eq!(
        fs::read(root.join(".roster").join("runs").join("a.json"))?,
        b"{\"run\":1}\n"
    );
    fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn a_raced_destination_is_not_overwritten_and_owned_staging_is_rolled_back() -> TestResult {
    let root = fixture()?;
    legacy(&root)?;
    let layout = migrate::Layout::resolve(&root)?;
    let second = layout.destination.join("runs").join("b.json");
    let failed = migrate::migrate_with(
        &root,
        false,
        "fixture-time",
        locks::probe_pid,
        |index, _| {
            if index == 0 {
                fs::write(&second, b"independent actor")
                    .map_err(|error| Error::new("Error", error.to_string()))?;
            }
            Ok(())
        },
    )
    .await;
    assert!(failed.is_err_and(|error| error.code == Some("E_ROLLBACK")));
    assert_eq!(fs::read(second)?, b"independent actor");
    assert!(!layout.destination.join("runs").join("a.json").exists());
    assert!(
        !layout
            .destination
            .join("runs")
            .join("b.json.migrate-tmp")
            .exists()
    );
    assert_eq!(migrate::read_record(&root)?, None);
    fs::remove_dir_all(root)?;
    Ok(())
}
