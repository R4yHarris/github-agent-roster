use std::path::Path;

use super::*;
use crate::paths;
use crate::state::secure;

pub fn split_frontmatter(task: &str) -> Result<(String, String), Error> {
    let task = task.replace("\r\n", "\n");
    if !task.starts_with("---\n") {
        return Ok((String::new(), task));
    }
    let end = task
        .get(3..)
        .and_then(|rest| rest.find("\n---\n"))
        .map(|end| end + 3)
        .ok_or_else(|| Error::new("Error", "TASK.md frontmatter must end with ---"))?;
    Ok((task[..end + 5].to_owned(), task[end + 5..].to_owned()))
}

pub fn validate_names(input: &Value) -> Result<Vec<String>, Error> {
    let message = "Task skill selection must be a list of names";
    let items = array(input, message)?;
    let names = items
        .iter()
        .map(|item| text(item, message))
        .collect::<Result<Vec<_>, _>>()?;
    let pattern = regex(r"^[a-z][a-z0-9-]{0,63}$")?;
    if names.iter().any(|name| !pattern.is_match(name))
        || distinct(names.clone()).len() != names.len()
    {
        return Err(Error::new(
            "Error",
            "TASK.md skills must contain distinct, simple skill names",
        ));
    }
    Ok(names)
}

pub fn task_names(input: &Value) -> Result<Vec<String>, Error> {
    let source = text(input, "TASK.md must be text")?;
    let (frontmatter, _) = split_frontmatter(&source)?;
    if frontmatter.is_empty() {
        return Ok(Vec::new());
    }
    let lines = frontmatter[4..frontmatter.len() - 5]
        .split('\n')
        .collect::<Vec<_>>();
    let entries = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with("skills:").then_some(index))
        .collect::<Vec<_>>();
    let Some(index) = entries.first().copied() else {
        return Ok(Vec::new());
    };
    if entries.len() != 1 {
        return Err(Error::new("Error", "TASK.md must not repeat skills"));
    }
    let message = "TASK.md skills must be a list of skill names";
    let value = trim(
        lines
            .get(index)
            .and_then(|line| line.strip_prefix("skills:"))
            .unwrap_or(""),
    );
    let names = if !value.is_empty() {
        let list = value
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .filter(|value| !value.contains(['[', ']']))
            .ok_or_else(|| Error::new("Error", message))?;
        if trim(list).is_empty() {
            Vec::new()
        } else {
            list.split(',').map(|name| trim(name).to_owned()).collect()
        }
    } else {
        let mut names = Vec::new();
        for line in lines.iter().skip(index + 1) {
            if line.chars().next().is_some_and(|c| !unit_space(c)) {
                break;
            }
            let name = regex(r"^  - (\S+)\s*$")?
                .captures(line)
                .and_then(|captures| captures.get(1).map(|entry| entry.as_str()))
                .ok_or_else(|| Error::new("Error", message))?;
            names.push(name.to_owned());
        }
        names
    };
    let names = names
        .into_iter()
        .map(|name| {
            if name.starts_with('"') && name.ends_with('"')
                || name.starts_with('\'') && name.ends_with('\'')
            {
                name.get(1..name.len().saturating_sub(1))
                    .unwrap_or("")
                    .to_owned()
            } else {
                name
            }
        })
        .collect::<Vec<_>>();
    validate_names(&strings(&names))
}

fn read_instruction(root: &Path, file: &Path, message: &str) -> Result<Text, Error> {
    paths::ensure_local_path(file, root)?;
    let snapshot = secure::read(file, MAX_ARTIFACT_BYTES as u64)?
        .ok_or_else(|| Error::new("Error", message))?;
    if paths::file_information(file)?.links != 1 {
        return Err(secure::path_error(file, "has multiple hard links"));
    }
    let source = String::from_utf8(snapshot.bytes)
        .map_err(|_| Error::type_error("Planning instructions must be UTF-8"))?;
    if source.trim().is_empty() {
        return Err(Error::new("Error", message));
    }
    Ok(Text::from(source.as_str()))
}

pub fn load(root: &Path, directory: &Path, names: &[String]) -> Result<Vec<Value>, Error> {
    validate_names(&strings(names))?;
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let root = paths::absolute(root)?;
    let directory = paths::resolve(&root, directory)?;
    paths::ensure_local_path(&directory, &root)?;
    names
        .iter()
        .map(|name| {
            let content = read_instruction(
                &root,
                &directory.join(name).join("SKILL.md"),
                &format!("Unknown or empty task skill: {name}"),
            )?;
            Ok(Value::object(vec![
                ("name", Value::string(name)),
                ("content", Value::String(content)),
            ]))
        })
        .collect()
}

pub fn load_principal(root: &Path, id: &str) -> Result<Value, Error> {
    if !["coder", "reviewer"].contains(&id) {
        return Err(Error::type_error(
            "The builtin SWE principal must be coder or reviewer",
        ));
    }
    let content = read_instruction(
        root,
        &root.join("principals").join(format!("{id}.md")),
        &format!("Missing or empty {id} principal"),
    )?;
    Ok(Value::object(vec![
        ("id", Value::string(id)),
        ("role", Value::string(id)),
        ("content", Value::String(content)),
        (
            "capabilities",
            strings(&if id == "reviewer" {
                vec!["comment".to_owned()]
            } else {
                vec!["commit_branch".to_owned(), "open_pr".to_owned()]
            }),
        ),
        (
            "deny",
            Value::object(vec![
                (
                    "capabilities",
                    strings(
                        &if id == "reviewer" {
                            vec![
                                "commit_branch",
                                "open_pr",
                                "merge",
                                "push_protected",
                                "deploy",
                            ]
                        } else {
                            vec!["merge", "push_protected", "deploy"]
                        }
                        .iter()
                        .map(|name| (*name).to_owned())
                        .collect::<Vec<_>>(),
                    ),
                ),
                (
                    "scope",
                    Value::string(if id == "reviewer" {
                        "REVIEW.md"
                    } else {
                        "TASK.md"
                    }),
                ),
            ]),
        ),
    ]))
}
