#![cfg(windows)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use github_agent_roster::Error;
use github_agent_roster::cancel::CancellationSource;
use github_agent_roster::execution::{
    backend, classification, continuity, counterfactual, e2e, evidence, finalization, hooks, tools,
};
use github_agent_roster::json::Value;
use github_agent_roster::llm::control::NoObserver;
use github_agent_roster::llm::http::{Body, Request, Response, Transport};
use github_agent_roster::llm::openai::Completion;
use github_agent_roster::planning::{artifacts, lifecycle};
use github_agent_roster::state::{files, locks, redaction};
use github_agent_roster::text::Text;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct NeverBody;

impl Body for NeverBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        Err(Error::new(
            "Error",
            "Unconfigured owned planner cannot read a model response",
        ))
    }
}

struct NeverTransport(usize);

impl Transport for NeverTransport {
    type Body = NeverBody;
    async fn send(
        &mut self,
        _: &Request,
    ) -> Result<Response<Self::Body>, github_agent_roster::llm::Failure> {
        self.0 += 1;
        Err(github_agent_roster::llm::Failure::network())
    }
}

fn io(error: std::io::Error) -> Error {
    Error::new("Error", error.to_string())
}

struct Fixture {
    container: PathBuf,
    root: PathBuf,
    assignment: lifecycle::Assignment,
}

impl Fixture {
    fn new(allowed: &[&str], mode: &str, checks: &[&str], task: bool) -> Result<Self, Error> {
        let container = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".execution-backend-fixture-{}-{}",
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
                ask: Text::from(
                    format!(
                        "Implement the scoped behavior.\n\n## Files allowed\n{}",
                        allowed
                            .iter()
                            .map(|file| format!("- {file}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                    .as_str(),
                ),
                title: None,
                reference: "issue:42".to_owned(),
                task: "issue-42".to_owned(),
                session: "backend-integration".to_owned(),
            },
        };
        fixture.git(&["init", "--quiet"])?;
        fixture.write(
            ".gitignore",
            ".roster/\nTASK.md\nRECIPE.yml\nRESULT.md\nREVIEW.md\nCONTEXT.md\ntarget/\n",
        )?;
        fixture.write(
            "AGENTS.md",
            "# Owned project\n\n## Coding\n\n- Keep exact source and receipts current.\n",
        )?;
        fixture.write("src/app.mjs", "export function answer() { return 1; }\n")?;
        fixture.write("tests/app.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('retained behavior', () => assert.equal(answer(), 1));\n")?;
        fixture.write("docs/guide.md", "# Guide\n\nBefore.\n")?;
        fixture.git(&["add", "."])?;
        fixture.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned execution backend base fixture",
        ])?;
        if task {
            fixture.task(allowed, mode, checks)?;
        }
        Ok(fixture)
    }

    fn task(&self, allowed: &[&str], mode: &str, checks: &[&str]) -> Result<(), Error> {
        let plan = artifacts::build(
            &Value::String(self.assignment.ask.clone()),
            artifacts::BuildOptions {
                reference: Some("issue:42"),
                files: Some(&Value::Array(
                    allowed.iter().map(|file| Value::string(file)).collect(),
                )),
                checks: Some(&Value::Array(
                    checks.iter().map(|check| Value::string(check)).collect(),
                )),
                ..artifacts::BuildOptions::default()
            },
        )?;
        let source = plan.task.to_utf8()?;
        let task = if source.lines().any(|line| line.starts_with("tests:")) {
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
        self.write("TASK.md", &task)?;
        self.write("RECIPE.yml", &plan.recipe.to_utf8()?)
    }

    fn write(&self, file: &str, content: &str) -> Result<(), Error> {
        let path = self.root.join(file);
        files::create_private_directories(
            path.parent()
                .ok_or_else(|| Error::type_error("Fixture file needs a parent"))?,
        )?;
        fs::write(path, content).map_err(io)
    }

    fn git(&self, args: &[&str]) -> Result<String, Error> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .env_clear()
            .envs(tools::Host::default().environment)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "NUL")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "--no-pager",
                "-c",
                "init.templateDir=",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Owned Fixture",
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
            return Err(Error::new("Error", String::from_utf8_lossy(&output.stderr)));
        }
        String::from_utf8(output.stdout).map_err(|error| Error::new("Error", error.to_string()))
    }

    fn host(&self, cargo: bool) -> Result<tools::Host, Error> {
        let mut host = tools::Host {
            node20: Some(node()?),
            ..tools::Host::default()
        };
        if cargo {
            let program = std::env::var_os("CARGO")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("CARGO_HOME")
                        .map(|home| PathBuf::from(home).join("bin").join("cargo.exe"))
                })
                .ok_or_else(|| Error::new("Error", "Approved Cargo executable is missing"))?;
            let program = fs::canonicalize(program).map_err(io)?;
            let mut environment = std::env::vars()
                .filter(|(name, _)| {
                    ["CARGO_HOME", "RUSTUP_HOME", "RUSTFLAGS", "RUSTUP_TOOLCHAIN"]
                        .contains(&name.as_str())
                })
                .collect::<BTreeMap<_, _>>();
            if let Ok(toolchain) = std::env::var("ROSTER_TEST_CARGO_TOOLCHAIN") {
                environment.insert("RUSTUP_TOOLCHAIN".to_owned(), toolchain);
            }
            environment.insert(
                "CARGO_TARGET_DIR".to_owned(),
                self.container
                    .join("approved-target")
                    .to_string_lossy()
                    .into_owned(),
            );
            host.cargo = Some(tools::CargoApproval {
                executable: program,
                environment,
            });
        }
        Ok(host)
    }

    async fn open(
        &self,
        source: &CancellationSource,
        cargo: bool,
    ) -> Result<tools::Boundary, Error> {
        tools::Boundary::open(
            &self.root,
            self.assignment.clone(),
            tools::Options::default(),
            self.host(cargo)?,
            redaction::Config::default(),
            source.token(),
        )
        .await
    }

    fn hook(&self, event: &str, code: &str, timeout: u64) -> Result<(), Error> {
        self.write(".roster/hooks.yml", &format!("hooks:\n  - event: {event}\n    script: .roster/hooks/owned.mjs\n    timeout_ms: {timeout}\n"))?;
        self.write(".roster/hooks/owned.mjs", code)?;
        self.git(&[
            "add",
            "--force",
            ".roster/hooks.yml",
            ".roster/hooks/owned.mjs",
        ])?;
        self.git(&["commit", "--quiet", "-m", "Owned reviewed lifecycle source"])?;
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.container);
    }
}

fn node() -> Result<PathBuf, Error> {
    std::env::var_os("ROSTER_TEST_NODE")
        .map(PathBuf::from)
        .ok_or_else(|| {
            Error::new(
                "Error",
                "These owned consumer fixtures require explicit ROSTER_TEST_NODE",
            )
        })
}

fn config(evidence: bool) -> Result<github_agent_roster::config::Config, Error> {
    github_agent_roster::config::parse(&Text::from(
        include_str!("../roster.config.example.yml")
            .replacen("base_url: \"\"", "base_url: http://127.0.0.1:1/v1", 1)
            .replacen("model: \"\"", "model: configured-fixture", 1)
            .replace(
                "evidence_workspace: false",
                &format!("evidence_workspace: {evidence}"),
            )
            .as_str(),
    ))
}

fn approval() -> Result<backend::Approval, Error> {
    Ok(backend::Approval {
        counterfactual: true,
        classifications: true,
        hooks_node: Some(fs::canonicalize(node()?).map_err(io)?),
        node: Some(fs::canonicalize(node()?).map_err(io)?),
        native_cli: Some(fs::canonicalize(env!("CARGO_BIN_EXE_roster")).map_err(io)?),
        timeout_ms: Some(15_000),
        ..backend::Approval::default()
    })
}

fn completion(content: Option<&str>, name: Option<&str>, args: Value) -> Completion {
    let message = if let Some(name) = name {
        Value::object(vec![
            ("role", Value::string("assistant")),
            (
                "tool_calls",
                Value::Array(vec![Value::object(vec![
                    ("id", Value::string("reused-owned-id")),
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
        ])
    } else {
        Value::object(vec![
            ("role", Value::string("assistant")),
            (
                "content",
                Value::string(content.unwrap_or("1. done - Current behavior verified.")),
            ),
        ])
    };
    Completion {
        message,
        usage: None,
        model: Text::from("served-fixture"),
        finish_reason: Some(Value::string(if name.is_some() {
            "tool_calls"
        } else {
            "stop"
        })),
    }
}

async fn start(
    boundary: &tools::Boundary,
    observed: bool,
    approve: bool,
) -> Result<finalization::Attempt, Error> {
    let mut attempt = finalization::Attempt::begin(
        boundary,
        &config(observed)?,
        finalization::Inputs {
            mode: finalization::Mode::Configured,
            capsule_version: continuity::CapsuleVersion::NativeV2,
            capsule: None,
            prior_wave_files: &[],
            initial_baseline: None,
        },
        &evidence::Projection::default(),
    )?;
    if approve {
        attempt.approve_backends(boundary, approval()?)?;
    }
    attempt
        .context(
            boundary,
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &evidence::Projection::default(),
        )
        .await?;
    Ok(attempt)
}

async fn call(
    attempt: &mut finalization::Attempt,
    boundary: &mut tools::Boundary,
    name: &str,
    args: Value,
) -> Result<Value, Error> {
    let decoded = attempt.response(boundary, &completion(None, Some(name), args))?;
    let call = decoded
        .calls
        .first()
        .ok_or_else(|| Error::new("Error", "Owned response did not decode a call"))?;
    attempt
        .execute(boundary, call, &evidence::Projection::default())
        .await
}

fn path(file: &str) -> Value {
    Value::object(vec![("path", Value::string(file))])
}
fn write(file: &str, content: &str) -> Value {
    Value::object(vec![
        ("path", Value::string(file)),
        ("content", Value::string(content)),
    ])
}

async fn close_checks(
    attempt: &mut finalization::Attempt,
    boundary: &mut tools::Boundary,
) -> Result<(), Error> {
    call(
        attempt,
        boundary,
        "update_checklist",
        Value::object(vec![(
            "items",
            Value::Array(vec![Value::object(vec![
                ("id", Value::Number(1.0)),
                ("status", Value::string("done")),
                (
                    "evidence",
                    Value::string("run_test observed current scoped checks"),
                ),
            ])]),
        )]),
    )
    .await?;
    Ok(())
}

async fn review(
    attempt: &mut finalization::Attempt,
    boundary: &tools::Boundary,
) -> Result<(), Error> {
    let request = attempt
        .self_review_request(boundary, &evidence::Projection::default())
        .await?;
    let response = completion(
        Some(
            r#"{"checks":[{"id":1,"met":true,"evidence":"Owned actual checks and current diff inspected."}],"findings":[]}"#,
        ),
        None,
        Value::Null,
    );
    let report = request
        .accept(boundary, &response, &evidence::Projection::default())
        .await?;
    attempt.attach_self_review(report)
}

#[tokio::test(flavor = "current_thread")]
async fn actual_node_counterfactual_is_red_green_not_red_or_setup_unavailable() -> Result<(), Error>
{
    for variant in ["red", "not-red", "setup"] {
        let mut fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "required",
            &["Current scoped behavior is verified."],
            true,
        )?;
        if variant == "setup" {
            fixture.assignment.ask = Text::from(
                format!(
                    "Expose newAnswer as a public library API.\n\n{}",
                    fixture.assignment.ask.to_utf8()?
                )
                .as_str(),
            );
            fixture.task(
                &["src/app.mjs", "tests/app.test.mjs"],
                "required",
                &["Current scoped behavior is verified."],
            )?;
        }
        if variant == "not-red" {
            fixture.write("src/app.mjs", "export function answer() { return 2; }\n")?;
            fixture.write("tests/app.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('retained behavior', () => assert.equal(answer(), 2));\n")?;
            fixture.git(&["add", "src/app.mjs", "tests/app.test.mjs"])?;
            fixture.git(&[
                "commit",
                "--quiet",
                "-m",
                "Owned already-green base control",
            ])?;
        }
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let result = async {
            let mut attempt = start(&boundary, true, true).await?;
            call(&mut attempt, &mut boundary, "read_file", path("src/app.mjs")).await?;
            let app = if variant == "setup" {"export function newAnswer() { return 2; }\n"} else {"export function answer() { return 1 + 1; }\n"};
            call(&mut attempt, &mut boundary, "write_file", write("src/app.mjs", app)).await?;
            let import = if variant == "setup" {"newAnswer"} else {"answer"};
            let tests = format!("import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport {{ {import} }} from '../src/app.mjs';\ntest('retained behavior', () => assert.equal({import}(), 2));\ntest('new acceptance', () => assert.equal({import}(), 2));\n");
            call(&mut attempt, &mut boundary, "read_file", path("tests/app.test.mjs")).await?;
            call(&mut attempt, &mut boundary, "write_file", write("tests/app.test.mjs", &tests)).await?;
            let checks = call(&mut attempt, &mut boundary, "run_test", Value::object(Vec::new())).await?;
            assert_eq!(checks.get("exit_code"), Some(&Value::Number(0.0)));
            close_checks(&mut attempt, &mut boundary).await?;
            attempt.response(&boundary, &completion(None, None, Value::Null))?;
            review(&mut attempt, &boundary).await?;
            let finalized = attempt.finish(&boundary, &evidence::Projection::default()).await?;
            assert_eq!(finalized.status(), match variant {"red" => finalization::Status::Pass, "not-red" => finalization::Status::Fail, _ => finalization::Status::Unavailable}, "{}", finalized.report().json());
            let report = finalized.result().get("redGreen").ok_or_else(|| Error::new("Error", "Real counterfactual report missing"))?;
            assert_eq!(report.get("status"), Some(&Value::string(match variant {"red" => "checked", "not-red" => "fail", _ => "unavailable"})));
            if variant == "red" {
                assert!(finalized.result().get("hooks").is_some());
                assert!(finalized.result().get("endToEnd").is_some());
            }
            Ok(())
        }.await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn actual_rust_integration_counterfactual_never_overlays_candidate_production_or_calls_compile_error_red()
-> Result<(), Error> {
    for variant in ["red", "compile", "inline"] {
        let allowed = if variant == "inline" {
            vec!["src/lib.rs"]
        } else {
            vec!["src/lib.rs", "tests/proof.rs"]
        };
        let mut fixture = Fixture::new(
            &allowed,
            "required",
            &["Current Rust behavior is verified."],
            true,
        )?;
        if variant == "compile" {
            fixture.assignment.ask = Text::from(
                format!(
                    "Expose new_value as a public Rust library API.\n\n{}",
                    fixture.assignment.ask.to_utf8()?
                )
                .as_str(),
            );
            fixture.task(
                &allowed,
                "required",
                &["Current Rust behavior is verified."],
            )?;
        }
        fixture.write(
            "Cargo.toml",
            "[package]\nname = \"owned_backend\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        )?;
        fixture.write(
            "Cargo.lock",
            "version = 4\n\n[[package]]\nname = \"owned_backend\"\nversion = \"0.0.0\"\n",
        )?;
        fixture.write("src/lib.rs", "pub fn value() -> u8 {\n    1\n}\n")?;
        fixture.git(&["add", "Cargo.toml", "Cargo.lock", "src/lib.rs"])?;
        fixture.git(&["commit", "--quiet", "-m", "Owned native base product"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, true).await?;
        let result = async {
            let mut attempt = start(&boundary, true, true).await?;
            call(&mut attempt, &mut boundary, "read_file", path("src/lib.rs")).await?;
            let function = if variant == "compile" {"new_value"} else {"value"};
            let mut app = format!("pub fn {function}() -> u8 {{\n    2\n}}\n");
            if variant == "inline" {app.push_str("\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn new_acceptance() {\n        assert_eq!(super::value(), 2);\n    }\n}\n");}
            call(&mut attempt, &mut boundary, "write_file", write("src/lib.rs", &app)).await?;
            if variant != "inline" {
                call(&mut attempt, &mut boundary, "write_file", write("tests/proof.rs", &format!("#[test]\nfn new_acceptance() {{\n    assert_eq!(owned_backend::{function}(), 2);\n}}\n"))).await?;
            }
            let checks = call(&mut attempt, &mut boundary, "run_test", Value::object(Vec::new())).await?;
            assert_eq!(checks.get("exit_code"), Some(&Value::Number(0.0)));
            assert_eq!(checks.get("native"), Some(&Value::Boolean(true)));
            close_checks(&mut attempt, &mut boundary).await?;
            attempt.response(&boundary, &completion(None, None, Value::Null))?;
            review(&mut attempt, &boundary).await?;
            let finalized = attempt.finish(&boundary, &evidence::Projection::default()).await?;
            assert_eq!(finalized.status(), if variant == "red" {finalization::Status::Pass} else {finalization::Status::Unavailable}, "{}", finalized.report().json());
            assert_eq!(fs::read_to_string(fixture.root.join("src").join("lib.rs")).map_err(io)?, app);
            Ok(())
        }.await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn actual_native_read_only_acceptance_and_missing_commands_are_distinct() -> Result<(), Error>
{
    for (check, expected) in [
        (
            "`roster arguments split` prints JSON.",
            finalization::Status::Pass,
        ),
        (
            "`roster recipe validate RECIPE.yml` validates the current recipe.",
            finalization::Status::Pass,
        ),
        (
            "`roster history list` lists the current history.",
            finalization::Status::Unavailable,
        ),
        (
            "`roster recipe validate RECIPE.yml` prints JSON.",
            finalization::Status::Fail,
        ),
        (
            "`roster run` implements a task.",
            finalization::Status::Unavailable,
        ),
    ] {
        let fixture = Fixture::new(&["docs/guide.md"], "none", &[check], true)?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let result = async {
            let mut attempt = start(&boundary, false, true).await?;
            call(
                &mut attempt,
                &mut boundary,
                "read_file",
                path("docs/guide.md"),
            )
            .await?;
            call(
                &mut attempt,
                &mut boundary,
                "edit_file",
                Value::object(vec![
                    ("path", Value::string("docs/guide.md")),
                    ("old_string", Value::string("Before.")),
                    ("new_string", Value::string("After.")),
                ]),
            )
            .await?;
            attempt.response(&boundary, &completion(None, None, Value::Null))?;
            let finalized = attempt
                .finish(&boundary, &evidence::Projection::default())
                .await?;
            assert_eq!(
                finalized.status(),
                expected,
                "{}",
                finalized.report().json()
            );
            assert_eq!(
                finalized
                    .result()
                    .get("endToEnd")
                    .and_then(|report| report.get("status")),
                Some(&Value::string(match expected {
                    finalization::Status::Pass => "pass",
                    finalization::Status::Fail => "fail",
                    _ => "unavailable",
                }))
            );
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn committed_hooks_run_without_keys_and_mutations_fail_real_finalization() -> Result<(), Error>
{
    for variant in [
        "pass",
        "mutation",
        "timeout",
        "overflow",
        "exit",
        "uncommitted",
    ] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &["Current docs are updated."],
            true,
        )?;
        let code = match variant {
            "mutation" => "require('node:fs').writeFileSync('docs/guide.md','hook mutation\\n');\n",
            "timeout" => "setInterval(()=>{},1000);\n",
            "overflow" => "process.stdout.write('€'.repeat(5000));setInterval(()=>{},1000);\n",
            "exit" => "process.stdout.write('owned failure');process.exitCode=7;\n",
            _ => {
                "if(process.env.GITHUB_TOKEN||process.env.GITHUB_APP_ID||process.env.ROSTER_API_KEY||process.env.NODE_OPTIONS)process.exitCode=9;else process.stdout.write('trusted hook');\n"
            }
        };
        fixture.hook(
            "post-coder",
            code,
            if variant == "timeout" { 1000 } else { 10_000 },
        )?;
        if variant == "uncommitted" {
            fixture.write(
                ".roster/hooks/owned.mjs",
                "process.stdout.write('unreviewed');\n",
            )?;
        }
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let result = async {
            let mut attempt = start(&boundary, false, true).await?;
            call(
                &mut attempt,
                &mut boundary,
                "read_file",
                path("docs/guide.md"),
            )
            .await?;
            call(
                &mut attempt,
                &mut boundary,
                "edit_file",
                Value::object(vec![
                    ("path", Value::string("docs/guide.md")),
                    ("old_string", Value::string("Before.")),
                    ("new_string", Value::string("After.")),
                ]),
            )
            .await?;
            attempt.response(&boundary, &completion(None, None, Value::Null))?;
            let finalized = attempt
                .finish(&boundary, &evidence::Projection::default())
                .await?;
            assert_eq!(
                finalized.status(),
                if variant == "pass" {
                    finalization::Status::Pass
                } else {
                    finalization::Status::Fail
                },
                "{}",
                finalized.report().json()
            );
            if variant != "uncommitted" {
                let report = finalized
                    .result()
                    .get("hooks")
                    .ok_or_else(|| Error::new("Error", "Executed hook report missing"))?;
                assert_eq!(report.get("pass"), Some(&Value::Boolean(variant == "pass")));
            }
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn pre_plan_lease_runs_before_task_or_is_genuinely_absent_and_source_bound()
-> Result<(), Error> {
    for present in [false, true] {
        let fixture = Fixture::new(&["docs/guide.md"], "none", &["Docs are updated."], false)?;
        if present {
            fixture.hook("pre-plan", "process.stdout.write('before TASK');\n", 10_000)?;
        }
        let source = CancellationSource::default();
        let mut lease = backend::RootLease::open(
            &fixture.root,
            "owned-pre-plan",
            fixture.host(false)?,
            redaction::Config::default(),
            source.token(),
        )
        .await?;
        let result = async {
            let supplied = if present {
                approval()?
            } else {
                backend::Approval::default()
            };
            let report = hooks::run_pre_plan(&lease, &supplied).await?;
            report.require_current(&fixture.root).await?;
            assert!(!fixture.root.join("TASK.md").exists());
            assert_eq!(
                report.as_value().get("status"),
                Some(&Value::string(if present { "pass" } else { "none" }))
            );
            if present {
                fixture.write(
                    ".roster/hooks/owned.mjs",
                    "process.stdout.write('changed');\n",
                )?;
            } else {
                fixture.write(".roster/hooks.yml", "hooks:\n")?;
            }
            assert!(report.require_current(&fixture.root).await.is_err());
            Ok(())
        }
        .await;
        locks::finish(result, lease.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn observed_preexisting_transient_and_regression_classifications_preserve_original_full_exit()
-> Result<(), Error> {
    for variant in ["preexisting", "transient", "regression"] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Scoped behavior is verified."],
            true,
        )?;
        let marker = fixture.container.join("transient-observed");
        let outside = if variant == "transient" {
            format!(
                "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport fs from 'node:fs';\nconst marker = {};\ntest('outside transient', () => {{ const seen = fs.existsSync(marker); fs.writeFileSync(marker, 'observed'); assert.equal(seen, true); }});\n",
                Value::string(&marker.to_string_lossy()).json()
            )
        } else {
            format!(
                "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport {{ answer }} from '../src/app.mjs';\ntest('outside behavior', () => assert.equal(answer(), {}));\n",
                if variant == "preexisting" { 2 } else { 1 }
            )
        };
        fixture.write("tests/outside.test.mjs", &outside)?;
        fixture.git(&["add", "tests/outside.test.mjs"])?;
        fixture.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned full-suite failure classification base",
        ])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let result = async {
            let mut attempt = start(&boundary, true, true).await?;
            call(
                &mut attempt,
                &mut boundary,
                "read_file",
                path("src/app.mjs"),
            )
            .await?;
            call(
                &mut attempt,
                &mut boundary,
                "write_file",
                write(
                    "src/app.mjs",
                    if variant == "regression" {
                        "export function answer() { return 2; }\n"
                    } else {
                        "export function answer() { return 0 + 1; }\n"
                    },
                ),
            )
            .await?;
            if variant == "regression" {
                call(
                    &mut attempt,
                    &mut boundary,
                    "read_file",
                    path("tests/app.test.mjs"),
                )
                .await?;
                call(
                    &mut attempt,
                    &mut boundary,
                    "edit_file",
                    Value::object(vec![
                        ("path", Value::string("tests/app.test.mjs")),
                        ("old_string", Value::string("answer(), 1")),
                        ("new_string", Value::string("answer(), 2")),
                    ]),
                )
                .await?;
            }
            let verified = attempt
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?;
            assert_eq!(
                verified.get("exit_code"),
                Some(&Value::Number(if variant == "regression" {
                    1.0
                } else {
                    0.0
                }))
            );
            if variant != "regression" {
                close_checks(&mut attempt, &mut boundary).await?;
            }
            attempt.response(&boundary, &completion(None, None, Value::Null))?;
            review(&mut attempt, &boundary).await?;
            let finalized = attempt
                .finish(&boundary, &evidence::Projection::default())
                .await?;
            assert_eq!(
                finalized.status(),
                if variant == "regression" {
                    finalization::Status::Fail
                } else {
                    finalization::Status::Pass
                },
                "{}",
                finalized.report().json()
            );
            let observed = finalized.result().get("classification").ok_or_else(|| {
                Error::new("Error", "Actual full-suite classification was missing")
            })?;
            assert_eq!(observed.get("original_exit"), Some(&Value::Number(1.0)));
            assert_eq!(
                observed.get("clean_full_suite"),
                Some(&Value::Boolean(false))
            );
            let field = match variant {
                "preexisting" => "preexisting_files",
                "transient" => "transient_files",
                _ => "regression_files",
            };
            assert_eq!(
                observed.get(field),
                Some(&Value::Array(vec![Value::string("tests/outside.test.mjs")]))
            );
            if variant != "regression" {
                assert!(
                    finalized
                        .body()
                        .to_utf8()?
                        .contains("full-suite success is not claimed")
                );
                assert!(
                    finalized
                        .body()
                        .to_utf8()?
                        .contains("Original full-suite exit: 1")
                );
            }
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn actual_classification_receipt_and_cli_receipt_reject_later_source_changes()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["`roster arguments split` prints JSON."],
        true,
    )?;
    fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('existing failure', () => assert.equal(answer(), 2));\n")?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned receipt subject fixture"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let result = async {
        boundary.execute("read_file", &path("src/app.mjs")).await?;
        boundary
            .execute(
                "write_file",
                &write(
                    "src/app.mjs",
                    "export function answer() { return 0 + 1; }\n",
                ),
            )
            .await?;
        assert_ne!(
            boundary.verify(true).await?.get("exit_code"),
            Some(&Value::Number(0.0))
        );
        let observed = classification::run(&boundary, &approval()?).await?;
        assert!(observed.all_outside_proven());
        observed.require_current(&boundary).await?;
        let cli = e2e::run(&boundary, &approval()?).await?;
        assert!(cli.passed());
        cli.require_current(&boundary).await?;
        fixture.write(
            "src/app.mjs",
            "export function answer() { return 1 + 0; }\n",
        )?;
        assert!(observed.require_current(&boundary).await.is_err());
        assert!(cli.require_current(&boundary).await.is_err());
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn real_local_submodule_base_requires_exact_approval_and_never_fetches_a_url()
-> Result<(), Error> {
    for case in [
        "unapproved",
        "approved",
        "nested-missing",
        "nested-approved",
        "stale-source",
    ] {
        let approved = case != "unapproved";
        let nested = case.starts_with("nested");
        let child = if nested {
            Some(Fixture::new(
                &["src/app.mjs"],
                "characterization",
                &["Nested behavior exists."],
                false,
            )?)
        } else {
            None
        };
        let module = Fixture::new(
            &["src/app.mjs"],
            "characterization",
            &["Module behavior exists."],
            false,
        )?;
        let original = if let Some(child) = &child {
            files::create_private_directories(&module.root.join("vendor"))?;
            module.git(&[
                "clone",
                "--quiet",
                "--shared",
                "--",
                child
                    .root
                    .to_str()
                    .ok_or_else(|| Error::type_error("Owned child source path is not Unicode"))?,
                "vendor/child",
            ])?;
            module.write(
                ".gitmodules",
                "[submodule \"child\"]\n\tpath = vendor/child\n\turl = disabled-native-child-url\n",
            )?;
            let original = "import { answer as child } from '../vendor/child/src/app.mjs';\nexport function answer() { return child(); }\n";
            module.write("src/app.mjs", original)?;
            module.git(&["add", ".gitmodules", "vendor/child", "src/app.mjs"])?;
            module.git(&[
                "commit",
                "--quiet",
                "-m",
                "Owned nested submodule base fixture",
            ])?;
            original
        } else {
            "export function answer() { return 1; }\n"
        };
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "required",
            &["Current behavior is verified."],
            true,
        )?;
        files::create_private_directories(&fixture.root.join("vendor"))?;
        fixture.git(&[
            "clone",
            "--quiet",
            "--shared",
            "--",
            module
                .root
                .to_str()
                .ok_or_else(|| Error::type_error("Owned module path is not Unicode"))?,
            "vendor/local",
        ])?;
        if let Some(child) = &child {
            fixture.git(&[
                "clone",
                "--quiet",
                "--shared",
                "--",
                child
                    .root
                    .to_str()
                    .ok_or_else(|| Error::type_error("Owned child path is not Unicode"))?,
                "vendor/local/vendor/child",
            ])?;
        }
        fixture.write(
            ".gitmodules",
            "[submodule \"local\"]\n\tpath = vendor/local\n\turl = disabled-native-test-url\n",
        )?;
        fixture.write("src/app.mjs", "import { answer as local } from '../vendor/local/src/app.mjs';\nexport function answer() { return local(); }\n")?;
        fixture.git(&["add", ".gitmodules", "vendor/local", "src/app.mjs"])?;
        fixture.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned local submodule base revision",
        ])?;
        if case == "stale-source" {
            module.write("docs/guide.md", "# Source moved\n")?;
            module.git(&["add", "docs/guide.md"])?;
            module.git(&[
                "commit",
                "--quiet",
                "-m",
                "Owned approved source HEAD mismatch control",
            ])?;
        }
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let result = async {
            boundary.execute("read_file", &path("src/app.mjs")).await?;
            boundary.execute("write_file", &write("src/app.mjs", "import { answer as local } from '../vendor/local/src/app.mjs';\nexport function answer() { return local() + 1; }\n")).await?;
            boundary.execute("read_file", &path("tests/app.test.mjs")).await?;
            boundary.execute("write_file", &write("tests/app.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('new local module behavior', () => assert.equal(answer(), 2));\n")).await?;
            let tested = boundary.verify(false).await?;
            assert_eq!(tested.get("exit_code"), Some(&Value::Number(0.0)));
            let changed = evidence::changes(&boundary, None).await?;
            let mut supplied = approval()?;
            if approved {supplied.submodules.push(backend::LocalSubmodule {path: "vendor/local".to_owned(), source: module.root.clone()});}
            if case == "nested-approved" {
                supplied.submodules.push(backend::LocalSubmodule {path: "vendor/local/vendor/child".to_owned(), source: child.as_ref().ok_or_else(|| Error::type_error("Nested source control is missing"))?.root.clone()});
            }
            let outcome = counterfactual::run(&boundary, &supplied, &changed, changed.files()).await;
            if case == "approved" || case == "nested-approved" {
                let observed = outcome?;
                assert!(observed.passed(), "{}", observed.as_value().json());
                observed.require_current(&boundary).await?;
            } else {assert!(outcome.is_err());}
            assert_eq!(fs::read_to_string(module.root.join("src").join("app.mjs")).map_err(io)?, original);
            Ok(())
        }.await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[test]
fn read_only_selection_maps_legacy_spelling_to_native_intent_without_a_source_fallback()
-> Result<(), Error> {
    let selected = e2e::selected_commands(&[
        Text::from("`node src/cli.mjs recipe validate RECIPE.yml` validates the recipe."),
        Text::from("`roster status` reports offline state."),
    ])?;
    let rendered = selected.json();
    assert!(rendered.contains("roster recipe validate RECIPE.yml"));
    assert!(rendered.contains("--offline"));
    assert!(rendered.contains("\"implemented\":false"));
    assert!(!rendered.contains("src/cli.mjs"));
    assert!(e2e::selected_commands(&[Text::from("`roster publish` publishes.")]).is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn actual_cargo_baseline_failures_are_observed_but_setup_errors_never_become_preexisting()
-> Result<(), Error> {
    for compile in [false, true] {
        let fixture = Fixture::new(
            &["src/lib.rs"],
            "characterization",
            &["Native behavior is verified."],
            true,
        )?;
        fixture.write(
            "Cargo.toml",
            "[package]\nname = \"owned_backend\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        )?;
        fixture.write(
            "Cargo.lock",
            "version = 4\n\n[[package]]\nname = \"owned_backend\"\nversion = \"0.0.0\"\n",
        )?;
        fixture.write("src/lib.rs", "pub fn value() -> u8 {\n    1\n}\n")?;
        fixture.write("tests/outside.rs", &format!("#[test]\nfn outside_known_failure() {{\n    assert_eq!(owned_backend::{}(), 2);\n}}\n", if compile {"new_value"} else {"value"}))?;
        fixture.git(&[
            "add",
            "Cargo.toml",
            "Cargo.lock",
            "src/lib.rs",
            "tests/outside.rs",
        ])?;
        fixture.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned Cargo baseline observation fixture",
        ])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, true).await?;
        let result = async {
            boundary.execute("read_file", &path("src/lib.rs")).await?;
            boundary
                .execute(
                    "write_file",
                    &write(
                        "src/lib.rs",
                        if compile {
                            "pub fn new_value() -> u8 {\n    1\n}\n"
                        } else {
                            "pub fn value() -> u8 {\n    u8::from(true)\n}\n"
                        },
                    ),
                )
                .await?;
            let failed = boundary.verify(false).await?;
            assert_ne!(failed.get("exit_code"), Some(&Value::Number(0.0)));
            assert_eq!(failed.get("native"), Some(&Value::Boolean(true)));
            let observed = classification::run(&boundary, &approval()?).await?;
            assert!(!observed.all_outside_proven());
            observed.require_current(&boundary).await?;
            if compile {
                assert_eq!(
                    observed.as_value().get("status"),
                    Some(&Value::string("unavailable"))
                );
                assert_eq!(
                    observed.as_value().get("preexisting_files"),
                    Some(&Value::Array(Vec::new()))
                );
            } else {
                assert_eq!(
                    observed.as_value().get("preexisting_files"),
                    Some(&Value::Array(vec![Value::string("tests/outside.rs")]))
                );
                assert_eq!(
                    observed.as_value().get("clean_full_suite"),
                    Some(&Value::Boolean(false))
                );
            }
            Ok(())
        }
        .await;
        locks::finish(result, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn canonical_image_normalization_cannot_substitute_a_different_real_file() -> Result<(), Error>
{
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["`roster arguments split` prints JSON."],
        true,
    )?;
    let ordinary = fixture.container.join("approved.exe");
    fs::copy(env!("CARGO_BIN_EXE_roster"), &ordinary).map_err(io)?;
    let mut selected = fs::canonicalize(&ordinary).map_err(io)?.into_os_string();
    selected.push(".");
    let selected = PathBuf::from(selected);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&selected)
        .map_err(io)?;
    assert_ne!(
        github_agent_roster::paths::file_information(&selected)?.identity,
        github_agent_roster::paths::file_information(&ordinary)?.identity
    );
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let mut supplied = approval()?;
        supplied.native_cli = Some(selected.clone());
        let report = e2e::run(&boundary, &supplied).await;
        let error = report.err().ok_or_else(|| {
            Error::new(
                "Error",
                "A different native image was executed through a normalized alias",
            )
        })?;
        assert_eq!(error.code, Some("ROSTER_TOOL_DENIED"));
        assert!(error.message.contains("different physical images"));
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn pre_plan_receipt_enters_the_existing_native_planner_only_with_its_exact_lease_token()
-> Result<(), Error> {
    let fixture = Fixture::new(&["docs/guide.md"], "none", &["Docs are updated."], false)?;
    fixture.hook(
        "pre-plan",
        "process.stdout.write('approved before TASK');\n",
        10_000,
    )?;
    let source = CancellationSource::default();
    let mut lease = backend::RootLease::open(
        &fixture.root,
        "owned-planner-entry",
        fixture.host(false)?,
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let outcome = async {
        let receipt = hooks::run_pre_plan(&lease, &approval()?).await?;
        assert!(!fixture.root.join("TASK.md").exists());
        let unconfigured = github_agent_roster::config::parse(&Text::from(include_str!(
            "../roster.config.example.yml"
        )))?;
        let options = lifecycle::RunOptions {
            worktree: &fixture.root,
            repo_root: &fixture.root,
            assignment: &fixture.assignment,
            config: unconfigured.as_value(),
            metadata: None,
            locked_model: None,
            kind: Some(github_agent_roster::planning::intake::Kind::Slice),
            plan_mode: false,
            repository_files: Some(&[]),
            evaluations: None,
            learning: None,
        };
        let mut transport = NeverTransport(0);
        let mut observer = NoObserver;
        let credentials = github_agent_roster::llm::openai::Credentials::default();
        let other_token = source.token();
        let mut driver = lifecycle::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &github_agent_roster::llm::admission::Admission::default(),
            signal: Some(&other_token),
            redaction: redaction::Config::default(),
            last_response: None,
            critic_evidence: Vec::new(),
        };
        assert!(
            driver
                .run_planner_owned_after_pre_plan(&options, &receipt)
                .await
                .is_err()
        );
        assert!(!fixture.root.join("TASK.md").exists());
        driver.signal = Some(lease.token());
        let planned = driver
            .run_planner_owned_after_pre_plan(&options, &receipt)
            .await
            .map_err(|failure| *failure.error)?;
        assert_eq!(planned.get("run"), Some(&Value::Null));
        assert!(fixture.root.join("TASK.md").is_file());
        assert!(fixture.root.join("RECIPE.yml").is_file());
        assert_eq!(transport.0, 0);
        assert_eq!(
            fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
            "# Guide\n\nBefore.\n"
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, lease.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn retired_roster_package_is_not_materialized_as_a_legacy_baseline() -> Result<(), Error> {
    for retired in [false, true] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "required",
            &["Consumer behavior is verified."],
            true,
        )?;
        fixture.write(
            "package.json",
            &Value::object(vec![
                (
                    "name",
                    Value::string(if retired {
                        "github-agent-roster"
                    } else {
                        "owned-consumer-fixture"
                    }),
                ),
                ("type", Value::string("module")),
            ])
            .json(),
        )?;
        fixture.git(&["add", "package.json"])?;
        fixture.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned package-name materialization control",
        ])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            boundary.execute("read_file", &path("src/app.mjs")).await?;
            boundary.execute("write_file", &write("src/app.mjs", "export function answer() { return 2; }\n")).await?;
            boundary.execute("read_file", &path("tests/app.test.mjs")).await?;
            boundary.execute("write_file", &write("tests/app.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('new consumer behavior', () => assert.equal(answer(), 2));\n")).await?;
            assert_eq!(boundary.verify(false).await?.get("exit_code"), Some(&Value::Number(0.0)));
            let changes = evidence::changes(&boundary, None).await?;
            let observed = counterfactual::run(&boundary, &approval()?, &changes, changes.files()).await;
            if retired {
                let error = observed.err().ok_or_else(|| Error::new("Error", "Retired framework baseline was unexpectedly materialized"))?;
                assert_eq!(error.code, Some("ROSTER_NATIVE_UNSUPPORTED"));
                assert!(error.message.contains("Retired Node Roster"));
            } else {assert!(observed?.passed());}
            assert_eq!(fs::read_dir(fixture.root.join(".git").join("roster-exec")).map_err(io)?.count(), 0);
            Ok(())
        }.await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn lifecycle_sources_cannot_mutate_themselves_or_outlive_a_replaced_trust_packet()
-> Result<(), Error> {
    for mutate in [true, false] {
        let fixture = Fixture::new(&["docs/guide.md"], "none", &["Docs are updated."], false)?;
        fixture.hook("pre-plan", if mutate {"require('node:fs').writeFileSync('.roster/hooks/owned.mjs','process.stdout.write(\"replaced\");\\n');\n"} else {"process.stdout.write('reviewed source');\n"}, 10_000)?;
        let source = CancellationSource::default();
        let mut lease = backend::RootLease::open(
            &fixture.root,
            "trusted-source-control",
            fixture.host(false)?,
            redaction::Config::default(),
            source.token(),
        )
        .await?;
        let outcome = async {
            let report = hooks::run_pre_plan(&lease, &approval()?).await?;
            if mutate {
                assert_eq!(
                    report.as_value().get("pass"),
                    Some(&Value::Boolean(false)),
                    "{}",
                    report.as_value().json()
                );
                assert!(report.require_current(&fixture.root).await.is_err());
            } else {
                report.require_current(&fixture.root).await?;
                fixture.write(
                    ".roster/hooks/owned.mjs",
                    "process.stdout.write('later replacement');\n",
                )?;
                assert!(
                    report.require_current(&fixture.root).await.is_err(),
                    "A replaced trusted script cannot retain its actual pre-plan receipt"
                );
            }
            assert!(!fixture.root.join("TASK.md").exists());
            Ok(())
        }
        .await;
        locks::finish(outcome, lease.close().await)?;
    }
    Ok(())
}

async fn review68_scoped_change(
    attempt: &mut finalization::Attempt,
    boundary: &mut tools::Boundary,
) -> Result<(), Error> {
    call(attempt, boundary, "read_file", path("src/app.mjs")).await?;
    call(
        attempt,
        boundary,
        "write_file",
        write("src/app.mjs", "export function answer() { return 2; }\n"),
    )
    .await?;
    call(attempt, boundary, "read_file", path("tests/app.test.mjs")).await?;
    call(
        attempt,
        boundary,
        "edit_file",
        Value::object(vec![
            ("path", Value::string("tests/app.test.mjs")),
            ("old_string", Value::string("answer(), 1")),
            ("new_string", Value::string("answer(), 2")),
        ]),
    )
    .await?;
    Ok(())
}

async fn review68_finish(
    mut attempt: finalization::Attempt,
    boundary: &mut tools::Boundary,
) -> Result<finalization::Finalized, Error> {
    close_checks(&mut attempt, boundary).await?;
    attempt.response(boundary, &completion(None, None, Value::Null))?;
    review(&mut attempt, boundary).await?;
    attempt
        .finish(boundary, &evidence::Projection::default())
        .await
}

#[tokio::test(flavor = "current_thread")]
async fn review68_failed_full_regression_survives_scoped_green() -> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Scoped behavior is verified."],
        true,
    )?;
    fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('outside regression', () => assert.equal(answer(), 1));\n")?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned outside regression base"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let mut attempt = start(&boundary, true, true).await?;
        review68_scoped_change(&mut attempt, &mut boundary).await?;
        assert_eq!(
            attempt
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?
                .get("exit_code"),
            Some(&Value::Number(1.0))
        );
        assert_eq!(
            call(
                &mut attempt,
                &mut boundary,
                "run_test",
                Value::object(Vec::new())
            )
            .await?
            .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        let finalized = review68_finish(attempt, &mut boundary).await?;
        assert_eq!(
            finalized.status(),
            finalization::Status::Fail,
            "{}",
            finalized.report().json()
        );
        assert!(
            finalized
                .body()
                .to_utf8()?
                .contains("Original full-suite exit: 1")
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn review68_same_file_unmapped_failure_cannot_be_outside_proof() -> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Scoped behavior is verified."],
        true,
    )?;
    fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('outside preexisting', () => assert.equal(0, 1));\nconst label = 'regression';\ntest(`outside ${label}`, () => assert.equal(answer(), 1));\n")?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned mixed-name failure base"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let baseline = boundary.verify(true).await?;
        assert_eq!(baseline.get("exit_code"), Some(&Value::Number(1.0)));
        assert!(
            baseline
                .get("stdout")
                .and_then(Value::as_string)
                .ok_or_else(|| Error::new("Error", "Actual base TAP output is missing"))?
                .to_utf8()?
                .lines()
                .any(|line| line.starts_with("ok ") && line.ends_with(" - outside regression"))
        );
        boundary.execute("read_file", &path("src/app.mjs")).await?;
        boundary
            .execute(
                "write_file",
                &write("src/app.mjs", "export function answer() { return 2; }\n"),
            )
            .await?;
        boundary
            .execute("read_file", &path("tests/app.test.mjs"))
            .await?;
        boundary
            .execute(
                "edit_file",
                &Value::object(vec![
                    ("path", Value::string("tests/app.test.mjs")),
                    ("old_string", Value::string("answer(), 1")),
                    ("new_string", Value::string("answer(), 2")),
                ]),
            )
            .await?;
        let current = boundary.verify(true).await?;
        assert_eq!(current.get("exit_code"), Some(&Value::Number(1.0)));
        assert!(
            current
                .get("stdout")
                .and_then(Value::as_string)
                .ok_or_else(|| Error::new("Error", "Actual candidate TAP output is missing"))?
                .to_utf8()?
                .lines()
                .any(|line| line.starts_with("not ok ") && line.ends_with(" - outside regression"))
        );
        let observed = classification::run(&boundary, &approval()?).await?;
        assert!(
            !observed.all_outside_proven(),
            "{}",
            observed.as_value().json()
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn review68_option_shaped_recipe_operand_cannot_read_outside_scope() -> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["`roster recipe validate -unchecked/../private/recipe.yml` validates the recipe."],
        true,
    )?;
    let recipe = fs::read_to_string(fixture.root.join("RECIPE.yml")).map_err(io)?;
    fixture.write("private/recipe.yml", &recipe)?;
    assert!(!fixture.root.join("-unchecked").exists());
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        match e2e::run(&boundary, &approval()?).await {
            Err(error) => assert_eq!(error.code, Some("ROSTER_TOOL_DENIED")),
            Ok(report) => {
                return Err(Error::new(
                    "Error",
                    format!(
                        "Unauthorized native CLI recipe read was executed: {}",
                        report.as_value().json()
                    ),
                ));
            }
        }
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn review68_windows_executable_name_keeps_the_cli_obligation() -> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["`roster.exe history list` lists current history."],
        true,
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let report = e2e::run(&boundary, &approval()?).await?;
        assert_eq!(
            report.as_value().get("status"),
            Some(&Value::string("unavailable")),
            "{}",
            report.as_value().json()
        );
        assert!(!report.passed());
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

async fn review68_change_value(
    attempt: &mut finalization::Attempt,
    boundary: &mut tools::Boundary,
    previous: u8,
    value: u8,
) -> Result<(), Error> {
    call(attempt, boundary, "read_file", path("src/app.mjs")).await?;
    let expression = if value == 1 {
        "Number(1)".to_owned()
    } else {
        value.to_string()
    };
    call(
        attempt,
        boundary,
        "write_file",
        write(
            "src/app.mjs",
            &format!("export function answer() {{ return {expression}; }}\n"),
        ),
    )
    .await?;
    call(attempt, boundary, "read_file", path("tests/app.test.mjs")).await?;
    call(
        attempt,
        boundary,
        "edit_file",
        Value::object(vec![
            ("path", Value::string("tests/app.test.mjs")),
            (
                "old_string",
                Value::string(&format!("answer(), {previous}")),
            ),
            ("new_string", Value::string(&format!("answer(), {value}"))),
        ]),
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review68_unclassified_full_failure_survives_a_new_attempt_and_scoped_pass()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Scoped behavior is verified."],
        true,
    )?;
    fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\nconst label = 'unmapped';\ntest(`outside ${label}`, () => assert.equal(answer(), 1));\n")?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned unmapped full-failure obligation",
    ])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let mut attempt = start(&boundary, true, true).await?;
        review68_scoped_change(&mut attempt, &mut boundary).await?;
        assert_eq!(
            attempt
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?
                .get("exit_code"),
            Some(&Value::Number(1.0))
        );
        assert_eq!(
            call(
                &mut attempt,
                &mut boundary,
                "run_test",
                Value::object(Vec::new())
            )
            .await?
            .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        let mut replacement = start(&boundary, true, true).await?;
        assert_eq!(
            call(
                &mut replacement,
                &mut boundary,
                "run_test",
                Value::object(Vec::new())
            )
            .await?
            .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        let finalized = review68_finish(replacement, &mut boundary).await?;
        assert_eq!(
            finalized.status(),
            finalization::Status::Fail,
            "{}",
            finalized.report().json()
        );
        let full = finalized
            .result()
            .get("fullVerification")
            .ok_or_else(|| Error::new("Error", "Retained full obligation missing"))?;
        assert_eq!(full.get("original_exit"), Some(&Value::Number(1.0)));
        assert_eq!(full.get("resolution"), Some(&Value::string("unresolved")));
        assert!(
            finalized
                .body()
                .to_utf8()?
                .contains("Original full-suite exit: 1")
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn review68_mutations_revoke_full_passes_and_outside_exceptions_without_erasing_failure()
-> Result<(), Error> {
    for mode in ["regression", "outside-exception", "full-pass"] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Scoped behavior is verified."],
            true,
        )?;
        fixture.write("tests/outside.test.mjs", if mode == "outside-exception" {"import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('outside preexisting', () => assert.equal(0, 1));\n"} else {"import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('outside regression', () => assert.equal(answer(), 1));\n"})?;
        fixture.git(&["add", "tests/outside.test.mjs"])?;
        fixture.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned mutation invalidation base",
        ])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let mut attempt = start(&boundary, true, true).await?;
            review68_scoped_change(&mut attempt, &mut boundary).await?;
            let observed = attempt
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?;
            assert_eq!(
                observed.get("exit_code"),
                Some(&Value::Number(if mode == "outside-exception" {
                    0.0
                } else {
                    1.0
                }))
            );
            let previous = if mode == "full-pass" {
                review68_change_value(&mut attempt, &mut boundary, 2, 1).await?;
                assert_eq!(
                    attempt
                        .verify_full(&mut boundary, &evidence::Projection::default())
                        .await?
                        .get("exit_code"),
                    Some(&Value::Number(0.0))
                );
                1
            } else {
                2
            };
            review68_change_value(&mut attempt, &mut boundary, previous, 3).await?;
            assert_eq!(
                call(
                    &mut attempt,
                    &mut boundary,
                    "run_test",
                    Value::object(Vec::new())
                )
                .await?
                .get("exit_code"),
                Some(&Value::Number(0.0))
            );
            let finalized = review68_finish(attempt, &mut boundary).await?;
            assert_eq!(
                finalized.status(),
                finalization::Status::Fail,
                "{mode}: {}",
                finalized.report().json()
            );
            let full = finalized
                .result()
                .get("fullVerification")
                .ok_or_else(|| Error::new("Error", "Mutation erased full verification history"))?;
            assert_eq!(full.get("original_exit"), Some(&Value::Number(1.0)));
            assert_eq!(
                full.get("latest_full_current"),
                Some(&Value::Boolean(false))
            );
            assert_eq!(full.get("clean_full_suite"), Some(&Value::Boolean(false)));
            assert_eq!(full.get("resolution"), Some(&Value::string("unresolved")));
            assert!(finalized.result().get("classification").is_none());
            assert!(
                finalized
                    .body()
                    .to_utf8()?
                    .contains("Original full-suite exit: 1")
            );
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review68_current_actual_full_success_recovers_without_rewriting_failed_exit_history()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Scoped behavior is verified."],
        true,
    )?;
    fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('outside regression', () => assert.equal(answer(), 1));\n")?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned full recovery base"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let mut attempt = start(&boundary, true, true).await?;
        review68_scoped_change(&mut attempt, &mut boundary).await?;
        assert_eq!(
            attempt
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?
                .get("exit_code"),
            Some(&Value::Number(1.0))
        );
        review68_change_value(&mut attempt, &mut boundary, 2, 1).await?;
        assert_eq!(
            attempt
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?
                .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        let finalized = review68_finish(attempt, &mut boundary).await?;
        assert_eq!(
            finalized.status(),
            finalization::Status::Pass,
            "{}",
            finalized.report().json()
        );
        let full = finalized
            .result()
            .get("fullVerification")
            .ok_or_else(|| Error::new("Error", "Recovered full verification history missing"))?;
        assert_eq!(full.get("original_exit"), Some(&Value::Number(1.0)));
        assert_eq!(full.get("latest_full_exit"), Some(&Value::Number(0.0)));
        assert_eq!(full.get("latest_full_current"), Some(&Value::Boolean(true)));
        assert_eq!(full.get("clean_full_suite"), Some(&Value::Boolean(true)));
        assert_eq!(full.get("resolution"), Some(&Value::string("full-pass")));
        assert!(finalized.result().get("classification").is_none());
        assert!(
            finalized
                .body()
                .to_utf8()?
                .contains("Original full-suite exit: 1")
        );
        assert!(
            finalized
                .body()
                .to_utf8()?
                .contains("Current actual full verification passed")
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn review68_every_same_file_failure_must_map_uniquely_before_an_outside_exception()
-> Result<(), Error> {
    for mode in [
        "complete",
        "template",
        "duplicate",
        "conditional-duplicate",
        "nested",
        "nested-duplicate",
        "unknown",
        "setup",
        "worker-termination",
    ] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Scoped behavior is verified."],
            true,
        )?;
        let mut outside = "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('outside preexisting', () => assert.equal(0, 1));\n".to_owned();
        outside.push_str(match mode {
            "complete" => "test('outside second', () => assert.equal(0, 2));\n",
            "template" => "const label = 'regression';\ntest(`outside ${label}`, () => assert.equal(answer(), 1));\n",
            "duplicate" => "test('outside duplicate', () => assert.equal(0, 1));\ntest('outside duplicate', () => assert.equal(0, 2));\n",
            "conditional-duplicate" => "if (answer() === 1) { test('outside conditional', () => assert.equal(0, 1)); } else { test('outside conditional', () => assert.equal(0, 2)); }\n",
            "nested" => "test('outside parent', async context => { await context.test('outside child', () => assert.equal(0, 1)); });\n",
            "nested-duplicate" => "test('outside parent', async context => { await context.test('outside child', () => assert.equal(0, 1)); await context.test('outside child', () => assert.equal(0, 2)); });\n",
            "unknown" => "for (const label of ['outside unknown']) { test(label, () => assert.equal(0, 1)); }\n",
            "setup" => "setImmediate(() => { throw new Error('owned setup failure'); });\n",
            _ => "setTimeout(() => process.kill(process.pid, 'SIGTERM'), 50);\n",
        });
        fixture.write("tests/outside.test.mjs", &outside)?;
        fixture.git(&["add", "tests/outside.test.mjs"])?;
        fixture.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned complete failure inventory base",
        ])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            boundary.execute("read_file", &path("src/app.mjs")).await?;
            boundary
                .execute(
                    "write_file",
                    &write("src/app.mjs", "export function answer() { return 2; }\n"),
                )
                .await?;
            boundary
                .execute("read_file", &path("tests/app.test.mjs"))
                .await?;
            boundary
                .execute(
                    "edit_file",
                    &Value::object(vec![
                        ("path", Value::string("tests/app.test.mjs")),
                        ("old_string", Value::string("answer(), 1")),
                        ("new_string", Value::string("answer(), 2")),
                    ]),
                )
                .await?;
            assert_eq!(
                boundary.verify(true).await?.get("exit_code"),
                Some(&Value::Number(1.0))
            );
            let report = classification::run(&boundary, &approval()?).await?;
            let expected = ["complete", "nested"].contains(&mode);
            assert_eq!(
                report.all_outside_proven(),
                expected,
                "{mode}: {}",
                report.as_value().json()
            );
            if expected {
                assert_eq!(
                    report.as_value().get("preexisting_files"),
                    Some(&Value::Array(vec![Value::string("tests/outside.test.mjs")]))
                );
            } else {
                assert_eq!(
                    report.as_value().get("status"),
                    Some(&Value::string("unavailable"))
                );
                assert_eq!(
                    report.as_value().get("preexisting_files"),
                    Some(&Value::Array(Vec::new()))
                );
            }
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review68_an_outside_receipt_cannot_cover_a_different_actual_full_observation()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Scoped behavior is verified."],
        true,
    )?;
    let marker = fixture.container.join("later-full-observation");
    fixture.write("tests/outside.test.mjs", &format!("import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport fs from 'node:fs';\ntest('outside preexisting', () => assert.equal(0, 1));\ntest('outside later failure', () => assert.equal(fs.existsSync({}), false));\n", Value::string(&marker.to_string_lossy()).json()))?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned observation identity base"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let mut attempt = start(&boundary, true, true).await?;
        review68_scoped_change(&mut attempt, &mut boundary).await?;
        assert_eq!(
            attempt
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?
                .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        fs::write(&marker, b"later failure").map_err(io)?;
        assert_eq!(
            boundary.verify(true).await?.get("exit_code"),
            Some(&Value::Number(1.0))
        );
        assert_eq!(
            call(
                &mut attempt,
                &mut boundary,
                "run_test",
                Value::object(Vec::new())
            )
            .await?
            .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        let finalized = review68_finish(attempt, &mut boundary).await?;
        assert_eq!(
            finalized.status(),
            finalization::Status::Fail,
            "{}",
            finalized.report().json()
        );
        assert_eq!(
            finalized
                .result()
                .get("fullVerification")
                .and_then(|full| full.get("resolution")),
            Some(&Value::string("unresolved"))
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

async fn review68_native_read_probe(fixture: &Fixture) -> Result<(PathBuf, PathBuf), Error> {
    let root = fixture.container.join("native-read-probe");
    let source = root.join("src");
    files::create_private_directories(&source)?;
    let marker = fixture.container.join("native-cli-executed");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"owned_read_probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .map_err(io)?;
    fs::write(
        root.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"owned_read_probe\"\nversion = \"0.0.0\"\n",
    )
    .map_err(io)?;
    fs::write(source.join("main.rs"), format!("fn main() -> Result<(), Box<dyn std::error::Error>> {{\nlet args = std::env::args().collect::<Vec<_>>();\nif args.iter().any(|arg| arg == \"--roster-review-invalid-option\") {{return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, \"owned invalid option\").into());}}\nlet path = args.get(3).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, \"owned recipe operand missing\"))?;\nstd::fs::write({}, b\"executed\")?;\nlet contents = std::fs::read_to_string(path)?;\nprint!(\"{{contents}}\");\nOk(())\n}}\n", Value::string(&marker.to_string_lossy()).json())).map_err(io)?;
    let host = fixture.host(true)?;
    let approved = host
        .cargo
        .as_ref()
        .ok_or_else(|| Error::new("Error", "Read-control compiler approval missing"))?;
    let target = root.join("target");
    let mut command = tokio::process::Command::new(&approved.executable);
    command
        .current_dir(&root)
        .env_clear()
        .envs(&host.environment)
        .envs(&approved.environment)
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_NET_OFFLINE", "true")
        .env("RUSTUP_AUTO_INSTALL", "0")
        .args(["build", "--locked", "--offline"]);
    github_agent_roster::execution::owned::execute(
        command,
        github_agent_roster::process::Limits {
            timeout: Some(std::time::Duration::from_secs(60)),
            max_output_bytes: 1024 * 1024,
        },
        None,
    )
    .await
    .map_err(|failure| {
        Error::new(
            "Error",
            format!("Compile owned read-control fixture: {failure}"),
        )
    })?;
    let program = target.join("debug").join("owned_read_probe.exe");
    if !program.is_file() {
        return Err(Error::new(
            "Error",
            "Actual native read-control executable missing",
        ));
    }
    Ok((program, marker))
}

#[tokio::test(flavor = "current_thread")]
async fn review68_recipe_operands_are_checked_before_any_native_cli_execution() -> Result<(), Error>
{
    let owner = Fixture::new(&["docs/guide.md"], "none", &["Docs are updated."], false)?;
    let (probe, marker) = review68_native_read_probe(&owner).await?;
    for operand in [
        "-unchecked/../private/recipe.yml",
        "-unchecked\\..\\private\\recipe.yml",
        "-unchecked/../.env",
        "-unchecked/../.github/workflows/private.yml",
        "-unchecked/../../outside-recipe.yml",
        "private/recipe.yml",
        ".env",
        "RECIPE.yml private/recipe.yml",
        "RECIPE.yml --unchecked.yml",
    ] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &[&format!(
                "`roster recipe validate {operand}` validates the recipe."
            )],
            true,
        )?;
        let recipe = fs::read_to_string(fixture.root.join("RECIPE.yml")).map_err(io)?;
        fixture.write("private/recipe.yml", &recipe)?;
        fixture.write(".env", &recipe)?;
        fixture.write(".github/workflows/private.yml", &recipe)?;
        fs::write(fixture.container.join("outside-recipe.yml"), &recipe).map_err(io)?;
        assert!(!fixture.root.join("-unchecked").exists());
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let mut supplied = approval()?;
            supplied.native_cli = Some(probe.clone());
            let result = e2e::run(&boundary, &supplied).await;
            assert!(
                !marker.exists(),
                "Unauthorized native CLI executed for {operand}"
            );
            let error = result.err().ok_or_else(|| {
                Error::new(
                    "Error",
                    format!("Unsafe recipe operand was not denied: {operand}"),
                )
            })?;
            assert_eq!(error.code, Some("ROSTER_TOOL_DENIED"));
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    for (allowed, operand) in [
        ("docs/guide.md", "RECIPE.yml"),
        ("recipes/approved.yml", "recipes/approved.yml"),
    ] {
        let fixture = Fixture::new(
            &[allowed],
            "none",
            &[&format!(
                "`roster recipe validate {operand}` validates the recipe."
            )],
            true,
        )?;
        if operand != "RECIPE.yml" {
            let recipe = fs::read_to_string(fixture.root.join("RECIPE.yml")).map_err(io)?;
            fixture.write(operand, &recipe)?;
        }
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let report = e2e::run(&boundary, &approval()?).await?;
            assert!(report.passed(), "{}", report.as_value().json());
            assert_eq!(
                report.as_value().get("status"),
                Some(&Value::string("pass"))
            );
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review68_native_executable_aliases_never_turn_selected_commands_into_na()
-> Result<(), Error> {
    let owner = Fixture::new(&["docs/guide.md"], "none", &["Docs are updated."], false)?;
    let (probe, marker) = review68_native_read_probe(&owner).await?;
    for (command, expected) in [
        ("roster.exe --help", "pass"),
        ("roster.exe\u{00a0}--help", "pass"),
        ("roster\u{feff}--help", "pass"),
        ("ROSTER.EXE --help", "pass"),
        ("Roster --help", "pass"),
        ("\"roster.exe\" --help", "pass"),
        ("roster.exe", "pass"),
        ("roster.exe history list", "unavailable"),
        ("roster.exe --unknown", "unavailable"),
        ("roster.exe --config=outside", "unavailable"),
        ("roster.exe --help unexpected", "unavailable"),
        ("roster.exe \"unterminated", "unavailable"),
        ("README.md", "none"),
        ("roster-roadmap", "none"),
        ("node unrelated-consumer.mjs", "none"),
    ] {
        let fixture = Fixture::new(
            &["docs/guide.md"],
            "none",
            &[&format!("`{command}` is inspected.")],
            true,
        )?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let mut supplied = approval()?;
            if expected != "pass" {
                supplied.native_cli = Some(probe.clone());
            }
            let report = e2e::run(&boundary, &supplied).await?;
            assert_eq!(
                report.as_value().get("status"),
                Some(&Value::string(expected)),
                "{command}: {}",
                report.as_value().json()
            );
            assert_eq!(report.passed(), expected != "unavailable");
            assert!(
                !marker.exists(),
                "Malformed, missing or absent CLI obligation executed for {command}"
            );
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review68_mixed_cargo_node_coverage_cannot_be_replaced_by_a_scoped_receipt()
-> Result<(), Error> {
    for native_failure in [false, true] {
        let fixture = Fixture::new(
            &["src/lib.rs", "src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Native and consumer behavior are verified."],
            true,
        )?;
        fixture.write("Cargo.toml", "[package]\nname = \"owned_full_obligation\"\nversion = \"0.0.0\"\nedition = \"2021\"\n")?;
        fixture.write(
            "Cargo.lock",
            "version = 4\n\n[[package]]\nname = \"owned_full_obligation\"\nversion = \"0.0.0\"\n",
        )?;
        fixture.write("src/lib.rs", "pub fn value() -> u8 {\n    1\n}\n")?;
        fixture.write("tests/native.rs", "#[test]\nfn native_behavior() {\n    assert_eq!(owned_full_obligation::value(), 1);\n}\n")?;
        fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('outside preexisting', () => assert.equal(0, 1));\n")?;
        fixture.git(&[
            "add",
            "Cargo.toml",
            "Cargo.lock",
            "src/lib.rs",
            "tests/native.rs",
            "tests/outside.test.mjs",
        ])?;
        fixture.git(&[
            "commit",
            "--quiet",
            "-m",
            "Owned mixed-engine obligation base",
        ])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, true).await?;
        let outcome = async {
            let mut attempt = start(&boundary, true, true).await?;
            review68_scoped_change(&mut attempt, &mut boundary).await?;
            call(&mut attempt, &mut boundary, "read_file", path("src/lib.rs")).await?;
            call(
                &mut attempt,
                &mut boundary,
                "write_file",
                write(
                    "src/lib.rs",
                    if native_failure {
                        "pub fn value() -> u8 {\n    2\n}\n"
                    } else {
                        "pub fn value() -> u8 {\n    u8::from(true)\n}\n"
                    },
                ),
            )
            .await?;
            let full = attempt
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?;
            assert_eq!(full.get("native"), Some(&Value::Boolean(true)));
            assert_eq!(
                full.get("exit_code"),
                Some(&Value::Number(if native_failure { 101.0 } else { 0.0 }))
            );
            if native_failure {
                call(&mut attempt, &mut boundary, "read_file", path("src/lib.rs")).await?;
                call(
                    &mut attempt,
                    &mut boundary,
                    "write_file",
                    write(
                        "src/lib.rs",
                        "pub fn value() -> u8 {\n    u8::from(true)\n}\n",
                    ),
                )
                .await?;
                let scoped = call(
                    &mut attempt,
                    &mut boundary,
                    "run_test",
                    Value::object(Vec::new()),
                )
                .await?;
                assert_eq!(scoped.get("native"), Some(&Value::Boolean(true)));
                assert_eq!(scoped.get("exit_code"), Some(&Value::Number(0.0)));
            }
            let finalized = review68_finish(attempt, &mut boundary).await?;
            assert_eq!(
                finalized.status(),
                if native_failure {
                    finalization::Status::Fail
                } else {
                    finalization::Status::Pass
                },
                "{}",
                finalized.report().json()
            );
            let full = finalized
                .result()
                .get("fullVerification")
                .ok_or_else(|| Error::new("Error", "Mixed actual full receipt missing"))?;
            assert_eq!(
                full.get("original_exit"),
                Some(&Value::Number(if native_failure { 101.0 } else { 1.0 }))
            );
            assert_eq!(
                full.get("resolution"),
                Some(&Value::string(if native_failure {
                    "unresolved"
                } else {
                    "outside-exception"
                }))
            );
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

use github_agent_roster::execution::{
    archive as recovery_archive, checkpoints as recovery_checkpoints, restore as recovery_restore,
};

struct CurrentUnpublished;

impl recovery_restore::PublicationCheck for CurrentUnpublished {
    async fn current(&mut self, subject: &Value) -> Result<recovery_restore::Publication, Error> {
        assert!(
            subject
                .get("checkpoint")
                .and_then(|record| record.get("validation"))
                .is_some()
        );
        Ok(recovery_restore::Publication::Unpublished)
    }
}

struct PublicationSequence {
    states: std::collections::VecDeque<recovery_restore::Publication>,
    calls: usize,
}

impl recovery_restore::PublicationCheck for PublicationSequence {
    async fn current(&mut self, subject: &Value) -> Result<recovery_restore::Publication, Error> {
        assert!(
            subject
                .get("checkpoint")
                .and_then(|record| record.get("validation"))
                .is_some()
        );
        self.calls += 1;
        self.states
            .pop_front()
            .ok_or_else(|| Error::new("Error", "Current publication lookup is unavailable"))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_publication_is_rechecked_at_execution_mutation_and_commit_without_default_false()
-> Result<(), Error> {
    use recovery_restore::Publication::{Published, Unknown, Unpublished};
    for (phase, states) in [
        ("before", vec![Published]),
        ("staged", vec![Unpublished, Unknown]),
        ("commit", vec![Unpublished, Unpublished, Published]),
        ("valid", vec![Unpublished, Unpublished, Unpublished]),
        ("unavailable", Vec::new()),
    ] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let green = recovery_green(&mut boundary).await?;
            recovery_node_value(&mut boundary, 2, 3).await?;
            fixture.write("RESULT.md", "Prior user evidence\n")?;
            let count = recovery_checkpoints::list(&boundary)?.len();
            let approved = recovery_restore::prepare(
                &boundary,
                green
                    .get("number")
                    .and_then(Value::as_number)
                    .map(|value| value as u64),
            )
            .await?
            .confirm(&boundary, Unpublished, true)
            .await?;
            let expected_calls = states.len().max(1);
            let mut publication = PublicationSequence {
                states: states.into(),
                calls: 0,
            };
            let restored =
                recovery_restore::execute(&mut boundary, approved, &mut publication).await;
            assert_eq!(publication.calls, expected_calls);
            if phase == "valid" {
                assert!(restored.is_ok());
                assert!(boundary.last_tests().is_none());
                assert_eq!(
                    fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
                    "export function answer() { return 2; }\n"
                );
            } else {
                assert!(restored.is_err());
                assert_eq!(
                    fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
                    "export function answer() { return 3; }\n"
                );
                assert_eq!(
                    fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                    "Prior user evidence\n"
                );
                if ["before", "unavailable"].contains(&phase) {
                    assert_eq!(recovery_checkpoints::list(&boundary)?.len(), count);
                    assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
                }
            }
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_boundary_reopen_requires_fresh_checks_and_refuses_unreconciled_journals()
-> Result<(), Error> {
    for mode in ["committed", "unreconciled"] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let green = recovery_green(&mut boundary).await?;
        recovery_node_value(&mut boundary, 2, 3).await?;
        let approved = recovery_restore::prepare(
            &boundary,
            green
                .get("number")
                .and_then(Value::as_number)
                .map(|value| value as u64),
        )
        .await?
        .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
        .await?;
        let restored =
            recovery_restore::execute(&mut boundary, approved, &mut CurrentUnpublished).await?;
        let directory = restored
            .as_value()
            .get("transaction")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::new("Error", "Actual restore journal path missing"))?
            .to_utf8()?;
        boundary.close().await?;
        if mode == "unreconciled" {
            fs::remove_file(Path::new(&directory).join("COMMITTED")).map_err(io)?;
            let opened = fixture.open(&source, false).await;
            assert_eq!(
                opened.err().and_then(|error| error.code),
                Some("ROSTER_RESTORE_RECOVERY")
            );
            assert!(
                fixture
                    .root
                    .join(".roster")
                    .join("locks")
                    .join("run.lock")
                    .exists()
            );
            assert_eq!(
                fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
                "export function answer() { return 2; }\n"
            );
        } else {
            let mut reopened = fixture.open(&source, false).await?;
            assert!(reopened.last_tests().is_none());
            let mut attempt = start(&reopened, false, false).await?;
            attempt.response(&reopened, &completion(None, None, Value::Null))?;
            review(&mut attempt, &reopened).await?;
            let result = attempt
                .finish(&reopened, &evidence::Projection::default())
                .await?;
            assert_ne!(result.status(), finalization::Status::Pass);
            assert!(result.report().json().contains("restored checkpoint"));
            reopened.close().await?;
        }
    }
    Ok(())
}

async fn recovery_node_value(
    boundary: &mut tools::Boundary,
    previous: u8,
    value: u8,
) -> Result<(), Error> {
    boundary.execute("read_file", &path("src/app.mjs")).await?;
    boundary
        .execute(
            "write_file",
            &write(
                "src/app.mjs",
                &format!("export function answer() {{ return {value}; }}\n"),
            ),
        )
        .await?;
    boundary
        .execute("read_file", &path("tests/app.test.mjs"))
        .await?;
    boundary
        .execute(
            "edit_file",
            &Value::object(vec![
                ("path", Value::string("tests/app.test.mjs")),
                (
                    "old_string",
                    Value::string(&format!("answer(), {previous}")),
                ),
                ("new_string", Value::string(&format!("answer(), {value}"))),
            ]),
        )
        .await?;
    Ok(())
}

async fn recovery_green(boundary: &mut tools::Boundary) -> Result<Value, Error> {
    recovery_node_value(boundary, 1, 2).await?;
    assert_eq!(
        boundary.verify(true).await?.get("exit_code"),
        Some(&Value::Number(0.0))
    );
    recovery_checkpoints::capture(boundary, true).await
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_archive_uses_legacy_layout_and_preserves_tracked_linked_secret_and_cancelled_inputs()
-> Result<(), Error> {
    let names = [
        "RECIPE.yml",
        "TASK.md",
        "PLAN.md",
        "ESTIMATE.md",
        "CONTEXT.md",
        "RESEARCH.md",
        "RESULT.md",
        "REVIEW.md",
    ];
    for mode in ["archive", "tracked", "linked", "secret", "cancel"] {
        let fixture = Fixture::new(&["docs/guide.md"], "none", &["Docs are updated."], false)?;
        for name in names {
            fixture.write(name, &format!("Prior {name}\n"))?;
        }
        if mode == "tracked" {
            fixture.git(&["add", "--force", "RESULT.md"])?;
        }
        if mode == "linked" {
            fs::hard_link(
                fixture.root.join("RESULT.md"),
                fixture.container.join("other-result"),
            )
            .map_err(io)?;
        }
        if mode == "secret" {
            fixture.write(
                "RESULT.md",
                "-----BEGIN PRIVATE KEY-----\nowned fixture marker\n-----END PRIVATE KEY-----\n",
            )?;
        }
        let before = names
            .iter()
            .map(|name| {
                Ok((
                    name.to_string(),
                    fs::read(fixture.root.join(name)).map_err(io)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        let source = CancellationSource::default();
        let mut lease = backend::RootLease::open(
            &fixture.root,
            "owned-archive",
            fixture.host(false)?,
            redaction::Config::default(),
            source.token(),
        )
        .await?;
        let outcome = async {
            let prepared = recovery_archive::prepare_before_plan(&lease, "issue-42", &[]).await;
            if ["tracked", "linked", "secret"].contains(&mode) {
                assert!(prepared.is_err(), "{mode} artifact was accepted");
                assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
            } else if mode == "cancel" {
                let plan = prepared?;
                let cancellation = async {
                    for _ in 0..2000 {
                        if !fixture.root.join("RECIPE.yml").exists() {
                            return source.cancel(github_agent_roster::cancel::Reason::Cancelled);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                    Err(Error::new(
                        "Error",
                        "Archive cancellation barrier was not reached",
                    ))
                };
                let (archived, cancelled) = tokio::join!(
                    recovery_archive::execute_before_plan(plan, &lease),
                    cancellation
                );
                cancelled?;
                assert!(archived.is_err());
            } else {
                let archived = recovery_archive::execute_before_plan(prepared?, &lease)
                    .await?
                    .ok_or_else(|| Error::new("Error", "Actual artifact archive missing"))?;
                assert_eq!(
                    archived
                        .directory()
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str()),
                    Some("issue-42")
                );
                let name = archived
                    .directory()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| Error::new("Error", "Archive name is not Unicode"))?;
                let (millis, nonce) = name.split_once('-').ok_or_else(|| {
                    Error::new("Error", "Legacy archive directory format missing")
                })?;
                assert!(millis.bytes().all(|value| value.is_ascii_digit()));
                assert_eq!(nonce.len(), 12);
                for (name, bytes) in &before {
                    assert!(!fixture.root.join(name).exists());
                    assert_eq!(
                        fs::read(archived.directory().join(name)).map_err(io)?,
                        *bytes
                    );
                }
            }
            if mode != "archive" {
                for (name, bytes) in &before {
                    assert_eq!(fs::read(fixture.root.join(name)).map_err(io)?, *bytes);
                }
            }
            Ok(())
        }
        .await;
        locks::finish(outcome, lease.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_restore_preserves_later_edits_and_revokes_context_calls_and_test_seals()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &[
            "src/app.mjs",
            "tests/app.test.mjs",
            "src/deleted.mjs",
            "src/new.mjs",
        ],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    fixture.write("src/deleted.mjs", "export const kept = 1;\n")?;
    fixture.write("outside.txt", "User file outside task scope\n")?;
    fixture.git(&["add", "src/deleted.mjs", "outside.txt"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned restore deleted-file base"])?;
    fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let green = recovery_green(&mut boundary).await?;
        recovery_node_value(&mut boundary, 2, 3).await?;
        boundary
            .execute("read_file", &path("src/deleted.mjs"))
            .await?;
        boundary
            .execute("delete_file", &path("src/deleted.mjs"))
            .await?;
        boundary
            .execute(
                "write_file",
                &write("src/new.mjs", "export const later = 3;\n"),
            )
            .await?;
        fixture.write("outside.txt", "User file outside task scope\n")?;
        fixture.write("RESULT.md", "Previous result\n")?;
        fixture.write("REVIEW.md", "Previous review\n")?;
        fixture.write("CONTEXT.md", "Previous context\n")?;
        fixture.git(&["add", "src/app.mjs"])?;
        let index = fixture.git(&["diff", "--cached", "--raw"])?;
        assert_eq!(
            boundary.verify(true).await?.get("exit_code"),
            Some(&Value::Number(0.0))
        );
        let task = fs::read(fixture.root.join("TASK.md")).map_err(io)?;
        let recipe = fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?;
        let mut old = start(&boundary, true, false).await?;
        let decoded = old.response(
            &boundary,
            &completion(None, Some("read_file"), path("src/app.mjs")),
        )?;
        let pending = decoded
            .calls
            .first()
            .ok_or_else(|| Error::new("Error", "Pending native call missing"))?
            .clone();
        let plan = recovery_checkpoints::prepare_restore(
            &boundary,
            green
                .get("number")
                .and_then(Value::as_number)
                .map(|number| number as u64),
        )
        .await?;
        let approved = plan
            .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
            .await?;
        let restored = old
            .restore_checkpoint(&mut boundary, approved, &mut CurrentUnpublished)
            .await?;
        assert_eq!(
            restored.as_value().get("status"),
            Some(&Value::string("restored-unverified"))
        );
        assert!(boundary.last_tests().is_none());
        assert!(
            old.execute(&mut boundary, &pending, &evidence::Projection::default())
                .await
                .is_err()
        );
        assert!(
            old.workspace()
                .export(&boundary, "verified", &evidence::Projection::default())
                .is_err()
        );
        assert!(
            !boundary
                .tool_names()
                .iter()
                .any(|name| name.contains("restore"))
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
            "export function answer() { return 2; }\n"
        );
        assert!(fixture.root.join("src").join("deleted.mjs").is_file());
        assert!(!fixture.root.join("src").join("new.mjs").exists());
        assert_eq!(fs::read(fixture.root.join("TASK.md")).map_err(io)?, task);
        assert_eq!(
            fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?,
            recipe
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("outside.txt")).map_err(io)?,
            "User file outside task scope\n"
        );
        assert_eq!(fixture.git(&["diff", "--cached", "--raw"])?, index);
        let archive = restored
            .as_value()
            .get("archive")
            .and_then(|value| value.get("directory"))
            .and_then(Value::as_string)
            .ok_or_else(|| Error::new("Error", "Restore did not archive current evidence"))?
            .to_utf8()?;
        assert_eq!(
            fs::read_to_string(Path::new(&archive).join("RESULT.md")).map_err(io)?,
            "Previous result\n"
        );
        assert!(!fixture.root.join("RESULT.md").exists());
        let backup_tree = restored
            .as_value()
            .get("backup")
            .and_then(|value| value.get("tree"))
            .and_then(Value::as_string)
            .ok_or_else(|| Error::new("Error", "Restore backup tree missing"))?
            .to_utf8()?;
        assert_eq!(
            fixture.git(&["show", &format!("{backup_tree}:src/app.mjs")])?,
            "export function answer() { return 3; }\n"
        );
        let reviews = recovery_archive::archived_artifacts(&boundary, "REVIEW.md", 1).await?;
        assert_eq!(reviews, vec![Text::from("Previous review\n")]);
        let mut stale = start(&boundary, false, false).await?;
        stale.response(&boundary, &completion(None, None, Value::Null))?;
        review(&mut stale, &boundary).await?;
        let unverified = stale
            .finish(&boundary, &evidence::Projection::default())
            .await?;
        assert_ne!(unverified.status(), finalization::Status::Pass);
        assert!(unverified.report().json().contains("restored checkpoint"));
        let mut fresh = start(&boundary, true, false).await?;
        assert_eq!(
            call(
                &mut fresh,
                &mut boundary,
                "run_test",
                Value::object(Vec::new())
            )
            .await?
            .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        assert_eq!(
            review68_finish(fresh, &mut boundary).await?.status(),
            finalization::Status::Pass
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_restore_requires_current_explicit_unpublished_approval_and_unchanged_inputs()
-> Result<(), Error> {
    for mode in [
        "unknown",
        "published",
        "unconfirmed",
        "stale-source",
        "stale-artifact",
        "stale-approval",
        "approval-artifact",
        "changed-ref",
        "linked",
        "secret",
    ] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let green = recovery_green(&mut boundary).await?;
            recovery_node_value(&mut boundary, 2, 3).await?;
            fixture.write("RESULT.md", "Preserve user result\n")?;
            let count = recovery_checkpoints::list(&boundary)?.len();
            let number = green
                .get("number")
                .and_then(Value::as_number)
                .map(|number| number as u64);
            if mode == "linked" {
                fs::hard_link(
                    fixture.root.join("src").join("app.mjs"),
                    fixture.container.join("other-app"),
                )
                .map_err(io)?;
                assert!(recovery_restore::prepare(&boundary, number).await.is_err());
            } else if mode == "secret" {
                fixture.write(
                    "src/app.mjs",
                    "-----BEGIN PRIVATE KEY-----\nowned fixture marker\n",
                )?;
                assert!(recovery_restore::prepare(&boundary, number).await.is_err());
            } else if mode == "changed-ref" {
                let reference = green
                    .get("ref")
                    .and_then(Value::as_string)
                    .ok_or_else(|| Error::new("Error", "Green ref missing"))?
                    .to_utf8()?;
                let earlier = recovery_checkpoints::list(&boundary)?
                    .into_iter()
                    .find(|record| record.get("tree") != green.get("tree"))
                    .ok_or_else(|| Error::new("Error", "Controlled alternate tree missing"))?;
                let tree = earlier
                    .get("tree")
                    .and_then(Value::as_string)
                    .ok_or_else(|| Error::new("Error", "Earlier tree missing"))?
                    .to_utf8()?;
                fixture.git(&["update-ref", &reference, &tree])?;
                assert!(recovery_restore::prepare(&boundary, number).await.is_err());
            } else {
                let plan = recovery_restore::prepare(&boundary, number).await?;
                if mode == "stale-source" || mode == "stale-artifact" {
                    if mode == "stale-artifact" {
                        fixture.write("RESULT.md", "New user result\n")?;
                    } else {
                        fixture.write("outside.txt", "New user bytes\n")?;
                    }
                    assert!(
                        plan.confirm(&boundary, recovery_restore::Publication::Unpublished, true)
                            .await
                            .is_err()
                    );
                } else {
                    let publication = match mode {
                        "unknown" => recovery_restore::Publication::Unknown,
                        "published" => recovery_restore::Publication::Published,
                        _ => recovery_restore::Publication::Unpublished,
                    };
                    let approval = plan
                        .confirm(&boundary, publication, mode != "unconfirmed")
                        .await;
                    if mode == "stale-approval" || mode == "approval-artifact" {
                        let approval = approval?;
                        if mode == "approval-artifact" {
                            fixture.write("RESULT.md", "New user result\n")?;
                        } else {
                            fixture.write("outside.txt", "After-approval user bytes\n")?;
                        }
                        assert!(
                            recovery_restore::execute(
                                &mut boundary,
                                approval,
                                &mut CurrentUnpublished
                            )
                            .await
                            .is_err()
                        );
                    } else {
                        assert!(approval.is_err());
                    }
                }
            }
            assert_eq!(recovery_checkpoints::list(&boundary)?.len(), count);
            assert_eq!(
                fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                if ["stale-artifact", "approval-artifact"].contains(&mode) {
                    "New user result\n"
                } else {
                    "Preserve user result\n"
                }
            );
            assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_restore_rolls_back_real_windows_denials_and_retains_unknown_cleanup_ownership()
-> Result<(), Error> {
    use std::os::windows::fs::OpenOptionsExt;
    for mode in ["denied", "cancel", "quarantine", "task-mutation"] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let green = recovery_green(&mut boundary).await?;
            recovery_node_value(&mut boundary, 2, 3).await?;
            fixture.write("RESULT.md", "Evidence before rollback\n")?;
            let plan = recovery_restore::prepare(
                &boundary,
                green
                    .get("number")
                    .and_then(Value::as_number)
                    .map(|number| number as u64),
            )
            .await?;
            let approval = plan
                .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
                .await?;
            let denied = if mode == "denied" {
                Some(
                    fs::OpenOptions::new()
                        .read(true)
                        .share_mode(1)
                        .open(fixture.root.join("tests").join("app.test.mjs"))
                        .map_err(io)?,
                )
            } else {
                None
            };
            let watcher = async {
                if mode == "denied" {
                    return Ok(None);
                }
                let started = std::time::Instant::now();
                while started.elapsed() < std::time::Duration::from_secs(120) {
                    if fs::read_to_string(fixture.root.join("src").join("app.mjs"))
                        .map_err(io)?
                        .contains("return 2;")
                    {
                        if mode == "task-mutation" {
                            fixture.write("TASK.md", "New human task must not be overwritten\n")?;
                            return Ok(None);
                        }
                        let handle = if mode == "quarantine" {
                            Some(
                                fs::OpenOptions::new()
                                    .read(true)
                                    .share_mode(1)
                                    .open(fixture.root.join("src").join("app.mjs"))
                                    .map_err(io)?,
                            )
                        } else {
                            None
                        };
                        source.cancel(github_agent_roster::cancel::Reason::Cancelled)?;
                        return Ok(handle);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
                Err(Error::new("Error", "Restore cancellation barrier missing"))
            };
            let mut publication = CurrentUnpublished;
            let (restored, held) = tokio::join!(
                recovery_restore::execute(&mut boundary, approval, &mut publication),
                watcher
            );
            let held = held?;
            assert!(restored.is_err());
            if mode == "quarantine" {
                assert_eq!(
                    restored.err().and_then(|error| error.code),
                    Some("ROSTER_RESTORE_CLEANUP")
                );
                assert!(boundary.close().await.is_err());
                assert!(
                    fixture
                        .root
                        .join(".roster")
                        .join("locks")
                        .join("run.lock")
                        .exists()
                );
            } else {
                assert_eq!(
                    fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
                    "export function answer() { return 3; }\n"
                );
                assert!(
                    fs::read_to_string(fixture.root.join("tests").join("app.test.mjs"))
                        .map_err(io)?
                        .contains("answer(), 3")
                );
                assert_eq!(
                    fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                    "Evidence before rollback\n"
                );
                if mode == "task-mutation" {
                    assert_eq!(
                        fs::read_to_string(fixture.root.join("TASK.md")).map_err(io)?,
                        "New human task must not be overwritten\n"
                    );
                }
            }
            drop(held);
            drop(denied);
            Ok(())
        }
        .await;
        if mode == "quarantine" {
            outcome?;
        } else {
            locks::finish(outcome, boundary.close().await)?;
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_restored_rust_requires_new_cargo_and_keeps_the_failed_full_obligation()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/lib.rs", "tests/native.rs"],
        "characterization",
        &["Native behavior is verified."],
        true,
    )?;
    fixture.write(
        "Cargo.toml",
        "[package]\nname = \"owned_native_restore\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )?;
    fixture.write(
        "Cargo.lock",
        "version = 4\n\n[[package]]\nname = \"owned_native_restore\"\nversion = \"0.0.0\"\n",
    )?;
    fixture.write("src/lib.rs", "pub fn value() -> u8 {\n    1\n}\n")?;
    fixture.write(
        "tests/native.rs",
        "#[test]\nfn native_behavior() {\n    assert_eq!(owned_native_restore::value(), 1);\n}\n",
    )?;
    fixture.write(
        "tests/outside.rs",
        "#[test]\nfn outside_behavior() {\n    assert!(owned_native_restore::value() <= 2);\n}\n",
    )?;
    fixture.git(&[
        "add",
        "Cargo.toml",
        "Cargo.lock",
        "src/lib.rs",
        "tests/native.rs",
        "tests/outside.rs",
    ])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned Cargo restore base"])?;
    fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, true).await?;
    let outcome = async {
        for (previous, value) in [(1, 2), (2, 3)] {
            boundary.execute("read_file", &path("src/lib.rs")).await?;
            boundary
                .execute(
                    "write_file",
                    &write(
                        "src/lib.rs",
                        &format!("pub fn value() -> u8 {{\n    {value}\n}}\n"),
                    ),
                )
                .await?;
            boundary
                .execute("read_file", &path("tests/native.rs"))
                .await?;
            boundary
                .execute(
                    "edit_file",
                    &Value::object(vec![
                        ("path", Value::string("tests/native.rs")),
                        ("old_string", Value::string(&format!("value(), {previous}"))),
                        ("new_string", Value::string(&format!("value(), {value}"))),
                    ]),
                )
                .await?;
            if value == 2 {
                let checks = boundary.verify(true).await?;
                assert_eq!(checks.get("native"), Some(&Value::Boolean(true)));
                assert_eq!(checks.get("exit_code"), Some(&Value::Number(0.0)));
                recovery_checkpoints::capture(&boundary, true).await?;
            }
        }
        let failed = boundary.verify(true).await?;
        assert_eq!(failed.get("native"), Some(&Value::Boolean(true)));
        assert_eq!(failed.get("exit_code"), Some(&Value::Number(101.0)));
        let approval = recovery_restore::prepare(&boundary, None)
            .await?
            .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
            .await?;
        recovery_restore::execute(&mut boundary, approval, &mut CurrentUnpublished).await?;
        boundary.close().await?;
        boundary = fixture.open(&source, true).await?;
        assert!(boundary.last_tests().is_none());
        let mut scoped = start(&boundary, true, false).await?;
        let checks = call(
            &mut scoped,
            &mut boundary,
            "run_test",
            Value::object(Vec::new()),
        )
        .await?;
        assert_eq!(checks.get("native"), Some(&Value::Boolean(true)));
        assert_eq!(checks.get("exit_code"), Some(&Value::Number(0.0)));
        assert_eq!(
            review68_finish(scoped, &mut boundary).await?.status(),
            finalization::Status::Fail
        );
        let mut full = start(&boundary, true, false).await?;
        assert_eq!(
            full.verify_full(&mut boundary, &evidence::Projection::default())
                .await?
                .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        assert_eq!(
            review68_finish(full, &mut boundary).await?.status(),
            finalization::Status::Pass
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

use github_agent_roster::execution::docker as native_docker;

async fn docker_fixture(owner: &Fixture) -> Result<PathBuf, Error> {
    let root = owner.container.join("dc");
    files::create_private_directories(&root.join("src"))?;
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"owned_docker_cli\"\nversion = \"0.0.0\"\nedition = \"2021\"\nrust-version = \"1.85\"\n\n[lints.rust]\nunsafe_code = \"forbid\"\n\n[lints.clippy]\nunwrap_used = \"deny\"\nexpect_used = \"deny\"\npanic = \"deny\"\n").map_err(io)?;
    fs::write(
        root.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"owned_docker_cli\"\nversion = \"0.0.0\"\n",
    )
    .map_err(io)?;
    fs::write(
        root.join("src").join("main.rs"),
        include_str!("fixtures/docker_cli.rs"),
    )
    .map_err(io)?;
    let host = owner.host(true)?;
    let cargo = host
        .cargo
        .as_ref()
        .ok_or_else(|| Error::new("Error", "Owned Docker CLI compiler approval missing"))?;
    let target = root.join("target");
    for args in [
        vec![
            "clippy",
            "--all-targets",
            "--locked",
            "--offline",
            "--",
            "-D",
            "warnings",
        ],
        vec!["build", "--locked", "--offline"],
    ] {
        let mut command = tokio::process::Command::new(&cargo.executable);
        command
            .current_dir(&root)
            .env_clear()
            .envs(&host.environment)
            .envs(&cargo.environment)
            .env("CARGO_TARGET_DIR", &target)
            .env("CARGO_NET_OFFLINE", "true")
            .env("RUSTUP_AUTO_INSTALL", "0")
            .args(args);
        github_agent_roster::execution::owned::execute(
            command,
            github_agent_roster::process::Limits {
                timeout: Some(std::time::Duration::from_secs(90)),
                max_output_bytes: 1024 * 1024,
            },
            None,
        )
        .await
        .map_err(|failure| {
            let diagnostic = String::from_utf8_lossy(&failure.captured.stderr);
            Error::new(
                "Error",
                format!("Compile/lint owned Docker CLI fixture: {failure}: {diagnostic}"),
            )
        })?;
    }
    let path = target.join("debug").join("owned_docker_cli.exe");
    if !path.is_file() {
        return Err(Error::new(
            "Error",
            "Owned native Docker CLI executable missing",
        ));
    }
    Ok(path)
}

fn docker_case(
    fixture: &Fixture,
    compiled: &Path,
    mode: &str,
    inputs: &[&str],
) -> Result<(native_docker::Approval, PathBuf), Error> {
    let executable = fixture.container.join("docker.exe");
    fs::copy(compiled, &executable).map_err(io)?;
    let state = fixture.container.join("docker-state");
    files::create_private_directories(&state)?;
    fs::write(state.join("mode"), mode).map_err(io)?;
    Ok((
        native_docker::Approval {
            enabled: true,
            executable,
            endpoint: native_docker::Endpoint::WindowsPipe,
            image: format!("sha256:{}", "a".repeat(64)),
            runtime: native_docker::Runtime::Native {
                entrypoint: "/usr/local/bin/owned-verify".to_owned(),
            },
            files: inputs.iter().map(|path| (*path).to_owned()).collect(),
            dependencies: Vec::new(),
            verifiers: Vec::new(),
            observations: vec![native_docker::Observation {
                check_id: 1,
                action: native_docker::Action::Native(vec!["echo".to_owned()]),
                expected_exit: 0,
                expected_stdout: "verified\n".to_owned(),
                expected_stderr: String::new(),
                evidence: "Owned isolated command observed".to_owned(),
            }],
            timeout_ms: 30_000,
        },
        state,
    ))
}

fn docker_calls(state: &Path) -> Result<Vec<Vec<String>>, Error> {
    let path = state.join("calls");
    let lines = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io(error)),
    };
    lines
        .lines()
        .map(|line| {
            let Value::Array(values) = github_agent_roster::json::parse(&Text::from(line))? else {
                return Err(Error::new("Error", "Owned Docker CLI log is not an array"));
            };
            values
                .iter()
                .map(|value| {
                    value
                        .as_string()
                        .ok_or_else(|| Error::new("Error", "Docker CLI argument is not text"))?
                        .to_utf8()
                })
                .collect()
        })
        .collect()
}

fn docker_objects(state: &Path) -> Result<usize, Error> {
    Ok(fs::read_dir(state)
        .map_err(io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io)?
        .iter()
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("args"))
        .count())
}

#[tokio::test(flavor = "current_thread")]
async fn docker_absence_is_na_but_selection_requires_explicit_enabled_host_approval()
-> Result<(), Error> {
    let owner = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Current behavior is verified."],
        false,
    )?;
    let compiled = docker_fixture(&owner).await?;
    let fixture = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    let (mut approval, state) = docker_case(&fixture, &compiled, "pass", &["src/app.mjs"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let none = native_docker::run(&boundary, &native_docker::Selection::None).await?;
        assert!(none.passed());
        assert_eq!(none.as_value().get("status"), Some(&Value::string("none")));
        let missing =
            native_docker::run(&boundary, &native_docker::Selection::Docker(None)).await?;
        assert!(!missing.passed());
        assert_eq!(
            missing.as_value().get("status"),
            Some(&Value::string("unavailable"))
        );
        approval.enabled = false;
        let disabled = native_docker::run(
            &boundary,
            &native_docker::Selection::Docker(Some(Box::new(approval.clone()))),
        )
        .await?;
        assert!(!disabled.passed());
        assert_eq!(
            disabled.as_value().get("status"),
            Some(&Value::string("unavailable"))
        );
        assert!(docker_calls(&state)?.is_empty());
        approval.enabled = true;
        approval.image = "mutable:latest".to_owned();
        assert!(
            native_docker::run(
                &boundary,
                &native_docker::Selection::Docker(Some(Box::new(approval)))
            )
            .await
            .is_err()
        );
        assert!(docker_calls(&state)?.is_empty());
        assert!(
            !boundary
                .tool_names()
                .iter()
                .any(|name| name.contains("docker") || name.contains("restore"))
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn docker_owned_protocol_binds_readonly_inputs_receipts_flags_and_current_subjects()
-> Result<(), Error> {
    let owner = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Current behavior is verified."],
        false,
    )?;
    let compiled = docker_fixture(&owner).await?;
    for node in [false, true] {
        let fixture = Fixture::new(
            &["src/app.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.write("RESULT.md", "Owned current result\n")?;
        let (mut approval, state) = docker_case(&fixture, &compiled, "pass", &["src/app.mjs"])?;
        if node {
            approval.runtime = native_docker::Runtime::Node20;
            approval
                .observations
                .get_mut(0)
                .ok_or_else(|| Error::new("Error", "Observation missing"))?
                .action = native_docker::Action::Node {
                script: native_docker::Script::Product("src/app.mjs".to_owned()),
                args: Vec::new(),
            };
        } else {
            let observation = approval
                .observations
                .get_mut(0)
                .ok_or_else(|| Error::new("Error", "Observation missing"))?;
            observation.action = native_docker::Action::Native(vec![
                "read-product".to_owned(),
                "src/app.mjs".to_owned(),
            ]);
            observation.expected_stdout = "export function answer() { return 1; }\n".to_owned();
            approval.dependencies.push("tests/app.test.mjs".to_owned());
        }
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let report = native_docker::run(
                &boundary,
                &native_docker::Selection::Docker(Some(Box::new(approval))),
            )
            .await?;
            assert!(report.passed(), "{}", report.as_value().json());
            report.require_current(&boundary).await?;
            assert_eq!(
                report.as_value().get("status"),
                Some(&Value::string("pass"))
            );
            let receipt = report
                .as_value()
                .get("receipt")
                .ok_or_else(|| Error::new("Error", "Actual native receipt missing"))?;
            let subject = report
                .as_value()
                .get("subject")
                .ok_or_else(|| Error::new("Error", "Current subject missing"))?;
            assert_eq!(
                github_agent_roster::execution::verification::judge_current_receipt(
                    receipt,
                    subject,
                    &fixture.root,
                    &["src/app.mjs".to_owned()],
                    1,
                    boundary.token()
                )?
                .get("status"),
                Some(&Value::string("pass"))
            );
            assert_eq!(docker_objects(&state)?, 0);
            let calls = docker_calls(&state)?;
            assert_eq!(
                calls
                    .iter()
                    .filter(|args| args.get(4).is_some_and(|value| value == "container")
                        && args.get(5).is_some_and(|value| value == "create"))
                    .count(),
                if node { 2 } else { 1 }
            );
            for args in &calls {
                assert_eq!(args.get(2).map(String::as_str), Some("--host"));
                assert_eq!(
                    args.get(3).map(String::as_str),
                    Some("npipe:////./pipe/docker_engine")
                );
                assert!(
                    !args
                        .get(4)
                        .is_some_and(|value| ["pull", "build", "login"].contains(&value.as_str()))
                );
                if args.get(5).is_some_and(|value| value == "create") {
                    assert!(args.windows(2).any(|pair| pair == ["--pull", "never"]));
                    assert!(args.windows(2).any(|pair| pair == ["--network", "none"]));
                    assert!(args.windows(2).any(|pair| pair == ["--memory", "256m"]));
                    assert!(args.iter().any(|value| value == "--read-only"));
                    assert!(
                        args.iter()
                            .any(|value| value.ends_with("target=/product,readonly"))
                    );
                }
                if args.get(5).is_some_and(|value| value == "rm") {
                    assert_eq!(args.last().map(String::len), Some(64));
                }
            }
            fixture.write("src/app.mjs", "export function answer() { return 7; }\n")?;
            assert!(report.require_current(&boundary).await.is_err());
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn docker_unavailable_metadata_and_incorrect_outputs_never_become_passing_receipts()
-> Result<(), Error> {
    let owner = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Current behavior is verified."],
        false,
    )?;
    let compiled = docker_fixture(&owner).await?;
    for mode in [
        "daemon-missing",
        "image-missing",
        "wrong-image",
        "unsafe-image-env",
        "bad-json",
        "wrong-isolation",
        "wrong-namespace",
        "writable-mount",
        "wrong-command",
        "create-no-cid",
        "invalid-output",
        "node-version",
        "mismatch",
        "exit-one",
    ] {
        let fixture = Fixture::new(
            &["src/app.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.write("RESULT.md", "Owned current result\n")?;
        let (mut approval, state) = docker_case(&fixture, &compiled, mode, &["src/app.mjs"])?;
        if mode == "node-version" {
            approval.runtime = native_docker::Runtime::Node20;
            approval
                .observations
                .get_mut(0)
                .ok_or_else(|| Error::new("Error", "Observation missing"))?
                .action = native_docker::Action::Node {
                script: native_docker::Script::Product("src/app.mjs".to_owned()),
                args: Vec::new(),
            };
        }
        if mode == "exit-one" {
            approval
                .observations
                .get_mut(0)
                .ok_or_else(|| Error::new("Error", "Observation missing"))?
                .expected_exit = 1;
        }
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let report = native_docker::run(
                &boundary,
                &native_docker::Selection::Docker(Some(Box::new(approval))),
            )
            .await?;
            assert_eq!(
                report.passed(),
                mode == "exit-one",
                "{mode}: {}",
                report.as_value().json()
            );
            assert_eq!(
                report.as_value().get("status"),
                Some(&Value::string(if mode == "exit-one" {
                    "pass"
                } else if mode == "mismatch" {
                    "fail"
                } else {
                    "unavailable"
                }))
            );
            assert_eq!(docker_objects(&state)?, 0);
            let calls = docker_calls(&state)?;
            if [
                "daemon-missing",
                "image-missing",
                "wrong-image",
                "unsafe-image-env",
                "bad-json",
            ]
            .contains(&mode)
            {
                assert!(
                    !calls
                        .iter()
                        .any(|args| args.get(5).is_some_and(|value| value == "create"))
                );
            }
            if [
                "wrong-isolation",
                "wrong-namespace",
                "writable-mount",
                "wrong-command",
            ]
            .contains(&mode)
            {
                assert!(
                    !calls
                        .iter()
                        .any(|args| args.get(5).is_some_and(|value| value == "start"))
                );
            }
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn docker_unknown_cleanup_quarantines_the_writer_and_preserves_snapshot_evidence()
-> Result<(), Error> {
    let owner = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Current behavior is verified."],
        false,
    )?;
    let compiled = docker_fixture(&owner).await?;
    for mode in [
        "wrong-owner",
        "cleanup-fail",
        "absence-unknown",
        "create-ambiguous",
        "create-wrong-cid",
    ] {
        let fixture = Fixture::new(
            &["src/app.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.write("RESULT.md", "Owned current result\n")?;
        let (approval, state) = docker_case(&fixture, &compiled, mode, &["src/app.mjs"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let result = native_docker::run(
            &boundary,
            &native_docker::Selection::Docker(Some(Box::new(approval))),
        )
        .await;
        assert!(
            result.is_err(),
            "{mode} cleanup uncertainty returned a receipt"
        );
        assert!(boundary.close().await.is_err());
        assert!(
            fixture
                .root
                .join(".roster")
                .join("locks")
                .join("run.lock")
                .exists()
        );
        assert!(
            fs::read_dir(fixture.root.join(".git").join("roster-exec"))
                .map_err(io)?
                .next()
                .is_some()
        );
        let calls = docker_calls(&state)?;
        if mode == "wrong-owner" {
            assert!(
                !calls
                    .iter()
                    .any(|args| args.get(5).is_some_and(|value| value == "rm"))
            );
        }
        for args in calls
            .iter()
            .filter(|args| args.get(5).is_some_and(|value| value == "rm"))
        {
            assert_eq!(args.last().map(String::len), Some(64));
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn docker_owned_deadline_overflow_and_cancellation_cleanup_do_not_accept_late_success()
-> Result<(), Error> {
    let owner = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Current behavior is verified."],
        false,
    )?;
    let compiled = docker_fixture(&owner).await?;
    for mode in ["timeout", "overflow", "cancel"] {
        let mut retained = docker_diagnostics::RetainedFixture::new(Fixture::new(
            &["src/app.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?);
        let fixture = retained.fixture()?;
        fixture.write("RESULT.md", "Owned current result\n")?;
        let (mut approval, state) = docker_case(fixture, &compiled, mode, &["src/app.mjs"])?;
        if mode == "timeout" {
            approval.timeout_ms = 5_000;
        }
        docker_diagnostics::Diagnostic::prepare_cli(fixture, &approval, &state).await?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let diagnostic = docker_diagnostics::Diagnostic::begin(mode)?;
        let outcome = async {
            let selection = native_docker::Selection::Docker(Some(Box::new(approval)));
            if mode == "cancel" {
                let cancel = async {
                    for _ in 0..10_000 {
                        if state.join("ready").exists() {
                            return source.cancel(github_agent_roster::cancel::Reason::Cancelled);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                    Err(Error::new("Error", "Docker cancellation barrier missing"))
                };
                let (result, cancelled) =
                    tokio::join!(native_docker::run(&boundary, &selection), cancel);
                cancelled?;
                assert!(
                    result
                        .err()
                        .is_some_and(|error| github_agent_roster::cancel::is_run_cancelled(&error))
                );
            } else {
                let report = native_docker::run(&boundary, &selection).await?;
                diagnostic.observe(&state, &report)?;
                assert!(!report.passed());
                assert_eq!(
                    report.as_value().get("status"),
                    Some(&Value::string("unavailable"))
                );
            }
            assert_eq!(docker_objects(&state)?, 0);
            assert!(
                docker_calls(&state)?
                    .iter()
                    .any(|args| args.get(5).is_some_and(|value| value == "rm"))
            );
            Ok(())
        }
        .await;
        let finished = locks::finish(outcome, boundary.close().await);
        let recorded = diagnostic.record(fixture, &state, finished.as_ref().err());
        locks::finish(finished, recorded)?;
        retained.complete();
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn docker_subject_or_task_mutation_revokes_receipts_and_still_attempts_owned_container_cleanup()
-> Result<(), Error> {
    let owner = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Current behavior is verified."],
        false,
    )?;
    let compiled = docker_fixture(&owner).await?;
    for mode in ["product-mutation", "task-mutation"] {
        let fixture = Fixture::new(
            &["src/app.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.write("RESULT.md", "Owned current result\n")?;
        let (approval, state) = docker_case(&fixture, &compiled, mode, &["src/app.mjs"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let result = native_docker::run(
            &boundary,
            &native_docker::Selection::Docker(Some(Box::new(approval))),
        )
        .await;
        assert!(result.is_err(), "{mode} returned a current receipt");
        assert_eq!(docker_objects(&state)?, 0);
        assert!(
            docker_calls(&state)?
                .iter()
                .any(|args| args.get(5).is_some_and(|value| value == "rm"))
        );
        if mode == "task-mutation" {
            assert!(boundary.close().await.is_err());
            assert_eq!(
                fs::read_to_string(fixture.root.join("TASK.md")).map_err(io)?,
                "Changed task must remain untouched by cleanup\n"
            );
        } else {
            boundary.close().await?;
        }
    }
    Ok(())
}

async fn docker_attempt(
    boundary: &tools::Boundary,
    selection: native_docker::Selection,
) -> Result<finalization::Attempt, Error> {
    let mut attempt = finalization::Attempt::begin(
        boundary,
        &config(true)?,
        finalization::Inputs {
            mode: finalization::Mode::Configured,
            capsule_version: continuity::CapsuleVersion::NativeV2,
            capsule: None,
            prior_wave_files: &[],
            initial_baseline: None,
        },
        &evidence::Projection::default(),
    )?;
    let mut backends = approval()?;
    backends.independent = selection;
    attempt.approve_backends(boundary, backends)?;
    attempt
        .context(
            boundary,
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &evidence::Projection::default(),
        )
        .await?;
    Ok(attempt)
}

#[tokio::test(flavor = "current_thread")]
async fn docker_finalization_freezes_host_plan_and_result_subject_without_replacing_existing_artifacts()
-> Result<(), Error> {
    let owner = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["Current behavior is verified."],
        false,
    )?;
    let compiled = docker_fixture(&owner).await?;
    for mode in ["pass", "missing", "scope-mismatch", "old-result"] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Scoped behavior is verified."],
            true,
        )?;
        let (mut plan, state) = docker_case(
            &fixture,
            &compiled,
            "pass",
            &["src/app.mjs", "tests/app.test.mjs"],
        )?;
        if mode == "scope-mismatch" {
            plan.files = vec!["src/app.mjs".to_owned()];
        }
        if mode == "old-result" {
            fixture.write("RESULT.md", "Previous user result must remain intact\n")?;
        }
        plan.observations
            .get_mut(0)
            .ok_or_else(|| Error::new("Error", "Observation missing"))?
            .action = native_docker::Action::Native(vec![
            "read-product".to_owned(),
            "src/app.mjs".to_owned(),
        ]);
        plan.observations
            .get_mut(0)
            .ok_or_else(|| Error::new("Error", "Observation missing"))?
            .expected_stdout = "export function answer() { return 2; }\n".to_owned();
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let selection = if mode == "missing" {
                native_docker::Selection::Docker(None)
            } else {
                native_docker::Selection::Docker(Some(Box::new(plan.clone())))
            };
            let mut attempt = docker_attempt(&boundary, selection).await?;
            plan.image = format!("sha256:{}", "b".repeat(64));
            plan.observations
                .get_mut(0)
                .ok_or_else(|| Error::new("Error", "Observation missing"))?
                .expected_stdout = "caller mutation cannot alter frozen approval".to_owned();
            review68_scoped_change(&mut attempt, &mut boundary).await?;
            assert_eq!(
                call(
                    &mut attempt,
                    &mut boundary,
                    "run_test",
                    Value::object(Vec::new())
                )
                .await?
                .get("exit_code"),
                Some(&Value::Number(0.0))
            );
            let finalized = review68_finish(attempt, &mut boundary).await?;
            if mode == "pass" {
                assert_eq!(
                    finalized.status(),
                    finalization::Status::Pass,
                    "{}",
                    finalized.report().json()
                );
                let independent = finalized
                    .independent()
                    .ok_or_else(|| Error::new("Error", "Composed independent backend missing"))?;
                assert!(independent.passed());
                assert!(independent.require_current(&boundary).await.is_err());
                assert_eq!(
                    independent.as_value().get("image"),
                    Some(&Value::string(&format!("sha256:{}", "a".repeat(64))))
                );
                let subject = independent
                    .as_value()
                    .get("subject")
                    .ok_or_else(|| Error::new("Error", "Composed subject missing"))?;
                assert_eq!(
                    subject.get("result_digest"),
                    Some(&Value::string(&format!(
                        "{:x}",
                        <sha2::Sha256 as sha2::Digest>::digest(
                            finalized.body().to_utf8()?.as_bytes()
                        )
                    )))
                );
                let written = finalized
                    .write(&boundary, &evidence::Projection::default())
                    .await?;
                assert_eq!(
                    fs::read_to_string(&written).map_err(io)?,
                    finalized.body().to_utf8()?
                );
                independent.require_current(&boundary).await?;
                let current = github_agent_roster::execution::verification::capture_subject(
                    &fixture.root,
                    &["src/app.mjs".to_owned(), "tests/app.test.mjs".to_owned()],
                )?;
                assert_eq!(&current, subject);
                fs::remove_file(&written).map_err(io)?;
                assert!(independent.require_current(&boundary).await.is_err());
                fixture.write("RESULT.md", "New user result\n")?;
                assert!(independent.require_current(&boundary).await.is_err());
                assert!(
                    finalized
                        .write(&boundary, &evidence::Projection::default())
                        .await
                        .is_err()
                );
            } else {
                assert_ne!(finalized.status(), finalization::Status::Pass, "{mode}");
                assert!(docker_calls(&state)?.is_empty());
                if mode == "old-result" {
                    assert_eq!(
                        fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                        "Previous user result must remain intact\n"
                    );
                }
            }
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

async fn recovery_index_tree(
    fixture: &Fixture,
    path: &str,
    contents: &str,
    mode: &str,
) -> Result<String, Error> {
    let payload = fixture.container.join("controlled-tree-payload");
    fs::write(&payload, contents).map_err(io)?;
    let blob = fixture
        .git(&[
            "hash-object",
            "-w",
            "--no-filters",
            "--",
            payload
                .to_str()
                .ok_or_else(|| Error::new("Error", "Controlled payload path is not Unicode"))?,
        ])?
        .trim()
        .to_owned();
    let index = fixture.container.join("controlled-index");
    let mut tree = String::new();
    for args in [
        vec!["read-tree".to_owned(), "--empty".to_owned()],
        vec![
            "update-index".to_owned(),
            "--add".to_owned(),
            "--cacheinfo".to_owned(),
            format!("{mode},{blob},{path}"),
        ],
        vec!["write-tree".to_owned()],
    ] {
        let mut command = tokio::process::Command::new("git");
        command
            .current_dir(&fixture.root)
            .env_clear()
            .envs(tools::Host::default().environment)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_CONFIG_GLOBAL", "NUL")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["--no-pager", "-c", "core.fsmonitor=false", "-c"])
            .arg(format!(
                "core.hooksPath={}",
                fixture.root.join("disabled-hooks").display()
            ))
            .args(args);
        let output = github_agent_roster::execution::owned::execute(
            command,
            github_agent_roster::process::Limits {
                timeout: Some(std::time::Duration::from_secs(30)),
                max_output_bytes: 65_536,
            },
            None,
        )
        .await
        .map_err(|failure| {
            Error::new(
                "Error",
                format!("Controlled tree construction failed: {failure}"),
            )
        })?;
        tree = String::from_utf8(output.captured.stdout)
            .map_err(|error| Error::new("Error", error.to_string()))?
            .trim()
            .to_owned();
    }
    Ok(tree)
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_green_metadata_never_grants_cross_writer_scope_secret_or_link_authority()
-> Result<(), Error> {
    for mode in [
        "new-writer",
        "other-root",
        "branch",
        "scope",
        "protected-tree",
        "secret-tree",
        "link-tree",
    ] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let green = recovery_green(&mut boundary).await?;
            recovery_node_value(&mut boundary, 2, 3).await?;
            fixture.write("RESULT.md", "Unchanged user evidence\n")?;
            let number = green
                .get("number")
                .and_then(Value::as_number)
                .map(|number| number as u64);
            let count = recovery_checkpoints::list(&boundary)?.len();
            if mode == "new-writer" || mode == "other-root" {
                let approved = recovery_restore::prepare(&boundary, number)
                    .await?
                    .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
                    .await?;
                if mode == "new-writer" {
                    boundary.close().await?;
                    boundary = fixture.open(&source, false).await?;
                    assert!(
                        recovery_restore::execute(&mut boundary, approved, &mut CurrentUnpublished)
                            .await
                            .is_err()
                    );
                } else {
                    let other = Fixture::new(
                        &["src/app.mjs", "tests/app.test.mjs"],
                        "characterization",
                        &["Current behavior is verified."],
                        true,
                    )?;
                    other.git(&["switch", "--quiet", "-c", "agents/42"])?;
                    let other_source = CancellationSource::default();
                    let mut other_boundary = other.open(&other_source, false).await?;
                    let denied = recovery_restore::execute(
                        &mut other_boundary,
                        approved,
                        &mut CurrentUnpublished,
                    )
                    .await;
                    assert!(denied.is_err());
                    other_boundary.close().await?;
                }
            } else if mode == "branch" {
                fixture.git(&["branch", "-m", "agents/43"])?;
                assert!(recovery_restore::prepare(&boundary, number).await.is_err());
            } else {
                let mut record = green.clone();
                let Value::Object(fields) = &mut record else {
                    return Err(Error::new(
                        "Error",
                        "Controlled checkpoint metadata is not an object",
                    ));
                };
                if mode == "scope" {
                    let scope = fields
                        .iter_mut()
                        .find(|(key, _)| *key == Text::from("scope"))
                        .ok_or_else(|| Error::new("Error", "Scope field missing"))?;
                    scope.1 = Value::Array(vec![Value::string("agent-policy.yml")]);
                } else {
                    let (path, contents, git_mode) = match mode {
                        "protected-tree" => {
                            ("agent-policy.yml", "human-owned policy marker\n", "100644")
                        }
                        "secret-tree" => (
                            "src/app.mjs",
                            "-----BEGIN PRIVATE KEY-----\ncontrolled fixture marker\n",
                            "100644",
                        ),
                        _ => ("src/app.mjs", "../../outside-user-file", "120000"),
                    };
                    let tree = recovery_index_tree(&fixture, path, contents, git_mode).await?;
                    let target = fields
                        .iter_mut()
                        .find(|(key, _)| *key == Text::from("tree"))
                        .ok_or_else(|| Error::new("Error", "Tree field missing"))?;
                    target.1 = Value::string(&tree);
                    let reference = green
                        .get("ref")
                        .and_then(Value::as_string)
                        .ok_or_else(|| Error::new("Error", "Checkpoint ref missing"))?
                        .to_utf8()?;
                    fixture.git(&["update-ref", &reference, &tree])?;
                }
                let path = fixture
                    .root
                    .join(".roster")
                    .join("checkpoints")
                    .join("42")
                    .join(
                        number
                            .ok_or_else(|| Error::new("Error", "Checkpoint number missing"))?
                            .to_string(),
                    );
                fs::write(path, format!("{}\n", record.json())).map_err(io)?;
                assert!(
                    recovery_restore::prepare(&boundary, number).await.is_err(),
                    "{mode} was accepted"
                );
            }
            assert_eq!(recovery_checkpoints::list(&boundary)?.len(), count);
            assert_eq!(
                fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
                "export function answer() { return 3; }\n"
            );
            assert_eq!(
                fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                "Unchanged user evidence\n"
            );
            assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[path = "support/recovery_review.rs"]
mod recovery_review;

#[path = "support/docker_diagnostics.rs"]
mod docker_diagnostics;
