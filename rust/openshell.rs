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

pub const OPENSHELL_DOCTOR_USAGE: &str = "Use roster doctor --openshell [--opt-in] [--approve-live] [--runtime PATH] [--prover PATH] [--gateway URL] [--policy PATH] [--provider ID].";
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
    pub approve_live: bool,
    pub runtime_path: Option<String>,
    pub prover_path: Option<String>,
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
            approve_live: false,
            runtime_path: None,
            prover_path: None,
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

pub(crate) fn is_forbidden_metadata_host(host: &str) -> bool {
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
                    "  {}:\n    name: {}\n    endpoints:\n      - host: {}\n        port: {}\n        protocol: {}\n        enforcement: enforce\n        access: {}\n    binaries:\n",
                    rule.name,
                    rule.name,
                    rule.host,
                    rule.port,
                    rule.protocol,
                    rule.access,
                ));
                for bin in &rule.binaries {
                    out.push_str(&format!("      - path: {bin}\n"));
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
                    "credentials:\n  - name: {env_name}\n    env_var: {env_name}\n    placeholder_only: true\n"
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

pub fn compile_active_llm_provider_profile(
    config: &Value,
    fleet_catalog: Option<&fleet::Fleet>,
    binaries: &[String],
) -> Result<OpenShellProviderProfile, Error> {
    let llm = config
        .get("llm")
        .ok_or_else(|| Error::type_error("Configuration is missing llm section"))?;
    let wanted_id = llm
        .get("fleet_profile")
        .or_else(|| llm.get("profile"))
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| !s.trim().is_empty());
    let base_url = llm
        .get("base_url")
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_default();
    let model = llm
        .get("model")
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_default();
    if let Some(catalog) = fleet_catalog {
        if let Some(ref wid) = wanted_id {
            if let Some(found) = catalog.profiles().iter().find(|p| {
                p.as_value()
                    .get("id")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .is_some_and(|id| id == *wid)
            }) {
                return compile_fleet_provider_profile(found.as_value(), binaries);
            }
        }
        if !base_url.is_empty() && !model.is_empty() {
            if let Some(found) = catalog.profiles().iter().find(|p| {
                let pv = p.as_value();
                let b = pv
                    .get("base_url")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .unwrap_or_default();
                let m = pv
                    .get("model")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .unwrap_or_default();
                b.trim_end_matches('/')
                    .eq_ignore_ascii_case(base_url.trim_end_matches('/'))
                    && m == model
            }) {
                return compile_fleet_provider_profile(found.as_value(), binaries);
            }
        }
    }
    let effective_base = if base_url.trim().is_empty() {
        "http://127.0.0.1:8000/v1".to_owned()
    } else {
        base_url
    };
    let effective_model = if model.trim().is_empty() || !run::is_model_id(&model) {
        "qwen3.8-27b".to_owned()
    } else {
        model
    };
    let profile_id = wanted_id
        .filter(|id| fleet::validate_id(Some(&Value::string(id))).is_ok())
        .unwrap_or_else(|| "active-llm".to_owned());
    let context_max = domain::safe_integer(llm.get("context_max"))
        .filter(|&n| n >= 1024)
        .unwrap_or(65_536);
    let mut synthesized = vec![
        ("id", Value::string(&profile_id)),
        ("base_url", Value::string(&effective_base)),
        ("model", Value::string(&effective_model)),
        ("provider", Value::string("vllm")),
        ("context_max", Value::Number(context_max as f64)),
        ("concurrency", Value::Number(1.0)),
        ("hardware", Value::string("host-configured")),
        ("notes", Value::string("synthesized from active llm config")),
    ];
    if let Some(api_key_env) = llm
        .get("api_key_env")
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| !s.trim().is_empty())
    {
        synthesized.push(("api_key_env", Value::string(&api_key_env)));
    }
    compile_fleet_provider_profile(&Value::object(synthesized), binaries)
}

fn extract_task_allowed_files(task_source: &str) -> Result<Vec<String>, Error> {
    if let Ok(doc) = crate::planning::task::parse(
        &Value::string(task_source),
        &crate::planning::task::Authority::default(),
    ) {
        if !doc.files_allowed.is_empty() {
            return Ok(doc.files_allowed);
        }
    }
    let mut files = Vec::new();
    let mut in_files = false;
    for raw_line in task_source.lines() {
        let trimmed = raw_line.trim();
        if let Some(heading) = trimmed.strip_prefix('#') {
            let h = heading.trim_start_matches('#').trim().to_ascii_lowercase();
            in_files = matches!(
                h.as_str(),
                "files allowed" | "allowed files" | "files" | "scope"
            );
            continue;
        }
        if in_files {
            if let Some(item) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                let cleaned = item.trim().trim_matches('`').trim();
                if !cleaned.is_empty() {
                    files.push(cleaned.to_owned());
                }
            }
        }
    }
    if files.is_empty() {
        return Err(Error::type_error(
            "OpenShell seat task policy requires a non-empty Files allowed section in TASK.md",
        ));
    }
    Ok(files)
}

pub fn compile_seat_task_policy(
    task_source: &str,
    recipe_source: Option<&str>,
    provider: Option<&OpenShellProviderProfile>,
    extra_read_only: &[String],
) -> Result<OpenShellPolicy, Error> {
    let allowed_files = extract_task_allowed_files(task_source)?;
    if let Some(rcp) = recipe_source.filter(|s| !s.trim().is_empty()) {
        if rcp.contains("seats:") && !rcp.contains("coder") {
            return Err(Error::type_error(
                "RECIPE.yml must include a coder seat for OpenShell task policy compilation",
            ));
        }
    }
    let mut rw_files = Vec::new();
    for file in &allowed_files {
        if !is_protected_write_surface(file) && !rw_files.contains(file) {
            rw_files.push(file.clone());
        }
    }
    let mut ro_files = Vec::new();
    for baseline in ["AGENTS.md", "docs/FEATURE_SPEC.md"]
        .into_iter()
        .map(str::to_owned)
        .chain(extra_read_only.iter().cloned())
    {
        if !rw_files.contains(&baseline)
            && !is_secret_read_surface(&baseline)
            && !ro_files.contains(&baseline)
        {
            ro_files.push(baseline);
        }
    }
    let mut network_rules = Vec::new();
    if let Some(prof) = provider {
        network_rules.push(OpenShellNetworkRule {
            name: prof.id.clone(),
            host: prof.endpoint_host.clone(),
            port: prof.endpoint_port,
            protocol: "rest".to_owned(),
            methods: vec!["GET".to_owned(), "POST".to_owned()],
            binaries: prof.binaries.clone(),
            access: "read-write".to_owned(),
        });
    }
    compile_task_policy(&OpenShellPolicySpec {
        read_only_files: ro_files,
        read_write_files: rw_files,
        run_as_user: "1500".to_owned(),
        run_as_group: "1500".to_owned(),
        landlock_compatibility: "hard_requirement".to_owned(),
        network_rules,
    })
}

fn windows_to_wsl_path(path: &Path) -> Option<String> {
    let raw = path.to_str()?;
    let bytes = raw.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = raw[3..].replace('\\', "/");
        return Some(format!("/mnt/{drive}/{rest}"));
    }
    None
}

pub async fn prove_openshell_policy_with_external_prover(
    candidate: &OpenShellPolicy,
    boundary: &OpenShellPolicy,
    prover_executable: Option<&Path>,
    work_dir: &Path,
    token: Option<&crate::cancel::CancellationToken>,
) -> Result<Value, Error> {
    if let Some(tok) = token {
        tok.check()?;
    }
    let native_proof = prove_openshell_policy_boundary(candidate, boundary)?;
    let native_verified = native_proof.get("verified") == Some(&Value::Boolean(true));
    let mut combined_findings = native_proof
        .get("findings")
        .and_then(domain::items)
        .map(<[Value]>::to_vec)
        .unwrap_or_default();
    let Some(prover_bin) = prover_executable else {
        return Ok(Value::object(vec![
            ("verified", Value::Boolean(native_verified)),
            ("proverMode", Value::string("native-only")),
            ("nativeVerified", Value::Boolean(native_verified)),
            ("externalProver", Value::Null),
            ("candidateDigest", Value::string(&candidate.digest())),
            ("boundaryDigest", Value::string(&boundary.digest())),
            ("findings", Value::Array(combined_findings)),
        ]));
    };
    let prover_str = prover_bin
        .to_str()
        .ok_or_else(|| Error::type_error("OpenShell prover path must be Unicode"))?;
    validate_local_path_arg(prover_str, "OpenShell prover path")?;
    let use_wsl = cfg!(windows) && prover_str.starts_with('/');
    if !use_wsl {
        let meta = fs::symlink_metadata(prover_bin).map_err(|_| {
            Error::type_error("OpenShell prover path must reference an existing regular file")
        })?;
        if !meta.is_file() || secure::linked(&meta) {
            return Err(Error::type_error(
                "OpenShell prover path must be a regular nonsymlink file",
            ));
        }
    }
    files::create_private_directories(work_dir)?;
    secure::check_components(work_dir)?;
    let nonce = files::nonce()?;
    let boundary_file = work_dir.join(format!("openshell-boundary-{nonce}.yaml"));
    let candidate_file = work_dir.join(format!("openshell-candidate-{nonce}.yaml"));
    files::atomic_write_bytes(
        &boundary_file,
        boundary.to_yaml().as_bytes(),
        MAX_CANDIDATE_FILE_BYTES,
    )?;
    files::atomic_write_bytes(
        &candidate_file,
        candidate.to_yaml().as_bytes(),
        MAX_CANDIDATE_FILE_BYTES,
    )?;

    let (boundary_arg, candidate_arg) = if use_wsl {
        let b_wsl = windows_to_wsl_path(&boundary_file).ok_or_else(|| {
            Error::type_error("Cannot map boundary policy path into WSL 2 mount namespace")
        })?;
        let c_wsl = windows_to_wsl_path(&candidate_file).ok_or_else(|| {
            Error::type_error("Cannot map candidate policy path into WSL 2 mount namespace")
        })?;
        (b_wsl, c_wsl)
    } else {
        (
            boundary_file.to_string_lossy().into_owned(),
            candidate_file.to_string_lossy().into_owned(),
        )
    };

    let mut cmd = if use_wsl {
        let mut c = tokio::process::Command::new("wsl");
        c.args([
            "-e",
            prover_str,
            "check",
            "--boundary",
            &boundary_arg,
            &candidate_arg,
            "-o",
            "json",
            "--timeout",
            "10s",
        ]);
        c
    } else {
        let mut c = tokio::process::Command::new(prover_bin);
        c.args([
            "check",
            "--boundary",
            &boundary_arg,
            &candidate_arg,
            "-o",
            "json",
            "--timeout",
            "10s",
        ]);
        c
    };
    cmd.env("OPENSHELL_TELEMETRY_ENABLED", "false")
        .env("NO_COLOR", "1");

    let exec_res = crate::process::execute(
        &mut cmd,
        crate::process::Limits {
            timeout: Some(std::time::Duration::from_secs(15)),
            max_output_bytes: 262_144,
        },
        token,
    )
    .await;
    let _ = fs::remove_file(&boundary_file);
    let _ = fs::remove_file(&candidate_file);

    let (exit_code, stdout_bytes, stderr_bytes) = match exec_res {
        Ok(out) => (0_i32, out.captured.stdout, out.captured.stderr),
        Err(failure) => match failure.kind {
            crate::process::FailureKind::Exit(Some(code)) => {
                (code, failure.captured.stdout, failure.captured.stderr)
            }
            _ => {
                return Err(Error::new(
                    "OpenShellProverError",
                    format!("openshell-prover execution failed: {failure}"),
                )
                .coded("ROSTER_OPENSHELL_PROVER_FAILED"));
            }
        },
    };
    let stdout_str = String::from_utf8_lossy(&stdout_bytes).trim().to_owned();
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).trim().to_owned();
    let parsed_json = crate::json::parse(&Text::from(stdout_str.as_str())).ok();
    let ext_result = parsed_json
        .as_ref()
        .and_then(|v| v.get("result"))
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_else(|| {
            if exit_code == 0 {
                "within_boundary".to_owned()
            } else {
                "error".to_owned()
            }
        });
    let ext_verified = exit_code == 0 && ext_result == "within_boundary";
    if !ext_verified {
        let reason = parsed_json
            .as_ref()
            .and_then(|v| v.get("reason"))
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if !stderr_str.is_empty() {
                    stderr_str.clone()
                } else {
                    format!("openshell-prover reported result={ext_result} (exit {exit_code})")
                }
            });
        combined_findings.push(Value::string(&format!(
            "External openshell-prover rejected candidate policy: {reason}"
        )));
    }
    let overall_verified = native_verified && ext_verified && combined_findings.is_empty();
    let ext_report = Value::object(vec![
        ("exitCode", Value::Number(f64::from(exit_code))),
        ("result", Value::string(&ext_result)),
        ("verified", Value::Boolean(ext_verified)),
        (
            "report",
            parsed_json.unwrap_or_else(|| Value::string(&stdout_str)),
        ),
    ]);
    Ok(Value::object(vec![
        ("verified", Value::Boolean(overall_verified)),
        ("proverMode", Value::string("native+external")),
        ("nativeVerified", Value::Boolean(native_verified)),
        ("externalVerified", Value::Boolean(ext_verified)),
        ("externalProver", ext_report),
        ("candidateDigest", Value::string(&candidate.digest())),
        ("boundaryDigest", Value::string(&boundary.digest())),
        ("findings", Value::Array(combined_findings)),
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

    pub fn emit_policy_and_provider_files(
        &self,
        output_dir: &Path,
        provider: &OpenShellProviderProfile,
    ) -> Result<Value, Error> {
        files::create_private_directories(output_dir)?;
        secure::check_components(output_dir)?;
        let policy_path = output_dir.join("policy.yml");
        let provider_path = output_dir.join("provider.yml");
        crate::paths::ensure_local_path(&policy_path, output_dir)?;
        crate::paths::ensure_local_path(&provider_path, output_dir)?;
        files::atomic_write_bytes(
            &policy_path,
            self.policy.to_yaml().as_bytes(),
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        files::atomic_write_bytes(
            &provider_path,
            provider.to_yaml().as_bytes(),
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        Ok(Value::object(vec![
            ("policyPath", Value::string(&policy_path.to_string_lossy())),
            (
                "providerPath",
                Value::string(&provider_path.to_string_lossy()),
            ),
            ("policyDigest", Value::string(&self.policy_digest)),
            ("providerId", Value::string(&provider.id)),
        ]))
    }

    pub fn hot_reload_network_policy(
        &mut self,
        updated: &OpenShellPolicy,
        boundary: &OpenShellPolicy,
        policy_path: &Path,
    ) -> Result<Value, Error> {
        if self.import_revoked || !matches!(self.state.as_str(), "prepared" | "running") {
            return Err(Error::new(
                "OpenShellLifecycleError",
                "Network policy hot-reload requires an active unrevoked sandbox attempt",
            ));
        }
        if updated.version != self.policy.version
            || updated.include_workdir != self.policy.include_workdir
            || updated.read_only != self.policy.read_only
            || updated.read_write != self.policy.read_write
            || updated.landlock_compatibility != self.policy.landlock_compatibility
            || updated.run_as_user != self.policy.run_as_user
            || updated.run_as_group != self.policy.run_as_group
        {
            return Err(Error::new(
                "PolicyViolation",
                "OpenShell filesystem, Landlock, and process rules are locked at sandbox creation time; only network_policies can be hot-reloaded",
            )
            .coded("ROSTER_OPENSHELL_IMMUTABLE_CREATE_POLICY"));
        }
        let proof = prove_openshell_policy_boundary(updated, boundary)?;
        if proof.get("verified") != Some(&Value::Boolean(true)) {
            return Err(Error::new(
                "PolicyViolation",
                "Hot-reloaded OpenShell network policy exceeds boundary policy",
            )
            .coded("ROSTER_OPENSHELL_BOUNDARY_EXCEEDED"));
        }
        if let Some(parent) = policy_path.parent() {
            files::create_private_directories(parent)?;
        }
        files::atomic_write_bytes(
            policy_path,
            updated.to_yaml().as_bytes(),
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        self.policy = updated.clone();
        self.policy_digest = updated.digest();
        let policy_set_args = vec![
            "policy".to_owned(),
            "set".to_owned(),
            "--policy".to_owned(),
            policy_path.to_string_lossy().into_owned(),
            self.sandbox_name.clone(),
            "--wait".to_owned(),
        ];
        Ok(Value::object(vec![
            ("policyDigest", Value::string(&self.policy_digest)),
            (
                "policySetArgs",
                Value::Array(
                    policy_set_args
                        .into_iter()
                        .map(|s| Value::string(&s))
                        .collect(),
                ),
            ),
        ]))
    }

    pub async fn execute_live<F, Fut>(
        &mut self,
        approval: &OpenShellDispatchApproval,
        token: &crate::cancel::CancellationToken,
        mut runner: F,
    ) -> Result<Value, Error>
    where
        F: FnMut(Vec<String>, Value) -> Fut,
        Fut: std::future::Future<Output = Result<(i32, String, String), Error>>,
    {
        if self.state != "prepared" || self.import_revoked {
            return Err(Error::new(
                "OpenShellLifecycleError",
                "OpenShell sandbox attempt must be in prepared state before live execution",
            ));
        }
        if let Some(parent) = approval.policy_path.parent() {
            files::create_private_directories(parent)?;
        }
        files::atomic_write_bytes(
            &approval.policy_path,
            self.policy.to_yaml().as_bytes(),
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        let plan = approval.validate()?;
        let env = plan
            .get("launchEnvironment")
            .cloned()
            .ok_or_else(|| Error::type_error("Missing OpenShell launch environment"))?;
        let to_vec = |key: &str| -> Result<Vec<String>, Error> {
            plan.get(key)
                .and_then(domain::items)
                .ok_or_else(|| Error::type_error("Missing OpenShell command args"))?
                .iter()
                .filter_map(Value::as_string)
                .map(Text::to_utf8)
                .collect()
        };
        let create_args = to_vec("createArgs")?;
        let attach_args = to_vec("attachArgs")?;
        let exec_args = to_vec("execArgs")?;
        let detach_args = to_vec("detachArgs")?;
        let delete_args = to_vec("deleteArgs")?;

        self.start_running()?;

        let create_res = runner(create_args, env.clone()).await;
        match create_res {
            Ok((0, _, _)) => {}
            Ok((create_exit, _, create_err)) => {
                let detach_res = runner(detach_args, env.clone()).await;
                let _ = runner(delete_args, env).await;
                let _ = self.cancel_and_detach(matches!(detach_res, Ok((0, _, _))));
                return Err(Error::new(
                    "OpenShellExecutionError",
                    format!("OpenShell sandbox creation failed (exit {create_exit}): {create_err}"),
                ));
            }
            Err(err) => {
                let detach_res = runner(detach_args, env.clone()).await;
                let _ = runner(delete_args, env).await;
                let _ = self.cancel_and_detach(matches!(detach_res, Ok((0, _, _))));
                return Err(err);
            }
        }

        let attach_res = runner(attach_args, env.clone()).await;
        match attach_res {
            Ok((0, _, _)) => {}
            Ok((attach_exit, _, attach_err)) => {
                let detach_res = runner(detach_args, env.clone()).await;
                let _ = runner(delete_args, env).await;
                let _ = self.cancel_and_detach(matches!(detach_res, Ok((0, _, _))));
                return Err(Error::new(
                    "OpenShellExecutionError",
                    format!("OpenShell provider attach failed (exit {attach_exit}): {attach_err}"),
                ));
            }
            Err(err) => {
                let detach_res = runner(detach_args, env.clone()).await;
                let _ = runner(delete_args, env).await;
                let _ = self.cancel_and_detach(matches!(detach_res, Ok((0, _, _))));
                return Err(err);
            }
        }

        let timeout_dur = std::time::Duration::from_millis(approval.timeout_ms);
        let exec_outcome = if token.check().is_err() {
            Err(token
                .check()
                .err()
                .unwrap_or_else(|| Error::new("Cancelled", "OpenShell execution cancelled")))
        } else {
            let exec_fut = runner(exec_args, env.clone());
            tokio::pin!(exec_fut);
            tokio::select! {
                biased;
                reason = token.cancelled() => Err(reason?.error()),
                () = tokio::time::sleep(timeout_dur) => Err(Error::new("OpenShellTimeout", "OpenShell sandbox command timed out").coded("ROSTER_TASK_TIMEOUT")),
                res = &mut exec_fut => res,
            }
        };

        let detach_res = runner(detach_args, env.clone()).await;
        let delete_res = runner(delete_args, env).await;
        let provider_detached = matches!(detach_res, Ok((0, _, _)));
        let sandbox_deleted = matches!(delete_res, Ok((0, _, _)));
        self.provider_detached = provider_detached;

        if exec_outcome.is_err() || token.check().is_err() {
            let _ = self.cancel_and_detach(provider_detached && sandbox_deleted);
            if !provider_detached || !sandbox_deleted {
                return Err(Error::new(
                    "OpenShellCleanupError",
                    "OpenShell provider detachment or sandbox deletion could not be verified",
                )
                .coded("ROSTER_OPENSHELL_CLEANUP_UNVERIFIED"));
            }
            if let Err(err) = exec_outcome {
                return Err(err);
            }
            token.check()?;
        }

        if !provider_detached || !sandbox_deleted {
            let _ = self.cancel_and_detach(false);
            return Err(Error::new(
                "OpenShellCleanupError",
                "OpenShell provider detachment or sandbox deletion could not be verified",
            )
            .coded("ROSTER_OPENSHELL_CLEANUP_UNVERIFIED"));
        }

        let (exit_code, stdout, stderr) = exec_outcome?;
        if exit_code != 0 {
            self.state = "failed".to_owned();
            return Err(Error::new(
                "OpenShellExecutionError",
                format!("OpenShell sandbox command exited with code {exit_code}: {stderr}"),
            ));
        }
        Ok(Value::object(vec![
            ("state", Value::string(&self.state)),
            ("exitCode", Value::Number(f64::from(exit_code))),
            ("stdout", Value::string(&stdout)),
            ("stderr", Value::string(&stderr)),
            ("providerDetached", Value::Boolean(provider_detached)),
            ("sandboxDeleted", Value::Boolean(sandbox_deleted)),
            ("policyDigest", Value::string(&self.policy_digest)),
            ("taskDigest", Value::string(&self.task_digest)),
        ]))
    }

    pub async fn execute_live_owned(
        &mut self,
        approval: &OpenShellDispatchApproval,
        cwd: &Path,
        token: &crate::cancel::CancellationToken,
    ) -> Result<Value, Error> {
        let rt_exe = approval.runtime_executable.clone();
        let work_dir = cwd.to_path_buf();
        let step_timeout =
            std::time::Duration::from_millis(approval.timeout_ms.clamp(5_000, 900_000));
        self.execute_live(approval, token, move |args, env_val| {
            let rt_exe = rt_exe.clone();
            let work_dir = work_dir.clone();
            async move {
                let mut cmd = tokio::process::Command::new(&rt_exe);
                cmd.args(&args).current_dir(&work_dir).env_clear();
                for keep in [
                    "PATH",
                    "SystemRoot",
                    "SYSTEMROOT",
                    "WINDIR",
                    "COMSPEC",
                    "ComSpec",
                ] {
                    if let Ok(v) = std::env::var(keep) {
                        cmd.env(keep, v);
                    }
                }
                if let Some(entries) = domain::members(&env_val) {
                    for (k, v) in entries {
                        if let (Ok(ks), Some(vs)) = (k.to_utf8(), v.as_string()) {
                            if let Ok(vu) = vs.to_utf8() {
                                cmd.env(ks, vu);
                            }
                        }
                    }
                }
                cmd.env("OPENSHELL_TELEMETRY_ENABLED", "false");
                match crate::process::execute(
                    &mut cmd,
                    crate::process::Limits {
                        timeout: Some(step_timeout),
                        max_output_bytes: 1024 * 1024,
                    },
                    None,
                )
                .await
                {
                    Ok(out) => Ok((
                        0,
                        String::from_utf8_lossy(&out.captured.stdout).into_owned(),
                        String::from_utf8_lossy(&out.captured.stderr).into_owned(),
                    )),
                    Err(failure) => match failure.kind {
                        crate::process::FailureKind::Exit(Some(code)) => Ok((
                            code,
                            String::from_utf8_lossy(&failure.captured.stdout).into_owned(),
                            String::from_utf8_lossy(&failure.captured.stderr).into_owned(),
                        )),
                        _ => Err(Error::new(
                            "OpenShellProcessError",
                            format!("Owned OpenShell process failed: {failure}"),
                        )
                        .coded("ROSTER_PROCESS_CLEANUP")),
                    },
                }
            }
        })
        .await
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
            "--approve-live",
            "--runtime",
            "--prover",
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
        } else if flag == "--approve-live" {
            options.approve_live = true;
        } else {
            index += 1;
            let value = args
                .get(index)
                .filter(|v| !trim(&Text::from(v.as_str()).0).is_empty() && !v.starts_with("--"))
                .ok_or_else(usage)?;
            match flag.as_str() {
                "--runtime" => options.runtime_path = Some(value.clone()),
                "--prover" => options.prover_path = Some(value.clone()),
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

pub(crate) fn validate_local_path_arg(path: &str, label: &str) -> Result<(), Error> {
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
    if let Some(prover) = &options.prover_path {
        validate_local_path_arg(prover, "OpenShell prover path")?;
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
    let mut policy_landlock_hard = options.landlock_state == "hard_requirement";
    let policy_reason = if !options.opt_in {
        "Policy check skipped until --opt-in is supplied."
    } else if let Some(policy_path) = &options.policy_path {
        match fs::symlink_metadata(policy_path) {
            Ok(meta) if meta.is_file() && !secure::linked(&meta) && meta.len() <= 65_536 => {
                policy_present = true;
                if let Ok(content) = fs::read_to_string(policy_path) {
                    if content.lines().any(|l| l.trim() == "version: 1")
                        && content
                            .lines()
                            .any(|l| l.trim() == "compatibility: hard_requirement")
                    {
                        policy_landlock_hard = true;
                    }
                }
                "Regular nonsymlink policy file present."
            }
            Ok(_) => "Policy path is not a bounded regular nonsymlink file.",
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
    if !policy_landlock_hard {
        blockers.push("OPENSHELL_LANDLOCK_UNVERIFIED");
    }
    if !options.approve_live {
        blockers.push("OPENSHELL_LIVE_DISPATCH_UNAPPROVED");
    }
    let ok = blockers.is_empty();
    let status = if ok { "ready" } else { "blocked" };
    let prover_present = options.prover_path.as_ref().is_some_and(|p| {
        fs::symlink_metadata(p).is_ok_and(|meta| meta.is_file() && !secure::linked(&meta))
    });
    Ok(Value::object(vec![
        ("ok", Value::Boolean(ok)),
        ("status", Value::string(status)),
        ("optIn", Value::Boolean(options.opt_in)),
        ("approveLive", Value::Boolean(options.approve_live)),
        ("runtimePresent", Value::Boolean(runtime_present)),
        ("runtimeReason", Value::string(runtime_reason)),
        ("proverPresent", Value::Boolean(prover_present)),
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
    let blockers_list = result
        .get("blockers")
        .and_then(domain::items)
        .ok_or_else(|| Error::type_error("Missing OpenShell blockers"))?
        .iter()
        .filter_map(Value::as_string)
        .map(Text::to_utf8)
        .collect::<Result<Vec<_>, _>>()?;
    let blockers = if blockers_list.is_empty() {
        "none".to_owned()
    } else {
        blockers_list.join(", ")
    };
    Ok(format!(
        "OpenShell doctor: {status} (offline; no gateway, container, or credential probe)\nRuntime: {runtime_reason}\nGateway: {gateway_reason}\nPolicy: {policy_reason}\nBlockers: {blockers}\n"
    ))
}

#[derive(Debug, Clone)]
pub struct OpenShellDispatchApproval {
    pub runtime_executable: std::path::PathBuf,
    pub policy_path: std::path::PathBuf,
    pub gateway_url: String,
    pub sandbox_name: String,
    pub image: String,
    pub policy: OpenShellPolicy,
    pub provider: OpenShellProviderProfile,
    pub command: Vec<String>,
    pub timeout_ms: u64,
}

impl OpenShellDispatchApproval {
    pub fn validate(&self) -> Result<Value, Error> {
        let rt_str = self
            .runtime_executable
            .to_str()
            .ok_or_else(|| Error::type_error("OpenShell runtime path must be Unicode"))?;
        validate_local_path_arg(rt_str, "OpenShell runtime path")?;
        let rt_meta = fs::symlink_metadata(&self.runtime_executable).map_err(|_| {
            Error::type_error("OpenShell runtime path must reference an existing regular file")
        })?;
        if !rt_meta.is_file() || secure::linked(&rt_meta) {
            return Err(Error::type_error(
                "OpenShell runtime path must be a regular nonsymlink file",
            ));
        }
        let pol_str = self
            .policy_path
            .to_str()
            .ok_or_else(|| Error::type_error("OpenShell policy path must be Unicode"))?;
        validate_local_path_arg(pol_str, "OpenShell policy path")?;
        let pol_meta = fs::symlink_metadata(&self.policy_path).map_err(|_| {
            Error::type_error("OpenShell policy path must reference an existing regular file")
        })?;
        if !pol_meta.is_file() || secure::linked(&pol_meta) {
            return Err(Error::type_error(
                "OpenShell policy path must be a regular nonsymlink file",
            ));
        }
        let parsed_gw = crate::config::base_url(
            &Text::from(self.gateway_url.as_str()),
            "OpenShell gateway URL",
        )?;
        if let Some(host) = parsed_gw.host_str() {
            if is_forbidden_metadata_host(host) {
                return Err(Error::new(
                    "PolicyViolation",
                    format!("OpenShell gateway URL denies cloud metadata endpoint: {host}"),
                )
                .coded("ROSTER_OPENSHELL_METADATA_DENIED"));
            }
        }
        let valid_sandbox =
            fleet::validate_id(Some(&Value::string(&self.sandbox_name)))?.to_utf8()?;
        let valid_provider =
            fleet::validate_id(Some(&Value::string(&self.provider.id)))?.to_utf8()?;
        if self.image.trim().is_empty()
            || self.image.starts_with('-')
            || self.image.bytes().any(|b| matches!(b, 0..=32 | 127))
        {
            return Err(Error::type_error(
                "OpenShell sandbox image must be a nonempty image reference without leading '-' or whitespace",
            ));
        }
        let proof = prove_openshell_policy_boundary(&self.policy, &self.policy)?;
        if proof.get("verified") != Some(&Value::Boolean(true)) {
            return Err(Error::new(
                "PolicyViolation",
                "OpenShell dispatch approval requires a boundary-verified hard_requirement policy",
            ));
        }
        if self.command.is_empty()
            || self.command.len() > 32
            || self
                .command
                .iter()
                .any(|arg| arg.is_empty() || arg.len() > 1024 || arg.contains('\0'))
        {
            return Err(Error::type_error(
                "OpenShell sandbox command must contain 1..=32 bounded arguments",
            ));
        }
        let cmd_bin = validate_posix_binary_path(&self.command[0])?;
        if !self.provider.binaries.contains(&cmd_bin) {
            return Err(Error::new(
                "PolicyViolation",
                format!(
                    "OpenShell command binary '{cmd_bin}' is not authorized by provider binaries allow-list"
                ),
            )
            .coded("ROSTER_OPENSHELL_BINARY_DENIED"));
        }
        if self.timeout_ms == 0 || self.timeout_ms > 900_000 {
            return Err(Error::type_error(
                "OpenShell sandbox timeout_ms must be within 1..=900000 ms",
            ));
        }
        let launch_env = openshell_launch_environment(&self.gateway_url, &self.provider)?;
        let create_args = vec![
            "sandbox".to_owned(),
            "create".to_owned(),
            "--name".to_owned(),
            valid_sandbox.clone(),
            "--from".to_owned(),
            self.image.clone(),
            "--policy".to_owned(),
            pol_str.to_owned(),
            "--provider".to_owned(),
            valid_provider.clone(),
        ];
        let mut exec_args = vec![
            "sandbox".to_owned(),
            "exec".to_owned(),
            "--name".to_owned(),
            valid_sandbox.clone(),
            "--".to_owned(),
        ];
        exec_args.extend(self.command.iter().cloned());
        let attach_args = vec![
            "sandbox".to_owned(),
            "provider".to_owned(),
            "attach".to_owned(),
            valid_sandbox.clone(),
            valid_provider.clone(),
            "--wait".to_owned(),
        ];
        let policy_set_args = vec![
            "policy".to_owned(),
            "set".to_owned(),
            "--policy".to_owned(),
            pol_str.to_owned(),
            valid_sandbox.clone(),
            "--wait".to_owned(),
        ];
        let detach_args = vec![
            "sandbox".to_owned(),
            "provider".to_owned(),
            "detach".to_owned(),
            valid_sandbox.clone(),
            valid_provider,
            "--wait".to_owned(),
        ];
        let delete_args = vec!["sandbox".to_owned(), "delete".to_owned(), valid_sandbox];
        Ok(Value::object(vec![
            (
                "createArgs",
                Value::Array(create_args.into_iter().map(|s| Value::string(&s)).collect()),
            ),
            (
                "attachArgs",
                Value::Array(attach_args.into_iter().map(|s| Value::string(&s)).collect()),
            ),
            (
                "policySetArgs",
                Value::Array(
                    policy_set_args
                        .into_iter()
                        .map(|s| Value::string(&s))
                        .collect(),
                ),
            ),
            (
                "execArgs",
                Value::Array(exec_args.into_iter().map(|s| Value::string(&s)).collect()),
            ),
            (
                "detachArgs",
                Value::Array(detach_args.into_iter().map(|s| Value::string(&s)).collect()),
            ),
            (
                "deleteArgs",
                Value::Array(delete_args.into_iter().map(|s| Value::string(&s)).collect()),
            ),
            ("launchEnvironment", launch_env),
            ("policyDigest", Value::string(&self.policy.digest())),
        ]))
    }
}

pub async fn run_openshell_sandbox_command<F, Fut>(
    approval: &OpenShellDispatchApproval,
    token: &crate::cancel::CancellationToken,
    mut runner: F,
) -> Result<Value, Error>
where
    F: FnMut(Vec<String>, Value) -> Fut,
    Fut: std::future::Future<Output = Result<(i32, String, String), Error>>,
{
    token.check()?;
    let plan = approval.validate()?;
    let env = plan
        .get("launchEnvironment")
        .cloned()
        .ok_or_else(|| Error::type_error("Missing OpenShell launch environment"))?;
    let to_vec = |key: &str| -> Result<Vec<String>, Error> {
        plan.get(key)
            .and_then(domain::items)
            .ok_or_else(|| Error::type_error("Missing OpenShell command args"))?
            .iter()
            .filter_map(Value::as_string)
            .map(Text::to_utf8)
            .collect()
    };
    let create_args = to_vec("createArgs")?;
    let exec_args = to_vec("execArgs")?;
    let detach_args = to_vec("detachArgs")?;
    let delete_args = to_vec("deleteArgs")?;

    let create_res = runner(create_args, env.clone()).await;
    match create_res {
        Ok((0, _, _)) => {}
        Ok((create_exit, _, create_err)) => {
            let _ = runner(detach_args, env.clone()).await;
            let _ = runner(delete_args, env).await;
            return Err(Error::new(
                "OpenShellExecutionError",
                format!("OpenShell sandbox creation failed (exit {create_exit}): {create_err}"),
            ));
        }
        Err(err) => {
            let _ = runner(detach_args, env.clone()).await;
            let _ = runner(delete_args, env).await;
            return Err(err);
        }
    }

    let timeout_dur = std::time::Duration::from_millis(approval.timeout_ms);
    let exec_outcome = if token.check().is_err() {
        Err(token
            .check()
            .err()
            .unwrap_or_else(|| Error::new("Cancelled", "OpenShell execution cancelled")))
    } else {
        let exec_fut = runner(exec_args, env.clone());
        tokio::pin!(exec_fut);
        tokio::select! {
            biased;
            reason = token.cancelled() => Err(reason?.error()),
            () = tokio::time::sleep(timeout_dur) => Err(Error::new("OpenShellTimeout", "OpenShell sandbox command timed out").coded("ROSTER_TASK_TIMEOUT")),
            res = &mut exec_fut => res,
        }
    };

    let detach_res = runner(detach_args, env.clone()).await;
    let delete_res = runner(delete_args, env).await;
    let provider_detached = matches!(detach_res, Ok((0, _, _)));
    let sandbox_deleted = matches!(delete_res, Ok((0, _, _)));
    if !provider_detached || !sandbox_deleted {
        return Err(Error::new(
            "OpenShellCleanupError",
            "OpenShell provider detachment or sandbox deletion could not be verified",
        )
        .coded("ROSTER_OPENSHELL_CLEANUP_UNVERIFIED"));
    }

    let (exit_code, stdout, stderr) = exec_outcome?;
    token.check()?;
    Ok(Value::object(vec![
        ("exitCode", Value::Number(f64::from(exit_code))),
        ("stdout", Value::string(&stdout)),
        ("stderr", Value::string(&stderr)),
        ("providerDetached", Value::Boolean(provider_detached)),
        ("sandboxDeleted", Value::Boolean(sandbox_deleted)),
        (
            "policyDigest",
            plan.get("policyDigest").cloned().unwrap_or(Value::Null),
        ),
    ]))
}

pub fn require_openshell_runtime_enabled(config: &Value) -> Result<(), Error> {
    let Some(os_cfg) = config.get("openshell") else {
        return Err(Error::new(
            "OpenShellConfigurationError",
            "OpenShell runtime requires an explicit openshell section in .roster/config.yml with enabled: true",
        )
        .coded("ROSTER_OPENSHELL_NOT_ENABLED"));
    };
    if os_cfg.get("enabled") != Some(&Value::Boolean(true)) {
        return Err(Error::new(
            "OpenShellConfigurationError",
            "OpenShell runtime is disabled by default; set openshell.enabled: true in .roster/config.yml",
        )
        .coded("ROSTER_OPENSHELL_NOT_ENABLED"));
    }
    let runtime_path = os_cfg
        .get("runtime_path")
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            Error::new(
                "OpenShellConfigurationError",
                "OpenShell runtime requires openshell.runtime_path in .roster/config.yml",
            )
            .coded("ROSTER_OPENSHELL_RUNTIME_UNPINNED")
        })?;
    validate_local_path_arg(&runtime_path, "openshell.runtime_path")?;
    let gateway_url = os_cfg
        .get("gateway_url")
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            Error::new(
                "OpenShellConfigurationError",
                "OpenShell runtime requires openshell.gateway_url in .roster/config.yml",
            )
            .coded("ROSTER_OPENSHELL_GATEWAY_UNCONFIGURED")
        })?;
    let parsed =
        crate::config::base_url(&Text::from(gateway_url.as_str()), "openshell.gateway_url")?;
    if let Some(host) = parsed.host_str() {
        if is_forbidden_metadata_host(host) {
            return Err(Error::new(
                "PolicyViolation",
                format!("OpenShell gateway URL denies cloud metadata endpoint: {host}"),
            )
            .coded("ROSTER_OPENSHELL_METADATA_DENIED"));
        }
    }
    Ok(())
}

pub async fn prepare_delivery_openshell_attempt(
    worktree: &Path,
    repo_root: &Path,
    config: &Value,
    task_id: &str,
    session_id: &str,
    token: &crate::cancel::CancellationToken,
) -> Result<OpenShellSandboxAttempt, Error> {
    token.check()?;
    require_openshell_runtime_enabled(config)?;
    let task_path = worktree.join("TASK.md");
    let task_bytes = checked_regular_file(&task_path)?;
    let task_source = String::from_utf8(task_bytes)
        .map_err(|_| Error::type_error("TASK.md must be valid UTF-8"))?;
    let recipe_path = worktree.join("RECIPE.yml");
    let recipe_source = if recipe_path.is_file() {
        let bytes = checked_regular_file(&recipe_path)?;
        Some(
            String::from_utf8(bytes)
                .map_err(|_| Error::type_error("RECIPE.yml must be valid UTF-8"))?,
        )
    } else {
        None
    };
    let catalog = fleet::load(repo_root).ok();
    let binaries = vec![
        "/usr/local/bin/roster".to_owned(),
        "/usr/bin/curl".to_owned(),
    ];
    let provider = compile_active_llm_provider_profile(config, catalog.as_ref(), &binaries)?;
    let policy =
        compile_seat_task_policy(&task_source, recipe_source.as_deref(), Some(&provider), &[])?;
    let openshell_dir = worktree.join(".roster").join("openshell");
    let prover_path = config
        .get("openshell")
        .and_then(|o| o.get("prover_path"))
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from);
    let proof = prove_openshell_policy_with_external_prover(
        &policy,
        &policy,
        prover_path.as_deref(),
        &openshell_dir,
        Some(token),
    )
    .await?;
    if proof.get("verified") != Some(&Value::Boolean(true)) {
        return Err(Error::new(
            "PolicyViolation",
            "Seat task policy failed OpenShell boundary prover verification",
        )
        .coded("ROSTER_OPENSHELL_BOUNDARY_EXCEEDED"));
    }
    let clean_task: String = task_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(32)
        .collect();
    let sandbox_name = format!("sbx-{}", clean_task.trim_matches('-'));
    let attempt = OpenShellSandboxAttempt::prepare(
        task_id,
        session_id,
        if sandbox_name == "sbx-" {
            "sbx-task"
        } else {
            &sandbox_name
        },
        &provider.id,
        &policy,
        &task_source,
    )?;
    attempt.emit_policy_and_provider_files(&openshell_dir, &provider)?;
    Ok(attempt)
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

    #[test]
    fn fleet_catalog_compiles_all_six_profiles_into_verified_openshell_policies_and_providers()
    -> Result<(), Error> {
        let catalog_src = Text::from(concat!(
            "profiles:\n",
            "  - id: default\n",
            "    base_url: http://127.0.0.1:11435/v1\n",
            "    model: qwen3.8:27b-smtek\n",
            "    provider: vllm\n",
            "    context_max: 64500\n",
            "    concurrency: 1\n",
            "    hardware: unspecified\n",
            "    notes: default\n",
            "  - id: rtx-3090-qwen\n",
            "    base_url: http://127.0.0.1:11435/v1\n",
            "    model: qwen3.8:27b-smtek\n",
            "    provider: vllm\n",
            "    context_max: 65536\n",
            "    concurrency: 1\n",
            "    hardware: rtx-3090-24gb\n",
            "    task_class: [fix, docs, test]\n",
            "    notes: local\n",
            "  - id: aperture-qwen\n",
            "    base_url: https://aperture.xen.blackmesaresearch.ai/v1\n",
            "    model: qwen3.8-27b\n",
            "    provider: vllm\n",
            "    context_max: 262144\n",
            "    concurrency: 1\n",
            "    hardware: rtx-6000-ada-48gb\n",
            "    task_class: [feat, fix, docs, test]\n",
            "    notes: aperture\n",
            "  - id: aperture-glm\n",
            "    base_url: https://aperture.xen.blackmesaresearch.ai/v1\n",
            "    model: glm-5.3-flash\n",
            "    provider: vllm\n",
            "    context_max: 1048576\n",
            "    concurrency: 1\n",
            "    hardware: dgx-spark-2n\n",
            "    task_class: [feat, fix, docs, test]\n",
            "    notes: aperture-glm\n",
            "  - id: aperture-dsv4\n",
            "    base_url: https://aperture.xen.blackmesaresearch.ai/v1\n",
            "    model: deepseek-v4-flash\n",
            "    provider: vllm\n",
            "    context_max: 1048576\n",
            "    concurrency: 1\n",
            "    hardware: dgx-spark-2n\n",
            "    task_class: [feat, fix, docs, test]\n",
            "    notes: aperture-dsv4\n",
            "  - id: spark-dsv41\n",
            "    base_url: http://192.168.1.48:8000/v1\n",
            "    model: deepseek-v4.1-flash\n",
            "    provider: vllm\n",
            "    context_max: 1048576\n",
            "    concurrency: 1\n",
            "    hardware: dgx-spark-4n\n",
            "    task_class: [feat, fix, docs, test]\n",
            "    notes: spark\n",
        ));
        let catalog = fleet::parse(&catalog_src)?;
        assert_eq!(catalog.profiles().len(), 6);
        let bins = vec!["/usr/local/bin/roster".to_owned()];
        for profile in catalog.profiles() {
            let compiled = compile_fleet_provider_profile(profile.as_value(), &bins)?;
            let env = openshell_launch_environment("http://127.0.0.1:8080", &compiled)?;
            assert_eq!(
                env.get("OPENSHELL_TELEMETRY_ENABLED"),
                Some(&Value::string("false"))
            );
            let policy = compile_task_policy(&OpenShellPolicySpec {
                read_only_files: vec!["docs/OPENSHELL_DESIGN.md".to_owned()],
                read_write_files: vec!["rust/openshell.rs".to_owned()],
                run_as_user: "1500".to_owned(),
                run_as_group: "1500".to_owned(),
                landlock_compatibility: "hard_requirement".to_owned(),
                network_rules: vec![OpenShellNetworkRule {
                    name: compiled.id.clone(),
                    host: compiled.endpoint_host.clone(),
                    port: compiled.endpoint_port,
                    protocol: "rest".to_owned(),
                    methods: vec!["POST".to_owned()],
                    binaries: bins.clone(),
                    access: "read-write".to_owned(),
                }],
            })?;
            let proof = prove_openshell_policy_boundary(&policy, &policy)?;
            assert_eq!(proof.get("verified"), Some(&Value::Boolean(true)));
        }
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_openshell_dispatch_doctor_ready_and_orchestrator_handoffs_succeed()
    -> Result<(), Error> {
        let temp = std::env::temp_dir().join(format!(
            "roster-openshell-live-{}-{}",
            std::process::id(),
            files::nonce()?
        ));
        files::create_private_directories(&temp)?;
        let fake_runtime = temp.join("openshell.exe");
        let fake_policy = temp.join("policy.yml");
        files::atomic_write_bytes(&fake_runtime, b"stub", MAX_CANDIDATE_FILE_BYTES)?;
        files::atomic_write_bytes(
            &fake_policy,
            b"version: 1\nlandlock:\n  compatibility: hard_requirement\n",
            MAX_CANDIDATE_FILE_BYTES,
        )?;

        let ready_opts = parse_openshell_doctor_args(&[
            "--opt-in".to_owned(),
            "--approve-live".to_owned(),
            "--runtime".to_owned(),
            fake_runtime.to_string_lossy().into_owned(),
            "--gateway".to_owned(),
            "http://127.0.0.1:8080".to_owned(),
            "--policy".to_owned(),
            fake_policy.to_string_lossy().into_owned(),
            "--provider".to_owned(),
            "rtx-3090-qwen".to_owned(),
        ])?;
        let ready_report = check_openshell_doctor(&ready_opts)?;
        assert_eq!(ready_report.get("ok"), Some(&Value::Boolean(true)));
        assert_eq!(ready_report.get("status"), Some(&Value::string("ready")));
        assert!(format_openshell_doctor(&ready_report)?.contains("OpenShell doctor: ready"));

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
        let provider =
            compile_fleet_provider_profile(&local_profile, &["/usr/local/bin/roster".to_owned()])?;
        let policy = compile_task_policy(&OpenShellPolicySpec {
            read_only_files: vec!["docs/OPENSHELL_DESIGN.md".to_owned()],
            read_write_files: vec!["rust/openshell.rs".to_owned()],
            ..OpenShellPolicySpec::default()
        })?;
        let approval = OpenShellDispatchApproval {
            runtime_executable: fake_runtime,
            policy_path: fake_policy,
            gateway_url: "http://127.0.0.1:8080".to_owned(),
            sandbox_name: "sbx-live-687".to_owned(),
            image: "ubuntu:24.04".to_owned(),
            policy,
            provider,
            command: vec!["/usr/local/bin/roster".to_owned(), "--help".to_owned()],
            timeout_ms: 30_000,
        };
        let source = crate::cancel::CancellationSource::default();
        let mut seen_steps = Vec::new();
        let run_result = run_openshell_sandbox_command(&approval, &source.token(), |args, _env| {
            seen_steps.push(args.join(" "));
            async move { Ok((0, "sandbox output ok\n".to_owned(), String::new())) }
        })
        .await?;
        assert_eq!(run_result.get("exitCode"), Some(&Value::Number(0.0)));
        assert_eq!(
            run_result.get("providerDetached"),
            Some(&Value::Boolean(true))
        );
        assert_eq!(
            run_result.get("sandboxDeleted"),
            Some(&Value::Boolean(true))
        );
        assert_eq!(seen_steps.len(), 4);

        let unix_plan =
            crate::execution::owned::unix_process_group_containment_plan(4242, 65536, Some(5000))?;
        assert_eq!(
            unix_plan.get("provider"),
            Some(&Value::string("unix-process-group"))
        );

        files::atomic_write_bytes(
            &temp.join("RESULT.md"),
            b"# Result\n\n## Verification\n\nChecks: PASS\n\n## Run\n\nAI-Run: 1|local|qwen3.8-27b@-|m|1000/65536|200|roster-1234567890abcdef|issue-684\n\n## Summary\n\nDelivered verified changes.\n",
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        files::atomic_write_bytes(
            &temp.join("REVIEW.md"),
            b"# Review\n\nVerdict: pass\n",
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        let pub_handoff = crate::run::prepare_verified_publication_handoff(
            &temp,
            "feat(cli): verified handoff",
            "qwen3.8-27b",
            Some(684),
            false,
            true,
            true,
        )?;
        assert_eq!(pub_handoff.get("eligible"), Some(&Value::Boolean(true)));
        assert_eq!(
            pub_handoff.get("model"),
            Some(&Value::string("qwen3.8-27b"))
        );

        let machine_home = temp.join("machine-home");
        let empty_workspace = temp.join("empty-workspace");
        files::create_private_directories(&machine_home.join(".roster"))?;
        files::create_private_directories(&empty_workspace)?;
        files::atomic_write_bytes(
            &machine_home.join(".roster").join("fleet.yml"),
            b"profiles:\n  - id: machine-gpu\n    base_url: \"http://127.0.0.1:11435/v1\"\n    model: \"qwen3.8:27b-smtek\"\n    provider: \"vllm\"\n    context_max: 65536\n    concurrency: 1\n    hardware: \"rtx-3090-24gb\"\n    notes: \"machine-wide\"\n",
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        files::atomic_write_bytes(
            &machine_home.join(".roster").join("capabilities.yml"),
            b"capabilities:\n  - profile_id: \"machine-gpu\"\n    task_class: \"feat\"\n    suggested_difficulty: 4\n    context_max: 65536\n    concurrency: 1\n    notes: \"machine-prior\"\n",
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        let machine_env = Value::object(vec![(
            "ROSTER_MACHINE_HOME",
            Value::string(&machine_home.to_string_lossy()),
        )]);
        let loaded_machine_fleet =
            crate::fleet::load_with_env(&empty_workspace, None, Some(&machine_env))?;
        assert_eq!(loaded_machine_fleet.profiles().len(), 1);
        assert_eq!(
            loaded_machine_fleet.profiles()[0].as_value().get("id"),
            Some(&Value::string("machine-gpu"))
        );
        let fleet_table = crate::local_cli::list_fleet(&empty_workspace, &machine_env)?;
        assert!(fleet_table.contains("machine-gpu\tqwen3.8:27b-smtek"));
        let loaded_caps = crate::capabilities::load_with_env(
            &empty_workspace,
            Path::new(env!("CARGO_MANIFEST_DIR")),
            Some(&machine_env),
        )?;
        assert!(
            loaded_caps
                .records()
                .iter()
                .any(|r| r.get("profile_id") == Some(&Value::string("machine-gpu")))
        );

        let _ = fs::remove_dir_all(&temp);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn seat_policy_emission_hot_reload_live_lifecycle_and_external_prover_validate_e2e()
    -> Result<(), Error> {
        let temp = std::env::temp_dir().join(format!(
            "roster-openshell-e2e-{}-{}",
            std::process::id(),
            files::nonce()?
        ));
        files::create_private_directories(&temp)?;
        let fake_runtime = temp.join("openshell.exe");
        files::atomic_write_bytes(&fake_runtime, b"stub", MAX_CANDIDATE_FILE_BYTES)?;

        let task_md = concat!(
            "# Task\n\n",
            "task_class: feat\n",
            "difficulty: 2\n",
            "estimate_min: 15\n",
            "skills: [implement]\n\n",
            "## Outcome\n\n",
            "Wire OpenShell policy compiler into seat delivery.\n\n",
            "## Files allowed\n\n",
            "- rust/openshell.rs\n",
            "- docs/OPENSHELL_DESIGN.md\n\n",
            "## Acceptance checks\n\n",
            "1. Seat policy compiles and passes boundary prover.\n"
        );
        let recipe_yml = concat!(
            "version: 1\n",
            "ask: Wire OpenShell policy compiler\n",
            "seats:\n",
            "  - id: planner\n",
            "    role: planner\n",
            "  - id: coder\n",
            "    role: coder\n",
            "  - id: reviewer\n",
            "    role: reviewer\n"
        );
        files::atomic_write_bytes(
            &temp.join("TASK.md"),
            task_md.as_bytes(),
            MAX_CANDIDATE_FILE_BYTES,
        )?;
        files::atomic_write_bytes(
            &temp.join("RECIPE.yml"),
            recipe_yml.as_bytes(),
            MAX_CANDIDATE_FILE_BYTES,
        )?;

        let cfg = Value::object(vec![
            (
                "llm",
                Value::object(vec![
                    (
                        "base_url",
                        Value::string("https://aperture.xen.blackmesaresearch.ai/v1"),
                    ),
                    ("model", Value::string("qwen3.8-27b")),
                    ("api_key_env", Value::string("APERTURE_API_KEY")),
                    ("context_max", Value::Number(262_144.0)),
                ]),
            ),
            (
                "openshell",
                Value::object(vec![
                    ("enabled", Value::Boolean(true)),
                    (
                        "runtime_path",
                        Value::string(&fake_runtime.to_string_lossy()),
                    ),
                    ("gateway_url", Value::string("https://127.0.0.1:17670")),
                ]),
            ),
        ]);

        let bins = vec![
            "/usr/local/bin/roster".to_owned(),
            "/usr/bin/curl".to_owned(),
        ];
        let provider = compile_active_llm_provider_profile(&cfg, None, &bins)?;
        assert!(provider.to_yaml().contains("name: APERTURE_API_KEY"));
        assert!(provider.to_yaml().contains("env_var: APERTURE_API_KEY"));

        let seat_policy =
            compile_seat_task_policy(task_md, Some(recipe_yml), Some(&provider), &[])?;
        let policy_yaml = seat_policy.to_yaml();
        assert!(policy_yaml.contains("enforcement: enforce"));
        assert!(policy_yaml.contains("- path: /usr/local/bin/roster"));
        assert!(policy_yaml.contains("- /sandbox/workspace/rust/openshell.rs"));

        let source = crate::cancel::CancellationSource::default();
        let token = source.token();
        let mut attempt =
            prepare_delivery_openshell_attempt(&temp, &temp, &cfg, "issue-756", "sess-1", &token)
                .await?;
        assert!(temp.join(".roster/openshell/policy.yml").is_file());
        assert!(temp.join(".roster/openshell/provider.yml").is_file());

        let mut fs_mutated = seat_policy.clone();
        fs_mutated
            .read_write
            .push("/sandbox/workspace/rust/main.rs".to_owned());
        let fs_err = attempt
            .hot_reload_network_policy(
                &fs_mutated,
                &seat_policy,
                &temp.join(".roster/openshell/policy.yml"),
            )
            .err()
            .ok_or_else(|| Error::type_error("Expected hot_reload_network_policy to fail"))?;
        assert_eq!(
            fs_err.code,
            Some("ROSTER_OPENSHELL_IMMUTABLE_CREATE_POLICY")
        );

        let mut net_narrowed = seat_policy.clone();
        net_narrowed.network_rules[0].methods = vec!["POST".to_owned()];
        let reload_res = attempt.hot_reload_network_policy(
            &net_narrowed,
            &seat_policy,
            &temp.join(".roster/openshell/policy.yml"),
        )?;
        assert!(reload_res.get("policySetArgs").is_some());

        let approval = OpenShellDispatchApproval {
            runtime_executable: fake_runtime,
            policy_path: temp.join(".roster/openshell/policy.yml"),
            gateway_url: "https://127.0.0.1:17670".to_owned(),
            sandbox_name: attempt.sandbox_name.clone(),
            image: "nvcr.io/nvidia/base/ubuntu:24.04".to_owned(),
            policy: attempt.policy.clone(),
            provider: provider.clone(),
            command: vec!["/usr/local/bin/roster".to_owned(), "--help".to_owned()],
            timeout_ms: 15_000,
        };
        let mut live_steps = Vec::new();
        let live_out = attempt
            .execute_live(&approval, &token, |args, env_val| {
                assert_eq!(
                    env_val.get("OPENSHELL_TELEMETRY_ENABLED"),
                    Some(&Value::string("false"))
                );
                assert_eq!(
                    env_val.get("APERTURE_API_KEY"),
                    Some(&Value::string("OPENSHELL_SUPERVISOR_PLACEHOLDER"))
                );
                assert!(env_val.get("GITHUB_APP_ID").is_none());
                assert!(env_val.get("GITHUB_APP_PRIVATE_KEY_PATH").is_none());
                live_steps.push(args.join(" "));
                async move { Ok((0, "live ok\n".to_owned(), String::new())) }
            })
            .await?;
        assert_eq!(live_out.get("exitCode"), Some(&Value::Number(0.0)));
        assert_eq!(
            live_out.get("providerDetached"),
            Some(&Value::Boolean(true))
        );
        assert_eq!(live_out.get("sandboxDeleted"), Some(&Value::Boolean(true)));
        assert_eq!(live_steps.len(), 5);
        assert!(live_steps[1].contains("sandbox provider attach"));
        assert!(live_steps[3].contains("sandbox provider detach"));
        assert!(!live_steps[4].contains("--wait"));

        let native_proof = prove_openshell_policy_with_external_prover(
            &seat_policy,
            &seat_policy,
            None,
            &temp,
            Some(&token),
        )
        .await?;
        assert_eq!(native_proof.get("verified"), Some(&Value::Boolean(true)));
        assert_eq!(
            native_proof.get("proverMode"),
            Some(&Value::string("native-only"))
        );

        let real_prover = Path::new("/usr/bin/openshell-prover");
        if real_prover.is_file() {
            let ext_proof = prove_openshell_policy_with_external_prover(
                &seat_policy,
                &seat_policy,
                Some(real_prover),
                &temp,
                Some(&token),
            )
            .await?;
            assert_eq!(ext_proof.get("verified"), Some(&Value::Boolean(true)));
            assert_eq!(
                ext_proof.get("proverMode"),
                Some(&Value::string("native+external"))
            );
            assert_eq!(
                ext_proof.get("externalVerified"),
                Some(&Value::Boolean(true))
            );
        }

        let _ = fs::remove_dir_all(&temp);
        Ok(())
    }
}
