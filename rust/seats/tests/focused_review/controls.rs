use super::*;

#[test]
fn coordinated_web_invocations_use_clause_modifiers_not_documentation_inheritance()
-> Result<(), Error> {
    for heading in ["", "# Task: ", "- "] {
        for (ask, expected) in [
            (
                "Document the result, but first use web_search; do not edit other files.",
                vec!["web_search"],
            ),
            (
                "Document the result, and also use web_fetch.",
                vec!["web_fetch"],
            ),
            (
                "Document the result, then first call web_search.",
                vec!["web_search"],
            ),
            (
                "Document the result but next actually invoke web_fetch.",
                vec!["web_fetch"],
            ),
            (
                "Document the result, but first then explicitly use web_search.",
                vec!["web_search"],
            ),
            (
                "Document the result, but first do not use web_search.",
                Vec::new(),
            ),
            (
                "Document the result, but do not first use web_search.",
                Vec::new(),
            ),
            (
                "Document the result, but first do not use web_search; then also use web_fetch.",
                vec!["web_fetch"],
            ),
            (
                "Document how to use web_search without invoking it.",
                Vec::new(),
            ),
            (
                "Document how to inspect and safely use web_search.",
                Vec::new(),
            ),
            (
                "Document how to inspect, but first use web_fetch.",
                vec!["web_fetch"],
            ),
            (
                "Document the phrase `Document the result, but first use web_search`.",
                Vec::new(),
            ),
        ] {
            let source = format!("{heading}{ask}");
            assert_eq!(
                crate::seats::preparation::selected_web_prerequisites(&source)?,
                expected,
                "{source}"
            );
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn coordinated_web_actions_are_unavailable_before_model_and_product_work() -> Result<(), Error>
{
    for (ask, required) in [
        (
            "Document the result, but first use web_search; do not edit other files.",
            true,
        ),
        (
            "Document the result, and also use web_fetch; do not edit other files.",
            true,
        ),
        ("Document the result, then first call web_search.", true),
        (
            "Document the result, but do not first use web_search.",
            false,
        ),
        ("Document how to inspect and safely use web_search.", false),
        (
            "Document the phrase `Document the result, but first use web_search`.",
            false,
        ),
    ] {
        let fixture = web_fixture(ask)?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let mut transport = Recorded::new(vec![
                calls(vec![(
                    "write",
                    "write_file",
                    write("docs/guide.md", "# Actual documented result\n"),
                )]),
                summary(),
            ]);
            let mut events = Events::default();
            let credentials = Credentials::default();
            let admission = Admission::default();
            let coder = driver(&mut transport, &mut events, &credentials, &admission)
                .run_coder(
                    &mut boundary,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &config(true, false, 10)?,
                    coder::Options::default(),
                )
                .await?;
            assert_eq!(
                coder.status(),
                if required {
                    finalization::Status::Unavailable
                } else {
                    finalization::Status::Pass
                },
                "{ask}: {:?}",
                coder.failure()
            );
            if required {
                assert!(transport.requests.is_empty());
                assert!(boundary.written_files().is_empty());
                assert_eq!(fixture.read("docs/guide.md")?, "# Guide\n\nBefore.\n");
            } else {
                assert_eq!(
                    fixture.read("docs/guide.md")?,
                    "# Actual documented result\n"
                );
                assert!(transport.requests.iter().all(|request| {
                    !request
                        .get("tools")
                        .is_some_and(|tools| tools.json().contains("\"name\":\"web_"))
                }));
            }
            assert!(boundary.last_tests().is_none());
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[test]
fn completed_native_test_failure_identity_excludes_only_nonfailure_build_activity()
-> Result<(), Error> {
    let stdout = "running 1 test\ntest fixed_native_failure ... FAILED\nfailures:\nassertion left: 1\nright: 2\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.25s\n";
    let warm = "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.01s\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s\nerror: test failed, to rerun pass `--test repeat`\n";
    let rebuilt = format!(
        "    Checking owned-review-timing v0.1.0 (D:\\owned\\project)\n   Compiling owned-review-timing v0.1.0 (D:\\owned\\project)\n{warm}"
    );
    let identity = crate::seats::coder::observed_failure_identity(stdout, warm, true)?;
    assert_eq!(
        identity,
        crate::seats::coder::observed_failure_identity(stdout, &rebuilt, true)?
    );
    let waiting = format!(
        "    Blocking waiting for file lock on package cache\n    Blocking waiting for file lock on package cache\n{rebuilt}    Blocking waiting for file lock on build directory\n"
    );
    assert_eq!(
        identity,
        crate::seats::coder::observed_failure_identity(stdout, &waiting, true)?
    );
    assert_ne!(
        identity,
        crate::seats::coder::observed_failure_identity(
            &stdout.replace("left: 1", "left: 3"),
            warm,
            true
        )?
    );
    assert_ne!(
        identity,
        crate::seats::coder::observed_failure_identity(
            stdout,
            &format!("{warm}error: could not acquire file lock on package cache\n"),
            true
        )?
    );
    assert_ne!(
        identity,
        crate::seats::coder::observed_failure_identity(
            &format!("{stdout}Blocking waiting for file lock on package cache\n"),
            warm,
            true
        )?
    );
    assert_ne!(
        identity,
        crate::seats::coder::observed_failure_identity(
            &stdout.replace("fixed_native_failure", "other_native_failure"),
            warm,
            true
        )?
    );
    assert_ne!(
        identity,
        crate::seats::coder::observed_failure_identity(
            stdout,
            &format!("{warm}error: additional compiler failure\n"),
            true
        )?
    );
    assert_ne!(
        identity,
        crate::seats::coder::observed_failure_identity(
            stdout,
            &format!("{warm}warning: actual warning changed\n"),
            true
        )?
    );
    assert_ne!(
        crate::seats::coder::observed_failure_identity("", warm, true)?,
        crate::seats::coder::observed_failure_identity("", &rebuilt, true)?,
    );
    assert_ne!(
        crate::seats::coder::observed_failure_identity(stdout, warm, false)?,
        crate::seats::coder::observed_failure_identity(stdout, &rebuilt, false)?,
    );
    assert_ne!(
        identity,
        crate::seats::coder::observed_failure_identity(
            &format!("{stdout}Compiling assertion-message v0.1.0 (D:\\actual)\n"),
            warm,
            true
        )?
    );
    Ok(())
}

#[test]
fn web_invocation_negation_is_local_and_documentation_is_not_an_internet_grant() -> Result<(), Error>
{
    for (source, expected) in [
        (
            "Use web_search to verify the prerequisite, but do not edit unrelated files.",
            vec!["web_search"],
        ),
        (
            "Do not edit unrelated files; use web_fetch for the prerequisite.",
            vec!["web_fetch"],
        ),
        ("Do not use web_search.", Vec::new()),
        (
            "Don't edit unrelated files; use web_search, don't widen permissions.",
            vec!["web_search"],
        ),
        ("Don't use web_search; don't use web_fetch.", Vec::new()),
        ("Never invoke web_fetch.", Vec::new()),
        ("Without using web_search, write the guide.", Vec::new()),
        ("Do not use web_search or web_fetch.", Vec::new()),
        (
            "Do not use web_search, but use web_fetch.",
            vec!["web_fetch"],
        ),
        (
            "Use web_search, but do not use web_fetch.",
            vec!["web_search"],
        ),
        (
            "Use `web_search` and `web_fetch`, but do not edit unrelated files.",
            vec!["web_search", "web_fetch"],
        ),
        (
            "Document web_search configuration; do not edit unrelated files.",
            Vec::new(),
        ),
        (
            "Document how to use web_search without invoking it.",
            Vec::new(),
        ),
        (
            "# Task: Document how to use web_search, without invoking it.",
            Vec::new(),
        ),
        ("Document the phrase `Use web_search`.", Vec::new()),
        (
            "Explain \"Use web_fetch\" as a literal example.",
            Vec::new(),
        ),
        (
            "```text\nUse web_search\n```\nDocument this example.",
            Vec::new(),
        ),
        (
            "Document the result, then use web_fetch.",
            vec!["web_fetch"],
        ),
        ("The prerequisite requires web_search.", vec!["web_search"]),
    ] {
        assert_eq!(
            crate::seats::preparation::selected_web_prerequisites(source)?,
            expected,
            "{source}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn configured_web_clause_controls_never_widen_disabled_internet_permissions()
-> Result<(), Error> {
    for (ask, required, internet) in [
        (
            "Use `web_fetch` to verify the prerequisite, but do not edit unrelated files.",
            true,
            false,
        ),
        (
            "Do not use web_search, but use web_fetch to verify the prerequisite.",
            true,
            false,
        ),
        ("Use web_search, but do not use web_fetch.", true, true),
        (
            "Do not use web_search; document the requested outcome.",
            false,
            false,
        ),
        (
            "Document how to use web_search, without performing that operation.",
            false,
            false,
        ),
    ] {
        let fixture = web_fixture(ask)?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let source_config = crate::config::format(config(true, false, 10)?.as_value())?
                .to_utf8()?
                .replace("internet: false", &format!("internet: {internet}"));
            let config = crate::config::parse(&Text::from(source_config.as_str()))?;
            let mut transport = Recorded::new(vec![
                calls(vec![(
                    "write",
                    "write_file",
                    write("docs/guide.md", "# Actual documented outcome\n"),
                )]),
                summary(),
            ]);
            let mut events = Events::default();
            let credentials = Credentials::default();
            let admission = Admission::default();
            let coder = driver(&mut transport, &mut events, &credentials, &admission)
                .run_coder(
                    &mut boundary,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &config,
                    coder::Options::default(),
                )
                .await?;
            assert_eq!(
                coder.status(),
                if required {
                    finalization::Status::Unavailable
                } else {
                    finalization::Status::Pass
                },
                "{ask}: {:?}",
                coder.failure()
            );
            if required {
                assert!(transport.requests.is_empty());
                assert!(boundary.written_files().is_empty());
                assert_eq!(fixture.read("docs/guide.md")?, "# Guide\n\nBefore.\n");
            } else {
                assert_eq!(
                    fixture.read("docs/guide.md")?,
                    "# Actual documented outcome\n"
                );
                assert!(transport.requests.iter().all(|request| {
                    !request
                        .get("tools")
                        .is_some_and(|tools| tools.json().contains("\"name\":\"web_"))
                }));
            }
            assert!(boundary.last_tests().is_none());
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[test]
fn cargo_libtest_and_node_timing_records_normalize_without_erasing_failure_data()
-> Result<(), Error> {
    let first = "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.02s\n    Finished test [unoptimized + debuginfo] target(s) in 1.21s\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s\n  duration_ms: 12.4\n# duration_ms 16.0\n✖ native case (1.02ms)\nassertion left: 7\nfailures:\n fixed_native_failure\n";
    let second = first
        .replace("0.02s", "0.87s")
        .replace("1.21s", "2.45s")
        .replace("12.4", "84.5")
        .replace("16.0", "77.0")
        .replace("1.02ms", "4.89ms");
    let fingerprint = crate::seats::coder::failure_fingerprint(first)?;
    assert_eq!(
        fingerprint,
        crate::seats::coder::failure_fingerprint(&second)?
    );
    for changed in [
        first.replace("left: 7", "left: 8"),
        first.replace("fixed_native_failure", "other_native_failure"),
        first.replace("1 failed", "2 failed"),
        first.replace("`dev` profile", "`release` profile"),
        format!("{first}\nassertion: observed duration_ms: 0.02\n"),
        format!("{first}\nassertion: expected delay (0.02s)\n"),
    ] {
        assert_ne!(
            fingerprint,
            crate::seats::coder::failure_fingerprint(&changed)?
        );
    }
    assert_ne!(
        crate::seats::coder::failure_fingerprint("assertion: expected delay (0.02s)")?,
        crate::seats::coder::failure_fingerprint("assertion: expected delay (0.87s)")?,
    );
    assert_ne!(
        crate::seats::coder::failure_fingerprint("assertion: duration_ms: 0.02")?,
        crate::seats::coder::failure_fingerprint("assertion: duration_ms: 0.87")?,
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn actual_cargo_timing_only_failure_cannot_extend_a_single_file_repair_budget()
-> Result<(), Error> {
    let fixture = timing_fixture()?;
    fixture.task(
        &["tests/repeat.rs"],
        "characterization",
        &["The actual native failure is repaired."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        fixture.assignment.clone(),
        tools::Options::default(),
        super::super::resources::cargo_host(&fixture)?,
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let result = async {
        boundary.enable_owned_checks()?;
        let warm = boundary.verify(false).await?;
        assert_ne!(warm.get("exit_code"), Some(&Value::Number(0.0)));
        let mut transport = Recorded::new(
            (0..5)
                .map(|_| calls(vec![("test", "run_test", Value::object(Vec::new()))]))
                .collect(),
        );
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 5)?,
                coder::Options::default(),
            )
            .await?;
        assert_eq!(
            coder.handoff(),
            Some(Handoff::RepairBudget),
            "{:?}",
            coder.failure()
        );
        assert_eq!(
            coder.result().get("testRepairBudget"),
            Some(&Value::Number(1.0))
        );
        assert_eq!(transport.requests.len(), 2);
        assert!(
            coder
                .result()
                .get("testFailures")
                .and_then(domain::items)
                .is_some_and(|failures| failures.len() == 2)
        );
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn actual_cargo_assertion_progress_still_extends_budget_and_reaches_real_green()
-> Result<(), Error> {
    let mut fixture = timing_fixture()?;
    fixture.assignment.ask = Text::from(
        "Repair the native public operation and verify the actual existing Rust assertion.\n\n## Files allowed\n- src/lib.rs",
    );
    fixture.task(
        &["src/lib.rs"],
        "characterization",
        &["The native public operation returns the required value."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        fixture.assignment.clone(),
        tools::Options::default(),
        super::super::resources::cargo_host(&fixture)?,
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let result = async {
        let mut responses = Vec::new();
        for value in [3, 4, 5, 2] {
            responses.push(calls(vec![
                ("read", "read_file", path("src/lib.rs")),
                (
                    "write",
                    "write_file",
                    write(
                        "src/lib.rs",
                        &format!("pub fn observed() -> u8 {{\n    {value}\n}}\n"),
                    ),
                ),
                ("test", "run_test", Value::object(Vec::new())),
            ]));
        }
        responses.push(summary());
        responses.push(self_review_packet(1, Vec::new()));
        let mut transport = Recorded::new(responses);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 12)?,
                coder::Options::default(),
            )
            .await?;
        assert_eq!(
            coder.status(),
            finalization::Status::Pass,
            "{:?}\n{}",
            coder.failure(),
            coder.result().json()
        );
        assert_eq!(coder.result().get("testRepairs"), Some(&Value::Number(3.0)));
        assert_eq!(
            coder.result().get("testRepairBudget"),
            Some(&Value::Number(3.0))
        );
        assert!(
            coder
                .result()
                .get("tests")
                .is_some_and(|tests| tests.get("native") == Some(&Value::Boolean(true))
                    && tests.get("exit_code") == Some(&Value::Number(0.0)))
        );
        assert!(
            fixture
                .read("RESULT.md")?
                .contains("fixed_native_failure ... ok")
        );
        let failures = coder
            .result()
            .get("testFailures")
            .and_then(domain::items)
            .ok_or_else(|| Error::type_error("Actual progress failure history missing"))?;
        assert_eq!(failures.len(), 3);
        assert_ne!(
            failures
                .first()
                .and_then(|failure| failure.get("fingerprint")),
            failures
                .get(1)
                .and_then(|failure| failure.get("fingerprint"))
        );
        assert_ne!(
            failures
                .get(1)
                .and_then(|failure| failure.get("fingerprint")),
            failures
                .get(2)
                .and_then(|failure| failure.get("fingerprint"))
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

async fn current_author_attempt(
    boundary: &mut tools::Boundary,
) -> Result<finalization::Attempt, Error> {
    boundary.enable_owned_checks()?;
    boundary.execute("read_file", &path("src/app.mjs")).await?;
    boundary
        .execute(
            "write_file",
            &write(
                "src/app.mjs",
                "export function answer(value = 0) { return value + 1; }\n",
            ),
        )
        .await?;
    let tests = boundary.verify(true).await?;
    assert_eq!(tests.get("exit_code"), Some(&Value::Number(0.0)));
    let config = config(true, false, 20)?;
    let projection = evidence::Projection::default();
    let mut attempt = finalization::Attempt::begin(
        boundary,
        &config,
        finalization::Inputs {
            mode: finalization::Mode::Configured,
            capsule_version: continuity::CapsuleVersion::NativeV2,
            capsule: None,
            prior_wave_files: &[],
            initial_baseline: None,
        },
        &projection,
    )?;
    attempt
        .context(boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &projection)
        .await?;
    attempt.response(
        boundary,
        &Completion {
            message: message(
                "assistant",
                Text::from("1. done: real source and matching checks are complete."),
            ),
            model: Text::from("actual-main-author"),
            usage: None,
            finish_reason: Some(Value::string("stop")),
        },
    )?;
    Ok(attempt)
}

#[tokio::test(flavor = "current_thread")]
async fn shared_author_acceptance_rejects_every_authority_invalid_envelope_and_preserves_measurements()
-> Result<(), Error> {
    for variant in 0..8 {
        let fixture = Fixture::new(
            &["src/app.mjs", "docs/guide.md"],
            "characterization",
            &["The real increment preserves the retained behavior."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let projection = evidence::Projection::default();
            let mut attempt = current_author_attempt(&mut boundary).await?;
            let mut response = Completion {
                message: author_message(
                    match variant {
                        0 => "user",
                        1 => "tool",
                        2 => "system",
                        _ => "assistant",
                    },
                    variant == 3,
                ),
                model: Text::from("actual-shared-invalid-author"),
                usage: Some(Value::object(vec![
                    ("prompt_tokens", Value::Number(0.0)),
                    ("completion_tokens", Value::Number(7.0)),
                ])),
                finish_reason: Some(Value::string("stop")),
            };
            match variant {
                4 => {
                    domain::remove(&mut response.message, "role")?;
                }
                5 => {
                    domain::put(
                        &mut response.message,
                        "tool_calls",
                        Value::Array(Vec::new()),
                    )?;
                }
                6 => {
                    response.finish_reason = Some(Value::string("tool_calls"));
                }
                7 => {
                    domain::put(&mut response.message, "function_call", Value::Null)?;
                }
                _ => {}
            }
            let request = attempt.self_review_request(&boundary, &projection).await?;
            let rejection = execution::verification::tool_free_assistant(&response)
                .err()
                .ok_or_else(|| Error::type_error("Authority-invalid envelope passed validation"))?;
            let report = request.accept(&boundary, &response, &projection).await?;
            assert_eq!(
                report.as_value().get("status"),
                Some(&Value::string("rejected")),
                "variant {variant}"
            );
            assert_eq!(
                report.as_value().get("model"),
                Some(&Value::string("actual-shared-invalid-author"))
            );
            assert_eq!(report.as_value().get("usage"), response.usage.as_ref());
            assert_eq!(
                report.require_current(&boundary, &projection).await.err(),
                Some(rejection)
            );
            attempt.attach_self_review(report)?;
            let finalized = attempt.finish(&boundary, &projection).await?;
            assert_ne!(
                finalized.status(),
                finalization::Status::Pass,
                "variant {variant}"
            );
            assert!(finalized.body().to_utf8()?.contains("Rejected:"));
            assert!(
                finalized
                    .result()
                    .get("selfReview")
                    .is_some_and(|review| domain::is_text(review.get("status"), "rejected"))
            );
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn shared_author_clean_and_explicit_content_transport_advisories_keep_their_existing_contracts()
-> Result<(), Error> {
    for variant in 0..3 {
        let fixture = Fixture::new(
            &["src/app.mjs", "docs/guide.md"],
            "characterization",
            &["The real increment preserves the retained behavior."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result =
            async {
                let projection = evidence::Projection::default();
                let mut attempt = current_author_attempt(&mut boundary).await?;
                let request = attempt.self_review_request(&boundary, &projection).await?;
                let report = if variant == 2 {
                    request.failed(
                        &boundary,
                        Error::new(
                            "OwnedTransportAdvisory",
                            "Explicit non-cancellation transport failure",
                        ),
                    )?
                } else {
                    request
                        .accept(
                            &boundary,
                            &Completion {
                                message: if variant == 0 {
                                    author_message("assistant", false)
                                } else {
                                    message("assistant", Text::from("not valid JSON"))
                                },
                                model: Text::from("actual-advisory-author"),
                                usage: Some(Value::object(vec![
                                    ("prompt_tokens", Value::Number(0.0)),
                                    ("completion_tokens", Value::Number(7.0)),
                                ])),
                                finish_reason: Some(Value::string("stop")),
                            },
                            &projection,
                        )
                        .await?
                };
                assert_eq!(
                    report.as_value().get("status"),
                    Some(&Value::string(if variant == 0 {
                        "clean"
                    } else {
                        "unavailable"
                    }))
                );
                report.require_current(&boundary, &projection).await?;
                if variant != 2 {
                    assert_eq!(
                        report.as_value().get("model"),
                        Some(&Value::string("actual-advisory-author"))
                    );
                    assert!(report.as_value().get("usage").is_some_and(|usage| {
                        usage.get("prompt_tokens") == Some(&Value::Number(0.0))
                    }));
                } else {
                    assert_eq!(report.as_value().get("usage"), Some(&Value::Null));
                    assert!(report.as_value().get("model").is_none());
                }
                attempt.attach_self_review(report)?;
                let finalized = attempt.finish(&boundary, &projection).await?;
                assert_eq!(
                    finalized.status(),
                    finalization::Status::Pass,
                    "variant {variant}: {}",
                    finalized.report().json()
                );
                assert!(finalized.result().get("selfReview").is_some_and(
                    |review| domain::is_text(
                        review.get("status"),
                        if variant == 0 { "clean" } else { "unavailable" }
                    )
                ));
                Ok(())
            }
            .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn author_advisory_adapter_cannot_hide_authority_rejection_or_cancellation()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "docs/guide.md"],
        "characterization",
        &["The real increment preserves the retained behavior."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let projection = evidence::Projection::default();
        let attempt = current_author_attempt(&mut boundary).await?;
        let request = attempt.self_review_request(&boundary, &projection).await?;
        let authority = execution::verification::tool_free_assistant(&Completion {
            message: author_message("tool", false),
            model: Text::from("actual-invalid-author"),
            usage: None,
            finish_reason: None,
        })
        .err()
        .ok_or_else(|| Error::type_error("Invalid envelope was accepted"))?;
        assert_eq!(
            request
                .failed(&boundary, authority)
                .err()
                .and_then(|error| error.code),
            Some("ROSTER_TOOL_FREE_AUTHORITY")
        );
        let request = attempt.self_review_request(&boundary, &projection).await?;
        assert_eq!(
            request
                .failed(&boundary, Reason::TaskTimeout(19).error())
                .err()
                .and_then(|error| error.code),
            Some("ROSTER_TASK_TIMEOUT")
        );
        let request = self_review::Request::prepare(
            &boundary,
            &evidence::changes(&boundary, None).await?,
            8000,
            &projection,
        )?;
        source.cancel(Reason::Cancelled)?;
        let response = Completion {
            message: author_message("assistant", false),
            model: Text::from("late-author"),
            usage: None,
            finish_reason: None,
        };
        assert_eq!(
            request
                .accept(&boundary, &response, &projection)
                .await
                .err()
                .and_then(|error| error.code),
            Some("ROSTER_CANCELLED")
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}
