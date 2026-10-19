use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::*;
use crate::identity::sha256_hex;
use crate::state::{files, secure};

const OUTPUT_LIMIT: usize = 65_536;
const MEMORY: u64 = 268_435_456;
const TMPFS: &str = "rw,noexec,nosuid,size=16m,mode=1777";
const OWNER: &str = "io.github-agent-roster.owner";
const SUBJECT: &str = "io.github-agent-roster.subject";
const PLAN: &str = "io.github-agent-roster.plan";

#[derive(Debug, Clone)]
pub enum Endpoint {
    WindowsPipe,
    UnixSocket(PathBuf),
}

#[derive(Debug, Clone)]
pub enum Runtime {
    Native { entrypoint: String },
    Node20,
}

#[derive(Debug, Clone)]
pub enum Script {
    Product(String),
    Verifier(String),
}

#[derive(Debug, Clone)]
pub enum Action {
    Native(Vec<String>),
    Node { script: Script, args: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct Observation {
    pub check_id: u64,
    pub action: Action,
    pub expected_exit: i32,
    pub expected_stdout: String,
    pub expected_stderr: String,
    pub evidence: String,
}

#[derive(Debug, Clone)]
pub struct Verifier {
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct Approval {
    pub enabled: bool,
    pub executable: PathBuf,
    pub endpoint: Endpoint,
    pub image: String,
    pub runtime: Runtime,
    pub files: Vec<String>,
    pub dependencies: Vec<String>,
    pub verifiers: Vec<Verifier>,
    pub observations: Vec<Observation>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub enum Selection {
    #[default]
    None,
    Docker(Option<Box<Approval>>),
}

pub struct Report {
    stamp: backend::Stamp,
    subject: Option<Value>,
    files: Vec<String>,
    candidate: Option<Vec<u8>>,
    result: Option<secure::Snapshot>,
    value: Value,
    pass: bool,
}

impl Report {
    pub fn as_value(&self) -> &Value {
        &self.value
    }
    pub fn passed(&self) -> bool {
        self.pass
    }
    pub async fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.require_phase(boundary, false).await
    }
    pub(crate) async fn require_staged_current(
        &self,
        boundary: &tools::Boundary,
    ) -> Result<(), Error> {
        self.require_phase(boundary, true).await
    }
    async fn require_phase(&self, boundary: &tools::Boundary, staged: bool) -> Result<(), Error> {
        self.stamp
            .require_current(backend::Owner::Execution(boundary))
            .await?;
        if let Some(expected) = &self.result {
            if !secure::unchanged(&boundary.root().join("RESULT.md"), expected, 65_536)? {
                return Err(denied(
                    "Independent verification RESULT identity or bytes changed",
                ));
            }
        }
        if let Some(subject) = &self.subject {
            let current = candidate_result(boundary, self.candidate.as_deref())?;
            if self.candidate.is_some() && !staged && current.is_none() {
                return Err(denied(
                    "Independent candidate RESULT must be written unchanged before current receipt use",
                ));
            }
            if verification::capture_subject_result(
                boundary.root(),
                &self.files,
                self.candidate.as_deref(),
            )? != *subject
            {
                return Err(denied(
                    "Independent verification current subject changed; receipt is stale",
                ));
            }
        }
        boundary.check()
    }
}

fn unavailable(message: impl Into<String>) -> Error {
    Error::new("DockerUnavailable", message).coded("ROSTER_DOCKER_UNAVAILABLE")
}

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn output(stdout: &str, stderr: &str) -> String {
    Value::object(vec![
        ("stdout", Value::string(stdout)),
        ("stderr", Value::string(stderr)),
    ])
    .json()
}

fn argv(values: &[String]) -> Result<(), Error> {
    if values.len() > 16
        || values
            .iter()
            .any(|value| value.encode_utf16().count() > 1000 || value.contains(['\0', '\r', '\n']))
    {
        return Err(Error::type_error(
            "Docker observations require at most 16 bounded single-line arguments",
        ));
    }
    Ok(())
}

impl Runtime {
    fn entrypoint(&self) -> &str {
        match self {
            Self::Native { entrypoint } => entrypoint,
            Self::Node20 => "node",
        }
    }
}

impl Approval {
    pub fn validate(&self) -> Result<(), Error> {
        if !self.executable.is_absolute()
            || self.executable.to_str().is_none()
            || self.timeout_ms == 0
            || self.timeout_ms > 30_000
            || !self.image.strip_prefix("sha256:").is_some_and(hash)
        {
            return Err(Error::type_error(
                "Docker verification requires an absolute approved executable, immutable local image ID and 1-30000 ms deadline",
            ));
        }
        if let Endpoint::UnixSocket(path) = &self.endpoint {
            if !path.is_absolute()
                || path
                    .to_str()
                    .is_none_or(|value| value.contains(['\0', '\r', '\n']))
            {
                return Err(denied(
                    "Docker endpoint must be an explicitly approved local socket",
                ));
            }
        }
        if let Runtime::Native { entrypoint } = &self.runtime {
            let name = entrypoint.rsplit('/').next().unwrap_or("");
            if !entrypoint.starts_with('/')
                || entrypoint.len() > 500
                || entrypoint
                    .split('/')
                    .skip(1)
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || !regex(r"^/[A-Za-z0-9_./-]+$")?.is_match(entrypoint)
                || [
                    "sh",
                    "bash",
                    "dash",
                    "zsh",
                    "cmd",
                    "powershell",
                    "pwsh",
                    "node",
                ]
                .contains(&name)
            {
                return Err(denied(
                    "Native Docker verification requires a fixed non-shell executable inside the approved image",
                ));
            }
        }
        if self.files.is_empty()
            || self.files.len() + self.dependencies.len() > MAX_SCOPE_ENTRIES
            || self.observations.is_empty()
            || self.observations.len() > 16
            || self.verifiers.len() > 16
        {
            return Err(Error::type_error(
                "Docker verification requires 1-64 exact inputs, 1-16 observations and at most 16 host verifiers",
            ));
        }
        let mut names = BTreeSet::new();
        for path in self.files.iter().chain(&self.dependencies) {
            if path.encode_utf16().count() > 500
                || !policy::exact_path(path)
                || policy::managed(path)
                || crate::planning::forbidden_read(path)
                || !names.insert(if cfg!(windows) {
                    path.to_lowercase()
                } else {
                    path.clone()
                })
            {
                return Err(denied(
                    "Docker inputs must be unique exact readable product or dependency paths",
                ));
            }
        }
        let mut verifiers = BTreeSet::new();
        let mut bytes = 0_usize;
        for verifier in &self.verifiers {
            if !regex(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}\.mjs$")?.is_match(&verifier.name)
                || !verifiers.insert(verifier.name.clone())
                || verifier.source.contains('\0')
                || verifier.source.len() > 65_536
            {
                return Err(Error::type_error(
                    "Host verifier sources require unique simple .mjs names and at most 64 KiB each",
                ));
            }
            bytes = bytes
                .checked_add(verifier.source.len())
                .filter(|bytes| *bytes <= 262_144)
                .ok_or_else(|| denied("Host verifier sources exceed 256 KiB"))?;
        }
        for observation in &self.observations {
            if !(1..=16).contains(&observation.check_id)
                || !(0..=124).contains(&observation.expected_exit)
                || observation.expected_stdout.encode_utf16().count()
                    + observation.expected_stderr.encode_utf16().count()
                    > 1500
                || output(&observation.expected_stdout, &observation.expected_stderr)
                    .encode_utf16()
                    .count()
                    > 2000
                || observation.evidence.is_empty()
                || observation.evidence.encode_utf16().count() > 500
                || observation.evidence.trim() != observation.evidence
                || observation
                    .evidence
                    .chars()
                    .any(|value| value <= '\u{1f}' || value == '\u{7f}')
            {
                return Err(Error::type_error(
                    "Docker observations require valid check IDs, ordinary exits, bounded exact outputs and evidence",
                ));
            }
            match (&self.runtime, &observation.action) {
                (Runtime::Native { .. }, Action::Native(args)) => argv(args)?,
                (Runtime::Node20, Action::Node { script, args }) => {
                    argv(args)?;
                    match script {
                        Script::Product(path)
                            if self.files.contains(path)
                                && regex(r"\.[cm]?js$")?.is_match(path) => {}
                        Script::Verifier(name) if verifiers.contains(name) => {}
                        _ => {
                            return Err(denied(
                                "Node observations must use an explicitly snapshotted product or host verifier script",
                            ));
                        }
                    }
                }
                _ => {
                    return Err(denied(
                        "Docker observation action does not match its explicitly approved runtime",
                    ));
                }
            }
        }
        Ok(())
    }

    fn endpoint(&self) -> Result<String, Error> {
        match &self.endpoint {
            Endpoint::WindowsPipe if cfg!(windows) => {
                Ok("npipe:////./pipe/docker_engine".to_owned())
            }
            Endpoint::UnixSocket(path) if cfg!(unix) => Ok(format!(
                "unix://{}",
                path.to_str()
                    .ok_or_else(|| denied("Docker socket must be Unicode"))?
            )),
            _ => Err(unavailable(
                "Approved local Docker endpoint is not supported on this platform",
            )),
        }
    }

    fn plan(&self) -> Value {
        Value::object(vec![
            ("schema_version", Value::Number(1.0)),
            ("docker", Value::string(&self.executable.to_string_lossy())),
            (
                "endpoint",
                Value::string(&match &self.endpoint {
                    Endpoint::WindowsPipe => "npipe:////./pipe/docker_engine".to_owned(),
                    Endpoint::UnixSocket(path) => format!("unix://{}", path.to_string_lossy()),
                }),
            ),
            ("timeout_ms", Value::Number(self.timeout_ms as f64)),
            ("image", Value::string(&self.image)),
            ("entrypoint", Value::string(self.runtime.entrypoint())),
            ("files", strings(&self.files)),
            ("dependencies", strings(&self.dependencies)),
            (
                "observations",
                Value::Array(
                    self.observations
                        .iter()
                        .enumerate()
                        .map(|(index, observation)| {
                            Value::object(vec![
                                ("id", Value::Number((index + 1) as f64)),
                                ("check_id", Value::Number(observation.check_id as f64)),
                                ("args", strings(&action_args(&observation.action))),
                                (
                                    "expected_exit",
                                    Value::Number(f64::from(observation.expected_exit)),
                                ),
                                (
                                    "expected_output",
                                    Value::string(&output(
                                        &observation.expected_stdout,
                                        &observation.expected_stderr,
                                    )),
                                ),
                                ("evidence", Value::string(&observation.evidence)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "verifiers",
                Value::Array(
                    self.verifiers
                        .iter()
                        .map(|verifier| {
                            Value::object(vec![
                                ("name", Value::string(&verifier.name)),
                                (
                                    "digest",
                                    Value::string(&sha256_hex(verifier.source.as_bytes())),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

fn action_args(action: &Action) -> Vec<String> {
    match action {
        Action::Native(args) => args.clone(),
        Action::Node { script, args } => {
            let mut values = vec![
                "--".to_owned(),
                match script {
                    Script::Product(path) => format!("/product/{path}"),
                    Script::Verifier(name) => format!("/verifier/{name}"),
                },
            ];
            values.extend(args.clone());
            values
        }
    }
}

fn candidate_result(
    boundary: &tools::Boundary,
    candidate: Option<&[u8]>,
) -> Result<Option<secure::Snapshot>, Error> {
    let path = boundary.root().join("RESULT.md");
    let current = secure::read(&path, 65_536)?;
    if current.is_some() && crate::paths::file_information(&path)?.links != 1 {
        return Err(denied("Independent RESULT must be single-link"));
    }
    if let Some(candidate) = candidate {
        if current
            .as_ref()
            .is_some_and(|snapshot| snapshot.bytes != candidate)
        {
            return Err(denied(
                "Existing RESULT differs from this current native candidate; archive it explicitly before verification",
            ));
        }
    }
    Ok(current)
}

fn strings_at(value: Option<&Value>) -> Result<Vec<String>, Error> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => domain::items(value)
            .ok_or_else(|| unavailable("Docker inspect array is malformed"))?
            .iter()
            .map(|value| text(value, "Docker inspect array must contain text"))
            .collect(),
    }
}

fn empty(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null))
        || value
            .and_then(domain::items)
            .is_some_and(|items| items.is_empty())
}

fn environment(values: &[String]) -> Result<BTreeMap<String, String>, Error> {
    let allowed = [
        "PATH",
        "HOME",
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "LANG",
        "LC_ALL",
        "NO_COLOR",
        "CI",
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "RUSTUP_HOME",
        "CARGO_NET_OFFLINE",
        "RUSTUP_AUTO_INSTALL",
        "NODE_VERSION",
        "YARN_VERSION",
        "RUST_VERSION",
    ];
    let mut result = BTreeMap::new();
    for value in values {
        let (name, value) = value
            .split_once('=')
            .ok_or_else(|| unavailable("Docker image environment is malformed"))?;
        if !allowed.contains(&name)
            || value.len() > 2000
            || value.contains(['\0', '\r', '\n'])
            || result.insert(name.to_owned(), value.to_owned()).is_some()
        {
            return Err(unavailable(
                "Docker image/container has unapproved ambient environment",
            ));
        }
    }
    Ok(result)
}

fn overrides() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HOME".to_owned(), "/tmp/home".to_owned()),
        ("TMPDIR".to_owned(), "/tmp".to_owned()),
        ("XDG_CONFIG_HOME".to_owned(), "/tmp/config".to_owned()),
        ("XDG_DATA_HOME".to_owned(), "/tmp/data".to_owned()),
        ("NO_COLOR".to_owned(), "1".to_owned()),
        ("CI".to_owned(), "1".to_owned()),
        ("CARGO_TARGET_DIR".to_owned(), "/tmp/target".to_owned()),
        ("CARGO_NET_OFFLINE".to_owned(), "true".to_owned()),
        ("RUSTUP_AUTO_INSTALL".to_owned(), "0".to_owned()),
    ])
}

fn inspect_json(run: &backend::Run) -> Result<Value, Error> {
    if !run.success() {
        return Err(unavailable(
            "Docker daemon/image/container inspection did not complete successfully",
        ));
    }
    crate::json::parse(&run.stdout_text()?)
        .map_err(|cause| unavailable("Docker inspection returned malformed JSON").caused_by(cause))
}

fn make_readable(root: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut directories = vec![root.to_path_buf()];
        while let Some(directory) = directories.pop() {
            secure::check_components(&directory)?;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).map_err(
                |error| secure::io_error("Set readonly Docker snapshot access", &directory, error),
            )?;
            for entry in fs::read_dir(&directory)
                .map_err(|error| secure::io_error("Read Docker snapshot", &directory, error))?
            {
                let entry = entry.map_err(|error| {
                    secure::io_error("Read Docker snapshot entry", &directory, error)
                })?;
                let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                    secure::io_error("Inspect Docker snapshot entry", &entry.path(), error)
                })?;
                if secure::linked(&metadata) {
                    return Err(denied("Readonly Docker snapshots may not contain links"));
                }
                if metadata.is_dir() {
                    directories.push(entry.path());
                } else {
                    fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o444)).map_err(
                        |error| {
                            secure::io_error(
                                "Set readonly Docker snapshot file",
                                &entry.path(),
                                error,
                            )
                        },
                    )?;
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = root;
    }
    Ok(())
}

struct Executable {
    identity: crate::paths::FileIdentity,
    size: u64,
    modified: SystemTime,
}

impl Executable {
    fn capture(path: &Path) -> Result<Self, Error> {
        let identity = crate::paths::file_information(path)?.identity;
        let metadata = fs::metadata(path)
            .map_err(|error| secure::io_error("Inspect approved Docker executable", path, error))?;
        if !metadata.is_file() {
            return Err(unavailable(
                "Approved Docker executable is not a regular file",
            ));
        }
        Ok(Self {
            identity,
            size: metadata.len(),
            modified: metadata.modified().map_err(|error| {
                secure::io_error("Inspect Docker executable timestamp", path, error)
            })?,
        })
    }
    fn check(&self, path: &Path) -> Result<(), Error> {
        let current = Self::capture(path)?;
        if current.identity != self.identity
            || current.size != self.size
            || current.modified != self.modified
        {
            return Err(unavailable(
                "Approved Docker executable changed between operations; no replacement command is authorized",
            ));
        }
        Ok(())
    }
}

struct Controller<'a> {
    owner: backend::Owner<'a>,
    approval: &'a Approval,
    root: &'a Path,
    prefix: Vec<String>,
    host_environment: BTreeMap<String, String>,
    expected_environment: BTreeMap<String, String>,
    product: PathBuf,
    verifier: Option<PathBuf>,
    subject: String,
    plan: String,
    started: Instant,
    executable: Executable,
    submissions: &'a RefCell<Vec<Value>>,
}

impl Controller<'_> {
    fn remaining(&self) -> Result<u64, Error> {
        let limit = Duration::from_millis(self.approval.timeout_ms);
        let remaining = limit
            .checked_sub(self.started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| unavailable("Docker verification deadline expired"))?;
        Ok(u64::try_from(remaining.as_millis()).unwrap_or(0).max(1))
    }
    async fn command(
        &self,
        args: &[String],
        cleanup: bool,
        started: Instant,
    ) -> Result<backend::Run, Error> {
        self.command_inner(args, cleanup, started, None).await
    }

    async fn command_inner(
        &self,
        args: &[String],
        cleanup: bool,
        started: Instant,
        submission: Option<&super::owned::Submission>,
    ) -> Result<backend::Run, Error> {
        self.executable.check(&self.approval.executable)?;
        let mut command = self.prefix.clone();
        command.extend_from_slice(args);
        if cleanup {
            let remaining = Duration::from_secs(10)
                .checked_sub(started.elapsed())
                .filter(|remaining| !remaining.is_zero())
                .ok_or_else(|| unavailable("Docker cleanup deadline expired"))?;
            self.owner
                .cleanup_run(
                    self.owner.root(),
                    &self.approval.executable,
                    &command,
                    &self.host_environment,
                    u64::try_from(remaining.as_millis()).unwrap_or(0).max(1),
                    OUTPUT_LIMIT,
                )
                .await
        } else {
            let timeout = self.remaining()?;
            if let Some(submission) = submission {
                self.owner
                    .run_tracked(
                        self.owner.root(),
                        &self.approval.executable,
                        &command,
                        &self.host_environment,
                        crate::process::Limits {
                            timeout: Some(Duration::from_millis(timeout)),
                            max_output_bytes: OUTPUT_LIMIT,
                        },
                        submission,
                    )
                    .await
            } else {
                self.owner
                    .run(
                        self.owner.root(),
                        &self.approval.executable,
                        &command,
                        &self.host_environment,
                        timeout,
                        OUTPUT_LIMIT,
                    )
                    .await
            }
        }
    }
    async fn inspect(
        &self,
        name: &str,
        cleanup: bool,
        started: Instant,
    ) -> Result<backend::Run, Error> {
        self.command(
            &[
                "container".to_owned(),
                "inspect".to_owned(),
                "--format".to_owned(),
                "{{json .}}".to_owned(),
                name.to_owned(),
            ],
            cleanup,
            started,
        )
        .await
    }
    fn identity(&self, value: &Value, container: &Container) -> Result<String, Error> {
        let id = text(
            required(value, "Id")?,
            "Docker container requires a full ID",
        )?;
        let labels = value.get("Config").and_then(|config| config.get("Labels"));
        if !hash(&id)
            || container
                .id
                .as_ref()
                .is_some_and(|expected| expected != &id)
            || !domain::is_text(value.get("Name"), &format!("/{}", container.name))
            || !domain::is_text(
                labels.and_then(|labels| labels.get(OWNER)),
                &container.owner,
            )
            || !domain::is_text(labels.and_then(|labels| labels.get(SUBJECT)), &self.subject)
            || !domain::is_text(labels.and_then(|labels| labels.get(PLAN)), &self.plan)
        {
            return Err(unavailable(
                "Docker container ownership ID/name/labels do not match the exact approved intent",
            ));
        }
        Ok(id)
    }
    fn configuration(&self, value: &Value, args: &[String]) -> Result<(), Error> {
        let config = required(value, "Config")?;
        let host = required(value, "HostConfig")?;
        if !domain::is_text(value.get("Image"), &self.approval.image)
            || !domain::is_text(config.get("Image"), &self.approval.image)
            || !domain::is_text(config.get("User"), "65534:65534")
            || !domain::is_text(config.get("WorkingDir"), "/product")
            || strings_at(config.get("Entrypoint"))?
                != [self.approval.runtime.entrypoint().to_owned()]
            || strings_at(config.get("Cmd"))? != args
            || environment(&strings_at(config.get("Env"))?)? != self.expected_environment
            || config.get("Tty") != Some(&Value::Boolean(false))
            || config.get("OpenStdin") != Some(&Value::Boolean(false))
            || !domain::is_text(host.get("NetworkMode"), "none")
            || host.get("ReadonlyRootfs") != Some(&Value::Boolean(true))
            || host.get("Privileged") != Some(&Value::Boolean(false))
            || host.get("AutoRemove") != Some(&Value::Boolean(false))
            || strings_at(host.get("CapDrop"))? != ["ALL".to_owned()]
            || !empty(host.get("CapAdd"))
            || !matches!(strings_at(host.get("SecurityOpt"))?.as_slice(), [value] if ["no-new-privileges", "no-new-privileges:true"].contains(&value.as_str()))
            || domain::safe_integer(host.get("Memory")) != Some(MEMORY)
            || domain::safe_integer(host.get("MemorySwap")) != Some(MEMORY)
            || domain::safe_integer(host.get("NanoCpus")) != Some(1_000_000_000)
            || domain::safe_integer(host.get("PidsLimit")) != Some(32)
            || !domain::is_text(host.get("IpcMode"), "none")
            || !domain::is_text(host.get("PidMode"), "")
            || !domain::is_text(host.get("UTSMode"), "")
            || !domain::is_text(host.get("UsernsMode"), "")
            || !domain::is_text(host.get("CgroupnsMode"), "private")
            || !domain::is_text(
                host.get("RestartPolicy")
                    .and_then(|policy| policy.get("Name")),
                "no",
            )
            || !domain::is_text(
                host.get("LogConfig").and_then(|config| config.get("Type")),
                "none",
            )
            || [
                "CapAdd",
                "Devices",
                "DeviceRequests",
                "VolumesFrom",
                "Binds",
            ]
            .iter()
            .any(|name| host.get(name).is_none())
            || !empty(host.get("Devices"))
            || !empty(host.get("DeviceRequests"))
            || !empty(host.get("VolumesFrom"))
            || !empty(host.get("Binds"))
            || !host.get("Tmpfs").is_some_and(|tmpfs| {
                domain::members(tmpfs).is_some_and(|entries| entries.len() == 1)
                    && domain::is_text(tmpfs.get("/tmp"), TMPFS)
            })
        {
            return Err(unavailable(
                "Docker container isolation, environment, resource flags or exact command differ from the approved plan",
            ));
        }
        let limits = host
            .get("Ulimits")
            .and_then(domain::items)
            .ok_or_else(|| unavailable("Docker ulimit evidence is missing"))?;
        if limits.len() != 1
            || limits.first().is_none_or(|limit| {
                !domain::is_text(limit.get("Name"), "nofile")
                    || domain::safe_integer(limit.get("Soft")) != Some(256)
                    || domain::safe_integer(limit.get("Hard")) != Some(256)
            })
        {
            return Err(unavailable("Docker nofile limit differs from approval"));
        }
        let mounts = value
            .get("Mounts")
            .and_then(domain::items)
            .ok_or_else(|| unavailable("Docker mount evidence is missing"))?;
        let mut expected = BTreeMap::from([(
            "/product".to_owned(),
            self.product.to_string_lossy().into_owned(),
        )]);
        if let Some(verifier) = &self.verifier {
            expected.insert(
                "/verifier".to_owned(),
                verifier.to_string_lossy().into_owned(),
            );
        }
        if mounts.len() != expected.len() {
            return Err(unavailable("Docker has unexpected mounts"));
        }
        let mut seen = BTreeSet::new();
        for mount in mounts {
            let destination = text(
                required(mount, "Destination")?,
                "Docker mount destination is missing",
            )?;
            let source = text(required(mount, "Source")?, "Docker mount source is missing")?;
            let expected = expected
                .get(&destination)
                .ok_or_else(|| unavailable("Docker has an unapproved mount destination"))?;
            let equal = if cfg!(windows) {
                source
                    .replace('\\', "/")
                    .eq_ignore_ascii_case(&expected.replace('\\', "/"))
            } else {
                &source == expected
            };
            if !seen.insert(destination)
                || !equal
                || !domain::is_text(mount.get("Type"), "bind")
                || mount.get("RW") != Some(&Value::Boolean(false))
            {
                return Err(unavailable(
                    "Docker snapshot mount identity or readonly mode differs from approval",
                ));
            }
        }
        Ok(())
    }
    fn missing(&self, run: &backend::Run, name: &str) -> Result<bool, Error> {
        if run.exit != Some(1)
            || run.timed_out
            || run.overflow
            || !run.stdout.iter().all(u8::is_ascii_whitespace)
        {
            return Ok(false);
        }
        let error = tools::bytes_text(&run.stderr)?;
        Ok(error.lines().count() == 1
            && [
                format!("Error: No such object: {name}"),
                format!("Error: No such container: {name}"),
                format!("Error response from daemon: No such container: {name}"),
            ]
            .contains(&error.trim().to_owned()))
    }
    async fn cleanup(&self, container: &Container) -> Result<(), Error> {
        if !container.submission.attempted() {
            return Ok(());
        }
        let started = Instant::now();
        let lookup = container.id.as_deref().unwrap_or(&container.name);
        let observed = self.inspect(lookup, true, started).await?;
        if self.missing(&observed, lookup)? && container.id.is_some() && container.confirmed {
            return Ok(());
        }
        let value = inspect_json(&observed)?;
        let id = self.identity(&value, container)?;
        let removed = self
            .command(
                &[
                    "container".to_owned(),
                    "rm".to_owned(),
                    "--force".to_owned(),
                    id.clone(),
                ],
                true,
                started,
            )
            .await?;
        if !removed.success() {
            return Err(unavailable("Owned Docker container removal failed"));
        }
        let absent = self.inspect(&id, true, started).await?;
        if !self.missing(&absent, &id)? {
            return Err(unavailable(
                "Owned Docker container absence could not be independently confirmed",
            ));
        }
        Ok(())
    }
    async fn execute(&self, args: &[String], position: usize) -> Result<backend::Run, Error> {
        self.owner.check()?;
        if self.submissions.borrow().len() >= 17 {
            return Err(unavailable(
                "Docker native submission receipt bound exceeded",
            ));
        }
        let nonce = files::nonce()?;
        let name = format!("roster-verify-{}-{nonce}", std::process::id());
        let directory = self.root.join(format!("container-{position}"));
        files::create_private_directories(&directory)?;
        let cidfile = directory.join("cid");
        let mut container = Container {
            name,
            owner: nonce,
            id: None,
            submission: super::owned::Submission::default(),
            confirmed: false,
        };
        let intent = Value::object(vec![
            ("name", Value::string(&container.name)),
            ("owner", Value::string(&container.owner)),
            ("image", Value::string(&self.approval.image)),
            ("subject", Value::string(&self.subject)),
            ("plan", Value::string(&self.plan)),
        ]);
        archive::write_owned(&directory.join("intent.json"), intent.json().as_bytes())?;
        let outcome = async {
            let mut create = vec!["container".to_owned(), "create".to_owned(), "--name".to_owned(), container.name.clone(), "--cidfile".to_owned(), cidfile.to_string_lossy().into_owned()];
            for (key, value) in [(OWNER, &container.owner), (SUBJECT, &self.subject), (PLAN, &self.plan)] {create.extend(["--label".to_owned(), format!("{key}={value}")]);}
            create.extend(["--pull", "never", "--network", "none", "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--user", "65534:65534", "--memory", "256m", "--memory-swap", "256m", "--cpus", "1", "--pids-limit", "32", "--ulimit", "nofile=256:256", "--ipc", "none", "--cgroupns", "private", "--restart", "no", "--log-driver", "none", "--tmpfs", "/tmp:rw,noexec,nosuid,size=16m,mode=1777"].map(str::to_owned));
            for (key, value) in overrides() {create.extend(["--env".to_owned(), format!("{key}={value}")]);}
            for (path, target) in [Some((&self.product, "/product")), self.verifier.as_ref().map(|path| (path, "/verifier"))].into_iter().flatten() {
                let source = path.to_str().ok_or_else(|| unavailable("Docker snapshot mount must be Unicode"))?;
                if source.contains([',', '"', '\r', '\n']) {return Err(unavailable("Docker snapshot path contains unsupported mount option separators"));}
                create.extend(["--mount".to_owned(), format!("type=bind,source={source},target={target},readonly")]);
            }
            create.extend(["--workdir".to_owned(), "/product".to_owned(), "--entrypoint".to_owned(), self.approval.runtime.entrypoint().to_owned(), self.approval.image.clone()]);
            create.extend_from_slice(args);
            self.owner.check()?;
            let created = self.command_inner(&create, false, self.started, Some(&container.submission)).await?;
            let file = secure::read(&cidfile, 128)?.ok_or_else(|| unavailable("Docker creation did not establish an owned CID file"))?;
            let id = tools::bytes_text(&file.bytes)?.trim().to_owned();
            if crate::paths::file_information(&cidfile)?.links != 1 || !hash(&id) || !secure::unchanged(&cidfile, &file, 128)? {return Err(unavailable("Docker CID file is malformed, linked or changed"));}
            container.id = Some(id.clone());
            if !created.success() || tools::bytes_text(&created.stdout)?.trim() != id {return Err(unavailable("Docker create output does not match its exact owned container ID"));}
            let value = inspect_json(&self.inspect(&id, false, self.started).await?)?;
            self.identity(&value, &container)?;
            container.confirmed = true;
            self.configuration(&value, args)?;
            if value.get("State").and_then(|state| state.get("Running")) != Some(&Value::Boolean(false)) || !domain::is_text(value.get("State").and_then(|state| state.get("Status")), "created") {return Err(unavailable("Docker container ran before approved inspection"));}
            let attached = self.command(&["container".to_owned(), "start".to_owned(), "--attach".to_owned(), id.clone()], false, self.started).await?;
            let value = inspect_json(&self.inspect(&id, false, self.started).await?)?;
            self.identity(&value, &container)?;
            self.configuration(&value, args)?;
            let state = required(&value, "State")?;
            let exit = domain::safe_integer(state.get("ExitCode")).filter(|exit| *exit <= 124).ok_or_else(|| unavailable("Docker exit is unavailable, signalled or infrastructure-specific"))? as i32;
            if state.get("Running") != Some(&Value::Boolean(false)) || !domain::is_text(state.get("Status"), "exited")
                || state.get("OOMKilled") != Some(&Value::Boolean(false)) || state.get("Dead") != Some(&Value::Boolean(false))
                || state.get("Error").and_then(Value::as_string).is_none_or(|value| !value.0.is_empty())
                || attached.timed_out || attached.overflow || ![Some(0), Some(exit)].contains(&attached.exit) {
                return Err(unavailable("Docker execution did not produce a complete ordinary stopped-container observation"));
            }
            self.remaining()?;
            Ok(backend::Run {exit: Some(exit), stdout: attached.stdout, stderr: attached.stderr, timed_out: false, overflow: false})
        }.await;
        let before_cleanup = self.started.elapsed();
        let cleanup_started = Instant::now();
        let cleanup = self.cleanup(&container).await.map_err(|cause| {
            Error::new(
                "OwnedDockerCleanupError",
                format!(
                    "Container {} cleanup is unverified; retain writer and snapshot evidence",
                    container.name
                ),
            )
            .coded("ROSTER_PROCESS_CLEANUP")
            .caused_by(cause)
        });
        let mut submission = container.submission.snapshot(self.started);
        domain::put(
            &mut submission,
            "before_cleanup_elapsed_ms",
            Value::Number(before_cleanup.as_secs_f64() * 1000.0),
        )?;
        domain::put(
            &mut submission,
            "cleanup_elapsed_ms",
            Value::Number(cleanup_started.elapsed().as_secs_f64() * 1000.0),
        )?;
        domain::put(
            &mut submission,
            "verification_timeout_ms",
            Value::Number(self.approval.timeout_ms as f64),
        )?;
        self.submissions.borrow_mut().push(submission.clone());
        let recorded = self.owner.cleanup_check().and_then(|()| {
            archive::write_owned(
                &directory.join("submission.json"),
                format!("{}\n", submission.json()).as_bytes(),
            )
            .map(|_| ())
        });
        backend::finish_owned(
            self.owner,
            crate::state::locks::finish(outcome, recorded),
            cleanup,
        )
    }
}

struct Container {
    name: String,
    owner: String,
    id: Option<String>,
    submission: super::owned::Submission,
    confirmed: bool,
}

fn report_value(
    status: &str,
    reasons: Vec<Value>,
    receipt: Value,
    subject: Value,
    approval: Option<&Approval>,
) -> Value {
    Value::object(vec![
        ("status", Value::string(status)),
        ("reasons", Value::Array(reasons)),
        ("receipt", receipt),
        ("subject", subject),
        (
            "image",
            approval.map_or(Value::Null, |approval| Value::string(&approval.image)),
        ),
    ])
}

pub async fn run(boundary: &tools::Boundary, selection: &Selection) -> Result<Report, Error> {
    run_candidate(boundary, selection, None).await
}

pub(crate) async fn run_candidate(
    boundary: &tools::Boundary,
    selection: &Selection,
    candidate: Option<&[u8]>,
) -> Result<Report, Error> {
    let owner = backend::Owner::Execution(boundary);
    let stamp = backend::Stamp::capture(owner).await?;
    let approval = match selection {
        Selection::None => {
            return Ok(Report {
                stamp,
                subject: None,
                files: Vec::new(),
                candidate: None,
                result: None,
                value: report_value("none", Vec::new(), Value::Null, Value::Null, None),
                pass: true,
            });
        }
        Selection::Docker(None) => {
            return Ok(Report {
                stamp,
                subject: None,
                files: Vec::new(),
                candidate: None,
                result: None,
                value: report_value(
                    "unavailable",
                    vec![Value::string(
                        "Selected Docker verification has no explicit host approval",
                    )],
                    Value::Null,
                    Value::Null,
                    None,
                ),
                pass: false,
            });
        }
        Selection::Docker(Some(approval)) => approval.as_ref(),
    };
    approval.validate()?;
    if !approval.enabled {
        return Ok(Report {
            stamp,
            subject: None,
            files: approval.files.clone(),
            candidate: None,
            result: None,
            value: report_value(
                "unavailable",
                vec![Value::string(
                    "Docker verification is selected but explicit host enablement is absent",
                )],
                Value::Null,
                Value::Null,
                Some(approval),
            ),
            pass: false,
        });
    }
    let count = boundary.document().acceptance_checks.len() as u64;
    if !(1..=16).contains(&count)
        || (1..=count).any(|check| {
            !approval
                .observations
                .iter()
                .any(|observation| observation.check_id == check)
        })
        || approval
            .observations
            .iter()
            .any(|observation| observation.check_id > count)
    {
        return Err(denied(
            "Approved independent observations must cover every authoritative TASK acceptance check",
        ));
    }
    archive::reject_secret(approval.plan().json().as_bytes(), boundary.redaction())?;
    let result = candidate_result(boundary, candidate)?;
    let subject =
        verification::capture_subject_result(boundary.root(), &approval.files, candidate)?;
    let scratch = scratch::Scratch::create(owner).await?;
    let submissions = RefCell::new(Vec::new());
    let outcome = async {
        let product = scratch.directory("product")?;
        let home = scratch.directory("docker-home")?;
        let config = home.join("config");
        files::create_private_directories(&config)?;
        archive::write_owned(&config.join("config.json"), b"{\"auths\":{}}\n")?;
        let mut host_environment = BTreeMap::new();
        for name in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "TEMP", "TMP"] {
            host_environment.insert(name.to_owned(), home.to_string_lossy().into_owned());
        }
        host_environment.insert("DOCKER_CONFIG".to_owned(), config.to_string_lossy().into_owned());
        host_environment.insert("LANG".to_owned(), "C".to_owned());
        host_environment.insert("LC_ALL".to_owned(), "C".to_owned());
        let mut digests = Vec::new();
        let mut total = 0_usize;
        for path in approval.files.iter().chain(&approval.dependencies) {
            owner.check()?;
            let value = verification::file_digest(boundary.root(), path, approval.files.contains(path))?;
            digests.push(Value::object(vec![("path", Value::string(path)), ("digest", value.clone())]));
            if value == Value::Null {continue;}
            let original = boundary.root().join(path);
            let snapshot = secure::read(&original, 65_536)?.ok_or_else(|| denied("Docker input disappeared while snapshotting"))?;
            if crate::paths::file_information(&original)?.links != 1 || value != Value::string(&sha256_hex(&snapshot.bytes)) || !secure::unchanged(&original, &snapshot, 65_536)? {return Err(denied("Docker input changed while snapshotting"));}
            archive::reject_secret(&snapshot.bytes, boundary.redaction())?;
            total = total.checked_add(snapshot.bytes.len()).filter(|total| *total <= 4 * 1024 * 1024).ok_or_else(|| denied("Docker readonly inputs exceed 4 MiB"))?;
            let target = product.join(path);
            files::create_private_directories(target.parent().ok_or_else(|| denied("Docker input needs a parent"))?)?;
            archive::write_owned(&target, &snapshot.bytes)?;
        }
        for (name, candidate) in [("TASK.md", None), ("RESULT.md", candidate)] {
            let bytes = if let Some(bytes) = candidate {bytes.to_vec()} else {secure::read(&boundary.root().join(name), 65_536)?.ok_or_else(|| denied("Independent task/result input disappeared"))?.bytes};
            archive::reject_secret(&bytes, boundary.redaction())?;
            archive::write_owned(&product.join(name), &bytes)?;
        }
        let verifier = if approval.verifiers.is_empty() {None} else {
            let directory = scratch.directory("verifier")?;
            for entry in &approval.verifiers {
                archive::reject_secret(entry.source.as_bytes(), boundary.redaction())?;
                archive::write_owned(&directory.join(&entry.name), entry.source.as_bytes())?;
            }
            make_readable(&directory)?;
            Some(directory)
        };
        make_readable(&product)?;
        let plan_digest = sha256_hex(Value::object(vec![("plan", approval.plan()), ("inputs", Value::Array(digests)), ("subject", subject.clone())]).json().as_bytes());
        let containers = scratch.directory("docker-containers")?;
        let mut controller = Controller {owner, approval, root: &containers, prefix: vec!["--config".to_owned(), config.to_string_lossy().into_owned(), "--host".to_owned(), approval.endpoint()?], host_environment, expected_environment: BTreeMap::new(), product, verifier, subject: text(required(&subject, "digest")?, "Independent subject digest is missing")?, plan: plan_digest, started: Instant::now(), executable: Executable::capture(&approval.executable)?, submissions: &submissions};
        let image = inspect_json(&controller.command(&["image".to_owned(), "inspect".to_owned(), "--format".to_owned(), "{{json .}}".to_owned(), approval.image.clone()], false, controller.started).await?)?;
        if !domain::is_text(image.get("Id"), &approval.image) || !domain::is_text(image.get("Os"), "linux")
            || image.get("Config").and_then(|config| config.get("Volumes")).is_some_and(|volumes| *volumes != Value::Null && domain::members(volumes).is_none_or(|entries| !entries.is_empty())) {
            return Err(unavailable("Docker image is missing, mutable, non-Linux or declares unapproved volumes; no acquisition is allowed"));
        }
        controller.expected_environment = environment(&strings_at(image.get("Config").and_then(|config| config.get("Env")))?)?;
        controller.expected_environment.extend(overrides());
        if let Runtime::Node20 = approval.runtime {
            let version = controller.execute(&["--input-type=commonjs".to_owned(), "-e".to_owned(), "if(process.platform!==\"linux\"||process.versions.node.split(\".\")[0]!==\"20\")process.exit(1);console.log(process.version)".to_owned()], 0).await?;
            if !version.success() || !regex(r"^v20\.[0-9]+\.[0-9]+\r?\n$")?.is_match(&tools::bytes_text(&version.stdout)?) || !version.stderr.is_empty() {return Err(unavailable("Approved Docker image did not actually provide Linux Node 20"));}
        }
        let mut observations = Vec::new();
        for (index, observation) in approval.observations.iter().enumerate() {
            owner.check()?;
            let executed = controller.execute(&action_args(&observation.action), index + 1).await?;
            let stdout = tools::bytes_text(&executed.stdout)?;
            let stderr = tools::bytes_text(&executed.stderr)?;
            if stdout.encode_utf16().count() + stderr.encode_utf16().count() > 1500 || output(&stdout, &stderr).encode_utf16().count() > 2000 {return Err(unavailable("Actual Docker output exceeds bounded receipt limits; no truncated success was assumed"));}
            observations.push(Value::object(vec![
                ("id", Value::Number((index + 1) as f64)),
                ("check_id", Value::Number(observation.check_id as f64)),
                ("expected_exit", Value::Number(f64::from(observation.expected_exit))),
                ("actual_exit", Value::Number(f64::from(executed.exit.ok_or_else(|| unavailable("Docker exit observation is missing"))?))),
                ("expected_output", Value::string(&output(&observation.expected_stdout, &observation.expected_stderr))),
                ("actual_output", Value::string(&output(&stdout, &stderr))),
                ("evidence", Value::string(&observation.evidence)),
            ]));
        }
        candidate_result(boundary, candidate)?;
        if verification::capture_subject_result(boundary.root(), &approval.files, candidate)? != subject {return Err(denied("Independent Docker subject changed during execution"));}
        let receipt = Value::object(vec![("schema_version", Value::Number(1.0)), ("subject_digest", required(&subject, "digest")?.clone()), ("observations", Value::Array(observations))]);
        let verdict = verification::judge_receipt(&receipt, &controller.subject, count)?;
        let pass = domain::is_text(verdict.get("status"), "pass");
        let mut value = report_value(if pass {"pass"} else {"fail"}, verdict.get("reasons").and_then(domain::items).unwrap_or(&[]).to_vec(), receipt, subject.clone(), Some(approval));
        domain::put(&mut value, "planDigest", Value::string(&controller.plan))?;
        let redacted = crate::planning::memory::redact(&Text::from(value.json().as_str()), boundary.redaction());
        let value = crate::json::parse(&redacted)?;
        Ok((value, pass))
    }.await;
    let cleaned = backend::finish_owned(owner, outcome, scratch.close().await);
    let (mut value, pass) = match cleaned {
        Ok(value) => value,
        Err(error)
            if !crate::cancel::is_run_cancelled(&error)
                && error.code != Some("ROSTER_PROCESS_CLEANUP")
                && owner.cleanup_certain() =>
        {
            (
                report_value(
                    "unavailable",
                    vec![Value::string(&error.message)],
                    Value::Null,
                    subject.clone(),
                    Some(approval),
                ),
                false,
            )
        }
        Err(error) => return Err(error),
    };
    domain::put(
        &mut value,
        "native_create_submissions",
        Value::Array(submissions.into_inner()),
    )?;
    stamp.require_current(owner).await?;
    Ok(Report {
        stamp,
        subject: Some(subject),
        files: approval.files.clone(),
        candidate: candidate.map(<[u8]>::to_vec),
        result,
        value,
        pass,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OwnedDirectory(PathBuf);

    impl Drop for OwnedDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn docker_cli_approval_does_not_follow_a_same_content_replacement_file() -> Result<(), Error> {
        let root = OwnedDirectory(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".docker-executable-binding-{}-{}",
            std::process::id(),
            files::nonce()?
        )));
        files::create_private_directories(&root.0)?;
        let program = root.0.join("docker-executable");
        archive::write_owned(&program, b"same executable bytes")?;
        let executable = Executable::capture(&program)?;
        fs::rename(&program, root.0.join("previous"))
            .map_err(|error| secure::io_error("Move owned executable fixture", &program, error))?;
        archive::write_owned(&program, b"same executable bytes")?;
        assert!(executable.check(&program).is_err());
        Ok(())
    }
}

#[cfg(all(test, windows))]
#[path = "docker/admission_tests.rs"]
mod admission_tests;
