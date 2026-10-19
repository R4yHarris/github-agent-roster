#![cfg(windows)]

#[path = "support/local_delivery.rs"]
mod support;

use std::fs;
use std::path::Path;

use github_agent_roster::Error;
use github_agent_roster::cancel::CancellationSource;
use github_agent_roster::config::{self, Source};
use github_agent_roster::delivery::{self, Input, Status};
use github_agent_roster::execution::{backend, tools};
use github_agent_roster::json::Value;
use github_agent_roster::llm::{admission::Admission, control::NoObserver, openai::Credentials};
use github_agent_roster::local_cli;
use github_agent_roster::paths;
use github_agent_roster::state::secure;
use github_agent_roster::text::Text;
use github_agent_roster::{run, start};

use support::{Fixture, io, put, text};

#[path = "support/local_start.rs"]
mod start_controls;

#[path = "support/local_cancellation.rs"]
mod cancellation_controls;

#[path = "support/local_binary_controls.rs"]
mod binary_controls;

#[path = "support/local_config_controls.rs"]
mod config_controls;

#[path = "support/local_handoff_controls.rs"]
mod handoff_controls;

#[path = "support/review49440.rs"]
mod review49440;

#[path = "support/local_vault.rs"]
mod vault_controls;

#[path = "support/local_continuations.rs"]
mod continuation_controls;

#[test]
fn invocation_defaults_are_validated_and_private_target_precedes_installation_without_mutation()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let installation = fixture.container.join("installation");
    secure::private_directory(&installation.join(".roster"), true)?;
    let install_config = config::format(config::defaults()?.as_value())?;
    secure::write_exclusive(
        &installation.join(".roster").join("config.yml"),
        install_config.to_utf8()?.as_bytes(),
    )?;
    let cwd = std::env::current_dir().map_err(io)?;
    let before = std::env::var_os("ROSTER_LLM_MODEL");
    let mut environment = Value::object(Vec::new());
    put(
        &mut environment,
        "ROSTER_LLM_MODEL",
        Value::string("host-route-override"),
    );
    put(
        &mut environment,
        "ROSTER_START_SYNC",
        Value::string("offline"),
    );
    let loaded = config::load(&installation, &fixture.root, &environment)?;
    assert_eq!(
        loaded.root,
        paths::repository_paths(&fixture.root)?.repository
    );
    assert!(matches!(loaded.source, Source::Private(_)));
    assert_eq!(
        loaded
            .config
            .as_value()
            .get("llm")
            .and_then(|value| value.get("model")),
        Some(&Value::string("host-route-override"))
    );
    assert_eq!(loaded.retention.get("optOut"), Some(&Value::Boolean(false)));
    fs::remove_file(fixture.root.join(".roster").join("config.yml")).map_err(io)?;
    let selected = config::load(&installation, &fixture.root, &Value::object(Vec::new()))?;
    assert_eq!(selected.root, installation);
    fs::remove_file(installation.join(".roster").join("config.yml")).map_err(io)?;
    let defaults = config::load(&installation, &fixture.root, &Value::object(Vec::new()))?;
    assert_eq!(defaults.source, Source::Defaults);
    assert_eq!(defaults.config, config::defaults()?);
    assert_eq!(std::env::current_dir().map_err(io)?, cwd);
    assert_eq!(std::env::var_os("ROSTER_LLM_MODEL"), before);
    assert!(!fixture.root.join(".roster").join("locks").exists());
    Ok(())
}

#[test]
fn invalid_private_config_never_falls_back_and_blank_or_invalid_overrides_never_become_defaults()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.write(".roster/config.yml", "schema: 900\n")?;
    let error = config::load(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &fixture.root,
        &Value::object(Vec::new()),
    )
    .err()
    .ok_or_else(|| Error::type_error("Invalid private config was accepted"))?;
    assert!(error.message.contains("Invalid roster config"));
    fixture.configure(None)?;
    for name in [
        "ROSTER_LLM_BASE_URL",
        "ROSTER_LLM_MODEL",
        "ROSTER_LLM_API_KEY_ENV",
        "ROSTER_LLM_REQUEST_TIMEOUT_MS",
        "ROSTER_TASK_TIMEOUT_MS",
        "ROSTER_START_BASE",
        "ROSTER_START_SYNC",
    ] {
        let environment = Value::object(vec![(name, Value::string(" "))]);
        let error = config::load(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &fixture.root,
            &environment,
        )
        .err()
        .ok_or_else(|| Error::type_error("Blank override was accepted"))?;
        assert!(error.message.contains(name));
    }
    for (name, value) in [
        (
            "ROSTER_LLM_BASE_URL",
            "https://test-only-private-api-key@not-a-service.invalid",
        ),
        (
            "ROSTER_LLM_MODEL",
            "test-only-private-api-key forbidden spaces",
        ),
        ("ROSTER_LLM_API_KEY_ENV", "GITHUB_APP_PRIVATE_KEY_PATH"),
        ("ROSTER_LLM_REQUEST_TIMEOUT_MS", "0"),
        ("ROSTER_TASK_TIMEOUT_MS", "0"),
        ("ROSTER_START_BASE", "HEAD~1"),
        ("ROSTER_START_SYNC", "automatic"),
        ("ROSTER_RETENTION_OPT_OUT", ""),
        ("ROSTER_RETENTION_WINDOW_MS_MACHINE", "0"),
    ] {
        let environment = Value::object(vec![(name, Value::string(value))]);
        let error = config::load(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &fixture.root,
            &environment,
        )
        .err()
        .ok_or_else(|| Error::type_error("Invalid override was accepted"))?;
        assert!(!error.message.contains("test-only-private-api-key"));
    }
    assert!(!fixture.root.join(".roster").join("locks").exists());
    Ok(())
}

#[test]
fn private_config_directory_and_hardlinks_are_not_adopted() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    let path = fixture.root.join(".roster").join("config.yml");
    let other = fixture.container.join("private-copy.yml");
    fs::hard_link(&path, &other).map_err(io)?;
    assert!(
        fixture
            .loaded()
            .is_err_and(|error| error.message.contains("single-link"))
    );
    fs::remove_file(&other).map_err(io)?;
    fs::remove_file(&path).map_err(io)?;
    fs::create_dir(&path).map_err(io)?;
    assert!(fixture.loaded().is_err());
    Ok(())
}

#[test]
fn init_is_offline_exclusive_idempotent_and_never_changes_policy_or_workflows() -> Result<(), Error>
{
    let fixture = Fixture::new()?;
    fixture.write("agent-policy.yml", "human-owned fixture policy\n")?;
    fixture.write(
        ".github/workflows/check.yml",
        "human-owned fixture workflow\n",
    )?;
    let environment = Value::object(vec![
        ("PATH", Value::string("")),
        ("GITHUB_APP_ID", Value::string("test-only-private-app-id")),
    ]);
    let first = fixture.invoke_with(&["init"], &environment, &fixture.root)?;
    assert!(first.status.success(), "{}", text(&first.stderr)?);
    assert!(first.stderr.is_empty());
    assert_eq!(
        text(&first.stdout)?,
        format!(
            "Created: {}\nCreated: {}\nPolicy: agent-policy.yml unchanged (human-owned)\n",
            fixture.root.join("roster.config.example.yml").display(),
            fixture.root.join("ROSTER-POLICY-NOTE.md").display()
        )
    );
    fixture.write("roster.config.example.yml", "human example bytes\n")?;
    let second = fixture.invoke_with(&["init"], &environment, &fixture.root)?;
    assert!(second.status.success());
    assert!(text(&second.stdout)?.starts_with("Kept existing: "));
    assert_eq!(
        fs::read_to_string(fixture.root.join("roster.config.example.yml")).map_err(io)?,
        "human example bytes\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("agent-policy.yml")).map_err(io)?,
        "human-owned fixture policy\n"
    );
    assert_eq!(
        fs::read_to_string(
            fixture
                .root
                .join(".github")
                .join("workflows")
                .join("check.yml")
        )
        .map_err(io)?,
        "human-owned fixture workflow\n"
    );
    assert!(!text(&second.stdout)?.contains("test-only-private-app-id"));
    Ok(())
}

#[test]
fn every_invalid_conflicting_or_unavailable_flag_fails_before_any_file_or_git_effect()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let before = fixture.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    for flags in [
        vec!["run"],
        vec!["run", "--seat", "coder"],
        vec!["run", "--seat", "reviewer", "--runtime", "builtin"],
        vec![
            "run",
            "--ask-file",
            "ASK.md",
            "--seat",
            "coder",
            "--runtime",
            "builtin",
        ],
        vec![
            "run",
            "--ask-file",
            "ASK.md",
            "--runtime",
            "builtin",
            "--fetch",
            "--no-fetch",
        ],
        vec![
            "run",
            "--ask-file",
            "ASK.md",
            "--runtime",
            "builtin",
            "--base",
            "HEAD~1",
        ],
        vec![
            "run",
            "--seat",
            "coder",
            "--runtime",
            "builtin",
            "--no-fetch",
        ],
        vec![
            "run",
            "--seat",
            "coder",
            "--runtime",
            "builtin",
            "--runtime",
            "builtin",
        ],
        vec!["run", "--issue", "42"],
        vec![
            "run",
            "--ask-file",
            "ASK.md",
            "--runtime",
            "builtin",
            "--publish",
        ],
        vec!["doctor", "--warm"],
        vec!["init", "--force"],
        vec![
            "run",
            "--seat",
            "coder",
            "--runtime",
            "builtin",
            "--skip-review",
        ],
    ] {
        let result = fixture.invoke(&flags)?;
        assert!(!result.status.success(), "{flags:?}");
        assert!(result.stdout.is_empty(), "{flags:?}");
        assert!(!result.stderr.is_empty(), "{flags:?}");
        assert_eq!(
            fixture.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?,
            before
        );
        assert!(!fixture.root.join(".worktrees").exists());
        assert!(!fixture.root.join(".roster").join("locks").exists());
        assert!(!fixture.root.join(".roster").join("runs").exists());
        assert!(!fixture.root.join("roster.config.example.yml").exists());
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn actual_existing_task_cli_stub_is_unverified_and_preserves_products_without_requests_or_tests()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.task(
        &["src/app.mjs"],
        "Nonzero input increments while zero input is preserved.",
    )?;
    let before = fs::read(fixture.root.join("src").join("app.mjs")).map_err(io)?;
    let mut environment = fixture.environment()?;
    put(&mut environment, "PATH", Value::string(""));
    let result = fixture.invoke_with(
        &["run", "--seat", "coder", "--runtime", "builtin"],
        &environment,
        &fixture.root,
    )?;
    assert!(result.status.success(), "{}", text(&result.stderr)?);
    assert!(result.stderr.is_empty());
    let stdout = text(&result.stdout)?;
    assert!(stdout.contains("Mode: stub\n"), "{stdout}");
    assert!(stdout.contains("no model calls, product edits or tests; no passing review."));
    assert!(stdout.contains("session-only; durable history was not recorded"));
    assert_eq!(
        fs::read(fixture.root.join("src").join("app.mjs")).map_err(io)?,
        before
    );
    assert!(
        fs::read_to_string(fixture.root.join("RESULT.md"))
            .map_err(io)?
            .contains("stub")
    );
    assert!(
        !fs::read_to_string(fixture.root.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: pass")
    );
    assert!(fixture.root.join("ESTIMATE.md").is_file());
    assert!(fixture.root.join("CONTEXT.md").is_file());
    assert!(
        !fixture
            .root
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    assert!(!fixture.root.join(".roster").join("memory").exists());
    Ok(())
}

#[cfg(windows)]
#[test]
fn actual_doctor_checks_local_config_and_submodule_without_endpoint_or_app_value_output()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.configure(Some("http://127.0.0.1:1/v1"))?;
    let mut environment = fixture.environment()?;
    put(
        &mut environment,
        "GITHUB_APP_ID",
        Value::string("test-only-private-app-id"),
    );
    put(
        &mut environment,
        "GITHUB_APP_PRIVATE_KEY_PATH",
        Value::string("test-only-private-pem-path"),
    );
    put(
        &mut environment,
        "ROSTER_API_KEY",
        Value::string("test-only-private-api-key"),
    );
    let result = fixture.invoke_with(&["doctor"], &environment, &fixture.root)?;
    assert!(result.status.success(), "{}", text(&result.stderr)?);
    let output = text(&result.stdout)?;
    assert!(output.contains("Doctor: offline"));
    assert!(output.contains("initialized submodule at the reviewed gitlink"));
    for hidden in ["test-only-private", "configured-route", "127.0.0.1:1"] {
        assert!(!output.contains(hidden));
        assert!(!text(&result.stderr)?.contains(hidden));
    }
    assert!(!fixture.root.join(".roster").join("locks").exists());
    assert!(!fixture.root.join(".roster").join("runs").exists());
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn owned_preplan_lease_transfers_atomically_into_boundary_without_double_lock_or_unowned_gap()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested outcome.",
    )?;
    let source = CancellationSource::default();
    let (host, _) = fixture.host()?;
    let lease = backend::RootLease::open(
        &fixture.root,
        "owned-root-lease",
        host.clone(),
        Default::default(),
        source.token(),
    )
    .await?;
    let lock_path = lease.ownership().path.clone();
    let lock_bytes = fs::read(&lock_path).map_err(io)?;
    let other = backend::RootLease::open(
        &fixture.root,
        "foreign-writer",
        host.clone(),
        Default::default(),
        source.token(),
    )
    .await;
    assert_eq!(
        other.err().and_then(|error| error.code),
        Some("E_LOCK_HELD")
    );
    let mut boundary =
        tools::Boundary::from_root_lease(lease, fixture.assignment()?, tools::Options::default())
            .await?;
    assert_eq!(fs::read(&lock_path).map_err(io)?, lock_bytes);
    let other = backend::RootLease::open(
        &fixture.root,
        "foreign-after-handoff",
        host,
        Default::default(),
        source.token(),
    )
    .await;
    assert_eq!(
        other.err().and_then(|error| error.code),
        Some("E_LOCK_HELD")
    );
    boundary.close().await?;
    assert!(!lock_path.exists());
    Ok(())
}

#[test]
fn help_only_advertises_actual_native_commands_and_keeps_unavailable_outer_runtime_explicit()
-> Result<(), Error> {
    let parsed = local_cli::parse(&[
        "run".to_owned(),
        "--seat".to_owned(),
        "coder".to_owned(),
        "--runtime".to_owned(),
        "builtin".to_owned(),
    ])?;
    assert!(matches!(parsed, local_cli::Command::Run(_)));
    assert!(local_cli::HELP.contains("roster run --seat coder --runtime builtin"));
    assert!(
        local_cli::HELP.contains("publication, resume, routing and review repair are unavailable")
    );
    assert!(!local_cli::HELP.contains("src/cli.mjs"));
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn configured_library_local_task_drives_actual_owned_coder_fresh_review_and_measured_zero_usage()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.configure(Some("http://127.0.0.1:1/v1"))?;
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    let loaded = fixture.loaded()?;
    let (host, approval) = fixture.host()?;
    let mut transport = support::Scripted::new(support::docs_script(true));
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let admission = Admission::default();
    let source = CancellationSource::default();
    let mut driver = delivery::Driver {
        transport: &mut transport,
        observer: &mut observer,
        credentials: &credentials,
        admission: &admission,
    };
    let mut ownership = delivery::Ownership::default();
    let result = driver
        .run_local(
            delivery::Options {
                cwd: &fixture.root,
                installation: Path::new(env!("CARGO_MANIFEST_DIR")),
                loaded: &loaded,
                input: Input::ExistingTask,
                host,
                approval,
                start: start::Options::resolve(&loaded.config, None, None)?,
                fetch: start::FetchPolicy::Deny,
                skip_review: false,
            },
            source.token(),
            &mut ownership,
        )
        .await?;
    assert_eq!(result.status(), Status::Verified, "{:?}", result.failure);
    assert!(result.succeeded());
    assert_eq!(
        result
            .coder_run
            .as_ref()
            .map(|run| run.metrics().model.as_str()),
        Some("actual-local-coder")
    );
    assert_eq!(
        result
            .reviewer_run
            .as_ref()
            .map(|run| run.metrics().model.as_str()),
        Some("actual-local-reviewer")
    );
    for run in [result.coder_run.as_ref(), result.reviewer_run.as_ref()]
        .into_iter()
        .flatten()
    {
        assert_eq!(
            run.metrics().usage,
            run::Usage {
                prompt_tokens: Some(0),
                completion_tokens: Some(0)
            }
        );
    }
    assert_eq!(transport.requests.len(), 5);
    assert!(transport.responses.is_empty());
    for request in transport.requests.iter().skip(3) {
        assert!(request.get("tools").is_none());
    }
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nThe requested bounded outcome is documented.\n"
    );
    assert!(
        fs::read_to_string(fixture.root.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: pass")
    );
    let log = fs::read_to_string(&result.log).map_err(io)?;
    for source in [
        "configured-route-not-a-response",
        "http://127.0.0.1:1",
        "Before.",
        "requested bounded outcome",
        "actual-local-coder",
    ] {
        assert!(!log.contains(source));
    }
    assert!(log.contains("\"prompt_tokens\":0"));
    assert!(!fixture.root.join(".roster").join("memory").exists());
    assert!(
        !fixture
            .root
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn actual_configured_task_cli_uses_owned_loopback_sse_real_edits_matching_node_checks_and_readonly_review()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.task(
        &["src/app.mjs", "tests/app.test.mjs"],
        "Nonzero input increments while zero input is preserved.",
    )?;
    let before = fs::read_to_string(fixture.root.join("tests").join("app.test.mjs")).map_err(io)?;
    let after =
        format!("{before}test('increments nonzero input', () => assert.equal(answer(2), 3));\n");
    let server = support::Server::start(support::code_script(
        "src/app.mjs",
        "tests/app.test.mjs",
        &before,
        &after,
    ))?;
    fixture.configure(Some(&server.endpoint))?;
    let result = fixture.invoke(&["run", "--seat", "coder", "--runtime", "builtin"])?;
    assert!(
        result.status.success(),
        "{}\n{}",
        text(&result.stdout)?,
        text(&result.stderr)?
    );
    assert!(result.stderr.is_empty());
    assert!(text(&result.stdout)?.contains("Mode: verified\n"));
    assert!(
        fs::read_to_string(fixture.root.join("src").join("app.mjs"))
            .map_err(io)?
            .contains("return value + 1")
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("tests").join("app.test.mjs")).map_err(io)?,
        after
    );
    let body = fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?;
    assert!(body.contains("Checks: PASS"), "{body}");
    assert!(body.contains("actual-local-coder"));
    assert!(body.contains("AI-Run"));
    assert!(
        fs::read_to_string(fixture.root.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: pass")
    );
    let requests = server.finish()?;
    assert_eq!(requests.len(), 7);
    assert!(
        requests
            .iter()
            .take(4)
            .any(|request| request.get("tools").is_some())
    );
    for request in requests.iter().skip(4) {
        assert!(request.get("tools").is_none());
    }
    assert!(
        !fixture
            .root
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn actual_configured_ask_file_cli_plans_critiques_edits_and_reviews_in_one_isolated_owned_worktree()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.write("ASK.md", "Update the guide to document the requested bounded outcome.\n\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide documents the requested bounded outcome.\n")?;
    let mut responses = vec![
        support::packet(
            support::message(
                &Value::object(vec![
                    ("title", Value::string("docs: bounded guide")),
                    (
                        "acceptance_checks",
                        Value::Array(vec![Value::string(
                            "The guide documents the requested bounded outcome.",
                        )]),
                    ),
                    (
                        "files_allowed",
                        Value::Array(vec![Value::string("docs/guide.md")]),
                    ),
                    ("task_class", Value::string("docs")),
                ])
                .json(),
            ),
            "stop",
            "actual-local-planner",
        ),
        support::packet(
            support::message("{\"defects\":[]}"),
            "stop",
            "actual-local-critic",
        ),
    ];
    responses.extend(support::docs_script(true));
    let server = support::Server::start(responses)?;
    fixture.configure(Some(&server.endpoint))?;
    fixture.critic(&server.endpoint)?;
    let mut environment = fixture.environment()?;
    put(
        &mut environment,
        "ROSTER_CRITIC_API_KEY",
        Value::string("test-only-private-api-key"),
    );
    let original_head = fixture.git(&["rev-parse", "HEAD"])?;
    let original_status =
        fixture.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    let result = fixture.invoke_with(
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
    assert!(
        result.status.success(),
        "{}\n{}",
        text(&result.stdout)?,
        text(&result.stderr)?
    );
    let output = text(&result.stdout)?;
    assert!(output.contains("Mode: verified\n"), "{output}");
    let worktree = output
        .lines()
        .find_map(|line| line.strip_prefix("Worktree: "))
        .ok_or_else(|| Error::type_error("Actual CLI returned no worktree"))?;
    let worktree = Path::new(worktree);
    assert!(worktree.starts_with(fixture.root.join(".worktrees")));
    assert_ne!(worktree, fixture.root);
    for file in [
        "RECIPE.yml",
        "TASK.md",
        "ESTIMATE.md",
        "ASSIGNMENT.md",
        "CONTEXT.md",
        "RESULT.md",
        "REVIEW.md",
    ] {
        assert!(worktree.join(file).is_file(), "{file}");
    }
    assert!(
        fixture
            .git_at(worktree, &["branch", "--show-current"])?
            .starts_with("agents/local-")
    );
    assert_eq!(fixture.git(&["rev-parse", "HEAD"])?, original_head);
    assert_eq!(
        fixture.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?,
        original_status
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nThe requested bounded outcome is documented.\n"
    );
    assert!(!fixture.root.join(".roster").join("memory").exists());
    assert!(
        !worktree
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    let requests = server.finish()?;
    assert_eq!(requests.len(), 7);
    assert_eq!(
        requests.get(1).and_then(|value| value.get("model")),
        Some(&Value::string("configured-critic-route"))
    );
    assert_eq!(
        requests.get(2).and_then(|value| value.get("model")),
        Some(&Value::string("configured-route-not-a-response"))
    );
    assert!(
        !output.contains("test-only-private-api-key")
            && !output.contains("configured-critic-route")
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn actual_review_failure_or_invalid_review_never_returns_a_green_local_delivery_exit()
-> Result<(), Error> {
    for invalid in [false, true] {
        let fixture = Fixture::new()?;
        fixture.task(
            &["docs/guide.md"],
            "The guide documents the requested bounded outcome.",
        )?;
        let mut responses = support::docs_script(false);
        if invalid {
            let invalid_review = support::review(Value::object(vec![
                ("verdict", Value::string("pass")),
                ("reasons", Value::Array(Vec::new())),
                ("security_notes", Value::Array(Vec::new())),
                ("checks", Value::Array(Vec::new())),
            ]));
            let last = responses
                .last_mut()
                .ok_or_else(|| Error::type_error("Review response missing"))?;
            *last = invalid_review.clone();
            responses.push(invalid_review);
        }
        let server = support::Server::start(responses)?;
        fixture.configure(Some(&server.endpoint))?;
        let config_path = fixture.root.join(".roster").join("config.yml");
        let configuration = fs::read_to_string(&config_path)
            .map_err(io)?
            .replace("\r\n", "\n");
        fixture.write(
            ".roster/config.yml",
            &configuration.replacen(
                "llm:\n",
                &format!("llm:\n  max_requests: {}\n", if invalid { 6 } else { 5 }),
                1,
            ),
        )?;
        let result = fixture.invoke(&["run", "--seat", "coder", "--runtime", "builtin"])?;
        assert!(!result.status.success());
        assert!(text(&result.stdout)?.contains("Mode: unverified\n"));
        assert!(!text(&result.stdout)?.contains("Mode: verified\n"));
        assert!(!result.stderr.is_empty());
        assert!(
            !fs::read_to_string(fixture.root.join("REVIEW.md"))
                .map_err(io)?
                .contains("Verdict: pass")
        );
        assert_eq!(server.finish()?.len(), if invalid { 6 } else { 5 });
        assert!(
            !fixture
                .root
                .join(".roster")
                .join("locks")
                .join("run.lock")
                .exists()
        );
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn existing_task_dirty_source_foreign_artifacts_and_missing_selected_engine_are_never_adopted()
-> Result<(), Error> {
    for scenario in ["dirty", "foreign", "engine"] {
        let fixture = Fixture::new()?;
        fixture.task(
            &["src/app.mjs"],
            "Nonzero input increments while zero input is preserved.",
        )?;
        fixture.configure(Some("http://127.0.0.1:1/v1"))?;
        let before = fs::read(fixture.root.join("src").join("app.mjs")).map_err(io)?;
        let mut environment = fixture.environment()?;
        match scenario {
            "dirty" => fixture.write("src/app.mjs", "human dirty product\n")?,
            "foreign" => fixture.write("RESULT.md", "human result evidence\n")?,
            "engine" => {
                if let Value::Object(entries) = &mut environment {
                    entries.retain(|(name, _)| name != &Text::from("ROSTER_NODE20"));
                }
            }
            _ => return Err(Error::type_error("Unknown owned negative control")),
        }
        let result = fixture.invoke_with(
            &["run", "--seat", "coder", "--runtime", "builtin"],
            &environment,
            &fixture.root,
        )?;
        assert!(!result.status.success(), "{scenario}");
        assert!(!text(&result.stdout)?.contains("Mode: verified"));
        if scenario == "foreign" {
            assert_eq!(
                fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                "human result evidence\n"
            );
            assert_eq!(
                fs::read(fixture.root.join("src").join("app.mjs")).map_err(io)?,
                before
            );
        } else if scenario == "dirty" {
            assert_eq!(
                fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
                "human dirty product\n"
            );
        } else {
            assert_eq!(
                fs::read(fixture.root.join("src").join("app.mjs")).map_err(io)?,
                before
            );
            assert!(text(&result.stdout)?.contains("Mode: unverified"));
        }
        assert!(
            !fixture
                .root
                .join(".roster")
                .join("locks")
                .join("run.lock")
                .exists()
        );
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn unreadable_windows_private_config_is_an_actionable_error_not_a_default_fallback()
-> Result<(), Error> {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new()?;
    let file = fixture.root.join(".roster").join("config.yml");
    let original = fs::read(&file).map_err(io)?;
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .map_err(io)?;
    let error = fixture
        .loaded()
        .err()
        .ok_or_else(|| Error::type_error("Sharing-denied private config was silently defaulted"))?;
    assert!(matches!(error.code, Some("EBUSY" | "EACCES")));
    assert!(error.message.contains("config.yml"));
    drop(handle);
    assert_eq!(fs::read(&file).map_err(io)?, original);
    Ok(())
}

#[cfg(windows)]
#[test]
fn configured_cargo_cli_uses_actual_locked_offline_checks_and_named_counterfactual_before_readonly_review()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let environment = fixture.native_cargo()?;
    fixture.task(
        &["rust/llm/worker.rs", "tests/added.rs"],
        "The native increment operation preserves zero and increments nonzero input.",
    )?;
    let write = |path: &str, content: &str| {
        Value::object(vec![
            ("path", Value::string(path)),
            ("content", Value::string(content)),
        ])
    };
    let responses = vec![
        support::calls(vec![(
            "read",
            "read_file",
            support::path("rust/llm/worker.rs"),
        )]),
        support::calls(vec![
            (
                "write-product",
                "write_file",
                write(
                    "rust/llm/worker.rs",
                    "pub fn next(value: u8) -> u8 {\n    value + 1\n}\n",
                ),
            ),
            (
                "write-test",
                "write_file",
                write(
                    "tests/added.rs",
                    "#[test]\nfn nonzero_increment() {\n    assert_eq!(owned_local_native_consumer::next(2), 3);\n}\n",
                ),
            ),
        ]),
        support::calls(vec![("matching", "run_test", Value::object(Vec::new()))]),
        support::summary(),
        support::packet(
            support::message(
                &Value::object(vec![
                    (
                        "checks",
                        support::verdict(true)
                            .get("checks")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    ("findings", Value::Array(Vec::new())),
                ])
                .json(),
            ),
            "stop",
            "actual-local-author-review",
        ),
        support::review(support::verdict(true)),
        support::review(support::verdict(true)),
    ];
    let server = support::Server::start(responses)?;
    fixture.configure(Some(&server.endpoint))?;
    let result = fixture.invoke_with(
        &["run", "--seat", "coder", "--runtime", "builtin"],
        &environment,
        &fixture.root,
    )?;
    assert!(
        result.status.success(),
        "{}\n{}",
        text(&result.stdout)?,
        text(&result.stderr)?
    );
    assert!(text(&result.stdout)?.contains("Mode: verified\n"));
    let body = fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?;
    assert!(
        body.contains("Checks: PASS")
            && body.contains("cargo")
            && body.contains("nonzero_increment"),
        "{body}"
    );
    let requests = server.finish()?;
    assert_eq!(requests.len(), 7);
    let first = requests
        .first()
        .ok_or_else(|| Error::type_error("Native CLI made no configured coder request"))?
        .json();
    for instruction in [
        "Rust implementation",
        "Rust Cargo",
        "Rust Tokio",
        "minimum Rust 1.85",
    ] {
        assert!(first.contains(instruction), "{instruction}");
    }
    assert!(!fixture.root.join("target").exists());
    assert!(fixture.container.join("approved-target").is_dir());
    Ok(())
}

#[cfg(windows)]
#[test]
fn configured_ask_cli_preserves_the_authoritative_ask_and_runs_real_matching_consumer_checks_after_planning()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let ask = "Implement nonzero input increment while preserving zero input.\n\n## Files allowed\n- src/app.mjs\n- tests/app.test.mjs\n\n## Acceptance checks\n- Nonzero input increments while zero input is preserved.\n";
    fixture.write("ASK.md", ask)?;
    let before = fs::read_to_string(fixture.root.join("tests").join("app.test.mjs")).map_err(io)?;
    let after =
        format!("{before}test('increments nonzero input', () => assert.equal(answer(2), 3));\n");
    let plan = Value::object(vec![
        (
            "title",
            Value::string("fix: preserve zero and increment input"),
        ),
        (
            "files_allowed",
            Value::Array(vec![
                Value::string("src/app.mjs"),
                Value::string("tests/app.test.mjs"),
            ]),
        ),
        (
            "acceptance_checks",
            Value::Array(vec![Value::string(
                "Nonzero input increments while zero input is preserved.",
            )]),
        ),
        ("task_class", Value::string("fix")),
        (
            "design",
            Value::object(vec![
                (
                    "extend",
                    Value::Array(vec![Value::object(vec![
                        ("file", Value::string("src/app.mjs")),
                        ("exports", Value::Array(vec![Value::string("answer")])),
                    ])]),
                ),
                ("new_exports", Value::Array(Vec::new())),
                (
                    "outline",
                    Value::Array(vec![Value::string(
                        "Extend the existing operation and assert nonzero input with the retained zero control.",
                    )]),
                ),
                (
                    "edge_cases",
                    Value::Array(vec![Value::string("Zero remains one.")]),
                ),
                (
                    "out_of_scope",
                    Value::Array(vec![Value::string(
                        "No publication or unrelated consumers.",
                    )]),
                ),
            ]),
        ),
    ]);
    let mut responses = vec![
        support::packet(
            support::message(&plan.json()),
            "stop",
            "actual-local-planner",
        ),
        support::packet(
            support::message("{\"defects\":[]}"),
            "stop",
            "actual-local-planner",
        ),
    ];
    responses.extend(support::code_script(
        "src/app.mjs",
        "tests/app.test.mjs",
        &before,
        &after,
    ));
    let server = support::Server::start(responses)?;
    fixture.configure(Some(&server.endpoint))?;
    let result = fixture.invoke(&[
        "run",
        "--ask-file",
        "ASK.md",
        "--runtime",
        "builtin",
        "--base",
        "current",
        "--no-fetch",
    ])?;
    assert!(
        result.status.success(),
        "{}\n{}",
        text(&result.stdout)?,
        text(&result.stderr)?
    );
    let output = text(&result.stdout)?;
    assert!(
        output.contains("Mode: verified\n")
            && output.contains("Start: current HEAD (freshness unknown)"),
        "{output}"
    );
    let worktree = Path::new(
        output
            .lines()
            .find_map(|line| line.strip_prefix("Worktree: "))
            .ok_or_else(|| Error::type_error("Local code Ask returned no worktree"))?,
    );
    let task = fs::read_to_string(worktree.join("TASK.md")).map_err(io)?;
    assert!(task.contains(ask.trim_end()));
    assert!(task.contains("## Design"));
    assert!(
        fs::read_to_string(worktree.join("RESULT.md"))
            .map_err(io)?
            .contains("Checks: PASS")
    );
    assert_eq!(server.finish()?.len(), 9);
    assert_eq!(
        fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
        "export function answer(value = 0) { return 1; }\n"
    );
    Ok(())
}

#[test]
fn invocation_configuration_seals_detect_replacements_and_new_target_precedence_before_effects()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let loaded = fixture.loaded()?;
    let file = fixture.root.join(".roster").join("config.yml");
    let original = fs::read(&file).map_err(io)?;
    let replacement = fixture.root.join(".roster").join("replacement.yml");
    secure::write_exclusive(&replacement, &original)?;
    fs::remove_file(&file).map_err(io)?;
    fs::rename(&replacement, &file).map_err(io)?;
    assert_eq!(
        loaded.require_current().err().and_then(|error| error.code),
        Some("ROSTER_STALE_CONFIG")
    );
    fs::remove_file(&file).map_err(io)?;
    let installation = fixture.container.join("installation");
    secure::private_directory(&installation, true)?;
    let defaults = config::load(&installation, &fixture.root, &Value::object(Vec::new()))?;
    assert_eq!(defaults.source, Source::Defaults);
    secure::write_exclusive(&file, &original)?;
    assert_eq!(
        defaults
            .require_current()
            .err()
            .and_then(|error| error.code),
        Some("ROSTER_STALE_CONFIG")
    );
    assert!(!fixture.root.join(".roster").join("locks").exists());
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn private_configuration_changed_during_actual_model_observation_cannot_authorize_a_product_edit()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.configure(Some("http://127.0.0.1:1/v1"))?;
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    let loaded = fixture.loaded()?;
    let file = fixture.root.join(".roster").join("config.yml");
    let mut transport = support::Scripted::new(support::docs_script(true));
    transport.on_send = Some(Box::new(move |_, _| {
        fs::write(&file, "human changed private configuration\n").map_err(io)
    }));
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let admission = Admission::default();
    let source = CancellationSource::default();
    let (host, approval) = fixture.host()?;
    let mut ownership = delivery::Ownership::default();
    let mut driver = delivery::Driver {
        transport: &mut transport,
        observer: &mut observer,
        credentials: &credentials,
        admission: &admission,
    };
    let error = driver
        .run_local(
            delivery::Options {
                cwd: &fixture.root,
                installation: Path::new(env!("CARGO_MANIFEST_DIR")),
                loaded: &loaded,
                input: Input::ExistingTask,
                host,
                approval,
                start: start::Options::resolve(&loaded.config, None, None)?,
                fetch: start::FetchPolicy::Deny,
                skip_review: false,
            },
            source.token(),
            &mut ownership,
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Changed private configuration returned success"))?;
    assert_eq!(error.code, Some("ROSTER_STALE_CONFIG"));
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    assert_eq!(transport.requests.len(), 1);
    assert!(ownership.root().is_none());
    assert!(
        !fixture
            .root
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    Ok(())
}
