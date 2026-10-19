use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use windows_permissions::constants::{SeObjectType, SecurityInformation};
use windows_permissions::{LocalBox, SecurityDescriptor, wrappers};
use winsafe::{TokenInfo, co, prelude::*};

use super::{Directory, MAX_DIRECTORY_ENTRIES, VaultError};
use crate::paths::{self, FileIdentity};
use crate::state::secure;

const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const READ_ATTRIBUTES: u32 = 0x80;
const LIST_DIRECTORY: u32 = 1;
const SHARE_READ: u32 = 1;
const SHARE_WRITE: u32 = 2;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;

fn effective_sid() -> Result<String, VaultError> {
    let token = winsafe::HACCESSTOKEN::GetCurrentThreadEffectiveToken();
    let info = token
        .GetTokenInformation(co::TOKEN_INFORMATION_CLASS::User)
        .map_err(|_| VaultError::Permissions)?;
    let TokenInfo::User(user) = info else {
        return Err(VaultError::Permissions);
    };
    let sid = user.User.Sid().ok_or(VaultError::Permissions)?;
    winsafe::ConvertSidToStringSid(sid).map_err(|_| VaultError::Permissions)
}

fn render(
    descriptor: &SecurityDescriptor,
    information: SecurityInformation,
) -> Result<String, VaultError> {
    wrappers::ConvertSecurityDescriptorToStringSecurityDescriptor(descriptor, information)
        .map_err(|_| VaultError::Permissions)?
        .into_string()
        .map_err(|_| VaultError::Permissions)
}

fn require_owner(
    actual: &SecurityDescriptor,
    expected: &SecurityDescriptor,
) -> Result<(), VaultError> {
    if render(actual, SecurityInformation::Owner)? != render(expected, SecurityInformation::Owner)?
    {
        return Err(VaultError::Ownership);
    }
    Ok(())
}

fn permissions(handle: &mut File, directory: bool) -> Result<(), VaultError> {
    let sid = effective_sid()?;
    let flags = if directory { "OICI" } else { "" };
    let expected: LocalBox<SecurityDescriptor> = format!("O:{sid}D:P(A;{flags};FA;;;{sid})")
        .parse()
        .map_err(|_| VaultError::Permissions)?;
    let actual = wrappers::GetSecurityInfo(
        handle,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Owner | SecurityInformation::Dacl,
    )
    .map_err(|_| VaultError::Permissions)?;
    require_owner(&actual, &expected)?;
    let desired = render(&expected, SecurityInformation::Dacl)?;
    let actual_dacl = render(&actual, SecurityInformation::Dacl)?;
    if actual_dacl != desired && actual_dacl != desired.replacen("D:P", "D:PAI", 1) {
        let dacl = wrappers::GetSecurityDescriptorDacl(&expected)
            .map_err(|_| VaultError::Permissions)?
            .ok_or(VaultError::Permissions)?;
        let information = SecurityInformation::Dacl | SecurityInformation::ProtectedDacl;
        wrappers::SetSecurityInfo(
            handle,
            SeObjectType::SE_FILE_OBJECT,
            information,
            None,
            None,
            Some(dacl),
            None,
        )
        .map_err(|_| VaultError::Permissions)?;
    }
    let verified = wrappers::GetSecurityInfo(
        handle,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Owner | SecurityInformation::Dacl,
    )
    .map_err(|_| VaultError::Permissions)?;
    let dacl = render(&verified, SecurityInformation::Dacl)?;
    require_owner(&verified, &expected)?;
    if dacl != desired && dacl != desired.replacen("D:P", "D:PAI", 1) {
        return Err(VaultError::Permissions);
    }
    Ok(())
}

fn directory_handle(path: &Path, private: bool) -> Result<File, VaultError> {
    let mut options = OpenOptions::new();
    let access =
        READ_ATTRIBUTES | LIST_DIRECTORY | if private { READ_CONTROL | WRITE_DAC } else { 0 };
    options
        .access_mode(access)
        .share_mode(SHARE_READ | SHARE_WRITE)
        .custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS);
    let handle = options.open(path).map_err(|_| VaultError::Io)?;
    let metadata = handle.metadata().map_err(|_| VaultError::Io)?;
    if !metadata.is_dir() || secure::linked(&metadata) {
        return Err(VaultError::Linked);
    }
    Ok(handle)
}

pub(super) fn prepare(path: &Path, create: bool) -> Result<Option<Directory>, VaultError> {
    let mut walked = PathBuf::new();
    let mut ancestors = Vec::new();
    for component in path.components() {
        walked.push(component);
        if !walked.has_root() {
            continue;
        }
        let created = match fs::symlink_metadata(&walked) {
            Ok(metadata) => {
                if secure::linked(&metadata) {
                    return Err(if walked == path {
                        VaultError::Linked
                    } else {
                        VaultError::Containment
                    });
                }
                if !metadata.is_dir() {
                    return Err(VaultError::Linked);
                }
                false
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if ancestors.is_empty() {
                    return Err(VaultError::Io);
                }
                if !create {
                    super::unchanged(&ancestors)?;
                    return Ok(None);
                }
                match fs::create_dir(&walked) {
                    Ok(()) => true,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
                    Err(_) => return Err(VaultError::Io),
                }
            }
            Err(_) => return Err(VaultError::Io),
        };
        let expected = paths::file_information(&walked)
            .map_err(|_| VaultError::Io)?
            .identity;
        let mut handle = directory_handle(&walked, created || walked == path)?;
        let identity = paths::handle_information(&handle)
            .map_err(|_| VaultError::Io)?
            .identity;
        if identity != expected {
            return Err(VaultError::Changed);
        }
        match fs::symlink_metadata(walked.join(".git")) {
            Ok(_) => return Err(VaultError::Containment),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(VaultError::Io),
        }
        if created || walked == path {
            permissions(&mut handle, true)?;
        }
        if paths::file_information(&walked)
            .map_err(|_| VaultError::Io)?
            .identity
            != identity
        {
            return Err(VaultError::Changed);
        }
        ancestors.push((walked.clone(), handle, identity));
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
    if !metadata.is_file()
        || secure::linked(&metadata)
        || paths::handle_information(file)
            .map_err(|_| VaultError::Io)?
            .links
            != 1
    {
        return Err(VaultError::Linked);
    }
    Ok(())
}

pub(super) fn inspect(directory: &Directory, name: &str) -> Result<Option<File>, VaultError> {
    let path = directory.path.join(name);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(VaultError::Io),
    };
    if !metadata.is_file() || secure::linked(&metadata) {
        return Err(VaultError::Linked);
    }
    let before = paths::file_information(&path).map_err(|_| VaultError::Io)?;
    let mut file = OpenOptions::new()
        .access_mode(GENERIC_READ | READ_CONTROL | WRITE_DAC)
        .share_mode(SHARE_READ)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(&path)
        .map_err(|_| VaultError::Io)?;
    verify_file(&file)?;
    if paths::handle_information(&file)
        .map_err(|_| VaultError::Io)?
        .identity
        != before.identity
    {
        return Err(VaultError::Changed);
    }
    permissions(&mut file, false)?;
    directory.current()?;
    Ok(Some(file))
}

pub(super) fn create(directory: &Directory, name: &str) -> Result<File, VaultError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .access_mode(GENERIC_READ | GENERIC_WRITE | READ_CONTROL | WRITE_DAC)
        .share_mode(0)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(directory.path.join(name))
        .map_err(|_| VaultError::Io)?;
    verify_file(&file)?;
    permissions(&mut file, false)?;
    Ok(file)
}

pub(super) fn entries(directory: &Directory) -> Result<Vec<String>, VaultError> {
    let _ = paths::handle_information(&directory.handle).map_err(|_| VaultError::Io)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&directory.path).map_err(|_| VaultError::Io)? {
        if entries.len() == MAX_DIRECTORY_ENTRIES {
            return Err(VaultError::Limit);
        }
        entries.push(
            entry
                .map_err(|_| VaultError::Io)?
                .file_name()
                .into_string()
                .map_err(|_| VaultError::InvalidEntry)?,
        );
    }
    directory.current()?;
    Ok(entries)
}

pub(super) fn publish(
    directory: &Directory,
    source: &str,
    target: &str,
    exclusive: bool,
) -> Result<bool, VaultError> {
    let source = directory.path.join(source);
    let target = directory.path.join(target);
    let flags = if exclusive {
        co::MOVEFILE::WRITE_THROUGH
    } else {
        co::MOVEFILE::REPLACE_EXISTING
    };
    match winsafe::MoveFileEx(
        source.to_str().ok_or(VaultError::Directory)?,
        Some(target.to_str().ok_or(VaultError::Directory)?),
        flags,
    ) {
        Ok(()) => Ok(true),
        Err(error)
            if exclusive && matches!(error, co::ERROR::ALREADY_EXISTS | co::ERROR::FILE_EXISTS) =>
        {
            Ok(false)
        }
        Err(_) => Err(VaultError::Io),
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
    drop(file);
    fs::remove_file(directory.path.join(name)).map_err(|_| VaultError::Io)
}

pub(super) fn sync(directory: &Directory) -> Result<(), VaultError> {
    directory.current()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_descriptors_require_the_effective_sid_not_a_foreign_or_absent_owner()
    -> Result<(), VaultError> {
        let sid = effective_sid()?;
        let expected: LocalBox<SecurityDescriptor> = format!("O:{sid}")
            .parse()
            .map_err(|_| VaultError::Permissions)?;
        let foreign = if sid == "S-1-5-18" {
            "S-1-1-0"
        } else {
            "S-1-5-18"
        };
        let foreign: LocalBox<SecurityDescriptor> = format!("O:{foreign}")
            .parse()
            .map_err(|_| VaultError::Permissions)?;
        assert_eq!(require_owner(&expected, &expected), Ok(()));
        assert_eq!(
            require_owner(&foreign, &expected),
            Err(VaultError::Ownership)
        );
        let absent: LocalBox<SecurityDescriptor> = "D:P(A;;FA;;;SY)"
            .parse()
            .map_err(|_| VaultError::Permissions)?;
        assert!(require_owner(&absent, &expected).is_err());
        Ok(())
    }
}
