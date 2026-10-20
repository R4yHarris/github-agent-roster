use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::domain;
use crate::json::Value;
use crate::learning;
use crate::paths;
use crate::planning::estimate;
use crate::state::{redaction, secure};
use crate::text::{Text, trim};

use super::{integrity, store};

pub const HISTORY_EXPORT_SCHEMA_VERSION: u32 = 1;
pub const EXPORT_SCHEMA: &str = "roster.history-export.v1";
pub const USAGE: &str = "Use roster history list [--format json|text] [--store DIR] [--repo HASH] [--issue N] [--seat NAME] [--model MODEL] [--outcome VALUE] [--since TIME] [--until TIME] or roster history show <session-or-run-or-record-id> [--format json|text] [--store DIR].";

const FILTER_KEYS: [&str; 7] = [
    "repository",
    "issue",
    "seat",
    "model",
    "outcome",
    "since",
    "until",
];
const SUMMARY_KEYS: [&str; 6] = ["at", "issue", "model", "outcome", "repository", "seat"];

fn first_present<'a>(candidates: &[Option<&'a Value>]) -> Option<&'a Value> {
    candidates
        .iter()
        .copied()
        .flatten()
        .find(|value| match value {
            Value::String(text) => !text.0.is_empty(),
            Value::Number(number) => number.is_finite(),
            Value::Boolean(_) => true,
            _ => false,
        })
}

pub fn history_fields(record: &Value) -> Value {
    let payload = record
        .get("payload")
        .filter(|value| matches!(value, Value::Object(_)));
    let issue_obj = record
        .get("issue")
        .filter(|value| matches!(value, Value::Object(_)));
    let payload_issue_obj = payload
        .and_then(|value| value.get("issue"))
        .filter(|value| matches!(value, Value::Object(_)));
    let seat_obj = record
        .get("seat")
        .filter(|value| matches!(value, Value::Object(_)));
    let payload_seat_obj = payload
        .and_then(|value| value.get("seat"))
        .filter(|value| matches!(value, Value::Object(_)));
    let repo = first_present(&[record.get("repoIdentity")]);
    let issue = first_present(&[
        issue_obj.and_then(|value| value.get("issue")),
        payload_issue_obj.and_then(|value| value.get("issue")),
        payload.and_then(|value| value.get("issue")),
    ]);
    let seat = first_present(&[
        seat_obj.and_then(|value| value.get("name")),
        payload_seat_obj.and_then(|value| value.get("name")),
        payload.and_then(|value| value.get("seat")),
    ]);
    let model = first_present(&[
        record.get("servedModel"),
        record.get("requestedModel"),
        payload.and_then(|value| value.get("model")),
    ]);
    let outcome = first_present(&[
        payload.and_then(|value| value.get("outcome")),
        record.get("outcome"),
        record.get("event"),
    ]);
    let at = first_present(&[record.get("createdAt")]);
    Value::object(vec![
        ("repository", repo.cloned().unwrap_or(Value::Null)),
        ("issue", issue.cloned().unwrap_or(Value::Null)),
        ("seat", seat.cloned().unwrap_or(Value::Null)),
        ("model", model.cloned().unwrap_or(Value::Null)),
        ("outcome", outcome.cloned().unwrap_or(Value::Null)),
        ("at", at.cloned().unwrap_or(Value::Null)),
    ])
}

fn to_millis(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) if number.is_finite() => Some(*number),
        Value::String(text) => {
            let raw = text.to_utf8().ok()?;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            let normalized = if trimmed.len() == 20
                && trimmed.ends_with('Z')
                && trimmed.as_bytes().get(10) == Some(&b'T')
            {
                format!("{}.000Z", &trimmed[..19])
            } else if trimmed.len() == 10
                && trimmed.as_bytes().get(4) == Some(&b'-')
                && trimmed.as_bytes().get(7) == Some(&b'-')
            {
                format!("{trimmed}T00:00:00.000Z")
            } else {
                trimmed.to_owned()
            };
            estimate::timestamp(&Value::string(&normalized)).map(|millis| millis as f64)
        }
        _ => None,
    }
}

fn scalar_text(value: Option<&Value>) -> Option<Text> {
    match value? {
        Value::Null | Value::Object(_) | Value::Array(_) => None,
        other => domain::string(other).ok(),
    }
}

pub fn filter_records(records: &[Value], filters: &Value) -> Result<Vec<Value>, Error> {
    let entries = domain::members(filters)
        .ok_or_else(|| Error::type_error("history filters must be an object."))?;
    for (key, _) in entries {
        let name = key.to_utf8()?;
        if !FILTER_KEYS.contains(&name.as_str()) {
            return Err(Error::type_error(format!(
                "Unknown history filter \"{name}\"."
            )));
        }
    }
    let mut since = None;
    let mut until = None;
    for (name, slot) in [("since", &mut since), ("until", &mut until)] {
        if let Some(value) = filters.get(name) {
            let parsed = to_millis(value)
                .ok_or_else(|| Error::type_error(format!("Invalid history {name} time.")))?;
            *slot = Some(parsed);
        }
    }
    if let (Some(since_ms), Some(until_ms)) = (since, until) {
        if since_ms > until_ms {
            return Err(Error::type_error("history since must not be after until."));
        }
    }
    let mut output = Vec::new();
    for record in records {
        if domain::members(record).is_none() {
            return Err(Error::type_error(
                "records must be an array of provenance records.",
            ));
        }
        let fields = history_fields(record);
        let mut matched = true;
        for key in ["repository", "issue", "seat", "model", "outcome"] {
            if let Some(expected) = filters.get(key) {
                let Some(expected_text) = scalar_text(Some(expected)) else {
                    matched = false;
                    break;
                };
                if scalar_text(fields.get(key)).as_ref() != Some(&expected_text) {
                    matched = false;
                    break;
                }
            }
        }
        if !matched {
            continue;
        }
        if since.is_some() || until.is_some() {
            let Some(at) = fields.get("at").and_then(to_millis) else {
                continue;
            };
            if since.is_some_and(|bound| at < bound) || until.is_some_and(|bound| at > bound) {
                continue;
            }
        }
        output.push(record.clone());
    }
    Ok(output)
}

pub fn filter_repositories(records: &[Value], repositories: &Value) -> Result<Vec<Value>, Error> {
    if let Some(items) = domain::items(repositories) {
        Ok(records
            .iter()
            .filter(|record| {
                record
                    .get("repoIdentity")
                    .filter(|value| matches!(value, Value::String(text) if !text.0.is_empty()))
                    .is_some_and(|identity| items.contains(identity))
            })
            .cloned()
            .collect())
    } else {
        filter_records(
            records,
            &Value::object(vec![("repository", repositories.clone())]),
        )
    }
}

pub fn find_history_record<'a>(records: &'a [Value], id: &str) -> Result<&'a Value, Error> {
    if trim(&Text::from(id).0).is_empty() {
        return Err(Error::type_error(
            "history show requires a session, run, or record id.",
        ));
    }
    records
        .iter()
        .find(|record| {
            ["runId", "sessionId", "id"]
                .iter()
                .any(|field| domain::is_text(record.get(field), id))
        })
        .ok_or_else(|| Error::new("Error", format!("No history record found for \"{id}\".")))
}

pub fn require_valid_provenance(records: &[Value]) -> Result<(), Error> {
    for (index, record) in records.iter().enumerate() {
        let verdict = integrity::validate(record)?;
        if verdict.get("ok") == Some(&Value::Boolean(false)) {
            let reason = verdict
                .get("error")
                .and_then(Value::as_string)
                .map(Text::to_utf8)
                .transpose()?
                .unwrap_or_else(|| "invalid provenance".to_owned());
            return Err(Error::new(
                "Error",
                format!("Provenance validation failed for record {index}: {reason}"),
            ));
        }
    }
    Ok(())
}

fn secret_key(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("secret")
        || lower.contains("token")
        || lower.contains("api_key")
        || lower.contains("api-key")
        || lower.contains("apikey")
        || lower.contains("password")
        || lower.contains("authorization")
        || lower.contains("credential")
        || lower.contains("bearer")
}

fn redact_export_value(value: &Value, redaction: &redaction::Config) -> Result<Value, Error> {
    match value {
        Value::String(text) => Ok(Value::String(redaction::diagnostic(
            &crate::planning::memory::redact(text, redaction),
            redaction,
        ))),
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|item| redact_export_value(item, redaction))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Object(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for (key, item) in entries {
                let name = key.to_utf8()?;
                let redacted = if secret_key(&name) {
                    Value::string("[redacted]")
                } else {
                    redact_export_value(item, redaction)?
                };
                out.push((key.clone(), redacted));
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

pub fn canonicalize_record(record: &Value, redaction: &redaction::Config) -> Result<Value, Error> {
    if domain::members(record).is_none() {
        return Err(Error::type_error(
            "canonicalizeRecord requires a provenance record object.",
        ));
    }
    let fields = history_fields(record);
    let fields_obj = Value::object(
        SUMMARY_KEYS
            .iter()
            .map(|key| (*key, fields.get(key).cloned().unwrap_or(Value::Null)))
            .collect(),
    );
    let verdict = integrity::validate(record)?;
    let status = verdict
        .get("integrity")
        .and_then(Value::as_string)
        .cloned()
        .unwrap_or_else(|| Text::from("invalid"));
    let mut entry = Value::object(vec![
        ("fields", fields_obj),
        ("id", record.get("id").cloned().unwrap_or(Value::Null)),
        ("integrity", Value::String(status)),
        ("runId", record.get("runId").cloned().unwrap_or(Value::Null)),
        (
            "sessionId",
            record.get("sessionId").cloned().unwrap_or(Value::Null),
        ),
    ]);
    if let Some(repo) = record
        .get("repoIdentity")
        .filter(|value| **value != Value::Null)
    {
        domain::put(&mut entry, "repository", repo.clone())?;
    }
    if record.get("keepHome").is_some_and(domain::truthy) {
        domain::put(&mut entry, "keepHome", Value::Boolean(true))?;
    }
    let mut entry = redaction::history_record(&entry, redaction)?;
    domain::remove(&mut entry, "keepHome")?;
    redact_export_value(&entry, redaction)
}

pub fn export_history(
    records: &[Value],
    meta: &Value,
    redaction: &redaction::Config,
) -> Result<Value, Error> {
    require_valid_provenance(records).map_err(|error| redaction::error(&error, redaction))?;
    let mut meta_obj = Value::object(vec![
        ("format", Value::string(EXPORT_SCHEMA)),
        ("recordCount", Value::Number(records.len() as f64)),
        (
            "schemaVersion",
            Value::Number(f64::from(HISTORY_EXPORT_SCHEMA_VERSION)),
        ),
        (
            "storeRoot",
            meta.get("storeRoot").cloned().unwrap_or(Value::Null),
        ),
    ]);
    for optional_key in ["exportedAt", "skippedCount"] {
        if let Some(value) = meta
            .get(optional_key)
            .filter(|value| **value != Value::Null)
        {
            domain::put(&mut meta_obj, optional_key, value.clone())?;
        }
    }
    let canonical = records
        .iter()
        .map(|record| canonicalize_record(record, redaction))
        .collect::<Result<Vec<_>, _>>()?;
    let exported = Value::object(vec![
        ("meta", meta_obj),
        ("records", Value::Array(canonical)),
    ]);
    let exported = redaction::history_record(&exported, redaction)?;
    redact_export_value(&exported, redaction)
}

pub fn render_history_json(
    records: &[Value],
    meta: &Value,
    redaction: &redaction::Config,
) -> Result<String, Error> {
    let exported = export_history(records, meta, redaction)?;
    learning::bounded_output(format!("{}\n", integrity::canonical_json(&exported)))
}

fn display_or_dash(value: Option<&Value>, redaction: &redaction::Config) -> Result<String, Error> {
    let Some(text) = scalar_text(value).filter(|text| !text.0.is_empty()) else {
        return Ok("-".to_owned());
    };
    let wrapped = redaction::history_record(&Value::String(text), redaction)?;
    let text = wrapped
        .as_string()
        .ok_or_else(|| Error::type_error("Redacted history field must be text"))?;
    redaction::diagnostic(&crate::planning::memory::redact(text, redaction), redaction).to_utf8()
}

pub fn render_history_summary(
    records: &[Value],
    meta: &Value,
    redaction: &redaction::Config,
) -> Result<String, Error> {
    require_valid_provenance(records).map_err(|error| redaction::error(&error, redaction))?;
    let mut lines = vec![format!("History summary: {} record(s)", records.len())];
    if let Some(store_root) = meta
        .get("storeRoot")
        .filter(|value| **value != Value::Null)
        .filter(|value| scalar_text(Some(*value)).is_some_and(|text| !text.0.is_empty()))
    {
        lines.push(format!(
            "Machine history store: {}",
            display_or_dash(Some(store_root), redaction)?
        ));
    }
    for record in records {
        let entry = canonicalize_record(record, redaction)?;
        let fields = entry.get("fields").unwrap_or(&Value::Null);
        let repo = entry
            .get("repository")
            .filter(|value| **value != Value::Null)
            .or_else(|| fields.get("repository"));
        lines.push(format!(
            "- {}  run={}  session={}",
            display_or_dash(entry.get("id"), redaction)?,
            display_or_dash(entry.get("runId"), redaction)?,
            display_or_dash(entry.get("sessionId"), redaction)?
        ));
        lines.push(format!("    repo={}", display_or_dash(repo, redaction)?));
        lines.push(format!(
            "    issue={}  seat={}  model={}  outcome={}  at={}",
            display_or_dash(fields.get("issue"), redaction)?,
            display_or_dash(fields.get("seat"), redaction)?,
            display_or_dash(fields.get("model"), redaction)?,
            display_or_dash(fields.get("outcome"), redaction)?,
            display_or_dash(fields.get("at"), redaction)?
        ));
    }
    if let Some(repo_state) = meta
        .get("repoState")
        .filter(|value| **value != Value::Null)
        .filter(|value| scalar_text(Some(*value)).is_some_and(|text| !text.0.is_empty()))
    {
        lines.push(format!(
            "Repository state (not part of machine history): {}",
            display_or_dash(Some(repo_state), redaction)?
        ));
    }
    learning::bounded_output(format!("{}\n", lines.join("\n")))
}

fn render_record_line(record: &Value, redaction: &redaction::Config) -> Result<String, Error> {
    let fields = history_fields(record);
    Ok([
        format!(
            "session={}",
            display_or_dash(record.get("sessionId"), redaction)?
        ),
        format!("run={}", display_or_dash(record.get("runId"), redaction)?),
        format!("id={}", display_or_dash(record.get("id"), redaction)?),
        format!(
            "repo={}",
            display_or_dash(fields.get("repository"), redaction)?
        ),
        format!("issue={}", display_or_dash(fields.get("issue"), redaction)?),
        format!("seat={}", display_or_dash(fields.get("seat"), redaction)?),
        format!("model={}", display_or_dash(fields.get("model"), redaction)?),
        format!(
            "outcome={}",
            display_or_dash(fields.get("outcome"), redaction)?
        ),
        format!("at={}", display_or_dash(fields.get("at"), redaction)?),
    ]
    .join("  "))
}

pub fn format_list(records: &[Value], redaction: &redaction::Config) -> Result<String, Error> {
    if records.is_empty() {
        return Ok("No history records matched.\n".to_owned());
    }
    let lines = records
        .iter()
        .map(|record| render_record_line(record, redaction))
        .collect::<Result<Vec<_>, _>>()?;
    learning::bounded_output(format!("{}\n", lines.join("\n")))
}

pub fn format_show(record: &Value, redaction: &redaction::Config) -> Result<String, Error> {
    learning::bounded_output(format!("{}\n", render_record_line(record, redaction)?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    Summary,
}

#[derive(Debug, Clone)]
pub enum Action {
    List { filters: Value },
    Show { id: String },
}

#[derive(Debug, Clone)]
pub struct Request {
    pub action: Action,
    pub store_path: Option<PathBuf>,
    pub format: Format,
}

impl Request {
    pub fn parse(args: &[String]) -> Result<Self, Error> {
        let usage = || Error::type_error(USAGE);
        let Some((action_name, mut rest)) = args.split_first() else {
            return Err(usage());
        };
        let is_show = match action_name.as_str() {
            "list" => false,
            "show" => true,
            _ => return Err(usage()),
        };
        let show_id = if is_show {
            let Some((id, tail)) = rest.split_first() else {
                return Err(usage());
            };
            if id.is_empty() || id.starts_with("--") {
                return Err(usage());
            }
            rest = tail;
            Some(id.clone())
        } else {
            None
        };
        if rest.len() % 2 != 0 {
            return Err(usage());
        }
        let mut seen = BTreeSet::new();
        let mut store_path = None;
        let mut format = Format::Text;
        let mut filters = Value::Object(Vec::new());
        for pair in rest.chunks_exact(2) {
            let flag = pair[0].as_str();
            let value = pair[1].as_str();
            if value.is_empty() || value.starts_with("--") || !seen.insert(flag) {
                return Err(usage());
            }
            match flag {
                "--store" => store_path = Some(PathBuf::from(value)),
                "--format" => {
                    format = match value {
                        "text" => Format::Text,
                        "json" => Format::Json,
                        "summary" => Format::Summary,
                        _ => return Err(usage()),
                    };
                }
                "--repo" if !is_show => {
                    domain::put(&mut filters, "repository", Value::string(value))?
                }
                "--issue" if !is_show => domain::put(&mut filters, "issue", Value::string(value))?,
                "--seat" if !is_show => domain::put(&mut filters, "seat", Value::string(value))?,
                "--model" if !is_show => domain::put(&mut filters, "model", Value::string(value))?,
                "--outcome" if !is_show => {
                    domain::put(&mut filters, "outcome", Value::string(value))?
                }
                "--since" if !is_show => domain::put(&mut filters, "since", Value::string(value))?,
                "--until" if !is_show => domain::put(&mut filters, "until", Value::string(value))?,
                _ => return Err(usage()),
            }
        }
        filter_records(&[], &filters)?;
        let action = match show_id {
            Some(id) => Action::Show { id },
            None => Action::List { filters },
        };
        Ok(Self {
            action,
            store_path,
            format,
        })
    }
}

fn history_redaction(environment: &Value) -> redaction::Config {
    let home = environment
        .get("HOME")
        .or_else(|| environment.get("USERPROFILE"))
        .and_then(Value::as_string)
        .filter(|text| !text.0.is_empty())
        .cloned();
    let mut values = Vec::new();
    for name in ["API_KEY", "PRIVATE_KEY_PATH"] {
        if let Some(value) = environment
            .get(name)
            .and_then(Value::as_string)
            .filter(|text| !text.0.is_empty())
        {
            values.push(value.clone());
        }
    }
    redaction::Config {
        environment: environment.clone(),
        home,
        values,
        ..Default::default()
    }
}

pub fn resolve_history_root(cwd: &Path, store_path: Option<&Path>) -> Result<PathBuf, Error> {
    match store_path {
        Some(path) => paths::resolve(cwd, path),
        None => Ok(paths::repository_paths(cwd)?
            .common_dir
            .join("roster")
            .join("provenance")),
    }
}

pub fn run(request: &Request, cwd: &Path, environment: &Value) -> Result<String, Error> {
    let redaction = history_redaction(environment);
    run_inner(request, cwd, &redaction).map_err(|error| redaction::error(&error, &redaction))
}

fn run_inner(
    request: &Request,
    cwd: &Path,
    redaction: &redaction::Config,
) -> Result<String, Error> {
    if let Action::List { filters } = &request.action {
        filter_records(&[], filters)?;
    }
    let (root, default_repo_state) = match &request.store_path {
        Some(path) => (resolve_history_root(cwd, Some(path))?, None),
        None => {
            let repo = paths::repository_paths(cwd)?;
            (
                repo.common_dir.join("roster").join("provenance"),
                Some(repo.repository.join(".roster")),
            )
        }
    };
    secure::check_components(&root)?;
    let mut missing = false;
    match fs::symlink_metadata(&root) {
        Ok(metadata) => {
            if secure::linked(&metadata) || !metadata.is_dir() {
                return Err(Error::type_error("History store root must be a directory."));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing = true,
        Err(error) => return Err(secure::io_error("Inspect history store root", &root, error)),
    }
    let loaded = store::Store::open(&root)?.read_all_bounded()?;
    let warning = if loaded.skipped.is_empty() {
        String::new()
    } else {
        format!(
            "Skipped {} unreadable or incompatible history record(s).\n",
            loaded.skipped.len()
        )
    };
    let root_text = root
        .to_str()
        .ok_or_else(|| Error::type_error("History store root must be Unicode"))?;
    let json_meta = Value::object(vec![
        ("storeRoot", Value::string(root_text)),
        ("skippedCount", Value::Number(loaded.skipped.len() as f64)),
    ]);
    let mut summary_meta = Value::object(vec![("storeRoot", Value::string(root_text))]);
    if let Some(repo_state) = &default_repo_state {
        if let Some(state_text) = repo_state.to_str() {
            domain::put(&mut summary_meta, "repoState", Value::string(state_text))?;
        }
    }
    match &request.action {
        Action::Show { id } => {
            let record = find_history_record(&loaded.records, id)?;
            match request.format {
                Format::Json => {
                    render_history_json(std::slice::from_ref(record), &json_meta, redaction)
                }
                Format::Summary => learning::bounded_output(format!(
                    "{warning}{}",
                    render_history_summary(std::slice::from_ref(record), &summary_meta, redaction,)?
                )),
                Format::Text => learning::bounded_output(format!(
                    "{warning}{}",
                    format_show(record, redaction)?
                )),
            }
        }
        Action::List { filters } => {
            if missing && request.format != Format::Json {
                return learning::bounded_output(format!(
                    "No history store found at {}.\n",
                    display_or_dash(Some(&Value::string(root_text)), redaction)?
                ));
            }
            let filtered = filter_records(&loaded.records, filters)?;
            match request.format {
                Format::Json => render_history_json(&filtered, &json_meta, redaction),
                Format::Summary => {
                    if filtered.is_empty() {
                        learning::bounded_output(format!("{warning}No history records matched.\n"))
                    } else {
                        learning::bounded_output(format!(
                            "{warning}{}",
                            render_history_summary(&filtered, &summary_meta, redaction)?
                        ))
                    }
                }
                Format::Text => learning::bounded_output(format!(
                    "{warning}{}",
                    format_list(&filtered, redaction)?
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse as parse_json;
    use crate::state::provenance::schema;

    struct Spec<'a> {
        id: &'a str,
        run_id: &'a str,
        session_id: &'a str,
        repo: &'a str,
        created_at: Value,
        issue: Option<&'a str>,
        seat: Option<&'a str>,
        served_model: Option<&'a str>,
        requested_model: Option<&'a str>,
        event: &'a str,
        payload: Value,
    }

    fn make_record(spec: Spec<'_>) -> Result<Value, Error> {
        let mut factory = Value::object(vec![
            ("run", Value::string(spec.run_id)),
            ("session", Value::string(spec.session_id)),
        ]);
        if let Some(issue) = spec.issue {
            domain::put(
                &mut factory,
                "issue",
                Value::object(vec![("issue", Value::string(issue))]),
            )?;
        }
        if let Some(seat) = spec.seat {
            domain::put(
                &mut factory,
                "seat",
                Value::object(vec![("name", Value::string(seat))]),
            )?;
        }
        if let Some(served) = spec.served_model {
            domain::put(&mut factory, "servedModel", Value::string(served))?;
        }
        if let Some(requested) = spec.requested_model {
            domain::put(&mut factory, "requestedModel", Value::string(requested))?;
        }
        let mut record = schema::create_record(&factory, spec.created_at)?;
        domain::put(&mut record, "id", Value::string(spec.id))?;
        domain::put(&mut record, "version", Value::Number(1.0))?;
        domain::put(&mut record, "repoIdentity", Value::string(spec.repo))?;
        domain::put(&mut record, "event", Value::string(spec.event))?;
        domain::put(&mut record, "payload", spec.payload)?;
        Ok(record)
    }

    #[test]
    fn history_filters_and_export_preserve_deterministic_canonical_records_and_redact_secrets()
    -> Result<(), Error> {
        let jan = 1_704_067_200_000.0;
        let feb = 1_706_745_600_000.0;
        let mar = 1_709_251_200_000.0;
        let repo_a = "sha256-1111111111111111111111111111111111111111111111111111111111111111";
        let repo_b = "sha256-2222222222222222222222222222222222222222222222222222222222222222";
        let records = vec![
            make_record(Spec {
                id: "record-1",
                run_id: "r1",
                session_id: "s1",
                repo: repo_a,
                created_at: Value::Number(jan),
                issue: Some("10"),
                seat: Some("coder"),
                served_model: Some("served-a"),
                requested_model: Some("requested-a"),
                event: "completed",
                payload: parse_json(&Text::from(r#"{"outcome":"success"}"#))?,
            })?,
            make_record(Spec {
                id: "record-2",
                run_id: "r2",
                session_id: "s2",
                repo: repo_b,
                created_at: Value::Number(feb),
                issue: None,
                seat: None,
                served_model: None,
                requested_model: None,
                event: "failure",
                payload: parse_json(&Text::from(
                    r#"{"issue":20,"seat":"reviewer","model":"payload-model","outcome":"rework"}"#,
                ))?,
            })?,
            make_record(Spec {
                id: "record-3",
                run_id: "r3",
                session_id: "s3",
                repo: repo_a,
                created_at: Value::Number(mar),
                issue: Some("10"),
                seat: Some("planner"),
                served_model: None,
                requested_model: Some("requested-a"),
                event: "completed",
                payload: Value::Object(Vec::new()),
            })?,
        ];
        let run_ids = |items: &[Value]| -> Vec<String> {
            let mut ids = items
                .iter()
                .filter_map(|item| item.get("runId").and_then(Value::as_string))
                .filter_map(|text| text.to_utf8().ok())
                .collect::<Vec<_>>();
            ids.sort();
            ids
        };
        assert_eq!(
            run_ids(&filter_records(
                &records,
                &parse_json(&Text::from(r#"{"issue":"10"}"#))?
            )?),
            vec!["r1", "r3"]
        );
        assert_eq!(
            run_ids(&filter_records(
                &records,
                &parse_json(&Text::from(r#"{"issue":20}"#))?
            )?),
            vec!["r2"]
        );
        assert_eq!(
            run_ids(&filter_records(
                &records,
                &parse_json(&Text::from(r#"{"seat":"coder"}"#))?
            )?),
            vec!["r1"]
        );
        assert_eq!(
            run_ids(&filter_records(
                &records,
                &parse_json(&Text::from(r#"{"model":"payload-model"}"#))?
            )?),
            vec!["r2"]
        );
        assert_eq!(
            run_ids(&filter_records(
                &records,
                &parse_json(&Text::from(r#"{"outcome":"completed"}"#))?
            )?),
            vec!["r3"]
        );
        assert_eq!(
            run_ids(&filter_records(
                &records,
                &Value::object(vec![("since", Value::Number(feb))])
            )?),
            vec!["r2", "r3"]
        );
        assert_eq!(
            run_ids(&filter_records(
                &records,
                &parse_json(&Text::from(r#"{"until":"2024-02-01T00:00:00Z"}"#))?
            )?),
            vec!["r1", "r2"]
        );
        assert!(filter_records(&[], &parse_json(&Text::from(r#"{"bogus":1}"#))?).is_err());
        assert!(filter_records(&[], &parse_json(&Text::from(r#"{"since":"invalid"}"#))?).is_err());
        assert!(
            filter_records(
                &[],
                &Value::object(vec![
                    ("since", Value::Number(mar)),
                    ("until", Value::Number(jan))
                ])
            )
            .is_err()
        );
        for key in ["r2", "s2", "record-2"] {
            assert_eq!(
                find_history_record(&records, key)?.get("runId"),
                Some(&Value::string("r2"))
            );
        }
        assert!(find_history_record(&records, "missing").is_err());
        assert!(find_history_record(&records, " ").is_err());

        let meta = parse_json(&Text::from(
            r#"{"exportedAt":"2025-01-01T00:00:00.000Z","storeRoot":"/store/root"}"#,
        ))?;
        let first_json = render_history_json(&records, &meta, &redaction::Config::default())?;
        let second_json = render_history_json(&records, &meta, &redaction::Config::default())?;
        assert_eq!(first_json, second_json);

        let sentinel = "test-only-private-api-key";
        let secret_record = make_record(Spec {
            id: "rec-secret",
            run_id: "run-secret",
            session_id: "session-secret",
            repo: repo_a,
            created_at: Value::string("2025-01-02T03:04:05.000Z"),
            issue: None,
            seat: None,
            served_model: Some("model-a"),
            requested_model: None,
            event: "failure",
            payload: parse_json(&Text::from(
                format!(r#"{{"outcome":"failed","api_key":"{sentinel}","token":"{sentinel}"}}"#)
                    .as_str(),
            ))?,
        })?;
        let redaction = redaction::Config {
            environment: parse_json(&Text::from(
                format!(r#"{{"ROSTER_API_KEY":"{sentinel}"}}"#).as_str(),
            ))?,
            api_key_env: Text::from("ROSTER_API_KEY"),
            ..Default::default()
        };
        let secret_json =
            render_history_json(std::slice::from_ref(&secret_record), &meta, &redaction)?;
        assert!(!secret_json.contains(sentinel));
        let summary = render_history_summary(
            &[secret_record],
            &parse_json(&Text::from(
                r#"{"storeRoot":"/home/me/repo/.git/roster/provenance","repoState":"/home/me/repo/.roster"}"#,
            ))?,
            &redaction,
        )?;
        assert!(!summary.contains(sentinel));
        assert!(summary.contains("Machine history store: /home/me/repo/.git/roster/provenance"));
        assert!(
            summary
                .contains("Repository state (not part of machine history): /home/me/repo/.roster")
        );

        assert!(filter_records(&records, &Value::object(vec![("since", Value::Null)])).is_err());
        assert!(filter_records(&records, &Value::object(vec![("until", Value::Null)])).is_err());
        assert!(filter_repositories(&records, &Value::Null)?.is_empty());

        let nested_payload_record = make_record(Spec {
            id: "rec-nested",
            run_id: "r-nested",
            session_id: "s-nested",
            repo: repo_a,
            created_at: Value::Number(jan),
            issue: Some(""),
            seat: None,
            served_model: Some("model-a"),
            requested_model: None,
            event: "completed",
            payload: parse_json(&Text::from(
                r#"{"issue":{"task":"fix-native"},"seat":{"name":"planner"},"outcome":"planned"}"#,
            ))?,
        })?;
        let canonical = canonicalize_record(&nested_payload_record, &redaction)?;
        assert_eq!(
            canonical.get("fields").and_then(|f| f.get("issue")),
            Some(&Value::Null)
        );
        assert_eq!(
            canonical.get("fields").and_then(|f| f.get("seat")),
            Some(&Value::string("planner"))
        );
        Ok(())
    }
}
