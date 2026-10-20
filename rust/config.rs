use std::collections::HashSet;
use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

use url::Url;

use crate::domain::{self, is_text, members, put};
use crate::json::Value;
use crate::run::Provider;
use crate::text::{Text, ascii, is_space, parse_json_string, trim, trim_end};
use crate::{Error, MAX_SAFE_INTEGER, cancel, request, settings};

const PROFILE_NAMES: &[&str] = &["vllm-local", "ollama", "lmstudio", "openai"];
const TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "glob_files",
    "list_dir",
    "run_test",
    "run_command",
    "search_text",
    "web_search",
    "web_fetch",
];
const EFFORTS: &[&str] = &["l", "m", "h", "x", "none"];
const SECTIONS: &[(&str, &[&str])] = &[
    (
        "llm",
        &[
            "base_url",
            "model",
            "api_key_env",
            "effort",
            "effort_override",
            "context_max",
            "profile",
            "api_key_optional",
            "provider",
            "request_timeout_ms",
            "served_model_label",
            "max_requests",
        ],
    ),
    ("planner", &["turn_budget", "critic_profile"]),
    (
        "seat",
        &[
            "id",
            "principal",
            "turn_budget",
            "tools",
            "context_chars",
            "scope_expansion",
            "max_attempts",
            "evidence_workspace",
            "task_timeout_ms",
        ],
    ),
    ("paths", &["memory", "skills", "asks", "worktrees"]),
    ("publish", &["enabled"]),
    ("tools", &["internet", "run_test"]),
    ("reviewer", &["required"]),
    ("review", &["required"]),
    ("loop", &["turns"]),
    ("context", &["budget"]),
    ("start", &["base", "sync"]),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Config(Value);

impl Config {
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub(crate) fn for_task(&self, options: &Value) -> Result<Self, Error> {
        crate::llm::reasoning::select_reasoning(&self.0, options).map(Self)
    }

    pub fn with_profile(&self, profile: &crate::fleet::Profile) -> Result<Self, Error> {
        crate::fleet::with_profile(&self.0, profile.as_value()).map(Self)
    }
}

pub fn invalid(message: impl AsRef<str>) -> Error {
    Error::new(
        "Error",
        format!("Invalid roster config: {}", message.as_ref()),
    )
}

fn section_fields(section: &str) -> Option<&'static [&'static str]> {
    SECTIONS
        .iter()
        .find(|(name, _)| *name == section)
        .map(|(_, fields)| *fields)
}

fn uncomment(line: &[u16]) -> &[u16] {
    let mut quoted = false;
    let mut escaped = false;
    for (index, &unit) in line.iter().enumerate() {
        if quoted && unit == 92 && !escaped {
            escaped = true;
            continue;
        }
        if unit == 34 && !escaped {
            quoted = !quoted;
        }
        if !quoted
            && unit == 35
            && (index == 0 || line.get(index - 1).is_some_and(|unit| is_space(*unit)))
        {
            return trim_end(&line[..index]);
        }
        escaped = false;
    }
    trim_end(line)
}

fn mapping_field(line: &[u16], indent: usize) -> Option<(String, &[u16])> {
    let body = line.get(indent..)?;
    let length = body
        .iter()
        .take_while(|unit| matches!(unit, 95 | 97..=122))
        .count();
    if length == 0 || body.get(length..length + 2) != Some(&[58, 32]) {
        return None;
    }
    let value = body.get(length + 2..)?;
    if value.is_empty() || value.iter().any(|unit| matches!(unit, 0x2028 | 0x2029)) {
        return None;
    }
    Some((Text(body[..length].to_vec()).to_utf8().ok()?, value))
}

fn root_field(line: &[u16]) -> Option<(String, Option<&[u16]>)> {
    let length = line
        .iter()
        .take_while(|unit| matches!(unit, 95 | 97..=122))
        .count();
    if length == 0 || line.get(length) != Some(&58) {
        return None;
    }
    let suffix = line.get(length + 1..)?;
    let value = if suffix.is_empty() {
        None
    } else if suffix.first() == Some(&32)
        && !suffix.iter().any(|unit| matches!(unit, 0x2028 | 0x2029))
    {
        Some(&suffix[1..])
    } else {
        return None;
    };
    Some((Text(line[..length].to_vec()).to_utf8().ok()?, value))
}

fn object() -> Value {
    Value::Object(Vec::new())
}

fn raw_section(config: &Value, name: &str) -> Result<Value, Error> {
    config
        .get(name)
        .cloned()
        .ok_or_else(|| invalid("missing configuration mapping"))
}

fn parse_sections(source: &Text) -> Result<(Value, HashSet<String>), Error> {
    if source.utf8_len() > 65_536 {
        return Err(invalid("expected UTF-8 text of at most 64 KiB"));
    }
    let mut config = Value::object(vec![
        ("llm", object()),
        ("profiles", object()),
        ("planner", object()),
        ("seat", object()),
        ("paths", object()),
        ("publish", object()),
        ("tools", object()),
        ("reviewer", object()),
        ("review", object()),
        ("loop", object()),
        ("context", object()),
        ("start", object()),
    ]);
    let mut roots = HashSet::new();
    let mut section: Option<String> = None;
    let mut profile: Option<String> = None;
    let normalized = source.normalized_lines();
    for (index, original) in normalized.0.split(|unit| *unit == 10).enumerate() {
        let line_number = index + 1;
        if original
            .iter()
            .any(|unit| matches!(unit, 0..=9 | 11..=31 | 127))
        {
            return Err(invalid(format!(
                "unsupported character on line {line_number}"
            )));
        }
        let line = uncomment(original);
        if line.is_empty() {
            continue;
        }
        if line.starts_with(&[32, 32, 32, 32]) {
            let failure = || {
                invalid(format!(
                    "unsupported or duplicate profile field on line {line_number}"
                ))
            };
            let (name, value) = mapping_field(line, 4).ok_or_else(failure)?;
            let profile_name = profile
                .as_deref()
                .filter(|_| section.as_deref() == Some("profiles"))
                .ok_or_else(failure)?;
            if !["base_url", "api_key_env", "api_key_optional"].contains(&name.as_str()) {
                return Err(failure());
            }
            let mut profiles = raw_section(&config, "profiles")?;
            let mut values = profiles.get(profile_name).cloned().ok_or_else(failure)?;
            if values.get(&name).is_some() {
                return Err(failure());
            }
            put(&mut values, &name, Value::String(Text(value.to_vec())))?;
            put(&mut profiles, profile_name, values)?;
            put(&mut config, "profiles", profiles)?;
            continue;
        }
        if line.starts_with(&[32, 32]) {
            if section.as_deref() == Some("profiles") {
                let failure = || {
                    invalid(format!(
                        "unsupported or duplicate profile on line {line_number}"
                    ))
                };
                let name = PROFILE_NAMES
                    .iter()
                    .find(|name| ascii(&line[2..], &format!("{name}:")))
                    .ok_or_else(failure)?;
                let mut profiles = raw_section(&config, "profiles")?;
                if profiles.get(name).is_some() {
                    return Err(failure());
                }
                put(&mut profiles, name, object())?;
                put(&mut config, "profiles", profiles)?;
                profile = Some((*name).to_owned());
                continue;
            }
            let failure = || {
                invalid(format!(
                    "unsupported or duplicate field on line {line_number}"
                ))
            };
            let (name, value) = mapping_field(line, 2).ok_or_else(failure)?;
            let section_name = section.as_deref().ok_or_else(failure)?;
            if !section_fields(section_name).is_some_and(|fields| fields.contains(&name.as_str())) {
                return Err(failure());
            }
            let mut values = raw_section(&config, section_name)?;
            if values.get(&name).is_some() {
                return Err(failure());
            }
            put(&mut values, &name, Value::String(Text(value.to_vec())))?;
            put(&mut config, section_name, values)?;
            continue;
        }
        let failure = || {
            invalid(format!(
                "unsupported or duplicate field on line {line_number}"
            ))
        };
        let (name, value) = root_field(line).ok_or_else(failure)?;
        if roots.contains(&name)
            || !matches!(name.as_str(), "schema" | "profiles") && section_fields(&name).is_none()
        {
            return Err(failure());
        }
        roots.insert(name.clone());
        section = (name != "schema").then_some(name.clone());
        profile = None;
        if name == "schema" {
            if !value.is_some_and(|value| ascii(value, "1")) {
                return Err(invalid("schema must be 1"));
            }
        } else if value.is_some() {
            return Err(invalid(format!("{name} must be a mapping")));
        }
    }
    for (section, fields) in [
        (
            "llm",
            &["base_url", "model", "api_key_env", "effort", "context_max"][..],
        ),
        ("seat", &["id", "principal", "turn_budget", "tools"][..]),
        ("paths", &["memory", "skills", "asks", "worktrees"][..]),
    ] {
        if !roots.contains("schema")
            || !roots.contains(section)
            || fields.iter().any(|field| {
                config
                    .get(section)
                    .and_then(|value| value.get(field))
                    .is_none()
            })
        {
            return Err(invalid(
                "schema, llm, seat, paths, and optional planner must contain every documented field",
            ));
        }
    }
    if roots.contains("planner")
        && config
            .get("planner")
            .and_then(|value| value.get("turn_budget"))
            .is_none()
    {
        return Err(invalid(
            "schema, llm, seat, paths, and optional planner must contain every documented field",
        ));
    }
    Ok((config, roots))
}

fn string_value(value: Option<&Value>, name: &str) -> Result<Text, Error> {
    let failure = || invalid(format!("{name} must be a plain or double-quoted string"));
    let text = value.and_then(Value::as_string).ok_or_else(failure)?;
    if text.0.first() == Some(&34) {
        return parse_json_string(&text.0).map_err(|_| failure());
    }
    if text.0.is_empty()
        || !text.0.iter().all(|unit| {
            matches!(
                unit,
                45..=47 | 48..=58 | 64..=90 | 92 | 95 | 97..=122
            )
        })
    {
        return Err(failure());
    }
    Ok(text.clone())
}

fn integer_value(value: Option<&Value>, name: &str) -> Result<Value, Error> {
    let failure = || invalid(format!("{name} must be a nonnegative safe integer"));
    let units = &value.and_then(Value::as_string).ok_or_else(failure)?.0;
    if units.is_empty()
        || units.len() > 1 && units.first() == Some(&48)
        || !units.iter().all(|unit| (48..=57).contains(unit))
    {
        return Err(failure());
    }
    let mut number = 0_u64;
    for &unit in units {
        number = number
            .checked_mul(10)
            .and_then(|number| number.checked_add(u64::from(unit - 48)))
            .filter(|number| *number <= MAX_SAFE_INTEGER)
            .ok_or_else(failure)?;
    }
    Ok(Value::Number(number as f64))
}

fn boolean_value(value: Option<&Value>, name: &str) -> Result<Value, Error> {
    if is_text(value, "true") {
        Ok(Value::Boolean(true))
    } else if is_text(value, "false") {
        Ok(Value::Boolean(false))
    } else {
        Err(invalid(format!("{name} must be true or false")))
    }
}

fn parse_string_field(values: &mut Value, name: &str, section: &str) -> Result<Text, Error> {
    let text = string_value(values.get(name), &format!("{section}.{name}"))?;
    put(values, name, Value::String(text.clone()))?;
    Ok(text)
}

fn parse_integer_field(values: &mut Value, name: &str, section: &str) -> Result<f64, Error> {
    let value = integer_value(values.get(name), &format!("{section}.{name}"))?;
    let number = value
        .as_number()
        .ok_or_else(|| invalid("expected a parsed integer"))?;
    put(values, name, value)?;
    Ok(number)
}

fn parse_boolean_field(values: &mut Value, name: &str, section: &str) -> Result<(), Error> {
    let value = boolean_value(values.get(name), &format!("{section}.{name}"))?;
    put(values, name, value)
}

fn default_profiles() -> Value {
    Value::object(vec![
        (
            "vllm-local",
            Value::object(vec![
                ("base_url", Value::string("http://127.0.0.1:8000/v1")),
                ("api_key_env", Value::string("ROSTER_API_KEY")),
                ("api_key_optional", Value::string("true")),
            ]),
        ),
        (
            "ollama",
            Value::object(vec![
                ("base_url", Value::string("http://127.0.0.1:11434/v1")),
                ("api_key_env", Value::string("ROSTER_API_KEY")),
            ]),
        ),
        (
            "lmstudio",
            Value::object(vec![
                ("base_url", Value::string("http://127.0.0.1:1234/v1")),
                ("api_key_env", Value::string("ROSTER_API_KEY")),
            ]),
        ),
        (
            "openai",
            Value::object(vec![
                ("base_url", Value::string("https://api.openai.com/v1")),
                ("api_key_env", Value::string("OPENAI_API_KEY")),
            ]),
        ),
    ])
}

fn api_key_name(value: &Text, name: &str) -> Result<(), Error> {
    let valid = value
        .0
        .first()
        .is_some_and(|unit| matches!(unit, 65..=90 | 95 | 97..=122))
        && value
            .0
            .iter()
            .all(|unit| matches!(unit, 48..=57 | 65..=90 | 95 | 97..=122));
    let app = value
        .0
        .get(..11)
        .is_some_and(|prefix| String::from_utf16_lossy(prefix).eq_ignore_ascii_case("GITHUB_APP_"));
    if !valid || app {
        return Err(invalid(format!(
            "{name} must name an LLM environment variable, not a GitHub App credential"
        )));
    }
    Ok(())
}

pub fn base_url(value: &Text, name: &str) -> Result<Url, Error> {
    let failure = || {
        invalid(format!(
            "{name} must be an HTTP(S) URL without credentials, query, or fragment"
        ))
    };
    let url = Url::parse(&String::from_utf16_lossy(&value.0)).map_err(|_| failure())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some_and(|value| !value.is_empty())
        || url.query().is_some_and(|value| !value.is_empty())
        || url.fragment().is_some_and(|value| !value.is_empty())
    {
        return Err(failure());
    }
    Ok(url)
}

pub fn validate_base_url(value: &Text, name: &str) -> Result<Text, Error> {
    base_url(value, name)?;
    Ok(value.clone())
}

fn validate_llm(config: &Value, roots: &HashSet<String>) -> Result<(Value, Value), Error> {
    let mut llm = raw_section(config, "llm")?;
    let mut url = parse_string_field(&mut llm, "base_url", "llm")?;
    let model = parse_string_field(&mut llm, "model", "llm")?;
    let key = parse_string_field(&mut llm, "api_key_env", "llm")?;
    api_key_name(&key, "llm.api_key_env")?;
    let effort = parse_string_field(&mut llm, "effort", "llm")?;
    if llm.get("effort_override").is_some() {
        let value = parse_string_field(&mut llm, "effort_override", "llm")?;
        if !EFFORTS.iter().any(|effort| ascii(&value.0, effort)) {
            return Err(invalid("llm.effort_override must be l, m, h, x, or none"));
        }
    }
    parse_integer_field(&mut llm, "context_max", "llm")?;
    if llm.get("max_requests").is_some() {
        let value = parse_integer_field(&mut llm, "max_requests", "llm")?;
        if !(1.0..=10000.0).contains(&value) {
            return Err(invalid(
                "llm.max_requests must be an integer from 1 to 10000",
            ));
        }
    }
    if llm.get("request_timeout_ms").is_some() {
        let value = parse_integer_field(&mut llm, "request_timeout_ms", "llm")?;
        request::validate_timeout(value, "llm.request_timeout_ms")
            .map_err(|error| invalid(error.message))?;
    }
    let selected = if llm.get("profile").is_some() {
        parse_string_field(&mut llm, "profile", "llm")?
    } else {
        put(&mut llm, "profile", Value::string(""))?;
        Text::default()
    };
    if llm.get("served_model_label").is_some() {
        let value = parse_string_field(&mut llm, "served_model_label", "llm")?;
        if !["trust", "ignore"]
            .iter()
            .any(|label| ascii(&value.0, label))
        {
            return Err(invalid("llm.served_model_label must be trust or ignore"));
        }
    }
    if llm.get("api_key_optional").is_some() {
        parse_boolean_field(&mut llm, "api_key_optional", "llm")?;
    }
    if llm.get("provider").is_some() {
        let value = parse_string_field(&mut llm, "provider", "llm")?;
        if value
            .to_utf8()
            .ok()
            .and_then(|value| Provider::parse(&value))
            .is_none()
        {
            return Err(invalid("llm.provider must name a supported model backend"));
        }
    }
    let profile_name = selected.to_utf8().map_err(|_| {
        invalid("llm.profile must be vllm-local, ollama, lmstudio, openai, or empty")
    })?;
    if !profile_name.is_empty() && !PROFILE_NAMES.contains(&profile_name.as_str()) {
        return Err(invalid(
            "llm.profile must be vllm-local, ollama, lmstudio, openai, or empty",
        ));
    }
    let defaults = default_profiles();
    let mut profiles = if roots.contains("profiles") {
        raw_section(config, "profiles")?
    } else {
        defaults.clone()
    };
    if profiles.get("vllm-local").is_none() {
        put(
            &mut profiles,
            "vllm-local",
            defaults
                .get("vllm-local")
                .cloned()
                .ok_or_else(|| invalid("missing vllm-local default"))?,
        )?;
    }
    for name in PROFILE_NAMES {
        let profile = profiles.get(name);
        if ["base_url", "api_key_env"]
            .iter()
            .any(|field| profile.and_then(|value| value.get(field)).is_none())
            || *name == "vllm-local"
                && profile
                    .and_then(|value| value.get("api_key_optional"))
                    .is_none()
        {
            return Err(invalid(
                "profiles need base_url and api_key_env for every named profile, plus api_key_optional for vllm-local",
            ));
        }
    }
    for name in PROFILE_NAMES {
        let mut values = profiles
            .get(name)
            .cloned()
            .ok_or_else(|| invalid("missing named profile"))?;
        let label = format!("profiles.{name}");
        let value = parse_string_field(&mut values, "base_url", &label)?;
        validate_base_url(&value, &format!("{label}.base_url"))?;
        let value = parse_string_field(&mut values, "api_key_env", &label)?;
        api_key_name(&value, &format!("{label}.api_key_env"))?;
        if values.get("api_key_optional").is_some() {
            parse_boolean_field(&mut values, "api_key_optional", &label)?;
        }
        put(&mut profiles, name, values)?;
    }
    if !profile_name.is_empty() {
        if !url.0.is_empty() && profile_name != "vllm-local" {
            return Err(invalid("choose either llm.profile or llm.base_url"));
        }
        let selected = profiles
            .get(&profile_name)
            .ok_or_else(|| invalid("missing selected profile"))?;
        if url.0.is_empty() {
            url = selected
                .get("base_url")
                .and_then(Value::as_string)
                .cloned()
                .ok_or_else(|| invalid("missing selected profile base_url"))?;
            put(&mut llm, "base_url", Value::String(url.clone()))?;
        }
        put(
            &mut llm,
            "api_key_env",
            selected
                .get("api_key_env")
                .cloned()
                .ok_or_else(|| invalid("missing selected profile api_key_env"))?,
        )?;
        if llm.get("api_key_optional").is_none() {
            if let Some(value) = selected.get("api_key_optional") {
                put(&mut llm, "api_key_optional", value.clone())?;
            }
        }
    }
    if !EFFORTS.iter().any(|value| ascii(&effort.0, value)) {
        return Err(invalid("llm.effort must be l, m, h, x, or none"));
    }
    if !model.0.iter().all(|unit| {
        matches!(
            unit,
            45..=47 | 48..=58 | 65..=90 | 95 | 97..=122
        )
    }) {
        return Err(invalid("llm.model must be a model name without whitespace"));
    }
    if !url.0.is_empty() {
        validate_base_url(&url, "llm.base_url")?;
    }
    Ok((llm, profiles))
}

fn relative_path(value: &Text, name: &str) -> Result<Text, Error> {
    let failure = || {
        invalid(format!(
            "{name} must be a relative path inside the roster repository"
        ))
    };
    let text = value.to_utf8().map_err(|_| failure())?;
    let parts = text.split(['/', '\\']).collect::<Vec<_>>();
    if Path::new(&text).is_absolute()
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|unit| unit.is_ascii_alphanumeric() || matches!(unit, b'_' | b'.' | b'-'))
                || matches!(*part, "." | "..")
                || part.eq_ignore_ascii_case(".git")
                || part.eq_ignore_ascii_case(".env")
        })
    {
        return Err(failure());
    }
    Ok(Text::from(parts.join(MAIN_SEPARATOR_STR).as_str()))
}

fn valid_profile_id(value: &Text) -> bool {
    (1..=64).contains(&value.0.len())
        && value
            .0
            .first()
            .is_some_and(|unit| matches!(unit, 48..=57 | 65..=90 | 97..=122))
        && value
            .0
            .iter()
            .all(|unit| matches!(unit, 45 | 46 | 48..=57 | 65..=90 | 95 | 97..=122))
}

pub fn validate_start_base(value: &Text, name: &str) -> Result<(), Error> {
    if value.0.is_empty()
        || value.0.len() > 200
        || value.0.first() == Some(&45)
        || matches!(value.0.last(), Some(46 | 47))
        || !value
            .0
            .iter()
            .all(|unit| matches!(unit, 45..=57 | 65..=90 | 95 | 97..=122))
        || value
            .0
            .windows(2)
            .any(|pair| pair == [46, 46] || pair == [47, 47] || pair == [64, 123])
    {
        return Err(Error::type_error(format!(
            "{name} must be trunk, current, or a Git ref name"
        )));
    }
    Ok(())
}

pub fn parse(source: &Text) -> Result<Config, Error> {
    let (mut config, roots) = parse_sections(source)?;
    let (llm, profiles) = validate_llm(&config, &roots)?;
    put(&mut config, "llm", llm)?;
    put(&mut config, "profiles", profiles)?;
    let mut seat = raw_section(&config, "seat")?;
    let evidence = if seat.get("evidence_workspace").is_some() {
        boolean_value(seat.get("evidence_workspace"), "seat.evidence_workspace")?
    } else {
        Value::Boolean(false)
    };
    put(&mut seat, "evidence_workspace", evidence)?;
    parse_string_field(&mut seat, "id", "seat")?;
    parse_string_field(&mut seat, "principal", "seat")?;
    let turns = parse_integer_field(&mut seat, "turn_budget", "seat")?;
    let context = if seat.get("context_chars").is_some() {
        integer_value(seat.get("context_chars"), "seat.context_chars")?
    } else {
        Value::Number(200000.0)
    };
    if context.as_number().is_none_or(|number| number < 1.0) {
        return Err(invalid("seat.context_chars must be positive"));
    }
    put(&mut seat, "context_chars", context)?;
    let scope = if seat.get("scope_expansion").is_some() {
        integer_value(seat.get("scope_expansion"), "seat.scope_expansion")?
    } else {
        Value::Number(3.0)
    };
    if scope.as_number().is_none_or(|number| number > 16.0) {
        return Err(invalid(
            "seat.scope_expansion must be 0-16 files (0 is strict scope)",
        ));
    }
    put(&mut seat, "scope_expansion", scope)?;
    if !is_text(seat.get("id"), "coder")
        || !is_text(seat.get("principal"), "coder")
        || !(1.0..=10000.0).contains(&turns)
    {
        return Err(invalid(
            "seat must be coder with principal coder and turn_budget between 1 and 10000",
        ));
    }
    let tools = seat
        .get("tools")
        .and_then(Value::as_string)
        .filter(|text| {
            text.0.first() == Some(&91)
                && text.0.last() == Some(&93)
                && text
                    .0
                    .get(1..text.0.len() - 1)
                    .is_some_and(|body| !body.iter().any(|unit| matches!(unit, 91 | 93)))
        })
        .ok_or_else(|| invalid("seat.tools must be an inline list of builtin tools"))?;
    let tool_list = tools.0[1..tools.0.len() - 1]
        .split(|unit| *unit == 44)
        .map(|value| Text(trim(value).to_vec()))
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    if tool_list
        .iter()
        .any(|tool| !seen.insert(tool.clone()) || !TOOLS.iter().any(|name| ascii(&tool.0, name)))
    {
        return Err(invalid(
            "seat.tools may only contain distinct read_file, write_file, edit_file, glob_files, list_dir, run_test, run_command, search_text, web_search, web_fetch tools",
        ));
    }
    put(
        &mut seat,
        "tools",
        Value::Array(tool_list.into_iter().map(Value::String).collect()),
    )?;
    let mut planner = raw_section(&config, "planner")?;
    let turns = if roots.contains("planner") {
        integer_value(planner.get("turn_budget"), "planner.turn_budget")?
    } else {
        Value::Number(1.0)
    };
    if turns
        .as_number()
        .is_none_or(|number| !(1.0..=10000.0).contains(&number))
    {
        return Err(invalid("planner.turn_budget must be between 1 and 10000"));
    }
    put(&mut planner, "turn_budget", turns)?;
    if planner
        .get("critic_profile")
        .and_then(Value::as_string)
        .is_some_and(|value| !valid_profile_id(value))
    {
        return Err(invalid("planner.critic_profile must name a fleet profile"));
    }
    if roots.contains("loop") {
        let mut value = raw_section(&config, "loop")?;
        let turns = parse_integer_field(&mut value, "turns", "loop")?;
        if !(1.0..=10000.0).contains(&turns) {
            return Err(invalid("loop.turns must be between 1 and 10000"));
        }
        put(&mut seat, "turn_budget", Value::Number(turns))?;
        put(&mut config, "loop", value)?;
    }
    let attempts = if seat.get("max_attempts").is_some() {
        integer_value(seat.get("max_attempts"), "seat.max_attempts")?
    } else {
        Value::Number(3.0)
    };
    if attempts
        .as_number()
        .is_none_or(|number| !(1.0..=16.0).contains(&number))
    {
        return Err(invalid("seat.max_attempts must be between 1 and 16"));
    }
    put(&mut seat, "max_attempts", attempts)?;
    if seat.get("task_timeout_ms").is_some() {
        let timeout = parse_integer_field(&mut seat, "task_timeout_ms", "seat")?;
        cancel::validate_task_timeout(Some(timeout)).map_err(|error| invalid(error.message))?;
    }
    if roots.contains("context") {
        let mut value = raw_section(&config, "context")?;
        let budget = parse_integer_field(&mut value, "budget", "context")?;
        if budget < 1.0 {
            return Err(invalid("context.budget must be positive"));
        }
        put(&mut seat, "context_chars", Value::Number(budget))?;
        put(&mut config, "context", value)?;
    }
    let mut paths = raw_section(&config, "paths")?;
    for name in ["memory", "skills", "asks", "worktrees"] {
        let label = format!("paths.{name}");
        let value = string_value(paths.get(name), &label)?;
        put(
            &mut paths,
            name,
            Value::String(relative_path(&value, &label)?),
        )?;
    }
    if !paths
        .get("memory")
        .and_then(Value::as_string)
        .is_some_and(|value| value.0.ends_with(&Text::from(".jsonl").0))
    {
        return Err(invalid("paths.memory must be a .jsonl file"));
    }
    for (section, defaults) in [
        ("publish", &[("enabled", true)][..]),
        ("tools", &[("internet", false), ("run_test", true)][..]),
        ("reviewer", &[("required", true)][..]),
    ] {
        let mut values = raw_section(&config, section)?;
        for (name, fallback) in defaults {
            let value = if values.get(name).is_some() {
                boolean_value(values.get(name), &format!("{section}.{name}"))?
            } else {
                Value::Boolean(*fallback)
            };
            put(&mut values, name, value)?;
        }
        put(&mut config, section, values)?;
    }
    if roots.contains("review") {
        let mut review = raw_section(&config, "review")?;
        parse_boolean_field(&mut review, "required", "review")?;
        let required = review
            .get("required")
            .cloned()
            .ok_or_else(|| invalid("missing review requirement"))?;
        let mut reviewer = raw_section(&config, "reviewer")?;
        if roots.contains("reviewer") && reviewer.get("required") != Some(&required) {
            return Err(invalid(
                "review.required and legacy reviewer.required disagree",
            ));
        }
        put(&mut reviewer, "required", required)?;
        put(&mut config, "reviewer", reviewer)?;
        put(&mut config, "review", review)?;
    }
    if roots.contains("start") {
        let mut start = raw_section(&config, "start")?;
        if start.get("base").is_some() {
            let value = parse_string_field(&mut start, "base", "start")?;
            validate_start_base(&value, "start.base").map_err(|error| invalid(error.message))?;
        }
        if start.get("sync").is_some() {
            parse_string_field(&mut start, "sync", "start")?;
            if !is_text(start.get("sync"), "fetch") && !is_text(start.get("sync"), "offline") {
                return Err(invalid("start.sync must be fetch or offline"));
            }
        }
        put(&mut config, "start", start)?;
    }
    let mut result = Value::object(vec![("schema", Value::Number(1.0))]);
    put(&mut config, "planner", planner)?;
    put(&mut config, "seat", seat)?;
    put(&mut config, "paths", paths)?;
    for section in [
        "llm", "profiles", "planner", "seat", "paths", "publish", "tools", "reviewer",
    ] {
        put(&mut result, section, raw_section(&config, section)?)?;
    }
    for section in ["review", "loop", "context", "start"] {
        if roots.contains(section) {
            put(&mut result, section, raw_section(&config, section)?)?;
        }
    }
    Ok(Config(result))
}

fn mapping(values: &Value, indent: &str, output: &mut Text) -> Result<(), Error> {
    let values = domain::spread(values);
    let entries = members(&values)
        .ok_or_else(|| Error::type_error("Cannot convert undefined or null to object"))?;
    for (name, value) in entries {
        output.push_str(indent);
        output.0.extend_from_slice(&name.0);
        if matches!(value, Value::Object(_)) {
            output.push_str(":\n");
            mapping(value, &format!("{indent}  "), output)?;
        } else {
            output.push_str(": ");
            output.0.extend(domain::scalar(value)?.0);
            output.push_str("\n");
        }
    }
    Ok(())
}

pub fn format(config: &Value) -> Result<Text, Error> {
    let mut persisted = config.clone();
    if let Some(llm) = domain::property(Some(config), "llm")?.filter(|value| domain::truthy(value))
    {
        persisted = domain::spread(config);
        let mut llm = domain::spread(llm);
        for name in ["fleet_profile", "concurrency", "hardware"] {
            domain::remove(&mut llm, name)?;
        }
        put(&mut persisted, "llm", llm)?;
    }
    let mut source = Text::from("# Private settings. No credentials belong in this file.\n");
    mapping(&persisted, "", &mut source)?;
    parse(&source)?;
    Ok(source)
}

pub fn read_source(file: &Path) -> Result<Text, Error> {
    settings::read_config_file(file)
}

pub fn read_private_source(root: &Path, name: settings::Name) -> Result<Option<Text>, Error> {
    settings::read_private_file(root, name)
}

pub fn read(file: &Path) -> Result<Config, Error> {
    parse(&read_source(file)?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Defaults,
    Private(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub root: PathBuf,
    pub source: Source,
    pub retention: Value,
    target: PathBuf,
    seal: Option<(Text, crate::paths::FileIdentity)>,
    critic: Option<crate::fleet::Profile>,
    critic_seal: Option<(Text, crate::paths::FileIdentity)>,
}

impl Loaded {
    pub fn require_current(&self) -> Result<(), Error> {
        if private_config(&self.root)? != self.seal
            || self.target != self.root && private_config(&self.target)?.is_some()
            || self.critic.is_some()
                && private_settings(&self.root, settings::Name::Fleet)? != self.critic_seal
        {
            return Err(Error::new(
                "StaleConfiguration",
                "Private configuration or its target-project precedence changed after invocation loading; stop and reload without adopting new authority",
            ).coded("ROSTER_STALE_CONFIG"));
        }
        Ok(())
    }

    pub fn critic_profile(&self) -> Option<&crate::fleet::Profile> {
        self.critic.as_ref()
    }
}

pub fn defaults() -> Result<Config, Error> {
    parse(&Text::from(include_str!("../roster.config.example.yml")))
}

pub(crate) fn shared_config_root(root: &Path) -> Result<PathBuf, Error> {
    let root = crate::paths::resolve_project_root(root)?;
    crate::state::secure::check_components(&root)?;
    match std::fs::symlink_metadata(root.join(".git")) {
        Ok(_) => Ok(crate::paths::repository_paths(&root)?.repository),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(root),
        Err(error) => Err(crate::state::secure::io_error(
            "Inspect configuration repository",
            &root,
            error,
        )),
    }
}

pub(crate) fn private_settings(
    root: &Path,
    name: settings::Name,
) -> Result<Option<(Text, crate::paths::FileIdentity)>, Error> {
    use crate::state::secure;

    let directory = root.join(".roster");
    if !secure::private_directory_if_present(&directory)? {
        return Ok(None);
    }
    let file = directory.join(name.as_str());
    let Some(snapshot) = secure::read(&file, 65_536)? else {
        return Ok(None);
    };
    if crate::paths::file_information(&file)?.links != 1 {
        return Err(invalid(format!(
            "private {} must be a single-link regular file",
            name.as_str()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::symlink_metadata(&file)
            .map_err(|error| secure::io_error("Inspect private configuration", &file, error))?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid(format!(
                "private {} permissions must not exceed 0600",
                name.as_str()
            )));
        }
    }
    let source = String::from_utf8(snapshot.bytes.clone())
        .map_err(|_| invalid(format!("private {} must be UTF-8", name.as_str())))?;
    if !secure::unchanged(&file, &snapshot, 65_536)? {
        return Err(invalid(format!(
            "private {} changed during configuration loading",
            name.as_str()
        )));
    }
    Ok(Some((Text::from(source.as_str()), snapshot.identity)))
}

fn private_config(root: &Path) -> Result<Option<(Text, crate::paths::FileIdentity)>, Error> {
    private_settings(root, settings::Name::Config)
}

fn installation_config_root(installation: &Path) -> Result<PathBuf, Error> {
    let installation = crate::paths::absolute(installation)?;
    crate::state::secure::check_components(&installation)?;
    match std::fs::symlink_metadata(installation.join(".git")) {
        Ok(_) => Ok(crate::paths::repository_paths(&installation)?.repository),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(installation),
        Err(error) => Err(crate::state::secure::io_error(
            "Inspect installation configuration",
            &installation,
            error,
        )),
    }
}

pub fn resolve_config_root(installation: &Path, cwd: &Path) -> Result<PathBuf, Error> {
    let project = shared_config_root(cwd)?;
    if private_config(&project)?.is_some() {
        return Ok(project);
    }
    installation_config_root(installation)
}

pub fn environment_value<'a>(
    environment: &'a Value,
    name: &str,
) -> Result<Option<&'a Text>, Error> {
    environment_entry(environment, name)?
        .map(|value| {
            value
                .as_string()
                .filter(|value| !trim(&value.0).is_empty())
                .ok_or_else(|| invalid(format!("{name} must be nonblank text when set")))
        })
        .transpose()
}

pub(crate) fn environment_entry<'a>(
    environment: &'a Value,
    name: &str,
) -> Result<Option<&'a Value>, Error> {
    if !cfg!(windows) {
        return Ok(environment.get(name));
    }
    let mut matches = domain::members(environment)
        .unwrap_or(&[])
        .iter()
        .filter(|(key, _)| {
            key.to_utf8()
                .is_ok_and(|key| key.eq_ignore_ascii_case(name))
        });
    let selected = matches.next().map(|(_, value)| value);
    if matches.next().is_some() {
        return Err(invalid(format!(
            "{name} has ambiguous Windows environment aliases"
        )));
    }
    Ok(selected)
}

fn invocation_overrides(config: Config, environment: &Value) -> Result<Config, Error> {
    let mut value = config.0;
    for (name, section, field) in [
        ("ROSTER_LLM_BASE_URL", "llm", "base_url"),
        ("ROSTER_LLM_MODEL", "llm", "model"),
        ("ROSTER_LLM_API_KEY_ENV", "llm", "api_key_env"),
        ("ROSTER_LLM_REQUEST_TIMEOUT_MS", "llm", "request_timeout_ms"),
        ("ROSTER_TASK_TIMEOUT_MS", "seat", "task_timeout_ms"),
        ("ROSTER_START_BASE", "start", "base"),
        ("ROSTER_START_SYNC", "start", "sync"),
    ] {
        let Some(raw) = environment_value(environment, name)? else {
            continue;
        };
        let replacement = match field {
            "base_url" => {
                validate_base_url(raw, name)?;
                Value::String(raw.clone())
            }
            "model" => {
                if !raw
                    .0
                    .iter()
                    .all(|unit| matches!(unit, 45..=47 | 48..=58 | 65..=90 | 95 | 97..=122))
                {
                    return Err(invalid(format!(
                        "{name} must be a model name without whitespace"
                    )));
                }
                Value::String(raw.clone())
            }
            "api_key_env" => {
                api_key_name(raw, name)?;
                Value::String(raw.clone())
            }
            "request_timeout_ms" | "task_timeout_ms" => {
                let parsed = integer_value(Some(&Value::String(raw.clone())), name)?;
                let milliseconds = parsed.as_number().ok_or_else(|| invalid(name))?;
                if field == "task_timeout_ms" {
                    cancel::validate_task_timeout(Some(milliseconds)).map_err(|_| {
                        invalid(format!(
                            "{name} must be an integer between 1 and 2147483647"
                        ))
                    })?;
                } else {
                    request::validate_timeout(milliseconds, name).map_err(|_| {
                        invalid(format!(
                            "{name} is outside the supported request timeout range"
                        ))
                    })?;
                }
                parsed
            }
            "base" => {
                validate_start_base(raw, name)?;
                Value::String(raw.clone())
            }
            "sync" => {
                if !ascii(&raw.0, "fetch") && !ascii(&raw.0, "offline") {
                    return Err(invalid(format!("{name} must be fetch or offline")));
                }
                Value::String(raw.clone())
            }
            _ => return Err(invalid("Unsupported invocation override")),
        };
        let mut mapping = value.get(section).cloned().unwrap_or_else(object);
        put(&mut mapping, field, replacement)?;
        if name == "ROSTER_LLM_BASE_URL" {
            put(&mut mapping, "profile", Value::string(""))?;
        }
        put(&mut value, section, mapping)?;
    }
    let configured = value
        .get("llm")
        .and_then(|llm| llm.get("base_url"))
        .and_then(Value::as_string)
        .is_some_and(|value| !value.0.is_empty());
    if configured
        && value
            .get("llm")
            .and_then(|llm| llm.get("model"))
            .and_then(Value::as_string)
            .is_none_or(|value| value.0.is_empty())
    {
        return Err(invalid(
            "A configured endpoint requires llm.model or ROSTER_LLM_MODEL; no model is silently selected",
        ));
    }
    Ok(Config(value))
}

pub fn load(installation: &Path, cwd: &Path, environment: &Value) -> Result<Loaded, Error> {
    if !matches!(environment, Value::Object(_)) {
        return Err(invalid("Invocation environment must be an object"));
    }
    let target = shared_config_root(cwd)?;
    let (root, seal) = match private_config(&target)? {
        Some(source) => (target.clone(), Some(source)),
        None => {
            let root = installation_config_root(installation)?;
            let source = private_config(&root)?;
            (root, source)
        }
    };
    let (config, source) = match &seal {
        Some((source, _)) => (
            parse(source)?,
            Source::Private(root.join(".roster").join("config.yml")),
        ),
        None => (defaults()?, Source::Defaults),
    };
    let config = invocation_overrides(config, environment)?;
    let (critic, critic_seal) = match config
        .as_value()
        .get("planner")
        .and_then(|value| value.get("critic_profile"))
    {
        Some(name) => {
            let source = private_settings(&root, settings::Name::Fleet)?.ok_or_else(|| invalid(
                "planner.critic_profile requires a private fleet.yml containing that exact profile",
            ))?;
            let fleet = crate::fleet::parse(&source.0)?;
            let selected = crate::fleet::get_profile(&fleet, name)?.clone();
            (Some(selected), Some(source))
        }
        None => (None, None),
    };
    let mut retention_environment = object();
    for name in [
        "ROSTER_RETENTION_OPT_OUT",
        "ROSTER_RETENTION_WINDOW_MS_MACHINE",
        "ROSTER_RETENTION_WINDOW_MS_REPO",
        "ROSTER_RETENTION_WINDOW_MS_WORKTREE",
        "ROSTER_RETENTION_WINDOW_MS_RUN",
    ] {
        if let Some(value) = environment_entry(environment, name)? {
            put(&mut retention_environment, name, value.clone())?;
        }
    }
    Ok(Loaded {
        config,
        root,
        source,
        retention: retention(&retention_environment)?,
        target,
        seal,
        critic,
        critic_seal,
    })
}

pub fn require_publication_enabled(config: &Value) -> Result<(), Error> {
    if domain::property(Some(config), "publish")?.and_then(|value| value.get("enabled"))
        == Some(&Value::Boolean(false))
    {
        return Err(Error::new(
            "Error",
            "Publishing is disabled by publish.enabled; update the private config or rerun roster onboard",
        ));
    }
    Ok(())
}

pub fn is_review_required(config: &Value) -> Result<bool, Error> {
    Ok(domain::property(Some(config), "review")?
        .and_then(|value| value.get("required"))
        .filter(|value| **value != Value::Null)
        .or_else(|| {
            config
                .get("reviewer")
                .and_then(|value| value.get("required"))
        })
        != Some(&Value::Boolean(false)))
}

pub fn without_llm_keys(env: &Value, config: &Value) -> Result<Value, Error> {
    let mut clean = domain::spread(env);
    let llm = domain::property(Some(config), "llm")?;
    let mut names = Vec::new();
    if let Some(name) = domain::property(llm, "api_key_env")?.and_then(Value::as_string) {
        names.push(name.clone());
    }
    if let Some(profiles) = config
        .get("profiles")
        .filter(|value| **value != Value::Null)
    {
        let profiles = domain::spread(profiles);
        for (_, profile) in members(&profiles)
            .ok_or_else(|| Error::type_error("Native profiles must be an object"))?
        {
            if let Some(name) =
                domain::property(Some(profile), "api_key_env")?.and_then(Value::as_string)
            {
                names.push(name.clone());
            }
        }
    }
    for name in names.iter().filter(|name| !name.0.is_empty()) {
        let Value::Object(entries) = &mut clean else {
            return Err(Error::type_error("Native environment must be an object"));
        };
        entries.retain(|(key, _)| {
            if cfg!(windows) {
                match (key.to_utf8(), name.to_utf8()) {
                    (Ok(k), Ok(n)) => !k.eq_ignore_ascii_case(&n),
                    _ => key != name,
                }
            } else {
                key != name
            }
        });
    }
    Ok(clean)
}

pub fn retention(env: &Value) -> Result<Value, Error> {
    if !matches!(env, Value::Object(_) | Value::Array(_)) {
        return Err(Error::new(
            "Error",
            "resolveRetentionConfig requires an env object",
        ));
    }
    let opt_out = match env.get("ROSTER_RETENTION_OPT_OUT") {
        None => false,
        Some(value) if is_text(Some(value), "true") => true,
        Some(value) if is_text(Some(value), "false") => false,
        Some(_) => {
            return Err(Error::new(
                "Error",
                "ROSTER_RETENTION_OPT_OUT must be \"true\" or \"false\" when set",
            ));
        }
    };
    let mut windows = object();
    for (scope, days) in [("machine", 90), ("repo", 14), ("worktree", 7), ("run", 1)] {
        let name = format!("ROSTER_RETENTION_WINDOW_MS_{}", scope.to_ascii_uppercase());
        let value = if let Some(raw) = env.get(&name) {
            integer_value(Some(raw), &name)
                .ok()
                .filter(|value| value.as_number().is_some_and(|value| value > 0.0))
                .ok_or_else(|| {
                    Error::new(
                        "Error",
                        format!("{name} must be a positive integer (ms) when set"),
                    )
                })?
        } else {
            Value::Number(f64::from(days) * 86_400_000.0)
        };
        put(&mut windows, scope, value)?;
    }
    Ok(Value::object(vec![
        ("optOut", Value::Boolean(opt_out)),
        ("windowsMs", windows),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../roster.config.example.yml");

    #[test]
    fn defaults_and_optional_presence_round_trip_without_fleet_state() -> Result<(), Error> {
        let parsed = parse(&Text::from(EXAMPLE))?;
        assert!(parsed.as_value().get("review").is_none());
        let mut value = parsed.as_value().clone();
        let mut llm = raw_section(&value, "llm")?;
        for (name, value) in [
            ("fleet_profile", Value::string("fixture")),
            ("concurrency", Value::Number(2.0)),
            ("hardware", Value::string("fixture-gpu")),
        ] {
            put(&mut llm, name, value)?;
        }
        put(&mut value, "llm", llm)?;
        assert_eq!(parse(&format(&value)?)?, parsed);
        Ok(())
    }

    #[test]
    fn malformed_overrides_cannot_become_defaults() {
        for (before, after) in [
            ("context_chars: 200000", "context_chars: 0"),
            ("turn_budget: 1000", "turn_budget: 10001"),
            ("effort: m", "effort: high"),
            ("model: \"\"", "model: \"bad model\""),
            ("skills: skills", "skills: ../outside"),
            ("enabled: true", "enabled: yes"),
        ] {
            assert!(parse(&Text::from(EXAMPLE.replace(before, after).as_str())).is_err());
        }
    }

    #[test]
    fn utf16_paths_and_strings_are_not_lossily_validated() -> Result<(), Error> {
        let source = EXAMPLE.replace("model: \"\"", "model: \"\\ud800\"");
        assert!(
            parse(&Text::from(source.as_str()))
                .is_err_and(|error| error.message.contains("model name without whitespace"))
        );
        let source = EXAMPLE.replace(
            "base_url: \"\"",
            "base_url: \"https://example.invalid/\\ud800\"",
        );
        let parsed = parse(&Text::from(source.as_str()))?;
        assert_eq!(parse(&format(parsed.as_value())?)?, parsed);
        Ok(())
    }
}
