use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::Error;
use crate::paths::{self, FileIdentity};
use crate::state::files;

pub fn io_error(operation: &str, path: &Path, error: io::Error) -> Error {
    let code = match (error.kind(), error.raw_os_error()) {
        (_, Some(32 | 33)) if cfg!(windows) => "EBUSY",
        (io::ErrorKind::NotFound, _) => "ENOENT",
        (io::ErrorKind::AlreadyExists, _) => "EEXIST",
        (io::ErrorKind::PermissionDenied, _) => "EACCES",
        (io::ErrorKind::NotADirectory, _) => "ENOTDIR",
        (io::ErrorKind::IsADirectory, _) => "EISDIR",
        _ => "EIO",
    };
    Error::new(
        "Error",
        format!("{operation} {} failed ({code}: {error})", path.display()),
    )
    .coded(code)
}

pub fn path_error(path: &Path, reason: &str) -> Error {
    Error::new(
        "PathContainmentError",
        format!(
            "State path {} {reason}; preserve it and choose a private, unlinked state root.",
            path.display()
        ),
    )
    .coded("E_CONTAINMENT")
}

pub fn linked(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn check_components(path: &Path) -> Result<(), Error> {
    let path = paths::absolute(path)?;
    let mut walked = PathBuf::new();
    for component in path.components() {
        walked.push(component);
        if !walked.has_root() {
            continue;
        }
        match fs::symlink_metadata(&walked) {
            Ok(metadata) if linked(&metadata) => return Err(path_error(&walked, "is linked")),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("Inspect state path", &walked, error)),
        }
    }
    Ok(())
}

pub fn private_directory_if_present(path: &Path) -> Result<bool, Error> {
    check_components(path)?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            private_directory(path, false)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("Inspect private state directory", path, error)),
    }
}

pub fn private_directory(path: &Path, create: bool) -> Result<(), Error> {
    check_components(path)?;
    if create {
        files::create_private_directories(path)?;
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error("Inspect state directory", path, error))?;
    if !metadata.is_dir() || linked(&metadata) {
        return Err(path_error(path, "is not a real directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(path_error(path, "has permissions wider than 0700"));
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct Snapshot {
    pub bytes: Vec<u8>,
    pub identity: FileIdentity,
}

pub fn read(path: &Path, maximum: u64) -> Result<Option<Snapshot>, Error> {
    check_components(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("Inspect state file", path, error)),
    };
    if linked(&metadata) || !metadata.is_file() {
        return Err(path_error(path, "is not a regular unlinked file"));
    }
    if metadata.len() > maximum {
        return Err(Error::new(
            "Error",
            format!(
                "State file {} exceeds {maximum} bytes; preserve it for explicit recovery.",
                path.display()
            ),
        )
        .coded("E_STATE_SIZE"));
    }
    let file = File::open(path).map_err(|error| io_error("Open state file", path, error))?;
    let information = paths::handle_information(&file)?;
    if paths::file_information(path)?.identity != information.identity {
        return Err(path_error(path, "changed while opening"));
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("Read state file", path, error))?;
    if bytes.len() as u64 > maximum {
        return Err(Error::new(
            "Error",
            format!(
                "State file {} exceeds {maximum} bytes; preserve it for explicit recovery.",
                path.display()
            ),
        )
        .coded("E_STATE_SIZE"));
    }
    Ok(Some(Snapshot {
        bytes,
        identity: information.identity,
    }))
}

pub fn unchanged(path: &Path, snapshot: &Snapshot, maximum: u64) -> Result<bool, Error> {
    Ok(read(path, maximum)?.is_some_and(|current| {
        current.identity == snapshot.identity && current.bytes == snapshot.bytes
    }))
}

pub fn remove_matching(path: &Path, snapshot: &Snapshot, maximum: u64) -> Result<bool, Error> {
    if !unchanged(path, snapshot, maximum)? {
        return Ok(false);
    }
    fs::remove_file(path).map_err(|error| io_error("Remove owned state file", path, error))?;
    Ok(true)
}

pub fn sync_directory(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|error| io_error("Sync state directory", path, error))
    }
    #[cfg(windows)]
    {
        check_components(path)
    }
}

pub fn write_exclusive(path: &Path, bytes: &[u8]) -> Result<FileIdentity, Error> {
    check_components(path)?;
    let mut handle = files::create_private_file(path)
        .map_err(|error| io_error("Create state file", path, error))?;
    handle
        .write_all(bytes)
        .and_then(|()| handle.sync_all())
        .map_err(|error| io_error("Write state file", path, error))?;
    paths::handle_information(&handle).map(|information| information.identity)
}

pub fn commit_new(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::type_error("State file requires a parent"))?;
    private_directory(parent, false)?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::type_error("State file requires a name"))?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy().trim_end_matches(".json"),
        files::nonce()?
    ));
    write_exclusive(&temporary, bytes)?;
    fs::hard_link(&temporary, path)
        .map_err(|error| io_error("Commit new state file", path, error))?;
    fs::remove_file(&temporary)
        .map_err(|error| io_error("Remove committed state temporary", &temporary, error))?;
    sync_directory(parent)
}

pub fn entries(path: &Path) -> Result<Vec<String>, Error> {
    check_components(path)?;
    let directory = match fs::read_dir(path) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error("Read state directory", path, error)),
    };
    let mut entries = Vec::new();
    for entry in directory {
        let entry = entry.map_err(|error| io_error("Read state entry", path, error))?;
        entries.push(
            entry
                .file_name()
                .into_string()
                .map_err(|_| path_error(path, "contains a non-Unicode entry"))?,
        );
    }
    entries.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
    Ok(entries)
}
