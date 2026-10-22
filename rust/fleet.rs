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
        || value
            .0
            .iter()
            .any(|unit| matches!(unit, 0..=31 | 127..=159 | 8232 | 8233))
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

pub fn list(fleet: &Fleet) -> Result<Text, Error> {
    if fleet.profiles().is_empty() {
        return Ok(Text::from(
            "No fleet profiles configured. Run roster onboard or roster fleet add.\n",
        ));
    }
    let mut output =
        Text::from("ID\tMODEL\tBASE_URL\tCONTEXT_MAX\tCONCURRENCY\tHARDWARE\tTASK_CLASS\n");
    for profile in fleet.profiles() {
        let profile = profile.as_value();
        let mut fields = Vec::new();
        for name in [
            "id",
            "model",
            "base_url",
            "context_max",
            "concurrency",
            "hardware",
        ] {
            let value = profile.get(name).ok_or_else(|| {
                Error::type_error("Validated fleet profile omitted a listed field")
            })?;
            fields.push(if name == "context_max" && value == &Value::Number(0.0) {
                Text::from("unknown")
            } else {
                domain::string(value)?
            });
        }
        let mut classes = Text::default();
        if let Some(values) = profile.get("task_class") {
            for (index, class) in items(values)
                .ok_or_else(|| Error::type_error("Fleet task hints must be an array"))?
                .iter()
                .enumerate()
            {
                if index != 0 {
                    classes.push_str(",");
                }
                classes.0.extend(domain::string(class)?.0);
            }
        } else {
            classes.push_str("-");
        }
        fields.push(classes);
        for (index, field) in fields.iter().enumerate() {
            if index != 0 {
                output.push_str("\t");
            }
            output.0.extend_from_slice(&field.0);
        }
        output.push_str("\n");
    }
    Ok(output)
}

pub fn load(repo_root: &Path) -> Result<Fleet, Error> {
    match config::private_settings(repo_root, settings::Name::Fleet)? {
        Some((source, _)) => parse(&source),
        None => Ok(Fleet {
            profiles: Vec::new(),
        }),
    }
}

struct HardwareSpec {
    id: &'static str,
    class: &'static str,
    memory_gb: u32,
    nodes: u32,
    notes: &'static str,
}

const HARDWARE_INDEX: [HardwareSpec; 5] = [
    HardwareSpec {
        id: "rtx-3090-24gb",
        class: "last-resort",
        memory_gb: 24,
        nodes: 1,
        notes: "Local workstation GPU",
    },
    HardwareSpec {
        id: "rtx-6000-ada-48gb",
        class: "dedicated",
        memory_gb: 48,
        nodes: 1,
        notes: "Dedicated data-center GPU",
    },
    HardwareSpec {
        id: "dgx-spark-gb10-128gb",
        class: "cluster",
        memory_gb: 128,
        nodes: 1,
        notes: "One GB10 Spark, 128GB unified memory",
    },
    HardwareSpec {
        id: "dgx-spark-2n",
        class: "cluster",
        memory_gb: 256,
        nodes: 2,
        notes: "Two GB10 Sparks",
    },
    HardwareSpec {
        id: "dgx-spark-4n",
        class: "cluster",
        memory_gb: 512,
        nodes: 4,
        notes: "Four GB10 Sparks",
    },
];

pub fn describe_hardware(id: &str) -> Result<Value, Error> {
    let spec = HARDWARE_INDEX
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| Error::type_error(format!("Unknown hardware id: {id}")))?;
    Ok(Value::object(vec![
        ("id", Value::string(spec.id)),
        ("class", Value::string(spec.class)),
        ("memory_gb", Value::Number(f64::from(spec.memory_gb))),
        ("nodes", Value::Number(f64::from(spec.nodes))),
        ("notes", Value::string(spec.notes)),
    ]))
}

pub fn hardware_cost(profile: &Value, difficulty: u8) -> Result<f64, Error> {
    if members(profile).is_none() {
        return Err(Error::type_error("Fleet profile must be a JSON object"));
    }
    let hardware = profile
        .get("hardware")
        .and_then(Value::as_string)
        .map(|text| String::from_utf16_lossy(&text.0))
        .unwrap_or_default();
    let indexed = HARDWARE_INDEX.iter().find(|entry| entry.id == hardware);
    let kind = if let Some(entry) = indexed {
        entry.class
    } else if hardware.contains("3090") {
        "last-resort"
    } else if hardware.contains("6000") || hardware.contains("aperture") {
        "dedicated"
    } else if hardware.contains("spark") || hardware.contains("dgx") {
        "cluster"
    } else {
        "unspecified"
    };
    let context_max = profile
        .get("context_max")
        .and_then(Value::as_number)
        .filter(|value| value.is_finite() && *value > 0.0);
    let context = context_max.map_or(2.0, |value| value / 1_000_000.0);
    Ok(match kind {
        "last-resort" => 100.0 + context,
        "cluster" => {
            (if difficulty >= 4 { 8.0 } else { 30.0 })
                + context
                + f64::from(indexed.map_or(1, |entry| entry.nodes))
        }
        "dedicated" => 10.0 + context,
        _ => 20.0 + context,
    })
}

pub struct RouteOptions<'a> {
    pub fleet: &'a Value,
    pub capabilities: &'a Value,
    pub records: &'a [Value],
    pub task_class: &'a str,
    pub difficulty: u8,
    pub context_required: u64,
    pub profile_id: Option<&'a str>,
    pub seat: &'a str,
    pub excluded_profile_ids: &'a [String],
    pub queue_depth: Option<&'a dyn Fn(&str) -> usize>,
    pub now: i64,
}

fn valid_route_identifier(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value != "-"
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn effort_rank(effort: Option<&Value>) -> usize {
    match effort.and_then(Value::as_string) {
        Some(text) if ascii(&text.0, "l") => 0,
        Some(text) if ascii(&text.0, "m") => 1,
        Some(text) if ascii(&text.0, "h") => 2,
        Some(text) if ascii(&text.0, "x") => 3,
        _ => 4,
    }
}

pub fn choose_route(options: &RouteOptions<'_>) -> Result<Option<Value>, Error> {
    if !crate::learning::TASK_CLASSES.contains(&options.task_class) {
        return Err(Error::type_error(
            "Routing task class must be feat, fix, docs, or test",
        ));
    }
    if !(1..=5).contains(&options.difficulty) {
        return Err(Error::type_error(
            "Routing difficulty must be an integer from 1 to 5",
        ));
    }
    if options.context_required > crate::MAX_SAFE_INTEGER {
        return Err(Error::type_error(
            "Routing context requirement must be a nonnegative safe integer",
        ));
    }
    for record in options.records {
        if members(record).is_none()
            || record
                .get("evaluation")
                .is_some_and(|eval| eval != &Value::Null && members(eval).is_none())
        {
            return Err(Error::type_error(
                "Routing requires joined run/evaluation records",
            ));
        }
    }
    if !valid_route_identifier(options.seat) {
        return Err(Error::type_error(
            "Routing seat must be an opaque 1-64 character identifier",
        ));
    }
    let mut excluded = HashSet::new();
    for id in options.excluded_profile_ids {
        if !valid_route_identifier(id) || !excluded.insert(id.as_str()) {
            return Err(Error::type_error(
                "Excluded fleet profiles must be distinct opaque identifiers",
            ));
        }
    }
    let ceilings = crate::learning::derive_difficulty_ceilings(
        options.records,
        &crate::state::redaction::Config::default(),
    )?;
    let catalog = validate(options.fleet)?;
    let priors = crate::capabilities::validate(
        options.capabilities,
        crate::capabilities::Completeness::Full,
    )?;
    let base_profiles = match options.profile_id {
        Some(id) => vec![get_profile(&catalog, &Value::string(id))?.clone()],
        None => catalog.profiles().to_vec(),
    };
    let mut profiles = Vec::new();
    for profile in base_profiles {
        let val = profile.as_value();
        let id = val
            .get("id")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Fleet profile requires id"))?
            .to_utf8()?;
        if excluded.contains(id.as_str()) {
            continue;
        }
        let model = val
            .get("model")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Fleet profile requires model"))?;
        let allowed_by_ceiling = ceilings
            .iter()
            .find(|entry| {
                domain::is_text(entry.get("seat"), options.seat)
                    && entry.get("model") == Some(&Value::String(model.clone()))
            })
            .and_then(|entry| domain::safe_integer(entry.get("ceiling")))
            .is_none_or(|ceiling| ceiling >= u64::from(options.difficulty));
        if allowed_by_ceiling {
            profiles.push(profile);
        }
    }
    let depth_of = |id: &str| -> usize { options.queue_depth.map_or(0, |f| f(id)) };
    let seat_text = Text::from(options.seat);
    let mut human = Vec::new();
    for record in options.records {
        if record
            .get("evaluation")
            .is_some_and(|eval| *eval != Value::Null)
            && crate::learning::learning_seat(record)? == seat_text
        {
            human.push(record.clone());
        }
    }
    let groups = crate::learning::summarize_learning(&human)?
        .into_iter()
        .filter(|group| {
            domain::is_text(group.get("task_class"), options.task_class)
                && domain::safe_integer(group.get("n")).is_some_and(|n| n >= 3)
                && group
                    .get("medianDifficulty")
                    .and_then(Value::as_number)
                    .is_some_and(|med| med >= f64::from(options.difficulty))
        })
        .collect::<Vec<_>>();
    let mut evaluated = Vec::new();
    for profile in &profiles {
        let val = profile.as_value();
        let ctx_max = domain::safe_integer(val.get("context_max")).unwrap_or(0);
        if ctx_max == 0 || ctx_max < options.context_required {
            continue;
        }
        let id_text = val
            .get("id")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Fleet profile requires id"))?;
        let id_str = id_text.to_utf8()?;
        let id_key = crate::learning::collation_key(id_text)?;
        for group in &groups {
            if group.get("model") == val.get("model") {
                let n = domain::safe_integer(group.get("n")).unwrap_or(0);
                let rate = group
                    .get("acceptRate")
                    .and_then(Value::as_number)
                    .unwrap_or(0.0);
                let recommendation = Value::object(
                    [
                        "model",
                        "effort",
                        "n",
                        "accepted",
                        "acceptRate",
                        "medianMinutes",
                        "medianDifficulty",
                        "estimate_min",
                    ]
                    .iter()
                    .map(|name| (*name, group.get(name).cloned().unwrap_or(Value::Null)))
                    .collect(),
                );
                let choice = Value::object(vec![
                    ("profile", val.clone()),
                    ("source", Value::string("evals")),
                    (
                        "reason",
                        Value::string(&format!(
                            "{n} distinct human evaluations; declared context is sufficient"
                        )),
                    ),
                    ("recommendation", recommendation),
                ]);
                evaluated.push((
                    rate,
                    n,
                    depth_of(&id_str),
                    id_key.clone(),
                    effort_rank(group.get("effort")),
                    choice,
                ));
            }
        }
    }
    evaluated.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
            .then_with(|| left.4.cmp(&right.4))
    });
    let attach_evidence = |mut choice: Value| -> Result<Value, Error> {
        let model = choice
            .get("profile")
            .and_then(|profile| profile.get("model"))
            .and_then(Value::as_string)
            .cloned()
            .ok_or_else(|| Error::type_error("Route profile requires model"))?;
        let effort = choice
            .get("recommendation")
            .and_then(|rec| rec.get("effort"))
            .cloned();
        let evidence = crate::learning::explain_route_evidence(
            options.records,
            crate::learning::EvidenceScope {
                model: &model,
                task_class: options.task_class,
                seat: &seat_text,
                difficulty: options.difficulty,
                effort: effort.as_ref(),
                now_ms: options.now as f64,
            },
        )?;
        put(&mut choice, "evidence", Value::Array(evidence))?;
        Ok(choice)
    };
    if let Some((_, _, _, _, _, choice)) = evaluated.into_iter().next() {
        return attach_evidence(choice).map(Some);
    }

    let mut candidates = Vec::new();
    for profile in &profiles {
        let val = profile.as_value();
        let ctx_max = domain::safe_integer(val.get("context_max")).unwrap_or(0);
        if options.context_required > 0 && ctx_max < options.context_required {
            continue;
        }
        let id_text = val
            .get("id")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Fleet profile requires id"))?;
        let id_str = id_text.to_utf8()?;
        let id_key = crate::learning::collation_key(id_text)?;
        let prior = priors
            .records()
            .iter()
            .find(|record| {
                record.get("profile_id") == Some(&Value::String(id_text.clone()))
                    && domain::is_text(record.get("task_class"), options.task_class)
            })
            .or_else(|| {
                priors.records().iter().find(|record| {
                    record.get("model_id") == val.get("model")
                        && domain::is_text(record.get("task_class"), options.task_class)
                })
            });
        let hinted = val
            .get("task_class")
            .and_then(items)
            .is_some_and(|classes| {
                classes
                    .iter()
                    .any(|item| domain::is_text(Some(item), options.task_class))
            });
        if let Some(prior_record) = prior {
            if domain::safe_integer(prior_record.get("suggested_difficulty"))
                .is_none_or(|suggested| suggested < u64::from(options.difficulty))
            {
                continue;
            }
        } else if !hinted {
            continue;
        }
        let reason = format!(
            "{}; smallest sufficient hardware; starting guess, not a benchmark{}",
            if prior.is_some() {
                "capability prior"
            } else {
                "fleet task-class hint"
            },
            if ctx_max == 0 {
                "; context capacity remains unknown"
            } else {
                ""
            }
        );
        let choice = Value::object(vec![
            ("profile", val.clone()),
            ("source", Value::string("prior")),
            ("recommendation", Value::Null),
            ("reason", Value::string(&reason)),
        ]);
        let cost = hardware_cost(val, options.difficulty)?;
        let context_fit = if ctx_max > 0 {
            ctx_max
        } else {
            crate::MAX_SAFE_INTEGER
        };
        let concurrency = domain::safe_integer(val.get("concurrency")).unwrap_or(1);
        candidates.push((
            hinted,
            cost,
            context_fit,
            depth_of(&id_str),
            concurrency,
            id_key,
            choice,
        ));
    }
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.total_cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
            .then_with(|| right.4.cmp(&left.4))
            .then_with(|| left.5.cmp(&right.5))
    });
    if let Some((_, _, _, _, _, _, choice)) = candidates.into_iter().next() {
        attach_evidence(choice).map(Some)
    } else {
        Ok(None)
    }
}

pub fn format_route(
    choice: Option<&Value>,
    task_class: &str,
    config: &Value,
    environment: &Value,
) -> Result<String, Error> {
    let Some(choice) = choice else {
        return crate::learning::format_recommendation(None, task_class, config, environment);
    };
    let profile = choice
        .get("profile")
        .ok_or_else(|| Error::type_error("Route choice requires a profile"))?;
    let id = domain::string(
        profile
            .get("id")
            .ok_or_else(|| Error::type_error("Route profile requires id"))?,
    )?
    .to_utf8()?;
    let model = domain::string(
        profile
            .get("model")
            .ok_or_else(|| Error::type_error("Route profile requires model"))?,
    )?
    .to_utf8()?;
    let source = domain::string(
        choice
            .get("source")
            .ok_or_else(|| Error::type_error("Route choice requires source"))?,
    )?
    .to_utf8()?;
    let reason = domain::string(
        choice
            .get("reason")
            .ok_or_else(|| Error::type_error("Route choice requires reason"))?,
    )?
    .to_utf8()?;
    let ctx_max = domain::safe_integer(profile.get("context_max")).unwrap_or(0);
    let ctx_str = if ctx_max == 0 {
        "unknown".to_owned()
    } else {
        ctx_max.to_string()
    };
    let concurrency = domain::safe_integer(profile.get("concurrency")).unwrap_or(1);
    let prefix = match choice
        .get("recommendation")
        .filter(|rec| **rec != Value::Null)
    {
        Some(recommendation) => crate::learning::format_recommendation(
            Some(recommendation),
            task_class,
            config,
            environment,
        )?
        .trim_end()
        .to_owned(),
        None => format!("{task_class}: {model}"),
    };
    let mut output = format!(
        "{prefix} profile={id} source={source} context_max={ctx_str} concurrency={concurrency} reason={reason}\n"
    );
    if let Some(evidence) = choice.get("evidence").and_then(items) {
        for entry in evidence {
            let entry_model = domain::string(
                entry
                    .get("model")
                    .ok_or_else(|| Error::type_error("Route evidence requires model"))?,
            )?
            .to_utf8()?;
            let seat = domain::string(
                entry
                    .get("seat")
                    .ok_or_else(|| Error::type_error("Route evidence requires seat"))?,
            )?
            .to_utf8()?;
            let entry_class = domain::string(
                entry
                    .get("task_class")
                    .ok_or_else(|| Error::type_error("Route evidence requires task_class"))?,
            )?
            .to_utf8()?;
            let effort = entry
                .get("effort")
                .filter(|v| **v != Value::Null)
                .map_or_else(|| Ok(Text::from("-")), domain::string)?
                .to_utf8()?;
            let samples = domain::safe_integer(entry.get("samples")).unwrap_or(0);
            let accepted = domain::safe_integer(entry.get("accepted")).unwrap_or(0);
            let rejected = domain::safe_integer(entry.get("rejected")).unwrap_or(0);
            let origin = if samples > 0 {
                "local-human-evaluations"
            } else {
                "none"
            };
            let accept_rate = match entry.get("acceptRate").and_then(Value::as_number) {
                Some(rate) => {
                    format!("{}%", crate::planning::feedback::percentage(rate * 100.0)?)
                }
                None => "unknown".to_owned(),
            };
            let latest = entry
                .get("latestEvaluation")
                .filter(|v| **v != Value::Null)
                .map_or_else(|| Ok(Text::from("unknown")), domain::string)?
                .to_utf8()?;
            let age_days = entry
                .get("ageDays")
                .filter(|v| **v != Value::Null)
                .map_or_else(|| Ok(Text::from("unknown")), domain::string)?
                .to_utf8()?;
            let sufficient = entry.get("sufficient") == Some(&Value::Boolean(true));
            let warning = if sufficient {
                "none"
            } else if samples > 0 {
                "insufficient-qualifying-evidence"
            } else {
                "no-human-evidence"
            };
            output.push_str(&format!(
                "evidence model={entry_model} seat={seat} task-class={entry_class} effort={effort} origin={origin} n={samples} accepted={accepted} rejected={rejected} accept-rate={accept_rate} latest={latest} age-days={age_days} warning={warning}\n"
            ));
        }
    }
    crate::learning::bounded_output(output)
}

pub const DISCOVERY_PORTS: &[u16] = &[8000, 8001, 8080, 8888, 11434, 11435, 1234];

pub fn local_candidates() -> Vec<String> {
    DISCOVERY_PORTS
        .iter()
        .map(|port| format!("http://127.0.0.1:{port}/v1"))
        .collect()
}

pub fn discovery_candidates(env: Option<&Value>, extra_urls: &[String]) -> Vec<String> {
    let mut list = local_candidates();
    if let Some(extra_raw) = env
        .and_then(|e| e.get("ROSTER_DISCOVER_HOSTS"))
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
    {
        for entry in extra_raw.split(',') {
            let trimmed = entry.trim();
            if !trimmed.is_empty() && !list.iter().any(|existing| existing == trimmed) {
                list.push(trimmed.to_owned());
            }
        }
    }
    for url in extra_urls {
        if !url.is_empty() && !list.iter().any(|existing| existing == url) {
            list.push(url.clone());
        }
    }
    list
}

pub fn merge_discovered_profiles(fleet: &Value, endpoints: &[Value]) -> Result<Fleet, Error> {
    let validated = validate(fleet)?;
    let mut profiles = validated
        .profiles()
        .iter()
        .map(|profile| profile.as_value().clone())
        .collect::<Vec<_>>();
    for endpoint in endpoints {
        let base_url_val = endpoint
            .get("baseUrl")
            .ok_or_else(|| Error::type_error("Discovered endpoint requires baseUrl"))?;
        let base_url = normalize_base_url(base_url_val)?.to_utf8()?;
        let hardware = endpoint
            .get("hardware")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unspecified".to_owned());
        let models = endpoint
            .get("models")
            .and_then(items)
            .ok_or_else(|| Error::type_error("Discovered endpoint requires models array"))?;
        for model in models {
            let Some(model_id) = model
                .get("id")
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            if profiles.iter().any(|existing| {
                domain::is_text(existing.get("base_url"), &base_url)
                    && domain::is_text(existing.get("model"), &model_id)
            }) {
                continue;
            }
            let Some(context_max) =
                domain::safe_integer(model.get("context_max")).filter(|ctx| *ctx >= 1)
            else {
                continue;
            };
            let digest = crate::identity::sha256_hex(format!("{base_url}\n{model_id}").as_bytes());
            let prefix = format!("discovered-{}", &digest[..16]);
            let mut id = prefix.clone();
            let mut suffix = 2_u64;
            while profiles
                .iter()
                .any(|existing| domain::is_text(existing.get("id"), &id))
            {
                id = format!("{prefix}-{suffix}");
                suffix += 1;
            }
            profiles.push(Value::object(vec![
                ("id", Value::string(&id)),
                ("base_url", Value::string(&base_url)),
                ("model", Value::string(&model_id)),
                ("provider", Value::string("vllm")),
                ("context_max", Value::Number(context_max as f64)),
                ("concurrency", Value::Number(1.0)),
                ("hardware", Value::string(&hardware)),
                (
                    "notes",
                    Value::string(
                        "Observed through GET /models; hardware and capacity not inferred.",
                    ),
                ),
            ]));
        }
    }
    validate(&Value::object(vec![("profiles", Value::Array(profiles))]))
}

pub fn select_remediation_profile(profiles: &[Value], current_id: Option<&str>) -> Option<Value> {
    let mut eligible = profiles
        .iter()
        .filter(|profile| {
            profile
                .get("id")
                .and_then(Value::as_string)
                .is_some_and(|s| !s.0.is_empty())
                && profile
                    .get("base_url")
                    .and_then(Value::as_string)
                    .is_some_and(|s| !s.0.is_empty())
        })
        .cloned()
        .collect::<Vec<_>>();
    if eligible.is_empty() {
        return None;
    }
    eligible.sort_by(|left, right| {
        let left_ctx = domain::safe_integer(left.get("context_max")).unwrap_or(0);
        let right_ctx = domain::safe_integer(right.get("context_max")).unwrap_or(0);
        right_ctx.cmp(&left_ctx).then_with(|| {
            let left_id = left
                .get("id")
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .unwrap_or_default();
            let right_id = right
                .get("id")
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .unwrap_or_default();
            left_id.cmp(&right_id)
        })
    });
    eligible.into_iter().next().or_else(|| {
        let target = current_id?;
        profiles
            .iter()
            .find(|p| domain::is_text(p.get("id"), target))
            .cloned()
    })
}

pub fn remediation_task(files: &[String], workspace: &str) -> Result<Value, Error> {
    let test_re = regex_lite::Regex::new(r"^(?:tests/)?[^/]+[._-]test\.[cm]?js$")
        .map_err(|error| Error::new("Error", error.to_string()))?;
    let mut tests = Vec::new();
    for file in files {
        let normalized = file.replace('\\', "/");
        if (test_re.is_match(&normalized) || normalized.starts_with("tests/"))
            && !tests.contains(&normalized)
        {
            tests.push(normalized);
        }
    }
    if tests.is_empty() {
        return Err(Error::type_error(
            "Remediation requires at least one workspace test file",
        ));
    }
    let ask = format!(
        "Remediate the failing tests in {workspace}. Edit only {}. Do not change product code unless a test proves the current behavior is wrong. Run only those test files. Stop when they pass.",
        tests.join(", ")
    );
    Ok(Value::object(vec![
        (
            "files",
            Value::Array(tests.iter().map(|f| Value::string(f)).collect()),
        ),
        ("ask", Value::string(&ask)),
    ]))
}

pub const FLEET_CLI_USAGE: &str = "Use roster fleet list, discover [--wsl] [--save], add --id NAME --base-url URL [--model MODEL] [--context N] [--concurrency N] [--hardware TEXT] [--task-class feat,fix,docs,test], probe ID [--set-model [MODEL]], hardware ID SKU, default ID, remove ID, or assist.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetCliCommand {
    List,
    Discover {
        wsl: bool,
        save: bool,
    },
    Assist,
    Hardware {
        id: String,
        hardware: String,
    },
    Default {
        id: String,
    },
    Remove {
        id: String,
    },
    Probe {
        id: String,
        set_model: bool,
        model: Option<String>,
    },
    Add {
        id: String,
        base_url: String,
        model: Option<String>,
        context: Option<String>,
        concurrency: Option<String>,
        hardware: Option<String>,
        task_class: Option<String>,
    },
}

pub fn parse_fleet_cli_args(args: &[String]) -> Result<FleetCliCommand, Error> {
    let usage = || Error::type_error(FLEET_CLI_USAGE);
    let Some((command, rest)) = args.split_first() else {
        return Err(usage());
    };
    if command == "list" && rest.is_empty() {
        return Ok(FleetCliCommand::List);
    }
    if command == "discover"
        && rest
            .iter()
            .all(|flag| matches!(flag.as_str(), "--wsl" | "--save"))
    {
        let mut seen = HashSet::new();
        if rest.iter().all(|f| seen.insert(f.as_str())) {
            return Ok(FleetCliCommand::Discover {
                wsl: seen.contains("--wsl"),
                save: seen.contains("--save"),
            });
        }
    }
    if command == "assist" && rest.is_empty() {
        return Ok(FleetCliCommand::Assist);
    }
    if command == "hardware" && rest.len() == 2 {
        let id = validate_id(Some(&Value::string(&rest[0])))?.to_utf8()?;
        return Ok(FleetCliCommand::Hardware {
            id,
            hardware: rest[1].clone(),
        });
    }
    if (command == "default" || command == "remove") && rest.len() == 1 {
        let id = validate_id(Some(&Value::string(&rest[0])))?.to_utf8()?;
        return Ok(if command == "default" {
            FleetCliCommand::Default { id }
        } else {
            FleetCliCommand::Remove { id }
        });
    }
    if command == "probe"
        && (1..=3).contains(&rest.len())
        && (rest.len() == 1
            || (rest[1] == "--set-model" && !rest.get(2).is_some_and(|m| m.starts_with("--"))))
    {
        let id = validate_id(Some(&Value::string(&rest[0])))?.to_utf8()?;
        return Ok(FleetCliCommand::Probe {
            id,
            set_model: rest.len() > 1,
            model: rest.get(2).cloned(),
        });
    }
    if command != "add" || rest.len() % 2 != 0 {
        return Err(usage());
    }
    let mut id = None;
    let mut base_url = None;
    let mut model = None;
    let mut context = None;
    let mut concurrency = None;
    let mut hardware = None;
    let mut task_class = None;
    let mut idx = 0;
    while idx < rest.len() {
        let flag = rest[idx].as_str();
        let val = &rest[idx + 1];
        if val.is_empty() || val.starts_with("--") {
            return Err(usage());
        }
        match flag {
            "--id" if id.is_none() => id = Some(val.clone()),
            "--base-url" if base_url.is_none() => base_url = Some(val.clone()),
            "--model" if model.is_none() => model = Some(val.clone()),
            "--context" if context.is_none() => context = Some(val.clone()),
            "--concurrency" if concurrency.is_none() => concurrency = Some(val.clone()),
            "--hardware" if hardware.is_none() => hardware = Some(val.clone()),
            "--task-class" if task_class.is_none() => task_class = Some(val.clone()),
            _ => return Err(usage()),
        }
        idx += 2;
    }
    let raw_id = id.ok_or_else(usage)?;
    let raw_url = base_url.ok_or_else(usage)?;
    let valid_id = validate_id(Some(&Value::string(&raw_id)))?.to_utf8()?;
    let normalized_url = normalize_base_url(&Value::string(&raw_url))?.to_utf8()?;
    Ok(FleetCliCommand::Add {
        id: valid_id,
        base_url: normalized_url,
        model,
        context,
        concurrency,
        hardware,
        task_class,
    })
}

pub fn matches_default(config_val: &Value, profile: &Value) -> bool {
    let Some(llm) = config_val.get("llm") else {
        return false;
    };
    let Some(cfg_url) = llm.get("base_url").filter(|v| domain::truthy(v)) else {
        return false;
    };
    let Ok(norm_cfg_url) = normalize_base_url(cfg_url) else {
        return false;
    };
    let prof_url = profile.get("base_url").and_then(Value::as_string);
    llm.get("model") == profile.get("model") && prof_url == Some(&norm_cfg_url)
}

pub fn apply_fleet_hardware(
    fleet: &Fleet,
    id: &str,
    hardware_sku: &str,
) -> Result<(Fleet, Value, String), Error> {
    let hw = describe_hardware(hardware_sku)?;
    let hw_id = hw
        .get("id")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Hardware descriptor missing id"))?
        .to_utf8()?;
    let memory_gb = domain::safe_integer(hw.get("memory_gb")).unwrap_or(0);
    let nodes = domain::safe_integer(hw.get("nodes")).unwrap_or(1);
    let profile = get_profile(fleet, &Value::string(id))?;
    let mut updated_profiles = Vec::with_capacity(fleet.profiles().len());
    for item in fleet.profiles() {
        let mut val = item.as_value().clone();
        if val.get("id") == profile.as_value().get("id") {
            domain::put(&mut val, "hardware", Value::string(&hw_id))?;
        }
        updated_profiles.push(val);
    }
    let updated = validate(&Value::object(vec![(
        "profiles",
        Value::Array(updated_profiles),
    )]))?;
    let selected = get_profile(&updated, &Value::string(id))?
        .as_value()
        .clone();
    let msg = format!(
        "Hardware for {id}: {hw_id} ({memory_gb}GB, {nodes} node(s)). Endpoint unchanged.\n"
    );
    Ok((updated, selected, msg))
}

pub fn apply_fleet_default(
    config_val: &Value,
    fleet: &Fleet,
    id: &str,
) -> Result<(Value, Text, String), Error> {
    let profile = get_profile(fleet, &Value::string(id))?;
    let updated_cfg = with_profile(config_val, profile.as_value())?;
    let formatted = config::format(&updated_cfg)?;
    let msg = format!("Default endpoint/model selected: {id}\n");
    Ok((updated_cfg, formatted, msg))
}

pub fn apply_fleet_remove(
    config_val: &Value,
    fleet: &Fleet,
    id: &str,
) -> Result<(Fleet, String), Error> {
    let profile = get_profile(fleet, &Value::string(id))?;
    if fleet.profiles().len() == 1 {
        return Err(Error::new("Error", "Cannot remove the last fleet profile"));
    }
    if matches_default(config_val, profile.as_value()) {
        return Err(Error::new(
            "Error",
            "Select another fleet default before removing the currently configured endpoint",
        ));
    }
    let remaining = fleet
        .profiles()
        .iter()
        .filter(|item| item.as_value().get("id") != profile.as_value().get("id"))
        .map(|item| item.as_value().clone())
        .collect::<Vec<_>>();
    let updated = validate(&Value::object(vec![("profiles", Value::Array(remaining))]))?;
    Ok((updated, format!("Removed fleet profile: {id}\n")))
}

pub fn parse_model_inventory_response(
    payload: &Value,
    env: Option<&Value>,
    api_key_env: Option<&str>,
) -> Result<Vec<Value>, Error> {
    let Some(data) = payload
        .get("data")
        .and_then(items)
        .filter(|d| !d.is_empty())
    else {
        return Err(Error::new("ModelProbeError", "invalid response"));
    };
    let env_value = env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    let mut redaction_cfg = crate::state::redaction::Config {
        environment: env_value,
        ..crate::state::redaction::Config::default()
    };
    if let Some(key) = api_key_env.filter(|s| !s.is_empty()) {
        redaction_cfg.api_key_env = Text::from(key);
    }
    redaction_cfg.validate()?;
    let model_re = regex_lite::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:/-]*$")
        .map_err(|error| Error::new("Error", error.to_string()))?;
    let mut seen: Vec<(String, Option<u64>)> = Vec::new();
    for entry in data {
        let Some(raw_id) = entry
            .get("id")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
        else {
            return Err(Error::new("ModelProbeError", "invalid response"));
        };
        if raw_id
            .chars()
            .any(|ch| matches!(ch, '\x00'..='\x1f' | '\x7f'))
        {
            return Err(Error::new("ModelProbeError", "invalid response"));
        }
        let redacted =
            crate::state::redaction::evidence(&Text::from(raw_id.as_str()), &redaction_cfg)
                .to_utf8()?;
        if redacted != raw_id {
            return Err(Error::new("ModelProbeError", "invalid response"));
        }
        let resolved = run::resolve_publish_model(
            None,
            Some(&Value::object(vec![("AI_MODEL", Value::string(&raw_id))])),
            None,
            false,
        )
        .map_err(|_| Error::new("ModelProbeError", "invalid response"))?;
        if resolved == "builtin-stub" || !model_re.is_match(&resolved) {
            return Err(Error::new("ModelProbeError", "invalid response"));
        }
        let ctx_max = crate::llm::window::reported_context_max(entry);
        if let Some(existing) = seen.iter_mut().find(|(id, _)| *id == resolved) {
            if existing.1.is_none() && ctx_max.is_some() {
                existing.1 = ctx_max;
            }
        } else {
            seen.push((resolved, ctx_max));
        }
    }
    Ok(seen
        .into_iter()
        .map(|(id, ctx)| {
            let mut fields = vec![("id", Value::string(&id))];
            if let Some(c) = ctx {
                fields.push(("context_max", Value::Number(c as f64)));
            }
            Value::object(fields)
        })
        .collect())
}

pub async fn probe_model_details<T: crate::llm::http::Transport>(
    base_url: &str,
    transport: &mut T,
    env: Option<&Value>,
    api_key_env: Option<&str>,
    authenticate: bool,
    timeout_ms: Option<u64>,
) -> Result<Vec<Value>, Error> {
    use crate::llm::http::{Body, Request};
    if let Some(ms) = timeout_ms {
        if !(1..=60_000).contains(&ms) {
            return Err(Error::type_error(
                "Model probe timeout must be a positive integer of at most 60000 ms",
            ));
        }
    }
    let env_value = env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    let mut redaction_cfg = crate::state::redaction::Config {
        environment: env_value.clone(),
        ..crate::state::redaction::Config::default()
    };
    if let Some(key) = api_key_env.filter(|s| !s.is_empty()) {
        redaction_cfg.api_key_env = Text::from(key);
    }
    redaction_cfg.validate()?;
    if base_url
        .chars()
        .any(|ch| matches!(ch, '\x00'..='\x1f' | '\x7f'))
        || crate::state::redaction::evidence(&Text::from(base_url), &redaction_cfg).to_utf8()?
            != base_url
    {
        return Err(Error::type_error(
            "Enter a public URL or model ID, not a secret",
        ));
    }
    let validated = config::validate_base_url(&Text::from(base_url), "llm.base_url")?.to_utf8()?;
    let mut url = url::Url::parse(&validated)
        .map_err(|_| Error::type_error("Invalid URL").coded("ERR_INVALID_URL"))?;
    let trimmed_path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&format!("{trimmed_path}/models"));
    let hostname = url.host_str().unwrap_or("");
    let effective_timeout =
        timeout_ms.unwrap_or(if hostname == "127.0.0.1" || hostname == "localhost" {
            5_000
        } else {
            60_000
        });
    let mut auth_source = "missing";
    let mut resolved_key = None;
    if authenticate {
        let key_names = if hostname.contains("aperture") {
            vec!["APERTURE_API_KEY".to_owned(), "ROSTER_API_KEY".to_owned()]
        } else {
            vec![api_key_env.unwrap_or("ROSTER_API_KEY").to_owned()]
        };
        for name in key_names {
            if let Ok(Some(val)) = config::environment_value(&env_value, &name) {
                if let Ok(s) = val.to_utf8() {
                    if !s.is_empty() {
                        auth_source = "env";
                        resolved_key = Some(s);
                        break;
                    }
                }
            }
            if let Ok(vault) = crate::vault::FileVault::from_environment(&env_value) {
                if let Ok(Some(s)) = vault.get(&name) {
                    if !s.is_empty() {
                        auth_source = "vault";
                        resolved_key = Some(s);
                        break;
                    }
                }
            }
        }
    }
    let mut headers = vec![("Accept".to_owned(), "application/json".to_owned())];
    if let Some(ref k) = resolved_key {
        headers.push(("Authorization".to_owned(), format!("Bearer {k}")));
    }
    let req = Request {
        url: url.as_str().to_owned(),
        method: "GET".to_owned(),
        headers,
        body: Vec::new(),
    };
    let body_bytes =
        match tokio::time::timeout(std::time::Duration::from_millis(effective_timeout), async {
            let res = transport
                .send(&req)
                .await
                .map_err(|_| Error::new("ModelProbeError", "network"))?;
            if !(200..300).contains(&res.status) {
                return Err(Error::new(
                    "ModelProbeError",
                    format!("HTTP {} auth={auth_source}", res.status),
                ));
            }
            let mut bytes = Vec::new();
            if let Some(mut body) = res.body {
                while let Some(chunk) = body
                    .read()
                    .await
                    .map_err(|_| Error::new("ModelProbeError", "invalid response"))?
                {
                    bytes.extend_from_slice(&chunk);
                }
            }
            Ok::<Vec<u8>, Error>(bytes)
        })
        .await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(err)) => return Err(err),
            Err(_) => return Err(Error::new("ModelProbeError", "timeout")),
        };
    let text = String::from_utf8(body_bytes)
        .map_err(|_| Error::new("ModelProbeError", "invalid response"))?;
    let payload = crate::json::parse(&Text::from(text.as_str()))
        .map_err(|_| Error::new("ModelProbeError", "invalid response"))?;
    parse_model_inventory_response(&payload, Some(&env_value), api_key_env)
}

pub async fn probe_models<T: crate::llm::http::Transport>(
    base_url: &str,
    transport: &mut T,
    env: Option<&Value>,
    api_key_env: Option<&str>,
    authenticate: bool,
    timeout_ms: Option<u64>,
) -> Result<Vec<String>, Error> {
    let details = probe_model_details(
        base_url,
        transport,
        env,
        api_key_env,
        authenticate,
        timeout_ms,
    )
    .await?;
    details
        .into_iter()
        .map(|entry| {
            entry
                .get("id")
                .and_then(Value::as_string)
                .ok_or_else(|| Error::new("ModelProbeError", "invalid response"))?
                .to_utf8()
        })
        .collect()
}

pub async fn run_fleet_cli<T: crate::llm::http::Transport>(
    cmd: &FleetCliCommand,
    cwd: &Path,
    installation: &Path,
    environment: &Value,
    is_tty: bool,
    transport: &mut T,
    token: Option<&crate::cancel::CancellationToken>,
) -> Result<String, Error> {
    if matches!(cmd, FleetCliCommand::Assist) && !is_tty {
        return Err(Error::new("Error", "roster fleet assist needs a terminal"));
    }
    if let FleetCliCommand::Add { model: None, .. } = cmd {
        if !is_tty {
            return Err(Error::type_error(
                "Non-TTY fleet add requires --id, --base-url, and --model",
            ));
        }
    }
    if let FleetCliCommand::Probe {
        set_model: true,
        model: None,
        ..
    } = cmd
    {
        if !is_tty {
            return Err(Error::type_error(
                "Non-TTY fleet probe --set-model requires a model ID",
            ));
        }
    }
    let repo_root = crate::config::shared_config_root(cwd)
        .or_else(|_| crate::paths::resolve_project_root(cwd))?;
    let previous_fleet = settings::read_private_file(&repo_root, settings::Name::Fleet)?;
    let fleet_val = match &previous_fleet {
        Some(src) => parse(src)?,
        None => validate(&Value::object(vec![("profiles", Value::Array(Vec::new()))]))?,
    };
    let current_cfg = || -> Result<(Option<Text>, Value), Error> {
        let prev = settings::read_private_file(&repo_root, settings::Name::Config)?;
        let cfg = match &prev {
            Some(src) => config::parse(src)?.as_value().clone(),
            None => config::load(installation, cwd, environment)?
                .config
                .as_value()
                .clone(),
        };
        Ok((prev, cfg))
    };
    match cmd {
        FleetCliCommand::List => list(&fleet_val)?.to_utf8(),
        FleetCliCommand::Hardware { id, hardware } => {
            let (updated, _, msg) = apply_fleet_hardware(&fleet_val, id, hardware)?;
            settings::ensure_private_files_ignored(&repo_root, &[settings::Name::Fleet], token)
                .await?;
            settings::write_private_documents(
                &repo_root,
                &[settings::Document {
                    name: settings::Name::Fleet,
                    source: format(&updated.as_value())?,
                    expected_source: Some(previous_fleet),
                }],
            )?;
            Ok(msg)
        }
        FleetCliCommand::Default { id } => {
            let (prev_cfg, cfg_val) = current_cfg()?;
            let (_, formatted_cfg, msg) = apply_fleet_default(&cfg_val, &fleet_val, id)?;
            settings::ensure_private_files_ignored(&repo_root, &[settings::Name::Config], token)
                .await?;
            settings::write_private_documents(
                &repo_root,
                &[settings::Document {
                    name: settings::Name::Config,
                    source: formatted_cfg,
                    expected_source: Some(prev_cfg),
                }],
            )?;
            Ok(msg)
        }
        FleetCliCommand::Remove { id } => {
            let (_, cfg_val) = current_cfg()?;
            let (updated, msg) = apply_fleet_remove(&cfg_val, &fleet_val, id)?;
            settings::ensure_private_files_ignored(&repo_root, &[settings::Name::Fleet], token)
                .await?;
            settings::write_private_documents(
                &repo_root,
                &[settings::Document {
                    name: settings::Name::Fleet,
                    source: format(&updated.as_value())?,
                    expected_source: Some(previous_fleet),
                }],
            )?;
            Ok(msg)
        }
        FleetCliCommand::Discover { wsl: _, save } => {
            let mut out = String::from(
                "Discovery uses unauthenticated model inventory/metadata probes; no model prompts or default changes.\n",
            );
            let candidates = discovery_candidates(Some(environment), &[]);
            let mut endpoints = Vec::new();
            let mut failures = 0_usize;
            for base_url in candidates {
                match probe_model_details(
                    &base_url,
                    transport,
                    Some(environment),
                    None,
                    false,
                    Some(5_000),
                )
                .await
                {
                    Ok(models) => {
                        let model_strs = models
                            .iter()
                            .filter_map(|m| {
                                let id = m.get("id")?.as_string()?.to_utf8().ok()?;
                                let ctx = domain::safe_integer(m.get("context_max")).map_or_else(
                                    || "unreported; use fleet add --context".to_owned(),
                                    |n| n.to_string(),
                                );
                                Some(format!("{id} (context={ctx})"))
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        out.push_str(&format!("{base_url}: {model_strs}\n"));
                        endpoints.push(Value::object(vec![
                            ("baseUrl", Value::string(&base_url)),
                            ("models", Value::Array(models)),
                        ]));
                    }
                    Err(_) => failures += 1,
                }
            }
            if endpoints.is_empty() {
                out.push_str(
                    "No model endpoint answered. Check server bind address, firewall and ROSTER_DISCOVER_HOSTS.\n",
                );
            }
            if failures > 0 {
                out.push_str(&format!(
                    "{failures} endpoint probe(s) failed or timed out; no credentials sent.\n"
                ));
            }
            let merged = merge_discovered_profiles(&fleet_val.as_value(), &endpoints)?;
            if *save && merged.profiles().len() != fleet_val.profiles().len() {
                settings::ensure_private_files_ignored(&repo_root, &[settings::Name::Fleet], token)
                    .await?;
                settings::write_private_documents(
                    &repo_root,
                    &[settings::Document {
                        name: settings::Name::Fleet,
                        source: format(&merged.as_value())?,
                        expected_source: Some(previous_fleet),
                    }],
                )?;
                out.push_str(&format!(
                    "Added {} profiles; existing profiles and default config unchanged.\n",
                    merged.profiles().len() - fleet_val.profiles().len()
                ));
            } else {
                out.push_str(
                    "Private settings unchanged. Use --save to add newly observed profiles.\n",
                );
            }
            Ok(out)
        }
        FleetCliCommand::Add {
            id,
            base_url,
            model,
            context,
            concurrency,
            hardware,
            task_class,
        } => {
            if fleet_val
                .profiles()
                .iter()
                .any(|p| domain::is_text(p.as_value().get("id"), id))
            {
                return Err(Error::new("Error", "Fleet profile ID already exists"));
            }
            let (_, cfg_val) = current_cfg()?;
            let api_key_env = cfg_val
                .get("llm")
                .and_then(|l| l.get("api_key_env"))
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok());
            let models = probe_model_details(
                base_url,
                transport,
                Some(environment),
                api_key_env.as_deref(),
                true,
                None,
            )
            .await?;
            let mut out = String::from("Available models:\n");
            for (idx, m) in models.iter().enumerate() {
                if let Some(m_id) = m
                    .get("id")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                {
                    out.push_str(&format!("  {}. {m_id}\n", idx + 1));
                }
            }
            let selected_model = model.clone().ok_or_else(|| {
                Error::type_error("Fleet questions need a terminal or explicit flags")
            })?;
            let matched = models
                .iter()
                .find(|m| domain::is_text(m.get("id"), &selected_model))
                .ok_or_else(|| {
                    Error::new(
                        "Error",
                        "Requested model is not in the endpoint models list",
                    )
                })?;
            let reported_ctx = domain::safe_integer(matched.get("context_max"));
            let ctx_str = match (context.as_deref(), reported_ctx) {
                (Some(c), _) => c.to_owned(),
                (None, Some(rc)) => {
                    out.push_str(&format!(
                        "Using context_max={rc} reported by /v1/models for {selected_model}.\n"
                    ));
                    rc.to_string()
                }
                (None, None) => {
                    return Err(Error::type_error(
                        "Non-TTY fleet add requires --context when /v1/models does not report a context limit",
                    ));
                }
            };
            let parse_pos = |raw: &str, field: &str| -> Result<u64, Error> {
                if !raw.bytes().all(|b| b.is_ascii_digit()) || raw.starts_with('0') {
                    return Err(Error::type_error(format!(
                        "Fleet {field} must be a positive safe integer"
                    )));
                }
                raw.parse::<u64>()
                    .ok()
                    .filter(|n| (1..=crate::MAX_SAFE_INTEGER).contains(n))
                    .ok_or_else(|| {
                        Error::type_error(format!("Fleet {field} must be a positive safe integer"))
                    })
            };
            let ctx_val = parse_pos(&ctx_str, "context")?;
            let conc_val = match concurrency.as_deref() {
                Some(c) => parse_pos(c, "concurrency")?,
                None => 1,
            };
            let mut prof_fields = vec![
                ("id", Value::string(id)),
                ("base_url", Value::string(base_url)),
                ("model", Value::string(&selected_model)),
                ("provider", Value::string("vllm")),
                ("context_max", Value::Number(ctx_val as f64)),
                ("concurrency", Value::Number(conc_val as f64)),
                (
                    "hardware",
                    Value::string(hardware.as_deref().unwrap_or("unspecified")),
                ),
                ("notes", Value::string("")),
            ];
            if let Some(tc) = task_class.as_deref() {
                prof_fields.push((
                    "task_class",
                    Value::Array(tc.split(',').map(Value::string).collect()),
                ));
            }
            let validated_prof = validate_profile(&Value::object(prof_fields))?;
            let mut all_profiles = fleet_val
                .profiles()
                .iter()
                .map(|p| p.as_value().clone())
                .collect::<Vec<_>>();
            all_profiles.push(validated_prof.as_value().clone());
            let updated = validate(&Value::object(vec![(
                "profiles",
                Value::Array(all_profiles),
            )]))?;
            settings::ensure_private_files_ignored(&repo_root, &[settings::Name::Fleet], token)
                .await?;
            settings::write_private_documents(
                &repo_root,
                &[settings::Document {
                    name: settings::Name::Fleet,
                    source: format(&updated.as_value())?,
                    expected_source: Some(previous_fleet),
                }],
            )?;
            out.push_str(&format!(
                "Added fleet profile: {id}. Default config unchanged.\n"
            ));
            Ok(out)
        }
        FleetCliCommand::Probe {
            id,
            set_model,
            model,
        } => {
            let profile = get_profile(&fleet_val, &Value::string(id))?;
            let base_url = profile
                .as_value()
                .get("base_url")
                .and_then(Value::as_string)
                .ok_or_else(|| Error::type_error("Fleet profile missing base_url"))?
                .to_utf8()?;
            let saved_model = profile
                .as_value()
                .get("model")
                .and_then(Value::as_string)
                .ok_or_else(|| Error::type_error("Fleet profile missing model"))?
                .to_utf8()?;
            let (prev_cfg, cfg_val) = current_cfg()?;
            let api_key_env = cfg_val
                .get("llm")
                .and_then(|l| l.get("api_key_env"))
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok());
            let models = probe_model_details(
                &base_url,
                transport,
                Some(environment),
                api_key_env.as_deref(),
                true,
                None,
            )
            .await?;
            if !*set_model {
                let mut out = String::from("Available models:\n");
                for m in &models {
                    if let Some(m_id) = m
                        .get("id")
                        .and_then(Value::as_string)
                        .and_then(|t| t.to_utf8().ok())
                    {
                        if let Some(ctx) = domain::safe_integer(m.get("context_max")) {
                            out.push_str(&format!("  {m_id} (context_max={ctx})\n"));
                        } else {
                            out.push_str(&format!("  {m_id}\n"));
                        }
                    }
                }
                out.push_str(&format!("Saved model unchanged: {saved_model}\n"));
                return Ok(out);
            }
            let mut out = String::from("Available models:\n");
            for (idx, m) in models.iter().enumerate() {
                if let Some(m_id) = m
                    .get("id")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                {
                    out.push_str(&format!("  {}. {m_id}\n", idx + 1));
                }
            }
            let selected_model = model.clone().ok_or_else(|| {
                Error::type_error("Fleet questions need a terminal or explicit flags")
            })?;
            let matched = models
                .iter()
                .find(|m| domain::is_text(m.get("id"), &selected_model))
                .ok_or_else(|| {
                    Error::new(
                        "Error",
                        "Requested model is not in the endpoint models list",
                    )
                })?;
            let reported_ctx = domain::safe_integer(matched.get("context_max"));
            let mut updated_prof = profile.as_value().clone();
            domain::put(&mut updated_prof, "model", Value::string(&selected_model))?;
            if let Some(ctx) = reported_ctx {
                domain::put(&mut updated_prof, "context_max", Value::Number(ctx as f64))?;
            }
            let updated_profiles = fleet_val
                .profiles()
                .iter()
                .map(|item| {
                    if item.as_value().get("id") == profile.as_value().get("id") {
                        updated_prof.clone()
                    } else {
                        item.as_value().clone()
                    }
                })
                .collect::<Vec<_>>();
            let updated_fleet = validate(&Value::object(vec![(
                "profiles",
                Value::Array(updated_profiles),
            )]))?;
            let mut docs = vec![settings::Document {
                name: settings::Name::Fleet,
                source: format(&updated_fleet.as_value())?,
                expected_source: Some(previous_fleet),
            }];
            let mut names = vec![settings::Name::Fleet];
            if matches_default(&cfg_val, profile.as_value()) {
                let next_cfg = with_profile(&cfg_val, &updated_prof)?;
                docs.push(settings::Document {
                    name: settings::Name::Config,
                    source: config::format(&next_cfg)?,
                    expected_source: Some(prev_cfg),
                });
                names.push(settings::Name::Config);
            }
            settings::ensure_private_files_ignored(&repo_root, &names, token).await?;
            settings::write_private_documents(&repo_root, &docs)?;
            out.push_str(&format!(
                "Updated saved model for {id}. {}",
                match reported_ctx {
                    Some(ctx) => format!("Using context_max={ctx} reported by /v1/models.\n"),
                    None => "Recheck its declared context limit for the new model.\n".to_owned(),
                }
            ));
            Ok(out)
        }
        FleetCliCommand::Assist => Err(Error::new("Error", "roster fleet assist needs a terminal")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../examples/fleet.yml");

    #[test]
    fn listed_fleet_text_keeps_validated_fields_and_lossless_internal_text() -> Result<(), Error> {
        let fleet = parse(&Text::from(EXAMPLE))?;
        let output = list(&fleet)?;
        assert!(output.0.starts_with(
            &Text::from("ID\tMODEL\tBASE_URL\tCONTEXT_MAX\tCONCURRENCY\tHARDWARE\tTASK_CLASS\n").0
        ));
        for profile in fleet.profiles() {
            assert!(
                output.to_utf8()?.contains(
                    &domain::string(
                        profile
                            .as_value()
                            .get("id")
                            .ok_or_else(|| Error::type_error("Missing owned profile ID"))?
                    )?
                    .to_utf8()?
                )
            );
        }
        let mut profile = fleet
            .profiles()
            .first()
            .ok_or_else(|| Error::type_error("Missing owned profile"))?
            .as_value()
            .clone();
        put(&mut profile, "hardware", Value::String(Text(vec![0xd800])))?;
        let fleet = validate(&Value::object(vec![(
            "profiles",
            Value::Array(vec![profile]),
        )]))?;
        let output = list(&fleet)?;
        assert!(output.0.contains(&0xd800));
        assert!(output.to_utf8().is_err());
        Ok(())
    }

    #[test]
    fn single_line_fleet_fields_refuse_terminal_controls_and_unicode_line_separators()
    -> Result<(), Error> {
        let fleet = parse(&Text::from(EXAMPLE))?;
        let original = fleet
            .profiles()
            .first()
            .ok_or_else(|| Error::type_error("Missing owned profile"))?
            .as_value();
        for field in ["hardware", "notes"] {
            for unit in [27, 127, 133, 155, 8232, 8233] {
                let mut profile = original.clone();
                put(&mut profile, field, Value::String(Text(vec![unit])))?;
                assert!(validate_profile(&profile).is_err(), "{field}: {unit}");
            }
        }
        Ok(())
    }

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

    #[test]
    fn hardware_index_and_cost_reflect_dedicated_cluster_and_last_resort_classes()
    -> Result<(), Error> {
        let spark = describe_hardware("dgx-spark-4n")?;
        assert_eq!(spark.get("class"), Some(&Value::string("cluster")));
        assert_eq!(spark.get("memory_gb"), Some(&Value::Number(512.0)));
        assert_eq!(spark.get("nodes"), Some(&Value::Number(4.0)));
        assert!(describe_hardware("unknown-sku").is_err());

        let rtx3090 = Value::object(vec![
            ("hardware", Value::string("rtx-3090-24gb")),
            ("context_max", Value::Number(32_768.0)),
        ]);
        let dedicated = Value::object(vec![
            ("hardware", Value::string("rtx-6000-ada-48gb")),
            ("context_max", Value::Number(65_536.0)),
        ]);
        let spark_1n = Value::object(vec![
            ("hardware", Value::string("dgx-spark-gb10-128gb")),
            ("context_max", Value::Number(65_536.0)),
        ]);
        let cluster = Value::object(vec![
            ("hardware", Value::string("dgx-spark-2n")),
            ("context_max", Value::Number(1_000_000.0)),
        ]);
        assert!(hardware_cost(&dedicated, 2)? < hardware_cost(&cluster, 2)?);
        assert!(hardware_cost(&spark_1n, 4)? < hardware_cost(&dedicated, 4)?);
        assert!(hardware_cost(&cluster, 4)? < hardware_cost(&cluster, 2)?);
        assert!(hardware_cost(&cluster, 2)? < hardware_cost(&rtx3090, 2)?);
        let surrogate_hw = Value::object(vec![
            ("hardware", Value::String(Text(vec![0xd800]))),
            ("context_max", Value::Number(32_768.0)),
        ]);
        assert!((hardware_cost(&surrogate_hw, 2)? - 20.032_768).abs() < 1e-9);
        assert!(hardware_cost(&Value::Null, 2).is_err());
        Ok(())
    }

    #[test]
    fn choose_route_and_format_route_honor_evals_priors_ceilings_exclusions_and_hardware()
    -> Result<(), Error> {
        let fleet_val = crate::json::parse(&Text::from(
            r#"{"profiles":[{"id":"fast","base_url":"https://fast.example.invalid/v1","model":"owner/fast","provider":"vllm","context_max":32768,"concurrency":4,"hardware":"test-gpu","task_class":["fix"],"notes":""},{"id":"steady","base_url":"https://steady.example.invalid/v1","model":"owner/steady","provider":"vllm","context_max":65536,"concurrency":1,"hardware":"test-gpu","task_class":["feat"],"notes":""}]}"#,
        ))?;
        let caps_val = crate::json::parse(&Text::from(
            r#"{"capabilities":[{"profile_id":"fast","task_class":"fix","suggested_difficulty":5,"context_max":32768,"concurrency":4,"notes":"Starting guess."},{"model_id":"owner/steady","task_class":"fix","suggested_difficulty":1,"context_max":65536,"concurrency":1,"notes":"Starting guess."}]}"#,
        ))?;
        let samples = |model: &str, count: usize, verdict: &str| -> Result<Vec<Value>, Error> {
            (0..count)
                .map(|index| {
                    let session = format!("{}-{index}", model.replace('/', "-"));
                    crate::json::parse(&Text::from(
                        format!(
                            r#"{{"model":"{model}","task_class":"fix","effort":"h","session":"{session}","evaluation":{{"session":"{session}","verdict":"{verdict}","difficulty":2,"again":true,"minutes":10}}}}"#
                        )
                        .as_str(),
                    ))
                })
                .collect()
        };
        let steady_evals = samples("owner/steady", 3, "accept")?;
        let eval_choice = choose_route(&RouteOptions {
            fleet: &fleet_val,
            capabilities: &caps_val,
            records: &steady_evals,
            task_class: "fix",
            difficulty: 2,
            context_required: 0,
            profile_id: None,
            seat: "coder",
            excluded_profile_ids: &[],
            queue_depth: None,
            now: 1_700_000_000_000,
        })?
        .ok_or_else(|| Error::type_error("Expected evaluated route"))?;
        assert_eq!(
            eval_choice.get("profile").and_then(|p| p.get("id")),
            Some(&Value::string("steady"))
        );
        assert_eq!(eval_choice.get("source"), Some(&Value::string("evals")));
        let formatted = format_route(Some(&eval_choice), "fix", &Value::Null, &Value::Null)?;
        assert!(formatted.contains("profile=steady source=evals"));
        assert!(formatted.contains("origin=local-human-evaluations n=3 accepted=3 rejected=0"));

        for count in [0, 2] {
            let partial = samples("owner/steady", count, "accept")?;
            let prior_choice = choose_route(&RouteOptions {
                fleet: &fleet_val,
                capabilities: &caps_val,
                records: &partial,
                task_class: "fix",
                difficulty: 2,
                context_required: 0,
                profile_id: None,
                seat: "coder",
                excluded_profile_ids: &[],
                queue_depth: None,
                now: 1_700_000_000_000,
            })?
            .ok_or_else(|| Error::type_error("Expected prior route"))?;
            assert_eq!(
                prior_choice.get("profile").and_then(|p| p.get("id")),
                Some(&Value::string("fast"))
            );
            assert_eq!(prior_choice.get("source"), Some(&Value::string("prior")));
        }

        let excluded_choice = choose_route(&RouteOptions {
            fleet: &fleet_val,
            capabilities: &caps_val,
            records: &[],
            task_class: "fix",
            difficulty: 2,
            context_required: 0,
            profile_id: None,
            seat: "coder",
            excluded_profile_ids: &["fast".to_owned()],
            queue_depth: None,
            now: 1_700_000_000_000,
        })?;
        assert!(excluded_choice.is_none());

        let rejected = samples("owner/fast", 1, "reject")?;
        assert!(
            choose_route(&RouteOptions {
                fleet: &fleet_val,
                capabilities: &caps_val,
                records: &rejected,
                task_class: "fix",
                difficulty: 2,
                context_required: 0,
                profile_id: None,
                seat: "coder",
                excluded_profile_ids: &[],
                queue_depth: None,
                now: 1_700_000_000_000,
            })?
            .is_none()
        );
        assert!(
            choose_route(&RouteOptions {
                fleet: &fleet_val,
                capabilities: &caps_val,
                records: &[],
                task_class: "fix",
                difficulty: 2,
                context_required: 0,
                profile_id: None,
                seat: "_coder",
                excluded_profile_ids: &["_excluded".to_owned()],
                queue_depth: None,
                now: 1_700_000_000_000,
            })?
            .is_some()
        );

        let candidates = discovery_candidates(
            Some(&Value::object(vec![(
                "ROSTER_DISCOVER_HOSTS",
                Value::string("http://10.0.0.1:8000/v1"),
            )])),
            &[],
        );
        assert!(candidates.contains(&"http://127.0.0.1:8000/v1".to_owned()));
        assert!(candidates.contains(&"http://10.0.0.1:8000/v1".to_owned()));

        let merged = merge_discovered_profiles(
            &fleet_val,
            &[Value::object(vec![
                ("baseUrl", Value::string("http://127.0.0.1:8000/v1")),
                (
                    "models",
                    Value::Array(vec![Value::object(vec![
                        ("id", Value::string("owner/discovered")),
                        ("context_max", Value::Number(16_384.0)),
                    ])]),
                ),
            ])],
        )?;
        assert_eq!(merged.profiles().len(), 3);

        let merged_profiles = merged
            .profiles()
            .iter()
            .map(|p| p.as_value().clone())
            .collect::<Vec<_>>();
        let rem_profile = select_remediation_profile(&merged_profiles, None)
            .ok_or_else(|| Error::new("TestError", "remediation profile"))?;
        assert_eq!(rem_profile.get("id"), Some(&Value::string("steady")));
        let rem_task = remediation_task(
            &["src/app.js".to_owned(), "tests/app.test.mjs".to_owned()],
            "repo",
        )?;
        assert_eq!(
            rem_task.get("files"),
            Some(&Value::Array(vec![Value::string("tests/app.test.mjs")]))
        );
        assert!(remediation_task(&["src/app.js".to_owned()], "repo").is_err());
        assert_eq!(
            parse_fleet_cli_args(&[
                "discover".to_owned(),
                "--wsl".to_owned(),
                "--save".to_owned()
            ])?,
            FleetCliCommand::Discover {
                wsl: true,
                save: true
            }
        );
        assert_eq!(
            parse_fleet_cli_args(&[
                "add".to_owned(),
                "--id".to_owned(),
                "local-gpu".to_owned(),
                "--base-url".to_owned(),
                "http://127.0.0.1:8000/v1/".to_owned(),
            ])?,
            FleetCliCommand::Add {
                id: "local-gpu".to_owned(),
                base_url: "http://127.0.0.1:8000/v1".to_owned(),
                model: None,
                context: None,
                concurrency: None,
                hardware: None,
                task_class: None,
            }
        );
        let (hw_fleet, _, hw_msg) = apply_fleet_hardware(&merged, "steady", "rtx-3090-24gb")?;
        assert!(hw_msg.contains("Hardware for steady: rtx-3090-24gb (24GB, 1 node(s))."));
        let base_cfg = config::defaults()?;
        let (def_cfg, _, def_msg) = apply_fleet_default(base_cfg.as_value(), &hw_fleet, "steady")?;
        assert_eq!(def_msg, "Default endpoint/model selected: steady\n");
        assert!(apply_fleet_remove(&def_cfg, &hw_fleet, "steady").is_err());
        let (after_rm, rm_msg) = apply_fleet_remove(&def_cfg, &hw_fleet, "fast")?;
        assert_eq!(rm_msg, "Removed fleet profile: fast\n");
        assert_eq!(after_rm.profiles().len(), 2);

        let inv = parse_model_inventory_response(
            &crate::json::parse(&Text::from(
                r#"{"data":[{"id":"owner/model"},{"id":"owner/model","max_model_len":32768}]}"#,
            ))?,
            Some(&Value::object(Vec::new())),
            None,
        )?;
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].get("context_max"), Some(&Value::Number(32_768.0)));
        Ok(())
    }
}
