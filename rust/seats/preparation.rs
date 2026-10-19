use std::collections::BTreeMap;

use super::*;
use crate::execution::{backend, policy};
use crate::planning::grounding;

pub(super) struct Prepared {
    pub triage: Value,
    pub research: Text,
    pub all_met: bool,
}

fn invocation_text(source: &str) -> Result<String, Error> {
    let quoted = execution::regex(r#"`[^`\r\n]*`|"[^"\r\n]*"|'[^'\r\n]*'"#)?;
    let mut result = String::new();
    let mut fenced = false;
    for line in source.lines() {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            result.push('\n');
            continue;
        }
        if fenced {
            result.push('\n');
            continue;
        }
        let mut end = 0;
        for span in quoted.find_iter(line) {
            if span.as_str().starts_with('\'')
                && line
                    .get(..span.start())
                    .and_then(|prefix| prefix.chars().last())
                    .is_some_and(char::is_alphanumeric)
            {
                continue;
            }
            result.push_str(
                line.get(end..span.start())
                    .ok_or_else(|| Error::type_error("Invocation text range is invalid"))?,
            );
            let body = span
                .as_str()
                .get(1..span.len().saturating_sub(1))
                .ok_or_else(|| Error::type_error("Invocation quote is invalid"))?
                .trim();
            if ["web_search", "web_fetch"].contains(&body) {
                result.push_str(body);
            } else {
                result.extend(std::iter::repeat_n(' ', span.len()));
            }
            end = span.end();
        }
        result.push_str(
            line.get(end..)
                .ok_or_else(|| Error::type_error("Invocation text suffix is invalid"))?,
        );
        result.push('\n');
    }
    Ok(result)
}

pub(super) fn selected_web_prerequisites(source: &str) -> Result<Vec<&'static str>, Error> {
    let source = invocation_text(source)?;
    let modifier = r"(?:[a-z]+ly|first|second|third|next|then|also|now|once)";
    let invocation = execution::regex(&format!(
        r"(?i)\b(?:(do\s+not|does\s+not|don't|must\s+not|never|avoid|without)(?:\s+{modifier})*\s+)?(?:use|using|invoke|invoking|run|running|call|calling|requires?)\s+(?:the\s+)?(web_(?:search|fetch)(?:\s*(?:,|and|or|/)\s*web_(?:search|fetch))*)\b",
    ))?;
    let documentation = execution::regex(
        r"(?i)^\s*(?:#{1,6}\s*)?(?:Task:\s*)?(?:(?:[-*]|\d+[.)])\s*)?(?:document|describe|explain|mention|quote|list|show|write\s+(?:(?:a|the)\s+)?(?:guide|documentation|docs))\b",
    )?;
    let coordination = execution::regex(r"(?i)\b(?:and|but|then)\b")?;
    let modifiers = execution::regex(&format!(r"(?i)^[ \t]*(?:{modifier}[ \t]+)*$"))?;
    let description = execution::regex(r"(?i)\b(?:how|whether|when|where)\s+to\b")?;
    let tool = execution::regex(r"(?i)\bweb_(?:search|fetch)\b")?;
    let mut selected = Vec::new();
    for line in source.lines() {
        for captures in invocation.captures_iter(line) {
            let full = captures
                .get(0)
                .ok_or_else(|| Error::type_error("Web invocation match is missing"))?;
            let prefix = line
                .get(..full.start())
                .ok_or_else(|| Error::type_error("Web invocation prefix is invalid"))?;
            let clause = prefix.rsplit([';', '.', '!', '?']).next().unwrap_or("");
            let independent_invocation = coordination.find_iter(clause).any(|boundary| {
                let prior = clause.get(..boundary.start()).unwrap_or("");
                (!description.is_match(prior) || prior.contains(','))
                    && clause
                        .get(boundary.end()..)
                        .is_some_and(|suffix| modifiers.is_match(suffix))
            });
            if captures.get(1).is_some()
                || documentation.is_match(clause) && !independent_invocation
            {
                continue;
            }
            let names = captures
                .get(2)
                .ok_or_else(|| Error::type_error("Web invocation tools are missing"))?;
            for found in tool.find_iter(names.as_str()) {
                let name = if found.as_str().eq_ignore_ascii_case("web_search") {
                    "web_search"
                } else {
                    "web_fetch"
                };
                if !selected.contains(&name) {
                    selected.push(name);
                }
            }
        }
    }
    Ok(selected)
}

pub(super) fn prerequisites(
    boundary: &tools::Boundary,
    config: &crate::config::Config,
    approval: Option<&backend::Approval>,
) -> Result<(), Error> {
    boundary.check()?;
    if let Some(approval) = approval {
        approval.validate()?;
    }
    if boundary.role() != execution::Role::Coder {
        return Err(execution::denied(
            "The native coder needs the existing coder writer lease",
        ));
    }
    boundary.require_coder_recipe()?;
    if boundary.checks_required()?
        && (config
            .as_value()
            .get("tools")
            .and_then(|tools| tools.get("run_test"))
            == Some(&Value::Boolean(false))
            || !boundary.recipe_permits("run_test")
            || !configured_tool(config, "run_test"))
    {
        return Err(Handoff::Unavailable.error("Required task verification has no host/configuration/recipe-approved run_test capability; no product writes were made"));
    }
    if !configured_tool(config, "read_file") || !boundary.recipe_permits("read_file") {
        return Err(Handoff::Unavailable.error("The selected coder requires approved read_file for its current handoff and source observations"));
    }
    let kind = planning::intake::classify(
        &Value::String(boundary.assignment().ask.clone()),
        boundary.assignment().title.as_ref(),
        &boundary.document().files_allowed,
    )?;
    if domain::is_text(kind.get("kind"), "initiative") {
        return Err(Handoff::Rescope.error("The authoritative Original Ask is an epic/initiative and must split before the coder can write products"));
    }
    boundary.verification_prerequisite()?;
    let cap = domain::safe_integer(
        config
            .as_value()
            .get("seat")
            .and_then(|seat| seat.get("scope_expansion")),
    )
    .unwrap_or(3);
    if !boundary.scope_budget_authorized(cap)? {
        return Err(execution::denied(
            "Frozen host scope expansion exceeds the frozen configuration limit; reopen with their explicit intersection before writes",
        ));
    }
    let task = boundary.task().to_utf8()?;
    if !selected_web_prerequisites(&task)?.is_empty() {
        return Err(Handoff::Unavailable.error("Selected web research prerequisite is unavailable in the native host; neither model text nor internet configuration supplies an execution provider"));
    }
    let commands = execution::e2e::selected_commands(&boundary.document().acceptance_checks)?;
    let commands = domain::items(&commands)
        .ok_or_else(|| Error::type_error("Native acceptance selection must be an array"))?;
    if !commands.is_empty()
        && (approval.is_none_or(|approval| approval.native_cli.is_none())
            || commands
                .iter()
                .any(|command| command.get("implemented") != Some(&Value::Boolean(true))))
    {
        return Err(Handoff::Unavailable.error("Selected native read-only CLI prerequisite is missing or unimplemented; no product writes were made"));
    }
    if policy::readme_only(&boundary.document().files_allowed)
        && (!configured_tool(config, "write_file") || !boundary.recipe_permits("write_file"))
    {
        return Err(Handoff::Unavailable
            .error("README-only tasks require approved write_file before the coder starts"));
    }
    Ok(())
}

pub(super) fn configured_tool(config: &crate::config::Config, name: &str) -> bool {
    config
        .as_value()
        .get("seat")
        .and_then(|seat| seat.get("tools"))
        .and_then(domain::items)
        .is_some_and(|tools| tools.iter().any(|tool| domain::is_text(Some(tool), name)))
}

pub(super) async fn design(boundary: &tools::Boundary) -> Result<(), Error> {
    let task = Value::String(boundary.task().clone());
    let Some(design) = grounding::parse_design(&task)? else {
        return Ok(());
    };
    let changes = evidence::changes(boundary, None).await?;
    let combined = format!(
        "{}\n{}",
        boundary.document().title.to_utf8()?,
        boundary.assignment().ask.to_utf8()?
    );
    let context = grounding::read_context(boundary.root(), changes.tracked(), &combined)?;
    let checks = boundary
        .document()
        .acceptance_checks
        .iter()
        .map(Text::to_utf8)
        .collect::<Result<Vec<_>, _>>()?;
    let errors = grounding::errors(
        &checks,
        Some(&design),
        &boundary.document().files_allowed,
        &combined,
        Some(&context.index),
    )?;
    if !errors.is_empty() {
        return Err(execution::denied(format!(
            "Design grounding refused before any product write: {}",
            errors.join("; ")
        )));
    }
    boundary.check()
}

pub(super) async fn run(boundary: &mut tools::Boundary, research: bool) -> Result<Prepared, Error> {
    let mut inventory = Text::from(
        "# Research\n\nRead-only current inventory; no implementation or acceptance is claimed.\n\n",
    );
    let mut files = boundary.scope()?;
    if files.iter().any(|file| !policy::exact_path(file)) {
        let snapshot = boundary.snapshot()?;
        files = snapshot
            .files
            .keys()
            .filter(|file| planning::is_allowed(file, &files))
            .cloned()
            .collect();
    }
    let mut sources = BTreeMap::new();
    if research {
        for file in files.iter().take(8) {
            let result = boundary
                .execute(
                    "read_file",
                    &Value::object(vec![
                        ("path", Value::string(file)),
                        ("max_lines", Value::Number(200.0)),
                    ]),
                )
                .await;
            inventory.push_str(&format!("### {file}\n\n"));
            match result {
                Ok(value) => {
                    let source = text_value(&value, "Research read_file must return text")?;
                    let mut excerpt = source.clone();
                    if excerpt.0.len() > 8000 {
                        excerpt.0.truncate(7970);
                        excerpt.push_str("\n[Excerpt truncated]\n");
                    }
                    inventory.0.extend(excerpt.0);
                    inventory.push_str("\n\n");
                    sources.insert(file.clone(), source);
                }
                Err(error) if error.code == Some("ENOENT") => {
                    inventory.push_str("(File not yet present.)\n\n")
                }
                Err(error) => return Err(error),
            }
        }
        if files.len() > 8 {
            inventory.push_str(
                "Additional files were not inspected: eight-file research budget reached.\n",
            );
        }
    }
    let mut entries = Vec::new();
    let mut tests = None;
    let command = execution::regex(
        r"^`(?:node --test(?:\s+[^`\s]+\.test\.[cm]?js)+|cargo (?:test|check|clippy)(?:[^`]*))`\s+(?:exits|exit with|returns)\s+(?:code\s+)?0\.?$",
    )?;
    let exports = execution::regex(
        r"`([A-Za-z_$][\w$]*)`\s+(?:is exported\s+)?(?:from|in)\s+`([^`]+\.[cm]?js)`",
    )?;
    for check in &boundary.document().acceptance_checks.clone() {
        let check_text = check.to_utf8()?;
        let mut met = None;
        let mut observations = Vec::new();
        if !policy::docs_only(&boundary.scope()?)?
            && command.is_match(&check_text)
            && boundary.checks_required()?
        {
            if tests.is_none() {
                tests = Some(boundary.verify(false).await?);
            }
            let observed = tests
                .as_ref()
                .ok_or_else(|| Error::type_error("Triage observation disappeared"))?;
            let named = check_text.split('`').nth(1).unwrap_or("");
            let matches = boundary.last_tests().is_some_and(|proof| {
                proof
                    .observed_commands()
                    .iter()
                    .any(|actual| actual == named)
            });
            if observed.get("exit_code") != Some(&Value::Number(0.0)) {
                met = Some(false);
            } else if matches && observed.get("skipped") != Some(&Value::Boolean(true)) {
                met = Some(true);
            }
            observations.push(Value::string("Actual approved matching-scope verification was observed before coding; it does not prove unexecuted behaviors."));
        }
        for captures in exports.captures_iter(&check_text) {
            let name = captures.get(1).map_or("", |capture| capture.as_str());
            let file = captures.get(2).map_or("", |capture| capture.as_str());
            if !policy::exact_path(file) || !planning::is_allowed(file, &boundary.scope()?) {
                continue;
            }
            let source = if let Some(source) = sources.get(file) {
                Some(source.clone())
            } else {
                match boundary
                    .execute(
                        "read_file",
                        &Value::object(vec![("path", Value::string(file))]),
                    )
                    .await
                {
                    Ok(source) => Some(text_value(&source, "Triage source must be text")?),
                    Err(error) if error.code == Some("ENOENT") => None,
                    Err(error) => return Err(error),
                }
            };
            let index = grounding::index_sources(
                &[file.to_owned()],
                &source
                    .into_iter()
                    .map(|source| (file.to_owned(), source))
                    .collect::<Vec<_>>(),
            )?;
            let exists = index
                .exports_by_file
                .iter()
                .any(|(path, exports)| path == file && exports.iter().any(|export| export == name));
            if !exists {
                met = Some(false);
            }
            observations.push(Value::string(&format!(
                "Current source {} export {name} in {file}.",
                if exists {
                    "declares"
                } else {
                    "does not declare"
                }
            )));
        }
        entries.push(Value::object(vec![
            ("check", Value::String(check.clone())),
            (
                "status",
                Value::string(match met {
                    Some(true) => "met",
                    Some(false) => "unmet",
                    None => "unknown",
                }),
            ),
            ("observations", Value::Array(observations)),
        ]));
    }
    let all_met = !entries.is_empty()
        && entries
            .iter()
            .all(|entry| domain::is_text(entry.get("status"), "met"));
    let triage = Value::object(vec![
        ("allMet", Value::Boolean(all_met)),
        ("entries", Value::Array(entries)),
    ]);
    boundary.check()?;
    Ok(Prepared {
        triage,
        research: inventory,
        all_met,
    })
}

pub(super) fn readme_status(boundary: &tools::Boundary) -> Result<bool, Error> {
    if !policy::readme_only(&boundary.document().files_allowed) {
        return Ok(true);
    }
    let title_ask = format!(
        "{}\n{}",
        boundary.document().title.to_utf8()?,
        boundary.assignment().ask.to_utf8()?
    );
    if !execution::regex(r"(?i)status\s+section|section.{0,30}status|##\s*status")?
        .is_match(&title_ask)
    {
        return Ok(true);
    }
    let Some(source) =
        evidence::read_product(boundary.root(), "README.md", execution::MAX_FILE_BYTES)?
    else {
        return Ok(false);
    };
    let source = source.to_utf8()?.replace("\r\n", "\n");
    let mut fenced = false;
    let mut in_status = false;
    let mut lines = 0;
    let fence = execution::regex(r"^ {0,3}(?:`{3,}|~{3,})")?;
    let heading = execution::regex(r"^#{1,2}\s+")?;
    for line in source.split('\n') {
        if fence.is_match(line) {
            fenced = !fenced;
        }
        if !fenced && execution::regex(r"^## Status[ \t]*$")?.is_match(line) {
            in_status = true;
            lines = 0;
        } else if in_status && heading.is_match(line) {
            in_status = false;
        } else if in_status && !line.trim().is_empty() {
            lines += 1;
        }
    }
    Ok(lines == 1)
}
