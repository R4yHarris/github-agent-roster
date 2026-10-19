use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Error;
use crate::cancel::CancellationToken;
use crate::config::Config;
use crate::execution::{backend, owned, policy, tools};
use crate::host::Image;
use crate::json::Value;
use crate::paths::{self, FileIdentity, RepositoryPaths};
use crate::process::{FailureKind, Limits};
use crate::state::{files, locks, redaction, secure};
use crate::text::Text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sync {
    Fetch,
    Offline,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub base: String,
    pub sync: Sync,
}

impl Options {
    pub fn resolve(config: &Config, base: Option<&str>, sync: Option<Sync>) -> Result<Self, Error> {
        let start = config.as_value().get("start");
        let base = base
            .map(str::to_owned)
            .or_else(|| {
                start
                    .and_then(|value| value.get("base"))
                    .and_then(Value::as_string)
                    .and_then(|value| value.to_utf8().ok())
            })
            .unwrap_or_else(|| "trunk".to_owned());
        crate::config::validate_start_base(&Text::from(base.as_str()), "--base")?;
        let sync = sync.unwrap_or_else(|| {
            if start
                .and_then(|value| value.get("sync"))
                .and_then(Value::as_string)
                .is_some_and(|value| value == &Text::from("offline"))
            {
                Sync::Offline
            } else {
                Sync::Fetch
            }
        });
        Ok(Self { base, sync })
    }
}

#[derive(Debug, Clone, Default)]
pub enum FetchPolicy {
    #[default]
    Deny,
    Http,
    Local {
        source: PathBuf,
        head: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssueBranch {
    pub number: u64,
    pub attempt: Option<u8>,
}

pub fn issue_branch(number: u64, attempt: Option<u8>) -> Result<String, Error> {
    if number == 0
        || number > crate::MAX_SAFE_INTEGER
        || attempt.is_some_and(|attempt| !(1..=16).contains(&attempt))
    {
        return Err(Error::type_error(
            "Issue number must be a positive safe integer and attempt must be between 1 and 16",
        ));
    }
    Ok(format!(
        "agents/{number}{}",
        attempt.map_or_else(String::new, |attempt| format!("-a{attempt}"))
    ))
}

pub fn parse_issue_branch(branch: &str) -> Option<IssueBranch> {
    let tail = branch
        .strip_prefix("agents/")
        .or_else(|| branch.strip_prefix("issue-"))?;
    let (number, attempt) = tail
        .split_once("-a")
        .map_or((tail, None), |(number, attempt)| (number, Some(attempt)));
    if number.starts_with('0') || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number = number.parse::<u64>().ok()?;
    let attempt = match attempt {
        Some(value) if value.starts_with('0') => return None,
        Some(value) => Some(value.parse::<u8>().ok()?),
        None => None,
    };
    issue_branch(number, attempt).ok()?;
    Some(IssueBranch { number, attempt })
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub locked: bool,
    pub prunable: bool,
}

fn sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn parse_inventory(bytes: &[u8]) -> Result<Vec<Worktree>, Error> {
    let source = std::str::from_utf8(bytes)
        .map_err(|_| Error::type_error("Git worktree inventory must be UTF-8"))?;
    let mut result = Vec::new();
    let mut issues = BTreeSet::new();
    let mut paths_seen = BTreeSet::new();
    for record in source.split("\0\0").filter(|record| !record.is_empty()) {
        let mut path = None;
        let mut head = None;
        let mut branch = None;
        let mut detached = false;
        let mut locked = false;
        let mut prunable = false;
        for field in record.split('\0').filter(|field| !field.is_empty()) {
            if let Some(value) = field.strip_prefix("worktree ") {
                if path.replace(PathBuf::from(value)).is_some() {
                    return Err(Error::type_error("Duplicate worktree path field"));
                }
            } else if let Some(value) = field.strip_prefix("HEAD ") {
                if !sha(value) || head.replace(value.to_owned()).is_some() {
                    return Err(Error::type_error(
                        "Worktree inventory has an invalid commit",
                    ));
                }
            } else if let Some(value) = field.strip_prefix("branch refs/heads/") {
                if branch.replace(value.to_owned()).is_some() || detached {
                    return Err(Error::type_error(
                        "Worktree inventory has ambiguous branch ownership",
                    ));
                }
            } else if field == "detached" && branch.is_none() {
                detached = true;
            } else if field == "locked" || field.starts_with("locked ") {
                locked = true;
            } else if field == "prunable" || field.starts_with("prunable ") {
                prunable = true;
            } else {
                return Err(Error::type_error(
                    "Unsupported Git worktree inventory field",
                ));
            }
        }
        let path = path
            .filter(|path| path.is_absolute())
            .ok_or_else(|| Error::type_error("Registered worktree has no absolute path"))?;
        let key = path.to_string_lossy().to_lowercase();
        if !paths_seen.insert(key) {
            return Err(Error::type_error(
                "Two Git worktree registrations name the same path",
            ));
        }
        if let Some(issue) = branch.as_deref().and_then(parse_issue_branch) {
            if !issues.insert((issue.number, issue.attempt)) {
                return Err(Error::new(
                    "WorktreeOwnershipError",
                    "Two issue branches have the same task ownership; preserve both and resolve their namespaces explicitly",
                ));
            }
        }
        result.push(Worktree {
            path,
            head: head.ok_or_else(|| Error::type_error("Registered worktree has no commit"))?,
            branch,
            locked,
            prunable,
        });
    }
    if result.is_empty() || result.len() > 256 {
        return Err(Error::type_error(
            "Git worktree inventory must contain 1-256 worktrees",
        ));
    }
    Ok(result)
}

pub struct Git {
    host: tools::Host,
    image: Image,
    no_hooks: PathBuf,
    uncertain: Cell<bool>,
    timings: Option<crate::execution::timing::Timings>,
}

struct Pending<'a> {
    uncertain: &'a Cell<bool>,
    settled: bool,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.uncertain.set(true);
        }
    }
}

fn git_failure_facts(root: &Path, args: &[&str], failure: &crate::process::Failure) -> Error {
    let operation = match args.first().copied() {
        Some("rev-parse") => "revision",
        Some("status") => "status",
        Some("config") => "configuration",
        Some("diff") => "diff",
        Some("worktree") => "worktree",
        Some("clone") => "clone",
        Some("submodule") => "submodule",
        _ => "other",
    };
    let cwd = match std::fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => "directory",
        Ok(_) => "not-directory",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "unavailable",
    };
    let os_code = failure
        .source
        .as_ref()
        .and_then(std::io::Error::raw_os_error);
    let io_kind = failure.source.as_ref().map(std::io::Error::kind);
    Error::new(
        "NativeGitFailureFacts",
        format!(
            "post-failure operation={operation}; os_code={os_code:?}; io_kind={io_kind:?}; cwd_state={cwd}; existing_image_checks=passed; existing_cancel_check=passed; cleanup_complete={}; child_pid_present={}",
            failure.cleanup_complete,
            failure.captured.pid.is_some()
        ),
    )
}

fn repository_paths_need_legacy(paths: &RepositoryPaths) -> bool {
    [&paths.worktree, &paths.common_dir, &paths.git_dir]
        .into_iter()
        .any(|path| path.as_os_str().as_encoded_bytes().contains(&b'\n'))
}

fn parse_repository_paths(bytes: &[u8]) -> Result<[String; 3], Error> {
    if !u64::try_from(bytes.len()).is_ok_and(|length| length <= crate::execution::MAX_FILE_BYTES) {
        return Err(Error::type_error("Git repository paths exceed 16 MiB"));
    }
    let source =
        std::str::from_utf8(bytes).map_err(|_| Error::type_error("Git metadata must be UTF-8"))?;
    let source = source
        .strip_suffix('\n')
        .ok_or_else(|| Error::type_error("Git repository paths require three complete lines"))?;
    let mut lines = source.split('\n');
    let [Some(root), Some(common), Some(git_dir)] = [lines.next(), lines.next(), lines.next()]
    else {
        return Err(Error::type_error(
            "Git repository paths require exactly three lines",
        ));
    };
    if lines.next().is_some() {
        return Err(Error::type_error(
            "Git repository paths require exactly three lines",
        ));
    }
    let result = [root, common, git_dir].map(|line| line.trim_end_matches('\r').to_owned());
    if result
        .iter()
        .any(|path| path.is_empty() || path.contains('\0'))
    {
        return Err(Error::type_error(
            "Git repository paths require three nonempty paths without NUL",
        ));
    }
    Ok(result)
}

impl Git {
    pub fn new(host: tools::Host, no_hooks: PathBuf) -> Result<Self, Error> {
        let image = Image::approve(&host.git)?;
        secure::check_components(&no_hooks)?;
        if !secure::entries(&no_hooks)?.is_empty() {
            return Err(Error::type_error(
                "The trusted Git hook-disable directory must be empty",
            ));
        }
        Ok(Self {
            host,
            image,
            no_hooks,
            uncertain: Cell::new(false),
            timings: None,
        })
    }

    pub(crate) fn bind_timings(&mut self, timings: crate::execution::timing::Timings) {
        self.timings = Some(timings);
    }

    pub fn cleanup_certain(&self) -> bool {
        !self.uncertain.get()
    }

    pub async fn run(
        &self,
        root: &Path,
        args: &[&str],
        token: &CancellationToken,
    ) -> Result<Vec<u8>, Error> {
        use crate::execution::timing::Label;
        let label = match args.first().copied() {
            Some("rev-parse")
                if args
                    == [
                        "rev-parse",
                        "--show-toplevel",
                        "--git-common-dir",
                        "--git-dir",
                    ] =>
            {
                Label::StartGitPaths
            }
            Some("rev-parse") if args.contains(&"--show-toplevel") => Label::StartGitRoot,
            Some("rev-parse") if args.contains(&"--git-common-dir") => Label::StartGitCommon,
            Some("rev-parse") if args.contains(&"--git-dir") => Label::StartGitDirectory,
            Some("rev-parse") => Label::StartGitRevision,
            Some("symbolic-ref") => Label::StartGitBranch,
            Some("config") => Label::StartGitConfig,
            Some("status") => Label::StartGitStatus,
            Some("worktree") => Label::StartGitWorktrees,
            Some("diff") => Label::StartGitSourceDiff,
            Some("ls-files") => Label::StartGitUntracked,
            Some("check-ignore") => Label::StartGitIgnore,
            _ => Label::StartGitOther,
        };
        let _scope = self.timings.as_ref().map(|timings| timings.start(label));
        token.check()?;
        let image_scope = _scope.as_ref().map(|span| span.child(Label::StartGitImage));
        let checked = self.image.require_current();
        if let Some(image_scope) = image_scope {
            image_scope.finish_status(checked.is_ok());
        }
        checked?;
        if self.uncertain.get() || !secure::entries(&self.no_hooks)?.is_empty() {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Git ownership or hook suppression is unverified; preserve the worktree and lock",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        let mut scoped = if args.first() == Some(&"check-ignore") {
            Vec::new()
        } else {
            [
                "-c",
                "core.longpaths=true",
                "-c",
                "submodule.recurse=false",
                "-c",
                "fetch.recurseSubmodules=false",
                "-c",
                "init.templateDir=",
                "-c",
                "protocol.allow=never",
            ]
            .map(str::to_owned)
            .to_vec()
        };
        scoped.extend(args.iter().map(|arg| (*arg).to_owned()));
        let command = self
            .host
            .git_command(root, &self.no_hooks, &scoped, &Default::default());
        let command_scope = self
            .timings
            .as_ref()
            .map(|timings| timings.start(Label::StartGitExecute));
        let mut pending = Pending {
            uncertain: &self.uncertain,
            settled: false,
        };
        let result = owned::execute_observed_timed(
            command,
            Limits {
                timeout: Some(Duration::from_secs(30)),
                max_output_bytes: 16 * 1024 * 1024,
            },
            Some(token),
            None,
            command_scope.as_ref(),
        )
        .await;
        pending.settled = true;
        if let Some(scope) = command_scope {
            scope.finish_status(result.is_ok());
        }
        if result.as_ref().is_err_and(|failure| {
            !failure.cleanup_complete || matches!(failure.kind, FailureKind::Exit(None))
        }) {
            self.uncertain.set(true);
        }
        if self.uncertain.get() {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Git process-tree shutdown is unverified; retain ownership for explicit recovery",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        let image_scope = _scope.as_ref().map(|span| span.child(Label::StartGitImage));
        let checked = self.image.require_current();
        if let Some(image_scope) = image_scope {
            image_scope.finish_status(checked.is_ok());
        }
        checked?;
        match result {
            Ok(output) => {
                token.check()?;
                Ok(output.captured.stdout)
            }
            Err(failure) => {
                if !failure.cleanup_complete || matches!(failure.kind, FailureKind::Exit(None)) {
                    self.uncertain.set(true);
                    return Err(Error::new(
                        "OwnedProcessCleanupError",
                        "Git process-tree shutdown is unverified; retain ownership for explicit recovery",
                    )
                    .coded("ROSTER_PROCESS_CLEANUP"));
                }
                token.check()?;
                let code = failure.code().unwrap_or("ROSTER_GIT_FAILED");
                let facts = git_failure_facts(root, args, &failure);
                Err(Error::new(
                    "NativeGitError",
                    format!(
                        "Approved Git operation failed: {failure}; verify the selected repository, ref and offline objects"
                    ),
                )
                .coded(code)
                .caused_by(facts))
            }
        }
    }

    pub async fn text(
        &self,
        root: &Path,
        args: &[&str],
        token: &CancellationToken,
    ) -> Result<String, Error> {
        String::from_utf8(self.run(root, args, token).await?)
            .map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
            .map_err(|_| Error::type_error("Git metadata must be UTF-8"))
    }

    pub(crate) async fn repository_paths(
        &self,
        paths: &RepositoryPaths,
        token: &CancellationToken,
    ) -> Result<[String; 3], Error> {
        if repository_paths_need_legacy(paths) {
            let root = self
                .text(&paths.worktree, &["rev-parse", "--show-toplevel"], token)
                .await?;
            let common = self
                .text(&paths.worktree, &["rev-parse", "--git-common-dir"], token)
                .await?;
            let git_dir = self
                .text(&paths.worktree, &["rev-parse", "--git-dir"], token)
                .await?;
            return Ok([root, common, git_dir]);
        }
        parse_repository_paths(
            &self
                .run(
                    &paths.worktree,
                    &[
                        "rev-parse",
                        "--show-toplevel",
                        "--git-common-dir",
                        "--git-dir",
                    ],
                    token,
                )
                .await?,
        )
    }

    async fn optional(
        &self,
        root: &Path,
        args: &[&str],
        token: &CancellationToken,
    ) -> Result<Option<String>, Error> {
        match self.text(root, args, token).await {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.code == Some("ROSTER_GIT_FAILED") && self.cleanup_certain() => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Repository {
    pub paths: RepositoryPaths,
    pub head: String,
    pub branch: Option<String>,
    pub dirty: bool,
    pub worktrees: Vec<Worktree>,
    root: FileIdentity,
    common: FileIdentity,
    git_dir: FileIdentity,
    status: Vec<u8>,
    config: Vec<u8>,
    source_digest: String,
}

async fn source_digest(git: &Git, root: &Path, token: &CancellationToken) -> Result<String, Error> {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    digest.update(
        git.run(
            root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--binary",
                "HEAD",
                "--",
            ],
            token,
        )
        .await?,
    );
    let untracked = git
        .text(
            root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            token,
        )
        .await?;
    let mut bytes = 0_u64;
    for (index, path) in untracked
        .split('\0')
        .filter(|path| !path.is_empty())
        .enumerate()
    {
        if index >= crate::execution::MAX_FILES {
            return Err(Error::type_error(
                "Untracked start-source inventory exceeds 2048 paths",
            ));
        }
        if !policy::exact_path(path) {
            return Err(Error::type_error(
                "Untracked source inventory contains a non-exact path",
            ));
        }
        digest.update(path.as_bytes());
        if crate::planning::forbidden_read(path) || policy::managed(path) {
            continue;
        }
        let file = root.join(path);
        let snapshot = secure::read(&file, crate::execution::MAX_FILE_BYTES)?
            .ok_or_else(|| Error::type_error("Untracked source disappeared while preparing"))?;
        bytes = bytes
            .checked_add(snapshot.bytes.len() as u64)
            .filter(|bytes| *bytes <= crate::execution::MAX_SNAPSHOT_BYTES)
            .ok_or_else(|| Error::type_error("Untracked start-source snapshot exceeds 128 MiB"))?;
        if paths::file_information(&file)?.links != 1
            || !secure::unchanged(&file, &snapshot, crate::execution::MAX_FILE_BYTES)?
        {
            return Err(Error::type_error(
                "Untracked source identity changed while preparing",
            ));
        }
        digest.update(snapshot.bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn reject_git_execution_config(bytes: &[u8]) -> Result<(), Error> {
    let source = std::str::from_utf8(bytes)
        .map_err(|_| Error::type_error("Local Git configuration must be UTF-8"))?;
    for entry in source.split('\0').filter(|entry| !entry.is_empty()) {
        let key = entry.split('\n').next().unwrap_or("").to_ascii_lowercase();
        if key.starts_with("include.")
            || key.starts_with("includeif.")
            || key.starts_with("filter.")
            || key.starts_with("url.")
            || key.starts_with("credential.")
            || key.starts_with("http.")
            || key.starts_with("extensions.")
            || key == "core.sshcommand"
            || key == "core.askpass"
            || key == "core.gitproxy"
            || key == "core.alternaterefscommand"
            || key.starts_with("uploadpack.")
            || key.starts_with("diff.")
                && (key.ends_with(".command")
                    || key.ends_with(".textconv")
                    || key == "diff.external")
            || key.ends_with(".uploadpack")
            || key.ends_with(".receivepack")
            || key.ends_with(".vcs")
            || key.ends_with(".proxy")
            || key.starts_with("submodule.") && key.ends_with(".update")
        {
            return Err(Error::new(
                "GitConfigurationDenied",
                "Local Git configuration selects includes, filters, helpers or custom execution; remove those selections in a human-reviewed checkout before native local preparation",
            )
            .coded("ROSTER_GIT_CONFIG_DENIED"));
        }
    }
    Ok(())
}

impl Repository {
    pub async fn inspect(cwd: &Path, git: &Git, token: &CancellationToken) -> Result<Self, Error> {
        let paths = paths::repository_paths(cwd)?;
        let root = paths::file_information(&paths.worktree)?.identity;
        let common = paths::file_information(&paths.common_dir)?.identity;
        let git_dir = paths::file_information(&paths.git_dir)?.identity;
        let [actual, actual_common, actual_dir] = git.repository_paths(&paths, token).await?;
        if paths::file_information(Path::new(&actual))?.identity != root
            || paths::file_information(&paths::resolve(
                &paths.worktree,
                Path::new(&actual_common),
            )?)?
            .identity
                != common
            || paths::file_information(&paths::resolve(&paths.worktree, Path::new(&actual_dir))?)?
                .identity
                != git_dir
        {
            return Err(Error::type_error(
                "Git and filesystem repository ownership disagree",
            ));
        }
        let head = git
            .text(
                &paths.worktree,
                &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
                token,
            )
            .await?;
        if !sha(&head) {
            return Err(Error::type_error(
                "Local delivery requires a complete committed Git HEAD",
            ));
        }
        let branch = git
            .optional(
                &paths.worktree,
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
                token,
            )
            .await?;
        let config = git
            .run(
                &paths.worktree,
                &["config", "--local", "--no-includes", "--null", "--list"],
                token,
            )
            .await?;
        reject_git_execution_config(&config)?;
        let status = git
            .run(
                &paths.worktree,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                token,
            )
            .await?;
        let worktrees = parse_inventory(
            &git.run(
                &paths.worktree,
                &["worktree", "list", "--porcelain", "-z"],
                token,
            )
            .await?,
        )?;
        let source_digest = source_digest(git, &paths.worktree, token).await?;
        if worktrees
            .iter()
            .filter(|entry| {
                paths::file_information(&entry.path).is_ok_and(|entry| entry.identity == root)
            })
            .count()
            != 1
        {
            return Err(Error::type_error(
                "Current checkout has no unique registered worktree owner",
            ));
        }
        Ok(Self {
            paths,
            head,
            branch,
            dirty: !status.is_empty(),
            worktrees,
            root,
            common,
            git_dir,
            status,
            config,
            source_digest,
        })
    }

    pub async fn require_current(&self, git: &Git, token: &CancellationToken) -> Result<(), Error> {
        let current = Self::inspect(&self.paths.worktree, git, token).await?;
        if self.root != current.root
            || self.common != current.common
            || self.git_dir != current.git_dir
            || self.head != current.head
            || self.branch != current.branch
            || self.status != current.status
            || self.config != current.config
            || self.source_digest != current.source_digest
        {
            return Err(Error::new(
                "StaleRepository",
                "Repository identity, HEAD, branch, index, local Git configuration or source status changed; no stale preparation is accepted",
            )
            .coded("ROSTER_STALE_SOURCE"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Start {
    pub mode: &'static str,
    pub reference: Option<String>,
    pub commit: String,
    pub fetched: bool,
    pub notes: Vec<String>,
}

impl Start {
    pub fn format(&self) -> String {
        let location = match &self.reference {
            Some(reference) => format!(
                "{} {reference}@{}",
                self.mode,
                self.commit.get(..7).unwrap_or("invalid")
            ),
            None => "current HEAD".to_owned(),
        };
        let mut parts = vec![format!(
            "Start: {location} ({})",
            if self.fetched {
                "fetched"
            } else {
                "freshness unknown"
            }
        )];
        parts.extend(self.notes.iter().cloned());
        parts.join("; ")
    }
}

async fn commit_of(
    git: &Git,
    root: &Path,
    reference: &str,
    token: &CancellationToken,
) -> Result<String, Error> {
    let full_sha = matches!(reference.len(), 40 | 64)
        && reference.bytes().all(|byte| byte.is_ascii_hexdigit());
    let selected = if full_sha {
        reference.to_ascii_lowercase()
    } else if reference == "HEAD" {
        "HEAD".to_owned()
    } else {
        let candidates = if reference.starts_with("refs/") {
            vec![reference.to_owned()]
        } else {
            vec![
                format!("refs/heads/{reference}"),
                format!("refs/tags/{reference}"),
                format!("refs/remotes/{reference}"),
                format!("refs/remotes/{reference}/HEAD"),
            ]
        };
        let mut args = vec!["for-each-ref", "--format=%(refname)"];
        args.extend(candidates.iter().map(String::as_str));
        let output = git.text(root, &args, token).await?;
        let found = output
            .lines()
            .filter(|line| candidates.iter().any(|candidate| candidate == line))
            .collect::<Vec<_>>();
        let [selected] = found.as_slice() else {
            return Err(Error::new(
                "NativeGitRefError",
                "Start base is missing or ambiguous; choose an existing complete refs/heads/, refs/tags/ or refs/remotes/ name, or a complete commit SHA",
            ).coded("ROSTER_GIT_FAILED"));
        };
        if !["refs/heads/", "refs/remotes/", "refs/tags/"]
            .iter()
            .any(|prefix| selected.starts_with(prefix))
        {
            return Err(Error::type_error(
                "Start base must be a tracked branch, tag or remote ref",
            ));
        }
        (*selected).to_owned()
    };
    let commit = git
        .text(
            root,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{selected}^{{commit}}"),
            ],
            token,
        )
        .await?;
    if !sha(&commit) || full_sha && !commit.eq_ignore_ascii_case(reference) {
        return Err(Error::type_error(
            "Selected start base does not name the exact complete commit",
        ));
    }
    Ok(commit)
}

impl Start {
    pub async fn require_current(
        &self,
        git: &Git,
        root: &Path,
        token: &CancellationToken,
    ) -> Result<(), Error> {
        let actual = commit_of(
            git,
            root,
            self.reference.as_deref().unwrap_or("HEAD"),
            token,
        )
        .await?;
        if actual != self.commit {
            return Err(Error::new(
                "StaleStart",
                "Selected start ref changed after resolution; preserve existing worktrees and resolve the base again",
            ).coded("ROSTER_STALE_SOURCE"));
        }
        Ok(())
    }
}

pub async fn resolve(
    repository: &Repository,
    git: &Git,
    options: &Options,
    fetch: &FetchPolicy,
    token: &CancellationToken,
) -> Result<Start, Error> {
    crate::config::validate_start_base(&Text::from(options.base.as_str()), "--base")?;
    repository.require_current(git, token).await?;
    let root = &repository.paths.worktree;
    let origin = git
        .optional(root, &["remote", "get-url", "--all", "origin"], token)
        .await?;
    let mut fetched = false;
    let mut notes = Vec::new();
    if origin.is_none() {
        notes.push("no origin remote; freshness unknown".to_owned());
    } else if options.sync == Sync::Offline {
        notes.push("fetch skipped (offline); freshness unknown".to_owned());
    } else {
        let origin = origin
            .as_deref()
            .ok_or_else(|| Error::type_error("Missing origin"))?;
        if origin.contains(['\r', '\n']) {
            return Err(Error::type_error(
                "Approved fetch requires one unambiguous origin URL",
            ));
        }
        match fetch {
            FetchPolicy::Deny => {
                return Err(crate::execution::unsupported(
                    "Fetching requires explicit trusted-host approval; choose --no-fetch for offline work",
                ));
            }
            FetchPolicy::Http => {
                let remote = url::Url::parse(origin).map_err(|_| {
                    Error::type_error(
                        "Approved production fetch supports only credential-free HTTP(S) origins",
                    )
                })?;
                if !matches!(remote.scheme(), "http" | "https")
                    || !remote.username().is_empty()
                    || remote.password().is_some()
                    || remote.query().is_some()
                    || remote.fragment().is_some()
                {
                    return Err(Error::type_error(
                        "Approved production fetch requires a credential-free HTTP(S) origin",
                    ));
                }
                git.run(
                    root,
                    &[
                        "-c",
                        "protocol.http.allow=always",
                        "-c",
                        "protocol.https.allow=always",
                        "fetch",
                        "--prune",
                        "--quiet",
                        "--no-tags",
                        "--no-prune-tags",
                        "--no-recurse-submodules",
                        "--refmap=",
                        "origin",
                        "+refs/heads/*:refs/remotes/origin/*",
                    ],
                    token,
                )
                .await?;
            }
            FetchPolicy::Local { source, head } => {
                if !source.is_absolute() || !sha(head) || !Path::new(origin).is_absolute() {
                    return Err(Error::type_error(
                        "Fixture fetch needs an explicit absolute local origin and complete approved HEAD",
                    ));
                }
                secure::check_components(source)?;
                secure::check_components(Path::new(origin))?;
                if paths::file_information(source)?.identity
                    != paths::file_information(Path::new(origin))?.identity
                    || git
                        .text(
                            source,
                            &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
                            token,
                        )
                        .await?
                        != *head
                {
                    return Err(Error::type_error(
                        "Local fetch source identity or exact approved HEAD changed",
                    ));
                }
                reject_git_execution_config(
                    &git.run(
                        source,
                        &["config", "--local", "--no-includes", "--null", "--list"],
                        token,
                    )
                    .await?,
                )?;
                git.run(
                    root,
                    &[
                        "-c",
                        "protocol.file.allow=always",
                        "fetch",
                        "--prune",
                        "--quiet",
                        "--no-tags",
                        "--no-prune-tags",
                        "--no-recurse-submodules",
                        "--refmap=",
                        "origin",
                        "+refs/heads/*:refs/remotes/origin/*",
                    ],
                    token,
                )
                .await?;
            }
        }
        fetched = true;
    }
    if options.base == "current" {
        return Ok(Start {
            mode: "current",
            reference: None,
            commit: repository.head.clone(),
            fetched,
            notes,
        });
    }
    if options.base != "trunk" {
        return Ok(Start {
            mode: "ref",
            reference: Some(options.base.clone()),
            commit: commit_of(git, root, &options.base, token).await?,
            fetched,
            notes,
        });
    }
    let mut candidates = Vec::new();
    if origin.is_some() {
        if let Some(head) = git
            .optional(
                root,
                &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
                token,
            )
            .await?
        {
            if head.starts_with("refs/remotes/origin/") && !head.contains(['\r', '\n']) {
                candidates.push(head);
            }
        }
        let known = git
            .text(
                root,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/remotes/origin/main",
                    "refs/remotes/origin/master",
                ],
                token,
            )
            .await?;
        for name in ["refs/remotes/origin/main", "refs/remotes/origin/master"] {
            if known.lines().any(|line| line == name)
                && !candidates.iter().any(|value| value == name)
            {
                candidates.push(name.to_owned());
            }
        }
    }
    for candidate in candidates {
        match commit_of(git, root, &candidate, token).await {
            Ok(commit) => {
                return Ok(Start {
                    mode: "trunk",
                    reference: Some(candidate),
                    commit,
                    fetched,
                    notes,
                });
            }
            Err(error) if error.code == Some("ROSTER_GIT_FAILED") => {}
            Err(error) => return Err(error),
        }
    }
    notes.push("origin default branch not found; starting from current HEAD".to_owned());
    Ok(Start {
        mode: "current",
        reference: None,
        commit: repository.head.clone(),
        fetched,
        notes,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drift {
    pub ahead: u64,
    pub behind: u64,
}

pub async fn branch_drift(
    repository: &Repository,
    git: &Git,
    branch: &str,
    start: &Start,
    token: &CancellationToken,
) -> Result<Option<Drift>, Error> {
    crate::config::validate_start_base(&Text::from(branch), "branch")?;
    if start.reference.is_none() {
        return Ok(None);
    }
    let output = git
        .text(
            &repository.paths.worktree,
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("refs/heads/{branch}...{}", start.commit),
            ],
            token,
        )
        .await?;
    let values = output
        .split_whitespace()
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::type_error("Git branch drift must contain two bounded counts"))?;
    match values.as_slice() {
        [ahead, behind] => Ok(Some(Drift {
            ahead: *ahead,
            behind: *behind,
        })),
        _ => Err(Error::type_error(
            "Git branch drift must contain exactly two counts",
        )),
    }
}

#[derive(Debug, Clone)]
struct Module {
    parent: PathBuf,
    path: String,
    source: PathBuf,
    commit: String,
}

async fn modules(
    git: &Git,
    root: &Path,
    commit: &str,
    approvals: &[backend::LocalSubmodule],
    token: &CancellationToken,
) -> Result<Vec<Module>, Error> {
    let mut pending = vec![(
        root.to_path_buf(),
        commit.to_owned(),
        PathBuf::new(),
        String::new(),
        0,
    )];
    let mut selected = Vec::new();
    let mut checkout_bytes = 0_u64;
    while let Some((source, commit, parent, prefix, depth)) = pending.pop() {
        if depth > 4 {
            return Err(Error::type_error(
                "Approved local submodules exceed four levels",
            ));
        }
        let tree = git
            .text(&source, &["ls-tree", "-r", "-l", "-z", &commit], token)
            .await?;
        if tree.split('\0').filter(|entry| !entry.is_empty()).count() > 2048 {
            return Err(Error::type_error(
                "Selected Git base exceeds 2048 tracked entries",
            ));
        }
        for entry in tree.split('\0').filter(|entry| !entry.is_empty()) {
            let (metadata, path) = entry
                .split_once('\t')
                .ok_or_else(|| Error::type_error("Malformed base tree entry"))?;
            let parts = metadata.split_whitespace().collect::<Vec<_>>();
            if !policy::exact_path(path) || parts.len() != 4 {
                return Err(Error::type_error(
                    "Selected Git base contains a non-exact path",
                ));
            }
            if parts.first() == Some(&"120000") {
                return Err(crate::execution::unsupported(
                    "Linked files in the selected local base",
                ));
            }
            if parts.first() != Some(&"160000") {
                let size = parts.get(3).and_then(|size| size.parse::<u64>().ok())
                    .filter(|size| *size <= crate::execution::MAX_FILE_BYTES)
                    .ok_or_else(|| Error::type_error("Selected local base contains a blob exceeding the 16 MiB per-file bound"))?;
                checkout_bytes = checkout_bytes
                    .checked_add(size)
                    .filter(|size| *size <= crate::execution::MAX_SNAPSHOT_BYTES)
                    .ok_or_else(|| {
                        Error::type_error("Selected local checkout and submodules exceed 128 MiB")
                    })?;
            }
            if path == "package.json" {
                let package = git
                    .text(
                        &source,
                        &[
                            "cat-file",
                            "blob",
                            parts
                                .get(2)
                                .copied()
                                .ok_or_else(|| Error::type_error("Missing package object"))?,
                        ],
                        token,
                    )
                    .await?;
                let package = crate::json::parse(&Text::from(package.as_str()))?;
                if crate::domain::is_text(package.get("name"), "github-agent-roster") {
                    return Err(crate::execution::unsupported(
                        "Materializing retired Node Roster production as a local base",
                    ));
                }
            }
            if parts.first() != Some(&"160000") {
                continue;
            }
            let full = format!("{prefix}{path}");
            let approval = approvals.iter().find(|approval| approval.path == full).ok_or_else(|| {
                crate::execution::unsupported("A selected gitlink has no explicit approved local submodule source; no .gitmodules URL was fetched")
            })?;
            if !approval.source.is_absolute() {
                return Err(Error::type_error(
                    "Submodule source must be an absolute approved local directory",
                ));
            }
            secure::check_components(&approval.source)?;
            let expected = parts
                .get(2)
                .copied()
                .filter(|value| sha(value))
                .ok_or_else(|| Error::type_error("Invalid submodule commit"))?;
            let actual = git
                .text(
                    &approval.source,
                    &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
                    token,
                )
                .await?;
            if actual != expected {
                return Err(Error::type_error(
                    "Approved local submodule HEAD must exactly equal the selected gitlink commit",
                ));
            }
            reject_git_execution_config(
                &git.run(
                    &approval.source,
                    &["config", "--local", "--no-includes", "--null", "--list"],
                    token,
                )
                .await?,
            )?;
            let manifest = git
                .text(
                    &source,
                    &[
                        "config",
                        "--blob",
                        &format!("{commit}:.gitmodules"),
                        "--no-includes",
                        "--null",
                        "--get-regexp",
                        r"^submodule\..*\.path$",
                    ],
                    token,
                )
                .await?;
            let names = manifest
                .split('\0')
                .filter_map(|entry| {
                    let (name, value) = entry.split_once('\n')?;
                    (value == path)
                        .then(|| name.strip_prefix("submodule.")?.strip_suffix(".path"))
                        .flatten()
                })
                .collect::<Vec<_>>();
            let [name] = names.as_slice() else {
                return Err(Error::type_error(
                    "Selected submodule needs one unambiguous declared .gitmodules path",
                ));
            };
            if name.is_empty()
                || !name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/')
                })
            {
                return Err(Error::type_error(
                    "Submodule name is not a bounded declarative identifier",
                ));
            }
            selected.push(Module {
                parent: parent.clone(),
                path: path.to_owned(),
                source: approval.source.clone(),
                commit: actual.clone(),
            });
            pending.push((
                approval.source.clone(),
                actual,
                parent.join(path),
                format!("{full}/"),
                depth + 1,
            ));
        }
    }
    if selected.len() != approvals.len() || selected.len() > 16 {
        return Err(Error::type_error(
            "Submodule approvals must exactly cover the selected local gitlinks",
        ));
    }
    Ok(selected)
}

pub struct Prepared {
    pub worktree: PathBuf,
    pub task: String,
    pub session: String,
    pub branch: String,
    pub start: Start,
}

#[derive(Default)]
pub struct Preparation {
    lease: Option<backend::RootLease>,
    root: Option<PathBuf>,
    acquiring: bool,
}

impl Preparation {
    pub fn root(&self) -> Option<&Path> {
        self.lease
            .as_ref()
            .map(backend::RootLease::root)
            .or(self.root.as_deref())
    }

    pub fn lease(&self) -> Result<&backend::RootLease, Error> {
        self.lease.as_ref().ok_or_else(|| {
            Error::new(
                "RootOwnershipError",
                "Preparation has no admitted root lease",
            )
        })
    }

    pub(crate) fn lease_slot(&mut self) -> Result<&mut Option<backend::RootLease>, Error> {
        if self.acquiring {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Root admission remains unverified",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        Ok(&mut self.lease)
    }

    pub async fn acquire(
        &mut self,
        root: &Path,
        session: &str,
        host: tools::Host,
        redaction: redaction::Config,
        token: CancellationToken,
    ) -> Result<(), Error> {
        if self.root().is_some() {
            return Err(Error::type_error(
                "Caller already retains a preparation root",
            ));
        }
        self.root = Some(paths::absolute(root)?);
        self.acquiring = true;
        match backend::RootLease::open(root, session, host, redaction, token).await {
            Ok(lease) => {
                self.lease = Some(lease);
                self.acquiring = false;
                Ok(())
            }
            Err(error) => {
                if !matches!(
                    error.code,
                    Some("ROSTER_PROCESS_CLEANUP" | "E_LOCK_RELEASE" | "E_LOCK_LOST")
                ) {
                    self.acquiring = false;
                }
                Err(error)
            }
        }
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        if self.acquiring {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Preparation admission was interrupted; retain the root for explicit recovery",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        if let Some(lease) = &mut self.lease {
            lease.close().await?;
            self.lease = None;
        }
        self.root = None;
        Ok(())
    }
}

struct PendingMaterialization<'a> {
    lease: &'a backend::RootLease,
    git: &'a Git,
    settled: bool,
}

impl Drop for PendingMaterialization<'_> {
    fn drop(&mut self) {
        if !self.settled || !self.git.cleanup_certain() {
            backend::Owner::PrePlan(self.lease).quarantine();
        }
    }
}

pub struct Contracts {
    root: PathBuf,
    identity: FileIdentity,
    commit: String,
    publisher: secure::Snapshot,
}

impl Contracts {
    pub async fn open(
        installation: &Path,
        root: &Path,
        git: &Git,
        token: &CancellationToken,
    ) -> Result<Self, Error> {
        secure::check_components(root)?;
        let pin = git
            .text(
                installation,
                &["ls-tree", "HEAD", "--", "vendor/github-agent-contracts"],
                token,
            )
            .await?;
        let parts = pin.split_whitespace().collect::<Vec<_>>();
        let ["160000", "commit", commit, "vendor/github-agent-contracts"] = parts.as_slice() else {
            return Err(crate::execution::unsupported(
                "The installation has no reviewed contracts gitlink pin; use a complete native development installation",
            ));
        };
        if !sha(commit) {
            return Err(Error::type_error(
                "Reviewed contracts gitlink is not a complete commit",
            ));
        }
        let publisher = secure::read(&root.join("scripts").join("agent-pr.mjs"), 512 * 1024)?
            .ok_or_else(|| Error::type_error("Required contracts publisher marker disappeared"))?;
        let seal = Self {
            root: paths::absolute(root)?,
            identity: paths::file_information(root)?.identity,
            commit: (*commit).to_owned(),
            publisher,
        };
        seal.require_current(git, token).await?;
        Ok(seal)
    }

    pub async fn require_current(&self, git: &Git, token: &CancellationToken) -> Result<(), Error> {
        secure::check_components(&self.root)?;
        if paths::file_information(&self.root)?.identity != self.identity
            || !secure::unchanged(
                &self.root.join("scripts").join("agent-pr.mjs"),
                &self.publisher,
                512 * 1024,
            )?
            || paths::file_information(&self.root.join("scripts").join("agent-pr.mjs"))?.links != 1
        {
            return Err(Error::type_error(
                "Required contracts dependency identity changed; no source was adopted",
            ));
        }
        reject_git_execution_config(
            &git.run(
                &self.root,
                &["config", "--local", "--no-includes", "--null", "--list"],
                token,
            )
            .await?,
        )?;
        let worktree = git
            .text(&self.root, &["rev-parse", "--show-toplevel"], token)
            .await?;
        if paths::file_information(Path::new(&worktree))?.identity != self.identity {
            return Err(Error::type_error(
                "Contracts Git worktree ownership differs from the selected dependency root",
            ));
        }
        let actual = git
            .text(
                &self.root,
                &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
                token,
            )
            .await?;
        if actual != self.commit
            || !git
                .run(
                    &self.root,
                    &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                    token,
                )
                .await?
                .is_empty()
        {
            return Err(Error::type_error(
                "Contracts dependency must be clean at the installation's exact reviewed gitlink; initialize the pinned submodule explicitly, without editing vendor sources",
            ));
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_local(
    repository: &Repository,
    git: &Git,
    config: &Config,
    start: Start,
    approval: &backend::Approval,
    redaction: redaction::Config,
    token: CancellationToken,
    preparation: &mut Preparation,
) -> Result<Prepared, Error> {
    token.check()?;
    approval.validate()?;
    if preparation.root().is_some() {
        return Err(Error::type_error(
            "Local preparation requires an empty caller-owned root slot",
        ));
    }
    repository.require_current(git, &token).await?;
    start
        .require_current(git, &repository.paths.worktree, &token)
        .await?;
    let modules = modules(
        git,
        &repository.paths.worktree,
        &start.commit,
        &approval.submodules,
        &token,
    )
    .await?;
    let task = format!("local-{}", files::nonce()?);
    let session = format!("roster-{}", files::nonce()?);
    let branch = format!("agents/{task}");
    let worktrees = config
        .as_value()
        .get("paths")
        .and_then(|value| value.get("worktrees"))
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Local preparation needs paths.worktrees"))?
        .to_utf8()?;
    let worktree = repository.paths.repository.join(&worktrees).join(&task);
    paths::ensure_local_path(&worktree, &repository.paths.repository)?;
    if worktree.exists() {
        return Err(Error::type_error(
            "New local worktree path already exists; unknown ownership is preserved",
        ));
    }
    git.run(&repository.paths.worktree, &["check-ignore", "--no-index", "--quiet", "--", &format!("{worktrees}/{task}/")], &token).await
        .map_err(|error| Error::new("WorktreeOwnershipError", "Managed worktree paths must already be Git-ignored; update ignore rules as a human before local preparation").caused_by(error))?;
    let namespace = repository.paths.common_dir.join("roster-local");
    secure::private_directory(&namespace, true)?;
    let target = namespace.join("prepare.lock");
    let mut lock = locks::acquire(
        "prepare",
        &target,
        &locks::Options {
            holder: session.clone(),
            run_id: Some(task.clone()),
            ..locks::Options::default()
        },
        Some(&token),
    )
    .await?;
    let result = async {
        lock.check()?;
        repository.require_current(git, &token).await?;
        start.require_current(git, &repository.paths.worktree, &token).await?;
        let branches = git.text(&repository.paths.worktree, &["for-each-ref", "--format=%(refname)", &format!("refs/heads/{branch}")], &token).await?;
        if !branches.is_empty() || worktree.exists() {
            return Err(Error::type_error("Local branch/path ownership collision; nothing was adopted or replaced"));
        }
        let record = Value::object(vec![
            ("schema_version", Value::Number(1.0)),
            ("task", Value::string(&task)),
            ("session", Value::string(&session)),
            ("branch", Value::string(&branch)),
            ("base", Value::string(&start.commit)),
            ("worktree", Value::string(worktree.to_str().ok_or_else(|| Error::type_error("Worktree path must be Unicode"))?)),
            ("publication", Value::string("unknown")),
        ]);
        secure::write_exclusive(&namespace.join(format!("{task}.json")), record.json().as_bytes())?;
        secure::private_directory(worktree.parent().ok_or_else(|| Error::type_error("Worktree needs a parent"))?, true)?;
        git.run(&repository.paths.worktree, &["worktree", "add", "--quiet", "-b", &branch, worktree.to_str().ok_or_else(|| Error::type_error("Worktree path must be Unicode"))?, &start.commit], &token).await?;
        let actual = Repository::inspect(&worktree, git, &token).await?;
        if actual.common != repository.common || actual.head != start.commit || actual.branch.as_deref() != Some(branch.as_str()) || actual.dirty {
            return Err(Error::type_error("New worktree identity, branch, base or cleanliness differs from its prepared claim"));
        }
        preparation.acquire(&worktree, &session, git.host.clone(), redaction.clone(), token.clone()).await?;
        let lease = preparation.lease()?;
        let mut pending = PendingMaterialization { lease, git, settled: false };
        let initialized = async {
            for module in &modules {
                let directory = worktree.join(&module.parent);
                let destination = directory.join(&module.path);
                git.run(&directory, &[
                    "-c", "protocol.file.allow=always",
                    "clone", "--quiet", "--no-checkout",                     "--shared", "--",
                    module.source.to_str().ok_or_else(|| Error::type_error("Approved submodule source must be Unicode"))?,
                    destination.to_str().ok_or_else(|| Error::type_error("Approved submodule destination must be Unicode"))?,
                ], &token).await?;
                git.run(&destination, &["checkout", "--quiet", "--detach", &module.commit], &token).await?;
                if git.text(&directory.join(&module.path), &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"], &token).await? != module.commit {
                    return Err(Error::type_error("Initialized local submodule differs from its exact approved gitlink"));
                }
                lease.check()?;
            }
            repository.require_current(git, &token).await?;
            start.require_current(git, &repository.paths.worktree, &token).await?;
            lease.check()
        }.await;
        pending.settled = true;
        drop(pending);
        initialized?;
        Ok(Prepared { worktree: worktree.clone(), task: task.clone(), session: session.clone(), branch, start })
    }.await;
    if !git.cleanup_certain() {
        return Err(Error::new(
            "OwnedProcessCleanupError",
            format!(
                "Preparation cleanup is unverified; preserve the claim, lock and worktree at {}",
                worktree.display()
            ),
        )
        .coded("ROSTER_PROCESS_CLEANUP"));
    }
    locks::finish(result, lock.release_required().await).map_err(|error| {
        Error::new(
            error.name,
            format!(
                "{}; local worktree location: {}",
                error.message,
                worktree.display()
            ),
        )
        .coded(error.code.unwrap_or("ROSTER_LOCAL_PREPARATION"))
        .caused_by(error)
    })
}

#[cfg(test)]
mod repository_path_tests {
    use super::*;

    #[test]
    fn repository_path_frames_preserve_legacy_values_and_record_trimming() -> Result<(), Error> {
        for expected in [
            ["D:/owned main", ".git", ".git"],
            [
                "/owned 'quotes' \"double\" révision 東京",
                "../common/.git",
                "../common/.git/worktrees/linked",
            ],
            [
                "  /owned/edge  ",
                "relative\rinside",
                "/owned/\u{a0}edge\u{2003}",
            ],
        ] {
            let [root, common, git_dir] = expected;
            for ending in ["\n", "\r\n", "\r\r\n"] {
                let raw = format!("{root}{ending}{common}{ending}{git_dir}{ending}");
                assert_eq!(parse_repository_paths(raw.as_bytes())?, expected);
            }
        }
        Ok(())
    }

    #[test]
    fn repository_path_frames_refuse_missing_extra_empty_and_unterminated_records() {
        for raw in [
            "",
            "root\n",
            "root\ncommon\n",
            "root\ncommon\ndir",
            "root\ncommon\ndir\nextra\n",
            "root\ncommon\ndir\n\n",
            "\ncommon\ndir\n",
            "root\n\ndir\n",
            "root\ncommon\n\n",
            "root\ncommon\n\r\r\n",
            "root\ncommon\0extra\ndir\n",
        ] {
            assert!(parse_repository_paths(raw.as_bytes()).is_err());
        }
    }

    #[test]
    fn repository_path_frames_refuse_invalid_utf8_and_enforce_exact_byte_bound() -> Result<(), Error>
    {
        for raw in [
            b"\xff\ncommon\ndir\n".as_slice(),
            b"root\n\xff\ndir\n".as_slice(),
            b"root\ncommon\n\xff\n".as_slice(),
        ] {
            assert!(parse_repository_paths(raw).is_err());
        }
        let bound = usize::try_from(crate::execution::MAX_FILE_BYTES)
            .map_err(|_| Error::type_error("Repository path bound must fit usize"))?;
        let length = bound
            .checked_sub(5)
            .ok_or_else(|| Error::type_error("Repository path bound must fit framing"))?;
        let mut raw = vec![b'x'; length];
        raw.extend_from_slice(b"\nc\nd\n");
        let [root, common, git_dir] = parse_repository_paths(&raw)?;
        assert_eq!(root.len(), length);
        assert_eq!(common, "c");
        assert_eq!(git_dir, "d");
        raw.push(b'\n');
        assert!(parse_repository_paths(&raw).is_err());
        Ok(())
    }

    #[test]
    fn repository_path_compatibility_is_selected_from_expected_delimiters_only() {
        let plain = RepositoryPaths {
            worktree: PathBuf::from("owned 'quotes' \"double\" révision 東京"),
            common_dir: PathBuf::from("common\rinside"),
            git_dir: PathBuf::from("relative/.git"),
            repository: PathBuf::from("repository"),
        };
        assert!(!repository_paths_need_legacy(&plain));
        for field in 0..3 {
            let mut paths = plain.clone();
            match field {
                0 => paths.worktree.push("line\ninside"),
                1 => paths.common_dir.push("line\ninside"),
                _ => paths.git_dir.push("line\ninside"),
            }
            assert!(repository_paths_need_legacy(&paths));
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut paths = plain.clone();
            paths.common_dir = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff, b'\n']));
            assert!(repository_paths_need_legacy(&paths));
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let mut paths = plain;
            paths.common_dir =
                PathBuf::from(std::ffi::OsString::from_wide(&[0xd800, u16::from(b'\n')]));
            assert!(repository_paths_need_legacy(&paths));
        }
    }
}
