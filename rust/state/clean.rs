use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::Error;
use crate::ask::generate_uuid_v4;
use crate::cancel::CancellationToken;
use crate::domain::put;
use crate::identity;
use crate::json::{self, Value};
use crate::paths;
use crate::start;
use crate::state::provenance::history::resolve_history_root;
use crate::state::provenance::store::{
    self, LOCK_NAME, LOG_DIRNAME, MAX_STORE_BYTES, SEGMENT_NAME,
};
use crate::state::{files, locks, redaction, roots, secure};
use crate::text::Text;

pub const ESCAPE_CODE: &str = "CLEAN_OPS_PATH_ESCAPE";
pub const FORBIDDEN_CODE: &str = "CLEAN_OPS_FORBIDDEN_PATH";
pub const SCOPE_CODE: &str = "CLEAN_OPS_SCOPE_MISMATCH";
pub const CONFIRMATION_CODE: &str = "CLEAN_OPS_CONFIRMATION_REQUIRED";
pub const LOCK_CODE: &str = "CLEAN_OPS_LOCK_HELD";
pub const PARTIAL_CODE: &str = "CLEAN_OPS_PARTIAL_CLEAN";
pub const LOG_CODE: &str = "CLEAN_OPS_LOG_FAILED";

pub const LOCKS_DIR: &str = "locks";
pub const TRASH_PREFIX: &str = ".roster-clean-trash-";
pub const CLEAN_TARGETS: &[&str] = &["issue", "repo", "machine-history", "curated-memory"];
pub const REPO_PRESERVED: &[&str] = &["identity", "schema.json", "worktrees"];
pub const STATE_SCOPES: &[&str] = &["machine", "repo", "repository", "worktree", "run"];

pub fn clean_error(code: &'static str, message: impl Into<String>) -> Error {
    Error::new("CleanOpsError", message).coded(code)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForbiddenRoot {
    pub label: String,
    pub path: PathBuf,
}

fn canonical_path(path: &Path) -> Result<PathBuf, Error> {
    if path.as_os_str().is_empty() {
        return Err(clean_error(FORBIDDEN_CODE, "a non-empty path is required"));
    }
    secure::check_components(path).map_err(|error| clean_error(ESCAPE_CODE, error.message))?;
    let absolute = paths::absolute(path)?;
    let mut existing = absolute.as_path();
    let mut suffix = Vec::new();
    loop {
        match fs::symlink_metadata(existing) {
            Ok(meta) => {
                if secure::linked(&meta) {
                    return Err(clean_error(
                        ESCAPE_CODE,
                        format!(
                            "refusing to resolve linked path component: {}",
                            existing.display()
                        ),
                    ));
                }
                let mut resolved = fs::canonicalize(existing)
                    .map(paths::strip_verbatim)
                    .map_err(|error| {
                        clean_error(
                            FORBIDDEN_CODE,
                            format!("Cannot canonicalize {}: {error}", existing.display()),
                        )
                    })?;
                for part in suffix.into_iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Ok(absolute);
                };
                suffix.push(name.to_os_string());
                let Some(parent) = existing.parent() else {
                    return Ok(absolute);
                };
                existing = parent;
            }
            Err(error) => {
                return Err(clean_error(
                    FORBIDDEN_CODE,
                    format!("Cannot inspect {}: {error}", existing.display()),
                ));
            }
        }
    }
}

fn filesystem_root(path: &Path) -> PathBuf {
    let mut root = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => root.push(prefix.as_os_str()),
            Component::RootDir => {
                root.push(component.as_os_str());
                break;
            }
            _ => break,
        }
    }
    if root.as_os_str().is_empty() {
        PathBuf::from(std::path::MAIN_SEPARATOR_STR)
    } else {
        root
    }
}

pub fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        let text = path.to_string_lossy();
        let trimmed = text.trim_end_matches(['/', '\\']);
        if cfg!(windows) {
            trimmed.to_lowercase()
        } else {
            trimmed.to_owned()
        }
    };
    normalize(left) == normalize(right)
}

pub fn is_contained_in(candidate: &Path, parent: &Path) -> bool {
    let Ok(candidate_abs) = paths::absolute(candidate) else {
        return false;
    };
    let Ok(parent_abs) = paths::absolute(parent) else {
        return false;
    };
    let parts = |path: &Path| {
        path.components()
            .filter(|component| !matches!(component, Component::CurDir))
            .map(|component| {
                let text = component.as_os_str().to_string_lossy();
                if cfg!(windows) {
                    text.to_lowercase()
                } else {
                    text.into_owned()
                }
            })
            .collect::<Vec<_>>()
    };
    let candidate_parts = parts(&candidate_abs);
    let parent_parts = parts(&parent_abs);
    candidate_parts.len() >= parent_parts.len() && candidate_parts.starts_with(&parent_parts)
}

fn check_state_root_components(root: &Path, scope: &str) -> Result<(), Error> {
    secure::check_components(root).map_err(|error| clean_error(ESCAPE_CODE, error.message))?;
    let abs = paths::absolute(root)?;
    match fs::symlink_metadata(&abs) {
        Ok(meta) => {
            if secure::linked(&meta) {
                return Err(clean_error(
                    ESCAPE_CODE,
                    format!(
                        "{scope} state path component {} is a symlink or reparse point",
                        abs.display()
                    ),
                ));
            }
            if !meta.is_dir() {
                return Err(Error::new(
                    "StateRootError",
                    format!(
                        "{scope} state root {} exists but is not a directory.",
                        abs.display()
                    ),
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(clean_error(FORBIDDEN_CODE, error.to_string())),
    }
}

fn default_home_dir(env: Option<&Value>) -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    if let Some(env) = env {
        if let Some(home) = env
            .get(key)
            .or_else(|| env.get("HOME"))
            .or_else(|| env.get("USERPROFILE"))
            .and_then(Value::as_string)
            .and_then(|text| text.to_utf8().ok())
            .filter(|text| !text.trim().is_empty())
        {
            return Some(PathBuf::from(home));
        }
    }
    std::env::var_os(key)
        .or_else(|| std::env::var_os("HOME"))
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn forbidden_state_roots(
    repo_root: Option<&Path>,
    worktree_root: Option<&Path>,
    env: Option<&Value>,
) -> Result<Vec<ForbiddenRoot>, Error> {
    let mut list = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut push = |label: &str, value: Option<&Path>| -> Result<(), Error> {
        let Some(value) = value.filter(|path| !path.as_os_str().is_empty()) else {
            return Ok(());
        };
        let normalized = canonical_path(value)?;
        list.push(ForbiddenRoot {
            label: label.to_owned(),
            path: normalized,
        });
        Ok(())
    };
    let fs_root = filesystem_root(&paths::absolute(&cwd)?);
    push("root", Some(&fs_root))?;
    let home = default_home_dir(env);
    push("home", home.as_deref())?;
    push("repo", repo_root)?;
    push("worktree", worktree_root)?;

    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for entry in list {
        let key = entry
            .path
            .to_string_lossy()
            .trim_end_matches(['/', '\\'])
            .to_lowercase();
        if seen.insert(key) {
            deduped.push(entry);
        }
    }
    Ok(deduped)
}

fn assert_not_forbidden(root: &Path, forbidden: &[ForbiddenRoot]) -> Result<PathBuf, Error> {
    if root.as_os_str().is_empty() {
        return Err(clean_error(FORBIDDEN_CODE, "a non-empty path is required"));
    }
    let target = canonical_path(root)?;
    let fs_root = filesystem_root(&target);
    if same_path(&target, &fs_root) {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "refusing to use a filesystem root as a state root",
        ));
    }
    for entry in forbidden {
        if same_path(&entry.path, &target) {
            return Err(clean_error(
                FORBIDDEN_CODE,
                format!(
                    "refusing to use protected directory as a state root: {} ({})",
                    entry.label,
                    entry.path.display()
                ),
            ));
        }
    }
    Ok(target)
}

fn directory_state_root(
    root: &Path,
    repo_root: Option<&Path>,
    worktree_root: Option<&Path>,
    env: Option<&Value>,
) -> Result<PathBuf, Error> {
    if root.as_os_str().is_empty() {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "state root handle is missing a root",
        ));
    }
    check_state_root_components(root, "clean")?;
    let forbidden = forbidden_state_roots(repo_root, worktree_root, env)?;
    let target = assert_not_forbidden(root, &forbidden)?;
    for entry in forbidden
        .iter()
        .filter(|entry| matches!(entry.label.as_str(), "repo" | "worktree"))
    {
        if is_contained_in(&target, &entry.path) || is_contained_in(&entry.path, &target) {
            return Err(clean_error(
                FORBIDDEN_CODE,
                "directory cleanup state must not overlap a repository or worktree",
            ));
        }
    }
    Ok(target)
}

pub fn normalize_scope(scope: &str) -> Result<&'static str, Error> {
    if scope.is_empty() {
        return Err(clean_error(SCOPE_CODE, "a state scope is required"));
    }
    if !STATE_SCOPES.contains(&scope) {
        return Err(clean_error(
            SCOPE_CODE,
            format!("unknown state scope \"{scope}\""),
        ));
    }
    Ok(roots::Scope::parse(scope)
        .map_err(|_| clean_error(SCOPE_CODE, format!("unknown state scope \"{scope}\"")))?
        .as_str())
}

#[derive(Debug, Clone)]
pub struct StateHandle {
    pub scope: String,
    pub root: PathBuf,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub run_id: Option<String>,
    pub repo_root: Option<PathBuf>,
    pub worktree_root: Option<PathBuf>,
    pub issue: Option<u64>,
}

pub fn assert_state_scope(handle: &StateHandle, expected: Option<&str>) -> Result<String, Error> {
    if handle.scope.is_empty() {
        return Err(clean_error(
            SCOPE_CODE,
            "state root handle is missing a scope",
        ));
    }
    if let Some(expected) = expected {
        let wanted = normalize_scope(expected)?;
        if handle.scope != wanted {
            return Err(clean_error(
                SCOPE_CODE,
                format!(
                    "state root scope mismatch: expected \"{wanted}\" but got \"{}\"",
                    handle.scope
                ),
            ));
        }
    }
    Ok(handle.scope.clone())
}

pub async fn resolve_repository_id(
    repo_root: &Path,
    token: Option<&CancellationToken>,
) -> Result<String, Error> {
    if repo_root.as_os_str().is_empty() {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "a repository root is required to resolve repository identity",
        ));
    }
    let id = identity::resolve_repo_identity(repo_root, token).await?;
    if id.is_empty() {
        return Err(clean_error(
            FORBIDDEN_CODE,
            format!(
                "repository identity could not be resolved for {}",
                repo_root.display()
            ),
        ));
    }
    Ok(id)
}

#[derive(Default)]
pub struct ResolveStateRootOptions<'a> {
    pub scope: Option<&'a str>,
    pub state_root: Option<&'a Path>,
    pub worktree_root: Option<&'a Path>,
    pub machine_root: Option<&'a Path>,
    pub env: Option<&'a Value>,
}

fn current_platform_name() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

pub async fn resolve_state_root(
    repo_root: Option<&Path>,
    options: &ResolveStateRootOptions<'_>,
    token: Option<&CancellationToken>,
) -> Result<StateHandle, Error> {
    let scope = normalize_scope(options.scope.unwrap_or("repo"))?;
    let forbidden = forbidden_state_roots(repo_root, options.worktree_root, options.env)?;
    let repo_id = match repo_root.filter(|path| !path.as_os_str().is_empty()) {
        Some(root) => Some(resolve_repository_id(root, token).await?),
        None => None,
    };
    if let Some(override_root) = options.state_root {
        let safe =
            directory_state_root(override_root, repo_root, options.worktree_root, options.env)?;
        files::create_private_directories(&safe)
            .map_err(|error| clean_error(FORBIDDEN_CODE, error.message))?;
        return Ok(StateHandle {
            scope: scope.to_owned(),
            root: safe,
            repo_id,
            worktree_id: None,
            run_id: None,
            repo_root: repo_root.map(Path::to_path_buf),
            worktree_root: options.worktree_root.map(Path::to_path_buf),
            issue: None,
        });
    }
    let Some(repo_root) = repo_root.filter(|path| !path.as_os_str().is_empty()) else {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "a repository root is required to resolve a state root",
        ));
    };
    let Some(repo_id) = repo_id else {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "a repository root is required to resolve a state root",
        ));
    };
    let mut env_value = options
        .env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    put(&mut env_value, "ROSTER_REPO_ID", Value::string(&repo_id))?;
    let home = default_home_dir(Some(&env_value));
    let machine = match options.machine_root {
        Some(machine) => canonical_path(machine)?,
        None => {
            roots::resolve_machine_root(&env_value, current_platform_name(), home.as_deref())?.root
        }
    };
    let handle = roots::resolve_repo_root(repo_root, &machine, &env_value)?;
    assert_not_forbidden(&handle.root, &forbidden)?;
    let state_handle = StateHandle {
        scope: handle.scope.as_str().to_owned(),
        root: handle.root,
        repo_id: handle.repo_id.or(Some(repo_id)),
        worktree_id: handle.worktree_id,
        run_id: handle.run_id,
        repo_root: Some(repo_root.to_path_buf()),
        worktree_root: options.worktree_root.map(Path::to_path_buf),
        issue: None,
    };
    assert_state_scope(&state_handle, Some(scope))?;
    Ok(state_handle)
}

pub fn resolve_state_path(handle: &StateHandle, relative_path: &str) -> Result<PathBuf, Error> {
    assert_state_scope(handle, None)?;
    if handle.root.as_os_str().is_empty() {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "state root handle is missing a root",
        ));
    }
    check_state_root_components(&handle.root, &handle.scope)?;
    let canonical_root = canonical_path(&handle.root)?;
    let candidate = paths::resolve(&canonical_root, Path::new(relative_path))?;
    if !is_contained_in(&candidate, &canonical_root) {
        return Err(clean_error(
            ESCAPE_CODE,
            format!("refusing to resolve path outside the state root: {relative_path}"),
        ));
    }
    let mut forbidden = forbidden_state_roots(
        handle.repo_root.as_deref(),
        handle.worktree_root.as_deref(),
        None,
    )?;
    forbidden.push(ForbiddenRoot {
        label: "state-root".to_owned(),
        path: canonical_root.clone(),
    });
    assert_not_forbidden(&candidate, &forbidden)?;
    secure::check_components(&candidate)
        .map_err(|error| clean_error(ESCAPE_CODE, error.message))?;
    let canonical = canonical_path(&candidate)?;
    if !is_contained_in(&canonical, &canonical_root) {
        return Err(clean_error(
            ESCAPE_CODE,
            format!("refusing to resolve path outside the state root: {relative_path}"),
        ));
    }
    Ok(canonical)
}

pub fn assert_safe_state_path(handle: &StateHandle, target_path: &str) -> Result<PathBuf, Error> {
    assert_state_scope(handle, None)?;
    if handle.root.as_os_str().is_empty() {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "state root handle is missing a root",
        ));
    }
    check_state_root_components(&handle.root, &handle.scope)?;
    let canonical_root = canonical_path(&handle.root)?;
    let candidate = paths::resolve(&canonical_root, Path::new(target_path))?;
    if !is_contained_in(&candidate, &canonical_root) {
        return Err(clean_error(
            ESCAPE_CODE,
            format!("refusing to resolve path outside the state root: {target_path}"),
        ));
    }
    if let Ok(stat) = fs::symlink_metadata(&candidate) {
        if secure::linked(&stat) {
            return Err(clean_error(
                ESCAPE_CODE,
                format!(
                    "refusing to operate on a symlink or reparse point inside the state root: {}",
                    candidate.display()
                ),
            ));
        }
    }
    resolve_state_path(handle, target_path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutcome {
    pub removed: bool,
    pub target: PathBuf,
    pub dry_run: bool,
}

pub fn remove_state_path(
    handle: &StateHandle,
    relative_path: &str,
    expected_scope: Option<&str>,
    dry_run: bool,
) -> Result<RemoveOutcome, Error> {
    assert_state_scope(handle, expected_scope)?;
    if handle.root.as_os_str().is_empty() {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "state root handle is missing a root",
        ));
    }
    let mut forbidden = forbidden_state_roots(
        handle.repo_root.as_deref(),
        handle.worktree_root.as_deref(),
        None,
    )?;
    let canonical_root = canonical_path(&handle.root)?;
    forbidden.push(ForbiddenRoot {
        label: "state-root".to_owned(),
        path: canonical_root.clone(),
    });
    let target = assert_safe_state_path(handle, relative_path)?;
    let normalized_target = assert_not_forbidden(&target, &forbidden)?;
    if !is_contained_in(&normalized_target, &canonical_root) {
        return Err(clean_error(
            ESCAPE_CODE,
            format!("refusing to resolve path outside the state root: {relative_path}"),
        ));
    }
    if dry_run {
        return Ok(RemoveOutcome {
            removed: false,
            target: normalized_target,
            dry_run: true,
        });
    }
    match fs::symlink_metadata(&normalized_target) {
        Ok(meta) if meta.is_dir() && !secure::linked(&meta) => {
            fs::remove_dir_all(&normalized_target)
                .map_err(|error| clean_error(PARTIAL_CODE, error.to_string()))?;
        }
        Ok(_) => {
            fs::remove_file(&normalized_target)
                .map_err(|error| clean_error(PARTIAL_CODE, error.to_string()))?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(clean_error(PARTIAL_CODE, error.to_string())),
    }
    Ok(RemoveOutcome {
        removed: true,
        target: normalized_target,
        dry_run: false,
    })
}

pub fn ensure_state_directory(handle: &StateHandle) -> Result<PathBuf, Error> {
    assert_state_scope(handle, None)?;
    let safe = directory_state_root(
        &handle.root,
        handle.repo_root.as_deref(),
        handle.worktree_root.as_deref(),
        None,
    )?;
    files::create_private_directories(&safe)
        .map_err(|error| clean_error(FORBIDDEN_CODE, error.message))?;
    Ok(safe)
}

fn canonical_state_root(handle: &StateHandle) -> Result<PathBuf, Error> {
    assert_state_scope(handle, None)?;
    directory_state_root(
        &handle.root,
        handle.repo_root.as_deref(),
        handle.worktree_root.as_deref(),
        None,
    )
}

fn validate_entry_names(names: &[String], label: &str) -> Result<Vec<String>, Error> {
    for name in names {
        if name.is_empty() || name.contains(['/', '\\']) || matches!(name.as_str(), "." | "..") {
            return Err(clean_error(
                SCOPE_CODE,
                format!("{label} must be a list of entry names"),
            ));
        }
    }
    Ok(names.to_vec())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanEntry {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockInfo {
    pub name: String,
    pub holder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedEntry {
    pub name: String,
    pub code: String,
}

fn enumerate_records(
    handle: &StateHandle,
    relative_dir: &str,
    exclude: &[String],
) -> Result<(PathBuf, PathBuf, Vec<CleanEntry>, Vec<String>), Error> {
    let root = canonical_state_root(handle)?;
    let dir = if !relative_dir.is_empty() && paths::resolve(&root, Path::new(relative_dir))? != root
    {
        resolve_state_path(handle, relative_dir)?
    } else {
        root.clone()
    };
    let at_root = dir == root;
    let read_dir = match fs::read_dir(&dir) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((root, dir, Vec::new(), Vec::new()));
        }
        Err(error) => return Err(clean_error(FORBIDDEN_CODE, error.to_string())),
    };
    let mut raw_entries = Vec::new();
    for item in read_dir {
        let item = item.map_err(|error| clean_error(FORBIDDEN_CODE, error.to_string()))?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| clean_error(FORBIDDEN_CODE, "State entry name must be UTF-8"))?;
        let is_dir = item.file_type().is_ok_and(|ft| ft.is_dir());
        raw_entries.push((name, is_dir));
    }
    raw_entries.sort_by(|a, b| a.0.cmp(&b.0));
    let skip = exclude.iter().cloned().collect::<BTreeSet<_>>();
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    for (name, is_dir) in raw_entries {
        if skip.contains(&name) || (at_root && name == LOCKS_DIR) {
            excluded.push(name);
        } else {
            included.push(CleanEntry {
                name,
                kind: if is_dir {
                    "directory".to_owned()
                } else {
                    "file".to_owned()
                },
            });
        }
    }
    Ok((root, dir, included, excluded))
}

pub fn list_run_locks(handle: &StateHandle) -> Result<Vec<LockInfo>, Error> {
    let dir = canonical_state_root(handle)?.join(LOCKS_DIR);
    secure::check_components(&dir).map_err(|error| clean_error(ESCAPE_CODE, error.message))?;
    let read_dir = match fs::read_dir(&dir) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(clean_error(FORBIDDEN_CODE, error.to_string())),
    };
    let mut found = Vec::new();
    for item in read_dir {
        let item = item.map_err(|error| clean_error(FORBIDDEN_CODE, error.to_string()))?;
        let item_path = dir.join(item.file_name());
        let Ok(meta) = fs::symlink_metadata(&item_path) else {
            continue;
        };
        if !meta.is_file() || secure::linked(&meta) {
            continue;
        }
        let Ok(file_name) = item.file_name().into_string() else {
            continue;
        };
        let Some(name) = file_name.strip_suffix(".lock") else {
            continue;
        };
        let holder = fs::read_to_string(&item_path)
            .ok()
            .and_then(|source| json::parse(&Text::from(source.as_str())).ok())
            .and_then(|record| {
                record
                    .get("holder")
                    .and_then(Value::as_string)
                    .and_then(|text| text.to_utf8().ok())
            });
        found.push(LockInfo {
            name: name.to_owned(),
            holder,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

pub async fn check_active_run_lock(
    handle: &StateHandle,
    token: Option<&CancellationToken>,
) -> Result<Vec<LockInfo>, Error> {
    let lock_root = canonical_state_root(handle)?;
    let mut held = Vec::new();
    for info in list_run_locks(handle)? {
        let target = locks::target(&info.name, &lock_root, &lock_root, Some(&lock_root))?;
        let options = locks::Options {
            holder: format!("clean-probe-{}", std::process::id()),
            wait: Duration::ZERO,
            ..locks::Options::default()
        };
        match locks::acquire(&info.name, &target, &options, token).await {
            Ok(mut lock) => {
                lock.release_required().await?;
            }
            Err(error) if error.code == Some("E_LOCK_HELD") => {
                held.push(info);
            }
            Err(error) => return Err(error),
        }
    }
    if !held.is_empty() {
        let names = held
            .iter()
            .map(|item| format!("\"{}\"", item.name))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(clean_error(
            LOCK_CODE,
            format!("an active run holds {names}; refusing to clean until it finishes"),
        ));
    }
    Ok(Vec::new())
}

#[derive(Debug, Clone, Default)]
pub struct CleanRunOptions {
    pub execute: bool,
    pub yes: bool,
    pub interactive: bool,
    pub relative_dir: String,
    pub exclude: Vec<String>,
    pub env: Option<Value>,
}

pub type CleanEventSink<'a> = dyn FnMut(Value) -> Result<(), Error> + 'a;

fn emit_clean_event(
    sink: &mut Option<&mut CleanEventSink<'_>>,
    scope: &str,
    entry: &str,
    outcome: &str,
    code: Option<&str>,
    env: Option<&Value>,
) -> Result<(), Error> {
    let Some(callback) = sink.as_deref_mut() else {
        return Ok(());
    };
    let mut fields = vec![
        ("type", Value::string("clean")),
        ("scope", Value::string(scope)),
        ("entry", Value::string(entry)),
        ("outcome", Value::string(outcome)),
    ];
    if let Some(code) = code {
        fields.push(("code", Value::string(code)));
    }
    let record = Value::object(fields);
    let config = redaction::Config {
        environment: env
            .cloned()
            .unwrap_or_else(crate::ask::environment_value_from_process),
        ..redaction::Config::default()
    };
    config.validate()?;
    let redacted = redaction::record(&record, &config)?;
    callback(redacted).map_err(|cause| {
        clean_error(
            LOG_CODE,
            "cleanup logging failed after an operation; inspect state before retrying",
        )
        .caused_by(cause)
    })
}

#[derive(Debug, Clone)]
pub struct CleanReport {
    pub dry_run: bool,
    pub scope: String,
    pub root: PathBuf,
    pub included: Vec<CleanEntry>,
    pub excluded: Vec<String>,
    pub record_count: usize,
    pub active_locks: Vec<LockInfo>,
    pub executed: bool,
    pub removed: Vec<RemoveOutcome>,
    pub failed: Vec<FailedEntry>,
}

fn remove_atomically(
    handle: &StateHandle,
    root: &Path,
    relative: &str,
) -> Result<RemoveOutcome, Error> {
    let dry = remove_state_path(handle, relative, None, true)?;
    let target = dry.target;
    if target
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(TRASH_PREFIX))
    {
        return remove_state_path(handle, relative, None, false);
    }
    let parent = target
        .parent()
        .ok_or_else(|| clean_error(ESCAPE_CODE, "target has no parent directory"))?;
    let trash_name = format!("{TRASH_PREFIX}{}", generate_uuid_v4());
    let trash = parent.join(&trash_name);
    match fs::rename(&target, &trash) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RemoveOutcome {
                removed: false,
                target,
                dry_run: false,
            });
        }
        Err(error) => return Err(clean_error(PARTIAL_CODE, error.to_string())),
    }
    let rel_trash = trash
        .strip_prefix(root)
        .map_err(|_| clean_error(ESCAPE_CODE, "trash path escaped state root"))?
        .to_string_lossy()
        .into_owned();
    remove_state_path(handle, &rel_trash, None, false)?;
    Ok(RemoveOutcome {
        removed: true,
        target,
        dry_run: false,
    })
}

pub async fn run_clean(
    handle: &StateHandle,
    options: &CleanRunOptions,
    token: Option<&CancellationToken>,
) -> Result<CleanReport, Error> {
    run_clean_with_events(handle, options, None, token).await
}

pub async fn run_clean_with_events(
    handle: &StateHandle,
    options: &CleanRunOptions,
    mut on_event: Option<&mut CleanEventSink<'_>>,
    token: Option<&CancellationToken>,
) -> Result<CleanReport, Error> {
    assert_state_scope(handle, None)?;
    let exclude = validate_entry_names(&options.exclude, "exclude")?;
    if options.execute && !options.yes {
        return Err(clean_error(
            CONFIRMATION_CODE,
            if options.interactive {
                "destructive clean requires explicit confirmation (--yes)"
            } else {
                "non-interactive destructive clean requires explicit --yes confirmation"
            },
        ));
    }
    let (root, dir, included, excluded) =
        enumerate_records(handle, &options.relative_dir, &exclude)?;
    let active_locks = list_run_locks(handle)?;
    let record_count = included.len();
    if !options.execute {
        return Ok(CleanReport {
            dry_run: true,
            scope: handle.scope.clone(),
            root,
            included,
            excluded,
            record_count,
            active_locks,
            executed: false,
            removed: Vec::new(),
            failed: Vec::new(),
        });
    }
    check_active_run_lock(handle, token).await?;
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for entry in &included {
        let rel = dir
            .join(&entry.name)
            .strip_prefix(&root)
            .map_err(|_| clean_error(ESCAPE_CODE, "entry path escaped state root"))?
            .to_string_lossy()
            .into_owned();
        let failure_code = match remove_atomically(handle, &root, &rel) {
            Ok(outcome) => {
                removed.push(outcome);
                None
            }
            Err(error) => {
                let code = error.code.unwrap_or("UNKNOWN").to_owned();
                failed.push(FailedEntry {
                    name: entry.name.clone(),
                    code: code.clone(),
                });
                Some(code)
            }
        };
        emit_clean_event(
            &mut on_event,
            &handle.scope,
            &entry.name,
            if failure_code.is_some() {
                "failed"
            } else {
                "removed"
            },
            failure_code.as_deref(),
            options.env.as_ref(),
        )?;
    }
    if !failed.is_empty() {
        let detail = failed
            .iter()
            .map(|item| format!("{}: {}", item.name, item.code))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(clean_error(
            PARTIAL_CODE,
            format!(
                "clean removed {} and failed {} of {} entries ({detail}); rerun to retry",
                removed.len(),
                failed.len(),
                included.len()
            ),
        ));
    }
    Ok(CleanReport {
        dry_run: false,
        scope: handle.scope.clone(),
        root,
        included,
        excluded,
        record_count,
        active_locks: Vec::new(),
        executed: true,
        removed,
        failed,
    })
}

pub fn format_clean_report(report: &CleanReport) -> String {
    let mut lines = vec![
        format!(
            "mode: {}",
            if report.dry_run {
                "dry-run (nothing deleted; rerun with --execute --yes to delete)"
            } else {
                "destructive"
            }
        ),
        format!("scope: {}", report.scope),
        format!("state root: {}", report.root.display()),
        format!("records: {}", report.record_count),
    ];
    for item in &report.included {
        lines.push(format!("  - {} {}", item.kind, item.name));
    }
    if !report.excluded.is_empty() {
        lines.push(format!("excluded: {}", report.excluded.join(", ")));
    }
    if !report.active_locks.is_empty() {
        let formatted = report
            .active_locks
            .iter()
            .map(|lock| match &lock.holder {
                Some(holder) => format!("{} ({holder})", lock.name),
                None => lock.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("active locks: {formatted}"));
    }
    if report.executed {
        lines.push(format!("removed: {}", report.removed.len()));
    }
    format!("{}\n", lines.join("\n"))
}

pub fn assert_target(target: &str) -> Result<&'static str, Error> {
    match CLEAN_TARGETS.iter().copied().find(|known| *known == target) {
        Some(found) => Ok(found),
        None => Err(clean_error(
            SCOPE_CODE,
            format!(
                "unknown clean target \"{target}\"; use one of {}",
                CLEAN_TARGETS.join(", ")
            ),
        )),
    }
}

#[derive(Debug, Clone)]
pub struct CleanTargetPlan {
    pub target: String,
    pub handle: StateHandle,
    pub exclude: Vec<String>,
}

#[derive(Default)]
pub struct ResolveCleanTargetOptions<'a> {
    pub repo_root: Option<&'a Path>,
    pub issue: Option<u64>,
    pub worktrees_dir: Option<&'a str>,
    pub machine_root: Option<&'a Path>,
    pub env: Option<&'a Value>,
}

pub async fn resolve_clean_target(
    target: &str,
    options: &ResolveCleanTargetOptions<'_>,
    token: Option<&CancellationToken>,
) -> Result<CleanTargetPlan, Error> {
    let target = assert_target(target)?;
    if matches!(target, "machine-history" | "curated-memory") {
        return Err(clean_error(
            SCOPE_CODE,
            format!("\"{target}\" prunes provenance records; use pruneProvenance"),
        ));
    }
    if target == "repo" {
        let handle = resolve_state_root(
            options.repo_root,
            &ResolveStateRootOptions {
                scope: Some("repo"),
                state_root: None,
                worktree_root: None,
                machine_root: options.machine_root,
                env: options.env,
            },
            token,
        )
        .await?;
        return Ok(CleanTargetPlan {
            target: target.to_owned(),
            handle,
            exclude: REPO_PRESERVED
                .iter()
                .map(|item| (*item).to_owned())
                .collect(),
        });
    }
    let Some(issue) = options.issue.filter(|issue| *issue >= 1) else {
        return Err(clean_error(
            SCOPE_CODE,
            "the issue target requires a positive issue number",
        ));
    };
    let Some(repo_root) = options
        .repo_root
        .filter(|path| !path.as_os_str().is_empty())
    else {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "a repository root is required to resolve a state root",
        ));
    };
    let worktree_root = repo_root
        .join(options.worktrees_dir.unwrap_or(".worktrees"))
        .join(format!("issue-{issue}"));
    let repo_id = resolve_repository_id(repo_root, token).await?;
    let mut env_value = options
        .env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    put(&mut env_value, "ROSTER_REPO_ID", Value::string(&repo_id))?;
    let home = default_home_dir(Some(&env_value));
    let machine = match options.machine_root {
        Some(machine) => canonical_path(machine)?,
        None => {
            roots::resolve_machine_root(&env_value, current_platform_name(), home.as_deref())?.root
        }
    };
    let resolved = roots::resolve_worktree_root(repo_root, &worktree_root, &machine, &env_value)?;
    let forbidden = forbidden_state_roots(Some(repo_root), Some(&worktree_root), Some(&env_value))?;
    assert_not_forbidden(&resolved.root, &forbidden)?;
    let handle = StateHandle {
        scope: resolved.scope.as_str().to_owned(),
        root: resolved.root,
        repo_id: resolved.repo_id,
        worktree_id: resolved.worktree_id,
        run_id: resolved.run_id,
        repo_root: Some(repo_root.to_path_buf()),
        worktree_root: Some(worktree_root),
        issue: Some(issue),
    };
    assert_state_scope(&handle, Some("worktree"))?;
    Ok(CleanTargetPlan {
        target: target.to_owned(),
        handle,
        exclude: Vec::new(),
    })
}

#[derive(Debug, Clone, Default)]
pub struct PruneProvenanceOptions {
    pub target: String,
    pub execute: bool,
    pub yes: bool,
    pub interactive: bool,
    pub repo_root: Option<PathBuf>,
    pub worktree_root: Option<PathBuf>,
    pub repo_identity: Option<String>,
    pub before_ms: Option<f64>,
    pub env: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ProvenanceCleanReport {
    pub target: String,
    pub scope: String,
    pub root: PathBuf,
    pub dry_run: bool,
    pub record_count: usize,
    pub records: Vec<String>,
    pub kept: usize,
    pub skipped: usize,
    pub executed: bool,
    pub removed: Vec<String>,
    pub failed: Vec<FailedEntry>,
}

fn matches_provenance_section(target: &str, section: Option<&str>) -> bool {
    match target {
        "machine-history" => matches!(section, None | Some("raw-history" | "compaction")),
        "curated-memory" => section == Some("curated-memory"),
        _ => false,
    }
}

fn sync_segment(handle: &StateHandle, kept_records: &[Value]) -> Result<(), Error> {
    let target = assert_safe_state_path(handle, SEGMENT_NAME)?;
    let Some(snapshot) = secure::read(&target, MAX_STORE_BYTES)? else {
        return Ok(());
    };
    let current = std::str::from_utf8(&snapshot.bytes)
        .map_err(|error| clean_error(PARTIAL_CODE, error.to_string()))?;
    let keep_ids = kept_records
        .iter()
        .filter_map(|record| {
            record
                .get("id")
                .and_then(Value::as_string)
                .and_then(|id| id.to_utf8().ok())
        })
        .collect::<BTreeSet<_>>();
    let all_kept = current.lines().filter(|line| !line.is_empty()).all(|line| {
        json::parse(&Text::from(line))
            .ok()
            .and_then(|record| {
                record
                    .get("id")
                    .and_then(Value::as_string)
                    .and_then(|id| id.to_utf8().ok())
            })
            .is_some_and(|id| keep_ids.contains(&id))
    });
    if all_kept {
        return Ok(());
    }
    let tmp = handle
        .root
        .join(format!("{SEGMENT_NAME}.{}.tmp", files::nonce()?));
    let mut body = String::new();
    for record in kept_records {
        body.push_str(&record.json());
        body.push('\n');
    }
    let write_result = (|| -> Result<(), Error> {
        let mut open = OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut handle_file = open
            .open(&tmp)
            .map_err(|error| clean_error(PARTIAL_CODE, error.to_string()))?;
        handle_file
            .write_all(body.as_bytes())
            .and_then(|()| handle_file.sync_all())
            .map_err(|error| clean_error(PARTIAL_CODE, error.to_string()))?;
        drop(handle_file);
        fs::rename(&tmp, &target).map_err(|error| clean_error(PARTIAL_CODE, error.to_string()))?;
        secure::sync_directory(&handle.root)
            .map_err(|error| clean_error(PARTIAL_CODE, error.message))?;
        Ok(())
    })();
    let _ = fs::remove_file(&tmp);
    write_result
}

pub async fn prune_provenance(
    store_root: &Path,
    options: &PruneProvenanceOptions,
    token: Option<&CancellationToken>,
) -> Result<ProvenanceCleanReport, Error> {
    prune_provenance_with_events(store_root, options, None, token).await
}

pub async fn prune_provenance_with_events(
    store_root: &Path,
    options: &PruneProvenanceOptions,
    mut on_event: Option<&mut CleanEventSink<'_>>,
    token: Option<&CancellationToken>,
) -> Result<ProvenanceCleanReport, Error> {
    let target = assert_target(&options.target)?;
    if !matches!(target, "machine-history" | "curated-memory") {
        return Err(clean_error(
            SCOPE_CODE,
            format!("\"{target}\" is a directory target; use resolveCleanTarget"),
        ));
    }
    if options.execute && !options.yes {
        return Err(clean_error(
            CONFIRMATION_CODE,
            if options.interactive {
                "destructive clean requires explicit confirmation (--yes)"
            } else {
                "non-interactive destructive clean requires explicit --yes confirmation"
            },
        ));
    }
    if store_root.as_os_str().is_empty() || !store_root.is_absolute() {
        return Err(clean_error(
            FORBIDDEN_CODE,
            "the provenance store root must be an absolute path",
        ));
    }
    check_state_root_components(store_root, "machine")?;
    let forbidden = forbidden_state_roots(
        options.repo_root.as_deref(),
        options.worktree_root.as_deref(),
        options.env.as_ref(),
    )?;
    let root = assert_not_forbidden(store_root, &forbidden)?;
    if let Some(cutoff) = options.before_ms {
        if !cutoff.is_finite() {
            return Err(clean_error(
                SCOPE_CODE,
                "before must be an epoch millisecond value or an ISO timestamp",
            ));
        }
    }
    if let Some(repo_identity) = &options.repo_identity {
        if repo_identity.is_empty() {
            return Err(clean_error(
                SCOPE_CODE,
                "repoIdentity must be a non-empty string",
            ));
        }
    }
    let handle = StateHandle {
        scope: "machine".to_owned(),
        root: root.clone(),
        repo_id: None,
        worktree_id: None,
        run_id: None,
        repo_root: options.repo_root.clone(),
        worktree_root: options.worktree_root.clone(),
        issue: None,
    };
    let matches_record = |record: &Value| -> bool {
        let section = record
            .get("section")
            .and_then(Value::as_string)
            .and_then(|text| text.to_utf8().ok());
        if !matches_provenance_section(target, section.as_deref()) {
            return false;
        }
        if let Some(expected_repo) = &options.repo_identity {
            let actual_repo = record
                .get("repoIdentity")
                .and_then(Value::as_string)
                .and_then(|text| text.to_utf8().ok());
            if actual_repo.as_deref() != Some(expected_repo.as_str()) {
                return false;
            }
        }
        if let Some(cutoff) = options.before_ms {
            let Some(created_at) = record.get("createdAt").and_then(Value::as_number) else {
                return false;
            };
            if created_at.partial_cmp(&cutoff) != Some(std::cmp::Ordering::Less) {
                return false;
            }
        }
        true
    };
    let scan = || -> Result<(Vec<Value>, Vec<String>, usize, usize), Error> {
        let opened = store::Store::open(&root)?;
        let loaded = opened.read_all()?;
        let mut pruned_ids = Vec::new();
        for record in &loaded.records {
            if matches_record(record) {
                if let Some(id) = record
                    .get("id")
                    .and_then(Value::as_string)
                    .and_then(|id| id.to_utf8().ok())
                {
                    pruned_ids.push(id);
                }
            }
        }
        let kept = loaded.records.len().saturating_sub(pruned_ids.len());
        pruned_ids.sort();
        Ok((loaded.records, pruned_ids, kept, loaded.skipped.len()))
    };
    if !options.execute {
        let (_, records, kept, skipped) = scan()?;
        return Ok(ProvenanceCleanReport {
            target: target.to_owned(),
            scope: "machine".to_owned(),
            root,
            dry_run: true,
            record_count: records.len(),
            records,
            kept,
            skipped,
            executed: false,
            removed: Vec::new(),
            failed: Vec::new(),
        });
    }

    secure::private_directory_if_present(&root)?;
    let lock_target = locks::target(LOCK_NAME, &root, &root, Some(&root))?;
    let mut lock = match locks::acquire(LOCK_NAME, &lock_target, &locks::Options::default(), token)
        .await
    {
        Ok(lock) => lock,
        Err(error) if error.code == Some("E_LOCK_HELD") => {
            return Err(clean_error(
                LOCK_CODE,
                "the provenance store is locked by an active writer; refusing to prune until it finishes",
            ));
        }
        Err(error) => return Err(error),
    };

    let result = (|| -> Result<ProvenanceCleanReport, Error> {
        let (all_records, pruned_ids, kept, skipped) = scan()?;
        let mut removed = Vec::new();
        let mut failed = Vec::new();
        for id in &pruned_ids {
            let rel = format!("{LOG_DIRNAME}/{id}.json");
            let failure_code = match remove_state_path(&handle, &rel, Some("machine"), false) {
                Ok(_) => {
                    removed.push(id.clone());
                    None
                }
                Err(error) => {
                    let code = error.code.unwrap_or("UNKNOWN").to_owned();
                    failed.push(FailedEntry {
                        name: id.clone(),
                        code: code.clone(),
                    });
                    Some(code)
                }
            };
            emit_clean_event(
                &mut on_event,
                "machine",
                id,
                if failure_code.is_some() {
                    "failed"
                } else {
                    "removed"
                },
                failure_code.as_deref(),
                options.env.as_ref(),
            )?;
        }
        removed.sort();
        let removed_set = removed.iter().cloned().collect::<BTreeSet<_>>();
        let remaining = all_records
            .into_iter()
            .filter(|record| {
                record
                    .get("id")
                    .and_then(Value::as_string)
                    .and_then(|id| id.to_utf8().ok())
                    .is_none_or(|id| !removed_set.contains(&id))
            })
            .collect::<Vec<_>>();
        sync_segment(&handle, &remaining).map_err(|cause| {
            clean_error(
                PARTIAL_CODE,
                "provenance records were processed but index synchronization failed; rerun to retry",
            )
            .caused_by(cause)
        })?;
        if !failed.is_empty() {
            return Err(clean_error(
                PARTIAL_CODE,
                format!(
                    "prune removed {} and failed {} of {} records; rerun to retry",
                    removed.len(),
                    failed.len(),
                    pruned_ids.len()
                ),
            ));
        }
        Ok(ProvenanceCleanReport {
            target: target.to_owned(),
            scope: "machine".to_owned(),
            root: root.clone(),
            dry_run: false,
            record_count: pruned_ids.len(),
            records: pruned_ids,
            kept,
            skipped,
            executed: true,
            removed,
            failed,
        })
    })();
    let cleanup = lock.release_required().await;
    locks::finish(result, cleanup)
}

pub fn format_provenance_report(report: &ProvenanceCleanReport) -> String {
    let mut lines = vec![
        format!(
            "mode: {}",
            if report.dry_run {
                "dry-run (nothing deleted; rerun with --execute --yes to delete)"
            } else {
                "destructive"
            }
        ),
        format!("target: {}", report.target),
        format!("store root: {}", report.root.display()),
        format!("records: {}", report.record_count),
    ];
    for id in &report.records {
        lines.push(format!("  - record {id}"));
    }
    lines.push(format!("kept: {}", report.kept));
    if report.skipped > 0 {
        lines.push(format!(
            "skipped malformed: {} (run history repair)",
            report.skipped
        ));
    }
    if report.executed {
        lines.push(format!("removed: {}", report.removed.len()));
    }
    format!("{}\n", lines.join("\n"))
}

#[derive(Debug, Clone, Default)]
pub struct CliOptions {
    pub target: Option<String>,
    pub issue: Option<u64>,
    pub store_path: Option<PathBuf>,
    pub scope: Option<String>,
    pub state_root: Option<PathBuf>,
    pub exclude: Vec<String>,
    pub execute: bool,
    pub yes: bool,
}

impl CliOptions {
    pub fn parse(args: &[String]) -> Result<Self, Error> {
        let usage = || {
            Error::type_error(
                "Use roster clean [--target issue|repo|machine-history|curated-memory] [--issue N] [--store DIR] [--scope SCOPE] [--state-root PATH] [--exclude NAME]... [--execute --yes].",
            )
        };
        let mut options = Self::default();
        let mut seen = BTreeSet::new();
        let mut index = 0;
        while let Some(flag) = args.get(index) {
            if flag != "--exclude" && !seen.insert(flag.clone()) {
                return Err(usage());
            }
            match flag.as_str() {
                "--execute" => options.execute = true,
                "--yes" => options.yes = true,
                "--scope" | "--state-root" | "--exclude" | "--target" | "--issue" | "--store" => {
                    index += 1;
                    let value = args
                        .get(index)
                        .filter(|value| !value.is_empty() && !value.starts_with("--"))
                        .ok_or_else(usage)?;
                    match flag.as_str() {
                        "--scope" => options.scope = Some(value.clone()),
                        "--state-root" => options.state_root = Some(PathBuf::from(value)),
                        "--exclude" => options.exclude.push(value.clone()),
                        "--target" => options.target = Some(value.clone()),
                        "--issue" => {
                            options.issue =
                                Some(start::parse_issue_number(value).map_err(|_| {
                                    clean_error(
                                        SCOPE_CODE,
                                        "the issue target requires a positive issue number",
                                    )
                                })?);
                        }
                        "--store" => options.store_path = Some(PathBuf::from(value)),
                        _ => return Err(usage()),
                    }
                }
                _ => return Err(usage()),
            }
            index += 1;
        }
        let provenance = matches!(
            options.target.as_deref(),
            Some("machine-history" | "curated-memory")
        );
        if (options.target.is_some() && (options.scope.is_some() || options.state_root.is_some()))
            || (options.issue.is_some() && options.target.as_deref() != Some("issue"))
            || (options.store_path.is_some() && !provenance)
            || (provenance && !options.exclude.is_empty())
        {
            return Err(usage());
        }
        Ok(options)
    }
}

pub async fn run_cli(
    options: &CliOptions,
    cwd: &Path,
    environment: &Value,
    interactive: bool,
    token: Option<&CancellationToken>,
) -> Result<String, Error> {
    let project_root = if options.state_root.is_none() {
        Some(paths::resolve_project_root(cwd)?)
    } else {
        None
    };
    if matches!(
        options.target.as_deref(),
        Some("machine-history" | "curated-memory")
    ) {
        let store_root = resolve_history_root(
            project_root.as_deref().unwrap_or(cwd),
            options.store_path.as_deref(),
        )?;
        let report = prune_provenance(
            &store_root,
            &PruneProvenanceOptions {
                target: options.target.clone().unwrap_or_default(),
                execute: options.execute,
                yes: options.yes,
                interactive,
                repo_root: project_root,
                worktree_root: None,
                repo_identity: None,
                before_ms: None,
                env: Some(environment.clone()),
            },
            token,
        )
        .await?;
        return Ok(format_provenance_report(&report));
    }
    let (handle, mut exclude) = if let Some(target) = &options.target {
        let plan = resolve_clean_target(
            target,
            &ResolveCleanTargetOptions {
                repo_root: project_root.as_deref(),
                issue: options.issue,
                worktrees_dir: None,
                machine_root: None,
                env: Some(environment),
            },
            token,
        )
        .await?;
        (plan.handle, plan.exclude)
    } else {
        let default_scope = if options.state_root.is_none() {
            "repo"
        } else {
            "worktree"
        };
        let handle = resolve_state_root(
            project_root.as_deref(),
            &ResolveStateRootOptions {
                scope: Some(options.scope.as_deref().unwrap_or(default_scope)),
                state_root: options.state_root.as_deref(),
                worktree_root: None,
                machine_root: None,
                env: Some(environment),
            },
            token,
        )
        .await?;
        (handle, Vec::new())
    };
    exclude.extend(options.exclude.iter().cloned());
    let report = run_clean(
        &handle,
        &CleanRunOptions {
            execute: options.execute,
            yes: options.yes,
            interactive,
            relative_dir: String::new(),
            exclude,
            env: Some(environment.clone()),
        },
        token,
    )
    .await?;
    Ok(format_clean_report(&report))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEST_SEQ: AtomicU64 = AtomicU64::new(1);

    fn test_err(message: impl Into<String>) -> Error {
        Error::new("TestError", message)
    }

    fn temp_dir(prefix: &str) -> Result<PathBuf, Error> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "roster-clean-test-{prefix}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        files::create_private_directories(&dir).map_err(|error| test_err(error.message))?;
        Ok(dir)
    }

    struct TempCleanup(PathBuf);

    impl Drop for TempCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clean_ops_dry_run_confirmation_lock_and_atomic_deletion() -> Result<(), Error> {
        let root = temp_dir("state")?;
        let _cleanup = TempCleanup(root.clone());
        let handle = resolve_state_root(
            None,
            &ResolveStateRootOptions {
                scope: Some("worktree"),
                state_root: Some(&root),
                ..ResolveStateRootOptions::default()
            },
            None,
        )
        .await?;
        fs::create_dir_all(root.join("alpha")).map_err(|error| test_err(error.to_string()))?;
        fs::write(root.join("alpha").join("data.txt"), "record alpha")
            .map_err(|error| test_err(error.to_string()))?;
        fs::write(root.join("beta.json"), "{}").map_err(|error| test_err(error.to_string()))?;
        fs::create_dir_all(root.join("locks")).map_err(|error| test_err(error.to_string()))?;

        let dry = run_clean(&handle, &CleanRunOptions::default(), None).await?;
        assert!(dry.dry_run);
        assert_eq!(dry.record_count, 2);
        assert!(dry.excluded.contains(&"locks".to_owned()));
        assert!(root.join("alpha").exists());
        assert!(
            format_clean_report(&dry)
                .contains("mode: dry-run (nothing deleted; rerun with --execute --yes to delete)")
        );

        let unconfirmed = run_clean(
            &handle,
            &CleanRunOptions {
                execute: true,
                yes: false,
                ..CleanRunOptions::default()
            },
            None,
        )
        .await
        .err()
        .ok_or_else(|| test_err("expected confirmation error"))?;
        assert_eq!(unconfirmed.code, Some(CONFIRMATION_CODE));

        let escape = assert_safe_state_path(&handle, "../outside")
            .err()
            .ok_or_else(|| test_err("expected escape error"))?;
        assert_eq!(escape.code, Some(ESCAPE_CODE));

        let dot_err = resolve_state_path(&handle, ".")
            .err()
            .ok_or_else(|| test_err("expected forbidden error on ."))?;
        assert_eq!(dot_err.code, Some(FORBIDDEN_CODE));

        let secret = "clean-secret-token-value-12345";
        fs::write(root.join(format!("entry-{secret}.json")), "{}")
            .map_err(|error| test_err(error.to_string()))?;
        let mut events = Vec::new();
        let mut sink = |event: Value| -> Result<(), Error> {
            events.push(event);
            Ok(())
        };
        let cleaned = run_clean_with_events(
            &handle,
            &CleanRunOptions {
                execute: true,
                yes: true,
                exclude: vec!["beta.json".to_owned()],
                env: Some(Value::object(vec![(
                    "ROSTER_API_KEY",
                    Value::string(secret),
                )])),
                ..CleanRunOptions::default()
            },
            Some(&mut sink),
            None,
        )
        .await?;
        assert!(cleaned.executed);
        assert_eq!(cleaned.removed.len(), 2);
        assert_eq!(events.len(), 2);
        for event in &events {
            assert!(!event.json().contains(secret));
        }
        assert!(!root.join("alpha").exists());
        assert!(root.join("beta.json").exists());
        assert!(root.join("locks").exists());
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prune_provenance_separates_machine_history_and_curated_memory() -> Result<(), Error> {
        let store_dir = temp_dir("provenance")?;
        let _cleanup = TempCleanup(store_dir.clone());
        let store = store::Store::open(&store_dir)?;
        let raw_record = Value::object(vec![
            ("version", Value::Number(1.0)),
            ("id", Value::string("rec-raw-1")),
            ("sessionId", Value::string("sess-1")),
            ("runId", Value::string("run-1")),
            ("repoIdentity", Value::string("sha256-repo")),
            ("section", Value::string("raw-history")),
            ("createdAt", Value::Number(1000.0)),
            (
                "payload",
                Value::object(vec![("seat", Value::string("coder"))]),
            ),
        ]);
        let mem_record = Value::object(vec![
            ("version", Value::Number(1.0)),
            ("id", Value::string("rec-mem-1")),
            ("sessionId", Value::string("sess-1")),
            ("runId", Value::string("run-1")),
            ("repoIdentity", Value::string("sha256-repo")),
            ("section", Value::string("curated-memory")),
            ("createdAt", Value::Number(1000.0)),
            (
                "payload",
                Value::object(vec![("note", Value::string("keep"))]),
            ),
        ]);
        store.append_record(&raw_record, None).await?;
        store.append_record(&mem_record, None).await?;

        let dry = prune_provenance(
            &store_dir,
            &PruneProvenanceOptions {
                target: "machine-history".to_owned(),
                execute: false,
                yes: false,
                ..PruneProvenanceOptions::default()
            },
            None,
        )
        .await?;
        assert!(dry.dry_run);
        assert_eq!(dry.records, vec!["rec-raw-1"]);
        assert_eq!(dry.kept, 1);

        let executed = prune_provenance(
            &store_dir,
            &PruneProvenanceOptions {
                target: "machine-history".to_owned(),
                execute: true,
                yes: true,
                ..PruneProvenanceOptions::default()
            },
            None,
        )
        .await?;
        assert!(executed.executed);
        assert_eq!(executed.removed, vec!["rec-raw-1"]);
        let remaining = store.read_all()?;
        assert_eq!(remaining.records.len(), 1);
        assert_eq!(
            remaining.records[0]
                .get("id")
                .and_then(Value::as_string)
                .ok_or_else(|| test_err("id"))?
                .to_utf8()?,
            "rec-mem-1"
        );
        Ok(())
    }
}
