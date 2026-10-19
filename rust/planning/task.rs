use super::*;

const DIRECTORY_PATTERN: &str = r#"(?i)(?:^|[.!?\n]\s*)(?:output|save|put|place)\s+(?:(?:it|them|the (?:output|documents|files))\s+)?(?:in|into|to)\s+(?:(?:a|an|the)\s+)?(?:new\s+)?(?:"([^"\n]+)"|'([^'\n]+)'|`([^`\n]+)`|([^\s"'`]+))\s+directory\b"#;
const DOCUMENTS: &str = r"docs?|documentation|documents?|designs?|setup|set-up|instructions?|guides?|manuals?|specifications?|reports?|proposals?|readme|plans?|pseudocode|outlines?|summar(?:y|ies)|tutorials?|notes";
const ARTIFACTS: &str = r"scripts?|programs?|applications?|apps?|code|websites?|web pages?|utility|utilities|modules?|functions?|librar(?:y|ies)|cli|packages?|converters?|calculators?";
const ASK_VERBS: &str = "implement|build|develop|code|write|generate|create|make";

pub fn normalized_line(input: &Value, label: &str) -> Result<String, Error> {
    let message = format!("{label} must be one nonempty line (at most 240 characters)");
    let source = text(input, &message)?;
    let source = regex(r"[\x00-\x1f\x7f]+")?.replace_all(&source, " ");
    let result = trim(&regex(r"\s+")?.replace_all(&source, " ")).to_owned();
    if result.is_empty() {
        return Err(Error::type_error(message));
    }
    Ok(result)
}

pub fn one_line(input: &Value, label: &str) -> Result<Text, Error> {
    Ok(decode(&slice(&normalized_line(input, label)?, 240)))
}

pub fn acceptance_check(input: &Value, index: usize) -> Result<Text, Error> {
    let check = normalized_line(input, "Acceptance check")?;
    let length = check.chars().count();
    if length > 240 {
        return Err(Error::type_error(format!(
            "Acceptance check must be at most 240 characters (check {}, got {length}, exceeds limit by {}); split it into several shorter checks (up to 8) without dropping requirements",
            index + 1,
            length - 240
        )));
    }
    Ok(decode(&check))
}

pub fn allowed_file(input: &Value) -> Result<String, Error> {
    let file = encode(&one_line(input, "Files allowed entry")?);
    if !matches(
        r"^(?:\*\*/\*|[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*(?:/\*\*)?)$",
        &file,
    )? || file.split('/').any(|part| part == "." || part == "..")
        || forbidden_write(&file)
    {
        return Err(Error::type_error(
            "Files allowed entries must stay inside the worktree and exclude protected files",
        ));
    }
    Ok(file)
}

fn quoted_spans(source: &str, remove_directives: bool) -> String {
    let mut result = String::new();
    let mut rest = source;
    while let Some((position, quote)) = rest
        .char_indices()
        .find(|(_, c)| matches!(c, '"' | '\'' | '`'))
    {
        result.push_str(&rest[..position]);
        let after = &rest[position + 1..];
        let end = after
            .find(quote)
            .filter(|end| !after[..*end].contains('\n'));
        if let Some(end) = end {
            let content = &after[..end];
            let directive = ["output ", "save ", "put ", "place "]
                .iter()
                .any(|word| content.to_lowercase().contains(word));
            if !remove_directives || !directive {
                result.push_str(&rest[position..position + end + 2]);
            }
            rest = &after[end + 1..];
        } else {
            result.push(quote);
            rest = after;
        }
    }
    result.push_str(rest);
    result
}

fn without_fences(source: &str) -> String {
    let mut lines = source.split_inclusive('\n').peekable();
    let mut result = String::new();
    while let Some(line) = lines.next() {
        let leading = line.len() - line.trim_start_matches(' ').len();
        let candidate = line.trim_start_matches(' ');
        let marker = candidate.chars().next().filter(|c| *c == '`' || *c == '~');
        let count = marker.map_or(0, |marker| {
            candidate.chars().take_while(|c| *c == marker).count()
        });
        if leading <= 3 && count >= 3 && line.ends_with('\n') {
            let opening = line.to_owned();
            let fence = marker
                .map(|c| c.to_string().repeat(count))
                .unwrap_or_default();
            let mut block = String::new();
            let mut closed = false;
            for next in lines.by_ref() {
                block.push_str(next);
                if next
                    .trim_start_matches(' ')
                    .trim_end_matches([' ', '\t', '\n'])
                    == fence
                {
                    closed = true;
                    break;
                }
            }
            if !closed {
                result.push_str(&opening);
                result.push_str(&block);
            }
        } else {
            result.push_str(line);
        }
    }
    result
}

pub fn output_directory(ask: &str) -> Result<Option<String>, Error> {
    if matches(
        r"(?im)^#{1,3} (?:Files allowed|Allowed files|files_allowed|allowed_files)\s*$",
        ask,
    )? {
        return Ok(None);
    }
    let source = intake::without_reference_urls(ask)?;
    let source = regex(r"(?s)<!--.*?-->")?.replace_all(&source, "");
    let source = without_fences(&source);
    let source = regex(r"(?m)^[ \t]*>.*$")?.replace_all(&source, "");
    let source = quoted_spans(&source, true);
    let mut directories = Vec::new();
    for captures in regex(DIRECTORY_PATTERN)?.captures_iter(&source) {
        let directory = (1..=4)
            .find_map(|index| captures.get(index).map(|entry| entry.as_str()))
            .ok_or_else(|| Error::type_error("Output directory directive is incomplete"))?;
        if !matches(
            r"^[A-Za-z0-9_-][A-Za-z0-9_.-]*(?:/[A-Za-z0-9_-][A-Za-z0-9_.-]*)*$",
            directory,
        )? || directory
            .split('/')
            .any(|part| part.ends_with('.') || part.eq_ignore_ascii_case("vendor"))
            || forbidden_write(directory)
            || forbidden_write(&format!("{directory}/output.md"))
        {
            return Err(Error::type_error(
                "Output directory must be relative, bounded, and exclude protected paths",
            ));
        }
        directories.push(directory.to_owned());
    }
    let directories = distinct(directories);
    if directories.len() > 1 {
        return Err(Error::type_error("Ask must grant one output directory"));
    }
    Ok(directories.first().cloned())
}

fn without_output_directive(source: &str) -> Result<String, Error> {
    Ok(regex(DIRECTORY_PATTERN)?
        .replace_all(source, |captures: &regex_lite::Captures<'_>| {
            let matched = captures.get(0).map_or("", |entry| entry.as_str());
            let prefix = matched
                .chars()
                .next()
                .filter(|c| matches!(c, '.' | '!' | '?' | '\n'));
            prefix.map_or_else(String::new, |c| {
                let rest = &matched[c.len_utf8()..];
                let spaces = rest
                    .chars()
                    .take_while(|c| unit_space(*c))
                    .collect::<String>();
                format!("{c}{spaces}")
            })
        })
        .into_owned())
}

fn ask_sentences(ask: &str) -> Result<Vec<String>, Error> {
    let source = without_output_directive(&intake::without_reference_urls(ask)?)?;
    let mut result = Vec::new();
    let mut start = 0;
    let mut preceding = None;
    for (position, character) in source.char_indices() {
        if character == '\n'
            || unit_space(character) && preceding.is_some_and(|c| matches!(c, '.' | '!' | '?'))
        {
            let part = trim(&source[start..position]);
            if !part.is_empty() {
                result.push(part.to_owned());
            }
            start = position + character.len_utf8();
        }
        if !unit_space(character) {
            preceding = Some(character);
        }
    }
    let part = trim(&source[start..]);
    if !part.is_empty() {
        result.push(part.to_owned());
    }
    Ok(result)
}

fn document_lead(sentence: &str) -> Result<bool, Error> {
    imperative_object(sentence, true)
}

fn imperative_object(sentence: &str, document: bool) -> Result<bool, Error> {
    let verbs = if document {
        format!("{ASK_VERBS}|compose|draft|document|prepare|give")
    } else {
        ASK_VERBS.to_owned()
    };
    let lead = regex(&format!(r"(?i)^(?:please\s+)?(?:{verbs})\s+(?:me\s+)?"))?;
    let Some(lead) = lead.find(sentence) else {
        return Ok(false);
    };
    let rest = &sentence[lead.end()..];
    let target = regex(&format!(
        r"(?i)^(?:{})\b",
        if document { DOCUMENTS } else { ARTIFACTS }
    ))?;
    let excluded = regex(&format!(
        r"(?i)^(?:{})\b",
        if document { ARTIFACTS } else { DOCUMENTS }
    ))?;
    let word = regex(r"^[A-Za-z0-9_.:-]+\s+")?;
    let mut remaining = rest;
    for _ in 0..=6 {
        if target.is_match(remaining) {
            return Ok(true);
        }
        if excluded.is_match(remaining) {
            break;
        }
        let Some(prefix) = word.find(remaining) else {
            break;
        };
        remaining = &remaining[prefix.end()..];
    }
    Ok(false)
}

fn test_deliverable(sentence: &str) -> Result<bool, Error> {
    let tests = regex(
        r"(?i)(?:^|\band\s+|,\s*)(?:please\s+)?(?:include|add|write|provide)\s+(?:(?:node|node:test|built-?in|unit|automated|executable|runnable)\s+){1,3}tests?\b",
    )?;
    for found in tests.find_iter(sentence) {
        let after = &sentence[found.end()..];
        if !matches(
            r"(?i)^\s+(?:(?:as|for)\s+(?:examples?|samples?|illustrations?|demonstrations?)|(?:are|is)\s+(?:examples?|samples?|illustrative))\b",
            after,
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn requests_executable(ask: &str) -> Result<bool, Error> {
    for sentence in ask_sentences(ask)? {
        if imperative_object(&sentence, false)?
            || !document_lead(&sentence)? && test_deliverable(&sentence)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn directory_document(ask: &str) -> Result<bool, Error> {
    Ok(output_directory(ask)?.is_some()
        && matches(
            &format!(r"(?i)\b(?:{DOCUMENTS})\b"),
            &without_output_directive(trim(ask))?,
        )?
        && !requests_executable(ask)?)
}

pub fn validate_directory_files(files: &[String], directory: &str) -> Result<(), Error> {
    let folded = |value: &str| {
        if cfg!(windows) {
            value.to_lowercase()
        } else {
            value.to_owned()
        }
    };
    for file in files {
        allowed_file(&value(file))?;
        if file.contains('*')
            || !folded(file).starts_with(&format!("{}/", folded(directory)))
            || file
                .split('/')
                .any(|part| part.starts_with('.') || part.eq_ignore_ascii_case("vendor"))
        {
            return Err(Error::type_error(format!(
                "Planner must choose concrete, unprotected files inside the granted output directory: \"{file}\" is not permitted beneath \"{directory}\". Use exact file paths; no wildcards, hidden path components (including .gitignore), vendor paths, or outside files. Keep test scratch fixtures in the OS temporary directory, not in files_allowed."
            )));
        }
    }
    Ok(())
}

pub fn validate_directory_executable(
    ask: &str,
    files: &[String],
    task_class: Option<&str>,
) -> Result<(), Error> {
    if output_directory(ask)?.is_none() || !requests_executable(ask)? {
        return Ok(());
    }
    if task_class == Some("docs")
        || files
            .iter()
            .all(|file| file.to_lowercase().ends_with(".md"))
    {
        return Err(Error::type_error(
            "The Ask requests a runnable utility; the plan must include executable code files, not Markdown-only documents or a docs task class",
        ));
    }
    let mut tests = false;
    for sentence in ask_sentences(ask)? {
        tests |= !document_lead(&sentence)? && test_deliverable(&sentence)?;
    }
    let test_file = regex(r"(?i)(?:^|[/.-])(?:tests?|spec)(?:[/.-]|$)")?;
    if tests
        && !files
            .iter()
            .any(|file| !file.to_lowercase().ends_with(".md") && test_file.is_match(file))
    {
        return Err(Error::type_error(
            "The Ask requests tests; the plan must include an executable (non-Markdown) test file among the allowed files",
        ));
    }
    Ok(())
}

pub fn validate_directory_documents(
    ask: &str,
    files: &[String],
    checks: &[String],
) -> Result<(), Error> {
    if !directory_document(ask)? {
        return Ok(());
    }
    if files
        .iter()
        .any(|file| !file.to_lowercase().ends_with(".md"))
    {
        return Err(Error::type_error(
            "Directory documentation asks authorize Markdown documents, not executable or deployment artifacts",
        ));
    }
    let commands = regex(
        r"(?i)\b(?:npm|npx|pnpm|yarn|pip|apt|docker|kubectl|terraform|curl|wget|ssh|bash|powershell|Invoke-WebRequest)\b|\b(?:git\s+(?:push|clone)|gh\s+(?:pr|release)|node\s+--test)\b",
    )?;
    let action = regex(
        r"(?i)\b(?:install(?:ation|ed|s)?|deploy(?:ment|ed|s)?|publish(?:ed|es|ing)?|purchase(?:d|s)?|buy|account|network|request|fetch|download(?:ed|s)?|upload(?:ed|s)?|serve(?:d|s)?|hosting)\b",
    )?;
    let assertion = regex(
        r"(?i)\b(?:run|execute|succeeds?|successful(?:ly)?|exits?|passes?|created|activated|completed|live|reachable)\b|\b(?:is|are|was|were)\s+(?:installed|deployed|published|purchased|serving|running)\b",
    )?;
    let content = regex(
        r"(?i)\b(?:documents?|describes?|explains?|outlines?|contains?|includes?|cover(?:s|ing)?|lists?|mentions?|states?|warns?|contrasts?|shows?|compares?|defines?|presents?|gives?|indexes)\b",
    )?;
    let active = regex(
        r"(?i)(?:[;:]|\b(?:and|then))\s+(?:run|execute|install|deploy|publish|purchase|buy|register|create an? account|fetch|download|upload|request)(?:s|ed)?\b",
    )?;
    for (index, check) in checks.iter().enumerate() {
        if !content.is_match(check)
            || active.is_match(check)
            || (commands.is_match(check) || action.is_match(check)) && assertion.is_match(check)
        {
            return Err(Error::type_error(format!(
                "Directory documentation requires document-content checks, not installation, deployment, publication, network, purchase or account actions (rejected check {}). Use a document-content verb and remove operation-success assertions.",
                index + 1
            )));
        }
    }
    Ok(())
}

pub fn checked_list<'a>(
    input: Option<&'a Value>,
    label: &str,
    maximum: usize,
) -> Result<&'a [Value], Error> {
    let items = input.and_then(domain::items);
    if items.is_none_or(|items| items.is_empty() || items.len() > maximum) {
        let suffix = items
            .filter(|items| items.len() > maximum)
            .map_or_else(String::new, |items| {
                format!(" (got {}; merge related entries)", items.len())
            });
        return Err(Error::type_error(format!(
            "{label} must contain 1-{maximum} entries{suffix}"
        )));
    }
    items.ok_or_else(|| Error::type_error("Missing validated planning list"))
}

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub start: usize,
    pub content_start: usize,
    pub content: String,
    pub raw: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct Parts {
    pub frontmatter: String,
    pub title: String,
    pub header: String,
    pub body: String,
    pub sections: Vec<Section>,
}

impl Parts {
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|section| section.name == name)
    }

    pub fn as_value(&self) -> Value {
        let body = format!("{}{}", self.header, self.body);
        Value::object(vec![
            ("frontmatter", value(&self.frontmatter)),
            ("title", value(&self.title)),
            ("header", value(&self.header)),
            ("body", value(&self.body)),
            (
                "sections",
                Value::Array(
                    self.sections
                        .iter()
                        .map(|section| {
                            Value::object(vec![
                                ("name", value(&section.name)),
                                (
                                    "start",
                                    Value::Number(body[..section.start].chars().count() as f64),
                                ),
                                (
                                    "contentStart",
                                    Value::Number(
                                        body[..section.content_start.min(body.len())]
                                            .chars()
                                            .count() as f64
                                            + usize::from(section.content_start > body.len())
                                                as f64,
                                    ),
                                ),
                                ("content", value(&section.content)),
                                ("rawContent", value(&section.raw)),
                                ("source", value(&section.source)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

fn section_name(heading: &str) -> Result<String, Error> {
    let heading = heading.to_lowercase();
    let heading = regex("[_-]+")?.replace_all(&heading, " ");
    let heading = regex(r"\s+")?.replace_all(&heading, " ");
    let heading = trim(heading.strip_suffix(':').unwrap_or(&heading));
    let heading = regex(r"^(ask|original ask)\s*\([^)]*\)$")?.replace(heading, "$1");
    Ok(match heading.as_ref() {
        "original ask" => "ask",
        "allowed files" => "files allowed",
        _ => &heading,
    }
    .to_owned())
}

pub fn sections(input: &Value) -> Result<Parts, Error> {
    let message = "TASK.md must be UTF-8 text of at most 64 KiB without control characters";
    let source = text(input, message)?;
    if decode(&source).utf8_len() > MAX_ARTIFACT_BYTES
        || source.chars().any(
            |c| matches!(c, '\0'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}'),
        )
    {
        return Err(Error::type_error(message));
    }
    let (frontmatter, body) = skills::split_frontmatter(&source)?;
    if body.contains('\r') {
        return Err(Error::type_error("TASK.md contains a lone carriage return"));
    }
    let heading = regex(r"(?m)^# +(.+?)[ \t]*$")?;
    let captures = heading
        .captures(&body)
        .filter(|captures| {
            captures
                .get(0)
                .is_some_and(|entry| trim(&body[..entry.start()]).is_empty())
        })
        .ok_or_else(|| Error::type_error("TASK.md needs a title heading"))?;
    let found = captures
        .get(0)
        .ok_or_else(|| Error::type_error("TASK.md needs a title heading"))?;
    let title = captures.get(1).map_or("", |entry| entry.as_str());
    let title = regex(r"(?i)^(?:task|title):[ \t]*")?.replace(title, "");
    let promoted = if matches(r"(?i)^(?:task[ \t]*)?title$|^task$", &title)? {
        body[found.end()..]
            .split('\n')
            .map(trim)
            .find(|line| !line.is_empty())
    } else {
        None
    };
    let title = match promoted {
        Some(promoted) if !matches(r"^(?:[#>|-]|\w+:)", promoted)? => promoted,
        _ => &title,
    };
    let title = encode(&one_line(
        &value(if title.is_empty() { "Task" } else { title }),
        "Task title",
    )?);
    let mut starts = Vec::new();
    let mut known = Vec::new();
    let mut offset = 0;
    let mut fence = None;
    let fenced = regex(r"^ {0,3}(`{3,}|~{3,})")?;
    let heading = regex(r"^(#{2,6}) +(.+?)[ \t]*$")?;
    for line in body.split('\n') {
        if let Some(marker) = fenced.find(line) {
            let marker = marker.as_str().trim_start().chars().next();
            if fence.is_none() {
                fence = marker;
            } else if marker == fence {
                fence = None;
            }
        } else if fence.is_none() {
            if let Some(captures) = heading.captures(line) {
                let heading = captures.get(2).map_or("", |entry| entry.as_str());
                let name = section_name(heading)?;
                if [
                    "ask",
                    "acceptance checks",
                    "files allowed",
                    "metadata",
                    "planning failure",
                ]
                .contains(&name.as_str())
                    && known.contains(&name)
                {
                    return Err(Error::type_error(format!(
                        "TASK.md has duplicate {name} sections"
                    )));
                }
                known.push(name.clone());
                starts.push((name.clone(), offset, offset + line.len() + 1));
                if name == "ask"
                    && !matches(r"(?i)^original[ _-]+ask\b", heading)?
                    && known.iter().any(|name| name == "acceptance checks")
                    && known.iter().any(|name| name == "files allowed")
                {
                    break;
                }
            }
        }
        offset += line.len() + 1;
    }
    let mut sections = Vec::new();
    for (index, (name, start, content_start)) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map_or(body.len(), |entry| entry.1);
        let raw = body.get(*content_start..end).unwrap_or("").to_owned();
        sections.push(Section {
            name: name.clone(),
            start: *start,
            content_start: *content_start,
            content: trim(&raw).to_owned(),
            raw,
            source: body[*start..end].to_owned(),
        });
    }
    let body_start = starts.first().map_or(body.len(), |entry| entry.1);
    Ok(Parts {
        frontmatter,
        title,
        header: body[..body_start].to_owned(),
        body: body[body_start..].to_owned(),
        sections,
    })
}

pub(super) fn section_list(
    section: Option<&Section>,
    label: &str,
    continuations: bool,
) -> Result<Vec<String>, Error> {
    let section = section
        .filter(|section| !section.content.is_empty())
        .ok_or_else(|| Error::type_error(format!("TASK.md must contain {label}")))?;
    let marker = regex(r"^(?:[-*+]|\d+[.)])[ \t]+")?;
    let mut items: Vec<String> = Vec::new();
    for line in section
        .content
        .split('\n')
        .filter(|line| !trim(line).is_empty())
    {
        let item = marker.replace(trim(line), "").into_owned();
        if continuations && line.chars().next().is_some_and(unit_space) && !items.is_empty() {
            if let Some(last) = items.last_mut() {
                last.push(' ');
                last.push_str(&item);
            }
        } else {
            let unquoted = regex(r"^`([^`]+)`$")?.replace(&item, "$1").into_owned();
            items.push(unquoted);
        }
    }
    Ok(items)
}

pub fn application_files(input: &Value) -> Result<Vec<String>, Error> {
    let items = array(input, "Files allowed must be a list")?;
    let filtered = Value::Array(
        items
            .iter()
            .filter(|entry| {
                entry.as_string().is_none_or(|entry| {
                    let entry = encode(entry);
                    !PLANNER_ARTIFACTS
                        .iter()
                        .chain(PLAN_ARTIFACTS)
                        .any(|name| entry.eq_ignore_ascii_case(name))
                })
            })
            .cloned()
            .collect(),
    );
    checked_list(Some(&filtered), "Files allowed", 32)?
        .iter()
        .map(allowed_file)
        .collect()
}

pub fn files_allowed(task: &Value) -> Result<Vec<String>, Error> {
    let parts = sections(task)?;
    application_files(&strings(&section_list(
        parts.section("files allowed"),
        "an Allowed Files list (or Files allowed)",
        false,
    )?))
}

pub fn ask_content_lines(source: &str) -> Result<Vec<String>, Error> {
    let source = regex(r"(?s)<!--.*?-->")?.replace_all(source, "");
    let mut result = Vec::new();
    for line in source.replace("\r\n", "\n").split('\n') {
        let text = trim(line);
        if matches(r"(?i)^#{1,6}\s+task[ _-]+metadata\s*:?$", text)? {
            break;
        }
        if matches(r"^(?:`{3,}|~{3,})[A-Za-z0-9_-]*$", text)? {
            continue;
        }
        let text = regex(r"^#{1,6}\s+")?.replace(text, "");
        let text = text.replace('`', "");
        let text = regex(r"\s+")?.replace_all(&text, " ");
        let text = trim(&text);
        if text.is_empty()
            || matches(
                r"(?i)^(?:original[ _-]+ask|ask|description|summary|details|title)(?:\s*\([^)]*\))?\s*:?$",
                text,
            )?
            || matches(r"(?i)^(?:task_class|difficulty|estimate_min):", text)?
        {
            continue;
        }
        result.push(text.to_owned());
    }
    Ok(result)
}

pub fn normalize_ask(source: &str) -> Result<String, Error> {
    Ok(ask_content_lines(source)?.join(" "))
}

pub fn ensure_original_ask(task: &Text, title: Option<&Text>) -> Result<Text, Error> {
    let source = encode(task);
    let required = title
        .map(encode)
        .map(|title| trim(&title).to_owned())
        .unwrap_or_default();
    if required.is_empty() || normalize_ask(&source)?.contains(&normalize_ask(&required)?) {
        return Ok(task.clone());
    }
    let heading = regex(r"(?im)^## (?:Original Ask|Ask)\s*$")?;
    if let Some(found) = heading.find(&source) {
        return Ok(decode(&format!(
            "{}{}\n\n{required}{}",
            &source[..found.start()],
            found.as_str(),
            &source[found.end()..]
        )));
    }
    Ok(decode(&format!(
        "{}\n\n## Original Ask\n\n{required}\n",
        trim(&source)
    )))
}

pub fn ensure_title(task: &Text, title: Option<&Text>) -> Result<Text, Error> {
    let source = encode(task);
    let required = title
        .map(encode)
        .map(|title| trim(&title).to_owned())
        .unwrap_or_default();
    if required.is_empty() || required.contains(['\r', '\n']) {
        return Ok(task.clone());
    }
    let (frontmatter, body) = match skills::split_frontmatter(&source) {
        Ok(parts) => parts,
        Err(_) => return Ok(task.clone()),
    };
    let pattern = regex(r"(?m)^# +(.+?)[ \t]*$")?;
    let Some(captures) = pattern.captures(&body) else {
        return Ok(task.clone());
    };
    let Some(found) = captures.get(0) else {
        return Ok(task.clone());
    };
    if !trim(&body[..found.start()]).is_empty() {
        return Ok(task.clone());
    }
    let heading = captures.get(1).map_or("", |entry| entry.as_str());
    let heading = regex(r"(?i)^(?:task|title):[ \t]*")?.replace(heading, "");
    if normalize_ask(&heading)? == normalize_ask(&required)?
        || matches(r"(?i)^(?:task[ \t]*)?title$|^task$", &heading)?
    {
        return Ok(task.clone());
    }
    Ok(decode(&format!(
        "{frontmatter}{}# Task: {required}{}",
        &body[..found.start()],
        &body[found.end()..]
    )))
}

pub fn named_files(ask: &str) -> Result<Vec<String>, Error> {
    let windows = regex(
        r"(?i)(?:allowed files|files allowed|only edit|edit only|do not edit any other file)[^\n]*",
    )?
    .find_iter(ask)
    .map(|entry| entry.as_str().to_owned())
    .collect::<Vec<_>>();
    let source = if windows.is_empty() {
        ask.to_owned()
    } else {
        windows.join("\n")
    };
    Ok(distinct(
        regex(r"(?:[\w.@-]+/)+[\w.@-]+|[\w.@-]+\.(?:md|mjs|js|cjs|json|yml|yaml|txt)")?
            .find_iter(&source)
            .map(|entry| entry.as_str().to_owned())
            .collect(),
    )
    .into_iter()
    .take(32)
    .collect())
}

pub fn ensure_files(task: &Text, ask: &str) -> Result<Text, Error> {
    let source = encode(task);
    if matches(r"(?im)^## +(?:Allowed Files|Files allowed)\s*$", &source)? {
        return Ok(task.clone());
    }
    let directory = output_directory(ask)?;
    let files = named_files(ask)?
        .into_iter()
        .filter(|file| {
            directory
                .as_ref()
                .is_none_or(|directory| file.starts_with(&format!("{directory}/")))
        })
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Ok(task.clone());
    }
    Ok(decode(&format!(
        "{}\n\n## Allowed Files\n\n{}\n",
        trim(&source),
        files
            .iter()
            .map(|file| format!("- {file}"))
            .collect::<Vec<_>>()
            .join("\n")
    )))
}

pub fn ensure_checks(task: &Text, ask: &str) -> Result<Text, Error> {
    let source = encode(task);
    if matches(
        r"(?im)^## +(?:Acceptance Checks|acceptance_checks)\s*$",
        &source,
    )? {
        return Ok(task.clone());
    }
    let checks = ask
        .replace("\r\n", "\n")
        .split('\n')
        .map(trim)
        .filter(|line| line.starts_with("- ") || line.starts_with("* "))
        .take(16)
        .map(|line| line.get(2..).unwrap_or("").to_owned())
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return Ok(task.clone());
    }
    Ok(decode(&format!(
        "{}\n\n## Acceptance Checks\n\n{}\n",
        trim(&source),
        checks
            .iter()
            .map(|check| format!("- {check}"))
            .collect::<Vec<_>>()
            .join("\n")
    )))
}

#[derive(Debug, Clone, Default)]
pub struct Authority {
    pub expected_ask: Option<Text>,
    pub issue_title: Option<Text>,
    pub issue_body: Option<Text>,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub title: Text,
    pub ask: Text,
    pub acceptance_checks: Vec<Text>,
    pub files_allowed: Vec<String>,
}

impl Document {
    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("title", Value::String(self.title.clone())),
            ("ask", Value::String(self.ask.clone())),
            (
                "acceptance_checks",
                Value::Array(
                    self.acceptance_checks
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            ("files_allowed", strings(&self.files_allowed)),
        ])
    }
}

pub fn parse(input: &Value, authority: &Authority) -> Result<Document, Error> {
    let parts = sections(input)?;
    let section = parts.section("ask");
    let mut ask = section
        .map_or("", |section| section.content.as_str())
        .to_owned();
    let normalized = normalize_ask(&ask)?;
    if normalized.is_empty() {
        return Err(Error::type_error(
            "Planner TASK.md must contain a nonempty Original Ask (or Ask)",
        ));
    }
    let authoritative = authority
        .expected_ask
        .as_ref()
        .or(authority.issue_body.as_ref())
        .map(encode);
    let directory = output_directory(authoritative.as_deref().unwrap_or(&ask))?;
    if directory.is_some() {
        let section = section.ok_or_else(|| Error::type_error("Missing Original Ask"))?;
        let original = matches(r"(?i)^#{2,6} +Original[ _-]+Ask\b", &section.source)?;
        ask = if original && section.raw.starts_with('\n') && section.raw.ends_with("\n\n") {
            section.raw[1..section.raw.len() - 2].to_owned()
        } else {
            section
                .raw
                .strip_suffix('\n')
                .unwrap_or(&section.raw)
                .to_owned()
        };
        if authoritative
            .as_ref()
            .is_some_and(|required| ask != required.replace("\r\n", "\n"))
        {
            return Err(Error::type_error(
                "Directory-scoped TASK.md must preserve the unchanged original Ask",
            ));
        }
    }
    if authority.expected_ask.is_some()
        || authority.issue_title.is_some()
        || authority.issue_body.is_some()
    {
        let mut candidates = Vec::new();
        if let Some(title) = &authority.issue_title {
            candidates.push(normalize_ask(&encode(title))?);
        }
        let body = authority
            .issue_body
            .as_ref()
            .or(authority.expected_ask.as_ref())
            .map(encode)
            .unwrap_or_default();
        candidates.extend(ask_content_lines(&body)?.into_iter().take(1));
        if !candidates
            .iter()
            .filter(|candidate| !candidate.is_empty())
            .any(|candidate| normalized.contains(candidate))
        {
            return Err(Error::type_error(
                "Planner TASK.md must contain the unchanged Ask title or first issue body line in Original Ask (or Ask)",
            ));
        }
    }
    let checks = section_list(
        parts.section("acceptance checks"),
        "Acceptance Checks (or acceptance_checks)",
        true,
    )?;
    let checks_value = strings(&checks);
    let checks = checked_list(Some(&checks_value), "Acceptance checks", 8)?
        .iter()
        .map(|entry| normalized_line(entry, "Acceptance check"))
        .collect::<Result<Vec<_>, _>>()?;
    let files = files_allowed(input)?;
    if let Some(directory) = directory {
        validate_directory_files(&files, &directory)?;
        let source = authoritative.as_deref().unwrap_or(&ask);
        validate_directory_documents(source, &files, &checks)?;
        let original = text(input, "TASK.md must be text")?;
        let class_re = regex(r"(?im)^(?:-[ \t]+)?task_class:[ \t]*(\w+)")?;
        let class = class_re
            .captures(&original)
            .and_then(|captures| captures.get(1).map(|entry| entry.as_str().to_lowercase()));
        validate_directory_executable(source, &files, class.as_deref())?;
        if files.iter().all(|file| file.ends_with(".md"))
            && checks.iter().all(|check| check.contains("node --test"))
        {
            return Err(Error::type_error(
                "Directory documentation requires document-content acceptance checks",
            ));
        }
    }
    Ok(Document {
        title: decode(&parts.title),
        ask: decode(&ask),
        acceptance_checks: checks.iter().map(|check| decode(check)).collect(),
        files_allowed: files,
    })
}
