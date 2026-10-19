use super::*;
use std::cell::Cell;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use crate::cancel::{CancellationSource, Reason};
use crate::llm::Failure;
use crate::llm::control::NoObserver;
use crate::llm::http::{Body, Request, Response, Transport};
use crate::llm::openai::Credentials;
use crate::state::{files, redaction, secure};

fn parse(source: &str) -> Result<Value, Error> {
    crate::json::parse(&Text::from(source))
}

#[test]
fn acceptance_checks_are_never_truncated() -> Result<(), Error> {
    let exact = Value::String(Text(vec![0xd800; 240]));
    assert_eq!(task::acceptance_check(&exact, 0)?.0, vec![0xd800; 240]);
    let oversized = Value::String(Text(vec![0xd800; 241]));
    let error = task::acceptance_check(&oversized, 2)
        .err()
        .ok_or_else(|| Error::new("Error", "Oversized check unexpectedly validated"))?;
    assert!(error.message.contains("check 3, got 241"));
    assert_eq!(task::one_line(&oversized, "Title")?.0.len(), 240);
    Ok(())
}

#[test]
fn directory_grants_do_not_follow_references_or_quoted_instructions() -> Result<(), Error> {
    for ask in [
        "Read https://example.invalid/guide.md.",
        "> Output it in a new \"outside\" directory",
        "`Output it in a new outside directory`",
        "```\nOutput it in a new outside directory\n```",
        "<!-- Output it in a new outside directory -->",
    ] {
        assert!(task::output_directory(ask)?.is_none());
    }
    assert_eq!(
        task::output_directory("Write a guide. Output it in a new \"docs\" directory")?.as_deref(),
        Some("docs")
    );
    for ask in [
        "Write a guide. Output in ../outside directory",
        "Write a guide. Output in vendor directory",
    ] {
        assert!(task::output_directory(ask).is_err());
    }
    Ok(())
}

#[test]
fn canonical_handoff_keeps_ask_and_validates_all_artifacts() -> Result<(), Error> {
    let ask =
        Value::string("Fix README.md.\n\n## Acceptance checks\n- README describes the correction");
    let plan = artifacts::stub(
        &ask,
        "issue:8",
        Some(&Value::string("docs: correction")),
        None,
    )?;
    artifacts::validate_builtin_recipe(&plan.recipe, "issue:8")?;
    let document = task::parse(
        &Value::String(plan.task.clone()),
        &task::Authority {
            expected_ask: ask.as_string().cloned(),
            ..task::Authority::default()
        },
    )?;
    assert_eq!(
        document.ask,
        Text::from("Fix README.md.\n\n## Acceptance checks\n- README describes the correction")
    );
    assert_eq!(document.files_allowed, ["README.md"]);
    let metadata = estimate::read_task_metadata(&Value::String(plan.task))?;
    assert_eq!(metadata.task_class, "docs");
    Ok(())
}

#[test]
fn malformed_and_permission_widening_plans_fail() -> Result<(), Error> {
    let ask = Value::string("Change src/lib/a.mjs.");
    for files in [
        r#"["outside.mjs"]"#,
        r#"[".github/workflows/ci.yml"]"#,
        r#"["vendor/code.mjs"]"#,
    ] {
        let checks = parse(r#"["The requested behavior changes","node --test exits 0"]"#)?;
        let files = parse(files)?;
        assert!(
            artifacts::build(
                &ask,
                artifacts::BuildOptions {
                    reference: Some("issue:1"),
                    files: Some(&files),
                    checks: Some(&checks),
                    ..artifacts::BuildOptions::default()
                }
            )
            .is_err()
        );
    }
    let invalid = parse(
        r#"{"outcomes":["Outcome"],"issues":[{"title":"A","outcome":"First","wave":2,"acceptance_checks":["First passes"]},{"title":"B","outcome":"Next","wave":3,"acceptance_checks":["Next passes"]}]}"#,
    )?;
    assert!(outline::validate(&invalid, intake::Kind::Feature, &[], None, false).is_err());
    Ok(())
}
struct LocalFixture {
    root: PathBuf,
}

impl LocalFixture {
    fn new() -> Result<Self, Error> {
        let root = std::env::current_dir()
            .map_err(|error| Error::new("Error", error.to_string()))?
            .join(format!(".planning-native-tests-{}", files::nonce()?));
        files::create_private_directories(&root)?;
        Ok(Self { root })
    }

    fn write(&self, name: &str, source: &str) -> Result<(), Error> {
        let path = self.root.join(name);
        if let Some(parent) = path.parent() {
            files::create_private_directories(parent)?;
        }
        fs::write(&path, source)
            .map_err(|error| secure::io_error("Write planning test fixture", &path, error))
    }
}

impl Drop for LocalFixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!(
                "Planning fixture cleanup failed for {}: {error}",
                self.root.display()
            );
        }
    }
}

fn source(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path)
        .map_err(|error| secure::io_error("Read planning test fixture", path, error))
}

fn assignment(ask: &str) -> lifecycle::Assignment {
    lifecycle::Assignment {
        ask: Text::from(ask),
        title: None,
        reference: "issue:42".to_owned(),
        task: "issue-42".to_owned(),
        session: "roster-42-planner".to_owned(),
    }
}

fn config(configured: bool, budget: u64) -> Result<Value, Error> {
    let mut config =
        crate::config::parse(&Text::from(include_str!("../../roster.config.example.yml")))?
            .as_value()
            .clone();
    let mut llm = required(&config, "llm")?.clone();
    domain::put(
        &mut llm,
        "base_url",
        Value::string(if configured {
            "http://127.0.0.1:8000/v1"
        } else {
            ""
        }),
    )?;
    domain::put(
        &mut llm,
        "model",
        Value::string(if configured { "served-model" } else { "" }),
    )?;
    domain::put(&mut llm, "api_key_optional", Value::Boolean(true))?;
    domain::put(&mut config, "llm", llm)?;
    let mut planner = required(&config, "planner")?.clone();
    domain::put(&mut planner, "turn_budget", Value::Number(budget as f64))?;
    domain::put(&mut config, "planner", planner)?;
    Ok(config)
}

fn description() -> Value {
    Value::object(vec![
        ("title", Value::string("docs: bounded change")),
        (
            "acceptance_checks",
            strings(&["README contains the requested change".to_owned()]),
        ),
        ("files_allowed", strings(&["README.md".to_owned()])),
    ])
}

fn completion(description: &Value, model: &str, usage: Option<Value>) -> Value {
    let mut payload = Value::object(vec![
        ("model", Value::string(model)),
        (
            "choices",
            Value::Array(vec![Value::object(vec![
                ("finish_reason", Value::string("stop")),
                (
                    "message",
                    Value::object(vec![
                        ("role", Value::string("assistant")),
                        ("content", Value::string(&description.json())),
                    ]),
                ),
            ])]),
        ),
    ]);
    if let Some(usage) = usage {
        if let Value::Object(fields) = &mut payload {
            fields.push((Text::from("usage"), usage));
        }
    }
    payload
}

struct FixtureBody {
    bytes: Option<Vec<u8>>,
    delay: Duration,
    closed: Rc<Cell<usize>>,
}

impl Body for FixtureBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
            self.delay = Duration::ZERO;
        }
        Ok(self.bytes.take())
    }
}

impl Drop for FixtureBody {
    fn drop(&mut self) {
        self.closed.set(self.closed.get() + 1);
    }
}

struct FixtureTransport {
    script: VecDeque<Value>,
    requests: Vec<Value>,
    delay: Duration,
    closed: Rc<Cell<usize>>,
}

impl FixtureTransport {
    fn new(script: Vec<Value>) -> Self {
        Self {
            script: script.into(),
            requests: Vec::new(),
            delay: Duration::ZERO,
            closed: Rc::default(),
        }
    }
}

impl Transport for FixtureTransport {
    type Body = FixtureBody;

    async fn send(&mut self, request: &Request) -> Result<Response<Self::Body>, Failure> {
        self.requests.push(parse(
            &String::from_utf8(request.body.clone()).map_err(|_| Failure::network())?,
        )?);
        let payload = self.script.pop_front().ok_or_else(Failure::network)?;
        Ok(Response {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Some(FixtureBody {
                bytes: Some(payload.json().into_bytes()),
                delay: self.delay,
                closed: self.closed.clone(),
            }),
        })
    }
}

#[test]
fn owned_artifact_writer_refuses_preexisting_files_and_external_replacements() -> Result<(), Error>
{
    let fixture = LocalFixture::new()?;
    fixture.write("README.md", "Human product content\n")?;
    fixture.write("TASK.md", "Preexisting task\n")?;
    let mut writer =
        artifacts::Writer::new(&fixture.root, false, redaction::Config::default(), None)?;
    assert!(writer.write("README.md", &Text::from("Changed\n")).is_err());
    assert!(writer.write("TASK.md", &Text::from("Changed\n")).is_err());
    writer.write("ESTIMATE.md", &Text::from("Owned\n"))?;
    fixture.write("replacement", "External\n")?;
    fs::remove_file(fixture.root.join("ESTIMATE.md"))
        .map_err(|error| Error::new("Error", error.to_string()))?;
    fs::rename(
        fixture.root.join("replacement"),
        fixture.root.join("ESTIMATE.md"),
    )
    .map_err(|error| Error::new("Error", error.to_string()))?;
    assert!(writer.write("ESTIMATE.md", &Text::from("Next\n")).is_err());
    assert_eq!(source(&fixture.root.join("ESTIMATE.md"))?, "External\n");
    assert_eq!(
        source(&fixture.root.join("README.md"))?,
        "Human product content\n"
    );
    Ok(())
}

#[test]
fn owned_artifact_writer_refuses_same_identity_tampering_and_hard_links() -> Result<(), Error> {
    let fixture = LocalFixture::new()?;
    let mut writer =
        artifacts::Writer::new(&fixture.root, false, redaction::Config::default(), None)?;
    writer.write("TASK.md", &Text::from("Original\n"))?;
    fixture.write("TASK.md", "Tampered\n")?;
    assert!(writer.write("TASK.md", &Text::from("Next\n")).is_err());
    writer.write("RECIPE.yml", &Text::from("Private\n"))?;
    fs::hard_link(
        fixture.root.join("RECIPE.yml"),
        fixture.root.join("shared-link"),
    )
    .map_err(|error| Error::new("Error", error.to_string()))?;
    assert!(
        writer
            .write("RECIPE.yml", &Text::from("Changed\n"))
            .is_err()
    );
    assert_eq!(source(&fixture.root.join("shared-link"))?, "Private\n");
    Ok(())
}

#[test]
fn planner_writer_denies_credentials_and_revoked_writes() -> Result<(), Error> {
    let fixture = LocalFixture::new()?;
    let cancellation = CancellationSource::default();
    let redaction = redaction::Config {
        environment: Value::object(vec![(
            "CUSTOM_KEY",
            Value::string("test-only-private-value"),
        )]),
        api_key_env: Text::from("CUSTOM_KEY"),
        ..redaction::Config::default()
    };
    let mut writer =
        artifacts::Writer::new(&fixture.root, false, redaction, Some(cancellation.token()))?;
    assert!(
        writer
            .write("TASK.md", &Text::from("test-only-private-value"))
            .is_err()
    );
    assert!(
        writer
            .write(
                "TASK.md",
                &Text::from("-----BEGIN PRIVATE KEY-----\nprivate")
            )
            .is_err()
    );
    cancellation.cancel(Reason::Cancelled)?;
    let failed = writer
        .write("TASK.md", &Text::from("No secret"))
        .err()
        .ok_or_else(|| Error::new("Error", "Revoked write succeeded"))?;
    assert_eq!(failed.code, Some("ROSTER_CANCELLED"));
    assert!(!fixture.root.join("TASK.md").exists());
    Ok(())
}

#[test]
fn planner_read_boundary_cannot_grant_execution_or_read_state_and_secrets() -> Result<(), Error> {
    let fixture = LocalFixture::new()?;
    fixture.write("README.md", "First\nSecond\n")?;
    fixture.write(".env", "not-a-real-key\n")?;
    fixture.write(".roster/private.json", "state\n")?;
    fixture.write(
        "nested/vendor/code.mjs",
        "export const notForPlanner = true;\n",
    )?;
    let writer = artifacts::Writer::new(&fixture.root, true, redaction::Config::default(), None)?;
    let mut boundary = tools::Boundary {
        writer,
        reads: true,
    };
    for name in [
        "edit_file",
        "run_test",
        "run_command",
        "web_fetch",
        "publish",
        "merge",
        "deploy",
    ] {
        let error = boundary
            .execute(name, &Value::object(Vec::new()))
            .err()
            .ok_or_else(|| Error::new("Error", "Forbidden planner tool succeeded"))?;
        assert_eq!(error.code, Some("ROSTER_TOOL_DENIED"));
    }
    for path in [
        ".env",
        ".roster/private.json",
        ".github/workflows/ci.yml",
        "vendor/source.mjs",
        "nested/vendor/code.mjs",
        "../outside",
    ] {
        let denied = boundary
            .execute(
                "read_file",
                &Value::object(vec![("path", Value::string(path))]),
            )
            .err()
            .ok_or_else(|| Error::new("Error", "Protected planner read succeeded"))?;
        assert_eq!(denied.code, Some("ROSTER_TOOL_DENIED"));
    }
    assert_eq!(
        boundary.execute(
            "read_file",
            &Value::object(vec![
                ("path", Value::string("README.md")),
                ("offset", Value::Number(2.0)),
                ("max_lines", Value::Number(1.0))
            ])
        )?,
        Value::string("Second")
    );
    assert!(
        boundary
            .execute(
                "write_file",
                &Value::object(vec![
                    ("path", Value::string("TASK.md")),
                    ("content", Value::string("Forbidden"))
                ])
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn literal_planner_search_reports_the_real_truncation_boundary() -> Result<(), Error> {
    let fixture = LocalFixture::new()?;
    fixture.write("README.md", &vec!["needle"; 51].join("\n"))?;
    let writer = artifacts::Writer::new(&fixture.root, true, redaction::Config::default(), None)?;
    let mut boundary = tools::Boundary {
        writer,
        reads: true,
    };
    let result = boundary.execute(
        "search_text",
        &Value::object(vec![
            ("query", Value::string("needle")),
            ("path", Value::string("README.md")),
        ]),
    )?;
    assert_eq!(result.get("truncated"), Some(&Value::Boolean(true)));
    assert_eq!(
        result
            .get("matches")
            .and_then(domain::items)
            .map(|items| items.len()),
        Some(50)
    );
    assert!(
        boundary
            .execute(
                "search_text",
                &Value::object(vec![("query", Value::string(""))])
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn own_seat_memory_interoperates_without_a_second_store() -> Result<(), Error> {
    let fixture = LocalFixture::new()?;
    let path = memory::planner_path(&fixture.root, ".roster/memory/coder.jsonl")?;
    let redaction = redaction::Config::default();
    memory::append(
        &path,
        &fixture.root,
        &parse(r#"{"session":"planner-old","status":"stub","summary":"Prior planning only"}"#)?,
        &redaction,
    )?;
    let records = memory::read(&path, &fixture.root, &redaction)?;
    assert_eq!(records.len(), 1);
    assert_eq!(
        source(&path)?,
        "{\"session\":\"planner-old\",\"status\":\"stub\",\"summary\":\"Prior planning only\"}\n"
    );
    assert!(
        !fixture
            .root
            .join(".roster")
            .join("memory")
            .join("coder.jsonl")
            .exists()
    );
    assert!(
        memory::append(
            &path,
            &fixture.root,
            &parse(r#"{"body":"Source content is forbidden"}"#)?,
            &redaction
        )
        .is_err()
    );
    fixture.write(
        ".roster/memory/planner.jsonl",
        "{\"summary\":\"unfinished\"}",
    )?;
    assert!(memory::read(&path, &fixture.root, &redaction).is_err());
    assert!(
        memory::append(
            &path,
            &fixture.root,
            &parse(r#"{"status":"failed"}"#)?,
            &redaction
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn critic_schema_has_no_empty_or_out_of_range_success_default() -> Result<(), Error> {
    for source in [
        r#"{}"#,
        r#"{"defects":null}"#,
        r#"{"defects":[{"check":0,"problem":"Missing","fix":"Add"}]}"#,
        r#"{"defects":[{"check":2,"problem":"Missing","fix":"Add"}]}"#,
        r#"{"defects":[{"check":null,"problem":"","fix":"Add"}]}"#,
        r#"{"defects":[],"pass":true}"#,
    ] {
        assert!(critic::validate_report(&parse(source)?, 1).is_err());
    }
    assert!(critic::validate_report(&parse(r#"{"defects":[]}"#)?, 1)?.is_empty());
    Ok(())
}

#[test]
fn directory_intent_preserves_code_and_test_obligations() -> Result<(), Error> {
    let ask = "Create a utility with setup instructions. Include Node built-in tests. Output in output directory";
    assert!(task::requests_executable(ask)?);
    assert!(!task::directory_document(ask)?);
    assert!(
        task::validate_directory_executable(ask, &["output/README.md".to_owned()], None).is_err()
    );
    assert!(
        task::validate_directory_executable(ask, &["output/code.mjs".to_owned()], None).is_err()
    );
    task::validate_directory_executable(
        ask,
        &[
            "output/code.mjs".to_owned(),
            "output/code.test.mjs".to_owned(),
        ],
        Some("feat"),
    )?;
    Ok(())
}

#[test]
fn grounding_rejects_duplicate_owners_and_unknown_cited_names() -> Result<(), Error> {
    let index = grounding::Index {
        files: vec!["src/old.mjs".to_owned()],
        symbols: vec!["openStore".to_owned()],
        exports_by_file: vec![("src/old.mjs".to_owned(), vec!["openStore".to_owned()])],
    };
    let design = grounding::normalize_design(Some(&parse(
        r#"{"new_exports":[{"name":"openStore","file":"src/new.mjs"}]}"#,
    )?))?;
    let errors = grounding::errors(
        &["`missingSymbol` returns a result".to_owned()],
        design.as_ref(),
        &["src/new.mjs".to_owned()],
        "",
        Some(&index),
    )?;
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().any(|error| error.contains("already exists")));
    assert!(errors.iter().any(|error| error.contains("does not exist")));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn configured_draft_uses_only_artifact_tools_and_response_backed_metrics() -> Result<(), Error>
{
    let config = config(true, 2)?;
    let assignment = assignment("Change README.md.");
    let measured = parse(r#"{"prompt_tokens":0,"completion_tokens":7}"#)?;
    let mut transport = FixtureTransport::new(vec![completion(
        &description(),
        "actual-model",
        Some(measured),
    )]);
    let mut observer = NoObserver;
    let credentials = Credentials {
        environment: Some(parse(
            r#"{"AI_MODEL":"not-the-served-model","AI_CONTEXT_USED":"999"}"#,
        )?),
        vault_entries: None,
    };
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
    assert_eq!(draft.turns, 1);
    assert_eq!(draft.usage.and_then(|usage| usage.prompt_tokens), Some(0));
    let run = lifecycle::response_run(&config, draft.response.as_ref(), &assignment)?
        .ok_or_else(|| Error::new("Error", "Measured run missing"))?;
    assert_eq!(run.metrics().model, "actual-model");
    assert_eq!(run.metrics().usage.completion_tokens, Some(7));
    assert_eq!(run.version(), "-");
    assert_eq!(transport.requests.len(), 1);
    assert!(
        transport
            .requests
            .first()
            .and_then(|request| request.get("tools"))
            .is_none()
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn empty_endpoint_run_never_calls_transport_or_changes_product_code() -> Result<(), Error> {
    let fixture = LocalFixture::new()?;
    fixture.write("worktree/README.md", "Original\n")?;
    let worktree = fixture.root.join("worktree");
    let config = config(false, 2)?;
    let assignment = assignment("Change README.md.");
    let mut transport = FixtureTransport::new(Vec::new());
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
    let result = driver
        .run_planner(lifecycle::RunOptions {
            worktree: &worktree,
            repo_root: &fixture.root,
            assignment: &assignment,
            config: &config,
            metadata: None,
            locked_model: None,
            kind: None,
            plan_mode: false,
            repository_files: None,
            evaluations: None,
            learning: None,
        })
        .await
        .map_err(|failure| *failure.error)?;
    assert_eq!(result.get("run"), Some(&Value::Null));
    assert_eq!(result.get("turns"), Some(&Value::Number(0.0)));
    assert!(transport.requests.is_empty());
    assert_eq!(source(&worktree.join("README.md"))?, "Original\n");
    for file in ["TASK.md", "RECIPE.yml", "ESTIMATE.md"] {
        assert!(worktree.join(file).is_file());
    }
    assert!(!worktree.join("RESULT.md").exists());
    assert!(!worktree.join("REVIEW.md").exists());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_owns_and_drops_inflight_planning_response() -> Result<(), Error> {
    let config = config(true, 2)?;
    let assignment = assignment("Change README.md.");
    let source = CancellationSource::default();
    let token = source.token();
    let mut transport =
        FixtureTransport::new(vec![completion(&description(), "served-model", None)]);
    transport.delay = Duration::from_secs(3600);
    let closed = transport.closed.clone();
    let mut observer = NoObserver;
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
    let cancellation = async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        source.cancel(Reason::Cancelled)
    };
    let (result, cancelled) = tokio::join!(
        driver.draft(
            &assignment,
            &config,
            lifecycle::DraftOptions::default(),
            None
        ),
        cancellation
    );
    assert!(cancelled?);
    let error = result
        .err()
        .ok_or_else(|| Error::new("Error", "Cancelled planner returned success"))?;
    assert_eq!(error.code, Some("ROSTER_CANCELLED"));
    assert_eq!(closed.get(), 1);
    assert!(driver.last_response.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn configured_draft_rejects_locked_model_switch_and_scope_expansion() -> Result<(), Error> {
    let config = config(true, 1)?;
    let assignment = assignment("Change README.md.");
    for changed in [
        domain::merge(&description(), &parse(r#"{"model":"other-model"}"#)?)?,
        domain::merge(
            &description(),
            &parse(r#"{"files_allowed":["outside.md"]}"#)?,
        )?,
    ] {
        let mut transport = FixtureTransport::new(vec![completion(&changed, "served-model", None)]);
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
                .draft(
                    &assignment,
                    &config,
                    lifecycle::DraftOptions {
                        locked_model: Some("served-model"),
                        ..lifecycle::DraftOptions::default()
                    },
                    None
                )
                .await
                .is_err()
        );
        assert_eq!(transport.requests.len(), 1);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_response_has_one_real_validation_repair_without_dropping_checks()
-> Result<(), Error> {
    let config = config(true, 1)?;
    let assignment = assignment("Change README.md.");
    let oversized = domain::merge(
        &description(),
        &Value::object(vec![("notes", Value::string(&"x".repeat(17_000)))]),
    )?;
    let mut transport = FixtureTransport::new(vec![
        completion(&oversized, "served-model", None),
        completion(&description(), "served-model", None),
    ]);
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
    assert_eq!(draft.turns, 2);
    assert_eq!(
        task::parse(
            &Value::String(draft.artifacts.task),
            &task::Authority::default()
        )?
        .acceptance_checks,
        [Text::from("README contains the requested change")]
    );
    assert_eq!(transport.requests.len(), 2);
    let correction = transport
        .requests
        .get(1)
        .and_then(|request| request.get("messages"))
        .and_then(domain::items)
        .and_then(|messages| messages.last())
        .and_then(|message| message.get("content"))
        .and_then(Value::as_string)
        .ok_or_else(|| Error::new("Error", "Missing actual correction request"))?;
    assert!(encode(correction).contains("do not truncate requirements"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_tool_payloads_end_in_an_explicit_unverified_stub() -> Result<(), Error> {
    let config = config(true, 1)?;
    let assignment = assignment("Change README.md.");
    let invalid = parse(
        r#"{"model":"served-model","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"bad","type":"function","function":{"name":"run_test","arguments":"{}"}}]}}]}"#,
    )?;
    let mut transport = FixtureTransport::new(vec![invalid.clone(), invalid]);
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
    assert!(draft.error.is_some());
    assert!(!draft.requires_run);
    assert!(
        task::sections(&Value::String(draft.artifacts.task))?
            .section("planning failure")
            .is_some()
    );
    assert_eq!(transport.requests.len(), 2);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn pre_plan_hooks_and_unintegrated_history_fail_instead_of_becoming_defaults()
-> Result<(), Error> {
    for (path, text) in [
        ("worktree/.roster/hooks.yml", "hooks: []\n"),
        (".roster/evals.jsonl", "{\"session\":\"prior\"}\n"),
    ] {
        let fixture = LocalFixture::new()?;
        fixture.write("worktree/README.md", "Original\n")?;
        fixture.write(path, text)?;
        let config = config(false, 1)?;
        let assignment = assignment("Change README.md.");
        let worktree = fixture.root.join("worktree");
        let mut transport = FixtureTransport::new(Vec::new());
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
                .run_planner(lifecycle::RunOptions {
                    worktree: &worktree,
                    repo_root: &fixture.root,
                    assignment: &assignment,
                    config: &config,
                    metadata: None,
                    locked_model: None,
                    kind: None,
                    plan_mode: false,
                    repository_files: None,
                    evaluations: None,
                    learning: None,
                })
                .await
                .is_err()
        );
        assert!(transport.requests.is_empty());
        assert!(!worktree.join("TASK.md").exists());
    }
    Ok(())
}

#[test]
fn cached_handoff_checks_live_bytes_before_estimate_or_acceptance() -> Result<(), Error> {
    let fixture = LocalFixture::new()?;
    let assignment = assignment("Change README.md.");
    let plan = artifacts::stub(
        &Value::String(assignment.ask.clone()),
        &assignment.reference,
        None,
        None,
    )?;
    fixture.write("TASK.md", &plan.task.to_utf8()?)?;
    fixture.write("RECIPE.yml", &plan.recipe.to_utf8()?)?;
    let cached = lifecycle::cached_handoff(&fixture.root, &assignment, None)?
        .ok_or_else(|| Error::new("Error", "Canonical handoff was not reusable"))?;
    fixture.write(
        "TASK.md",
        &format!("{}\nExternal change", plan.task.to_utf8()?),
    )?;
    assert!(
        lifecycle::prepare_handoff(
            &fixture.root,
            &cached,
            "",
            &[],
            redaction::Config::default(),
            None
        )
        .is_err()
    );
    assert!(!fixture.root.join("ESTIMATE.md").exists());
    Ok(())
}

#[test]
fn critic_revision_cannot_widen_scope_or_change_ask() -> Result<(), Error> {
    let plan = artifacts::stub(&Value::string("Change README.md."), "issue:42", None, None)?;
    let changed_ask = Text::from(
        plan.task
            .to_utf8()?
            .replace("Change README.md.", "A different Ask.")
            .as_str(),
    );
    let changed_scope = Text::from(
        plan.task
            .to_utf8()?
            .replace("- `README.md`", "- `README.md`\n- `outside.md`")
            .as_str(),
    );
    let defect =
        parse(r#"{"check":null,"problem":"Missing evidence","fix":"Add regression evidence"}"#)?;
    for revised in [&changed_ask, &changed_scope] {
        assert!(!critic::revision_allowed(&plan.task, revised, &[])?);
        assert!(
            critic::revision_allowed(&plan.task, revised, std::slice::from_ref(&defect)).is_err()
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn native_critic_uses_distinct_profile_and_one_response_backed_revision() -> Result<(), Error>
{
    let fixture = LocalFixture::new()?;
    let mut config = config(true, 2)?;
    let mut planner = required(&config, "planner")?.clone();
    domain::put(&mut planner, "critic_profile", Value::string("critic"))?;
    domain::put(&mut config, "planner", planner)?;
    let fleet = crate::fleet::validate(&parse(
        r#"{"profiles":[{"id":"critic","base_url":"http://127.0.0.1:8001/v1","model":"critic-model","provider":"vllm","context_max":8192,"concurrency":1,"hardware":"fixture","notes":"Owned fake transport"}]}"#,
    )?)?;
    let assignment = assignment("Change README.md.");
    let source = artifacts::stub(
        &Value::String(assignment.ask.clone()),
        &assignment.reference,
        None,
        None,
    )?
    .task;
    let task = Text::from(
        source
            .to_utf8()?
            .replace(
                "- The requested behavior in the Ask is implemented",
                "- Existing test `already covered` exists",
            )
            .as_str(),
    );
    let context = grounding::Context {
        index: grounding::Index {
            files: vec!["README.md".to_owned()],
            ..grounding::Index::default()
        },
        test_names: vec!["already covered".to_owned()],
        ..grounding::Context::default()
    };
    let report = parse(
        r#"{"defects":[{"check":null,"problem":"Name the changed behavior","fix":"Require the requested status change"}]}"#,
    )?;
    let usage = parse(r#"{"prompt_tokens":4,"completion_tokens":0}"#)?;
    let mut transport = FixtureTransport::new(vec![
        completion(&report, "critic-model", Some(usage.clone())),
        completion(&description(), "served-model", Some(usage.clone())),
        completion(&parse(r#"{"defects":[]}"#)?, "critic-model", Some(usage)),
    ]);
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
    let result = driver
        .critique_plan(
            &task,
            lifecycle::CritiqueOptions {
                assignment: &assignment,
                config: &config,
                learning_root: &fixture.root,
                grounding: Some(&context),
                metadata: None,
                locked_model: None,
                fleet: Some(&fleet),
            },
        )
        .await?;
    assert!(result.revised);
    assert!(result.defects.is_empty());
    assert_eq!(result.runs.len(), 2);
    assert_eq!(result.turns, 1);
    assert!(result.revision_run.is_some());
    assert_eq!(
        driver
            .last_response
            .as_ref()
            .map(|response| response.model.clone()),
        Some(Text::from("served-model"))
    );
    assert_eq!(transport.requests.len(), 3);
    for position in [0, 2] {
        let request = transport
            .requests
            .get(position)
            .ok_or_else(|| Error::new("Error", "Missing critic request"))?;
        assert_eq!(request.get("model"), Some(&Value::string("critic-model")));
        assert_eq!(request.get("max_tokens"), Some(&Value::Number(600.0)));
        assert!(request.get("tools").is_none());
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn planning_provenance_reuses_the_existing_store_and_checks_actual_artifact_bytes()
-> Result<(), Error> {
    let fixture = LocalFixture::new()?;
    let assignment = assignment("Change README.md.");
    let config = config(false, 1)?;
    let plan = artifacts::stub(
        &Value::String(assignment.ask.clone()),
        &assignment.reference,
        None,
        None,
    )?;
    fixture.write("TASK.md", &plan.task.to_utf8()?)?;
    fixture.write("RECIPE.yml", &plan.recipe.to_utf8()?)?;
    let result = plan.as_value();
    let mut store = crate::state::provenance::api::ProvenanceStore::open(
        &fixture.root.join("existing-provenance"),
        crate::state::provenance::api::Identity::Explicit(format!("v1-{}", "a".repeat(64))),
    )?;
    let record = provenance::record_result(
        &mut store,
        provenance::Observation {
            worktree: &fixture.root,
            assignment: &assignment,
            config: &config,
            result: &result,
            response: None,
            opt_out: false,
        },
        None,
    )
    .await?;
    assert_eq!(record.get("durable"), Some(&Value::Boolean(true)));
    assert_eq!(record.get("outcome"), Some(&Value::string("stub")));
    assert_eq!(
        record
            .get("metrics")
            .and_then(|metrics| metrics.get("tokens_prompt")),
        Some(&Value::string("unknown"))
    );
    let records = store
        .query(crate::state::provenance::api::Query::default(), None)
        .await?;
    assert_eq!(records.len(), 1);
    assert_eq!(
        records
            .first()
            .and_then(|record| record.get("seat"))
            .and_then(|seat| seat.get("name")),
        Some(&Value::string("planner"))
    );
    fixture.write("TASK.md", "Externally replaced task\n")?;
    assert!(
        provenance::record_result(
            &mut store,
            provenance::Observation {
                worktree: &fixture.root,
                assignment: &assignment,
                config: &config,
                result: &result,
                response: None,
                opt_out: false,
            },
            None
        )
        .await
        .is_err()
    );
    assert_eq!(
        store
            .query(crate::state::provenance::api::Query::default(), None)
            .await?
            .len(),
        1
    );
    Ok(())
}
