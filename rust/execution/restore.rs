use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::identity::sha256_hex;
use crate::paths::{self, FileIdentity};
use crate::state::{files, secure};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publication {
    Unknown,
    Published,
    Unpublished,
}

pub trait PublicationCheck {
    fn current(&mut self, subject: &Value) -> impl Future<Output = Result<Publication, Error>>;
}

async fn require_current_unpublished<P: PublicationCheck>(
    boundary: &tools::Boundary,
    subject: &Value,
    publication: &mut P,
) -> Result<(), Error> {
    boundary.check()?;
    let current = tokio::select! {
        result = publication.current(subject) => result,
        reason = boundary.token().cancelled() => return Err(reason?.error()),
        () = tokio::time::sleep(Duration::from_secs(30)) => return Err(Error::new("RestoreApprovalError", "Current host publication check exceeded its bounded deadline; publication is unknown").coded("ROSTER_RESTORE_APPROVAL")),
    }?;
    boundary.check()?;
    if current != Publication::Unpublished {
        return Err(Error::new(
            "RestoreApprovalError",
            "Current host publication is published or unknown; restoration was not authorized",
        )
        .coded("ROSTER_RESTORE_APPROVAL"));
    }
    Ok(())
}

struct Product {
    before: Option<secure::Snapshot>,
    after: Option<(Vec<u8>, String)>,
}

pub struct Plan {
    record: Value,
    validation: Value,
    snapshot: tools::Snapshot,
    audit: BTreeMap<String, String>,
    root: PathBuf,
    root_identity: FileIdentity,
    common: PathBuf,
    common_identity: FileIdentity,
    writer: Arc<()>,
    authority: String,
    checkpoint_file: PathBuf,
    checkpoint_snapshot: secure::Snapshot,
    index: PathBuf,
    index_snapshot: Option<secure::Snapshot>,
    products: BTreeMap<String, Product>,
    parents: BTreeMap<PathBuf, FileIdentity>,
    archive: archive::Plan,
}

pub struct Approval {
    plan: Plan,
}

pub struct Report {
    value: Value,
}

pub(crate) struct ResumeRequirements {
    pub generation: u64,
    pub fresh: bool,
    pub failures: Option<(i32, i32)>,
}

fn physical_identity(identity: FileIdentity) -> Value {
    Value::Array(vec![
        Value::string(&identity.device.to_string()),
        Value::string(&identity.index.to_string()),
    ])
}

fn marker(path: &Path, expected: &[u8]) -> Result<bool, Error> {
    let Some(snapshot) = secure::read(path, 256)? else {
        return Ok(false);
    };
    if snapshot.bytes != expected
        || paths::file_information(path)?.links != 1
        || !secure::unchanged(path, &snapshot, 256)?
    {
        return Err(denied("Restore completion marker is malformed or changed"));
    }
    Ok(true)
}

pub(crate) fn resume_requirements(boundary: &tools::Boundary) -> Result<ResumeRequirements, Error> {
    boundary.check()?;
    let task = boundary
        .assignment()
        .task
        .strip_prefix("issue-")
        .unwrap_or(&boundary.assignment().task);
    let root = boundary.state_directory(&["checkpoints", task])?;
    let mut requirements = ResumeRequirements {
        generation: 0,
        fresh: false,
        failures: None,
    };
    if !secure::private_directory_if_present(&root)? {
        return Ok(requirements);
    }
    let entries = secure::entries(&root)?;
    if entries.len() > MAX_FILES {
        return Err(denied("Restore recovery listing exceeds its bound"));
    }
    let pattern = regex(r"^restore-[a-f0-9]{16}$")?;
    let mut histories = Vec::new();
    for name in entries.into_iter().filter(|name| pattern.is_match(name)) {
        let directory = root.join(name);
        secure::private_directory(&directory, false)?;
        let path = directory.join("transaction.json");
        let snapshot = secure::read(&path, MAX_FILE_BYTES)?.ok_or_else(|| Error::new("RestoreRecoveryError", "An incomplete restore transaction has no completed journal; explicit operator reconciliation is required").coded("ROSTER_RESTORE_RECOVERY"))?;
        let journal =
            crate::json::parse(&Text::from(tools::bytes_text(&snapshot.bytes)?.as_str()))?;
        if paths::file_information(&path)?.links != 1
            || !secure::unchanged(&path, &snapshot, MAX_FILE_BYTES)?
            || journal.get("schema_version") != Some(&Value::Number(1.0))
            || !domain::is_text(journal.get("task"), &boundary.assignment().task)
            || journal.get("worktree_identity")
                != Some(&physical_identity(
                    paths::file_information(boundary.root())?.identity,
                ))
            || journal.get("directory_identity")
                != Some(&physical_identity(
                    paths::file_information(&directory)?.identity,
                ))
        {
            return Err(Error::new("RestoreRecoveryError", "Restore journal identity or schema changed; preserve it for explicit operator reconciliation").coded("ROSTER_RESTORE_RECOVERY"));
        }
        let committed = marker(
            &directory.join("COMMITTED"),
            b"restored candidate; fresh verification required\n",
        )?;
        let rolled_back = marker(
            &directory.join("ROLLED-BACK"),
            b"owned product state restored; fresh verification required\n",
        )?;
        let reconciled = marker(
            &directory.join("RECONCILED"),
            b"owned source and artifact rollback reconciled; fresh verification required\n",
        )?;
        if (rolled_back || !committed) && !reconciled {
            return Err(Error::new("RestoreRecoveryError", "Restore transaction is not committed or fully reconciled; retain source backups, artifacts and the writer for explicit recovery").coded("ROSTER_RESTORE_RECOVERY"));
        }
        requirements.fresh = true;
        let generation = domain::safe_integer(journal.get("generation"))
            .and_then(|value| value.checked_add(1))
            .filter(|value| *value <= crate::MAX_SAFE_INTEGER)
            .ok_or_else(|| denied("Restore journal generation is invalid"))?;
        requirements.generation = requirements.generation.max(generation);
        let time = text(
            required(&journal, "time")?,
            "Restore journal time is missing",
        )?;
        if !regex(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$")?.is_match(&time) {
            return Err(denied("Restore journal time is invalid"));
        }
        if journal.get("full_failures") != Some(&Value::Null) {
            let failures = journal
                .get("full_failures")
                .and_then(domain::items)
                .filter(|values| values.len() == 2)
                .ok_or_else(|| denied("Restore full-failure history is malformed"))?;
            let mut exits = Vec::new();
            for value in failures {
                let value = value
                    .as_number()
                    .filter(|value| {
                        value.is_finite()
                            && value.fract() == 0.0
                            && *value >= f64::from(i32::MIN)
                            && *value <= f64::from(i32::MAX)
                            && *value != 0.0
                    })
                    .ok_or_else(|| denied("Restore full-failure exit is invalid"))?;
                exits.push(value as i32);
            }
            histories.push((
                time,
                *exits
                    .first()
                    .ok_or_else(|| denied("Restore original exit is missing"))?,
                *exits
                    .get(1)
                    .ok_or_else(|| denied("Restore latest failed exit is missing"))?,
            ));
        }
    }
    histories.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some(first) = histories.first() {
        let last = histories
            .last()
            .ok_or_else(|| denied("Restore failure history is incomplete"))?;
        requirements.failures = Some((first.1, last.2));
    }
    boundary.check()?;
    Ok(requirements)
}

impl Report {
    pub fn as_value(&self) -> &Value {
        &self.value
    }
}

fn equal_snapshot(
    path: &Path,
    expected: Option<&secure::Snapshot>,
    maximum: u64,
) -> Result<bool, Error> {
    match (expected, secure::read(path, maximum)?) {
        (None, None) => Ok(true),
        (Some(expected), Some(actual)) => Ok(expected.identity == actual.identity
            && expected.bytes == actual.bytes
            && paths::file_information(path)?.links == 1),
        _ => Ok(false),
    }
}

fn task_branch(boundary: &tools::Boundary, validation: &Value) -> Result<(), Error> {
    let task = &boundary.assignment().task;
    if !regex(r"^issue-[1-9][0-9]*$")?.is_match(task) {
        return Err(denied("Verified restore requires an isolated issue task"));
    }
    let number = task
        .strip_prefix("issue-")
        .ok_or_else(|| denied("Restore issue identity is missing"))?;
    let branch = text(
        required(validation, "branch")?,
        "Restore branch must be text",
    )?;
    let prefix = format!("agents/{number}");
    let attempt = branch.strip_prefix(&format!("{prefix}-attempt-"));
    if branch != prefix && attempt.is_none_or(|attempt| !["1", "2", "3"].contains(&attempt)) {
        return Err(denied(
            "Verified restore requires the unchanged isolated agents issue branch",
        ));
    }
    Ok(())
}

fn permitted(boundary: &tools::Boundary, path: &str, scope: &[String]) -> Result<(), Error> {
    if boundary.checked_product_write_path(path)? != path
        || !crate::planning::is_allowed(path, scope)
    {
        return Err(denied(
            "Restore products must stay inside exact current unprotected task scope",
        ));
    }
    Ok(())
}

fn parent_identities(
    root: &Path,
    products: &BTreeMap<String, Product>,
) -> Result<BTreeMap<PathBuf, FileIdentity>, Error> {
    let mut parents = BTreeMap::new();
    for name in products.keys() {
        let mut parent = root.join(name).parent().map(Path::to_path_buf);
        while let Some(path) = parent {
            if path != root {
                paths::ensure_local_path(&path, root)?;
            }
            secure::check_components(&path)?;
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() && !secure::linked(&metadata) => {
                    parents.insert(path.clone(), paths::file_information(&path)?.identity);
                }
                Ok(_) => {
                    return Err(denied(
                        "Restore parent paths must be real directories; file/directory transitions are not implicit",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(secure::io_error("Inspect restore parent", &path, error)),
            }
            if path == root {
                break;
            }
            parent = path.parent().map(Path::to_path_buf);
        }
    }
    Ok(parents)
}

fn check_parents(parents: &BTreeMap<PathBuf, FileIdentity>) -> Result<(), Error> {
    for (path, expected) in parents {
        secure::check_components(path)?;
        if paths::file_information(path)?.identity != *expected
            || !fs::symlink_metadata(path)
                .map_err(|error| secure::io_error("Recheck restore parent", path, error))?
                .is_dir()
        {
            return Err(denied(
                "Restore parent identity changed; preserve current user files",
            ));
        }
    }
    Ok(())
}

pub async fn prepare(boundary: &tools::Boundary, number: Option<u64>) -> Result<Plan, Error> {
    boundary.check()?;
    if boundary.role() != Role::Coder {
        return Err(denied(
            "Checkpoint restoration is a host-only operation on the owning coder boundary",
        ));
    }
    boundary.require_restore_capacity()?;
    let record = checkpoints::validate_green(boundary, number).await?;
    let validation = checkpoints::validation(boundary).await?;
    task_branch(boundary, &validation)?;
    let scope = boundary.scope()?;
    if scope.is_empty() || scope.len() > MAX_SCOPE_ENTRIES {
        return Err(denied(
            "Restore requires a bounded nonempty current task scope",
        ));
    }
    let owner = backend::Owner::Execution(boundary);
    let tree = text(
        required(&record, "tree")?,
        "Restore checkpoint requires a tree",
    )?;
    let entries = tools::bytes_text(
        &owner
            .git(
                boundary.root(),
                &["ls-tree".to_owned(), "-r".to_owned(), "-z".to_owned(), tree],
                false,
            )
            .await?,
    )?;
    let snapshot = boundary.snapshot()?;
    let audit = boundary.audit()?;
    let mut products = BTreeMap::new();
    let mut total = 0_u64;
    for name in snapshot.files.keys() {
        permitted(boundary, name, &scope)?;
        let path = boundary.root().join(name);
        let before = secure::read(&path, MAX_FILE_BYTES)?
            .ok_or_else(|| denied("Current restore product disappeared"))?;
        if paths::file_information(&path)?.links != 1 {
            return Err(denied("Restore products must be single-link files"));
        }
        archive::reject_secret(&before.bytes, boundary.redaction())?;
        total = total
            .checked_add(before.bytes.len() as u64)
            .filter(|size| *size <= MAX_SNAPSHOT_BYTES)
            .ok_or_else(|| denied("Restore snapshots exceed 128 MiB"))?;
        products.insert(
            name.clone(),
            Product {
                before: Some(before),
                after: None,
            },
        );
    }
    let mut names = BTreeSet::new();
    for entry in entries.split('\0').filter(|entry| !entry.is_empty()) {
        boundary.check()?;
        let (metadata, name) = entry
            .split_once('\t')
            .ok_or_else(|| denied("Restore Git tree entry is malformed"))?;
        let parts = metadata.split(' ').collect::<Vec<_>>();
        let mode = parts.first().copied().unwrap_or("");
        let blob = parts.get(2).copied().unwrap_or("");
        if parts.len() != 3
            || !["100644", "100755"].contains(&mode)
            || parts.get(1) != Some(&"blob")
            || !regex(r"^[a-f0-9]{40}(?:[a-f0-9]{24})?$")?.is_match(blob)
        {
            return Err(denied(
                "Restore trees may contain only bounded regular product blobs",
            ));
        }
        permitted(boundary, name, &scope)?;
        let key = if cfg!(windows) {
            name.to_lowercase()
        } else {
            name.to_owned()
        };
        if !names.insert(key) || names.len() > MAX_FILES {
            return Err(denied(
                "Restore tree has aliased or excessive product paths",
            ));
        }
        let bytes = owner
            .git(
                boundary.root(),
                &["cat-file".to_owned(), "blob".to_owned(), blob.to_owned()],
                false,
            )
            .await?;
        archive::reject_secret(&bytes, boundary.redaction())?;
        total = total
            .checked_add(bytes.len() as u64)
            .filter(|size| *size <= MAX_SNAPSHOT_BYTES)
            .ok_or_else(|| denied("Restore snapshots exceed 128 MiB"))?;
        let product = products.entry(name.to_owned()).or_insert(Product {
            before: None,
            after: None,
        });
        product.after = Some((bytes, mode.to_owned()));
    }
    if products.len() > MAX_FILES {
        return Err(denied("Restore union exceeds 2048 files"));
    }
    let paths = products.keys().collect::<Vec<_>>();
    for (index, name) in paths.iter().enumerate() {
        if paths.iter().skip(index + 1).any(|other| {
            other.starts_with(&format!("{name}/")) || name.starts_with(&format!("{other}/"))
        }) {
            return Err(denied(
                "Restore file/directory transitions require an explicit separate operation",
            ));
        }
    }
    let common = archive::common_directory(owner).await?;
    let index = tools::bytes_text(
        &owner
            .git(
                boundary.root(),
                &["rev-parse", "--git-path", "index"].map(str::to_owned),
                false,
            )
            .await?,
    )?;
    let index = paths::resolve(boundary.root(), Path::new(index.trim()))?;
    let index_snapshot = secure::read(&index, MAX_FILE_BYTES)?;
    if index_snapshot.is_some() && paths::file_information(&index)?.links != 1 {
        return Err(denied("Restore Git index must be single-link"));
    }
    let task = boundary
        .assignment()
        .task
        .strip_prefix("issue-")
        .ok_or_else(|| denied("Restore task identity disappeared"))?;
    let checkpoint_file = boundary.state_directory(&["checkpoints", task])?.join(
        domain::safe_integer(record.get("number"))
            .ok_or_else(|| denied("Restore checkpoint number missing"))?
            .to_string(),
    );
    let checkpoint_snapshot = secure::read(&checkpoint_file, 262_144)?
        .ok_or_else(|| denied("Restore checkpoint metadata disappeared"))?;
    let parents = parent_identities(boundary.root(), &products)?;
    let archive = archive::prepare(boundary, &archive::PRESERVE_AUTHORITY).await?;
    let plan = Plan {
        record,
        validation,
        snapshot,
        audit,
        root: boundary.root().to_path_buf(),
        root_identity: paths::file_information(boundary.root())?.identity,
        common_identity: paths::file_information(&common)?.identity,
        common,
        writer: boundary.writer_identity(),
        authority: boundary.authority_digest(),
        checkpoint_file,
        checkpoint_snapshot,
        index,
        index_snapshot,
        products,
        parents,
        archive,
    };
    plan.check(boundary).await?;
    Ok(plan)
}

impl Plan {
    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("checkpoint", self.record.clone()),
            (
                "files",
                Value::Array(
                    self.products
                        .iter()
                        .map(|(name, product)| {
                            Value::object(vec![
                                ("path", Value::string(name)),
                                (
                                    "before",
                                    product.before.as_ref().map_or(Value::Null, |snapshot| {
                                        Value::string(&sha256_hex(&snapshot.bytes))
                                    }),
                                ),
                                (
                                    "after",
                                    product.after.as_ref().map_or(Value::Null, |(bytes, _)| {
                                        Value::string(&sha256_hex(bytes))
                                    }),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("archive", self.archive.as_value()),
        ])
    }
    async fn check(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        boundary.check()?;
        if boundary.root() != self.root
            || paths::file_information(boundary.root())?.identity != self.root_identity
            || paths::file_information(&self.common)?.identity != self.common_identity
            || !Arc::ptr_eq(&self.writer, &boundary.writer_identity())
            || self.authority != boundary.authority_digest()
            || self.snapshot != boundary.snapshot()?
            || self.audit != boundary.audit()?
        {
            return Err(denied(
                "Restore approval is stale for the current writer, product snapshot or repository",
            ));
        }
        check_parents(&self.parents)?;
        self.archive.require_current(boundary).await?;
        if !equal_snapshot(&self.index, self.index_snapshot.as_ref(), MAX_FILE_BYTES)?
            || !equal_snapshot(
                &self.checkpoint_file,
                Some(&self.checkpoint_snapshot),
                262_144,
            )?
            || checkpoints::validation(boundary).await? != self.validation
            || checkpoints::validate_green(
                boundary,
                domain::safe_integer(self.record.get("number")),
            )
            .await?
                != self.record
        {
            return Err(denied(
                "Restore checkpoint, branch, ref, task or Git index changed after preflight",
            ));
        }
        Ok(())
    }
    pub async fn confirm(
        self,
        boundary: &tools::Boundary,
        publication: Publication,
        confirmed: bool,
    ) -> Result<Approval, Error> {
        self.check(boundary).await?;
        if publication != Publication::Unpublished || !confirmed {
            return Err(Error::new("RestoreApprovalError", "Restore requires explicit CURRENT host confirmation that this exact branch is unpublished; unknown publication is not false").coded("ROSTER_RESTORE_APPROVAL"));
        }
        Ok(Approval { plan: self })
    }
}

struct Entry {
    name: String,
    before: Option<secure::Snapshot>,
    staged: Option<secure::Snapshot>,
    staged_path: PathBuf,
    backup_path: PathBuf,
    backup: bool,
    removed: bool,
    installed: bool,
}

struct Transaction {
    directory: PathBuf,
    identity: FileIdentity,
    entries: Vec<Entry>,
    parents: BTreeMap<PathBuf, FileIdentity>,
    created: Vec<(PathBuf, FileIdentity)>,
}

fn mode(path: &Path, git_mode: &str) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if git_mode == "100755" { 0o755 } else { 0o644 }),
        )
        .map_err(|error| secure::io_error("Restore product mode", path, error))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, git_mode);
    }
    Ok(())
}

impl Transaction {
    fn check(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        boundary.check_cleanup_ownership()?;
        secure::private_directory(&self.directory, false)?;
        if paths::file_information(&self.directory)?.identity != self.identity {
            return Err(denied("Restore transaction directory changed identity"));
        }
        check_parents(&self.parents)
    }
    fn parents_for(&mut self, boundary: &tools::Boundary, name: &str) -> Result<(), Error> {
        let parent = boundary
            .root()
            .join(name)
            .parent()
            .ok_or_else(|| denied("Restore path requires a parent"))?
            .to_path_buf();
        let mut missing = Vec::new();
        let mut next = Some(parent);
        while let Some(path) = next {
            if path != boundary.root() {
                paths::ensure_local_path(&path, boundary.root())?;
            }
            secure::check_components(&path)?;
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() && !secure::linked(&metadata) => break,
                Ok(_) => return Err(denied("Restore parent became a non-directory or link")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(path.clone())
                }
                Err(error) => {
                    return Err(secure::io_error("Inspect new restore parent", &path, error));
                }
            }
            next = path.parent().map(Path::to_path_buf);
        }
        for path in missing.into_iter().rev() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder
                .create(&path)
                .map_err(|error| secure::io_error("Create owned restore parent", &path, error))?;
            let identity = paths::file_information(&path)?.identity;
            self.parents.insert(path.clone(), identity);
            self.created.push((path, identity));
        }
        Ok(())
    }
    fn rollback(&mut self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.check(boundary)?;
        for entry in self.entries.iter_mut().rev() {
            let target = boundary.root().join(&entry.name);
            secure::check_components(&target)?;
            if entry.installed {
                let staged = entry
                    .staged
                    .as_ref()
                    .ok_or_else(|| denied("Restore installed identity is missing"))?;
                let stage_present = secure::unchanged(&entry.staged_path, staged, MAX_FILE_BYTES)?;
                if paths::file_information(&target)?.links != if stage_present { 2 } else { 1 }
                    || !secure::remove_matching(&target, staged, MAX_FILE_BYTES)?
                {
                    return Err(denied("Restore rollback preserved a changed user product"));
                }
                entry.installed = false;
            }
            if entry.removed {
                if secure::read(&target, MAX_FILE_BYTES)?.is_some() {
                    return Err(denied(
                        "Restore rollback will not overwrite a new user file",
                    ));
                }
                let before = entry
                    .before
                    .as_ref()
                    .ok_or_else(|| denied("Restore original identity is missing"))?;
                if !secure::unchanged(&entry.backup_path, before, MAX_FILE_BYTES)? {
                    return Err(denied(
                        "Restore original backup changed; retain the transaction",
                    ));
                }
                fs::hard_link(&entry.backup_path, &target).map_err(|error| {
                    secure::io_error("Rollback original product", &target, error)
                })?;
                entry.removed = false;
            }
            if entry.backup {
                let before = entry
                    .before
                    .as_ref()
                    .ok_or_else(|| denied("Restore backup identity is missing"))?;
                if !secure::unchanged(&target, before, MAX_FILE_BYTES)?
                    || !secure::remove_matching(&entry.backup_path, before, MAX_FILE_BYTES)?
                {
                    return Err(denied(
                        "Restore rollback could not release its own backup link",
                    ));
                }
                entry.backup = false;
            }
            if let Some(staged) = &entry.staged {
                if secure::read(&entry.staged_path, MAX_FILE_BYTES)?.is_some()
                    && !secure::remove_matching(&entry.staged_path, staged, MAX_FILE_BYTES)?
                {
                    return Err(denied("Restore rollback staging identity changed"));
                }
            }
        }
        for (path, identity) in self.created.iter().rev() {
            if paths::file_information(path)?.identity != *identity
                || !secure::entries(path)?.is_empty()
            {
                return Err(denied(
                    "Restore rollback preserved a foreign directory or file",
                ));
            }
            fs::remove_dir(path).map_err(|error| {
                secure::io_error("Rollback owned restore directory", path, error)
            })?;
        }
        Ok(())
    }
}

pub async fn execute<P: PublicationCheck>(
    boundary: &mut tools::Boundary,
    approval: Approval,
    publication: &mut P,
) -> Result<Report, Error> {
    approval.plan.check(boundary).await?;
    let publication_subject = approval.plan.as_value();
    require_current_unpublished(boundary, &publication_subject, publication).await?;
    approval.plan.check(boundary).await?;
    let backup = checkpoints::capture(boundary, false).await?;
    checkpoints::validate_ref(boundary, &backup).await?;
    approval.plan.check(boundary).await?;
    let task_id = boundary
        .assignment()
        .task
        .strip_prefix("issue-")
        .ok_or_else(|| denied("Restore issue identity disappeared"))?;
    let backup_file = boundary.state_directory(&["checkpoints", task_id])?.join(
        domain::safe_integer(backup.get("number"))
            .ok_or_else(|| denied("Restore backup number disappeared"))?
            .to_string(),
    );
    let backup_snapshot = secure::read(&backup_file, 262_144)?
        .ok_or_else(|| denied("Restore backup metadata disappeared"))?;
    let Plan {
        record,
        validation,
        snapshot: _,
        audit,
        root: _,
        root_identity: _,
        common: _,
        common_identity: _,
        writer: _,
        authority: _,
        checkpoint_file,
        checkpoint_snapshot,
        index,
        index_snapshot,
        products,
        parents,
        archive,
    } = approval.plan;
    let task = boundary
        .assignment()
        .task
        .strip_prefix("issue-")
        .ok_or_else(|| denied("Restore task identity disappeared"))?;
    let parent = boundary.state_directory(&["checkpoints", task])?;
    let directory = parent.join(format!("restore-{}", files::nonce()?));
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let mut transaction = None;
    let mut touched = false;
    let post_archive = archive.postcondition();
    let mut archived = archive::execute(archive, boundary).await?;
    let outcome = async {
        post_archive.require_current(boundary)?;
        boundary.check()?;
        builder.create(&directory).map_err(|error| {
            secure::io_error("Create exclusive restore transaction", &directory, error)
        })?;
        secure::private_directory(&directory, false)?;
        let identity = paths::file_information(&directory)?.identity;
        transaction = Some(Transaction {
            directory: directory.clone(),
            identity,
            entries: Vec::new(),
            parents,
            created: Vec::new(),
        });
        let tx = transaction
            .as_mut()
            .ok_or_else(|| denied("Restore transaction identity is missing"))?;
        files::create_private_directories(&directory.join("before"))?;
        files::create_private_directories(&directory.join("after"))?;
        let mut manifest = Vec::new();
        for (position, (name, product)) in products.into_iter().enumerate() {
            boundary.check()?;
            post_archive.require_current(boundary)?;
            let staged_path = directory.join("after").join(position.to_string());
            let staged = if let Some((bytes, _)) = &product.after {
                let staged = archive::write_owned(&staged_path, bytes)?;
                Some(staged)
            } else {
                None
            };
            manifest.push(Value::object(vec![
                ("path", Value::string(&name)),
                (
                    "before",
                    product.before.as_ref().map_or(Value::Null, |snapshot| {
                        Value::string(&sha256_hex(&snapshot.bytes))
                    }),
                ),
                (
                    "after",
                    staged.as_ref().map_or(Value::Null, |snapshot| {
                        Value::string(&sha256_hex(&snapshot.bytes))
                    }),
                ),
            ]));
            tx.entries.push(Entry {
                name,
                before: product.before,
                staged,
                staged_path,
                backup_path: directory.join("before").join(position.to_string()),
                backup: false,
                removed: false,
                installed: false,
            });
            if let Some((_, git_mode)) = product.after {
                mode(
                    &tx.entries
                        .last()
                        .ok_or_else(|| denied("Staged restore entry disappeared"))?
                        .staged_path,
                    &git_mode,
                )?;
            }
        }
        let journal = Value::object(vec![
            ("schema_version", Value::Number(1.0)),
            ("task", Value::string(&boundary.assignment().task)),
            (
                "worktree_identity",
                physical_identity(paths::file_information(boundary.root())?.identity),
            ),
            ("directory_identity", physical_identity(identity)),
            ("time", Value::string(&crate::clock::now_iso()?)),
            (
                "generation",
                Value::Number(boundary.restore_generation() as f64),
            ),
            (
                "full_failures",
                boundary
                    .full_verification()
                    .and_then(|record| record.failure_exits())
                    .map_or(Value::Null, |(original, latest)| {
                        Value::Array(vec![
                            Value::Number(f64::from(original)),
                            Value::Number(f64::from(latest)),
                        ])
                    }),
            ),
            ("checkpoint", record.clone()),
            ("backup", backup.clone()),
            ("files", Value::Array(manifest)),
            ("publication", Value::string("host-confirmed-unpublished")),
        ]);
        archive::write_owned(
            &directory.join("transaction.json"),
            format!("{}\n", journal.json()).as_bytes(),
        )?;
        tx.check(boundary)?;
        require_current_unpublished(boundary, &publication_subject, publication).await?;
        post_archive.require_current(boundary)?;
        if checkpoints::validation(boundary).await? != validation
            || !equal_snapshot(&index, index_snapshot.as_ref(), MAX_FILE_BYTES)?
            || !equal_snapshot(&checkpoint_file, Some(&checkpoint_snapshot), 262_144)?
            || !equal_snapshot(&backup_file, Some(&backup_snapshot), 262_144)?
            || checkpoints::validate_green(boundary, domain::safe_integer(record.get("number")))
                .await?
                != record
            || boundary.audit()? != audit
        {
            return Err(denied(
                "Restore consent, task, branch, index or source changed before product mutation",
            ));
        }
        post_archive.require_current(boundary)?;
        checkpoints::validate_ref(boundary, &backup).await?;
        post_archive.require_current(boundary)?;
        boundary.check()?;
        touched = true;
        boundary.clear_restore_evidence();
        let mut expected_audit = audit;
        for position in 0..tx.entries.len() {
            boundary.check()?;
            post_archive.require_current(boundary)?;
            tx.check(boundary)?;
            if boundary.audit()? != expected_audit {
                return Err(denied("User source changed between restore operations"));
            }
            let name = tx
                .entries
                .get(position)
                .ok_or_else(|| denied("Restore product entry disappeared"))?
                .name
                .clone();
            tx.parents_for(boundary, &name)?;
            let entry = tx
                .entries
                .get_mut(position)
                .ok_or_else(|| denied("Restore product entry disappeared"))?;
            let target = boundary.root().join(&entry.name);
            if !equal_snapshot(&target, entry.before.as_ref(), MAX_FILE_BYTES)? {
                return Err(denied("Restore will not overwrite a changed user product"));
            }
            if let Some(before) = &entry.before {
                fs::hard_link(&target, &entry.backup_path).map_err(|error| {
                    secure::io_error("Preserve original product", &entry.backup_path, error)
                })?;
                entry.backup = true;
                if paths::file_information(&target)?.links != 2
                    || !secure::remove_matching(&target, before, MAX_FILE_BYTES)?
                {
                    return Err(denied("Restore lost original product ownership"));
                }
                entry.removed = true;
            }
            if let Some(staged) = &entry.staged {
                if !secure::unchanged(&entry.staged_path, staged, MAX_FILE_BYTES)?
                    || paths::file_information(&entry.staged_path)?.links != 1
                {
                    return Err(denied("Restore staged product changed"));
                }
                fs::hard_link(&entry.staged_path, &target).map_err(|error| {
                    secure::io_error("Install restored product exclusively", &target, error)
                })?;
                entry.installed = true;
                if !secure::remove_matching(&entry.staged_path, staged, MAX_FILE_BYTES)? {
                    return Err(denied("Restore staging link could not be removed"));
                }
                if paths::file_information(&target)?.links != 1
                    || !secure::unchanged(&target, staged, MAX_FILE_BYTES)?
                {
                    return Err(denied(
                        "Restored product identity changed during installation",
                    ));
                }
            } else if secure::read(&target, MAX_FILE_BYTES)?.is_some() {
                return Err(denied(
                    "A foreign product appeared after the approved deletion",
                ));
            }
            let after = boundary.audit()?;
            if expected_audit
                .iter()
                .any(|(path, value)| path != &entry.name && after.get(path) != Some(value))
                || after
                    .keys()
                    .any(|path| path != &entry.name && !expected_audit.contains_key(path))
            {
                return Err(denied(
                    "Foreign user changes occurred during restore; rollback preserves them",
                ));
            }
            if !equal_snapshot(&target, entry.staged.as_ref(), MAX_FILE_BYTES)? {
                return Err(denied(
                    "Restored bytes changed during the transaction audit",
                ));
            }
            expected_audit = after;
            tokio::task::yield_now().await;
            post_archive.require_current(boundary)?;
            boundary.check()?;
        }
        for entry in &tx.entries {
            if !equal_snapshot(
                &boundary.root().join(&entry.name),
                entry.staged.as_ref(),
                MAX_FILE_BYTES,
            )? {
                return Err(denied("Restored product set changed before commit"));
            }
        }
        require_current_unpublished(boundary, &publication_subject, publication).await?;
        post_archive.require_current(boundary)?;
        if checkpoints::validation(boundary).await? != validation
            || !equal_snapshot(&index, index_snapshot.as_ref(), MAX_FILE_BYTES)?
            || !equal_snapshot(&checkpoint_file, Some(&checkpoint_snapshot), 262_144)?
            || !equal_snapshot(&backup_file, Some(&backup_snapshot), 262_144)?
            || checkpoints::validate_green(boundary, domain::safe_integer(record.get("number")))
                .await?
                != record
            || boundary.audit()? != expected_audit
        {
            return Err(denied(
                "Restore final identity changed; no success was accepted",
            ));
        }
        post_archive.require_current(boundary)?;
        checkpoints::validate_ref(boundary, &backup).await?;
        post_archive.require_current(boundary)?;
        tx.check(boundary)?;
        archive::write_owned(
            &directory.join("COMMITTED"),
            b"restored candidate; fresh verification required\n",
        )?;
        post_archive.require_current(boundary)?;
        boundary.check()?;
        Ok(Report {
            value: Value::object(vec![
                ("status", Value::string("restored-unverified")),
                ("checkpoint", record),
                ("backup", backup),
                (
                    "archive",
                    archived
                        .as_ref()
                        .map_or(Value::Null, archive::Archived::as_value),
                ),
                ("transaction", Value::string(&directory.to_string_lossy())),
                ("requiresFreshVerification", Value::Boolean(true)),
            ]),
        })
    }
    .await;
    let mut cleanup = if outcome.is_err() {
        let rollback = if let Some(tx) = &mut transaction {
            tx.rollback(boundary).and_then(|()| {
                archive::write_owned(
                    &tx.directory.join("ROLLED-BACK"),
                    b"owned product state restored; fresh verification required\n",
                )
                .map(|_| ())
            })
        } else {
            Ok(())
        };
        let artifacts = if rollback.is_ok() {
            archived
                .as_mut()
                .map_or(Ok(()), |archive| archive.rollback(boundary))
        } else {
            Ok(())
        };
        crate::state::locks::finish(rollback, artifacts)
    } else {
        Ok(())
    };
    if outcome.is_err() && cleanup.is_ok() {
        if let Some(tx) = &transaction {
            cleanup = archive::write_owned(
                &tx.directory.join("RECONCILED"),
                b"owned source and artifact rollback reconciled; fresh verification required\n",
            )
            .map(|_| ());
        }
    }
    if touched {
        if let Err(error) = boundary.revoke_after_restore() {
            boundary.quarantine_owned();
            return Err(error);
        }
    }
    let cleanup = cleanup.map_err(|cause| Error::new("OwnedRestoreCleanupError", "Restore rollback or cleanup is unverified; retain the writer, source backups and artifact archive for inspection").coded("ROSTER_RESTORE_CLEANUP").caused_by(cause));
    if cleanup.is_err() {
        boundary.quarantine_owned();
    }
    if outcome.as_ref().err().is_some_and(|error| {
        ["ROSTER_ARTIFACT_CLEANUP", "ROSTER_PROCESS_CLEANUP"].contains(&error.code.unwrap_or(""))
    }) {
        boundary.quarantine_owned();
    }
    backend::finish_owned(backend::Owner::Execution(boundary), outcome, cleanup)
}
