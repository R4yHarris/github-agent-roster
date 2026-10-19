use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::identity::sha256_hex;
use crate::paths::{self, FileIdentity};
use crate::state::{files, secure};

const NAMES: [&str; 8] = [
    "RECIPE.yml",
    "TASK.md",
    "PLAN.md",
    "ESTIMATE.md",
    "CONTEXT.md",
    "RESEARCH.md",
    "RESULT.md",
    "REVIEW.md",
];
pub const PRESERVE_AUTHORITY: [&str; 4] = ["RECIPE.yml", "TASK.md", "PLAN.md", "ESTIMATE.md"];

struct Artifact {
    name: String,
    snapshot: secure::Snapshot,
    permissions: fs::Permissions,
}

pub struct Plan {
    root: PathBuf,
    root_identity: FileIdentity,
    common: PathBuf,
    common_identity: FileIdentity,
    writer: Arc<()>,
    authority: String,
    task: String,
    artifacts: Vec<Artifact>,
    absent: Vec<String>,
}

pub(crate) struct PostArchive {
    root: PathBuf,
    root_identity: FileIdentity,
    writer: Arc<()>,
    authority: String,
    names: Vec<String>,
}

fn require_absent(root: &Path, names: &[String]) -> Result<(), Error> {
    for name in names {
        if secure::read(&root.join(name), MAX_FILE_BYTES)?.is_some() {
            return Err(denied(
                "Unapproved human run artifact appeared after absence was sealed; preserve it and refuse restoration",
            ));
        }
    }
    Ok(())
}

impl PostArchive {
    fn check(&self, owner: backend::Owner<'_>) -> Result<(), Error> {
        owner.check()?;
        if owner.root() != self.root
            || self.root_identity != paths::file_information(owner.root())?.identity
            || !Arc::ptr_eq(&self.writer, &owner.identity())
            || self.authority != owner.authority()
        {
            return Err(denied(
                "Post-archive absence belongs to another current writer or worktree",
            ));
        }
        require_absent(&self.root, &self.names)
    }
    pub(crate) fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.check(backend::Owner::Execution(boundary))
    }
}

struct Moved {
    artifact: Artifact,
    archived: secure::Snapshot,
    removed: bool,
}

pub struct Archived {
    root: PathBuf,
    directory: PathBuf,
    identity: FileIdentity,
    writer: Arc<()>,
    authority: String,
    moved: Vec<Moved>,
}

pub(crate) async fn common_directory(owner: backend::Owner<'_>) -> Result<PathBuf, Error> {
    owner.check()?;
    let common = tools::bytes_text(
        &owner
            .git(
                owner.root(),
                &["rev-parse", "--git-common-dir"].map(str::to_owned),
                false,
            )
            .await?,
    )?;
    let resolved = paths::resolve(owner.root(), Path::new(common.trim()))?;
    secure::check_components(&resolved)?;
    let common = paths::strip_verbatim(
        fs::canonicalize(&resolved)
            .map_err(|error| secure::io_error("Resolve Git common directory", &resolved, error))?,
    );
    secure::check_components(&common)?;
    if !fs::symlink_metadata(&common)
        .map_err(|error| secure::io_error("Inspect Git common directory", &common, error))?
        .is_dir()
    {
        return Err(denied(
            "Artifact storage requires the actual Git common directory",
        ));
    }
    owner.check()?;
    Ok(common)
}

fn task_name(task: &str) -> Result<String, Error> {
    if !regex(r"^(?:[A-Za-z0-9._-]{1,64})$")?.is_match(task) || task == "-" {
        return Err(Error::type_error(
            "Run archive requires an opaque task identifier",
        ));
    }
    Ok(
        if regex(r"^(?:issue-[1-9][0-9]*|local-[a-f0-9]{16})$")?.is_match(task) {
            task.to_owned()
        } else {
            format!(
                "task-{}",
                sha256_hex(task.as_bytes())
                    .get(..16)
                    .ok_or_else(|| Error::type_error("Archive task digest is incomplete"))?
            )
        },
    )
}

pub(crate) fn reject_secret(
    bytes: &[u8],
    redaction: &crate::state::redaction::Config,
) -> Result<(), Error> {
    let content = Text::from(tools::bytes_text(bytes)?.as_str());
    if crate::planning::memory::redact(&content, redaction) != content
        || !crate::state::redaction::secret_material_lines(&content, redaction).is_empty()
    {
        return Err(denied(
            "Artifact and recovery snapshots must not persist secret material; originals were preserved",
        ));
    }
    Ok(())
}

pub(crate) fn write_owned(path: &Path, bytes: &[u8]) -> Result<secure::Snapshot, Error> {
    secure::check_components(path)?;
    let mut file = files::create_private_file(path)
        .map_err(|error| secure::io_error("Create owned recovery file", path, error))?;
    let identity = paths::handle_information(&file)
        .map_err(|error| {
            Error::new(
                "OwnedArtifactCleanupError",
                "New recovery file identity could not be established; preserve its directory",
            )
            .coded("ROSTER_ARTIFACT_CLEANUP")
            .caused_by(error)
        })?
        .identity;
    let mut written = 0_usize;
    let outcome = (|| {
        while written < bytes.len() {
            let remaining = bytes
                .get(written..)
                .ok_or_else(|| Error::type_error("Owned write exceeded its byte range"))?;
            let count = file
                .write(remaining)
                .map_err(|error| secure::io_error("Write owned recovery file", path, error))?;
            if count == 0 {
                return Err(
                    Error::new("Error", "Owned recovery write made no progress").coded("EIO")
                );
            }
            written = written
                .checked_add(count)
                .ok_or_else(|| Error::type_error("Owned write exceeded its byte bound"))?;
        }
        file.sync_all()
            .map_err(|error| secure::io_error("Sync owned recovery file", path, error))?;
        Ok(())
    })();
    drop(file);
    let snapshot = secure::Snapshot {
        identity,
        bytes: bytes
            .get(..written)
            .ok_or_else(|| Error::type_error("Owned write prefix is invalid"))?
            .to_vec(),
    };
    if let Err(error) = outcome {
        return match secure::remove_matching(path, &snapshot, MAX_FILE_BYTES) {
            Ok(true) => Err(error),
            cleanup => Err(Error::new(
                "OwnedArtifactCleanupError",
                format!("Incomplete recovery file cleanup could not be verified: {cleanup:?}"),
            )
            .coded("ROSTER_ARTIFACT_CLEANUP")
            .caused_by(error)),
        };
    }
    Ok(snapshot)
}

async fn prepare_owner(
    owner: backend::Owner<'_>,
    task: &str,
    preserve: &[&str],
) -> Result<Plan, Error> {
    owner.check()?;
    task_name(task)?;
    if preserve.len() > PRESERVE_AUTHORITY.len()
        || preserve
            .iter()
            .any(|name| !PRESERVE_AUTHORITY.contains(name))
        || preserve
            .iter()
            .enumerate()
            .any(|(index, name)| preserve.iter().skip(index + 1).any(|other| other == name))
    {
        return Err(Error::type_error(
            "Run archive may preserve only distinct recipe, task, plan and estimate artifacts",
        ));
    }
    let mut artifacts = Vec::new();
    let mut absent = Vec::new();
    let mut total = 0_u64;
    for name in NAMES {
        owner.check()?;
        let path = owner.root().join(name);
        paths::ensure_local_path(&path, owner.root())?;
        let Some(snapshot) = secure::read(&path, MAX_FILE_BYTES)? else {
            if !preserve.contains(&name) {
                absent.push(name.to_owned());
            }
            continue;
        };
        if paths::file_information(&path)?.links != 1
            || !secure::unchanged(&path, &snapshot, MAX_FILE_BYTES)?
        {
            return Err(denied(
                "Previous run artifacts must remain regular single-link files",
            ));
        }
        if preserve.contains(&name) {
            continue;
        }
        total = total
            .checked_add(snapshot.bytes.len() as u64)
            .filter(|total| *total <= MAX_SNAPSHOT_BYTES)
            .ok_or_else(|| denied("Run artifact archive exceeds 128 MiB"))?;
        reject_secret(&snapshot.bytes, owner.redaction())?;
        let permissions = fs::metadata(&path)
            .map_err(|error| secure::io_error("Inspect artifact permissions", &path, error))?
            .permissions();
        artifacts.push(Artifact {
            name: name.to_owned(),
            snapshot,
            permissions,
        });
    }
    if !artifacts.is_empty() {
        let mut args = vec!["ls-files".to_owned(), "-z".to_owned(), "--".to_owned()];
        args.extend(artifacts.iter().map(|artifact| artifact.name.clone()));
        if !owner.git(owner.root(), &args, false).await?.is_empty() {
            return Err(denied(
                "Tracked planning or run artifacts may not be archived or replaced",
            ));
        }
    }
    let common = common_directory(owner).await?;
    Ok(Plan {
        root: owner.root().to_path_buf(),
        root_identity: paths::file_information(owner.root())?.identity,
        common_identity: paths::file_information(&common)?.identity,
        common,
        writer: owner.identity(),
        authority: owner.authority(),
        task: task.to_owned(),
        artifacts,
        absent,
    })
}

pub async fn prepare(boundary: &tools::Boundary, preserve: &[&str]) -> Result<Plan, Error> {
    if boundary.role() != Role::Coder {
        return Err(denied("Artifact archival requires the owning coder host"));
    }
    if !PRESERVE_AUTHORITY
        .iter()
        .all(|name| preserve.contains(name))
    {
        return Err(denied(
            "An active task boundary cannot archive or rewrite its immutable authority",
        ));
    }
    prepare_owner(
        backend::Owner::Execution(boundary),
        &boundary.assignment().task,
        preserve,
    )
    .await
}

pub async fn prepare_before_plan(
    lease: &backend::RootLease,
    task: &str,
    preserve: &[&str],
) -> Result<Plan, Error> {
    prepare_owner(backend::Owner::PrePlan(lease), task, preserve).await
}

pub(crate) async fn prepare_owned(
    boundary: &tools::Boundary,
    name: &str,
    expected: &secure::Snapshot,
) -> Result<Plan, Error> {
    boundary.check()?;
    if boundary.role() != Role::Coder
        || !["CONTEXT.md", "RESEARCH.md", "RESULT.md", "REVIEW.md"].contains(&name)
    {
        return Err(denied(
            "Only the current host can rotate its own non-authority seat artifact",
        ));
    }
    let owner = backend::Owner::Execution(boundary);
    let path = boundary.root().join(name);
    paths::ensure_local_path(&path, boundary.root())?;
    if paths::file_information(&path)?.links != 1
        || !secure::unchanged(&path, expected, MAX_FILE_BYTES)?
    {
        return Err(denied(
            "Owned artifact changed before rotation; preserve the human replacement",
        ));
    }
    reject_secret(&expected.bytes, boundary.redaction())?;
    if !owner
        .git(
            owner.root(),
            &["ls-files", "-z", "--", name].map(str::to_owned),
            false,
        )
        .await?
        .is_empty()
    {
        return Err(denied("Tracked seat artifacts cannot be rotated"));
    }
    let common = common_directory(owner).await?;
    let permissions = fs::metadata(&path)
        .map_err(|error| secure::io_error("Inspect owned artifact", &path, error))?
        .permissions();
    Ok(Plan {
        root: boundary.root().to_path_buf(),
        root_identity: paths::file_information(boundary.root())?.identity,
        common_identity: paths::file_information(&common)?.identity,
        common,
        writer: owner.identity(),
        authority: owner.authority(),
        task: boundary.assignment().task.clone(),
        artifacts: vec![Artifact {
            name: name.to_owned(),
            snapshot: secure::Snapshot {
                identity: expected.identity,
                bytes: expected.bytes.clone(),
            },
            permissions,
        }],
        absent: Vec::new(),
    })
}

impl Plan {
    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("task", Value::string(&self.task)),
            ("absent", strings(&self.absent)),
            (
                "files",
                Value::Array(
                    self.artifacts
                        .iter()
                        .map(|artifact| {
                            Value::object(vec![
                                ("name", Value::string(&artifact.name)),
                                (
                                    "digest",
                                    Value::string(&sha256_hex(&artifact.snapshot.bytes)),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }

    fn check(&self, owner: backend::Owner<'_>) -> Result<(), Error> {
        owner.check()?;
        if owner.root() != self.root
            || self.root_identity != paths::file_information(owner.root())?.identity
            || self.common_identity != paths::file_information(&self.common)?.identity
            || !Arc::ptr_eq(&self.writer, &owner.identity())
            || self.authority != owner.authority()
        {
            return Err(denied(
                "Archive plan no longer belongs to this current writer and repository",
            ));
        }
        secure::check_components(&self.common)?;
        for artifact in &self.artifacts {
            let path = owner.root().join(&artifact.name);
            if paths::file_information(&path)?.links != 1
                || !secure::unchanged(&path, &artifact.snapshot, MAX_FILE_BYTES)?
            {
                return Err(denied(
                    "Run artifact changed after archive preflight; preserve the new user bytes",
                ));
            }
        }
        require_absent(&self.root, &self.absent)?;
        Ok(())
    }
    pub(crate) fn postcondition(&self) -> PostArchive {
        let mut names = self
            .artifacts
            .iter()
            .map(|artifact| artifact.name.clone())
            .collect::<Vec<_>>();
        names.extend(self.absent.clone());
        PostArchive {
            root: self.root.clone(),
            root_identity: self.root_identity,
            writer: self.writer.clone(),
            authority: self.authority.clone(),
            names,
        }
    }
    pub(crate) async fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        let owner = backend::Owner::Execution(boundary);
        self.check(owner)?;
        if !self.artifacts.is_empty() {
            let mut tracked = vec!["ls-files".to_owned(), "-z".to_owned(), "--".to_owned()];
            tracked.extend(self.artifacts.iter().map(|artifact| artifact.name.clone()));
            if !owner.git(owner.root(), &tracked, false).await?.is_empty() {
                return Err(denied("Artifact tracking changed after archive approval"));
            }
        }
        self.check(owner)
    }
}

async fn execute_owner(plan: Plan, owner: backend::Owner<'_>) -> Result<Option<Archived>, Error> {
    plan.check(owner)?;
    let postcondition = plan.postcondition();
    if plan.artifacts.is_empty() {
        postcondition.check(owner)?;
        return Ok(None);
    }
    let mut tracked = vec!["ls-files".to_owned(), "-z".to_owned(), "--".to_owned()];
    tracked.extend(plan.artifacts.iter().map(|artifact| artifact.name.clone()));
    if !owner.git(owner.root(), &tracked, false).await?.is_empty() {
        return Err(denied("Artifact tracking changed after archive preflight"));
    }
    plan.check(owner)?;
    let root = plan.common.join("roster-artifacts");
    secure::private_directory(&root, true)?;
    let task_root = root.join(task_name(&plan.task)?);
    secure::private_directory(&task_root, true)?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::new("Error", "Archive clock precedes the Unix epoch"))?
        .as_millis();
    let nonce = files::nonce()?;
    let directory = task_root.join(format!(
        "{millis}-{}",
        nonce
            .get(..12)
            .ok_or_else(|| Error::type_error("Archive nonce is incomplete"))?
    ));
    owner.check()?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&directory)
        .map_err(|error| secure::io_error("Create exclusive run archive", &directory, error))?;
    secure::private_directory(&directory, false)?;
    let identity = paths::file_information(&directory)?.identity;
    let mut archived = Archived {
        root: plan.root.clone(),
        directory,
        identity,
        writer: plan.writer.clone(),
        authority: plan.authority.clone(),
        moved: Vec::new(),
    };
    let outcome = async {
        plan.check(owner)?;
        let absent = plan.absent;
        for artifact in plan.artifacts {
            owner.check()?;
            require_absent(owner.root(), &absent)?;
            archived.require_no_replacements(owner)?;
            let path = owner.root().join(&artifact.name);
            if paths::file_information(&path)?.links != 1
                || !secure::unchanged(&path, &artifact.snapshot, MAX_FILE_BYTES)?
            {
                return Err(denied(
                    "Run artifact changed during archival; new user bytes were not replaced",
                ));
            }
            let destination = archived.directory.join(&artifact.name);
            let copy = write_owned(&destination, &artifact.snapshot.bytes)?;
            archived.moved.push(Moved {
                artifact,
                archived: copy,
                removed: false,
            });
            if !owner.git(owner.root(), &tracked, false).await?.is_empty() {
                return Err(denied(
                    "Run artifacts became tracked during archival; preserve the staged user data",
                ));
            }
            require_absent(owner.root(), &absent)?;
            archived.require_no_replacements(owner)?;
            let moved = archived
                .moved
                .last_mut()
                .ok_or_else(|| denied("Archive lost its staged artifact identity"))?;
            if paths::file_information(&path)?.links != 1
                || !secure::remove_matching(&path, &moved.artifact.snapshot, MAX_FILE_BYTES)?
            {
                return Err(denied("Archive no longer owns its source artifact"));
            }
            moved.removed = true;
            tokio::task::yield_now().await;
            owner.check()?;
            require_absent(owner.root(), &absent)?;
            archived.require_no_replacements(owner)?;
        }
        secure::sync_directory(&archived.directory)?;
        owner.check()?;
        postcondition.check(owner)?;
        Ok(())
    }
    .await;
    if let Err(error) = outcome {
        let cleanup = archived.rollback_owner(owner);
        return backend::finish_owned(owner, Err(error), cleanup);
    }
    Ok(Some(archived))
}

pub async fn execute(plan: Plan, boundary: &tools::Boundary) -> Result<Option<Archived>, Error> {
    execute_owner(plan, backend::Owner::Execution(boundary)).await
}

pub async fn execute_before_plan(
    plan: Plan,
    lease: &backend::RootLease,
) -> Result<Option<Archived>, Error> {
    execute_owner(plan, backend::Owner::PrePlan(lease)).await
}

impl Archived {
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn as_value(&self) -> Value {
        Value::object(vec![
            (
                "directory",
                Value::string(&self.directory.to_string_lossy()),
            ),
            (
                "files",
                strings(
                    &self
                        .moved
                        .iter()
                        .map(|moved| moved.artifact.name.clone())
                        .collect::<Vec<_>>(),
                ),
            ),
        ])
    }
    fn check_owner(&self, owner: backend::Owner<'_>) -> Result<(), Error> {
        owner.cleanup_check()?;
        if owner.root() != self.root
            || !Arc::ptr_eq(&self.writer, &owner.identity())
            || self.authority != owner.authority()
            || self.identity != paths::file_information(&self.directory)?.identity
        {
            return Err(denied(
                "Archive rollback no longer owns its directory or writer",
            ));
        }
        secure::private_directory(&self.directory, false)
    }
    fn require_no_replacements(&self, owner: backend::Owner<'_>) -> Result<(), Error> {
        self.check_owner(owner)?;
        let names = self
            .moved
            .iter()
            .filter(|moved| moved.removed)
            .map(|moved| moved.artifact.name.clone())
            .collect::<Vec<_>>();
        require_absent(&self.root, &names)
    }
    fn rollback_owner(&mut self, owner: backend::Owner<'_>) -> Result<(), Error> {
        self.check_owner(owner)?;
        for moved in self.moved.iter().rev() {
            let source = self.root.join(&moved.artifact.name);
            let archived = self.directory.join(&moved.artifact.name);
            if !moved.removed {
                if !secure::unchanged(&source, &moved.artifact.snapshot, MAX_FILE_BYTES)?
                    || !secure::remove_matching(&archived, &moved.archived, MAX_FILE_BYTES)?
                {
                    return Err(denied(
                        "Archive rollback preserved a changed source and its retained copy",
                    ));
                }
                continue;
            }
            if secure::read(&source, MAX_FILE_BYTES)?.is_some()
                || !secure::unchanged(&archived, &moved.archived, MAX_FILE_BYTES)?
            {
                return Err(denied(
                    "Archive rollback preserves a new artifact or changed archive; inspect retained evidence",
                ));
            }
            let restored = write_owned(&source, &moved.artifact.snapshot.bytes)?;
            fs::set_permissions(&source, moved.artifact.permissions.clone()).map_err(|error| {
                secure::io_error("Restore artifact permissions", &source, error)
            })?;
            if !secure::unchanged(&source, &restored, MAX_FILE_BYTES)?
                || !secure::remove_matching(&archived, &moved.archived, MAX_FILE_BYTES)?
            {
                return Err(denied("Archive rollback lost owned artifact identity"));
            }
        }
        self.moved.clear();
        self.check_owner(owner)?;
        if !secure::entries(&self.directory)?.is_empty() {
            return Err(denied(
                "Archive rollback found foreign entries; preserve the directory",
            ));
        }
        fs::remove_dir(&self.directory)
            .map_err(|error| secure::io_error("Remove rolled-back archive", &self.directory, error))
    }
    pub(crate) fn rollback(&mut self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.rollback_owner(backend::Owner::Execution(boundary))
    }
}

pub async fn archived_artifacts(
    boundary: &tools::Boundary,
    name: &str,
    limit: usize,
) -> Result<Vec<Text>, Error> {
    boundary.check()?;
    if !NAMES.contains(&name) || limit == 0 || limit > 50 {
        return Err(Error::type_error(
            "Archived artifact reads require a known name and 1-50 records",
        ));
    }
    let common = common_directory(backend::Owner::Execution(boundary)).await?;
    let root = common
        .join("roster-artifacts")
        .join(task_name(&boundary.assignment().task)?);
    if !secure::private_directory_if_present(&root)? {
        return Ok(Vec::new());
    }
    let mut entries = secure::entries(&root)?;
    if entries.len() > MAX_FILES {
        return Err(denied("Archive listing exceeds 2048 entries"));
    }
    let pattern = regex(r"^[0-9]+-[a-f0-9]{12}$")?;
    entries.retain(|entry| pattern.is_match(entry));
    entries.sort_by(|left, right| right.cmp(left));
    let mut result = Vec::new();
    for entry in entries.into_iter().take(limit) {
        boundary.check()?;
        let directory = root.join(entry);
        secure::private_directory(&directory, false)?;
        let path = directory.join(name);
        if let Some(snapshot) = secure::read(&path, 262_144)? {
            if paths::file_information(&path)?.links != 1
                || !secure::unchanged(&path, &snapshot, 262_144)?
            {
                return Err(denied(
                    "Archived artifacts must remain bounded stable single-link files",
                ));
            }
            result.push(crate::planning::memory::redact(
                &Text::from(tools::bytes_text(&snapshot.bytes)?.as_str()),
                boundary.redaction(),
            ));
        }
    }
    boundary.check()?;
    Ok(result)
}
