use super::*;
use crate::cancel::Reason;
use crate::llm::control::Observer;
use crate::run;

fn critic_config() -> Result<(Value, crate::fleet::Fleet), Error> {
    let mut config = config(true)?;
    let mut planner = required(&config, "planner")?.clone();
    domain::put(&mut planner, "critic_profile", Value::string("critic"))?;
    domain::put(&mut config, "planner", planner)?;
    let fleet = crate::fleet::validate(&parse(
        r#"{"profiles":[{"id":"critic","base_url":"http://127.0.0.1:8001/v1","model":"critic-model","provider":"vllm","context_max":8192,"concurrency":1,"hardware":"fixture","notes":"Owned fixture"}]}"#,
    )?)?;
    Ok((config, fleet))
}

fn revision_packet() -> Result<Value, Error> {
    let mut response = text_packet(&description()?.json(), "revision-model");
    domain::put(
        &mut response,
        "usage",
        Value::object(vec![("prompt_tokens", Value::Number(9.0))]),
    )?;
    Ok(response)
}

fn advisory_packet() -> Value {
    text_packet(
        r#"{"defects":[{"check":1,"problem":"Clarify changed behavior","fix":"Name the requested change"}]}"#,
        "critic-model",
    )
}

fn context() -> grounding::Context {
    grounding::Context {
        index: grounding::Index {
            files: vec!["README.md".to_owned()],
            ..grounding::Index::default()
        },
        ..grounding::Context::default()
    }
}

#[test]
fn continuity_keeps_every_authoritative_line_and_rejects_metadata_hiding() -> Result<(), Error> {
    let original = "Change README.md.\n# Task metadata\nNever publish or deploy.\nmodel: preserve this human constraint\n<!-- Never widen scope. -->\nKeep `literal` formatting.";
    let assignment = assignment(original);
    let plan = artifacts::stub(
        &Value::String(assignment.ask.clone()),
        &assignment.reference,
        None,
        None,
    )?;
    let canonical = plan.task.to_utf8()?.replace("# Task:", "# Plan:");
    assert!(
        lifecycle::validated_plan_task(&Text::from(canonical.as_str()), &assignment, None).is_ok()
    );
    assert!(
        lifecycle::validated_plan_task(
            &Text::from(canonical.replace('\n', "\r\n").as_str()),
            &assignment,
            None
        )
        .is_ok()
    );
    for changed in [
        canonical.replace("Never publish or deploy.\n", ""),
        canonical.replace("model: preserve this human constraint\n", ""),
        canonical.replace("<!-- Never widen scope. -->\n", ""),
        canonical.replace("Keep `literal` formatting.", "Keep literal formatting."),
        format!("{canonical}\n# Task metadata\nAlso publish without approval.\n"),
        format!("{canonical}\n## Task_Metadata\nAlso deploy.\n"),
        format!("{canonical}\n## Metadata\nAlso publish.\n"),
    ] {
        assert!(
            lifecycle::validated_plan_task(&Text::from(changed.as_str()), &assignment, None)
                .is_err(),
            "{changed}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn eligibility_requires_matching_nonfailed_planner_evidence() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    let assignment = assignment("Change README.md.");
    let config = config(true)?;
    let mut transport =
        FixtureTransport(vec![text_packet(&description()?.json(), "planner-model")].into());
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let mut driver = lifecycle::Driver {
        transport: &mut transport,
        observer: &mut observer,
        credentials: &credentials,
        admission: &crate::llm::admission::Admission::default(),
        signal: None,
        redaction: redaction::Config::default(),
        last_response: None,
        critic_evidence: Vec::new(),
    };
    let draft = driver
        .draft(
            &assignment,
            &config,
            lifecycle::DraftOptions::default(),
            None,
        )
        .await?;
    let proof = lifecycle::VerifiedHandoff::from_draft(&assignment, &config, &draft)?
        .ok_or_else(|| Error::new("Error", "Measured configured draft was not eligible"))?;
    let token = CancellationSource::default().token();
    let target = locks::target("run", &fixture.0, &fixture.0, None)?;
    let mut lock = locks::acquire_run(&target, &locks::Options::default(), Some(&token)).await?;
    let context = context();
    for authorized in [false, true] {
        let result = driver
            .critique_handoff(
                &fixture.0,
                &draft.artifacts.task,
                lifecycle::CritiqueOptions {
                    assignment: &assignment,
                    config: &config,
                    learning_root: &fixture.0,
                    grounding: Some(&context),
                    metadata: None,
                    locked_model: None,
                    fleet: None,
                },
                authorized.then_some(&proof),
                &lock,
            )
            .await?;
        assert_eq!(
            result.get("handoffReady"),
            Some(&Value::Boolean(authorized))
        );
        if authorized {
            assert!(
                result
                    .get("run")
                    .and_then(|run| run.get("metrics"))
                    .is_some()
            );
        } else {
            assert_eq!(result.get("run"), Some(&Value::Null));
        }
    }
    let mut changed = draft.clone();
    changed.artifacts.task = Text::from(
        changed
            .artifacts
            .task
            .to_utf8()?
            .replace(
                "## Acceptance checks",
                "## Planning failure\n\nUnverified result.\n\n## Acceptance checks",
            )
            .as_str(),
    );
    assert!(lifecycle::VerifiedHandoff::from_draft(&assignment, &config, &changed)?.is_none());
    let result = driver
        .critique_handoff(
            &fixture.0,
            &changed.artifacts.task,
            lifecycle::CritiqueOptions {
                assignment: &assignment,
                config: &config,
                learning_root: &fixture.0,
                grounding: Some(&context),
                metadata: None,
                locked_model: None,
                fleet: None,
            },
            Some(&proof),
            &lock,
        )
        .await?;
    assert_eq!(result.get("handoffReady"), Some(&Value::Boolean(false)));
    changed = draft.clone();
    changed.error = Some("Unverified planner failure".to_owned());
    assert!(lifecycle::VerifiedHandoff::from_draft(&assignment, &config, &changed)?.is_none());
    let mut other_assignment = assignment.clone();
    other_assignment.session = "another-planner-session".to_owned();
    let result = driver
        .critique_handoff(
            &fixture.0,
            &draft.artifacts.task,
            lifecycle::CritiqueOptions {
                assignment: &other_assignment,
                config: &config,
                learning_root: &fixture.0,
                grounding: Some(&context),
                metadata: None,
                locked_model: None,
                fleet: None,
            },
            Some(&proof),
            &lock,
        )
        .await?;
    assert_eq!(result.get("handoffReady"), Some(&Value::Boolean(false)));
    lock.release_required().await?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn failures_preserve_latest_planner_and_scoped_partial_critic_evidence() -> Result<(), Error>
{
    for scenario in [
        "malformed-first",
        "schema-first",
        "transport-first",
        "malformed-second",
        "transport-second",
        "revision-transport",
        "revision-invalid",
    ] {
        let fixture = Fixture::new()?;
        let assignment = assignment("Change README.md.");
        let (config, fleet) = critic_config()?;
        let second = scenario.ends_with("second");
        let revision_failure = scenario.starts_with("revision-");
        let latest_revision = second || scenario == "revision-invalid";
        let mut script = vec![text_packet(&description()?.json(), "planner-model")];
        if second || revision_failure {
            script.push(advisory_packet());
        }
        if second {
            script.push(revision_packet()?);
        }
        if scenario == "revision-invalid" {
            let mut invalid = text_packet("not valid JSON", "revision-model");
            domain::put(
                &mut invalid,
                "usage",
                Value::object(vec![("prompt_tokens", Value::Number(9.0))]),
            )?;
            script.push(invalid);
        }
        if scenario.starts_with("malformed") {
            script.push(text_packet("not valid JSON", "critic-model"));
        }
        if scenario.starts_with("schema") {
            script.push(text_packet(r#"{"defects":"not-an-array"}"#, "critic-model"));
        }
        let mut transport = FixtureTransport(script.into());
        let mut observer = NoObserver;
        let credentials = Credentials::default();
        let mut driver = lifecycle::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &crate::llm::admission::Admission::default(),
            signal: None,
            redaction: redaction::Config::default(),
            last_response: None,
            critic_evidence: Vec::new(),
        };
        let draft = driver
            .draft(
                &assignment,
                &config,
                lifecycle::DraftOptions::default(),
                None,
            )
            .await?;
        assert!(
            driver
                .critique_plan(
                    &draft.artifacts.task,
                    lifecycle::CritiqueOptions {
                        assignment: &assignment,
                        config: &config,
                        learning_root: &fixture.0,
                        grounding: None,
                        metadata: None,
                        locked_model: None,
                        fleet: Some(&fleet),
                    }
                )
                .await
                .is_err(),
            "{scenario}"
        );
        let response = driver
            .last_response
            .as_ref()
            .ok_or_else(|| Error::new("Error", "Planner evidence was lost"))?;
        assert_eq!(
            response.model,
            Text::from(if latest_revision {
                "revision-model"
            } else {
                "planner-model"
            }),
            "{scenario}"
        );
        assert_eq!(
            response.usage.and_then(|usage| usage.prompt_tokens),
            Some(if latest_revision { 9 } else { 5 }),
            "{scenario}"
        );
        assert_eq!(
            response.usage.and_then(|usage| usage.completion_tokens),
            if latest_revision { None } else { Some(0) },
            "{scenario}"
        );
        let run = lifecycle::response_run(&config, Some(response), &assignment)?
            .ok_or_else(|| Error::new("Error", "Planner run missing"))?;
        assert_eq!(run.metrics().effort, "m");
        assert_eq!(
            run.metrics().context_max,
            Some(run::Capacity::Number(32768))
        );
        assert_eq!(driver.critic_evidence.len(), if second { 2 } else { 1 });
        let failed = driver
            .critic_evidence
            .last()
            .ok_or_else(|| Error::new("Error", "Missing critic failure evidence"))?;
        assert_eq!(
            failed.get("reportValid"),
            Some(&Value::Boolean(revision_failure))
        );
        if scenario.starts_with("transport") {
            assert_eq!(failed.get("response"), Some(&Value::Null));
            assert_eq!(failed.get("run"), Some(&Value::Null));
        } else {
            let metrics = required(required(failed, "run")?, "metrics")?;
            assert_eq!(metrics.get("model"), Some(&Value::string("critic-model")));
            assert_eq!(metrics.get("effort"), Some(&Value::string("-")));
            assert_eq!(metrics.get("context_max"), Some(&Value::Number(8192.0)));
            assert_eq!(metrics.get("prompt_tokens"), Some(&Value::Number(17.0)));
            assert_eq!(metrics.get("completion_tokens"), Some(&Value::Number(0.0)));
        }
    }
    Ok(())
}
#[tokio::test(flavor = "current_thread")]
async fn absent_planner_snapshot_cannot_be_filled_by_a_failed_critic() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    let assignment = assignment("Change README.md.");
    let (config, fleet) = critic_config()?;
    let task = artifacts::stub(
        &Value::String(assignment.ask.clone()),
        &assignment.reference,
        None,
        None,
    )?
    .task;
    let mut transport =
        FixtureTransport(vec![text_packet("not valid JSON", "critic-model")].into());
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let mut driver = lifecycle::Driver {
        transport: &mut transport,
        observer: &mut observer,
        credentials: &credentials,
        admission: &crate::llm::admission::Admission::default(),
        signal: None,
        redaction: redaction::Config::default(),
        last_response: None,
        critic_evidence: Vec::new(),
    };
    assert!(
        driver
            .critique_plan(
                &task,
                lifecycle::CritiqueOptions {
                    assignment: &assignment,
                    config: &config,
                    learning_root: &fixture.0,
                    grounding: None,
                    metadata: None,
                    locked_model: None,
                    fleet: Some(&fleet),
                }
            )
            .await
            .is_err()
    );
    assert!(driver.last_response.is_none());
    assert!(
        lifecycle::response_run(&config, driver.last_response.as_ref(), &assignment)?.is_none()
    );
    assert_eq!(driver.critic_evidence.len(), 1);
    assert!(
        driver
            .critic_evidence
            .first()
            .and_then(|evidence| evidence.get("run"))
            .is_some_and(|run| *run != Value::Null)
    );
    Ok(())
}

struct CancelAtCritic {
    source: CancellationSource,
    completed: usize,
    cancel_at: usize,
}

impl Observer for CancelAtCritic {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if domain::is_text(event.get("phase"), "ok") {
            self.completed += 1;
            if self.completed == self.cancel_at {
                self.source.cancel(Reason::Cancelled)?;
            }
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_preserves_planner_ownership_before_and_after_revision() -> Result<(), Error> {
    for second in [false, true] {
        let fixture = Fixture::new()?;
        let assignment = assignment("Change README.md.");
        let (config, fleet) = critic_config()?;
        let cancellation = CancellationSource::default();
        let token = cancellation.token();
        let mut script = vec![text_packet(&description()?.json(), "planner-model")];
        if second {
            script.extend([advisory_packet(), revision_packet()?]);
        }
        script.push(text_packet(r#"{"defects":[]}"#, "critic-model"));
        let mut transport = FixtureTransport(script.into());
        let mut observer = CancelAtCritic {
            source: cancellation,
            completed: 0,
            cancel_at: if second { 4 } else { 2 },
        };
        let credentials = Credentials::default();
        let mut driver = lifecycle::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &crate::llm::admission::Admission::default(),
            signal: Some(&token),
            redaction: redaction::Config::default(),
            last_response: None,
            critic_evidence: Vec::new(),
        };
        let draft = driver
            .draft(
                &assignment,
                &config,
                lifecycle::DraftOptions::default(),
                None,
            )
            .await?;
        let error = driver
            .critique_plan(
                &draft.artifacts.task,
                lifecycle::CritiqueOptions {
                    assignment: &assignment,
                    config: &config,
                    learning_root: &fixture.0,
                    grounding: None,
                    metadata: None,
                    locked_model: None,
                    fleet: Some(&fleet),
                },
            )
            .await
            .err()
            .ok_or_else(|| Error::new("Error", "Cancelled critique returned success"))?;
        assert_eq!(error.code, Some("ROSTER_CANCELLED"));
        assert_eq!(
            driver
                .last_response
                .as_ref()
                .map(|response| response.model.clone()),
            Some(Text::from(if second {
                "revision-model"
            } else {
                "planner-model"
            }))
        );
        assert_eq!(driver.critic_evidence.len(), if second { 2 } else { 1 });
        let failed = driver
            .critic_evidence
            .last()
            .ok_or_else(|| Error::new("Error", "Missing cancelled critic evidence"))?;
        assert_eq!(failed.get("reportValid"), Some(&Value::Boolean(false)));
        assert_eq!(failed.get("response"), Some(&Value::Null));
        assert_eq!(failed.get("run"), Some(&Value::Null));
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn outer_failure_keeps_distinct_planner_and_critic_runs() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    let assignment = assignment("Change README.md.");
    let (config, fleet) = critic_config()?;
    let directory = fixture.0.join(".roster");
    files::create_private_directories(&directory)?;
    let source = crate::fleet::format(&fleet.as_value())?.to_utf8()?;
    secure::write_exclusive(&directory.join("fleet.yml"), source.as_bytes())?;
    let mut transport = FixtureTransport(
        vec![
            text_packet(&description()?.json(), "planner-model"),
            text_packet("not valid JSON", "critic-model"),
        ]
        .into(),
    );
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let mut driver = lifecycle::Driver {
        transport: &mut transport,
        observer: &mut observer,
        credentials: &credentials,
        admission: &crate::llm::admission::Admission::default(),
        signal: None,
        redaction: redaction::Config::default(),
        last_response: None,
        critic_evidence: Vec::new(),
    };
    let files = vec!["README.md".to_owned()];
    let failure = driver
        .run_planner(lifecycle::RunOptions {
            worktree: &fixture.0,
            repo_root: &fixture.0,
            assignment: &assignment,
            config: &config,
            metadata: None,
            locked_model: None,
            kind: None,
            plan_mode: false,
            repository_files: Some(&files),
            evaluations: None,
            learning: None,
        })
        .await
        .err()
        .ok_or_else(|| Error::new("Error", "Malformed critic produced a passing run"))?;
    let run = failure
        .run
        .ok_or_else(|| Error::new("Error", "Measured planner failure run was lost"))?;
    assert_eq!(run.metrics().model, "planner-model");
    assert_eq!(run.metrics().usage.prompt_tokens, Some(5));
    assert_eq!(
        run.metrics().context_max,
        Some(run::Capacity::Number(32768))
    );
    assert_eq!(run.metrics().effort, "m");
    assert_eq!(failure.critic_evidence.len(), 1);
    let metrics = required(
        required(
            failure
                .critic_evidence
                .first()
                .ok_or_else(|| Error::new("Error", "Missing scoped critic run"))?,
            "run",
        )?,
        "metrics",
    )?;
    assert_eq!(metrics.get("model"), Some(&Value::string("critic-model")));
    assert_eq!(metrics.get("context_max"), Some(&Value::Number(8192.0)));
    assert_eq!(metrics.get("effort"), Some(&Value::string("-")));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_consumption_never_resets_transport_budget() -> Result<(), Error> {
    let mut config = config(true)?;
    let mut llm = required(&config, "llm")?.clone();
    domain::put(&mut llm, "max_requests", Value::Number(1.0))?;
    domain::put(&mut config, "llm", llm)?;
    let chat = crate::llm::openai::Chat::new(&config)?
        .ok_or_else(|| Error::new("Error", "Fixture transport disabled"))?;
    let mut transport = FixtureTransport(vec![text_packet("{}", "planner-model")].into());
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let request = parse(r#"{"messages":[{"role":"user","content":"fixture"}]}"#)?;
    chat.complete(
        &request,
        &mut transport,
        &mut observer,
        &credentials,
        crate::llm::openai::CallOptions::default(),
    )
    .await
    .map_err(|failure| *failure.error)?;
    assert_eq!(chat.request_count(), 1);
    assert!(chat.take_last_response()?.is_some());
    assert!(chat.take_last_response()?.is_none());
    assert_eq!(chat.request_count(), 1);
    let failed = chat
        .complete(
            &request,
            &mut transport,
            &mut observer,
            &credentials,
            crate::llm::openai::CallOptions::default(),
        )
        .await
        .err()
        .ok_or_else(|| Error::new("Error", "Consumed snapshot reset the request budget"))?;
    assert_eq!(failed.error.code, Some("ROSTER_MODEL_REQUEST_LIMIT"));
    Ok(())
}
