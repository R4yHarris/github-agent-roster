use std::collections::{BTreeMap, BTreeSet};

use super::*;

const GENERIC: &[&str] = &[
    "all", "data", "file", "files", "value", "item", "items", "entry", "entries", "text", "result",
    "options", "config", "default", "new", "from", "by", "of", "for", "and", "or", "with", "js",
    "mjs", "index", "util", "utils", "lib", "helper", "helpers", "store", "state",
];

pub fn words(name: &str) -> Result<Vec<String>, Error> {
    let source = regex(r"([a-z0-9])([A-Z])")?.replace_all(name, "$1 $2");
    let source = regex(r"([A-Z]+)([A-Z][a-z])")?.replace_all(&source, "$1 $2");
    Ok(source
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect())
}
fn singular(word: &str) -> String {
    if word.len() > 3 && word.ends_with("ies") {
        format!("{}y", word.strip_suffix("ies").unwrap_or(word))
    } else if word.len() > 3 && word.ends_with('s') && !word.ends_with("ss") {
        word.strip_suffix('s').unwrap_or(word).to_owned()
    } else {
        word.to_owned()
    }
}
fn role(word: &str) -> Option<&'static str> {
    for (role, names) in [
        (
            "read",
            &[
                "read", "load", "get", "fetch", "list", "find", "lookup", "query", "all",
            ][..],
        ),
        (
            "write",
            &[
                "write", "save", "store", "put", "persist", "append", "insert", "add", "record",
            ],
        ),
        (
            "validate",
            &[
                "validate", "check", "verify", "assert", "ensure", "require", "is",
            ],
        ),
        (
            "digest",
            &["hash", "digest", "checksum", "sha", "fingerprint"],
        ),
        ("parse", &["parse", "decode", "deserialize"]),
        (
            "format",
            &["format", "render", "serialize", "stringify", "encode", "to"],
        ),
        ("redact", &["redact", "mask", "scrub", "sanitize"]),
    ] {
        if names.contains(&word) {
            return Some(role);
        }
    }
    None
}
#[derive(Clone)]
struct Profile {
    role: Option<&'static str>,
    nouns: BTreeSet<String>,
}
fn profile(name: &str) -> Result<Profile, Error> {
    let words = words(name)?;
    Ok(Profile {
        role: words.iter().find_map(|word| role(word)),
        nouns: words
            .iter()
            .filter(|word| role(word).is_none() || word.as_str() == "record")
            .map(|word| singular(word))
            .filter(|word| word.len() > 2 && !GENERIC.contains(&word.as_str()))
            .collect(),
    })
}
pub fn export_names(source: &Text, rust: bool) -> Result<Vec<String>, Error> {
    let source = source.to_utf8()?;
    let mut names = Vec::new();
    for capture in regex(r"(?m)^export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|class|const|let|var)\s+([A-Za-z_$][\w$]*)")?.captures_iter(&source) {
        if let Some(name) = capture.get(1) { if !names.iter().any(|old| old == name.as_str()) { names.push(name.as_str().to_owned()); } }
    }
    for capture in regex(r"(?m)^export\s*\{([^}]*)\}")?.captures_iter(&source) {
        let end = capture.get(0).map_or(0, |value| value.end());
        if source
            .get(end..)
            .is_some_and(|rest| rest.trim_start().starts_with("from"))
        {
            continue;
        }
        for part in capture.get(1).map_or("", |value| value.as_str()).split(',') {
            let parts = regex(r"\s+as\s+")?.split(part.trim()).collect::<Vec<_>>();
            let name = parts.last().copied().unwrap_or("").trim();
            if name != "default"
                && regex(r"^[A-Za-z_$][\w$]*$")?.is_match(name)
                && !names.iter().any(|old| old == name)
            {
                names.push(name.to_owned());
            }
        }
    }
    if rust {
        for capture in regex(r"(?m)^pub\s+(?:async\s+)?(?:fn|struct|enum|trait|type|const|static)\s+([A-Za-z_][A-Za-z0-9_]*)")?.captures_iter(&source) {
            if let Some(name) = capture.get(1) { if !names.iter().any(|old| old == name.as_str()) { names.push(name.as_str().to_owned()); } }
        }
    }
    Ok(names)
}

fn product(file: &str) -> Result<bool, Error> {
    Ok(
        (regex(r"\.[cm]?[jt]sx?$")?.is_match(file) || file.ends_with(".rs"))
            && !substance::is_test_file(file)?
            && !file.starts_with("tests/")
            && !file.starts_with("vendor/")
            && !crate::planning::forbidden_read(file),
    )
}
fn occurrences(source: &str, name: &str) -> usize {
    source
        .match_indices(name)
        .filter(|(start, _)| {
            (*start == 0
                || source
                    .as_bytes()
                    .get(start - 1)
                    .is_none_or(|byte| !byte.is_ascii_alphanumeric() && !b"_$".contains(byte)))
                && source
                    .as_bytes()
                    .get(start + name.len())
                    .is_none_or(|byte| !byte.is_ascii_alphanumeric() && !b"_$".contains(byte))
        })
        .count()
}

pub async fn check(
    boundary: &tools::Boundary,
    files: &[String],
    changes: &evidence::Changes,
    prior: &[String],
    task: &Text,
) -> Result<Value, Error> {
    changes.require_current(boundary)?;
    let tracked = changes.tracked();
    let changed = files
        .iter()
        .filter_map(|file| match product(file) {
            Ok(true) => Some(Ok(file.clone())),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if changed.is_empty() {
        return Ok(Value::object(vec![
            ("status", Value::string("none")),
            ("findings", Value::Array(Vec::new())),
        ]));
    }
    let mut sources = BTreeMap::new();
    let mut bases = BTreeMap::new();
    let mut added = Vec::new();
    for file in tracked.iter().chain(changed.iter()) {
        if !product(file)? || sources.contains_key(file) {
            continue;
        }
        let source = evidence::read_product(boundary.root(), file, 262_144)?;
        sources.insert(file.clone(), source);
        if changed.contains(file) {
            bases.insert(file.clone(), changes.head_product(boundary, file).await?);
        }
    }
    for file in &changed {
        let Some(Some(source)) = sources.get(file) else {
            continue;
        };
        let previous = bases.get(file).and_then(Option::as_ref);
        let old = previous
            .map(|source| export_names(source, file.ends_with(".rs")))
            .transpose()?
            .unwrap_or_default();
        for name in export_names(source, file.ends_with(".rs"))? {
            if !old.contains(&name) {
                added.push((file.clone(), name, previous.is_none()));
            }
        }
    }
    if added.is_empty() {
        return Ok(Value::object(vec![
            ("status", Value::string("checked")),
            ("findings", Value::Array(Vec::new())),
        ]));
    }
    let mut modules = Vec::new();
    for file in tracked {
        if !product(file)? {
            continue;
        }
        let source = if changed.contains(file) {
            bases.get(file).and_then(Option::as_ref)
        } else {
            sources.get(file).and_then(Option::as_ref)
        };
        let Some(source) = source else { continue };
        let names = export_names(source, file.ends_with(".rs"))?;
        if names.is_empty() {
            continue;
        }
        let basename = file.rsplit('/').next().unwrap_or("");
        let stem = regex(r"\.[^.]+$")?.replace(basename, "");
        let stem = words(&stem)?
            .into_iter()
            .map(|word| singular(&word))
            .filter(|word| word.len() > 2 && !GENERIC.contains(&word.as_str()))
            .collect::<BTreeSet<_>>();
        let profiles = names
            .iter()
            .map(|name| profile(name))
            .collect::<Result<Vec<_>, _>>()?;
        modules.push((file.clone(), names, stem, profiles));
    }
    let mut findings = Vec::new();
    let mut flagged = BTreeSet::new();
    for (file, name, new_file) in &added {
        let same = modules
            .iter()
            .find(|(other, names, _, _)| other != file && names.contains(name));
        if let Some((other, _, _, _)) = same {
            let label = format!(
                "{other}{}",
                if prior.contains(other) {
                    " (earlier wave)"
                } else {
                    ""
                }
            );
            let reason = format!(
                "Shadow module: {file} adds {name}, which {label} already exports; import or extend {name} in {other} instead."
            );
            findings.push(Value::object(vec![
                ("file", Value::string(file)),
                ("name", Value::string(name)),
                ("existing", Value::string(other)),
                ("reason", Value::string(&reason)),
            ]));
            flagged.insert((file.clone(), name.clone()));
            continue;
        }
        if !new_file {
            continue;
        }
        let candidate = profile(name)?;
        if candidate.role.is_none() || candidate.nouns.is_empty() {
            continue;
        }
        let mut peer = None;
        for (other, names, stem, profiles) in &modules {
            if other == file {
                continue;
            }
            let same_role = profiles
                .iter()
                .enumerate()
                .filter(|(_, profile)| profile.role == candidate.role)
                .collect::<Vec<_>>();
            let chosen = same_role
                .iter()
                .find(|(_, profile)| profile.nouns == candidate.nouns)
                .copied()
                .or_else(|| {
                    candidate
                        .nouns
                        .is_subset(stem)
                        .then(|| same_role.first().copied())
                        .flatten()
                });
            if let Some((index, _)) = chosen {
                peer = Some((
                    other,
                    names
                        .get(index)
                        .ok_or_else(|| Error::type_error("Shadow profile index changed"))?,
                ));
                break;
            }
        }
        if let Some((other, peer)) = peer {
            let label = format!(
                "{other}{}",
                if prior.contains(other) {
                    " (earlier wave)"
                } else {
                    ""
                }
            );
            let reason = format!(
                "Shadow module: new file {file} adds {name}, which duplicates the {} role of {peer} in {label}; extend {other} instead of a parallel module.",
                candidate.role.unwrap_or("")
            );
            findings.push(Value::object(vec![
                ("file", Value::string(file)),
                ("name", Value::string(name)),
                ("existing", Value::string(other)),
                ("reason", Value::string(&reason)),
            ]));
            flagged.insert((file.clone(), name.clone()));
        }
    }
    let task = task.to_utf8()?;
    for (file, name, _) in &added {
        if flagged.contains(&(file.clone(), name.clone()))
            || file == "src/cli.mjs"
            || file.starts_with("bin/")
            || occurrences(&task, name) > 0
        {
            continue;
        }
        let used = sources.iter().any(|(other, source)| {
            source
                .as_ref()
                .and_then(|source| source.to_utf8().ok())
                .is_some_and(|source| occurrences(&source, name) > usize::from(other == file))
        });
        if used {
            continue;
        }
        let reason = format!(
            "Shadow module: {file} exports {name}, but no product module uses it (only tests, or nothing); wire it into the production caller the task names, make it module-private, or delete it. If a later wave owns the caller, say so in RESULT.md."
        );
        findings.push(Value::object(vec![
            ("file", Value::string(file)),
            ("name", Value::string(name)),
            ("unused", Value::Boolean(true)),
            ("reason", Value::string(&reason)),
        ]));
    }
    boundary.check()?;
    Ok(Value::object(vec![
        (
            "status",
            Value::string(if findings.is_empty() {
                "checked"
            } else {
                "flagged"
            }),
        ),
        ("findings", Value::Array(findings)),
        ("exports", Value::Number(added.len() as f64)),
    ]))
}

pub fn section(shadow: &Value) -> Result<Text, Error> {
    let mut body = Text::from(
        format!(
            "Status: {}\n\n",
            text(required(shadow, "status")?, "Shadow status must be text")?
        )
        .as_str(),
    );
    let findings = shadow
        .get("findings")
        .and_then(domain::items)
        .unwrap_or(&[]);
    if findings.is_empty() {
        body.push_str("- No new export duplicates an existing module or lacks a product caller.\n");
    } else {
        for (index, finding) in findings.iter().enumerate() {
            if index != 0 {
                body.0.push(10);
            }
            let reason = text(
                required(finding, "reason")?,
                "Shadow finding requires a reason",
            )?;
            body.push_str("- ");
            body.push_str(reason.strip_prefix("Shadow module: ").unwrap_or(&reason));
        }
        body.0.push(10);
    }
    Ok(body)
}
