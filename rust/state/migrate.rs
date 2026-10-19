use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::Error;
use crate::clock;
use crate::domain;
use crate::identity::sha256_hex;
use crate::json::{self, Value};
use crate::paths;
use crate::state::{files, layout, locks, secure};
use crate::text::Text;

const ID: &str = "split-layout-v1";
const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FILES: usize = 4096;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const ENTRIES: [&str; 2] = ["checkpoints", "runs"];

fn failure(code: &'static str, message: impl Into<String>) -> Error {
    Error::new("MigrationError", format!("{}. Recovery: preserve the legacy .roster directory, compare .roster-state/migration/backup.json, then re-run with dryRun first.", message.into())).coded(code)
}

pub struct Layout {
    pub root: PathBuf,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub record: PathBuf,
    pub backup: PathBuf,
}

impl Layout {
    pub fn resolve(root: &Path) -> Result<Self, Error> {
        let root = paths::absolute(root)?;
        secure::check_components(&root)?;
        let destination =
            layout::resolve_directory(layout::Scope::PerWorktree, &root, Some(&root), None)?.dir;
        let migration = root.join(".roster-state").join("migration");
        Ok(Self {
            source: root.join(".roster"),
            destination,
            record: migration.join("record.json"),
            backup: migration.join("backup.json"),
            root,
        })
    }

    fn relative(&self, path: &Path) -> Result<String, Error> {
        relative(&self.root, path)
    }

    pub fn json(&self) -> Result<String, Error> {
        Ok(Value::object(vec![
            ("root", path_value(&self.root)),
            ("source", path_value(&self.source)),
            ("destination", path_value(&self.destination)),
            ("sourceLayout", Value::string(".roster")),
            (
                "destinationLayout",
                Value::string(&self.relative(&self.destination)?),
            ),
            ("record", path_value(&self.record)),
            ("backup", path_value(&self.backup)),
        ])
        .json())
    }
}

fn path_value(path: &Path) -> Value {
    Value::string(path.to_string_lossy().as_ref())
}

fn relative(root: &Path, path: &Path) -> Result<String, Error> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| failure("E_CONFLICT", "Migration path escaped its declared root"))?;
    relative
        .to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| failure("E_CORRUPT_STATE", "Migration path is not Unicode"))
}

fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.contains('\\')
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && path
            .components()
            .next()
            .is_some_and(|part| ENTRIES.iter().any(|entry| part.as_os_str() == *entry))
}

fn read_json(path: &Path) -> Result<Option<(Value, secure::Snapshot)>, Error> {
    secure::read(path, MAX_FILE_BYTES)?
        .map(|snapshot| {
            let source = std::str::from_utf8(&snapshot.bytes).map_err(|_| {
                failure(
                    "E_CORRUPT_STATE",
                    format!("Migration record {} is not UTF-8", path.display()),
                )
            })?;
            let value = json::parse(&Text::from(source)).map_err(|cause| {
                failure(
                    "E_CORRUPT_STATE",
                    format!("Migration record {} is not valid JSON", path.display()),
                )
                .caused_by(cause)
            })?;
            Ok((value, snapshot))
        })
        .transpose()
}

#[derive(Clone)]
struct FileHashes {
    sha256: String,
    previous_sha256: Option<String>,
}

impl FileHashes {
    fn accepts(&self, actual: &str, pending: bool) -> bool {
        self.sha256 == actual || pending && self.previous_sha256.as_deref() == Some(actual)
    }
}

fn checksum(file: &Value, field: &str) -> Result<String, Error> {
    file.get(field)
        .and_then(Value::as_string)
        .filter(|hash| {
            hash.0.len() == 64
                && hash
                    .0
                    .iter()
                    .all(|unit| (48..=57).contains(unit) || (97..=102).contains(unit))
        })
        .map(Text::to_utf8)
        .transpose()?
        .ok_or_else(|| {
            failure(
                "E_CORRUPT_STATE",
                format!("Migration record contains an invalid {field} checksum"),
            )
        })
}

fn recorded_values(recorded: &BTreeMap<String, FileHashes>, pending: bool) -> Value {
    Value::Array(
        recorded
            .iter()
            .map(|(path, hashes)| {
                let mut fields = vec![
                    ("path", Value::string(path)),
                    ("sha256", Value::string(&hashes.sha256)),
                ];
                if let Some(previous) = hashes.previous_sha256.as_deref().filter(|_| pending) {
                    fields.push(("previousSha256", Value::string(previous)));
                }
                Value::object(fields)
            })
            .collect(),
    )
}

fn recorded_files(record: &Value) -> Result<BTreeMap<String, FileHashes>, Error> {
    let files = record
        .get("files")
        .and_then(domain::items)
        .ok_or_else(|| failure("E_CORRUPT_STATE", "Migration record has no file manifest"))?;
    if files.len() > MAX_FILES {
        return Err(failure(
            "E_CORRUPT_STATE",
            "Migration manifest exceeds 4096 files",
        ));
    }
    let mut result = BTreeMap::new();
    for file in files {
        let path = file
            .get("path")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .filter(|path| safe_relative(path))
            .ok_or_else(|| {
                failure(
                    "E_CORRUPT_STATE",
                    "Migration record contains an unsafe path",
                )
            })?;
        let sha256 = checksum(file, "sha256")?;
        let previous_sha256 = file
            .get("previousSha256")
            .map(|_| checksum(file, "previousSha256"))
            .transpose()?
            .filter(|_| domain::is_text(record.get("status"), "in-progress"));
        if result
            .insert(
                path,
                FileHashes {
                    sha256,
                    previous_sha256,
                },
            )
            .is_some()
        {
            return Err(failure(
                "E_CORRUPT_STATE",
                "Migration record contains duplicate paths",
            ));
        }
    }
    Ok(result)
}

pub fn read_record(root: &Path) -> Result<Option<Value>, Error> {
    read_previous(&Layout::resolve(root)?).map(|previous| previous.map(|(value, _)| value))
}

fn read_previous(layout: &Layout) -> Result<Option<(Value, secure::Snapshot)>, Error> {
    let Some((value, snapshot)) = read_json(&layout.record)? else {
        return Ok(None);
    };
    if !domain::is_text(value.get("id"), ID)
        || value.get("version").and_then(Value::as_number) != Some(1.0)
        || !(domain::is_text(value.get("status"), "in-progress")
            || domain::is_text(value.get("status"), "complete"))
    {
        return Err(failure(
            "E_CORRUPT_STATE",
            format!(
                "Migration record {} does not match the split-layout-v1 record shape",
                layout.record.display()
            ),
        ));
    }
    recorded_files(&value)?;
    Ok(Some((value, snapshot)))
}

pub fn detect_active(
    root: &Path,
    probe: impl Fn(u64) -> Result<locks::Liveness, Error>,
) -> Result<Value, Error> {
    detect_active_inner(&Layout::resolve(root)?, &probe, false)
}

fn detect_active_inner(
    layout: &Layout,
    probe: &impl Fn(u64) -> Result<locks::Liveness, Error>,
    own_migration: bool,
) -> Result<Value, Error> {
    let mut active = Vec::new();
    for directory in [
        layout.source.join("locks"),
        layout.destination.join("locks"),
    ] {
        for name in secure::entries(&directory)? {
            if !name.ends_with(".lock")
                || own_migration
                    && directory == layout.source.join("locks")
                    && name == "migrate.lock"
            {
                continue;
            }
            let path = directory.join(name);
            let Some(snapshot) = secure::read(&path, 16 * 1024)? else {
                continue;
            };
            let record = std::str::from_utf8(&snapshot.bytes)
                .ok()
                .and_then(|source| json::parse(&Text::from(source)).ok());
            let pid = record
                .as_ref()
                .and_then(|record| domain::safe_integer(record.get("pid")))
                .filter(|pid| *pid > 0);
            let dead = match pid {
                Some(pid) => probe(pid)? == locks::Liveness::Dead,
                None => false,
            };
            if !dead {
                active.push(Value::object(vec![
                    ("path", path_value(&path)),
                    (
                        "holder",
                        record
                            .as_ref()
                            .and_then(|record| record.get("holder"))
                            .cloned()
                            .unwrap_or_else(|| Value::string("unreadable lock record")),
                    ),
                    (
                        "pid",
                        pid.map_or(Value::Null, |pid| Value::Number(pid as f64)),
                    ),
                ]));
            }
        }
    }
    Ok(Value::object(vec![
        ("active", Value::Boolean(!active.is_empty())),
        ("locks", Value::Array(active)),
    ]))
}

fn list_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    secure::check_components(root)?;
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(secure::io_error("Inspect migration source", root, error)),
    };
    if metadata.is_file() {
        return Ok(vec![root.to_owned()]);
    }
    if !metadata.is_dir() {
        return Err(failure(
            "E_CORRUPT_STATE",
            "Migration entry is not a regular file or directory",
        ));
    }
    let mut files = Vec::new();
    let mut pending = vec![root.to_owned()];
    let mut inspected = 0;
    while let Some(directory) = pending.pop() {
        for name in secure::entries(&directory)? {
            inspected += 1;
            if inspected > MAX_FILES * 2 {
                return Err(failure(
                    "E_CORRUPT_STATE",
                    "Migration tree exceeds the bounded entry count",
                ));
            }
            let path = directory.join(name);
            let metadata = fs::symlink_metadata(&path)
                .map_err(|cause| secure::io_error("Inspect migration entry", &path, cause))?;
            if secure::linked(&metadata) {
                return Err(secure::path_error(&path, "is linked"));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                files.push(path);
            } else {
                return Err(failure(
                    "E_CORRUPT_STATE",
                    "Migration entry is not a regular file",
                ));
            }
            if files.len() > MAX_FILES {
                return Err(failure("E_CORRUPT_STATE", "Migration exceeds 4096 files"));
            }
        }
    }
    files.sort();
    Ok(files)
}

struct Move {
    from: PathBuf,
    to: PathBuf,
    hash: String,
    source: secure::Snapshot,
    previous: Option<secure::Snapshot>,
    written: Option<secure::Snapshot>,
    staged: Option<(PathBuf, secure::Snapshot)>,
    unverified_stage: bool,
}

struct Plan {
    moves: Vec<Move>,
    temporaries: Vec<PathBuf>,
    already: usize,
    recorded: BTreeMap<String, FileHashes>,
}

fn plan(layout: &Layout, previous: Option<&Value>) -> Result<Plan, Error> {
    let owned = previous
        .map(recorded_files)
        .transpose()?
        .unwrap_or_default();
    let mut result = Plan {
        moves: Vec::new(),
        temporaries: Vec::new(),
        already: 0,
        recorded: owned.clone(),
    };
    let pending = previous.is_some_and(|value| domain::is_text(value.get("status"), "in-progress"));
    let mut sources = BTreeSet::new();
    let mut total = 0_u64;
    for entry in ENTRIES {
        for path in list_files(&layout.destination.join(entry))? {
            if path
                .to_str()
                .is_some_and(|path| path.ends_with(".migrate-tmp"))
            {
                result.temporaries.push(path);
            }
        }
        for from in list_files(&layout.source.join(entry))? {
            let relative = relative(&layout.source, &from)?;
            sources.insert(relative.clone());
            let source = secure::read(&from, MAX_FILE_BYTES)?.ok_or_else(|| {
                failure(
                    "E_CORRUPT_STATE",
                    "Legacy state changed during migration planning",
                )
            })?;
            total += source.bytes.len() as u64;
            if total > MAX_BYTES {
                return Err(failure("E_CORRUPT_STATE", "Migration exceeds 32 MiB"));
            }
            let hash = sha256_hex(&source.bytes);
            let to = layout.destination.join(&relative);
            let current = secure::read(&to, MAX_FILE_BYTES)?;
            let current_hash = current.as_ref().map(|current| sha256_hex(&current.bytes));
            let recorded = owned.get(&relative);
            if recorded.is_some_and(|recorded| {
                current_hash
                    .as_deref()
                    .map_or(pending && recorded.previous_sha256.is_some(), |actual| {
                        !recorded.accepts(actual, pending)
                    })
            }) {
                return Err(failure(
                    "E_CONFLICT",
                    format!(
                        "Recorded migration destination {} matches neither its authorized intended nor previous checksum; independent edits are never overwritten",
                        to.display()
                    ),
                ));
            }
            if current_hash.as_deref() == Some(hash.as_str()) {
                result.already += 1;
                if recorded.is_some_and(|recorded| recorded.accepts(&hash, pending)) {
                    result.recorded.insert(
                        relative,
                        FileHashes {
                            sha256: hash,
                            previous_sha256: None,
                        },
                    );
                }
                continue;
            }
            if let Some(actual) = current_hash.as_deref() {
                if !recorded.is_some_and(|recorded| recorded.accepts(actual, pending)) {
                    return Err(failure(
                        "E_CONFLICT",
                        format!(
                            "Split-layout file {} differs from {} and its recorded owned checksum; independent edits are never overwritten",
                            to.display(),
                            from.display()
                        ),
                    ));
                }
            }
            result.recorded.insert(
                relative.clone(),
                FileHashes {
                    sha256: hash.clone(),
                    previous_sha256: current_hash,
                },
            );
            result.moves.push(Move {
                from,
                to,
                hash,
                source,
                previous: current,
                written: None,
                staged: None,
                unverified_stage: false,
            });
        }
    }
    if pending {
        for (path, hashes) in &owned {
            if hashes.previous_sha256.is_some() && !sources.contains(path) {
                return Err(failure(
                    "E_CONFLICT",
                    "A pending replacement source is missing; preserve both layouts before retrying.",
                ));
            }
        }
    }
    for path in &result.temporaries {
        let relative = relative(&layout.destination, path)?;
        let target = relative
            .strip_suffix(".migrate-tmp")
            .ok_or_else(|| failure("E_CONFLICT", "Invalid migration temporary"))?;
        if !previous.is_some_and(|previous| domain::is_text(previous.get("status"), "in-progress"))
            || !owned.contains_key(target)
        {
            return Err(failure(
                "E_CONFLICT",
                format!(
                    "Unattributed migration temporary {} is preserved",
                    path.display()
                ),
            ));
        }
    }
    Ok(result)
}

fn build_report(
    layout: &Layout,
    plan: &Plan,
    previous: Option<&Value>,
    active: Value,
    dry_run: bool,
) -> Result<Value, Error> {
    let retained = secure::entries(&layout.source)?
        .into_iter()
        .filter(|name| !ENTRIES.contains(&name.as_str()))
        .map(|name| Value::string(&name))
        .collect();
    Ok(Value::object(vec![
        ("id", Value::string(ID)),
        ("dryRun", Value::Boolean(dry_run)),
        ("applied", Value::Boolean(false)),
        ("sourceLayout", Value::string(".roster")),
        (
            "destinationLayout",
            Value::string(&layout.relative(&layout.destination)?),
        ),
        ("activeRun", active),
        (
            "interrupted",
            Value::Boolean(
                previous.is_some_and(|value| domain::is_text(value.get("status"), "in-progress")),
            ),
        ),
        (
            "planned",
            Value::object(vec![
                ("files", Value::Number(plan.moves.len() as f64)),
                (
                    "bytes",
                    Value::Number(
                        plan.moves
                            .iter()
                            .map(|item| item.source.bytes.len())
                            .sum::<usize>() as f64,
                    ),
                ),
                (
                    "repairs",
                    Value::Number(
                        plan.moves
                            .iter()
                            .filter(|item| item.previous.is_some())
                            .count() as f64,
                    ),
                ),
                ("temporaries", Value::Number(plan.temporaries.len() as f64)),
            ]),
        ),
        ("alreadyMigrated", Value::Number(plan.already as f64)),
        ("retained", Value::Array(retained)),
        (
            "moves",
            Value::Array(
                plan.moves
                    .iter()
                    .map(|item| {
                        Ok(Value::object(vec![
                            ("from", Value::string(&layout.relative(&item.from)?)),
                            ("to", Value::string(&layout.relative(&item.to)?)),
                        ]))
                    })
                    .collect::<Result<_, Error>>()?,
            ),
        ),
        ("record", Value::string(&layout.relative(&layout.record)?)),
    ]))
}

fn write_json(path: &Path, value: &Value) -> Result<secure::Snapshot, Error> {
    secure::private_directory(
        path.parent()
            .ok_or_else(|| failure("E_CORRUPT_STATE", "Migration record has no parent"))?,
        true,
    )?;
    files::atomic_write_file(path, &Text::from(format!("{}\n", value.pretty()).as_str()))?;
    secure::sync_directory(
        path.parent()
            .ok_or_else(|| failure("E_CORRUPT_STATE", "Migration record has no parent"))?,
    )?;
    secure::read(path, MAX_FILE_BYTES)?.ok_or_else(|| {
        failure(
            "E_CORRUPT_STATE",
            "Migration record disappeared after write",
        )
    })
}

fn rollback(
    layout: &Layout,
    moves: &[Move],
    record: &secure::Snapshot,
    previous: Option<&secure::Snapshot>,
) -> Result<(), Error> {
    let mut failed = Vec::new();
    for item in moves.iter().rev() {
        if item.unverified_stage {
            failed.push(format!(
                "Staged copy for {} has unverified ownership; preserve it",
                item.to.display()
            ));
        }
        if let Some((path, snapshot)) = &item.staged {
            match secure::remove_matching(path, snapshot, MAX_FILE_BYTES) {
                Ok(true) => {}
                Ok(false) => failed.push(format!(
                    "Staged copy {} changed before rollback",
                    path.display()
                )),
                Err(error) => failed.push(error.message),
            }
        }
        let Some(written) = &item.written else {
            continue;
        };
        let restore = (|| {
            if !secure::unchanged(&item.to, written, MAX_FILE_BYTES)? {
                return Err(failure(
                    "E_CONFLICT",
                    format!("{} changed after migration copied it", item.to.display()),
                ));
            }
            if let Some(previous) = &item.previous {
                files::atomic_write_bytes(&item.to, &previous.bytes, MAX_FILE_BYTES)?;
            } else if !secure::remove_matching(&item.to, written, MAX_FILE_BYTES)? {
                return Err(failure(
                    "E_CONFLICT",
                    "New migration destination changed before rollback",
                ));
            }
            Ok(())
        })();
        if let Err(error) = restore {
            failed.push(error.message);
        }
    }
    if failed.is_empty() {
        if !secure::unchanged(&layout.record, record, MAX_FILE_BYTES)? {
            failed.push("Migration record changed during rollback".to_owned());
        } else if let Some(previous) = previous {
            if let Err(error) =
                files::atomic_write_bytes(&layout.record, &previous.bytes, MAX_FILE_BYTES)
            {
                failed.push(error.message);
            }
        } else if let Err(error) = secure::remove_matching(&layout.record, record, MAX_FILE_BYTES) {
            failed.push(error.message);
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failure(
            "E_ROLLBACK_FAILED",
            format!(
                "Migration rollback requires explicit recovery: {}",
                failed.join("; ")
            ),
        ))
    }
}

pub async fn migrate(root: &Path, dry_run: bool) -> Result<Value, Error> {
    migrate_with(
        root,
        dry_run,
        &clock::now_iso()?,
        locks::probe_pid,
        |_, _| Ok(()),
    )
    .await
}

pub async fn migrate_with(
    root: &Path,
    dry_run: bool,
    now: &str,
    probe: impl Fn(u64) -> Result<locks::Liveness, Error>,
    mut observe: impl FnMut(usize, &Path) -> Result<(), Error>,
) -> Result<Value, Error> {
    let layout = Layout::resolve(root)?;
    let active = detect_active_inner(&layout, &probe, false)?;
    let previous = read_previous(&layout)?;
    let planned = plan(&layout, previous.as_ref().map(|(value, _)| value))?;
    let initial_report = build_report(
        &layout,
        &planned,
        previous.as_ref().map(|(value, _)| value),
        active.clone(),
        dry_run,
    )?;
    if dry_run {
        return Ok(initial_report);
    }
    if active.get("active") == Some(&Value::Boolean(true)) {
        return Err(failure(
            "E_ACTIVE_RUN",
            "Active or unknown run ownership detected; migration refused before any mutation",
        ));
    }
    let interrupted = previous
        .as_ref()
        .is_some_and(|(value, _)| domain::is_text(value.get("status"), "in-progress"));
    if planned.moves.is_empty() && planned.temporaries.is_empty() && !interrupted {
        return Ok(initial_report);
    }
    let path = locks::target("migrate", &layout.root, &layout.root, None)?;
    let mut lock =
        locks::acquire_with_probe("migrate", &path, &locks::Options::default(), None, &probe)
            .await?;
    let result = (|| {
        let gate = locks::Gate::try_acquire(&path)?.ok_or_else(|| {
            failure(
                "E_ACTIVE_RUN",
                "Lock namespace changed before migration; retry after other writers finish",
            )
        })?;
        let operation = (|| {
            let active = detect_active_inner(&layout, &probe, true)?;
            if active.get("active") == Some(&Value::Boolean(true)) {
                return Err(failure(
                    "E_ACTIVE_RUN",
                    "Another writer started before the migration guard; no state was copied",
                ));
            }
            let previous = read_previous(&layout)?;
            let mut planned = plan(&layout, previous.as_ref().map(|(value, _)| value))?;
            let mut report = build_report(
                &layout,
                &planned,
                previous.as_ref().map(|(value, _)| value),
                active,
                false,
            )?;
            if let Some(retained) = initial_report.get("retained") {
                domain::put(&mut report, "retained", retained.clone())?;
            }
            let backup_exists = read_json(&layout.backup)?;
            if backup_exists.is_none() {
                let mut prior = Vec::new();
                for entry in ENTRIES {
                    for path in list_files(&layout.destination.join(entry))? {
                        let bytes = secure::read(&path, MAX_FILE_BYTES)?.ok_or_else(|| {
                            failure("E_CORRUPT_STATE", "Prior migration destination disappeared")
                        })?;
                        prior.push(Value::object(vec![
                            ("path", Value::string(&layout.relative(&path)?)),
                            ("sha256", Value::string(&sha256_hex(&bytes.bytes))),
                        ]));
                    }
                }
                let backup = Value::object(vec![
                    ("id", Value::string(ID)),
                    ("createdAt", Value::string(now)),
                    ("sourceLayout", Value::string(".roster")),
                    (
                        "destinationLayout",
                        Value::string(&layout.relative(&layout.destination)?),
                    ),
                    ("legacyRetained", Value::Boolean(true)),
                    ("prior", Value::Array(prior)),
                ]);
                secure::private_directory(
                    layout
                        .backup
                        .parent()
                        .ok_or_else(|| failure("E_CORRUPT_STATE", "Missing migration directory"))?,
                    true,
                )?;
                secure::commit_new(&layout.backup, format!("{}\n", backup.pretty()).as_bytes())?;
            } else if !backup_exists
                .as_ref()
                .is_some_and(|(value, _)| domain::is_text(value.get("id"), ID))
            {
                return Err(failure(
                    "E_CORRUPT_STATE",
                    "Existing migration backup is incompatible; preserve it",
                ));
            }
            let record = Value::object(vec![
                ("id", Value::string(ID)),
                ("version", Value::Number(1.0)),
                ("sourceLayout", Value::string(".roster")),
                (
                    "destinationLayout",
                    Value::string(&layout.relative(&layout.destination)?),
                ),
                ("legacyRetained", Value::Boolean(true)),
                ("files", recorded_values(&planned.recorded, false)),
            ]);
            let mut in_progress = record.clone();
            domain::put(
                &mut in_progress,
                "files",
                recorded_values(&planned.recorded, true),
            )?;
            domain::put(&mut in_progress, "status", Value::string("in-progress"))?;
            let record_snapshot = write_json(&layout.record, &in_progress)?;
            let execution = (|| {
                let mut preserved = Vec::new();
                for temporary in &planned.temporaries {
                    let snapshot = secure::read(temporary, MAX_FILE_BYTES)?.ok_or_else(|| {
                        failure("E_CONFLICT", "Interrupted migration file disappeared")
                    })?;
                    let backup = layout
                        .backup
                        .parent()
                        .ok_or_else(|| failure("E_CORRUPT_STATE", "Missing migration directory"))?
                        .join(format!("interrupted-{}.bak", files::nonce()?));
                    secure::commit_new(&backup, &snapshot.bytes)?;
                    if !secure::remove_matching(temporary, &snapshot, MAX_FILE_BYTES)? {
                        return Err(failure(
                            "E_CONFLICT",
                            "Interrupted migration file changed; original was preserved",
                        ));
                    }
                    preserved.push(path_value(&backup));
                }
                if !preserved.is_empty() {
                    domain::put(&mut report, "preservedTemporaries", Value::Array(preserved))?;
                }
                for (index, item) in planned.moves.iter_mut().enumerate() {
                    lock.check()?;
                    gate.check()?;
                    if !secure::unchanged(&item.from, &item.source, MAX_FILE_BYTES)? {
                        return Err(failure(
                            "E_CONFLICT",
                            "Legacy source changed after planning; it was not copied",
                        ));
                    }
                    let parent = item
                        .to
                        .parent()
                        .ok_or_else(|| failure("E_CORRUPT_STATE", "Destination has no parent"))?;
                    secure::private_directory(parent, true)?;
                    let temporary =
                        PathBuf::from(format!("{}.migrate-tmp", item.to.to_string_lossy()));
                    secure::check_components(&temporary)?;
                    let mut handle = files::create_private_file(&temporary).map_err(|cause| {
                        secure::io_error("Create staged migration copy", &temporary, cause)
                    })?;
                    item.unverified_stage = true;
                    let identity = paths::handle_information(&handle)?.identity;
                    let written = handle
                        .write_all(&item.source.bytes)
                        .and_then(|()| handle.sync_all());
                    drop(handle);
                    let staged = secure::read(&temporary, MAX_FILE_BYTES)?.ok_or_else(|| {
                        failure("E_CORRUPT_STATE", "Staged migration copy disappeared")
                    })?;
                    if staged.identity != identity || !item.source.bytes.starts_with(&staged.bytes)
                    {
                        return Err(failure(
                            "E_CONFLICT",
                            "Staged migration file was changed by another actor; preserve it",
                        ));
                    }
                    item.unverified_stage = false;
                    item.staged = Some((
                        temporary.clone(),
                        secure::Snapshot {
                            identity: staged.identity,
                            bytes: staged.bytes.clone(),
                        },
                    ));
                    written.map_err(|cause| {
                        secure::io_error("Write staged migration copy", &temporary, cause)
                    })?;
                    if sha256_hex(&staged.bytes) != item.hash {
                        return Err(failure(
                            "E_CORRUPT_STATE",
                            "Staged migration copy did not verify",
                        ));
                    }
                    if let Some(previous) = &item.previous {
                        if !secure::unchanged(&item.to, previous, MAX_FILE_BYTES)? {
                            return Err(failure(
                                "E_CONFLICT",
                                "Migration destination changed after planning; independent edits were preserved",
                            ));
                        }
                        fs::rename(&temporary, &item.to).map_err(|cause| {
                            secure::io_error("Replace owned migration copy", &item.to, cause)
                        })?;
                        item.staged = None;
                    } else {
                        fs::hard_link(&temporary, &item.to).map_err(|cause| {
                            secure::io_error("Commit new migration copy", &item.to, cause)
                        })?;
                        item.written = Some(secure::Snapshot {
                            identity: staged.identity,
                            bytes: staged.bytes.clone(),
                        });
                        if !secure::remove_matching(&temporary, &staged, MAX_FILE_BYTES)? {
                            return Err(failure(
                                "E_CONFLICT",
                                "Migration temporary changed after commit",
                            ));
                        }
                        item.staged = None;
                    }
                    item.written = Some(secure::Snapshot {
                        identity: staged.identity,
                        bytes: staged.bytes,
                    });
                    secure::sync_directory(parent)?;
                    observe(index, &item.to)?;
                }
                if !secure::unchanged(&layout.record, &record_snapshot, MAX_FILE_BYTES)? {
                    return Err(failure(
                        "E_CONFLICT",
                        "Migration record was edited during copy; preserve it",
                    ));
                }
                let mut complete = record;
                domain::put(&mut complete, "status", Value::string("complete"))?;
                write_json(&layout.record, &complete)?;
                Ok(())
            })();
            if let Err(cause) = execution {
                let previous_snapshot = previous.as_ref().map(|(_, snapshot)| snapshot);
                if let Err(rollback) =
                    rollback(&layout, &planned.moves, &record_snapshot, previous_snapshot)
                {
                    return Err(rollback.caused_by(cause));
                }
                return Err(failure("E_ROLLBACK", format!("Migration failed; its committed copies were rolled back and legacy state is unchanged ({cause})")).caused_by(cause));
            }
            domain::put(&mut report, "applied", Value::Boolean(true))?;
            Ok(report)
        })();
        locks::finish(operation, gate.finish())
    })();
    let cleanup = lock.release_required().await;
    locks::finish(result, cleanup)
}
