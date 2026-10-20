use super::*;
use crate::execution::{continuity, self_review};
use crate::llm::openai::Completion;

mod controls;

fn cargo_failure_diagnostics(container: &Path, failures: &[Value]) -> Result<String, Error> {
    let mut records = Vec::new();
    let mut summary = Vec::new();
    let mut previous = String::new();
    for (index, failure) in failures.iter().enumerate() {
        let output = failure
            .get("output")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Native diagnostic output is missing"))?;
        let normalized = match failure.get("identity").and_then(Value::as_string) {
            Some(identity) if failure.get("identityComplete") == Some(&Value::Boolean(true)) => {
                identity.to_utf8()?
            }
            _ => crate::seats::coder::failure_fingerprint(&output.to_utf8()?)?,
        };
        let difference = previous
            .bytes()
            .zip(normalized.bytes())
            .position(|(left, right)| left != right)
            .unwrap_or(previous.len().min(normalized.len()));
        records.push(Value::object(vec![
            ("observation", Value::Number(index as f64 + 1.0)),
            (
                "storedFingerprint",
                failure.get("fingerprint").cloned().unwrap_or(Value::Null),
            ),
            (
                "recomputedFingerprint",
                Value::string(&crate::identity::sha256_hex(normalized.as_bytes())),
            ),
            ("recordComplete", Value::Boolean(output.0.len() < 8192)),
            ("completeOutput", Value::Boolean(output.0.len() <= 4096)),
            (
                "normalizedComplete",
                Value::Boolean(normalized.chars().count() <= 4096),
            ),
            ("differenceOffset", Value::Number(difference as f64)),
            (
                "normalized",
                Value::string(&normalized.chars().take(4096).collect::<String>()),
            ),
            (
                "output",
                Value::String(Text(output.0.iter().take(4096).copied().collect())),
            ),
        ]));
        summary.push(Value::object(vec![
            ("observation", Value::Number(index as f64 + 1.0)),
            (
                "fingerprint",
                failure.get("fingerprint").cloned().unwrap_or(Value::Null),
            ),
            ("differenceOffset", Value::Number(difference as f64)),
            (
                "difference",
                Value::string(
                    &normalized
                        .chars()
                        .skip(difference.saturating_sub(80))
                        .take(400)
                        .collect::<String>(),
                ),
            ),
        ]));
        previous = normalized;
    }
    let name = format!(
        ".native-cargo-diagnostic-{}-{}.json",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let path = container.join(&name);
    let json = Value::Array(records).json();
    if json.len() > 131_072 {
        return Err(Error::type_error(
            "Owned Cargo diagnostic exceeded its explicit bound",
        ));
    }
    secure::write_exclusive(&path, json.as_bytes())?;
    Ok(Value::object(vec![
        ("file", Value::string(&name)),
        ("transitions", Value::Array(summary)),
    ])
    .json())
}

fn web_fixture(ask: &str) -> Result<Fixture, Error> {
    let mut fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The prerequisite is verified and the requested guide is delivered."],
    )?;
    fixture.assignment.ask =
        Text::from(format!("{ask}\n\n## Files allowed\n- docs/guide.md").as_str());
    fixture.task(
        &["docs/guide.md"],
        "none",
        &["The prerequisite is verified and the requested guide is delivered."],
    )?;
    Ok(fixture)
}

#[tokio::test(flavor = "current_thread")]
async fn review_repro_web_unrelated_negation_is_unavailable_before_writes() -> Result<(), Error> {
    let fixture =
        web_fixture("Use web_search to verify the prerequisite, but do not edit unrelated files.")?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Incorrectly admitted prerequisite\n"),
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
            finalization::Status::Unavailable,
            "{:?}",
            coder.failure()
        );
        assert_eq!(fixture.read("docs/guide.md")?, "# Guide\n\nBefore.\n");
        assert!(transport.requests.is_empty());
        assert!(boundary.written_files().is_empty());
        assert!(boundary.last_tests().is_none());
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn review_repro_documentation_coordination_modifiers_do_not_hide_required_web()
-> Result<(), Error> {
    let ask = "Document the result, but first use web_search; do not edit other files.";
    let fixture = web_fixture(ask)?;
    assert!(
        fixture
            .read("TASK.md")?
            .contains("# Task: Document the result, but first use web_search")
    );
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write(
                    "docs/guide.md",
                    "# Incorrectly accepted coordinated invocation\n",
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
            finalization::Status::Unavailable,
            "{ask}: {:?}",
            coder.failure()
        );
        assert_eq!(fixture.read("docs/guide.md")?, "# Guide\n\nBefore.\n");
        assert!(transport.requests.is_empty());
        assert!(boundary.written_files().is_empty());
        assert!(boundary.last_tests().is_none());
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

fn timing_fixture() -> Result<Fixture, Error> {
    let fixture = Fixture::new(
        &["docs/guide.md", "tests/repeat.rs"],
        "characterization",
        &["The requested guide and actual Rust checks are completed."],
    )?;
    fixture.write(
        "Cargo.toml",
        "[package]\nname = \"owned-review-timing\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    fixture.write(
        "Cargo.lock",
        "version = 4\n\n[[package]]\nname = \"owned-review-timing\"\nversion = \"0.1.0\"\n",
    )?;
    fixture.write("src/lib.rs", "pub fn observed() -> u8 {\n    1\n}\n")?;
    fixture.write(
        "tests/repeat.rs",
        r#"use std::fs;
use std::time::Duration;

#[test]
fn fixed_native_failure() -> Result<(), Box<dyn std::error::Error>> {
    std::panic::set_hook(Box::new(|info| {
        if let Some(message) = info.payload().downcast_ref::<String>() {
            eprintln!("{message}");
        } else if let Some(message) = info.payload().downcast_ref::<&str>() {
            eprintln!("{message}");
        }
    }));
    let path = ".roster/logs/native-cargo-counter";
    let turn = match fs::read_to_string(path) {
        Ok(value) => value.parse::<u64>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    if turn > 32 {
        return Err(std::io::Error::other("Owned timing fixture exceeded its check bound").into());
    }
    fs::write(path, (turn + 1).to_string())?;
    std::thread::sleep(Duration::from_millis(100 + turn * 150));
    assert_eq!(owned_review_timing::observed(), 2);
    Ok(())
}
"#,
    )?;
    fixture.git(&[
        "add",
        "Cargo.toml",
        "Cargo.lock",
        "src/lib.rs",
        "tests/repeat.rs",
    ])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned stable native failure with measured timing",
    ])?;
    files::create_private_directories(&fixture.root.join(".roster").join("logs"))?;
    Ok(fixture)
}

#[tokio::test(flavor = "current_thread")]
async fn review_repro_actual_cargo_timing_changes_cannot_extend_repeated_failure_repairs()
-> Result<(), Error> {
    let fixture = timing_fixture()?;
    let source = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        fixture.assignment.clone(),
        tools::Options::default(),
        super::resources::cargo_host(&fixture)?,
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let result = async {
        boundary.enable_owned_checks()?;
        let warm = boundary.verify(false).await?;
        assert_ne!(warm.get("exit_code"), Some(&Value::Number(0.0)));
        assert!(
            warm.get("stdout")
                .is_some_and(|output| output.json().contains("fixed_native_failure"))
        );
        let mut responses = vec![calls(vec![(
            "write",
            "write_file",
            write("docs/guide.md", "# Actual bounded repair attempt\n"),
        )])];
        for _ in 0..8 {
            responses.push(calls(vec![("test", "run_test", Value::object(Vec::new()))]));
        }
        let mut transport = Recorded::new(responses);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 9)?,
                coder::Options::default(),
            )
            .await?;
        let failures = coder
            .result()
            .get("testFailures")
            .and_then(domain::items)
            .ok_or_else(|| Error::type_error("Actual native failure history is missing"))?;
        assert!(
            failures
                .iter()
                .all(|failure| failure.get("output").is_some_and(|output| output
                    .json()
                    .contains("fixed_native_failure")
                    && output.json().contains("left: 1")
                    && output.json().contains("right: 2")))
        );
        assert_eq!(
            coder.handoff(),
            Some(Handoff::RepeatedFailure),
            "{:?}; actual checks {}",
            coder.failure(),
            failures.len()
        );
        let diagnostics = cargo_failure_diagnostics(&fixture.container, failures)?;
        eprintln!("Actual Cargo failure transitions: {diagnostics}");
        assert_eq!(failures.len(), 3, "{diagnostics}");
        assert_eq!(transport.requests.len(), 4);
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

fn author_message(role: &str, function: bool) -> Value {
    let mut value = message(
        role,
        Text::from(
            r#"{"checks":[{"id":1,"met":true,"evidence":"Current actual source and checks were observed."}],"findings":[]}"#,
        ),
    );
    if function {
        if let Value::Object(fields) = &mut value {
            fields.push((
                Text::from("function_call"),
                Value::object(vec![
                    ("name", Value::string("write_file")),
                    ("arguments", Value::string("{}")),
                ]),
            ));
        }
    }
    value
}

#[tokio::test(flavor = "current_thread")]
async fn review_repro_native_cache_rebuild_cannot_count_as_test_repair_progress()
-> Result<(), Error> {
    let fixture = timing_fixture()?;
    let source = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        fixture.assignment.clone(),
        tools::Options::default(),
        super::resources::cargo_host(&fixture)?,
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let result = async {
        boundary.enable_owned_checks()?;
        let warm = boundary.verify(false).await?;
        assert_ne!(warm.get("exit_code"), Some(&Value::Number(0.0)));
        let target = fixture
            .container
            .join("approved-target")
            .join("debug")
            .join(".fingerprint");
        let mut responses = vec![calls(vec![(
            "write",
            "write_file",
            write("docs/guide.md", "# Actual bounded repair attempt\n"),
        )])];
        for _ in 0..8 {
            responses.push(calls(vec![("test", "run_test", Value::object(Vec::new()))]));
        }
        let mut transport = Recorded::new(responses);
        transport.action = Some(Box::new(move |request, _| {
            if request == 2 {
                fs::remove_dir_all(&target).map_err(io)?;
            }
            Ok(())
        }));
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 9)?,
                coder::Options::default(),
            )
            .await?;
        let failures = coder
            .result()
            .get("testFailures")
            .and_then(domain::items)
            .ok_or_else(|| {
                Error::type_error("Actual native cache-rebuild failure history missing")
            })?;
        let diagnostics = cargo_failure_diagnostics(&fixture.container, failures)?;
        eprintln!("Actual Cargo cache-rebuild transitions: {diagnostics}");
        assert!(
            failures
                .iter()
                .all(|failure| failure.get("output").is_some_and(|output| output
                    .json()
                    .contains("fixed_native_failure")
                    && output.json().contains("left: 1")
                    && output.json().contains("right: 2")))
        );
        assert_eq!(
            coder.handoff(),
            Some(Handoff::RepeatedFailure),
            "{diagnostics}"
        );
        assert_eq!(failures.len(), 3, "{diagnostics}");
        assert_eq!(transport.requests.len(), 4);
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}
async fn author_envelope_repro(role: &str, function: bool) -> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["The requested increment preserves the retained zero behavior."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let invalid = packet(
            author_message(role, function),
            "stop",
            "actual-invalid-author",
            Some(Value::object(vec![
                ("prompt_tokens", Value::Number(0.0)),
                ("completion_tokens", Value::Number(7.0)),
            ])),
        );
        let mut transport = Recorded::new(vec![
            calls(vec![
                ("read", "read_file", path("src/app.mjs")),
                (
                    "write",
                    "write_file",
                    write(
                        "src/app.mjs",
                        "export function answer(value = 0) { return value + 1; }\n",
                    ),
                ),
            ]),
            summary(),
            invalid,
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
        assert_ne!(coder.status(), finalization::Status::Pass);
        assert_eq!(
            coder.failure().and_then(|error| error.code),
            Some("ROSTER_TOOL_FREE_AUTHORITY")
        );
        assert!(
            boundary
                .last_tests()
                .is_some_and(|proof| proof.result().get("exit_code") == Some(&Value::Number(0.0)))
        );
        assert!(coder.result().get("selfReview").is_some_and(|review| {
            domain::is_text(review.get("status"), "rejected")
                && domain::is_text(review.get("model"), "actual-invalid-author")
                && review.get("usage").is_some_and(|usage| {
                    usage.get("prompt_tokens") == Some(&Value::Number(0.0))
                        && usage.get("completion_tokens") == Some(&Value::Number(7.0))
                })
        }));
        assert!(
            coder
                .result()
                .get("selfReviewResponse")
                .is_some_and(|response| domain::is_text(
                    response.get("model"),
                    "actual-invalid-author"
                ))
        );
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn review_repro_authority_invalid_author_envelope_cannot_finalize_clean() -> Result<(), Error>
{
    author_envelope_repro("user", false).await
}

#[tokio::test(flavor = "current_thread")]
async fn review_repro_tool_role_author_envelope_cannot_finalize_clean() -> Result<(), Error> {
    author_envelope_repro("tool", false).await
}

#[tokio::test(flavor = "current_thread")]
async fn review_repro_legacy_function_call_author_envelope_cannot_finalize_clean()
-> Result<(), Error> {
    author_envelope_repro("assistant", true).await
}
