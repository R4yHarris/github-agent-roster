use super::*;
use crate::execution::archive;

#[tokio::test(flavor = "current_thread")]
async fn current_native_guidance_refresh_revokes_pending_calls_and_rotates_only_owned_context()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md", "docs/other.md"],
        "none",
        &["The bounded requested outcome is delivered."],
    )?;
    fixture.write("REVIEW.md", "Human review must remain untouched\n")?;
    let source = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        fixture.assignment.clone(),
        tools::Options {
            scope_expansion: 1,
            ..tools::Options::default()
        },
        super::resources::cargo_host(&fixture)?,
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let result = async {
        let code = "pub fn native_leaf() -> u8 {\n    42\n}\n";
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "native",
                "write_file",
                write("rust/seats/new.rs", code),
            )]),
            calls(vec![(
                "native-fresh",
                "write_file",
                write("rust/seats/new.rs", code),
            )]),
        ]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 2)?,
                coder::Options::default(),
            )
            .await?;
        assert_eq!(
            coder.handoff(),
            Some(Handoff::TurnBudget),
            "{:?}",
            coder.failure()
        );
        assert_eq!(fixture.read("rust/seats/new.rs")?, code);
        assert_eq!(
            fixture.read("REVIEW.md")?,
            "Human review must remain untouched\n"
        );
        let second = transport
            .requests
            .get(1)
            .ok_or_else(|| Error::type_error("Refreshed request missing"))?
            .json();
        for name in [
            "Rust implementation",
            "Rust Cargo",
            "Rust Tokio",
            "Tokio runtime",
        ] {
            assert!(second.contains(name), "Missing refreshed {name}");
        }
        let archived = archive::archived_artifacts(&boundary, "CONTEXT.md", 10).await?;
        assert!(!archived.is_empty());
        assert!(archived.iter().all(|body| {
            body.to_utf8()
                .is_ok_and(|body| !body.contains("Rust Tokio"))
        }));
        assert!(
            archive::archived_artifacts(&boundary, "REVIEW.md", 10)
                .await?
                .is_empty()
        );
        assert_eq!(
            events
                .0
                .iter()
                .filter(|event| domain::is_text(event.get("type"), "seat-tool")
                    && domain::is_text(event.get("status"), "ok"))
                .count(),
            1
        );
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn required_recording_failure_archives_only_actual_owned_result_and_keeps_human_review()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    fixture.write("REVIEW.md", "Human review remains private and current\n")?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Delivered guide\n"),
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
                coder::Options {
                    recording: Recording {
                        promote_summary: true,
                        ..Recording::default()
                    },
                    ..coder::Options::default()
                },
            )
            .await?;
        assert_ne!(coder.status(), finalization::Status::Pass);
        assert!(
            coder
                .failure()
                .is_some_and(|error| error.message.contains("repository-bound machine store"))
        );
        assert_eq!(
            fixture.read("REVIEW.md")?,
            "Human review remains private and current\n"
        );
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        let archived = archive::archived_artifacts(&boundary, "RESULT.md", 10).await?;
        assert!(archived.iter().any(|body| {
            body.to_utf8()
                .is_ok_and(|body| body.contains("Checks: PASS"))
        }));
        assert!(
            archive::archived_artifacts(&boundary, "REVIEW.md", 10)
                .await?
                .is_empty()
        );
        assert!(
            events
                .0
                .iter()
                .any(|event| domain::is_text(event.get("type"), "seat-stopped"))
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn design_grounding_is_checked_against_actual_tracked_exports_before_any_product_write()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let task = crate::planning::grounding::with_design(
        &Value::string(&fixture.read("TASK.md")?),
        &Value::object(vec![
            (
                "extend",
                Value::Array(vec![Value::object(vec![
                    ("file", Value::string("src/app.mjs")),
                    (
                        "exports",
                        Value::Array(vec![Value::string("missingExport")]),
                    ),
                ])]),
            ),
            ("new_exports", Value::Array(Vec::new())),
            (
                "outline",
                Value::Array(vec![Value::string("Extend the existing module.")]),
            ),
            ("edge_cases", Value::Array(Vec::new())),
        ]),
    )?;
    fixture.write("TASK.md", &task.to_utf8()?)?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
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
        assert!(
            coder
                .failure()
                .is_some_and(|error| error.message.contains("Design grounding")
                    && error.message.contains("does not export"))
        );
        assert!(transport.requests.is_empty());
        assert!(boundary.written_files().is_empty());
        assert_eq!(fixture.read("docs/guide.md")?, "# Guide\n\nBefore.\n");
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn actual_counterfactual_and_selfreview_findings_each_get_one_current_source_correction()
-> Result<(), Error> {
    for author_finding in [false, true] {
        let checks = if author_finding {
            vec![
                "Nonzero input increments while zero behavior is preserved.",
                "Non-finite input is rejected.",
            ]
        } else {
            vec!["Nonzero input increments while zero behavior is preserved."]
        };
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.next.test.mjs"],
            "required",
            &checks,
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let prefix = "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\n";
            let initial_test = if author_finding { "test('increments nonzero input', () => assert.equal(answer(2), 3));\n" }
                else { "test('new named operation', () => assert.equal(answer(0), 1));\n" };
            let repaired_test = if author_finding { "test('increments nonzero input', () => assert.equal(answer(2), 3));\ntest('rejects non-finite input', () => assert.throws(() => answer(Number.NaN), TypeError));\n" }
                else { "test('new named operation', () => assert.equal(answer(2), 3));\n" };
            let summary = || packet(message("assistant", Text::from(if author_finding { "1. done: increments are implemented.\n2. done: non-finite input is rejected." } else { "1. done: increments are implemented." })), "stop", "actual-coder", None);
            let mut first_review = self_review_packet(checks.len(), if author_finding { vec!["src/app.mjs answer does not reject the requested non-finite input."] } else { Vec::new() });
            if author_finding {
                let report = Value::object(vec![
                    ("checks", Value::Array(vec![
                        Value::object(vec![("id", Value::Number(1.0)), ("met", Value::Boolean(true)), ("evidence", Value::string("src/app.mjs answer increments finite input."))]),
                        Value::object(vec![("id", Value::Number(2.0)), ("met", Value::Boolean(false)), ("evidence", Value::string("src/app.mjs answer accepts non-finite input."))]),
                    ])),
                    ("findings", Value::Array(vec![Value::string("src/app.mjs answer must reject non-finite input to meet check 2.")])),
                ]);
                first_review = packet(message("assistant", Text::from(report.json().as_str())), "stop", "actual-author-review", None);
            }
            let repaired_source = if author_finding { "export function answer(value = 0) { if (!Number.isFinite(value)) throw new TypeError('finite input required'); return value + 1; }\n" }
                else { "export function answer(value = 0) { return value + 1; }\n" };
            let mut transport = Recorded::new(vec![
                calls(vec![("read", "read_file", path("src/app.mjs")), ("write", "write_file", write("src/app.mjs", "export function answer(value = 0) { return value + 1; }\n")), ("test-write", "write_file", write("tests/app.next.test.mjs", &format!("{prefix}{initial_test}")))]),
                calls(vec![("test", "run_test", Value::object(Vec::new()))]), summary(), first_review,
                calls(vec![("read-again", "read_file", path("src/app.mjs")), ("test-read", "read_file", path("tests/app.next.test.mjs")),
                    ("repair-source", "write_file", write("src/app.mjs", repaired_source)), ("repair-test", "write_file", write("tests/app.next.test.mjs", &format!("{prefix}{repaired_test}")))]),
                calls(vec![("test-again", "run_test", Value::object(Vec::new()))]), summary(), self_review_packet(checks.len(), Vec::new()),
            ]);
            let mut events = Events::default();
            let credentials = Credentials::default();
            let admission = Admission::default();
            let node = std::env::var_os("ROSTER_TEST_NODE").map(PathBuf::from).ok_or_else(|| Error::type_error("Owned repair fixture requires Node 20"))?;
            let coder = driver(&mut transport, &mut events, &credentials, &admission).run_coder(
                &mut boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &config(true, false, 16)?, coder::Options {
                    approval: Some(backend::Approval { node: Some(node), counterfactual: true, timeout_ms: Some(60_000), ..backend::Approval::default() }), ..coder::Options::default()
                },
            ).await?;
            assert_eq!(coder.status(), finalization::Status::Pass, "{:?}\n{}", coder.failure(), coder.result().json());
            let key = if author_finding { "self-review" } else { "red-green" };
            assert!(coder.result().get("corrections").is_some_and(|corrections| corrections.get(key) == Some(&Value::Number(1.0))));
            assert!(coder.result().get("redGreen").is_some());
            assert!(coder.result().get("selfReview").is_some_and(|report| domain::is_text(report.get("status"), "clean")));
            assert_eq!(coder.run().map(|run| run.metrics().model.as_str()), Some("actual-coder"));
            Ok(())
        }.await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn reviewer_read_round_limit_cannot_become_a_verdict_or_expose_writes() -> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let request = review_packet(Value::object(vec![(
            "requests",
            Value::Array(vec![Value::object(vec![
                ("tool", Value::string("read_file")),
                ("path", Value::string("docs/guide.md")),
            ])]),
        )]));
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Delivered guide\n"),
            )]),
            summary(),
            review_packet(verdict(1)),
            request.clone(),
            request.clone(),
            request.clone(),
            request,
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
        assert_eq!(review.status(), reviewer::Status::Unavailable);
        assert!(
            review
                .failure()
                .is_some_and(|error| error.message.contains("read-round budget"))
        );
        assert_eq!(transport.requests.len(), 7);
        assert_eq!(fixture.read("docs/guide.md")?, "# Delivered guide\n");
        assert!(!fixture.read("REVIEW.md")?.contains("Verdict: pass"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}
