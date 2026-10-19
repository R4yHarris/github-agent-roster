use super::*;
use intake::Kind;

fn proposing(files: &[String], proposed: bool, repository: Option<&[String]>) -> bool {
    files.is_empty() && (proposed || repository.is_some())
}

pub fn validate(
    input: &Value,
    kind: Kind,
    files: &[String],
    repository: Option<&[String]>,
    proposed: bool,
) -> Result<Value, Error> {
    if !matches!(kind, Kind::Feature | Kind::Initiative) {
        return Err(Error::type_error(
            "PLAN requires a feature or initiative Ask",
        ));
    }
    fields(
        input,
        &["outcomes", "issues"],
        &[],
        "PLAN fields must contain outcomes, issues",
    )?;
    let outcomes = task::checked_list(input.get("outcomes"), "Plan outcomes", 8)?
        .iter()
        .map(|entry| task::one_line(entry, "Outcome").map(Value::String))
        .collect::<Result<Vec<_>, _>>()?;
    let maximum = if kind == Kind::Feature { 5 } else { 12 };
    let issues = input
        .get("issues")
        .and_then(domain::items)
        .filter(|issues| (2..=maximum).contains(&issues.len()))
        .ok_or_else(|| {
            Error::type_error(format!(
                "{} PLAN must contain 2-{maximum} child issue drafts",
                kind.as_str()
            ))
        })?;
    let mut titles = Vec::new();
    let mut result = Vec::new();
    for issue in issues {
        fields(
            issue,
            &["title", "outcome", "acceptance_checks", "wave"],
            &["files_allowed"],
            "PLAN fields must contain title, outcome, acceptance_checks, wave",
        )?;
        let title = encode(&task::one_line(
            required(issue, "title")?,
            "Child issue title",
        )?);
        let folded = title.to_lowercase();
        if titles.contains(&folded) {
            return Err(Error::type_error("Child issue titles must be distinct"));
        }
        titles.push(folded);
        let wave = domain::safe_integer(issue.get("wave"))
            .filter(|wave| (1..=8).contains(wave))
            .ok_or_else(|| Error::type_error("Issue wave must be an integer from 1 to 8"))?;
        let child_files = if issue.get("files_allowed").is_none()
            || issue
                .get("files_allowed")
                .and_then(domain::items)
                .is_some_and(|items| items.is_empty())
        {
            Vec::new()
        } else {
            task::checked_list(issue.get("files_allowed"), "Child issue files", 32)?
                .iter()
                .map(task::allowed_file)
                .collect::<Result<Vec<_>, _>>()?
        };
        let proposed = proposing(files, proposed, repository);
        if !proposed && child_files.iter().any(|file| !is_allowed(file, files)) {
            return Err(Error::type_error(
                "PLAN cannot invent allowed files beyond the human Ask scope",
            ));
        }
        if proposed {
            if let Some(repository) = repository {
                if child_files.is_empty() {
                    return Err(text_error(
                        "TypeError",
                        &format!(
                            "Child draft \"{title}\" needs files_allowed so it can run as a slice"
                        ),
                    ));
                }
                let unknown = child_files
                    .iter()
                    .filter(|file| {
                        if file.as_str() == "**/*" {
                            return true;
                        }
                        if let Some(prefix) = file.strip_suffix("**") {
                            return !repository.iter().any(|entry| entry.starts_with(prefix));
                        }
                        if repository.contains(file) {
                            return false;
                        }
                        file.rsplit_once('/').is_none_or(|(directory, _)| {
                            !repository
                                .iter()
                                .any(|entry| entry.starts_with(&format!("{directory}/")))
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if !unknown.is_empty() {
                    return Err(Error::type_error(format!(
                        "PLAN files must be tracked repository paths or new files in tracked directories: {}",
                        unknown.join(", ")
                    )));
                }
            }
        }
        let checks = task::checked_list(issue.get("acceptance_checks"), "Child issue checks", 8)?
            .iter()
            .enumerate()
            .map(|(index, entry)| task::acceptance_check(entry, index).map(Value::String))
            .collect::<Result<Vec<_>, _>>()?;
        result.push(Value::object(vec![
            ("title", value(&title)),
            (
                "outcome",
                Value::String(task::one_line(
                    required(issue, "outcome")?,
                    "Child issue outcome",
                )?),
            ),
            ("acceptance_checks", Value::Array(checks)),
            ("wave", Value::Number(wave as f64)),
            ("files_allowed", strings(&child_files)),
        ]));
    }
    let mut waves = result
        .iter()
        .filter_map(|issue| domain::safe_integer(issue.get("wave")))
        .collect::<Vec<_>>();
    waves.sort_unstable();
    waves.dedup();
    if waves
        .iter()
        .enumerate()
        .any(|(index, wave)| *wave != index as u64 + 1)
    {
        return Err(Error::type_error(
            "Issue wave labels must start at wave:1 with no gaps",
        ));
    }
    let mut plan = Value::object(vec![
        ("outcomes", Value::Array(outcomes)),
        ("issues", Value::Array(result)),
    ]);
    if proposing(files, proposed, repository) {
        domain::put(&mut plan, "proposedScope", Value::Boolean(true))?;
    }
    Ok(plan)
}

pub fn stub(ask: &Value, kind: Kind, files: &[String]) -> Result<Value, Error> {
    let requirements = intake::requirements(ask, true)?;
    let clean = intake::clean_ask(ask)?;
    let first = clean
        .split('\n')
        .find(|line| !trim(line).is_empty() && !line.starts_with('#'))
        .unwrap_or(&clean);
    let outcomes = requirements
        .outcomes
        .unwrap_or_else(|| vec![slice(first, 240)]);
    let issue = |title: String, outcome: &str, wave: usize, checks: Vec<String>| {
        Value::object(vec![
            ("title", value(&title)),
            ("outcome", value(outcome)),
            ("wave", Value::Number(wave as f64)),
            ("acceptance_checks", strings(&checks)),
            ("files_allowed", strings(files)),
        ])
    };
    let mut issues = if outcomes.len() > 1 {
        outcomes
            .iter()
            .enumerate()
            .map(|(index, outcome)| {
                issue(
                    slice(&format!("Slice {}: {outcome}", index + 1), 240),
                    outcome,
                    index + 1,
                    vec![
                        "The named outcome has passing acceptance evidence".to_owned(),
                        "Allowed files are explicitly agreed before an executable TASK is written"
                            .to_owned(),
                    ],
                )
            })
            .collect::<Vec<_>>()
    } else {
        let outcome = outcomes.first().map_or("", String::as_str);
        vec![
            issue(
                "Define the first bounded outcome and file scope".to_owned(),
                outcome,
                1,
                vec!["A human agrees one outcome, allowed files, and acceptance checks".to_owned()],
            ),
            issue(
                "Deliver and verify the agreed slice".to_owned(),
                outcome,
                2,
                vec!["The agreed slice passes tests, excellence, and read-only review".to_owned()],
            ),
        ]
    };
    if kind == Kind::Initiative {
        issues.push(issue(
            "Verify integrated outcomes and remaining waves".to_owned(),
            "Confirm the initiative outcomes with linked issue and PR evidence",
            (issues.len() + 1).min(8),
            vec![
                "Each outcome has issue and reviewed PR evidence".to_owned(),
                "Remaining work stays in GitHub issues".to_owned(),
            ],
        ));
    }
    validate(
        &Value::object(vec![
            ("outcomes", strings(&outcomes)),
            ("issues", Value::Array(issues)),
        ]),
        kind,
        files,
        None,
        false,
    )
}

pub fn render(
    plan: &Value,
    ask: &Value,
    title: Option<&Value>,
    kind: Kind,
    reference: &str,
) -> Result<Text, Error> {
    let ask = intake::clean_ask(ask)?;
    let default_title = regex(r"^#+\s*")?
        .replace(ask.split('\n').next().unwrap_or(""), "")
        .into_owned();
    let title = title
        .cloned()
        .unwrap_or_else(|| value(&slice(&default_title, 240)));
    let heading = encode(&task::one_line(&title, "Plan title")?);
    let reference = encode(&task::one_line(&value(reference), "Plan reference")?);
    let issues = array(required(plan, "issues")?, "Plan issues must be arrays")?;
    let outcomes = array(required(plan, "outcomes")?, "Plan outcomes must be arrays")?;
    let all_scoped = issues.iter().all(|issue| {
        issue
            .get("files_allowed")
            .and_then(domain::items)
            .is_some_and(|files| !files.is_empty())
    });
    let mut text = format!(
        "# Plan: {heading}\n\nAsk kind: {}\nReference: {reference}\n\n",
        kind.as_str()
    );
    if plan.get("proposedScope") == Some(&Value::Boolean(true)) {
        text.push_str("Scope: planner-proposed\n\n");
    }
    text.push_str(
        "Planning only. Child issues below are drafts, not created issues or executable tasks. ",
    );
    text.push_str(if all_scoped {
        "An issue run opens them as linked GitHub issues and continues with the first open wave slice; planned files are scope that tiered expansion and review govern.\n\n"
    } else { "Review and create them on GitHub, then /run each bounded slice separately. No coder or publisher ran.\n\n" });
    text.push_str("## Outcomes\n\n");
    text.push_str(
        &outcomes
            .iter()
            .map(|outcome| {
                super::text(outcome, "Outcome must be text").map(|outcome| format!("- {outcome}"))
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("\n"),
    );
    let mut waves = issues
        .iter()
        .filter_map(|issue| domain::safe_integer(issue.get("wave")))
        .collect::<Vec<_>>();
    waves.sort_unstable();
    waves.dedup();
    text.push_str("\n\n## Waves\n\n");
    let wave_lines = waves
        .iter()
        .map(|wave| {
            let drafts = issues
                .iter()
                .enumerate()
                .filter_map(|(index, issue)| {
                    (domain::safe_integer(issue.get("wave")) == Some(*wave))
                        .then_some((index + 1).to_string())
                })
                .collect::<Vec<_>>();
            format!("- `wave:{wave}`: drafts {}", drafts.join(", "))
        })
        .collect::<Vec<_>>();
    text.push_str(&wave_lines.join("\n"));
    text.push_str("\n\nWaves are issue labels, not a separate queue or database.\n\n## Child issue drafts\n\n");
    let mut children = Vec::new();
    for (index, issue) in issues.iter().enumerate() {
        let title = super::text(required(issue, "title")?, "Issue title must be text")?;
        let outcome = super::text(required(issue, "outcome")?, "Issue outcome must be text")?;
        let wave = domain::safe_integer(issue.get("wave"))
            .ok_or_else(|| Error::type_error("Missing issue wave"))?;
        let checks = array(
            required(issue, "acceptance_checks")?,
            "Issue checks must be arrays",
        )?
        .iter()
        .map(|check| {
            super::text(check, "Issue check must be text").map(|check| format!("- {check}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
        let files = array(
            required(issue, "files_allowed")?,
            "Issue files must be arrays",
        )?;
        let files = if files.is_empty() {
            "- Human must name allowed files before this draft becomes an executable TASK."
                .to_owned()
        } else {
            files
                .iter()
                .map(|file| {
                    super::text(file, "Issue file must be text").map(|file| format!("- `{file}`"))
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("\n")
        };
        children.push(format!("### Draft {}: {title}\n\nLabels: `wave:{wave}`\n\nOutcome: {outcome}\n\nAcceptance checks:\n{}\n\nAllowed files:\n{files}", index + 1, checks.join("\n")));
    }
    text.push_str(&children.join("\n\n"));
    text.push_str(&format!("\n\n## Original Ask\n\n{ask}\n"));
    let result = decode(&text);
    if result.utf8_len() > MAX_ARTIFACT_BYTES {
        return Err(Error::type_error("PLAN.md must be at most 64 KiB"));
    }
    Ok(result)
}

fn bullets(source: &str) -> Result<Vec<String>, Error> {
    trim(source)
        .split('\n')
        .map(|line| {
            let line = line.strip_prefix("- ").ok_or_else(|| {
                Error::new("Error", "Plan lists must contain explicit bullet entries")
            })?;
            Ok(line
                .strip_prefix('`')
                .and_then(|line| line.strip_suffix('`'))
                .filter(|line| !line.is_empty())
                .unwrap_or(line)
                .to_owned())
        })
        .collect()
}

pub fn parse_document(input: &Value) -> Result<Value, Error> {
    let source = text(input, "PLAN.md must be at most 64 KiB")?;
    if decode(&source).utf8_len() > MAX_ARTIFACT_BYTES {
        return Err(Error::type_error("PLAN.md must be at most 64 KiB"));
    }
    let source = source.replace("\r\n", "\n");
    let invalid = || Error::new("Error", "Current PLAN.md has no valid child wave drafts");
    let kind = regex(r"(?m)^Ask kind: (feature|initiative)$")?
        .captures(&source)
        .and_then(|captures| captures.get(1).map(|entry| entry.as_str()))
        .ok_or_else(invalid)?;
    let reference = regex(r"(?m)^Reference: (issue:[1-9]\d*|local:[A-Za-z0-9._-]+)$")?
        .captures(&source)
        .and_then(|captures| captures.get(1).map(|entry| entry.as_str()))
        .ok_or_else(invalid)?;
    let outcomes = source
        .split_once("## Outcomes\n\n")
        .and_then(|(_, rest)| rest.split_once("\n\n## Waves\n"))
        .map(|(outcomes, _)| outcomes)
        .ok_or_else(invalid)?;
    let children = source
        .split_once("## Child issue drafts\n\n")
        .and_then(|(_, rest)| rest.split_once("\n\n## Original Ask\n\n"))
        .ok_or_else(invalid)?;
    if outcomes.is_empty() || children.1.is_empty() {
        return Err(invalid());
    }
    let start = regex(r"(?m)^### Draft [1-9]\d*:")?;
    let positions = start
        .find_iter(children.0)
        .map(|entry| entry.start())
        .collect::<Vec<_>>();
    let mut issues = Vec::new();
    let pattern = regex(
        r"(?s)^### Draft ([1-9]\d*): ([^\n]+)\n\nLabels: `wave:([1-8])`\n\nOutcome: ([^\n]+)\n\nAcceptance checks:\n(.*?)\n\nAllowed files:\n(.+)$",
    )?;
    for (index, position) in positions.iter().enumerate() {
        let end = positions
            .get(index + 1)
            .copied()
            .unwrap_or(children.0.len());
        let captures = pattern
            .captures(trim(&children.0[*position..end]))
            .ok_or_else(|| Error::new("Error", "Plan draft numbering or fields are invalid"))?;
        let get = |index| captures.get(index).map_or("", |entry| entry.as_str());
        if get(1).parse::<usize>().ok() != Some(index + 1) {
            return Err(Error::new(
                "Error",
                "Plan draft numbering or fields are invalid",
            ));
        }
        let files = if trim(get(6))
            == "- Human must name allowed files before this draft becomes an executable TASK."
        {
            Vec::new()
        } else {
            bullets(get(6))?
        };
        issues.push(Value::object(vec![
            ("title", value(get(2))),
            (
                "wave",
                Value::Number(get(3).parse::<u8>().map_err(|_| invalid())? as f64),
            ),
            ("outcome", value(get(4))),
            ("acceptance_checks", strings(&bullets(get(5))?)),
            ("files_allowed", strings(&files)),
        ]));
    }
    let ask = trim(children.1);
    let files = intake::requirements(&value(ask), true)?.files;
    let proposed = matches(
        r"(?m)^Scope: planner-proposed$",
        source.split("\n## Outcomes\n").next().unwrap_or(""),
    )?;
    let plan = validate(
        &Value::object(vec![
            ("outcomes", strings(&bullets(outcomes)?)),
            ("issues", Value::Array(issues)),
        ]),
        Kind::parse(kind)?,
        &files,
        None,
        proposed,
    )?;
    domain::merge(
        &Value::object(vec![
            ("kind", value(kind)),
            ("reference", value(reference)),
            ("ask", value(ask)),
        ]),
        &plan,
    )
}
