use super::*;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use github_agent_roster::vault::FileVault;

const PRIMARY: &str = "ordinary-opaque-vault-primary";
const CRITIC: &str = "ordinary-opaque-vault-critic";

struct Retained(std::mem::ManuallyDrop<Fixture>);

impl Retained {
    fn new() -> Result<Self, Error> {
        let fixture = Fixture::new()?;
        eprintln!(
            "Owned vault delivery fixture: {} | home: {}",
            fixture.container.display(),
            fixture.home.display()
        );
        Ok(Self(std::mem::ManuallyDrop::new(fixture)))
    }

    fn finish(self) -> Result<(), Error> {
        let container = self.container.clone();
        let home = self.home.clone();
        drop(std::mem::ManuallyDrop::into_inner(self.0));
        if container.exists() || home.exists() {
            return Err(Error::type_error(
                "Owned vault delivery fixture cleanup is incomplete",
            ));
        }
        Ok(())
    }
}

impl std::ops::Deref for Retained {
    type Target = Fixture;

    fn deref(&self) -> &Fixture {
        &self.0
    }
}

fn vault(fixture: &Fixture) -> Result<FileVault, Error> {
    FileVault::new(fixture.home.join(".roster").join("vault")).map_err(Error::from)
}

fn selected(fixture: &Fixture) -> Result<(), Error> {
    fixture.configure(Some("http://127.0.0.1:1/v1"))?;
    fixture.critic("http://127.0.0.1:2/v1")?;
    Ok(())
}

fn required(fixture: &Fixture) -> Result<(), Error> {
    let source = fs::read_to_string(fixture.root.join(".roster").join("config.yml")).map_err(io)?;
    fixture.write(
        ".roster/config.yml",
        &source
            .replace("\r\n", "\n")
            .replacen("llm:\n", "llm:\n  api_key_optional: false\n", 1),
    )
}

#[test]
fn selected_provider_reads_only_primary_and_effective_critic_without_enumeration()
-> Result<(), Error> {
    let fixture = Retained::new()?;
    selected(&fixture)?;
    let vault = vault(&fixture)?;
    vault.set("ROSTER_API_KEY", PRIMARY)?;
    vault.set("ROSTER_CRITIC_API_KEY", CRITIC)?;
    let directory = fixture.home.join(".roster").join("vault");
    fs::write(
        directory.join("not-an-entry.json"),
        "corrupt unselected entry",
    )
    .map_err(io)?;
    fs::write(
        directory.join("4749544855425f4150505f4944.json"),
        "forbidden unselected entry",
    )
    .map_err(io)?;
    let loaded = fixture.loaded()?;
    for value in [None, Some(Value::string(""))] {
        let mut environment = fixture.environment()?;
        if let Some(value) = value {
            put(&mut environment, "ROSTER_API_KEY", value);
        }
        put(
            &mut environment,
            "GITHUB_APP_PRIVATE_KEY_PATH",
            Value::Number(19.0),
        );
        put(&mut environment, "UNSELECTED_KEY", Value::Boolean(false));
        let mut reads = Vec::new();
        let credentials =
            delivery::invocation_credentials_with_vault(&environment, &loaded, &mut |name| {
                reads.push(name.to_owned());
                vault
                    .get(name)
                    .map(|value| value.map(|value| Value::string(value.as_str())))
                    .map_err(Error::from)
            })?;
        assert_eq!(reads, ["ROSTER_API_KEY", "ROSTER_CRITIC_API_KEY"]);
        assert_eq!(
            credentials
                .vault_entries
                .as_ref()
                .and_then(|value| value.get("ROSTER_API_KEY")),
            Some(&Value::string(PRIMARY))
        );
        assert_eq!(
            credentials
                .vault_entries
                .as_ref()
                .and_then(|value| value.get("ROSTER_CRITIC_API_KEY")),
            Some(&Value::string(CRITIC))
        );
        assert_eq!(
            credentials
                .vault_entries
                .as_ref()
                .and_then(|value| value.get("GITHUB_APP_PRIVATE_KEY_PATH")),
            None
        );
        let failure = Error::new(
            "Callback",
            format!("custom non-HTTP failure {PRIMARY} {CRITIC}"),
        )
        .caused_by(Error::new("Inner", format!("{CRITIC} {PRIMARY}")));
        let safe = delivery::safe_invocation_error(&failure, &loaded, &credentials)?;
        assert!(!format!("{safe:?}").contains(PRIMARY) && !format!("{safe:?}").contains(CRITIC));
    }
    assert!(vault.list().is_err());
    fixture.finish()
}

#[test]
fn selected_nonempty_environment_wins_without_a_home_or_any_vault_read() -> Result<(), Error> {
    let fixture = Retained::new()?;
    selected(&fixture)?;
    let loaded = fixture.loaded()?;
    let environment = Value::object(vec![
        ("ROSTER_API_KEY", Value::string(PRIMARY)),
        ("ROSTER_CRITIC_API_KEY", Value::string(CRITIC)),
        ("GITHUB_APP_ID", Value::Number(0.0)),
    ]);
    let mut reads = 0;
    let credentials =
        delivery::invocation_credentials_with_vault(&environment, &loaded, &mut |_| {
            reads += 1;
            Err(Error::new("Fixture", "must not read any vault"))
        })?;
    assert_eq!(reads, 0);
    assert!(credentials.vault_entries.is_none());
    assert_eq!(
        credentials
            .environment
            .as_ref()
            .and_then(|value| value.get("ROSTER_API_KEY")),
        Some(&Value::string(PRIMARY))
    );
    assert!(
        delivery::native_invocation_credentials(&environment, &loaded)?
            .vault_entries
            .is_none()
    );
    assert!(!fixture.home.join(".roster").exists());
    fixture.finish()
}

#[test]
fn selected_invalid_environment_provider_values_and_nested_failures_are_not_optional_success()
-> Result<(), Error> {
    let fixture = Retained::new()?;
    selected(&fixture)?;
    let loaded = fixture.loaded()?;
    for role in ["ROSTER_API_KEY", "ROSTER_CRITIC_API_KEY"] {
        let mut environment = Value::object(vec![
            ("ROSTER_API_KEY", Value::string(PRIMARY)),
            ("ROSTER_CRITIC_API_KEY", Value::string(CRITIC)),
        ]);
        put(&mut environment, role, Value::Number(7.0));
        let mut reads = 0;
        assert!(
            delivery::invocation_credentials_with_vault(&environment, &loaded, &mut |_| {
                reads += 1;
                Ok(None)
            })
            .is_err_and(|error| error.name == "TypeError")
        );
        assert_eq!(reads, 0);
    }
    for invalid in [Value::Null, Value::Number(3.0), Value::string("")] {
        assert!(
            delivery::invocation_credentials_with_vault(
                &Value::object(Vec::new()),
                &loaded,
                &mut |_| Ok(Some(invalid.clone()))
            )
            .is_err_and(|error| error.message.contains("invalid secret"))
        );
    }
    let failure = delivery::invocation_credentials_with_vault(
        &Value::object(Vec::new()),
        &loaded,
        &mut |_| Err(Error::new("Fixture", PRIMARY).caused_by(Error::new("Fixture", CRITIC))),
    )
    .err()
    .ok_or_else(|| Error::type_error("Failing credential provider passed"))?;
    assert_eq!(failure.code, Some("ROSTER_SECRET_RESOLUTION"));
    assert!(failure.cause.is_none());
    assert!(!format!("{failure:?}").contains(PRIMARY) && !format!("{failure:?}").contains(CRITIC));
    fixture.finish()
}

#[test]
fn effective_keyless_critic_inherits_primary_name_without_duplicate_provider_reads()
-> Result<(), Error> {
    let fixture = Retained::new()?;
    selected(&fixture)?;
    let fleet = fs::read_to_string(fixture.root.join(".roster").join("fleet.yml")).map_err(io)?;
    let fleet = fleet
        .lines()
        .filter(|line| !line.trim_start().starts_with("api_key_env:"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fixture.write(".roster/fleet.yml", &fleet)?;
    let loaded = fixture.loaded()?;
    let mut reads = Vec::new();
    let credentials = delivery::invocation_credentials_with_vault(
        &fixture.environment()?,
        &loaded,
        &mut |name| {
            reads.push(name.to_owned());
            Ok(None)
        },
    )?;
    assert_eq!(reads, ["ROSTER_API_KEY"]);
    assert!(credentials.vault_entries.is_none());
    assert!(
        delivery::native_invocation_credentials(&fixture.environment()?, &loaded)?
            .vault_entries
            .is_none()
    );
    assert!(!fixture.home.join(".roster").exists());
    fixture.finish()
}

#[test]
fn actual_cli_corrupt_vault_stops_before_effects_even_for_optional_primary_or_critic()
-> Result<(), Error> {
    for role in ["ROSTER_API_KEY", "ROSTER_CRITIC_API_KEY"] {
        let fixture = Retained::new()?;
        selected(&fixture)?;
        fixture.task(
            &["docs/guide.md"],
            "The guide documents the requested bounded outcome.",
        )?;
        let vault = vault(&fixture)?;
        vault.set(role, PRIMARY)?;
        fs::remove_file(fixture.home.join(".roster").join("vault").join(".key")).map_err(io)?;
        let mut environment = fixture.environment()?;
        if role == "ROSTER_CRITIC_API_KEY" {
            put(&mut environment, "ROSTER_API_KEY", Value::string(CRITIC));
        }
        let output = fixture.invoke_with(
            &["run", "--seat", "coder", "--runtime", "builtin"],
            &environment,
            &fixture.root,
        )?;
        assert!(!output.status.success() && output.stdout.is_empty());
        let error = text(&output.stderr)?;
        assert!(error.contains("Unable to resolve the secret from the file vault."));
        assert!(!error.contains(PRIMARY) && !error.contains(CRITIC));
        assert!(
            !error.contains(
                fixture
                    .home
                    .to_str()
                    .ok_or_else(|| Error::type_error("Fixture home must be Unicode"))?
            )
        );
        assert!(!fixture.root.join(".roster").join("runs").exists());
        assert!(!fixture.root.join(".roster").join("locks").exists());
        assert!(!fixture.root.join("RESULT.md").exists());
        assert!(
            !fixture
                .home
                .join(".roster")
                .join("vault")
                .join(".key")
                .exists()
        );
        fixture.finish()?;
    }
    Ok(())
}

struct AuthEndpoint {
    endpoint: String,
    requests: Arc<Mutex<Vec<(bool, Value)>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), Error>>>,
}

fn receive(stream: &mut TcpStream, expected: &str) -> Result<(bool, Value), Error> {
    stream.set_nonblocking(false).map_err(io)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(io)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(io)?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let (start, length, authorized) = loop {
        let count = stream.read(&mut buffer).map_err(io)?;
        if count == 0 || bytes.len() + count > 4 * 1024 * 1024 {
            return Err(Error::type_error(
                "Owned authenticated HTTP input closed or exceeded its bound",
            ));
        }
        bytes.extend_from_slice(
            buffer
                .get(..count)
                .ok_or_else(|| Error::type_error("Invalid owned HTTP read"))?,
        );
        if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let headers = text(
                bytes
                    .get(..end)
                    .ok_or_else(|| Error::type_error("Owned headers missing"))?,
            )?;
            let fields = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .collect::<Vec<_>>();
            let authorized = fields.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("authorization")
                    && value.trim() == format!("Bearer {expected}")
            });
            let length = fields
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .filter(|length| *length <= 4 * 1024 * 1024)
                .ok_or_else(|| Error::type_error("Owned authenticated HTTP length invalid"))?;
            break (end + 4, length, authorized);
        }
    };
    while bytes.len() < start + length {
        let count = stream.read(&mut buffer).map_err(io)?;
        if count == 0 || bytes.len() + count > 4 * 1024 * 1024 {
            return Err(Error::type_error(
                "Owned authenticated HTTP body closed or exceeded its bound",
            ));
        }
        bytes.extend_from_slice(
            buffer
                .get(..count)
                .ok_or_else(|| Error::type_error("Invalid owned body read"))?,
        );
    }
    let body = github_agent_roster::json::parse(&Text::from(text(
        bytes
            .get(start..start + length)
            .ok_or_else(|| Error::type_error("Owned authenticated body missing"))?,
    )?))?;
    Ok((authorized, body))
}

impl AuthEndpoint {
    fn start(packets: Vec<Value>, key: &str) -> Result<Self, Error> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(io)?;
        listener.set_nonblocking(true).map_err(io)?;
        let endpoint = format!("http://{}/v1", listener.local_addr().map_err(io)?);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let records = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let key = key.to_owned();
        let mut packets = VecDeque::from(packets);
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(600);
            while !stopped.load(Ordering::SeqCst) {
                if Instant::now() >= deadline {
                    return Err(Error::type_error(
                        "Owned authenticated endpoint hang guard expired",
                    ));
                }
                match listener.accept() {
                    Ok((mut stream, address)) => {
                        if !address.ip().is_loopback() {
                            return Err(Error::type_error(
                                "Owned endpoint rejected non-loopback client",
                            ));
                        }
                        let received = receive(&mut stream, &key)?;
                        let authorized = received.0;
                        records
                            .lock()
                            .map_err(|_| Error::type_error("Owned authenticated records poisoned"))?
                            .push(received);
                        let packet = packets.pop_front().ok_or_else(|| {
                            Error::type_error("Owned authenticated packets exhausted")
                        })?;
                        if authorized {
                            let body = packet.json();
                            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).map_err(io)?;
                        } else {
                            stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").map_err(io)?;
                        }
                        stream.flush().map_err(io)?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => return Err(io(error)),
                }
            }
            Ok(())
        });
        Ok(Self {
            endpoint,
            requests,
            stop,
            worker: Some(worker),
        })
    }

    fn finish(mut self) -> Result<Vec<(bool, Value)>, Error> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| Error::type_error("Owned authenticated worker panicked"))??;
        }
        self.requests
            .lock()
            .map_err(|_| Error::type_error("Owned authenticated records poisoned"))
            .map(|records| records.clone())
    }
}

impl Drop for AuthEndpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn actual_local_cli_resolves_required_primary_and_effective_critic_from_native_vault()
-> Result<(), Error> {
    let fixture = Retained::new()?;
    fixture.write("ASK.md", "Update the guide with the requested bounded outcome.\n\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide documents the requested bounded outcome.\n")?;
    let plan = Value::object(vec![
        ("title", Value::string("docs: bounded native vault guide")),
        (
            "files_allowed",
            Value::Array(vec![Value::string("docs/guide.md")]),
        ),
        (
            "acceptance_checks",
            Value::Array(vec![Value::string(
                "The guide documents the requested bounded outcome.",
            )]),
        ),
        ("task_class", Value::string("docs")),
    ]);
    let mut packets = vec![support::packet(
        support::message(&plan.json()),
        "stop",
        "actual-vault-planner",
    )];
    packets.extend(support::docs_script(true));
    let primary = AuthEndpoint::start(packets, PRIMARY)?;
    let critic = AuthEndpoint::start(
        vec![support::packet(
            support::message("{\"defects\":[]}"),
            "stop",
            "actual-vault-critic",
        )],
        CRITIC,
    )?;
    fixture.configure(Some(&primary.endpoint))?;
    required(&fixture)?;
    fixture.critic(&critic.endpoint)?;
    let vault = vault(&fixture)?;
    vault.set("ROSTER_API_KEY", PRIMARY)?;
    vault.set("ROSTER_CRITIC_API_KEY", CRITIC)?;
    fs::write(
        fixture
            .home
            .join(".roster")
            .join("vault")
            .join("not-selected.json"),
        "unselected corruption",
    )
    .map_err(io)?;
    let mut environment = fixture.environment()?;
    put(&mut environment, "ROSTER_API_KEY", Value::string(""));
    let output = fixture.invoke_with(
        &[
            "run",
            "--ask-file",
            "ASK.md",
            "--runtime",
            "builtin",
            "--no-fetch",
        ],
        &environment,
        &fixture.root,
    )?;
    let primary = primary.finish()?;
    let critic = critic.finish()?;
    assert!(
        output.status.success(),
        "{}\n{}",
        text(&output.stdout)?,
        text(&output.stderr)?
    );
    assert!(output.stderr.is_empty());
    let stdout = text(&output.stdout)?;
    assert!(stdout.contains("Mode: verified\n"));
    let worktree = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Worktree: "))
        .map(Path::new)
        .ok_or_else(|| Error::type_error("Native vault CLI has no worktree"))?;
    assert_eq!(
        fs::read_to_string(worktree.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nThe requested bounded outcome is documented.\n"
    );
    let mut recordings = String::new();
    for name in [
        "RESULT.md",
        "REVIEW.md",
        "CONTEXT.md",
        "ASSIGNMENT.md",
        "ESTIMATE.md",
    ] {
        recordings.push_str(&fs::read_to_string(worktree.join(name)).map_err(io)?);
    }
    for entry in fs::read_dir(worktree.join(".roster").join("runs")).map_err(io)? {
        let entry = entry.map_err(io)?;
        if entry.file_type().map_err(io)?.is_file() {
            recordings.push_str(&fs::read_to_string(entry.path()).map_err(io)?);
        }
    }
    recordings.push_str(stdout);
    assert!(!recordings.contains(PRIMARY) && !recordings.contains(CRITIC));
    assert!(recordings.contains("actual-local-coder"));
    assert!(recordings.contains("actual-local-reviewer"));
    assert!(recordings.contains("AI-Run: 1|local|actual-local-coder@-|-|0/-|0|"));
    assert!(recordings.contains("AI-Run: 1|local|actual-local-reviewer@-|m|0/-|0|"));
    assert_eq!(primary.len(), 6);
    assert_eq!(critic.len(), 1);
    assert!(
        primary
            .iter()
            .chain(&critic)
            .all(|(authorized, _)| *authorized)
    );
    assert_eq!(
        critic.first().and_then(|(_, request)| request.get("model")),
        Some(&Value::string("configured-critic-route"))
    );
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn vault_backed_primary_and_critic_keep_actual_served_models_measured_and_unknown_usage()
-> Result<(), Error> {
    let fixture = Retained::new()?;
    let mut measured = support::packet(
        support::message("primary"),
        "stop",
        "actual-vault-primary-measured",
    );
    put(
        &mut measured,
        "usage",
        Value::object(vec![
            ("prompt_tokens", Value::Number(11.0)),
            ("completion_tokens", Value::Number(7.0)),
        ]),
    );
    let mut unknown = support::packet(
        support::message("unknown"),
        "stop",
        "actual-vault-primary-unknown",
    );
    put(&mut unknown, "usage", Value::Null);
    let primary = AuthEndpoint::start(vec![measured, unknown], PRIMARY)?;
    let mut measured = support::packet(
        support::message("critic"),
        "stop",
        "actual-vault-critic-measured",
    );
    put(
        &mut measured,
        "usage",
        Value::object(vec![
            ("prompt_tokens", Value::Number(13.0)),
            ("completion_tokens", Value::Number(5.0)),
        ]),
    );
    let critic = AuthEndpoint::start(vec![measured], CRITIC)?;
    fixture.configure(Some(&primary.endpoint))?;
    required(&fixture)?;
    fixture.critic(&critic.endpoint)?;
    let vault = vault(&fixture)?;
    vault.set("ROSTER_API_KEY", PRIMARY)?;
    vault.set("ROSTER_CRITIC_API_KEY", CRITIC)?;
    let loaded = fixture.loaded()?;
    let credentials = delivery::native_invocation_credentials(&fixture.environment()?, &loaded)?;
    let effective = loaded.config.with_profile(
        loaded
            .critic_profile()
            .ok_or_else(|| Error::type_error("Selected native vault critic missing"))?,
    )?;
    let admission = Admission::default();
    let mut transport =
        github_agent_roster::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
    let request = Value::object(vec![
        ("stream", Value::Boolean(false)),
        (
            "messages",
            Value::Array(vec![Value::object(vec![
                ("role", Value::string("user")),
                ("content", Value::string("owned native vault request")),
            ])]),
        ),
    ]);
    for (config, models, expected) in [
        (
            &loaded.config,
            vec![
                "actual-vault-primary-measured",
                "actual-vault-primary-unknown",
            ],
            vec![
                run::Usage {
                    prompt_tokens: Some(11),
                    completion_tokens: Some(7),
                },
                run::Usage::default(),
            ],
        ),
        (
            &effective,
            vec!["actual-vault-critic-measured"],
            vec![run::Usage {
                prompt_tokens: Some(13),
                completion_tokens: Some(5),
            }],
        ),
    ] {
        let mut chat = github_agent_roster::llm::builtin::BuiltinChat::new(
            config.as_value(),
            &admission,
            github_agent_roster::llm::builtin::Options::default(),
        )?
        .ok_or_else(|| Error::type_error("Native vault chat disabled"))?;
        for (model, usage) in models.into_iter().zip(expected) {
            let response = chat
                .complete(
                    &request,
                    &mut transport,
                    &mut NoObserver,
                    &credentials,
                    github_agent_roster::llm::openai::CallOptions::default(),
                )
                .await
                .map_err(|failure| *failure.error)?;
            assert_eq!(response.model, Text::from(model));
            assert_eq!(response.measured_usage()?, usage);
            assert_eq!(
                chat.last_response().map(|response| &response.model),
                Some(&Text::from(model))
            );
            if usage == run::Usage::default() {
                assert_eq!(
                    chat.last_response().and_then(|response| response.usage),
                    None
                );
            } else {
                assert_eq!(
                    chat.last_response().and_then(|response| response.usage),
                    Some(usage)
                );
            }
        }
    }
    let primary = primary.finish()?;
    let critic = critic.finish()?;
    assert_eq!(primary.len(), 2);
    assert_eq!(critic.len(), 1);
    assert!(
        primary
            .iter()
            .chain(&critic)
            .all(|(authorized, _)| *authorized)
    );
    fixture.finish()
}
