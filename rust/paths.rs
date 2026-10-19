use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::Error;
use crate::text::{Text, trim};

fn io_error(operation: &str, error: io::Error) -> Error {
    Error::new("Error", format!("{operation}: {error}"))
}

pub fn absolute(path: &Path) -> Result<PathBuf, Error> {
    let raw = std::path::absolute(path).map_err(|error| io_error("Cannot resolve path", error))?;
    let mut output = PathBuf::new();
    for component in raw.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if output
                    .components()
                    .next_back()
                    .is_some_and(|part| matches!(part, Component::Normal(_)))
                {
                    output.pop();
                }
            }
            _ => output.push(component),
        }
    }
    Ok(output)
}

pub fn resolve(base: &Path, path: &Path) -> Result<PathBuf, Error> {
    absolute(&base.join(path))
}

pub fn resolve_project_root(cwd: &Path) -> Result<PathBuf, Error> {
    let start = absolute(cwd)?;
    let mut current = start.as_path();
    loop {
        match fs::symlink_metadata(current.join(".git")) {
            Ok(marker) => {
                if marker.file_type().is_symlink() || (!marker.is_file() && !marker.is_dir()) {
                    return Err(Error::new(
                        "Error",
                        "Git worktree marker must be a regular file or directory",
                    ));
                }
                return Ok(current.to_owned());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("Cannot inspect Git worktree marker", error)),
        }
        let Some(parent) = current.parent() else {
            return Ok(start);
        };
        current = parent;
    }
}

#[derive(Debug, Clone)]
pub struct RepositoryPaths {
    pub worktree: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub repository: PathBuf,
}

pub fn repository_paths(cwd: &Path) -> Result<RepositoryPaths, Error> {
    use crate::state::secure;

    fn directory(path: &Path) -> Result<PathBuf, Error> {
        secure::check_components(path)?;
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| secure::io_error("Inspect repository directory", path, error))?;
        if !metadata.is_dir() || secure::linked(&metadata) {
            return Err(secure::path_error(
                path,
                "is not a real repository directory",
            ));
        }
        fs::canonicalize(path)
            .map(strip_verbatim)
            .map_err(|error| secure::io_error("Resolve repository directory", path, error))
    }

    fn pointer(path: &Path) -> Result<Option<String>, Error> {
        let Some(snapshot) = secure::read(path, 65_536)? else {
            return Ok(None);
        };
        if file_information(path)?.links != 1 || !secure::unchanged(path, &snapshot, 65_536)? {
            return Err(secure::path_error(
                path,
                "is not a stable single-link Git pointer",
            ));
        }
        let text = String::from_utf8(snapshot.bytes)
            .map_err(|_| Error::type_error("Git directory pointers must be UTF-8"))?;
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() || text.contains(['\r', '\n', '\0']) {
            return Err(Error::type_error(
                "Git directory pointer must contain one nonempty path",
            ));
        }
        Ok(Some(text.to_owned()))
    }

    let worktree = directory(&resolve_project_root(cwd)?)?;
    let marker = worktree.join(".git");
    secure::check_components(&marker)?;
    let metadata = fs::symlink_metadata(&marker)
        .map_err(|error| secure::io_error("Inspect Git worktree marker", &marker, error))?;
    let git_dir = if metadata.is_dir() {
        directory(&marker)?
    } else {
        let text = pointer(&marker)?
            .ok_or_else(|| Error::type_error("Git worktree marker disappeared"))?;
        let path = text
            .strip_prefix("gitdir: ")
            .ok_or_else(|| Error::type_error("Git worktree marker must contain a gitdir path"))?;
        directory(&resolve(&worktree, Path::new(path))?)?
    };
    let common_dir = match pointer(&git_dir.join("commondir"))? {
        Some(path) => directory(&resolve(&git_dir, Path::new(&path))?)?,
        None => git_dir.clone(),
    };
    let repository = common_dir
        .parent()
        .ok_or_else(|| Error::type_error("Git common directory has no owning repository"))?
        .to_path_buf();
    let canonical_marker = repository.join(".git");
    secure::check_components(&canonical_marker)?;
    if !canonical_marker.is_dir()
        || file_information(&canonical_marker)?.identity != file_information(&common_dir)?.identity
    {
        return Err(Error::type_error(
            "Git common directory must belong to a real non-bare repository; ambiguous ownership is refused",
        ));
    }
    if git_dir != common_dir {
        let backlink = pointer(&git_dir.join("gitdir"))?
            .ok_or_else(|| Error::type_error("Linked worktree registration has no Git backlink"))?;
        let backlink = resolve(&git_dir, Path::new(&backlink))?;
        secure::check_components(&backlink)?;
        if file_information(&backlink)?.identity != file_information(&marker)?.identity {
            return Err(Error::type_error(
                "Linked worktree registration belongs to a different Git marker",
            ));
        }
    }
    Ok(RepositoryPaths {
        worktree,
        git_dir,
        common_dir,
        repository,
    })
}

pub fn resolve_contracts_path(
    repo_root: &Path,
    cwd: &Path,
    configured: Option<&Path>,
) -> Result<PathBuf, Error> {
    if let Some(configured) = configured {
        let text = configured
            .to_str()
            .ok_or_else(|| Error::type_error("GITHUB_AGENT_CONTRACTS must be a non-empty path"))?;
        if trim(&Text::from(text).0).is_empty() {
            return Err(Error::type_error(
                "GITHUB_AGENT_CONTRACTS must be a non-empty path",
            ));
        }
    }
    let root = absolute(repo_root)?;
    let mut candidates = vec![resolve(
        &root,
        Path::new("vendor").join("github-agent-contracts").as_path(),
    )?];
    if let Some(configured) = configured {
        candidates.push(resolve(cwd, configured)?);
    }
    candidates.push(resolve(
        root.parent().unwrap_or(&root),
        Path::new("github-agent-contracts"),
    )?);
    for candidate in &candidates {
        match fs::metadata(candidate.join("scripts").join("agent-pr.mjs")) {
            Ok(file) if file.is_file() => return Ok(candidate.clone()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("Cannot inspect contracts publisher", error)),
        }
    }
    Err(Error::new(
        "Error",
        format!(
            "github-agent-contracts is required: no scripts/agent-pr.mjs file found in {}. \
         Run git submodule update --init --recursive or set GITHUB_AGENT_CONTRACTS to a complete clone.",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ))
}

fn path_key(path: &Path) -> Result<Vec<String>, Error> {
    path.components()
        .map(|component| {
            let text = component
                .as_os_str()
                .to_str()
                .ok_or_else(|| Error::type_error("Path must contain valid Unicode"))?;
            Ok(if cfg!(windows) {
                text.to_lowercase()
            } else {
                text.to_owned()
            })
        })
        .collect()
}

pub(crate) fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        let mut components = path.components();
        let mut output = match components.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", drive as char)),
                Prefix::VerbatimUNC(server, share) => {
                    let mut name = std::ffi::OsString::from("\\\\");
                    name.push(server);
                    name.push("\\");
                    name.push(share);
                    PathBuf::from(name)
                }
                _ => return path,
            },
            _ => return path,
        };
        for component in components {
            output.push(component);
        }
        output
    }
    #[cfg(not(windows))]
    {
        path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub device: u64,
    pub index: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct FileInformation {
    pub identity: FileIdentity,
    pub links: u64,
}

#[cfg(windows)]
pub fn file_information(path: &Path) -> Result<FileInformation, Error> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .access_mode(0x80)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)
        .map_err(|error| io_error("Cannot inspect file identity", error))?;
    handle_information(&file)
}

#[cfg(windows)]
pub fn handle_information(file: &fs::File) -> Result<FileInformation, Error> {
    let information = winapi_util::file::information(file)
        .map_err(|error| io_error("Cannot inspect file identity", error))?;
    Ok(FileInformation {
        identity: FileIdentity {
            device: information.volume_serial_number(),
            index: information.file_index(),
        },
        links: information.number_of_links(),
    })
}

#[cfg(unix)]
pub fn file_information(path: &Path) -> Result<FileInformation, Error> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error("Cannot inspect file identity", error))?;
    Ok(FileInformation {
        identity: FileIdentity {
            device: metadata.dev(),
            index: metadata.ino(),
        },
        links: metadata.nlink(),
    })
}

#[cfg(unix)]
pub fn handle_information(file: &fs::File) -> Result<FileInformation, Error> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("Cannot inspect file identity", error))?;
    Ok(FileInformation {
        identity: FileIdentity {
            device: metadata.dev(),
            index: metadata.ino(),
        },
        links: metadata.nlink(),
    })
}

fn physically_contained(root: &Path, nearest: &Path) -> Result<bool, Error> {
    let identity = file_information(root)?.identity;
    for ancestor in nearest.ancestors() {
        if file_information(ancestor)?.identity == identity {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn ensure_local_path(file: &Path, repo_root: &Path) -> Result<(), Error> {
    let root = fs::canonicalize(repo_root)
        .map(strip_verbatim)
        .map_err(|error| io_error("Cannot resolve roster repository", error))?;
    let target = absolute(file)?;
    let root_parts = path_key(&root)?;
    let target_parts = path_key(&target)?;
    if target_parts.len() <= root_parts.len() || !target_parts.starts_with(&root_parts) {
        return Err(Error::new(
            "Error",
            "Path must stay inside the roster repository",
        ));
    }
    let mut current = PathBuf::new();
    let mut nearest = None;
    for component in target.components() {
        current.push(component);
        if !current.has_root() {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(entry) if entry.file_type().is_symlink() => {
                return Err(Error::new("Error", "Roster path may not contain symlinks"));
            }
            Ok(_) => nearest = Some(current.clone()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(io_error("Cannot inspect roster path", error)),
        }
    }
    let nearest = nearest
        .ok_or_else(|| Error::new("Error", "Path must stay inside the roster repository"))?;
    if !physically_contained(&root, &nearest)? {
        return Err(Error::new(
            "Error",
            "Path must stay inside the roster repository",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_resolution_is_lexical_and_stays_at_the_native_root() -> Result<(), Error> {
        let base = absolute(Path::new("."))?;
        assert_eq!(
            resolve(&base, Path::new("a").join("..").join("b").as_path())?,
            base.join("b")
        );
        Ok(())
    }
}
