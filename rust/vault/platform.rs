use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::{MAX_DIRECTORY_ENTRIES, VaultError, hexadecimal, random};
use crate::paths::{self, FileIdentity};

#[cfg(unix)]
#[path = "unix.rs"]
mod native;
#[cfg(windows)]
#[path = "windows.rs"]
mod native;

pub(super) struct Directory {
    path: PathBuf,
    handle: File,
    ancestors: Vec<(PathBuf, File, FileIdentity)>,
}

fn unchanged(ancestors: &[(PathBuf, File, FileIdentity)]) -> Result<(), VaultError> {
    for (path, handle, identity) in ancestors {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| VaultError::Changed)?;
        if crate::state::secure::linked(&metadata)
            || !metadata.is_dir()
            || paths::handle_information(handle)
                .map_err(|_| VaultError::Changed)?
                .identity
                != *identity
            || paths::file_information(path)
                .map_err(|_| VaultError::Changed)?
                .identity
                != *identity
        {
            return Err(VaultError::Changed);
        }
    }
    Ok(())
}

impl Directory {
    pub(super) fn prepare(path: &Path, create: bool) -> Result<Option<Self>, VaultError> {
        native::prepare(path, create)
    }

    pub(super) fn current(&self) -> Result<(), VaultError> {
        if paths::handle_information(&self.handle)
            .map_err(|_| VaultError::Changed)?
            .identity
            != paths::file_information(&self.path)
                .map_err(|_| VaultError::Changed)?
                .identity
        {
            return Err(VaultError::Changed);
        }
        unchanged(&self.ancestors)
    }

    pub(super) fn entries(&self) -> Result<Vec<String>, VaultError> {
        self.current()?;
        native::entries(self)
    }

    pub(super) fn inspect(&self, name: &str) -> Result<Option<File>, VaultError> {
        self.current()?;
        let file = native::inspect(self, name)?;
        self.current()?;
        Ok(file)
    }

    pub(super) fn read(
        &self,
        name: &str,
        maximum: usize,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
        let Some(handle) = self.inspect(name)? else {
            return Ok(None);
        };
        let length = handle.metadata().map_err(|_| VaultError::Io)?.len();
        if length > maximum as u64 {
            return Err(if name == ".key" {
                VaultError::KeySize
            } else {
                VaultError::Limit
            });
        }
        let mut bytes = Zeroizing::new(Vec::new());
        (&handle)
            .take(maximum as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| VaultError::Io)?;
        if bytes.len() > maximum {
            return Err(VaultError::Limit);
        }
        native::verify_file(&handle)?;
        self.current()?;
        Ok(Some(bytes))
    }

    pub(super) fn complete(&self, bytes: &[u8]) -> Result<Pending<'_>, VaultError> {
        self.current()?;
        let name = format!(".tmp-{}", hexadecimal(&random::<16>()?));
        let mut file = native::create(self, &name)?;
        let identity = paths::handle_information(&file)
            .map_err(|_| VaultError::Io)?
            .identity;
        let mut pending = Pending {
            directory: self,
            name,
            identity,
            active: true,
        };
        let result = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| VaultError::Io)
            .and_then(|()| native::verify_file(&file));
        drop(file);
        if let Err(error) = result {
            pending.cleanup()?;
            return Err(error);
        }
        Ok(pending)
    }

    pub(super) fn atomic(
        &self,
        name: &str,
        bytes: &[u8],
        exclusive: bool,
    ) -> Result<bool, VaultError> {
        if let Some(existing) = self.inspect(name)? {
            drop(existing);
            if exclusive {
                return Ok(false);
            }
        }
        let mut pending = self.complete(bytes)?;
        let result = pending.publish(name, exclusive);
        pending.cleanup()?;
        result
    }
}

pub(super) struct Pending<'a> {
    directory: &'a Directory,
    name: String,
    identity: FileIdentity,
    active: bool,
}

impl Pending<'_> {
    pub(super) fn publish(&mut self, name: &str, exclusive: bool) -> Result<bool, VaultError> {
        self.directory.current()?;
        let file = self
            .directory
            .inspect(&self.name)?
            .ok_or(VaultError::Changed)?;
        if paths::handle_information(&file)
            .map_err(|_| VaultError::Io)?
            .identity
            != self.identity
        {
            return Err(VaultError::Changed);
        }
        drop(file);
        if let Some(existing) = self.directory.inspect(name)? {
            drop(existing);
            if exclusive {
                return Ok(false);
            }
        }
        if native::publish(self.directory, &self.name, name, exclusive)? {
            self.active = false;
            native::sync(self.directory)?;
            self.directory.current()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn cleanup(&mut self) -> Result<(), VaultError> {
        if self.active {
            native::remove(self.directory, &self.name, &self.identity)
                .map_err(|_| VaultError::Cleanup)?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}
