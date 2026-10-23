use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::Path;

use crate::Error;
use crate::domain;
use crate::fleet;
use crate::identity::sha256_hex;
use crate::json::Value;
use crate::run;
use crate::state::{files, secure};
use crate::text::{Text, trim};

pub const OPENSHELL_DOCTOR_USAGE: &str = "Use roster doctor --openshell [--opt-in] [--runtime PATH] [--gateway URL] [--policy PATH] [--provider ID].";
const MAX_POLICY_PATHS: usize = 256;
const MAX_PATH_BYTES: usize = 4096;
const MAX_CANDIDATE_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenShellNetworkRule {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub protocol: String,
    pub methods: Vec<String>,
    pub binaries: Vec<String>,
    pub access: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenShellPolicySpec {
    pub read_only_files: Vec<String>,
    pub read_write_files: Vec<String>,
    pub run_as_user: String,
    pub run_as_group: String,
    pub landlock_compatibility: String,
    pub network_rules: Vec<OpenShellNetworkRule>,
}

impl Default for OpenShellPolicySpec {
    fn default() -> Self {
        Self {
            read_only_files: Vec::new(),
            read_write_files: Vec::new(),
            run_as_user: "1500".to_owned(),
            run_as_group: "1500".to_owned(),
            landlock_compatibility: "hard_requirement".to_owned(),
            network_rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenShellPolicy {
    pub version: u32,
    pub include_workdir: bool,
    pub read_only: Vec<String>,
    pub read_write: Vec<String>,
    pub landlock_compatibility: String,
    pub run_as_user: String,
    pub run_as_group: String,
    pub network_rules: Vec<OpenShellNetworkRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenShellProviderProfile {
    pub id: String,
    pub display_name: String,
    pub model: String,
    pub endpoint_host: String,
    pub endpoint_port: u16,
    pub endpoint_scheme: String,
    pub base_url: String,
    pub credential_env: Option<String>,
    pub binaries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenShellDoctorOptions {
    pub opt_in: bool,
    pub runtime_path: Option<String>,
    pub gateway_url: Option<String>,
    pub policy_path: Option<String>,
    pub provider_id: Option<String>,
    pub landlock_state: String,
    pub is_tty: Option<bool>,
}

impl Default for OpenShellDoctorOptions {
    fn default() -> Self {
        Self {
            opt_in: false,
            runtime_path: None,
            gateway_url: None,
            policy_path: None,
            provider_id: None,
            landlock_state: "unknown".to_owned(),
            is_tty: None,
        }
    }
}

fn validate_process_identity(value: &str, field: &str) -> Result<String, Error> {
    let trimmed = value.trim();
    if trimmed == "sandbox" {
        return Ok(trimmed.to_owned());
    }
    let parsed = trimmed.parse::<u32>().map_err(|_| {
        Error::type_error(format!(
            "OpenShell {field} must be 'sandbox' or a non-root numeric ID (1..=4294967294)"
        ))
    })?;
    if parsed == 0 || parsed == u32::MAX {
        return Err(Error::type_error(format!(
            "OpenShell {field} rejects root (0) and reserved UID/GID 4294967295"
        )));
    }
    Ok(parsed.to_string())
}

fn is_protected_write_surface(relative: &str) -> bool {
    let trimmed = relative.trim_start_matches('/');
    let lower = trimmed.to_ascii_lowercase();
    if crate::planning::forbidden_write(trimmed) || crate::execution::policy::managed(trimmed) {
        return true;
    }
    let segments = lower.split('/').collect::<Vec<_>>();
    let first = segments.first().copied().unwrap_or("");
    if matches!(
        first,
        ".github" | "vendor" | ".git" | ".roster" | ".roster-state" | "agent-policy.yml"
    ) {
        return true;
    }
    segments.iter().any(|seg| {
        matches!(
            *seg,
            ".git" | ".roster" | ".roster-state" | "agent-policy.yml" | ".env"
        ) || seg.starts_with(".env.")
            || seg.ends_with(".env")
            || seg.ends_with(".pem")
            || seg.ends_with(".key")
    })
}

fn is_secret_read_surface(relative: &str) -> bool {
    let trimmed = relative.trim_start_matches('/');
    let lower = trimmed.to_ascii_lowercase();
    if crate::planning::forbidden_read(trimmed) {
        return true;
    }
    lower.split('/').any(|seg| {
        matches!(seg, ".git" | ".roster" | ".roster-state" | ".env")
            || seg.starts_with(".env.")
            || seg.ends_with(".env")
            || seg.ends_with(".pem")
            || seg.ends_with(".key")
    })
}

fn validate_posix_binary_path(bin: &str) -> Result<String, Error> {
    if !bin.starts_with('/')
        || bin.len() > MAX_PATH_BYTES
        || bin.contains('\\')
        || bin
            .bytes()
            .any(|b| matches!(b, 0..=32 | 127 | b':' | b'#' | b'"' | b'\''))
    {
        return Err(Error::type_error(format!(
            "OpenShell binary path must be a single-line absolute POSIX path without whitespace or metacharacters: {bin}"
        )));
    }
    let segments = bin.trim_start_matches('/').split('/').collect::<Vec<_>>();
    if segments
        .iter()
        .any(|seg| seg.is_empty() || *seg == "." || *seg == "..")
    {
        return Err(Error::type_error(format!(
            "OpenShell binary path must not contain empty, '.' or '..' segments: {bin}"
        )));
    }
    Ok(format!("/{}", segments.join("/")))
}

fn normalize_sandbox_path(raw: &str, writable: bool) -> Result<String, Error> {
    if raw.is_empty()
        || raw.len() > MAX_PATH_BYTES
        || raw.bytes().any(|b| matches!(b, 0..=31 | 127))
        || raw.contains('\\')
    {
        return Err(Error::type_error(
            "OpenShell policy path must be a bounded POSIX path without control characters",
        ));
    }
    if raw == "/" {
        if writable {
            return Err(Error::type_error(
                "OpenShell filesystem_policy.read_write cannot contain '/'",
            ));
        }
        return Err(Error::type_error(
            "OpenShell policy must not grant whole-root '/' read access",
        ));
    }
    let segments = raw
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<&str>>();
    if segments
        .iter()
        .any(|seg| seg.is_empty() || *seg == "." || *seg == "..")
    {
        return Err(Error::type_error(
            "OpenShell policy path must not contain empty, '.' or '..' segments",
        ));
    }
    let normalized_rel = segments.join("/");
    if writable && is_protected_write_surface(&normalized_rel) {
        return Err(Error::new(
            "PolicyViolation",
            format!("OpenShell read_write denies protected surface: {normalized_rel}"),
        )
        .coded("ROSTER_OPENSHELL_PROTECTED_SURFACE"));
    }
    if !writable && is_secret_read_surface(&normalized_rel) {
        return Err(Error::new(
            "PolicyViolation",
            format!("OpenShell read_only denies private/secret surface: {normalized_rel}"),
        )
        .coded("ROSTER_OPENSHELL_SECRET_SURFACE"));
    }
    if raw.starts_with('/') {
        let allowed_system = ["/usr/", "/lib/", "/lib64/", "/bin/", "/etc/", "/tmp/"]
            .iter()
            .any(|prefix| raw.starts_with(prefix) || raw == &prefix[..prefix.len() - 1]);
        let allowed_sandbox = raw.starts_with("/sandbox/workspace/");
        if !allowed_system && !allowed_sandbox {
            return Err(Error::type_error(format!(
                "OpenShell absolute path must reside under /sandbox/workspace or baseline system dirs: {raw}"
            )));
        }
        if writable && !allowed_sandbox && !raw.starts_with("/tmp") {
            return Err(Error::type_error(format!(
                "OpenShell read_write absolute path must reside under /sandbox/workspace or /tmp: {raw}"
            )));
        }
        if allowed_sandbox {
            let ws_rel = raw.trim_start_matches("/sandbox/workspace/");
            if writable && is_protected_write_surface(ws_rel) {
                return Err(Error::new(
                    "PolicyViolation",
                    format!("OpenShell read_write denies protected surface: {ws_rel}"),
                )
                .coded("ROSTER_OPENSHELL_PROTECTED_SURFACE"));
            }
            if !writable && is_secret_read_surface(ws_rel) {
                return Err(Error::new(
                    "PolicyViolation",
                    format!("OpenShell read_only denies private/secret surface: {ws_rel}"),
                )
                .coded("ROSTER_OPENSHELL_SECRET_SURFACE"));
            }
        }
        Ok(raw.to_owned())
    } else {
        Ok(format!("/sandbox/workspace/{normalized_rel}"))
    }
}

fn is_forbidden_metadata_host(host: &str) -> bool {
    let lower = host
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    lower == "169.254.169.254"
        || lower == "::ffff:169.254.169.254"
        || lower == "0:0:0:0:0:ffff:a9fe:a9fe"
        || lower == "metadata.google.internal"
        || lower == "metadata"
        || lower == "100.100.100.200"
        || lower == "fd00:ec2::254"
}

fn validate_network_rule(rule: &OpenShellNetworkRule) -> Result<OpenShellNetworkRule, Error> {
    let name = rule.name.trim();
    if name.is_empty()
        || name.starts_with("_provider_")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::type_error(
            "OpenShell network rule name must be alphanumeric/hyphen/underscore and must not start with '_provider_'",
        ));
    }
    let host = rule.host.trim();
    if host.is_empty()
        || host
            .bytes()
            .any(|b| matches!(b, 0..=32 | 127 | b'/' | b'\\' | b'#' | b'"' | b'\''))
    {
        return Err(Error::type_error(
            "OpenShell network rule host must be a valid hostname or IP",
        ));
    }
    if is_forbidden_metadata_host(host) {
        return Err(Error::new(
            "PolicyViolation",
            format!("OpenShell network rule denies cloud metadata endpoint: {host}"),
        )
        .coded("ROSTER_OPENSHELL_METADATA_DENIED"));
    }
    if rule.port == 0 {
        return Err(Error::type_error(
            "OpenShell network rule port must be 1..=65535",
        ));
    }
    if !matches!(rule.protocol.as_str(), "rest" | "https" | "http") {
        return Err(Error::type_error(
            "OpenShell network rule protocol must be rest, https, or http",
        ));
    }
    if !matches!(rule.access.as_str(), "read-only" | "read-write") {
        return Err(Error::type_error(
            "OpenShell network rule access must be read-only or read-write",
        ));
    }
    if rule.methods.is_empty() {
        return Err(Error::type_error(
            "OpenShell network rule must declare at least one HTTP method",
        ));
    }
    let mut normalized_methods = Vec::new();
    for method in &rule.methods {
        let m = method.trim();
        if !matches!(
            m,
            "GET" | "HEAD" | "OPTIONS" | "POST" | "PUT" | "PATCH" | "DELETE"
        ) {
            return Err(Error::type_error(format!(
                "Unsupported OpenShell HTTP method: {method}"
            )));
        }
        if rule.access == "read-only" && !matches!(m, "GET" | "HEAD" | "OPTIONS") {
            return Err(Error::type_error(format!(
                "OpenShell read-only network rule cannot grant mutating HTTP method {m}"
            )));
        }
        if !normalized_methods.iter().any(|existing| existing == m) {
            normalized_methods.push(m.to_owned());
        }
    }
    if rule.binaries.is_empty() {
        return Err(Error::type_error(
            "OpenShell network rule must bind at least one binary path",
        ));
    }
    let mut normalized_binaries = Vec::new();
    for binary in &rule.binaries {
        let valid_bin = validate_posix_binary_path(binary)?;
        if !normalized_binaries.contains(&valid_bin) {
            normalized_binaries.push(valid_bin);
        }
    }
    Ok(OpenShellNetworkRule {
        name: name.to_owned(),
        host: host.to_owned(),
        port: rule.port,
        protocol: rule.protocol.clone(),
        methods: normalized_methods,
        binaries: normalized_binaries,
        access: rule.access.clone(),
    })
}

pub fn compile_task_policy(spec: &OpenShellPolicySpec) -> Result<OpenShellPolicy, Error> {
    if spec.landlock_compatibility != "hard_requirement" {
        return Err(Error::new(
            "PolicyViolation",
            "Roster requires OpenShell landlock.compatibility to be 'hard_requirement'",
        )
        .coded("ROSTER_OPENSHELL_LANDLOCK_REQUIRED"));
    }
    let run_as_user = validate_process_identity(&spec.run_as_user, "run_as_user")?;
    let run_as_group = validate_process_identity(&spec.run_as_group, "run_as_group")?;
    let mut read_only = BTreeSet::new();
    for path in ["/usr", "/lib", "/bin", "/etc"] {
        read_only.insert(path.to_owned());
    }
    for path in &spec.read_only_files {
        read_only.insert(normalize_sandbox_path(path, false)?);
    }
    let mut read_write = BTreeSet::new();
    read_write.insert("/tmp".to_owned());
    for path in &spec.read_write_files {
        let normalized = normalize_sandbox_path(path, true)?;
        read_only.remove(&normalized);
        read_write.insert(normalized);
    }
    if read_only.len() + read_write.len() > MAX_POLICY_PATHS {
        return Err(Error::type_error(
            "OpenShell filesystem_policy cannot exceed 256 total paths",
        ));
    }
    let mut seen_rules = HashSet::new();
    let mut network_rules = Vec::new();
    for rule in &spec.network_rules {
        let validated = validate_network_rule(rule)?;
        if !seen_rules.insert(validated.name.clone()) {
            return Err(Error::type_error(
                "OpenShell network_policies rule names must be unique",
            ));
        }
        network_rules.push(validated);
    }
    Ok(OpenShellPolicy {
        version: 1,
        include_workdir: false,
        read_only: read_only.into_iter().collect(),
        read_write: read_write.into_iter().collect(),
        landlock_compatibility: spec.landlock_compatibility.clone(),
        run_as_user,
        run_as_group,
        network_rules,
    })
}

impl OpenShellPolicy {
    pub fn to_yaml(&self) -> String {
        let mut out = String::from(
            "version: 1\nfilesystem_policy:\n  include_workdir: false\n  read_only:\n",
        );
        for path in &self.read_only {
            out.push_str(&format!("    - {path}\n"));
        }
        out.push_str("  read_write:\n");
        for path in &self.read_write {
            out.push_str(&format!("    - {path}\n"));
        }
        out.push_str(&format!(
            "landlock:\n  compatibility: {}\nprocess:\n  run_as_user: \"{}\"\n  run_as_group: \"{}\"\n",
            self.landlock_compatibility, self.run_as_user, self.run_as_group
        ));
        if self.network_rules.is_empty() {
            out.push_str("network_policies: {}\n");
        } else {
            out.push_str("network_policies:\n");
            for rule in &self.network_rules {
                out.push_str(&format!(
                    "  {}:\n    name: {}\n    endpoints:\n      - host: {}\n        port: {}\n        protocol: {}\n        access: {}\n        methods: [{}]\n    binaries:\n",
                    rule.name,
                    rule.name,
                    rule.host,
                    rule.port,
                    rule.protocol,
                    rule.access,
                    rule.methods.join(", ")
                ));
                for bin in &rule.binaries {
                    out.push_str(&format!("      - {bin}\n"));
                }
            }
        }
        out
    }

    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("version", Value::Number(f64::from(self.version))),
            (
                "filesystem_policy",
                Value::object(vec![
                    ("include_workdir", Value::Boolean(self.include_workdir)),
                    (
                        "read_only",
                        Value::Array(self.read_only.iter().map(|s| Value::string(s)).collect()),
                    ),
                    (
                        "read_write",
                        Value::Array(self.read_write.iter().map(|s| Value::string(s)).collect()),
                    ),
                ]),
            ),
            (
                "landlock",
                Value::object(vec![(
                    "compatibility",
                    Value::string(&self.landlock_compatibility),
                )]),
            ),
            (
                "process",
                Value::object(vec![
                    ("run_as_user", Value::string(&self.run_as_user)),
                    ("run_as_group", Value::string(&self.run_as_group)),
                ]),
            ),
            (
                "network_policies",
                Value::Array(
                    self.network_rules
                        .iter()
                        .map(|r| {
                            Value::object(vec![
                                ("name", Value::string(&r.name)),
                                ("host", Value::string(&r.host)),
                                ("port", Value::Number(f64::from(r.port))),
                                ("protocol", Value::string(&r.protocol)),
                                ("access", Value::string(&r.access)),
                                (
                                    "methods",
                                    Value::Array(
                                        r.methods.iter().map(|m| Value::string(m)).collect(),
                                    ),
                                ),
                                (
                                    "binaries",
                                    Value::Array(
                                        r.binaries.iter().map(|b| Value::string(b)).collect(),
                                    ),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }

    pub fn digest(&self) -> String {
        sha256_hex(self.to_yaml().as_bytes())
    }
}

pub fn compile_fleet_provider_profile(
    profile: &Value,
    binaries: &[String],
) -> Result<OpenShellProviderProfile, Error> {
    let validated = fleet::validate_profile(profile)?;
    let val = validated.as_value();
    let id = val
        .get("id")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Fleet profile id is required"))?
        .to_utf8()?;
    let model = val
        .get("model")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Fleet profile model is required"))?
        .to_utf8()?;
    let raw_base = val
        .get("base_url")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Fleet profile base_url is required"))?;
    let parsed_url = crate::config::base_url(raw_base, "fleet base_url")?;
    let raw_host = parsed_url
        .host_str()
        .ok_or_else(|| Error::type_error("Fleet base_url must include a host"))?;
    if is_forbidden_metadata_host(raw_host) {
        return Err(Error::new(
            "PolicyViolation",
            format!("OpenShell provider profile denies cloud metadata endpoint: {raw_host}"),
        )
        .coded("ROSTER_OPENSHELL_METADATA_DENIED"));
    }
    let mapped_host = match raw_host {
        "127.0.0.1" | "localhost" | "[::1]" | "::1" => "host.openshell.internal".to_owned(),
        other => other.to_owned(),
    };
    let port = parsed_url.port_or_known_default().unwrap_or(80);
    let scheme = parsed_url.scheme().to_owned();
    let path = parsed_url.path().trim_end_matches('/');
    let default_port = (scheme == "http" && port == 80) || (scheme == "https" && port == 443);
    let routed_base_url = if default_port {
        format!("{scheme}://{mapped_host}{path}")
    } else {
        format!("{scheme}://{mapped_host}:{port}{path}")
    };
    let credential_env = match val.get("api_key_env").and_then(Value::as_string) {
        Some(secret_name) => {
            let env_str = secret_name.to_utf8()?;
            let upper = env_str.to_ascii_uppercase();
            if upper.starts_with("GITHUB_APP_")
                || upper.starts_with("OPENSHELL_")
                || matches!(
                    upper.as_str(),
                    "OPENAI_BASE_URL"
                        | "ROSTER_MODEL"
                        | "HOME"
                        | "TMPDIR"
                        | "NO_COLOR"
                        | "GIT_TERMINAL_PROMPT"
                )
            {
                return Err(Error::new(
                    "PolicyViolation",
                    format!(
                        "OpenShell provider profile rejects reserved or GitHub App credential env: {env_str}"
                    ),
                )
                .coded("ROSTER_OPENSHELL_RESERVED_CREDENTIAL"));
            }
            Some(env_str)
        }
        None => None,
    };
    if binaries.is_empty() {
        return Err(Error::type_error(
            "OpenShell provider profile requires at least one allowed binary path",
        ));
    }
    let mut normalized_bins = Vec::new();
    for bin in binaries {
        let valid_bin = validate_posix_binary_path(bin)?;
        if !normalized_bins.contains(&valid_bin) {
            normalized_bins.push(valid_bin);
        }
    }
    Ok(OpenShellProviderProfile {
        id: format!("roster-{id}"),
        display_name: format!("Roster Fleet {id} ({model})"),
        model,
        endpoint_host: mapped_host,
        endpoint_port: port,
        endpoint_scheme: scheme,
        base_url: routed_base_url,
        credential_env,
        binaries: normalized_bins,
    })
}

impl OpenShellProviderProfile {
    pub fn to_yaml(&self) -> String {
        let mut out = format!(
            "id: {}\ndisplay_name: \"{}\"\ncategory: inference\ninference_capable: true\n",
            self.id, self.display_name
        );
        match &self.credential_env {
            Some(env_name) => {
                out.push_str(&format!(
                    "credentials:\n  - env_var: {env_name}\n    placeholder_only: true\n"
                ));
            }
            None => out.push_str("credentials: []\n"),
        }
        out.push_str(&format!(
            "endpoints:\n  - host: {}\n    port: {}\n    protocol: rest\n    access: read-write\n    enforcement: enforce\nbinaries:\n",
            self.endpoint_host, self.endpoint_port
        ));
        for bin in &self.binaries {
            out.push_str(&format!("  - {bin}\n"));
        }
        out
    }

    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("id", Value::string(&self.id)),
            ("display_name", Value::string(&self.display_name)),
            ("category", Value::string("inference")),
            ("inference_capable", Value::Boolean(true)),
            ("model", Value::string(&self.model)),
            ("base_url", Value::string(&self.base_url)),
            ("endpoint_host", Value::string(&self.endpoint_host)),
            (
                "endpoint_port",
                Value::Number(f64::from(self.endpoint_port)),
            ),
            (
                "credential_env",
                self.credential_env
                    .as_deref()
                    .map_or(Value::Null, Value::string),
            ),
            (
                "binaries",
                Value::Array(self.binaries.iter().map(|b| Value::string(b)).collect()),
            ),
        ])
    }
}

pub fn openshell_launch_environment(
    gateway_url: &str,
    provider: &OpenShellProviderProfile,
) -> Result<Value, Error> {
    let gw_text = Text::from(gateway_url);
    let parsed_gw = crate::config::base_url(&gw_text, "OpenShell gateway URL")?;
    if let Some(gw_host) = parsed_gw.host_str() {
        if is_forbidden_metadata_host(gw_host) {
            return Err(Error::new(
                "PolicyViolation",
                format!("OpenShell gateway URL denies cloud metadata endpoint: {gw_host}"),
            )
            .coded("ROSTER_OPENSHELL_METADATA_DENIED"));
        }
    }
    if is_forbidden_metadata_host(&provider.endpoint_host) {
        return Err(Error::new(
            "PolicyViolation",
            format!(
                "OpenShell provider endpoint denies cloud metadata host: {}",
                provider.endpoint_host
            ),
        )
        .coded("ROSTER_OPENSHELL_METADATA_DENIED"));
    }
    let mut entries = vec![
        ("OPENSHELL_TELEMETRY_ENABLED", Value::string("false")),
        (
            "OPENSHELL_GATEWAY_URL",
            Value::string(parsed_gw.as_str().trim_end_matches('/')),
        ),
        ("OPENSHELL_PROVIDER_ID", Value::string(&provider.id)),
        ("OPENAI_BASE_URL", Value::string(&provider.base_url)),
        ("ROSTER_MODEL", Value::string(&provider.model)),
        ("HOME", Value::string("/sandbox/home")),
        ("TMPDIR", Value::string("/tmp")),
        ("NO_COLOR", Value::string("1")),
        ("GIT_TERMINAL_PROMPT", Value::string("0")),
    ];
    if let Some(env_var) = &provider.credential_env {
        let upper = env_var.to_ascii_uppercase();
        if upper.starts_with("GITHUB_APP_")
            || upper.starts_with("OPENSHELL_")
            || matches!(
                upper.as_str(),
                "OPENAI_BASE_URL"
                    | "ROSTER_MODEL"
                    | "HOME"
                    | "TMPDIR"
                    | "NO_COLOR"
                    | "GIT_TERMINAL_PROMPT"
            )
        {
            return Err(Error::new(
                "PolicyViolation",
                format!("OpenShell launch environment rejects reserved credential env: {env_var}"),
            )
            .coded("ROSTER_OPENSHELL_RESERVED_CREDENTIAL"));
        }
        entries.push((
            env_var.as_str(),
            Value::string("OPENSHELL_SUPERVISOR_PLACEHOLDER"),
        ));
    } else {
        entries.push(("OPENAI_API_KEY", Value::string("unused")));
    }
    Ok(Value::object(entries))
}

pub fn prove_openshell_policy_boundary(
    candidate: &OpenShellPolicy,
    boundary: &OpenShellPolicy,
) -> Result<Value, Error> {
    let mut findings = Vec::new();
    if candidate.version != 1 || boundary.version != 1 {
        findings.push(Value::string("Policy version must be 1."));
    }
    if candidate.include_workdir || boundary.include_workdir {
        findings.push(Value::string(
            "Policy enables include_workdir; explicit path allow-lists are required.",
        ));
    }
    if candidate.landlock_compatibility != "hard_requirement"
        || boundary.landlock_compatibility != "hard_requirement"
    {
        findings.push(Value::string(
            "Policy weakens Landlock compatibility from hard_requirement.",
        ));
    }
    let cand_user = validate_process_identity(&candidate.run_as_user, "candidate.run_as_user").ok();
    let cand_group =
        validate_process_identity(&candidate.run_as_group, "candidate.run_as_group").ok();
    let bound_user = validate_process_identity(&boundary.run_as_user, "boundary.run_as_user").ok();
    let bound_group =
        validate_process_identity(&boundary.run_as_group, "boundary.run_as_group").ok();
    if cand_user.is_none()
        || cand_group.is_none()
        || bound_user.is_none()
        || bound_group.is_none()
        || cand_user != bound_user
        || cand_group != bound_group
    {
        findings.push(Value::string(
            "Candidate or boundary policy has invalid or mismatched non-root UID/GID identity.",
        ));
    }
    if candidate.read_only.len() + candidate.read_write.len() > MAX_POLICY_PATHS
        || boundary.read_only.len() + boundary.read_write.len() > MAX_POLICY_PATHS
    {
        findings.push(Value::string(
            "Policy filesystem path count exceeds 256 limit.",
        ));
    }
    let mut bound_rw = Vec::new();
    for raw in &boundary.read_write {
        match normalize_sandbox_path(raw, true) {
            Ok(norm) => bound_rw.push(norm),
            Err(err) => findings.push(Value::string(&format!(
                "Invalid boundary read_write path '{raw}': {}",
                err.message
            ))),
        }
    }
    let mut bound_ro = Vec::new();
    for raw in &boundary.read_only {
        match normalize_sandbox_path(raw, false) {
            Ok(norm) => bound_ro.push(norm),
            Err(err) => findings.push(Value::string(&format!(
                "Invalid boundary read_only path '{raw}': {}",
                err.message
            ))),
        }
    }
    for raw in &candidate.read_write {
        match normalize_sandbox_path(raw, true) {
            Ok(path) => {
                let covered = bound_rw
                    .iter()
                    .any(|allowed| &path == allowed || path.starts_with(&format!("{allowed}/")));
                if !covered {
                    findings.push(Value::string(&format!(
                        "Candidate policy expands read_write outside boundary: {path}"
                    )));
                }
            }
            Err(err) => findings.push(Value::string(&format!(
                "Invalid candidate read_write path '{raw}': {}",
                err.message
            ))),
        }
    }
    for raw in &candidate.read_only {
        match normalize_sandbox_path(raw, false) {
            Ok(path) => {
                let covered = bound_ro
                    .iter()
                    .chain(bound_rw.iter())
                    .any(|allowed| &path == allowed || path.starts_with(&format!("{allowed}/")));
                if !covered {
                    findings.push(Value::string(&format!(
                        "Candidate policy expands read_only outside boundary: {path}"
                    )));
                }
            }
            Err(err) => findings.push(Value::string(&format!(
                "Invalid candidate read_only path '{raw}': {}",
                err.message
            ))),
        }
    }
    let mut valid_boundary_rules = Vec::new();
    for b_rule in &boundary.network_rules {
        match validate_network_rule(b_rule) {
            Ok(valid) => valid_boundary_rules.push(valid),
            Err(err) => findings.push(Value::string(&format!(
                "Invalid boundary network rule '{}': {}",
                b_rule.name, err.message
            ))),
        }
    }
    for raw_rule in &candidate.network_rules {
        let rule = match validate_network_rule(raw_rule) {
            Ok(valid) => valid,
            Err(err) => {
                findings.push(Value::string(&format!(
                    "Invalid candidate network rule '{}': {}",
                    raw_rule.name, err.message
                )));
                continue;
            }
        };
        let matching = valid_boundary_rules
            .iter()
            .filter(|b| b.host == rule.host && b.port == rule.port && b.protocol == rule.protocol)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            findings.push(Value::string(&format!(
                "Candidate network rule adds unapproved endpoint {}:{} ({})",
                rule.host, rule.port, rule.name
            )));
            continue;
        }
        let covered_by_any = matching.iter().any(|bound| {
            let access_ok = !(rule.access == "read-write" && bound.access != "read-write");
            let methods_ok = rule.methods.iter().all(|m| bound.methods.contains(m));
            let bins_ok = rule.binaries.iter().all(|b| bound.binaries.contains(b));
            access_ok && methods_ok && bins_ok
        });
        if !covered_by_any {
            findings.push(Value::string(&format!(
                "Candidate network rule '{}' widens access, methods, or binaries beyond boundary rules for {}:{}",
                rule.name, rule.host, rule.port
            )));
        }
    }
    let verified = findings.is_empty();
    Ok(Value::object(vec![
        ("verified", Value::Boolean(verified)),
        ("candidateDigest", Value::string(&candidate.digest())),
        ("boundaryDigest", Value::string(&boundary.digest())),
        ("findings", Value::Array(findings)),
    ]))
}

pub struct OpenShellSandboxAttempt {
    pub task_id: String,
    pub attempt_id: String,
    pub sandbox_name: String,
    pub provider_name: String,
    pub state: String,
    pub import_revoked: bool,
    pub provider_detached: bool,
    pub detach_command: Vec<String>,
    pub policy: OpenShellPolicy,
    pub policy_digest: String,
    pub task_digest: String,
    pub candidate_digest: Option<String>,
    pub staged_files: BTreeMap<String, String>,
    pub candidate_files: BTreeMap<String, String>,
}

fn validate_workspace_relative_file(rel: &str, writable: bool) -> Result<String, Error> {
    if rel.starts_with('/') || rel.contains('\\') {
        return Err(Error::type_error(
            "OpenShell workspace file path must be relative to the workspace root",
        ));
    }
    normalize_sandbox_path(rel, writable)
}

fn checked_regular_file(path: &Path) -> Result<Vec<u8>, Error> {
    use std::io::Read;
    let meta = fs::symlink_metadata(path).map_err(|_| {
        Error::new(
            "OpenShellStagingError",
            format!("Missing or unreadable file: {}", path.display()),
        )
    })?;
    if !meta.is_file() || secure::linked(&meta) {
        return Err(Error::new(
            "OpenShellStagingError",
            format!("Refusing non-regular or symlinked file: {}", path.display()),
        )
        .coded("ROSTER_OPENSHELL_LINKED_FILE"));
    }
    let file = fs::File::open(path).map_err(|_| {
        Error::new(
            "OpenShellStagingError",
            format!("Cannot open file: {}", path.display()),
        )
    })?;
    let handle_meta = file.metadata().map_err(|_| {
        Error::new(
            "OpenShellStagingError",
            format!("Cannot inspect file handle: {}", path.display()),
        )
    })?;
    if !handle_meta.is_file() || secure::linked(&handle_meta) {
        return Err(Error::new(
            "OpenShellStagingError",
            format!(
                "Refusing non-regular or symlinked file handle: {}",
                path.display()
            ),
        )
        .coded("ROSTER_OPENSHELL_LINKED_FILE"));
    }
    if handle_meta.len() > MAX_CANDIDATE_FILE_BYTES {
        return Err(Error::new(
            "OpenShellStagingError",
            format!("File exceeds 1 MiB sandbox bound: {}", path.display()),
        ));
    }
    if crate::paths::handle_information(&file)?.links != 1 {
        return Err(Error::new(
            "OpenShellStagingError",
            format!("Refusing multi-link file: {}", path.display()),
        )
        .coded("ROSTER_OPENSHELL_LINKED_FILE"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_CANDIDATE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            Error::new(
                "OpenShellStagingError",
                format!("Cannot read file bytes: {}", path.display()),
            )
        })?;
    if bytes.len() as u64 > MAX_CANDIDATE_FILE_BYTES {
        return Err(Error::new(
            "OpenShellStagingError",
            format!("File grew beyond 1 MiB sandbox bound: {}", path.display()),
        ));
    }
    Ok(bytes)
}

impl OpenShellSandboxAttempt {
    pub fn prepare(
        task_id: &str,
        attempt_id: &str,
        sandbox_name: &str,
        provider_name: &str,
        policy: &OpenShellPolicy,
        task_source: &str,
    ) -> Result<Self, Error> {
        let valid_task = fleet::validate_id(Some(&Value::string(task_id)))?.to_utf8()?;
        let valid_attempt = fleet::validate_id(Some(&Value::string(attempt_id)))?.to_utf8()?;
        let valid_sandbox = fleet::validate_id(Some(&Value::string(sandbox_name)))?.to_utf8()?;
        let valid_provider = fleet::validate_id(Some(&Value::string(provider_name)))?.to_utf8()?;
        if task_source.trim().is_empty() {
            return Err(Error::type_error(
                "OpenShell sandbox attempt requires a nonempty task source",
            ));
        }
        let proof = prove_openshell_policy_boundary(policy, policy)?;
        if proof.get("verified") != Some(&Value::Boolean(true)) {
            return Err(Error::new(
                "PolicyViolation",
                "OpenShell sandbox attempt requires a valid policy",
            ));
        }
        Ok(Self {
            task_id: valid_task,
            attempt_id: valid_attempt,
            sandbox_name: valid_sandbox.clone(),
            provider_name: valid_provider.clone(),
            state: "prepared".to_owned(),
            import_revoked: false,
            provider_detached: false,
            detach_command: vec![
                "openshell".to_owned(),
                "sandbox".to_owned(),
                "provider".to_owned(),
                "detach".to_owned(),
                valid_sandbox,
                valid_provider,
                "--wait".to_owned(),
            ],
            policy: policy.clone(),
            policy_digest: policy.digest(),
            task_digest: sha256_hex(task_source.as_bytes()),
            candidate_digest: None,
            staged_files: BTreeMap::new(),
            candidate_files: BTreeMap::new(),
        })
    }

    pub fn stage_workspace_files(
        &mut self,
        host_root: &Path,
        staging_root: &Path,
        allowed_files: &[String],
    ) -> Result<Value, Error> {
        if self.state != "prepared" || self.import_revoked {
            return Err(Error::new(
                "OpenShellLifecycleError",
                "Workspace files can only be staged in the prepared state before revocation",
            ));
        }
        secure::check_components(host_root)?;
        files::create_private_directories(staging_root)?;
        secure::check_components(staging_root)?;
        for rel in allowed_files {
            let sandbox_path = validate_workspace_relative_file(rel, false)?;
            let covered = self
                .policy
                .read_only
                .iter()
                .chain(self.policy.read_write.iter())
                .any(|allowed| {
                    &sandbox_path == allowed || sandbox_path.starts_with(&format!("{allowed}/"))
                });
            if !covered {
                return Err(Error::new(
                    "PolicyViolation",
                    format!("Staged file is not authorized by OpenShell policy: {rel}"),
                )
                .coded("ROSTER_OPENSHELL_SCOPE_DENIED"));
            }
            let src = host_root.join(rel);
            crate::paths::ensure_local_path(&src, host_root)?;
            let bytes = checked_regular_file(&src)?;
            let dst = staging_root.join(rel);
            if let Some(parent) = dst.parent() {
                files::create_private_directories(parent)?;
            }
            crate::paths::ensure_local_path(&dst, staging_root)?;
            files::atomic_write_bytes(&dst, &bytes, MAX_CANDIDATE_FILE_BYTES)?;
            self.staged_files.insert(rel.clone(), sha256_hex(&bytes));
        }
        Ok(Value::object(vec![
            ("state", Value::string(&self.state)),
            ("stagedCount", Value::Number(self.staged_files.len() as f64)),
            ("taskDigest", Value::string(&self.task_digest)),
            ("policyDigest", Value::string(&self.policy_digest)),
        ]))
    }

    pub fn start_running(&mut self) -> Result<(), Error> {
        if self.state != "prepared" || self.import_revoked {
            return Err(Error::new(
                "OpenShellLifecycleError",
                "Sandbox attempt must be in prepared state to start running",
            ));
        }
        self.state = "running".to_owned();
        Ok(())
    }

    pub fn cancel_and_detach(&mut self, detach_verified: bool) -> Result<Value, Error> {
        self.state = "stopping".to_owned();
        self.import_revoked = true;
        self.provider_detached = detach_verified;
        if detach_verified {
            self.state = "cancelled".to_owned();
        } else {
            self.state = "failed".to_owned();
        }
        Ok(Value::object(vec![
            ("state", Value::string(&self.state)),
            ("importRevoked", Value::Boolean(self.import_revoked)),
            ("providerDetached", Value::Boolean(self.provider_detached)),
            (
                "detachCommand",
                Value::Array(
                    self.detach_command
                        .iter()
                        .map(|arg| Value::string(arg))
                        .collect(),
                ),
            ),
        ]))
    }

    pub fn inspect_candidate(
        &mut self,
        staging_root: &Path,
        allowed_write_files: &[String],
    ) -> Result<Value, Error> {
        if self.import_revoked || self.state != "running" {
            return Err(Error::new(
                "OpenShellLifecycleError",
                "Cannot inspect or import candidate after cancellation, revocation, or non-running state",
            )
            .coded("ROSTER_OPENSHELL_REVOKED"));
        }
        if allowed_write_files.is_empty() {
            return Err(Error::type_error(
                "OpenShell candidate inspection requires at least one allowed write file",
            ));
        }
        secure::check_components(staging_root)?;
        let mut seen_writes = HashSet::new();
        self.candidate_files.clear();
        for rel in allowed_write_files {
            let sandbox_path = validate_workspace_relative_file(rel, true)?;
            if !seen_writes.insert(sandbox_path.clone()) {
                return Err(Error::type_error(format!(
                    "Duplicate OpenShell candidate write path: {rel}"
                )));
            }
            let covered = self.policy.read_write.iter().any(|allowed| {
                &sandbox_path == allowed || sandbox_path.starts_with(&format!("{allowed}/"))
            });
            if !covered {
                return Err(Error::new(
                    "PolicyViolation",
                    format!(
                        "Candidate write path is not authorized by OpenShell policy.read_write: {rel}"
                    ),
                )
                .coded("ROSTER_OPENSHELL_SCOPE_DENIED"));
            }
            let candidate_path = staging_root.join(rel);
            crate::paths::ensure_local_path(&candidate_path, staging_root)?;
            let bytes = checked_regular_file(&candidate_path)?;
            let file_hash = sha256_hex(&bytes);
            self.candidate_files.insert(rel.clone(), file_hash);
        }
        for (staged_rel, expected_hash) in &self.staged_files {
            if !self.candidate_files.contains_key(staged_rel) {
                let staged_path = staging_root.join(staged_rel);
                crate::paths::ensure_local_path(&staged_path, staging_root)?;
                let current_bytes = checked_regular_file(&staged_path)?;
                if &sha256_hex(&current_bytes) != expected_hash {
                    return Err(Error::new(
                        "PolicyViolation",
                        format!("Read-only staged file was modified in sandbox: {staged_rel}"),
                    )
                    .coded("ROSTER_OPENSHELL_READONLY_MODIFIED"));
                }
            }
        }
        let mut digest_material = String::new();
        for (rel, file_hash) in &self.candidate_files {
            digest_material.push_str(rel);
            digest_material.push(':');
            digest_material.push_str(file_hash);
            digest_material.push('\n');
        }
        let digest = sha256_hex(digest_material.as_bytes());
        self.candidate_digest = Some(digest.clone());
        self.state = "candidate".to_owned();
        Ok(Value::object(vec![
            ("state", Value::string(&self.state)),
            ("candidateDigest", Value::string(&digest)),
            ("taskDigest", Value::string(&self.task_digest)),
            ("policyDigest", Value::string(&self.policy_digest)),
        ]))
    }

    pub fn verify_host_review(&mut self, review_receipt: &Value) -> Result<Value, Error> {
        if self.import_revoked || self.state != "candidate" {
            return Err(Error::new(
                "OpenShellLifecycleError",
                "Host review requires an unrevoked candidate state",
            ));
        }
        self.state = "reviewing".to_owned();
        let Some(candidate_digest) = &self.candidate_digest else {
            self.state = "blocked".to_owned();
            return Err(Error::new(
                "OpenShellReviewError",
                "Missing candidate digest before host review",
            ));
        };
        let verdict_ok = review_receipt
            .get("verdict")
            .and_then(Value::as_string)
            .is_some_and(|v| v == &Text::from("pass"));
        let independent_ok = review_receipt.get("independent") == Some(&Value::Boolean(true));
        let task_ok = review_receipt
            .get("taskDigest")
            .and_then(Value::as_string)
            .is_some_and(|v| v == &Text::from(self.task_digest.as_str()));
        let candidate_ok = review_receipt
            .get("candidateDigest")
            .and_then(Value::as_string)
            .is_some_and(|v| v == &Text::from(candidate_digest.as_str()));
        let policy_ok = review_receipt
            .get("policyDigest")
            .and_then(Value::as_string)
            .is_some_and(|v| v == &Text::from(self.policy_digest.as_str()));
        if !verdict_ok || !independent_ok || !task_ok || !candidate_ok || !policy_ok {
            self.state = "blocked".to_owned();
            return Err(Error::new(
                "OpenShellReviewError",
                "OpenShell candidate failed independent host review or digest binding",
            )
            .coded("ROSTER_OPENSHELL_REVIEW_BLOCKED"));
        }
        self.state = "eligible".to_owned();
        Ok(Value::object(vec![
            ("state", Value::string(&self.state)),
            ("eligibleForPublication", Value::Boolean(true)),
            ("candidateDigest", Value::string(candidate_digest)),
            ("taskDigest", Value::string(&self.task_digest)),
            ("policyDigest", Value::string(&self.policy_digest)),
        ]))
    }

    pub fn emit_managed_artifacts(
        &self,
        output_dir: &Path,
        model_evidence: &Value,
    ) -> Result<Value, Error> {
        if self.state != "eligible" || self.import_revoked {
            return Err(Error::new(
                "OpenShellLifecycleError",
                "Managed artifacts require an eligible, unrevoked OpenShell sandbox attempt",
            ));
        }
        let Some(candidate_digest) = &self.candidate_digest else {
            return Err(Error::new(
                "OpenShellLifecycleError",
                "Missing candidate digest for artifact emission",
            ));
        };
        let served_model = model_evidence
            .get("servedModel")
            .and_then(Value::as_string)
            .map(|t| t.to_utf8())
            .transpose()?
            .filter(|m| run::is_model_id(m))
            .unwrap_or_else(|| "unknown".to_owned());
        let input_tokens = domain::safe_integer(model_evidence.get("inputTokens"))
            .map_or_else(|| "unknown".to_owned(), |n| n.to_string());
        let output_tokens = domain::safe_integer(model_evidence.get("outputTokens"))
            .map_or_else(|| "unknown".to_owned(), |n| n.to_string());
        files::create_private_directories(output_dir)?;
        secure::check_components(output_dir)?;
        let result_path = output_dir.join("RESULT.md");
        let review_path = output_dir.join("REVIEW.md");
        crate::paths::ensure_local_path(&result_path, output_dir)?;
        crate::paths::ensure_local_path(&review_path, output_dir)?;
        let result_body = format!(
            "# OpenShell Sandbox Result\n\n- Execution kind: external-openshell-sandbox\n- Task: {}\n- Attempt: {}\n- Sandbox: {}\n- Provider: {}\n- Served model: {}\n- Input tokens: {}\n- Output tokens: {}\n- Policy digest: {}\n- Task digest: {}\n- Candidate digest: {}\n",
            self.task_id,
            self.attempt_id,
            self.sandbox_name,
            self.provider_name,
            served_model,
            input_tokens,
            output_tokens,
            self.policy_digest,
            self.task_digest,
            candidate_digest
        );
        let review_body = format!(
            "# Independent Host Review\n\n- Verdict: pass\n- Independent: true\n- Execution kind: external-openshell-sandbox\n- Policy digest: {}\n- Task digest: {}\n- Candidate digest: {}\n- Publication Ready: {}\n",
            self.policy_digest,
            self.task_digest,
            candidate_digest,
            served_model != "unknown"
        );
        files::atomic_write_bytes(
            &result_path,
            result_body.as_bytes(),
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        files::atomic_write_bytes(
            &review_path,
            review_body.as_bytes(),
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        Ok(Value::object(vec![
            ("executionKind", Value::string("external-openshell-sandbox")),
            ("servedModel", Value::string(&served_model)),
            ("inputTokens", Value::string(&input_tokens)),
            ("outputTokens", Value::string(&output_tokens)),
            (
                "publicationReady",
                Value::Boolean(served_model != "unknown"),
            ),
            ("resultPath", Value::string(&result_path.to_string_lossy())),
            ("reviewPath", Value::string(&review_path.to_string_lossy())),
        ]))
    }
}

pub fn parse_openshell_doctor_args(args: &[String]) -> Result<OpenShellDoctorOptions, Error> {
    let usage = || Error::type_error(OPENSHELL_DOCTOR_USAGE);
    let mut options = OpenShellDoctorOptions::default();
    let mut seen = HashSet::new();
    let mut index = 0;
    while let Some(flag) = args.get(index) {
        if ![
            "--opt-in",
            "--runtime",
            "--gateway",
            "--policy",
            "--provider",
        ]
        .contains(&flag.as_str())
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
                .filter(|v| !trim(&Text::from(v.as_str()).0).is_empty() && !v.starts_with("--"))
                .ok_or_else(usage)?;
            match flag.as_str() {
                "--runtime" => options.runtime_path = Some(value.clone()),
                "--gateway" => options.gateway_url = Some(value.clone()),
                "--policy" => options.policy_path = Some(value.clone()),
                "--provider" => options.provider_id = Some(value.clone()),
                _ => return Err(usage()),
            }
        }
        index += 1;
    }
    Ok(options)
}

fn validate_local_path_arg(path: &str, label: &str) -> Result<(), Error> {
    if trim(&Text::from(path).0).is_empty() || path.bytes().any(|b| matches!(b, 0..=31 | 127)) {
        return Err(Error::type_error(format!(
            "{label} must be a nonempty local filesystem path without ASCII control characters."
        )));
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
        return Err(Error::type_error(format!(
            "{label} must be local; network, device namespace, and URI paths are not allowed."
        )));
    }
    Ok(())
}

pub fn check_openshell_doctor(options: &OpenShellDoctorOptions) -> Result<Value, Error> {
    if !matches!(
        options.landlock_state.as_str(),
        "unknown" | "best_effort" | "hard_requirement"
    ) {
        return Err(Error::type_error(
            "Invalid OpenShell landlock_state; use unknown, best_effort, or hard_requirement.",
        ));
    }
    if let Some(runtime) = &options.runtime_path {
        validate_local_path_arg(runtime, "OpenShell runtime path")?;
    }
    if let Some(policy) = &options.policy_path {
        validate_local_path_arg(policy, "OpenShell policy path")?;
    }
    if let Some(gateway) = &options.gateway_url {
        let parsed =
            crate::config::base_url(&Text::from(gateway.as_str()), "OpenShell gateway URL")?;
        if let Some(host) = parsed.host_str() {
            if is_forbidden_metadata_host(host) {
                return Err(Error::new(
                    "PolicyViolation",
                    format!("OpenShell gateway URL denies cloud metadata endpoint: {host}"),
                )
                .coded("ROSTER_OPENSHELL_METADATA_DENIED"));
            }
        }
    }
    if let Some(provider_id) = &options.provider_id {
        let _ = fleet::validate_id(Some(&Value::string(provider_id)))?;
    }
    let mut blockers = Vec::new();
    if !options.opt_in {
        blockers.push("OPENSHELL_NOT_OPTED_IN");
    }
    let mut runtime_present = false;
    let runtime_reason = if !options.opt_in {
        "Runtime inspection skipped: explicitly pass --opt-in for offline OpenShell diagnostics."
    } else if let Some(runtime_path) = &options.runtime_path {
        match fs::symlink_metadata(runtime_path) {
            Ok(meta) if meta.is_file() && !secure::linked(&meta) => {
                runtime_present = true;
                "Regular nonsymlink openshell binary present; no process was launched."
            }
            Ok(_) => "Runtime path is not a regular nonsymlink executable file.",
            Err(_) => "Runtime file is missing or unreadable; install openshell externally.",
        }
    } else {
        "No --runtime PATH supplied; supply a pinned openshell executable for metadata check."
    };
    if !runtime_present {
        blockers.push("OPENSHELL_RUNTIME_UNPINNED");
    }
    let gateway_ok = options.opt_in && options.gateway_url.is_some();
    let gateway_reason = if !options.opt_in {
        "Gateway check skipped until --opt-in is supplied."
    } else if let Some(gw) = &options.gateway_url {
        let _ = gw;
        "Gateway URL syntax validated offline; no network connection was opened."
    } else {
        "No --gateway URL supplied; OpenShell requires an explicit local or cluster gateway."
    };
    if !gateway_ok {
        blockers.push("OPENSHELL_GATEWAY_UNCONFIGURED");
    }
    let mut policy_present = false;
    let policy_reason = if !options.opt_in {
        "Policy check skipped until --opt-in is supplied."
    } else if let Some(policy_path) = &options.policy_path {
        match fs::symlink_metadata(policy_path) {
            Ok(meta) if meta.is_file() && !secure::linked(&meta) => {
                policy_present = true;
                "Regular nonsymlink policy file present."
            }
            Ok(_) => "Policy path is not a regular nonsymlink file.",
            Err(_) => "Policy file is missing or unreadable.",
        }
    } else {
        "No --policy PATH supplied; OpenShell sandboxes require a version: 1 YAML policy."
    };
    if !policy_present {
        blockers.push("OPENSHELL_POLICY_MISSING");
    }
    let provider_ok = options.opt_in && options.provider_id.is_some();
    if !provider_ok {
        blockers.push("OPENSHELL_PROVIDER_UNBOUND");
    }
    if options.landlock_state != "hard_requirement" {
        blockers.push("OPENSHELL_LANDLOCK_UNVERIFIED");
    }
    blockers.push("OPENSHELL_LIVE_DISPATCH_UNAPPROVED");
    Ok(Value::object(vec![
        ("ok", Value::Boolean(false)),
        ("status", Value::string("blocked")),
        ("optIn", Value::Boolean(options.opt_in)),
        ("runtimePresent", Value::Boolean(runtime_present)),
        ("runtimeReason", Value::string(runtime_reason)),
        ("gatewayConfigured", Value::Boolean(gateway_ok)),
        ("gatewayReason", Value::string(gateway_reason)),
        ("policyPresent", Value::Boolean(policy_present)),
        ("policyReason", Value::string(policy_reason)),
        ("providerBound", Value::Boolean(provider_ok)),
        (
            "blockers",
            Value::Array(blockers.into_iter().map(Value::string).collect()),
        ),
    ]))
}

pub fn format_openshell_doctor(result: &Value) -> Result<String, Error> {
    let status = result
        .get("status")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Missing OpenShell doctor status"))?
        .to_utf8()?;
    let runtime_reason = result
        .get("runtimeReason")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Missing OpenShell runtimeReason"))?
        .to_utf8()?;
    let gateway_reason = result
        .get("gatewayReason")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Missing OpenShell gatewayReason"))?
        .to_utf8()?;
    let policy_reason = result
        .get("policyReason")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Missing OpenShell policyReason"))?
        .to_utf8()?;
    let blockers = result
        .get("blockers")
        .and_then(domain::items)
        .ok_or_else(|| Error::type_error("Missing OpenShell blockers"))?
        .iter()
        .filter_map(Value::as_string)
        .map(Text::to_utf8)
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    Ok(format!(
        "OpenShell doctor: {status} (offline; no gateway, container, or credential probe)\nRuntime: {runtime_reason}\nGateway: {gateway_reason}\nPolicy: {policy_reason}\nBlockers: {blockers}\n"
    ))
}

pub fn evaluate_openshell_calibration_scorecard(manifest: &Value) -> Result<Value, Error> {
    let runs = manifest
        .get("runs")
        .and_then(domain::items)
        .ok_or_else(|| Error::type_error("OpenShell calibration manifest requires a runs list"))?;
    if runs.is_empty() {
        return Err(Error::type_error(
            "OpenShell calibration manifest requires at least one run",
        ));
    }
    let mut seen_m1 = false;
    let mut seen_m2 = false;
    let mut seen_m3 = false;
    let mut m1_passed = true;
    let mut m2_passed = true;
    let mut m3_passed = true;
    let mut profiles = BTreeSet::new();
    let mut evaluated_runs = Vec::new();
    for entry in runs {
        let milestone = entry
            .get("milestone")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Calibration run requires milestone M1, M2, or M3"))?
            .to_utf8()?;
        let profile = fleet::validate_id(entry.get("profile"))?.to_utf8()?;
        profiles.insert(profile.clone());
        let boundary_ok = entry.get("boundaryVerified") == Some(&Value::Boolean(true));
        let landlock_ok = entry
            .get("landlock")
            .and_then(Value::as_string)
            .is_some_and(|v| v == &Text::from("hard_requirement"));
        let detached_ok = entry.get("providerDetached") == Some(&Value::Boolean(true));
        let review_ok = entry
            .get("hostReview")
            .and_then(Value::as_string)
            .is_some_and(|v| v == &Text::from("pass"));
        let pass = boundary_ok && landlock_ok && detached_ok && review_ok;
        match milestone.as_str() {
            "M1" => {
                seen_m1 = true;
                m1_passed &= pass;
            }
            "M2" => {
                seen_m2 = true;
                m2_passed &= pass;
            }
            "M3" => {
                seen_m3 = true;
                m3_passed &= pass;
            }
            _ => {
                return Err(Error::type_error(
                    "OpenShell calibration milestone must be M1, M2, or M3",
                ));
            }
        }
        evaluated_runs.push(Value::object(vec![
            ("milestone", Value::string(&milestone)),
            ("profile", Value::string(&profile)),
            ("passed", Value::Boolean(pass)),
        ]));
    }
    let m1_effective = seen_m1 && m1_passed;
    let m2_effective = m1_effective && seen_m2 && m2_passed;
    let m3_effective = m2_effective && seen_m3 && m3_passed;
    let recommendation = if m3_effective && profiles.len() >= 2 {
        "eligible-for-human-sandbox-review"
    } else {
        "hold-in-offline-sandbox-gate"
    };
    Ok(Value::object(vec![
        ("m1Passed", Value::Boolean(m1_effective)),
        ("m2Passed", Value::Boolean(m2_effective)),
        ("m3Passed", Value::Boolean(m3_effective)),
        ("profileCount", Value::Number(profiles.len() as f64)),
        ("recommendation", Value::string(recommendation)),
        ("runs", Value::Array(evaluated_runs)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_policy_compiler_enforces_hard_landlock_nonroot_and_protected_surfaces()
    -> Result<(), Error> {
        let spec = OpenShellPolicySpec {
            read_only_files: vec!["docs/FEATURE_SPEC.md".to_owned()],
            read_write_files: vec!["rust/openshell.rs".to_owned(), "tests/cli.rs".to_owned()],
            run_as_user: "1500".to_owned(),
            run_as_group: "1500".to_owned(),
            landlock_compatibility: "hard_requirement".to_owned(),
            network_rules: vec![OpenShellNetworkRule {
                name: "local-vllm".to_owned(),
                host: "host.openshell.internal".to_owned(),
                port: 11435,
                protocol: "rest".to_owned(),
                methods: vec!["POST".to_owned()],
                binaries: vec!["/usr/local/bin/roster".to_owned()],
                access: "read-write".to_owned(),
            }],
        };
        let policy = compile_task_policy(&spec)?;
        let yaml = policy.to_yaml();
        assert!(yaml.contains("version: 1\n"));
        assert!(yaml.contains("include_workdir: false\n"));
        assert!(yaml.contains("compatibility: hard_requirement\n"));
        assert!(yaml.contains("- /sandbox/workspace/rust/openshell.rs\n"));

        for forbidden in [
            "agent-policy.yml",
            ".github",
            ".github/workflows/ci.yml",
            "vendor",
            "vendor/github-agent-contracts/scripts/agent-pr.mjs",
            ".git/config",
            ".roster/config.yml",
            "nested/.env",
            "nested/prod.env",
            "TASK.md",
            "REVIEW.md",
            "../escape.txt",
            "/",
        ] {
            let mut bad = spec.clone();
            bad.read_write_files = vec![forbidden.to_owned()];
            assert!(
                compile_task_policy(&bad).is_err(),
                "Accepted forbidden write path: {forbidden}"
            );
        }

        let mut root_user = spec.clone();
        root_user.run_as_user = "0".to_owned();
        assert!(compile_task_policy(&root_user).is_err());

        let mut soft_landlock = spec.clone();
        soft_landlock.landlock_compatibility = "best_effort".to_owned();
        assert!(compile_task_policy(&soft_landlock).is_err());

        let mut reserved_rule = spec;
        reserved_rule.network_rules[0].name = "_provider_spoof".to_owned();
        assert!(compile_task_policy(&reserved_rule).is_err());
        Ok(())
    }

    #[test]
    fn fleet_provider_bridge_maps_local_and_remote_endpoints_without_leaking_secrets()
    -> Result<(), Error> {
        let local_profile = Value::object(vec![
            ("id", Value::string("rtx-3090-qwen")),
            ("base_url", Value::string("http://127.0.0.1:11435/v1")),
            ("model", Value::string("qwen3.8:27b-smtek")),
            ("provider", Value::string("vllm")),
            ("context_max", Value::Number(65536.0)),
            ("concurrency", Value::Number(1.0)),
            ("hardware", Value::string("rtx-3090-24gb")),
            ("notes", Value::string("local")),
        ]);
        let compiled_local =
            compile_fleet_provider_profile(&local_profile, &["/usr/local/bin/roster".to_owned()])?;
        assert_eq!(compiled_local.endpoint_host, "host.openshell.internal");
        assert_eq!(compiled_local.endpoint_port, 11435);
        assert_eq!(
            compiled_local.base_url,
            "http://host.openshell.internal:11435/v1"
        );
        assert!(compiled_local.to_yaml().contains("credentials: []"));

        let remote_profile = Value::object(vec![
            ("id", Value::string("aperture-qwen")),
            (
                "base_url",
                Value::string("https://aperture.xen.blackmesaresearch.ai/v1"),
            ),
            ("model", Value::string("qwen3.8-27b")),
            ("provider", Value::string("vllm")),
            ("context_max", Value::Number(262144.0)),
            ("concurrency", Value::Number(1.0)),
            ("hardware", Value::string("rtx-6000-ada-48gb")),
            ("api_key_env", Value::string("APERTURE_API_KEY")),
            ("notes", Value::string("remote")),
        ]);
        let compiled_remote =
            compile_fleet_provider_profile(&remote_profile, &["/usr/local/bin/roster".to_owned()])?;
        assert_eq!(
            compiled_remote.endpoint_host,
            "aperture.xen.blackmesaresearch.ai"
        );
        assert_eq!(compiled_remote.endpoint_port, 443);
        let yaml = compiled_remote.to_yaml();
        assert!(yaml.contains("env_var: APERTURE_API_KEY"));
        assert!(yaml.contains("placeholder_only: true"));
        let env = openshell_launch_environment("http://127.0.0.1:8080", &compiled_remote)?;
        assert_eq!(
            env.get("OPENSHELL_TELEMETRY_ENABLED"),
            Some(&Value::string("false"))
        );
        assert_eq!(
            env.get("APERTURE_API_KEY"),
            Some(&Value::string("OPENSHELL_SUPERVISOR_PLACEHOLDER"))
        );
        Ok(())
    }

    #[test]
    fn boundary_prover_catches_scope_and_network_privilege_escalation() -> Result<(), Error> {
        let boundary = compile_task_policy(&OpenShellPolicySpec {
            read_only_files: vec!["docs/FEATURE_SPEC.md".to_owned()],
            read_write_files: vec!["rust/openshell.rs".to_owned()],
            run_as_user: "1500".to_owned(),
            run_as_group: "1500".to_owned(),
            landlock_compatibility: "hard_requirement".to_owned(),
            network_rules: vec![OpenShellNetworkRule {
                name: "inference".to_owned(),
                host: "host.openshell.internal".to_owned(),
                port: 11435,
                protocol: "rest".to_owned(),
                methods: vec!["POST".to_owned()],
                binaries: vec!["/usr/local/bin/roster".to_owned()],
                access: "read-write".to_owned(),
            }],
        })?;
        let valid_proof = prove_openshell_policy_boundary(&boundary, &boundary)?;
        assert_eq!(valid_proof.get("verified"), Some(&Value::Boolean(true)));

        let mut escalated = boundary.clone();
        escalated
            .read_write
            .push("/sandbox/workspace/rust/main.rs".to_owned());
        let failed_proof = prove_openshell_policy_boundary(&escalated, &boundary)?;
        assert_eq!(failed_proof.get("verified"), Some(&Value::Boolean(false)));

        let mut traversal = boundary.clone();
        traversal
            .read_write
            .push("/sandbox/workspace/rust/openshell.rs/../../agent-policy.yml".to_owned());
        let traversal_proof = prove_openshell_policy_boundary(&traversal, &boundary)?;
        assert_eq!(
            traversal_proof.get("verified"),
            Some(&Value::Boolean(false))
        );
        Ok(())
    }

    #[test]
    fn sandbox_attempt_enforces_single_link_staging_revocation_and_host_review() -> Result<(), Error>
    {
        let temp = std::env::temp_dir().join(format!(
            "roster-openshell-test-{}-{}",
            std::process::id(),
            files::nonce()?
        ));
        let host_root = temp.join("host");
        let staging_root = temp.join("staging");
        let out_dir = temp.join("out");
        files::create_private_directories(&host_root.join("rust"))?;
        files::atomic_write_bytes(
            &host_root.join("rust/openshell.rs"),
            b"pub fn demo() {}\n",
            MAX_CANDIDATE_FILE_BYTES,
        )?;

        let policy = compile_task_policy(&OpenShellPolicySpec {
            read_only_files: Vec::new(),
            read_write_files: vec!["rust/openshell.rs".to_owned()],
            ..OpenShellPolicySpec::default()
        })?;
        let mut attempt = OpenShellSandboxAttempt::prepare(
            "issue-675",
            "attempt-1",
            "sbx-675",
            "roster-rtx-3090-qwen",
            &policy,
            "TASK: OpenShell slice",
        )?;
        attempt.stage_workspace_files(
            &host_root,
            &staging_root,
            &["rust/openshell.rs".to_owned()],
        )?;
        attempt.start_running()?;
        files::atomic_write_bytes(
            &staging_root.join("rust/openshell.rs"),
            b"pub fn demo() -> u32 { 42 }\n",
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        let inspected =
            attempt.inspect_candidate(&staging_root, &["rust/openshell.rs".to_owned()])?;
        let cand_digest = inspected
            .get("candidateDigest")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Missing candidateDigest"))?
            .to_utf8()?;

        let receipt = Value::object(vec![
            ("verdict", Value::string("pass")),
            ("independent", Value::Boolean(true)),
            ("taskDigest", Value::string(&attempt.task_digest)),
            ("candidateDigest", Value::string(&cand_digest)),
            ("policyDigest", Value::string(&attempt.policy_digest)),
        ]);
        attempt.verify_host_review(&receipt)?;
        let emitted = attempt.emit_managed_artifacts(
            &out_dir,
            &Value::object(vec![("servedModel", Value::string("qwen3.8-27b"))]),
        )?;
        assert_eq!(emitted.get("publicationReady"), Some(&Value::Boolean(true)));
        assert_eq!(emitted.get("inputTokens"), Some(&Value::string("unknown")));

        let mut cancelled = OpenShellSandboxAttempt::prepare(
            "issue-675",
            "attempt-2",
            "sbx-675-cancel",
            "roster-rtx-3090-qwen",
            &policy,
            "TASK: OpenShell slice",
        )?;
        cancelled.start_running()?;
        let detach = cancelled.cancel_and_detach(true)?;
        assert_eq!(detach.get("state"), Some(&Value::string("cancelled")));
        assert!(
            cancelled
                .inspect_candidate(&staging_root, &["rust/openshell.rs".to_owned()])
                .is_err()
        );

        let _ = fs::remove_dir_all(&temp);
        Ok(())
    }

    #[test]
    fn openshell_doctor_and_calibration_scorecard_validate_gates() -> Result<(), Error> {
        let opts = parse_openshell_doctor_args(&[
            "--opt-in".to_owned(),
            "--gateway".to_owned(),
            "http://127.0.0.1:8080".to_owned(),
            "--provider".to_owned(),
            "rtx-3090-qwen".to_owned(),
        ])?;
        let report = check_openshell_doctor(&opts)?;
        let formatted = format_openshell_doctor(&report)?;
        assert!(formatted.contains("OpenShell doctor: blocked"));
        assert!(formatted.contains("OPENSHELL_RUNTIME_UNPINNED"));

        let scorecard = evaluate_openshell_calibration_scorecard(&Value::object(vec![(
            "runs",
            Value::Array(vec![
                Value::object(vec![
                    ("milestone", Value::string("M1")),
                    ("profile", Value::string("rtx-3090-qwen")),
                    ("boundaryVerified", Value::Boolean(true)),
                    ("landlock", Value::string("hard_requirement")),
                    ("providerDetached", Value::Boolean(true)),
                    ("hostReview", Value::string("pass")),
                ]),
                Value::object(vec![
                    ("milestone", Value::string("M2")),
                    ("profile", Value::string("aperture-qwen")),
                    ("boundaryVerified", Value::Boolean(true)),
                    ("landlock", Value::string("hard_requirement")),
                    ("providerDetached", Value::Boolean(true)),
                    ("hostReview", Value::string("pass")),
                ]),
                Value::object(vec![
                    ("milestone", Value::string("M3")),
                    ("profile", Value::string("aperture-dsv4")),
                    ("boundaryVerified", Value::Boolean(true)),
                    ("landlock", Value::string("hard_requirement")),
                    ("providerDetached", Value::Boolean(true)),
                    ("hostReview", Value::string("pass")),
                ]),
            ]),
        )]))?;
        assert_eq!(
            scorecard.get("recommendation"),
            Some(&Value::string("eligible-for-human-sandbox-review"))
        );

        let incomplete_scorecard =
            evaluate_openshell_calibration_scorecard(&Value::object(vec![(
                "runs",
                Value::Array(vec![
                    Value::object(vec![
                        ("milestone", Value::string("M1")),
                        ("profile", Value::string("rtx-3090-qwen")),
                        ("boundaryVerified", Value::Boolean(true)),
                        ("landlock", Value::string("hard_requirement")),
                        ("providerDetached", Value::Boolean(true)),
                        ("hostReview", Value::string("pass")),
                    ]),
                    Value::object(vec![
                        ("milestone", Value::string("M1")),
                        ("profile", Value::string("aperture-qwen")),
                        ("boundaryVerified", Value::Boolean(true)),
                        ("landlock", Value::string("hard_requirement")),
                        ("providerDetached", Value::Boolean(true)),
                        ("hostReview", Value::string("pass")),
                    ]),
                ]),
            )]))?;
        assert_eq!(
            incomplete_scorecard.get("m2Passed"),
            Some(&Value::Boolean(false))
        );
        assert_eq!(
            incomplete_scorecard.get("recommendation"),
            Some(&Value::string("hold-in-offline-sandbox-gate"))
        );

        for bad_runtime in [
            r"\??\C:\secret\openshell.exe",
            "file:///C:/secret/openshell.exe",
            "https://example.com/openshell",
        ] {
            let mut bad_opts = opts.clone();
            bad_opts.runtime_path = Some(bad_runtime.to_owned());
            assert!(check_openshell_doctor(&bad_opts).is_err());
        }
        Ok(())
    }
}
