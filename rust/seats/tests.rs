use std::collections::VecDeque;
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::cancel::{CancellationSource, Reason};
use crate::execution::{backend, finalization};
use crate::llm::Failure;
use crate::llm::http::{Body, Request, Response};
use crate::planning::{artifacts, lifecycle};
use crate::state::{files, locks, redaction};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

mod composition_controls;
mod focused_review;
mod loop_controls;
mod metadata_controls;
mod resources;

fn io(error: std::io::Error) -> Error {
    Error::new("Error", error.to_string())
}

struct Fixture {
    container: PathBuf,
    root: PathBuf,
    assignment: lifecycle::Assignment,
}

impl Fixture {
    fn new(paths: &[&str], mode: &str, checks: &[&str]) -> Result<Self, Error> {
        let container = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".native-seats-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        files::create_private_directories(&container)?;
        let root = container.join("project");
        files::create_private_directories(&root)?;
        let fixture = Self {
            container,
            root,
            assignment: lifecycle::Assignment {
                ask: Text::from(format!("Implement the requested bounded outcome and preserve existing behavior.\n\n## Files allowed\n{}",
                    paths.iter().map(|path| format!("- {path}")).collect::<Vec<_>>().join("\n")).as_str()),
                title: None, reference: "issue:42".to_owned(), task: "issue-42".to_owned(), session: "native-seat-tests".to_owned(),
            },
        };
        fixture.git(&["init", "--quiet"])?;
        fixture.write(".gitignore", ".roster/\n.roster-state/\nTASK.md\nRECIPE.yml\nRESULT.md\nREVIEW.md\nRESEARCH.md\nCONTEXT.md\ntarget/\n")?;
        fixture.write("AGENTS.md", "# Owned consumer\n\n## Coding\n\n- Preserve current source and truthful native receipts.\n")?;
        fixture.write("docs/guide.md", "# Guide\n\nBefore.\n")?;
        fixture.write("README.md", "# Owned consumer\n")?;
        fixture.write(
            "src/app.mjs",
            "export function answer(value = 0) { return 1; }\n",
        )?;
        fixture.write("src/obsolete.mjs", "export const obsolete = true;\n")?;
        fixture.write("tests/app.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('retained zero input', () => assert.equal(answer(0), 1));\n")?;
        fixture.git(&["add", "."])?;
        fixture.git(&["commit", "--quiet", "-m", "Owned native seat consumer base"])?;
        fixture.task(paths, mode, checks)?;
        Ok(fixture)
    }

    fn task(&self, paths: &[&str], mode: &str, checks: &[&str]) -> Result<(), Error> {
        let plan = artifacts::build(
            &Value::String(self.assignment.ask.clone()),
            artifacts::BuildOptions {
                reference: Some("issue:42"),
                files: Some(&Value::Array(
                    paths.iter().map(|path| Value::string(path)).collect(),
                )),
                checks: Some(&Value::Array(
                    checks.iter().map(|check| Value::string(check)).collect(),
                )),
                ..artifacts::BuildOptions::default()
            },
        )?;
        let source = plan.task.to_utf8()?;
        let source = if source.lines().any(|line| line.starts_with("tests:")) {
            source
                .lines()
                .map(|line| {
                    if line.starts_with("tests:") {
                        format!("tests: {mode}")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        } else {
            source.replacen("---\n", &format!("---\ntests: {mode}\n"), 1)
        };
        self.write("TASK.md", &source)?;
        self.write("RECIPE.yml", &plan.recipe.to_utf8()?)
    }

    fn write(&self, file: &str, source: &str) -> Result<(), Error> {
        let path = self.root.join(file);
        files::create_private_directories(
            path.parent()
                .ok_or_else(|| Error::type_error("Fixture path needs a parent"))?,
        )?;
        fs::write(path, source).map_err(io)
    }

    fn read(&self, file: &str) -> Result<String, Error> {
        fs::read_to_string(self.root.join(file)).map_err(io)
    }

    fn git(&self, args: &[&str]) -> Result<String, Error> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .env_clear()
            .envs(tools::Host::default().environment)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "--no-pager",
                "-c",
                "init.templateDir=",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Owned Seat Fixture",
                "-c",
                "user.email=owned@example.invalid",
                "-c",
            ])
            .arg(format!(
                "core.hooksPath={}",
                self.root.join("disabled-hooks").display()
            ))
            .args(args)
            .output()
            .map_err(io)?;
        if !output.status.success() {
            return Err(Error::new("Error", tools::bytes_text(&output.stderr)?));
        }
        tools::bytes_text(&output.stdout)
    }

    async fn open(&self, source: &CancellationSource) -> Result<tools::Boundary, Error> {
        tools::Boundary::open(
            &self.root,
            self.assignment.clone(),
            tools::Options {
                scope_expansion: 0,
                slice_reads_only: true,
                ..tools::Options::default()
            },
            tools::Host::default(),
            redaction::Config::default(),
            source.token(),
        )
        .await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.container) {
            eprintln!("Owned native seat fixture cleanup failed: {error}");
        }
    }
}

struct Bytes(Option<Vec<u8>>);

impl Body for Bytes {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.0.take())
    }
}

type SendAction = Box<dyn FnMut(usize, &Value) -> Result<(), Error>>;

#[derive(Default)]
struct Recorded {
    responses: VecDeque<Value>,
    requests: Vec<Value>,
    action: Option<SendAction>,
}

impl Recorded {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: responses.into(),
            ..Self::default()
        }
    }
}

impl Transport for Recorded {
    type Body = Bytes;
    async fn send(&mut self, request: &Request) -> Result<Response<Bytes>, Failure> {
        let value = crate::json::parse(&Text::from(
            std::str::from_utf8(&request.body)
                .map_err(|_| Error::type_error("Fixture request must be UTF-8 JSON"))?,
        ))?;
        self.requests.push(value.clone());
        if let Some(action) = &mut self.action {
            action(self.requests.len(), &value)?;
        }
        let value = self.responses.pop_front().ok_or_else(|| {
            Error::new(
                "OwnedScriptExhausted",
                "No scripted native seat response remains",
            )
        })?;
        Ok(Response {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Some(Bytes(Some(value.json().into_bytes()))),
        })
    }
}

#[derive(Default)]
struct Events(Vec<Value>);

impl Observer for Events {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        self.0.push(event);
        Ok(())
    }
}

fn config(configured: bool, evidence: bool, turns: u64) -> Result<crate::config::Config, Error> {
    let mut source = include_str!("../../roster.config.example.yml")
        .replacen("model: \"\"", "model: configured-route", 1)
        .replace("turn_budget: 1000", &format!("turn_budget: {turns}"))
        .replace(
            "evidence_workspace: false",
            &format!("evidence_workspace: {evidence}"),
        );
    if configured {
        source = source.replacen("base_url: \"\"", "base_url: http://127.0.0.1:1/v1", 1);
    }
    crate::config::parse(&Text::from(source.as_str()))
}

fn packet(message: Value, finish: &str, model: &str, usage: Option<Value>) -> Value {
    let mut value = Value::object(vec![
        ("model", Value::string(model)),
        (
            "choices",
            Value::Array(vec![Value::object(vec![
                ("message", message),
                ("finish_reason", Value::string(finish)),
            ])]),
        ),
    ]);
    if let Some(usage) = usage {
        if let Value::Object(fields) = &mut value {
            fields.push((Text::from("usage"), usage));
        }
    }
    value
}

fn summary() -> Value {
    packet(
        message(
            "assistant",
            Text::from("1. done: the requested current files and matching checks were verified."),
        ),
        "stop",
        "actual-coder",
        None,
    )
}

fn calls(calls: Vec<(&str, &str, Value)>) -> Value {
    packet(
        Value::object(vec![
            ("role", Value::string("assistant")),
            ("content", Value::Null),
            (
                "tool_calls",
                Value::Array(
                    calls
                        .into_iter()
                        .map(|(id, name, args)| {
                            Value::object(vec![
                                ("id", Value::string(id)),
                                ("type", Value::string("function")),
                                (
                                    "function",
                                    Value::object(vec![
                                        ("name", Value::string(name)),
                                        ("arguments", Value::string(&args.json())),
                                    ]),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        "tool_calls",
        "actual-coder",
        None,
    )
}

fn path(path: &str) -> Value {
    Value::object(vec![("path", Value::string(path))])
}
fn write(path: &str, content: &str) -> Value {
    Value::object(vec![
        ("path", Value::string(path)),
        ("content", Value::string(content)),
    ])
}

fn verdict(checks: usize) -> Value {
    Value::object(vec![
        ("verdict", Value::string("pass")), ("reasons", Value::Array(Vec::new())),
        ("security_notes", Value::Array(Vec::new())),
        ("checks", Value::Array((1..=checks).map(|id| Value::object(vec![
            ("id", Value::Number(id as f64)), ("met", Value::Boolean(true)),
            ("evidence", Value::string("Current delivered source and actual host observations meet the requested outcome.")),
        ])).collect())),
    ])
}

fn review_packet(report: Value) -> Value {
    packet(
        message("assistant", Text::from(report.json().as_str())),
        "stop",
        "actual-reviewer",
        None,
    )
}

fn driver<'a>(
    transport: &'a mut Recorded,
    events: &'a mut Events,
    credentials: &'a Credentials,
    admission: &'a Admission,
) -> Driver<'a, Recorded, Events> {
    Driver {
        transport,
        observer: events,
        credentials,
        admission,
        signal: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn configured_docs_coder_and_fresh_readonly_reviewer_execute_and_bind_actual_artifacts()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The guide explains the requested outcome."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![("read", "read_file", path("docs/guide.md"))]),
            calls(vec![(
                "write",
                "write_file",
                write(
                    "docs/guide.md",
                    "# Guide\n\nThe requested outcome is documented.\n",
                ),
            )]),
            summary(),
            review_packet(verdict(1)),
            review_packet(Value::object(vec![(
                "requests",
                Value::Array(vec![Value::object(vec![
                    ("tool", Value::string("read_file")),
                    ("path", Value::string("docs/guide.md")),
                ])]),
            )])),
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
        assert_eq!(
            coder.status(),
            finalization::Status::Pass,
            "{:?}",
            coder.failure()
        );
        assert_eq!(
            coder.run().map(|run| run.metrics().model.as_str()),
            Some("actual-coder")
        );
        assert!(fixture.read("RESULT.md")?.contains("Checks: PASS"));
        assert_eq!(
            fixture.read("docs/guide.md")?,
            "# Guide\n\nThe requested outcome is documented.\n"
        );
        let reviewer = driver
            .run_reviewer(
                &boundary,
                &coder,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 10)?,
                reviewer::Options::default(),
            )
            .await?;
        assert_eq!(
            reviewer.status(),
            reviewer::Status::Pass,
            "{:?}",
            reviewer.failure()
        );
        reviewer
            .require_passing(&boundary, &coder, &evidence::Projection::default())
            .await?;
        assert!(fixture.read("REVIEW.md")?.contains("Verdict: pass"));
        assert_eq!(transport.requests.len(), 6);
        for request in transport.requests.iter().skip(3) {
            assert!(request.get("tools").is_none());
        }
        assert!(
            !transport
                .requests
                .get(3)
                .ok_or_else(|| Error::type_error("Audit request missing"))?
                .json()
                .contains("Actual coder RESULT.md")
        );
        fixture.write("docs/guide.md", "Human late source mutation\n")?;
        assert!(
            reviewer
                .require_passing(&boundary, &coder, &evidence::Projection::default())
                .await
                .is_err()
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn stub_writes_only_owned_unverified_summary_without_model_product_or_test_execution()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs"],
        "required",
        &["The requested behavior is implemented."],
    )?;
    let original = fixture.read("src/app.mjs")?;
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
                &config(false, false, 10)?,
                coder::Options::default(),
            )
            .await?;
        assert_eq!(coder.status(), finalization::Status::Stub);
        assert!(coder.run().is_none());
        assert!(transport.requests.is_empty());
        assert_eq!(fixture.read("src/app.mjs")?, original);
        assert!(boundary.written_files().is_empty());
        assert!(boundary.last_tests().is_none());
        assert!(boundary.first_checkpoint().is_none());
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        assert!(
            fixture
                .read("RESULT.md")?
                .contains("no implementation, product edits or tests")
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

fn self_review_packet(checks: usize, findings: Vec<&str>) -> Value {
    let mut report = verdict(checks);
    let checks = report.get("checks").cloned().unwrap_or(Value::Null);
    report = Value::object(vec![
        ("checks", checks),
        (
            "findings",
            Value::Array(
                findings
                    .iter()
                    .map(|finding| Value::string(finding))
                    .collect(),
            ),
        ),
    ]);
    packet(
        message("assistant", Text::from(report.json().as_str())),
        "stop",
        "actual-author-review",
        Some(Value::object(vec![
            ("prompt_tokens", Value::Number(9.0)),
            ("completion_tokens", Value::Number(0.0)),
        ])),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn configured_product_coder_executes_read_edit_write_delete_tests_counterfactual_and_separate_selfreview()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "src/obsolete.mjs", "tests/app.next.test.mjs"],
        "required",
        &["Nonzero input increments while existing zero behavior is preserved."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let test_source = "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('increments nonzero input', () => assert.equal(answer(2), 3));\n";
        let mut transport = Recorded::new(vec![
            calls(vec![("a", "read_file", path("src/app.mjs")), ("b", "read_file", path("src/obsolete.mjs"))]),
            calls(vec![
                ("edit", "edit_file", Value::object(vec![("path", Value::string("src/app.mjs")), ("old_string", Value::string("return 1")), ("new_string", Value::string("return value + 1"))])),
                ("write", "write_file", write("tests/app.next.test.mjs", test_source)),
                ("delete", "delete_file", path("src/obsolete.mjs")),
            ]),
            calls(vec![("test", "run_test", Value::object(Vec::new()))]),
            summary(), self_review_packet(1, Vec::new()),
            review_packet(verdict(1)), review_packet(verdict(1)),
        ]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let mut driver = driver(&mut transport, &mut events, &credentials, &admission);
        let node = std::env::var_os("ROSTER_TEST_NODE").map(PathBuf::from).ok_or_else(|| Error::type_error("Owned consumer requires explicit Node 20"))?;
        let coder = driver.run_coder(&mut boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &config(true, false, 20)?, coder::Options {
            approval: Some(backend::Approval { counterfactual: true, node: Some(node), timeout_ms: Some(30_000), ..backend::Approval::default() }),
            ..coder::Options::default()
        }).await?;
        assert_eq!(coder.status(), finalization::Status::Pass, "{:?}\n{}", coder.failure(), coder.excellence().json());
        assert!(fixture.read("src/app.mjs")?.contains("return value + 1"));
        assert_eq!(fixture.read("tests/app.next.test.mjs")?, test_source);
        assert!(!fixture.root.join("src").join("obsolete.mjs").exists());
        assert!(coder.result().get("tests").is_some_and(|tests| tests.get("exit_code") == Some(&Value::Number(0.0))));
        assert!(coder.result().get("redGreen").is_some());
        assert!(coder.result().get("selfReview").is_some_and(|review| domain::is_text(review.get("model"), "actual-author-review")));
        assert_eq!(coder.run().map(|run| run.metrics().model.as_str()), Some("actual-coder"));
        assert_eq!(coder.run().map(|run| run.metrics().usage), Some(crate::run::Usage::default()));
        assert!(boundary.first_checkpoint().is_some());
        let review = driver.run_reviewer(&boundary, &coder, Path::new(env!("CARGO_MANIFEST_DIR")), &config(true, false, 20)?, reviewer::Options::default()).await?;
        assert_eq!(review.status(), reviewer::Status::Pass, "{:?}", review.failure());
        review.require_passing(&boundary, &coder, &evidence::Projection::default()).await?;
        assert!(fixture.read("RESULT.md")?.contains("increments nonzero input"));
        Ok(())
    }.await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_call_correction_and_reused_gateway_ids_execute_only_current_normalized_batch()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide text is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let malformed = packet(
            Value::object(vec![
                ("role", Value::string("assistant")),
                ("content", Value::Null),
                (
                    "tool_calls",
                    Value::Array(vec![Value::object(vec![
                        ("id", Value::string("gateway")),
                        ("type", Value::string("function")),
                        (
                            "function",
                            Value::object(vec![
                                ("name", Value::string("read_file")),
                                ("arguments", Value::string("not-json")),
                            ]),
                        ),
                    ])]),
                ),
            ]),
            "tool_calls",
            "actual-coder",
            None,
        );
        let mut transport = Recorded::new(vec![
            malformed,
            calls(vec![("gateway", "read_file", path("docs/guide.md"))]),
            calls(vec![(
                "gateway",
                "write_file",
                write("docs/guide.md", "# Actual guide\n\nRequested text.\n"),
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
            fixture.read("docs/guide.md")?,
            "# Actual guide\n\nRequested text.\n"
        );
        let execution_ids = events
            .0
            .iter()
            .filter(|event| domain::is_text(event.get("type"), "seat-tool"))
            .filter_map(|event| event.get("id").and_then(Value::as_string))
            .collect::<Vec<_>>();
        assert_eq!(execution_ids.len(), 2);
        assert_ne!(execution_ids.first(), execution_ids.get(1));
        let last = transport
            .requests
            .last()
            .ok_or_else(|| Error::type_error("Final request missing"))?;
        let tool_ids = last
            .get("messages")
            .and_then(domain::items)
            .ok_or_else(|| Error::type_error("Messages missing"))?
            .iter()
            .filter(|message| domain::is_text(message.get("role"), "tool"))
            .filter_map(|message| message.get("tool_call_id").and_then(Value::as_string))
            .collect::<Vec<_>>();
        assert_eq!(tool_ids, execution_ids);
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn protected_or_unoffered_calls_cannot_write_harness_policy_vendor_or_products()
-> Result<(), Error> {
    for path_name in [
        "RESULT.md",
        "TASK.md",
        ".env",
        "vendor/foreign.txt",
        ".github/workflows/build.yml",
        "../escape.txt",
    ] {
        let fixture = Fixture::new(&["docs/guide.md"], "none", &["The guide is updated."])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let task = fixture.read("TASK.md")?;
            let mut transport = Recorded::new(vec![calls(vec![(
                "denied",
                "write_file",
                write(path_name, "Model cannot own this"),
            )])]);
            let mut events = Events::default();
            let credentials = Credentials::default();
            let admission = Admission::default();
            let coder = driver(&mut transport, &mut events, &credentials, &admission)
                .run_coder(
                    &mut boundary,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &config(true, false, 4)?,
                    coder::Options::default(),
                )
                .await?;
            assert_ne!(coder.status(), finalization::Status::Pass);
            assert_eq!(fixture.read("TASK.md")?, task);
            assert_eq!(fixture.read("docs/guide.md")?, "# Guide\n\nBefore.\n");
            assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
            assert!(boundary.last_tests().is_none());
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn no_endpoint_reviewer_is_unavailable_and_never_stub_passes() -> Result<(), Error> {
    let fixture = Fixture::new(&["docs/guide.md"], "none", &["The guide is updated."])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Updated guide\n"),
            )]),
            summary(),
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
        assert_eq!(coder.status(), finalization::Status::Pass);
        let review = driver
            .run_reviewer(
                &boundary,
                &coder,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(false, false, 10)?,
                reviewer::Options::default(),
            )
            .await?;
        assert_eq!(review.status(), reviewer::Status::Unavailable);
        assert!(review.response().is_none());
        assert!(
            review
                .require_passing(&boundary, &coder, &evidence::Projection::default())
                .await
                .is_err()
        );
        assert!(fixture.read("REVIEW.md")?.contains("configured endpoint"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn future_cancel_after_current_request_barrier_preserves_first_reason_and_unverified_edits()
-> Result<(), Error> {
    let fixture = Fixture::new(&["docs/guide.md"], "none", &["The guide is updated."])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let cancel = source.clone();
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# In-progress guide\n"),
            )]),
            summary(),
        ]);
        transport.action = Some(Box::new(move |request, _| {
            if request == 2 {
                cancel.cancel(Reason::TaskTimeout(7))?;
                cancel.cancel(Reason::Cancelled)?;
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
                &config(true, false, 10)?,
                coder::Options::default(),
            )
            .await?;
        assert_eq!(coder.status(), finalization::Status::Cancelled);
        assert_eq!(
            coder.failure().and_then(|error| error.code),
            Some("ROSTER_TASK_TIMEOUT")
        );
        assert_eq!(fixture.read("docs/guide.md")?, "# In-progress guide\n");
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        assert!(fixture.read("RESULT.md")?.contains("after 7 ms"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn actual_failed_full_suite_survives_a_subsequent_green_scoped_model_call()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Nonzero input increments and zero behavior remains."],
    )?;
    fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('outside existing failure', () => assert.equal(1, 2));\n")?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned preexisting outside failure",
    ])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![
                ("read", "read_file", path("src/app.mjs")),
                (
                    "edit",
                    "write_file",
                    write(
                        "src/app.mjs",
                        "export function answer(value = 0) { return value + 1; }\n",
                    ),
                ),
            ]),
            summary(),
            calls(vec![("scoped", "run_test", Value::object(Vec::new()))]),
            summary(),
        ]);
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
        assert_ne!(coder.status(), finalization::Status::Pass);
        let full = coder
            .result()
            .get("fullVerification")
            .ok_or_else(|| Error::type_error("Failed full obligation missing"))?;
        assert_eq!(full.get("original_exit"), Some(&Value::Number(1.0)));
        assert_eq!(full.get("last_failed_exit"), Some(&Value::Number(1.0)));
        assert_eq!(full.get("clean_full_suite"), Some(&Value::Boolean(false)));
        let request = transport
            .requests
            .last()
            .ok_or_else(|| Error::type_error("Scoped response request missing"))?;
        let responses = request
            .get("messages")
            .and_then(domain::items)
            .ok_or_else(|| Error::type_error("Messages missing"))?;
        assert!(
            responses
                .iter()
                .filter(|message| domain::is_text(message.get("name"), "run_test"))
                .any(|message| message
                    .get("content")
                    .and_then(Value::as_string)
                    .is_some_and(|content| content
                        .to_utf8()
                        .is_ok_and(|content| content.contains("\"exit_code\":0"))))
        );
        assert!(
            fixture
                .read("RESULT.md")?
                .contains("Original full-suite exit: 1")
        );
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn complete_current_outside_classification_and_fresh_scoped_evidence_can_finish_with_retained_original_exit()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Nonzero input increments and zero behavior remains."],
    )?;
    fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('outside existing failure', () => assert.equal(1, 2));\n")?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned fully mapped baseline failure",
    ])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![
                ("read", "read_file", path("src/app.mjs")),
                (
                    "edit",
                    "write_file",
                    write(
                        "src/app.mjs",
                        "export function answer(value = 0) { return value + 1; }\n",
                    ),
                ),
            ]),
            summary(),
            self_review_packet(1, Vec::new()),
        ]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let node = std::env::var_os("ROSTER_TEST_NODE")
            .map(PathBuf::from)
            .ok_or_else(|| Error::type_error("Owned consumer needs Node 20"))?;
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 12)?,
                coder::Options {
                    approval: Some(backend::Approval {
                        classifications: true,
                        node: Some(node),
                        timeout_ms: Some(30_000),
                        ..backend::Approval::default()
                    }),
                    ..coder::Options::default()
                },
            )
            .await?;
        assert_eq!(
            coder.status(),
            finalization::Status::Pass,
            "{:?}\nGates: {}\nClassification: {}",
            coder.failure(),
            coder
                .result()
                .get("gateFailures")
                .map_or("null".to_owned(), Value::json),
            coder
                .result()
                .get("fullClassification")
                .map_or("null".to_owned(), Value::json)
        );
        let full = coder
            .result()
            .get("fullVerification")
            .ok_or_else(|| Error::type_error("Full obligation missing"))?;
        assert_eq!(full.get("original_exit"), Some(&Value::Number(1.0)));
        assert_eq!(
            full.get("resolution"),
            Some(&Value::string("outside-exception"))
        );
        assert_eq!(full.get("clean_full_suite"), Some(&Value::Boolean(false)));
        assert!(coder.result().get("classification").is_some());
        assert!(
            fixture
                .read("RESULT.md")?
                .contains("PASS (with test exceptions)")
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn actual_repair_failure_fingerprints_stop_oscillation_without_success() -> Result<(), Error>
{
    let fixture = Fixture::new(
        &["src/app.mjs", "docs/guide.md"],
        "characterization",
        &["The original zero behavior remains correct."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut scripted = Vec::new();
        for number in [2, 3, 2, 2] {
            scripted.push(calls(vec![
                ("read", "read_file", path("src/app.mjs")),
                (
                    "write",
                    "write_file",
                    write(
                        "src/app.mjs",
                        &format!("export function answer() {{ return {number}; }}\n"),
                    ),
                ),
                ("test", "run_test", Value::object(Vec::new())),
            ]));
        }
        let mut transport = Recorded::new(scripted);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let coder = driver(&mut transport, &mut events, &credentials, &admission)
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 16)?,
                coder::Options::default(),
            )
            .await?;
        assert_eq!(
            coder.handoff(),
            Some(Handoff::RepeatedFailure),
            "{:?}\n{}",
            coder.failure(),
            coder
                .result()
                .get("testFailures")
                .map_or(Value::Null, Clone::clone)
                .json()
        );
        assert_ne!(coder.status(), finalization::Status::Pass);
        assert_eq!(transport.requests.len(), 4);
        assert!(fixture.read("RESULT.md")?.contains("repeated-failure"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn progressing_real_test_repairs_handoff_on_measured_context_not_guessed_usage()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["The original zero behavior remains correct."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut scripted = Vec::new();
        for number in [2, 3] {
            let mut response = calls(vec![
                ("read", "read_file", path("src/app.mjs")),
                (
                    "write",
                    "write_file",
                    write(
                        "src/app.mjs",
                        &format!("export function answer() {{ return {number}; }}\n"),
                    ),
                ),
                ("test", "run_test", Value::object(Vec::new())),
            ]);
            domain::put(
                &mut response,
                "usage",
                Value::object(vec![
                    ("prompt_tokens", Value::Number(60.0)),
                    ("completion_tokens", Value::Number(2.0)),
                ]),
            )?;
            scripted.push(response);
        }
        let source_config = crate::config::format(config(true, false, 10)?.as_value())?
            .to_utf8()?
            .replace("context_max: 0", "context_max: 100");
        let config = crate::config::parse(&Text::from(source_config.as_str()))?;
        let mut transport = Recorded::new(scripted);
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
            coder.handoff(),
            Some(Handoff::ContextBudget),
            "{:?}",
            coder.failure()
        );
        assert!(
            coder
                .failure()
                .is_some_and(|error| error.message.contains("measured context"))
        );
        assert!(fixture.read("RESULT.md")?.contains("Checks: FAIL"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn scope_limit_is_an_explicit_unverified_host_handoff_with_preserved_first_edits()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        fixture.assignment.clone(),
        tools::Options {
            scope_expansion: 1,
            ..tools::Options::default()
        },
        tools::Host::default(),
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let result = async {
        let mut transport = Recorded::new(vec![calls(vec![
            (
                "first",
                "write_file",
                write("docs/one.md", "# Recorded expansion\n"),
            ),
            (
                "second",
                "write_file",
                write("docs/two.md", "# Never authorized\n"),
            ),
        ])]);
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
            coder.handoff(),
            Some(Handoff::ScopeBlocked),
            "{:?}",
            coder.failure()
        );
        assert_eq!(fixture.read("docs/one.md")?, "# Recorded expansion\n");
        assert!(!fixture.root.join("docs").join("two.md").exists());
        assert_eq!(
            coder.result().get("scopeBlocked"),
            Some(&Value::Array(vec![Value::string("docs/two.md")]))
        );
        assert_eq!(
            coder.result().get("scopeFiles"),
            Some(&Value::Array(vec![Value::string("docs/one.md")]))
        );
        assert_eq!(
            coder.result().get("writtenFiles"),
            Some(&Value::Array(vec![Value::string("docs/one.md")]))
        );
        assert!(
            fixture
                .read("RESULT.md")?
                .contains("Observed write/delete paths (not a verified diff)")
        );
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn turn_budget_noop_and_free_text_never_mint_implementation() -> Result<(), Error> {
    for (responses, turns, handoff) in [
        (
            vec![calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# New guide\n"),
            )])],
            1,
            Handoff::TurnBudget,
        ),
        (vec![summary(), summary()], 5, Handoff::NoProgress),
        (
            vec![
                packet(
                    message("assistant", Text::from("write_file docs/guide.md hacked")),
                    "stop",
                    "actual-coder",
                    None,
                ),
                summary(),
            ],
            5,
            Handoff::NoProgress,
        ),
    ] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["The requested guide is delivered."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let mut transport = Recorded::new(responses);
            let mut events = Events::default();
            let credentials = Credentials::default();
            let admission = Admission::default();
            let coder = driver(&mut transport, &mut events, &credentials, &admission)
                .run_coder(
                    &mut boundary,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &config(true, false, turns)?,
                    coder::Options::default(),
                )
                .await?;
            assert_eq!(coder.handoff(), Some(handoff), "{:?}", coder.failure());
            assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
            assert!(!fixture.read("docs/guide.md")?.contains("hacked"));
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_selected_skills_and_web_prerequisites_are_unavailable_before_writes()
-> Result<(), Error> {
    for skill in [true, false] {
        let mut fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["The requested guide is delivered."],
        )?;
        if skill {
            let task = fixture.read("TASK.md")?;
            let task = task
                .lines()
                .map(|line| {
                    if line.starts_with("skills:") {
                        "skills: [native-missing-prerequisite]"
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            fixture.write("TASK.md", &task)?;
        } else {
            fixture.assignment.ask = Text::from(
                "Use web_search to verify the prerequisite and document it.\n\n## Files allowed\n- docs/guide.md",
            );
            fixture.task(
                &["docs/guide.md"],
                "none",
                &["The requested guide is delivered."],
            )?;
        }
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
            assert_eq!(
                coder.status(),
                finalization::Status::Unavailable,
                "{:?}",
                coder.failure()
            );
            assert_eq!(coder.handoff(), Some(Handoff::Unavailable));
            assert!(transport.requests.is_empty());
            assert!(boundary.written_files().is_empty());
            assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn configured_unsupported_finish_after_a_real_write_never_falls_back_to_stub_or_pass()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Unverified guide\n"),
            )]),
            packet(
                message("assistant", Text::from("No verified completion")),
                "content_filter",
                "actual-failed-model",
                None,
            ),
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
            Some("ROSTER_LLM_FINISH_REASON")
        );
        assert_eq!(coder.result().get("mode"), Some(&Value::string("llm")));
        assert!(fixture.read("RESULT.md")?.contains("actual-failed-model"));
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn actual_zero_and_unknown_usage_remain_distinct_from_configured_model_and_separate_critic_identity()
-> Result<(), Error> {
    for absent in [false, true] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["The requested guide is delivered."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let mut write = calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Actual guide\n"),
            )]);
            domain::put(
                &mut write,
                "usage",
                Value::object(vec![
                    ("prompt_tokens", Value::Number(0.0)),
                    ("completion_tokens", Value::Number(0.0)),
                ]),
            )?;
            let mut done = summary();
            if !absent {
                domain::put(
                    &mut done,
                    "usage",
                    Value::object(vec![
                        ("prompt_tokens", Value::Number(0.0)),
                        ("completion_tokens", Value::Number(0.0)),
                    ]),
                )?;
            }
            let mut transport = Recorded::new(vec![write, done]);
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
            assert_eq!(coder.status(), finalization::Status::Pass);
            let metrics = coder
                .run()
                .ok_or_else(|| Error::type_error("Actual run missing"))?
                .metrics();
            assert_eq!(metrics.model, "actual-coder");
            assert_eq!(
                metrics.usage.prompt_tokens,
                if absent { None } else { Some(0) }
            );
            assert_eq!(
                metrics.usage.completion_tokens,
                if absent { None } else { Some(0) }
            );
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn reviewer_denies_write_requests_and_native_function_calls_without_mutation_or_success()
-> Result<(), Error> {
    for function in [false, true] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["The requested guide is delivered."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let denied = if function {
                calls(vec![(
                    "write",
                    "write_file",
                    write("docs/guide.md", "Reviewer intrusion"),
                )])
            } else {
                review_packet(Value::object(vec![(
                    "requests",
                    Value::Array(vec![Value::object(vec![
                        ("tool", Value::string("write_file")),
                        ("path", Value::string("docs/guide.md")),
                        ("content", Value::string("Reviewer intrusion")),
                    ])]),
                )]))
            };
            let mut transport = Recorded::new(vec![
                calls(vec![(
                    "write",
                    "write_file",
                    write("docs/guide.md", "# Delivered guide\n"),
                )]),
                summary(),
                review_packet(verdict(1)),
                denied,
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
            assert_ne!(review.status(), reviewer::Status::Pass);
            assert!(review.failure().is_some());
            assert_eq!(fixture.read("docs/guide.md")?, "# Delivered guide\n");
            assert!(!fixture.read("REVIEW.md")?.contains("Verdict: pass"));
            assert!(
                review
                    .require_passing(&boundary, &coder, &evidence::Projection::default())
                    .await
                    .is_err()
            );
            assert!(
                transport
                    .requests
                    .iter()
                    .skip(2)
                    .all(|request| request.get("tools").is_none())
            );
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn reviewer_requires_complete_unique_known_check_ids_and_strict_root_fields()
-> Result<(), Error> {
    for variant in 0..5 {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["The requested guide is delivered."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let mut invalid = verdict(1);
            match variant {
                0 => domain::put(&mut invalid, "checks", Value::Array(Vec::new()))?,
                1 => {
                    let entry = invalid
                        .get("checks")
                        .and_then(domain::items)
                        .and_then(|checks| checks.first())
                        .cloned()
                        .ok_or_else(|| Error::type_error("Fixture check missing"))?;
                    domain::put(
                        &mut invalid,
                        "checks",
                        Value::Array(vec![entry.clone(), entry]),
                    )?;
                }
                2 => {
                    let mut checks = invalid
                        .get("checks")
                        .and_then(domain::items)
                        .ok_or_else(|| Error::type_error("Fixture checks missing"))?
                        .to_vec();
                    let first = checks
                        .first_mut()
                        .ok_or_else(|| Error::type_error("Fixture check missing"))?;
                    domain::put(first, "id", Value::Number(2.0))?;
                    domain::put(&mut invalid, "checks", Value::Array(checks))?;
                }
                3 => domain::put(&mut invalid, "invented_requirement", Value::Boolean(true))?,
                _ => domain::put(
                    &mut invalid,
                    "reasons",
                    Value::Array(vec![Value::string("Negative reason contradicts pass")]),
                )?,
            }
            let mut transport = Recorded::new(vec![
                calls(vec![(
                    "write",
                    "write_file",
                    write("docs/guide.md", "# Delivered guide\n"),
                )]),
                summary(),
                review_packet(verdict(1)),
                review_packet(invalid.clone()),
                review_packet(invalid),
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
                reviewer::Status::Unavailable,
                "variant {variant}"
            );
            assert!(review.failure().is_some());
            assert!(!fixture.read("REVIEW.md")?.contains("Verdict: pass"));
            assert_eq!(transport.requests.len(), 5);
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn reviewer_unresolved_previous_findings_or_concrete_changed_file_defects_override_passing_checks()
-> Result<(), Error> {
    for previous in [false, true] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["The requested guide is delivered."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let mut report = verdict(1);
            if previous {
                domain::put(
                    &mut report,
                    "findings",
                    Value::Array(vec![Value::object(vec![
                        ("id", Value::Number(1.0)),
                        ("resolved", Value::Boolean(false)),
                        (
                            "evidence",
                            Value::string("docs/guide.md still omits the requested outcome."),
                        ),
                    ])]),
                )?;
            } else {
                domain::put(
                    &mut report,
                    "defects",
                    Value::Array(vec![Value::object(vec![
                        ("file", Value::string("docs/guide.md")),
                        ("symbol", Value::string("Instructions")),
                        ("input", Value::string("Follow the stated final operation")),
                        (
                            "outcome",
                            Value::string("The requested outcome is not obtained"),
                        ),
                    ])]),
                )?;
            }
            let mut transport = Recorded::new(vec![
                calls(vec![(
                    "write",
                    "write_file",
                    write("docs/guide.md", "# Delivered guide\n"),
                )]),
                summary(),
                review_packet(verdict(1)),
                review_packet(report),
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
            let findings = if previous {
                vec![Text::from("The guide omits the requested final operation.")]
            } else {
                Vec::new()
            };
            let review = driver
                .run_reviewer(
                    &boundary,
                    &coder,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &config(true, false, 10)?,
                    reviewer::Options {
                        previous_findings: &findings,
                        ..reviewer::Options::default()
                    },
                )
                .await?;
            assert_eq!(review.status(), reviewer::Status::Fail);
            assert!(review.failure().is_none());
            assert!(
                review
                    .report()
                    .get("checks")
                    .and_then(domain::items)
                    .is_some_and(|checks| checks
                        .iter()
                        .all(|check| check.get("met") == Some(&Value::Boolean(true))))
            );
            assert!(
                !review
                    .report()
                    .get("reasons")
                    .and_then(domain::items)
                    .is_none_or(|reasons| reasons.is_empty())
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
async fn original_ask_outcome_audit_dominates_weaker_green_planner_checks_with_exact_source_grounds()
-> Result<(), Error> {
    let mut fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The guide contains a local preview."],
    )?;
    fixture.assignment.ask = Text::from(
        "Document a local preview and the final public-serving procedure.\n\n## Files allowed\n- docs/guide.md",
    );
    fixture.task(
        &["docs/guide.md"],
        "none",
        &["The guide contains a local preview."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
            let reason = "docs/guide.md documents only local preview and omits the requested final public-serving procedure.";
            let mut audit = verdict(1);
            domain::put(&mut audit, "verdict", Value::string("fail"))?;
            domain::put(&mut audit, "reasons", Value::Array(vec![Value::string(reason)]))?;
            domain::put(&mut audit, "grounds", Value::Array(vec![Value::object(vec![
                ("obligation", Value::string("the final public-serving procedure")), ("file", Value::string("docs/guide.md")),
                ("excerpt", Value::string("Local preview only.")), ("reason", Value::string(reason)),
            ])]))?;
            let mut transport = Recorded::new(vec![
                calls(vec![("write", "write_file", write("docs/guide.md", "# Guide\n\nLocal preview only.\n"))]), summary(),
                review_packet(audit), review_packet(verdict(1)),
            ]);
            let mut events = Events::default();
            let credentials = Credentials::default();
            let admission = Admission::default();
            let mut driver = driver(&mut transport, &mut events, &credentials, &admission);
            let coder = driver.run_coder(&mut boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &config(true, false, 10)?, coder::Options::default()).await?;
            assert_eq!(coder.status(), finalization::Status::Pass);
            let review = driver.run_reviewer(&boundary, &coder, Path::new(env!("CARGO_MANIFEST_DIR")), &config(true, false, 10)?, reviewer::Options::default()).await?;
            assert_eq!(review.status(), reviewer::Status::Fail, "{:?}", review.failure());
            assert!(review.report().get("reasons").is_some_and(|reasons| reasons.json().contains("Original Ask audit")));
            assert!(fixture.read("REVIEW.md")?.contains("public-serving procedure"));
            assert!(!transport.requests.get(2).ok_or_else(|| Error::type_error("Audit request missing"))?.json().contains("The guide contains a local preview."));
            Ok(())
        }.await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn invented_original_ask_obligations_or_source_excerpts_are_not_accepted_as_grounded_failure()
-> Result<(), Error> {
    for invented_ask in [false, true] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["The requested guide is delivered."],
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source).await?;
        let result = async {
            let reason = "A concrete requested outcome is missing in docs/guide.md.";
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
                        Value::string(if invented_ask {
                            "Deploy an unrequested external system"
                        } else {
                            "Implement the requested bounded outcome"
                        }),
                    ),
                    ("file", Value::string("docs/guide.md")),
                    (
                        "excerpt",
                        Value::string(if invented_ask {
                            "Delivered guide."
                        } else {
                            "Invented absent source text"
                        }),
                    ),
                    ("reason", Value::string(reason)),
                ])]),
            )?;
            let mut transport = Recorded::new(vec![
                calls(vec![(
                    "write",
                    "write_file",
                    write("docs/guide.md", "# Guide\n\nDelivered guide.\n"),
                )]),
                summary(),
                review_packet(audit),
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
            assert!(review.failure().is_some_and(|error| error.message.contains(
                if invented_ask {
                    "obligation"
                } else {
                    "excerpt"
                }
            )));
            assert_eq!(transport.requests.len(), 3);
            assert!(!fixture.read("REVIEW.md")?.contains("Verdict: pass"));
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn late_human_result_mutation_during_actual_review_response_revokes_pass_and_is_preserved()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let root = fixture.root.clone();
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Delivered guide\n"),
            )]),
            summary(),
            review_packet(verdict(1)),
            review_packet(verdict(1)),
        ]);
        transport.action = Some(Box::new(move |index, _| {
            if index == 4 {
                fs::write(root.join("RESULT.md"), "Late human result evidence\n").map_err(io)?;
            }
            Ok(())
        }));
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
        assert_eq!(fixture.read("RESULT.md")?, "Late human result evidence\n");
        assert!(!fixture.read("REVIEW.md")?.contains("Verdict: pass"));
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

#[tokio::test(flavor = "current_thread")]
async fn existing_human_context_result_and_review_artifacts_are_never_clobbered()
-> Result<(), Error> {
    for name in ["CONTEXT.md", "RESULT.md", "REVIEW.md"] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["The requested guide is delivered."],
        )?;
        fixture.write(name, "Human-owned artifact\n")?;
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
                review_packet(verdict(1)),
                review_packet(verdict(1)),
            ]);
            let mut events = Events::default();
            let credentials = Credentials::default();
            let admission = Admission::default();
            let mut driver = driver(&mut transport, &mut events, &credentials, &admission);
            let coded = driver
                .run_coder(
                    &mut boundary,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &config(true, false, 10)?,
                    coder::Options::default(),
                )
                .await;
            match name {
                "CONTEXT.md" => {
                    assert_ne!(coded?.status(), finalization::Status::Pass);
                    assert!(transport.requests.is_empty());
                }
                "RESULT.md" => assert!(coded.is_err()),
                _ => {
                    let coder = coded?;
                    assert_eq!(coder.status(), finalization::Status::Pass);
                    assert!(
                        driver
                            .run_reviewer(
                                &boundary,
                                &coder,
                                Path::new(env!("CARGO_MANIFEST_DIR")),
                                &config(true, false, 10)?,
                                reviewer::Options::default()
                            )
                            .await
                            .is_err()
                    );
                }
            }
            assert_eq!(fixture.read(name)?, "Human-owned artifact\n");
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn private_receipt_rechecks_same_byte_artifact_identity_not_only_model_json_or_hash()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
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
            review_packet(verdict(1)),
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
        review
            .require_passing(&boundary, &coder, &evidence::Projection::default())
            .await?;
        files::create_private_directories(&fixture.root.join(".roster"))?;
        let result = fixture.read("RESULT.md")?;
        fs::rename(
            fixture.root.join("RESULT.md"),
            fixture.root.join(".roster").join("retained-result"),
        )
        .map_err(io)?;
        fixture.write("RESULT.md", &result)?;
        assert!(
            coder
                .require_current(&boundary, &evidence::Projection::default())
                .await
                .is_err()
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
