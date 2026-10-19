use super::*;

pub const CLARIFICATION_HINT: &str = "Clarify one concrete outcome and name the allowed files, grant a relative output directory, or state a feature or initiative to plan. No seat or implementation ran.";

fn url_spans(source: &str) -> Result<Vec<(usize, usize)>, Error> {
    let pattern = regex(
        r"(?i)(?:^|[^A-Za-z0-9_./-])((?:[a-z][a-z0-9+.-]*://|//|www\.|(?:file|data|mailto|urn):)[^\s<>]+)",
    )?;
    Ok(pattern
        .captures_iter(source)
        .filter_map(|captures| captures.get(1))
        .map(|entry| (entry.start(), entry.end()))
        .collect())
}

pub fn without_reference_urls(source: &str) -> Result<String, Error> {
    let mut definitions = Vec::new();
    for captures in regex(r"(?m)^ {0,3}\[([^\]\n]+)\]:\s*(\S+)")?.captures_iter(source) {
        if !url_spans(captures.get(2).map_or("", |entry| entry.as_str()))?.is_empty() {
            definitions
                .push(trim(captures.get(1).map_or("", |entry| entry.as_str())).to_lowercase());
        }
    }
    let mut links = Vec::new();
    for found in regex(r"!?\[[^\]\n]*\]\(")?.find_iter(source) {
        let start = found.end();
        let mut depth = 1_u32;
        let mut end = start;
        let mut escaped = false;
        for (offset, character) in source[start..].char_indices() {
            if character == '\n' && !escaped {
                break;
            }
            end = start + offset + character.len_utf8();
            if escaped {
                escaped = false;
                continue;
            }
            if character == '\\' {
                escaped = true;
            } else if character == '(' {
                depth += 1;
            } else if character == ')' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }
        if !url_spans(&source[start..end])?.is_empty() {
            links.push((found.start(), end));
        }
    }
    links.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in links {
        if let Some(previous) = merged.last_mut().filter(|previous| start <= previous.1) {
            previous.1 = previous.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    let mut result = source.to_owned();
    for (start, end) in merged.into_iter().rev() {
        if result.get(start..end).is_none() {
            return Err(Error::type_error(
                "Reference spans changed during bounded parsing",
            ));
        }
        result.replace_range(start..end, " ");
    }
    result = regex(r"!?\[([^\]\n]+)\](?:\[([^\]\n]*)\])?")?
        .replace_all(&result, |captures: &regex_lite::Captures<'_>| {
            let label = captures.get(1).map_or("", |entry| entry.as_str());
            let id = captures.get(2).map_or(label, |entry| {
                if entry.as_str().is_empty() {
                    label
                } else {
                    entry.as_str()
                }
            });
            if definitions.contains(&trim(id).to_lowercase()) {
                " ".to_owned()
            } else {
                captures
                    .get(0)
                    .map_or("", |entry| entry.as_str())
                    .to_owned()
            }
        })
        .into_owned();
    let spans = url_spans(&result)?;
    for (start, end) in spans.into_iter().rev() {
        let punctuation = result[start..end]
            .chars()
            .rev()
            .take_while(|c| matches!(c, '.' | '!' | '?'))
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        result.replace_range(start..end, &format!(" {punctuation}"));
    }
    Ok(result)
}

pub fn clean_ask(input: &Value) -> Result<String, Error> {
    let message = "Ask must be nonempty UTF-8 text of at most 16 KiB";
    let source = text(input, message)?;
    if trim(&source).is_empty() || decode(&source).utf8_len() > MAX_ASK_BYTES {
        return Err(Error::type_error(message));
    }
    let source = source.replace("\r\n", "\n");
    if source
        .chars()
        .any(|c| matches!(c, '\0'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}'))
    {
        return Err(Error::type_error(message));
    }
    Ok(if task::output_directory(&source)?.is_some() {
        source
    } else {
        trim(&source).to_owned()
    })
}

pub fn list_in_ask(ask: &str, heading: &str) -> Result<Option<Vec<String>>, Error> {
    let heading_pattern = regex(&format!(r"(?i)^#{{1,3}} {heading}\s*$"))?;
    let next_heading = regex(r"^#{1,3} ")?;
    let lines = ask.split('\n').collect::<Vec<_>>();
    let Some(start) = lines.iter().enumerate().find_map(|(index, line)| {
        (index + 1 < lines.len() && heading_pattern.is_match(line)).then_some(index)
    }) else {
        return Ok(None);
    };
    let mut items = Vec::new();
    let bullet = regex(r"^\s*-\s+(.+?)\s*$")?;
    for line in lines.iter().skip(start + 1) {
        if next_heading.is_match(line) {
            break;
        }
        if trim(line).is_empty() {
            continue;
        }
        let item = bullet
            .captures(line)
            .and_then(|captures| captures.get(1).map(|entry| entry.as_str()))
            .ok_or_else(|| {
                Error::type_error(format!("{heading} must contain only bullet points"))
            })?;
        let item = item
            .strip_prefix('`')
            .and_then(|item| item.strip_suffix('`'))
            .unwrap_or(item);
        items.push(item.to_owned());
    }
    if items.is_empty() {
        return Err(Error::type_error(format!(
            "{heading} must contain at least one bullet point"
        )));
    }
    Ok(Some(items))
}

#[derive(Debug, Clone)]
pub struct Requirements {
    pub files: Vec<String>,
    pub outcomes: Option<Vec<String>>,
    pub explicit: bool,
    pub output_directory: Option<String>,
    pub requires_run: bool,
}

impl Requirements {
    pub fn as_value(&self) -> Value {
        let mut fields = vec![
            ("files", strings(&self.files)),
            (
                "outcomes",
                self.outcomes
                    .as_ref()
                    .map_or(Value::Null, |items| strings(items)),
            ),
            ("explicit", Value::Boolean(self.explicit)),
        ];
        if let Some(directory) = &self.output_directory {
            fields.push(("outputDirectory", value(directory)));
        }
        fields.push(("requiresRun", Value::Boolean(self.requires_run)));
        Value::object(fields)
    }

    pub fn check_files(&self, files: &[String]) -> Result<(), Error> {
        if let Some(directory) = &self.output_directory {
            task::validate_directory_files(files, directory)?;
        }
        if !self.files.is_empty() && files.iter().any(|file| !is_allowed(file, &self.files)) {
            return Err(Error::type_error(
                "Planner cannot invent extra files beyond the human Ask allowed paths",
            ));
        }
        Ok(())
    }
}

pub fn requirements(input: &Value, allow_missing: bool) -> Result<Requirements, Error> {
    let ask = clean_ask(input)?;
    let explicit = list_in_ask(
        &ask,
        "(?:Files allowed|Allowed files|files_allowed|allowed_files)",
    )?;
    let output_directory = task::output_directory(&ask)?;
    let files = if let Some(explicit) = &explicit {
        let input = strings(explicit);
        task::checked_list(Some(&input), "Files allowed", 32)?
            .iter()
            .map(task::allowed_file)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let source = without_reference_urls(&ask)?;
        let mut files = Vec::new();
        for entry in regex(r"(?:[A-Za-z0-9_.-]+/)*[A-Za-z0-9_.-]+\.(?:mjs|js|ts|tsx|json|yml|yaml|md|css|html|txt)")?.find_iter(&source) {
            let file = entry.as_str();
            if task::allowed_file(&value(file)).is_err()
                || output_directory.as_deref() == Some(file)
                || matches(&format!(r"node --test(?:\s+\S+)*\s+{}\b", regex_lite::escape(file)), &ask)?
                || output_directory.as_ref().is_some_and(|directory| !file.starts_with(&format!("{directory}/")))
            {
                continue;
            }
            files.push(file.to_owned());
        }
        distinct(files)
    };
    if files.is_empty() && output_directory.is_none() && !allow_missing {
        return Err(Error::type_error(
            "Ask must name or declare allowed files before TASK can validate; no file scope will be invented",
        ));
    }
    let outcomes = list_in_ask(&ask, "Outcomes")?;
    let requires_run = explicit.is_none() || outcomes.as_ref().map_or(1, Vec::len) != 1;
    Ok(Requirements {
        files,
        outcomes,
        explicit: explicit.is_some(),
        output_directory,
        requires_run,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Clarify,
    Slice,
    Feature,
    Initiative,
}

impl Kind {
    pub fn parse(input: &str) -> Result<Self, Error> {
        match input {
            "clarify" => Ok(Self::Clarify),
            "slice" => Ok(Self::Slice),
            "feature" => Ok(Self::Feature),
            "initiative" => Ok(Self::Initiative),
            _ => Err(Error::type_error("Unknown Ask kind")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clarify => "clarify",
            Self::Slice => "slice",
            Self::Feature => "feature",
            Self::Initiative => "initiative",
        }
    }
}

pub fn classify(
    input: &Value,
    title: Option<&Text>,
    files_allowed: &[String],
) -> Result<Value, Error> {
    let ask = clean_ask(input)?;
    let combined = title.map_or_else(|| ask.clone(), |title| format!("{}\n{ask}", encode(title)));
    let requirements = requirements(&value(&combined), true)?;
    let source = regex(r"(?s)<!--.*?-->")?.replace_all(&ask, "");
    let mut lead = String::new();
    for line in source.split('\n') {
        let normalized = task::normalize_ask(line)?;
        if !normalized.is_empty() {
            lead = normalized;
            break;
        }
    }
    let mut directives = title
        .map(encode)
        .into_iter()
        .chain(Some(lead))
        .chain(requirements.outcomes.clone().unwrap_or_default())
        .filter(|line| !line.is_empty())
        .map(|line| task::normalize_ask(&line))
        .collect::<Result<Vec<_>, _>>()?;
    let prefix = regex(r"(?i)^(?:feat|fix|docs|test)(?:\([^)]*\))?:\s*")?;
    for line in &mut directives {
        *line = prefix.replace(line, "").to_lowercase();
    }
    let summary = directives.join(" ");
    let incident = matches(
        r"\b(?:outage|(?:prod|production)(?:\s+is)?\s+down|regression|hotfix)\b",
        &summary,
    )?;
    let questions = regex(r"^(?:who|what|when|where|why|how|which|can|could|is|are|does|do)\b")?;
    let question = directives.iter().any(|line| questions.is_match(line))
        && !matches(
            r"\b(?:add|build|create|develop|deliver|launch|implement|fix|update|change|remove|delete|replace|rewrite|refactor|rename|edit)\b",
            &summary,
        )?;
    let result = |kind: Kind, reason: &str| {
        Value::object(vec![
            ("kind", Value::string(kind.as_str())),
            (
                "specKind",
                if incident {
                    Value::string("incident")
                } else if question {
                    Value::string("question")
                } else {
                    match kind {
                        Kind::Slice => Value::string("slice"),
                        Kind::Feature => Value::string("story"),
                        Kind::Initiative => Value::string("epic"),
                        Kind::Clarify => Value::Null,
                    }
                },
            ),
            ("reason", Value::string(reason)),
        ])
    };
    let explicit_epic = matches(
        r"(?im)^(?:#{1,6}\s*)?(?:epic|initiative)(?:\s+outcome)?\s*:?\s*$",
        &ask,
    )? || matches(
        r"(?i)^(?:epic|initiative)\s*:",
        &title.map(encode).unwrap_or_default(),
    )?;
    let initiative = regex(
        r"^(?:build|create|develop|deliver|launch|implement)\s+(?:(?:a|an|the|new|entire|complete|full|standalone|end-to-end)\s+)*(?:orchestrator|platform|product|ecosystem|suite|operating system)\b",
    )?;
    let multi = regex(
        r"^(?:initiative\b|(?:plan|deliver|launch|build|create)\b.*\b(?:multi-wave|multi-team|initiative)\b)",
    )?;
    if explicit_epic
        || requirements.outcomes.as_ref().map_or(0, Vec::len) > 5
        || directives
            .iter()
            .any(|line| initiative.is_match(line) || multi.is_match(line))
    {
        return Ok(result(
            Kind::Initiative,
            if explicit_epic {
                "Explicit epic or initiative outcome needs an issue plan"
            } else {
                "Whole-system or multi-wave outcome needs an issue plan"
            },
        ));
    }
    let one_liner = matches(
        r"\b(?:readme(?:\.md)?\b.*\bone[- ]liner?\b|one[- ]liner?\b.*\breadme(?:\.md)?)\b",
        &summary,
    )?;
    let acceptance = matches(
        r"(?im)^(?:#{1,6}\s*)?acceptance(?:\s+(?:checks|criteria))?\s*:?\s*$",
        &ask,
    )?;
    if requirements.explicit
        && !requirements.files.is_empty()
        && acceptance
        && requirements.outcomes.as_ref().map_or(1, Vec::len) <= 1
    {
        return Ok(result(
            Kind::Slice,
            "Human-declared allow-list, one outcome, and acceptance checks make an executable slice",
        ));
    }
    let feature = regex(
        r"^(?:feature\b|(?:add|build|create|implement|deliver)\b.*\b(?:feature|end-to-end|multi-component)\b)",
    )?;
    if requirements.outcomes.as_ref().map_or(1, Vec::len) > 1
        || !one_liner && directives.iter().any(|line| feature.is_match(line))
    {
        return Ok(result(
            Kind::Feature,
            "Feature or multiple outcomes need child issue drafts",
        ));
    }
    if !requirements.files.is_empty() || !files_allowed.is_empty() || one_liner {
        return Ok(result(
            Kind::Slice,
            "One bounded, named-file outcome can use the sequential seats",
        ));
    }
    if requirements.output_directory.is_some() {
        return Ok(result(
            Kind::Slice,
            "One explicit relative output directory bounds planner-chosen files for the sequential seats",
        ));
    }
    Ok(result(
        Kind::Clarify,
        "The Ask lacks executable file scope or a clear planning outcome",
    ))
}

pub fn ambiguous(input: &Value) -> Result<Value, Error> {
    let ask = clean_ask(input)?;
    let without_comments = regex(r"(?s)<!--.*?-->")?.replace_all(&ask, "");
    let mut lead = None;
    for line in without_comments.split('\n') {
        let normalized = task::normalize_ask(line)?;
        if !normalized.is_empty() {
            lead = Some(normalized);
            break;
        }
    }
    let lead = lead.unwrap_or(ask);
    let first = regex(r"^(?:please|can you|could you|let'?s)\s+")?
        .replace(&lead.to_lowercase(), "")
        .into_owned();
    let verbs = [
        (
            "improve|enhance|polish|optimi[sz]e|clean\\s*up|tidy",
            "improve",
        ),
        ("fix|repair|resolve|debug", "fix"),
        ("add|create|build|implement|make", "add"),
        ("update|change|modify|adjust|tweak", "change"),
        ("remove|delete|drop", "remove"),
        ("explain|document|describe", "document"),
        ("why|what|how|which|where|who|when", "explain"),
    ];
    let mut verb = "";
    for (pattern, name) in verbs {
        if matches(&format!(r"^(?:{pattern})\b"), &first)? {
            verb = name;
            break;
        }
    }
    let clean_lead = regex(r"[.!?]+$")?.replace(&lead, "").into_owned();
    let articles =
        regex(r"(?i)^(?:the|a|an|this|that|it|things?|stuff|everything|overall|some|my|our)$")?;
    let words = regex(r"\s+")?
        .split(&clean_lead)
        .skip(1)
        .filter(|word| !articles.is_match(word))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let topic = slice(&words.join(" "), 80);
    let target = if topic.is_empty() {
        "the repository".to_owned()
    } else {
        format!("\"{topic}\"")
    };
    let interpretation = match verb {
        "improve" => format!("Make one small, reviewable improvement to {target}."),
        "fix" => format!("Find and fix one defect in {target} and prove it with a test."),
        "add" => format!(
            "Add one new capability, {}, as a single slice.",
            if topic.is_empty() {
                "as described".to_owned()
            } else {
                format!("\"{topic}\"")
            }
        ),
        "change" => format!("Change one existing behavior of {target} without widening scope."),
        "remove" => format!("Remove one obsolete piece of {target} and its references."),
        "document" => format!("Write or correct documentation for {target}."),
        "explain" => {
            format!("Answer a question about {target} from repository files; no code change.")
        }
        _ => format!(
            "Treat \"{}\" as one small slice in {target}.",
            slice(&lead, 120)
        ),
    };
    let assumptions = vec![
        "One bounded outcome, not a rewrite or multi-wave initiative.".to_owned(),
        "Only files you name are in scope; Roster will not guess an allow-list.".to_owned(),
        "Existing behavior and tests stay unchanged outside that outcome.".to_owned(),
    ];
    let questions = vec![
        "Which file(s) or directory should change? (e.g. `README.md`, or `docs/` as an output directory)".to_owned(),
        match verb { "explain" => "Which file(s) should the answer be grounded in?", "fix" => "What is the observed failure, and what should happen instead?", _ => "What single observable result means this is done?" }.to_owned(),
        "Is this one slice, or a larger feature that should be split into child issues first?".to_owned(),
    ];
    let template = format!(
        "{clean_lead}.\n\n## Allowed Files\n- `<path>`\n\n## Acceptance checks\n- <one observable result>"
    );
    Ok(Value::object(vec![
        ("interpretation", value(&interpretation)),
        ("assumptions", strings(&assumptions)),
        ("questions", strings(&questions)),
        ("template", value(&template)),
    ]))
}

pub fn clarification(input: &Value) -> Result<Value, Error> {
    let reading = ambiguous(input)?;
    let interpretation = text(
        required(&reading, "interpretation")?,
        "Invalid interpretation",
    )?;
    let assumptions = array(required(&reading, "assumptions")?, "Invalid assumptions")?;
    let questions = array(required(&reading, "questions")?, "Invalid questions")?;
    let template = text(required(&reading, "template")?, "Invalid template")?;
    let mut lines = vec![
        format!("Best-effort reading: {interpretation}"),
        "Assumptions (reply to correct any):".to_owned(),
    ];
    for item in assumptions {
        lines.push(format!("  - {}", text(item, "Invalid assumption")?));
    }
    lines.push("Questions (answer one or more, in a follow-up ask):".to_owned());
    for (index, item) in questions.iter().enumerate() {
        lines.push(format!(
            "  {}. {}",
            index + 1,
            text(item, "Invalid question")?
        ));
    }
    lines.push("Ready-to-fill template:".to_owned());
    lines.extend(template.split('\n').map(|line| format!("  | {line}")));
    lines.push("Nothing was guessed or changed; re-submit the Ask with the allowed files filled in to proceed.".to_owned());
    let mut result = Value::object(vec![("clarification", Value::string(CLARIFICATION_HINT))]);
    result = domain::merge(&result, &reading)?;
    domain::put(&mut result, "guidance", value(&lines.join("\n")))?;
    Ok(result)
}
