use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::*;
use crate::cancel::CancellationToken;
use crate::paths;
use crate::recipe;
use crate::state::{redaction, secure};

#[derive(Debug, Clone)]
pub struct Plan {
    pub recipe: Text,
    pub task: Text,
}

fn render_template(template: &str, values: &[(&str, String)]) -> Result<String, Error> {
    let template = template.replace("\r\n", "\n");
    let mut rendered = String::new();
    let mut position = 0;
    for captures in regex(r"\{\{([A-Z_]+)\}\}")?.captures_iter(&template) {
        let matched = captures
            .get(0)
            .ok_or_else(|| Error::new("Error", "Missing template placeholder"))?;
        let field = captures
            .get(1)
            .ok_or_else(|| Error::new("Error", "Missing template field"))?
            .as_str();
        let replacement = values
            .iter()
            .find(|(name, _)| *name == field)
            .map(|(_, value)| value)
            .ok_or_else(|| {
                Error::new(
                    "Error",
                    format!("Unknown SDLC template field {}", matched.as_str()),
                )
            })?;
        rendered.push_str(
            template
                .get(position..matched.start())
                .ok_or_else(|| Error::new("Error", "Invalid template span"))?,
        );
        rendered.push_str(replacement);
        position = matched.end();
    }
    rendered.push_str(
        template
            .get(position..)
            .ok_or_else(|| Error::new("Error", "Invalid final template span"))?,
    );
    Ok(rendered)
}

impl Plan {
    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("recipe", Value::String(self.recipe.clone())),
            ("task", Value::String(self.task.clone())),
        ])
    }
}

pub fn runtime_recipe(reference: &str) -> Result<Text, Error> {
    let source = include_str!("../../templates/sdlc/RECIPE.yml")
        .replace("\r\n", "\n")
        .replacen("issue:N", reference, 1);
    let text = Text::from(source.as_str());
    recipe::parse(&text)?;
    Ok(text)
}

pub fn validate_builtin_recipe(source: &Text, reference: &str) -> Result<(), Error> {
    let parsed = recipe::parse(source)?;
    let parsed = crate::json::parse(&Text::from(parsed.json().as_str()))?;
    let seats = parsed.get("seats").and_then(domain::items);
    if !domain::is_text(parsed.get("ask"), reference)
        || seats.is_none_or(|seats| {
            seats.len() != 3
                || seats
                    .iter()
                    .zip(["planner", "coder", "reviewer"])
                    .any(|(seat, id)| {
                        !domain::is_text(seat.get("id"), id)
                            || !domain::is_text(seat.get("worker"), "builtin")
                    })
        })
    {
        return Err(Error::type_error(
            "Builtin planner must emit planner, coder, and reviewer seats for this issue in order",
        ));
    }
    Ok(())
}

#[derive(Default)]
pub struct BuildOptions<'a> {
    pub reference: Option<&'a str>,
    pub title: Option<&'a Value>,
    pub checks: Option<&'a Value>,
    pub files: Option<&'a Value>,
    pub metadata: Option<&'a Value>,
    pub scope: Option<&'a intake::Requirements>,
    pub grounding: Option<&'a grounding::Context>,
    pub design: Option<&'a Value>,
}

pub fn build(ask: &Value, options: BuildOptions<'_>) -> Result<Plan, Error> {
    let ask = intake::clean_ask(ask)?;
    let files = task::checked_list(options.files, "Files allowed", 32)?
        .iter()
        .map(task::allowed_file)
        .collect::<Result<Vec<_>, _>>()?;
    let docs_only = files.iter().all(|file| file.ends_with(".md"));
    let rust_only = !files.is_empty()
        && files
            .iter()
            .all(|file| file.ends_with(".rs") || file.ends_with(".toml"))
        && !ask.contains("node --test");
    let checks = task::checked_list(options.checks, "Acceptance checks", 8)?
        .iter()
        .enumerate()
        .map(|(index, entry)| task::acceptance_check(entry, index).map(|entry| encode(&entry)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|check| !((docs_only || rust_only) && check.contains("node --test")))
        .collect::<Vec<_>>();
    let requirements = match options.scope {
        Some(scope) => scope.clone(),
        None => intake::requirements(&value(&ask), false)?,
    };
    if requirements.output_directory.is_some() && checks.is_empty() {
        return Err(Error::type_error(
            "Acceptance checks must include a verifiable requested outcome",
        ));
    }
    requirements.check_files(&files)?;
    task::validate_directory_documents(&ask, &files, &checks)?;
    let metadata = options
        .metadata
        .cloned()
        .unwrap_or_else(|| Value::object(Vec::new()));
    let class = metadata
        .get("task_class")
        .and_then(Value::as_string)
        .map(encode);
    task::validate_directory_executable(&ask, &files, class.as_deref())?;
    let design = if options.grounding.is_some() && !docs_only && grounding::is_code_slice(&files)? {
        grounding::normalize_design(options.design)?
    } else {
        None
    };
    let title = options
        .title
        .filter(|title| domain::truthy(title))
        .map(|title| text(title, "Task title must be text"))
        .transpose()?;
    let errors = grounding::errors(
        &checks,
        design.as_ref(),
        &files,
        &format!("{}\n{ask}", title.as_deref().unwrap_or("")),
        options.grounding.map(|grounding| &grounding.index),
    )?;
    if !errors.is_empty() {
        return Err(text_error(
            "TypeError",
            &format!(
                "Plan cites names that do not exist: {}",
                errors
                    .iter()
                    .take(6)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        ));
    }
    let mut metadata = metadata;
    domain::remove(&mut metadata, "design")?;
    if metadata.get("task_class").is_none() {
        domain::put(
            &mut metadata,
            "task_class",
            Value::string(
                estimate::infer_class(title.as_deref().unwrap_or(""))?
                    .as_deref()
                    .unwrap_or("feat"),
            ),
        )?;
    }
    let estimate = estimate::estimate(&metadata, &[], "")?;
    let heading = title.unwrap_or_else(|| {
        ask.split('\n')
            .find(|line| !trim(line).is_empty())
            .unwrap_or("Task")
            .to_owned()
    });
    let heading = encode(&task::one_line(&value(&heading), "Task title")?);
    let replacements = [
        ("TITLE", heading),
        ("DIFFICULTY", estimate.difficulty.to_string()),
        ("ESTIMATE_MIN", estimate.minutes.to_string()),
        ("TASK_CLASS", estimate.task_class),
        ("MODEL", estimate.model),
        (
            "CHECKS",
            checks
                .iter()
                .map(|check| format!("- {check}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "FILES",
            files
                .iter()
                .map(|file| format!("- `{file}`"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        ("ASK", ask.clone()),
    ];
    let mut rendered =
        render_template(include_str!("../../templates/sdlc/TASK.md"), &replacements)?;
    if requirements.output_directory.is_some() {
        let new_files = files
            .iter()
            .filter(|file| {
                options
                    .grounding
                    .is_none_or(|grounding| !grounding.index.files.contains(file))
            })
            .map(|file| format!("- `{file}`"))
            .collect::<Vec<_>>();
        if !new_files.is_empty() {
            rendered = rendered.replacen(
                "## Ask\n",
                &format!("## New files\n\n{}\n\n## Ask\n", new_files.join("\n")),
                1,
            );
        }
    }
    let task = if let Some(design) = design {
        grounding::with_design(&value(&rendered), &design)?
    } else {
        decode(&rendered)
    };
    if task.utf8_len() > MAX_ARTIFACT_BYTES {
        return Err(Error::type_error(
            "Planner artifact content must be at most 64 KiB",
        ));
    }
    Ok(Plan {
        recipe: runtime_recipe(options.reference.unwrap_or("undefined"))?,
        task,
    })
}

pub fn stub(
    ask: &Value,
    reference: &str,
    title: Option<&Value>,
    metadata: Option<&Value>,
) -> Result<Plan, Error> {
    let clean = intake::clean_ask(ask)?;
    let checks = intake::list_in_ask(&clean, "Acceptance checks")?.unwrap_or(
        if task::directory_document(&clean)? {
            vec!["The Markdown documents describe the requested design and instructions".to_owned()]
        } else {
            vec![
                "node --test exits 0".to_owned(),
                "The requested behavior in the Ask is implemented".to_owned(),
            ]
        },
    );
    let title_text = title
        .map(|title| text(title, "Task title must be text"))
        .transpose()?;
    let scope = intake::requirements(
        &value(
            &title_text
                .as_ref()
                .map_or_else(|| clean.clone(), |title| format!("{title}\n{clean}")),
        ),
        false,
    )?;
    let files = if scope.files.is_empty() {
        vec![format!(
            "{}/README.md",
            scope.output_directory.as_deref().unwrap_or("undefined")
        )]
    } else {
        scope.files.clone()
    };
    let heading = title
        .cloned()
        .unwrap_or_else(|| value(&slice(trim(&clean).split('\n').next().unwrap_or(""), 200)));
    build(
        ask,
        BuildOptions {
            reference: Some(reference),
            title: Some(&heading),
            checks: Some(&strings(&checks)),
            files: Some(&strings(&files)),
            metadata,
            scope: Some(&scope),
            ..BuildOptions::default()
        },
    )
}

pub fn from_task(
    input: &Value,
    ask: &Text,
    title: Option<&Text>,
    body: Option<&Text>,
) -> Result<Value, Error> {
    let authority = task::Authority {
        expected_ask: Some(ask.clone()),
        issue_title: title.cloned(),
        issue_body: body.cloned(),
    };
    let document = task::parse(input, &authority)?;
    for (index, check) in document.acceptance_checks.iter().enumerate() {
        task::acceptance_check(&Value::String(check.clone()), index)?;
    }
    let combined = title.map_or_else(
        || encode(ask),
        |title| format!("{}\n{}", encode(title), encode(ask)),
    );
    intake::requirements(&value(&combined), true)?.check_files(&document.files_allowed)?;
    let estimate = estimate::read_task_metadata(input)?;
    let mut result = document.as_value();
    domain::remove(&mut result, "ask")?;
    for field in ["difficulty", "estimate_min", "task_class", "model"] {
        domain::put(
            &mut result,
            field,
            required(&estimate.as_value(), field)?.clone(),
        )?;
    }
    domain::put(
        &mut result,
        "design",
        grounding::parse_design(input)?.unwrap_or(Value::Null),
    )?;
    Ok(result)
}

pub fn leading_json(source: &Text) -> Result<Value, Error> {
    if let Ok(value) = crate::json::parse(source) {
        return Ok(value);
    }
    let source = encode(source);
    if !source.starts_with('{') {
        return crate::json::parse(&decode(&source));
    }
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (index, c) in source.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth = depth
                .checked_sub(1)
                .ok_or_else(|| Error::new("SyntaxError", "Invalid JSON object"))?;
            if depth == 0 {
                return crate::json::parse(&decode(&source[..index + 1]));
            }
        }
    }
    crate::json::parse(&decode(&source))
}

pub fn normalize_description(input: &Value) -> Result<Value, Error> {
    let supported = [
        "title",
        "task",
        "acceptance_checks",
        "files_allowed",
        "difficulty",
        "estimate_min",
        "task_class",
        "model",
        "steps",
        "notes",
        "design",
    ];
    fields(
        input,
        &["acceptance_checks", "files_allowed"],
        &supported,
        "LLM planner returned an unsupported task plan",
    )?;
    if input.get("title").is_some()
        && input.get("task").is_some()
        && input.get("title") != input.get("task")
    {
        return Err(Error::type_error(
            "LLM planner returned an unsupported task plan",
        ));
    }
    let title = input
        .get("title")
        .or_else(|| input.get("task"))
        .ok_or_else(|| Error::type_error("LLM planner returned an unsupported task plan"))?;
    let mut result = Value::object(vec![("title", title.clone())]);
    for field in [
        "acceptance_checks",
        "files_allowed",
        "difficulty",
        "estimate_min",
        "task_class",
        "model",
        "design",
    ] {
        if let Some(entry) = input.get(field) {
            domain::put(&mut result, field, entry.clone())?;
        }
    }
    Ok(result)
}

pub fn render_ask(ask: &Text) -> Text {
    decode(
        &include_str!("../../templates/sdlc/ASK.md")
            .replace("\r\n", "\n")
            .replace("{{ASK}}", trim(&encode(ask))),
    )
}

pub fn render_assignment(issue: &Value) -> Result<Text, Error> {
    let mut replacements = Vec::new();
    for (field, placeholder) in [
        ("url", "ISSUE_URL"),
        ("number", "ISSUE_NUMBER"),
        ("title", "TITLE"),
        ("body", "ASK"),
    ] {
        let rendered = issue
            .get(field)
            .map(domain::string)
            .transpose()?
            .map_or_else(|| "undefined".to_owned(), |text| encode(&text));
        replacements.push((placeholder, rendered));
    }
    let mut source = render_template(
        include_str!("../../templates/sdlc/ASSIGNMENT.md"),
        &replacements,
    )?;
    let mut waves = Vec::new();
    if issue.get("labels").is_some() && issue.get("labels").and_then(domain::items).is_none() {
        return Err(Error::new("Error", "Issue wave labels must be an array"));
    }
    if let Some(labels) = issue.get("labels").and_then(domain::items) {
        for label in labels {
            let label = if let Some(text) = label.as_string() {
                Some(encode(text))
            } else {
                label.get("name").and_then(Value::as_string).map(encode)
            };
            if let Some(label) = label {
                if let Some(wave) = label
                    .strip_prefix("wave:")
                    .filter(|wave| wave.len() == 1)
                    .and_then(|wave| wave.parse::<u64>().ok())
                    .filter(|wave| (1..=8).contains(wave))
                {
                    waves.push(wave);
                }
            }
        }
    }
    if let Some(body) = issue.get("body").and_then(Value::as_string) {
        if let Some(wave) = regex(r"<!-- Roster-Wave: ([1-8]) -->")?
            .captures(&encode(body))
            .and_then(|captures| {
                captures
                    .get(1)
                    .and_then(|entry| entry.as_str().parse::<u64>().ok())
            })
        {
            waves.push(wave);
        }
    }
    waves.sort_unstable();
    waves.dedup();
    if waves.len() > 1 {
        return Err(Error::new("Error", "Issue has conflicting wave metadata"));
    }
    if let Some(wave) = waves.first() {
        source = source.replacen("## Ask\n", &format!("- Wave: {wave}\n\n## Ask\n"), 1);
    }
    Ok(decode(&source))
}

pub fn read(root: &Path, name: &str) -> Result<Option<Text>, Error> {
    if !PLANNER_ARTIFACTS
        .iter()
        .chain(PLAN_ARTIFACTS)
        .chain(["REVIEW.md"].iter())
        .any(|entry| *entry == name)
    {
        return Err(Error::type_error("Unavailable planning artifact"));
    }
    let path = root.join(name);
    paths::ensure_local_path(&path, root)?;
    let Some(snapshot) = secure::read(&path, MAX_ARTIFACT_BYTES as u64)? else {
        return Ok(None);
    };
    if paths::file_information(&path)?.links != 1 {
        return Err(Error::new(
            "Error",
            "Existing planning artifacts must be regular, single-link files of at most 64 KiB",
        ));
    }
    Ok(Some(Text::from(
        String::from_utf8(snapshot.bytes)
            .map_err(|_| Error::type_error("Planning artifacts must be UTF-8"))?
            .as_str(),
    )))
}

pub struct Writer {
    root: PathBuf,
    allowed: &'static [&'static str],
    owned: Vec<(String, secure::Snapshot)>,
    redaction: redaction::Config,
    signal: Option<CancellationToken>,
}

impl Writer {
    pub fn new(
        root: &Path,
        plan_only: bool,
        redaction: redaction::Config,
        signal: Option<CancellationToken>,
    ) -> Result<Self, Error> {
        let root = paths::absolute(root)?;
        secure::check_components(&root)?;
        redaction.validate()?;
        if !fs::symlink_metadata(&root)
            .map_err(|error| secure::io_error("Inspect planning worktree", &root, error))?
            .is_dir()
        {
            return Err(Error::type_error("Planning worktree must be a directory"));
        }
        Ok(Self {
            root,
            allowed: if plan_only {
                PLAN_ARTIFACTS
            } else {
                PLANNER_ARTIFACTS
            },
            owned: Vec::new(),
            redaction,
            signal,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn set_redaction(&mut self, redaction: redaction::Config) -> Result<(), Error> {
        redaction.validate()?;
        self.redaction = redaction;
        Ok(())
    }

    pub fn check(&self) -> Result<(), Error> {
        if let Some(signal) = &self.signal {
            signal.check()?;
        }
        Ok(())
    }

    pub fn write(&mut self, name: &str, content: &Text) -> Result<Value, Error> {
        self.check()?;
        if !self.allowed.contains(&name) {
            return Err(Error::new(
                "ToolAccessError",
                format!(
                    "Planner write_file allows only root {}",
                    self.allowed.join(", ")
                ),
            )
            .coded("ROSTER_TOOL_DENIED"));
        }
        if content.utf8_len() > MAX_ARTIFACT_BYTES {
            return Err(Error::type_error(
                "Planner artifact content must be at most 64 KiB",
            ));
        }
        if memory::redact(content, &self.redaction) != *content {
            return Err(Error::new(
                "Error",
                "Planner artifacts must not contain credentials or private keys",
            ));
        }
        let bytes = content.to_utf8()?.into_bytes();
        let path = self.root.join(name);
        paths::ensure_local_path(&path, &self.root)?;
        let previous = self.owned.iter().position(|(file, _)| file == name);
        let existing = secure::read(&path, MAX_ARTIFACT_BYTES as u64)?;
        if existing.is_some() != previous.is_some() {
            return Err(Error::new(
                "Error",
                "Planner cannot overwrite pre-existing or externally replaced artifacts",
            ));
        }
        let written_identity = if let Some(index) = previous {
            let (_, snapshot) = self
                .owned
                .get(index)
                .ok_or_else(|| Error::new("Error", "Missing owned planning artifact"))?;
            if !secure::unchanged(&path, snapshot, MAX_ARTIFACT_BYTES as u64)? {
                return Err(Error::new(
                    "Error",
                    "Planner artifact changed outside the planner writer",
                ));
            }
            let mut handle = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|error| secure::io_error("Open owned planning artifact", &path, error))?;
            let information = paths::handle_information(&handle)?;
            if information.identity != snapshot.identity || information.links != 1 {
                return Err(Error::new(
                    "Error",
                    "Planner artifact changed outside the planner writer",
                ));
            }
            let mut actual = Vec::new();
            (&mut handle)
                .take(MAX_ARTIFACT_BYTES as u64 + 1)
                .read_to_end(&mut actual)
                .map_err(|error| {
                    secure::io_error("Verify owned planning artifact", &path, error)
                })?;
            if actual != snapshot.bytes
                || paths::file_information(&path)?.identity != information.identity
            {
                return Err(Error::new(
                    "Error",
                    "Planner artifact changed outside the planner writer",
                ));
            }
            self.check()?;
            handle
                .set_len(0)
                .and_then(|()| handle.seek(SeekFrom::Start(0)))
                .and_then(|_| handle.write_all(&bytes))
                .and_then(|()| handle.sync_all())
                .map_err(|error| secure::io_error("Write owned planning artifact", &path, error))?;
            information.identity
        } else {
            self.check()?;
            secure::write_exclusive(&path, &bytes)?
        };
        let snapshot = secure::read(&path, MAX_ARTIFACT_BYTES as u64)?
            .ok_or_else(|| Error::new("Error", "Planning artifact disappeared after writing"))?;
        let current = paths::file_information(&path)?;
        if snapshot.bytes != bytes
            || snapshot.identity != written_identity
            || current.identity != written_identity
            || current.links != 1
        {
            return Err(Error::new(
                "Error",
                "Planner artifact changed outside the planner writer",
            ));
        }
        if let Some(index) = previous {
            if let Some(entry) = self.owned.get_mut(index) {
                entry.1 = snapshot;
            }
        } else {
            self.owned.push((name.to_owned(), snapshot));
        }
        self.check()?;
        Ok(Value::object(vec![
            ("path", Value::string(name)),
            ("bytes", Value::Number(bytes.len() as f64)),
        ]))
    }

    pub fn adopt_validated(&mut self, name: &str, content: &Text) -> Result<(), Error> {
        self.check()?;
        if !self.allowed.contains(&name) || self.owned.iter().any(|(file, _)| file == name) {
            return Err(Error::type_error(
                "Only validated planning artifacts can be adopted once",
            ));
        }
        let path = self.root.join(name);
        paths::ensure_local_path(&path, &self.root)?;
        let snapshot = secure::read(&path, MAX_ARTIFACT_BYTES as u64)?
            .ok_or_else(|| Error::new("Error", "Validated planning artifact disappeared"))?;
        let current = paths::file_information(&path)?;
        if snapshot.bytes != content.to_utf8()?.as_bytes()
            || current.identity != snapshot.identity
            || current.links != 1
        {
            return Err(Error::new(
                "Error",
                "Existing RECIPE/TASK changed after validation; refusing the cached handoff",
            ));
        }
        self.owned.push((name.to_owned(), snapshot));
        Ok(())
    }
}
