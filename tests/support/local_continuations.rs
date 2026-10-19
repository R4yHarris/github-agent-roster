use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::support::{self, Fixture, io, put, text};
use github_agent_roster::Error;
use github_agent_roster::cancel::{CancellationSource, CancellationToken, Reason};
use github_agent_roster::delivery::{self, Counts, Input, Status};
use github_agent_roster::execution::{backend, controls, tools};
use github_agent_roster::json::Value;
use github_agent_roster::llm::admission::Admission;
use github_agent_roster::llm::control::{NoObserver, Observer};
use github_agent_roster::llm::http::HttpTransport;
use github_agent_roster::paths;
use github_agent_roster::seats::Handoff;
use github_agent_roster::start;
use github_agent_roster::state::secure;
use github_agent_roster::text::Text;

#[path = "local_continuations/c5bc2474.rs"]
mod c5bc2474;

fn configure(fixture: &Fixture, endpoint: &str, options: &[(&str, &str)]) -> Result<(), Error> {
    fixture.configure(Some(endpoint))?;
    let path = fixture.root.join(".roster").join("config.yml");
    let mut source = fs::read_to_string(path).map_err(io)?.replace("\r\n", "\n");
    for (old, new) in options {
        if !source.contains(old) {
            return Err(Error::type_error(format!(
                "Owned outer config has no {old}"
            )));
        }
        source = source.replacen(old, new, 1);
    }
    fixture.write(".roster/config.yml", &source)
}

fn environment(fixture: &Fixture) -> Result<Value, Error> {
    let mut environment = fixture.environment()?;
    put(
        &mut environment,
        "ROSTER_API_KEY",
        Value::string("test-only-private-api-key"),
    );
    Ok(environment)
}

async fn local(
    fixture: &Fixture,
    observer: &mut impl Observer,
    token: CancellationToken,
    admission: &Admission,
    skip_review: bool,
) -> Result<delivery::Outcome, Error> {
    Box::pin(local_profile(
        fixture,
        observer,
        token,
        admission,
        skip_review,
        None,
    ))
    .await
}

async fn local_profile(
    fixture: &Fixture,
    observer: &mut impl Observer,
    token: CancellationToken,
    admission: &Admission,
    skip_review: bool,
    profile: Option<&github_agent_roster::fleet::Profile>,
) -> Result<delivery::Outcome, Error> {
    let mut loaded = fixture.loaded()?;
    if let Some(profile) = profile {
        loaded.config = loaded.config.with_profile(profile)?;
    }
    let environment = environment(fixture)?;
    let credentials = delivery::invocation_credentials(&environment, &loaded)?;
    let (host, approval) = github_agent_roster::host::invocation_host(
        &environment,
        Path::new(env!("CARGO_BIN_EXE_roster")),
    )?;
    let diagnostics = fixture.diagnostics.clone();
    diagnostics.counting_transport();
    let mut transport = support::diagnostics::Counting {
        inner: HttpTransport::new().map_err(|failure| *failure.error)?,
        diagnostics: diagnostics.clone(),
    };
    let mut observed = support::diagnostics::Observed {
        inner: observer,
        diagnostics: diagnostics.clone(),
    };
    let mut driver = delivery::Driver {
        transport: &mut transport,
        observer: &mut observed,
        credentials: &credentials,
        admission,
    };
    let mut ownership = delivery::Ownership::default();
    diagnostics.native_timings(ownership.timings());
    diagnostics.client_started(&fixture.root);
    let result = Box::pin(driver.run_local(
        delivery::Options {
            cwd: &fixture.root,
            installation: Path::new(env!("CARGO_MANIFEST_DIR")),
            loaded: &loaded,
            input: Input::ExistingTask,
            host,
            approval,
            start: start::Options::resolve(&loaded.config, None, None)?,
            fetch: start::FetchPolicy::Deny,
            skip_review,
        },
        token,
        &mut ownership,
    ))
    .await;
    diagnostics.client_returned(&result);
    assert!(ownership.root().is_none());
    assert!(
        !fixture
            .root
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    result
}

fn checklist() -> Value {
    support::calls(vec![(
        "close",
        "update_checklist",
        Value::object(vec![(
            "items",
            Value::Array(vec![Value::object(vec![
                ("id", Value::Number(1.0)),
                ("status", Value::string("done")),
                ("evidence", Value::string("docs/guide.md")),
            ])]),
        )]),
    )])
}

fn reviewed(pass: bool, previous: u32) -> Value {
    let mut report = support::verdict(pass);
    if previous != 0 {
        put(
            &mut report,
            "findings",
            Value::Array(
                (1..=previous)
                    .map(|id| {
                        Value::object(vec![
                            ("id", Value::Number(f64::from(id))),
                            ("resolved", Value::Boolean(pass)),
                            (
                                "evidence",
                                Value::string("Current guide was independently checked."),
                            ),
                        ])
                    })
                    .collect(),
            ),
        );
    }
    support::review(report)
}

fn repair(pass: bool, previous: u32, from: &str, to: &str, workspace: bool) -> Vec<Value> {
    let mut script = vec![
        support::calls(vec![("read", "read_file", support::path("docs/guide.md"))]),
        support::calls(vec![(
            "edit",
            "edit_file",
            support::edit("docs/guide.md", from, to),
        )]),
    ];
    if workspace {
        script.push(checklist());
    }
    script.extend([
        support::summary(),
        support::review(support::verdict(true)),
        reviewed(pass, previous),
    ]);
    script
}

fn docs_task(fixture: &Fixture) -> Result<(), Error> {
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    Ok(())
}

fn system(request: &Value) -> Result<String, Error> {
    let messages = request
        .get("messages")
        .and_then(|value| match value {
            Value::Array(values) => Some(values),
            _ => None,
        })
        .ok_or_else(|| Error::type_error("Actual request messages are absent"))?;
    messages
        .first()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Actual request system text is absent"))?
        .to_utf8()
}

fn check_current_sources(root: &Path) -> Result<Vec<(PathBuf, secure::Snapshot)>, Error> {
    [
        "TASK.md",
        "RECIPE.yml",
        "ESTIMATE.md",
        ".roster/locks/run.lock",
    ]
    .into_iter()
    .map(|name| {
        let path = root.join(name);
        let snapshot = secure::read(&path, 65_536)?
            .ok_or_else(|| Error::type_error("Outer immutable/lease fixture is missing"))?;
        Ok((path, snapshot))
    })
    .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn outer_semantic_fail_repairs_actual_docs_with_fresh_review_unknown_usage_and_one_lease()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let mut script = support::docs_script(false);
    script.insert(2, checklist());
    script.extend(repair(
        true,
        2,
        "The requested bounded outcome is documented.",
        "The requested bounded outcome is documented and repaired.",
        true,
    ));
    for packet in script.iter_mut().skip(6) {
        put(packet, "usage", Value::Null);
    }
    let root = fixture.root.clone();
    let frozen = Arc::new(Mutex::new(None));
    let observe_frozen = frozen.clone();
    let server = support::Server::observed_for(&fixture, script, move |_, request| {
        let mut frozen = observe_frozen
            .lock()
            .map_err(|_| Error::type_error("Owned freeze lock"))?;
        if frozen.is_none() {
            *frozen = Some(check_current_sources(&root)?);
        }
        for (path, snapshot) in frozen
            .as_ref()
            .ok_or_else(|| Error::type_error("Owned source freeze missing"))?
        {
            assert!(secure::unchanged(path, snapshot, 65_536)?);
        }
        assert_eq!(
            request.get("model"),
            Some(&Value::string("configured-route-not-a-response"))
        );
        Ok(())
    })?;
    configure(
        &fixture,
        &server.endpoint,
        &[("evidence_workspace: false", "evidence_workspace: true")],
    )?;
    let admission = Admission::default();
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &admission,
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Verified, "{:?}", outcome.failure);
    assert!(outcome.succeeded());
    assert_eq!(
        outcome.orchestration.counts,
        Counts {
            review_repairs: 1,
            ..Counts::default()
        }
    );
    assert_eq!(outcome.orchestration.attempts.len(), 4);
    assert_eq!(outcome.orchestration.archives.len(), 1);
    let archive = outcome
        .orchestration
        .archives
        .first()
        .ok_or_else(|| Error::type_error("Outer archive missing"))?;
    for name in ["RESULT.md", "REVIEW.md", "CONTEXT.md"] {
        assert!(archive.join(name).is_file());
    }
    for name in ["TASK.md", "RECIPE.yml", "ESTIMATE.md"] {
        assert!(!archive.join(name).exists());
    }
    assert!(
        fs::read_to_string(archive.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: fail")
    );
    assert!(
        fs::read_to_string(fixture.root.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: pass")
    );
    let earlier = outcome
        .orchestration
        .attempts
        .first()
        .and_then(|attempt| attempt.run.as_ref())
        .ok_or_else(|| Error::type_error("Earlier actual coder run missing"))?;
    assert_eq!(earlier.metrics().usage.prompt_tokens, Some(0));
    for run in [outcome.coder_run.as_ref(), outcome.reviewer_run.as_ref()]
        .into_iter()
        .flatten()
    {
        assert_eq!(run.metrics().usage.prompt_tokens, None);
        assert_eq!(run.metrics().usage.completion_tokens, None);
        assert!(run.metrics().model.starts_with("actual-local-"));
    }
    assert_eq!(
        outcome
            .orchestration
            .transitions
            .first()
            .and_then(|event| event.get("unmetChecks")),
        Some(&Value::Array(vec![Value::Number(1.0)]))
    );
    let log = fs::read_to_string(&outcome.log).map_err(io)?;
    assert!(log.contains("\"event\":\"review-repair\""));
    assert!(!log.contains("test-only-private-api-key"));
    let requests = server.finish()?;
    assert_eq!(requests.len(), 12);
    let next = system(
        requests
            .get(6)
            .ok_or_else(|| Error::type_error("Fresh coder request missing"))?,
    )?;
    assert!(next.contains("Review repair 1"));
    assert!(next.contains("reopened by independent review"));
    assert!(!next.contains("done —"));
    assert!(!next.contains("node --test until"));
    for index in [4, 5, 10, 11] {
        assert!(
            requests
                .get(index)
                .ok_or_else(|| Error::type_error("Actual reviewer request missing"))?
                .get("tools")
                .is_none()
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_two_semantic_repairs_stop_and_same_unmet_checks_change_perspective()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let mut script = support::docs_script(false);
    script.extend(repair(
        false,
        2,
        "The requested bounded outcome is documented.",
        "The requested bounded outcome is documented after repair one.",
        false,
    ));
    script.extend(repair(
        false,
        4,
        "after repair one",
        "after repair two",
        false,
    ));
    let server = support::Server::start_for(&fixture, script)?;
    configure(&fixture, &server.endpoint, &[])?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert!(!outcome.succeeded());
    assert_eq!(outcome.orchestration.counts.review_repairs, 2);
    assert_eq!(outcome.orchestration.counts.perspectives, 0);
    assert_eq!(
        outcome
            .orchestration
            .attempts
            .iter()
            .filter(|attempt| attempt.seat == "coder")
            .count(),
        3
    );
    assert_eq!(
        outcome
            .orchestration
            .attempts
            .iter()
            .filter(|attempt| attempt.seat == "reviewer")
            .count(),
        3
    );
    assert_eq!(outcome.orchestration.archives.len(), 2);
    let transitions = &outcome.orchestration.transitions;
    assert_eq!(
        transitions
            .first()
            .and_then(|event| event.get("changed_perspective")),
        Some(&Value::Boolean(false))
    );
    assert_eq!(
        transitions
            .get(1)
            .and_then(|event| event.get("changed_perspective")),
        Some(&Value::Boolean(true))
    );
    let requests = server.finish()?;
    assert_eq!(requests.len(), 15);
    assert!(
        system(
            requests
                .get(10)
                .ok_or_else(|| Error::type_error("Second repair context missing"))?
        )?
        .contains("do not repeat that approach")
    );
    assert!(
        requests
            .get(14)
            .ok_or_else(|| Error::type_error("Fresh reviewer missing"))?
            .json()
            .contains("Previous findings")
    );
    assert!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md"))
            .map_err(io)?
            .contains("after repair two")
    );
    Ok(())
}

fn write(path: &str, content: &str) -> Value {
    Value::object(vec![
        ("path", Value::string(path)),
        ("content", Value::string(content)),
    ])
}

fn expansions(from: u32, through: u32) -> Value {
    let values = (from..=through).map(|index| (
        format!("expand-{index}"),
        write(&format!("docs/part-{index}.md"), &format!("# Bounded contribution {index}\n\nThe requested document outcome is extended without changing TASK authority.\n")),
    )).collect::<Vec<_>>();
    support::calls(
        values
            .iter()
            .map(|(id, args)| (id.as_str(), "write_file", args.clone()))
            .collect(),
    )
}

fn scope_task(fixture: &Fixture) -> Result<(), Error> {
    fixture.write(
        "docs/overview.md",
        "# Overview\n\nExisting guide overview.\n",
    )?;
    fixture.git(&["add", "docs/overview.md"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned scope base"])?;
    fixture.task(
        &["docs/guide.md", "docs/overview.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_actual_scope_three_six_twelve_preserves_writes_records_and_fresh_readonly_review()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    scope_task(&fixture)?;
    let mut script = support::docs_script(true)
        .into_iter()
        .take(2)
        .collect::<Vec<_>>();
    script.extend([
        expansions(1, 4),
        expansions(4, 7),
        expansions(7, 8),
        support::summary(),
        support::review(support::verdict(true)),
        support::review(support::verdict(true)),
    ]);
    let server = support::Server::start_for(&fixture, script)?;
    configure(&fixture, &server.endpoint, &[])?;
    let task = fs::read(fixture.root.join("TASK.md")).map_err(io)?;
    let recipe = fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Verified, "{:?}", outcome.failure);
    assert_eq!(
        outcome.orchestration.counts,
        Counts {
            rescopes: 2,
            ..Counts::default()
        }
    );
    let transitions = &outcome.orchestration.transitions;
    for (index, from, to, needed) in [
        (0, 3.0, 6.0, "docs/part-4.md"),
        (1, 6.0, 12.0, "docs/part-7.md"),
    ] {
        let event = transitions
            .get(index)
            .ok_or_else(|| Error::type_error("Actual rescope event missing"))?;
        assert_eq!(event.get("from"), Some(&Value::Number(from)));
        assert_eq!(event.get("to"), Some(&Value::Number(to)));
        assert_eq!(
            event.get("files"),
            Some(&Value::Array(vec![Value::string(needed)]))
        );
    }
    assert_eq!(fs::read(fixture.root.join("TASK.md")).map_err(io)?, task);
    assert_eq!(
        fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?,
        recipe
    );
    let scope = outcome
        .coder_result
        .as_ref()
        .and_then(|result| result.get("scopeFiles"))
        .and_then(|value| match value {
            Value::Array(values) => Some(values),
            _ => None,
        })
        .ok_or_else(|| Error::type_error("Final recorded scope missing"))?;
    assert_eq!(scope.len(), 8);
    for index in 1..=8 {
        assert!(
            fixture
                .root
                .join("docs")
                .join(format!("part-{index}.md"))
                .is_file()
        );
    }
    assert!(outcome.format().contains("Recorded scope expansions:"));
    let requests = server.finish()?;
    assert_eq!(requests.len(), 8);
    let second = system(
        requests
            .get(3)
            .ok_or_else(|| Error::type_error("First rescope coder missing"))?,
    )?;
    let third = system(
        requests
            .get(4)
            .ok_or_else(|| Error::type_error("Second rescope coder missing"))?,
    )?;
    assert!(second.contains("budget is now 6"));
    assert!(third.contains("budget is now 12"));
    let reviewer = requests
        .last()
        .ok_or_else(|| Error::type_error("Scope reviewer missing"))?
        .json();
    assert!(reviewer.contains("Recorded scope expansions"));
    assert!(reviewer.contains("docs/part-1.md"));
    assert!(reviewer.contains("docs/part-8.md"));
    let log = fs::read_to_string(&outcome.log).map_err(io)?;
    assert!(log.contains("\"event\":\"scope-expansion\""));
    assert!(log.contains("\"event\":\"rescope\""));
    Ok(())
}

async fn scope_stop(
    initial: u32,
    expected_rescopes: u32,
    ranges: &[(u32, u32)],
    limit: usize,
) -> Result<(), Error> {
    let fixture = Fixture::new()?;
    scope_task(&fixture)?;
    let mut script = support::docs_script(true)
        .into_iter()
        .take(2)
        .collect::<Vec<_>>();
    script.extend(
        ranges
            .iter()
            .map(|(from, through)| expansions(*from, *through)),
    );
    let server = support::Server::start(script)?;
    configure(
        &fixture,
        &server.endpoint,
        &[("seat:\n", &format!("seat:\n  scope_expansion: {initial}\n"))],
    )?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(
        outcome.status(),
        Status::PlanningEscalation,
        "{:?}",
        outcome.failure
    );
    assert!(!outcome.succeeded());
    assert_eq!(
        outcome.failure.as_ref().and_then(|error| error.code),
        Some("ROSTER_PLANNING_ESCALATION")
    );
    assert_eq!(outcome.orchestration.counts.rescopes, expected_rescopes);
    assert_eq!(outcome.orchestration.counts.perspectives, 0);
    assert_eq!(outcome.orchestration.counts.review_repairs, 0);
    assert_eq!(
        outcome.orchestration.archives.len(),
        usize::try_from(expected_rescopes).map_err(|_| Error::type_error("Fixture count"))?
    );
    assert!(outcome.reviewer_run.is_none());
    assert!(outcome.reviewer_report.is_none());
    assert!(outcome.format().contains("new human/planner handoff"));
    let count = outcome
        .coder_result
        .as_ref()
        .and_then(|result| result.get("scopeFiles"))
        .and_then(|value| match value {
            Value::Array(values) => Some(values.len()),
            _ => None,
        })
        .ok_or_else(|| Error::type_error("Scope stop lost actual scope"))?;
    assert_eq!(count, limit);
    let requests = server.finish()?;
    assert_eq!(requests.len(), 2 + ranges.len());
    assert!(
        !fixture
            .root
            .join("docs")
            .join(format!("part-{}.md", limit + 1))
            .exists()
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_two_rescope_exhaustion_is_explicit_planning_not_success_or_task_failure()
-> Result<(), Error> {
    Box::pin(scope_stop(3, 2, &[(1, 4), (4, 7), (7, 13)], 12)).await
}

#[tokio::test(flavor = "current_thread")]
async fn outer_scope_twelve_caps_at_sixteen_then_hands_back_to_planning() -> Result<(), Error> {
    Box::pin(scope_stop(12, 1, &[(1, 8), (9, 13), (13, 17)], 16)).await
}

#[tokio::test(flavor = "current_thread")]
async fn outer_initial_sixteen_never_widens_further() -> Result<(), Error> {
    Box::pin(scope_stop(16, 0, &[(1, 8), (9, 16), (17, 17)], 16)).await
}

#[tokio::test(flavor = "current_thread")]
async fn outer_strict_zero_stays_zero_with_recorded_planning_escalation() -> Result<(), Error> {
    Box::pin(scope_stop(0, 0, &[(1, 1)], 0)).await
}

#[tokio::test(flavor = "current_thread")]
async fn outer_protected_private_vendor_paths_cannot_authorize_scope_or_perspective()
-> Result<(), Error> {
    for file in [
        ".github/workflows/new.yml",
        ".roster/private.json",
        "vendor/unreviewed.txt",
        "agent-policy.yml",
        ".env",
    ] {
        let fixture = Fixture::new()?;
        scope_task(&fixture)?;
        let server = support::Server::start(vec![support::calls(vec![(
            "denied",
            "write_file",
            write(file, "Not authorized.\n"),
        )])])?;
        configure(
            &fixture,
            &server.endpoint,
            &[("seat:\n", "seat:\n  scope_expansion: 16\n")],
        )?;
        let outcome = Box::pin(local(
            &fixture,
            &mut NoObserver,
            CancellationSource::default().token(),
            &Admission::default(),
            false,
        ))
        .await?;
        assert_eq!(outcome.status(), Status::Unverified);
        assert_eq!(
            outcome.coder_handoff,
            Some(Handoff::PolicyDenied),
            "{file}: {:?}",
            outcome.failure
        );
        assert_eq!(outcome.orchestration.counts, Counts::default());
        assert!(outcome.orchestration.archives.is_empty());
        assert!(!fixture.root.join(file).exists());
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

fn turn_script() -> Vec<Value> {
    vec![
        support::calls(vec![(
            "keep",
            "write_file",
            write(
                "docs/guide.md",
                "# Guide\n\nCorrect work survives every fresh perspective.\n",
            ),
        )]),
        support::calls(vec![("read", "read_file", support::path("docs/guide.md"))]),
        support::calls(vec![("read", "read_file", support::path("docs/guide.md"))]),
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn outer_actual_turn_exhaustion_gets_only_two_same_profile_contexts_and_preserves_work()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let server = support::Server::start(turn_script())?;
    configure(
        &fixture,
        &server.endpoint,
        &[("turn_budget: 16", "turn_budget: 1")],
    )?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::TurnBudget));
    assert_eq!(
        outcome.orchestration.counts,
        Counts {
            perspectives: 2,
            ..Counts::default()
        }
    );
    let coders = outcome
        .orchestration
        .attempts
        .iter()
        .filter(|attempt| attempt.seat == "coder")
        .collect::<Vec<_>>();
    assert_eq!(coders.len(), 3);
    assert!(
        coders
            .iter()
            .all(|attempt| attempt.handoff == Some(Handoff::TurnBudget))
    );
    assert!(coders.iter().all(|attempt| {
        attempt
            .run
            .as_ref()
            .is_some_and(|run| run.metrics().model == "actual-local-coder")
    }));
    assert!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md"))
            .map_err(io)?
            .contains("Correct work survives")
    );
    for archive in &outcome.orchestration.archives {
        assert!(archive.join("RESULT.md").is_file());
        assert!(!archive.join("REVIEW.md").exists());
    }
    let requests = server.finish()?;
    assert_eq!(requests.len(), 3);
    for (index, request) in requests.iter().enumerate().skip(1) {
        let context = system(request)?;
        assert!(context.contains(&format!("Fresh perspective {index}")));
        assert!(context.contains("same selected model"));
        assert!(context.contains("docs/guide.md"));
        assert!(!request.json().contains("\"role\":\"tool\""));
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_shared_request_limit_cannot_reset_on_fresh_coder_context() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let server = support::Server::start(turn_script().into_iter().take(2).collect())?;
    configure(
        &fixture,
        &server.endpoint,
        &[
            ("turn_budget: 16", "turn_budget: 1"),
            ("llm:\n", "llm:\n  max_requests: 2\n"),
        ],
    )?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(
        outcome.failure.as_ref().and_then(|error| error.code),
        Some("ROSTER_SEAT_TURN_BUDGET")
    );
    assert_eq!(
        outcome
            .failure
            .as_ref()
            .and_then(|error| error.cause.as_ref())
            .and_then(|cause| cause.code),
        Some("ROSTER_MODEL_REQUEST_LIMIT")
    );
    assert_eq!(outcome.orchestration.counts.perspectives, 1);
    assert_eq!(outcome.orchestration.archives.len(), 1);
    assert_eq!(server.finish()?.len(), 2);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_review_repair_cannot_reset_the_invocation_request_limit_or_archive_terminal_feedback()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let server = support::Server::start(support::docs_script(false))?;
    configure(
        &fixture,
        &server.endpoint,
        &[("llm:\n", "llm:\n  max_requests: 5\n")],
    )?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(
        outcome.failure.as_ref().and_then(|error| error.code),
        Some("ROSTER_MODEL_REQUEST_LIMIT")
    );
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert!(outcome.orchestration.archives.is_empty());
    assert!(
        fs::read_to_string(fixture.root.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: fail")
    );
    assert_eq!(server.finish()?.len(), 5);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_incomplete_or_malformed_review_is_not_a_completed_semantic_failure()
-> Result<(), Error> {
    for audit in [false, true] {
        let fixture = Fixture::new()?;
        docs_task(&fixture)?;
        let mut script = support::docs_script(true);
        let malformed = support::packet(
            support::message("not a semantic verdict"),
            "stop",
            "actual-local-reviewer",
        );
        if audit {
            script.truncate(3);
            script.push(malformed);
        } else {
            script.truncate(4);
            script.extend([malformed.clone(), malformed]);
        }
        let count = script.len();
        let server = support::Server::start(script)?;
        configure(&fixture, &server.endpoint, &[])?;
        let outcome = Box::pin(local(
            &fixture,
            &mut NoObserver,
            CancellationSource::default().token(),
            &Admission::default(),
            false,
        ))
        .await?;
        assert_eq!(outcome.status(), Status::Unverified);
        assert_eq!(outcome.orchestration.counts, Counts::default());
        assert!(outcome.orchestration.archives.is_empty());
        assert!(outcome.failure.is_some());
        assert_eq!(server.finish()?.len(), count);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_arbitrary_response_failure_never_becomes_a_perspective_retry() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let server = support::Server::start(vec![support::packet(
        support::message("This is not a supported finish."),
        "forbidden-finish",
        "actual-error-model",
    )])?;
    configure(&fixture, &server.endpoint, &[])?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert!(outcome.failure.is_some());
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_perspective_success_uses_earliest_baseline_without_repeating_correct_writes()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let mut script = turn_script().into_iter().take(2).collect::<Vec<_>>();
    script.extend([
        support::summary(),
        support::review(support::verdict(true)),
        support::review(support::verdict(true)),
    ]);
    let server = support::Server::start(script)?;
    configure(
        &fixture,
        &server.endpoint,
        &[("turn_budget: 16", "turn_budget: 2")],
    )?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Verified, "{:?}", outcome.failure);
    assert_eq!(outcome.orchestration.counts.perspectives, 1);
    assert_eq!(
        outcome
            .orchestration
            .attempts
            .iter()
            .filter(|attempt| attempt.seat == "coder")
            .count(),
        2
    );
    assert_eq!(server.finish()?.len(), 5);
    assert!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md"))
            .map_err(io)?
            .contains("Correct work survives")
    );
    Ok(())
}

struct StopTransition {
    root: PathBuf,
    action: &'static str,
    source: CancellationSource,
    reached: bool,
}

impl Observer for StopTransition {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if event.get("type") != Some(&Value::string("perspective-escalation")) || self.reached {
            return Ok(());
        }
        self.reached = true;
        match self.action {
            "cancel" => {
                self.source.cancel(Reason::Cancelled)?;
            }
            "source" => {
                fs::write(
                    self.root.join("docs").join("guide.md"),
                    "Human changed source during continuation.\n",
                )
                .map_err(io)?;
            }
            "config" => {
                fs::write(
                    self.root.join(".roster").join("config.yml"),
                    "Human changed configuration.\n",
                )
                .map_err(io)?;
            }
            "estimate" => {
                fs::write(
                    self.root.join("ESTIMATE.md"),
                    "Human changed estimate authority.\n",
                )
                .map_err(io)?;
            }
            "observer-error" => {
                return Err(Error::new(
                    "OwnedObserverFailure",
                    "Actual continuation observer failed",
                ));
            }
            "deadline" => {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(3601)).await;
                std::future::pending::<()>().await;
            }
            _ => {
                return Err(Error::type_error("Unknown owned transition control"));
            }
        }
        Ok(())
    }
}

async fn transition_stop(action: &'static str) -> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let server = support::Server::start(turn_script().into_iter().take(1).collect())?;
    configure(
        &fixture,
        &server.endpoint,
        &[("turn_budget: 16", "turn_budget: 1")],
    )?;
    let source = CancellationSource::default();
    let mut observer = StopTransition {
        root: fixture.root.clone(),
        action,
        source: source.clone(),
        reached: false,
    };
    let result = Box::pin(local(
        &fixture,
        &mut observer,
        source.token(),
        &Admission::default(),
        false,
    ))
    .await;
    assert!(observer.reached);
    let error = result
        .err()
        .ok_or_else(|| Error::type_error("Stopped transition yielded a delivery outcome"))?;
    match action {
        "cancel" => assert_eq!(error.code, Some("ROSTER_CANCELLED")),
        "config" => assert_eq!(error.code, Some("ROSTER_STALE_CONFIG")),
        "source" | "estimate" => assert_eq!(error.code, Some("ROSTER_TOOL_DENIED")),
        "observer-error" => assert!(
            error
                .message
                .contains("Actual continuation observer failed")
        ),
        _ => return Err(Error::type_error("Unexpected transition-stop fixture")),
    }
    assert_eq!(server.finish()?.len(), 1);
    assert!(!fixture.root.join("RESULT.md").exists());
    assert!(!fixture.root.join("REVIEW.md").exists());
    if action == "source" {
        assert_eq!(
            fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
            "Human changed source during continuation.\n"
        );
    }
    if action == "estimate" {
        assert_eq!(
            fs::read_to_string(fixture.root.join("ESTIMATE.md")).map_err(io)?,
            "Human changed estimate authority.\n"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_caller_cancellation_at_transition_revokes_new_context_and_settles_lease()
-> Result<(), Error> {
    Box::pin(transition_stop("cancel")).await
}

#[tokio::test(flavor = "current_thread")]
async fn outer_source_change_at_transition_is_not_a_perspective_reason() -> Result<(), Error> {
    Box::pin(transition_stop("source")).await
}

#[tokio::test(flavor = "current_thread")]
async fn outer_config_change_at_transition_stops_without_adopting_new_authority()
-> Result<(), Error> {
    Box::pin(transition_stop("config")).await
}

#[tokio::test(flavor = "current_thread")]
async fn outer_estimate_bytes_are_immutable_through_every_context() -> Result<(), Error> {
    Box::pin(transition_stop("estimate")).await
}

#[tokio::test(flavor = "current_thread")]
async fn outer_actual_observer_error_stops_instead_of_retrying_it_as_stuck() -> Result<(), Error> {
    Box::pin(transition_stop("observer-error")).await
}

#[tokio::test(flavor = "current_thread")]
async fn outer_task_deadline_remains_shared_while_a_new_context_waits() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let server = support::Server::start(turn_script().into_iter().take(1).collect())?;
    configure(
        &fixture,
        &server.endpoint,
        &[("turn_budget: 16", "turn_budget: 1")],
    )?;
    let source = CancellationSource::default();
    let mut observer = StopTransition {
        root: fixture.root.clone(),
        action: "deadline",
        source: source.clone(),
        reached: false,
    };
    let admission = Admission::default();
    let result = controls::with_task_deadline(&source.token(), Some(3_600_000), |token| {
        Box::pin(local(&fixture, &mut observer, token, &admission, false))
    })
    .await;
    if observer.reached {
        tokio::time::resume();
    }
    let error = result
        .err()
        .ok_or_else(|| Error::type_error("Whole-task deadline yielded success"))?;
    assert!(observer.reached);
    assert_eq!(error.code, Some("ROSTER_TASK_TIMEOUT"));
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_external_source_change_during_actual_model_request_cannot_become_retryable()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let root = fixture.root.clone();
    let server = support::Server::observed(
        vec![support::calls(vec![(
            "read",
            "read_file",
            support::path("docs/guide.md"),
        )])],
        move |_, _| {
            fs::write(
                root.join("docs").join("guide.md"),
                "Human source edit during request.\n",
            )
            .map_err(io)
        },
    )?;
    configure(
        &fixture,
        &server.endpoint,
        &[("turn_budget: 16", "turn_budget: 1")],
    )?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::PolicyDenied));
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert_eq!(
        outcome
            .coder_response
            .as_ref()
            .map(|response| response.model.clone()),
        Some(Text::from("actual-local-coder"))
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "Human source edit during request.\n"
    );
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_foreign_context_replacement_is_never_adopted_or_archived() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let root = fixture.root.clone();
    let server =
        support::Server::observed(turn_script().into_iter().take(1).collect(), move |_, _| {
            fs::write(
                root.join("CONTEXT.md"),
                "Human replacement of the context.\n",
            )
            .map_err(io)
        })?;
    configure(
        &fixture,
        &server.endpoint,
        &[("turn_budget: 16", "turn_budget: 1")],
    )?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::PolicyDenied));
    assert!(outcome.orchestration.archives.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.root.join("CONTEXT.md")).map_err(io)?,
        "Human replacement of the context.\n"
    );
    assert_eq!(server.finish()?.len(), 1);
    let common = paths::repository_paths(&fixture.root)?
        .common_dir
        .join("roster-artifacts");
    assert!(!common.exists());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_skip_review_and_stub_never_enter_review_repair_or_claim_reviewed()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let server = support::Server::start(support::docs_script(true).into_iter().take(3).collect())?;
    configure(
        &fixture,
        &server.endpoint,
        &[("required: true", "required: false")],
    )?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        true,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::ReviewSkipped);
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert!(outcome.reviewer_run.is_none());
    assert!(!fixture.root.join("REVIEW.md").exists());
    assert_eq!(server.finish()?.len(), 3);
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Stub);
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert!(outcome.reviewer_run.is_none());
    assert!(outcome.coder_run.is_none());
    assert!(
        fs::read_to_string(fixture.root.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: unavailable")
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_failed_full_own_regression_survives_rescope_scoped_green_and_both_perspectives()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('retained existing nonzero behavior', () => assert.equal(answer(2), 1));\n")?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned outside full-suite obligation",
    ])?;
    fixture.task(
        &["src/app.mjs", "tests/app.test.mjs"],
        "Nonzero input increments while zero input is preserved.",
    )?;
    let before = fs::read_to_string(fixture.root.join("tests").join("app.test.mjs")).map_err(io)?;
    let after =
        format!("{before}test('increments nonzero input', () => assert.equal(answer(2), 3));\n");
    let mut script = support::code_script("src/app.mjs", "tests/app.test.mjs", &before, &after)
        .into_iter()
        .take(2)
        .collect::<Vec<_>>();
    script.extend([support::summary(), expansions(1, 4)]);
    for _ in 0..3 {
        script.push(support::calls(vec![(
            "scoped-green",
            "run_test",
            Value::object(Vec::new()),
        )]));
        script.extend([support::summary(), support::summary(), support::summary()]);
    }
    let server = support::Server::start_for(&fixture, script)?;
    configure(&fixture, &server.endpoint, &[])?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(
        outcome.coder_handoff,
        Some(Handoff::RepeatedFailure),
        "{:?}",
        outcome.failure
    );
    assert_eq!(
        outcome.orchestration.counts,
        Counts {
            rescopes: 1,
            perspectives: 2,
            review_repairs: 0
        }
    );
    let negative = outcome
        .coder_result
        .as_ref()
        .and_then(|result| result.get("fullVerification"))
        .ok_or_else(|| Error::type_error("Earlier actual failed-full obligation disappeared"))?;
    assert_eq!(negative.get("original_exit"), Some(&Value::Number(1.0)));
    assert_eq!(
        negative.get("clean_full_suite"),
        Some(&Value::Boolean(false))
    );
    assert_eq!(outcome.orchestration.archives.len(), 3);
    for archive in &outcome.orchestration.archives {
        let body = fs::read_to_string(archive.join("RESULT.md")).map_err(io)?;
        assert!(body.contains("## Full verification obligation"), "{body}");
        assert!(body.contains("Original full-suite exit: 1"));
        assert!(body.contains("a scoped pass does not erase the failed full run"));
        assert!(!archive.join("REVIEW.md").exists());
    }
    assert!(outcome.reviewer_run.is_none());
    let requests = server.finish()?;
    assert_eq!(requests.len(), 16);
    for index in [4, 8, 12] {
        let context = system(
            requests
                .get(index)
                .ok_or_else(|| Error::type_error("Current continuation context missing"))?,
        )?;
        assert!(context.contains("src/app.mjs"));
        assert!(context.contains("tests/app.test.mjs"));
    }
    assert_eq!(
        fs::read_to_string(fixture.root.join("tests").join("outside.test.mjs")).map_err(io)?,
        "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('retained existing nonzero behavior', () => assert.equal(answer(2), 1));\n"
    );
    Ok(())
}

#[test]
fn outer_actual_ask_file_cli_repairs_in_exactly_one_new_worktree_and_retains_authority_bytes()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.write("ASK.md", "Update the guide to document the requested bounded outcome.\n\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide documents the requested bounded outcome.\n")?;
    let mut script = vec![
        support::packet(
            support::message(
                &Value::object(vec![
                    ("title", Value::string("docs: bounded outer guide")),
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
    script.extend(support::docs_script(false));
    script.extend(repair(
        true,
        2,
        "The requested bounded outcome is documented.",
        "The requested bounded outcome is documented and repaired.",
        false,
    ));
    let root = fixture.root.clone();
    let frozen = Arc::new(Mutex::new(None));
    let observed = frozen.clone();
    let server = support::Server::observed(script, move |index, _| {
        if index < 3 {
            return Ok(());
        }
        let worktrees = fs::read_dir(root.join(".worktrees"))
            .map_err(io)?
            .map(|entry| entry.map(|entry| entry.path()).map_err(io))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(worktrees.len(), 1);
        let worktree = worktrees
            .first()
            .ok_or_else(|| Error::type_error("Actual isolated worktree missing"))?;
        let mut observed = observed
            .lock()
            .map_err(|_| Error::type_error("Owned CLI authority lock"))?;
        if observed.is_none() {
            *observed = Some(check_current_sources(worktree)?);
        }
        for (path, snapshot) in observed
            .as_ref()
            .ok_or_else(|| Error::type_error("CLI authority freeze missing"))?
        {
            assert!(secure::unchanged(path, snapshot, 65_536)?);
        }
        Ok(())
    })?;
    configure(&fixture, &server.endpoint, &[])?;
    fixture.critic(&server.endpoint)?;
    let mut environment = environment(&fixture)?;
    put(
        &mut environment,
        "ROSTER_CRITIC_API_KEY",
        Value::string("test-only-private-critic-key"),
    );
    let head = fixture.git(&["rev-parse", "HEAD"])?;
    let before = fs::read(fixture.root.join("docs").join("guide.md")).map_err(io)?;
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
    assert!(
        output.status.success(),
        "{}\n{}\nObserved requests: {}",
        text(&output.stdout)?,
        text(&output.stderr)?,
        server
            .requests
            .lock()
            .map_err(|_| Error::type_error("Owned request diagnostic lock"))?
            .len(),
    );
    let stdout = text(&output.stdout)?;
    assert!(stdout.contains("Mode: verified\n"), "{stdout}");
    assert!(stdout.contains("1 review repair(s)"));
    assert_eq!(fixture.git(&["rev-parse", "HEAD"])?, head);
    assert_eq!(
        fs::read(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        before
    );
    let worktree = Path::new(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("Worktree: "))
            .ok_or_else(|| Error::type_error("Native repaired CLI omitted worktree"))?,
    );
    assert!(worktree.starts_with(fixture.root.join(".worktrees")));
    assert!(
        fs::read_to_string(worktree.join("docs").join("guide.md"))
            .map_err(io)?
            .contains("and repaired.")
    );
    assert!(
        fs::read_to_string(worktree.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: pass")
    );
    assert!(
        !worktree
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    assert_eq!(server.finish()?.len(), 12);
    Ok(())
}

fn author() -> Value {
    support::packet(
        support::message(
            &Value::object(vec![
                (
                    "checks",
                    Value::Array(vec![Value::object(vec![
                        ("id", Value::Number(1.0)),
                        ("met", Value::Boolean(true)),
                        (
                            "evidence",
                            Value::string(
                                "Current native operation and actual host checks were inspected.",
                            ),
                        ),
                    ])]),
                ),
                ("findings", Value::Array(Vec::new())),
            ])
            .json(),
        ),
        "stop",
        "actual-local-author-review",
    )
}

#[test]
fn outer_configured_native_cli_rust_repair_uses_cargo_context_fresh_checks_and_private_final_review()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let mut environment = fixture.native_cargo()?;
    put(
        &mut environment,
        "ROSTER_API_KEY",
        Value::string("test-only-private-api-key"),
    );
    fixture.task(&["rust/llm/worker.rs", "tests/added.rs"], "The native increment operation preserves zero, increments nonzero input and saturates maximum u8.")?;
    let initial = "pub fn next(value: u8) -> u8 {\n    value + 1\n}\n";
    let initial_test = "#[test]\nfn nonzero_increment() {\n    assert_eq!(owned_local_native_consumer::next(2), 3);\n}\n";
    let final_test = format!(
        "{initial_test}\n#[test]\nfn saturates_maximum() {{\n    assert_eq!(owned_local_native_consumer::next(u8::MAX), u8::MAX);\n}}\n"
    );
    let script = vec![
        support::calls(vec![(
            "read",
            "read_file",
            support::path("rust/llm/worker.rs"),
        )]),
        support::calls(vec![
            (
                "product",
                "write_file",
                write("rust/llm/worker.rs", initial),
            ),
            ("test", "write_file", write("tests/added.rs", initial_test)),
        ]),
        support::calls(vec![("matching", "run_test", Value::object(Vec::new()))]),
        support::summary(),
        author(),
        support::review(support::verdict(true)),
        support::review(support::verdict(false)),
        support::calls(vec![
            (
                "read-product",
                "read_file",
                support::path("rust/llm/worker.rs"),
            ),
            ("read-test", "read_file", support::path("tests/added.rs")),
        ]),
        support::calls(vec![
            (
                "repair-product",
                "edit_file",
                support::edit("rust/llm/worker.rs", "value + 1", "value.saturating_add(1)"),
            ),
            (
                "repair-test",
                "edit_file",
                support::edit("tests/added.rs", initial_test, &final_test),
            ),
        ]),
        support::calls(vec![(
            "fresh-matching",
            "run_test",
            Value::object(Vec::new()),
        )]),
        support::summary(),
        author(),
        support::review(support::verdict(true)),
        reviewed(true, 2),
    ];
    let server = support::Server::start_for(&fixture, script)?;
    configure(&fixture, &server.endpoint, &[])?;
    let output = fixture.invoke_with(
        &["run", "--seat", "coder", "--runtime", "builtin"],
        &environment,
        &fixture.root,
    )?;
    assert!(
        output.status.success(),
        "{}\n{}\nObserved requests: {}",
        text(&output.stdout)?,
        text(&output.stderr)?,
        server
            .requests
            .lock()
            .map_err(|_| Error::type_error("Owned request diagnostic lock"))?
            .len(),
    );
    assert!(text(&output.stdout)?.contains("Mode: verified\n"));
    assert!(text(&output.stdout)?.contains("1 review repair(s)"));
    let requests = server.finish()?;
    assert_eq!(requests.len(), 14);
    let fresh = system(
        requests
            .get(7)
            .ok_or_else(|| Error::type_error("Native repair context missing"))?,
    )?;
    assert!(fresh.contains("Run matching Cargo checks"));
    assert!(fresh.contains("rust-implementation"));
    assert!(fresh.contains("rust-cargo"));
    assert!(fresh.contains("rust-tokio"));
    assert!(!fresh.contains("rerun node --test"));
    let result = fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?;
    assert!(result.contains("Checks: PASS"));
    assert!(result.contains("cargo"));
    assert!(
        fs::read_to_string(fixture.root.join("rust").join("llm").join("worker.rs"))
            .map_err(io)?
            .contains("saturating_add(1)")
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("tests").join("added.rs")).map_err(io)?,
        final_test
    );
    assert!(
        fs::read_to_string(fixture.root.join("REVIEW.md"))
            .map_err(io)?
            .contains("Verdict: pass")
    );
    Ok(())
}

struct SharedAdmission {
    root: PathBuf,
    host: tools::Host,
    admission: Admission,
    held: Option<github_agent_roster::llm::admission::Lease>,
    queued: u32,
    writer_refused: bool,
}

impl Observer for SharedAdmission {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if event.get("type") == Some(&Value::string("perspective-escalation")) {
            let path = self.root.join(".roster").join("locks").join("run.lock");
            let before = secure::read(&path, 65_536)?
                .ok_or_else(|| Error::type_error("Writer disappeared between seats"))?;
            match Box::pin(backend::RootLease::open(
                &self.root,
                "owned-second-writer",
                self.host.clone(),
                Default::default(),
                CancellationSource::default().token(),
            ))
            .await
            {
                Ok(mut lease) => {
                    lease.close().await?;
                    return Err(Error::type_error("A second actual writer was admitted"));
                }
                Err(error) => assert_eq!(error.code, Some("E_LOCK_HELD")),
            }
            assert!(secure::unchanged(&path, &before, 65_536)?);
            self.writer_refused = true;
            self.held = Some(
                self.admission
                    .acquire(&Text::from("owned-fixed"), None, None)?
                    .wait()
                    .await?,
            );
        } else if event.get("queued") == Some(&Value::Boolean(true)) {
            let lease = self
                .held
                .as_mut()
                .ok_or_else(|| Error::type_error("Fresh seat queued without shared admission"))?;
            assert!(lease.release()?);
            self.held = None;
            self.queued += 1;
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn outer_fixed_profile_continuation_reuses_actual_shared_admission_and_refuses_second_writer()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let mut script = turn_script().into_iter().take(2).collect::<Vec<_>>();
    script.extend([
        support::summary(),
        support::review(support::verdict(true)),
        support::review(support::verdict(true)),
    ]);
    let server = support::Server::start(script)?;
    configure(
        &fixture,
        &server.endpoint,
        &[("turn_budget: 16", "turn_budget: 2")],
    )?;
    let profile = github_agent_roster::fleet::validate_profile(&Value::object(vec![
        ("id", Value::string("owned-fixed")),
        ("base_url", Value::string(&server.endpoint)),
        ("model", Value::string("configured-route-not-a-response")),
        ("provider", Value::string("vllm")),
        ("api_key_env", Value::string("ROSTER_API_KEY")),
        ("context_max", Value::Number(32768.0)),
        ("concurrency", Value::Number(1.0)),
        ("hardware", Value::string("Owned loopback")),
        (
            "notes",
            Value::string("Explicit fixed profile; no routing alternative."),
        ),
    ]))?;
    let admission = Admission::default();
    let (host, _) = fixture.host()?;
    let mut observer = SharedAdmission {
        root: fixture.root.clone(),
        host,
        admission: admission.clone(),
        held: None,
        queued: 0,
        writer_refused: false,
    };
    let outcome = Box::pin(local_profile(
        &fixture,
        &mut observer,
        CancellationSource::default().token(),
        &admission,
        false,
        Some(&profile),
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Verified, "{:?}", outcome.failure);
    assert_eq!(outcome.orchestration.counts.perspectives, 1);
    assert!(observer.writer_refused);
    assert_eq!(observer.queued, 1);
    assert!(observer.held.is_none());
    assert_eq!(admission.depth(&Text::from("owned-fixed"))?, 0);
    assert_eq!(server.finish()?.len(), 5);
    Ok(())
}

fn two_check_review(met: [bool; 2], previous: u32) -> Value {
    let pass = met.iter().all(|met| *met);
    let mut value = support::verdict(pass);
    put(
        &mut value,
        "checks",
        Value::Array(
            met.into_iter()
                .zip(1_u32..)
                .map(|(met, id)| {
                    Value::object(vec![
                        ("id", Value::Number(f64::from(id))),
                        ("met", Value::Boolean(met)),
                        (
                            "evidence",
                            Value::string(
                                "The current guide was inspected for this requested behavior.",
                            ),
                        ),
                    ])
                })
                .collect(),
        ),
    );
    if previous != 0 {
        put(
            &mut value,
            "findings",
            Value::Array(
                (1..=previous)
                    .map(|id| {
                        Value::object(vec![
                            ("id", Value::Number(f64::from(id))),
                            ("resolved", Value::Boolean(pass)),
                            (
                                "evidence",
                                Value::string(
                                    "The current delivered guide was independently inspected.",
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        );
    }
    support::review(value)
}

#[tokio::test(flavor = "current_thread")]
async fn outer_subset_of_previous_unmet_checks_also_requires_a_changed_strategy()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let ask = Text::from(
        "Document both requested guide requirements.\n\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide documents the first requirement.\n- The guide documents the second requirement.",
    );
    let plan = github_agent_roster::planning::artifacts::build(
        &Value::String(ask),
        github_agent_roster::planning::artifacts::BuildOptions {
            reference: Some("local:owned-task"),
            files: Some(&Value::Array(vec![Value::string("docs/guide.md")])),
            checks: Some(&Value::Array(vec![
                Value::string("The guide documents the first requirement."),
                Value::string("The guide documents the second requirement."),
            ])),
            ..Default::default()
        },
    )?;
    fixture.write(
        "TASK.md",
        &plan
            .task
            .to_utf8()?
            .replacen("---\n", "---\ntests: none\n", 1),
    )?;
    fixture.write("RECIPE.yml", &plan.recipe.to_utf8()?)?;
    let mut script = support::docs_script(true)
        .into_iter()
        .take(4)
        .collect::<Vec<_>>();
    script.push(two_check_review([false, false], 0));
    let mut first = repair(
        false,
        3,
        "The requested bounded outcome is documented.",
        "The requested bounded outcome now documents requirement one.",
        false,
    );
    let last = first
        .last_mut()
        .ok_or_else(|| Error::type_error("Subset first review missing"))?;
    *last = two_check_review([true, false], 3);
    script.extend(first);
    let mut second = repair(
        true,
        5,
        "now documents requirement one",
        "now documents both requirements",
        false,
    );
    let last = second
        .last_mut()
        .ok_or_else(|| Error::type_error("Subset final review missing"))?;
    *last = two_check_review([true, true], 5);
    script.extend(second);
    let server = support::Server::start_for(&fixture, script)?;
    configure(&fixture, &server.endpoint, &[])?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Verified, "{:?}", outcome.failure);
    assert_eq!(outcome.orchestration.counts.review_repairs, 2);
    let events = &outcome.orchestration.transitions;
    assert_eq!(
        events.first().and_then(|event| event.get("unmetChecks")),
        Some(&Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]))
    );
    assert_eq!(
        events.get(1).and_then(|event| event.get("unmetChecks")),
        Some(&Value::Array(vec![Value::Number(2.0)]))
    );
    assert_eq!(
        events
            .get(1)
            .and_then(|event| event.get("changed_perspective")),
        Some(&Value::Boolean(true))
    );
    let requests = server.finish()?;
    assert_eq!(requests.len(), 15);
    assert!(
        system(
            requests
                .get(10)
                .ok_or_else(|| Error::type_error("Subset changed-perspective context missing"))?
        )?
        .contains("do not repeat that approach")
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_real_original_ask_audit_failure_is_not_waived_by_green_numbered_checks()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let reason = "The guide omits the requested retained behavior.";
    let mut audit = support::verdict(false);
    put(
        &mut audit,
        "reasons",
        Value::Array(vec![Value::string(reason)]),
    );
    put(
        &mut audit,
        "grounds",
        Value::Array(vec![Value::object(vec![
            ("obligation", Value::string("preserve existing behavior")),
            ("file", Value::string("docs/guide.md")),
            (
                "excerpt",
                Value::string("The requested bounded outcome is documented."),
            ),
            ("reason", Value::string(reason)),
        ])]),
    );
    let mut script = support::docs_script(true);
    let audit_packet = script
        .get_mut(3)
        .ok_or_else(|| Error::type_error("Original audit packet missing"))?;
    *audit_packet = support::review(audit);
    script.extend(repair(
        true,
        2,
        "The requested bounded outcome is documented.",
        "The requested bounded outcome is documented with its retained behavior.",
        false,
    ));
    let server = support::Server::start_for(&fixture, script)?;
    configure(&fixture, &server.endpoint, &[])?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Verified, "{:?}", outcome.failure);
    assert_eq!(outcome.orchestration.counts.review_repairs, 1);
    let first_review = outcome
        .orchestration
        .attempts
        .iter()
        .find(|attempt| attempt.seat == "reviewer")
        .ok_or_else(|| Error::type_error("Original audited review missing"))?;
    assert_eq!(first_review.status, "fail");
    assert_eq!(
        first_review
            .ask_audit
            .as_ref()
            .and_then(|audit| audit.get("verdict")),
        Some(&Value::string("fail"))
    );
    assert_eq!(
        outcome
            .orchestration
            .transitions
            .first()
            .and_then(|event| event.get("unmetChecks")),
        Some(&Value::Array(Vec::new()))
    );
    let requests = server.finish()?;
    assert_eq!(requests.len(), 10);
    let fresh = system(
        requests
            .get(5)
            .ok_or_else(|| Error::type_error("Audit repair context missing"))?,
    )?;
    assert!(fresh.contains("Original Ask audit:"));
    assert!(fresh.contains("preserve existing behavior"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_actual_git_change_during_model_request_revokes_pending_writes_and_perspective()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    fixture.git(&["branch", "owned-alternative"])?;
    let root = fixture.root.clone();
    let git = fixture.git.clone();
    let server =
        support::Server::observed(turn_script().into_iter().take(1).collect(), move |_, _| {
            let output = std::process::Command::new(&git)
                .current_dir(&root)
                .env_clear()
                .envs(support::minimal()?)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                )
                .args([
                    "--no-pager",
                    "symbolic-ref",
                    "HEAD",
                    "refs/heads/owned-alternative",
                ])
                .output()
                .map_err(io)?;
            if !output.status.success() {
                return Err(Error::new(
                    "OwnedGitDriftFailure",
                    text(&output.stderr)?.to_owned(),
                ));
            }
            Ok(())
        })?;
    configure(
        &fixture,
        &server.endpoint,
        &[("turn_budget: 16", "turn_budget: 1")],
    )?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::PolicyDenied));
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert!(outcome.orchestration.archives.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    assert_eq!(
        fixture.git(&["branch", "--show-current"])?.trim(),
        "owned-alternative"
    );
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_repeated_secret_content_denial_cannot_reset_its_correction_in_a_perspective()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let denied = support::calls(vec![(
        "denied",
        "write_file",
        write("docs/guide.md", "# Guide\n\ntest-only-private-api-key\n"),
    )]);
    let server = support::Server::start(vec![denied.clone(), denied])?;
    configure(&fixture, &server.endpoint, &[])?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(outcome.coder_handoff, Some(Handoff::NoProgress));
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert!(outcome.orchestration.archives.is_empty());
    assert_eq!(
        outcome
            .coder_result
            .as_ref()
            .and_then(|result| result.get("corrections"))
            .and_then(|value| value.get("secret")),
        Some(&Value::Number(1.0))
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    assert!(
        !fs::read_to_string(fixture.root.join("RESULT.md"))
            .map_err(io)?
            .contains("test-only-private-api-key")
    );
    assert_eq!(server.finish()?.len(), 2);
    Ok(())
}

#[derive(Default)]
struct StoppedFailure(u32);

impl Observer for StoppedFailure {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if event.get("type") == Some(&Value::string("seat-stopped"))
            && event.get("seat") == Some(&Value::string("coder"))
        {
            self.0 += 1;
            return Err(Error::new(
                "OwnedStoppedObserver",
                "Actual stopped-seat notification failed",
            ));
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn outer_stop_notification_exception_is_surfaced_without_starting_a_new_context()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let server = support::Server::start(turn_script().into_iter().take(1).collect())?;
    configure(
        &fixture,
        &server.endpoint,
        &[("turn_budget: 16", "turn_budget: 1")],
    )?;
    let mut observer = StoppedFailure::default();
    let outcome = Box::pin(local(
        &fixture,
        &mut observer,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Unverified);
    assert_eq!(
        outcome.failure.as_ref().and_then(|error| error.code),
        Some("ROSTER_SEAT_TURN_BUDGET")
    );
    assert_ne!(
        outcome.failure.as_ref().map(|error| error.name),
        Some("OwnedStoppedObserver")
    );
    assert!(outcome.format().contains("OwnedStoppedObserver"));
    assert_eq!(outcome.coder_handoff, Some(Handoff::TurnBudget));
    assert_eq!(outcome.orchestration.counts, Counts::default());
    assert!(outcome.orchestration.archives.is_empty());
    assert_eq!(outcome.orchestration.attempts.len(), 1);
    assert_eq!(
        outcome
            .orchestration
            .attempts
            .first()
            .and_then(|attempt| attempt.continuation_failure.as_ref())
            .map(|error| error.name),
        Some("OwnedStoppedObserver")
    );
    assert_eq!(observer.0, 1);
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_bounded_real_unicode_findings_keep_valid_context_and_all_fresh_review_obligations()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    docs_task(&fixture)?;
    let mut report = support::verdict(false);
    let reasons = (0..7)
        .map(|index| {
            Value::string(&match index {
                5 => format!("{}😀{}", "a".repeat(482), "b".repeat(16)),
                6 => "The remaining concrete guide finding must also be resolved.".to_owned(),
                _ => "a".repeat(500),
            })
        })
        .collect();
    put(&mut report, "reasons", Value::Array(reasons));
    put(
        &mut report,
        "checks",
        support::verdict(true)
            .get("checks")
            .cloned()
            .ok_or_else(|| Error::type_error("Owned Unicode review checks missing"))?,
    );
    let mut script = support::docs_script(true)
        .into_iter()
        .take(4)
        .collect::<Vec<_>>();
    script.push(support::review(report));
    script.extend(repair(
        true,
        7,
        "The requested bounded outcome is documented.",
        "The requested bounded outcome is documented after every finding was addressed.",
        false,
    ));
    let server = support::Server::start_for(&fixture, script)?;
    configure(&fixture, &server.endpoint, &[])?;
    let outcome = Box::pin(local(
        &fixture,
        &mut NoObserver,
        CancellationSource::default().token(),
        &Admission::default(),
        false,
    ))
    .await?;
    assert_eq!(outcome.status(), Status::Verified, "{:?}", outcome.failure);
    assert_eq!(outcome.orchestration.counts.review_repairs, 1);
    let requests = server.finish()?;
    assert_eq!(requests.len(), 10);
    let fresh = system(
        requests
            .get(5)
            .ok_or_else(|| Error::type_error("Unicode repair context missing"))?,
    )?;
    assert!(fresh.contains("Actual bounded redacted findings"));
    let last = requests
        .last()
        .ok_or_else(|| Error::type_error("Unicode fresh reviewer missing"))?
        .json();
    assert!(last.contains("remaining concrete guide finding"));
    let findings = outcome
        .reviewer_report
        .as_ref()
        .and_then(|report| report.get("findings"))
        .and_then(|value| match value {
            Value::Array(items) => Some(items),
            _ => None,
        })
        .ok_or_else(|| Error::type_error("Unicode complete fresh findings mapping missing"))?;
    assert_eq!(findings.len(), 7);
    Ok(())
}
