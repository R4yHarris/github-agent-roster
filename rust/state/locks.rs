use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;

use crate::Error;
use crate::cancel::CancellationToken;
use crate::clock;
use crate::domain;
use crate::json::{self, Value};
use crate::paths::{self, FileIdentity};
use crate::state::{files, layout, secure};
use crate::text::Text;

const MAX_LOCK_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Dead,
    Unknown,
}

#[cfg(windows)]
pub fn probe_pid(pid: u64) -> Result<Liveness, Error> {
    use winsafe::{HPROCESS, co, prelude::*};
    let Ok(pid) = u32::try_from(pid) else {
        return Ok(Liveness::Unknown);
    };
    if pid == 0 {
        return Ok(Liveness::Unknown);
    }
    let handle = match HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(error) if error == co::ERROR::INVALID_PARAMETER => return Ok(Liveness::Dead),
        Err(error) if error == co::ERROR::ACCESS_DENIED => return Ok(Liveness::Unknown),
        Err(error) => {
            return Err(Error::new(
                "Error",
                format!("Specific-PID process probe failed: {error}"),
            ));
        }
    };
    match handle.WaitForSingleObject(Some(0)) {
        Ok(result) if result == co::WAIT::OBJECT_0 => Ok(Liveness::Dead),
        Ok(result) if result == co::WAIT::TIMEOUT => Ok(Liveness::Alive),
        Ok(_) => Ok(Liveness::Unknown),
        Err(error) => Err(Error::new(
            "Error",
            format!("Specific-PID process wait failed: {error}"),
        )),
    }
}

#[cfg(unix)]
pub fn probe_pid(pid: u64) -> Result<Liveness, Error> {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return Ok(Liveness::Unknown);
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => Ok(Liveness::Alive),
        Err(rustix::io::Errno::SRCH) => Ok(Liveness::Dead),
        Err(rustix::io::Errno::PERM) => Ok(Liveness::Unknown),
        Err(error) => Err(Error::new(
            "Error",
            format!("Specific-PID process probe failed: {error}"),
        )),
    }
}

fn lock_error(code: &'static str, message: impl Into<String>) -> Error {
    Error::new("RepoLockError", message).coded(code)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordFormat {
    #[default]
    Repository,
    Run,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub holder: String,
    pub wait: Duration,
    pub poll: Duration,
    pub run_id: Option<String>,
    pub format: RecordFormat,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            holder: format!("pid-{}", std::process::id()),
            wait: Duration::ZERO,
            poll: Duration::from_millis(25),
            run_id: None,
            format: RecordFormat::Repository,
        }
    }
}

struct PreparedRecord {
    acquired_at: String,
    bytes: Vec<u8>,
}

fn prepare_record(options: &Options) -> Result<PreparedRecord, Error> {
    let value = match options.format {
        RecordFormat::Repository => Some(options.holder.as_str()),
        RecordFormat::Run => options.run_id.as_deref(),
    };
    if value.is_some_and(|value| value.len() as u64 > MAX_LOCK_BYTES) {
        return Err(lock_error(
            "E_LOCK_SIZE",
            "Serialized lock record must not exceed 16 KiB including its newline.",
        ));
    }
    let acquired_at = clock::now_iso()?;
    let record = match options.format {
        RecordFormat::Repository => Value::object(vec![
            ("holder", Value::string(&options.holder)),
            ("pid", Value::Number(std::process::id() as f64)),
            ("acquiredAt", Value::string(&acquired_at)),
        ]),
        RecordFormat::Run => Value::object(vec![
            ("pid", Value::Number(std::process::id() as f64)),
            (
                "runId",
                options.run_id.as_deref().map_or(Value::Null, Value::string),
            ),
            ("acquiredAt", Value::string(&acquired_at)),
        ]),
    };
    let bytes = format!("{}\n", record.json()).into_bytes();
    if bytes.len() as u64 > MAX_LOCK_BYTES {
        return Err(lock_error(
            "E_LOCK_SIZE",
            "Serialized lock record must not exceed 16 KiB including its newline.",
        ));
    }
    Ok(PreparedRecord { acquired_at, bytes })
}

pub struct Gate {
    file: File,
    path: PathBuf,
    identity: FileIdentity,
}

impl Gate {
    pub fn try_acquire(target: &Path) -> Result<Option<Self>, Error> {
        let path = target
            .parent()
            .ok_or_else(|| Error::type_error("Lock guard requires a parent directory"))?
            .join(".roster-locks.guard");
        secure::check_components(&path)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(3);
        }
        let file = options
            .open(&path)
            .map_err(|error| secure::io_error("Open lock guard", &path, error))?;
        let information = paths::handle_information(&file)?;
        if information.links != 1
            || !file
                .metadata()
                .map_err(|error| secure::io_error("Inspect lock guard", &path, error))?
                .is_file()
        {
            return Err(secure::path_error(
                &path,
                "is not a regular single-link lock guard",
            ));
        }
        if !FileExt::try_lock_exclusive(&file)
            .map_err(|error| secure::io_error("Lock state guard", &path, error))?
        {
            return Ok(None);
        }
        let guard = Self {
            file,
            path,
            identity: information.identity,
        };
        guard.check()?;
        Ok(Some(guard))
    }

    pub fn check(&self) -> Result<(), Error> {
        secure::check_components(&self.path)?;
        if paths::file_information(&self.path)?.identity != self.identity {
            return Err(secure::path_error(&self.path, "was replaced while locked"));
        }
        Ok(())
    }

    pub fn finish(self) -> Result<(), Error> {
        self.check()?;
        FileExt::unlock(&self.file)
            .map_err(|error| secure::io_error("Unlock state guard", &self.path, error))
    }
}

pub struct RepoLock {
    pub name: String,
    pub path: PathBuf,
    pub holder: String,
    pub acquired_at: String,
    snapshot: secure::Snapshot,
    released: bool,
}

impl RepoLock {
    pub fn check(&self) -> Result<(), Error> {
        if self.released || !secure::unchanged(&self.path, &self.snapshot, MAX_LOCK_BYTES)? {
            return Err(lock_error(
                "E_LOCK_LOST",
                format!(
                    "lock \"{}\" is no longer owned by this operation; stop writing.",
                    self.name
                ),
            ));
        }
        Ok(())
    }

    pub async fn release(&mut self) -> Result<bool, Error> {
        if self.released {
            return Ok(false);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(gate) = Gate::try_acquire(&self.path)? {
                let removed = secure::remove_matching(&self.path, &self.snapshot, MAX_LOCK_BYTES)
                    .map_err(|cause| {
                        lock_error(
                            "E_LOCK_RELEASE",
                            format!(
                                "Lock {} could not be released; ownership is unverified.",
                                self.path.display()
                            ),
                        )
                        .caused_by(cause)
                    });
                let unlock = gate.finish();
                let removed = finish(removed, unlock)?;
                self.released = true;
                return Ok(removed);
            }

            if Instant::now() >= deadline {
                return Err(lock_error(
                    "E_LOCK_RELEASE",
                    "Lock namespace guard remained busy; ownership is not released.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    pub async fn release_required(&mut self) -> Result<(), Error> {
        if self.release().await? {
            Ok(())
        } else {
            Err(lock_error(
                "E_LOCK_RELEASE",
                format!(
                    "Lock {} was lost before cleanup; release is unverified.",
                    self.path.display()
                ),
            ))
        }
    }
}

pub fn finish<T>(result: Result<T, Error>, cleanup: Result<(), Error>) -> Result<T, Error> {
    match (result, cleanup) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
        (Err(cause), Err(error)) => Err(Error::new(
            error.name,
            format!(
                "{}; operation also failed: {}",
                error.message, cause.message
            ),
        )
        .coded(error.code.unwrap_or("E_LOCK_RELEASE"))
        .caused_by(cause)),
    }
}

pub fn target(
    name: &str,
    repo_root: &Path,
    worktree_root: &Path,
    lock_root: Option<&Path>,
) -> Result<PathBuf, Error> {
    validate_name(name)?;
    if let Some(root) = lock_root {
        return Ok(paths::absolute(root)?
            .join("locks")
            .join(format!("{name}.lock")));
    }
    Ok(layout::state_path(
        layout::resolve_directory(
            layout::Scope::PerWorktree,
            repo_root,
            Some(worktree_root),
            Some(".roster"),
        )?,
        &["locks", &format!("{name}.lock")],
    )?
    .path)
}

fn validate_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(Error::new(
            "RepoLockError",
            "Lock name must be an opaque identifier of at most 64 characters.",
        ));
    }
    Ok(())
}

pub async fn acquire(
    name: &str,
    target: &Path,
    options: &Options,
    token: Option<&CancellationToken>,
) -> Result<RepoLock, Error> {
    acquire_with_probe(name, target, options, token, probe_pid).await
}

pub async fn acquire_run(
    target: &Path,
    options: &Options,
    token: Option<&CancellationToken>,
) -> Result<RepoLock, Error> {
    let mut options = options.clone();
    options.format = RecordFormat::Run;
    acquire("run", target, &options, token).await
}

pub async fn acquire_with_probe(
    name: &str,
    target: &Path,
    options: &Options,
    token: Option<&CancellationToken>,
    probe: impl Fn(u64) -> Result<Liveness, Error>,
) -> Result<RepoLock, Error> {
    validate_name(name)?;
    if options.holder.encode_utf16().count() > 4096
        || options.wait > Duration::from_millis(i32::MAX as u64)
    {
        return Err(Error::type_error(
            "Lock holder or wait exceeds the native bound",
        ));
    }
    prepare_record(options)?;
    let path = paths::absolute(target)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::type_error("Lock requires a parent directory"))?;
    if let Some(token) = token {
        token.check()?;
    }
    secure::private_directory(parent, true)?;
    let deadline = Instant::now() + options.wait;
    let mut changed_attempts = 0;
    loop {
        if let Some(token) = token {
            token.check()?;
        }
        if let Some(gate) = Gate::try_acquire(&path)? {
            let attempt = attempt_acquire(name, &path, options, &gate, &probe);
            let attempt = finish(attempt, gate.finish())?;
            match attempt {
                Attempt::Acquired(mut lock) => {
                    if let Some(token) = token {
                        if let Err(cause) = token.check() {
                            let cleanup = lock.release_required().await;
                            return finish(Err(cause), cleanup);
                        }
                    }
                    return Ok(lock);
                }
                Attempt::Retry => {
                    changed_attempts += 1;
                    if changed_attempts > 128 {
                        return Err(lock_error(
                            "E_LOCK_CHANGED",
                            "Lock ownership changed repeatedly during acquisition; preserve it and retry after other actors stop.",
                        ));
                    }
                    continue;
                }
                Attempt::Held(holder) if Instant::now() >= deadline => {
                    return Err(lock_error(
                        "E_LOCK_HELD",
                        format!(
                            "lock \"{name}\" is held by {}; concurrent writers are refused. Wait for release or remove the stale lock once its owner is gone.",
                            holder.as_deref().unwrap_or("an unknown holder")
                        ),
                    ));
                }
                Attempt::Blocked(error) if Instant::now() >= deadline => return Err(error),
                Attempt::Held(_) | Attempt::Blocked(_) => {}
            }
        } else if Instant::now() >= deadline {
            return Err(lock_error(
                "E_LOCK_HELD",
                format!("lock \"{name}\" namespace is busy; concurrent writers are refused."),
            ));
        }
        let delay = options
            .poll
            .max(Duration::from_millis(1))
            .min(deadline.saturating_duration_since(Instant::now()));
        if let Some(token) = token {
            tokio::select! {
                biased;
                reason = token.cancelled() => return Err(reason?.error()),
                () = tokio::time::sleep(delay) => {}
            }
        } else {
            tokio::time::sleep(delay).await;
        }
    }
}

enum Attempt {
    Acquired(RepoLock),
    Retry,
    Held(Option<String>),
    Blocked(Error),
}

fn attempt_acquire(
    name: &str,
    path: &Path,
    options: &Options,
    gate: &Gate,
    probe: &impl Fn(u64) -> Result<Liveness, Error>,
) -> Result<Attempt, Error> {
    gate.check()?;
    let PreparedRecord { acquired_at, bytes } = prepare_record(options)?;
    let mut handle = match files::create_private_file(path) {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = match secure::read(path, MAX_LOCK_BYTES) {
                Ok(Some(existing)) => existing,
                Ok(None) => return Ok(Attempt::Retry),
                Err(error) if error.code == Some("ENOENT") => return Ok(Attempt::Retry),
                Err(error) if cfg!(windows) && matches!(error.code, Some("EACCES" | "EBUSY")) => {
                    return Ok(Attempt::Blocked(error));
                }
                Err(error) => return Err(error),
            };
            let record = std::str::from_utf8(&existing.bytes)
                .ok()
                .and_then(|text| json::parse(&Text::from(text)).ok());
            let holder = record
                .as_ref()
                .and_then(|record| record.get("holder"))
                .and_then(Value::as_string)
                .map(|text| String::from_utf16_lossy(&text.0));
            let pid = record
                .as_ref()
                .and_then(|record| domain::safe_integer(record.get("pid")))
                .filter(|pid| *pid > 0);
            let Some(pid) = pid else {
                return Ok(Attempt::Held(holder));
            };
            let liveness = probe(pid).map_err(|cause| lock_error("E_LOCK_PROBE", format!("lock \"{name}\" cannot be taken over: liveness probe failed for pid {pid}; the lock is left untouched.")).caused_by(cause))?;
            if liveness != Liveness::Dead {
                return Ok(Attempt::Held(holder));
            }
            gate.check()?;
            secure::remove_matching(path, &existing, MAX_LOCK_BYTES).map_err(|cause| {
                lock_error(
                    "E_LOCK_STALE",
                    format!(
                        "stale lock \"{name}\" could not be taken over; preserve it for recovery."
                    ),
                )
                .caused_by(cause)
            })?;
            return Ok(Attempt::Retry);
        }
        Err(cause) => {
            let waitable = cfg!(windows) && matches!(cause.raw_os_error(), Some(5 | 32 | 33));
            let error = lock_error(
                "E_LOCK_CREATE",
                format!("Lock {name} could not be created ({cause})."),
            );
            return if waitable {
                Ok(Attempt::Blocked(error))
            } else {
                Err(error)
            };
        }
    };
    let identity = paths::handle_information(&handle)?.identity;
    if let Err(cause) = handle.write_all(&bytes).and_then(|()| handle.sync_all()) {
        drop(handle);
        return Err(lock_error(
            "E_LOCK_RECORD",
            format!(
                "Lock {name} could not record its holder ({cause}); preserve the partial lock for explicit recovery."
            ),
        ));
    }
    drop(handle);
    secure::sync_directory(
        path.parent()
            .ok_or_else(|| Error::type_error("Lock requires a parent"))?,
    )?;
    Ok(Attempt::Acquired(RepoLock {
        name: name.to_owned(),
        path: path.to_owned(),
        holder: options.holder.clone(),
        acquired_at,
        snapshot: secure::Snapshot { bytes, identity },
        released: false,
    }))
}
