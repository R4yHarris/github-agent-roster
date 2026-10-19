use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::paths::{self, FileIdentity};
use crate::state::{files, secure};

pub(crate) struct Scratch<'a> {
    owner: backend::Owner<'a>,
    path: PathBuf,
    identity: FileIdentity,
    parent: PathBuf,
    parent_identity: FileIdentity,
    writer: Arc<()>,
    authority: String,
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> Error {
    secure::io_error(action, path, error)
}

impl<'a> Scratch<'a> {
    pub(crate) async fn create(owner: backend::Owner<'a>) -> Result<Self, Error> {
        let _scope = owner.span(timing::Label::ScratchCreate);
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
        let common = paths::resolve(owner.root(), Path::new(common.trim()))?;
        secure::check_components(&common)?;
        let parent = common.join("roster-exec");
        files::create_private_directories(&parent)?;
        secure::private_directory(&parent, false)?;
        let parent_identity = paths::file_information(&parent)?.identity;
        let stamp = crate::identity::sha256_hex(crate::clock::now_iso()?.as_bytes());
        let stamp = stamp
            .get(..8)
            .ok_or_else(|| denied("Scratch nonce is missing"))?;
        let mut selected = None;
        for number in 0..64 {
            owner.check()?;
            let path = parent.join(format!("s-{:x}-{stamp}-{number}", std::process::id()));
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder
            };
            #[cfg(not(unix))]
            let builder = fs::DirBuilder::new();
            match builder.create(&path) {
                Ok(()) => {
                    selected = Some(path);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(io_error("Create owned execution scratch", &path, error)),
            }
        }
        let path =
            selected.ok_or_else(|| denied("Owned scratch exclusive-create budget exhausted"))?;
        let identity = paths::file_information(&path)?.identity;
        let result = Self {
            owner,
            path,
            identity,
            parent,
            parent_identity,
            writer: owner.identity(),
            authority: owner.authority(),
        };
        if let Err(error) = result.check(false) {
            return backend::finish_owned(owner, Err(error), result.close().await);
        }
        Ok(result)
    }

    fn check(&self, cleanup: bool) -> Result<(), Error> {
        if cleanup {
            self.owner.storage_check()?;
        } else {
            self.owner.check()?;
        }
        secure::check_components(&self.path)?;
        secure::check_components(&self.parent)?;
        if !Arc::ptr_eq(&self.writer, &self.owner.identity())
            || self.authority != self.owner.authority()
            || paths::file_information(&self.path)?.identity != self.identity
            || paths::file_information(&self.parent)?.identity != self.parent_identity
        {
            return Err(denied(
                "Execution scratch root or writer identity changed; preserve it",
            ));
        }
        Ok(())
    }

    pub(crate) fn directory(&self, name: &str) -> Result<PathBuf, Error> {
        self.check(false)?;
        if !regex(r"^[a-z][a-z0-9-]{0,63}$")?.is_match(name) {
            return Err(denied(
                "Scratch child directory requires a bounded internal name",
            ));
        }
        let path = self.path.join(name);
        files::create_private_directories(&path)?;
        secure::private_directory(&path, false)?;
        Ok(path)
    }

    fn write_materialized(
        &self,
        destination: &Path,
        mode: &str,
        name: &str,
        body: &[u8],
    ) -> Result<(), Error> {
        if !policy::exact_path(name) || !["100644", "100755"].contains(&mode) {
            return Err(denied(
                "Materialized blob path or regular-file mode is invalid",
            ));
        }
        let target = destination.join(name);
        paths::ensure_local_path(&target, destination)?;
        files::create_private_directories(
            target
                .parent()
                .ok_or_else(|| denied("Base file needs a parent"))?,
        )?;
        secure::write_exclusive(&target, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &target,
                fs::Permissions::from_mode(if mode == "100755" { 0o700 } else { 0o600 }),
            )
            .map_err(|error| io_error("Apply owned base executable mode", &target, error))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        Ok(())
    }

    async fn materialize_chunked(
        &self,
        destination: &Path,
        selected: &[(String, String, blob::Object)],
    ) -> Result<(), Error> {
        let objects = selected
            .iter()
            .map(|(_, _, object)| object.clone())
            .collect::<Vec<_>>();
        let mut bytes = 0_u64;
        for range in blob::chunks(&objects)? {
            let requests = objects
                .get(range.clone())
                .ok_or_else(|| denied("Blob chunk plan range is invalid"))?;
            let output = self.owner.git_blob_chunk(destination, requests).await?;
            let entries = selected
                .get(range)
                .ok_or_else(|| denied("Blob materialization range is invalid"))?;
            for (index, (name, mode, _)) in entries.iter().enumerate() {
                self.check(false)?;
                let body = output.body(index)?;
                bytes = bytes
                    .checked_add(
                        u64::try_from(body.len())
                            .map_err(|_| denied("Base body size exceeds u64"))?,
                    )
                    .ok_or_else(|| denied("Base byte count overflow"))?;
                if bytes > MAX_SNAPSHOT_BYTES {
                    return Err(denied("Base revision exceeds 128 MiB"));
                }
                self.write_materialized(destination, mode, name, body)?;
            }
        }
        Ok(())
    }

    async fn materialize(
        &self,
        source: &Path,
        revision: &str,
        destination: &Path,
        modules: &[backend::LocalSubmodule],
        depth: usize,
    ) -> Result<(), Error> {
        self.check(false)?;
        if depth > 4 || !regex(r"^[a-f0-9]{40}(?:[a-f0-9]{24})?$")?.is_match(revision) {
            return Err(denied(
                "Base materialization requires a captured commit and bounded local submodules",
            ));
        }
        secure::check_components(source)?;
        let batching = self.owner.host().git.is_absolute();
        let tree_args = if batching {
            vec!["ls-tree", "-r", "-l", "-z", revision]
        } else {
            vec!["ls-tree", "-r", "-z", revision]
        };
        let tree = tools::bytes_text(
            &self
                .owner
                .git(
                    source,
                    &tree_args.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    false,
                )
                .await?,
        )?;
        let entries = tree
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .collect::<Vec<_>>();
        if entries.len() > MAX_FILES {
            return Err(denied("Base revision exceeds 2048 tree entries"));
        }
        for entry in &entries {
            let Some((metadata, path)) = entry.split_once('\t') else {
                return Err(denied("Base tree entry is malformed"));
            };
            if path == "package.json" {
                let object = metadata
                    .split(' ')
                    .nth(2)
                    .ok_or_else(|| denied("Base package object is missing"))?;
                let package = self
                    .owner
                    .git(
                        source,
                        &["cat-file".to_owned(), "blob".to_owned(), object.to_owned()],
                        false,
                    )
                    .await?;
                let package =
                    crate::json::parse(&Text::from(tools::bytes_text(&package)?.as_str()))?;
                if domain::is_text(package.get("name"), "github-agent-roster") {
                    return Err(unsupported(
                        "Retired Node Roster production is not materialized as a baseline or frozen oracle checkout",
                    ));
                }
            }
        }
        self.owner
            .git(
                self.owner.root(),
                &[
                    "clone".to_owned(),
                    "--shared".to_owned(),
                    "--quiet".to_owned(),
                    "--no-checkout".to_owned(),
                    "--config".to_owned(),
                    "core.fsmonitor=false".to_owned(),
                    "--".to_owned(),
                    source.to_string_lossy().into_owned(),
                    destination.to_string_lossy().into_owned(),
                ],
                false,
            )
            .await?;
        self.owner
            .git(
                destination,
                &["update-ref", "--no-deref", "HEAD", revision].map(str::to_owned),
                false,
            )
            .await?;
        self.owner
            .git(
                destination,
                &["read-tree", revision].map(str::to_owned),
                false,
            )
            .await?;
        let mut skipped = Vec::new();
        let mut links = Vec::new();
        let mut bytes = 0_u64;
        let mut selected = Vec::new();
        for entry in entries {
            self.check(false)?;
            let (metadata, name) = entry
                .split_once('\t')
                .ok_or_else(|| denied("Base tree entry is malformed"))?;
            if !policy::exact_path(name) {
                return Err(denied("Base tree contains a non-exact path"));
            }
            let parts = if batching {
                metadata.split_ascii_whitespace().collect::<Vec<_>>()
            } else {
                metadata.split(' ').collect::<Vec<_>>()
            };
            let (mode, kind, object, length) = if batching {
                let [mode, kind, object, length] = parts.as_slice() else {
                    return Err(denied("Sized base tree metadata is malformed"));
                };
                (*mode, *kind, *object, Some(*length))
            } else {
                let [mode, kind, object] = parts.as_slice() else {
                    return Err(denied("Base tree metadata is malformed"));
                };
                (*mode, *kind, *object, None)
            };
            if kind == "commit" && mode == "160000" {
                if batching && length != Some("-") {
                    return Err(denied("Base gitlink size metadata is malformed"));
                }
                links.push((name.to_owned(), object.to_owned()));
                continue;
            }
            if kind != "blob" || !["100644", "100755"].contains(&mode) {
                return Err(unsupported(
                    "Symbolic/special entries in selected base revision",
                ));
            }
            let planned_size = length
                .map(|length| blob::size(length.as_bytes()))
                .transpose()?;
            if crate::planning::forbidden_read(name)
                || policy::managed(name)
                || name.starts_with(".roster/")
            {
                skipped.push(name.to_owned());
                continue;
            }
            if let Some(size) = planned_size {
                selected.push((
                    name.to_owned(),
                    mode.to_owned(),
                    blob::Object::new(object, size)?,
                ));
                continue;
            }
            timing::finish(self.owner.span(timing::Label::ProofBlobRequested), None);
            let body = self
                .owner
                .git(
                    destination,
                    &["cat-file".to_owned(), "blob".to_owned(), object.to_owned()],
                    false,
                )
                .await?;
            bytes = bytes
                .checked_add(
                    u64::try_from(body.len()).map_err(|_| denied("Base body size exceeds u64"))?,
                )
                .ok_or_else(|| denied("Base byte count overflow"))?;
            if bytes > MAX_SNAPSHOT_BYTES {
                return Err(denied("Base revision exceeds 128 MiB"));
            }
            self.write_materialized(destination, mode, name, &body)?;
        }
        self.materialize_chunked(destination, &selected).await?;
        for batch in skipped.chunks(16) {
            let mut args = vec![
                "update-index".to_owned(),
                "--skip-worktree".to_owned(),
                "--".to_owned(),
            ];
            args.extend_from_slice(batch);
            self.owner.git(destination, &args, false).await?;
        }

        for (path, object) in links {
            let approved = modules
                .iter()
                .find(|module| module.path == path)
                .ok_or_else(|| {
                    unsupported("Initialized base submodule has no explicit local-source approval")
                })?;
            let expected = tools::bytes_text(
                &self
                    .owner
                    .git(
                        &approved.source,
                        &["rev-parse", "--verify", "HEAD"].map(str::to_owned),
                        false,
                    )
                    .await?,
            )?;
            if expected.trim() != object {
                return Err(denied(
                    "Approved submodule source does not match the captured gitlink commit",
                ));
            }
            let target = destination.join(&path);
            paths::ensure_local_path(&target, destination)?;
            files::create_private_directories(
                target
                    .parent()
                    .ok_or_else(|| denied("Submodule needs a parent"))?,
            )?;
            let prefix = format!("{path}/");
            let nested = modules
                .iter()
                .filter_map(|module| {
                    module
                        .path
                        .strip_prefix(&prefix)
                        .map(|path| backend::LocalSubmodule {
                            path: path.to_owned(),
                            source: module.source.clone(),
                        })
                })
                .collect::<Vec<_>>();
            Box::pin(self.materialize(&approved.source, &object, &target, &nested, depth + 1))
                .await?;
        }
        self.check(false)
    }

    pub(crate) async fn base(
        &self,
        revision: &str,
        approval: &backend::Approval,
    ) -> Result<PathBuf, Error> {
        let _scope = self.owner.span(timing::Label::ScratchBase);
        let destination = self.path.join("base");
        self.materialize(
            self.owner.root(),
            revision,
            &destination,
            &approval.submodules,
            0,
        )
        .await?;
        Ok(destination)
    }

    pub(crate) fn overlay_tests(&self, base: &Path, changed: &[String]) -> Result<(), Error> {
        self.check(false)?;
        for name in changed {
            if !policy::exact_path(name)
                || crate::planning::forbidden_read(name)
                || !(name.starts_with("tests/") || substance::is_test_file(name)?)
            {
                continue;
            }
            let source = evidence::read_product(self.owner.root(), name, MAX_FILE_BYTES)?;
            let destination = base.join(name);
            paths::ensure_local_path(&destination, base)?;
            match source {
                None => {
                    if let Some(current) = secure::read(&destination, MAX_FILE_BYTES)? {
                        if !secure::remove_matching(&destination, &current, MAX_FILE_BYTES)? {
                            return Err(denied("Base test deletion lost ownership"));
                        }
                    }
                }
                Some(source) => {
                    files::create_private_directories(
                        destination
                            .parent()
                            .ok_or_else(|| denied("Overlay file needs a parent"))?,
                    )?;
                    if let Some(previous) = secure::read(&destination, MAX_FILE_BYTES)? {
                        if !secure::remove_matching(&destination, &previous, MAX_FILE_BYTES)? {
                            return Err(denied("Base test overlay lost ownership"));
                        }
                    }
                    secure::write_exclusive(&destination, source.to_utf8()?.as_bytes())?;
                }
            }
        }
        self.check(false)
    }

    pub(crate) async fn close(self) -> Result<(), Error> {
        let _scope = self.owner.span(timing::Label::ScratchClose);
        if !self.owner.cleanup_certain() {
            return Err(Error::new(
                "OwnedProcessCleanupError",
                "Scratch retained while descendant shutdown is unverified",
            )
            .coded("ROSTER_PROCESS_CLEANUP"));
        }
        self.check(true)?;
        let mut pending = vec![self.path.clone()];
        let mut count = 0;
        while let Some(directory) = pending.pop() {
            secure::check_components(&directory)?;
            for entry in fs::read_dir(&directory)
                .map_err(|error| io_error("Read scratch cleanup entry", &directory, error))?
            {
                let entry = entry
                    .map_err(|error| io_error("Read scratch cleanup entry", &directory, error))?;
                count += 1;
                if count > 65_536 {
                    return Err(denied("Owned scratch cleanup exceeds its entry bound"));
                }
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|error| io_error("Inspect scratch cleanup", &entry.path(), error))?;
                if secure::linked(&metadata) {
                    return Err(denied(
                        "Scratch cleanup encountered a link or junction; preserve ownership",
                    ));
                }
                if metadata.is_dir() {
                    pending.push(entry.path());
                }
            }
        }
        self.check(true)?;
        fs::remove_dir_all(&self.path)
            .map_err(|error| io_error("Remove owned execution scratch", &self.path, error))
    }
}

#[cfg(all(test, windows))]
#[path = "scratch/blob_tests.rs"]
mod blob_tests;
