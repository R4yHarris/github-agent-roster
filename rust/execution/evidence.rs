use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::identity::sha256_hex;
use crate::paths::{self, FileIdentity};
use crate::planning;
use crate::state::secure;

pub const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;

fn append_diff(diff: &mut String, source: &str) -> Result<(), Error> {
    if diff
        .len()
        .checked_add(source.len())
        .is_none_or(|size| size > MAX_DIFF_BYTES)
    {
        return Err(denied(
            "Combined tracked/untracked diff exceeds the bounded 4 MiB evidence limit",
        ));
    }
    diff.push_str(source);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedStamp {
    pub identity: FileIdentity,
    pub size: u64,
    pub mtime_ns: i128,
    pub ctime_ns: i128,
}

#[derive(Debug, Clone, Default)]
pub struct Projection {
    pub protected: BTreeMap<String, ProtectedStamp>,
    pub order: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    root: FileIdentity,
    entries: BTreeMap<String, String>,
    binding: BTreeMap<String, String>,
    projection_failure: Option<Error>,
    source: Option<String>,
}

impl Snapshot {
    pub(crate) fn after_owned_context_archive(mut self) -> Self {
        for name in ["CONTEXT.md", "RESEARCH.md"] {
            self.entries.remove(name);
            self.binding.remove(name);
        }
        self.source = None;
        self
    }

    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.entries
    }
    pub fn as_value(&self) -> Value {
        Value::Array(
            self.entries
                .iter()
                .map(|(name, stamp)| Value::Array(vec![Value::string(name), Value::string(stamp)]))
                .collect(),
        )
    }
    pub fn source(&self) -> Result<&str, Error> {
        self.source.as_deref().ok_or_else(|| {
            self.projection_failure
                .clone()
                .unwrap_or_else(|| unsupported("Acceptance-source projection"))
        })
    }
    pub fn projection_failure(&self) -> Option<&Error> {
        self.projection_failure.as_ref()
    }
    pub fn same_source(&self, other: &Self) -> bool {
        self.root == other.root && self.entries == other.entries && self.binding == other.binding
    }
    pub fn changed(&self, other: &Self) -> Result<Vec<String>, Error> {
        if self.root != other.root {
            return Err(denied(
                "Evidence snapshots belong to different worktree identities",
            ));
        }
        let files = self
            .entries
            .keys()
            .chain(other.entries.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        Ok(files
            .into_iter()
            .filter(|file| self.entries.get(file) != other.entries.get(file))
            .collect())
    }
}

pub fn ignored_notebook(file: &str, memory: Option<&str>) -> bool {
    let lower = file.to_lowercase();
    let parts = lower.split('/').collect::<Vec<_>>();
    parts.first() == Some(&".roster")
        && (matches!(
            parts.get(1),
            Some(&"logs" | &"history" | &"checkpoints" | &"memory")
        ) || parts
            .get(1)
            .is_some_and(|part| part.starts_with("history."))
            || lower == ".roster/map.md"
            || parts.len() == 3
                && parts.get(1) == Some(&"runs")
                && parts.last().is_some_and(|part| part.ends_with(".log")))
        || memory == Some(file)
}

fn time_ns(metadata: &fs::Metadata) -> Result<i128, Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok((i128::from(metadata.last_write_time()) - 116_444_736_000_000_000) * 100)
    }
}

fn protected_stamp(
    path: &Path,
    name: &str,
    metadata: &fs::Metadata,
    projection: &Projection,
) -> Result<ProtectedStamp, Error> {
    let identity = paths::file_information(path)?.identity;
    let size = metadata.len();
    let mtime_ns = time_ns(metadata)?;
    if let Some(stamp) = projection.protected.get(name) {
        if stamp.identity != identity || stamp.size != size || stamp.mtime_ns != mtime_ns {
            return Err(denied(
                "Trusted protected metadata is stale or bound to another file",
            ));
        }
        return Ok(stamp.clone());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(ProtectedStamp {
            identity,
            size,
            mtime_ns,
            ctime_ns: i128::from(metadata.ctime()) * 1_000_000_000
                + i128::from(metadata.ctime_nsec()),
        })
    }
    #[cfg(windows)]
    {
        Err(unsupported(
            "Exact Windows protected ctimeNs requires a current trusted-host metadata projection",
        ))
    }
}

fn ascii_key(name: &str) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let punctuation = b" _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    let mut primary = Vec::new();
    let mut tertiary = Vec::new();
    for byte in name.bytes() {
        let weight = if let Some(index) =
            punctuation.iter().position(|candidate| *candidate == byte)
        {
            index as u8
        } else if byte.is_ascii_digit() {
            40 + byte - b'0'
        } else if byte.is_ascii_alphabetic() {
            60 + byte.to_ascii_lowercase() - b'a'
        } else {
            return Err(unsupported(
                "Acceptance filename collation requires an explicit trusted-host order outside the verified en-US ASCII profile",
            ));
        };
        primary.push(weight);
        tertiary.push(u8::from(byte.is_ascii_uppercase()));
    }

    Ok((primary, tertiary))
}

pub(crate) fn name_key(name: &str) -> Result<(Vec<u8>, Vec<u8>), Error> {
    ascii_key(name)
}

pub fn source_identity(
    entries: &BTreeMap<String, String>,
    projection: &Projection,
) -> Result<String, Error> {
    let mut names = entries
        .keys()
        .filter(|file| !policy::managed(file))
        .cloned()
        .collect::<Vec<_>>();
    if let Some(order) = &projection.order {
        let wanted = names.iter().cloned().collect::<BTreeSet<_>>();
        let supplied = order.iter().cloned().collect::<BTreeSet<_>>();
        if wanted != supplied || order.len() != names.len() {
            return Err(denied(
                "Trusted acceptance collation must contain every current non-managed filename exactly once",
            ));
        }
        names.clone_from(order);
    } else {
        let mut keyed = names
            .into_iter()
            .map(|name| ascii_key(&name).map(|key| (key, name)))
            .collect::<Result<Vec<_>, _>>()?;
        keyed.sort_by(|left, right| left.0.cmp(&right.0));
        names = keyed.into_iter().map(|(_, name)| name).collect();
    }
    let value = Value::Array(
        names
            .iter()
            .map(|name| {
                Value::Array(vec![
                    Value::string(name),
                    Value::string(entries.get(name).map_or("", String::as_str)),
                ])
            })
            .collect(),
    );
    Ok(sha256_hex(value.json().as_bytes()))
}

pub fn capture(
    root: &Path,
    memory: Option<&str>,
    projection: &Projection,
) -> Result<Snapshot, Error> {
    let root = paths::absolute(root)?;
    secure::check_components(&root)?;
    let root_identity = paths::file_information(&root)?.identity;
    let mut queue = vec![PathBuf::new()];
    let mut entries = BTreeMap::new();
    let mut binding = BTreeMap::new();
    let mut failure = None;
    let mut used = 0_u64;
    let mut count = 0;
    while let Some(directory) = queue.pop() {
        let target = root.join(&directory);
        secure::check_components(&target)?;
        for entry in fs::read_dir(&target)
            .map_err(|error| secure::io_error("Read evidence directory", &target, error))?
        {
            let entry =
                entry.map_err(|error| secure::io_error("Read evidence entry", &target, error))?;
            let relative = directory.join(entry.file_name());
            let name = relative
                .to_str()
                .ok_or_else(|| Error::type_error("Evidence filenames must contain valid Unicode"))?
                .replace('\\', "/");
            if name == ".git"
                || ["RESULT.md", "REVIEW.md"].contains(&name.as_str())
                || ignored_notebook(&name, memory)
            {
                continue;
            }
            count += 1;
            if count > 8192 {
                return Err(denied("Full worktree evidence exceeds 8192 entries"));
            }
            let file = root.join(&relative);
            let metadata = fs::symlink_metadata(&file)
                .map_err(|error| secure::io_error("Inspect evidence entry", &file, error))?;
            let information = paths::file_information(&file)?;
            let stamp = if planning::forbidden_read(&name) {
                match protected_stamp(&file, &name, &metadata, projection) {
                    Ok(stamp) => format!(
                        "protected:{}:{}:{}:{}",
                        stamp.identity.index, stamp.size, stamp.mtime_ns, stamp.ctime_ns
                    ),
                    Err(error) => {
                        failure = failure.or(Some(error));
                        format!(
                            "unavailable-protected:{}:{}:{}",
                            information.identity.index,
                            metadata.len(),
                            time_ns(&metadata)?
                        )
                    }
                }
            } else if secure::linked(&metadata) {
                let target = fs::read_link(&file)
                    .map_err(|error| secure::io_error("Read evidence link", &file, error))?;
                format!(
                    "symlink:{}",
                    target.to_str().ok_or_else(|| Error::type_error(
                        "Evidence link target must contain valid Unicode"
                    ))?
                )
            } else if metadata.is_dir() {
                String::new()
            } else if metadata.is_file() {
                paths::ensure_local_path(&file, &root)?;
                let snapshot = secure::read(&file, MAX_FILE_BYTES)?
                    .ok_or_else(|| denied("Evidence file disappeared"))?;
                if information.links != 1
                    || snapshot.identity != information.identity
                    || !secure::unchanged(&file, &snapshot, MAX_FILE_BYTES)?
                {
                    return Err(denied("Evidence requires stable regular single-link files"));
                }
                used += snapshot.bytes.len() as u64;
                if used > MAX_SNAPSHOT_BYTES {
                    return Err(denied("Full worktree evidence exceeds 128 MiB"));
                }
                sha256_hex(&snapshot.bytes)
            } else {
                #[cfg(unix)]
                let mode = {
                    use std::os::unix::fs::MetadataExt;
                    metadata.mode()
                };
                #[cfg(windows)]
                let mode = 0;
                format!("special:{mode}")
            };
            if !stamp.is_empty() {
                binding.insert(
                    name.clone(),
                    format!(
                        "{:?}:{}:{}",
                        information.identity,
                        information.links,
                        time_ns(&metadata)?
                    ),
                );
                entries.insert(name.clone(), stamp);
            }
            if metadata.is_dir() && !secure::linked(&metadata) {
                queue.push(relative);
            }
        }
    }
    if paths::file_information(&root)?.identity != root_identity {
        return Err(denied("Evidence worktree identity changed during capture"));
    }
    let source = if failure.is_none() {
        match source_identity(&entries, projection) {
            Ok(source) => Some(source),
            Err(error) => {
                failure = Some(error);
                None
            }
        }
    } else {
        None
    };
    Ok(Snapshot {
        root: root_identity,
        entries,
        binding,
        projection_failure: failure,
        source,
    })
}

#[derive(Debug, Clone)]
pub struct Changes {
    pub(crate) files: Vec<String>,
    pub(crate) diff: Text,
    pub(crate) tracked: Vec<String>,
    pub(crate) validation: Value,
    audit: BTreeMap<String, String>,
    authority: String,
}

impl Changes {
    pub(crate) async fn head_product(
        &self,
        boundary: &tools::Boundary,
        file: &str,
    ) -> Result<Option<Text>, Error> {
        self.require_current(boundary)?;
        if !policy::exact_path(file) || planning::forbidden_read(file) {
            return Err(denied(
                "HEAD evidence requires an exact readable product path",
            ));
        }
        let head = text(
            required(&self.validation, "head")?,
            "Captured HEAD must be text",
        )?;
        let listed = tools::bytes_text(
            &boundary
                .git(
                    &[
                        "ls-tree",
                        "--full-tree",
                        "-r",
                        "--name-only",
                        "-z",
                        &head,
                        "--",
                        file,
                    ]
                    .map(str::to_owned),
                    &BTreeMap::new(),
                )
                .await?,
        )?;
        let names = listed
            .split('\0')
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>();
        let source = if names.is_empty() {
            None
        } else if names == [file] {
            Some(Text::from(
                tools::bytes_text(
                    &boundary
                        .git(
                            &["show".to_owned(), format!("{head}:{file}")],
                            &BTreeMap::new(),
                        )
                        .await?,
                )?
                .as_str(),
            ))
        } else {
            return Err(denied(
                "Captured HEAD lookup did not identify one exact product file",
            ));
        };
        self.require_current(boundary)?;
        Ok(source)
    }

    pub fn files(&self) -> &[String] {
        &self.files
    }
    pub fn diff(&self) -> &Text {
        &self.diff
    }
    pub fn tracked(&self) -> &[String] {
        &self.tracked
    }
    pub(crate) fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        boundary.check()?;
        if self.authority != boundary.authority_digest() || self.audit != boundary.audit()? {
            return Err(denied(
                "Safe tracked/untracked diff no longer matches its observed worktree and authority",
            ));
        }
        Ok(())
    }
}

pub async fn changes(boundary: &tools::Boundary, memory: Option<&str>) -> Result<Changes, Error> {
    boundary.check()?;
    let validation = checkpoints::validation(boundary).await?;
    let audit = boundary.audit()?;
    let git = |args: Vec<String>| async move {
        tools::bytes_text(&boundary.git(&args, &BTreeMap::new()).await?)
    };
    let tracked = git(vec!["ls-files".to_owned(), "-z".to_owned()])
        .await?
        .split('\0')
        .filter(|file| !file.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let modified = git([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--name-only",
        "-z",
        "HEAD",
        "--",
    ]
    .map(str::to_owned)
    .to_vec())
    .await?;
    let untracked = git(["ls-files", "--others", "--exclude-standard", "-z"]
        .map(str::to_owned)
        .to_vec())
    .await?;
    let mut files = Vec::new();
    for file in modified
        .split('\0')
        .chain(untracked.split('\0'))
        .filter(|file| !file.is_empty())
    {
        if !policy::exact_path(file) {
            return Err(denied(
                "Git evidence paths must be exact bounded worktree paths",
            ));
        }
        if !policy::managed(file)
            && !ignored_notebook(file, memory)
            && !files.iter().any(|seen| seen == file)
        {
            files.push(file.to_owned());
        }
    }
    if files.len() > MAX_FILES {
        return Err(denied("Changed evidence exceeds 2048 paths"));
    }
    let readable = files
        .iter()
        .filter(|file| !planning::forbidden_read(file))
        .cloned()
        .collect::<Vec<_>>();
    let mut diff = if readable.is_empty() {
        String::new()
    } else {
        let mut args = [
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--unified=0",
            "HEAD",
            "--",
        ]
        .map(str::to_owned)
        .to_vec();
        args.extend(readable.iter().cloned());
        git(args).await?
    };
    if diff.len() > MAX_DIFF_BYTES {
        return Err(denied(
            "Combined tracked/untracked diff exceeds the bounded 4 MiB evidence limit",
        ));
    }
    for file in untracked
        .split('\0')
        .filter(|file| !file.is_empty() && readable.iter().any(|entry| entry == file))
    {
        let Some(source) = read_product(boundary.root(), file, MAX_FILE_BYTES)? else {
            return Err(denied(
                "Untracked diff file disappeared during safe evidence capture",
            ));
        };
        if !diff.is_empty() && !diff.ends_with('\n') {
            append_diff(&mut diff, "\n")?;
        }
        let source = source.to_utf8()?;
        append_diff(
            &mut diff,
            &format!(
                "diff --git a/{file} b/{file}\n--- /dev/null\n+++ b/{file}\n@@ -0,0 +1,{} @@\n",
                source.split_terminator('\n').count()
            ),
        )?;
        for line in source.split_terminator('\n') {
            append_diff(&mut diff, "+")?;
            append_diff(&mut diff, line)?;
            append_diff(&mut diff, "\n")?;
        }
    }
    boundary.check()?;
    let changes = Changes {
        files,
        diff: Text::from(diff.as_str()),
        tracked,
        validation,
        audit,
        authority: boundary.authority_digest(),
    };
    changes.require_current(boundary)?;
    if changes.validation != checkpoints::validation(boundary).await? {
        return Err(denied(
            "Git or authoritative handoff changed during safe tracked/untracked diff capture",
        ));
    }
    Ok(changes)
}

pub fn read_product(root: &Path, file: &str, maximum: u64) -> Result<Option<Text>, Error> {
    if !policy::exact_path(file) || planning::forbidden_read(file) {
        return Err(denied(
            "Evidence cannot read protected or non-product paths",
        ));
    }
    let target = root.join(file);
    paths::ensure_local_path(&target, root)?;
    let Some(snapshot) = secure::read(&target, maximum)? else {
        return Ok(None);
    };
    if paths::file_information(&target)?.links != 1
        || !secure::unchanged(&target, &snapshot, maximum)?
    {
        return Err(denied("Evidence reads require stable single-link files"));
    }
    Ok(Some(Text::from(
        tools::bytes_text(&snapshot.bytes)?.as_str(),
    )))
}
