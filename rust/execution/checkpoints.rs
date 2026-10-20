use std::collections::BTreeMap;
use std::fs;

use super::*;
use crate::identity::sha256_hex;
use crate::state::{files, secure};
use tools::{Boundary, bytes_text};

pub(super) const SUBJECT_ARGS: [&str; 5] = [
    "for-each-ref",
    "--count=1",
    "--sort=-HEAD",
    "--format=%(HEAD)%00%(objectname)%00%(objecttype)%00%(refname)",
    "refs/heads/",
];

struct GitSubject {
    head: String,
    branch: String,
}

impl GitSubject {
    fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if !u64::try_from(bytes.len()).is_ok_and(|length| length <= MAX_FILE_BYTES) {
            return Err(denied("Verified checkpoint requires bounded Git output"));
        }
        let output = bytes_text(bytes)?;
        let mut lines = output.lines();
        let (Some(record), None) = (lines.next(), lines.next()) else {
            return Err(denied(
                "Verified checkpoint requires a committed task branch",
            ));
        };
        let mut fields = record.split('\0');
        let (Some("*"), Some(head), Some("commit"), Some(reference), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(denied(
                "Verified checkpoint requires a committed task branch",
            ));
        };
        let branch = reference
            .strip_prefix("refs/heads/")
            .ok_or_else(|| denied("Verified checkpoint requires a committed task branch"))?
            .trim();
        if !oid(head) || branch.is_empty() || branch.contains(['\r', '\n', '\0']) {
            return Err(denied(
                "Verified checkpoint requires a committed task branch",
            ));
        }
        Ok(Self {
            head: head.to_owned(),
            branch: branch.to_owned(),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct IndexSeal {
    files: Vec<(std::path::PathBuf, Option<secure::Snapshot>)>,
}

impl IndexSeal {
    pub(crate) fn capture(root: &std::path::Path, primary: &str) -> Result<Self, Error> {
        if primary.is_empty() {
            return Err(denied("Original Git index path is unavailable"));
        }
        let mut seal = Self { files: Vec::new() };
        seal.include_shared(root, primary)?;
        Ok(seal)
    }

    pub(crate) fn include_shared(
        &mut self,
        root: &std::path::Path,
        path: &str,
    ) -> Result<(), Error> {
        if path.is_empty() {
            return Ok(());
        }
        let path = crate::paths::resolve(root, std::path::Path::new(path))?;
        secure::check_components(&path)?;
        let snapshot = secure::read(&path, MAX_FILE_BYTES)?;
        if snapshot.is_some() && crate::paths::file_information(&path)?.links != 1 {
            return Err(denied(
                "Original Git index must be a single-link regular file",
            ));
        }
        self.files.push((path, snapshot));
        Ok(())
    }

    pub(crate) fn require_current(&self) -> Result<(), Error> {
        for (path, expected) in &self.files {
            let current = secure::read(path, MAX_FILE_BYTES)?;
            match (expected, current) {
                (None, None) => {}
                (Some(expected), Some(current))
                    if expected.identity == current.identity
                        && expected.bytes == current.bytes
                        && crate::paths::file_information(path)?.links == 1 => {}
                _ => {
                    return Err(denied(
                        "Original worktree Git index changed; pending effects and continuation are revoked",
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(crate) struct GitSeal {
    validation: Value,
    indexes: IndexSeal,
}

impl GitSeal {
    pub(crate) fn capture<'a>(
        boundary: &'a Boundary,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self, Error>> + 'a>> {
        Box::pin(async move {
            let original = validation(boundary).await?;
            let primary = bytes_text(
                &boundary
                    .git(
                        &["rev-parse", "--git-path", "index"].map(str::to_owned),
                        &BTreeMap::new(),
                    )
                    .await?,
            )?;
            let mut indexes = IndexSeal::capture(boundary.root(), primary.trim())?;
            let shared = bytes_text(
                &boundary
                    .git(
                        &["rev-parse", "--shared-index-path"].map(str::to_owned),
                        &BTreeMap::new(),
                    )
                    .await?,
            )?;
            indexes.include_shared(boundary.root(), shared.trim())?;
            let seal = Self {
                validation: original,
                indexes,
            };
            seal.require_current(boundary).await?;
            Ok(seal)
        })
    }

    pub(crate) fn validation(&self) -> &Value {
        &self.validation
    }

    pub(crate) fn require_current<'a>(
        &'a self,
        boundary: &'a Boundary,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + 'a>> {
        Box::pin(async move {
            let _scope = boundary.span(super::timing::Label::GitValidate);
            boundary.check()?;
            {
                let scope = boundary.span(super::timing::Label::GitIndex);
                let result = self.indexes.require_current();
                super::timing::finish(scope, result.as_ref().err());
                result?;
            }
            if self.validation != validation(boundary).await? {
                return Err(denied(
                    "Original Git HEAD, branch or task binding changed; pending effects and continuation are revoked",
                ));
            }
            {
                let scope = boundary.span(super::timing::Label::GitIndex);
                let result = self.indexes.require_current();
                super::timing::finish(scope, result.as_ref().err());
                result?;
            }
            boundary.check()
        })
    }
}

fn oid(value: &str) -> bool {
    [40, 64].contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hash(value: Option<&Value>) -> bool {
    value.and_then(Value::as_string).is_some_and(|value| {
        value.0.len() == 64
            && value
                .0
                .iter()
                .all(|unit| matches!(unit, 48..=57 | 97..=102))
    })
}

fn identity(task: &str) -> Result<&str, Error> {
    if task.is_empty()
        || task.len() > 64
        || !task
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !task
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(Error::type_error(
            "Checkpoint task must be an opaque identifier",
        ));
    }
    let identity = task.strip_prefix("issue-").unwrap_or(task);
    if identity.is_empty() {
        return Err(Error::type_error(
            "Checkpoint task must be an opaque identifier",
        ));
    }
    Ok(identity)
}

pub async fn validation(boundary: &Boundary) -> Result<Value, Error> {
    boundary.check()?;
    let task = &boundary.assignment().task;
    if regex(r"^issue-[1-9][0-9]*$")?.is_match(task)
        && task
            .strip_prefix("issue-")
            .is_some_and(|number| boundary.assignment().reference != format!("issue:{number}"))
    {
        return Err(Error::new(
            "Error",
            "Verified checkpoint recipe does not match task identity",
        ));
    }
    let mut sources = Vec::new();
    for name in ["TASK.md", "RECIPE.yml", "ASSIGNMENT.md"] {
        let file = boundary.root().join(name);
        crate::paths::ensure_local_path(&file, boundary.root())?;
        let snapshot = secure::read(&file, 65_536)?;
        let value = match snapshot {
            None if name == "ASSIGNMENT.md" => Value::Null,
            None => {
                return Err(denied(
                    "Verified checkpoint requires bounded task artifacts",
                ));
            }
            Some(snapshot) => {
                if crate::paths::file_information(&file)?.links != 1 {
                    return Err(denied(
                        "Verified checkpoint requires bounded single-link task artifacts",
                    ));
                }
                Value::string(&bytes_text(&snapshot.bytes)?)
            }
        };
        sources.push(value);
    }
    let GitSubject { head, branch } = GitSubject::parse(
        &boundary
            .git(&SUBJECT_ARGS.map(str::to_owned), &BTreeMap::new())
            .await?,
    )?;
    let location =
        crate::paths::strip_verbatim(fs::canonicalize(boundary.root()).map_err(|error| {
            secure::io_error("Resolve checkpoint owner", boundary.root(), error)
        })?);
    let location = location
        .to_str()
        .ok_or_else(|| Error::type_error("Checkpoint owner must be Unicode"))?;
    let location = if cfg!(windows) {
        location.to_lowercase()
    } else {
        location.to_owned()
    };
    boundary.check()?;
    Ok(Value::object(vec![
        ("head", Value::string(&head)),
        ("branch", Value::string(&branch)),
        (
            "taskSource",
            Value::string(&sha256_hex(Value::Array(sources).json().as_bytes())),
        ),
        ("owner", Value::string(&sha256_hex(location.as_bytes()))),
    ]))
}

pub fn list(boundary: &Boundary) -> Result<Vec<Value>, Error> {
    boundary.check()?;
    let directory =
        boundary.state_directory(&["checkpoints", identity(&boundary.assignment().task)?])?;
    crate::paths::ensure_local_path(&directory, boundary.root())?;
    let mut numbers = Vec::new();
    for name in secure::entries(&directory)? {
        if !name.is_empty()
            && !name.starts_with('0')
            && name.bytes().all(|byte| byte.is_ascii_digit())
        {
            let number = name
                .parse::<u64>()
                .ok()
                .filter(|number| (1..=crate::MAX_SAFE_INTEGER).contains(number))
                .ok_or_else(|| {
                    Error::new(
                        "Error",
                        "Checkpoint metadata does not match the requested task",
                    )
                })?;
            numbers.push((number, name));
        }
    }
    if numbers.len() > MAX_FILES {
        return Err(denied("Checkpoint metadata exceeds 2048 records"));
    }
    numbers.sort_by_key(|(number, _)| *number);
    let mut records = Vec::new();
    for (number, name) in numbers {
        let file = directory.join(name);
        let snapshot = secure::read(&file, 262_144)?
            .ok_or_else(|| Error::new("Error", "Checkpoint metadata disappeared"))?;
        if crate::paths::file_information(&file)?.links != 1 {
            return Err(Error::new(
                "Error",
                "Checkpoint metadata must be a bounded regular single-link file",
            ));
        }
        let record = crate::json::parse(&Text::from(bytes_text(&snapshot.bytes)?.as_str()))
            .map_err(|_| Error::new("Error", "Checkpoint metadata is invalid"))?;
        let tree = record
            .get("tree")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_default();
        let status = record
            .get("status")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_default();
        let time = record
            .get("time")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_default();
        let scope = record.get("scope").and_then(domain::items);
        if domain::safe_integer(record.get("number")) != Some(number)
            || !domain::is_text(record.get("task"), &boundary.assignment().task)
            || !domain::is_text(record.get("seat"), "coder")
            || !["before-write", "verified"].contains(&status.as_str())
            || !oid(&tree)
            || !domain::is_text(
                record.get("ref"),
                &format!(
                    "refs/roster/checkpoints/{}/{number}",
                    boundary.assignment().task
                ),
            )
            || scope.is_none_or(|scope| {
                scope.len() > MAX_SCOPE_ENTRIES
                    || scope.iter().any(|file| file.as_string().is_none())
            })
            || !regex(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$")?.is_match(&time)
        {
            return Err(Error::new(
                "Error",
                "Checkpoint metadata does not match the requested task",
            ));
        }
        if status == "verified" {
            let validation = record.get("validation");
            let head = validation
                .and_then(|value| value.get("head"))
                .and_then(Value::as_string)
                .map(Text::to_utf8)
                .transpose()?
                .unwrap_or_default();
            let branch = validation
                .and_then(|value| value.get("branch"))
                .and_then(Value::as_string);
            if !oid(&head)
                || branch.is_none_or(|branch| branch.0.is_empty())
                || !hash(validation.and_then(|value| value.get("taskSource")))
                || !hash(validation.and_then(|value| value.get("owner")))
            {
                return Err(Error::new(
                    "Error",
                    "Verified checkpoint metadata is invalid",
                ));
            }
        }
        records.push(record);
    }
    boundary.check()?;
    Ok(records)
}

async fn ignored_directory(boundary: &Boundary, directory: &std::path::Path) -> Result<(), Error> {
    let relative = directory
        .strip_prefix(boundary.root())
        .map_err(|_| denied("Checkpoint directory left the worktree"))?;
    let mut probe = relative
        .to_str()
        .ok_or_else(|| Error::type_error("Checkpoint directory must be Unicode"))?
        .replace('\\', "/");
    probe.push_str("/1");
    boundary.git(&["check-ignore".to_owned(), "--no-index".to_owned(), "--quiet".to_owned(), "--".to_owned(), probe], &BTreeMap::new()).await
        .map_err(|cause| denied("Checkpoint runtime state must already be Git-ignored; native tools do not modify ignore rules").caused_by(cause))?;
    Ok(())
}

pub async fn capture(boundary: &Boundary, verified: bool) -> Result<Value, Error> {
    boundary.check()?;
    let scope = boundary.scope()?;
    if scope.len() > MAX_SCOPE_ENTRIES {
        return Err(denied(
            "Checkpoint scope exceeds 64 entries; no checkpoint was written",
        ));
    }
    if boundary.role() != Role::Coder {
        return Err(denied(
            "Only the coder host can capture product checkpoints",
        ));
    }
    boundary.require_ignored(".roster/locks/run.lock").await?;
    let validation = if verified {
        let tests = boundary
            .last_tests()
            .filter(|tests| {
                tests.result.get("exit_code") == Some(&Value::Number(0.0))
                    && tests.result.get("skipped") != Some(&Value::Boolean(true))
            })
            .ok_or_else(|| {
                Error::new(
                    "Error",
                    "Verified checkpoint requires actual passing tests, not skipped evidence",
                )
            })?;
        tests.require_current_coverage(boundary)?;
        let mut files = boundary.scope()?;
        files.extend(boundary.written_files());
        if policy::requires_native(&files)? && !tests.native {
            return Err(denied(
                "Rust checkpoint requires observed host-approved Cargo checks",
            ));
        }
        if tests.authority != boundary.authority_digest()
            || tests.snapshot != boundary.snapshot()?
        {
            return Err(denied(
                "Product changed after verified checks; green checkpoint refused",
            ));
        }
        if tests.audit != boundary.audit()? {
            return Err(denied(
                "Worktree changed after verified checks; green checkpoint refused",
            ));
        }
        let validation = self::validation(boundary).await?;
        if tests.validation.as_ref() != Some(&validation) {
            return Err(denied(
                "Task branch or revision changed after verified checks; green checkpoint refused",
            ));
        }
        Some(validation)
    } else {
        None
    };
    let task = &boundary.assignment().task;
    if crate::planning::memory::redact(&Text::from(task.as_str()), boundary.redaction())
        != Text::from(task.as_str())
    {
        return Err(denied(
            "Checkpoint identifiers must not contain secret material",
        ));
    }
    if scope.iter().any(|file| {
        crate::planning::memory::redact(&Text::from(file.as_str()), boundary.redaction())
            != Text::from(file.as_str())
    }) {
        return Err(denied(
            "Checkpoint scope must contain public task paths, not secret material",
        ));
    }
    let records = list(boundary)?;
    let number = records
        .last()
        .and_then(|record| domain::safe_integer(record.get("number")))
        .unwrap_or(0)
        .checked_add(1)
        .filter(|number| *number <= crate::MAX_SAFE_INTEGER)
        .ok_or_else(|| denied("Checkpoint number exceeds its safe-integer bound"))?;
    let directory = boundary.state_directory(&["checkpoints", identity(task)?])?;
    ignored_directory(boundary, &directory).await?;
    boundary.check()?;
    secure::private_directory(&directory, true)?;
    let index = directory.join(format!("index-{}", files::nonce()?));
    secure::check_components(&index)?;
    let environment = BTreeMap::from([("GIT_INDEX_FILE".to_owned(), index.display().to_string())]);
    let capture = async {
        boundary
            .git(&["read-tree", "--empty"].map(str::to_owned), &environment)
            .await?;
        let expected = boundary.snapshot()?;
        if verified
            && boundary
                .last_tests()
                .is_none_or(|tests| tests.snapshot != expected)
        {
            return Err(denied(
                "Product changed after verified checks; green checkpoint refused",
            ));
        }
        for (file, product) in &expected.files {
            boundary.check()?;
            let target = boundary.root().join(file);
            let content = secure::read(&target, MAX_FILE_BYTES)?
                .ok_or_else(|| denied("Checkpoint product disappeared"))?;
            if content.identity != product.identity
                || sha256_hex(&content.bytes) != product.digest
                || crate::paths::file_information(&target)?.links != 1
            {
                return Err(denied("Product changed during checkpoint capture"));
            }
            let hash = bytes_text(
                &boundary
                    .git(
                        &[
                            "hash-object".to_owned(),
                            "-w".to_owned(),
                            "--no-filters".to_owned(),
                            "--".to_owned(),
                            file.clone(),
                        ],
                        &BTreeMap::new(),
                    )
                    .await?,
            )?
            .trim()
            .to_owned();
            if !oid(&hash) {
                return Err(denied("Checkpoint Git blob identity is invalid"));
            }
            let blob = boundary
                .git(
                    &["cat-file".to_owned(), "blob".to_owned(), hash.clone()],
                    &BTreeMap::new(),
                )
                .await?;
            if blob != content.bytes || !secure::unchanged(&target, &content, MAX_FILE_BYTES)? {
                return Err(denied("Product changed during checkpoint capture"));
            }
            boundary
                .git(
                    &[
                        "update-index".to_owned(),
                        "--add".to_owned(),
                        "--cacheinfo".to_owned(),
                        format!("{},{hash},{file}", product.mode),
                    ],
                    &environment,
                )
                .await?;
        }
        if boundary.snapshot()? != expected {
            return Err(denied("Product set changed during checkpoint capture"));
        }
        let tree = bytes_text(
            &boundary
                .git(&["write-tree".to_owned()], &environment)
                .await?,
        )?
        .trim()
        .to_owned();
        if !oid(&tree) {
            return Err(denied("Checkpoint Git tree identity is invalid"));
        }
        let reference = format!("refs/roster/checkpoints/{task}/{number}");
        let mut record = Value::object(vec![
            ("number", Value::Number(number as f64)),
            ("task", Value::string(task)),
            ("seat", Value::string("coder")),
            (
                "status",
                Value::string(if verified { "verified" } else { "before-write" }),
            ),
            ("time", Value::string(&crate::clock::now_iso()?)),
            ("ref", Value::string(&reference)),
            ("tree", Value::string(&tree)),
            ("scope", strings(&scope)),
        ]);
        if let Some(validation) = &validation {
            if self::validation(boundary).await? != *validation {
                return Err(denied(
                    "Task identity changed during verified checkpoint capture",
                ));
            }
            domain::put(&mut record, "validation", validation.clone())?;
        }
        boundary.check()?;
        boundary
            .git(
                &[
                    "update-ref".to_owned(),
                    reference,
                    tree.clone(),
                    "0".repeat(tree.len()),
                ],
                &BTreeMap::new(),
            )
            .await.map_err(|cause| if cause.code == Some("ROSTER_GIT_FAILED") { Error::new("CheckpointError", "Checkpoint ref could not be created exclusively; preserve orphaned or conflicting refs for explicit recovery.").coded("ROSTER_CHECKPOINT_CONFLICT").caused_by(cause) } else { cause })?;
        boundary.check()?;
        let data = format!("{}\n", record.json());
        secure::commit_new(&directory.join(number.to_string()), data.as_bytes())?;
        secure::sync_directory(&directory)?;
        boundary.check()?;
        Ok(record)
    }
    .await;
    let cleanup = (|| match secure::read(&index, MAX_FILE_BYTES)? {
        None => Ok(()),
        Some(snapshot) => {
            if crate::paths::file_information(&index)?.links != 1
                || !secure::remove_matching(&index, &snapshot, MAX_FILE_BYTES)?
            {
                Err(denied(
                    "Checkpoint index cleanup could not be verified; retain state for inspection",
                ))
            } else {
                Ok(())
            }
        }
    })();
    if cleanup.is_err() {
        boundary.quarantine_owned();
    }
    crate::state::locks::finish(capture, cleanup)
}

pub async fn validate_green(boundary: &Boundary, number: Option<u64>) -> Result<Value, Error> {
    boundary.check()?;
    let records = list(boundary)?;
    let record = records
        .iter()
        .rev()
        .find(|record| {
            domain::is_text(record.get("status"), "verified")
                && number
                    .is_none_or(|number| domain::safe_integer(record.get("number")) == Some(number))
        })
        .ok_or_else(|| Error::new("Error", "No verified green checkpoint exists for this task"))?;
    if record.get("validation") != Some(&validation(boundary).await?)
        || record.get("scope") != Some(&strings(&boundary.scope()?))
    {
        return Err(Error::new(
            "Error",
            "Verified checkpoint task, scope, branch, revision or worktree identity changed; restoration is refused",
        ));
    }
    validate_ref(boundary, record).await?;
    boundary.check()?;
    Ok(record.clone())
}

pub(crate) async fn validate_ref(boundary: &Boundary, record: &Value) -> Result<(), Error> {
    let reference = text(required(record, "ref")?, "Checkpoint requires a ref")?;
    let tree = text(required(record, "tree")?, "Checkpoint requires a tree")?;
    let current = bytes_text(
        &boundary
            .git(
                &["rev-parse".to_owned(), "--verify".to_owned(), reference],
                &BTreeMap::new(),
            )
            .await?,
    )?;
    if current.trim() != tree {
        return Err(Error::new(
            "Error",
            "Checkpoint ref changed; restoration is refused",
        ));
    }
    boundary.check()?;
    Ok(())
}

pub fn require_restore_integration() -> Result<(), Error> {
    Err(denied(
        "Checkpoint restore requires a current native prepare/confirm approval with explicit unpublished-branch authority; the model has no restore grant",
    ))
}

pub async fn prepare_restore(
    boundary: &Boundary,
    number: Option<u64>,
) -> Result<super::restore::Plan, Error> {
    super::restore::prepare(boundary, number).await
}

#[cfg(test)]
mod query_parser_tests {
    use super::*;

    #[test]
    fn query_parser_accepts_full_width_commits_and_actual_branch_displays() -> Result<(), Error> {
        for width in [40, 64] {
            let head = "a".repeat(width);
            for branch in [
                "(detached)",
                "heads/topic",
                "refs/heads/topic",
                "révision/東京",
            ] {
                let source = format!("*\0{head}\0commit\0refs/heads/{branch}\n");
                let subject = GitSubject::parse(source.as_bytes())?;
                assert_eq!(subject.head, head);
                assert_eq!(subject.branch, branch);
            }
        }
        Ok(())
    }

    #[test]
    fn query_parser_rejects_missing_extra_records_or_fields() {
        let head = "a".repeat(40);
        let valid = format!("*\0{head}\0commit\0refs/heads/topic");
        for source in [
            String::new(),
            "*".to_owned(),
            format!("*\0{head}\0commit"),
            format!("{valid}\0extra\n"),
            format!("{valid}\n\n"),
            format!("{valid}\n{valid}\n"),
            format!("extra\n{valid}\n"),
        ] {
            assert!(GitSubject::parse(source.as_bytes()).is_err_and(|error| {
                error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
            }));
        }
    }

    #[test]
    fn query_parser_rejects_noncurrent_markers_and_noncommit_objects() {
        let head = "a".repeat(40);
        for marker in ["", " ", "+", "HEAD", "**"] {
            let source = format!("{marker}\0{head}\0commit\0refs/heads/topic\n");
            assert!(GitSubject::parse(source.as_bytes()).is_err());
        }
        for kind in ["", "tag", "tree", "blob", "Commit"] {
            let source = format!("*\0{head}\0{kind}\0refs/heads/topic\n");
            assert!(GitSubject::parse(source.as_bytes()).is_err());
        }
    }

    #[test]
    fn query_parser_rejects_nonhead_refs_and_invalid_object_identifiers() {
        let head = "a".repeat(40);
        for reference in ["HEAD", "refs/tags/HEAD", "refs/heads/", "refs/heads/a\0b"] {
            let source = format!("*\0{head}\0commit\0{reference}\n");
            assert!(GitSubject::parse(source.as_bytes()).is_err());
        }
        for head in [
            "a".repeat(39),
            "a".repeat(41),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(40),
            "g".repeat(40),
        ] {
            let source = format!("*\0{head}\0commit\0refs/heads/topic\n");
            assert!(GitSubject::parse(source.as_bytes()).is_err());
        }
    }

    #[test]
    fn query_parser_rejects_invalid_utf8_and_oversized_output() -> Result<(), Error> {
        assert!(GitSubject::parse(&[0xff]).is_err_and(|error| error.name == "TypeError"));
        let length = usize::try_from(MAX_FILE_BYTES)
            .ok()
            .and_then(|length| length.checked_add(1))
            .ok_or_else(|| Error::type_error("Owned query size bound must fit usize"))?;
        assert!(GitSubject::parse(&vec![0; length]).is_err_and(|error| {
            error.name == "ToolAccessError" && error.code == Some("ROSTER_TOOL_DENIED")
        }));
        Ok(())
    }
}
