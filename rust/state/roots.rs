use std::fs;
use std::path::{Component, Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::Error;
use crate::json::Value;
use crate::paths;
use crate::state::{files, secure};
use crate::text::{Text, trim};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Machine,
    Repo,
    Worktree,
    Run,
}

impl Scope {
    pub fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "machine" => Ok(Self::Machine),
            "repo" | "repository" => Ok(Self::Repo),
            "worktree" => Ok(Self::Worktree),
            "run" => Ok(Self::Run),
            _ => Err(Error::new("StateScopeError", format!("\"{value}\" is not a roster state scope; expected one of machine, repo, worktree, run.")).coded("E_SCOPE")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Machine => "machine",
            Self::Repo => "repo",
            Self::Worktree => "worktree",
            Self::Run => "run",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Handle {
    pub scope: Scope,
    pub root: PathBuf,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub run_id: Option<String>,
    pub writable: bool,
}

impl Handle {
    pub fn path(&self, scope: Scope, segments: &[&str]) -> Result<PathBuf, Error> {
        if scope != self.scope {
            return Err(Error::new("StateScopeError", format!("state handle is scoped to \"{}\", but \"{}\" state was requested; resolve a matching scoped handle.", self.scope.as_str(), scope.as_str())).coded("E_SCOPE"));
        }
        let mut result = self.root.clone();
        for segment in segments {
            if Path::new(segment)
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
            {
                return Err(secure::path_error(&result, "escapes its declared scope"));
            }
            result.push(segment);
        }
        secure::check_components(&result)?;
        Ok(result)
    }

    pub fn ensure(&self) -> Result<&Path, Error> {
        secure::private_directory(&self.root, true)?;
        Ok(&self.root)
    }

    pub fn json(&self) -> String {
        Value::object(vec![
            ("scope", Value::string(self.scope.as_str())),
            ("root", Value::string(self.root.to_string_lossy().as_ref())),
            (
                "repoId",
                self.repo_id.as_deref().map_or(Value::Null, Value::string),
            ),
            (
                "worktreeId",
                self.worktree_id
                    .as_deref()
                    .map_or(Value::Null, Value::string),
            ),
            (
                "runId",
                self.run_id.as_deref().map_or(Value::Null, Value::string),
            ),
            ("writable", Value::Boolean(self.writable)),
        ])
        .json()
    }
}

fn env_path(env: &Value, name: &str) -> Result<Option<PathBuf>, Error> {
    env.get(name)
        .map(|value| {
            let text = value
                .as_string()
                .ok_or_else(|| root_error(format!("{name} must be a non-empty path when set.")))?;
            if trim(&text.0).is_empty() {
                return Err(root_error(format!(
                    "{name} must be a non-empty path when set."
                )));
            }
            text.to_utf8().map(PathBuf::from)
        })
        .transpose()
}

fn root_error(message: impl Into<String>) -> Error {
    Error::new("StateRootError", message)
}

pub fn platform_default_root(
    env: &Value,
    platform: &str,
    home: Option<&Path>,
) -> Result<PathBuf, Error> {
    match platform {
        "win32" => {
            let local = env_path(env, "LOCALAPPDATA")?.or_else(|| home.map(|home| home.join("AppData").join("Local")))
                .ok_or_else(|| root_error("Machine state requires LOCALAPPDATA or an explicit user home; no cwd fallback is permitted."))?;
            Ok(paths::absolute(&local)?.join("roster").join("state"))
        }
        "darwin" => home
            .map(|home| {
                home.join("Library")
                    .join("Application Support")
                    .join("roster")
            })
            .ok_or_else(|| {
                root_error(
                    "Machine state requires an explicit user home; no cwd fallback is permitted.",
                )
            }),
        _ => {
            let state = env_path(env, "XDG_STATE_HOME")?.or_else(|| home.map(|home| home.join(".local").join("state")))
                .ok_or_else(|| root_error("Machine state requires XDG_STATE_HOME or an explicit user home; no cwd fallback is permitted."))?;
            Ok(paths::absolute(&state)?.join("roster"))
        }
    }
}

pub fn resolve_machine_root(
    env: &Value,
    platform: &str,
    home: Option<&Path>,
) -> Result<Handle, Error> {
    let configured = if env
        .get("PATHS_OVERRIDE")
        .is_some_and(|value| value != &Value::Null)
    {
        env_path(env, "PATHS_OVERRIDE")?
    } else {
        env_path(env, "ROSTER_STATE_ROOT")?
    };
    let overridden = configured.is_some();
    let root = paths::absolute(
        &configured.map_or_else(|| platform_default_root(env, platform, home), Ok)?,
    )?;
    secure::check_components(&root)?;
    if overridden {
        secure::private_directory(&root, false)?;
        let probe = root.join(format!(
            ".roster-probe-{}-{}",
            std::process::id(),
            files::nonce()?
        ));
        let identity = secure::write_exclusive(&probe, b"")?;
        let snapshot = secure::Snapshot {
            identity,
            bytes: Vec::new(),
        };
        if !secure::remove_matching(&probe, &snapshot, 1)? {
            return Err(root_error(
                "Machine state write probe changed; preserve the root for inspection.",
            ));
        }
    }
    Ok(Handle {
        scope: Scope::Machine,
        root,
        repo_id: None,
        worktree_id: None,
        run_id: None,
        writable: overridden,
    })
}

fn hash_id(value: &str) -> String {
    format!("{:x}", Sha1::digest(value.as_bytes()))
        .chars()
        .take(16)
        .collect()
}

fn utf8_path(path: &Path) -> Result<&str, Error> {
    path.to_str()
        .ok_or_else(|| Error::type_error("State path must contain valid Unicode"))
}

fn slug(value: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in value.to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !result.is_empty() {
                result.push('-');
            }
            separator = false;
            result.push(character);
        } else {
            separator = true;
        }
    }
    result.truncate(result.len().min(32));
    if result.is_empty() {
        "repo".to_owned()
    } else {
        result
    }
}

pub fn repository_id(repo_root: &Path, env: &Value) -> Result<String, Error> {
    let root = paths::resolve_project_root(repo_root)?;
    let remote = env
        .get("GIT_REMOTE_URL")
        .and_then(Value::as_string)
        .filter(|text| !trim(&text.0).is_empty());
    if let Some(remote) = remote {
        let remote = Text(trim(&remote.0).to_vec()).to_utf8()?;
        let name = if remote.to_ascii_lowercase().ends_with(".git") {
            remote
                .get(..remote.len() - 4)
                .ok_or_else(|| Error::type_error("Invalid remote name"))?
        } else {
            &remote
        };
        let name = name
            .rsplit(['\\', '/', ':'])
            .find(|part| !part.is_empty())
            .unwrap_or("repo");
        return Ok(format!(
            "{}-{}",
            slug(name),
            hash_id(&format!("remote:{remote}"))
        ));
    }
    let identity = paths::file_information(&root)?.identity;
    let number = |value| Value::Number(value as f64).json();
    let basename = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    Ok(format!(
        "{}-{}",
        slug(basename),
        hash_id(&format!(
            "checkout:{}:{}",
            number(identity.device),
            number(identity.index)
        ))
    ))
}

pub fn resolve_repository_identity(repo_root: &Path, env: &Value) -> Result<String, Error> {
    let derived = repository_id(repo_root, env)?;
    if let Some(value) = env.get("ROSTER_REPO_ID") {
        let value = value
            .as_string()
            .filter(|text| !trim(&text.0).is_empty())
            .ok_or_else(|| {
                root_error("ROSTER_REPO_ID must be a non-empty repository id when set.")
            })?;
        let value = Text(trim(&value.0).to_vec()).to_utf8()?;
        validate_id(&value)?;
        return Ok(value);
    }
    Ok(derived)
}

pub fn validate_id(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(root_error(
            "State identifier must be an opaque, filesystem-safe identifier.",
        ));
    }
    Ok(())
}

pub fn worktree_id(repo: &Path, worktree: &Path) -> Result<String, Error> {
    Ok(format!(
        "wt-{}",
        hash_id(&format!(
            "{}::{}",
            utf8_path(&paths::absolute(repo)?)?,
            utf8_path(&paths::absolute(worktree)?)?
        ))
    ))
}

fn canonical(path: &Path) -> Result<PathBuf, Error> {
    secure::check_components(path)?;
    match fs::canonicalize(path) {
        Ok(path) => Ok(paths::strip_verbatim(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => paths::absolute(path),
        Err(error) => Err(secure::io_error("Canonicalize state path", path, error)),
    }
}

fn contained(child: &Path, parent: &Path) -> bool {
    let key = |path: &Path| {
        path.components()
            .map(|part| {
                let text = part.as_os_str().to_string_lossy();
                if cfg!(windows) {
                    text.to_lowercase()
                } else {
                    text.into_owned()
                }
            })
            .collect::<Vec<_>>()
    };
    key(child).starts_with(&key(parent))
}

pub fn resolve_repo_root(repo: &Path, machine: &Path, env: &Value) -> Result<Handle, Error> {
    let machine = canonical(machine)?;
    let repo = canonical(repo)?;
    let id = resolve_repository_identity(&repo, env)?;
    let root = machine.join("repos").join(&id);
    if contained(&root, &repo) || contained(&repo, &root) {
        return Err(secure::path_error(
            &root,
            "must stay outside the repository",
        ));
    }
    Ok(Handle {
        scope: Scope::Repo,
        root,
        repo_id: Some(id),
        worktree_id: None,
        run_id: None,
        writable: true,
    })
}

pub fn resolve_worktree_root(
    repo: &Path,
    worktree: &Path,
    machine: &Path,
    env: &Value,
) -> Result<Handle, Error> {
    let mut handle = resolve_repo_root(repo, machine, env)?;
    let id = worktree_id(repo, worktree)?;
    handle.root = handle.root.join("worktrees").join(&id);
    handle.scope = Scope::Worktree;
    handle.worktree_id = Some(id);
    if contained(&handle.root, &canonical(worktree)?)
        || contained(&canonical(worktree)?, &handle.root)
    {
        return Err(secure::path_error(
            &handle.root,
            "must stay outside the worktree",
        ));
    }
    Ok(handle)
}

pub fn resolve_run_root(
    repo: &Path,
    worktree: &Path,
    machine: &Path,
    env: &Value,
    run_id: &str,
) -> Result<Handle, Error> {
    validate_id(run_id)?;
    let mut handle = resolve_worktree_root(repo, worktree, machine, env)?;
    handle.root = handle.root.join("runs").join(run_id);
    handle.scope = Scope::Run;
    handle.run_id = Some(run_id.to_owned());
    Ok(handle)
}
