use super::*;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use github_agent_roster::execution::backend::LocalSubmodule;
use github_agent_roster::llm;
use github_agent_roster::llm::control::Observer;
use github_agent_roster::llm::openai::{CallOptions, Chat};
use github_agent_roster::state::redaction;

const SENTINEL: &str = "ordinary-opaque-value-49440";

#[path = "review49440_ownership.rs"]
mod ownership;

#[path = "review49440_transport.rs"]
mod transport;

#[path = "review49440_endpoints.rs"]
mod endpoints;

#[path = "review49440_events.rs"]
mod events;

fn critic(fixture: &Fixture, endpoint: &str, key: Option<&str>) -> Result<(), Error> {
    fixture.critic(endpoint)?;
    let mut value = github_agent_roster::fleet::load(&fixture.root)?.as_value();
    let Value::Object(fields) = &mut value else {
        return Err(Error::type_error("Owned fleet must be an object"));
    };
    let profiles = fields
        .iter_mut()
        .find(|(key, _)| key == &Text::from("profiles"))
        .and_then(|(_, value)| match value {
            Value::Array(profiles) => Some(profiles),
            _ => None,
        })
        .ok_or_else(|| Error::type_error("Owned fleet profiles missing"))?;
    let profile = profiles
        .first_mut()
        .ok_or_else(|| Error::type_error("Owned critic profile missing"))?;
    let Value::Object(fields) = profile else {
        return Err(Error::type_error("Owned critic profile must be an object"));
    };
    fields.retain(|(key, _)| key != &Text::from("api_key_env"));
    if let Some(key) = key {
        fields.push((Text::from("api_key_env"), Value::string(key)));
    }
    let source = github_agent_roster::fleet::format(&value)?;
    fixture.write(".roster/fleet.yml", &source.to_utf8()?)
}

fn plan_packets() -> Vec<Value> {
    let plan = Value::object(vec![
        ("title", Value::string("docs: bounded guide")),
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
    let mut packets = vec![
        support::packet(
            support::message(&plan.json()),
            "stop",
            "actual-review49440-planner",
        ),
        support::packet(
            support::message("{\"defects\":[]}"),
            "stop",
            "actual-review49440-critic",
        ),
    ];
    packets.extend(support::docs_script(true));
    packets
}

#[test]
fn review49440_valid_keyless_critic_uses_effective_fleet_auth_in_the_real_cli() -> Result<(), Error>
{
    let fixture = Fixture::new()?;
    fixture.write("ASK.md", "Update the guide with the requested bounded outcome.\n\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide documents the requested bounded outcome.\n")?;
    let server = support::Server::start(plan_packets())?;
    fixture.configure(Some(&server.endpoint))?;
    critic(&fixture, &server.endpoint, None)?;
    let output = fixture.invoke(&[
        "run",
        "--ask-file",
        "ASK.md",
        "--runtime",
        "builtin",
        "--no-fetch",
    ])?;
    let requests = server.finish()?;
    assert!(
        output.status.success(),
        "{}\n{}",
        text(&output.stdout)?,
        text(&output.stderr)?
    );
    assert!(text(&output.stdout)?.contains("Mode: verified"));
    assert_eq!(requests.len(), 7);
    Ok(())
}

#[test]
fn review49440_selected_nonheuristic_critic_value_cannot_enter_generated_assignment_or_task()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    critic(&fixture, "http://127.0.0.1:1/v1", Some("CRITIC_AUTH"))?;
    fixture.write("ASK.md", &format!("Document {SENTINEL} in the guide.\n\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide documents the requested bounded outcome.\n"))?;
    let mut environment = fixture.environment()?;
    put(&mut environment, "CRITIC_AUTH", Value::string(SENTINEL));
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
    let mut leaked = Vec::new();
    let worktrees = fixture.root.join(".worktrees");
    if worktrees.is_dir() {
        for entry in fs::read_dir(&worktrees).map_err(io)? {
            let root = entry.map_err(io)?.path();
            for name in [
                "ASSIGNMENT.md",
                "TASK.md",
                "ESTIMATE.md",
                "PLAN.md",
                "CONTEXT.md",
                "RESULT.md",
                "REVIEW.md",
            ] {
                if let Some(snapshot) = secure::read(&root.join(name), 65_536)? {
                    if text(&snapshot.bytes)?.contains(SENTINEL) {
                        leaked.push(name.to_owned());
                    }
                }
            }
        }
    }
    assert!(
        leaked.is_empty(),
        "Explicitly selected CRITIC_AUTH value leaked into {leaked:?}"
    );
    assert!(!text(&output.stdout)?.contains(SENTINEL));
    assert!(!text(&output.stderr)?.contains(SENTINEL));
    assert!(
        !output.status.success(),
        "A secret-bearing original Ask must not produce accepted artifacts"
    );
    Ok(())
}

#[test]
fn review49440_all_selected_values_share_native_artifact_and_filesystem_error_protection()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.configure(Some("http://127.0.0.1:1/v1"))?;
    critic(&fixture, "http://127.0.0.1:2/v1", Some("CRITIC_AUTH"))?;
    let loaded = fixture.loaded()?;
    let credentials = delivery::invocation_credentials(
        &Value::object(vec![
            (
                "ROSTER_API_KEY",
                Value::string("primary-ordinary-value-49440"),
            ),
            ("CRITIC_AUTH", Value::string(SENTINEL)),
            ("NOT_SELECTED", Value::string("unselected-ordinary-value")),
        ]),
        &loaded,
    )?;
    let selected = credentials
        .environment
        .as_ref()
        .ok_or_else(|| Error::type_error("No selected typed credentials"))?;
    assert!(selected.get("NOT_SELECTED").is_none());
    let mut rules = redaction::Config::default();
    rules.add_selected_credentials(selected)?;
    let body = Text::from(
        format!("primary-ordinary-value-49440 {SENTINEL} unselected-ordinary-value").as_str(),
    );
    let safe = redaction::evidence(&body, &rules).to_utf8()?;
    assert!(!safe.contains("primary-ordinary-value-49440") && !safe.contains(SENTINEL));
    assert!(safe.contains("unselected-ordinary-value"));
    let mut writer = github_agent_roster::planning::artifacts::Writer::new(
        &fixture.root,
        false,
        rules.clone(),
        None,
    )?;
    for name in ["TASK.md", "RECIPE.yml", "ESTIMATE.md"] {
        assert!(
            writer.write(name, &body).is_err(),
            "Selected value accepted by native planning artifact writer"
        );
        assert!(!fixture.root.join(name).exists());
    }
    let mut writer =
        github_agent_roster::planning::artifacts::Writer::new(&fixture.root, true, rules, None)?;
    assert!(writer.write("PLAN.md", &body).is_err());
    assert!(!fixture.root.join("PLAN.md").exists());
    let path = fixture.root.join(SENTINEL);
    fs::write(&path, "owned non-directory").map_err(io)?;
    let actual = secure::private_directory(&path, false)
        .err()
        .ok_or_else(|| Error::type_error("Owned filesystem fault did not occur"))?;
    assert!(actual.message.contains(SENTINEL));
    let safe = delivery::safe_invocation_error(&actual, &loaded, &credentials)?;
    assert!(!safe.message.contains(SENTINEL));
    assert_eq!(safe.name, actual.name);
    assert_eq!(safe.code, actual.code);
    Ok(())
}

struct CallbackError {
    calls: usize,
    message: String,
}

impl Observer for CallbackError {
    async fn emit(&mut self, _: Value) -> Result<(), Error> {
        self.calls += 1;
        Err(Error::new("OwnedCallback", self.message.clone()))
    }
}

fn request() -> Value {
    Value::object(vec![
        (
            "messages",
            Value::Array(vec![Value::object(vec![
                ("role", Value::string("user")),
                ("content", Value::string("Owned bounded response")),
            ])]),
        ),
        ("stream", Value::Boolean(true)),
    ])
}

fn optional_config(fixture: &Fixture, endpoint: &str) -> Result<config::Loaded, Error> {
    fixture.configure(Some(endpoint))?;
    let path = fixture.root.join(".roster").join("config.yml");
    let source = fs::read_to_string(path)
        .map_err(io)?
        .replace("\r\n", "\n")
        .replacen(
            "llm:\n",
            "llm:\n  api_key_optional: true\n  request_timeout_ms: 50\n",
            1,
        );
    fixture.write(".roster/config.yml", &source)?;
    fixture.loaded()
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_real_observer_failure_cannot_leak_a_selected_critic_value() -> Result<(), Error>
{
    let fixture = Fixture::new()?;
    let server = support::Server::pending()?;
    let _ = optional_config(&fixture, &server.endpoint)?;
    critic(&fixture, &server.endpoint, Some("CRITIC_AUTH"))?;
    let loaded = fixture.loaded()?;
    let credentials = delivery::invocation_credentials(
        &Value::object(vec![("CRITIC_AUTH", Value::string(SENTINEL))]),
        &loaded,
    )?;
    let chat = Chat::new(loaded.config.as_value())?
        .ok_or_else(|| Error::type_error("Configured callback chat unavailable"))?;
    let mut transport = llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
    let mut observer = CallbackError {
        calls: 0,
        message: format!("Owned callback observed {SENTINEL}"),
    };
    let failure = chat
        .complete(
            &request(),
            &mut transport,
            &mut observer,
            &credentials,
            CallOptions::default(),
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Actual observer failure unexpectedly passed"))?;
    assert!(observer.calls > 0);
    let error = delivery::safe_invocation_error(&failure.error, &loaded, &credentials)?;
    let _ = server.finish()?;
    assert!(
        !error.message.contains(SENTINEL),
        "Explicitly selected nonheuristic credential escaped actual callback diagnostics"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_actual_owned_http_timeout_cannot_leak_endpoint_authority() -> Result<(), Error>
{
    let fixture = Fixture::new()?;
    let server = support::Server::pending()?;
    let endpoint = server.endpoint.clone();
    let loaded = optional_config(&fixture, &server.endpoint)?;
    let credentials = delivery::invocation_credentials(&Value::object(Vec::new()), &loaded)?;
    let chat = Chat::new(loaded.config.as_value())?
        .ok_or_else(|| Error::type_error("Configured timeout chat unavailable"))?;
    let mut transport = llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
    let mut observer = NoObserver;
    let failure = chat
        .complete(
            &request(),
            &mut transport,
            &mut observer,
            &credentials,
            CallOptions::default(),
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Owned incomplete HTTP body unexpectedly passed"))?;
    let requests = server.finish()?;
    assert!(!requests.is_empty());
    assert_eq!(failure.error.code, Some("ROSTER_LLM_TIMEOUT"));
    let authority = url::Url::parse(&endpoint)
        .map_err(|_| Error::type_error("Owned endpoint invalid"))?
        [url::Position::BeforeHost..url::Position::AfterPort]
        .to_owned();
    let error = delivery::safe_invocation_error(&failure.error, &loaded, &credentials)?;
    assert!(
        !error.message.contains(&authority),
        "Actual timeout leaked configured host authority"
    );
    Ok(())
}

struct GitProxy {
    image: PathBuf,
    control: PathBuf,
}

fn proxy(fixture: &Fixture, phase: &str) -> Result<GitProxy, Error> {
    let root = fixture.container.join("git-proxy");
    secure::private_directory(&root, true)?;
    let control = root.join("control");
    secure::private_directory(&control, true)?;
    fs::write(control.join("selected"), phase).map_err(io)?;
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"owned-review49440-git\"\nversion = \"0.1.0\"\nedition = \"2024\"\nrust-version = \"1.85\"\n[[bin]]\nname = \"owned-review49440-git\"\npath = \"main.rs\"\n").map_err(io)?;
    fs::write(
        root.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"owned-review49440-git\"\nversion = \"0.1.0\"\n",
    )
    .map_err(io)?;
    fs::write(
        root.join("main.rs"),
        include_str!("../fixtures/review49440_git.rs"),
    )
    .map_err(io)?;
    let cargo = std::env::var_os("CARGO")
        .ok_or_else(|| Error::type_error("Owned proxy needs invoking Cargo"))?;
    let mut environment = support::minimal()?;
    for name in ["CARGO_HOME", "RUSTUP_HOME", "RUSTFLAGS", "RUSTUP_TOOLCHAIN"] {
        if let Ok(value) = std::env::var(name) {
            environment.insert(name.to_owned(), value);
        }
    }
    if let Ok(toolchain) = std::env::var("ROSTER_TEST_CARGO_TOOLCHAIN") {
        environment.insert("RUSTUP_TOOLCHAIN".to_owned(), toolchain);
    }
    let target = fixture.container.join("git-proxy-target");
    for args in [
        vec!["fmt", "--all"],
        vec![
            "clippy",
            "--quiet",
            "--all-targets",
            "--locked",
            "--offline",
            "--",
            "-D",
            "warnings",
        ],
        vec!["build", "--quiet", "--locked", "--offline"],
    ] {
        let output = Command::new(&cargo)
            .current_dir(&root)
            .env_clear()
            .envs(&environment)
            .env("CARGO_TARGET_DIR", &target)
            .env("CARGO_NET_OFFLINE", "true")
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env("REVIEW49440_REAL_GIT", &fixture.git)
            .env("REVIEW49440_CONTROL", &control)
            .args(args)
            .output()
            .map_err(io)?;
        if !output.status.success() {
            return Err(Error::new("FixtureBuild", text(&output.stderr)?.to_owned()));
        }
    }
    Ok(GitProxy {
        image: target.join("debug").join("owned-review49440-git.exe"),
        control,
    })
}

async fn ready(path: &Path) -> Result<u32, Error> {
    tokio::time::timeout(Duration::from_secs(600), async {
        loop {
            if let Some(snapshot) = secure::read(path, 4096)? {
                if let Ok(value) =
                    github_agent_roster::json::parse(&Text::from(text(&snapshot.bytes)?))
                {
                    if let Some(pid) = value.get("pid").and_then(Value::as_number).filter(|pid| {
                        pid.fract() == 0.0 && (1.0..=f64::from(u32::MAX)).contains(pid)
                    }) {
                        return Ok::<_, Error>(pid as u32);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| {
        Error::new(
            "FixtureReadiness",
            format!(
                "Actual owned Git readiness not observed for {}",
                path.display()
            ),
        )
    })?
}

fn module(fixture: &Fixture) -> Result<PathBuf, Error> {
    let module = fixture.container.join("module");
    secure::private_directory(&module, true)?;
    fixture.git_at(&module, &["init", "--quiet", "--initial-branch=main"])?;
    fs::write(module.join("value.txt"), "actual approved dependency\n").map_err(io)?;
    fixture.git_at(&module, &["add", "value.txt"])?;
    fixture.git_at(
        &module,
        &["commit", "--quiet", "-m", "Owned reviewed dependency"],
    )?;
    fixture.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        "--",
        module
            .to_str()
            .ok_or_else(|| Error::type_error("Module path must be Unicode"))?,
        "vendor/owned",
    ])?;
    fixture.git(&["add", ".gitmodules", "vendor/owned"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned gitlink"])?;
    Ok(module)
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_actual_dropped_owned_git_fault_cannot_release_new_tree_run_lock()
-> Result<(), Error> {
    use winsafe::{HPROCESS, co, prelude::*};
    let mut released = Vec::new();
    for phase in ["clone", "checkout"] {
        let fixture = Fixture::new()?;
        let module = module(&fixture)?;
        let proxy = proxy(&fixture, phase)?;
        let loaded = fixture.loaded()?;
        let (mut host, mut approval) = fixture.host()?;
        host.git = proxy.image;
        approval.submodules = vec![LocalSubmodule {
            path: "vendor/owned".to_owned(),
            source: module,
        }];
        let paths = paths::repository_paths(&fixture.root)?;
        let git = start::Git::new(host, paths.common_dir.join("review49440-no-hooks"))?;
        let source = CancellationSource::default();
        let token = source.token();
        let repository = start::Repository::inspect(&fixture.root, &git, &token).await?;
        let selected = start::resolve(
            &repository,
            &git,
            &start::Options {
                base: "current".to_owned(),
                sync: start::Sync::Offline,
            },
            &start::FetchPolicy::Deny,
            &token,
        )
        .await?;
        let mut preparation = start::Preparation::default();
        let mut prepare = Box::pin(start::prepare_local(
            &repository,
            &git,
            &loaded.config,
            selected,
            &approval,
            redaction::Config::default(),
            token.clone(),
            &mut preparation,
        ));
        let phase_ready = proxy.control.join(format!("{phase}.ready"));
        tokio::select! {
            result = ready(&phase_ready) => { result?; }
            result = &mut prepare => return Err(Error::new("FixtureError", format!("Preparation ended before actual {phase}: {:?}", result.err()))),
        }
        let roots = fs::read_dir(fixture.root.join(".worktrees"))
            .map_err(io)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(io)?;
        let [root] = roots.as_slice() else {
            return Err(Error::type_error("Actual new worktree not uniquely found"));
        };
        let lock = root.join(".roster").join("locks").join("run.lock");
        assert!(
            lock.is_file(),
            "Real new-tree RootLease missing before {phase}"
        );
        let probe_source = CancellationSource::default();
        let probe_token = probe_source.token();
        let mut probe = Box::pin(git.run(&fixture.root, &["--review49440-pending"], &probe_token));
        let probe_ready = proxy.control.join("probe.ready");
        let pid = tokio::select! {
            result = ready(&probe_ready) => result?,
            result = &mut probe => return Err(Error::new("FixtureError", format!("Owned probe ended before readiness: {:?}", result.err()))),
        };
        let handle = HPROCESS::OpenProcess(
            co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
            false,
            pid,
        )
        .map_err(|error| Error::new("FixtureError", error.to_string()))?;
        probe_source.cancel(github_agent_roster::cancel::Reason::Cancelled)?;
        drop(probe);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if handle
                    .WaitForSingleObject(Some(0))
                    .map_err(|error| Error::new("FixtureError", error.to_string()))?
                    == co::WAIT::OBJECT_0
                {
                    return Ok::<_, Error>(());
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .map_err(|_| Error::type_error("Actual dropped owned probe did not stop"))??;
        assert!(!git.cleanup_certain());
        fs::write(
            proxy.control.join(format!("{phase}.release")),
            "release known phase",
        )
        .map_err(io)?;
        let failure = prepare
            .await
            .err()
            .ok_or_else(|| Error::type_error("Uncertain Git preparation unexpectedly passed"))?;
        assert_eq!(failure.code, Some("ROSTER_PROCESS_CLEANUP"));
        assert_eq!(preparation.root(), Some(root.as_path()));
        assert_eq!(
            preparation.close().await.err().and_then(|error| error.code),
            Some("ROSTER_PROCESS_CLEANUP")
        );
        if !lock.is_file() {
            released.push(phase);
        }
    }
    assert!(
        released.is_empty(),
        "Actual owned {released:?} faults released run.lock while Git cleanup remained unknown"
    );
    Ok(())
}
