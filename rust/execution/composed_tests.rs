use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::cancel::{CancellationSource, Reason};
use crate::llm::openai::Completion;
use crate::planning::{artifacts, lifecycle};
use crate::state::{files, locks, redaction};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn review_controls_exact_tracked_utf8_diff_byte_threshold() -> Result<(), Error> {
    let owned = Owned::new(&["docs/large.md"], "none")?;
    owned.git(&["config", "core.abbrev", "40"])?;
    owned.write("docs/large.md", "old\n")?;
    owned.git(&["add", "docs/large.md"])?;
    owned.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned UTF8 diff threshold fixture",
    ])?;
    owned.write("docs/large.md", "x\n")?;
    let measured = owned.git(&[
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--unified=0",
        "HEAD",
        "--",
        "docs/large.md",
    ])?;
    let payload_bytes = evidence::MAX_DIFF_BYTES
        .checked_sub(measured.len() - 2)
        .and_then(|bytes| bytes.checked_sub(1))
        .ok_or_else(|| Error::new("Error", "Owned diff threshold underflow"))?;
    let mut payload = "é".repeat(payload_bytes / 2);
    if payload_bytes % 2 != 0 {
        payload.push('x');
    }
    payload.push('\n');
    owned.write("docs/large.md", &payload)?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let exact = evidence::changes(&boundary, None).await?;
        assert_eq!(exact.diff().to_utf8()?.len(), evidence::MAX_DIFF_BYTES);
        assert!(exact.diff().0.len() < evidence::MAX_DIFF_BYTES);
        payload.insert(payload.len() - 1, 'x');
        owned.write("docs/large.md", &payload)?;
        assert!(evidence::changes(&boundary, None).await.is_err());
        assert_eq!(payload.pop(), Some('\n'));
        assert_eq!(payload.pop(), Some('x'));
        match payload.pop() {
            Some('é') => payload.push('x'),
            Some('x') => {}
            _ => {
                return Err(Error::new(
                    "Error",
                    "Owned UTF8 threshold lost its final character",
                ));
            }
        }
        payload.push('\n');
        owned.write("docs/large.md", &payload)?;
        let below = evidence::changes(&boundary, None).await?;
        assert_eq!(below.diff().to_utf8()?.len(), evidence::MAX_DIFF_BYTES - 1);
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn review_controls_combined_diff_counts_actual_tracked_and_untracked_bytes()
-> Result<(), Error> {
    let owned = Owned::new(&["docs/large.md", "docs/new.md"], "none")?;
    owned.write("docs/large.md", "old\n")?;
    owned.git(&["add", "docs/large.md"])?;
    owned.git(&["commit", "--quiet", "-m", "Owned cumulative diff fixture"])?;
    owned.write("docs/large.md", &format!("{}\n", "x".repeat(1024 * 1024)))?;
    owned.write("docs/new.md", &"y".repeat(1024 * 1024))?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let below = evidence::changes(&boundary, None).await?;
        assert_eq!(below.files().len(), 2);
        assert!(below.diff().to_utf8()?.len() < evidence::MAX_DIFF_BYTES);
        owned.write(
            "docs/large.md",
            &format!("{}\n", "x".repeat(2 * 1024 * 1024)),
        )?;
        owned.write("docs/new.md", &"y".repeat(2 * 1024 * 1024))?;
        assert!(evidence::changes(&boundary, None).await.is_err());
        owned.write("docs/large.md", "old\n")?;
        owned.write("docs/new.md", &"y".repeat(5 * 1024 * 1024))?;
        assert!(evidence::changes(&boundary, None).await.is_err());
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn review_controls_head_absence_does_not_swallow_git_errors_or_cancellation()
-> Result<(), Error> {
    let owned = Owned::new(&["src/app.mjs", "src/new.mjs"], "characterization")?;
    owned.write(
        "src/new.mjs",
        "export function stagedAnswer() { return 42; }\n",
    )?;
    owned.git(&["add", "src/new.mjs"])?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let changes = evidence::changes(&boundary, None).await?;
        assert!(changes.tracked().contains(&"src/new.mjs".to_owned()));
        assert!(
            changes
                .head_product(&boundary, "src/new.mjs")
                .await?
                .is_none()
        );
        assert!(
            changes
                .head_product(&boundary, "src/missing.mjs")
                .await?
                .is_none()
        );
        assert!(
            changes
                .head_product(&boundary, "vendor/private.mjs")
                .await
                .is_err()
        );
        assert!(
            changes
                .head_product(&boundary, "../outside.mjs")
                .await
                .is_err()
        );
        assert_eq!(
            changes.head_product(&boundary, "src/app.mjs").await?,
            Some(Text::from("export function answer() { return 42; }\n"))
        );
        let mut invalid = changes.clone();
        domain::put(
            &mut invalid.validation,
            "head",
            Value::string(&"0".repeat(40)),
        )?;
        match invalid.head_product(&boundary, "src/app.mjs").await {
            Err(error) => assert_eq!(error.code, Some("ROSTER_GIT_FAILED")),
            Ok(_) => {
                return Err(Error::new(
                    "Error",
                    "Invalid HEAD became absent product evidence",
                ));
            }
        }
        let blob = owned.git(&["rev-parse", "HEAD:src/app.mjs"])?;
        let blob = blob.trim();
        let object = owned
            .root
            .join(".git")
            .join("objects")
            .join(
                blob.get(..2)
                    .ok_or_else(|| Error::new("Error", "Owned blob ID missing prefix"))?,
            )
            .join(
                blob.get(2..)
                    .ok_or_else(|| Error::new("Error", "Owned blob ID missing suffix"))?,
            );
        #[cfg(windows)]
        {
            let permissions = fs::metadata(owned.root.join("src").join("app.mjs"))
                .map_err(io)?
                .permissions();
            fs::set_permissions(&object, permissions).map_err(io)?;
        }
        fs::remove_file(&object).map_err(io)?;
        match changes.head_product(&boundary, "src/app.mjs").await {
            Err(error) => assert_eq!(error.code, Some("ROSTER_GIT_FAILED")),
            Ok(_) => {
                return Err(Error::new(
                    "Error",
                    "Unreadable existing HEAD blob became absent evidence",
                ));
            }
        }
        source.cancel(Reason::Cancelled)?;
        match changes.head_product(&boundary, "src/new.mjs").await {
            Err(error) => assert!(crate::cancel::is_run_cancelled(&error)),
            Ok(_) => {
                return Err(Error::new(
                    "Error",
                    "Cancelled HEAD lookup became absent evidence",
                ));
            }
        }
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn review_controls_staged_new_acceptance_test_selects_deferred_counterfactual()
-> Result<(), Error> {
    let owned = Owned::new(&["src/app.mjs", "tests/new.test.mjs"], "required")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let projection = evidence::Projection::default();
        let mut attempt = attempt(&boundary, &config(true)?, finalization::Mode::Configured)?;
        attempt.context(&boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &projection).await?;
        execute(&mut attempt, &mut boundary, "read_file", path("src/app.mjs")).await?;
        execute(&mut attempt, &mut boundary, "edit_file", edit("src/app.mjs", "42", "6 * 7")).await?;
        execute(&mut attempt, &mut boundary, "write_file", Value::object(vec![("path", Value::string("tests/new.test.mjs")), ("content", Value::string("import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('new staged acceptance', () => assert.equal(answer(), 42));\n"))])).await?;
        owned.git(&["add", "tests/new.test.mjs"])?;
        let tested = execute(&mut attempt, &mut boundary, "run_test", Value::object(Vec::new())).await?;
        assert_eq!(tested.get("exit_code"), Some(&Value::Number(0.0)));
        execute(&mut attempt, &mut boundary, "update_checklist", parsed(r#"{"items":[{"id":1,"status":"done","evidence":"run_test passed the staged acceptance test"}]}"#)?).await?;
        attempt.response(&boundary, &summary())?;
        review(&mut attempt, &boundary).await?;
        let finalized = attempt.finish(&boundary, &projection).await?;
        assert_eq!(finalized.status(), finalization::Status::Unavailable);
        assert_eq!(finalized.report().get("pending"), Some(&strings(&["independent red/green execution".to_owned()])));
        assert!(!finalized.report().json().contains("could not read independent base evidence"));
        Ok(())
    }.await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn review_controls_deletions_preserve_explicit_counterfactual_exemptions() -> Result<(), Error>
{
    for (product, new_name, mode) in [
        (true, false, "required"),
        (true, true, "characterization"),
        (false, true, "required"),
    ] {
        let file = if product {
            "src/remove.mjs"
        } else {
            "docs/guide.md"
        };
        let owned = Owned::new(&[file, "tests/app.test.mjs"], mode)?;
        let task = fs::read_to_string(owned.root.join("TASK.md")).map_err(io)?;
        if !task.lines().any(|line| line.starts_with("tests:")) {
            owned.write(
                "TASK.md",
                &task.replacen("---\n", &format!("---\ntests: {mode}\n"), 1),
            )?;
        }
        if product {
            owned.write(file, "export function obsolete() { return 0; }\n")?;
            owned.git(&["add", file])?;
            owned.git(&[
                "commit",
                "--quiet",
                "-m",
                "Owned deletion applicability fixture",
            ])?;
        }
        let source = CancellationSource::default();
        let mut boundary = owned.open(&source).await?;
        let result = async {
            assert_eq!(policy::tests_mode(boundary.task())?, mode);
            let projection = evidence::Projection::default();
            let mut attempt = attempt(&boundary, &config(true)?, finalization::Mode::Configured)?;
            attempt.context(&boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &projection).await?;
            if new_name {
                let prior = fs::read_to_string(owned.root.join("tests").join("app.test.mjs")).map_err(io)?;
                execute(&mut attempt, &mut boundary, "read_file", path("tests/app.test.mjs")).await?;
                execute(&mut attempt, &mut boundary, "write_file", Value::object(vec![("path", Value::string("tests/app.test.mjs")), ("content", Value::string(&format!("{prior}\ntest('new retained answer', () => assert.equal(answer(), 42));\n")))])).await?;
            }
            execute(&mut attempt, &mut boundary, "read_file", path(file)).await?;
            execute(&mut attempt, &mut boundary, "delete_file", path(file)).await?;
            let tested = execute(&mut attempt, &mut boundary, "run_test", Value::object(Vec::new())).await?;
            assert_eq!(tested.get("exit_code"), Some(&Value::Number(0.0)));
            execute(&mut attempt, &mut boundary, "update_checklist", parsed(r#"{"items":[{"id":1,"status":"done","evidence":"run_test passed the scoped exemption control"}]}"#)?).await?;
            attempt.response(&boundary, &summary())?;
            review(&mut attempt, &boundary).await?;
            let finalized = attempt.finish(&boundary, &projection).await?;
            assert_eq!(finalized.status(), finalization::Status::Pass, "{}", finalized.report().json());
            assert_eq!(finalized.report().get("pending"), Some(&Value::Array(Vec::new())));
            Ok(())
        }.await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test]
async fn review_repro_deleted_product_cannot_skip_counterfactual_after_two_real_green_runs()
-> Result<(), Error> {
    let owned = Owned::new(&["src/remove.mjs", "tests/app.test.mjs"], "required")?;
    owned.write(
        "src/remove.mjs",
        "export function obsolete() { return 0; }\n",
    )?;
    owned.git(&["add", "src/remove.mjs"])?;
    owned.git(&["commit", "--quiet", "-m", "Owned removable product fixture"])?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let projection = evidence::Projection::default();
        let mut attempt = attempt(&boundary, &config(true)?, finalization::Mode::Configured)?;
        attempt.context(&boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &projection).await?;
        let previous = fs::read_to_string(owned.root.join("tests").join("app.test.mjs")).map_err(io)?;
        let candidate = format!("{previous}\ntest('new acceptance on retained product', () => assert.equal(answer(), 42));\n");
        execute(&mut attempt, &mut boundary, "read_file", path("tests/app.test.mjs")).await?;
        execute(&mut attempt, &mut boundary, "write_file", Value::object(vec![("path", Value::string("tests/app.test.mjs")), ("content", Value::string(&candidate))])).await?;
        let before = execute(&mut attempt, &mut boundary, "run_test", Value::object(Vec::new())).await?;
        assert_eq!(before.get("exit_code"), Some(&Value::Number(0.0)));
        execute(&mut attempt, &mut boundary, "read_file", path("src/remove.mjs")).await?;
        execute(&mut attempt, &mut boundary, "delete_file", path("src/remove.mjs")).await?;
        let after = execute(&mut attempt, &mut boundary, "run_test", Value::object(Vec::new())).await?;
        assert_eq!(after.get("exit_code"), Some(&Value::Number(0.0)));
        execute(&mut attempt, &mut boundary, "update_checklist", parsed(r#"{"items":[{"id":1,"status":"done","evidence":"run_test passed after removing the scoped obsolete module"}]}"#)?).await?;
        attempt.response(&boundary, &summary())?;
        review(&mut attempt, &boundary).await?;
        let finalized = attempt.finish(&boundary, &projection).await?;
        assert_eq!(finalized.status(), finalization::Status::Unavailable, "{}", finalized.report().json());
        assert_eq!(finalized.report().get("pending"), Some(&strings(&["independent red/green execution".to_owned()])));
        assert_ne!(finalized.capsule().and_then(|capsule| capsule.get("outcome")), Some(&Value::string("verified")));
        Ok(())
    }.await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn review_repro_staged_new_module_is_not_a_head_blob_or_an_aborted_finalization()
-> Result<(), Error> {
    let owned = Owned::new(&["src/app.mjs", "src/new.mjs"], "characterization")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let projection = evidence::Projection::default();
        let mut attempt = attempt(&boundary, &config(true)?, finalization::Mode::Configured)?;
        attempt.context(&boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &projection).await?;
        execute(&mut attempt, &mut boundary, "read_file", path("src/app.mjs")).await?;
        execute(&mut attempt, &mut boundary, "write_file", Value::object(vec![("path", Value::string("src/app.mjs")), ("content", Value::string("import { calculateAnswer } from './new.mjs';\nexport function answer() { return calculateAnswer(); }\n"))])).await?;
        execute(&mut attempt, &mut boundary, "write_file", Value::object(vec![("path", Value::string("src/new.mjs")), ("content", Value::string("export function calculateAnswer() { return 42; }\n"))])).await?;
        owned.git(&["add", "src/new.mjs"])?;
        let tested = execute(&mut attempt, &mut boundary, "run_test", Value::object(Vec::new())).await?;
        assert_eq!(tested.get("exit_code"), Some(&Value::Number(0.0)));
        execute(&mut attempt, &mut boundary, "update_checklist", parsed(r#"{"items":[{"id":1,"status":"done","evidence":"run_test passed through the real product caller"}]}"#)?).await?;
        attempt.response(&boundary, &summary())?;
        review(&mut attempt, &boundary).await?;
        let finalized = attempt.finish(&boundary, &projection).await?;
        assert_eq!(finalized.status(), finalization::Status::Pass, "{}", finalized.report().json());
        assert_eq!(finalized.result().get("shadow").and_then(|shadow| shadow.get("status")), Some(&Value::string("checked")));
        Ok(())
    }.await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn review_repro_tracked_only_five_mib_diff_is_not_accepted() -> Result<(), Error> {
    let owned = Owned::new(&["docs/large.md"], "none")?;
    owned.write("docs/large.md", "old\n")?;
    owned.git(&["add", "docs/large.md"])?;
    owned.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned bounded tracked-diff fixture",
    ])?;
    owned.write("docs/large.md", &"x".repeat(5 * 1024 * 1024))?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        match evidence::changes(&boundary, None).await {
            Ok(changes) => Err(Error::new(
                "Error",
                format!(
                    "Oversized tracked-only native diff accepted: {} bytes",
                    changes.diff().to_utf8()?.len()
                ),
            )),
            Err(error) => {
                assert_eq!(error.code, Some("ROSTER_TOOL_DENIED"));
                assert!(error.message.contains("4 MiB"));
                Ok(())
            }
        }
    }
    .await;
    locks::finish(result, boundary.close().await)
}

fn io(error: std::io::Error) -> Error {
    Error::new("Error", error.to_string())
}

fn parsed(source: &str) -> Result<Value, Error> {
    crate::json::parse(&Text::from(source))
}

struct Owned {
    root: PathBuf,
    assignment: lifecycle::Assignment,
}

impl Owned {
    fn new(paths: &[&str], mode: &str) -> Result<Self, Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".composed-native-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        files::create_private_directories(&root)?;
        let owned = Self {
            root,
            assignment: lifecycle::Assignment {
                ask: Text::from(format!("Refactor the scoped implementation without changing behavior.\n\n## Files allowed\n{}", paths.iter().map(|path| format!("- {path}")).collect::<Vec<_>>().join("\n")).as_str()),
                title: None,
                reference: "issue:42".to_owned(),
                task: "issue-42".to_owned(),
                session: "composed-native-test".to_owned(),
            },
        };
        owned.git(&["init", "--quiet"])?;
        owned.write(".gitignore", ".roster/\n.roster-state/\nTASK.md\nRECIPE.yml\nRESULT.md\nREVIEW.md\nCONTEXT.md\ntarget/\n.env\n")?;
        owned.write(
            "AGENTS.md",
            "# Owned repository\n\n## Coding\n\n- Preserve exact scope and observed evidence.\n",
        )?;
        owned.write("docs/guide.md", "# Guide\n\nBefore.\n")?;
        owned.write("src/app.mjs", "export function answer() { return 42; }\n")?;
        owned.write("tests/app.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('owned answer', () => assert.equal(answer(), 42));\n")?;
        let plan = artifacts::build(
            &Value::String(owned.assignment.ask.clone()),
            artifacts::BuildOptions {
                reference: Some("issue:42"),
                files: Some(&Value::Array(
                    paths.iter().map(|path| Value::string(path)).collect(),
                )),
                checks: Some(&Value::Array(vec![Value::string(
                    "The requested task behavior is verified.",
                )])),
                ..artifacts::BuildOptions::default()
            },
        )?;
        let task = plan
            .task
            .to_utf8()?
            .replace("tests: required", &format!("tests: {mode}"));
        owned.write("TASK.md", &task)?;
        owned.write("RECIPE.yml", &plan.recipe.to_utf8()?)?;
        owned.git(&["add", "."])?;
        owned.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned composed execution fixture",
        ])?;
        Ok(owned)
    }

    fn write(&self, file: &str, body: &str) -> Result<(), Error> {
        let target = self.root.join(file);
        files::create_private_directories(
            target
                .parent()
                .ok_or_else(|| Error::type_error("Owned file requires a parent"))?,
        )?;
        fs::write(&target, body).map_err(io)
    }

    fn git(&self, args: &[&str]) -> Result<String, Error> {
        let output = Command::new("git")
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
                "commit.gpgsign=false",
                "-c",
                "user.name=Owned Native Fixture",
                "-c",
                "user.email=owned@example.invalid",
                "-c",
            ])
            .arg(format!(
                "core.hooksPath={}",
                self.root.join("disabled-fixture-hooks").display()
            ))
            .args(args)
            .current_dir(&self.root)
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
            tools::Options::default(),
            tools::Host::default(),
            redaction::Config::default(),
            source.token(),
        )
        .await
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn config(evidence: bool) -> Result<crate::config::Config, Error> {
    let source = include_str!("../../roster.config.example.yml")
        .replacen("base_url: \"\"", "base_url: http://127.0.0.1:1/v1", 1)
        .replacen("model: \"\"", "model: configured-fixture", 1)
        .replace(
            "evidence_workspace: false",
            &format!("evidence_workspace: {evidence}"),
        );
    crate::config::parse(&Text::from(source.as_str()))
}

fn completion(message: Value, finish: &str) -> Completion {
    Completion {
        message,
        usage: None,
        model: Text::from("served-fixture"),
        finish_reason: Some(Value::string(finish)),
    }
}

fn summary() -> Completion {
    completion(
        Value::object(vec![
            ("role", Value::string("assistant")),
            ("content", Value::string("The scoped behavior is verified.")),
        ]),
        "stop",
    )
}

fn tool(name: &str, args: Value, id: &str) -> Completion {
    completion(
        Value::object(vec![
            ("role", Value::string("assistant")),
            (
                "tool_calls",
                Value::Array(vec![Value::object(vec![
                    ("id", Value::string(id)),
                    ("type", Value::string("function")),
                    (
                        "function",
                        Value::object(vec![
                            ("name", Value::string(name)),
                            ("arguments", Value::string(&args.json())),
                        ]),
                    ),
                ])]),
            ),
        ]),
        "tool_calls",
    )
}

fn path(file: &str) -> Value {
    Value::object(vec![("path", Value::string(file))])
}

fn edit(file: &str, old: &str, new: &str) -> Value {
    Value::object(vec![
        ("path", Value::string(file)),
        ("old_string", Value::string(old)),
        ("new_string", Value::string(new)),
    ])
}

fn attempt(
    boundary: &tools::Boundary,
    config: &crate::config::Config,
    mode: finalization::Mode,
) -> Result<finalization::Attempt, Error> {
    finalization::Attempt::begin(
        boundary,
        config,
        finalization::Inputs {
            mode,
            capsule_version: continuity::CapsuleVersion::NativeV2,
            capsule: None,
            prior_wave_files: &[],
            initial_baseline: None,
        },
        &evidence::Projection::default(),
    )
}

async fn execute(
    attempt: &mut finalization::Attempt,
    boundary: &mut tools::Boundary,
    name: &str,
    args: Value,
) -> Result<Value, Error> {
    let decoded = attempt.response(boundary, &tool(name, args, "reused-owned-id"))?;
    let call = decoded
        .calls
        .first()
        .ok_or_else(|| Error::new("Error", "Owned fixture expected a structured call"))?;
    attempt
        .execute(boundary, call, &evidence::Projection::default())
        .await
}

async fn review(
    attempt: &mut finalization::Attempt,
    boundary: &tools::Boundary,
) -> Result<(), Error> {
    let projection = evidence::Projection::default();
    let request = attempt.self_review_request(boundary, &projection).await?;
    assert!(request.request().get("tools").is_none());
    let report = request.accept(boundary, &completion(Value::object(vec![("role", Value::string("assistant")), ("content", Value::string(r#"{"checks":[{"id":1,"met":true,"evidence":"Owned current diff and actual checks were inspected."}],"findings":[]}"#))]), "stop"), &projection).await?;
    attempt.attach_self_review(report)
}

#[test]
fn coder_text_and_planner_shaped_json_never_execute() -> Result<(), Error> {
    let offered = vec!["write_file".to_owned()];
    let mut decoder = coder_calls::Decoder::default();
    let value = completion(
        Value::object(vec![(
            "content",
            Value::string(
                r#"{"tool_calls":[{"name":"write_file","arguments":{"path":"src/app.mjs","content":"unauthorized"}}]}"#,
            ),
        )]),
        "stop",
    );
    let decoded = decoder.decode(&value, &offered)?;
    assert!(decoded.calls.is_empty());
    assert!(decoded.correction.is_none());
    Ok(())
}

#[test]
fn coder_correction_cannot_finish_without_real_structured_tools() -> Result<(), Error> {
    let mut decoder = coder_calls::Decoder::default();
    let offered = vec!["read_file".to_owned()];
    let malformed = completion(
        parsed(
            r#"{"tool_calls":[{"type":"function","function":{"name":"read_file","arguments":{}}}]}"#,
        )?,
        "tool_calls",
    );
    assert!(decoder.decode(&malformed, &offered)?.correction.is_some());
    assert!(decoder.decode(&summary(), &offered).is_err());
    Ok(())
}

#[test]
fn coder_tool_finish_requires_tools_even_without_previous_correction() -> Result<(), Error> {
    let mut decoder = coder_calls::Decoder::default();
    let value = completion(
        Value::object(vec![("content", Value::string("No calls supplied"))]),
        "tool_calls",
    );
    assert!(
        decoder
            .decode(&value, &["read_file".to_owned()])?
            .correction
            .is_some()
    );
    assert!(
        decoder
            .decode(&summary(), &["read_file".to_owned()])
            .is_err()
    );
    Ok(())
}

#[test]
fn coder_reused_ids_are_reminted_without_generated_collisions() -> Result<(), Error> {
    let mut decoder = coder_calls::Decoder::default();
    let offered = vec!["read_file".to_owned()];
    let first = decoder.decode(
        &tool("read_file", path("src/app.mjs"), "coder-2-0"),
        &offered,
    )?;
    let second = decoder.decode(
        &tool("read_file", path("src/app.mjs"), "coder-2-0"),
        &offered,
    )?;
    assert_ne!(first.calls[0].id, second.calls[0].id);
    assert_eq!(second.calls[0].id, "coder-2-0-1");
    Ok(())
}

#[test]
fn coder_withheld_write_is_terminal_but_unknown_tool_gets_one_correction() -> Result<(), Error> {
    let offered = vec!["read_file".to_owned()];
    let mut withheld = coder_calls::Decoder::default();
    assert!(
        withheld
            .decode(&tool("write_file", path("src/app.mjs"), "one"), &offered)
            .is_err()
    );
    let mut unknown = coder_calls::Decoder::default();
    assert!(
        unknown
            .decode(
                &tool("imaginary", Value::object(Vec::new()), "one"),
                &offered
            )?
            .correction
            .is_some()
    );
    assert!(
        unknown
            .decode(
                &tool("imaginary", Value::object(Vec::new()), "two"),
                &offered
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn acceptance_non_ascii_order_is_explicit_and_cannot_drop_or_duplicate_source() -> Result<(), Error>
{
    let entries = std::collections::BTreeMap::from([
        ("é.mjs".to_owned(), "one".to_owned()),
        ("README.md".to_owned(), "two".to_owned()),
    ]);
    assert!(evidence::source_identity(&entries, &evidence::Projection::default()).is_err());
    let valid = evidence::Projection {
        order: Some(vec!["é.mjs".to_owned(), "README.md".to_owned()]),
        ..evidence::Projection::default()
    };
    assert_eq!(evidence::source_identity(&entries, &valid)?.len(), 64);
    let duplicate = evidence::Projection {
        order: Some(vec!["é.mjs".to_owned(), "é.mjs".to_owned()]),
        ..evidence::Projection::default()
    };
    assert!(evidence::source_identity(&entries, &duplicate).is_err());
    Ok(())
}

#[test]
fn substance_distinguishes_actual_app_calls_from_local_assertions_and_unused_sentinels()
-> Result<(), Error> {
    let source = Text::from(
        "import { answer } from '../src/app.mjs';\ntest('local', () => assert.equal(42, 42));",
    );
    assert!(!substance::analyze(&Text::from("tests/app.test.mjs"), &source, &source)?.is_empty());
    let real = Text::from(
        "import { answer } from '../src/app.mjs';\ntest('actual', () => assert.equal(answer(), 42));",
    );
    assert!(substance::analyze(&Text::from("tests/app.test.mjs"), &real, &real)?.is_empty());
    Ok(())
}

#[test]
fn native_counterfactual_names_are_explicit_rust_test_functions() -> Result<(), Error> {
    assert_eq!(
        substance::counterfactual_names(
            "rust/process.rs",
            &Text::from(
                "#[tokio::test(flavor = \"current_thread\")]\nasync fn owned_async_case() {}"
            )
        )?,
        vec![Text::from("owned_async_case")]
    );
    assert_eq!(
        substance::counterfactual_names(
            "src/lib.rs",
            &Text::from("#[test]\nfn owned_case() {}\nfn helper() {}")
        )?,
        vec![Text::from("owned_case")]
    );
    assert!(
        substance::counterfactual_names(
            "tests/app.test.mjs",
            &Text::from("test(`computed ${name}`, () => {});")
        )?
        .is_empty()
    );
    Ok(())
}

#[test]
fn snapshot_hash_compatibility_does_not_replace_private_file_identity() -> Result<(), Error> {
    let owned = Owned::new(&["src/app.mjs"], "characterization")?;
    let projection = evidence::Projection::default();
    let before = evidence::capture(&owned.root, None, &projection)?;
    let source = fs::read(owned.root.join("src/app.mjs")).map_err(io)?;
    fs::remove_file(owned.root.join("src/app.mjs")).map_err(io)?;
    fs::write(owned.root.join("src/app.mjs"), source).map_err(io)?;
    let after = evidence::capture(&owned.root, None, &projection)?;
    assert_eq!(before.source()?, after.source()?);
    assert!(before.changed(&after)?.is_empty());
    assert!(!before.same_source(&after));
    Ok(())
}

#[test]
fn public_rust_signatures_are_native_context_evidence_not_node_exports() -> Result<(), Error> {
    let owned = Owned::new(&["src/lib.rs"], "characterization")?;
    owned.write(
        "src/lib.rs",
        "pub fn answer() -> u8 {\n    42\n}\nfn private_helper() {}\npub struct Reply;\n",
    )?;
    let signatures =
        enrichment::module_exports(&owned.root, &["src/lib.rs".to_owned()], 2400)?.to_utf8()?;
    assert!(signatures.contains("pub fn answer() -> u8"));
    assert!(signatures.contains("pub struct Reply;"));
    assert!(!signatures.contains("private_helper"));
    Ok(())
}

#[tokio::test]
async fn configured_docs_attempt_produces_real_diff_run_and_exclusive_result() -> Result<(), Error>
{
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let mut attempt = attempt(&boundary, &config(false)?, finalization::Mode::Configured)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &evidence::Projection::default(),
            )
            .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "read_file",
            path("docs/guide.md"),
        )
        .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "edit_file",
            edit("docs/guide.md", "Before.", "After."),
        )
        .await?;
        attempt.response(&boundary, &summary())?;
        let finalized = attempt
            .finish(&boundary, &evidence::Projection::default())
            .await?;
        assert_eq!(finalized.status(), finalization::Status::Pass);
        assert!(finalized.result().get("tests").is_none());
        let run = parsed(
            &finalized
                .run()
                .ok_or_else(|| Error::new("Error", "Observed run missing"))?
                .json(),
        )?;
        assert_eq!(
            run.get("metrics").and_then(|metrics| metrics.get("model")),
            Some(&Value::string("served-fixture"))
        );
        let target = finalized
            .write(&boundary, &evidence::Projection::default())
            .await?;
        assert_eq!(
            fs::read_to_string(target).map_err(io)?,
            finalized.body().to_utf8()?
        );
        assert!(
            finalized
                .write(&boundary, &evidence::Projection::default())
                .await
                .is_err()
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn frozen_context_is_required_and_changed_root_instructions_revoke_responses()
-> Result<(), Error> {
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        assert!(
            finalization::Attempt::begin(
                &boundary,
                &config(false)?,
                finalization::Inputs {
                    mode: finalization::Mode::Configured,
                    capsule_version: continuity::CapsuleVersion::NativeV2,
                    capsule: None,
                    prior_wave_files: &["src/app.mjs".to_owned()],
                    initial_baseline: None
                },
                &evidence::Projection::default()
            )
            .is_err()
        );
        let mut attempt = attempt(&boundary, &config(false)?, finalization::Mode::Configured)?;
        assert!(attempt.response(&boundary, &summary()).is_err());
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &evidence::Projection::default(),
            )
            .await?;
        owned.write(
            "AGENTS.md",
            "# Owned repository\n\n## Coding\n\n- Changed current instructions.\n",
        )?;
        assert!(attempt.response(&boundary, &summary()).is_err());
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn stub_attempt_cannot_use_model_calls_or_host_verification() -> Result<(), Error> {
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let mut attempt = attempt(&boundary, &config(false)?, finalization::Mode::Stub)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &evidence::Projection::default(),
            )
            .await?;
        assert!(
            attempt
                .response(&boundary, &tool("write_file", path("docs/guide.md"), "one"))
                .is_err()
        );
        assert!(
            attempt
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await
                .is_err()
        );
        let finalized = attempt
            .finish(&boundary, &evidence::Projection::default())
            .await?;
        assert_eq!(finalized.status(), finalization::Status::Stub);
        assert!(finalized.run().is_none());
        assert_eq!(
            fs::read_to_string(owned.root.join("docs/guide.md")).map_err(io)?,
            "# Guide\n\nBefore.\n"
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn sealed_result_rejects_product_mutation_before_its_write() -> Result<(), Error> {
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let mut attempt = attempt(&boundary, &config(false)?, finalization::Mode::Configured)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &evidence::Projection::default(),
            )
            .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "read_file",
            path("docs/guide.md"),
        )
        .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "edit_file",
            edit("docs/guide.md", "Before.", "After."),
        )
        .await?;
        attempt.response(&boundary, &summary())?;
        let finalized = attempt
            .finish(&boundary, &evidence::Projection::default())
            .await?;
        owned.write(
            "docs/guide.md",
            "# Externally changed after sealed verdict\n",
        )?;
        assert!(
            finalized
                .write(&boundary, &evidence::Projection::default())
                .await
                .is_err()
        );
        assert!(!owned.root.join("RESULT.md").exists());
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn actual_node_observations_roundtrip_v2_and_same_byte_replacement_revokes_them()
-> Result<(), Error> {
    let owned = Owned::new(&["src/app.mjs"], "characterization")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let projection = evidence::Projection::default();
        let cfg = config(true)?;
        let mut workspace = continuity::Workspace::new(&boundary, &cfg, continuity::CapsuleVersion::NativeV2, None, &projection)?;
        workspace.execute(&mut boundary, "read_file", &path("src/app.mjs"), &projection).await?;
        workspace.execute(&mut boundary, "edit_file", &edit("src/app.mjs", "42", "6 * 7"), &projection).await?;
        workspace.execute(&mut boundary, "run_test", &Value::object(Vec::new()), &projection).await?;
        workspace.execute(&mut boundary, "update_checklist", &parsed(r#"{"items":[{"id":1,"status":"done","evidence":"run_test verified owned behavior"}]}"#)?, &projection).await?;
        let capsule = workspace.export(&boundary, "verified", &projection)?;
        let hydrated = continuity::Workspace::new(&boundary, &cfg, continuity::CapsuleVersion::NativeV2, Some(&capsule), &projection)?;
        assert!(hydrated.require_closed().is_ok());
        assert!(continuity::Workspace::new(&boundary, &cfg, continuity::CapsuleVersion::NodeV1, Some(&capsule), &projection).is_err());
        let mut forged = capsule.clone();
        let mut observed = required(&forged, "observations")?.clone();
        if let Value::Array(entries) = &mut observed {
            let item = entries.iter_mut().find(|item| domain::is_text(item.get("tool"), "run_test")).ok_or_else(|| Error::new("Error", "Observed test reference missing"))?;
            let mut verification = required(item, "verification")?.clone();
            domain::put(&mut verification, "required", strings(&["cargo".to_owned()]))?;
            domain::put(&mut verification, "observed", strings(&["cargo".to_owned()]))?;
            domain::put(&mut verification, "cargo_version", Value::string("cargo 1.85.0"))?;
            domain::put(&mut verification, "command_count", Value::Number(1.0))?;
            domain::put(item, "verification", verification)?;
            domain::put(item, "reference", Value::string("cargo checks"))?;
        }
        domain::put(&mut forged, "observations", observed)?;
        assert!(continuity::Workspace::new(&boundary, &cfg, continuity::CapsuleVersion::NativeV2, Some(&forged), &projection).is_err());
        let previous_source = workspace.source().map(str::to_owned);
        let bytes = fs::read(owned.root.join("src/app.mjs")).map_err(io)?;
        fs::remove_file(owned.root.join("src/app.mjs")).map_err(io)?;
        fs::write(owned.root.join("src/app.mjs"), bytes).map_err(io)?;
        workspace.refresh(&boundary, &projection)?;
        assert_eq!(workspace.source(), previous_source.as_deref());
        assert!(workspace.observations().is_empty());
        assert!(workspace.require_closed().is_err());
        assert!(workspace.export(&boundary, "verified", &projection).is_err());
        Ok(())
    }.await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn actual_pass_does_not_survive_a_changed_git_identity() -> Result<(), Error> {
    let owned = Owned::new(&["src/app.mjs"], "characterization")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let mut attempt = attempt(&boundary, &config(false)?, finalization::Mode::Configured)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &evidence::Projection::default(),
            )
            .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "read_file",
            path("src/app.mjs"),
        )
        .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "edit_file",
            edit("src/app.mjs", "42", "6 * 7"),
        )
        .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "run_test",
            Value::object(Vec::new()),
        )
        .await?;
        owned.git(&[
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "Owned external Git identity change",
        ])?;
        attempt.response(&boundary, &summary())?;
        review(&mut attempt, &boundary).await?;
        let finalized = attempt
            .finish(&boundary, &evidence::Projection::default())
            .await?;
        assert_eq!(finalized.status(), finalization::Status::Fail);
        assert!(finalized.report().json().contains("checkpoint identity"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn untracked_product_diff_reaches_tool_free_review_and_stale_responses_are_refused()
-> Result<(), Error> {
    let owned = Owned::new(&["src/new.mjs"], "characterization")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        boundary
            .execute(
                "write_file",
                &Value::object(vec![
                    ("path", Value::string("src/new.mjs")),
                    (
                        "content",
                        Value::string("export function newAnswer() { return 42; }\n"),
                    ),
                ]),
            )
            .await?;
        let changes = evidence::changes(&boundary, None).await?;
        assert!(
            changes
                .diff()
                .to_utf8()?
                .contains("+export function newAnswer")
        );
        let request = self_review::Request::prepare(
            &boundary,
            &changes,
            8000,
            &evidence::Projection::default(),
        )?;
        assert!(request.request().json().contains("newAnswer"));
        owned.write(
            "src/new.mjs",
            "export function newAnswer() { return 43; }\n",
        )?;
        assert!(
            request
                .accept(&boundary, &summary(), &evidence::Projection::default())
                .await
                .is_err()
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn cancelled_diagnostic_result_cannot_cross_worktree_or_writer_identity() -> Result<(), Error>
{
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let prepared = attempt(&boundary, &config(false)?, finalization::Mode::Configured)?;
    source.cancel(Reason::Cancelled)?;
    let finalized = prepared
        .finish(&boundary, &evidence::Projection::default())
        .await?;
    assert_eq!(finalized.status(), finalization::Status::Cancelled);
    boundary.close().await?;
    let fresh = CancellationSource::default();
    let mut reopened = owned.open(&fresh).await?;
    let result = async {
        assert!(
            finalized
                .write(&reopened, &evidence::Projection::default())
                .await
                .is_err()
        );
        assert!(!owned.root.join("RESULT.md").exists());
        let other = Owned::new(&["docs/guide.md"], "none")?;
        let other_source = CancellationSource::default();
        let mut other_boundary = other.open(&other_source).await?;
        let denied = finalized
            .write(&other_boundary, &evidence::Projection::default())
            .await;
        assert!(denied.is_err());
        locks::finish(Ok(()), other_boundary.close().await)
    }
    .await;
    locks::finish(result, reopened.close().await)
}

#[tokio::test]
async fn unused_js_source_projection_does_not_replace_or_disable_default_native_audit()
-> Result<(), Error> {
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    owned.write(".env", "OWNED_FIXTURE=not-a-credential\n")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let mut attempt = attempt(&boundary, &config(false)?, finalization::Mode::Configured)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &evidence::Projection::default(),
            )
            .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "read_file",
            path("docs/guide.md"),
        )
        .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "edit_file",
            edit("docs/guide.md", "Before.", "After."),
        )
        .await?;
        attempt.response(&boundary, &summary())?;
        let finalized = attempt
            .finish(&boundary, &evidence::Projection::default())
            .await?;
        assert_eq!(finalized.status(), finalization::Status::Pass);
        #[cfg(windows)]
        {
            assert_eq!(
                finalized.report().get("acceptanceProjectionUnavailable"),
                Some(&Value::Boolean(true))
            );
            assert_eq!(
                finalized.report().get("acceptanceSourceIdentity"),
                Some(&Value::Null)
            );
            assert!(attempt_source_required(&boundary).is_err());
        }
        #[cfg(unix)]
        assert_eq!(
            finalized.report().get("acceptanceProjectionUnavailable"),
            Some(&Value::Boolean(false))
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[cfg(windows)]
fn attempt_source_required(boundary: &tools::Boundary) -> Result<finalization::Attempt, Error> {
    attempt(boundary, &config(true)?, finalization::Mode::Configured)
}

#[tokio::test]
async fn frozen_turn_budget_cannot_keep_an_earlier_success_after_a_late_response()
-> Result<(), Error> {
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let mut value = config(false)?.as_value().clone();
        let mut seat = required(&value, "seat")?.clone();
        domain::put(&mut seat, "turn_budget", Value::Number(3.0))?;
        domain::put(&mut value, "seat", seat)?;
        let cfg = crate::config::parse(&crate::config::format(&value)?)?;
        let mut attempt = attempt(&boundary, &cfg, finalization::Mode::Configured)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &evidence::Projection::default(),
            )
            .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "read_file",
            path("docs/guide.md"),
        )
        .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "edit_file",
            edit("docs/guide.md", "Before.", "After."),
        )
        .await?;
        attempt.response(&boundary, &summary())?;
        assert!(attempt.response(&boundary, &summary()).is_err());
        let finalized = attempt
            .finish(&boundary, &evidence::Projection::default())
            .await?;
        assert_eq!(finalized.status(), finalization::Status::Fail);
        assert!(finalized.report().json().contains("response budget"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn forged_or_replayed_calls_never_inherit_a_pending_structured_read() -> Result<(), Error> {
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let mut attempt = attempt(&boundary, &config(false)?, finalization::Mode::Configured)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &evidence::Projection::default(),
            )
            .await?;
        let decoded =
            attempt.response(&boundary, &tool("read_file", path("docs/guide.md"), "one"))?;
        let original = decoded
            .calls
            .first()
            .ok_or_else(|| Error::new("Error", "Owned structured call missing"))?
            .clone();
        let mut forged = original.clone();
        forged.args = path(".env");
        assert!(
            attempt
                .execute(&mut boundary, &forged, &evidence::Projection::default())
                .await
                .is_err()
        );
        attempt
            .execute(&mut boundary, &original, &evidence::Projection::default())
            .await?;
        assert!(
            attempt
                .execute(&mut boundary, &original, &evidence::Projection::default())
                .await
                .is_err()
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn newly_proposed_native_scope_requires_full_guidance_and_a_new_response_before_mutation()
-> Result<(), Error> {
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let cfg = config(false)?;
    let mut boundary = tools::Boundary::open(
        &owned.root,
        owned.assignment.clone(),
        tools::Options::from_config(&cfg, Role::Coder)?,
        tools::Host::default(),
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let result = async {
        let projection = evidence::Projection::default();
        let original_task = boundary.task().clone();
        let mut attempt = attempt(&boundary, &cfg, finalization::Mode::Configured)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &projection,
            )
            .await?;
        let args = Value::object(vec![
            ("path", Value::string("rust/port.rs")),
            (
                "content",
                Value::string("pub fn port() -> u8 {\n    42\n}\n"),
            ),
        ]);
        let decoded = attempt.response(
            &boundary,
            &tool("write_file", args.clone(), "reused-native-id"),
        )?;
        let original = decoded
            .calls
            .first()
            .ok_or_else(|| Error::new("Error", "Owned native call missing"))?
            .clone();
        assert!(
            attempt
                .execute(&mut boundary, &original, &projection)
                .await
                .is_err()
        );
        assert!(!owned.root.join("rust").join("port.rs").exists());
        assert!(boundary.scope_files().is_empty());
        assert!(boundary.first_checkpoint().is_none());
        let refreshed = attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &projection,
            )
            .await?;
        assert!(refreshed.pack.to_utf8()?.contains("cargo.instructions.md"));
        assert!(
            refreshed
                .pack
                .to_utf8()?
                .contains("Previously decoded calls are revoked")
        );
        assert!(
            attempt
                .execute(&mut boundary, &original, &projection)
                .await
                .is_err()
        );
        let decoded = attempt.response(&boundary, &tool("write_file", args, "reused-native-id"))?;
        let fresh = decoded
            .calls
            .first()
            .ok_or_else(|| Error::new("Error", "Owned refreshed native call missing"))?;
        assert_ne!(fresh.id, original.id);
        attempt.execute(&mut boundary, fresh, &projection).await?;
        assert_eq!(boundary.scope_files(), ["rust/port.rs".to_owned()]);
        assert_eq!(boundary.task(), &original_task);
        assert!(owned.root.join("rust").join("port.rs").is_file());
        assert!(boundary.last_tests().is_none());
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test]
async fn a_new_structured_response_revokes_an_earlier_final_summary() -> Result<(), Error> {
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let projection = evidence::Projection::default();
        let mut attempt = attempt(&boundary, &config(false)?, finalization::Mode::Configured)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &projection,
            )
            .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "read_file",
            path("docs/guide.md"),
        )
        .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "edit_file",
            edit("docs/guide.md", "Before.", "After."),
        )
        .await?;
        attempt.response(&boundary, &summary())?;
        execute(
            &mut attempt,
            &mut boundary,
            "read_file",
            path("docs/guide.md"),
        )
        .await?;
        let finalized = attempt.finish(&boundary, &projection).await?;
        assert_eq!(finalized.status(), finalization::Status::Fail);
        assert!(
            finalized
                .report()
                .json()
                .contains("Coder result summary is missing")
        );
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn product_change_during_result_side_effect_removes_the_owned_stale_pass() -> Result<(), Error>
{
    let owned = Owned::new(&["docs/guide.md"], "none")?;
    let source = CancellationSource::default();
    let mut boundary = owned.open(&source).await?;
    let result = async {
        let projection = evidence::Projection::default();
        let mut attempt = attempt(&boundary, &config(false)?, finalization::Mode::Configured)?;
        attempt
            .context(
                &boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &projection,
            )
            .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "read_file",
            path("docs/guide.md"),
        )
        .await?;
        execute(
            &mut attempt,
            &mut boundary,
            "edit_file",
            edit("docs/guide.md", "Before.", "After."),
        )
        .await?;
        attempt.response(&boundary, &summary())?;
        let finalized = attempt.finish(&boundary, &projection).await?;
        assert_eq!(finalized.status(), finalization::Status::Pass);
        let root = owned.root.clone();
        let watch = tokio::spawn(async move {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                if root.join("RESULT.md").is_file() {
                    fs::write(
                        root.join("docs").join("guide.md"),
                        "# Owned external edit during RESULT creation\n",
                    )
                    .map_err(io)?;
                    return Ok(());
                }
                tokio::task::yield_now().await;
            }
            Err(Error::new(
                "Error",
                "Owned race watcher did not observe RESULT creation",
            ))
        });
        let written = finalized.write(&boundary, &projection).await;
        watch
            .await
            .map_err(|error| Error::new("Error", format!("Owned race task: {error}")))??;
        assert!(written.is_err());
        assert!(!owned.root.join("RESULT.md").exists());
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}
