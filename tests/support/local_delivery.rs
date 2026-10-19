use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use github_agent_roster::Error;
use github_agent_roster::config;
use github_agent_roster::execution::{backend, tools};
use github_agent_roster::host;
use github_agent_roster::json::Value;
use github_agent_roster::llm;
use github_agent_roster::llm::http::{Body, Request, Response, Transport};
use github_agent_roster::planning::{artifacts, lifecycle};
use github_agent_roster::state::{files, secure};
use github_agent_roster::text::Text;

#[path = "local_diagnostics.rs"]
pub mod diagnostics;

pub fn io(error: std::io::Error) -> Error {
    Error::new("FixtureError", error.to_string())
}

pub fn git_image() -> Result<PathBuf, Error> {
    let name = if cfg!(windows) { "git.exe" } else { "git" };
    let image = std::env::var_os("ROSTER_TEST_GIT")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|path| path.join(name))
                    .find(|path| path.is_file())
            })
        })
        .ok_or_else(|| Error::type_error("Owned local Git fixtures need an installed Git image"))?;
    Ok(host::Image::approve(&image)?.path().to_path_buf())
}

pub fn minimal() -> Result<BTreeMap<String, String>, Error> {
    let input = Value::Object(
        std::env::vars()
            .map(|(key, value)| (Text::from(key.as_str()), Value::string(value.as_str())))
            .collect(),
    );
    host::minimal_environment(&input)
}

pub struct Fixture {
    pub container: PathBuf,
    pub root: PathBuf,
    pub git: PathBuf,
    pub home: PathBuf,
    pub diagnostics: diagnostics::Diagnostics,
    pub retain_on_drop: bool,
}

impl Fixture {
    pub fn new() -> Result<Self, Error> {
        let container = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".native-local-{}-{}",
            std::process::id(),
            files::nonce()?
        ));
        let root = container.join("project");
        let home = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or_else(|| Error::type_error("Owned fixture home needs an outside-Git parent"))?
            .join(format!(
                ".native-local-home-{}-{}",
                std::process::id(),
                files::nonce()?
            ));
        secure::private_directory(&home, true)?;
        secure::private_directory(&root, true)?;
        let fixture = Self {
            diagnostics: diagnostics::Diagnostics::new(&root, &home, &container),
            retain_on_drop: false,
            container,
            root,
            git: git_image()?,
            home,
        };
        fixture.git(&["init", "--quiet", "--initial-branch=main"])?;
        fixture.write(".gitignore", ".roster/\n.roster-state/\n.worktrees/\nTASK.md\nRECIPE.yml\nASSIGNMENT.md\nESTIMATE.md\nCONTEXT.md\nRESULT.md\nREVIEW.md\nPLAN.md\nRESEARCH.md\ntarget/\n")?;
        fixture.write("AGENTS.md", "# Owned local consumer\n\nPreserve existing behavior and use actual matching checks. No publication.\n")?;
        fixture.write("README.md", "# Owned local consumer\n")?;
        fixture.write("docs/guide.md", "# Guide\n\nBefore.\n")?;
        fixture.write(
            "src/app.mjs",
            "export function answer(value = 0) { return 1; }\n",
        )?;
        fixture.write("tests/app.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('retained zero input', () => assert.equal(answer(0), 1));\n")?;
        fixture.git(&["add", "."])?;
        fixture.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned native local delivery base",
        ])?;
        fixture.configure(None)?;
        Ok(fixture)
    }

    pub fn git(&self, args: &[&str]) -> Result<String, Error> {
        self.git_at(&self.root, args)
    }

    pub fn git_at(&self, root: &Path, args: &[&str]) -> Result<String, Error> {
        let output = Command::new(&self.git)
            .current_dir(root)
            .env_clear()
            .envs(minimal()?)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "--no-pager",
                "-c",
                "init.templateDir=",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Owned Local Fixture",
                "-c",
                "user.email=owned@example.invalid",
                "-c",
            ])
            .arg(format!(
                "core.hooksPath={}",
                self.container.join("no-hooks").display()
            ))
            .args(args)
            .output()
            .map_err(io)?;
        if !output.status.success() {
            return Err(Error::new(
                "FixtureGit",
                String::from_utf8(output.stderr)
                    .map_err(|_| Error::type_error("Fixture Git diagnostic must be UTF-8"))?,
            ));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| Error::type_error("Fixture Git output must be UTF-8"))
    }

    pub fn write(&self, file: &str, source: &str) -> Result<(), Error> {
        let path = self.root.join(file);
        secure::private_directory(
            path.parent()
                .ok_or_else(|| Error::type_error("Fixture file needs a parent"))?,
            true,
        )?;
        fs::write(&path, source).map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(io)?;
        }
        Ok(())
    }

    pub fn configure(&self, endpoint: Option<&str>) -> Result<(), Error> {
        let mut source = include_str!("../../roster.config.example.yml")
            .replace("turn_budget: 1000", "turn_budget: 16");
        if let Some(endpoint) = endpoint {
            source = source
                .replacen("base_url: \"\"", &format!("base_url: \"{endpoint}\""), 1)
                .replacen("model: \"\"", "model: configured-route-not-a-response", 1);
        }
        self.write(".roster/config.yml", &source)
    }

    pub fn task(&self, files: &[&str], check: &str) -> Result<Text, Error> {
        let ask = Text::from(format!(
            "Implement the requested bounded outcome and preserve existing behavior.\n\n## Files allowed\n{}\n\n## Acceptance checks\n- {check}",
            files.iter().map(|file| format!("- {file}")).collect::<Vec<_>>().join("\n")
        ).as_str());
        let plan = artifacts::build(
            &Value::String(ask.clone()),
            artifacts::BuildOptions {
                reference: Some("local:owned-task"),
                files: Some(&Value::Array(
                    files.iter().map(|file| Value::string(file)).collect(),
                )),
                checks: Some(&Value::Array(vec![Value::string(check)])),
                ..artifacts::BuildOptions::default()
            },
        )?;
        let mode = if files.iter().all(|path| path.ends_with(".md")) {
            "none"
        } else {
            "required"
        };
        let task = plan.task.to_utf8()?;
        let task = if task.lines().any(|line| line.starts_with("tests:")) {
            task.lines()
                .map(|line| {
                    if line.starts_with("tests:") {
                        format!("tests: {mode}")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        } else {
            task.replacen("---\n", &format!("---\ntests: {mode}\n"), 1)
        };
        self.write("TASK.md", &task)?;
        self.write("RECIPE.yml", &plan.recipe.to_utf8()?)?;
        Ok(ask)
    }

    pub fn environment(&self) -> Result<Value, Error> {
        let mut environment = Value::Object(
            minimal()?
                .into_iter()
                .map(|(key, value)| (Text::from(key.as_str()), Value::string(value.as_str())))
                .collect(),
        );
        for name in ["HOME", "USERPROFILE"] {
            put(
                &mut environment,
                name,
                Value::string(
                    self.home
                        .to_str()
                        .ok_or_else(|| Error::type_error("Owned fixture home must be Unicode"))?,
                ),
            );
        }
        put(
            &mut environment,
            "ROSTER_GIT",
            Value::string(
                self.git
                    .to_str()
                    .ok_or_else(|| Error::type_error("Fixture Git path must be Unicode"))?,
            ),
        );
        put(
            &mut environment,
            "ROSTER_INSTALLATION_ROOT",
            Value::string(env!("CARGO_MANIFEST_DIR")),
        );
        if let Some(node) = std::env::var_os("ROSTER_TEST_NODE") {
            put(
                &mut environment,
                "ROSTER_NODE20",
                Value::string(
                    node.to_str()
                        .ok_or_else(|| Error::type_error("Fixture Node image must be Unicode"))?,
                ),
            );
        }
        Ok(environment)
    }

    pub fn host(&self) -> Result<(tools::Host, backend::Approval), Error> {
        host::invocation_host(
            &self.environment()?,
            Path::new(env!("CARGO_BIN_EXE_roster")),
        )
    }

    pub fn loaded(&self) -> Result<config::Loaded, Error> {
        config::load(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &self.root,
            &Value::object(Vec::new()),
        )
    }

    pub fn native_cargo(&self) -> Result<Value, Error> {
        self.write("Cargo.toml", "[package]\nname = \"owned-local-native-consumer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"rust/llm/worker.rs\"\n")?;
        self.write("Cargo.lock", "version = 3\n\n[[package]]\nname = \"owned-local-native-consumer\"\nversion = \"0.1.0\"\n")?;
        self.write(
            "rust/llm/worker.rs",
            "pub fn next(_value: u8) -> u8 {\n    1\n}\n",
        )?;
        self.write("tests/base.rs", "#[test]\nfn retained_zero() {\n    assert_eq!(owned_local_native_consumer::next(0), 1);\n}\n")?;
        self.git(&[
            "add",
            "Cargo.toml",
            "Cargo.lock",
            "rust/llm/worker.rs",
            "tests/base.rs",
        ])?;
        self.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned dependency-free native consumer base",
        ])?;
        let mut environment = self.environment()?;
        let cargo = std::env::var_os("CARGO").ok_or_else(|| {
            Error::type_error("Owned Cargo consumer needs the invoking installed Cargo image")
        })?;
        let cargo = fs::canonicalize(cargo).map_err(io)?;
        let cargo = host::Image::approve(&cargo)?;
        put(
            &mut environment,
            "ROSTER_CARGO",
            Value::string(
                cargo
                    .path()
                    .to_str()
                    .ok_or_else(|| Error::type_error("Cargo image path must be Unicode"))?,
            ),
        );
        for name in ["CARGO_HOME", "RUSTUP_HOME", "RUSTFLAGS", "RUSTUP_TOOLCHAIN"] {
            if let Ok(value) = std::env::var(name) {
                put(&mut environment, name, Value::string(&value));
            }
        }
        if let Ok(toolchain) = std::env::var("ROSTER_TEST_CARGO_TOOLCHAIN") {
            put(
                &mut environment,
                "RUSTUP_TOOLCHAIN",
                Value::string(&toolchain),
            );
        }
        put(
            &mut environment,
            "CARGO_TARGET_DIR",
            Value::string(
                self.container
                    .join("approved-target")
                    .to_str()
                    .ok_or_else(|| Error::type_error("Cargo target must be Unicode"))?,
            ),
        );
        put(
            &mut environment,
            "ROSTER_BACKEND_TIMEOUT_MS",
            Value::string("120000"),
        );
        Ok(environment)
    }

    pub fn critic(&self, endpoint: &str) -> Result<(), Error> {
        let source = fs::read_to_string(self.root.join(".roster").join("config.yml"))
            .map_err(io)?
            .replace("\r\n", "\n")
            .replacen(
                "planner:\n",
                "planner:\n  critic_profile: local-critic\n",
                1,
            );
        self.write(".roster/config.yml", &source)?;
        let fleet = github_agent_roster::fleet::format(&Value::object(vec![(
            "profiles",
            Value::Array(vec![Value::object(vec![
                ("id", Value::string("local-critic")),
                ("base_url", Value::string(endpoint)),
                ("model", Value::string("configured-critic-route")),
                ("provider", Value::string("vllm")),
                ("api_key_env", Value::string("ROSTER_CRITIC_API_KEY")),
                ("context_max", Value::Number(32768.0)),
                ("concurrency", Value::Number(1.0)),
                ("hardware", Value::string("owned fixture")),
                ("notes", Value::string("Owned loopback critic")),
            ])]),
        )]))?;
        self.write(".roster/fleet.yml", &fleet.to_utf8()?)
    }

    pub fn invoke(&self, args: &[&str]) -> Result<Output, Error> {
        self.invoke_with(args, &self.environment()?, &self.root)
    }

    pub fn invoke_with(
        &self,
        args: &[&str],
        environment: &Value,
        cwd: &Path,
    ) -> Result<Output, Error> {
        self.diagnostics.client_started(cwd);
        let output = self.command(args, environment, cwd)?.output();
        self.diagnostics.cli_returned(&output);
        output.map_err(io)
    }

    pub fn command(
        &self,
        args: &[&str],
        environment: &Value,
        cwd: &Path,
    ) -> Result<Command, Error> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_roster"));
        command.args(args).current_dir(cwd).env_clear();
        let Value::Object(entries) = environment else {
            return Err(Error::type_error("Fixture environment must be an object"));
        };
        for (name, value) in entries {
            command.env(
                name.to_utf8()?,
                value
                    .as_string()
                    .ok_or_else(|| Error::type_error("Fixture environment value must be text"))?
                    .to_utf8()?,
            );
        }
        Ok(command)
    }

    pub fn assignment(&self) -> Result<lifecycle::Assignment, Error> {
        let task = fs::read_to_string(self.root.join("TASK.md")).map_err(io)?;
        let document =
            github_agent_roster::planning::task::parse(&Value::string(&task), &Default::default())?;
        Ok(lifecycle::Assignment {
            ask: document.ask,
            title: None,
            reference: "local:owned-task".to_owned(),
            task: "owned-task".to_owned(),
            session: "owned-root-lease".to_owned(),
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.retain_on_drop || std::thread::panicking() || self.diagnostics.failed() {
            self.diagnostics.root(&self.root);
            self.diagnostics.retain("fixture-retained");
            eprintln!(
                "Owned failed fixture retained: root={} home={}",
                self.root.display(),
                self.home.display()
            );
            return;
        }
        if let Err(error) = fs::remove_dir_all(&self.home) {
            eprintln!("Owned fixture home cleanup failed: {error}");
        }
        if let Err(error) = fs::remove_dir_all(&self.container) {
            eprintln!("Owned local fixture cleanup failed: {error}");
        }
    }
}

pub fn put(value: &mut Value, name: &str, replacement: Value) {
    if let Value::Object(entries) = value {
        if let Some((_, old)) = entries.iter_mut().find(|(key, _)| key == &Text::from(name)) {
            *old = replacement;
        } else {
            entries.push((Text::from(name), replacement));
        }
    }
}

pub fn text(bytes: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(bytes).map_err(|_| Error::type_error("Native fixture output must be UTF-8"))
}

pub fn packet(message: Value, finish: &str, model: &str) -> Value {
    Value::object(vec![
        ("model", Value::string(model)),
        (
            "choices",
            Value::Array(vec![Value::object(vec![
                ("message", message),
                ("finish_reason", Value::string(finish)),
            ])]),
        ),
        (
            "usage",
            Value::object(vec![
                ("prompt_tokens", Value::Number(0.0)),
                ("completion_tokens", Value::Number(0.0)),
            ]),
        ),
    ])
}

pub fn message(content: &str) -> Value {
    Value::object(vec![
        ("role", Value::string("assistant")),
        ("content", Value::string(content)),
    ])
}

pub fn calls(calls: Vec<(&str, &str, Value)>) -> Value {
    packet(
        Value::object(vec![
            ("role", Value::string("assistant")),
            ("content", Value::Null),
            (
                "tool_calls",
                Value::Array(
                    calls
                        .into_iter()
                        .map(|(id, name, arguments)| {
                            Value::object(vec![
                                ("id", Value::string(id)),
                                ("type", Value::string("function")),
                                (
                                    "function",
                                    Value::object(vec![
                                        ("name", Value::string(name)),
                                        ("arguments", Value::string(&arguments.json())),
                                    ]),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        "tool_calls",
        "actual-local-coder",
    )
}

pub fn path(path: &str) -> Value {
    Value::object(vec![("path", Value::string(path))])
}

pub fn edit(path: &str, old: &str, new: &str) -> Value {
    Value::object(vec![
        ("path", Value::string(path)),
        ("old_string", Value::string(old)),
        ("new_string", Value::string(new)),
    ])
}

pub fn verdict(pass: bool) -> Value {
    Value::object(vec![
        ("verdict", Value::string(if pass { "pass" } else { "fail" })),
        (
            "reasons",
            Value::Array(if pass {
                Vec::new()
            } else {
                vec![Value::string("The actual requested check remains unmet.")]
            }),
        ),
        ("security_notes", Value::Array(Vec::new())),
        (
            "checks",
            Value::Array(vec![Value::object(vec![
                ("id", Value::Number(1.0)),
                ("met", Value::Boolean(pass)),
                (
                    "evidence",
                    Value::string(
                        "Current delivered source and actual matching host checks were independently inspected.",
                    ),
                ),
            ])]),
        ),
    ])
}

pub fn review(value: Value) -> Value {
    packet(message(&value.json()), "stop", "actual-local-reviewer")
}

pub fn summary() -> Value {
    packet(
        message("1. done: the requested current files and matching checks were verified."),
        "stop",
        "actual-local-coder",
    )
}

pub fn docs_script(pass: bool) -> Vec<Value> {
    vec![
        calls(vec![("read", "read_file", path("docs/guide.md"))]),
        calls(vec![(
            "edit",
            "edit_file",
            edit(
                "docs/guide.md",
                "Before.",
                "The requested bounded outcome is documented.",
            ),
        )]),
        summary(),
        review(verdict(true)),
        review(verdict(pass)),
    ]
}

pub fn code_script(file: &str, test_file: &str, before: &str, after: &str) -> Vec<Value> {
    vec![
        calls(vec![
            ("read-product", "read_file", path(file)),
            ("read-test", "read_file", path(test_file)),
        ]),
        calls(vec![
            (
                "edit-product",
                "edit_file",
                edit(file, "return 1", "return value + 1"),
            ),
            ("edit-test", "edit_file", edit(test_file, before, after)),
        ]),
        calls(vec![(
            "matching-test",
            "run_test",
            Value::object(Vec::new()),
        )]),
        summary(),
        packet(
            message(
                &Value::object(vec![
                    (
                        "checks",
                        verdict(true).get("checks").cloned().unwrap_or(Value::Null),
                    ),
                    ("findings", Value::Array(Vec::new())),
                ])
                .json(),
            ),
            "stop",
            "actual-local-author-review",
        ),
        review(verdict(true)),
        review(verdict(true)),
    ]
}

pub struct Bytes(Option<Vec<u8>>);

impl Body for Bytes {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.0.take())
    }
}

type RequestAction = Box<dyn FnMut(usize, &Value) -> Result<(), Error>>;
type ServerAction = Box<dyn FnMut(usize, &Value) -> Result<(), Error> + Send>;

pub struct Scripted {
    pub responses: VecDeque<Value>,
    pub requests: Vec<Value>,
    pub on_send: Option<RequestAction>,
}

impl Scripted {
    pub fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: responses.into(),
            requests: Vec::new(),
            on_send: None,
        }
    }
}

impl Transport for Scripted {
    type Body = Bytes;

    async fn send(&mut self, request: &Request) -> Result<Response<Bytes>, llm::Failure> {
        let request = github_agent_roster::json::parse(&Text::from(text(&request.body)?))?;
        self.requests.push(request.clone());
        if let Some(action) = &mut self.on_send {
            action(self.requests.len(), &request)?;
        }
        let response = self
            .responses
            .pop_front()
            .ok_or_else(|| Error::type_error("Owned scripted response sequence is exhausted"))?;
        Ok(Response {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Some(Bytes(Some(response.json().into_bytes()))),
        })
    }
}

fn receive(stream: &mut TcpStream) -> Result<Value, Error> {
    stream.set_nonblocking(false).map_err(io)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(io)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(io)?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let (start, length) = loop {
        let read = stream.read(&mut buffer).map_err(io)?;
        if read == 0 {
            return Err(Error::type_error(
                "Loopback HTTP closed before request headers",
            ));
        }
        bytes.extend_from_slice(
            buffer
                .get(..read)
                .ok_or_else(|| Error::type_error("Invalid loopback read size"))?,
        );
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(Error::type_error("Loopback request exceeds 4 MiB"));
        }
        if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let headers = text(
                bytes
                    .get(..end)
                    .ok_or_else(|| Error::type_error("Loopback header range missing"))?,
            )?;
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then_some(value.trim())
                })
                .ok_or_else(|| Error::type_error("Loopback request needs Content-Length"))?
                .parse::<usize>()
                .map_err(|_| Error::type_error("Loopback Content-Length must be bounded"))?;
            if length > 4 * 1024 * 1024 {
                return Err(Error::type_error("Loopback request body exceeds 4 MiB"));
            }
            break (end + 4, length);
        }
    };
    while bytes.len() < start + length {
        let read = stream.read(&mut buffer).map_err(io)?;
        if read == 0 {
            return Err(Error::type_error(
                "Loopback HTTP closed before request body",
            ));
        }
        bytes.extend_from_slice(
            buffer
                .get(..read)
                .ok_or_else(|| Error::type_error("Invalid loopback read size"))?,
        );
    }
    github_agent_roster::json::parse(&Text::from(text(
        bytes
            .get(start..start + length)
            .ok_or_else(|| Error::type_error("Loopback body range missing"))?,
    )?))
}

fn response(stream: &mut TcpStream, request: &Value, packet: &Value) -> Result<(), Error> {
    let streaming = request.get("stream") == Some(&Value::Boolean(true));
    let (content_type, bytes) = if streaming {
        let choice = packet
            .get("choices")
            .and_then(|value| match value {
                Value::Array(items) => items.first(),
                _ => None,
            })
            .ok_or_else(|| Error::type_error("Loopback packet has no choice"))?;
        let mut delta = choice
            .get("message")
            .cloned()
            .ok_or_else(|| Error::type_error("Loopback packet has no assistant message"))?;
        if let Some(Value::Array(calls)) = delta.get("tool_calls").cloned() {
            let calls = calls
                .into_iter()
                .enumerate()
                .map(|(index, mut call)| {
                    put(&mut call, "index", Value::Number(index as f64));
                    call
                })
                .collect();
            put(&mut delta, "tool_calls", Value::Array(calls));
        }
        let chunk = Value::object(vec![
            ("model", packet.get("model").cloned().unwrap_or(Value::Null)),
            (
                "choices",
                Value::Array(vec![Value::object(vec![
                    ("index", Value::Number(0.0)),
                    ("delta", delta),
                    ("finish_reason", Value::Null),
                ])]),
            ),
        ]);
        let terminal = Value::object(vec![
            ("model", packet.get("model").cloned().unwrap_or(Value::Null)),
            ("usage", packet.get("usage").cloned().unwrap_or(Value::Null)),
            (
                "choices",
                Value::Array(vec![Value::object(vec![
                    ("index", Value::Number(0.0)),
                    ("delta", Value::object(Vec::new())),
                    (
                        "finish_reason",
                        choice.get("finish_reason").cloned().unwrap_or(Value::Null),
                    ),
                ])]),
            ),
        ]);
        (
            "text/event-stream",
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                chunk.json(),
                terminal.json()
            )
            .into_bytes(),
        )
    } else {
        ("application/json", packet.json().into_bytes())
    };
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).map_err(io)?;
    stream.write_all(&bytes).map_err(io)?;
    stream.flush().map_err(io)
}

pub struct Server {
    pub endpoint: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<(), Error>>>,
    diagnostics: Option<diagnostics::Diagnostics>,
}

impl Server {
    pub fn start(packets: Vec<Value>) -> Result<Self, Error> {
        Self::launch(packets, false, None, None)
    }

    pub fn start_for(fixture: &Fixture, packets: Vec<Value>) -> Result<Self, Error> {
        Self::launch(packets, false, None, Some(fixture.diagnostics.clone()))
    }

    pub fn pending() -> Result<Self, Error> {
        Self::launch(Vec::new(), true, None, None)
    }

    pub fn observed(
        packets: Vec<Value>,
        observe: impl FnMut(usize, &Value) -> Result<(), Error> + Send + 'static,
    ) -> Result<Self, Error> {
        Self::launch(packets, false, Some(Box::new(observe)), None)
    }

    pub fn observed_for(
        fixture: &Fixture,
        packets: Vec<Value>,
        observe: impl FnMut(usize, &Value) -> Result<(), Error> + Send + 'static,
    ) -> Result<Self, Error> {
        Self::launch(
            packets,
            false,
            Some(Box::new(observe)),
            Some(fixture.diagnostics.clone()),
        )
    }

    fn launch(
        packets: Vec<Value>,
        pending: bool,
        mut observe: Option<ServerAction>,
        diagnostics: Option<diagnostics::Diagnostics>,
    ) -> Result<Self, Error> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(io)?;
        let endpoint = format!("http://{}/v1", listener.local_addr().map_err(io)?);
        listener.set_nonblocking(true).map_err(io)?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let mut packets = VecDeque::from(packets);
        let diagnostic = diagnostics.clone();
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(600);
            if let Some(diagnostic) = &diagnostic {
                diagnostic.server_start(deadline - Duration::from_secs(600), packets.len());
            }
            let result = (|| {
                loop {
                    if cancelled.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    if Instant::now() >= deadline {
                        if let Some(diagnostic) = &diagnostic {
                            diagnostic.server_phase("guard");
                            diagnostic.retain("server-guard");
                        }
                        return Err(Error::type_error(
                            "Owned loopback server exceeded its workflow hang guard",
                        ));
                    }
                    match listener.accept() {
                        Ok((mut stream, address)) => {
                            if !address.ip().is_loopback() {
                                return Err(Error::type_error("Non-loopback client refused"));
                            }
                            if let Some(diagnostic) = &diagnostic {
                                diagnostic.server_phase("receiving");
                            }
                            let request = receive(&mut stream)?;
                            if let Some(diagnostic) = &diagnostic {
                                diagnostic.accepted(&request);
                            }
                            if pending {
                                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\n{").map_err(io)?;
                                stream.flush().map_err(io)?;
                                observed
                                    .lock()
                                    .map_err(|_| {
                                        Error::type_error("Loopback request lock is poisoned")
                                    })?
                                    .push(request);
                                while !cancelled.load(Ordering::SeqCst) && Instant::now() < deadline
                                {
                                    thread::sleep(Duration::from_millis(5));
                                }
                                return Ok(());
                            }
                            observed
                                .lock()
                                .map_err(|_| {
                                    Error::type_error("Loopback request lock is poisoned")
                                })?
                                .push(request.clone());
                            if let Some(observe) = &mut observe {
                                if let Some(diagnostic) = &diagnostic {
                                    diagnostic.server_phase("callback");
                                }
                                let count = observed
                                    .lock()
                                    .map_err(|_| {
                                        Error::type_error("Loopback request lock is poisoned")
                                    })?
                                    .len();
                                observe(count, &request)?;
                            }
                            let packet = packets.pop_front().ok_or_else(|| {
                                Error::type_error("Actual CLI exceeded its owned response sequence")
                            })?;
                            if let Some(diagnostic) = &diagnostic {
                                diagnostic.popped(packets.len());
                            }
                            response(&mut stream, &request, &packet)?;
                            if let Some(diagnostic) = &diagnostic {
                                diagnostic.responded();
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => return Err(io(error)),
                    }
                }
            })();
            if let Err(error) = &result {
                if let Some(diagnostic) = &diagnostic {
                    diagnostic.server_error(error);
                    diagnostic.retain("server-error");
                }
            }
            result
        });
        Ok(Self {
            endpoint,
            requests,
            stop,
            thread: Some(thread),
            diagnostics,
        })
    }

    pub fn finish(mut self) -> Result<Vec<Value>, Error> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let joined = thread
                .join()
                .map_err(|_| Error::type_error("Owned loopback server panicked"))
                .and_then(|result| result);
            if let Err(error) = joined {
                if let Some(diagnostic) = &self.diagnostics {
                    diagnostic.server_error(&error);
                    diagnostic.retain("server-finish-error");
                }
                return Err(error);
            }
        }
        self.requests
            .lock()
            .map_err(|_| Error::type_error("Loopback request lock is poisoned"))
            .map(|requests| requests.clone())
    }
}

pub struct OwnedCli(pub Option<Child>);

impl Drop for OwnedCli {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            match child.try_wait() {
                Ok(None) => {
                    if let Err(error) = child.kill() {
                        eprintln!("Owned CLI termination failed: {error}");
                    }
                }
                Ok(Some(_)) => {}
                Err(error) => eprintln!("Owned CLI status failed: {error}"),
            }
            if let Err(error) = child.wait() {
                eprintln!("Owned CLI wait failed: {error}");
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("Owned loopback server cleanup: {error}"),
                Err(_) => eprintln!("Owned loopback server cleanup panicked"),
            }
        }
    }
}
