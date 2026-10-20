use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::domain::{self, items, members, put};
use crate::fleet::{self, TaskClass};
use crate::json::Value;
use crate::text::{Text, ascii, trim};
use crate::{Error, catalog, config, paths, settings};

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
        Ok(())
    }
}
