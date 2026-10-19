use super::*;
use sha2::Digest;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(super) struct RetainedFixture {
    inner: Option<Fixture>,
    completed: bool,
}

impl RetainedFixture {
    pub(super) fn new(fixture: Fixture) -> Self {
        Self {
            inner: Some(fixture),
            completed: false,
        }
    }
    pub(super) fn fixture(&self) -> Result<&Fixture, Error> {
        self.inner.as_ref().ok_or_else(|| {
            Error::new(
                "Error",
                "Owned Docker diagnostic fixture is no longer available",
            )
        })
    }
    pub(super) fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for RetainedFixture {
    fn drop(&mut self) {
        if !self.completed {
            if let Some(fixture) = self.inner.take() {
                eprintln!(
                    "Retained owned Docker failure root: {}",
                    fixture.container.display()
                );
                std::mem::forget(fixture);
            }
        }
    }
}

fn native_record(path: &Path) -> Result<Value, Error> {
    use std::io::Read;
    let mut bytes = Vec::new();
    match fs::File::open(path) {
        Ok(file) => {
            file.take(65_537).read_to_end(&mut bytes).map_err(io)?;
            if bytes.len() > 65_536 {
                return Err(Error::new(
                    "Error",
                    "Native Docker submission record exceeds its diagnostic bound",
                ));
            }
            let text =
                String::from_utf8(bytes).map_err(|error| Error::new("Error", error.to_string()))?;
            github_agent_roster::json::parse(&Text::from(text.as_str()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(error) => Err(io(error)),
    }
}

fn cid_state(path: &Path) -> Value {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::string("absent"),
        Err(error) => Value::object(vec![
            ("state", Value::string("inspect-error")),
            ("kind", Value::string(&format!("{:?}", error.kind()))),
            (
                "os",
                error
                    .raw_os_error()
                    .map_or(Value::Null, |code| Value::Number(f64::from(code))),
            ),
        ]),
        Ok(metadata)
            if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 128 =>
        {
            Value::string("nonregular-linked-or-oversized")
        }
        Ok(_) => {
            use std::io::Read;
            let mut bytes = Vec::new();
            let read = fs::File::open(path).and_then(|file| file.take(129).read_to_end(&mut bytes));
            match read {
                Ok(_) => {
                    let text = std::str::from_utf8(&bytes).ok().map(str::trim);
                    match text.filter(|text| {
                        text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
                    }) {
                        Some(value) => Value::object(vec![
                            ("state", Value::string("valid")),
                            ("id", Value::string(value)),
                        ]),
                        None => Value::string("malformed"),
                    }
                }
                Err(error) => Value::object(vec![
                    ("state", Value::string("read-error")),
                    ("kind", Value::string(&format!("{:?}", error.kind()))),
                ]),
            }
        }
    }
}

pub(super) struct Diagnostic {
    mode: String,
    started: Instant,
    at_us: u64,
}

impl Diagnostic {
    pub(super) async fn prepare_cli(
        fixture: &Fixture,
        approval: &native_docker::Approval,
        state: &Path,
    ) -> Result<(), Error> {
        use std::io::Write;
        let before = fs::metadata(&approval.executable).map_err(io)?;
        let identity = github_agent_roster::paths::file_information(&approval.executable)?.identity;
        let home = fixture.container.join("docker-setup-home");
        let config = home.join("config");
        files::create_private_directories(&config)?;
        fs::write(config.join("config.json"), b"{\"auths\":{}}\n").map_err(io)?;
        let host = fixture.host(false)?;
        let mut command = tokio::process::Command::new(&approval.executable);
        command
            .current_dir(&fixture.root)
            .env_clear()
            .envs(&host.environment);
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "TEMP",
            "TMP",
        ] {
            command.env(name, &home);
        }
        command
            .env("DOCKER_CONFIG", &config)
            .env("LANG", "C")
            .env("LC_ALL", "C");
        command.args(["--config"]).arg(&config).args([
            "--host",
            "npipe:////./pipe/docker_engine",
            "image",
            "inspect",
            "--format",
            "{{json .}}",
            &approval.image,
        ]);
        let started = Instant::now();
        let output = github_agent_roster::execution::owned::execute(
            command,
            github_agent_roster::process::Limits {
                timeout: Some(std::time::Duration::from_secs(30)),
                max_output_bytes: 65_536,
            },
            None,
        )
        .await
        .map_err(|failure| {
            Error::new(
                "Error",
                format!(
                    "Owned fixture readiness probe failed before timed verification: {failure}"
                ),
            )
        })?;
        let text = String::from_utf8(output.captured.stdout)
            .map_err(|error| Error::new("Error", error.to_string()))?;
        let image = github_agent_roster::json::parse(&Text::from(text.trim()))?;
        assert_eq!(image.get("Id"), Some(&Value::string(&approval.image)));
        assert_eq!(image.get("Os"), Some(&Value::string("linux")));
        assert!(output.captured.stderr.is_empty());
        let after = fs::metadata(&approval.executable).map_err(io)?;
        assert_eq!(
            github_agent_roster::paths::file_information(&approval.executable)?.identity,
            identity
        );
        assert_eq!(before.len(), after.len());
        assert_eq!(
            before.modified().map_err(io)?,
            after.modified().map_err(io)?
        );
        let calls = docker_calls(state)?;
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls
                .first()
                .and_then(|args| args.get(4))
                .map(String::as_str),
            Some("image")
        );
        assert_eq!(
            calls
                .first()
                .and_then(|args| args.get(5))
                .map(String::as_str),
            Some("inspect")
        );
        assert_eq!(docker_objects(state)?, 0);
        let value = Value::object(vec![
            ("schema_version", Value::Number(1.0)),
            ("fixture_setup_only", Value::Boolean(true)),
            (
                "native_pid",
                output
                    .captured
                    .pid
                    .map_or(Value::Null, |pid| Value::Number(f64::from(pid))),
            ),
            (
                "elapsed_ms",
                Value::Number(started.elapsed().as_secs_f64() * 1000.0),
            ),
            (
                "calls_before_verification",
                Value::Number(calls.len() as f64),
            ),
            (
                "verification_timeout_ms",
                Value::Number(approval.timeout_ms as f64),
            ),
            ("image", Value::string(&approval.image)),
        ]);
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(state.join("fixture-readiness.json"))
            .map_err(io)?;
        file.write_all(format!("{}\n", value.json()).as_bytes())
            .map_err(io)?;
        file.sync_all().map_err(io)
    }

    pub(super) fn observe(
        &self,
        state: &Path,
        report: &native_docker::Report,
    ) -> Result<(), Error> {
        use std::io::Write;
        let value = report
            .as_value()
            .get("native_create_submissions")
            .cloned()
            .unwrap_or(Value::Null);
        let bytes = format!("{}\n", value.json());
        if bytes.len() > 65_536 {
            return Err(Error::new(
                "Error",
                "Observed native Docker receipts exceed their diagnostic bound",
            ));
        }
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(state.join("native-submissions.json"))
            .map_err(io)?;
        file.write_all(bytes.as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)
    }

    pub(super) fn begin(mode: &str) -> Result<Self, Error> {
        Ok(Self {
            mode: mode.to_owned(),
            started: Instant::now(),
            at_us: u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| Error::new("Error", error.to_string()))?
                    .as_micros(),
            )
            .map_err(|error| Error::new("Error", error.to_string()))?,
        })
    }

    pub(super) fn snapshot(
        &self,
        fixture: &Fixture,
        state: &Path,
        error: Option<&Error>,
    ) -> Result<Value, Error> {
        let events = match fs::read(state.join("events")) {
            Ok(bytes) if bytes.len() <= 65_536 => {
                String::from_utf8(bytes).map_err(|error| Error::new("Error", error.to_string()))?
            }
            Ok(_) => {
                return Err(Error::new(
                    "Error",
                    "Owned fixture event log exceeds its diagnostic bound",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(io(error)),
        };
        let events_truncated = events.lines().count() > 192;
        let events = events
            .lines()
            .take(192)
            .map(|line| github_agent_roster::json::parse(&Text::from(line)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut errors = Vec::new();
        let mut next = error;
        for _ in 0..8 {
            let Some(error) = next else {
                break;
            };
            let class = [
                "Docker verification deadline expired",
                "Docker cleanup deadline expired",
                "Owned job shutdown deadline expired",
            ]
            .into_iter()
            .find(|message| error.message.contains(message))
            .unwrap_or("other");
            errors.push(Value::object(vec![
                ("name", Value::string(error.name)),
                ("code", error.code.map_or(Value::Null, Value::string)),
                ("class", Value::string(class)),
                (
                    "message_digest",
                    Value::string(&format!(
                        "{:x}",
                        sha2::Sha256::digest(error.message.as_bytes())
                    )),
                ),
            ]));
            next = error.cause.as_deref();
        }
        match fs::metadata(state.join("calls")) {
            Ok(metadata) if metadata.len() > 65_536 => {
                return Err(Error::new(
                    "Error",
                    "Owned fixture call log exceeds its diagnostic bound",
                ));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(io(error)),
            _ => {}
        }
        let calls = docker_calls(state)?;
        let commands_truncated = calls.len() > 64;
        let mut commands = Vec::new();
        for command in calls.into_iter().take(64) {
            let operation = match (
                command.get(4).map(String::as_str),
                command.get(5).map(String::as_str),
            ) {
                (Some("image"), Some("inspect")) => "image-inspect",
                (Some("container"), Some("create")) => "create",
                (Some("container"), Some("inspect")) => "inspect",
                (Some("container"), Some("start")) => "start",
                (Some("container"), Some("rm")) => "rm",
                _ => "other",
            };
            let selector = command
                .last()
                .filter(|value| {
                    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .map_or(Value::Null, |value| Value::string(value));
            commands.push(Value::object(vec![
                ("operation", Value::string(operation)),
                ("full_id_selector", selector),
            ]));
        }
        let root = fixture.root.join(".git").join("roster-exec");
        let mut transactions = Vec::new();
        if root.is_dir() {
            for entry in fs::read_dir(&root).map_err(io)?.take(16) {
                let path = entry.map_err(io)?.path().join("docker-containers");
                if !path.is_dir() {
                    continue;
                }
                for entry in fs::read_dir(path).map_err(io)?.take(16) {
                    let path = entry.map_err(io)?.path();
                    transactions.push(Value::object(vec![
                        ("intent", Value::Boolean(path.join("intent.json").is_file())),
                        ("cid", cid_state(&path.join("cid"))),
                        ("submission", native_record(&path.join("submission.json"))?),
                    ]));
                }
            }
        }
        let value = Value::object(vec![
            ("schema", Value::Number(1.0)),
            ("mode", Value::string(&self.mode)),
            ("started_at_us", Value::Number(self.at_us as f64)),
            (
                "elapsed_ms",
                Value::Number(self.started.elapsed().as_secs_f64() * 1000.0),
            ),
            (
                "fixture_root",
                Value::string(&fixture.container.to_string_lossy()),
            ),
            ("objects", Value::Number(docker_objects(state)? as f64)),
            ("ready", Value::Boolean(state.join("ready").is_file())),
            (
                "writer_marker",
                Value::Boolean(
                    fixture
                        .root
                        .join(".roster")
                        .join("locks")
                        .join("run.lock")
                        .is_file(),
                ),
            ),
            ("events", Value::Array(events)),
            ("events_truncated", Value::Boolean(events_truncated)),
            ("commands", Value::Array(commands)),
            ("commands_truncated", Value::Boolean(commands_truncated)),
            ("transactions", Value::Array(transactions)),
            (
                "native_create_submissions",
                native_record(&state.join("native-submissions.json"))?,
            ),
            (
                "fixture_setup",
                native_record(&state.join("fixture-readiness.json"))?,
            ),
            ("errors", Value::Array(errors)),
        ]);
        if value.json().len() > 65_536 {
            return Err(Error::new(
                "Error",
                "Native Docker diagnostic exceeds its record bound",
            ));
        }
        Ok(value)
    }

    pub(super) fn record(
        &self,
        fixture: &Fixture,
        state: &Path,
        error: Option<&Error>,
    ) -> Result<(), Error> {
        let value = self.snapshot(fixture, state, error)?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(".docker-480-evidence");
        files::create_private_directories(&root)?;
        let file = root.join(format!(
            "{}-{}-{}.json",
            std::process::id(),
            self.mode,
            self.at_us
        ));
        fs::write(&file, format!("{}\n", value.json())).map_err(io)?;
        if error.is_some() {
            eprintln!("Owned Docker failure evidence: {}", file.display());
        }
        Ok(())
    }
}
