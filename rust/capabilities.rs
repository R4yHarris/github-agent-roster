use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::domain::{self, items, members, put, safe_integer};
use crate::fleet::{self, TaskClass};
use crate::json::Value;
use crate::text::{Text, ascii, trim};
use crate::{Error, MAX_SAFE_INTEGER, catalog, config, paths, settings};

const FIELDS: &[&str] = &[
    "profile_id",
    "model_id",
    "task_class",
    "suggested_difficulty",
    "context_max",
    "concurrency",
    "notes",
];
const REQUIRED: &[&str] = &[
    "suggested_difficulty",
    "context_max",
    "concurrency",
    "notes",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    Full,
    Partial,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Capabilities {
    records: Vec<Value>,
}

impl Capabilities {
    pub fn records(&self) -> &[Value] {
        &self.records
    }

    pub fn as_value(&self) -> Value {
        Value::object(vec![("capabilities", Value::Array(self.records.clone()))])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Selector {
    Profile(Text),
    Model(Text),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    selector: Selector,
    task_class: TaskClass,
}

fn invalid_record() -> Error {
    Error::type_error(
        "Capability prior needs one profile_id or model_id, a task_class, and documented fields",
    )
}

fn key(record: &Value) -> Result<Key, Error> {
    let selector = if let Some(value) = record.get("profile_id").and_then(Value::as_string) {
        Selector::Profile(value.clone())
    } else {
        Selector::Model(
            record
                .get("model_id")
                .and_then(Value::as_string)
                .cloned()
                .ok_or_else(invalid_record)?,
        )
    };
    Ok(Key {
        selector,
        task_class: TaskClass::parse(record.get("task_class")).ok_or_else(invalid_record)?,
    })
}

fn has_ipv4(units: &[u16]) -> bool {
    for start in 0..units.len() {
        let mut position = start;
        let mut groups = 0;
        while groups < 3 {
            let digits = units
                .get(position..)
                .into_iter()
                .flatten()
                .take(3)
                .take_while(|unit| (48..=57).contains(*unit))
                .count();
            if digits == 0 || units.get(position + digits) != Some(&46) {
                break;
            }
            position += digits + 1;
            groups += 1;
        }
        if groups == 3
            && units
                .get(position)
                .is_some_and(|unit| (48..=57).contains(unit))
        {
            return true;
        }
    }
    false
}

fn has_ipv6(units: &[u16]) -> bool {
    units.iter().enumerate().any(|(index, unit)| {
        if *unit != 58 {
            return false;
        }
        let digits = units
            .get(index + 1..)
            .into_iter()
            .flatten()
            .take(4)
            .take_while(|unit| matches!(unit, 48..=57 | 65..=70 | 97..=102))
            .count();
        units.get(index + 1 + digits) == Some(&58)
    })
}

fn validate_record(record: &Value, completeness: Completeness) -> Result<Value, Error> {
    if members(record).is_none()
        || members(record).is_some_and(|entries| {
            entries
                .iter()
                .any(|(name, _)| !FIELDS.iter().any(|field| ascii(&name.0, field)))
        })
        || record.get("profile_id").is_some() == record.get("model_id").is_some()
        || TaskClass::parse(record.get("task_class")).is_none()
        || completeness == Completeness::Full
            && REQUIRED.iter().any(|name| record.get(name).is_none())
    {
        return Err(invalid_record());
    }
    if record.get("profile_id").is_some() {
        fleet::validate_id(record.get("profile_id"))?;
    } else {
        let model = fleet::actual_model(record.get("model_id"))?;
        if ascii(&model.0, "builtin-stub") {
            return Err(Error::type_error(
                "Capability model_id must name an actual model",
            ));
        }
    }
    if record.get("suggested_difficulty").is_some()
        && domain::safe_integer(record.get("suggested_difficulty"))
            .is_none_or(|number| !(1..=5).contains(&number))
    {
        return Err(Error::type_error(
            "Capability suggested_difficulty must be an integer from 1 to 5",
        ));
    }
    for field in ["context_max", "concurrency"] {
        if record.get(field).is_some()
            && domain::safe_integer(record.get(field)).is_none_or(|number| number == 0)
        {
            return Err(Error::type_error(format!(
                "Capability {field} must be a positive safe integer"
            )));
        }
    }
    if record.get("notes").is_some()
        && record
            .get("notes")
            .and_then(Value::as_string)
            .is_none_or(|value| {
                trim(&value.0).is_empty()
                    || value.0 != trim(&value.0)
                    || value.0.len() > 240
                    || value.0.iter().any(|unit| matches!(unit, 0..=31 | 127))
                    || has_ipv4(&value.0)
                    || has_ipv6(&value.0)
            })
    {
        return Err(Error::type_error(
            "Capability notes must be short single-line text without IP addresses",
        ));
    }
    Ok(record.clone())
}

pub fn validate(catalog: &Value, completeness: Completeness) -> Result<Capabilities, Error> {
    let records = catalog
        .get("capabilities")
        .and_then(items)
        .filter(|_| members(catalog).is_some_and(|entries| entries.len() == 1))
        .ok_or_else(|| Error::type_error("Capabilities must contain only a capabilities list"))?;
    let records = records
        .iter()
        .map(|record| validate_record(record, completeness))
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = HashSet::new();
    for record in &records {
        if !seen.insert(key(record)?) {
            return Err(Error::type_error(
                "Capability selector and task_class pairs must be unique",
            ));
        }
    }
    Ok(Capabilities { records })
}

pub fn parse(source: &Text, completeness: Completeness) -> Result<Capabilities, Error> {
    let records = domain::catalog_values(catalog::parse(source, "capabilities", FIELDS)?);
    validate(
        &Value::object(vec![("capabilities", Value::Array(records))]),
        completeness,
    )
}

pub fn format(catalog: &Value, completeness: Completeness) -> Result<Text, Error> {
    domain::format_catalog("capabilities", &validate(catalog, completeness)?.records)
}

pub fn merge(baseline: &Value, overlay: &Value) -> Result<Capabilities, Error> {
    let mut baseline = validate(baseline, Completeness::Full)?;
    let overlay = validate(overlay, Completeness::Partial)?;
    let mut positions = baseline
        .records
        .iter()
        .enumerate()
        .map(|(position, record)| key(record).map(|key| (key, position)))
        .collect::<Result<HashMap<_, _>, _>>()?;
    for record in overlay.records {
        let selector = key(&record)?;
        let position = positions.get(&selector).copied();
        if let Some(position) = position {
            let existing = baseline.records.get(position).ok_or_else(invalid_record)?;
            let merged = validate_record(&domain::merge(existing, &record)?, Completeness::Full)?;
            let existing = baseline
                .records
                .get_mut(position)
                .ok_or_else(invalid_record)?;
            *existing = merged;
        } else {
            positions.insert(selector, baseline.records.len());
            baseline
                .records
                .push(validate_record(&record, Completeness::Full)?);
        }
    }
    validate(&baseline.as_value(), Completeness::Full)
}

pub fn builtin() -> Result<Capabilities, Error> {
    parse(
        &Text::from(include_str!("../examples/capabilities.yml")),
        Completeness::Full,
    )
}

pub fn model_prior(
    model: &Value,
    task_class: Option<&Value>,
    catalog: Option<&Value>,
) -> Result<Value, Error> {
    let default_task = Value::string("feat");
    let task_class = task_class.unwrap_or(&default_task);
    if TaskClass::parse(Some(task_class)).is_none() {
        return Err(Error::type_error("Model prior requires a known task class"));
    }
    let builtin = builtin()?;
    let selected = catalog
        .filter(|catalog| **catalog != Value::Null)
        .cloned()
        .unwrap_or_else(|| builtin.as_value());
    let selected = validate(&selected, Completeness::Full)?;
    for record in selected.records {
        if record.get("model_id") == Some(model) && record.get("task_class") == Some(task_class) {
            let difficulty = domain::safe_integer(record.get("suggested_difficulty"))
                .ok_or_else(invalid_record)?;
            let mut result = record;
            put(
                &mut result,
                "strength",
                Value::string(if difficulty >= 4 {
                    "strong"
                } else if difficulty == 3 {
                    "standard"
                } else {
                    "limited"
                }),
            )?;
            return Ok(result);
        }
    }
    Ok(Value::object(vec![
        ("strength", Value::string("unknown")),
        ("suggested_difficulty", Value::Number(2.0)),
    ]))
}

pub fn load(repo_root: &Path, installation_root: &Path) -> Result<Capabilities, Error> {
    let file = installation_root.join("examples").join("capabilities.yml");
    paths::ensure_local_path(&file, installation_root)?;
    let baseline = parse(&config::read_source(&file)?, Completeness::Full)?;
    match config::read_private_source(repo_root, settings::Name::Capabilities)? {
        Some(source) => merge(
            &baseline.as_value(),
            &parse(&source, Completeness::Partial)?.as_value(),
        ),
        None => Ok(baseline),
    }
}

const CONTROL_NAMES: [&str; 9] = [
    "immutableTask",
    "exactScope",
    "boundedCallbacks",
    "candidateHashes",
    "cancellation",
    "deadline",
    "exitAndCleanup",
    "separateProvenance",
    "independentReview",
];
const AUTHORITY_NAMES: [&str; 6] = ["launch", "network", "auth", "shell", "git", "publisher"];
const LIMIT_NAMES: [&str; 5] = ["files", "bytes", "callbacks", "events", "duration"];
const LIVE_BLOCKERS: [&str; 7] = [
    "production-capability-bridge-unqualified",
    "physical-containment-unqualified",
    "inference-auth-unapproved",
    "account-data-model-spend-unapproved",
    "remote-cancellation-unqualified",
    "authoring-provenance-unverified",
    "dependency-gate-unresolved",
];

fn closed_record(
    value: Option<&Value>,
    fields: &[&str],
    label: &str,
    required: bool,
) -> Result<Value, Error> {
    let Some(value) = value else {
        return Err(Error::type_error(format!("{label} must be a plain record")));
    };
    let entries = members(value)
        .ok_or_else(|| Error::type_error(format!("{label} must be a plain record")))?;
    let mut seen = HashSet::new();
    for (key, _) in entries {
        let name = key.to_utf8().map_err(|_| {
            Error::type_error(format!("{label} must use only the documented fields"))
        })?;
        if !fields.contains(&name.as_str()) || !seen.insert(name) {
            return Err(Error::type_error(format!(
                "{label} must use only the documented fields"
            )));
        }
    }
    if required && fields.iter().any(|field| !seen.contains(*field)) {
        return Err(Error::type_error(format!(
            "{label} must use only the documented fields"
        )));
    }
    Ok(value.clone())
}

pub fn assess_copilot_worker_capabilities(report: &Value) -> Result<Value, Error> {
    let input = closed_record(
        Some(report),
        &[
            "schema",
            "kind",
            "evidence",
            "concurrency",
            "tools",
            "authority",
            "limits",
            "controls",
        ],
        "Offline worker capability report",
        true,
    )?;
    if input.get("schema") != Some(&Value::Number(1.0))
        || !domain::is_text(input.get("kind"), "external-full-agent")
        || !domain::is_text(input.get("evidence"), "synthetic")
        || input.get("concurrency") != Some(&Value::Number(1.0))
    {
        return Err(Error::type_error(
            "Offline report requires schema 1, external-full-agent, synthetic evidence and concurrency 1",
        ));
    }
    let Some(tools) = input.get("tools").and_then(items) else {
        return Err(Error::type_error(
            "Offline tools must contain only read_file",
        ));
    };
    if tools.len() != 1 || !domain::is_text(tools.first(), "read_file") {
        return Err(Error::type_error(
            "Offline tools must contain only read_file",
        ));
    }
    let authority = closed_record(
        input.get("authority"),
        &AUTHORITY_NAMES,
        "Offline authority",
        true,
    )?;
    if AUTHORITY_NAMES
        .iter()
        .any(|key| authority.get(key) != Some(&Value::Boolean(false)))
    {
        return Err(Error::type_error(
            "Offline launch, network, auth, shell, git and publisher authority must be false",
        ));
    }
    let limits = closed_record(input.get("limits"), &LIMIT_NAMES, "Offline limits", true)?;
    if LIMIT_NAMES
        .iter()
        .any(|key| domain::safe_integer(limits.get(key)).is_none_or(|value| value < 1))
    {
        return Err(Error::type_error(
            "Offline limits must all be finite positive safe integers",
        ));
    }
    let controls = closed_record(
        input.get("controls"),
        &CONTROL_NAMES,
        "Offline controls",
        false,
    )?;
    if let Some(entries) = members(&controls) {
        if entries
            .iter()
            .any(|(_, value)| !matches!(value, Value::Null | Value::Boolean(_)))
        {
            return Err(Error::type_error(
                "Offline controls must be true, false or null (unknown)",
            ));
        }
    }
    let mut unknown_controls = Vec::new();
    let mut unsupported_controls = Vec::new();
    let mut declared_controls = Vec::with_capacity(CONTROL_NAMES.len());
    for name in CONTROL_NAMES {
        match controls.get(name) {
            None | Some(Value::Null) => {
                unknown_controls.push(Value::string(name));
                declared_controls.push((name, Value::Null));
            }
            Some(Value::Boolean(false)) => {
                unsupported_controls.push(Value::string(name));
                declared_controls.push((name, Value::Boolean(false)));
            }
            Some(Value::Boolean(true)) => {
                declared_controls.push((name, Value::Boolean(true)));
            }
            _ => {
                return Err(Error::type_error(
                    "Offline controls must be true, false or null (unknown)",
                ));
            }
        }
    }
    let compatible = unknown_controls.is_empty() && unsupported_controls.is_empty();
    Ok(Value::object(vec![
        ("schema", Value::Number(1.0)),
        ("kind", Value::string("external-full-agent")),
        ("evidence", Value::string("synthetic")),
        (
            "status",
            Value::string(if compatible {
                "offline-contract-compatible"
            } else {
                "blocked"
            }),
        ),
        ("offlineContractCompatible", Value::Boolean(compatible)),
        ("unknownControls", Value::Array(unknown_controls)),
        ("unsupportedControls", Value::Array(unsupported_controls)),
        (
            "capabilities",
            Value::object(vec![
                ("concurrency", Value::Number(1.0)),
                ("tools", Value::Array(vec![Value::string("read_file")])),
                ("authority", authority),
                ("limits", limits),
                ("controls", Value::object(declared_controls)),
            ]),
        ),
        ("liveDispatchEligible", Value::Boolean(false)),
        ("publicationEligible", Value::Boolean(false)),
        (
            "liveBlockers",
            Value::Array(LIVE_BLOCKERS.iter().map(|b| Value::string(b)).collect()),
        ),
    ]))
}

pub const COPILOT_DOCTOR_USAGE: &str =
    "Use roster doctor --copilot [--opt-in] [--runtime PATH] [--model ID].";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotDoctorOptions {
    pub opt_in: bool,
    pub runtime_path: Option<String>,
    pub model: Option<String>,
    pub is_tty: Option<bool>,
    pub auth_state: String,
    pub auth_source: String,
    pub model_state: String,
}

impl Default for CopilotDoctorOptions {
    fn default() -> Self {
        Self {
            opt_in: false,
            runtime_path: None,
            model: None,
            is_tty: None,
            auth_state: "unknown".to_owned(),
            auth_source: "unknown".to_owned(),
            model_state: "unknown".to_owned(),
        }
    }
}

pub fn parse_copilot_doctor_args(args: &[String]) -> Result<CopilotDoctorOptions, Error> {
    let usage = || Error::type_error(COPILOT_DOCTOR_USAGE);
    let mut options = CopilotDoctorOptions::default();
    let mut seen = HashSet::new();
    let mut index = 0;
    while let Some(flag) = args.get(index) {
        if !["--opt-in", "--runtime", "--model"].contains(&flag.as_str())
            || !seen.insert(flag.as_str())
        {
            return Err(usage());
        }
        if flag == "--opt-in" {
            options.opt_in = true;
        } else {
            index += 1;
            let value = args
                .get(index)
                .filter(|value| {
                    !trim(&Text::from(value.as_str()).0).is_empty() && !value.starts_with("--")
                })
                .ok_or_else(usage)?;
            if flag == "--runtime" {
                options.runtime_path = Some(value.clone());
            } else {
                options.model = Some(value.clone());
            }
        }
        index += 1;
    }
    Ok(options)
}

fn copilot_auth_reason(state: &str) -> Option<&'static str> {
    match state {
        "unknown" => Some(
            "Authentication is unverified; offline diagnostics never inspect credentials or contact the provider.",
        ),
        "signed-out" => Some(
            "Declared signed out. After capability and account approval, the human must use official interactive CLI authentication.",
        ),
        "expired" => Some(
            "Declared expired authentication. The human must renew separately approved inference authentication; no automatic login or retry.",
        ),
        "revoked" => Some(
            "Declared revoked authentication. Ask the account administrator to review access before human reauthentication.",
        ),
        "policy-disabled" => Some(
            "Declared Copilot policy disabled. Ask the organization administrator; Roster cannot change policy.",
        ),
        "permission-denied" => Some(
            "Declared permission denied. Review the separately approved inference account and permissions; never reuse the publishing App.",
        ),
        _ => None,
    }
}

fn copilot_source_reason(source: &str) -> Option<&'static str> {
    match source {
        "unknown" => Some(
            "Authentication source is unknown; no ambient credential or CLI login is accepted as inference authorization.",
        ),
        "environment" => Some(
            "Declared environment token source. Ambient tokens are not approved inference credentials and must not be forwarded.",
        ),
        "stored-oauth" => Some(
            "Declared stored CLI OAuth source. Host credential stores must remain inaccessible to the future worker.",
        ),
        "gh-fallback" => Some(
            "Declared gh fallback source. Human gh authentication must not be inherited by the future worker.",
        ),
        "separate-inference" => Some(
            "Declared separate inference source; scope, expiry, revocation and isolation still require independent approval.",
        ),
        _ => None,
    }
}

fn copilot_model_reason(state: &str) -> Option<&'static str> {
    match state {
        "unknown" => Some(
            "Model availability and entitlement are unverified; an exact requested ID does not prove access.",
        ),
        "unavailable" => Some(
            "Declared requested model unavailable. Review the account/model policy; do not silently select another model.",
        ),
        "policy-disabled" => Some(
            "Declared model policy disabled. Ask the organization administrator; Roster cannot enable the model.",
        ),
        _ => None,
    }
}

fn validate_copilot_runtime_path(path: &str) -> Result<(), Error> {
    if trim(&Text::from(path).0).is_empty() || path.bytes().any(|byte| matches!(byte, 0..=31 | 127))
    {
        return Err(Error::type_error(
            "Runtime path must be a nonempty local filesystem path without ASCII control characters.",
        ));
    }
    let bytes = path.as_bytes();
    let starts_two_slashes =
        bytes.len() >= 2 && matches!(bytes[0], b'/' | b'\\') && matches!(bytes[1], b'/' | b'\\');
    let starts_nt_device = bytes.len() >= 4
        && matches!(bytes[0], b'/' | b'\\')
        && bytes[1] == b'?'
        && bytes[2] == b'?'
        && matches!(bytes[3], b'/' | b'\\');
    let drive_colon = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let uri_like = !drive_colon
        && path.split_once(':').is_some_and(|(scheme, _)| {
            !scheme.is_empty()
                && scheme.as_bytes()[0].is_ascii_alphabetic()
                && scheme
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
        });
    if starts_two_slashes || starts_nt_device || uri_like {
        return Err(Error::type_error(
            "Runtime path must be local; network, device namespace and URI paths are not allowed.",
        ));
    }
    Ok(())
}

pub fn check_copilot_doctor(options: &CopilotDoctorOptions) -> Result<Value, Error> {
    let Some(auth_reason) = copilot_auth_reason(&options.auth_state) else {
        return Err(Error::type_error(
            "Invalid offline Copilot diagnostic options; use documented states only.",
        ));
    };
    let Some(source_reason) = copilot_source_reason(&options.auth_source) else {
        return Err(Error::type_error(
            "Invalid offline Copilot diagnostic options; use documented states only.",
        ));
    };
    let Some(model_reason) = copilot_model_reason(&options.model_state) else {
        return Err(Error::type_error(
            "Invalid offline Copilot diagnostic options; use documented states only.",
        ));
    };
    if let Some(runtime_path) = &options.runtime_path {
        validate_copilot_runtime_path(runtime_path)?;
    }
    if let Some(model) = &options.model {
        if !crate::run::is_model_id(model) {
            return Err(Error::type_error(
                "Requested model must be an exact model ID, not a placeholder or whitespace.",
            ));
        }
    }
    let mut runtime_reason = "No runtime path supplied. Installation is unverified; supply a human-selected pinned executable with --runtime PATH for metadata inspection only.";
    let mut runtime_present = false;
    if !options.opt_in {
        runtime_reason = "Runtime inspection skipped: explicitly opt in to offline diagnostics with --opt-in. This does not enable Copilot execution.";
    } else if let Some(runtime_path) = &options.runtime_path {
        match std::fs::symlink_metadata(runtime_path) {
            Ok(metadata) => {
                runtime_present = metadata.is_file() && !crate::state::secure::linked(&metadata);
                runtime_reason = if runtime_present {
                    "Regular file present only; executable identity, version, platform support and containment are unqualified. Mounted-drive locality is not proven; the operator must choose a local filesystem. No executable was launched."
                } else {
                    "Runtime path is not a regular nonsymlink file. Choose a trusted pinned executable; Roster will not install or launch it."
                };
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) || error.raw_os_error() == Some(267) =>
            {
                runtime_reason = "Runtime file is missing. Consult official CLI installation guidance; Roster will not install it.";
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                runtime_reason = "Runtime metadata permission denied. Ask the human to review the path and access; Roster will not change permissions.";
            }
            Err(_) => {
                return Err(Error::new(
                    "Error",
                    "Runtime metadata inspection failed; review the supplied path and filesystem access.",
                ));
            }
        }
    }
    let is_tty = options.is_tty.unwrap_or_else(|| {
        std::io::IsTerminal::is_terminal(&std::io::stdin())
            && std::io::IsTerminal::is_terminal(&std::io::stdout())
    });
    let check = |name: &str, ok: bool, reason: &str| {
        Value::object(vec![
            ("name", Value::string(name)),
            ("ok", Value::Boolean(ok)),
            ("reason", Value::string(reason)),
        ])
    };
    let checks = vec![
        check(
            "offline diagnostic opt-in",
            options.opt_in,
            if options.opt_in {
                "Explicit for this invocation only; no provider or saved default was changed."
            } else {
                "Copilot remains off. Use --opt-in only to inspect offline readiness; local/stub defaults are unchanged."
            },
        ),
        check("runtime file metadata", runtime_present, runtime_reason),
        check(
            "runtime capability qualification",
            false,
            "Unqualified: #571 production containment/capability and auth-mode gates remain. Copilot execution is unavailable.",
        ),
        check(
            "interactive auth handoff",
            is_tty,
            if is_tty {
                "A terminal is present, but diagnostics never start login; authentication remains human-controlled."
            } else {
                "No interactive terminal. Offline reporting is allowed; human authentication must happen separately in a terminal after approval."
            },
        ),
        check("inference authentication", false, auth_reason),
        check("inference auth source", false, source_reason),
        check("model availability", false, model_reason),
        check(
            "account/data/policy/budget approval",
            false,
            "License, policy, repository data disclosure, retention and spend authorization are unknown. Cloud execution stays off.",
        ),
    ];
    Ok(Value::object(vec![
        ("ok", Value::Boolean(false)),
        ("enabled", Value::Boolean(false)),
        ("qualified", Value::Boolean(false)),
        ("evidence", Value::string("offline-declared")),
        ("checks", Value::Array(checks)),
    ]))
}

pub fn format_copilot_doctor(result: &Value) -> Result<String, Error> {
    let invalid = || Error::type_error("Expected blocked offline Copilot diagnostic checks.");
    if result.get("ok") != Some(&Value::Boolean(false))
        || result.get("enabled") != Some(&Value::Boolean(false))
        || result.get("qualified") != Some(&Value::Boolean(false))
        || !domain::is_text(result.get("evidence"), "offline-declared")
    {
        return Err(invalid());
    }
    let checks = result.get("checks").and_then(items).ok_or_else(invalid)?;
    if checks.len() != 8 {
        return Err(invalid());
    }
    let mut lines = Vec::with_capacity(checks.len());
    for entry in checks {
        let name = entry
            .get("name")
            .and_then(Value::as_string)
            .ok_or_else(invalid)?
            .to_utf8()?;
        let Some(Value::Boolean(ok)) = entry.get("ok") else {
            return Err(invalid());
        };
        let reason = entry
            .get("reason")
            .and_then(Value::as_string)
            .ok_or_else(invalid)?
            .to_utf8()?;
        lines.push(format!(
            "{} {name}: {reason}",
            if *ok { "OK" } else { "FAIL" }
        ));
    }
    Ok(format!(
        "Copilot offline diagnostics: BLOCKED (not provider enablement or live qualification)\n{}\nAuth guidance: supported environment tokens take precedence (COPILOT_GITHUB_TOKEN, GH_TOKEN, GITHUB_TOKEN), followed by stored CLI OAuth and possible gh fallback. Diagnostics read none of these sources and launch no child process. Never forward ambient tokens, GITHUB_APP_* publishing credentials, host HOME/keychain or gh login to inference.\nLogin/logout guidance: after separate capability/account approval, the human should follow official CLI authentication and logout instructions in their own terminal. Roster does not extract, display, copy or delete credentials. Logout alone does not remove an environment-token override; the human must review its source separately.\nOfficial guidance: https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli\nInstall guidance: https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli (npm CLI requires Node 22+; Roster remains Node 20 ESM with no runtime dependencies).\n",
        lines.join("\n")
    ))
}

fn single_line_nonempty(value: Option<&Value>) -> Option<String> {
    let text = value?.as_string()?;
    let decoded = text.to_utf8().ok()?;
    let trimmed = decoded.trim();
    if trimmed.is_empty() || decoded.contains(['\0', '\r', '\n']) {
        None
    } else {
        Some(decoded)
    }
}

fn closed_fields(
    value: Option<&Value>,
    required: &[&str],
    optional: &[&str],
    error_msg: &str,
) -> Result<Value, Error> {
    let Some(value) = value else {
        return Err(Error::type_error(error_msg));
    };
    let entries = members(value).ok_or_else(|| Error::type_error(error_msg))?;
    let mut seen = HashSet::new();
    for (k, _) in entries {
        let name = k.to_utf8().map_err(|_| Error::type_error(error_msg))?;
        if (!required.contains(&name.as_str()) && !optional.contains(&name.as_str()))
            || !seen.insert(name)
        {
            return Err(Error::type_error(error_msg));
        }
    }
    if required.iter().any(|req| !seen.contains(*req)) {
        return Err(Error::type_error(error_msg));
    }
    Ok(value.clone())
}

pub fn offline_copilot_inference_context(parent_env: &Value) -> Result<Value, Error> {
    if members(parent_env).is_none() {
        return Err(Error::type_error("Parent environment must be an object"));
    }
    Ok(Value::object(vec![
        ("env", Value::object(Vec::new())),
        ("useLoggedInUser", Value::Boolean(false)),
        ("executable", Value::Null),
        ("ghFallback", Value::string("unavailable-no-process")),
    ]))
}

pub fn run_offline_copilot_turn(
    request: &Value,
    fixture: &Value,
    worktree: &Path,
    allowed_files: &[String],
    parent_env: &Value,
    token: Option<&crate::cancel::CancellationToken>,
) -> Result<Value, Error> {
    if allowed_files.is_empty()
        || allowed_files
            .iter()
            .any(|f| f.trim().is_empty() || f.contains(['\0', '\r', '\n', '*']))
    {
        return Err(Error::type_error(
            "Offline tool scope requires explicit file names",
        ));
    }
    closed_fields(
        Some(request),
        &["model", "messages", "tools"],
        &[],
        "Offline fixture has missing or unknown fields",
    )?;
    let Some(_req_model) = single_line_nonempty(request.get("model")) else {
        return Err(Error::type_error(
            "Offline turn requires bounded text messages and only the Roster read_file tool",
        ));
    };
    let Some(msgs) = request.get("messages").and_then(items) else {
        return Err(Error::type_error(
            "Offline turn requires bounded text messages and only the Roster read_file tool",
        ));
    };
    if msgs.is_empty() {
        return Err(Error::type_error(
            "Offline turn requires bounded text messages and only the Roster read_file tool",
        ));
    }
    for msg in msgs {
        closed_fields(
            Some(msg),
            &["role", "content"],
            &[],
            "Offline turn requires bounded text messages and only the Roster read_file tool",
        )?;
        let Some(role) = single_line_nonempty(msg.get("role")) else {
            return Err(Error::type_error(
                "Offline turn requires bounded text messages and only the Roster read_file tool",
            ));
        };
        if !matches!(role.as_str(), "system" | "user")
            || msg.get("content").and_then(Value::as_string).is_none()
        {
            return Err(Error::type_error(
                "Offline turn requires bounded text messages and only the Roster read_file tool",
            ));
        }
    }
    let Some(req_tools) = request.get("tools").and_then(items) else {
        return Err(Error::type_error(
            "Offline turn requires bounded text messages and only the Roster read_file tool",
        ));
    };
    if req_tools.len() > 1
        || req_tools
            .iter()
            .any(|t| !domain::is_text(Some(t), "read_file"))
    {
        return Err(Error::type_error(
            "Offline turn requires bounded text messages and only the Roster read_file tool",
        ));
    }
    closed_fields(
        Some(fixture),
        &["events"],
        &["delayMs"],
        "Offline fixture has missing or unknown fields",
    )?;
    let Some(ev_list) = fixture.get("events").and_then(items) else {
        return Err(Error::type_error(
            "Offline fixture exceeds its event, delay or byte budget",
        ));
    };
    if !(2..=4).contains(&ev_list.len()) {
        return Err(Error::type_error(
            "Offline fixture exceeds its event, delay or byte budget",
        ));
    }
    if let Some(delay) = fixture.get("delayMs") {
        let Some(ms) = safe_integer(Some(delay)) else {
            return Err(Error::type_error(
                "Offline fixture exceeds its event, delay or byte budget",
            ));
        };
        if ms > 1000 {
            return Err(Error::type_error(
                "Offline fixture exceeds its event, delay or byte budget",
            ));
        }
    }
    let combined_bytes = request.json().len().saturating_add(fixture.json().len());
    if combined_bytes > 65_536 {
        return Err(Error::type_error(
            "Offline fixture exceeds its event, delay or byte budget",
        ));
    }
    let mut usage_val: Option<Value> = None;
    let mut call_val: Option<(String, String, String)> = None;
    let mut content_val: Option<String> = None;
    let mut idle_seen = false;
    for event in ev_list {
        closed_fields(
            Some(event),
            &["type", "data"],
            &[],
            "Offline fixture has missing or unknown fields",
        )?;
        if idle_seen {
            return Err(Error::type_error("Offline fixture has events after idle"));
        }
        let ev_type = single_line_nonempty(event.get("type"))
            .ok_or_else(|| Error::type_error("Offline fixture event type is unavailable"))?;
        let data = event.get("data");
        match ev_type.as_str() {
            "assistant.usage" => {
                if usage_val.is_some() || content_val.is_some() {
                    return Err(Error::type_error("Duplicate or late usage"));
                }
                let d = closed_fields(
                    data,
                    &[],
                    &["model", "inputTokens", "outputTokens", "cost"],
                    "Offline fixture usage is invalid",
                )?;
                if let Some(m) = d.get("model") {
                    if single_line_nonempty(Some(m)).is_none() {
                        return Err(Error::type_error("Offline fixture usage is invalid"));
                    }
                }
                for tok_key in ["inputTokens", "outputTokens"] {
                    if let Some(tv) = d.get(tok_key) {
                        if safe_integer(Some(tv)).is_none() {
                            return Err(Error::type_error("Offline fixture usage is invalid"));
                        }
                    }
                }
                if let Some(cv) = d.get("cost") {
                    let Some(c) = cv.as_number() else {
                        return Err(Error::type_error("Offline fixture usage is invalid"));
                    };
                    if !c.is_finite() || c < 0.0 {
                        return Err(Error::type_error("Offline fixture usage is invalid"));
                    }
                }
                usage_val = Some(d);
            }
            "fixture.tool_request" => {
                if call_val.is_some() || content_val.is_some() {
                    return Err(Error::type_error(
                        "Offline turn permits one tool request before its message",
                    ));
                }
                let d = closed_fields(
                    data,
                    &["id", "name", "arguments"],
                    &[],
                    "Offline fixture requested an unavailable or malformed tool",
                )?;
                let Some(call_id) = single_line_nonempty(d.get("id")) else {
                    return Err(Error::type_error(
                        "Offline fixture requested an unavailable or malformed tool",
                    ));
                };
                if !domain::is_text(d.get("name"), "read_file") || req_tools.len() != 1 {
                    return Err(Error::type_error(
                        "Offline fixture requested an unavailable or malformed tool",
                    ));
                }
                let args_obj = closed_fields(
                    d.get("arguments"),
                    &["path"],
                    &["offset", "max_lines"],
                    "Offline fixture tool arguments are invalid",
                )?;
                let Some(path_str) = single_line_nonempty(args_obj.get("path")) else {
                    return Err(Error::type_error(
                        "Offline fixture tool arguments are invalid",
                    ));
                };
                for opt_k in ["offset", "max_lines"] {
                    if let Some(ov) = args_obj.get(opt_k) {
                        if safe_integer(Some(ov)).is_none_or(|n| n < 1) {
                            return Err(Error::type_error(
                                "Offline fixture tool arguments are invalid",
                            ));
                        }
                    }
                }
                call_val = Some((call_id, "read_file".to_owned(), path_str));
            }
            "assistant.message" => {
                let d = closed_fields(
                    data,
                    &["content"],
                    &[],
                    "Offline fixture requires one assistant message",
                )?;
                if content_val.is_some() {
                    return Err(Error::type_error(
                        "Offline fixture requires one assistant message",
                    ));
                }
                let Some(c_text) = d.get("content").and_then(Value::as_string) else {
                    return Err(Error::type_error(
                        "Offline fixture requires one assistant message",
                    ));
                };
                content_val = Some(c_text.to_utf8()?);
            }
            "session.idle" => {
                closed_fields(
                    data,
                    &[],
                    &[],
                    "Offline fixture has missing or unknown fields",
                )?;
                if content_val.is_none() {
                    return Err(Error::type_error(
                        "Offline fixture idle preceded its message",
                    ));
                }
                idle_seen = true;
            }
            _ => {
                return Err(Error::type_error(
                    "Offline fixture event type is unavailable",
                ));
            }
        }
    }
    let Some(content_str) = content_val else {
        return Err(Error::type_error("Offline fixture did not complete a turn"));
    };
    if !idle_seen || (content_str.trim().is_empty() && call_val.is_none()) {
        return Err(Error::type_error("Offline fixture did not complete a turn"));
    }
    let context = offline_copilot_inference_context(parent_env)?;
    if let Some(tok) = token {
        tok.check()?;
    }
    let mut events_out = vec![Value::object(vec![
        ("type", Value::string("toolset")),
        ("tools", Value::Array(req_tools.to_vec())),
    ])];
    let mut tool_messages = Vec::new();
    if let Some((ref call_id, ref name, ref rel_path)) = call_val {
        if !crate::execution::policy::scope_expansion_file(rel_path)
            || !allowed_files.iter().any(|allowed| allowed == rel_path)
        {
            return Err(Error::new(
                "PolicyDenied",
                "Offline fixture requested a protected or out-of-scope file",
            )
            .coded("ROSTER_POLICY_DENIED"));
        }
        let full_path = worktree.join(rel_path);
        crate::paths::ensure_local_path(&full_path, worktree)?;
        let Some(snap) = crate::state::secure::read(&full_path, 65_536)? else {
            return Err(Error::new("Error", "Requested file does not exist").coded("ENOENT"));
        };
        let read_content =
            String::from_utf8(snap.bytes).map_err(|e| Error::new("Error", e.to_string()))?;
        tool_messages.push(Value::object(vec![
            ("role", Value::string("tool")),
            ("tool_call_id", Value::string(call_id)),
            ("content", Value::string(&read_content)),
        ]));
        events_out.push(Value::object(vec![
            ("type", Value::string("tool-result")),
            ("name", Value::string(name)),
            ("status", Value::string("ok")),
        ]));
    }
    let finish_reason = if call_val.is_some() {
        "tool_calls"
    } else {
        "stop"
    };
    events_out.push(Value::object(vec![
        ("type", Value::string("completion")),
        ("reason", Value::string(finish_reason)),
    ]));
    let model_out = usage_val
        .as_ref()
        .and_then(|u| u.get("model"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut unknown = Vec::new();
    for key in ["model", "inputTokens", "outputTokens", "cost"] {
        if usage_val.as_ref().and_then(|u| u.get(key)).is_none() {
            unknown.push(Value::string(key));
        }
    }
    let usage_out = match &usage_val {
        Some(u) if u.get("inputTokens").is_some() || u.get("outputTokens").is_some() => {
            let mut fields = Vec::new();
            if let Some(inp) = u.get("inputTokens") {
                fields.push(("prompt_tokens", inp.clone()));
            }
            if let Some(outp) = u.get("outputTokens") {
                fields.push(("completion_tokens", outp.clone()));
            }
            Value::object(fields)
        }
        _ => Value::Null,
    };
    let reported_cost = usage_val
        .as_ref()
        .and_then(|u| u.get("cost"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut message_fields = vec![
        ("role", Value::string("assistant")),
        ("content", Value::string(&content_str)),
    ];
    if let Some((call_id, name, rel_path)) = call_val {
        let arg_json = Value::object(vec![("path", Value::string(&rel_path))]).json();
        message_fields.push((
            "tool_calls",
            Value::Array(vec![Value::object(vec![
                ("id", Value::string(&call_id)),
                ("type", Value::string("function")),
                (
                    "function",
                    Value::object(vec![
                        ("name", Value::string(&name)),
                        ("arguments", Value::string(&arg_json)),
                    ]),
                ),
            ])]),
        ));
    }
    Ok(Value::object(vec![
        ("mode", Value::string("offline-fixture")),
        (
            "response",
            Value::object(vec![
                ("message", Value::object(message_fields)),
                ("model", model_out.clone()),
                ("usage", usage_out),
                ("finish_reason", Value::string(finish_reason)),
            ]),
        ),
        ("toolMessages", Value::Array(tool_messages)),
        ("events", Value::Array(events_out)),
        (
            "provenance",
            Value::object(vec![
                ("provider", Value::string("offline-fixture")),
                ("model", model_out),
                ("unknown", Value::Array(unknown)),
                ("reportedCost", reported_cost),
                ("publishable", Value::Boolean(false)),
            ]),
        ),
        ("context", context),
        (
            "lifecycle",
            Value::object(vec![
                ("active", Value::Boolean(false)),
                ("turns", Value::Number(1.0)),
                ("aborted", Value::Boolean(false)),
                ("disconnected", Value::Boolean(true)),
                ("stopped", Value::Boolean(true)),
            ]),
        ),
    ]))
}

fn canonicalize_bounded_json(value: &Value) -> Result<Value, Error> {
    let mut nodes = 0_usize;
    let mut bytes = 0_usize;
    fn walk(
        item: &Value,
        depth: usize,
        nodes: &mut usize,
        bytes: &mut usize,
    ) -> Result<Value, Error> {
        *nodes += 1;
        if *nodes > 4096 || depth > 16 {
            return Err(Error::new(
                "Error",
                "Contract JSON exceeds structural bounds",
            ));
        }
        match item {
            Value::Null | Value::Boolean(_) => {
                *bytes = bytes.saturating_add(8);
                if *bytes > 2_000_000 {
                    return Err(Error::new("Error", "Contract JSON exceeds byte bounds"));
                }
                Ok(item.clone())
            }
            Value::Number(num) => {
                if !num.is_finite() {
                    return Err(Error::new("Error", "Expected plain JSON data"));
                }
                *bytes = bytes.saturating_add(8);
                if *bytes > 2_000_000 {
                    return Err(Error::new("Error", "Contract JSON exceeds byte bounds"));
                }
                Ok(Value::Number(*num))
            }
            Value::String(text) => {
                *bytes = bytes.saturating_add(text.utf8_len());
                if *bytes > 2_000_000 {
                    return Err(Error::new("Error", "Contract JSON exceeds byte bounds"));
                }
                Ok(Value::String(text.clone()))
            }
            Value::Array(arr) => {
                *bytes = bytes.saturating_add(8);
                let mut out = Vec::with_capacity(arr.len());
                for child in arr {
                    out.push(walk(child, depth + 1, nodes, bytes)?);
                }
                Ok(Value::Array(out))
            }
            Value::Object(entries) => {
                *bytes = bytes.saturating_add(8);
                let mut sorted = Vec::with_capacity(entries.len());
                for (k, v) in entries {
                    let key_str = k.to_utf8()?;
                    if key_str == "__proto__" {
                        return Err(Error::new("Error", "Invalid JSON property"));
                    }
                    *bytes = bytes.saturating_add(key_str.len());
                    if *bytes > 2_000_000 {
                        return Err(Error::new("Error", "Contract JSON exceeds byte bounds"));
                    }
                    sorted.push((k.clone(), walk(v, depth + 1, nodes, bytes)?));
                }
                sorted.sort_by(|a, b| a.0.0.cmp(&b.0.0));
                Ok(Value::Object(sorted))
            }
        }
    }
    walk(value, 0, &mut nodes, &mut bytes)
}

pub fn offline_copilot_digest(value: &Value) -> Result<String, Error> {
    let canonical = canonicalize_bounded_json(value)?;
    Ok(crate::identity::sha256_hex(canonical.json().as_bytes()))
}

pub fn offline_copilot_host_review(subject: &Value, checks: &Value) -> Result<Value, Error> {
    let Some(list) = items(checks).filter(|l| !l.is_empty()) else {
        return Err(Error::new("Error", "Missing trusted fixture checks"));
    };
    for check in list {
        if !domain::is_text(check.get("source"), "synthetic-host-check")
            || check.get("pass") != Some(&Value::Boolean(true))
        {
            return Err(Error::new("Error", "Missing trusted fixture checks"));
        }
    }
    Ok(Value::object(vec![
        ("source", Value::string("synthetic-independent-host")),
        ("subject", Value::string(&offline_copilot_digest(subject)?)),
        ("checks", Value::string(&offline_copilot_digest(checks)?)),
        ("verdict", Value::string("pass")),
    ]))
}

fn validate_worker_relative_file(value: &str) -> Result<String, Error> {
    if value.trim().is_empty()
        || value.contains('\\')
        || value.contains([':', '\0', '*', '?'])
        || !crate::execution::policy::scope_expansion_file(value)
    {
        return Err(Error::new("Error", "Protected or ambiguous path"));
    }
    for part in value.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || matches!(
                part.to_ascii_lowercase().as_str(),
                "task.md" | "recipe.yml" | "estimate.md" | "result.md" | "review.md" | "agents.md"
            )
        {
            return Err(Error::new("Error", "Protected or ambiguous path"));
        }
        let stem = part.split('.').next().unwrap_or("");
        if matches!(
            stem.to_ascii_lowercase().as_str(),
            "con"
                | "prn"
                | "aux"
                | "nul"
                | "com0"
                | "com1"
                | "com2"
                | "com3"
                | "com4"
                | "com5"
                | "com6"
                | "com7"
                | "com8"
                | "com9"
                | "lpt0"
                | "lpt1"
                | "lpt2"
                | "lpt3"
                | "lpt4"
                | "lpt5"
                | "lpt6"
                | "lpt7"
                | "lpt8"
                | "lpt9"
        ) {
            return Err(Error::new("Error", "Protected or ambiguous path"));
        }
    }
    Ok(value.to_owned())
}

fn validate_worker_scope_list(value: Option<&Value>) -> Result<Vec<String>, Error> {
    let Some(arr) = value.and_then(items) else {
        return Err(Error::new("Error", "Invalid exact scope"));
    };
    if arr.is_empty() || arr.len() > 16 {
        return Err(Error::new("Error", "Invalid exact scope"));
    }
    let mut files = Vec::with_capacity(arr.len());
    let mut seen = HashSet::new();
    for item in arr {
        let raw = single_line_nonempty(Some(item))
            .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
        let valid = validate_worker_relative_file(&raw)?;
        if !seen.insert(valid.to_ascii_lowercase()) {
            return Err(Error::new("Error", "Duplicate scope"));
        }
        files.push(valid);
    }
    Ok(files)
}

fn checked_worker_file(root: &Path, relative: &str) -> Result<std::path::PathBuf, Error> {
    let root_meta =
        std::fs::symlink_metadata(root).map_err(|_| Error::new("Error", "Invalid fixture root"))?;
    if !root_meta.is_dir() || crate::state::secure::linked(&root_meta) {
        return Err(Error::new("Error", "Invalid fixture root"));
    }
    let valid = validate_worker_relative_file(relative)?;
    let parts = valid.split('/').collect::<Vec<_>>();
    let mut current = root.to_path_buf();
    for (idx, part) in parts.iter().enumerate() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                if crate::state::secure::linked(&meta) {
                    return Err(Error::new("Error", "Special or linked file"));
                }
                if idx + 1 < parts.len() {
                    if !meta.is_dir() {
                        return Err(Error::new("Error", "Special or linked file"));
                    }
                } else if !meta.is_file() || crate::paths::file_information(&current)?.links != 1 {
                    return Err(Error::new("Error", "Special or linked file"));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(Error::new("Error", "Special or linked file")),
        }
    }
    Ok(current)
}

#[derive(Debug, Clone)]
pub struct OfflineWorkerAttempt {
    root: std::path::PathBuf,
    read_files: Vec<String>,
    write_files: Vec<String>,
    delete_files: Vec<String>,
    requested_model: String,
    limit_files: usize,
    limit_bytes: usize,
    limit_callbacks: usize,
    limit_events: usize,
    limit_duration: u64,
    anchors: Value,
    state: String,
    reason: Option<String>,
    last_time: u64,
    callback_count: usize,
    event_count: usize,
    idle: bool,
    exit: bool,
    cleanup: bool,
    candidate: Option<Value>,
    imported: bool,
    calls: Vec<Value>,
    seen_ids: HashSet<String>,
}

impl OfflineWorkerAttempt {
    pub fn create(root: &Path, contract: &Value) -> Result<Self, Error> {
        let input = canonicalize_bounded_json(contract)?;
        closed_fields(
            Some(&input),
            &[
                "ask", "task", "base", "run", "attempt", "scope", "config", "limits",
            ],
            &[],
            "Malformed contract shape",
        )?;
        let ask = single_line_nonempty(input.get("ask"))
            .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
        let task = single_line_nonempty(input.get("task"))
            .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
        let base = single_line_nonempty(input.get("base"))
            .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
        let run = single_line_nonempty(input.get("run"))
            .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
        let attempt = single_line_nonempty(input.get("attempt"))
            .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
        let scope = closed_fields(
            input.get("scope"),
            &["read", "write", "delete"],
            &[],
            "Malformed contract shape",
        )?;
        let read_files = validate_worker_scope_list(scope.get("read"))?;
        let write_files = validate_worker_scope_list(scope.get("write"))?;
        let Some(del_arr) = scope.get("delete").and_then(items) else {
            return Err(Error::new("Error", "Invalid deletion scope"));
        };
        let mut delete_files = Vec::with_capacity(del_arr.len());
        let mut del_seen = HashSet::new();
        for item in del_arr {
            let raw = single_line_nonempty(Some(item))
                .ok_or_else(|| Error::new("Error", "Invalid deletion scope"))?;
            let valid = validate_worker_relative_file(&raw)?;
            if !write_files.contains(&valid) || !del_seen.insert(valid.clone()) {
                return Err(Error::new("Error", "Invalid deletion scope"));
            }
            delete_files.push(valid);
        }
        let config = closed_fields(
            input.get("config"),
            &["requestedModel", "executable", "version"],
            &[],
            "Malformed contract shape",
        )?;
        let requested_model = single_line_nonempty(config.get("requestedModel"))
            .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
        let _ = single_line_nonempty(config.get("executable"))
            .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
        let _ = single_line_nonempty(config.get("version"))
            .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
        let limits = closed_fields(
            input.get("limits"),
            &["files", "bytes", "callbacks", "events", "duration"],
            &[],
            "Malformed contract shape",
        )?;
        let check_ceil = |key: &str, max_v: u64| -> Result<u64, Error> {
            safe_integer(limits.get(key))
                .filter(|&v| (1..=max_v).contains(&v))
                .ok_or_else(|| Error::new("Error", "Invalid finite attempt limit"))
        };
        let limit_files = check_ceil("files", 16)? as usize;
        let limit_bytes = check_ceil("bytes", 1_000_000)? as usize;
        let limit_callbacks = check_ceil("callbacks", 32)? as usize;
        let limit_events = check_ceil("events", 128)? as usize;
        let limit_duration = check_ceil("duration", 60_000)?;
        let anchors = canonicalize_bounded_json(&Value::object(vec![
            (
                "ask",
                Value::string(&offline_copilot_digest(&Value::string(&ask))?),
            ),
            (
                "task",
                Value::string(&offline_copilot_digest(&Value::string(&task))?),
            ),
            ("base", Value::string(&base)),
            (
                "baseDigest",
                Value::string(&offline_copilot_digest(&Value::string(&base))?),
            ),
            ("scope", Value::string(&offline_copilot_digest(&scope)?)),
            ("config", Value::string(&offline_copilot_digest(&config)?)),
            ("run", Value::string(&run)),
            ("attempt", Value::string(&attempt)),
        ]))?;
        Ok(Self {
            root: root.to_path_buf(),
            read_files,
            write_files,
            delete_files,
            requested_model,
            limit_files,
            limit_bytes,
            limit_callbacks,
            limit_events,
            limit_duration,
            anchors,
            state: "running".to_owned(),
            reason: None,
            last_time: 0,
            callback_count: 0,
            event_count: 0,
            idle: false,
            exit: false,
            cleanup: false,
            candidate: None,
            imported: false,
            calls: Vec::new(),
            seen_ids: HashSet::new(),
        })
    }

    pub fn anchors(&self) -> &Value {
        &self.anchors
    }

    pub fn revoke(&mut self, why: &str) {
        self.state = "blocked".to_owned();
        if self.reason.is_none() {
            self.reason = Some(why.to_owned());
        }
        self.candidate = None;
        self.imported = false;
    }

    fn check_live(&mut self, now_ms: u64) -> Result<(), Error> {
        if now_ms < self.last_time {
            self.revoke("invalid-clock");
        }
        self.last_time = now_ms;
        if now_ms >= self.limit_duration {
            self.revoke("deadline");
        }
        if self.state == "blocked" {
            return Err(Error::new(
                "Error",
                self.reason.as_deref().unwrap_or("revoked"),
            ));
        }
        Ok(())
    }

    fn check_bound(&self, current: &Value) -> Result<(), Error> {
        if offline_copilot_digest(current)? != offline_copilot_digest(&self.anchors)? {
            return Err(Error::new("Error", "Task/base/scope/config drift"));
        }
        Ok(())
    }

    pub fn callback(&mut self, name: &str, args: &Value, now_ms: u64) -> Result<String, Error> {
        let res = (|| -> Result<String, Error> {
            self.check_live(now_ms)?;
            self.callback_count += 1;
            if self.state != "running" || self.idle || self.callback_count > self.limit_callbacks {
                return Err(Error::new("Error", "Callback revoked or bounded"));
            }
            let copied = canonicalize_bounded_json(args)?;
            closed_fields(Some(&copied), &["path"], &[], "Malformed contract shape")?;
            let raw_path = single_line_nonempty(copied.get("path"))
                .ok_or_else(|| Error::new("Error", "Callback capability denied"))?;
            let rel = validate_worker_relative_file(&raw_path)?;
            if name != "read_file" || !self.read_files.contains(&rel) {
                return Err(Error::new("Error", "Callback capability denied"));
            }
            let target = checked_worker_file(&self.root, &rel)?;
            let Some(snap) = crate::state::secure::read(&target, 1_000_000)? else {
                return Err(Error::new("Error", "Callback file missing"));
            };
            String::from_utf8(snap.bytes).map_err(|e| Error::new("Error", e.to_string()))
        })();
        if let Err(ref err) = res {
            self.revoke(&err.message);
        }
        res
    }

    pub fn observe(&mut self, raw: &Value, now_ms: u64) -> Result<(), Error> {
        let res = (|| -> Result<(), Error> {
            self.check_live(now_ms)?;
            self.event_count += 1;
            if self.state != "running" || self.event_count > self.limit_events {
                return Err(Error::new("Error", "Late or excess event"));
            }
            let event = canonicalize_bounded_json(raw)?;
            let ev_type = single_line_nonempty(event.get("type"))
                .ok_or_else(|| Error::new("Error", "Unknown event"))?;
            match ev_type.as_str() {
                "inference" => {
                    closed_fields(
                        Some(&event),
                        &["type", "id"],
                        &["servedModel", "usage"],
                        "Malformed contract shape",
                    )?;
                    let id = single_line_nonempty(event.get("id"))
                        .ok_or_else(|| Error::new("Error", "Expected nonempty text"))?;
                    if self.idle || !self.seen_ids.insert(id) {
                        return Err(Error::new("Error", "Late or duplicate inference"));
                    }
                    if let Some(sm) = event.get("servedModel") {
                        if single_line_nonempty(Some(sm)).is_none() {
                            return Err(Error::new("Error", "Expected nonempty text"));
                        }
                    }
                    if let Some(u) = event.get("usage") {
                        closed_fields(
                            Some(u),
                            &["unit", "value", "complete"],
                            &[],
                            "Malformed contract shape",
                        )?;
                        if single_line_nonempty(u.get("unit")).is_none()
                            || u.get("value").and_then(Value::as_number).is_none_or(|v| {
                                !v.is_finite() || !(0.0..=MAX_SAFE_INTEGER as f64).contains(&v)
                            })
                            || !matches!(u.get("complete"), Some(Value::Boolean(_)))
                        {
                            return Err(Error::new("Error", "Malformed usage"));
                        }
                    }
                    self.calls.push(event);
                }
                "idle" => {
                    closed_fields(Some(&event), &["type"], &[], "Malformed contract shape")?;
                    if self.idle {
                        return Err(Error::new("Error", "Duplicate idle or active callback"));
                    }
                    self.idle = true;
                }
                "exit" => {
                    closed_fields(
                        Some(&event),
                        &["type", "code"],
                        &[],
                        "Malformed contract shape",
                    )?;
                    if !self.idle || self.exit || safe_integer(event.get("code")) != Some(0) {
                        return Err(Error::new("Error", "Missing idle or failed exit"));
                    }
                    self.exit = true;
                }
                "cleanup" => {
                    closed_fields(
                        Some(&event),
                        &["type", "ok"],
                        &[],
                        "Malformed contract shape",
                    )?;
                    if !self.exit || self.cleanup || event.get("ok") != Some(&Value::Boolean(true))
                    {
                        return Err(Error::new("Error", "Missing exit or cleanup failure"));
                    }
                    self.cleanup = true;
                }
                _ => return Err(Error::new("Error", "Unknown event")),
            }
            Ok(())
        })();
        if let Err(ref err) = res {
            self.revoke(&err.message);
        }
        res
    }

    pub fn finish(&mut self, raw: &Value, now_ms: u64) -> Result<Value, Error> {
        let res = (|| -> Result<Value, Error> {
            self.check_live(now_ms)?;
            if self.state != "running"
                || !self.idle
                || !self.exit
                || !self.cleanup
                || self.calls.is_empty()
            {
                return Err(Error::new(
                    "Error",
                    "Missing idle/exit/cleanup/inference evidence",
                ));
            }
            let envelope = canonicalize_bounded_json(raw)?;
            closed_fields(
                Some(&envelope),
                &["kind", "evidence", "anchors", "files"],
                &["selfReview"],
                "Malformed contract shape",
            )?;
            if !domain::is_text(envelope.get("kind"), "external-full-agent")
                || !domain::is_text(envelope.get("evidence"), "synthetic")
            {
                return Err(Error::new("Error", "Invalid attempt kind"));
            }
            let Some(env_anchors) = envelope.get("anchors") else {
                return Err(Error::new("Error", "Task/base/scope/config drift"));
            };
            self.check_bound(env_anchors)?;
            let Some(file_list) = envelope.get("files").and_then(items) else {
                return Err(Error::new("Error", "Invalid file count"));
            };
            if file_list.is_empty() || file_list.len() > self.limit_files {
                return Err(Error::new("Error", "Invalid file count"));
            }
            let mut seen = HashSet::new();
            let mut total_bytes = 0_usize;
            let mut out_files = Vec::with_capacity(file_list.len());
            for f in file_list {
                closed_fields(
                    Some(f),
                    &["path"],
                    &["content", "delete"],
                    "Malformed contract shape",
                )?;
                let raw_p = single_line_nonempty(f.get("path"))
                    .ok_or_else(|| Error::new("Error", "Scope or duplicate artifact"))?;
                let rel = validate_worker_relative_file(&raw_p)?;
                if !self.write_files.contains(&rel) || !seen.insert(rel.to_ascii_lowercase()) {
                    return Err(Error::new("Error", "Scope or duplicate artifact"));
                }
                let target = checked_worker_file(&self.root, &rel)?;
                if f.get("delete") == Some(&Value::Boolean(true)) && f.get("content").is_none() {
                    if !self.delete_files.contains(&rel) || !target.is_file() {
                        return Err(Error::new("Error", "Deletion denied"));
                    }
                    out_files.push(Value::object(vec![
                        ("delete", Value::Boolean(true)),
                        ("path", Value::string(&rel)),
                    ]));
                    continue;
                }
                if f.get("delete").is_some() {
                    return Err(Error::new("Error", "Malformed artifact bytes"));
                }
                let Some(content_text) = f.get("content").and_then(Value::as_string) else {
                    return Err(Error::new("Error", "Malformed artifact bytes"));
                };
                let content_str = content_text
                    .to_utf8()
                    .map_err(|_| Error::new("Error", "Malformed UTF-8 bytes"))?;
                total_bytes = total_bytes.saturating_add(content_str.len());
                if total_bytes > self.limit_bytes {
                    return Err(Error::new("Error", "Artifact bytes exceeded"));
                }
                let hash = offline_copilot_digest(&Value::string(&content_str))?;
                out_files.push(Value::object(vec![
                    ("content", Value::string(&content_str)),
                    ("hash", Value::string(&hash)),
                    ("path", Value::string(&rel)),
                ]));
            }
            let files_val = Value::Array(out_files);
            let digest = offline_copilot_digest(&files_val)?;
            let cand = canonicalize_bounded_json(&Value::object(vec![
                ("digest", Value::string(&digest)),
                ("files", files_val),
            ]))?;
            self.candidate = Some(cand.clone());
            self.state = "candidate".to_owned();
            Ok(cand)
        })();
        if let Err(ref err) = res {
            self.revoke(&err.message);
        }
        res
    }

    fn current_subject(&self) -> Result<Value, Error> {
        let digest_val = self
            .candidate
            .as_ref()
            .and_then(|c| c.get("digest"))
            .cloned()
            .unwrap_or(Value::Null);
        canonicalize_bounded_json(&Value::object(vec![
            ("anchors", self.anchors.clone()),
            ("candidate", digest_val),
            ("evidence", Value::string("synthetic")),
            ("kind", Value::string("external-full-agent")),
        ]))
    }

    pub fn import_candidate(
        &mut self,
        current_anchors: &Value,
        now_ms: u64,
    ) -> Result<Value, Error> {
        let res = (|| -> Result<Value, Error> {
            self.check_live(now_ms)?;
            if self.state != "candidate" {
                return Err(Error::new("Error", "Import denied"));
            }
            self.check_bound(current_anchors)?;
            let Some(cand) = &self.candidate else {
                return Err(Error::new("Error", "Import denied"));
            };
            let files = cand
                .get("files")
                .and_then(items)
                .ok_or_else(|| Error::new("Error", "Import denied"))?;
            for f in files {
                let rel = single_line_nonempty(f.get("path"))
                    .ok_or_else(|| Error::new("Error", "Import denied"))?;
                checked_worker_file(&self.root, &rel)?;
            }
            for f in files {
                let rel = single_line_nonempty(f.get("path"))
                    .ok_or_else(|| Error::new("Error", "Import denied"))?;
                let target = checked_worker_file(&self.root, &rel)?;
                if f.get("delete") == Some(&Value::Boolean(true)) {
                    std::fs::remove_file(&target)
                        .map_err(|e| Error::new("Error", e.to_string()))?;
                } else {
                    let content = f
                        .get("content")
                        .and_then(Value::as_string)
                        .ok_or_else(|| Error::new("Error", "Import denied"))?
                        .to_utf8()?;
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| Error::new("Error", e.to_string()))?;
                    }
                    std::fs::write(&target, content.as_bytes())
                        .map_err(|e| Error::new("Error", e.to_string()))?;
                }
            }
            self.imported = true;
            self.current_subject()
        })();
        if let Err(ref err) = res {
            self.revoke(&err.message);
        }
        res
    }

    pub fn review_eligibility(
        &mut self,
        receipt: &Value,
        current_anchors: &Value,
        now_ms: u64,
    ) -> Result<Value, Error> {
        let res = (|| -> Result<Value, Error> {
            self.check_live(now_ms)?;
            if self.state != "candidate" || !self.imported {
                return Err(Error::new("Error", "Review denied"));
            }
            self.check_bound(current_anchors)?;
            let Some(cand) = &self.candidate else {
                return Err(Error::new("Error", "Review denied"));
            };
            let files = cand
                .get("files")
                .and_then(items)
                .ok_or_else(|| Error::new("Error", "Review denied"))?;
            for f in files {
                let rel = single_line_nonempty(f.get("path"))
                    .ok_or_else(|| Error::new("Error", "Candidate byte drift"))?;
                let target = checked_worker_file(&self.root, &rel)?;
                if f.get("delete") == Some(&Value::Boolean(true)) {
                    if std::fs::symlink_metadata(&target).is_ok() {
                        return Err(Error::new("Error", "Candidate byte drift"));
                    }
                } else {
                    let expected_hash = single_line_nonempty(f.get("hash"))
                        .ok_or_else(|| Error::new("Error", "Candidate byte drift"))?;
                    let actual_str = std::fs::read_to_string(&target)
                        .map_err(|_| Error::new("Error", "Candidate byte drift"))?;
                    if offline_copilot_digest(&Value::string(&actual_str))? != expected_hash {
                        return Err(Error::new("Error", "Candidate byte drift"));
                    }
                }
            }
            let expected_subject = offline_copilot_digest(&self.current_subject()?)?;
            closed_fields(
                Some(receipt),
                &["source", "subject", "checks", "verdict"],
                &[],
                "Missing or stale independent host receipt",
            )?;
            let valid_checks_hash = single_line_nonempty(receipt.get("checks")).is_some_and(|h| {
                h.len() == 64
                    && h.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
            if !domain::is_text(receipt.get("source"), "synthetic-independent-host")
                || !domain::is_text(receipt.get("verdict"), "pass")
                || !domain::is_text(receipt.get("subject"), &expected_subject)
                || !valid_checks_hash
            {
                return Err(Error::new(
                    "Error",
                    "Missing or stale independent host receipt",
                ));
            }
            Ok(Value::object(vec![
                ("hostReviewEligible", Value::Boolean(true)),
                ("publishable", Value::Boolean(false)),
                ("builtinSeat", Value::Boolean(false)),
                ("aiRun", Value::Null),
            ]))
        })();
        if let Err(ref err) = res {
            self.revoke(&err.message);
        }
        res
    }

    pub fn emit_managed_artifacts(
        &mut self,
        receipt: &Value,
        current_anchors: &Value,
        summary: &str,
        now_ms: u64,
    ) -> Result<Value, Error> {
        let eligibility = self.review_eligibility(receipt, current_anchors, now_ms)?;
        if summary.trim().is_empty() || summary.contains('\0') {
            self.revoke("invalid-summary");
            return Err(Error::new("Error", "Expected nonempty text"));
        }
        let prov = self.provenance();
        let result_body = format!(
            "# Result\n\n- Kind: external-full-agent\n- Evidence: synthetic\n- Requested model: {}\n- Model status: {}\n- Summary: {}\n",
            self.requested_model,
            prov.get("modelStatus")
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .unwrap_or_else(|| "unknown".to_owned()),
            summary.trim()
        );
        let review_body = "# Review\n\nVerdict: pass\nSource: synthetic-independent-host\nNote: Read-only independent review receipt; external full-agent attempt is not a builtin seat and remains nonpublishable.\n";
        let result_path = self.root.join("RESULT.md");
        let review_path = self.root.join("REVIEW.md");
        std::fs::write(&result_path, result_body.as_bytes())
            .map_err(|e| Error::new("Error", e.to_string()))?;
        std::fs::write(&review_path, review_body.as_bytes())
            .map_err(|e| Error::new("Error", e.to_string()))?;
        Ok(Value::object(vec![
            (
                "resultPath",
                Value::string(&result_path.display().to_string()),
            ),
            (
                "reviewPath",
                Value::string(&review_path.display().to_string()),
            ),
            ("reviewVerdict", Value::string("pass")),
            (
                "hostReviewEligible",
                eligibility
                    .get("hostReviewEligible")
                    .cloned()
                    .unwrap_or(Value::Boolean(false)),
            ),
            ("builtinSeat", Value::Boolean(false)),
            ("publishable", Value::Boolean(false)),
            ("aiRun", Value::Null),
        ]))
    }

    pub fn provenance(&self) -> Value {
        let mut models = Vec::new();
        for call in &self.calls {
            if let Some(sm) = single_line_nonempty(call.get("servedModel")) {
                if !models.contains(&sm) {
                    models.push(sm);
                }
            }
        }
        let model_status = if models.len() > 1 {
            "mixed"
        } else if self.calls.is_empty()
            || self
                .calls
                .iter()
                .any(|c| single_line_nonempty(c.get("servedModel")).is_none())
        {
            "unknown"
        } else {
            "known"
        };
        let mismatch = models.iter().any(|m| m != &self.requested_model);
        let mut units = Vec::new();
        let mut all_complete = !self.calls.is_empty();
        let mut any_usage = false;
        let mut all_zero = true;
        let mut sum = 0.0_f64;
        for call in &self.calls {
            if let Some(u) = call.get("usage") {
                any_usage = true;
                if let Some(unit) = single_line_nonempty(u.get("unit")) {
                    if !units.contains(&unit) {
                        units.push(unit);
                    }
                }
                if u.get("complete") != Some(&Value::Boolean(true)) {
                    all_complete = false;
                }
                if let Some(v) = u.get("value").and_then(Value::as_number) {
                    if v != 0.0 {
                        all_zero = false;
                    }
                    sum += v;
                }
            } else {
                all_complete = false;
            }
        }
        let safe_total =
            all_complete && units.len() == 1 && sum.is_finite() && sum <= MAX_SAFE_INTEGER as f64;
        let usage_status = if units.len() > 1 {
            "mixed"
        } else if !any_usage {
            "unknown"
        } else if !all_complete || !safe_total {
            "incomplete"
        } else if all_zero {
            "zero"
        } else {
            "complete"
        };
        Value::object(vec![
            ("kind", Value::string("external-full-agent")),
            ("evidence", Value::string("synthetic")),
            ("builtinSeat", Value::Boolean(false)),
            ("publishable", Value::Boolean(false)),
            ("aiRun", Value::Null),
            ("requestedModel", Value::string(&self.requested_model)),
            (
                "servedModels",
                Value::Array(models.iter().map(|m| Value::string(m)).collect()),
            ),
            ("modelStatus", Value::string(model_status)),
            ("mismatch", Value::Boolean(mismatch)),
            ("observedCalls", Value::Number(self.calls.len() as f64)),
            ("usageStatus", Value::string(usage_status)),
            (
                "units",
                Value::Array(units.iter().map(|u| Value::string(u)).collect()),
            ),
            (
                "total",
                if safe_total {
                    Value::Number(sum)
                } else {
                    Value::Null
                },
            ),
        ])
    }

    pub fn status(&self) -> Value {
        Value::object(vec![
            ("state", Value::string(&self.state)),
            (
                "reason",
                self.reason
                    .as_ref()
                    .map_or(Value::Null, |r| Value::string(r)),
            ),
            ("callbackCount", Value::Number(self.callback_count as f64)),
            ("eventCount", Value::Number(self.event_count as f64)),
            ("imported", Value::Boolean(self.imported)),
            (
                "syntheticCleanupComplete",
                Value::Boolean(self.idle && self.exit && self.cleanup),
            ),
            ("writerDeathProven", Value::Boolean(false)),
            ("publishable", Value::Boolean(false)),
        ])
    }
}

pub fn assess_copilot_smoke_admission(approval: &Value, task: &Value) -> Result<Value, Error> {
    let approval_rec = closed_fields(
        Some(approval),
        &[
            "repository",
            "dataClassification",
            "accountRoute",
            "policyApproved",
            "selectedModels",
            "creditCeiling",
            "stopConditions",
            "approver",
        ],
        &[],
        "Smoke approval must declare all required human fields",
    )?;
    let task_rec = closed_fields(
        Some(task),
        &[
            "baseRevision",
            "allowedFiles",
            "namedTests",
            "independentReviewer",
            "offlinePreflight",
            "wallTimeMs",
            "maxRequests",
        ],
        &[],
        "Smoke task must declare all required bounds",
    )?;
    let mut blockers = Vec::new();
    for str_field in [
        "repository",
        "dataClassification",
        "accountRoute",
        "approver",
    ] {
        if single_line_nonempty(approval_rec.get(str_field)).is_none() {
            blockers.push(Value::string(&format!("missing-{str_field}")));
        }
    }
    if approval_rec.get("policyApproved") != Some(&Value::Boolean(true)) {
        blockers.push(Value::string("policy-unapproved"));
    }
    let models_ok = approval_rec
        .get("selectedModels")
        .and_then(items)
        .is_some_and(|m| {
            !m.is_empty()
                && m.iter().all(|item| {
                    single_line_nonempty(Some(item)).is_some_and(|id| crate::run::is_model_id(&id))
                })
        });
    if !models_ok {
        blockers.push(Value::string("models-unapproved"));
    }
    let ceiling_ok = safe_integer(approval_rec.get("creditCeiling")).is_some_and(|c| c >= 1);
    if !ceiling_ok {
        blockers.push(Value::string("credit-ceiling-invalid"));
    }
    let stops_ok = approval_rec
        .get("stopConditions")
        .and_then(items)
        .is_some_and(|s| {
            !s.is_empty()
                && s.iter()
                    .all(|item| single_line_nonempty(Some(item)).is_some())
        });
    if !stops_ok {
        blockers.push(Value::string("stop-conditions-missing"));
    }
    let rev_ok = single_line_nonempty(task_rec.get("baseRevision")).is_some_and(|rev| {
        matches!(rev.len(), 40 | 64) && rev.bytes().all(|b| b.is_ascii_hexdigit())
    });
    if !rev_ok {
        blockers.push(Value::string("base-revision-unpinned"));
    }
    if validate_worker_scope_list(task_rec.get("allowedFiles")).is_err() {
        blockers.push(Value::string("allowed-files-invalid"));
    }
    let tests_ok = task_rec.get("namedTests").and_then(items).is_some_and(|t| {
        !t.is_empty()
            && t.iter()
                .all(|item| single_line_nonempty(Some(item)).is_some())
    });
    if !tests_ok {
        blockers.push(Value::string("named-tests-missing"));
    }
    if task_rec.get("independentReviewer") != Some(&Value::Boolean(true)) {
        blockers.push(Value::string("independent-reviewer-required"));
    }
    if task_rec.get("offlinePreflight") != Some(&Value::Boolean(true)) {
        blockers.push(Value::string("offline-preflight-required"));
    }
    if !safe_integer(task_rec.get("wallTimeMs")).is_some_and(|ms| (1..=900_000).contains(&ms)) {
        blockers.push(Value::string("wall-time-unbounded"));
    }
    if !safe_integer(task_rec.get("maxRequests")).is_some_and(|r| (1..=32).contains(&r)) {
        blockers.push(Value::string("request-limit-unbounded"));
    }
    let preflight_ready = blockers.is_empty();
    Ok(Value::object(vec![
        (
            "status",
            Value::string(if preflight_ready {
                "smoke-preflight-ready"
            } else {
                "blocked"
            }),
        ),
        ("preflightReady", Value::Boolean(preflight_ready)),
        ("liveExecutionInTests", Value::Boolean(false)),
        ("liveSmokeEligible", Value::Boolean(false)),
        ("blockers", Value::Array(blockers)),
    ]))
}

pub fn evaluate_copilot_calibration_scorecard(manifest: &Value) -> Result<Value, Error> {
    let root = closed_fields(
        Some(manifest),
        &["tiers", "attempts"],
        &["limitations"],
        "Calibration manifest must declare tiers and attempts",
    )?;
    let Some(tiers) = root.get("tiers").and_then(items).filter(|t| !t.is_empty()) else {
        return Err(Error::type_error("Calibration manifest requires tiers"));
    };
    let Some(attempts) = root
        .get("attempts")
        .and_then(items)
        .filter(|a| !a.is_empty())
    else {
        return Err(Error::type_error("Calibration manifest requires attempts"));
    };
    let mut tier_controls = std::collections::BTreeMap::new();
    let mut prior_tier_passed = true;
    let mut progression_blocked = Vec::new();
    let expected_order = ["M1", "M2", "M3"];
    if tiers.len() > expected_order.len() {
        return Err(Error::type_error(
            "Calibration tier id must be M1, M2 or M3",
        ));
    }
    for (idx, tier) in tiers.iter().enumerate() {
        let t = closed_fields(
            Some(tier),
            &[
                "id",
                "difficulty",
                "task",
                "baseRevision",
                "allowedFiles",
                "namedTests",
                "modelSelectionRule",
                "effort",
                "independentReviewer",
                "renewedApproval",
            ],
            &[],
            "Malformed calibration tier",
        )?;
        let id = single_line_nonempty(t.get("id"))
            .ok_or_else(|| Error::type_error("Calibration tier requires id"))?;
        if id != expected_order[idx] {
            return Err(Error::type_error(
                "Calibration tiers must be unique and ordered M1, M2, M3",
            ));
        }
        if t.get("independentReviewer") != Some(&Value::Boolean(true)) {
            return Err(Error::type_error(
                "Calibration tier requires independentReviewer: true",
            ));
        }
        if id != "M1"
            && (!prior_tier_passed || t.get("renewedApproval") != Some(&Value::Boolean(true)))
        {
            progression_blocked.push(Value::string(&id));
        }
        let tier_attempts = attempts
            .iter()
            .filter(|att| domain::is_text(att.get("tier"), &id))
            .collect::<Vec<_>>();
        let mut providers = HashSet::new();
        for att in &tier_attempts {
            if let Some(p) = single_line_nonempty(att.get("provider")) {
                providers.insert(p);
            }
        }
        let tier_passed = providers.len() >= 2
            && tier_attempts.iter().all(|att| {
                domain::is_text(att.get("testResult"), "pass")
                    && domain::is_text(att.get("reviewVerdict"), "pass")
            });
        prior_tier_passed = prior_tier_passed && tier_passed;
        tier_controls.insert(id, t);
    }
    let mut rows = Vec::with_capacity(attempts.len());
    let mut failed_count = 0_usize;
    let mut human_accepted = 0_usize;
    for att in attempts {
        let a = closed_fields(
            Some(att),
            &[
                "tier",
                "provider",
                "attempt",
                "task",
                "baseRevision",
                "allowedFiles",
                "namedTests",
                "modelSelectionRule",
                "effort",
                "independentReviewer",
                "testResult",
                "reviewVerdict",
            ],
            &[
                "servedModel",
                "promptTokens",
                "completionTokens",
                "cachedTokens",
                "aiCredits",
                "reportedCost",
                "elapsedMs",
                "humanEval",
            ],
            "Malformed calibration attempt",
        )?;
        let tier_id = single_line_nonempty(a.get("tier"))
            .ok_or_else(|| Error::type_error("Attempt requires tier"))?;
        let Some(expected_tier) = tier_controls.get(&tier_id) else {
            return Err(Error::type_error("Attempt references undeclared tier"));
        };
        for control_key in [
            "task",
            "baseRevision",
            "allowedFiles",
            "namedTests",
            "modelSelectionRule",
            "effort",
            "independentReviewer",
        ] {
            if a.get(control_key) != expected_tier.get(control_key) {
                return Err(Error::type_error(
                    "Calibration attempt drifted from matched tier controls",
                ));
            }
        }
        let passed_gates = domain::is_text(a.get("testResult"), "pass")
            && domain::is_text(a.get("reviewVerdict"), "pass");
        if !passed_gates {
            failed_count += 1;
        }
        if passed_gates && domain::is_text(a.get("humanEval"), "accept") {
            human_accepted += 1;
        }
        let mut unknowns = Vec::new();
        for opt_field in [
            "servedModel",
            "promptTokens",
            "completionTokens",
            "cachedTokens",
            "aiCredits",
            "reportedCost",
            "elapsedMs",
            "humanEval",
        ] {
            if a.get(opt_field).is_none_or(|v| *v == Value::Null) {
                unknowns.push(Value::string(opt_field));
            }
        }
        let mut row = a.clone();
        domain::put(&mut row, "unknownFields", Value::Array(unknowns))?;
        rows.push(row);
    }
    let has_limitations = root
        .get("limitations")
        .and_then(items)
        .is_some_and(|l| !l.is_empty());
    let superiority_allowed =
        progression_blocked.is_empty() && rows.len() >= 6 && human_accepted >= 6 && has_limitations;
    Ok(Value::object(vec![
        ("totalAttempts", Value::Number(rows.len() as f64)),
        ("failedAttempts", Value::Number(failed_count as f64)),
        (
            "humanAcceptedAttempts",
            Value::Number(human_accepted as f64),
        ),
        ("progressionBlockedTiers", Value::Array(progression_blocked)),
        (
            "superiorityClaimAllowed",
            Value::Boolean(superiority_allowed),
        ),
        ("attempts", Value::Array(rows)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::is_text;

    #[test]
    fn catalog_order_and_partial_overlays_round_trip() -> Result<(), Error> {
        let baseline = builtin()?;
        assert_eq!(baseline.records().len(), 10);
        assert_eq!(
            parse(
                &format(&baseline.as_value(), Completeness::Full)?,
                Completeness::Full,
            )?,
            baseline
        );
        let overlay = parse(
            &Text::from(
                "capabilities:\n  - profile_id: default\n    task_class: fix\n    suggested_difficulty: 4\n",
            ),
            Completeness::Partial,
        )?;
        let merged = merge(&baseline.as_value(), &overlay.as_value())?;
        assert_eq!(merged.records().len(), baseline.records().len());
        let changed = merged
            .records()
            .iter()
            .find(|record| {
                is_text(record.get("profile_id"), "default")
                    && is_text(record.get("task_class"), "fix")
            })
            .ok_or_else(invalid_record)?;
        assert_eq!(
            changed.get("suggested_difficulty"),
            Some(&Value::Number(4.0))
        );
        assert_eq!(changed.get("context_max"), Some(&Value::Number(8192.0)));
        Ok(())
    }

    #[test]
    fn unknown_priors_remain_unknown_and_do_not_register_models() -> Result<(), Error> {
        let prior = model_prior(&Value::string("unrated"), None, None)?;
        assert_eq!(
            prior,
            Value::object(vec![
                ("strength", Value::string("unknown")),
                ("suggested_difficulty", Value::Number(2.0)),
            ])
        );
        let strong = model_prior(&Value::string("deepseek-v4.1-flash"), None, None)?;
        assert!(is_text(strong.get("strength"), "strong"));
        assert_eq!(strong.get("context_max"), Some(&Value::Number(1048576.0)));
        Ok(())
    }

    #[test]
    fn ip_note_matching_has_the_original_unanchored_boundaries() {
        for note in [
            "Private host 192.0.2.10",
            "IPv6 host [2001:db8::1]",
            "x9999.0.0.12345x",
            "colons :a:",
        ] {
            let units = Text::from(note);
            assert!(has_ipv4(&units.0) || has_ipv6(&units.0), "{note}");
        }
        for note in ["Version 1.2.3", "one:colon", "colons :abcde:", "not:gh:hex"] {
            let units = Text::from(note);
            assert!(!has_ipv4(&units.0) && !has_ipv6(&units.0), "{note}");
        }
    }

    #[test]
    fn copilot_worker_capabilities_and_doctor_keep_offline_blockers_and_refuse_unsafe_paths()
    -> Result<(), Error> {
        let base_report = crate::json::parse(&Text::from(
            r#"{"schema":1,"kind":"external-full-agent","evidence":"synthetic","concurrency":1,"tools":["read_file"],"authority":{"launch":false,"network":false,"auth":false,"shell":false,"git":false,"publisher":false},"limits":{"files":16,"bytes":1000000,"callbacks":32,"events":128,"duration":60000},"controls":{"immutableTask":true,"exactScope":true,"boundedCallbacks":true,"candidateHashes":true,"cancellation":true,"deadline":true,"exitAndCleanup":true,"separateProvenance":true,"independentReview":true}}"#,
        ))?;
        let assessed = assess_copilot_worker_capabilities(&base_report)?;
        assert_eq!(
            assessed.get("status"),
            Some(&Value::string("offline-contract-compatible"))
        );
        assert_eq!(
            assessed.get("offlineContractCompatible"),
            Some(&Value::Boolean(true))
        );
        assert_eq!(
            assessed.get("liveDispatchEligible"),
            Some(&Value::Boolean(false))
        );
        assert_eq!(
            assessed.get("publicationEligible"),
            Some(&Value::Boolean(false))
        );

        let mut blocked_report = base_report.clone();
        let mut controls = blocked_report
            .get("controls")
            .cloned()
            .ok_or_else(|| Error::type_error("Missing controls"))?;
        domain::remove(&mut controls, "immutableTask")?;
        domain::put(&mut controls, "cancellation", Value::Boolean(false))?;
        domain::put(&mut controls, "deadline", Value::Null)?;
        domain::put(&mut blocked_report, "controls", controls)?;
        let blocked = assess_copilot_worker_capabilities(&blocked_report)?;
        assert_eq!(blocked.get("status"), Some(&Value::string("blocked")));
        assert_eq!(
            blocked.get("offlineContractCompatible"),
            Some(&Value::Boolean(false))
        );

        let mut invalid_auth = base_report.clone();
        let mut auth = invalid_auth
            .get("authority")
            .cloned()
            .ok_or_else(|| Error::type_error("Missing authority"))?;
        domain::put(&mut auth, "launch", Value::Boolean(true))?;
        domain::put(&mut invalid_auth, "authority", auth)?;
        assert!(assess_copilot_worker_capabilities(&invalid_auth).is_err());

        for control_key in CONTROL_NAMES {
            for state in ["missing", "null", "false"] {
                let mut candidate = base_report.clone();
                let mut ctrls = candidate
                    .get("controls")
                    .cloned()
                    .ok_or_else(|| Error::type_error("Missing controls"))?;
                match state {
                    "missing" => domain::remove(&mut ctrls, control_key)?,
                    "null" => domain::put(&mut ctrls, control_key, Value::Null)?,
                    _ => domain::put(&mut ctrls, control_key, Value::Boolean(false))?,
                }
                domain::put(&mut candidate, "controls", ctrls)?;
                let res = assess_copilot_worker_capabilities(&candidate)?;
                assert_eq!(res.get("status"), Some(&Value::string("blocked")));
                assert_eq!(
                    res.get("offlineContractCompatible"),
                    Some(&Value::Boolean(false))
                );
                assert_eq!(
                    res.get("liveDispatchEligible"),
                    Some(&Value::Boolean(false))
                );
                assert_eq!(res.get("publicationEligible"), Some(&Value::Boolean(false)));
            }
        }
        for auth_key in AUTHORITY_NAMES {
            let mut candidate = base_report.clone();
            let mut auths = candidate
                .get("authority")
                .cloned()
                .ok_or_else(|| Error::type_error("Missing authority"))?;
            domain::put(&mut auths, auth_key, Value::Boolean(true))?;
            domain::put(&mut candidate, "authority", auths)?;
            assert!(assess_copilot_worker_capabilities(&candidate).is_err());
        }
        for bad_tools in [
            vec![],
            vec![Value::string("read_file"), Value::string("read_file")],
            vec![Value::string("read_file"), Value::string("write_file")],
            vec![Value::string("shell")],
            vec![Value::string("READ_FILE")],
        ] {
            let mut candidate = base_report.clone();
            domain::put(&mut candidate, "tools", Value::Array(bad_tools))?;
            assert!(assess_copilot_worker_capabilities(&candidate).is_err());
        }
        for limit_key in LIMIT_NAMES {
            for bad_val in [0.0, -1.0, 1.5, (crate::MAX_SAFE_INTEGER as f64) + 2.0] {
                let mut candidate = base_report.clone();
                let mut lims = candidate
                    .get("limits")
                    .cloned()
                    .ok_or_else(|| Error::type_error("Missing limits"))?;
                domain::put(&mut lims, limit_key, Value::Number(bad_val))?;
                domain::put(&mut candidate, "limits", lims)?;
                assert!(assess_copilot_worker_capabilities(&candidate).is_err());
            }
        }

        let parsed = parse_copilot_doctor_args(&[
            "--runtime".to_owned(),
            r"C:\fixture path\copilot.exe".to_owned(),
            "--opt-in".to_owned(),
            "--model".to_owned(),
            "exact-model".to_owned(),
        ])?;
        assert!(parsed.opt_in);
        assert_eq!(
            parsed.runtime_path.as_deref(),
            Some(r"C:\fixture path\copilot.exe")
        );
        assert_eq!(parsed.model.as_deref(), Some("exact-model"));
        assert!(parse_copilot_doctor_args(&["--warm".to_owned()]).is_err());
        assert!(
            parse_copilot_doctor_args(&["--opt-in".to_owned(), "--opt-in".to_owned()]).is_err()
        );

        let opt_out = check_copilot_doctor(&CopilotDoctorOptions {
            is_tty: Some(false),
            ..Default::default()
        })?;
        assert_eq!(opt_out.get("ok"), Some(&Value::Boolean(false)));
        assert_eq!(opt_out.get("enabled"), Some(&Value::Boolean(false)));
        assert_eq!(opt_out.get("qualified"), Some(&Value::Boolean(false)));
        let formatted = format_copilot_doctor(&opt_out)?;
        assert!(formatted.contains("Copilot offline diagnostics: BLOCKED"));
        assert!(formatted.contains("Runtime inspection skipped"));

        let tty_report = check_copilot_doctor(&CopilotDoctorOptions {
            opt_in: true,
            runtime_path: Some("Cargo.toml/copilot.exe".to_owned()),
            is_tty: Some(true),
            ..Default::default()
        })?;
        let tty_formatted = format_copilot_doctor(&tty_report)?;
        assert!(tty_formatted.contains("OK interactive auth handoff: A terminal is present"));
        assert!(tty_formatted.contains("FAIL runtime file metadata: Runtime file is missing."));

        assert!(
            parse_copilot_doctor_args(&["--runtime".to_owned(), "\u{feff}".to_owned()]).is_err()
        );
        assert!(
            check_copilot_doctor(&CopilotDoctorOptions {
                opt_in: true,
                runtime_path: Some("\u{feff}".to_owned()),
                ..Default::default()
            })
            .is_err()
        );

        for unsafe_path in [
            r"\\secret-server\share\copilot.exe",
            "//secret-server/share/copilot.exe",
            r"\\?\UNC\secret-server\share\copilot.exe",
            r"\??\C:\secret-path\copilot.exe",
            "file:///secret-path/copilot.exe",
            "https://secret-server/copilot.exe",
            "secret\npath",
        ] {
            let err = check_copilot_doctor(&CopilotDoctorOptions {
                opt_in: true,
                runtime_path: Some(unsafe_path.to_owned()),
                ..Default::default()
            })
            .err()
            .ok_or_else(|| Error::type_error("Unsafe runtime path was accepted"))?;
            assert!(!err.message.contains("secret"));
        }

        for auth_state in [
            "unknown",
            "signed-out",
            "expired",
            "revoked",
            "policy-disabled",
            "permission-denied",
        ] {
            let rep = check_copilot_doctor(&CopilotDoctorOptions {
                auth_state: auth_state.to_owned(),
                ..Default::default()
            })?;
            assert_eq!(rep.get("ok"), Some(&Value::Boolean(false)));
        }
        for auth_source in [
            "unknown",
            "environment",
            "stored-oauth",
            "gh-fallback",
            "separate-inference",
        ] {
            let rep = check_copilot_doctor(&CopilotDoctorOptions {
                auth_source: auth_source.to_owned(),
                ..Default::default()
            })?;
            assert_eq!(rep.get("ok"), Some(&Value::Boolean(false)));
        }
        for model_state in ["unknown", "unavailable", "policy-disabled"] {
            let rep = check_copilot_doctor(&CopilotDoctorOptions {
                model_state: model_state.to_owned(),
                ..Default::default()
            })?;
            assert_eq!(rep.get("ok"), Some(&Value::Boolean(false)));
        }
        Ok(())
    }

    #[test]
    fn copilot_offline_feasibility_worker_attempt_and_calibration_gates() -> Result<(), Error> {
        let root = std::env::temp_dir().join(format!(
            "roster-copilot-gates-{}-{}",
            std::process::id(),
            crate::state::files::nonce()?
        ));
        crate::state::files::create_private_directories(&root)?;
        std::fs::write(root.join("allowed.txt"), "Synthetic scoped context.\n")
            .map_err(|e| Error::new("Error", e.to_string()))?;
        std::fs::write(root.join("outside.txt"), "Out of scope.\n")
            .map_err(|e| Error::new("Error", e.to_string()))?;
        std::fs::write(root.join("sample.txt"), "before\n")
            .map_err(|e| Error::new("Error", e.to_string()))?;
        std::fs::write(root.join("remove.txt"), "remove\n")
            .map_err(|e| Error::new("Error", e.to_string()))?;

        let parent_env = Value::object(vec![
            ("COPILOT_GITHUB_TOKEN", Value::string("secret-copilot-tok")),
            ("GH_TOKEN", Value::string("secret-gh-tok")),
            ("GITHUB_APP_ID", Value::string("secret-app")),
        ]);
        let inf_ctx = offline_copilot_inference_context(&parent_env)?;
        assert_eq!(inf_ctx.get("env"), Some(&Value::object(Vec::new())));
        assert_eq!(inf_ctx.get("useLoggedInUser"), Some(&Value::Boolean(false)));

        let req = crate::json::parse(&Text::from(
            r#"{"model":"requested-fixture-model","messages":[{"role":"system","content":"Use only the offered Roster tool."},{"role":"user","content":"Read the named file."}],"tools":["read_file"]}"#,
        ))?;
        let fix = crate::json::parse(&Text::from(
            r#"{"events":[{"type":"assistant.usage","data":{"model":"fixture-reported-model","inputTokens":12,"outputTokens":3,"cost":0.5}},{"type":"fixture.tool_request","data":{"id":"fixture-call-1","name":"read_file","arguments":{"path":"allowed.txt"}}},{"type":"assistant.message","data":{"content":"Synthetic answer."}},{"type":"session.idle","data":{}}]}"#,
        ))?;
        let turn_res = run_offline_copilot_turn(
            &req,
            &fix,
            &root,
            &["allowed.txt".to_owned()],
            &parent_env,
            None,
        )?;
        assert_eq!(
            turn_res.get("mode"),
            Some(&Value::string("offline-fixture"))
        );
        assert_eq!(
            turn_res.get("response").and_then(|r| r.get("model")),
            Some(&Value::string("fixture-reported-model"))
        );
        assert_eq!(
            turn_res
                .get("provenance")
                .and_then(|p| p.get("publishable")),
            Some(&Value::Boolean(false))
        );

        let no_usage_fix = crate::json::parse(&Text::from(
            r#"{"events":[{"type":"assistant.message","data":{"content":"Synthetic answer."}},{"type":"session.idle","data":{}}]}"#,
        ))?;
        let no_usage_res = run_offline_copilot_turn(
            &req,
            &no_usage_fix,
            &root,
            &["allowed.txt".to_owned()],
            &parent_env,
            None,
        )?;
        assert_eq!(
            no_usage_res.get("response").and_then(|r| r.get("model")),
            Some(&Value::Null)
        );
        assert_eq!(
            no_usage_res.get("response").and_then(|r| r.get("usage")),
            Some(&Value::Null)
        );

        for escaped in ["outside.txt", "../outside.txt", ".env", "agent-policy.yml"] {
            let bad_fix = crate::json::parse(&Text::from(
                format!(
                    r#"{{"events":[{{"type":"fixture.tool_request","data":{{"id":"c1","name":"read_file","arguments":{{"path":"{escaped}"}}}}}},{{"type":"assistant.message","data":{{"content":"ok"}}}},{{"type":"session.idle","data":{{}}}}]}}"#
                )
                .as_str(),
            ))?;
            assert!(
                run_offline_copilot_turn(
                    &req,
                    &bad_fix,
                    &root,
                    &["allowed.txt".to_owned()],
                    &parent_env,
                    None,
                )
                .is_err()
            );
        }

        let contract = crate::json::parse(&Text::from(
            r#"{"ask":"Change the approved sample.","task":"Issue 611; check sample.","base":"reviewed-base","run":"offline-run","attempt":"attempt-1","scope":{"read":["sample.txt"],"write":["sample.txt","remove.txt"],"delete":["remove.txt"]},"config":{"requestedModel":"requested-fixture-model","executable":"not-a-real-cli","version":"fixture-v1"},"limits":{"files":2,"bytes":64,"callbacks":4,"events":12,"duration":100}}"#,
        ))?;
        let mut attempt = OfflineWorkerAttempt::create(&root, &contract)?;
        let read_out = attempt.callback(
            "read_file",
            &Value::object(vec![("path", Value::string("sample.txt"))]),
            10,
        )?;
        assert_eq!(read_out, "before\n");
        attempt.observe(
            &crate::json::parse(&Text::from(
                r#"{"type":"inference","id":"call-1","servedModel":"actual-served-model","usage":{"unit":"ai_credits","value":1.5,"complete":true}}"#,
            ))?,
            20,
        )?;
        attempt.observe(&Value::object(vec![("type", Value::string("idle"))]), 30)?;
        attempt.observe(
            &Value::object(vec![
                ("type", Value::string("exit")),
                ("code", Value::Number(0.0)),
            ]),
            40,
        )?;
        attempt.observe(
            &Value::object(vec![
                ("type", Value::string("cleanup")),
                ("ok", Value::Boolean(true)),
            ]),
            50,
        )?;
        let anchors = attempt.anchors().clone();
        let finish_env = Value::object(vec![
            ("kind", Value::string("external-full-agent")),
            ("evidence", Value::string("synthetic")),
            ("anchors", anchors.clone()),
            (
                "files",
                Value::Array(vec![
                    Value::object(vec![
                        ("path", Value::string("sample.txt")),
                        ("content", Value::string("after\n")),
                    ]),
                    Value::object(vec![
                        ("path", Value::string("remove.txt")),
                        ("delete", Value::Boolean(true)),
                    ]),
                ]),
            ),
        ]);
        attempt.finish(&finish_env, 60)?;
        let subject = attempt.import_candidate(&anchors, 70)?;
        assert_eq!(
            std::fs::read_to_string(root.join("sample.txt"))
                .map_err(|e| Error::new("Error", e.to_string()))?,
            "after\n"
        );
        assert!(!root.join("remove.txt").exists());
        let checks = Value::Array(vec![Value::object(vec![
            ("source", Value::string("synthetic-host-check")),
            ("pass", Value::Boolean(true)),
        ])]);
        let receipt = offline_copilot_host_review(&subject, &checks)?;
        let rev_elig = attempt.review_eligibility(&receipt, &anchors, 80)?;
        assert_eq!(
            rev_elig.get("hostReviewEligible"),
            Some(&Value::Boolean(true))
        );
        assert_eq!(rev_elig.get("publishable"), Some(&Value::Boolean(false)));
        let emitted = attempt.emit_managed_artifacts(
            &receipt,
            &anchors,
            "Updated sample.txt and removed remove.txt.",
            85,
        )?;
        assert_eq!(emitted.get("reviewVerdict"), Some(&Value::string("pass")));
        assert_eq!(emitted.get("builtinSeat"), Some(&Value::Boolean(false)));
        assert_eq!(emitted.get("publishable"), Some(&Value::Boolean(false)));
        assert!(root.join("RESULT.md").is_file());
        assert!(root.join("REVIEW.md").is_file());
        let prov = attempt.provenance();
        assert_eq!(prov.get("modelStatus"), Some(&Value::string("known")));
        assert_eq!(prov.get("mismatch"), Some(&Value::Boolean(true)));
        assert_eq!(prov.get("usageStatus"), Some(&Value::string("complete")));
        assert_eq!(prov.get("total"), Some(&Value::Number(1.5)));

        let approval = crate::json::parse(&Text::from(
            r#"{"repository":"R4yHarris/github-agent-roster","dataClassification":"public-nonsecret","accountRoute":"individual-subscription","policyApproved":true,"selectedModels":["GPT-6.1-Sol"],"creditCeiling":50,"stopConditions":["cancel-on-rate-limit","stop-on-scope-escape"],"approver":"R4yHarris"}"#,
        ))?;
        let smoke_task = crate::json::parse(&Text::from(
            r#"{"baseRevision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","allowedFiles":["sample.txt"],"namedTests":["cargo test --lib capabilities::"],"independentReviewer":true,"offlinePreflight":true,"wallTimeMs":60000,"maxRequests":4}"#,
        ))?;
        let smoke = assess_copilot_smoke_admission(&approval, &smoke_task)?;
        assert_eq!(
            smoke.get("status"),
            Some(&Value::string("smoke-preflight-ready"))
        );
        assert_eq!(smoke.get("liveSmokeEligible"), Some(&Value::Boolean(false)));

        let cal_manifest = crate::json::parse(&Text::from(
            r#"{"tiers":[{"id":"M1","difficulty":1,"task":"Add status heading","baseRevision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","allowedFiles":["README.md"],"namedTests":["cargo test --test cli"],"modelSelectionRule":"locked","effort":"m","independentReviewer":true,"renewedApproval":true},{"id":"M2","difficulty":2,"task":"Add parser check","baseRevision":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","allowedFiles":["rust/capabilities.rs"],"namedTests":["cargo test --lib capabilities::"],"modelSelectionRule":"locked","effort":"h","independentReviewer":true,"renewedApproval":false}],"limitations":["Synthetic offline fixture only; no live billing."],"attempts":[{"tier":"M1","provider":"local-vllm","attempt":1,"task":"Add status heading","baseRevision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","allowedFiles":["README.md"],"namedTests":["cargo test --test cli"],"modelSelectionRule":"locked","effort":"m","independentReviewer":true,"testResult":"pass","reviewVerdict":"pass","servedModel":"qwen3.8-27b","promptTokens":100,"completionTokens":20,"elapsedMs":1200},{"tier":"M1","provider":"copilot-worker","attempt":1,"task":"Add status heading","baseRevision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","allowedFiles":["README.md"],"namedTests":["cargo test --test cli"],"modelSelectionRule":"locked","effort":"m","independentReviewer":true,"testResult":"fail","reviewVerdict":"fail"}]}"#,
        ))?;
        let scorecard = evaluate_copilot_calibration_scorecard(&cal_manifest)?;
        assert_eq!(scorecard.get("totalAttempts"), Some(&Value::Number(2.0)));
        assert_eq!(scorecard.get("failedAttempts"), Some(&Value::Number(1.0)));
        assert_eq!(
            scorecard.get("progressionBlockedTiers"),
            Some(&Value::Array(vec![Value::string("M2")]))
        );
        assert_eq!(
            scorecard.get("superiorityClaimAllowed"),
            Some(&Value::Boolean(false))
        );

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
