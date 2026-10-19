use super::*;

#[tokio::test(flavor = "current_thread")]
async fn observed_checklist_closure_is_revoked_by_new_source_and_needs_current_read_evidence()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The guide explains the current requested outcome."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let close = || {
            Value::object(vec![(
                "items",
                Value::Array(vec![Value::object(vec![
                    ("id", Value::Number(1.0)),
                    ("status", Value::string("done")),
                    (
                        "evidence",
                        Value::string("docs/guide.md contains the current observed outcome"),
                    ),
                ])]),
            )])
        };
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# First current guide\n"),
            )]),
            calls(vec![("close", "update_checklist", close())]),
            calls(vec![(
                "rewrite",
                "write_file",
                write("docs/guide.md", "# Final current guide\n"),
            )]),
            summary(),
            calls(vec![("current-read", "read_file", path("docs/guide.md"))]),
            calls(vec![("current-close", "update_checklist", close())]),
            summary(),
        ]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, true, 16)?,
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
        assert_eq!(fixture.read("docs/guide.md")?, "# Final current guide\n");
        assert_eq!(transport.requests.len(), 7);
        let capsule = coder
            .capsule()
            .ok_or_else(|| Error::type_error("Current observed continuation missing"))?;
        assert_eq!(capsule.get("version"), Some(&Value::Number(2.0)));
        assert_eq!(capsule.get("outcome"), Some(&Value::string("verified")));
        assert!(capsule.json().contains("\"evidenceRef\":3"));
        assert!(
            coder
                .result()
                .get("checklist")
                .is_some_and(|checklist| checklist.json().contains("\"status\":\"done\""))
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn fabricated_observation_references_and_summary_prose_cannot_close_current_checks()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The current guide meets the requested outcome."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let fabricated = Value::object(vec![(
            "items",
            Value::Array(vec![Value::object(vec![
                ("id", Value::Number(1.0)),
                ("status", Value::string("done")),
                ("evidenceRef", Value::Number(999.0)),
                (
                    "evidence",
                    Value::string("docs/guide.md contains current bytes"),
                ),
            ])]),
        )]);
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Candidate guide\n"),
            )]),
            calls(vec![("fabricated", "update_checklist", fabricated.clone())]),
            calls(vec![("fabricated-again", "update_checklist", fabricated)]),
        ]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, true, 12)?,
                coder::Options::default(),
            )
            .await?;
        assert_ne!(coder.status(), finalization::Status::Pass);
        assert!(
            coder
                .capsule()
                .is_some_and(|capsule| capsule.get("outcome") == Some(&Value::string("failed")))
        );
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_readme_status_gets_one_real_model_correction_not_a_deterministic_code_fallback()
-> Result<(), Error> {
    let mut fixture = Fixture::new(
        &["README.md"],
        "none",
        &["README has a one-line Status section."],
    )?;
    fixture.assignment.ask =
        Text::from("Add a one-line Status section to README.md.\n\n## Files allowed\n- README.md");
    fixture.task(
        &["README.md"],
        "none",
        &["README has a one-line Status section."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write(
                    "README.md",
                    "# Consumer\n\n## Status\nFirst line.\nSecond line.\n",
                ),
            )]),
            summary(),
            calls(vec![(
                "correct",
                "write_file",
                write(
                    "README.md",
                    "# Consumer\n\n## Status\nExperimental and subject to change.\n",
                ),
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
            finalization::Status::Pass,
            "{:?}",
            coder.failure()
        );
        assert_eq!(
            fixture.read("README.md")?,
            "# Consumer\n\n## Status\nExperimental and subject to change.\n"
        );
        assert_eq!(
            events
                .0
                .iter()
                .filter(|event| domain::is_text(event.get("type"), "seat-tool")
                    && domain::is_text(event.get("name"), "write_file"))
                .count(),
            2
        );
        assert!(boundary.last_tests().is_none());
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn identical_named_file_writes_do_not_claim_implementation() -> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let original = fixture.read("docs/guide.md")?;
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "noop",
                "write_file",
                write("docs/guide.md", &original),
            )]),
            summary(),
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
        assert_eq!(coder.handoff(), Some(Handoff::NoProgress));
        assert_eq!(fixture.read("docs/guide.md")?, original);
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn selected_unavailable_cargo_or_disabled_host_verification_refuses_before_product_writes()
-> Result<(), Error> {
    for native in [false, true] {
        let paths = if native {
            vec!["rust/missing.rs"]
        } else {
            vec!["src/app.mjs"]
        };
        let fixture = Fixture::new(
            &paths,
            "required",
            &["The requested operation is verified."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = tools::Boundary::open(
            &fixture.root,
            fixture.assignment.clone(),
            tools::Options {
                allow_run_test: native,
                ..tools::Options::default()
            },
            tools::Host::default(),
            redaction::Config::default(),
            source.token(),
        )
        .await?;
        let result = async {
            let mut transport = Recorded::default();
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
                finalization::Status::Unavailable,
                "{:?}",
                coder.failure()
            );
            assert!(transport.requests.is_empty());
            assert!(boundary.written_files().is_empty());
            assert!(boundary.last_tests().is_none());
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn known_secret_correction_preserves_zero_usage_and_only_explicitly_promotes_compact_machine_summary()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered without leaking secrets."],
    )?;
    fixture.git(&[
        "remote",
        "add",
        "origin",
        "https://example.invalid/owned/native-seat-memory-fixture.git",
    ])?;
    let source = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        fixture.assignment.clone(),
        tools::Options::default(),
        tools::Host::default(),
        redaction::Config {
            environment: Value::object(vec![(
                "ROSTER_API_KEY",
                Value::string("test-only-private-api-key"),
            )]),
            ..redaction::Config::default()
        },
        source.token(),
    )
    .await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Guide\n\ntest-only-private-api-key\n"),
            )]),
            summary(),
            calls(vec![(
                "correct",
                "write_file",
                write(
                    "docs/guide.md",
                    "# Guide\n\nThe requested outcome is documented without credentials.\n",
                ),
            )]),
            summary(),
        ]);
        for response in &mut transport.responses {
            domain::put(
                response,
                "usage",
                Value::object(vec![
                    ("prompt_tokens", Value::Number(0.0)),
                    ("completion_tokens", Value::Number(0.0)),
                ]),
            )?;
        }
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let mut history = provenance::api::ProvenanceStore::open(
            &fixture.container.join("machine-history"),
            provenance::api::Identity::Repository(fixture.root.clone()),
        )?;
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 10)?,
                coder::Options {
                    recording: Recording {
                        history: Some(&mut history),
                        promote_summary: true,
                    },
                    ..coder::Options::default()
                },
            )
            .await?;
        assert_eq!(
            coder.status(),
            finalization::Status::Pass,
            "{:?}\n{}",
            coder.failure(),
            coder.result().json()
        );
        assert!(
            !fixture
                .read("docs/guide.md")?
                .contains("test-only-private-api-key")
        );
        assert!(
            !fixture
                .read("RESULT.md")?
                .contains("test-only-private-api-key")
        );
        let memory = Value::Array(
            history
                .query(
                    provenance::Query {
                        section: Some("curated-memory"),
                        ..provenance::Query::default()
                    },
                    None,
                )
                .await?,
        )
        .json();
        assert!(!memory.contains("test-only-private-api-key"));
        assert!(!memory.contains("The requested outcome is documented without credentials."));
        assert!(memory.contains("\"outcome\":\"pass\""));
        let raw = Value::Array(history.query(provenance::Query::default(), None).await?).json();
        assert!(!raw.contains("test-only-private-api-key"));
        assert!(!raw.contains("The requested outcome is documented without credentials."));
        assert!(raw.contains("\"seat\":{\"name\":\"coder\"}"));
        assert!(raw.contains("\"tokens_prompt\":0") && raw.contains("\"tokens_completion\":0"));
        let curated = crate::execution::context::curated_memory(
            &history,
            &fixture.root,
            execution::Role::Coder,
            boundary.token(),
        )
        .await?;
        let reviewer_memory = crate::execution::context::curated_memory(
            &history,
            &fixture.root,
            execution::Role::Reviewer,
            boundary.token(),
        )
        .await?;
        let coder_context = crate::execution::context::load(
            &boundary,
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &config(true, false, 10)?,
            crate::execution::context::Options {
                memory: Some(&curated),
                ..crate::execution::context::Options::default()
            },
        )
        .await?;
        let reviewer_context = crate::execution::context::load_as(
            &boundary,
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &config(true, false, 10)?,
            crate::execution::context::Options {
                memory: Some(&reviewer_memory),
                ..crate::execution::context::Options::default()
            },
            execution::Role::Reviewer,
        )
        .await?;
        assert!(
            coder_context
                .pack
                .to_utf8()?
                .contains("coder task issue-42 finished pass")
        );
        assert!(
            !reviewer_context
                .pack
                .to_utf8()?
                .contains("coder task issue-42 finished pass")
        );
        assert!(
            coder
                .result()
                .get("corrections")
                .is_some_and(|corrections| corrections.get("secret") == Some(&Value::Number(1.0)))
        );
        assert!(
            events
                .0
                .iter()
                .any(|event| domain::is_text(event.get("type"), "seat-tool")
                    && domain::is_text(event.get("status"), "denied"))
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn committed_human_post_coder_hook_executes_and_gets_one_observed_source_repair()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The guide includes the required approved result."],
    )?;
    fixture.write(".roster/hooks.yml", "hooks:\n  - event: post-coder\n    script: .roster/hooks/check.mjs\n    timeout_ms: 30000\n")?;
    fixture.write(".roster/hooks/check.mjs", "import { readFileSync } from 'node:fs';\nif (!readFileSync('docs/guide.md', 'utf8').includes('Approved')) { console.error('guide lacks required approved result'); process.exitCode = 1; }\n")?;
    fixture.git(&[
        "add",
        "--force",
        ".roster/hooks.yml",
        ".roster/hooks/check.mjs",
    ])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned human hook fixture"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Candidate guide\n"),
            )]),
            summary(),
            calls(vec![(
                "correct",
                "write_file",
                write("docs/guide.md", "# Approved guide\n"),
            )]),
            summary(),
        ]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let node = std::env::var_os("ROSTER_TEST_NODE")
            .map(PathBuf::from)
            .ok_or_else(|| Error::type_error("Owned hook requires approved Node 20"))?;
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 10)?,
                coder::Options {
                    approval: Some(backend::Approval {
                        hooks_node: Some(node),
                        ..backend::Approval::default()
                    }),
                    ..coder::Options::default()
                },
            )
            .await?;
        assert_eq!(
            coder.status(),
            finalization::Status::Pass,
            "{:?}\n{}",
            coder.failure(),
            coder.result().json()
        );
        assert_eq!(fixture.read("docs/guide.md")?, "# Approved guide\n");
        assert!(
            coder
                .result()
                .get("hooks")
                .is_some_and(|hooks| hooks.get("pass") == Some(&Value::Boolean(true)))
        );
        assert!(
            coder
                .result()
                .get("gateFailures")
                .is_some_and(|failures| failures.json().contains("hook"))
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}
