use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::cancel::CancellationToken;
use crate::json::{self, Value};
use crate::paths;
use crate::state::{files, locks, redaction, secure};
use crate::text::Text;

use super::schema;

pub const LOCK_NAME: &str = "provenance-store";
pub const LOG_DIRNAME: &str = "log";
pub const QUARANTINE_DIRNAME: &str = "quarantine";
pub const SEGMENT_NAME: &str = "provenance.log";
pub const MAX_RECORD_BYTES: u64 = 1024 * 1024;
pub const MAX_STORE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_RECORDS: usize = 4096;

fn error(code: &'static str, message: impl Into<String>) -> Error {
    Error::new("ProvenanceStoreError", message).coded(code)
}

fn safe_filename(id: &Text) -> String {
    id.0.iter()
        .map(|unit| {
            if *unit <= 127
                && ((*unit as u8).is_ascii_alphanumeric() || matches!(*unit, 46 | 95 | 45))
            {
                char::from(*unit as u8)
            } else {
                '_'
            }
        })
        .collect()
}

fn json_bytes(bytes: &[u8]) -> Result<Value, Error> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::new("SyntaxError", "Provenance JSON must contain valid UTF-8"))?;
    json::parse(&Text::from(text))
}

pub struct Skipped {
    pub record_id: String,
    pub reason: String,
    pub path: PathBuf,
    snapshot: secure::Snapshot,
}

impl Skipped {
    fn value(&self) -> Value {
        Value::object(vec![
            ("recordId", Value::string(&self.record_id)),
            ("reason", Value::string(&self.reason)),
            ("path", Value::string(self.path.to_string_lossy().as_ref())),
        ])
    }
}

pub struct ReadAll {
    pub records: Vec<Value>,
    pub skipped: Vec<Skipped>,
}

impl ReadAll {
    pub fn json(&self) -> String {
        Value::object(vec![
            ("records", Value::Array(self.records.clone())),
            (
                "skipped",
                Value::Array(self.skipped.iter().map(Skipped::value).collect()),
            ),
        ])
        .json()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendStage {
    RecordSynced,
    RecordCommitted,
    IndexSynced,
}

pub struct Store {
    pub root: PathBuf,
    pub options: locks::Options,
    pub redaction: redaction::Config,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self, Error> {
        if root.as_os_str().is_empty() {
            return Err(Error::type_error("ProvenanceStore requires a root path"));
        }
        let root = paths::absolute(root)?;
        secure::check_components(&root)?;
        Ok(Self {
            root,
            options: locks::Options::default(),
            redaction: redaction::Config::default(),
        })
    }

    fn ensure_layout(&self) -> Result<(), Error> {
        for path in [
            &self.root,
            &self.root.join(LOG_DIRNAME),
            &self.root.join(QUARANTINE_DIRNAME),
        ] {
            secure::private_directory(path, true)?;
        }
        Ok(())
    }

    async fn acquire(&self, token: Option<&CancellationToken>) -> Result<locks::RepoLock, Error> {
        secure::private_directory_if_present(&self.root)?;
        let target = locks::target(LOCK_NAME, &self.root, &self.root, Some(&self.root))?;
        locks::acquire(LOCK_NAME, &target, &self.options, token).await
    }

    pub async fn append_record(
        &self,
        record: &Value,
        token: Option<&CancellationToken>,
    ) -> Result<Value, Error> {
        check_input(record, &self.redaction)?;
        let record = redaction::record(record, &self.redaction)?;
        self.append_prepared_with(&record, token, |_, _| Ok(()))
            .await
    }

    pub(super) async fn append_prepared(
        &self,
        record: &Value,
        token: Option<&CancellationToken>,
    ) -> Result<Value, Error> {
        self.append_prepared_with(record, token, |_, _| Ok(()))
            .await
    }

    pub async fn append_record_with(
        &self,
        record: &Value,
        token: Option<&CancellationToken>,
        observe: impl FnMut(AppendStage, &Path) -> Result<(), Error>,
    ) -> Result<Value, Error> {
        check_input(record, &self.redaction)?;
        let record = redaction::record(record, &self.redaction)?;
        self.append_prepared_with(&record, token, observe).await
    }

    async fn append_prepared_with(
        &self,
        record: &Value,
        token: Option<&CancellationToken>,
        mut observe: impl FnMut(AppendStage, &Path) -> Result<(), Error>,
    ) -> Result<Value, Error> {
        let verdict = schema::validate_record(record);
        if let Some(reason) = verdict.reason {
            return Err(error(
                "E_INVALID_RECORD",
                format!("refusing to append an invalid record: {reason}"),
            ));
        }
        let id = record
            .get("id")
            .and_then(Value::as_string)
            .ok_or_else(|| error("E_INVALID_RECORD", "record.id must be a non-empty string"))?;
        let name = safe_filename(id);
        if id.0.first() == Some(&46) || !id.0.iter().copied().eq(name.encode_utf16()) {
            return Err(error(
                "E_INVALID_RECORD",
                "Record id cannot be persisted without filename rewriting or hiding; use an opaque ASCII identifier.",
            ));
        }
        let bytes = format!("{}\n", record.json()).into_bytes();
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(error(
                "E_RECORD_SIZE",
                "Provenance record exceeds 1 MiB; no record was written.",
            ));
        }
        let mut lock = self.acquire(token).await?;
        let result = (|| {
            lock.check()?;
            if let Some(token) = token {
                token.check()?;
            }
            self.ensure_layout()?;
            let target = self.root.join(LOG_DIRNAME).join(format!("{name}.json"));
            if let Some(existing) = secure::read(&target, MAX_RECORD_BYTES)? {
                let existing = json_bytes(&existing.bytes).map_err(|cause| error("E_RECORD_UNREADABLE", format!("record {} exists but is unreadable; recover or quarantine it before retrying this id.", target.display())).caused_by(cause))?;
                let verdict = schema::validate_record(&existing);
                if verdict.reason.is_none()
                    && existing.get("id") == Some(&Value::String(id.clone()))
                {
                    return Err(error(
                        "E_DUPLICATE_RECORD",
                        format!(
                            "record id {} already has a committed provenance record; this retry is already accepted, not another write.",
                            id.json()
                        ),
                    ));
                }
                return Err(error(
                    "E_RECORD_CONFLICT",
                    format!(
                        "record {} collides with a different or corrupt committed record; preserve and quarantine it before retrying.",
                        target.display()
                    ),
                ));
            }
            self.check_capacity(bytes.len() as u64)?;
            let temporary = self
                .root
                .join(LOG_DIRNAME)
                .join(format!(".{name}.{}.tmp", files::nonce()?));
            secure::write_exclusive(&temporary, &bytes).map_err(|cause| error("E_RECORD_WRITE", "Atomic record write failed before commit; preserve any temporary for recovery.").caused_by(cause))?;
            observe(AppendStage::RecordSynced, &temporary)?;
            lock.check()?;
            if let Some(token) = token {
                token.check()?;
            }
            fs::hard_link(&temporary, &target).map_err(|cause| {
                let code = if cause.kind() == std::io::ErrorKind::AlreadyExists {
                    "E_DUPLICATE_RECORD"
                } else {
                    "E_RECORD_RENAME"
                };
                error(
                    code,
                    format!(
                        "Atomic record commit failed for {}; no existing record was overwritten.",
                        target.display()
                    ),
                )
                .caused_by(secure::io_error(
                    "Commit immutable provenance",
                    &target,
                    cause,
                ))
            })?;
            fs::remove_file(&temporary).map_err(|cause| {
                error(
                    "E_RECORD_COMMITTED",
                    "Record is committed but its temporary remains; run recovery before retrying.",
                )
                .caused_by(secure::io_error(
                    "Remove record temporary",
                    &temporary,
                    cause,
                ))
            })?;
            secure::sync_directory(&self.root.join(LOG_DIRNAME))?;
            observe(AppendStage::RecordCommitted, &target).map_err(|cause| {
                error(
                    "E_RECORD_COMMITTED",
                    "Record is committed; its advisory index is not confirmed.",
                )
                .caused_by(cause)
            })?;
            check_cancel(token, true)?;
            lock.check()?;
            self.append_segment(&bytes).map_err(|cause| error("E_RECORD_COMMITTED", "Record is committed but the advisory index append failed; read record files, then repair the index before retrying.").caused_by(cause))?;
            observe(AppendStage::IndexSynced, &target)?;
            Ok(Value::object(vec![
                ("recordId", Value::String(id.clone())),
                ("path", Value::string(target.to_string_lossy().as_ref())),
            ]))
        })();
        let cleanup = lock.release_required().await;
        let result = locks::finish(result, cleanup)?;
        check_cancel(token, true)?;
        Ok(result)
    }

    fn check_capacity(&self, additional: u64) -> Result<(), Error> {
        let mut count = 0;
        let mut total = additional;
        for name in secure::entries(&self.root.join(LOG_DIRNAME))? {
            if name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }
            count += 1;
            let file = self.root.join(LOG_DIRNAME).join(name);
            let metadata = fs::symlink_metadata(&file)
                .map_err(|cause| secure::io_error("Inspect provenance capacity", &file, cause))?;
            total = total
                .checked_add(metadata.len())
                .ok_or_else(|| error("E_STORE_SIZE", "Provenance byte count overflowed"))?;
            if count >= MAX_RECORDS || total > MAX_STORE_BYTES {
                return Err(error(
                    "E_STORE_SIZE",
                    "Provenance exceeds 4096 records or 32 MiB; explicitly prune/export before appending.",
                ));
            }
        }
        Ok(())
    }

    fn append_segment(&self, bytes: &[u8]) -> Result<(), Error> {
        let path = self.root.join(SEGMENT_NAME);
        let prior = secure::read(&path, MAX_STORE_BYTES)?;
        if prior
            .as_ref()
            .is_some_and(|prior| !prior.bytes.is_empty() && prior.bytes.last() != Some(&b'\n'))
        {
            return Err(error(
                "E_SEGMENT_APPEND",
                "Segment has a truncated final line; preserve it and run repair.",
            ));
        }
        let length = prior.as_ref().map_or(0, |prior| prior.bytes.len());
        if length + bytes.len() > MAX_STORE_BYTES as usize {
            return Err(error("E_SEGMENT_SIZE", "Provenance segment exceeds 32 MiB"));
        }
        let mut options = fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut handle = options
            .open(&path)
            .map_err(|cause| secure::io_error("Open provenance segment", &path, cause))?;
        if let Some(prior) = prior {
            if paths::handle_information(&handle)?.identity != prior.identity {
                return Err(error(
                    "E_SEGMENT_CHANGED",
                    "Segment was replaced while appending; preserve it for recovery.",
                ));
            }
        }
        let written = handle
            .write(bytes)
            .map_err(|cause| secure::io_error("Append provenance segment", &path, cause))?;
        if written != bytes.len() {
            return Err(error(
                "E_SEGMENT_APPEND",
                "Segment append was short; record files remain authoritative, run repair.",
            ));
        }
        handle
            .sync_all()
            .map_err(|cause| secure::io_error("Sync provenance segment", &path, cause))?;
        drop(handle);
        secure::sync_directory(&self.root)
    }

    pub fn read_all(&self) -> Result<ReadAll, Error> {
        secure::private_directory_if_present(&self.root)?;
        secure::private_directory_if_present(&self.root.join(LOG_DIRNAME))?;
        let mut result = ReadAll {
            records: Vec::new(),
            skipped: Vec::new(),
        };
        let mut total = 0_u64;
        let mut count = 0;
        for name in secure::entries(&self.root.join(LOG_DIRNAME))? {
            if name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }
            count += 1;
            if count > MAX_RECORDS {
                return Err(error(
                    "E_STORE_SIZE",
                    "Provenance exceeds 4096 records; explicitly prune/export before reading.",
                ));
            }
            let path = self.root.join(LOG_DIRNAME).join(&name);
            let Some(snapshot) = secure::read(&path, MAX_RECORD_BYTES)? else {
                return Err(error(
                    "E_RECORD_CHANGED",
                    "Provenance changed during read; retry under its store lock.",
                ));
            };
            total += snapshot.bytes.len() as u64;
            if total > MAX_STORE_BYTES {
                return Err(error(
                    "E_STORE_SIZE",
                    "Provenance exceeds 32 MiB; explicitly prune/export before reading.",
                ));
            }
            let record_id = name
                .strip_suffix(".json")
                .ok_or_else(|| {
                    error(
                        "E_INVALID_RECORD",
                        "Committed provenance filename has no JSON suffix",
                    )
                })?
                .to_owned();
            let (parsed, reason) = match json_bytes(&snapshot.bytes) {
                Ok(parsed) => {
                    let reason = schema::validate_record(&parsed).reason.or_else(|| {
                        (parsed.get("id").and_then(Value::as_string)
                            != Some(&Text::from(record_id.as_str())))
                        .then(|| "record id does not match its file name".to_owned())
                    });
                    (Some(parsed), reason)
                }
                Err(cause) => (
                    None,
                    Some(format!("unreadable or corrupt JSON ({})", cause.message)),
                ),
            };
            if let Some(reason) = reason {
                result.skipped.push(Skipped {
                    record_id,
                    reason,
                    path,
                    snapshot,
                });
            } else if let Some(parsed) = parsed {
                result.records.push(parsed);
            }
        }
        Ok(result)
    }

    pub async fn recover(&self, token: Option<&CancellationToken>) -> Result<Value, Error> {
        let mut lock = self.acquire(token).await?;
        let result = (|| {
            self.ensure_layout()?;
            let mut recovered = Vec::new();
            let mut interrupted = Vec::new();
            for (directory, segment) in [
                (self.root.join(LOG_DIRNAME), false),
                (self.root.clone(), true),
            ] {
                for name in secure::entries(&directory)? {
                    let record_id = if segment {
                        let Some(nonce) = name
                            .strip_prefix("provenance.log.")
                            .and_then(|name| name.strip_suffix(".tmp"))
                        else {
                            continue;
                        };
                        if !hex_nonce(nonce) {
                            continue;
                        }
                        None
                    } else {
                        let Some(body) = name
                            .strip_prefix('.')
                            .and_then(|name| name.strip_suffix(".tmp"))
                        else {
                            continue;
                        };
                        let Some((id, nonce)) = body.rsplit_once('.') else {
                            continue;
                        };
                        if !hex_nonce(nonce) || id.is_empty() {
                            continue;
                        }
                        Some(id.to_owned())
                    };
                    let path = directory.join(&name);
                    let Some(snapshot) = secure::read(&path, MAX_STORE_BYTES)? else {
                        continue;
                    };
                    lock.check()?;
                    if let Some(token) = token {
                        token.check()?;
                    }
                    if !secure::remove_matching(&path, &snapshot, MAX_STORE_BYTES)? {
                        return Err(error(
                            "E_RECORD_CHANGED",
                            "Interrupted write changed during recovery; preserve it for inspection.",
                        ));
                    }
                    let id = record_id.as_deref().map_or(Value::Null, Value::string);
                    interrupted.push(Value::object(vec![
                        ("recordId", id.clone()),
                        ("incomplete", Value::Boolean(true)),
                        ("path", Value::string(path.to_string_lossy().as_ref())),
                        (
                            "kind",
                            Value::string(if segment {
                                "segment-append"
                            } else {
                                "record-write"
                            }),
                        ),
                    ]));
                    recovered.push(Value::object(vec![
                        ("recordId", id),
                        (
                            "action",
                            Value::string(if segment {
                                "removed-interrupted-append"
                            } else {
                                "removed-interrupted-write"
                            }),
                        ),
                    ]));
                }
                secure::sync_directory(&directory)?;
            }
            Ok(Value::object(vec![
                ("recovered", Value::Array(recovered)),
                ("interrupted", Value::Array(interrupted)),
            ]))
        })();
        let cleanup = lock.release_required().await;
        let result = locks::finish(result, cleanup)?;
        check_cancel(token, false)?;
        Ok(result)
    }

    pub async fn repair(&self, token: Option<&CancellationToken>) -> Result<Value, Error> {
        let mut lock = self.acquire(token).await?;
        let result = (|| {
            self.ensure_layout()?;
            let scanned = self.read_all()?;
            let mut quarantined = Vec::new();
            for (index, skipped) in scanned.skipped.iter().enumerate() {
                lock.check()?;
                if let Some(token) = token {
                    token.check()?;
                }
                if !secure::unchanged(&skipped.path, &skipped.snapshot, MAX_RECORD_BYTES)? {
                    return Err(error(
                        "E_RECORD_CHANGED",
                        "Record changed during repair; no changed entry was removed.",
                    ));
                }
                let name = skipped
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        error("E_QUARANTINE", "Quarantine source requires a Unicode name")
                    })?;
                let destination = self.quarantine_copy(
                    &format!("{name}.{}.quarantined", index + 1),
                    &skipped.snapshot.bytes,
                )?;
                if !secure::remove_matching(&skipped.path, &skipped.snapshot, MAX_RECORD_BYTES)? {
                    return Err(error(
                        "E_RECORD_CHANGED",
                        "Record changed after quarantine copy; the original was not removed.",
                    ));
                }
                quarantined.push(Value::object(vec![
                    ("recordId", Value::string(&skipped.record_id)),
                    ("reason", Value::string(&skipped.reason)),
                    (
                        "from",
                        Value::string(skipped.path.to_string_lossy().as_ref()),
                    ),
                    ("to", Value::string(destination.to_string_lossy().as_ref())),
                ]));
            }
            let index_repair = self.rebuild_segment(&scanned.records, &lock)?;
            secure::sync_directory(&self.root.join(LOG_DIRNAME))?;
            let mut report = Value::object(vec![
                ("records", Value::Number(scanned.records.len() as f64)),
                ("quarantined", Value::Array(quarantined)),
            ]);
            if let Some(index_repair) = index_repair {
                crate::domain::put(&mut report, "indexRepair", index_repair)?;
            }
            Ok(report)
        })();
        let cleanup = lock.release_required().await;
        let result = locks::finish(result, cleanup)?;
        check_cancel(token, false)?;
        Ok(result)
    }

    fn quarantine_copy(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, Error> {
        let mut destination = self.root.join(QUARANTINE_DIRNAME).join(name);
        if secure::read(&destination, MAX_STORE_BYTES)?.is_some() {
            destination = self
                .root
                .join(QUARANTINE_DIRNAME)
                .join(format!("{name}.{}", files::nonce()?));
        }
        secure::commit_new(&destination, bytes).map_err(|cause| {
            error(
                "E_QUARANTINE",
                "Quarantine copy failed; original evidence is preserved.",
            )
            .caused_by(cause)
        })?;
        let copied = secure::read(&destination, MAX_STORE_BYTES)?.ok_or_else(|| {
            error(
                "E_QUARANTINE",
                "Quarantine copy disappeared; original evidence is preserved.",
            )
        })?;
        if copied.bytes != bytes {
            return Err(error(
                "E_QUARANTINE",
                "Quarantine copy did not verify; original evidence is preserved.",
            ));
        }
        Ok(destination)
    }

    fn rebuild_segment(
        &self,
        records: &[Value],
        lock: &locks::RepoLock,
    ) -> Result<Option<Value>, Error> {
        let mut expected = Vec::new();
        for record in records {
            let line = format!("{}\n", record.json());
            if expected.len() + line.len() > MAX_STORE_BYTES as usize {
                return Err(error("E_SEGMENT_SIZE", "Rebuilt segment exceeds 32 MiB"));
            }
            expected.extend_from_slice(line.as_bytes());
        }
        let path = self.root.join(SEGMENT_NAME);
        let old = secure::read(&path, MAX_STORE_BYTES)?;
        if old.as_ref().is_some_and(|old| old.bytes == expected) {
            return Ok(None);
        }
        let backup = old
            .as_ref()
            .map(|old| self.quarantine_copy("provenance.log.rebuilt.quarantined", &old.bytes))
            .transpose()?;
        lock.check()?;
        if let Some(old) = old {
            if !secure::unchanged(&path, &old, MAX_STORE_BYTES)? {
                return Err(error(
                    "E_SEGMENT_CHANGED",
                    "Segment changed during repair; preserved both copies.",
                ));
            }
        }
        files::atomic_write_bytes(&path, &expected, MAX_STORE_BYTES)?;
        secure::sync_directory(&self.root)?;
        Ok(Some(Value::object(vec![
            ("action", Value::string("rebuilt-advisory-index")),
            (
                "preserved",
                backup.as_ref().map_or(Value::Null, |path| {
                    Value::string(path.to_string_lossy().as_ref())
                }),
            ),
        ])))
    }
}

fn check_cancel(token: Option<&CancellationToken>, committed: bool) -> Result<(), Error> {
    if let Some(token) = token {
        token.check().map_err(|cause| {
            if committed {
                cause.caused_by(error("E_RECORD_COMMITTED", "Provenance record committed before cancellation; query the authoritative record files before retrying."))
            } else {
                cause
            }
        })?;
    }
    Ok(())
}

fn check_input(record: &Value, config: &redaction::Config) -> Result<(), Error> {
    config.validate()?;
    if let Some(reason) = schema::validate_record(record).reason {
        let reason = redaction::evidence(&Text::from(reason.as_str()), config).to_utf8()?;
        return Err(error(
            "E_INVALID_RECORD",
            format!("refusing to append an invalid record: {reason}"),
        ));
    }
    if record.json().len() + 1 > MAX_RECORD_BYTES as usize {
        return Err(error(
            "E_RECORD_SIZE",
            "Provenance record exceeds 1 MiB; no record was written.",
        ));
    }
    Ok(())
}

fn hex_nonce(value: &str) -> bool {
    value.len() >= 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
