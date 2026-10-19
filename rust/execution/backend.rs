use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::cancel::CancellationToken;
use crate::paths::{self, FileIdentity};
use crate::process::{FailureKind, Limits};
use crate::state::{locks, redaction, secure};

#[derive(Debug, Clone)]
pub struct LocalSubmodule {
    pub path: String,
    pub source: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct Approval {
    pub counterfactual: bool,
    pub classifications: bool,
    pub hooks_node: Option<PathBuf>,
    pub native_cli: Option<PathBuf>,
    pub node: Option<PathBuf>,
    pub submodules: Vec<LocalSubmodule>,
    pub timeout_ms: Option<u64>,
    pub independent: docker::Selection,
}

impl Approval {
    pub fn validate(&self) -> Result<(), Error> {
        if let docker::Selection::Docker(Some(approval)) = &self.independent {
            approval.validate()?;
        }
        if self
            .timeout_ms
            .is_some_and(|value| value == 0 || value > policy::FULL_TEST_TIMEOUT_MS)
            || self.submodules.len() > 16
        {
            return Err(Error::type_error(
                "Backend approval exceeds timeout or local-submodule bounds",
            ));
        }
        let mut paths = Vec::new();
        for module in &self.submodules {
            if !policy::exact_path(&module.path)
                || !module.source.is_absolute()
                || paths.contains(&module.path)
            {
                return Err(denied(
                    "Submodule approval requires unique exact paths and absolute local sources",
                ));
            }
            paths.push(module.path.clone());
        }
        for program in [&self.hooks_node, &self.native_cli, &self.node]
            .into_iter()
            .flatten()
        {
            if !program.is_absolute() {
                return Err(denied(
                    "Backend executable approvals must be absolute paths",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn timeout(&self) -> u64 {
        self.timeout_ms.unwrap_or(30_000)
    }
}

pub struct RootLease {
    root: PathBuf,
    root_id: FileIdentity,
    marker_id: FileIdentity,
    marker: Option<secure::Snapshot>,
    host: tools::Host,
    redaction: redaction::Config,
    token: CancellationToken,
    ownership: locks::RepoLock,
    identity: Arc<()>,
    uncertain: Cell<bool>,
    no_hooks: PathBuf,
    images: Vec<crate::host::Image>,
}

impl RootLease {
    pub async fn open(
        root: &Path,
        session: &str,
        host: tools::Host,
        redaction: redaction::Config,
        token: CancellationToken,
    ) -> Result<Self, Error> {
        token.check()?;
        redaction.validate()?;
        let images = crate::host::pins(&host)?;
        let root = paths::absolute(root)?;
        secure::check_components(&root)?;
        let root_id = paths::file_information(&root)?.identity;
        let git = root.join(".git");
        secure::check_components(&git)?;
        let marker_info = paths::file_information(&git)?;
        let marker = if git.is_file() {
            secure::read(&git, 65_536)?
        } else {
            None
        };
        if marker.is_some() && marker_info.links != 1 {
            return Err(denied(
                "Pre-plan root lease requires a single-link Git marker",
            ));
        }
        let target = locks::target("run", &root, &root, None)?;
        let no_hooks = target
            .parent()
            .ok_or_else(|| denied("Run lock needs a parent"))?
            .join("no-hooks");
        if !secure::entries(&no_hooks)?.is_empty() {
            return Err(denied("Native Git hook-disable directory must be empty"));
        }
        let ownership = locks::acquire_run(
            &target,
            &locks::Options {
                run_id: Some(session.to_owned()),
                ..locks::Options::default()
            },
            Some(&token),
        )
        .await?;
        let mut lease = Self {
            root,
            root_id,
            marker_id: marker_info.identity,
            marker,
            host,
            redaction,
            token,
            ownership,
            identity: Arc::new(()),
            uncertain: Cell::new(false),
            no_hooks,
            images,
        };
        let prepared = async {
            secure::private_directory(&lease.no_hooks, true)?;
            lease.check()?;
            let owner = Owner::PrePlan(&lease);
            let location = tools::bytes_text(
                &owner
                    .git(
                        &lease.root,
                        &["rev-parse", "--show-toplevel"].map(str::to_owned),
                        false,
                    )
                    .await?,
            )?;
            let resolved = std::fs::canonicalize(Path::new(location.trim())).map_err(|error| {
                secure::io_error("Resolve pre-plan root", Path::new(location.trim()), error)
            })?;
            if paths::file_information(&resolved)?.identity != lease.root_id {
                return Err(denied(
                    "Pre-plan lease requires the actual Git repository worktree root",
                ));
            }
            owner
                .git(
                    &lease.root,
                    &[
                        "check-ignore",
                        "--no-index",
                        "--quiet",
                        "--",
                        ".roster/locks/run.lock",
                    ]
                    .map(str::to_owned),
                    false,
                )
                .await?;
            lease.check()
        }
        .await;
        if let Err(error) = prepared {
            return locks::finish(Err(error), lease.close().await);
        }
        Ok(lease)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn ownership(&self) -> &locks::RepoLock {
        &self.ownership
    }
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
    pub(crate) fn host(&self) -> &tools::Host {
        &self.host
    }
    pub(crate) fn redaction(&self) -> &redaction::Config {
        &self.redaction
    }
    pub(crate) fn images(&self) -> &[crate::host::Image] {
        &self.images
    }
    pub(crate) fn writer_identity(&self) -> Arc<()> {
        self.identity.clone()
    }

    pub(crate) fn storage_check(&self) -> Result<(), Error> {
        self.ownership.check()?;
        for image in &self.images {
            image.require_current()?;
        }
        secure::check_components(&self.root)?;
        let git = self.root.join(".git");
        secure::check_components(&git)?;
        if paths::file_information(&self.root)?.identity != self.root_id
            || paths::file_information(&git)?.identity != self.marker_id
            || !secure::entries(&self.no_hooks)?.is_empty()
        {
            return Err(denied(
                "Pre-plan lease root, Git marker or hook-disable identity changed",
            ));
        }
        if let Some(expected) = &self.marker {
            if paths::file_information(&git)?.links != 1
                || !secure::unchanged(&git, expected, 65_536)?
            {
                return Err(denied("Pre-plan Git marker bytes changed"));
            }
        }
        Ok(())
    }

    pub fn check(&self) -> Result<(), Error> {
        self.storage_check()?;
        self.token.check()?;
        if self.uncertain.get() {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Pre-plan writer retains uncertain descendant ownership",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        if self.uncertain.get() {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Pre-plan lease release refused while descendant shutdown is unverified",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        self.ownership.release_required().await
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Owner<'a> {
    Execution(&'a tools::Boundary),
    PrePlan(&'a RootLease),
}

struct PendingOwner<'a> {
    owner: Owner<'a>,
    settled: bool,
}

struct RunControl<'a> {
    cleanup: bool,
    submission: Option<&'a owned::Submission>,
}

impl Drop for PendingOwner<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.owner.quarantine();
        }
    }
}

#[cfg(all(test, windows))]
pub(super) fn input_test_pending_owner(owner: Owner<'_>) -> impl Drop + '_ {
    PendingOwner {
        owner,
        settled: false,
    }
}

impl<'a> Owner<'a> {
    pub(crate) fn span(self, label: timing::Label) -> Option<timing::Span> {
        match self {
            Self::Execution(boundary) => boundary.span(label),
            Self::PrePlan(_) => None,
        }
    }

    pub(crate) fn root(self) -> &'a Path {
        match self {
            Self::Execution(boundary) => boundary.root(),
            Self::PrePlan(lease) => &lease.root,
        }
    }
    pub(crate) fn token(self) -> &'a CancellationToken {
        match self {
            Self::Execution(boundary) => boundary.token(),
            Self::PrePlan(lease) => &lease.token,
        }
    }
    pub(crate) fn host(self) -> &'a tools::Host {
        match self {
            Self::Execution(boundary) => boundary.host(),
            Self::PrePlan(lease) => &lease.host,
        }
    }
    pub(crate) fn redaction(self) -> &'a redaction::Config {
        match self {
            Self::Execution(boundary) => boundary.redaction(),
            Self::PrePlan(lease) => &lease.redaction,
        }
    }
    pub(crate) fn check(self) -> Result<(), Error> {
        let scope = self.span(timing::Label::ProofOwnerCheck);
        let result = match self {
            Self::Execution(boundary) => boundary.check(),
            Self::PrePlan(lease) => lease.check(),
        };
        timing::finish(scope, result.as_ref().err());
        result
    }
    pub(crate) fn storage_check(self) -> Result<(), Error> {
        let scope = self.span(timing::Label::ProofOwnerCheck);
        let result = match self {
            Self::Execution(boundary) => boundary.check_diagnostic_authority(),
            Self::PrePlan(lease) => lease.storage_check(),
        };
        timing::finish(scope, result.as_ref().err());
        result
    }
    pub(crate) fn cleanup_check(self) -> Result<(), Error> {
        let scope = self.span(timing::Label::ProofOwnerCheck);
        let result = match self {
            Self::Execution(boundary) => boundary.check_cleanup_ownership(),
            Self::PrePlan(lease) => lease.storage_check(),
        };
        timing::finish(scope, result.as_ref().err());
        result
    }
    pub(crate) fn identity(self) -> Arc<()> {
        match self {
            Self::Execution(boundary) => boundary.writer_identity(),
            Self::PrePlan(lease) => lease.identity.clone(),
        }
    }
    pub(crate) fn authority(self) -> String {
        match self {
            Self::Execution(boundary) => boundary.authority_digest(),
            Self::PrePlan(lease) => format!("pre-plan:{}", lease.ownership.acquired_at),
        }
    }
    pub(crate) fn quarantine(self) {
        match self {
            Self::Execution(boundary) => boundary.quarantine_owned(),
            Self::PrePlan(lease) => lease.uncertain.set(true),
        }
    }
    pub(crate) fn cleanup_certain(self) -> bool {
        match self {
            Self::Execution(boundary) => boundary.owned_cleanup_certain(),
            Self::PrePlan(lease) => !lease.uncertain.get(),
        }
    }
    fn no_hooks(self) -> Result<PathBuf, Error> {
        match self {
            Self::PrePlan(lease) => Ok(lease.no_hooks.clone()),
            Self::Execution(boundary) => {
                Ok(
                    locks::target("run", boundary.root(), boundary.root(), None)?
                        .parent()
                        .ok_or_else(|| denied("Run lock needs a parent"))?
                        .join("no-hooks"),
                )
            }
        }
    }

    pub(crate) async fn git(
        self,
        directory: &Path,
        args: &[String],
        cleanup: bool,
    ) -> Result<Vec<u8>, Error> {
        let label = match args.first().map(String::as_str) {
            Some("rev-parse") if args.iter().any(|arg| arg == "HEAD") => {
                timing::Label::ProofGitHead
            }
            Some("diff") if args.iter().any(|arg| arg == "--cached") => {
                timing::Label::ProofGitIndex
            }
            Some("show-ref") => timing::Label::ProofGitRefs,
            Some("config") => timing::Label::ProofGitConfig,
            Some("ls-tree") => timing::Label::ProofGitTree,
            Some("cat-file") => timing::Label::ProofGitBlob,
            Some("clone") => timing::Label::ProofGitClone,
            _ => timing::Label::ProofGitOther,
        };
        let _scope = self.span(label);
        if cleanup {
            self.storage_check()?;
        } else {
            self.check()?;
        }
        let mut scoped = if args.first().is_some_and(|arg| arg == "check-ignore") {
            Vec::new()
        } else {
            vec!["-c".to_owned(), "core.longpaths=true".to_owned()]
        };
        scoped.extend_from_slice(args);
        let mut command =
            self.host()
                .git_command(directory, &self.no_hooks()?, &scoped, &BTreeMap::new());
        let limits = Limits {
            timeout: Some(Duration::from_secs(30)),
            max_output_bytes: MAX_FILE_BYTES as usize,
        };
        let token = if cleanup { None } else { Some(self.token()) };
        let command_scope = self.span(timing::Label::ProofGitExecute);
        let mut pending = PendingOwner {
            owner: self,
            settled: false,
        };
        let result =
            if self.host().git.is_absolute() || args.first().is_some_and(|arg| arg == "clone") {
                Box::pin(owned::execute_observed_timed(
                    command,
                    limits,
                    token,
                    None,
                    command_scope.as_ref(),
                ))
                .await
            } else {
                Box::pin(crate::process::execute_timed(
                    &mut command,
                    limits,
                    token,
                    command_scope.as_ref(),
                ))
                .await
            };
        pending.settled = true;
        if let Some(scope) = command_scope {
            scope.finish_status(result.is_ok());
        }
        match result {
            Ok(output) => {
                if cleanup {
                    self.storage_check()?;
                } else {
                    self.check()?;
                }
                Ok(output.captured.stdout)
            }
            Err(failure) => {
                if !failure.cleanup_complete || matches!(failure.kind, FailureKind::Exit(None)) {
                    self.quarantine();
                }
                if !cleanup {
                    self.token().check()?;
                }
                let code = if !failure.cleanup_complete {
                    "ROSTER_PROCESS_CLEANUP"
                } else {
                    failure.code().unwrap_or("ROSTER_GIT_FAILED")
                };
                let diagnostic = Text::from(
                    std::str::from_utf8(&failure.captured.stderr)
                        .unwrap_or("non-UTF8 Git diagnostic"),
                );
                let diagnostic = crate::planning::memory::redact(&diagnostic, self.redaction());
                let diagnostic = one_line(&diagnostic, 1200).to_utf8()?;
                Err(Error::new(
                    "NativeGitError",
                    format!(
                        "Owned Git {} failed: {failure}: {diagnostic}",
                        args.first().map_or("operation", String::as_str)
                    ),
                )
                .coded(code))
            }
        }
    }

    pub(crate) async fn git_blob_chunk(
        self,
        directory: &Path,
        objects: &[blob::Object],
    ) -> Result<blob::Output, Error> {
        let _scope = self.span(timing::Label::ProofGitBlobBatch);
        if !self.host().git.is_absolute() {
            return Err(unsupported(
                "Blob batching requires an absolute approved Git image",
            ));
        }
        let (request, maximum) = blob::request(objects)?;
        let input = owned::FiniteInput::try_from(request)?;
        self.check()?;
        for _ in objects {
            timing::finish(self.span(timing::Label::ProofBlobRequested), None);
        }
        let command = self.host().git_command(
            directory,
            &self.no_hooks()?,
            &["-c", "core.longpaths=true", "cat-file", "--batch"].map(str::to_owned),
            &BTreeMap::new(),
        );
        let submission = owned::Submission::default();
        let command_scope = self.span(timing::Label::ProofGitExecute);
        let mut pending = PendingOwner {
            owner: self,
            settled: false,
        };
        let observed = Box::pin(owned::execute_with_input(
            command,
            Limits {
                timeout: Some(Duration::from_secs(30)),
                max_output_bytes: maximum,
            },
            Some(self.token()),
            Some(&submission),
            Some(input),
        ))
        .await;
        pending.settled = true;
        if let Some(scope) = command_scope {
            scope.finish_status(observed.is_ok());
        }
        match observed {
            Ok(output) => {
                self.check()?;
                blob::Output::parse(output.captured.stdout, objects)
            }
            Err(failure) => {
                if !failure.cleanup_complete || matches!(failure.kind, FailureKind::Exit(None)) {
                    self.quarantine();
                }
                let facts = Error::new(
                    "NativeBlobFailureFacts",
                    format!(
                        "submitted={}; cleanup_complete={}; child_pid_present={}",
                        submission.attempted(),
                        failure.cleanup_complete,
                        failure.captured.pid.is_some()
                    ),
                );
                if let Err(error) = self.token().check() {
                    return Err(error.caused_by(facts));
                }
                let code = if failure.cleanup_complete {
                    failure.code().unwrap_or("ROSTER_GIT_FAILED")
                } else {
                    "ROSTER_PROCESS_CLEANUP"
                };
                let diagnostic = Text::from(
                    std::str::from_utf8(&failure.captured.stderr)
                        .unwrap_or("non-UTF8 Git diagnostic"),
                );
                let diagnostic = crate::planning::memory::redact(&diagnostic, self.redaction());
                Err(Error::new(
                    "NativeGitError",
                    format!(
                        "Owned Git blob batch failed: {failure}: {}",
                        one_line(&diagnostic, 1200).to_utf8()?
                    ),
                )
                .coded(code)
                .caused_by(facts))
            }
        }
    }

    pub(crate) async fn run(
        self,
        directory: &Path,
        executable: &Path,
        args: &[String],
        environment: &BTreeMap<String, String>,
        timeout_ms: u64,
        maximum: usize,
    ) -> Result<Run, Error> {
        self.run_inner(
            directory,
            executable,
            args,
            environment,
            Limits {
                timeout: Some(Duration::from_millis(timeout_ms)),
                max_output_bytes: maximum,
            },
            RunControl {
                cleanup: false,
                submission: None,
            },
        )
        .await
    }

    pub(crate) async fn run_tracked(
        self,
        directory: &Path,
        executable: &Path,
        args: &[String],
        environment: &BTreeMap<String, String>,
        limits: Limits,
        submission: &owned::Submission,
    ) -> Result<Run, Error> {
        self.run_inner(
            directory,
            executable,
            args,
            environment,
            limits,
            RunControl {
                cleanup: false,
                submission: Some(submission),
            },
        )
        .await
    }

    pub(crate) async fn cleanup_run(
        self,
        directory: &Path,
        executable: &Path,
        args: &[String],
        environment: &BTreeMap<String, String>,
        timeout_ms: u64,
        maximum: usize,
    ) -> Result<Run, Error> {
        self.run_inner(
            directory,
            executable,
            args,
            environment,
            Limits {
                timeout: Some(Duration::from_millis(timeout_ms)),
                max_output_bytes: maximum,
            },
            RunControl {
                cleanup: true,
                submission: None,
            },
        )
        .await
    }

    async fn run_inner(
        self,
        directory: &Path,
        executable: &Path,
        args: &[String],
        environment: &BTreeMap<String, String>,
        limits: Limits,
        control: RunControl<'_>,
    ) -> Result<Run, Error> {
        let cleanup = control.cleanup;
        if cleanup {
            self.cleanup_check()?;
        } else {
            self.check()?;
        }
        let image_scope = self.span(timing::Label::ProofImagePlan);
        let approved = executable;
        let selected = paths::file_information(approved)?;
        let normalized = paths::strip_verbatim(executable.to_path_buf());
        let executable = normalized.as_path();
        secure::check_components(executable)?;
        let image = paths::file_information(executable)?;
        if selected.identity != image.identity {
            return Err(denied(
                "Approved and checked backend executable paths name different physical images",
            ));
        }
        let before = std::fs::metadata(executable).map_err(|error| {
            secure::io_error("Inspect approved backend executable", executable, error)
        })?;
        let modified = before.modified().map_err(|error| {
            secure::io_error("Read backend image modification time", executable, error)
        })?;
        if !before.is_file() {
            return Err(denied(
                "Backend executable must be a regular approved image",
            ));
        }
        let command = self
            .host()
            .command(directory, executable, args, environment);
        timing::finish(image_scope, None);
        let command_scope = self.span(timing::Label::ProofCommandExecute);
        let mut pending = PendingOwner {
            owner: self,
            settled: false,
        };
        let token = if cleanup { None } else { Some(self.token()) };
        let observed = owned::execute_observed_timed(
            command,
            limits,
            token,
            control.submission,
            command_scope.as_ref(),
        )
        .await;
        pending.settled = true;
        if let Some(scope) = command_scope {
            scope.finish_status(observed.is_ok());
        }
        let run = match observed {
            Ok(output) => Run {
                exit: Some(0),
                stdout: output.captured.stdout,
                stderr: output.captured.stderr,
                timed_out: false,
                overflow: false,
            },
            Err(failure) => {
                if !failure.cleanup_complete || matches!(failure.kind, FailureKind::Exit(None)) {
                    self.quarantine();
                }
                if let FailureKind::Cancelled(reason) = failure.kind {
                    return Err(reason.error());
                }
                if !failure.cleanup_complete {
                    return Err(Error::new("OwnedProcessCleanupError", failure.to_string())
                        .coded("ROSTER_PROCESS_CLEANUP"));
                }
                match failure.kind {
                    FailureKind::Exit(Some(exit)) => Run {
                        exit: Some(exit),
                        stdout: failure.captured.stdout,
                        stderr: failure.captured.stderr,
                        timed_out: false,
                        overflow: false,
                    },
                    FailureKind::Timeout => Run {
                        exit: None,
                        stdout: failure.captured.stdout,
                        stderr: failure.captured.stderr,
                        timed_out: true,
                        overflow: false,
                    },
                    FailureKind::OutputLimit => Run {
                        exit: None,
                        stdout: failure.captured.stdout,
                        stderr: failure.captured.stderr,
                        timed_out: false,
                        overflow: true,
                    },
                    _ => {
                        return Err(Error::new("BackendUnavailable", failure.to_string())
                            .coded(failure.code().unwrap_or("ROSTER_NATIVE_UNSUPPORTED")));
                    }
                }
            }
        };
        if cleanup {
            self.cleanup_check()?;
        } else {
            self.check()?;
        }
        let image_scope = self.span(timing::Label::ProofImagePlan);
        let after = std::fs::metadata(executable)
            .map_err(|error| secure::io_error("Recheck backend executable", executable, error))?;
        if paths::file_information(approved)?.identity != image.identity
            || paths::file_information(executable)?.identity != image.identity
            || before.len() != after.len()
            || modified
                != after.modified().map_err(|error| {
                    secure::io_error("Recheck backend modification time", executable, error)
                })?
        {
            return Err(denied("Approved backend image changed during execution"));
        }
        timing::finish(image_scope, None);
        Ok(run)
    }
}

pub(crate) struct Run {
    pub(crate) exit: Option<i32>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) timed_out: bool,
    pub(crate) overflow: bool,
}

pub(crate) fn finish_owned<T>(
    owner: Owner<'_>,
    result: Result<T, Error>,
    cleanup: Result<(), Error>,
) -> Result<T, Error> {
    if cleanup.is_err() {
        owner.quarantine();
    }
    locks::finish(result, cleanup)
}

impl Run {
    pub(crate) fn success(&self) -> bool {
        self.exit == Some(0) && !self.timed_out && !self.overflow
    }
    fn decode(&self, bytes: &[u8]) -> Result<String, Error> {
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(text.to_owned()),
            Err(error) if self.overflow => Ok(std::str::from_utf8(
                bytes
                    .get(..error.valid_up_to())
                    .ok_or_else(|| Error::type_error("Invalid bounded output prefix"))?,
            )
            .map_err(|_| Error::type_error("Invalid bounded UTF8 output prefix"))?
            .to_owned()),
            Err(_) => Err(Error::type_error(
                "Native backend output must be valid UTF-8",
            )),
        }
    }
    pub(crate) fn stdout_text(&self) -> Result<Text, Error> {
        Ok(Text::from(self.decode(&self.stdout)?.as_str()))
    }
    pub(crate) fn text(&self) -> Result<Text, Error> {
        let mut text = self.decode(&self.stdout)?;
        if !self.stderr.is_empty() {
            text.push('\n');
            text.push_str(&self.decode(&self.stderr)?);
        }
        Ok(Text::from(text.as_str()))
    }
}

pub(crate) struct Stamp {
    identity: Arc<()>,
    authority: String,
    snapshot: evidence::Snapshot,
    git: Value,
}

fn signature_head(refs: &[u8]) -> Result<Vec<u8>, Error> {
    if !u64::try_from(refs.len()).is_ok_and(|length| length <= MAX_FILE_BYTES) {
        return Err(denied(
            "Git signature refs observation exceeds its byte bound",
        ));
    }
    let mut head = None;
    for record in refs.split_inclusive(|byte| *byte == b'\n') {
        let (line, terminated) = match record.strip_suffix(b"\n") {
            Some(line) => (line, true),
            None => (record, false),
        };
        let Some(separator) = line.iter().position(|byte| *byte == b' ') else {
            continue;
        };
        let (object, name) = line.split_at(separator);
        if name.strip_prefix(b" ") != Some(b"HEAD".as_slice()) {
            continue;
        }
        if head.is_some() {
            return Err(denied(
                "Git signature refs observation has duplicate HEAD entries",
            ));
        }
        if !terminated
            || ![40, 64].contains(&object.len())
            || !object
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(denied(
                "Git signature refs observation has a malformed HEAD entry",
            ));
        }
        let mut bytes = object.to_vec();
        bytes.push(b'\n');
        head = Some(bytes);
    }
    head.ok_or_else(|| denied("Git signature refs observation has no HEAD entry"))
}

pub(crate) async fn git_signature(owner: Owner<'_>, directory: &Path) -> Result<Value, Error> {
    let index = crate::identity::sha256_hex(
        &owner
            .git(
                directory,
                &[
                    "diff",
                    "--cached",
                    "--raw",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-z",
                ]
                .map(str::to_owned),
                false,
            )
            .await?,
    );
    let refs = owner
        .git(directory, &["show-ref", "--head"].map(str::to_owned), false)
        .await?;
    let scope = owner.span(timing::Label::ProofSignatureHead);
    let head = signature_head(&refs);
    timing::finish(scope, head.as_ref().err());
    let head = crate::identity::sha256_hex(&head?);
    let refs_hash = crate::identity::sha256_hex(&refs);
    drop(refs);
    let config = crate::identity::sha256_hex(
        &owner
            .git(
                directory,
                &["config", "--local", "--null", "--list"].map(str::to_owned),
                false,
            )
            .await?,
    );
    Ok(Value::Array(vec![
        Value::string(&head),
        Value::string(&index),
        Value::string(&refs_hash),
        Value::string(&config),
    ]))
}

impl Stamp {
    pub(crate) async fn capture(owner: Owner<'_>) -> Result<Self, Error> {
        let _scope = owner.span(timing::Label::ProofStampCapture);
        owner.check()?;
        let source_scope = owner.span(timing::Label::ProofSource);
        let snapshot = evidence::capture(owner.root(), None, &evidence::Projection::default());
        timing::finish(source_scope, snapshot.as_ref().err());
        let snapshot = snapshot?;
        let git = git_signature(owner, owner.root()).await?;
        Ok(Self {
            identity: owner.identity(),
            authority: owner.authority(),
            snapshot,
            git,
        })
    }
    pub(crate) async fn require_current(&self, owner: Owner<'_>) -> Result<(), Error> {
        let _scope = owner.span(timing::Label::ProofStampCurrent);
        let current = Self::capture(owner).await?;
        let compare_scope = owner.span(timing::Label::ProofCompare);
        if !Arc::ptr_eq(&self.identity, &current.identity)
            || self.authority != current.authority
            || !self.snapshot.same_source(&current.snapshot)
            || self.git != current.git
        {
            return Err(denied(
                "Backend receipt no longer matches its current writer, Git state and source",
            ));
        }
        timing::finish(compare_scope, None);
        owner.check()
    }
}

#[cfg(test)]
mod signature_parser_tests {
    use super::*;

    #[test]
    fn signature_parser_derives_exact_head_bytes_for_both_hashes() -> Result<(), Error> {
        for width in [40, 64] {
            let object = "a".repeat(width);
            let refs = format!("{object} HEAD\n{object} refs/heads/topic\n");
            assert_eq!(
                signature_head(refs.as_bytes())?,
                format!("{object}\n").as_bytes()
            );
        }
        Ok(())
    }

    #[test]
    fn signature_parser_matches_only_exact_pseudoentry_and_preserves_opaque_refs()
    -> Result<(), Error> {
        let object = "a".repeat(40);
        let mut refs = format!(
                "{object} refs/tags/HEAD\n{object} refs/heads/HEAD\n{object} refs/heads/aHEAD\n{object} HEAD^{{}}\n"
            )
            .into_bytes();
        refs.extend_from_slice(format!("{object} refs/tags/").as_bytes());
        refs.extend_from_slice(&[0xff, 0xfe, b'\n']);
        refs.extend_from_slice(format!("{object} HEAD\n").as_bytes());
        let original = refs.clone();
        assert_eq!(signature_head(&refs)?, format!("{object}\n").as_bytes());
        assert_eq!(refs, original);
        Ok(())
    }

    #[test]
    fn signature_parser_rejects_missing_and_duplicate_head() {
        let object = "a".repeat(40);
        for refs in [
            Vec::new(),
            format!("{object} refs/tags/HEAD\n").into_bytes(),
            format!("{object} HEAD^{{}}\n").into_bytes(),
            format!("{object} HEAD\n{object} HEAD\n").into_bytes(),
            format!("{object} HEAD\n{} HEAD\n", "b".repeat(64)).into_bytes(),
        ] {
            assert!(signature_head(&refs).is_err_and(|error| {
                error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
            }));
        }
    }

    #[test]
    fn signature_parser_rejects_malformed_raw_object_ids() {
        for object in [
            String::new(),
            "a".repeat(39),
            "a".repeat(41),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(40),
            "g".repeat(40),
            format!("{}\0", "a".repeat(39)),
        ] {
            assert!(signature_head(format!("{object} HEAD\n").as_bytes()).is_err());
        }
    }

    #[test]
    fn signature_parser_rejects_malformed_head_record_delimiters() {
        let object = "a".repeat(40);
        for refs in [
            format!("{object} HEAD"),
            format!("{object}\tHEAD\n"),
            format!("{object}  HEAD\n"),
            format!(" {object} HEAD\n"),
            format!("{object} HEAD\r\n"),
            format!("{object} HEAD extra\n"),
        ] {
            assert!(signature_head(refs.as_bytes()).is_err_and(|error| {
                error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
            }));
        }
    }

    #[test]
    fn signature_parser_rejects_oversized_observations() -> Result<(), Error> {
        let length = usize::try_from(MAX_FILE_BYTES)
            .ok()
            .and_then(|length| length.checked_add(1))
            .ok_or_else(|| Error::type_error("Owned signature bound must fit usize"))?;
        assert!(signature_head(&vec![0; length]).is_err_and(|error| {
            error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
        }));
        Ok(())
    }
}
