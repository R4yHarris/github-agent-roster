use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{self as native, AtFlags, Mode, OFlags};

use super::{Directory, MAX_DIRECTORY_ENTRIES, VaultError};
use crate::paths::{self, FileIdentity};

fn owned(file: &File, directory: bool) -> Result<(), VaultError> {
    let metadata = file.metadata().map_err(|_| VaultError::Io)?;
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(VaultError::Ownership);
    }
    native::fchmod(
        file,
        Mode::from_raw_mode(if directory { 0o700 } else { 0o600 }),
    )
    .map_err(|_| VaultError::Permissions)?;
    let metadata = file.metadata().map_err(|_| VaultError::Io)?;
    if metadata.mode() & 0o7777 != if directory { 0o700 } else { 0o600 } {
        return Err(VaultError::Permissions);
    }
    Ok(())
}

pub(super) fn prepare(path: &Path, create: bool) -> Result<Option<Directory>, VaultError> {
    let root = native::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| VaultError::Io)?;
    let mut walked = PathBuf::from("/");
    let identity = paths::handle_information(&root)
        .map_err(|_| VaultError::Io)?
        .identity;
    let mut ancestors = vec![(walked.clone(), root, identity)];
    for component in path.components().chain(std::iter::once(Component::CurDir)) {
        let parent = &ancestors.last().ok_or(VaultError::Directory)?.1;
        match native::statat(parent, ".git", AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Err(VaultError::Containment),
            Err(rustix::io::Errno::NOENT) => {}
            Err(_) => return Err(VaultError::Io),
        }
        let Component::Normal(name) = component else {
            continue;
        };
        let mut created = false;
        let open = || {
            native::openat(
                parent,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
        };
        let file = match open() {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) => {
                if !create {
                    super::unchanged(&ancestors)?;
                    return Ok(None);
                }
                match native::mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) => created = true,
                    Err(rustix::io::Errno::EXIST) => {}
                    Err(_) => return Err(VaultError::Io),
                }
                File::from(open().map_err(|_| VaultError::Linked)?)
            }
            Err(_) => return Err(VaultError::Linked),
        };
        walked.push(name);
        match native::statat(&file, ".git", AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Err(VaultError::Containment),
            Err(rustix::io::Errno::NOENT) => {}
            Err(_) => return Err(VaultError::Io),
        }
        if created || walked == path {
            owned(&file, true)?;
        }
        let identity = paths::handle_information(&file)
            .map_err(|_| VaultError::Io)?
            .identity;
        ancestors.push((walked.clone(), file, identity));
    }
    let handle = ancestors
        .last()
        .ok_or(VaultError::Directory)?
        .1
        .try_clone()
        .map_err(|_| VaultError::Io)?;
    let directory = Directory {
        path: path.to_path_buf(),
        handle,
        ancestors,
    };
    directory.current()?;
    Ok(Some(directory))
}

pub(super) fn verify_file(file: &File) -> Result<(), VaultError> {
    let metadata = file.metadata().map_err(|_| VaultError::Io)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(VaultError::Linked);
    }
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(VaultError::Ownership);
    }
    Ok(())
}

pub(super) fn inspect(directory: &Directory, name: &str) -> Result<Option<File>, VaultError> {
    let file = match native::openat(
        &directory.handle,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => File::from(file),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM) => {
            return Err(VaultError::Permissions);
        }
        Err(rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) => return Err(VaultError::Linked),
        Err(_) => return Err(VaultError::Io),
    };
    verify_file(&file)?;
    owned(&file, false)?;
    Ok(Some(file))
}

pub(super) fn create(directory: &Directory, name: &str) -> Result<File, VaultError> {
    let file = native::openat(
        &directory.handle,
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map(File::from)
    .map_err(|_| VaultError::Io)?;
    verify_file(&file)?;
    owned(&file, false)?;
    Ok(file)
}

pub(super) fn entries(directory: &Directory) -> Result<Vec<String>, VaultError> {
    let mut entries = Vec::new();
    let mut reader = native::Dir::read_from(&directory.handle).map_err(|_| VaultError::Io)?;
    for entry in &mut reader {
        let entry = entry.map_err(|_| VaultError::Io)?;
        let name = entry
            .file_name()
            .to_str()
            .map_err(|_| VaultError::InvalidEntry)?;
        if matches!(name, "." | "..") {
            continue;
        }
        if entries.len() == MAX_DIRECTORY_ENTRIES {
            return Err(VaultError::Limit);
        }
        entries.push(name.to_owned());
    }
    Ok(entries)
}

pub(super) fn publish(
    directory: &Directory,
    source: &str,
    target: &str,
    exclusive: bool,
) -> Result<bool, VaultError> {
    if !exclusive {
        native::renameat(&directory.handle, source, &directory.handle, target)
            .map_err(|_| VaultError::Io)?;
        return Ok(true);
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "redox"
    ))]
    {
        match native::renameat_with(
            &directory.handle,
            source,
            &directory.handle,
            target,
            native::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => Ok(true),
            Err(rustix::io::Errno::EXIST) => Ok(false),
            Err(rustix::io::Errno::NOSYS | rustix::io::Errno::NOTSUP) => {
                Err(VaultError::Unsupported)
            }
            Err(_) => Err(VaultError::Io),
        }
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "redox"
    )))]
    {
        Err(VaultError::Unsupported)
    }
}

pub(super) fn remove(
    directory: &Directory,
    name: &str,
    identity: &FileIdentity,
) -> Result<(), VaultError> {
    directory.current()?;
    let Some(file) = inspect(directory, name)? else {
        return Ok(());
    };
    if paths::handle_information(&file)
        .map_err(|_| VaultError::Io)?
        .identity
        != *identity
    {
        return Err(VaultError::Changed);
    }
    native::unlinkat(&directory.handle, name, AtFlags::empty()).map_err(|_| VaultError::Io)
}

pub(super) fn sync(directory: &Directory) -> Result<(), VaultError> {
    directory.handle.sync_all().map_err(|_| VaultError::Io)
}
