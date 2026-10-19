use std::path::Path;

use super::*;
use crate::run::RunRecord;
use crate::state::redaction;

fn field_text(input: &Value, name: &str, fallback: &str) -> Result<String, Error> {
    input
        .get(name)
        .filter(|value| **value != Value::Null)
        .map(|value| domain::string(value).and_then(|value| value.to_utf8()))
        .transpose()
        .map(|value| value.unwrap_or_else(|| fallback.to_owned()))
}
fn flag(input: &Value, name: &str) -> bool {
    input.get(name) == Some(&Value::Boolean(true))
}
fn list<'a>(input: &'a Value, name: &str) -> &'a [Value] {
    input.get(name).and_then(domain::items).unwrap_or(&[])
}
fn join(input: &Value, name: &str, separator: &str) -> Result<String, Error> {
    list(input, name)
        .iter()
        .map(|value| domain::string(value).and_then(|value| value.to_utf8()))
        .collect::<Result<Vec<_>, _>>()
        .map(|values| values.join(separator))
}

fn observed_output(body: &mut Text, output: &Text) {
    let width = output
        .0
        .split(|unit| *unit != 96)
        .map(<[u16]>::len)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .max(3);
    let fence = "`".repeat(width);
    body.push_str(&format!("\n{fence}text\n"));
    body.0.extend(&output.0);
    body.push_str(&format!("\n{fence}\n"));
}

pub fn declared_test_names(source: &Text) -> Vec<Text> {
    let mut names = Vec::new();
    let units = &source.0;
    let mut index = 0;
    while index < units.len() {
        let rest = units.get(index..).unwrap_or(&[]);
        let token = if rest.starts_with(&Text::from("test(").0) {
            5
        } else if rest.starts_with(&Text::from("it(").0) {
            3
        } else {
            index += 1;
            continue;
        };
        if index > 0
            && units
                .get(index - 1)
                .is_some_and(|unit| matches!(unit, 48..=57 | 65..=90 | 97..=122 | 95))
        {
            index += 1;
            continue;
        }
        let mut cursor = index + token;
        while units
            .get(cursor)
            .copied()
            .is_some_and(crate::text::is_space)
        {
            cursor += 1;
        }
        let Some(quote) = units
            .get(cursor)
            .copied()
            .filter(|unit| [34, 39, 96].contains(unit))
        else {
            index += 1;
            continue;
        };
        cursor += 1;
        let mut name = Text::default();
        let mut closed = false;
        while let Some(unit) = units.get(cursor).copied() {
            if unit == quote {
                closed = true;
                cursor += 1;
                break;
            }
            if unit == 92 {
                if let Some(escaped) = units.get(cursor + 1) {
                    name.0.push(*escaped);
                    cursor += 2;
                    continue;
                }
                break;
            }
            name.0.push(unit);
            cursor += 1;
        }
        if closed && !(quote == 96 && name.0.windows(2).any(|window| window == [36, 123])) {
            names.push(name);
        }
        index = cursor.max(index + 1);
    }
    names
}

pub fn test_evidence(tests: &Value, changed: &[(String, Vec<Text>)]) -> Result<Text, Error> {
    let output = ["stdout", "stderr"]
        .iter()
        .filter_map(|name| tests.get(name).and_then(Value::as_string))
        .filter(|source| !crate::text::trim(&source.0).is_empty())
        .map(Text::to_utf8)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    let output = js_trim(&output);
    let mut diagnostic = false;
    let mut lines = Vec::new();
    for line in output
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
    {
        if regex(r"^\s+---\s*$")?.is_match(line) {
            diagnostic = true;
        }
        if !diagnostic {
            lines.push(line);
        }
        if diagnostic && regex(r"^\s+\.\.\.\s*$")?.is_match(line) {
            diagnostic = false;
        }
    }
    let totals = lines
        .iter()
        .filter(|line| {
            regex(r"^ℹ (tests|suites|pass|fail|cancelled|skipped|todo) \d+$")
                .is_ok_and(|pattern| pattern.is_match(js_trim(line)))
                || regex(r"^# (tests|suites|pass|fail|cancelled|skipped|todo) \d+$")
                    .is_ok_and(|pattern| pattern.is_match(line))
        })
        .copied()
        .collect::<Vec<_>>();
    let mut changed_text = Text::default();
    for (index, (file, names)) in changed.iter().enumerate() {
        if index != 0 {
            changed_text.0.push(10);
        }
        let mut results = Vec::new();
        for line in &lines {
            let line = js_trim(line);
            let matched = if let Some(capture) =
                regex(r"(?i)^(?:not )?ok \d+ - (.+?)(?: # (?:SKIP|TODO)\b.*)?$")?.captures(line)
            {
                capture
                    .get(1)
                    .is_some_and(|name| names.contains(&Text::from(name.as_str())))
            } else if line.starts_with(['✔', '✖', '﹣']) {
                let body = line.chars().skip(2).collect::<String>();
                names.iter().any(|name| {
                    name.to_utf8()
                        .is_ok_and(|name| body == name || body.starts_with(&format!("{name} (")))
                })
            } else {
                false
            };
            if matched {
                results.push(line);
            }
        }
        changed_text.push_str(&format!(
            "{file}: {} of {} declared tests reported\n",
            results.len(),
            names.len()
        ));
        let mut unique = Vec::new();
        for result in results {
            if !unique.contains(&result) {
                unique.push(result);
            }
        }
        for (index, result) in unique.into_iter().take(40).enumerate() {
            if index != 0 {
                changed_text.0.push(10);
            }
            changed_text.push_str("  ");
            changed_text.push_str(result);
        }
    }
    changed_text.0.truncate(4000);
    let exit = field_text(tests, "exit_code", "undefined")?;
    let mut evidence = Text::from(
        format!("node --test exited {exit}\nCommand: node --test\nExit code: {exit}\n").as_str(),
    );
    if !totals.is_empty() {
        evidence.push_str("Totals:\n");
        for line in totals {
            evidence.push_str("  ");
            evidence.push_str(js_trim(line));
            evidence.0.push(10);
        }
    }
    if !changed_text.0.is_empty() {
        evidence.push_str("Changed test files:\n");
        evidence.0.extend(changed_text.0);
        evidence.0.push(10);
    }
    evidence.push_str("Output:\n");
    evidence.0.extend(if output.is_empty() {
        Text::from("(no output)").0
    } else {
        output.encode_utf16().take(1200).collect()
    });
    Ok(evidence)
}

fn exception_notes(result: &Value) -> Result<Vec<String>, Error> {
    let mut notes = Vec::new();
    let tests = result.get("tests").unwrap_or(&Value::Null);
    let preexisting = if !list(result, "baselineFailures").is_empty() {
        list(result, "baselineFailures")
    } else {
        list(tests, "preexisting_files")
    };
    if !preexisting.is_empty() {
        let files = preexisting
            .iter()
            .map(|value| domain::string(value).and_then(|value| value.to_utf8()))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        notes.push(format!("- Baseline test exception: {} failure(s) classified outside this change in {files}. These exceptions do not count as a clean full-suite pass. Classification alone does not establish a passing full suite or successful acceptance.", preexisting.len()));
    }
    if tests
        .get("full_suite_exit_code")
        .is_some_and(|value| *value != Value::Number(0.0))
    {
        let original = field_text(tests, "full_suite_exit_code", "undefined")?;
        let rerun = if !list(tests, "transient_files").is_empty() {
            format!("rerun passed for {}", join(tests, "transient_files", ", ")?)
        } else {
            "isolated rerun evidence is unavailable".to_owned()
        };
        notes.push(format!("- Transient test exception: full suite exited {original}; {rerun}. The original full-suite exit {original} is preserved and distinct from the passing rerun classification."));
    }
    Ok(notes)
}

fn red_green_section(value: &Value) -> Result<String, Error> {
    let status = field_text(value, "status", "undefined")?;
    if status != "checked" {
        return Ok(format!(
            "Red/green: {status}{}\n",
            value
                .get("reason")
                .map(|_| field_text(value, "reason", ""))
                .transpose()?
                .map_or(String::new(), |reason| format!(" ({reason})"))
        ));
    }
    let mut result = "| Test file | Test | Base | Candidate |\n|---|---|---|---|\n".to_owned();
    for row in list(value, "tests") {
        result.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            field_text(row, "file", "undefined")?.replace('|', "\\|"),
            field_text(row, "name", "undefined")?.replace('|', "\\|"),
            field_text(row, "base", "undefined")?,
            field_text(row, "candidate", "undefined")?
        ));
    }
    Ok(result)
}

pub fn render(
    root: &Path,
    result: &Value,
    excellence: &Value,
    run: Option<&RunRecord>,
    redaction: &redaction::Config,
) -> Result<Text, Error> {
    let timed_out = flag(result, "timedOut");
    let blocked = flag(result, "blocked");
    let cancelled = flag(result, "cancelled");
    let notes = exception_notes(result)?;
    let passed = flag(excellence, "pass") && !timed_out && !blocked && !cancelled;
    let checks = if blocked {
        "BLOCKED"
    } else if !passed {
        "FAIL"
    } else if notes.is_empty() {
        "PASS"
    } else {
        "PASS (with test exceptions)"
    };
    let summary = if timed_out {
        Text::from(
            "Coder HTTP request timed out. No change was verified; this run did not complete.",
        )
    } else {
        result
            .get("summary")
            .and_then(Value::as_string)
            .cloned()
            .unwrap_or_else(|| Text::from("undefined"))
    };
    let tests = result.get("tests");
    let mut test_text = if let Some(tests) = tests {
        if flag(tests, "skipped") {
            Text::from(
                format!(
                    "Tests skipped: {}",
                    field_text(tests, "stdout", "undefined")?
                )
                .as_str(),
            )
        } else if flag(tests, "native") {
            let mut body = Text::from(
                format!(
                    "Native verification exited {}\nObserved Cargo: {}\nCommands:\n",
                    field_text(tests, "exit_code", "undefined")?,
                    field_text(tests, "cargo_version", "unavailable")?
                )
                .as_str(),
            );
            for command in list(tests, "commands") {
                body.push_str("  ");
                body.0.extend(required_string(command)?.0);
                body.0.push(10);
            }
            body.push_str("Output:\n");
            for name in ["stdout", "stderr"] {
                if let Some(source) = tests.get(name).and_then(Value::as_string) {
                    body.0.extend(source.0.iter().take(1200));
                    body.0.push(10);
                }
            }
            body
        } else {
            let mut changed = Vec::new();
            for file in list(excellence, "files") {
                let file = text(file, "Changed test file must be text")?;
                if !substance::is_test_file(&file)? {
                    continue;
                }
                if let Some(source) = evidence::read_product(root, &file, MAX_FILE_BYTES)? {
                    let names = declared_test_names(&source);
                    if !names.is_empty() {
                        changed.push((file, names));
                    }
                }
            }
            test_evidence(tests, &changed)?
        }
    } else if flag(result, "testsSkipped") {
        Text::from("Tests skipped: docs-only change is checked by reading the file.")
    } else {
        Text::from("Tests were not run.")
    };
    if let Some(tests) = tests.filter(|tests| {
        tests
            .get("full_suite_exit_code")
            .is_some_and(|value| *value != Value::Number(0.0))
    }) {
        test_text.push_str(&format!("\nFull suite: exited {} before isolated rerun classification; reported test result exit {} is separate from that original exit.", field_text(tests, "full_suite_exit_code", "undefined")?, field_text(tests, "exit_code", "undefined")?));
    }
    let mut body = Text::from("# Result\n\n");
    if blocked {
        body.push_str("Outcome: blocked (contracts infrastructure)\n\n");
    } else if timed_out {
        body.push_str("Outcome: timed out (unverified)\n\n");
    } else if cancelled {
        body.push_str("Outcome: cancelled (unverified)\n\n");
    }
    body.push_str(&format!("## Verification\n\nChecks: {checks}\n"));
    if passed {
        body.push_str(if notes.is_empty() {
            "- Operational checks passed.\n"
        } else {
            "- Operational gate passed with test exceptions; full-suite success is not claimed.\n"
        });
    } else {
        for (index, reason) in list(excellence, "reasons").iter().enumerate() {
            if index != 0 {
                body.0.push(10);
            }
            body.push_str(if index == 0 {
                "- First failure: "
            } else {
                "- "
            });
            body.0.extend(required_string(reason)?.0);
        }
        body.0.push(10);
    }
    if !notes.is_empty() {
        body.push_str(&notes.join("\n"));
        body.0.push(10);
    }
    body.push_str("- ");
    body.0.extend(test_text.0);
    body.push_str("\n\n");
    if !list(result, "checklist").is_empty() {
        body.push_str("## Checklist\n\n");
        body.0.extend(
            checklist::table(&Value::object(vec![(
                "items",
                Value::Array(list(result, "checklist").to_vec()),
            )]))?
            .0,
        );
        body.0.push(10);
    }
    if let Some(value) = result.get("redGreen") {
        body.push_str("## Red/green\n\n");
        body.push_str(&red_green_section(value)?);
        body.0.push(10);
    }
    if let Some(value) = result.get("shadow") {
        body.push_str("## Shadow modules\n\n");
        body.0.extend(shadow::section(value)?.0);
        body.0.push(10);
    }
    if let Some(hooks) = result
        .get("hooks")
        .filter(|hooks| !list(hooks, "entries").is_empty())
    {
        body.push_str(&format!(
            "## Lifecycle hooks\n\nStatus: {}\n\n",
            if flag(hooks, "pass") { "pass" } else { "fail" }
        ));
        for (index, hook) in list(hooks, "entries").iter().enumerate() {
            if index != 0 {
                body.0.push(10);
            }
            body.push_str(&format!(
                "- {}: {}",
                field_text(hook, "script", "undefined")?,
                field_text(hook, "status", "undefined")?
            ));
        }
        body.push_str("\n\n");
    }
    if let Some(independent) = result.get("independent") {
        body.push_str(&format!(
            "## Independent verification\n\nStatus: {}\n",
            field_text(independent, "status", "unavailable")?
        ));
        for reason in list(independent, "reasons") {
            body.push_str("- ");
            body.0.extend(required_string(reason)?.0);
            body.0.push(10);
        }
        body.0.push(10);
    }
    if let Some(full) = result.get("fullVerification") {
        body.push_str("## Full verification obligation\n\n");
        body.push_str(&format!(
            "Original full-suite exit: {}\nLast failed full-suite exit: {}\nLatest actual full-suite exit: {}\nResolution: {}\n",
            field_text(full, "original_exit", "unavailable")?,
            field_text(full, "last_failed_exit", "unavailable")?,
            field_text(full, "latest_full_exit", "unavailable")?,
            field_text(full, "resolution", "unresolved")?,
        ));
        if flag(full, "clean_full_suite") {
            body.push_str("Current actual full verification passed after the earlier failure.\n\n");
        } else {
            body.push_str("Full-suite success is not claimed; a scoped pass does not erase the failed full run.\n\n");
        }
    }
    if let Some(classification) = result.get("classification") {
        body.push_str("## Observed baseline classification\n\n");
        body.push_str(&format!(
            "Original full-suite exit: {}\nThis classification is not a clean full-suite pass.\n",
            field_text(classification, "original_exit", "unavailable")?
        ));
        for key in [
            "regression_files",
            "preexisting_files",
            "transient_files",
            "unclassified_files",
        ] {
            body.push_str(&format!("- {key}: {}\n", join(classification, key, ", ")?));
        }
        body.0.push(10);
    }
    if let Some(end_to_end) = result
        .get("endToEnd")
        .filter(|report| !list(report, "runs").is_empty())
    {
        body.push_str("## Harness read-only native CLI runs\n\n");
        body.push_str(&format!(
            "Status: {}\n",
            field_text(end_to_end, "status", "unavailable")?
        ));
        for run in list(end_to_end, "runs") {
            body.push_str(&format!(
                "\n### `{}` (check {})\n\nExit {}\n",
                field_text(run, "command", "unavailable")?,
                field_text(run, "check", "unavailable")?,
                field_text(run, "exitCode", "unavailable")?
            ));
            observed_output(&mut body, &required_string(required(run, "output")?)?);
            if let Some(error_case) = run.get("errorCase") {
                body.push_str(&format!(
                    "\nInvalid-option control exit {}\n",
                    field_text(error_case, "exitCode", "unavailable")?
                ));
                observed_output(
                    &mut body,
                    &required_string(required(error_case, "output")?)?,
                );
            }
            body.push_str("\n");
        }
    }
    if let Some(value) = result.get("selfReview") {
        body.push_str("## Self-review\n\n");
        body.0.extend(self_review::section(value)?.0);
        body.0.push(10);
    }
    let model = run.map_or_else(
        || field_text(result, "model", "undefined"),
        |run| Ok(run.metrics().model.clone()),
    )?;
    body.push_str(&format!(
        "## Run\n\nModel: {model}\nTool-loop turns: {}\nResearch turns: {}\n",
        field_text(result, "turns", "undefined")?,
        result
            .get("research")
            .map(|research| field_text(research, "turns", "0"))
            .transpose()?
            .unwrap_or_else(|| "0".to_owned())
    ));
    if result.get("testRepairs").is_some() {
        body.push_str(&format!(
            "Test repairs: {} of {}\nAdditional failing-test scope: {}\n",
            field_text(result, "testRepairs", "")?,
            field_text(result, "testRepairBudget", "4")?,
            if list(result, "repairFiles").is_empty() {
                "(none)".to_owned()
            } else {
                join(result, "repairFiles", ", ")?
            }
        ));
    }
    if !list(result, "scopeFiles").is_empty() {
        body.push_str(&format!(
            "Files outside planned scope: {}\n",
            join(result, "scopeFiles", ", ")?
        ));
    }
    if flag(result, "noProgressRepairUsed") {
        body.push_str("No-progress corrections: 1 of 1\n");
    }
    if let Some(path) = result.get("implementationPath") {
        body.push_str("Implementation path: ");
        body.0.extend(required_string(path)?.0);
        body.0.push(10);
    }
    if result.get("stages").is_some() {
        body.push_str(&format!(
            "Stages: {} -> result\n",
            join(result, "stages", " -> ")?
        ));
    }
    if let Some(handoff) = result.get("handoff").filter(|value| **value != Value::Null) {
        body.push_str(&format!(
            "Host handoff required: {}\n",
            required_string(handoff)?.to_utf8()?
        ));
        if !list(result, "scopeBlocked").is_empty() {
            body.push_str(&format!(
                "Scope-blocked paths: {}\n",
                join(result, "scopeBlocked", ", ")?
            ));
        }
    }
    if let Some(error) = result.get("error") {
        body.push_str(&format!(
            "Retained failure: {} [{}]: {}\n",
            field_text(error, "name", "Error")?,
            field_text(error, "code", "unclassified")?,
            field_text(error, "message", "unavailable")?
        ));
    }
    if let Some(run) = run {
        body.push_str(&format!("AI-Run: {}\n", run.line()));
    } else if domain::is_text(result.get("mode"), "stub") {
        body.push_str("AI-Run: not emitted for a deterministic stub.\n");
    } else {
        body.push_str("AI-Run: unavailable; metadata validation failed.\n");
    }
    body.push_str(if flag(result, "unverifiedObservation") {
        "\n## Observed write/delete paths (not a verified diff)\n\n"
    } else {
        "\n## Files changed\n\n"
    });
    if list(excellence, "files").is_empty() {
        body.push_str(if flag(result, "unverifiedObservation") {
            "(no completed product writes were observed)"
        } else {
            "(none)"
        });
    } else {
        for (index, file) in list(excellence, "files").iter().enumerate() {
            if index != 0 {
                body.0.push(10);
            }
            body.push_str("- ");
            body.0.extend(required_string(file)?.0);
        }
    }
    body.push_str(&format!(
        "\n\n## {}\n\n",
        if passed {
            "Summary"
        } else {
            "Unverified summary"
        }
    ));
    body.0.extend(summary.0);
    body.0.push(10);
    Ok(redaction::evidence(&body, redaction))
}

fn required_string(value: &Value) -> Result<Text, Error> {
    value
        .as_string()
        .cloned()
        .ok_or_else(|| Error::type_error("Result fields must be text"))
}

#[cfg(test)]
mod backend_output_tests {
    use super::*;

    #[test]
    fn actual_cli_output_cannot_close_its_untrusted_result_fence() {
        let mut body = Text::default();
        observed_output(&mut body, &Text::from("output\n```\n## Not a gate\n"));
        assert_eq!(
            body,
            Text::from("\n````text\noutput\n```\n## Not a gate\n\n````\n")
        );
    }
}
