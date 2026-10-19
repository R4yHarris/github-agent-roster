use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::*;
use crate::json;
use crate::paths;
use crate::state::{files, redaction, secure};

pub fn redact(text: &Text, config: &redaction::Config) -> Text {
    let safe = redaction::evidence(text, config);
    let source = encode(&safe);
    let mut position = 0;
    while let Some(offset) = source
        .get(position..)
        .and_then(|source| source.find("-----BEGIN "))
    {
        let start = position + offset;
        let after = start + "-----BEGIN ".len();
        let rest = &source[after..];
        if let Some(end) = rest.find("PRIVATE KEY-----") {
            let prefix = &rest[..end];
            if !prefix.contains(['-', '\r', '\n']) {
                return decode(&format!("{}[redacted]", &source[..start]));
            }
        }
        position = after;
    }
    safe
}

pub fn planner_path(repo_root: &Path, configured: &str) -> Result<PathBuf, Error> {
    if forbidden_read(configured) {
        return Err(Error::new(
            "Error",
            "Seat memory must not use a protected or secret path",
        ));
    }
    let coder = paths::resolve(repo_root, Path::new(configured))?;
    paths::ensure_local_path(&coder, repo_root)?;
    Ok(coder
        .parent()
        .ok_or_else(|| Error::type_error("Seat memory needs a parent directory"))?
        .join("planner.jsonl"))
}

pub fn safe_record(record: &Value, redaction: &redaction::Config) -> Result<Value, Error> {
    let entries = domain::members(record)
        .ok_or_else(|| Error::type_error("Memory record must be a plain JSON object"))?;
    let mut safe = Value::object(Vec::new());
    for (key, value) in entries {
        let key = encode(key);
        if !matches(r"(?i)^[a-z][a-z0-9_]*$", &key)?
            || matches(
                r"(?i)(?:^|_)(?:content|body|messages|password|secret|token|key)(?:_|$)",
                &key,
            )?
        {
            return Err(Error::type_error(
                "Memory accepts notebook metadata, not credentials or file/message bodies",
            ));
        }
        let safe_value = match value {
            Value::Null | Value::Boolean(_) => value.clone(),
            Value::Number(number) if number.is_finite() => value.clone(),
            Value::String(value) => {
                let text = redact(value, redaction);
                let source = encode(&text);
                let first = trim(
                    source
                        .split('\n')
                        .next()
                        .unwrap_or("")
                        .strip_suffix('\r')
                        .unwrap_or(source.split('\n').next().unwrap_or("")),
                );
                super::value(&if first.chars().count() > 480 || first != trim(&source) {
                    format!("{} [details omitted]", slice(first, 480))
                } else {
                    first.to_owned()
                })
            }
            _ => {
                return Err(Error::type_error(
                    "Memory values must be compact scalar summaries",
                ));
            }
        };
        domain::put(&mut safe, &key, safe_value)?;
    }
    Ok(safe)
}

pub fn planner_text(memory: &[Text], redaction: &redaction::Config) -> Result<Text, Error> {
    for line in memory {
        if line.0.iter().any(|unit| matches!(unit, 10 | 13)) {
            return Err(Error::type_error(
                "Planner memory must contain single-line JSONL data",
            ));
        }
        let record = crate::json::parse(line)
            .map_err(|_| Error::type_error("Planner memory must contain JSON objects"))?;
        if domain::members(&record).is_none() {
            return Err(Error::type_error(
                "Planner memory must contain JSON objects",
            ));
        }
    }
    let mut lines = Vec::new();
    let mut used = 0;
    let mut omitted = memory.len() > 20;
    for line in memory.iter().rev().take(20) {
        let safe = redact(line, redaction);
        if used + safe.0.len() + 1 > 3960 {
            omitted = true;
            break;
        }
        used += safe.0.len() + 1;
        lines.push(encode(&safe));
    }
    lines.reverse();
    if omitted {
        lines.insert(0, "[Older planner memory omitted by budget]".to_owned());
    }
    Ok(decode(&lines.join("\n")))
}

pub fn read(
    file: &Path,
    repo_root: &Path,
    redaction: &redaction::Config,
) -> Result<Vec<Text>, Error> {
    paths::ensure_local_path(file, repo_root)?;
    secure::check_components(file)?;
    let mut handle = match fs::File::open(file) {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(secure::io_error("Read planner memory", file, error)),
    };
    let information = paths::handle_information(&handle)?;
    if information.links != 1
        || !handle
            .metadata()
            .map_err(|error| secure::io_error("Inspect planner memory", file, error))?
            .is_file()
        || paths::file_information(file)?.identity != information.identity
    {
        return Err(secure::path_error(
            file,
            "is not a regular single-link memory file",
        ));
    }
    let size = handle
        .metadata()
        .map_err(|error| secure::io_error("Inspect planner memory", file, error))?
        .len();
    let start = size.saturating_sub(20 * 4096 + 4096);
    handle
        .seek(SeekFrom::Start(start))
        .map_err(|error| secure::io_error("Seek planner memory tail", file, error))?;
    let mut bytes = Vec::new();
    (&mut handle)
        .take(size - start + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| secure::io_error("Read planner memory tail", file, error))?;
    if bytes.len() as u64 != size - start
        || paths::file_information(file)?.identity != information.identity
    {
        return Err(Error::new("Error", "Memory changed while reading its tail"));
    }
    if size != 0 && bytes.last() != Some(&b'\n') {
        return Err(Error::new(
            "Error",
            "Memory ends with an incomplete JSONL line",
        ));
    }
    let bytes = if start > 0 {
        let newline = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| Error::type_error("Planner memory line exceeds bounded tail"))?;
        &bytes[newline + 1..]
    } else {
        bytes.as_slice()
    };
    let source = String::from_utf8(bytes.to_vec())
        .map_err(|_| Error::type_error("Planner memory must be UTF-8 JSONL"))?;
    let mut lines = source.split('\n').collect::<Vec<_>>();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let start_line = lines.len().saturating_sub(20);
    lines
        .iter()
        .skip(start_line)
        .enumerate()
        .map(|(index, line)| {
            let record = json::parse(&Text::from(line.strip_suffix('\r').unwrap_or(line)))
                .map_err(|_| {
                    Error::new(
                        "Error",
                        format!("Invalid memory JSONL tail entry {}", index + 1),
                    )
                })?;
            Ok(Text::from(safe_record(&record, redaction)?.json().as_str()))
        })
        .collect()
}

pub fn append(
    file: &Path,
    repo_root: &Path,
    record: &Value,
    redaction: &redaction::Config,
) -> Result<(), Error> {
    paths::ensure_local_path(file, repo_root)?;
    let data = format!("{}\n", safe_record(record, redaction)?.json());
    if data.len() > 4096 {
        return Err(Error::type_error("Memory record exceeds 4096 bytes"));
    }
    let parent = file
        .parent()
        .ok_or_else(|| Error::type_error("Planner memory needs a parent directory"))?;
    secure::check_components(parent)?;
    files::create_private_directories(parent)?;
    secure::check_components(file)?;
    let mut options = OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut handle = options
        .open(file)
        .map_err(|error| secure::io_error("Open planner memory", file, error))?;
    fs4::fs_std::FileExt::try_lock_exclusive(&handle)
        .map_err(|error| secure::io_error("Lock planner memory", file, error))?;
    let information = paths::handle_information(&handle)?;
    if information.links != 1 || paths::file_information(file)?.identity != information.identity {
        return Err(secure::path_error(
            file,
            "changed while opening planner memory",
        ));
    }
    let stat = handle
        .metadata()
        .map_err(|error| secure::io_error("Inspect planner memory", file, error))?;
    if !stat.is_file() {
        return Err(Error::new("Error", "Memory must be a regular JSONL file"));
    }
    if stat.len() > 0 {
        handle
            .seek(SeekFrom::End(-1))
            .map_err(|error| secure::io_error("Seek planner memory", file, error))?;
        let mut last = [0];
        handle
            .read_exact(&mut last)
            .map_err(|error| secure::io_error("Read planner memory boundary", file, error))?;
        if last != *b"\n" {
            return Err(Error::new(
                "Error",
                "Memory ends with an incomplete JSONL line; refusing to rewrite it",
            ));
        }
    }
    handle
        .write_all(data.as_bytes())
        .and_then(|()| handle.sync_all())
        .map_err(|error| secure::io_error("Append planner memory", file, error))?;
    if paths::file_information(file)?.identity != information.identity {
        return Err(secure::path_error(
            file,
            "changed during planner memory append",
        ));
    }
    fs4::fs_std::FileExt::unlock(&handle)
        .map_err(|error| secure::io_error("Unlock planner memory", file, error))
}
