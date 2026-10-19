use std::collections::HashSet;
use std::path::Path;

use crate::domain::{self, is_text, items, members, put};
use crate::json::Value;
use crate::text::{Text, ascii, trim};
use crate::{Error, catalog, config, request, run, settings};

const FIELDS: &[&str] = &[
    "id",
    "base_url",
    "model",
    "provider",
    "context_max",
    "concurrency",
    "hardware",
    "task_class",
    "notes",
    "api_key_env",
    "request_timeout_ms",
    "served_model_label",
];
const REQUIRED: &[&str] = &[
    "id",
    "base_url",
    "model",
    "provider",
    "context_max",
    "concurrency",
    "hardware",
    "notes",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskClass {
    Feat,
    Fix,
    Docs,
    Test,
}

impl TaskClass {
    pub fn parse(value: Option<&Value>) -> Option<Self> {
        match value.and_then(Value::as_string) {
            Some(value) if ascii(&value.0, "feat") => Some(Self::Feat),
            Some(value) if ascii(&value.0, "fix") => Some(Self::Fix),
            Some(value) if ascii(&value.0, "docs") => Some(Self::Docs),
            Some(value) if ascii(&value.0, "test") => Some(Self::Test),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feat => "feat",
            Self::Fix => "fix",
            Self::Docs => "docs",
            Self::Test => "test",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile(Value);

impl Profile {
    pub fn as_value(&self) -> &Value {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fleet {
    profiles: Vec<Profile>,
}

impl Fleet {
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    pub fn as_value(&self) -> Value {
        Value::object(vec![(
            "profiles",
            Value::Array(
                self.profiles
                    .iter()
                    .map(|profile| profile.0.clone())
                    .collect(),
            ),
        )])
    }
}

fn text(value: Option<&Value>, label: &str, empty: bool, maximum: usize) -> Result<Text, Error> {
    let failure = || {
        Error::type_error(format!(
            "Fleet {label} must be {}single-line text",
            if empty { "" } else { "nonempty " }
        ))
    };
    let value = value.and_then(Value::as_string).ok_or_else(failure)?;
    if (!empty && trim(&value.0).is_empty())
        || value.0 != trim(&value.0)
        || value.0.len() > maximum
        || value.0.iter().any(|unit| matches!(unit, 0..=31 | 127))
    {
        return Err(failure());
    }
    Ok(value.clone())
}

pub fn normalize_base_url(value: &Value) -> Result<Text, Error> {
    let text = text(Some(value), "base_url", false, 2048)?;
    let mut url = config::base_url(&text, "fleet base_url")?;
    let pathname = url.path().trim_end_matches('/');
    if pathname.ends_with("/models") || pathname.ends_with("/chat/completions") {
        return Err(Error::type_error(
            "Fleet base_url must be an API base, not a complete request endpoint",
        ));
    }
    let pathname = if pathname.ends_with("/v1") {
        pathname.to_owned()
    } else {
        format!("{pathname}/v1")
    };
    url.set_path(&pathname);
    Ok(Text::from(url.as_str()))
}

pub fn validate_id(value: Option<&Value>) -> Result<Text, Error> {
    let id = text(value, "id", false, 64)?;
    if !id
        .0
        .first()
        .is_some_and(|unit| matches!(unit, 48..=57 | 65..=90 | 97..=122))
        || !id
            .0
            .iter()
            .all(|unit| matches!(unit, 45 | 46 | 48..=57 | 65..=90 | 95 | 97..=122))
    {
        return Err(Error::type_error(
            "Fleet id must be an opaque identifier of at most 64 characters",
        ));
    }
    Ok(id)
}

pub(crate) fn actual_model(value: Option<&Value>) -> Result<Text, Error> {
    let model = value
        .and_then(Value::as_string)
        .filter(|model| {
            model
                .to_utf8()
                .is_ok_and(|value| run::is_model_id(&value))
        })
        .ok_or_else(|| {
            Error::type_error(
                "set model: configure llm.model, AI_MODEL, or ROSTER_MODEL with the actual model id (not unknown)",
            )
        })?;
    Ok(model.clone())
}

pub fn validate_profile(profile: &Value) -> Result<Profile, Error> {
    if members(profile).is_none()
        || REQUIRED.iter().any(|name| profile.get(name).is_none())
        || members(profile).is_some_and(|entries| {
            entries
                .iter()
                .any(|(name, _)| !FIELDS.iter().any(|field| ascii(&name.0, field)))
        })
    {
        return Err(Error::type_error(
            "Fleet profile must contain the documented fields",
        ));
    }
    let id = validate_id(profile.get("id"))?;
    let model = actual_model(profile.get("model"))?;
    if ascii(&model.0, "builtin-stub") {
        return Err(Error::type_error(
            "Fleet model must be an actual served model",
        ));
    }
    if !is_text(profile.get("provider"), "vllm") {
        return Err(Error::type_error("Fleet provider must be vllm"));
    }
    if domain::safe_integer(profile.get("context_max"))
        .is_none_or(|value| value == 0 && !ascii(&id.0, "default"))
    {
        return Err(Error::type_error(
            "Fleet context_max must be positive; only the onboard default may use 0 for unknown",
        ));
    }
    if domain::safe_integer(profile.get("concurrency")).is_none_or(|value| value == 0) {
        return Err(Error::type_error(
            "Fleet concurrency must be a positive safe integer",
        ));
    }
    if let Some(value) = profile.get("task_class") {
        let mut classes = HashSet::new();
        if items(value).is_none_or(|items| {
            items.is_empty()
                || items.iter().any(|item| {
                    TaskClass::parse(Some(item)).is_none_or(|class| !classes.insert(class))
                })
        }) {
            return Err(Error::type_error(
                "Fleet task_class must contain distinct feat, fix, docs, or test hints",
            ));
        }
    }
    if let Some(value) = profile.get("api_key_env") {
        let value = domain::string(value)?;
        if !(1..=64).contains(&value.0.len())
            || !value
                .0
                .first()
                .is_some_and(|unit| matches!(unit, 65..=90 | 95 | 97..=122))
            || !value
                .0
                .iter()
                .all(|unit| matches!(unit, 48..=57 | 65..=90 | 95 | 97..=122))
        {
            return Err(Error::type_error(
                "Fleet api_key_env must be a vault secret name",
            ));
        }
    }
    if let Some(timeout) = profile.get("request_timeout_ms") {
        let number = timeout.as_number().ok_or_else(|| {
            Error::type_error(
                "fleet request_timeout_ms must be an integer between 1 and 2147483647.",
            )
        })?;
        request::validate_timeout(number, "fleet request_timeout_ms")?;
    }
    if profile.get("served_model_label").is_some()
        && !is_text(profile.get("served_model_label"), "trust")
        && !is_text(profile.get("served_model_label"), "ignore")
    {
        return Err(Error::type_error(
            "Fleet served_model_label must be trust or ignore",
        ));
    }
    let url =
        normalize_base_url(profile.get("base_url").ok_or_else(|| {
            Error::type_error("Fleet profile must contain the documented fields")
        })?)?;
    let hardware = text(profile.get("hardware"), "hardware", false, 120)?;
    let mut result = Value::object(vec![
        ("id", Value::String(id)),
        ("base_url", Value::String(url)),
        ("model", Value::String(model)),
        ("provider", Value::string("vllm")),
        (
            "context_max",
            profile
                .get("context_max")
                .cloned()
                .ok_or_else(|| Error::type_error("Fleet context_max is required"))?,
        ),
        (
            "concurrency",
            profile
                .get("concurrency")
                .cloned()
                .ok_or_else(|| Error::type_error("Fleet concurrency is required"))?,
        ),
        ("hardware", Value::String(hardware)),
    ]);
    for name in [
        "task_class",
        "api_key_env",
        "request_timeout_ms",
        "served_model_label",
    ] {
        if let Some(value) = profile.get(name) {
            if name != "api_key_env" || domain::truthy(value) {
                put(&mut result, name, value.clone())?;
            }
        }
    }
    put(
        &mut result,
        "notes",
        Value::String(text(profile.get("notes"), "notes", true, 500)?),
    )?;
    Ok(Profile(result))
}

pub fn validate(catalog: &Value) -> Result<Fleet, Error> {
    let records = catalog
        .get("profiles")
        .and_then(items)
        .filter(|_| members(catalog).is_some_and(|entries| entries.len() == 1));
    let records = records
        .ok_or_else(|| Error::type_error("Fleet catalog must contain only a profiles list"))?;
    let profiles = records
        .iter()
        .map(validate_profile)
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = HashSet::new();
    for profile in &profiles {
        let id = profile
            .0
            .get("id")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Fleet id is required"))?;
        if !seen.insert(id) {
            return Err(Error::type_error("Fleet profile IDs must be unique"));
        }
    }
    Ok(Fleet { profiles })
}

pub fn parse(source: &Text) -> Result<Fleet, Error> {
    let records = domain::catalog_values(catalog::parse(source, "profiles", FIELDS)?);
    validate(&Value::object(vec![("profiles", Value::Array(records))]))
}

pub fn format(catalog: &Value) -> Result<Text, Error> {
    let fleet = validate(catalog)?;
    domain::format_catalog(
        "profiles",
        &fleet
            .profiles
            .into_iter()
            .map(|profile| profile.0)
            .collect::<Vec<_>>(),
    )
}

pub fn get_profile<'a>(catalog: &'a Fleet, id: &Value) -> Result<&'a Profile, Error> {
    let id = validate_id(Some(id))?;
    catalog
        .profiles
        .iter()
        .find(|profile| profile.0.get("id").and_then(Value::as_string) == Some(&id))
        .ok_or_else(|| {
            Error::new(
                "Error",
                "Fleet profile ID was not found in .roster/fleet.yml",
            )
        })
}

fn configured_key<'a>(config: &'a Value, name: &str) -> Result<Option<&'a Value>, Error> {
    let profiles = domain::property(Some(config), "profiles")?;
    let local = domain::property(profiles, "vllm-local")?;
    domain::property(local, name)
}

pub fn with_profile(config: &Value, profile: &Value) -> Result<Value, Error> {
    let selected = validate_profile(profile)?;
    let selected = selected.as_value();
    let inherited = domain::property(Some(config), "llm")?;
    let inherited = inherited
        .filter(|value| **value != Value::Null)
        .ok_or_else(|| {
            Error::type_error(format!(
                "Cannot destructure property 'served_model_label' of 'config.llm' as it is {}.",
                if inherited.is_none() {
                    "undefined"
                } else {
                    "null"
                }
            ))
        })?;
    let mut llm = domain::spread(inherited);
    domain::remove(&mut llm, "served_model_label")?;
    put(&mut llm, "profile", Value::string(""))?;
    for (target, source) in [
        ("base_url", "base_url"),
        ("model", "model"),
        ("fleet_profile", "id"),
        ("concurrency", "concurrency"),
        ("hardware", "hardware"),
        ("provider", "provider"),
        ("context_max", "context_max"),
    ] {
        put(
            &mut llm,
            target,
            selected
                .get(source)
                .cloned()
                .ok_or_else(|| Error::type_error("Fleet selection requires a validated profile"))?,
        )?;
    }
    let api_key = match selected.get("api_key_env") {
        Some(value) => Some(value),
        None => configured_key(config, "api_key_env")?,
    };
    let optional = match inherited
        .get("api_key_optional")
        .filter(|value| **value != Value::Null)
    {
        Some(value) => Some(value.clone()),
        None => configured_key(config, "api_key_optional")?.cloned(),
    };
    for (name, value) in [
        ("api_key_env", api_key.cloned()),
        ("api_key_optional", optional),
    ] {
        if let Some(value) = value {
            put(&mut llm, name, value)?;
        } else {
            domain::remove(&mut llm, name)?;
        }
    }
    if let Some(value) = selected.get("request_timeout_ms") {
        put(&mut llm, "request_timeout_ms", value.clone())?;
    }
    if is_text(selected.get("served_model_label"), "ignore") {
        put(&mut llm, "served_model_label", Value::string("ignore"))?;
    }
    let mut result = domain::spread(config);
    put(&mut result, "llm", llm)?;
    Ok(result)
}

pub fn load(repo_root: &Path) -> Result<Fleet, Error> {
    match config::read_private_source(repo_root, settings::Name::Fleet)? {
        Some(source) => parse(&source),
        None => Ok(Fleet {
            profiles: Vec::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../examples/fleet.yml");

    #[test]
    fn fictional_profiles_round_trip_without_registering_endpoints() -> Result<(), Error> {
        let fleet = parse(&Text::from(EXAMPLE))?;
        assert_eq!(fleet.profiles().len(), 3);
        assert_eq!(parse(&format(&fleet.as_value())?)?, fleet);
        assert!(parse(&Text::from("profiles: []\n"))?.profiles().is_empty());
        Ok(())
    }

    #[test]
    fn full_request_urls_and_unknown_models_are_not_fleet_profiles() -> Result<(), Error> {
        for url in [
            "http://example.invalid/v1/models/",
            "https://example.invalid/chat/completions",
        ] {
            assert!(normalize_base_url(&Value::string(url)).is_err());
        }
        let fleet = parse(&Text::from(EXAMPLE))?;
        let profile = fleet
            .profiles()
            .first()
            .ok_or_else(|| Error::type_error("Fixture profile missing"))?;
        let mut invalid = profile.as_value().clone();
        put(&mut invalid, "model", Value::string("unknown"))?;
        assert!(validate_profile(&invalid).is_err());
        put(&mut invalid, "model", Value::string("builtin-stub"))?;
        assert!(validate_profile(&invalid).is_err());
        Ok(())
    }

    #[test]
    fn routing_selection_removes_inherited_served_label_without_changing_declared_settings()
    -> Result<(), Error> {
        let config = config::parse(&Text::from(include_str!("../roster.config.example.yml")))?;
        let fleet = parse(&Text::from(EXAMPLE))?;
        let profile = fleet
            .profiles()
            .first()
            .ok_or_else(|| Error::type_error("Fixture profile missing"))?;
        let selected = with_profile(config.as_value(), profile.as_value())?;
        assert!(
            selected
                .get("llm")
                .and_then(|llm| llm.get("served_model_label"))
                .is_none()
        );
        assert_eq!(selected.get("profiles"), config.as_value().get("profiles"));
        assert!(
            config
                .as_value()
                .get("llm")
                .and_then(|llm| llm.get("fleet_profile"))
                .is_none()
        );
        Ok(())
    }
}
