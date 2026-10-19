use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::files;
use crate::Error;
use crate::identity::sha256_hex;
use crate::paths;
use crate::text::{Text, trim};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Shared,
    PerWorktree,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::PerWorktree => "per-worktree",
        }
    }

    pub fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "shared" => Ok(Self::Shared),
            "per-worktree" => Ok(Self::PerWorktree),
            _ => Err(Error::new(
                "RepoStateError",
                format!(
                    "state scope must be \"shared\" or \"per-worktree\"; got {}.",
                    Text::from(value).json()
                ),
            )
            .coded("E_SCOPE")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Directory {
    pub scope: Scope,
    pub root: PathBuf,
    pub dir: PathBuf,
    pub legacy_root: PathBuf,
    pub legacy_dir_name: String,
    pub worktree_root: Option<PathBuf>,
    declared_layout: bool,
}

#[derive(Debug, Clone)]
pub struct StatePath {
    pub directory: Directory,
    pub path: PathBuf,
    pub legacy: bool,
}

fn present_path(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|value| !trim(&Text::from(value).0).is_empty())
}

fn bounded_relative(path: &Path) -> Result<(), Error> {
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(Error::new(
            "RepoStateError",
            "State path components must stay inside the declared scope",
        )
        .coded("E_SCOPE_PATH"));
    }
    Ok(())
}

pub fn worktree_state_key(root: &Path) -> Result<String, Error> {
    if !present_path(root) {
        return Err(Error::new(
            "RepoStateError",
            "worktree state requires a worktree root path.",
        )
        .coded("E_WORKTREE_KEY"));
    }
    let root = paths::absolute(root)?;
    let root = root
        .to_str()
        .ok_or_else(|| Error::type_error("Worktree path must contain valid Unicode"))?;
    Ok(format!(
        "wt-{}",
        &sha256_hex(root.to_lowercase().as_bytes())[..16]
    ))
}

pub fn resolve_directory(
    scope: Scope,
    repo_root: &Path,
    worktree_root: Option<&Path>,
    layout_name: Option<&str>,
) -> Result<Directory, Error> {
    if !present_path(repo_root) {
        return Err(Error::new(
            "RepoStateError",
            "state resolution requires a repository root.",
        )
        .coded("E_REPO_ROOT"));
    }
    if scope == Scope::PerWorktree && !worktree_root.is_some_and(present_path) {
        return Err(Error::new(
            "RepoStateError",
            "per-worktree state resolution requires a worktree root.",
        )
        .coded("E_WORKTREE_KEY"));
    }
    let root = paths::absolute(repo_root)?;
    let declared_layout = layout_name.is_some_and(|name| !name.is_empty());
    let dir = if let Some(name) = layout_name.filter(|name| !name.is_empty()) {
        bounded_relative(Path::new(name))?;
        root.join(name)
    } else {
        match scope {
            Scope::Shared => root.join(".roster-state").join("shared"),
            Scope::PerWorktree => {
                root.join(".roster-state")
                    .join("worktrees")
                    .join(worktree_state_key(worktree_root.ok_or_else(|| {
                        Error::new(
                            "RepoStateError",
                            "per-worktree state resolution requires a worktree root.",
                        )
                        .coded("E_WORKTREE_KEY")
                    })?)?)
            }
        }
    };
    Ok(Directory {
        scope,
        legacy_root: root.join(".roster"),
        root,
        dir,
        legacy_dir_name: layout_name.unwrap_or(".roster").to_owned(),
        worktree_root: worktree_root.map(paths::absolute).transpose()?,
        declared_layout,
    })
}

fn directory_exists(path: &Path) -> Result<bool, Error> {
    match fs::metadata(path) {
        Ok(entry) => Ok(entry.is_dir()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::new(
            "RepoStateError",
            format!(
                "State directory {} could not be inspected: {error}",
                path.display()
            ),
        )
        .coded("E_STATE_ROOT")),
    }
}

pub fn state_path(directory: Directory, segments: &[&str]) -> Result<StatePath, Error> {
    let mut target = directory.dir.clone();
    let mut legacy = directory.legacy_root.clone();
    for segment in segments {
        bounded_relative(Path::new(segment))?;
        target.push(segment);
        legacy.push(segment);
    }
    let canonical_exists = directory_exists(&directory.dir)?;
    let legacy_exists = directory_exists(&directory.legacy_root)?;
    let declared_layout = directory.declared_layout;
    if declared_layout {
        return Ok(StatePath {
            directory,
            path: target,
            legacy: true,
        });
    }
    let use_legacy = !canonical_exists && legacy_exists;
    Ok(StatePath {
        directory,
        path: if use_legacy { legacy } else { target },
        legacy: use_legacy,
    })
}

pub fn read_scoped_state(path: &StatePath) -> Result<Option<Text>, Error> {
    match files::read_state_file(&path.path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.code == Some("ENOENT") => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn write_scoped_state(path: &StatePath, value: &Text) -> Result<PathBuf, Error> {
    let parent = path
        .path
        .parent()
        .ok_or_else(|| Error::type_error("Scoped state requires a parent directory"))?;
    files::create_private_directories(parent)?;
    files::atomic_write_file(&path.path, value)
}

impl StatePath {
    pub fn json(&self) -> String {
        use crate::json::Value;
        Value::object(vec![
            ("scope", Value::string(self.directory.scope.as_str())),
            (
                "root",
                Value::string(self.directory.root.to_string_lossy().as_ref()),
            ),
            (
                "dir",
                Value::string(self.directory.dir.to_string_lossy().as_ref()),
            ),
            (
                "legacyRoot",
                Value::string(self.directory.legacy_root.to_string_lossy().as_ref()),
            ),
            (
                "legacyDirName",
                Value::string(&self.directory.legacy_dir_name),
            ),
            (
                "worktreeRoot",
                self.directory
                    .worktree_root
                    .as_ref()
                    .map_or(Value::Null, |path| {
                        Value::string(path.to_string_lossy().as_ref())
                    }),
            ),
            ("path", Value::string(self.path.to_string_lossy().as_ref())),
            ("legacy", Value::Boolean(self.legacy)),
        ])
        .json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_paths_refuse_traversal_and_unknown_scope() {
        assert!(Scope::parse("machine").is_err_and(|error| error.code == Some("E_SCOPE")));
        assert!(bounded_relative(Path::new("../outside")).is_err());
        assert!(bounded_relative(Path::new("checkpoints/task")).is_ok());
    }

    #[test]
    fn native_worktree_keys_are_stable_and_distinct() -> Result<(), Error> {
        let root = paths::absolute(Path::new("."))?;
        assert_eq!(
            worktree_state_key(&root)?,
            worktree_state_key(&root.join("x").join(".."))?
        );
        assert_ne!(
            worktree_state_key(&root)?,
            worktree_state_key(&root.join("other"))?
        );
        Ok(())
    }
}
