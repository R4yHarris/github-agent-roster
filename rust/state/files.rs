use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Error;
use crate::identity::{Comparison, compare_identity, sha256_hex};
use crate::json::{self, Value};
use crate::text::{Text, trim};

pub const SCHEMA_VERSION: u32 = 1;
pub const SCHEMA_MARKER: &str = "schema.json";
pub const IDENTITY_FILE: &str = "identity";
const MAX_STATE_BYTES: u64 = 1024 * 1024;
static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn state_error(code: &'static str, message: impl Into<String>) -> Error {
    Error::new("RepoStateError", message).coded(code)
}

fn io_code(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::AlreadyExists => "EEXIST",
        io::ErrorKind::PermissionDenied => "EACCES",
        io::ErrorKind::NotADirectory => "ENOTDIR",
        io::ErrorKind::IsADirectory => "EISDIR",
        _ => "EIO",
    }
}

fn io_error(operation: &str, path: &Path, error: io::Error) -> Error {
    let code = io_code(&error);
    Error::new(
        "Error",
        format!("{operation} {} failed ({code}: {error})", path.display()),
    )
    .coded(code)
}

pub fn nonce() -> Result<String, Error> {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::new("Error", "System clock precedes the Unix epoch"))?;
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(
        sha256_hex(format!("{}:{}:{counter}", std::process::id(), time.as_nanos()).as_bytes())
            [..16]
            .to_owned(),
    )
}

pub fn create_private_directories(path: &Path) -> Result<(), Error> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| io_error("Create state directory", path, error))
}

pub(crate) fn create_private_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn remove_temporary(path: &Path, cause: Error) -> Error {
    match fs::remove_file(path) {
        Ok(()) => cause,
        Err(error) if error.kind() == io::ErrorKind::NotFound => cause,
        Err(error) => state_error("E_ATOMIC_CLEANUP", format!(
            "Atomic write failed and temporary file {} could not be removed ({error}); preserve it for recovery",
            path.display()
        )).caused_by(cause),
    }
}

pub fn atomic_write_file(file: &Path, data: &Text) -> Result<PathBuf, Error> {
    atomic_write_file_with(file, data, |from, to| fs::rename(from, to))
}

pub fn atomic_write_file_with(
    file: &Path,
    data: &Text,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf, Error> {
    let bytes = String::from_utf16_lossy(&data.0);
    atomic_write_bytes_with(file, bytes.as_bytes(), MAX_STATE_BYTES, rename)
}

pub fn atomic_write_bytes(file: &Path, data: &[u8], maximum: u64) -> Result<PathBuf, Error> {
    atomic_write_bytes_with(file, data, maximum, |from, to| fs::rename(from, to))
}

fn atomic_write_bytes_with(
    file: &Path,
    data: &[u8],
    maximum: u64,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf, Error> {
    if data.len() as u64 > maximum {
        let message = if maximum == MAX_STATE_BYTES {
            "State file exceeds 1 MiB".to_owned()
        } else {
            format!("State file exceeds {maximum} bytes")
        };
        return Err(state_error("E_STATE_SIZE", message));
    }
    let name = file
        .file_name()
        .ok_or_else(|| Error::type_error("Atomic write requires a named file"))?;
    let mut temporary_name = name.to_os_string();
    temporary_name.push(format!(".{}.tmp", nonce()?));
    let temporary = file.with_file_name(temporary_name);
    let mut handle = create_private_file(&temporary)
        .map_err(|error| io_error("Create atomic state file", &temporary, error))?;
    if let Err(error) = handle.write_all(data).and_then(|()| handle.sync_all()) {
        drop(handle);
        return Err(remove_temporary(
            &temporary,
            io_error("Write atomic state file", &temporary, error),
        ));
    }
    drop(handle);
    if let Err(error) = rename(&temporary, file) {
        return Err(state_error("E_ATOMIC_RENAME", format!(
            "Atomic write of {} failed during the final rename ({error}); previous contents are untouched and {} remains for recovery",
            file.display(), temporary.display()
        )).caused_by(io_error("Rename atomic state file", file, error)));
    }
    Ok(file.to_owned())
}

pub fn read_state_file(file: &Path) -> Result<Text, Error> {
    let handle = match fs::File::open(file) {
        Ok(handle) => handle,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = file
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let base = file
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default();
            let entries = match fs::read_dir(parent) {
                Ok(entries) => entries
                    .map(|entry| entry.map(|entry| entry.file_name()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| io_error("Read state directory", parent, error))?,
                Err(parent_error) if parent_error.kind() == io::ErrorKind::NotFound => Vec::new(),
                Err(parent_error) => {
                    return Err(io_error("Read state directory", parent, parent_error));
                }
            };
            if entries.iter().any(|name| {
                let name = name.to_string_lossy();
                name.starts_with(&format!("{base}.")) && name.ends_with(".tmp")
            }) {
                return Err(state_error(
                    "E_INTERRUPTED_WRITE",
                    format!(
                        "State file {} is missing but an interrupted atomic write left a temp file; preserve the directory for explicit recovery",
                        file.display()
                    ),
                ));
            }
            return Err(io_error("Read state file", file, error));
        }
        Err(error) => return Err(io_error("Read state file", file, error)),
    };
    let mut bytes = Vec::new();
    handle
        .take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("Read state file", file, error))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(state_error("E_STATE_SIZE", "State file exceeds 1 MiB"));
    }
    Ok(Text::from(String::from_utf8_lossy(&bytes).as_ref()))
}

pub fn read_marker(root: &Path) -> Result<Value, Error> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error("Read schema directory", root, error))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(state_error(
                "E_NO_STATE_DIR",
                "state directory does not exist yet.",
            ));
        }
        Err(error) => {
            return Err(state_error(
                "E_STATE_ROOT",
                format!(
                    "State directory {} could not be read ({error}); preserve it for recovery",
                    root.display()
                ),
            ));
        }
    };
    let file = root.join(SCHEMA_MARKER);
    if !entries.iter().any(|name| name == SCHEMA_MARKER) {
        if entries.iter().any(|name| {
            let name = name.to_string_lossy();
            name.starts_with("schema.json.") && name.ends_with(".tmp")
        }) {
            return Err(state_error(
                "E_INTERRUPTED_WRITE",
                format!(
                    "Schema marker {} is missing but an interrupted atomic write remains; preserve it for recovery",
                    file.display()
                ),
            ));
        }
        return Err(state_error(
            "E_MARKER_MISSING",
            format!(
                "Schema marker {} is missing; existing state was not initialized by this version",
                file.display()
            ),
        ));
    }
    let parsed = read_state_file(&file)
        .and_then(|text| json::parse(&text))
        .map_err(|error| {
            state_error(
                "E_MARKER_CORRUPT",
                format!(
                    "Schema marker {} is unreadable or corrupt; expected version {SCHEMA_VERSION}",
                    file.display()
                ),
            )
            .caused_by(error)
        })?;
    if parsed.get("version").and_then(Value::as_number) != Some(SCHEMA_VERSION as f64) {
        return Err(state_error(
            "E_MARKER_VERSION",
            format!(
                "Schema marker {} has an incompatible version; expected {SCHEMA_VERSION}",
                file.display()
            ),
        ));
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Copy, Default)]
pub enum IdentityPolicy {
    #[default]
    Refuse,
    Reinitialize,
}

#[derive(Debug)]
pub struct OpenState {
    pub root: PathBuf,
    pub marker: Value,
    pub initialized: bool,
    pub reinitialized: Option<(String, String)>,
}

impl OpenState {
    pub fn json(&self) -> String {
        let mut entries = vec![
            (
                "stateRoot",
                Value::string(self.root.to_string_lossy().as_ref()),
            ),
            ("state", Value::Object(Vec::new())),
            ("marker", self.marker.clone()),
        ];
        if self.initialized {
            entries.push(("initialized", Value::Boolean(true)));
        }
        if let Some((from, to)) = &self.reinitialized {
            entries.push((
                "identity",
                Value::object(vec![
                    ("reinitialized", Value::Boolean(true)),
                    ("from", Value::string(from)),
                    ("to", Value::string(to)),
                ]),
            ));
        }
        Value::object(entries).json()
    }
}

fn write_marker(root: &Path) -> Result<Value, Error> {
    let marker = Value::object(vec![("version", Value::Number(SCHEMA_VERSION as f64))]);
    atomic_write_file(
        &root.join(SCHEMA_MARKER),
        &Text::from(format!("{}\n", marker.pretty()).as_str()),
    )?;
    Ok(marker)
}

fn write_identity(root: &Path, identity: &str) -> Result<(), Error> {
    atomic_write_file(
        &root.join(IDENTITY_FILE),
        &Text::from(format!("{identity}\n").as_str()),
    )?;
    Ok(())
}

pub fn open_repo_state(
    root: &Path,
    identity: Option<&str>,
    policy: IdentityPolicy,
) -> Result<OpenState, Error> {
    if root.as_os_str().is_empty() {
        return Err(state_error(
            "E_STATE_ROOT",
            "openRepoState requires a state root path from the path resolver.",
        ));
    }
    let marker = match read_marker(root) {
        Ok(marker) => marker,
        Err(error) if error.code == Some("E_NO_STATE_DIR") => {
            if let Some(parent) = root.parent() {
                create_private_directories(parent)?;
            }
            let mut builder = fs::DirBuilder::new();
            builder.recursive(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(root) {
                Ok(()) => {
                    let marker = write_marker(root)?;
                    if let Some(identity) = identity {
                        write_identity(root, identity)?;
                    }
                    return Ok(OpenState {
                        root: root.to_owned(),
                        marker,
                        initialized: true,
                        reinitialized: None,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => read_marker(root)?,
                Err(error) => return Err(io_error("Create state directory", root, error)),
            }
        }
        Err(error) => return Err(error),
    };
    if let Some(identity) = identity {
        let recorded = match read_state_file(&root.join(IDENTITY_FILE)) {
            Ok(text) => Some(Text(trim(&text.0).to_vec()).to_utf8()?),
            Err(error) if error.code == Some("ENOENT") => None,
            Err(error) => return Err(state_error(
                "E_IDENTITY_UNREADABLE",
                format!(
                    "Recorded repository identity {} could not be read; preserve it for recovery",
                    root.join(IDENTITY_FILE).display()
                ),
            )
            .caused_by(error)),
        };
        match compare_identity(recorded.as_deref(), identity) {
            Comparison::Match => {}
            Comparison::Initialize(_) => write_identity(root, identity)?,
            Comparison::Mismatch { recorded, current } => match policy {
                IdentityPolicy::Refuse => {
                    return Err(state_error(
                        "E_IDENTITY_MISMATCH",
                        format!(
                            "Repository identity mismatch in {}: recorded {recorded}, current {current}; preserve state before explicit recovery",
                            root.display()
                        ),
                    ));
                }
                IdentityPolicy::Reinitialize => {
                    let marker = write_marker(root)?;
                    write_identity(root, identity)?;
                    return Ok(OpenState {
                        root: root.to_owned(),
                        marker,
                        initialized: false,
                        reinitialized: Some((recorded, current)),
                    });
                }
            },
        }
    }
    Ok(OpenState {
        root: root.to_owned(),
        marker,
        initialized: false,
        reinitialized: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_relative_filenames_detect_interrupted_writes_in_the_current_directory()
    -> Result<(), Error> {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                if let Err(error) = fs::remove_file(&self.0) {
                    eprintln!("Relative state fixture cleanup failed: {error}");
                }
            }
        }
        let name = format!(".roster-native-relative-{}.json", nonce()?);
        let scratch = Scratch(PathBuf::from(format!("{name}.deadbeef.tmp")));
        create_private_file(&scratch.0)
            .and_then(|mut file| file.write_all(b"interrupted"))
            .map_err(|error| io_error("Create relative recovery fixture", &scratch.0, error))?;
        assert!(
            read_state_file(Path::new(&name))
                .is_err_and(|error| error.code == Some("E_INTERRUPTED_WRITE"))
        );
        Ok(())
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Result<Self, Error> {
            let root = std::env::temp_dir().join(format!("roster-native-state-{}", nonce()?));
            create_private_directories(&root)?;
            Ok(Self(root))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                eprintln!("Native state fixture cleanup failed: {error}");
            }
        }
    }

    #[test]
    fn initializes_compatible_markers_and_refuses_unapproved_identity_changes() -> Result<(), Error>
    {
        let fixture = Fixture::new()?;
        let root = fixture.0.join("state");
        let fresh = open_repo_state(&root, Some("sha256-first"), IdentityPolicy::Refuse)?;
        assert!(fresh.initialized);
        assert_eq!(
            fs::read_to_string(root.join(SCHEMA_MARKER)).map_err(|error| io_error(
                "Read fixture",
                &root,
                error
            ))?,
            "{\n  \"version\": 1\n}\n"
        );
        assert!(!open_repo_state(&root, Some("sha256-first"), IdentityPolicy::Refuse)?.initialized);
        assert!(
            open_repo_state(&root, Some("sha256-other"), IdentityPolicy::Refuse)
                .is_err_and(|error| error.code == Some("E_IDENTITY_MISMATCH"))
        );
        let replaced = open_repo_state(&root, Some("sha256-other"), IdentityPolicy::Reinitialize)?;
        assert_eq!(
            replaced.reinitialized,
            Some(("sha256-first".to_owned(), "sha256-other".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn failed_rename_preserves_previous_bytes_and_an_explicit_recovery_marker() -> Result<(), Error>
    {
        let fixture = Fixture::new()?;
        let target = fixture.0.join("value.json");
        atomic_write_file(&target, &Text::from("before"))?;
        let result = atomic_write_file_with(&target, &Text::from("after"), |_, _| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected rename failure",
            ))
        });
        assert!(result.is_err_and(|error| error.code == Some("E_ATOMIC_RENAME")));
        assert_eq!(read_state_file(&target)?, Text::from("before"));
        fs::remove_file(&target)
            .map_err(|error| io_error("Remove fixture value", &target, error))?;
        assert!(
            read_state_file(&target).is_err_and(|error| error.code == Some("E_INTERRUPTED_WRITE"))
        );
        Ok(())
    }

    #[test]
    fn existing_unmarked_and_corrupt_state_never_silently_reinitializes() -> Result<(), Error> {
        let fixture = Fixture::new()?;
        assert!(read_marker(&fixture.0).is_err_and(|error| error.code == Some("E_MARKER_MISSING")));
        atomic_write_file(&fixture.0.join(SCHEMA_MARKER), &Text::from("bad JSON"))?;
        assert!(
            open_repo_state(&fixture.0, Some("new"), IdentityPolicy::Refuse)
                .is_err_and(|error| error.code == Some("E_MARKER_CORRUPT"))
        );
        assert_eq!(
            read_state_file(&fixture.0.join(SCHEMA_MARKER))?,
            Text::from("bad JSON")
        );
        Ok(())
    }

    #[test]
    fn state_marker_identity_and_reopen_preserve_the_version_one_wire_contract() -> Result<(), Error>
    {
        let fixture = Fixture::new()?;
        let native_root = fixture.0.join("native");
        let first = open_repo_state(&native_root, Some("sha256-shared"), IdentityPolicy::Refuse)?;
        assert!(first.initialized);
        assert_eq!(
            read_state_file(&native_root.join(SCHEMA_MARKER))?,
            Text::from("{\n  \"version\": 1\n}\n")
        );
        assert_eq!(
            read_state_file(&native_root.join(IDENTITY_FILE))?,
            Text::from("sha256-shared\n")
        );
        let actual = open_repo_state(&native_root, Some("sha256-shared"), IdentityPolicy::Refuse)?;
        assert!(!actual.initialized);
        assert!(actual.reinitialized.is_none());
        let expected = Value::object(vec![
            (
                "stateRoot",
                Value::string(native_root.to_string_lossy().as_ref()),
            ),
            ("state", Value::object(Vec::new())),
            (
                "marker",
                Value::object(vec![("version", Value::Number(1.0))]),
            ),
        ]);
        assert_eq!(json::parse(&Text::from(actual.json().as_str()))?, expected);
        assert_eq!(
            read_state_file(&native_root.join(SCHEMA_MARKER))?,
            Text::from("{\n  \"version\": 1\n}\n")
        );
        assert_eq!(
            read_state_file(&native_root.join(IDENTITY_FILE))?,
            Text::from("sha256-shared\n")
        );
        Ok(())
    }
}
