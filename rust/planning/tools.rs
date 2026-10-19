use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::json;
use crate::paths;
use crate::state::secure;

pub const MAX_TOOL_BYTES: usize = 131_072;

#[derive(Debug, Clone)]
pub struct Call {
    pub id: String,
    pub args: Value,
}

impl Call {
    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("id", value(&self.id)),
            ("type", Value::string("function")),
            (
                "function",
                Value::object(vec![
                    ("name", Value::string("write_file")),
                    ("arguments", Value::string(&self.args.json())),
                ]),
            ),
            ("args", self.args.clone()),
        ])
    }

    pub fn wire_value(&self) -> Value {
        let mut call = self.as_value();
        if let Value::Object(entries) = &mut call {
            entries.retain(|(key, _)| !crate::text::ascii(&key.0, "args"));
        }
        call
    }
}

#[derive(Debug, Clone)]
pub struct Calls {
    pub calls: Vec<Call>,
    pub from_text: bool,
}

impl Calls {
    pub fn as_value(&self) -> Value {
        Value::object(vec![
            (
                "calls",
                Value::Array(self.calls.iter().map(Call::as_value).collect()),
            ),
            ("fromText", Value::Boolean(self.from_text)),
        ])
    }
}

fn payload_json(source: &str) -> Result<Value, Error> {
    json::parse(&decode(source)).map_err(|_| {
        Error::new(
            "Error",
            "Planner tool payload or arguments are not valid JSON",
        )
    })
}

fn embedded_json(source: &str) -> Result<Option<&str>, Error> {
    let start = regex(r"\{|\[\s*[\[{]")?
        .find(source)
        .map(|entry| entry.start());
    let Some(start) = start else {
        return Ok(None);
    };
    let mut stack = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    for (offset, c) in source[start..].char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        if c == '"' {
            quoted = true;
        } else if matches!(c, '{' | '[') {
            stack.push(c);
        } else if matches!(c, '}' | ']') {
            if stack.pop() != Some(if c == '}' { '{' } else { '[' }) {
                return Ok(None);
            }
            if stack.is_empty() {
                return Ok(Some(&source[start..start + offset + c.len_utf8()]));
            }
        }
    }
    Ok(None)
}

pub fn parse_calls(message: &Value, turn: usize, used_ids: &[String]) -> Result<Calls, Error> {
    let fail = |message| Error::new("Error", message);
    if domain::members(message).is_none() {
        return Err(fail("Planner returned no assistant message"));
    }
    let mut payload = message.get("tool_calls").cloned();
    let mut from_text = false;
    if payload.as_ref().is_none_or(|value| {
        value == &Value::Null || domain::items(value).is_some_and(|items| items.is_empty())
    }) {
        let Some(content) = message.get("content").and_then(Value::as_string) else {
            return Ok(Calls {
                calls: Vec::new(),
                from_text: false,
            });
        };
        let source = encode(content);
        if !matches(r#""(?:tool_calls|function|arguments|path)"\s*:"#, &source)? {
            return Ok(Calls {
                calls: Vec::new(),
                from_text: false,
            });
        }
        if content.utf8_len() > MAX_TOOL_BYTES {
            return Err(fail("Planner tool payload exceeds 128 KiB"));
        }
        let extracted = embedded_json(&source)?
            .ok_or_else(|| fail("Planner text contains an incomplete JSON tool payload"))?;
        payload = Some(payload_json(extracted)?);
        from_text = true;
    } else if let Some(Value::String(source)) = &payload {
        if source.utf8_len() > MAX_TOOL_BYTES {
            return Err(fail("Planner tool payload exceeds 128 KiB"));
        }
        payload = Some(payload_json(&encode(source))?);
    }
    let payload =
        payload.ok_or_else(|| fail("Planner must emit at most three function tool calls"))?;
    let payload = payload.get("tool_calls").unwrap_or(&payload);
    let raw = if let Some(items) = domain::items(payload) {
        items.to_vec()
    } else if domain::members(payload).is_some() {
        vec![payload.clone()]
    } else {
        return Err(fail("Planner must emit at most three function tool calls"));
    };
    let real = raw
        .iter()
        .filter(|entry| {
            !entry
                .get("function")
                .filter(|value| domain::members(value).is_some())
                .is_some_and(|function| {
                    (function.get("arguments").is_none()
                        || domain::is_text(function.get("arguments"), ""))
                        && function.get("path").is_none()
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    let entries = if !real.is_empty() && real.len() < raw.len() {
        real
    } else {
        raw
    };
    if entries.len() > 3 {
        return Err(fail("Planner must emit at most three function tool calls"));
    }
    let mut ids = used_ids.to_vec();
    let mut calls = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if domain::members(entry).is_none()
            || entry
                .get("type")
                .is_some_and(|value| !domain::is_text(Some(value), "function"))
        {
            return Err(fail("Planner tool call must be a function object"));
        }
        let definition = entry
            .get("function")
            .filter(|value| domain::members(value).is_some())
            .unwrap_or(entry);
        let name = definition
            .get("name")
            .filter(|value| **value != Value::Null)
            .cloned()
            .or_else(|| {
                (from_text && definition.get("path").is_some()).then(|| Value::string("write_file"))
            });
        if !domain::is_text(name.as_ref(), "write_file") {
            return Err(fail(
                "Planner tool is unavailable; only write_file is allowed",
            ));
        }
        let mut args = definition
            .get("arguments")
            .filter(|value| **value != Value::Null)
            .cloned()
            .or_else(|| {
                (from_text && definition.get("path").is_some()).then(|| definition.clone())
            });
        if let Some(Value::String(source)) = &args {
            if source.utf8_len() > MAX_TOOL_BYTES {
                return Err(fail("Planner arguments exceed 128 KiB"));
            }
            args = Some(payload_json(&encode(source))?);
        }
        let args = args.ok_or_else(|| {
            fail("Planner write_file arguments must be a JSON object with path and content strings")
        })?;
        if fields(&args, &["path", "content"], &[], "").is_err()
            || args.get("path").and_then(Value::as_string).is_none()
            || args.get("content").and_then(Value::as_string).is_none()
        {
            return Err(fail(
                "Planner write_file arguments must be a JSON object with path and content strings",
            ));
        }
        let id = entry
            .get("id")
            .filter(|value| **value != Value::Null)
            .cloned()
            .unwrap_or_else(|| Value::string(&format!("planner-{turn}-{index}")));
        let id = id
            .as_string()
            .filter(|id| {
                !id.0.is_empty()
                    && id.0.len() <= 128
                    && !id.0.iter().any(|unit| matches!(unit, 0..=31 | 127))
            })
            .map(encode)
            .filter(|id| !ids.contains(id))
            .ok_or_else(|| fail("Planner tool call ID is invalid or duplicated"))?;
        ids.push(id.clone());
        calls.push(Call { id, args });
    }
    Ok(Calls { calls, from_text })
}

fn definition(name: &str, description: &str, required_fields: &[&str], properties: Value) -> Value {
    Value::object(vec![
        ("type", Value::string("function")),
        (
            "function",
            Value::object(vec![
                ("name", Value::string(name)),
                ("description", Value::string(description)),
                (
                    "parameters",
                    Value::object(vec![
                        ("type", Value::string("object")),
                        ("properties", properties),
                        (
                            "required",
                            Value::Array(
                                required_fields
                                    .iter()
                                    .map(|field| Value::string(field))
                                    .collect(),
                            ),
                        ),
                        ("additionalProperties", Value::Boolean(false)),
                    ]),
                ),
            ]),
        ),
    ])
}

pub fn definitions(plan_mode: bool) -> Value {
    let write = definition(
        "write_file",
        if plan_mode {
            "Write only the root PLAN.md. Product files, TASK.md, and RECIPE.yml are forbidden."
        } else {
            "Write a planning draft at RECIPE.yml, TASK.md, or ESTIMATE.md only. The harness validates the final task and finalizes these managed artifacts; no app-code writes."
        },
        &["path", "content"],
        Value::object(vec![
            (
                "path",
                Value::object(vec![
                    ("type", Value::string("string")),
                    (
                        "enum",
                        strings(
                            &(if plan_mode {
                                PLAN_ARTIFACTS
                            } else {
                                PLANNER_ARTIFACTS
                            })
                            .iter()
                            .map(|name| (*name).to_owned())
                            .collect::<Vec<_>>(),
                        ),
                    ),
                ]),
            ),
            (
                "content",
                Value::object(vec![
                    ("type", Value::string("string")),
                    ("maxLength", Value::Number(MAX_ARTIFACT_BYTES as f64)),
                ]),
            ),
        ]),
    );
    if !plan_mode {
        return Value::Array(vec![write]);
    }
    let string_type = || Value::object(vec![("type", Value::string("string"))]);
    let positive = || {
        Value::object(vec![
            ("type", Value::string("integer")),
            ("minimum", Value::Number(1.0)),
        ])
    };
    Value::Array(vec![
        definition(
            "read_file",
            "Read a UTF-8 file inside the worktree.",
            &["path"],
            Value::object(vec![
                ("path", string_type()),
                ("max_lines", positive()),
                ("offset", positive()),
            ]),
        ),
        write,
        definition(
            "list_dir",
            "List entries in a worktree directory.",
            &[],
            Value::object(vec![("path", string_type())]),
        ),
        definition(
            "search_text",
            "Find literal text in regular worktree files; returns at most 50 matching lines.",
            &["query"],
            Value::object(vec![("query", string_type()), ("path", string_type())]),
        ),
    ])
}

fn access(message: impl Into<String>) -> Error {
    Error::new("ToolAccessError", message).coded("ROSTER_TOOL_DENIED")
}

fn locate(root: &Path, input: &str, directory: bool) -> Result<PathBuf, Error> {
    let normalized = input.replace('\\', "/");
    let parts = normalized.split('/').collect::<Vec<_>>();
    if input.is_empty()
        || Path::new(input).is_absolute()
        || normalized.starts_with('/')
        || parts.iter().any(|part| *part == ".." || part.is_empty())
        || forbidden_read(&normalized)
        || parts
            .iter()
            .any(|part| part.eq_ignore_ascii_case(".roster"))
        || parts.iter().any(|part| part.eq_ignore_ascii_case("vendor"))
        || !directory && normalized == "."
    {
        return Err(access(
            "Refused: protected, secret, ambiguous, or outside planning path.",
        ));
    }
    let path = root.join(input);
    if paths::absolute(&path)? != paths::absolute(root)? {
        paths::ensure_local_path(&path, root)?;
    }
    secure::check_components(&path)?;
    Ok(path)
}

pub struct Boundary {
    pub writer: artifacts::Writer,
    pub reads: bool,
}

impl Boundary {
    pub fn execute(&mut self, name: &str, args: &Value) -> Result<Value, Error> {
        self.writer.check()?;
        let result = match name {
            "write_file" => {
                fields(
                    args,
                    &["path", "content"],
                    &[],
                    "write_file requires only path and content",
                )?;
                let path = required(args, "path")?
                    .as_string()
                    .ok_or_else(|| Error::type_error("write_file path must be text"))?
                    .to_utf8()?;
                let content = required(args, "content")?
                    .as_string()
                    .ok_or_else(|| Error::type_error("write_file content must be text"))?;
                self.writer.write(&path, content)?
            }
            "read_file" if self.reads => {
                fields(
                    args,
                    &["path"],
                    &["max_lines", "offset"],
                    "Invalid read_file arguments",
                )?;
                let path = required(args, "path")?
                    .as_string()
                    .ok_or_else(|| Error::type_error("read_file path must be text"))?
                    .to_utf8()?;
                let mut counts = Vec::new();
                for (field, message) in [
                    (
                        "max_lines",
                        "read_file max_lines must be a positive safe integer",
                    ),
                    (
                        "offset",
                        "read_file offset must be a positive safe integer line number",
                    ),
                ] {
                    counts.push(if args.get(field).is_some() {
                        Some(
                            domain::safe_integer(args.get(field))
                                .filter(|value| *value > 0)
                                .ok_or_else(|| Error::type_error(message))?,
                        )
                    } else {
                        None
                    });
                }
                let target = locate(self.writer.root(), &path, false)?;
                let snapshot = secure::read(&target, 256 * 1024)?.ok_or_else(|| Error::new("Error", format!("read_file: {path} does not exist. Use list_dir or search_text to find the right path")).coded("ENOENT"))?;
                if paths::file_information(&target)?.links != 1 {
                    return Err(access("Planning reads require single-link regular files"));
                }
                let source = String::from_utf8(snapshot.bytes)
                    .map_err(|_| Error::type_error("Planning reads require UTF-8 text"))?;
                if counts.iter().all(Option::is_none) {
                    Value::string(&source)
                } else {
                    let offset = counts.get(1).copied().flatten().unwrap_or(1) - 1;
                    let count = counts.first().copied().flatten().unwrap_or(u64::MAX);
                    Value::string(
                        &source
                            .replace("\r\n", "\n")
                            .split('\n')
                            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
                            .take(usize::try_from(count).unwrap_or(usize::MAX))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }
            }
            "list_dir" if self.reads => {
                fields(args, &[], &["path"], "Invalid list_dir arguments")?;
                let path = args
                    .get("path")
                    .map(|value| {
                        text(value, "list_dir path must be text")
                            .and_then(|value| decode(&value).to_utf8())
                    })
                    .transpose()?
                    .unwrap_or_else(|| ".".to_owned());
                let target = locate(self.writer.root(), &path, true)?;
                let mut entries = Vec::new();
                for entry in fs::read_dir(&target)
                    .map_err(|error| secure::io_error("List planning directory", &target, error))?
                {
                    let entry = entry.map_err(|error| {
                        secure::io_error("Read planning directory entry", &target, error)
                    })?;
                    let name = entry.file_name().into_string().map_err(|_| {
                        Error::type_error("Planning directory names must be Unicode")
                    })?;
                    let child = if path == "." {
                        name.clone()
                    } else {
                        format!("{path}/{name}")
                    };
                    if forbidden_read(&child)
                        || child
                            .split('/')
                            .any(|part| part.eq_ignore_ascii_case("vendor"))
                        || child
                            .split('/')
                            .any(|part| part.eq_ignore_ascii_case(".roster"))
                    {
                        continue;
                    }
                    let metadata = entry.file_type().map_err(|error| {
                        secure::io_error("Inspect planning directory entry", &entry.path(), error)
                    })?;
                    entries.push((
                        name,
                        if metadata.is_dir() {
                            "directory"
                        } else if metadata.is_file() {
                            "file"
                        } else if metadata.is_symlink() {
                            "symlink"
                        } else {
                            "other"
                        },
                    ));
                    if entries.len() > 4096 {
                        return Err(Error::type_error("Planning directory exceeds 4096 entries"));
                    }
                }
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                Value::Array(
                    entries
                        .iter()
                        .map(|(name, kind)| {
                            Value::object(vec![
                                ("name", Value::string(name)),
                                ("type", Value::string(kind)),
                            ])
                        })
                        .collect(),
                )
            }
            "search_text" if self.reads => self.search(args)?,
            _ => {
                return Err(access(
                    "Planner tool is unavailable; no app-write, execution, network, publication, merge, or deployment tools are granted",
                ));
            }
        };
        self.writer.check()?;
        Ok(result)
    }

    fn search(&self, args: &Value) -> Result<Value, Error> {
        fields(args, &["query"], &["path"], "Invalid search_text arguments")?;
        let query = text(
            required(args, "query")?,
            "search_text query must be nonempty, single-line literal text",
        )?;
        if query.is_empty() || query.contains(['\r', '\n', '\0']) {
            return Err(Error::type_error(
                "search_text query must be nonempty, single-line literal text",
            ));
        }
        let query = decode(&query).to_utf8()?;
        let path = args
            .get("path")
            .map(|value| {
                text(value, "search_text path must be text")
                    .and_then(|value| decode(&value).to_utf8())
            })
            .transpose()?
            .unwrap_or_else(|| ".".to_owned());
        let target = locate(self.writer.root(), &path, true)?;
        let mut pending = vec![target];
        let mut found = Vec::new();
        let mut visited = 0;
        let mut truncated = false;
        while let Some(path) = pending.pop() {
            self.writer.check()?;
            if paths::absolute(&path)? != paths::absolute(self.writer.root())? {
                paths::ensure_local_path(&path, self.writer.root())?;
            }
            let relative = path
                .strip_prefix(self.writer.root())
                .map_err(|_| access("Search escaped planning worktree"))?
                .to_str()
                .ok_or_else(|| Error::type_error("Search paths must be Unicode"))?
                .replace('\\', "/");
            if forbidden_read(&relative)
                || relative
                    .split('/')
                    .any(|part| part.eq_ignore_ascii_case("vendor"))
                || relative
                    .split('/')
                    .any(|part| [".roster", ".git", "node_modules", "vendor"].contains(&part))
            {
                continue;
            }
            let status = fs::symlink_metadata(&path)
                .map_err(|error| secure::io_error("Inspect planning search path", &path, error))?;
            if secure::linked(&status) {
                continue;
            }
            if status.is_dir() {
                let mut children = fs::read_dir(&path)
                    .map_err(|error| {
                        secure::io_error("Read planning search directory", &path, error)
                    })?
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        secure::io_error("Read planning search entry", &path, error)
                    })?;
                if children.len() > 4096 {
                    return Err(Error::type_error(
                        "Planning search directory exceeds 4096 entries",
                    ));
                }
                children.sort();
                pending.extend(children.into_iter().rev());
            } else if status.is_file() {
                visited += 1;
                if visited > 400 || found.len() > 50 {
                    truncated = true;
                    break;
                }
                if status.len() > 256 * 1024 {
                    continue;
                }
                let Some(snapshot) = secure::read(&path, 256 * 1024)? else {
                    continue;
                };
                if paths::file_information(&path)?.links != 1 {
                    continue;
                }
                let source = String::from_utf8(snapshot.bytes)
                    .map_err(|_| Error::type_error("Planning search encountered non-UTF-8 text"))?;
                if source.contains('\0') {
                    continue;
                }
                for (index, line) in source.replace("\r\n", "\n").split('\n').enumerate() {
                    if line.contains(&query) {
                        found.push(Value::object(vec![
                            ("path", Value::string(&relative)),
                            ("line", Value::Number((index + 1) as f64)),
                            ("text", Value::string(line)),
                        ]));
                        if found.len() > 50 {
                            truncated = true;
                            break;
                        }
                    }
                }
            }
        }
        found.truncate(50);
        Ok(Value::object(vec![
            ("matches", Value::Array(found)),
            ("truncated", Value::Boolean(truncated)),
        ]))
    }
}
