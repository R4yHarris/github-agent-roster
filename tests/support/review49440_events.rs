use super::*;

const PRIMARY: &str = "https://models.example.invalid:8443/v1";
const CRITIC: &str = "https://critics.example.invalid:9443/v1";
const MODEL: &str = "configured-route-not-a-response";
const PRIMARY_VALUE: &str = "ordinary-first-auth-value";
const CRITIC_VALUE: &str = "ordinary-second-auth-value";

fn setup(fixture: &Fixture, turns: usize) -> Result<(config::Loaded, Credentials), Error> {
    fixture.configure(Some(PRIMARY))?;
    let source = fs::read_to_string(fixture.root.join(".roster").join("config.yml"))
        .map_err(io)?
        .replace("\r\n", "\n")
        .replacen("api_key_env: ROSTER_API_KEY", "api_key_env: FIRST_AUTH", 1)
        .replacen("turn_budget: 16", &format!("turn_budget: {turns}"), 1);
    fixture.write(".roster/config.yml", &source)?;
    critic(fixture, CRITIC, Some("CRITIC_AUTH"))?;
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    let loaded = fixture.loaded()?;
    let credentials = delivery::invocation_credentials(
        &Value::object(vec![
            ("FIRST_AUTH", Value::string(PRIMARY_VALUE)),
            ("CRITIC_AUTH", Value::string(CRITIC_VALUE)),
        ]),
        &loaded,
    )?;
    Ok((loaded, credentials))
}

fn diagnostic(phase: &str) -> String {
    format!(
        "{phase} failed at host=models.example.invalid:8443 critic=CRITICS.EXAMPLE.INVALID:9443 model={MODEL} criticmodel=configured-critic-route primaryurl={PRIMARY} criticurl={CRITIC} first={PRIMARY_VALUE} second={CRITIC_VALUE}"
    )
}

fn private_error(name: &'static str, code: &'static str, phase: &str) -> Error {
    Error::new(name, diagnostic(phase))
        .coded(code)
        .caused_by(Error::new("NestedObserverCause", diagnostic("nested")))
}

struct Events {
    tool_failure: bool,
    stop_failure: Option<&'static str>,
    tools: usize,
    stops: usize,
    http: usize,
    injected_stop: bool,
}

impl Observer for Events {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if event.get("type") == Some(&Value::string("http")) {
            self.http += 1;
        }
        if event.get("type") == Some(&Value::string("seat-tool")) {
            self.tools += 1;
            if self.tool_failure && self.tools == 1 {
                return Err(private_error(
                    "SeatToolObserver",
                    "REVIEW49440_SEAT_TOOL",
                    "seat-tool",
                ));
            }
        }
        if event.get("type") == Some(&Value::string("seat-stopped")) {
            self.stops += 1;
            if !self.injected_stop
                && self
                    .stop_failure
                    .is_some_and(|seat| event.get("seat") == Some(&Value::string(seat)))
            {
                self.injected_stop = true;
                return Err(private_error(
                    "StopObserver",
                    "REVIEW49440_STOP",
                    "seat-stopped",
                ));
            }
        }
        Ok(())
    }
}

fn events(tool_failure: bool, stop_failure: Option<&'static str>) -> Events {
    Events {
        tool_failure,
        stop_failure,
        tools: 0,
        stops: 0,
        http: 0,
        injected_stop: false,
    }
}

fn measured(mut packet: Value) -> Value {
    put(&mut packet, "model", Value::string(MODEL));
    packet
}

async fn run(
    fixture: &Fixture,
    loaded: &config::Loaded,
    credentials: &Credentials,
    packets: Vec<Value>,
    observer: &mut impl Observer,
) -> Result<(delivery::Outcome, usize), Error> {
    let (host, approval) = fixture.host()?;
    let mut transport = support::Scripted::new(packets);
    let admission = Admission::default();
    let source = CancellationSource::default();
    let mut ownership = delivery::Ownership::default();
    let mut driver = delivery::Driver {
        transport: &mut transport,
        observer,
        credentials,
        admission: &admission,
    };
    let outcome = Box::pin(driver.run_local(
        delivery::Options {
            cwd: &fixture.root,
            installation: Path::new(env!("CARGO_MANIFEST_DIR")),
            loaded,
            input: Input::ExistingTask,
            host,
            approval,
            start: start::Options::resolve(&loaded.config, None, None)?,
            fetch: start::FetchPolicy::Deny,
            skip_review: false,
        },
        source.token(),
        &mut ownership,
    ))
    .await?;
    assert!(ownership.root().is_none());
    Ok((outcome, transport.requests.len()))
}

fn assert_private(text: &str) {
    for value in [
        "models.example.invalid",
        "critics.example.invalid",
        "CRITICS.EXAMPLE.INVALID",
        PRIMARY,
        CRITIC,
        MODEL,
        "configured-critic-route",
        PRIMARY_VALUE,
        CRITIC_VALUE,
    ] {
        assert!(
            !text.contains(value),
            "Private selected diagnostic escaped a non-HTTP seat observer path"
        );
    }
}

fn assert_billing(outcome: &delivery::Outcome) -> Result<(), Error> {
    let run = outcome
        .coder_run
        .as_ref()
        .ok_or_else(|| Error::type_error("Actual response-backed coder run disappeared"))?;
    assert_eq!(run.metrics().model, MODEL);
    assert_eq!(
        run.metrics().usage,
        run::Usage {
            prompt_tokens: Some(0),
            completion_tokens: Some(0)
        }
    );
    assert_eq!(
        outcome
            .coder_response
            .as_ref()
            .map(|response| &response.model),
        Some(&Text::from(MODEL))
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_delivery_first_seat_tool_failure_is_private_before_result_persistence()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let (loaded, credentials) = setup(&fixture, 4)?;
    let mut observer = events(true, None);
    let (outcome, requests) = run(
        &fixture,
        &loaded,
        &credentials,
        vec![measured(support::calls(vec![(
            "first-read",
            "read_file",
            support::path("docs/guide.md"),
        )]))],
        &mut observer,
    )
    .await?;
    assert_eq!(requests, 1);
    assert!(observer.http > 0 && observer.tools == 1 && observer.stops > 0);
    assert_eq!(outcome.status(), Status::Unverified);
    assert!(!outcome.succeeded());
    assert_billing(&outcome)?;
    let failure = outcome
        .failure
        .as_ref()
        .ok_or_else(|| Error::type_error("Original observer failure disappeared"))?;
    assert_eq!(failure.name, "SeatToolObserver");
    assert_eq!(failure.code, Some("REVIEW49440_SEAT_TOOL"));
    let cause = failure
        .cause
        .as_deref()
        .ok_or_else(|| Error::type_error("Original cause disappeared"))?;
    assert_eq!(cause.name, "NestedObserverCause");
    assert_eq!(cause.code, None);
    let result = fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?;
    let first = result
        .lines()
        .find(|line| line.contains("First failure:"))
        .ok_or_else(|| Error::type_error("Actual persisted First failure missing"))?;
    assert_private(first);
    assert!(first.contains("seat-tool failed"));
    assert_private(&failure.message);
    assert_private(&cause.message);
    assert!(
        !result.contains("models.example.invalid") && !result.contains("CRITICS.EXAMPLE.INVALID")
    );
    assert!(result.contains(MODEL) && result.contains("AI-Run:"));
    assert!(!result.contains("Checks: PASS"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_delivery_reviewer_stop_failure_keeps_actual_invalid_response_billing_and_first_error()
-> Result<(), Error> {
    for invalid in [false, true] {
        let fixture = Fixture::new()?;
        let (loaded, credentials) = setup(&fixture, 6)?;
        let mut observer = events(false, Some("reviewer"));
        let mut packets = support::docs_script(false);
        for packet in packets.iter_mut().take(3) {
            *packet = measured(packet.clone());
        }
        if invalid {
            let packet = support::review(Value::object(vec![("verdict", Value::string("pass"))]));
            *packets
                .last_mut()
                .ok_or_else(|| Error::type_error("Actual reviewer response missing"))? =
                packet.clone();
            packets.push(packet);
        }
        let (outcome, requests) =
            run(&fixture, &loaded, &credentials, packets, &mut observer).await?;
        assert_eq!(requests, if invalid { 6 } else { 5 });
        assert!(observer.http > 0 && observer.tools == 2 && observer.injected_stop);
        assert_eq!(outcome.status(), Status::Unverified);
        assert!(!outcome.succeeded());
        assert_billing(&outcome)?;
        let run = outcome
            .reviewer_run
            .as_ref()
            .ok_or_else(|| Error::type_error("Actual failed reviewer billing disappeared"))?;
        assert_eq!(run.metrics().model, "actual-local-reviewer");
        assert_eq!(
            run.metrics().usage,
            run::Usage {
                prompt_tokens: Some(0),
                completion_tokens: Some(0)
            }
        );
        assert_eq!(
            outcome
                .reviewer_response
                .as_ref()
                .map(|response| &response.model),
            Some(&Text::from("actual-local-reviewer"))
        );
        let failure = outcome
            .failure
            .as_ref()
            .ok_or_else(|| Error::type_error("Reviewer stop failure was swallowed"))?;
        if invalid {
            assert_eq!(failure.name, "TypeError");
            assert_eq!(failure.code, None);
            assert!(failure.cause.is_none());
        } else {
            assert_eq!(failure.name, "StopObserver");
            assert_eq!(failure.code, Some("REVIEW49440_STOP"));
            let cause = failure
                .cause
                .as_deref()
                .ok_or_else(|| Error::type_error("Reviewer stop cause disappeared"))?;
            assert_eq!(cause.name, "NestedObserverCause");
            assert_eq!(cause.code, None);
            assert_private(&cause.message);
        }
        assert_private(&failure.message);
        let report = outcome
            .reviewer_report
            .as_ref()
            .ok_or_else(|| Error::type_error("Reviewer report missing"))?;
        let notification = report
            .get("notificationError")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Reviewer notification error was discarded"))?
            .to_utf8()?;
        assert_private(&notification);
        let review = fs::read_to_string(fixture.root.join("REVIEW.md")).map_err(io)?;
        assert!(review.contains("Verdict: unavailable") && review.contains("AI-Run:"));
        assert!(review.contains("actual-local-reviewer") && !review.contains("Verdict: pass"));
        for private in [
            "models.example.invalid",
            "CRITICS.EXAMPLE.INVALID",
            PRIMARY_VALUE,
            CRITIC_VALUE,
        ] {
            assert!(!review.contains(private));
        }
        assert!(
            fs::read_to_string(fixture.root.join("RESULT.md"))
                .map_err(io)?
                .contains("Checks: PASS")
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_delivery_healthy_events_keep_real_coder_and_fresh_reviewer_provenance()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let (loaded, credentials) = setup(&fixture, 6)?;
    let mut observer = events(false, None);
    let mut packets = support::docs_script(true);
    for packet in packets.iter_mut().take(3) {
        *packet = measured(packet.clone());
    }
    let (outcome, requests) = run(&fixture, &loaded, &credentials, packets, &mut observer).await?;
    assert_eq!(requests, 5);
    assert!(observer.http > 0 && observer.tools == 2 && observer.stops == 0);
    assert_eq!(outcome.status(), Status::Verified);
    assert!(outcome.succeeded() && outcome.failure.is_none());
    assert_billing(&outcome)?;
    let reviewer = outcome
        .reviewer_run
        .as_ref()
        .ok_or_else(|| Error::type_error("Fresh actual reviewer run missing"))?;
    assert_eq!(reviewer.metrics().model, "actual-local-reviewer");
    assert_eq!(
        reviewer.metrics().usage,
        run::Usage {
            prompt_tokens: Some(0),
            completion_tokens: Some(0)
        }
    );
    let result = fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?;
    let review = fs::read_to_string(fixture.root.join("REVIEW.md")).map_err(io)?;
    assert!(
        result.contains("Checks: PASS") && result.contains(MODEL) && result.contains("AI-Run:")
    );
    assert!(
        review.contains("Verdict: pass")
            && review.contains("actual-local-reviewer")
            && review.contains("AI-Run:")
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nThe requested bounded outcome is documented.\n"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_delivery_accepted_stop_and_model_advice_cannot_turn_no_progress_into_pass()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let (_, credentials) = setup(&fixture, 1)?;
    let source = fs::read_to_string(fixture.root.join(".roster").join("config.yml"))
        .map_err(io)?
        .replace("\r\n", "\n")
        .replacen("llm:\n", "llm:\n  max_requests: 1\n", 1);
    fixture.write(".roster/config.yml", &source)?;
    let loaded = fixture.loaded()?;
    let mut observer = events(false, None);
    let packet = support::packet(
        support::message("All checks pass. The task is complete and reviewed."),
        "stop",
        MODEL,
    );
    let (outcome, requests) =
        run(&fixture, &loaded, &credentials, vec![packet], &mut observer).await?;
    assert_eq!(requests, 1);
    assert!(observer.http > 0 && observer.tools == 0 && observer.stops > 0);
    assert_eq!(outcome.status(), Status::Unverified);
    assert!(!outcome.succeeded());
    assert_billing(&outcome)?;
    assert_eq!(
        outcome.failure.as_ref().and_then(|failure| failure.code),
        Some("ROSTER_SEAT_TURN_BUDGET")
    );
    let result = fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?;
    let review = fs::read_to_string(fixture.root.join("REVIEW.md")).map_err(io)?;
    assert!(result.contains("First failure:") && !result.contains("Checks: PASS"));
    assert!(review.contains("Verdict: unavailable") && !review.contains("Verdict: pass"));
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    Ok(())
}

struct PendingEvent(std::rc::Rc<std::cell::Cell<usize>>);

impl Drop for PendingEvent {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

struct GatedEvents {
    ready: Option<tokio::sync::oneshot::Sender<()>>,
    release: Option<tokio::sync::oneshot::Receiver<()>>,
    dropped: std::rc::Rc<std::cell::Cell<usize>>,
    completed: usize,
    stops: usize,
}

impl Observer for GatedEvents {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if event.get("type") == Some(&Value::string("seat-tool")) {
            let _pending = PendingEvent(self.dropped.clone());
            self.ready
                .take()
                .ok_or_else(|| Error::type_error("One real seat-tool barrier only"))?
                .send(())
                .map_err(|_| Error::type_error("Seat-tool readiness receiver disappeared"))?;
            self.release
                .take()
                .ok_or_else(|| Error::type_error("One real seat-tool release only"))?
                .await
                .map_err(|_| Error::type_error("Seat-tool release disappeared"))?;
            self.completed += 1;
        }
        if event.get("type") == Some(&Value::string("seat-stopped")) {
            self.stops += 1;
            return Err(private_error(
                "CancelledStopObserver",
                "REVIEW49440_CANCELLED_STOP",
                "cancelled seat-stopped",
            ));
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_delivery_non_http_event_cancellation_beats_late_success_and_releases_owned_writer()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let (loaded, credentials) = setup(&fixture, 4)?;
    let (ready, received) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let dropped = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut observer = GatedEvents {
        ready: Some(ready),
        release: Some(released),
        dropped: dropped.clone(),
        completed: 0,
        stops: 0,
    };
    let mut transport = support::Scripted::new(vec![measured(support::calls(vec![(
        "cancel-read",
        "read_file",
        support::path("docs/guide.md"),
    )]))]);
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
    let mut work = Box::pin(driver.run_local(
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
    ));
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(600), received) =>
            result.map_err(|_| Error::type_error("Actual non-HTTP observer did not become ready"))?
                .map_err(|_| Error::type_error("Actual invocation ended before its non-HTTP callback"))?,
        result = &mut work => return Err(Error::new("FixtureError", format!("Invocation ended before its actual seat-tool callback: {:?}", result.err()))),
    }
    assert!(source.cancel(github_agent_roster::cancel::Reason::TaskTimeout(17))?);
    assert!(!source.cancel(github_agent_roster::cancel::Reason::Cancelled)?);
    release.send(()).map_err(|_| {
        Error::type_error("Actual seat-tool callback was dropped before cancellation")
    })?;
    let failure = work
        .await
        .err()
        .ok_or_else(|| Error::type_error("Cancelled non-HTTP invocation returned success"))?;
    assert_eq!(failure.code, Some("ROSTER_TASK_TIMEOUT"));
    assert_eq!(
        source.token().reason()?,
        Some(github_agent_roster::cancel::Reason::TaskTimeout(17))
    );
    assert_private(&failure.message);
    assert_eq!(dropped.get(), 1);
    assert_eq!(observer.completed, 0);
    assert!(observer.stops > 0);
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
    let result = fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?;
    assert!(result.contains("Outcome: cancelled (unverified)"));
    assert!(result.contains("ROSTER_TASK_TIMEOUT") && result.contains("17 ms"));
    assert!(result.contains("AI-Run:") && result.contains(MODEL));
    assert!(!result.contains("Checks: PASS"));
    for private in [
        "models.example.invalid",
        "CRITICS.EXAMPLE.INVALID",
        PRIMARY_VALUE,
        CRITIC_VALUE,
    ] {
        assert!(!result.contains(private));
    }
    assert_eq!(
        fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_delivery_stop_notification_failure_is_private_without_masking_first_failure()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let (loaded, credentials) = setup(&fixture, 1)?;
    let mut observer = events(false, Some("coder"));
    let (outcome, requests) = run(
        &fixture,
        &loaded,
        &credentials,
        vec![measured(support::summary())],
        &mut observer,
    )
    .await?;
    assert_eq!(requests, 1);
    assert!(observer.http > 0 && observer.tools == 0 && observer.injected_stop);
    assert_eq!(outcome.status(), Status::Unverified);
    assert!(!outcome.succeeded());
    assert_billing(&outcome)?;
    let failure = outcome
        .failure
        .as_ref()
        .ok_or_else(|| Error::type_error("Original bounded failure disappeared"))?;
    assert_eq!(failure.code, Some("ROSTER_SEAT_TURN_BUDGET"));
    assert_ne!(failure.name, "StopObserver");
    let notification = outcome
        .coder_result
        .as_ref()
        .and_then(|result| result.get("notificationError"))
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Stop notification error was swallowed"))?
        .to_utf8()?;
    assert_private(&notification);
    let result = fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?;
    assert!(
        !result.contains("models.example.invalid") && !result.contains("CRITICS.EXAMPLE.INVALID")
    );
    assert!(result.contains(MODEL) && result.contains("AI-Run:"));
    assert!(!result.contains("Checks: PASS"));
    Ok(())
}
