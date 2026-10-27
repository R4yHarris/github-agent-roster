use std::path::{Path, PathBuf};

use super::*;
use crate::cancel::CancellationToken;
use crate::planning::{self, estimate, intake, skills, task};
use crate::state::provenance::api::{ProvenanceStore, Query};
use crate::state::{redaction, secure};

pub const OMITTED: &str = "[Omitted by context budget]";
pub const OMITTED_RULES: &str = "(Publication, credential, and session-metadata rules are omitted: the harness publishes after review; this seat never commits, pushes, or opens a PR.)";
const CATEGORIES: &[&str] = &[
    "research",
    "implement",
    "test",
    "review",
    "incident",
    "docs",
];
const LANGUAGES: &[&str] = &["rust-implementation", "rust-cargo", "rust-tokio"];

#[derive(Debug, Clone)]
pub struct Section {
    pub heading: Text,
    pub body: Text,
    pub required: bool,
    pub recent: bool,
}

impl Section {
    pub fn required(heading: &str, body: Text) -> Self {
        Self {
            heading: Text::from(heading),
            body,
            required: true,
            recent: false,
        }
    }
}

fn render(sections: &[Section], bodies: &[Text], minimum: bool) -> Text {
    let mut output = Text::from("# Coder context\n\n");
    if minimum {
        output.0.extend(Text::from("Minimum team context: use the Ask, outcome, allowed files, checks, and supplied skills. The harness enforces tests, paths, secrets, read-only review, and human eval.\n\n").0);
    }
    for (index, (section, body)) in sections.iter().zip(bodies).enumerate() {
        if index != 0 {
            output.0.extend([10, 10]);
        }
        output.0.extend([35, 35, 32]);
        output.0.extend(&section.heading.0);
        output.0.extend([10, 10]);
        output.0.extend(&body.0);
    }
    output.0.push(10);
    output
}

pub fn bounded_pack(
    sections: &[Section],
    budget: usize,
    minimum: bool,
) -> Result<(Text, bool), Error> {
    let omitted = Text::from(OMITTED);
    let mut bodies = sections
        .iter()
        .map(|section| {
            if section.required || section.body.0.len() <= omitted.0.len() {
                section.body.clone()
            } else {
                omitted.clone()
            }
        })
        .collect::<Vec<_>>();
    if render(sections, &bodies, minimum).0.len() > budget {
        return Err(Error::new(
            "Error",
            "Required coder conduct, repository rules, TASK, skills and prior feedback exceed seat.context_chars; increase the context budget",
        ));
    }
    let mut truncated = false;
    for (index, section) in sections.iter().enumerate() {
        if section.required {
            continue;
        }
        let current = bodies
            .get(index)
            .ok_or_else(|| Error::type_error("Invalid native context section"))?
            .0
            .len();
        let available = budget - render(sections, &bodies, minimum).0.len() + current;
        let body = if section.body.0.len() <= available {
            section.body.clone()
        } else {
            truncated = true;
            let mut lines = section.body.0.split(|unit| *unit == 10).collect::<Vec<_>>();
            if section.recent {
                lines.reverse();
            }
            let mut kept = Vec::new();
            let mut used = 0;
            for line in lines {
                if used + line.len() + omitted.0.len() + 2 > available {
                    break;
                }
                used += line.len() + usize::from(!kept.is_empty());
                kept.push(line);
            }
            if section.recent {
                kept.reverse();
            }
            let mut body = Vec::new();
            for line in kept {
                if !body.is_empty() {
                    body.push(10);
                }
                body.extend(line);
            }
            if !body.is_empty() {
                body.push(10);
            }
            body.extend(&omitted.0);
            Text(body)
        };
        let slot = bodies
            .get_mut(index)
            .ok_or_else(|| Error::type_error("Invalid native context section"))?;
        *slot = body;
    }
    let pack = render(sections, &bodies, minimum);
    if pack.0.len() > budget {
        return Err(Error::type_error(
            "Native context exceeded its UTF-16 budget",
        ));
    }
    Ok((pack, truncated))
}

pub fn seat_rules(agents: &str) -> Result<String, Error> {
    let heading_omitted =
        regex(r"(?i)publish|publication|metadata|credential|contracts dependency|ghcp")?;
    let publication = regex(
        r"(?i)agent-pr|GITHUB_APP|gh pr create|git push|git commit|\bAI_[A-Z_]+|merge-when-green|draft PR|App credentials|private[- ]key|GHCP|publish",
    )?;
    let item = regex(r"^(?:[-*]|\d+\.) ")?;
    let text = agents.replace("\r\n", "\n");
    let mut sections = Vec::new();
    let mut section = Vec::new();
    for line in text.split('\n') {
        if line.starts_with("## ") && !section.is_empty() {
            sections.push(section.join("\n"));
            section.clear();
        }
        section.push(line);
    }
    sections.push(section.join("\n"));
    let mut kept = Vec::new();
    let mut omitted = false;
    for section in sections {
        let mut lines = section.split('\n');
        let first = lines.next().unwrap_or("");
        let heading = first.starts_with("## ").then_some(first);
        if heading.is_some_and(|heading| heading_omitted.is_match(heading)) {
            omitted = true;
            continue;
        }
        let body = if heading.is_some() {
            lines.collect::<Vec<_>>().join("\n")
        } else {
            section.clone()
        };
        let mut blocks = Vec::new();
        let mut current = Vec::new();
        for line in body.split('\n') {
            if (js_trim(line).is_empty() || item.is_match(line)) && !current.is_empty() {
                blocks.push(current.join("\n"));
                current.clear();
            }
            if !js_trim(line).is_empty() {
                current.push(line);
            }
        }
        if !current.is_empty() {
            blocks.push(current.join("\n"));
        }
        blocks.retain(|block| {
            let remove = publication.is_match(block);
            omitted |= remove;
            !remove
        });
        if heading.is_some() && blocks.is_empty() {
            continue;
        }
        let mut joined = String::new();
        let mut last_item = false;
        for block in blocks {
            let is_item = item.is_match(&block);
            if !joined.is_empty() {
                joined.push_str(if is_item && last_item { "\n" } else { "\n\n" });
            }
            joined.push_str(&block);
            last_item = is_item;
        }
        kept.push(heading.map_or(joined.clone(), |heading| format!("{heading}\n\n{joined}")));
    }
    let rules = js_trim(&kept.join("\n\n")).to_owned();
    Ok(if omitted {
        js_trim(&format!("{rules}\n\n{OMITTED_RULES}")).to_owned()
    } else {
        rules
    })
}

pub fn coder_skill_names(source: &Text) -> Result<Vec<String>, Error> {
    let mut names = skills::task_names(&Value::String(source.clone()))?;
    let document = task::parse(&Value::String(source.clone()), &task::Authority::default())?;
    let classification = intake::classify(
        &Value::String(document.ask.clone()),
        None,
        &document.files_allowed,
    )?;
    if !domain::is_text(classification.get("kind"), "slice")
        || !domain::is_text(classification.get("specKind"), "incident")
    {
        return Ok(names);
    }
    let cleaned = intake::clean_ask(&Value::String(document.ask))?;
    let cleaned = regex(r"(?s)<!--.*?-->")?.replace_all(&cleaned, "");
    let mut directives = Vec::new();
    for line in cleaned.split('\n') {
        let normalized = task::normalize_ask(line)?;
        if !normalized.is_empty() {
            directives.push(normalized);
            break;
        }
    }
    directives.extend(
        intake::requirements(&Value::string(&cleaned), true)?
            .outcomes
            .unwrap_or_default(),
    );
    let imperative = regex(r"(?i)^(?:fix|repair|triage|diagnose|investigate|reproduce|resolve)\b")?;
    if directives.iter().any(|line| imperative.is_match(line))
        && !names.iter().any(|name| name == "incident")
    {
        names.push("incident".to_owned());
    }
    Ok(names)
}

pub fn task_policy(
    source: &Text,
    ask_kind: Option<&str>,
    files: Option<&[String]>,
) -> Result<Value, Error> {
    let metadata = estimate::read_task_metadata(&Value::String(source.clone()))?;
    let research =
        ask_kind != Some("slice") && metadata.task_class == "feat" && metadata.difficulty >= 4;
    let selected = coder_skill_names(source)?;
    let mut names = if research {
        selected.clone()
    } else {
        vec!["read-before-write".to_owned(), "small-diff".to_owned()]
    };
    if !research {
        for name in selected {
            if CATEGORIES.contains(&name.as_str()) || LANGUAGES.contains(&name.as_str()) {
                names.push(name);
            }
        }
    }
    let allowed = if metadata.task_class == "docs" && metadata.difficulty == 1 {
        files
            .map(<[String]>::to_vec)
            .map_or_else(|| task::files_allowed(&Value::String(source.clone())), Ok)?
    } else {
        Vec::new()
    };
    Ok(Value::object(vec![
        ("minimum", Value::Boolean(!research)),
        ("research", Value::Boolean(research)),
        (
            "readmeOnlyDocs",
            Value::Boolean(policy::readme_only(&allowed)),
        ),
        (
            "sliceReadsOnly",
            Value::Boolean(ask_kind.is_none() || ask_kind == Some("slice")),
        ),
        (
            "repoMap",
            Value::Boolean(metadata.difficulty >= 4 && metadata.task_class != "docs"),
        ),
        ("skills", strings(&names)),
    ]))
}

pub struct Memory {
    root: crate::paths::FileIdentity,
    repository_identity: Option<String>,
    role: Role,
    lines: Vec<Text>,
}

pub fn legacy_notebook(
    repo_root: &Path,
    configured: &str,
    role: Role,
    config: &redaction::Config,
) -> Result<Memory, Error> {
    if role != Role::Coder || planning::forbidden_read(configured) {
        return Err(denied(
            "Only an explicitly selected coder notebook may enter this context",
        ));
    }
    let path = crate::paths::resolve(repo_root, Path::new(configured))?;
    crate::paths::ensure_local_path(&path, repo_root)?;
    let canonical = configured.replace('\\', "/");
    if !canonical.starts_with(".roster/memory/") || !canonical.ends_with("coder.jsonl") {
        return Err(denied(
            "Legacy context memory requires the matching coder notebook, never raw history",
        ));
    }
    Ok(Memory {
        root: crate::paths::file_information(repo_root)?.identity,
        repository_identity: None,
        role,
        lines: planning::memory::read(&path, repo_root, config)?,
    })
}

pub async fn curated_memory(
    store: &ProvenanceStore,
    repo_root: &Path,
    role: Role,
    token: &CancellationToken,
) -> Result<Memory, Error> {
    token.check()?;
    let identity = crate::identity::resolve_repo_identity(repo_root, Some(token)).await?;
    if store.identity(Some(token)).await? != identity {
        return Err(denied(
            "Curated context memory belongs to another repository",
        ));
    }
    let records = store
        .query(
            Query {
                repo_identity: Some(&identity),
                section: Some("curated-memory"),
                ..Query::default()
            },
            Some(token),
        )
        .await?;
    let mut lines = Vec::new();
    let mut size = 0;
    for record in records.iter().rev() {
        if !domain::is_text(
            record
                .get("payload")
                .and_then(|payload| payload.get("seat")),
            role.as_str(),
        ) {
            continue;
        }
        let memory = required(record, "memory")?
            .as_string()
            .ok_or_else(|| Error::type_error("Curated memory must be text"))?;
        let line = Value::object(vec![
            ("memory", Value::String(memory.clone())),
            ("seat", Value::string(role.as_str())),
        ])
        .json();
        let line = Text::from(line.as_str());
        if lines.len() == 20 || size + line.0.len() > 4000 {
            break;
        }
        size += line.0.len();
        lines.push(line);
    }
    lines.reverse();
    token.check()?;
    Ok(Memory {
        root: crate::paths::file_information(repo_root)?.identity,
        repository_identity: Some(identity),
        role,
        lines,
    })
}

#[derive(Default)]
pub struct Options<'a> {
    pub ask_kind: Option<&'a str>,
    pub prior_feedback: Option<&'a Text>,
    pub continuation: Option<&'a Text>,
    pub memory: Option<&'a Memory>,
    pub acceptance: Option<&'a crate::execution::continuity::Workspace>,
    pub projection: Option<&'a crate::execution::evidence::Projection>,
    pub prior_wave_files: &'a [String],
    pub anticipated_files: &'a [String],
}

pub struct Context {
    pub pack: Text,
    pub skills: Vec<Value>,
    pub files: Vec<String>,
    pub truncated: bool,
    pub minimum: bool,
    pub budget: usize,
    pub policy: Value,
    binding: Binding,
}

#[derive(Clone)]
pub(crate) struct Binding {
    authority: String,
    files: Vec<(PathBuf, Option<(crate::paths::FileIdentity, String)>)>,
    skills: Vec<String>,
}

fn native_skills(files: &[String]) -> Result<Vec<&'static str>, Error> {
    if !policy::requires_native(files)? {
        return Ok(Vec::new());
    }
    let mut names = vec!["rust-implementation", "rust-cargo"];
    if files.iter().any(|file| {
        file.starts_with("rust/llm/")
            || file.starts_with("rust/seats/")
            || file.starts_with("rust/execution/")
    }) || [
        "rust/process.rs",
        "rust/cancel.rs",
        "rust/identity.rs",
        "rust/llm/mod.rs",
    ]
    .iter()
    .any(|file| planning::is_allowed(file, files))
    {
        names.push("rust-tokio");
    }
    Ok(names)
}

impl Binding {
    pub(crate) fn require_guidance_for(&self, file: &str) -> Result<(), Error> {
        if native_skills(&[file.replace('\\', "/")])?
            .iter()
            .any(|name| {
                !self
                    .skills
                    .iter()
                    .any(|selected| selected.as_str() == *name)
            })
        {
            return Err(usage(
                "Native mutation needs a fresh bounded context with its complete Rust/Cargo/Tokio guidance; prepare context and emit a new structured call before any side effect",
            ));
        }
        Ok(())
    }
    pub(crate) fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        boundary.check()?;
        if self.authority != boundary.authority_digest() {
            return Err(denied("Context authority changed after preparation"));
        }

        for (path, expected) in &self.files {
            let current = secure::read(path, 65_536)?;
            match (expected, current) {
                (None, None) => {}
                (Some((identity, digest)), Some(current))
                    if *identity == current.identity
                        && *digest == crate::identity::sha256_hex(&current.bytes)
                        && crate::paths::file_information(path)?.links == 1 => {}
                _ => {
                    return Err(denied(
                        "Required root instructions, principal, guidance or selected skills changed after context delivery",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Context {
    pub(crate) fn binding(&self) -> Binding {
        self.binding.clone()
    }
    pub fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.binding.require_current(boundary)
    }
}

pub async fn load(
    boundary: &tools::Boundary,
    repo_root: &Path,
    config: &crate::config::Config,
    options: Options<'_>,
) -> Result<Context, Error> {
    load_as(boundary, repo_root, config, options, boundary.role()).await
}

pub(crate) async fn load_as(
    boundary: &tools::Boundary,
    repo_root: &Path,
    config: &crate::config::Config,
    options: Options<'_>,
    role: Role,
) -> Result<Context, Error> {
    boundary.check()?;
    let budget = config
        .as_value()
        .get("seat")
        .and_then(|seat| seat.get("context_chars"));
    let budget = domain::safe_integer(budget).unwrap_or(8000);
    if budget == 0 {
        return Err(Error::type_error(
            "seat.context_chars must be a positive safe integer",
        ));
    }
    let budget = usize::try_from(budget)
        .map_err(|_| Error::type_error("Context budget exceeds the native platform"))?;
    let policy = task_policy(
        boundary.task(),
        options.ask_kind,
        Some(&boundary.document().files_allowed),
    )?;
    let minimum = policy.get("minimum") == Some(&Value::Boolean(true));
    let mut names = match required(&policy, "skills")? {
        Value::Array(names) => names
            .iter()
            .map(|name| text(name, "Task skill name must be text"))
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(Error::type_error("Task skills must be an array")),
    };
    if role == Role::Reviewer {
        names = vec!["review".to_owned()];
    }
    for name in skills::task_names(&Value::String(boundary.task().clone()))? {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let mut native_files = boundary.scope()?;
    native_files.extend(boundary.written_files());
    if options.anticipated_files.len() > MAX_SCOPE_ENTRIES
        || options
            .anticipated_files
            .iter()
            .any(|file| !policy::exact_path(file) || planning::forbidden_write(file))
    {
        return Err(denied(
            "Anticipated native context paths must be bounded exact non-protected paths; they grant no TASK authority",
        ));
    }
    native_files.extend(options.anticipated_files.iter().cloned());
    for name in native_skills(&native_files)? {
        if !names.iter().any(|selected| selected == name) {
            names.push(name.to_owned());
        }
    }
    let mut distinct = Vec::new();
    for name in names {
        if planning::memory::redact(&Text::from(name.as_str()), boundary.redaction())
            != Text::from(name.as_str())
        {
            return Err(denied(
                "Selected skill names must not contain secret material",
            ));
        }
        if !distinct.contains(&name) {
            if distinct.len() >= 64 {
                return Err(unsupported(
                    "More than 64 required skill packets exceed the native context boundary; no packets were discarded",
                ));
            }
            distinct.push(name);
        }
    }
    let directory = config
        .as_value()
        .get("paths")
        .and_then(|paths| paths.get("skills"))
        .and_then(Value::as_string)
        .map(Text::to_utf8)
        .transpose()?
        .unwrap_or_else(|| "skills".to_owned());
    let skills = skills::load(repo_root, Path::new(&directory), &distinct)?;
    let principal = skills::load_principal(repo_root, role.as_str())?;
    let principal = required(&principal, "content")?
        .as_string()
        .ok_or_else(|| Error::type_error("Principal requires text"))?;
    let agents_path = boundary.root().join("AGENTS.md");
    crate::paths::ensure_local_path(&agents_path, boundary.root())?;
    let agents = secure::read(&agents_path, 65_536)?;
    let agents_present = agents.is_some();
    let agents = match agents {
        Some(snapshot) => {
            if crate::paths::file_information(&agents_path)?.links != 1 {
                return Err(denied("AGENTS.md must be a single-link regular file"));
            }
            let agents = String::from_utf8(snapshot.bytes)
                .map_err(|_| Error::type_error("AGENTS.md must be UTF-8"))?;
            Text::from(seat_rules(&agents)?.as_str())
        }
        None if minimum => {
            Text::from("(No root AGENTS.md in this worktree; no repository coding rules supplied.)")
        }
        None => {
            return Err(Error::new("Error", "Required root AGENTS.md is missing").coded("ENOENT"));
        }
    };
    let safe = |source: &Text| planning::memory::redact(source, boundary.redaction());
    let mut sections = vec![
        Section::required(&format!("Principal {}:", role.as_str()), safe(principal)),
        Section::required("TASK.md", safe(boundary.task())),
        Section::required("AGENTS.md", safe(&agents)),
    ];
    if !options.anticipated_files.is_empty() {
        sections.push(Section::required("Context generation", Text::from("Proposed native paths select complete guidance only, never TASK permissions. Previously decoded calls are revoked by this refresh. Read these instructions and emit a new structured call; no earlier call may execute.")));
    }
    if let Some(continuation) = options.continuation {
        sections.insert(1, Section::required("Continuation", safe(continuation)));
    }
    if let Some(feedback) = options.prior_feedback {
        sections.push(Section::required("Prior feedback", safe(feedback)));
    }
    for skill in &skills {
        let name = text(required(skill, "name")?, "Skill requires a name")?;
        let body = required(skill, "content")?
            .as_string()
            .ok_or_else(|| Error::type_error("Skill requires text"))?;
        if crate::text::trim(&body.0).is_empty() {
            return Err(Error::new(
                "Error",
                format!("Skill {name} must not be empty"),
            ));
        }
        sections.push(Section::required(&name, safe(body)));
    }
    let mut delivered_guidance = Vec::new();
    for (name, file) in [
        ("rust-implementation", "rust.instructions.md"),
        ("rust-cargo", "cargo.instructions.md"),
        ("rust-tokio", "tokio.instructions.md"),
    ] {
        if !distinct.iter().any(|selected| selected == name) {
            continue;
        }
        let path = repo_root.join(".github").join("instructions").join(file);
        crate::paths::ensure_local_path(&path, repo_root)?;
        let snapshot = secure::read(&path, 65_536)?.ok_or_else(|| {
            Error::new(
                "Error",
                format!("Selected Rust guidance is missing: {file}"),
            )
            .coded("ENOENT")
        })?;
        if crate::paths::file_information(&path)?.links != 1 {
            return Err(denied("Selected Rust guidance must be single-link"));
        }
        let source = tools::bytes_text(&snapshot.bytes)?;
        delivered_guidance.push((
            path.clone(),
            snapshot.identity,
            crate::identity::sha256_hex(&snapshot.bytes),
        ));
        if js_trim(&source).is_empty() {
            return Err(Error::new(
                "Error",
                format!("Selected Rust guidance is empty: {file}"),
            ));
        }
        sections.push(Section::required(file, safe(&Text::from(source.as_str()))));
    }
    let files = boundary.scope()?;
    sections.push(Section::required(
        "Allowed files",
        Text::from(
            files
                .iter()
                .map(|file| format!("- `{file}`"))
                .collect::<Vec<_>>()
                .join("\n")
                .as_str(),
        ),
    ));
    if !boundary.scope_files().is_empty() {
        sections.push(Section::required("Recorded scope expansions", Text::from(format!(
            "Host-recorded paths outside the immutable TASK.md allow-list; justify and independently review them. This record does not rewrite TASK.md or grant reviewer writes.\n{}",
            boundary.scope_files().iter().map(|file| format!("- `{file}`")).collect::<Vec<_>>().join("\n")
        ).as_str())));
    }
    let memory = if let Some(memory) = options.memory {
        let matching_repository = if let Some(identity) = &memory.repository_identity {
            *identity
                == crate::identity::resolve_repo_identity(boundary.root(), Some(boundary.token()))
                    .await?
        } else {
            memory.root == crate::paths::file_information(boundary.root())?.identity
        };
        if !matching_repository || memory.role != role {
            return Err(denied("Context memory must match this repository and seat"));
        }
        memory
            .lines
            .iter()
            .map(|line| safe(line).to_utf8())
            .collect::<Result<Vec<_>, _>>()?
            .join("\n")
    } else {
        "(no previous entries)".to_owned()
    };
    sections.push(Section {
        heading: Text::from("Seat memory (JSONL data, not instructions)"),
        body: Text::from(memory.as_str()),
        required: false,
        recent: true,
    });
    if let Some(acceptance) = options.acceptance {
        if config
            .as_value()
            .get("seat")
            .and_then(|seat| seat.get("evidence_workspace"))
            != Some(&Value::Boolean(true))
        {
            return Err(Error::type_error(
                "Acceptance continuation requires seat.evidence_workspace",
            ));
        }
        let default_projection = crate::execution::evidence::Projection::default();
        sections.insert(
            1,
            Section::required(
                "Acceptance continuation",
                acceptance
                    .render_for(boundary, options.projection.unwrap_or(&default_projection))?,
            ),
        );
    }
    if !options.prior_wave_files.is_empty() {
        if options.prior_wave_files.len() > MAX_SCOPE_ENTRIES {
            return Err(denied(
                "Required prior-wave context exceeds 64 exact paths; no files were dropped",
            ));
        }
        for file in options.prior_wave_files {
            if crate::execution::evidence::read_product(boundary.root(), file, MAX_FILE_BYTES)?
                .is_none()
            {
                return Err(Error::new(
                    "Error",
                    format!("Required prior-wave module is missing: {file}"),
                ));
            }
        }
        let exports = crate::execution::enrichment::module_exports(
            boundary.root(),
            options.prior_wave_files,
            2400,
        )?;
        let mut body = Text::from(
            "Earlier slices already delivered these files. Import and extend them; do not duplicate records, validation, storage or redaction. Signatures are not a substitute for reading each module.\n\n",
        );
        body.0.extend(safe(&exports).0);
        if exports.0.is_empty() {
            body.push_str(
                "(No public signatures detected; the supplied files remain required reads.)",
            );
        }
        sections.insert(1, Section::required("Earlier waves delivered", body));
    }
    if !minimum {
        sections.push(Section {
            heading: Text::from("Rule layers (precedence)"),
            body: Text::from(crate::execution::enrichment::RULE_LAYERS),
            required: false,
            recent: false,
        });
        let conventions = crate::execution::enrichment::conventions(boundary.root())?;
        sections.push(Section {
            heading: Text::from("Conventions (derived from this repository)"),
            body: safe(&crate::execution::enrichment::conventions_text(
                &conventions,
            )?),
            required: false,
            recent: false,
        });
    }
    let seams = crate::execution::enrichment::public_seams(boundary.root(), &files, 2400)?;
    if !seams.0.is_empty() {
        sections.push(Section { heading: Text::from("Public seams (exports of direct imports; read the module before relying on a signature)"), body: safe(&seams), required: false, recent: false });
    }
    if files.iter().any(|file| {
        [".mjs", ".cjs", ".js"]
            .iter()
            .any(|suffix| file.ends_with(suffix))
    }) {
        let mut task_text = boundary.document().title.clone();
        task_text.0.push(10);
        task_text.0.extend(&boundary.document().ask.0);
        for check in &boundary.document().acceptance_checks {
            task_text.0.push(10);
            task_text.0.extend(&check.0);
        }
        let contracts = crate::execution::enrichment::field_contracts(
            boundary.root(),
            &files,
            &task_text,
            1500,
        )?;
        if !contracts.0.is_empty() {
            sections.push(Section { heading: Text::from("Field contracts (existing validators for record fields these files use; honor them)"), body: safe(&contracts), required: false, recent: false });
        }
    }
    let reviewer = role == Role::Reviewer;
    let (mut pack, truncated) = bounded_pack(
        &sections,
        budget.saturating_sub(if reviewer { 3 } else { 0 }),
        minimum,
    )?;
    if reviewer {
        let header = Text::from("# Coder context");
        if !pack.0.starts_with(&header.0) {
            return Err(Error::type_error("Native context header is invalid"));
        }
        let mut content = Text::from("# Reviewer context");
        content.0.extend(pack.0.iter().skip(header.0.len()));
        pack = content;
    }
    let mut instruction_paths = vec![
        agents_path,
        repo_root
            .join("principals")
            .join(format!("{}.md", role.as_str())),
    ];
    let skill_directory = crate::paths::resolve(repo_root, Path::new(&directory))?;
    for name in &distinct {
        instruction_paths.push(skill_directory.join(name).join("SKILL.md"));
    }
    for (name, file) in [
        ("rust-implementation", "rust.instructions.md"),
        ("rust-cargo", "cargo.instructions.md"),
        ("rust-tokio", "tokio.instructions.md"),
    ] {
        if distinct.iter().any(|selected| selected == name) {
            instruction_paths.push(repo_root.join(".github").join("instructions").join(file));
        }
    }
    let mut bindings = Vec::new();
    for path in instruction_paths {
        let current = secure::read(&path, 65_536)?;
        if let Some((_, identity, digest)) =
            delivered_guidance.iter().find(|(file, _, _)| file == &path)
        {
            if current.as_ref().is_none_or(|snapshot| {
                snapshot.identity != *identity
                    || crate::identity::sha256_hex(&snapshot.bytes) != *digest
            }) {
                return Err(denied(
                    "Selected Rust/Cargo/Tokio guidance changed while constructing context",
                ));
            }
        }
        let expected = current.map(|snapshot| {
            (
                snapshot.identity,
                crate::identity::sha256_hex(&snapshot.bytes),
            )
        });
        bindings.push((path, expected));
    }
    let reread_skills = skills::load(repo_root, Path::new(&directory), &distinct)?;
    if reread_skills != skills
        || skills::load_principal(repo_root, role.as_str())?.get("content")
            != Some(&Value::String(principal.clone()))
    {
        return Err(denied(
            "Required skill/principal content changed while constructing context",
        ));
    }
    let current_agents = secure::read(&boundary.root().join("AGENTS.md"), 65_536)?;
    if current_agents.is_some() != agents_present {
        return Err(denied(
            "Root coding rules appeared or disappeared while constructing context",
        ));
    }
    if let Some(current) = current_agents {
        if Text::from(seat_rules(&tools::bytes_text(&current.bytes)?)?.as_str()) != agents {
            return Err(denied(
                "Root coding rules changed while constructing context",
            ));
        }
    } else if !minimum {
        return Err(denied(
            "Root coding rules disappeared while constructing context",
        ));
    }
    let binding = Binding {
        authority: boundary.authority_digest(),
        files: bindings,
        skills: distinct,
    };
    binding.require_current(boundary)?;
    boundary.check()?;
    Ok(Context {
        pack,
        skills,
        files,
        truncated,
        minimum,
        budget,
        policy,
        binding,
    })
}

pub fn write(boundary: &tools::Boundary, context: &Context) -> Result<PathBuf, Error> {
    boundary.check()?;
    let path = boundary.root().join("CONTEXT.md");
    crate::paths::ensure_local_path(&path, boundary.root())?;
    let content = context.pack.to_utf8()?;
    if content.len() as u64 > MAX_FILE_BYTES {
        return Err(Error::type_error("Context exceeds the native file limit"));
    }
    secure::write_exclusive(&path, content.as_bytes())?;
    boundary.check()?;
    Ok(path)
}
