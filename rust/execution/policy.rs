use super::*;
use crate::planning;

pub const NATIVE_TEST_REQUIRED: &str = "Rust changes require native verification; Node consumer tests cannot verify them. Configure an approved Cargo image for formatting, strict all-target Clippy and locked/offline native tests.";
pub const FULL_TEST_TIMEOUT_MS: u64 = 900_000;

pub fn managed(file: &str) -> bool {
    let normalized = file.replace('\\', "/").to_lowercase();
    let parts = normalized.split('/').collect::<Vec<_>>();
    parts.len() == 1
        && [
            "assignment.md",
            "task.md",
            "recipe.yml",
            "plan.md",
            "context.md",
            "research.md",
            "result.md",
            "review.md",
            "estimate.md",
        ]
        .contains(&normalized.as_str())
        || parts.first() == Some(&".roster")
            && (matches!(parts.get(1), Some(&"logs" | &"history" | &"checkpoints"))
                || parts
                    .get(1)
                    .is_some_and(|part| part.starts_with("history."))
                || normalized == ".roster/map.md"
                || parts.len() == 3
                    && parts.get(1) == Some(&"runs")
                    && parts.last().is_some_and(|part| part.ends_with(".log")))
}

pub fn exact_path(file: &str) -> bool {
    !file.is_empty()
        && file.encode_utf16().count() <= 500
        && !file.starts_with(['/', '\\'])
        && !file.contains(['\\', '*', '?', '\0', '\r', '\n', ':'])
        && file
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

pub fn scope_expansion_file(file: &str) -> bool {
    let lower = file.to_lowercase();
    exact_path(file)
        && !planning::forbidden_write(file)
        && !managed(file)
        && !lower.split('/').any(|part| part == "vendor")
        && !matches!(
            lower.split('/').next(),
            Some(".roster" | ".roster-state" | ".github")
        )
}

pub fn repair_test_file(file: &str) -> Result<bool, Error> {
    Ok(exact_path(file)
        && !planning::forbidden_write(file)
        && regex(r"(?:^|/)(?:test(?:[._-][^/]+)?|[^/]+[._-]test)\.[cm]?js$")?.is_match(file))
}

pub fn task_and_repair_files(
    allowed: &[String],
    repair: &[String],
    expanded: &[String],
) -> Result<Vec<String>, Error> {
    if repair
        .iter()
        .any(|file| !repair_test_file(file).unwrap_or(false))
    {
        return Err(Error::type_error(
            "Repair scope must contain only worktree-relative, unprotected test files",
        ));
    }
    if expanded.iter().any(|file| !scope_expansion_file(file)) {
        return Err(Error::type_error(
            "Scope expansion must contain only worktree-relative, unprotected product files",
        ));
    }
    let mut result = Vec::new();
    for file in allowed.iter().chain(repair).chain(expanded) {
        if !result.contains(file) {
            result.push(file.clone());
        }
    }
    Ok(result)
}

pub fn readme_only(files: &[String]) -> bool {
    files == ["README.md"]
}

pub fn docs_only(files: &[String]) -> Result<bool, Error> {
    let pattern = regex(r"^(README|docs/[^/]+)\.md$")?;
    Ok(!files.is_empty()
        && files
            .iter()
            .all(|file| pattern.is_match(&file.replace('\\', "/"))))
}

pub fn requires_native(files: &[String]) -> Result<bool, Error> {
    let pattern = regex(r"(?i)(?:\.rs$|(?:^|/)Cargo\.(?:toml|lock)$|^rust/)")?;
    Ok(files
        .iter()
        .any(|file| pattern.is_match(&file.replace('\\', "/"))))
}

pub fn relevant_tests(files: &[String]) -> Result<Vec<String>, Error> {
    if docs_only(files)? {
        return Ok(Vec::new());
    }
    let pattern = regex(r"(?:^|/)(?:test(?:[._-][^/]+)?|[^/]+[._-]test)\.[cm]?js$")?;
    let extension = regex(r"\.[cm]?js$")?;
    let mut tests = Vec::new();
    for file in files {
        let file = file.replace('\\', "/");
        if pattern.is_match(&file) && !tests.contains(&file) {
            tests.push(file.clone());
        }
        let base = file.rsplit('/').next().unwrap_or("");
        let base = extension.replace(base, "");
        let candidate = format!("tests/{base}.test.mjs");
        if !base.is_empty() && file.starts_with("src/") && !tests.contains(&candidate) {
            tests.push(candidate);
        }
    }
    Ok(tests)
}

pub fn shard_base(file: &str) -> Result<Option<String>, Error> {
    Ok(regex(r"^tests/([^/.]+)\.[^/]+\.test\.mjs$")?
        .captures(&file.replace('\\', "/"))
        .and_then(|captures| {
            captures
                .get(1)
                .map(|entry| format!("tests/{}.test.mjs", entry.as_str()))
        }))
}

pub fn covers_test(tests: &[String], file: &str) -> Result<bool, Error> {
    Ok(tests.contains(&file.replace('\\', "/"))
        || shard_base(file)?.is_some_and(|base| tests.contains(&base)))
}

pub fn expand_shards(
    files: &[String],
    available: &[String],
    planned: &[String],
) -> Result<Vec<String>, Error> {
    let mut result = Vec::new();
    for file in files {
        let selected_shard = planned.iter().try_fold(false, |found, selected| {
            Ok::<_, Error>(
                found
                    || shard_base(selected)?.as_ref() == Some(file) && available.contains(selected),
            )
        })?;
        if !planned.contains(file) && selected_shard {
            continue;
        }
        if available.contains(file) && !result.contains(file) {
            result.push(file.clone());
        }
        for candidate in available {
            if shard_base(candidate)?.as_ref() == Some(file) && !result.contains(candidate) {
                result.push(candidate.clone());
            }
        }
    }
    Ok(result)
}

pub fn test_concurrency(available: u32) -> u32 {
    (available / 2).max(1)
}

pub fn verification_decision(files: &[String]) -> Result<Value, Error> {
    let docs = docs_only(files)?;
    let update = relevant_tests(files)?
        .into_iter()
        .filter(|file| files.contains(file) || file.starts_with("tests/"))
        .collect::<Vec<_>>();
    Ok(Value::object(vec![
        ("run", Value::Boolean(!docs && !update.is_empty())),
        ("update", strings(&update)),
        (
            "reason",
            Value::string(if docs {
                "docs-only change is checked by reading the file"
            } else if update.is_empty() {
                "no relevant test covers the changed files"
            } else {
                "run and update only the tests that cover the changed files"
            }),
        ),
    ]))
}

pub fn test_command(files: &[String], available: u32) -> Result<Value, Error> {
    let tests = relevant_tests(files)?;
    let docs = docs_only(files)?;
    if docs || tests.is_empty() {
        return Ok(Value::object(vec![
            ("skip", Value::Boolean(true)),
            ("args", Value::Array(Vec::new())),
            ("timeoutMs", Value::Number(0.0)),
            (
                "label",
                Value::string(if docs {
                    "docs-only: tests skipped"
                } else {
                    "no relevant tests for the changed files"
                }),
            ),
        ]));
    }
    let mut args = vec![
        "--test".to_owned(),
        "--test-concurrency".to_owned(),
        test_concurrency(available).to_string(),
    ];
    args.extend(tests);
    Ok(Value::object(vec![
        ("args", strings(&args)),
        ("timeoutMs", Value::Number(FULL_TEST_TIMEOUT_MS as f64)),
        ("label", Value::string(&format!("node {}", args.join(" ")))),
    ]))
}

pub fn tests_mode(task: &Text) -> Result<String, Error> {
    let (frontmatter, _) = planning::skills::split_frontmatter(&task.to_utf8()?)?;
    let lines = frontmatter
        .split('\n')
        .filter(|line| line.starts_with("tests:"))
        .collect::<Vec<_>>();
    if lines.len() > 1 {
        return Err(Error::new("Error", "TASK.md must not repeat tests"));
    }
    let Some(line) = lines.first() else {
        return Ok("required".to_owned());
    };
    regex(r"^tests: (none|required|characterization)[ \t]*$")?
        .captures(line)
        .and_then(|captures| captures.get(1).map(|entry| entry.as_str().to_owned()))
        .ok_or_else(|| {
            Error::new(
                "Error",
                "TASK.md tests must be none, required, or characterization",
            )
        })
}

pub fn status_section_present(text: &str) -> Result<bool, Error> {
    let mut fenced = false;
    let mut section = None;
    let lines = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>();
    let fence = regex(r"^ {0,3}(?:`{3,}|~{3,})")?;
    let status = regex(r"^## Status[ \t]*$")?;
    for (index, line) in lines.iter().enumerate() {
        if fence.is_match(line) {
            fenced = !fenced;
        }
        if !fenced && status.is_match(line) {
            section = Some(index);
        }
    }
    let Some(start) = section else {
        return Ok(false);
    };
    let heading = regex(r"^#{1,2}\s+")?;
    Ok(lines
        .iter()
        .skip(start + 1)
        .take_while(|line| !heading.is_match(line))
        .any(|line| !js_trim(line).is_empty()))
}

pub fn whitespace_edit(source: &str, old: &str, new: &str) -> Result<Value, Error> {
    let lines = source.split('\n').collect::<Vec<_>>();
    let mut needle = old.split('\n').collect::<Vec<_>>();
    while needle.last().is_some_and(|line| js_trim(line).is_empty()) {
        needle.pop();
    }
    while needle.first().is_some_and(|line| js_trim(line).is_empty()) {
        needle.remove(0);
    }
    let starts = if needle.is_empty() {
        Vec::new()
    } else {
        lines
            .windows(needle.len())
            .enumerate()
            .filter_map(|(start, window)| {
                window
                    .iter()
                    .zip(&needle)
                    .all(|(line, old)| js_trim(line) == js_trim(old))
                    .then_some(start)
            })
            .collect::<Vec<_>>()
    };
    let mut value = Value::object(vec![("count", Value::Number(starts.len() as f64))]);
    if let [start] = starts.as_slice() {
        let leading = |line: &str| {
            line.chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect::<String>()
        };
        let from = leading(needle.first().copied().unwrap_or(""));
        let to = leading(lines.get(*start).copied().unwrap_or(""));
        let replacement = new
            .trim_end_matches('\n')
            .split('\n')
            .map(|line| {
                line.strip_prefix(&from)
                    .map_or_else(|| line.to_owned(), |line| format!("{to}{line}"))
            })
            .collect::<Vec<_>>();
        let mut result = lines
            .iter()
            .take(*start)
            .map(|line| (*line).to_owned())
            .collect::<Vec<_>>();
        result.extend(replacement);
        result.extend(
            lines
                .iter()
                .skip(start + needle.len())
                .map(|line| (*line).to_owned()),
        );
        domain::put(&mut value, "content", Value::string(&result.join("\n")))?;
    }
    Ok(value)
}

pub fn parse_hooks(source: &Text) -> Result<Value, Error> {
    let hooks = domain::catalog_values(crate::catalog::parse(
        source,
        "hooks",
        &["event", "script", "timeout_ms"],
    )?);
    if hooks.len() > 16 {
        return Err(Error::type_error(
            "At most 16 lifecycle hooks may be configured",
        ));
    }
    let script = regex(r"^\.roster/hooks/[A-Za-z0-9_-]+\.[cm]?js$")?;
    let mut result = Vec::new();
    for mut hook in hooks {
        if !["pre-plan", "post-coder", "pre-publish"]
            .iter()
            .any(|event| domain::is_text(hook.get("event"), event))
        {
            return Err(Error::type_error(
                "Hook event must be pre-plan, post-coder, or pre-publish",
            ));
        }
        if hook
            .get("script")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .is_none_or(|source| !script.is_match(&source))
        {
            return Err(Error::type_error(
                "Hook script must be a Node script directly under .roster/hooks/",
            ));
        }
        let timeout = if hook.get("timeout_ms").is_some() {
            domain::safe_integer(hook.get("timeout_ms"))
        } else {
            Some(10_000)
        }
        .filter(|timeout| (1..=60_000).contains(timeout))
        .ok_or_else(|| Error::type_error("Hook timeout_ms must be an integer from 1 to 60000"))?;
        domain::put(&mut hook, "timeout_ms", Value::Number(timeout as f64))?;
        result.push(hook);
    }
    Ok(Value::Array(result))
}

pub fn hook_environment(environment: &Value) -> Result<Value, Error> {
    let mut output = Value::object(Vec::new());
    for (name, value) in domain::members(environment)
        .ok_or_else(|| Error::type_error("Hook environment must be an object"))?
    {
        let name = name.to_utf8()?;
        if [
            "path",
            "pathext",
            "systemroot",
            "windir",
            "comspec",
            "temp",
            "tmp",
            "lang",
            "lc_all",
            "tz",
        ]
        .contains(&name.to_ascii_lowercase().as_str())
            && value.as_string().is_some()
        {
            domain::put(&mut output, &name, value.clone())?;
        }
    }
    Ok(output)
}
