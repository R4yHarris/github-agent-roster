use super::*;

#[tokio::test(flavor = "current_thread")]
async fn invalid_actual_coder_model_refuses_calls_and_retains_real_zero_usage_without_declared_substitution()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut invalid = calls(vec![(
            "write",
            "write_file",
            write("docs/guide.md", "# Must not execute\n"),
        )]);
        domain::put(&mut invalid, "model", Value::string("unknown"))?;
        domain::put(
            &mut invalid,
            "usage",
            Value::object(vec![
                ("prompt_tokens", Value::Number(0.0)),
                ("completion_tokens", Value::Number(0.0)),
            ]),
        )?;
        let mut transport = Recorded::new(vec![invalid]);
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
        assert_ne!(coder.status(), finalization::Status::Pass);
        assert_eq!(
            coder.failure().and_then(|error| error.code),
            Some("ROSTER_SEAT_MODEL")
        );
        assert!(coder.run().is_none());
        assert_eq!(
            coder.response().map(|response| response.model.clone()),
            Some(Text::from("unknown"))
        );
        assert_eq!(
            coder
                .response()
                .and_then(|response| response.usage)
                .and_then(|usage| usage.prompt_tokens),
            Some(0)
        );
        assert_eq!(fixture.read("docs/guide.md")?, "# Guide\n\nBefore.\n");
        assert!(boundary.written_files().is_empty());
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        assert!(
            !fixture
                .read("RESULT.md")?
                .contains("Model: configured-route")
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_actual_auditor_or_reviewer_identity_keeps_scoped_response_but_never_passes_review()
-> Result<(), Error> {
    for audit in [false, true] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["The requested guide is delivered."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let mut invalid = review_packet(verdict(1));
            domain::put(&mut invalid, "model", Value::string("unknown"))?;
            domain::put(
                &mut invalid,
                "usage",
                Value::object(vec![
                    ("prompt_tokens", Value::Number(0.0)),
                    ("completion_tokens", Value::Number(0.0)),
                ]),
            )?;
            let mut responses = vec![
                calls(vec![(
                    "write",
                    "write_file",
                    write("docs/guide.md", "# Delivered guide\n"),
                )]),
                summary(),
            ];
            if !audit {
                responses.push(review_packet(verdict(1)));
            }
            responses.push(invalid);
            let mut transport = Recorded::new(responses);
            let mut events = Events::default();
            let credentials = Credentials::default();
            let admission = Admission::default();
            let mut driver = driver(&mut transport, &mut events, &credentials, &admission);
            let coder = driver
                .run_coder(
                    &mut boundary,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &config(true, false, 10)?,
                    coder::Options::default(),
                )
                .await?;
            let review = driver
                .run_reviewer(
                    &boundary,
                    &coder,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &config(true, false, 10)?,
                    reviewer::Options::default(),
                )
                .await?;
            assert_eq!(review.status(), reviewer::Status::Unavailable);
            assert_eq!(
                review.failure().and_then(|error| error.code),
                Some("ROSTER_SEAT_MODEL")
            );
            if audit {
                assert!(review.ask_audit().is_some_and(|audit| {
                    audit
                        .get("response")
                        .is_some_and(|response| domain::is_text(response.get("model"), "unknown"))
                }));
                assert!(review.response().is_none());
            } else {
                assert_eq!(
                    review.response().map(|response| response.model.clone()),
                    Some(Text::from("unknown"))
                );
                assert_eq!(
                    review
                        .response()
                        .and_then(|response| response.usage)
                        .and_then(|usage| usage.prompt_tokens),
                    Some(0)
                );
            }
            assert!(review.run().is_none());
            assert!(
                review
                    .require_passing(&boundary, &coder, &evidence::Projection::default())
                    .await
                    .is_err()
            );
            assert!(!fixture.read("REVIEW.md")?.contains("Verdict: pass"));
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn authoritative_ask_missing_file_has_real_absence_ground_even_when_omitted_from_planned_scope()
-> Result<(), Error> {
    let mut fixture = Fixture::new(&["docs/guide.md"], "none", &["The guide is delivered."])?;
    fixture.assignment.ask = Text::from(
        "Write docs/guide.md and docs/additional.md.\n\n## Files allowed\n- docs/guide.md\n- docs/additional.md",
    );
    fixture.task(&["docs/guide.md"], "none", &["The guide is delivered."])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let reason = "docs/additional.md is a requested deliverable and is absent.";
        let mut audit = verdict(1);
        domain::put(&mut audit, "verdict", Value::string("fail"))?;
        domain::put(
            &mut audit,
            "reasons",
            Value::Array(vec![Value::string(reason)]),
        )?;
        domain::put(
            &mut audit,
            "grounds",
            Value::Array(vec![Value::object(vec![
                (
                    "obligation",
                    Value::string("Write docs/guide.md and docs/additional.md."),
                ),
                ("file", Value::string("docs/additional.md")),
                ("excerpt", Value::Null),
                ("reason", Value::string(reason)),
            ])]),
        )?;
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Delivered guide\n"),
            )]),
            summary(),
            review_packet(audit),
            review_packet(verdict(1)),
        ]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let mut driver = driver(&mut transport, &mut events, &credentials, &admission);
        let coder = driver
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 10)?,
                coder::Options::default(),
            )
            .await?;
        let review = driver
            .run_reviewer(
                &boundary,
                &coder,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 10)?,
                reviewer::Options::default(),
            )
            .await?;
        assert_eq!(
            review.status(),
            reviewer::Status::Fail,
            "{:?}",
            review.failure()
        );
        assert!(review.failure().is_none());
        assert!(!fixture.root.join("docs").join("additional.md").exists());
        assert!(
            review
                .report()
                .get("reasons")
                .is_some_and(|reasons| reasons.json().contains("Original Ask audit"))
        );
        assert!(!fixture.read("REVIEW.md")?.contains("Verdict: pass"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn reviewer_cannot_override_an_unmet_check_with_a_passing_verdict_word() -> Result<(), Error>
{
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut invalid_pass = verdict(1);
        domain::put(
            &mut invalid_pass,
            "checks",
            Value::Array(vec![Value::object(vec![
                ("id", Value::Number(1.0)),
                ("met", Value::Boolean(false)),
                (
                    "evidence",
                    Value::string("docs/guide.md omits the requested outcome."),
                ),
            ])]),
        )?;
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Candidate guide\n"),
            )]),
            summary(),
            review_packet(verdict(1)),
            review_packet(invalid_pass),
        ]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let mut driver = driver(&mut transport, &mut events, &credentials, &admission);
        let coder = driver
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 10)?,
                coder::Options::default(),
            )
            .await?;
        let review = driver
            .run_reviewer(
                &boundary,
                &coder,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 10)?,
                reviewer::Options::default(),
            )
            .await?;
        assert_eq!(review.status(), reviewer::Status::Fail);
        assert!(
            review
                .report()
                .get("reasons")
                .is_some_and(|reasons| reasons.json().contains("Check 1 unmet"))
        );
        assert!(
            review
                .require_passing(&boundary, &coder, &evidence::Projection::default())
                .await
                .is_err()
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}
