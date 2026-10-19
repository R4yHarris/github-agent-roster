use std::collections::BTreeMap;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::cancel::{CancellationSource, Reason};
use crate::llm::control::Observer;
use crate::planning::{artifacts, lifecycle};
use crate::state::{files, redaction};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[path = "query_tests.rs"]
mod query_tests;

struct Fixture {
    root: PathBuf,
    assignment: lifecycle::Assignment,
    retain_on_drop: bool,
}

fn io(error: std::io::Error) -> Error {
    Error::new("Error", error.to_string())
}

impl Fixture {
    fn new(paths: &[&str], committed_base: bool) -> Result<Self, Error> {
        Self::with_object_format(paths, committed_base, None)
    }

    fn with_object_format(
        paths: &[&str],
        committed_base: bool,
        object_format: Option<&str>,
    ) -> Result<Self, Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".execution-native-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = fs::DirBuilder::new();
        builder.create(&root).map_err(io)?;
        let fixture = Self {
            root,
            retain_on_drop: false,
            assignment: lifecycle::Assignment {
                ask: Text::from(
                    format!(
                        "Change the scoped files.\n\n## Files allowed\n{}",
                        paths
                            .iter()
                            .map(|path| format!("- {path}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                    .as_str(),
                ),
                title: None,
                reference: "issue:42".to_owned(),
                task: "issue-42".to_owned(),
                session: "execution-tests".to_owned(),
            },
        };
        if committed_base {
            let output = fixture
                .git_command()
                .args(["clone", "--quiet", "--local", "--shared", "--no-checkout"])
                .arg(env!("CARGO_MANIFEST_DIR"))
                .arg(&fixture.root)
                .output()
                .map_err(io)?;
            if !output.status.success() {
                return Err(Error::new("Error", tools::bytes_text(&output.stderr)?));
            }
        } else {
            match object_format {
                None => fixture.git(&["init", "--quiet"])?,
                Some(format) => {
                    fixture.git(&["init", "--quiet", &format!("--object-format={format}")])?
                }
            };
        }
        fixture.write(
            ".gitignore",
            ".roster/\n.roster-state/\ntarget/\ncargo-output/\n",
        )?;
        let plan = artifacts::build(
            &Value::String(fixture.assignment.ask.clone()),
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
        fixture.write("TASK.md", &plan.task.to_utf8()?)?;
        fixture.write("RECIPE.yml", &plan.recipe.to_utf8()?)?;
        fixture.write("AGENTS.md", "# Repository\n\n## Coding\n\n- Keep the boundary strict.\n\n## Publication\n\nNever git push from the seat.\n")?;
        fixture.write(
            "principals/coder.md",
            "Act only on the task; keep evidence truthful.\n",
        )?;
        fixture.write(
            "principals/reviewer.md",
            "Review independently and read only.\n",
        )?;
        for name in [
            "implement-task",
            "run-tests",
            "read-before-write",
            "small-diff",
            "result-report",
        ] {
            let mut packet = format!("# {name}\n");
            for index in 0..50 {
                writeln!(&mut packet, "Instruction line {index}.").map_err(|error| {
                    Error::new("Error", format!("Format owned skill fixture: {error}"))
                })?;
            }
            fixture.write(&format!("skills/{name}/SKILL.md"), &packet)?;
        }
        Ok(fixture)
    }

    fn write(&self, file: &str, content: &str) -> Result<(), Error> {
        let file = self.root.join(file);
        files::create_private_directories(
            file.parent()
                .ok_or_else(|| Error::type_error("Fixture path needs a parent"))?,
        )?;
        fs::write(&file, content).map_err(io)
    }

    fn git(&self, args: &[&str]) -> Result<String, Error> {
        let output = self.git_command().args(args).output().map_err(io)?;
        if !output.status.success() {
            return Err(Error::new("Error", tools::bytes_text(&output.stderr)?));
        }
        tools::bytes_text(&output.stdout)
    }

    fn git_command(&self) -> Command {
        let mut command = Command::new("git");
        command
            .env_clear()
            .envs(tools::Host::default().environment)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["--no-pager", "-c", "init.templateDir=", "-c"])
            .arg(format!(
                "core.hooksPath={}",
                self.root.join("disabled-fixture-hooks").display()
            ))
            .current_dir(&self.root);
        command
    }

    async fn open(
        &self,
        options: tools::Options,
        host: tools::Host,
        source: &CancellationSource,
    ) -> Result<tools::Boundary, Error> {
        tools::Boundary::open(
            &self.root,
            self.assignment.clone(),
            options,
            host,
            redaction::Config::default(),
            source.token(),
        )
        .await
    }

    fn node_tests(&self, source: &str) -> Result<(), Error> {
        self.write("src/subject.mjs", "export const value = 42;\n")?;
        self.write("tests/subject.topic.test.mjs", source)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.retain_on_drop {
            eprintln!("Owned Git-query fixture retained: {}", self.root.display());
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn read(file: &str) -> Value {
    Value::object(vec![("path", Value::string(file))])
}
fn write(file: &str, content: &str) -> Value {
    Value::object(vec![
        ("path", Value::string(file)),
        ("content", Value::string(content)),
    ])
}

fn error(result: Result<Value, Error>) -> Result<Error, Error> {
    match result {
        Err(error) => Ok(error),
        Ok(value) => Err(Error::new(
            "Error",
            format!("Unexpected success: {}", value.json()),
        )),
    }
}

#[test]
fn scoped_test_selection_preserves_shards_and_docs() -> Result<(), Error> {
    let scope = vec![
        "src/subject.mjs".to_owned(),
        "tests/subject.topic.test.mjs".to_owned(),
    ];
    let candidates = policy::relevant_tests(&scope)?;
    assert_eq!(
        candidates,
        ["tests/subject.test.mjs", "tests/subject.topic.test.mjs"]
    );
    let available = vec![
        "tests/subject.test.mjs".to_owned(),
        "tests/subject.topic.test.mjs".to_owned(),
        "tests/subject.other.test.mjs".to_owned(),
    ];
    assert_eq!(
        policy::expand_shards(&candidates, &available, &scope)?,
        ["tests/subject.topic.test.mjs"]
    );
    assert!(policy::covers_test(
        &["tests/subject.test.mjs".to_owned()],
        "tests/subject.other.test.mjs"
    )?);
    assert!(policy::docs_only(&["docs/guide.md".to_owned()])?);
    assert!(!policy::docs_only(&["docs/nested/guide.md".to_owned()])?);
    assert!(policy::requires_native(&["src/port.rs".to_owned()])?);
    assert!(policy::requires_native(&["nested/Cargo.lock".to_owned()])?);
    assert_eq!(policy::test_concurrency(7), 3);
    Ok(())
}

#[test]
fn required_context_cannot_be_truncated_or_count_unicode_scalars() -> Result<(), Error> {
    let section = context::Section::required("Rules", Text(vec![0xd800, 0xd83d, 0xde00]));
    let (pack, truncated) = context::bounded_pack(std::slice::from_ref(&section), 1000, false)?;
    assert!(!truncated);
    let (exact, _) = context::bounded_pack(std::slice::from_ref(&section), pack.0.len(), false)?;
    assert_eq!(pack, exact);
    assert!(context::bounded_pack(&[section], pack.0.len() - 1, false).is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn write_requires_current_read_and_captures_real_git_tree_before_mutation()
-> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write("src/subject.mjs", "export const value = 1;\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    assert_eq!(
        error(
            boundary
                .execute(
                    "write_file",
                    &write("src/subject.mjs", "export const value = 2;\n")
                )
                .await
        )?
        .name,
        "ToolUsageError"
    );
    boundary
        .execute("read_file", &read("src/subject.mjs"))
        .await?;
    boundary
        .execute(
            "write_file",
            &write("src/subject.mjs", "export const value = 2;"),
        )
        .await?;
    assert_eq!(
        fs::read_to_string(fixture.root.join("src/subject.mjs")).map_err(io)?,
        "export const value = 2;\n"
    );
    let record = boundary
        .first_checkpoint()
        .ok_or_else(|| Error::new("Error", "Missing pre-write checkpoint"))?;
    let tree = text(required(record, "tree")?, "Missing tree")?;
    assert_eq!(
        fixture.git(&["show", &format!("{tree}:src/subject.mjs")])?,
        "export const value = 1;\n"
    );
    assert_eq!(checkpoints::list(&boundary)?.len(), 1);
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn replacement_with_same_bytes_invalidates_the_read_file_identity() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write("src/subject.mjs", "export const value = 1;\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    boundary
        .execute("read_file", &read("src/subject.mjs"))
        .await?;
    fs::rename(
        fixture.root.join("src/subject.mjs"),
        fixture.root.join("src/previous.mjs"),
    )
    .map_err(io)?;
    fixture.write("src/subject.mjs", "export const value = 1;\n")?;
    assert_eq!(
        error(
            boundary
                .execute(
                    "write_file",
                    &write("src/subject.mjs", "export const value = 2;\n")
                )
                .await
        )?
        .name,
        "ToolUsageError"
    );
    assert!(boundary.first_checkpoint().is_none());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn read_before_write_does_not_break_targeted_edits_or_docs() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs", "docs/guide.md"], false)?;
    fixture.write("src/subject.mjs", "export const value = 1;\r\n")?;
    fixture.write("docs/guide.md", "Old\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    boundary
        .execute(
            "edit_file",
            &Value::object(vec![
                ("path", Value::string("src/subject.mjs")),
                ("old_string", Value::string("value = 1")),
                ("new_string", Value::string("value = 2")),
            ]),
        )
        .await?;
    boundary
        .execute("write_file", &write("docs/guide.md", "New\n"))
        .await?;
    assert!(
        fs::read_to_string(fixture.root.join("src/subject.mjs"))
            .map_err(io)?
            .contains("value = 2")
    );
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn hard_linked_reads_edits_and_deletes_are_denied() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write("src/subject.mjs", "export const value = 1;\n")?;
    fs::hard_link(
        fixture.root.join("src/subject.mjs"),
        fixture.root.join("other.mjs"),
    )
    .map_err(io)?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    for name in ["read_file", "delete_file"] {
        assert_eq!(
            error(boundary.execute(name, &read("src/subject.mjs")).await)?.code,
            Some("ROSTER_TOOL_DENIED")
        );
    }
    assert!(boundary.first_checkpoint().is_none());
    assert_eq!(
        fs::read_to_string(fixture.root.join("other.mjs")).map_err(io)?,
        "export const value = 1;\n"
    );
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn protected_paths_are_denied_even_under_broad_scope_or_expansion() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let options = tools::Options {
        scope_expansion: 16,
        slice_reads_only: false,
        ..tools::Options::default()
    };
    let mut boundary = fixture
        .open(
            options,
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    for file in [
        "../escape.mjs",
        "vendor/source.mjs",
        "nested/agent-policy.yml",
        ".env.local",
        "nested/key.pem",
        ".github/workflows/test.yml",
        "TASK.md",
        "RECIPE.yml",
        "RESULT.md",
        ".roster/history.jsonl",
        ".roster-state/shared/private.json",
        "src/subject.mjs:alternate",
        "src/ambiguous.",
    ] {
        assert!(
            boundary
                .execute("write_file", &write(file, "content\n"))
                .await
                .is_err(),
            "{file}"
        );
    }
    assert!(boundary.scope_files().is_empty());
    assert!(boundary.first_checkpoint().is_none());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn expansion_is_capped_recorded_and_included_in_checkpoint_scope() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let mut boundary = fixture
        .open(
            tools::Options {
                scope_expansion: 1,
                ..tools::Options::default()
            },
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let result = boundary
        .execute(
            "write_file",
            &write("src/extra.mjs", "export const extra = 1;\n"),
        )
        .await?;
    assert_eq!(result.get("scope_expanded"), Some(&Value::Boolean(true)));
    assert_eq!(boundary.scope_files(), ["src/extra.mjs"]);
    assert!(
        boundary
            .first_checkpoint()
            .and_then(|record| record.get("scope"))
            .is_some_and(|scope| scope.json().contains("src/extra.mjs"))
    );
    assert!(
        boundary
            .execute(
                "write_file",
                &write("src/another.mjs", "export const extra = 2;\n")
            )
            .await
            .is_err()
    );
    assert!(!fixture.root.join("src/another.mjs").exists());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn scope_continuation_never_drops_excess_paths() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let options = tools::Options {
        scope_expansion: 1,
        initial_scope_files: vec!["a.mjs".to_owned(), "b.mjs".to_owned()],
        ..tools::Options::default()
    };
    assert!(
        fixture
            .open(
                options,
                tools::Host::default(),
                &CancellationSource::default()
            )
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn new_modules_require_prior_wave_reads_without_granting_writes() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write("src/earlier.mjs", "export const earlier = 1;\n")?;
    let mut boundary = fixture
        .open(
            tools::Options {
                required_reads: vec!["src/earlier.mjs".to_owned()],
                ..tools::Options::default()
            },
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    assert!(
        boundary
            .execute(
                "write_file",
                &write("src/subject.mjs", "export const value = 1;\n")
            )
            .await
            .is_err()
    );
    boundary
        .execute("read_file", &read("src/earlier.mjs"))
        .await?;
    boundary
        .execute(
            "write_file",
            &write("src/subject.mjs", "export const value = 1;\n"),
        )
        .await?;
    assert!(
        boundary
            .execute("delete_file", &read("src/earlier.mjs"))
            .await
            .is_err()
    );
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn slice_search_is_scoped_but_direct_imports_are_readable() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write(
        "src/subject.mjs",
        "import { value } from './helper.mjs';\nexport { value };\n",
    )?;
    fixture.write("src/helper.mjs", "export const value = 1;\n")?;
    fixture.write("src/unrelated.mjs", "export const unrelated = 1;\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    assert!(
        boundary
            .execute("read_file", &read("src/helper.mjs"))
            .await
            .is_ok()
    );
    assert!(
        boundary
            .execute("read_file", &read("src/unrelated.mjs"))
            .await
            .is_err()
    );
    let result = boundary
        .execute(
            "search_text",
            &Value::object(vec![("query", Value::string("export"))]),
        )
        .await?;
    assert!(!result.json().contains("unrelated"));
    boundary.close().await
}

struct CancelEvent {
    source: CancellationSource,
    event: &'static str,
}

impl Observer for CancelEvent {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if domain::is_text(event.get("type"), self.event) {
            self.source.cancel(Reason::Cancelled)?;
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_after_expansion_prevents_the_first_product_side_effect() -> Result<(), Error>
{
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let source = CancellationSource::default();
    let mut boundary = fixture
        .open(
            tools::Options {
                scope_expansion: 1,
                ..tools::Options::default()
            },
            tools::Host::default(),
            &source,
        )
        .await?;
    let mut observer = CancelEvent {
        source,
        event: "scope-expansion",
    };
    assert_eq!(
        error(
            boundary
                .execute_with(
                    "write_file",
                    &write("src/extra.mjs", "export const value = 1;\n"),
                    &mut observer
                )
                .await
        )?
        .code,
        Some("ROSTER_CANCELLED")
    );
    assert!(!fixture.root.join("src/extra.mjs").exists());
    assert!(boundary.first_checkpoint().is_none());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_after_a_write_revokes_success_but_preserves_actual_edits() -> Result<(), Error>
{
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let source = CancellationSource::default();
    let mut boundary = fixture
        .open(tools::Options::default(), tools::Host::default(), &source)
        .await?;
    let mut observer = CancelEvent {
        source,
        event: "tool-result",
    };
    assert_eq!(
        error(
            boundary
                .execute_with(
                    "write_file",
                    &write("src/subject.mjs", "export const value = 1;\n"),
                    &mut observer
                )
                .await
        )?
        .code,
        Some("ROSTER_CANCELLED")
    );
    assert!(fixture.root.join("src/subject.mjs").is_file());
    assert!(boundary.last_tests().is_none());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn an_active_writer_blocks_every_other_boundary_in_the_worktree() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let other = fixture
        .open(
            tools::Options {
                role: Role::Reviewer,
                ..tools::Options::default()
            },
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await;
    assert!(other.is_err_and(|error| error.code == Some("E_LOCK_HELD")));
    boundary.close().await?;
    let mut next = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    next.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn reviewer_authority_cannot_reach_writes_tests_shell_or_network() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write("src/subject.mjs", "export const value = 1;\n")?;
    let mut boundary = fixture
        .open(
            tools::Options {
                role: Role::Reviewer,
                ..tools::Options::default()
            },
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    assert_eq!(
        boundary.tool_names(),
        ["read_file", "search_text", "git_diff"]
    );
    for name in [
        "write_file",
        "edit_file",
        "delete_file",
        "run_test",
        "run_command",
        "web_fetch",
        "web_search",
        "list_dir",
    ] {
        assert_eq!(
            error(boundary.execute(name, &Value::object(Vec::new())).await)?.code,
            Some("ROSTER_TOOL_DENIED")
        );
    }
    assert!(
        boundary
            .execute("read_file", &read("src/subject.mjs"))
            .await
            .is_ok()
    );
    assert!(!boundary.definitions().json().contains("write_file"));
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn task_recipe_or_assignment_replacement_revokes_the_boundary() -> Result<(), Error> {
    for name in ["TASK.md", "RECIPE.yml", "ASSIGNMENT.md"] {
        let fixture = Fixture::new(&["src/subject.mjs"], false)?;
        let mut boundary = fixture
            .open(
                tools::Options::default(),
                tools::Host::default(),
                &CancellationSource::default(),
            )
            .await?;
        fixture.write(name, "Externally replaced\n")?;
        assert_eq!(
            error(boundary.execute("read_file", &read("TASK.md")).await)?.code,
            Some("ROSTER_TOOL_DENIED")
        );
        boundary.close().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn structured_calls_are_bounded_deduplicated_and_authority_filtered() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let call = Value::object(vec![
        ("id", Value::string("read-1")),
        ("type", Value::string("function")),
        (
            "function",
            Value::object(vec![
                ("name", Value::string("read_file")),
                ("arguments", Value::string("{\"path\":\"TASK.md\"}")),
            ]),
        ),
    ]);
    let message = Value::object(vec![("tool_calls", Value::Array(vec![call.clone()]))]);
    assert_eq!(tools::parse_calls(&boundary, &message, 1, &[])?.len(), 1);
    assert!(tools::parse_calls(&boundary, &message, 1, &["read-1".to_owned()]).is_err());
    assert!(
        tools::parse_calls(
            &boundary,
            &Value::object(vec![("tool_calls", Value::Array(vec![call; 9]))]),
            1,
            &[]
        )
        .is_err()
    );
    assert!(
        boundary
            .execute(
                "run_command",
                &Value::object(vec![(
                    "argv",
                    Value::Array(vec![
                        Value::string("git"),
                        Value::string("diff"),
                        Value::string("--ext-diff")
                    ])
                )])
            )
            .await
            .is_err()
    );
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn rust_deletions_never_fall_back_to_a_passing_node_suite() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs", "rust/port.rs"], false)?;
    fixture.write("rust/port.rs", "pub fn value() -> u8 {\n    1\n}\n")?;
    let host = tools::Host {
        node20: None,
        ..tools::Host::default()
    };
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            host,
            &CancellationSource::default(),
        )
        .await?;
    boundary
        .execute("delete_file", &read("rust/port.rs"))
        .await?;
    let error = error(
        boundary
            .execute("run_test", &Value::object(Vec::new()))
            .await,
    )?;
    assert_eq!(error.message, policy::NATIVE_TEST_REQUIRED);
    assert!(!fixture.root.join("rust/port.rs").exists());
    assert!(boundary.last_tests().is_none());
    boundary.close().await
}

const REAL_NODE_TEST: &str = "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { value } from '../src/subject.mjs';\ntest('actual product behavior and clean Node 20 environment', () => {\n  assert.equal(value, 42);\n  assert.equal(process.versions.node.split('.')[0], '20');\n  assert.equal(process.env.GITHUB_APP_ID, undefined);\n  assert.equal(process.env.GH_TOKEN, undefined);\n  assert.equal(process.env.NODE_OPTIONS, undefined);\n});\n";

#[tokio::test(flavor = "current_thread")]
async fn actual_node20_scoped_shards_produce_bound_test_evidence_and_green_checkpoint()
-> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], true)?;
    fixture.node_tests(REAL_NODE_TEST)?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let tests = boundary.verify(false).await?;
    assert_eq!(tests.get("exit_code"), Some(&Value::Number(0.0)));
    assert!(tests.get("skipped").is_none());
    assert!(tests.json().contains("actual product behavior"));
    let green = checkpoints::capture(&boundary, true).await?;
    assert_eq!(green.get("status"), Some(&Value::string("verified")));
    assert_eq!(checkpoints::validate_green(&boundary, None).await?, green);
    fixture.write("src/subject.mjs", "export const value = 0;\n")?;
    assert!(checkpoints::capture(&boundary, true).await.is_err());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn native_green_evidence_cannot_be_rebound_to_another_branch() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], true)?;
    fixture.node_tests(REAL_NODE_TEST)?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    boundary.verify(false).await?;
    fixture.git(&["branch", "execution-other", "HEAD"])?;
    fixture.git(&["symbolic-ref", "HEAD", "refs/heads/execution-other"])?;
    assert!(
        checkpoints::capture(&boundary, true)
            .await
            .is_err_and(|error| error.message.contains("revision changed"))
    );
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_real_test_is_not_a_green_checkpoint() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], true)?;
    fixture.node_tests(REAL_NODE_TEST)?;
    fixture.write("src/subject.mjs", "export const value = 0;\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let tests = boundary.verify(false).await?;
    assert_eq!(tests.get("exit_code"), Some(&Value::Number(1.0)));
    assert!(checkpoints::capture(&boundary, true).await.is_err());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn actual_approved_cargo_checks_run_and_observed_failure_remains_failure() -> Result<(), Error>
{
    let fixture = Fixture::new(
        &["Cargo.toml", "Cargo.lock", "src/lib.rs", "src/subject.mjs"],
        true,
    )?;
    fixture.write("Cargo.toml", "[package]\nname = \"execution-owned-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\nrust-version = \"1.85\"\n")?;
    fixture.write(
        "Cargo.lock",
        "version = 4\n\n[[package]]\nname = \"execution-owned-fixture\"\nversion = \"0.0.0\"\n",
    )?;
    fixture.write("src/lib.rs", "pub fn answer() -> u8 {\n    42\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn actual_behavior() {\n        assert_eq!(super::answer(), 42);\n    }\n}\n")?;
    fixture.node_tests(REAL_NODE_TEST)?;
    let mut environment = std::env::vars()
        .filter(|(name, _)| {
            ["CARGO_HOME", "RUSTUP_HOME", "RUSTFLAGS", "RUSTUP_TOOLCHAIN"].contains(&name.as_str())
        })
        .collect::<BTreeMap<_, _>>();
    if let Ok(toolchain) = std::env::var("ROSTER_TEST_CARGO_TOOLCHAIN") {
        environment.insert("RUSTUP_TOOLCHAIN".to_owned(), toolchain);
    }
    environment.insert(
        "CARGO_TARGET_DIR".to_owned(),
        fixture.root.join("cargo-output").display().to_string(),
    );
    let host = tools::Host {
        cargo: Some(tools::CargoApproval {
            executable: PathBuf::from("cargo"),
            environment,
        }),
        ..tools::Host::default()
    };
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            host,
            &CancellationSource::default(),
        )
        .await?;
    let tests = boundary.verify(false).await?;
    assert_eq!(tests.get("exit_code"), Some(&Value::Number(0.0)));
    assert_eq!(tests.get("native"), Some(&Value::Boolean(true)));
    assert_eq!(
        tests
            .get("commands")
            .and_then(domain::items)
            .map(<[Value]>::len),
        Some(4)
    );
    assert!(tests.json().contains("actual_behavior"));
    assert!(tests.json().contains("actual product behavior"));
    checkpoints::capture(&boundary, true).await?;
    fixture.write("src/lib.rs", "pub fn answer()->u8{42}\n")?;
    let failed = boundary.verify(false).await?;
    assert_eq!(failed.get("exit_code"), Some(&Value::Number(1.0)));
    assert!(checkpoints::capture(&boundary, true).await.is_err());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn docs_skip_is_explicit_and_cannot_create_a_green_checkpoint() -> Result<(), Error> {
    let fixture = Fixture::new(&["README.md"], false)?;
    fixture.write("README.md", "# Example\n\n## Status\nExperimental.\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let result = boundary.verify(false).await?;
    assert_eq!(result.get("skipped"), Some(&Value::Boolean(true)));
    assert!(checkpoints::capture(&boundary, true).await.is_err());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn full_context_delivers_root_rules_all_declared_skills_and_no_automatic_history()
-> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write(
        ".roster/history.jsonl",
        "{\"summary\":\"raw history must not become context\"}\n",
    )?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let config =
        crate::config::parse(&Text::from(include_str!("../../roster.config.example.yml")))?;
    let context = context::load(
        &boundary,
        &fixture.root,
        &config,
        context::Options::default(),
    )
    .await?;
    let pack = context.pack.to_utf8()?;
    assert!(pack.contains("Keep the boundary strict."));
    assert!(pack.contains(context::OMITTED_RULES));
    assert!(pack.contains("Instruction line 49."));
    assert_eq!(context.skills.len(), 5);
    assert!(!pack.contains("raw history must not become context"));
    context::write(&boundary, &context)?;
    assert!(context::write(&boundary, &context).is_err());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn selected_skills_and_rust_guidance_cannot_be_silently_missing() -> Result<(), Error> {
    let fixture = Fixture::new(&["rust/port.rs"], false)?;
    let source = fs::read_to_string(fixture.root.join("TASK.md"))
        .map_err(io)?
        .replace("result-report]", "result-report, rust-implementation]");
    fixture.write("TASK.md", &source)?;
    fixture.write(
        "skills/rust-implementation/SKILL.md",
        "Read the complete Rust guidance.\n",
    )?;
    fixture.write("skills/rust-cargo/SKILL.md", "Run actual Cargo checks.\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let config =
        crate::config::parse(&Text::from(include_str!("../../roster.config.example.yml")))?;
    assert!(
        context::load(
            &boundary,
            &fixture.root,
            &config,
            context::Options::default()
        )
        .await
        .is_err_and(|error| error.message.contains("guidance is missing"))
    );
    fixture.write(
        ".github/instructions/rust.instructions.md",
        "No unsafe code; retain unknown usage.\n",
    )?;
    fixture.write(
        ".github/instructions/cargo.instructions.md",
        "Lock dependencies and remain offline.\n",
    )?;
    let context = context::load(
        &boundary,
        &fixture.root,
        &config,
        context::Options::default(),
    )
    .await?;
    assert!(
        context
            .pack
            .to_utf8()?
            .contains("No unsafe code; retain unknown usage.")
    );
    boundary.close().await
}

#[test]
fn continuation_keeps_exact_obligations_and_invalidates_unverified_done() -> Result<(), Error> {
    let fixture = Fixture::new(&["README.md"], false)?;
    let task = Text::from(
        fs::read_to_string(fixture.root.join("TASK.md"))
            .map_err(io)?
            .as_str(),
    );
    let checks = checklist::authoritative_checks(&task)?;
    let mut item = checklist::create(&checks, true);
    item = checklist::update(
        &item,
        &Value::object(vec![(
            "items",
            Value::Array(vec![Value::object(vec![
                ("id", Value::Number(1.0)),
                ("status", Value::string("done")),
                ("evidence", Value::string("README.md")),
            ])]),
        )]),
    )?;
    if let Value::Object(fields) = &mut item {
        if let Some((_, Value::Array(items))) = fields
            .iter_mut()
            .find(|(name, _)| *name == Text::from("items"))
        {
            if let Some(item) = items.first_mut() {
                domain::put(item, "evidenceRef", Value::Number(1.0))?;
            }
        }
    }
    let source = "a".repeat(64);
    let observed = Value::object(vec![
        ("id", Value::Number(1.0)),
        ("tool", Value::string("read_file")),
        ("reference", Value::string("README.md")),
        ("source", Value::string(&source)),
        ("verdict", Value::string("observed")),
    ]);
    let capsule = checklist::export_continuation(
        &Value::object(vec![
            ("checklist", item),
            ("source", Value::string(&source)),
            ("outcome", Value::string("failed")),
            ("observations", Value::Array(vec![observed])),
        ]),
        &task,
        &fixture.root,
    )?;
    let restored = checklist::import_continuation(
        &capsule,
        &task,
        &fixture.root,
        &checks,
        &source,
        &redaction::Config::default(),
    )?;
    assert!(restored.json().contains("invalidated"));
    assert_eq!(
        checklist::open(required(&restored, "checklist")?)?.len(),
        checks.len()
    );
    assert!(
        checklist::import_continuation(
            &capsule,
            &task,
            &fixture.root,
            &checks,
            &"b".repeat(64),
            &redaction::Config::default()
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn native_and_skipped_test_observations_never_claim_node_evidence() -> Result<(), Error> {
    for field in ["native", "skipped"] {
        let input = Value::object(vec![
            ("tool", Value::string("run_test")),
            (
                "result",
                Value::object(vec![
                    ("exit_code", Value::Number(0.0)),
                    (field, Value::Boolean(true)),
                ]),
            ),
        ]);
        assert!(checklist::observation(&input, &redaction::Config::default()).is_err());
    }
    Ok(())
}

#[test]
fn independent_subject_binds_actual_missing_and_changed_product_bytes() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write("RESULT.md", "Observed fixture result.\n")?;
    fixture.write("src/subject.mjs", "export const value = 1;\n")?;
    let files = vec!["src/subject.mjs".to_owned(), "src/deleted.mjs".to_owned()];
    let subject = verification::capture_subject(&fixture.root, &files)?;
    let digest = text(required(&subject, "digest")?, "Missing subject digest")?;
    let receipt = Value::object(vec![
        ("schema_version", Value::Number(1.0)),
        ("subject_digest", Value::string(&digest)),
        (
            "observations",
            Value::Array(vec![Value::object(vec![
                ("id", Value::Number(1.0)),
                ("check_id", Value::Number(1.0)),
                ("expected_output", Value::string("1\n")),
                ("actual_output", Value::string("1\n")),
                ("expected_exit", Value::Number(0.0)),
                ("actual_exit", Value::Number(0.0)),
                ("evidence", Value::string("Owned fixture output")),
            ])]),
        ),
    ]);
    assert_eq!(
        verification::judge_current_receipt(
            &receipt,
            &subject,
            &fixture.root,
            &files,
            1,
            &CancellationSource::default().token()
        )?
        .get("status"),
        Some(&Value::string("pass"))
    );
    fixture.write("src/subject.mjs", "export const value = 2;\n")?;
    assert!(
        verification::judge_current_receipt(
            &receipt,
            &subject,
            &fixture.root,
            &files,
            1,
            &CancellationSource::default().token()
        )
        .is_err()
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn deadline_keeps_ownership_until_cleanup_and_rejects_late_success() -> Result<(), Error> {
    let source = CancellationSource::default();
    let closed = std::cell::Cell::new(false);
    let closed_ref = &closed;
    let result = controls::with_task_deadline(&source.token(), Some(10), |token| async move {
        token.cancelled().await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        closed_ref.set(true);
        Ok(Value::Boolean(true))
    })
    .await;
    assert!(closed.get());
    assert_eq!(error(result)?.code, Some("ROSTER_TASK_TIMEOUT"));
    source.token().check()?;
    Ok(())
}

#[test]
fn steering_only_interrupts_the_owned_request_and_preserves_first_failure() -> Result<(), Error> {
    let source = CancellationSource::default();
    let mut steering = controls::Steering::new(source.token());
    assert!(steering.steer(&Text::from("Keep scope unchanged")).is_err());
    let token = steering.begin()?;
    steering.steer(&Text::from("  Keep scope unchanged  "))?;
    assert_eq!(token.reason()?, Some(Reason::Cancelled));
    assert_eq!(
        steering
            .finish::<Value>(Ok(Value::Boolean(true)))
            .err()
            .and_then(|error| error.code),
        Some("ROSTER_STEERING")
    );
    assert_eq!(steering.take(), Some(Text::from("Keep scope unchanged")));
    assert!(steering.take().is_none());
    steering.begin()?;
    steering.steer(&Text::from("Keep scope unchanged"))?;
    assert_eq!(
        steering
            .finish::<Value>(Err(Reason::TaskTimeout(10).error()))
            .err()
            .and_then(|error| error.code),
        Some("ROSTER_TASK_TIMEOUT")
    );
    Ok(())
}

#[test]
fn not_integrated_gates_never_default_to_pass_or_acquire_images() {
    assert!(require_complete_seat_gates().is_err());
    assert!(checkpoints::require_restore_integration().is_err());
    assert!(verification::require_docker_executor(false, "").is_err());
    assert!(
        verification::require_docker_executor(true, &format!("sha256:{}", "a".repeat(64)))
            .is_err_and(|error| error.code == Some("ROSTER_NATIVE_UNSUPPORTED"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn passing_tests_that_modify_unplanned_files_do_not_produce_green_evidence()
-> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], true)?;
    fixture.node_tests("import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { writeFileSync } from 'node:fs';\nimport { value } from '../src/subject.mjs';\ntest('actual product with an out-of-scope side effect', () => {\n  writeFileSync(new URL('../outside.md', import.meta.url), 'Changed\\n');\n  assert.equal(value, 42);\n});\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let failure = error(boundary.verify(false).await)?;
    assert!(
        failure
            .message
            .contains("Worktree product or protected metadata changed")
    );
    assert!(fixture.root.join("outside.md").is_file());
    assert!(boundary.last_tests().is_none());
    assert!(checkpoints::capture(&boundary, true).await.is_err());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn protected_metadata_changes_during_tests_revoke_success_without_reading_secret_bodies()
-> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], true)?;
    fixture.write(".env", "OWNED_FIXTURE=before\n")?;
    fixture.node_tests("import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { writeFileSync } from 'node:fs';\nimport { value } from '../src/subject.mjs';\ntest('actual product with a protected fixture side effect', () => {\n  writeFileSync(new URL('../.env', import.meta.url), 'OWNED_FIXTURE=after\\n');\n  assert.equal(value, 42);\n});\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    assert!(
        boundary
            .verify(false)
            .await
            .is_err_and(|error| error.message.contains("protected metadata changed"))
    );
    assert!(boundary.last_tests().is_none());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn full_node_verification_checks_planned_syntax_before_accepting_other_tests()
-> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs", "src/unimported.mjs"], true)?;
    fixture.node_tests(REAL_NODE_TEST)?;
    fixture.write("src/unimported.mjs", "export const broken = ;\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let result = boundary.verify(true).await?;
    assert_eq!(result.get("exit_code"), Some(&Value::Number(1.0)));
    assert_eq!(result.get("syntax_check"), Some(&Value::Boolean(true)));
    assert_eq!(
        result.get("failing_files"),
        Some(&Value::Array(vec![Value::string("src/unimported.mjs")]))
    );
    assert!(checkpoints::capture(&boundary, true).await.is_err());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn nested_planned_node_test_files_are_not_silently_skipped() -> Result<(), Error> {
    let fixture = Fixture::new(&["tests/nested/behavior.test.mjs"], true)?;
    fixture.write("tests/nested/behavior.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('owned nested test', () => assert.equal(process.versions.node.split('.')[0], '20'));\n")?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let result = boundary.verify(false).await?;
    assert_eq!(result.get("exit_code"), Some(&Value::Number(0.0)));
    assert!(result.get("skipped").is_none());
    assert!(result.json().contains("owned nested test"));
    boundary.close().await
}

#[test]
fn disposed_steering_request_cannot_accept_an_uncooperative_late_result() -> Result<(), Error> {
    let mut steering = controls::Steering::new(CancellationSource::default().token());
    let token = steering.begin()?;
    steering.dispose()?;
    assert!(token.check().is_err());
    assert!(
        steering
            .finish(Ok(Value::Boolean(true)))
            .is_err_and(|error| error.code == Some("ROSTER_CANCELLED"))
    );
    assert!(steering.finish(Ok(Value::Boolean(true))).is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_state_is_not_created_before_its_ignore_gate_passes() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write(".gitignore", "")?;
    assert!(
        fixture
            .open(
                tools::Options::default(),
                tools::Host::default(),
                &CancellationSource::default()
            )
            .await
            .is_err_and(|error| error.message.contains("Git-ignored"))
    );
    assert!(!fixture.root.join(".roster").exists());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn replacing_the_git_marker_revokes_ownership_even_when_product_bytes_are_unchanged()
-> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    fs::rename(fixture.root.join(".git"), fixture.root.join("previous-git")).map_err(io)?;
    fs::create_dir(fixture.root.join(".git")).map_err(io)?;
    assert!(
        boundary
            .execute("read_file", &read("TASK.md"))
            .await
            .is_err_and(|error| error
                .message
                .contains("Git worktree marker identity changed"))
    );
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn recorded_expansions_and_repairs_reach_read_only_review_scope() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    fixture.write("src/extra.mjs", "export const extra = 1;\n")?;
    let options = tools::Options {
        role: Role::Reviewer,
        initial_scope_files: vec!["src/extra.mjs".to_owned()],
        initial_repair_files: vec!["tests/subject.topic.test.mjs".to_owned()],
        ..tools::Options::default()
    };
    let mut boundary = fixture
        .open(
            options,
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    assert!(boundary.scope()?.contains(&"src/extra.mjs".to_owned()));
    assert!(
        boundary
            .scope()?
            .contains(&"tests/subject.topic.test.mjs".to_owned())
    );
    assert!(
        boundary
            .execute("read_file", &read("src/extra.mjs"))
            .await
            .is_ok()
    );
    assert!(
        boundary
            .execute("write_file", &write("src/extra.mjs", "changed\n"))
            .await
            .is_err()
    );
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn real_owned_node_cancellation_is_not_reported_as_descendant_cleanup_proof()
-> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let source = CancellationSource::default();
    let host = tools::Host::default();
    let node = host
        .node20
        .as_ref()
        .cloned()
        .ok_or_else(|| Error::new("Error", "Node 20 is required for owned process controls"))?;
    let mut boundary = fixture
        .open(tools::Options::default(), host, &source)
        .await?;
    let pid_path = fixture.root.join("owned-node.pid");
    let path = pid_path
        .to_str()
        .ok_or_else(|| Error::type_error("Fixture path must be Unicode"))?;
    let script = format!(
        "const fs=require('node:fs');const p={};fs.writeFileSync(p+'.writing',String(process.pid));fs.renameSync(p+'.writing',p);setInterval(()=>{{}},1000)",
        Text::from(path).json()
    );
    let args = vec!["--eval".to_owned(), script];
    let environment = BTreeMap::new();
    let (result, pid) = tokio::join!(
        boundary.process(&node, &args, &environment, 10_000, 1024),
        async {
            for _ in 0..200 {
                if let Ok(pid) = fs::read_to_string(&pid_path) {
                    let pid = pid
                        .parse::<u64>()
                        .map_err(|_| Error::type_error("Owned fixture PID must be numeric"))?;
                    source.cancel(Reason::Cancelled)?;
                    return Ok::<_, Error>(pid);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(Error::new(
                "Error",
                "Owned Node fixture did not become ready",
            ))
        }
    );
    let pid = pid?;
    assert!(result.is_err_and(|error| error.code == Some("ROSTER_CANCELLED")));
    assert_eq!(
        crate::state::locks::probe_pid(pid)?,
        crate::state::locks::Liveness::Dead
    );
    assert!(
        boundary
            .close()
            .await
            .is_err_and(|error| error.code == Some("ROSTER_PROCESS_CLEANUP"))
    );
    assert!(
        fixture
            .root
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .is_file()
    );
    Ok(())
}

#[test]
fn typed_configuration_reaches_native_scope_and_test_permissions() -> Result<(), Error> {
    let mut value =
        crate::config::parse(&Text::from(include_str!("../../roster.config.example.yml")))?
            .as_value()
            .clone();
    let mut tools = value
        .get("tools")
        .cloned()
        .ok_or_else(|| Error::type_error("Fixture requires tools config"))?;
    domain::put(&mut tools, "run_test", Value::Boolean(false))?;
    domain::put(&mut value, "tools", tools)?;
    let config = crate::config::parse(&crate::config::format(&value)?)?;
    let options = tools::Options::from_config(&config, Role::Coder)?;
    assert!(!options.allow_run_test);
    assert!(options.scope_expansion <= 16);
    let reviewer = tools::Options::from_config(&config, Role::Reviewer)?;
    assert!(!reviewer.allow_run_test);
    assert_eq!(reviewer.scope_expansion, 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn issue_number_and_recipe_mismatch_cannot_receive_green_identity() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], true)?;
    let mut assignment = fixture.assignment.clone();
    assignment.task = "issue-43".to_owned();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        assignment,
        tools::Options::default(),
        tools::Host::default(),
        redaction::Config::default(),
        CancellationSource::default().token(),
    )
    .await?;
    assert!(
        checkpoints::validation(&boundary)
            .await
            .is_err_and(|error| error
                .message
                .contains("recipe does not match task identity"))
    );
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn session_identifiers_are_checked_before_lock_persistence() -> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let mut assignment = fixture.assignment.clone();
    assignment.session = "test-only-private-api-key".to_owned();
    let config = redaction::Config {
        environment: Value::object(vec![(
            "ROSTER_API_KEY",
            Value::string("test-only-private-api-key"),
        )]),
        ..redaction::Config::default()
    };
    assert!(
        tools::Boundary::open(
            &fixture.root,
            assignment,
            tools::Options::default(),
            tools::Host::default(),
            config,
            CancellationSource::default().token()
        )
        .await
        .is_err_and(|error| error.code == Some("ROSTER_TOOL_DENIED"))
    );
    assert!(!fixture.root.join(".roster").exists());
    Ok(())
}

const FOCUSED_RUST_SOURCE: &str = "pub fn answer() -> u8 {\n    42\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn owned_behavior() {\n        assert_eq!(super::answer(), 42);\n    }\n}\n";

struct FocusedCargoFixture {
    fixture: Fixture,
    build: PathBuf,
}

impl FocusedCargoFixture {
    fn new(paths: &[&str], node_passes: Option<bool>) -> Result<Self, Error> {
        let fixture = Fixture::new(paths, true)?;
        let name = fixture
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::type_error("Owned fixture requires a Unicode name"))?;
        let build = fixture.root.with_file_name(format!("{name}-focused-build"));
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = fs::DirBuilder::new();
        builder.create(&build).map_err(io)?;
        let owned = Self { fixture, build };
        owned.fixture.write("Cargo.toml", "[package]\nname = \"execution-focused-owned-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\nrust-version = \"1.85\"\n\n[lib]\npath = \"rust/port.rs\"\n")?;
        owned.fixture.write("Cargo.lock", "version = 4\n\n[[package]]\nname = \"execution-focused-owned-fixture\"\nversion = \"0.0.0\"\n")?;
        owned.fixture.write("rust/port.rs", FOCUSED_RUST_SOURCE)?;
        owned.fixture.write("docs/guide.md", "# Owned guide\n")?;
        if let Some(passes) = node_passes {
            let expected = if passes { 42 } else { 99 };
            owned.fixture.node_tests(&format!("import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport {{ value }} from '../src/subject.mjs';\ntest('focused actual Node evidence', () => assert.equal(value, {expected}));\n"))?;
        }
        Ok(owned)
    }

    fn host(&self) -> tools::Host {
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
            self.build.display().to_string(),
        );
        tools::Host {
            cargo: Some(tools::CargoApproval {
                executable: PathBuf::from("cargo"),
                environment,
            }),
            ..tools::Host::default()
        }
    }

    fn mixed_options() -> tools::Options {
        tools::Options {
            scope_expansion: 2,
            initial_scope_files: vec!["rust/port.rs".to_owned(), "src/subject.mjs".to_owned()],
            ..tools::Options::default()
        }
    }
}

impl Drop for FocusedCargoFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.build);
    }
}

async fn focused_docs_mixed_failure(full: bool) -> Result<(), Error> {
    let owned = FocusedCargoFixture::new(&["docs/guide.md"], Some(false))?;
    let mut boundary = owned
        .fixture
        .open(
            FocusedCargoFixture::mixed_options(),
            owned.host(),
            &CancellationSource::default(),
        )
        .await?;
    let result = boundary.verify(full).await?;
    let green = checkpoints::capture(&boundary, true).await;
    boundary.close().await?;
    assert_eq!(
        result.get("exit_code"),
        Some(&Value::Number(1.0)),
        "full={full}; mixed result={}, accepted green={}",
        result.json(),
        green.is_ok()
    );
    assert!(green.is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_docs_expansions_require_actual_scoped_node() -> Result<(), Error> {
    focused_docs_mixed_failure(false).await
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_docs_expansions_require_actual_full_node() -> Result<(), Error> {
    focused_docs_mixed_failure(true).await
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_wildcard_writes_cannot_accept_cargo_only_green() -> Result<(), Error> {
    let owned = FocusedCargoFixture::new(&["**/*"], Some(true))?;
    let mut boundary = owned
        .fixture
        .open(
            tools::Options::default(),
            owned.host(),
            &CancellationSource::default(),
        )
        .await?;
    boundary.execute("read_file", &read("rust/port.rs")).await?;
    boundary
        .execute(
            "write_file",
            &write(
                "rust/port.rs",
                &format!("pub const OTHER: u8 = 7;\n\n{FOCUSED_RUST_SOURCE}"),
            ),
        )
        .await?;
    boundary
        .execute("read_file", &read("src/subject.mjs"))
        .await?;
    boundary
        .execute(
            "write_file",
            &write("src/subject.mjs", "export const value = 43;\n"),
        )
        .await?;
    let result = boundary.verify(false).await?;
    let green = checkpoints::capture(&boundary, true).await;
    boundary.close().await?;
    assert_eq!(
        result.get("exit_code"),
        Some(&Value::Number(1.0)),
        "wildcard mixed result={}, accepted green={}",
        result.json(),
        green.is_ok()
    );
    assert!(green.is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_aggregate_scope_is_checked_before_first_expansion() -> Result<(), Error> {
    let planned = (0..32)
        .map(|index| format!("src/planned-{index}.mjs"))
        .collect::<Vec<_>>();
    let paths = planned.iter().map(String::as_str).collect::<Vec<_>>();
    let fixture = Fixture::new(&paths, false)?;
    let repair = (0..32)
        .map(|index| format!("tests/repair-{index}.test.mjs"))
        .collect::<Vec<_>>();
    let options = tools::Options {
        scope_expansion: 1,
        initial_repair_files: repair,
        ..tools::Options::default()
    };
    let mut boundary = fixture
        .open(
            options,
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    let result = boundary
        .execute(
            "write_file",
            &write("src/sixty-five.mjs", "export const extra = 65;\n"),
        )
        .await;
    let scope = boundary.scope()?;
    let expanded = boundary.scope_files().to_vec();
    let checkpoint = boundary.first_checkpoint().cloned();
    let reader = checkpoints::list(&boundary);
    boundary.close().await?;
    assert!(
        result.is_err(),
        "aggregate {} accepted; checkpoint={}, reader rejected={}",
        scope.len(),
        checkpoint.is_some(),
        reader.is_err()
    );
    assert_eq!(scope.len(), 64);
    assert!(expanded.is_empty());
    assert!(checkpoint.is_none());
    assert!(reader?.is_empty());
    assert!(!fixture.root.join("src/sixty-five.mjs").exists());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_injected_signal_status_retains_writer_for_check_and_probe()
-> Result<(), Error> {
    for descendants_possible in [true, false] {
        let fixture = Fixture::new(&["src/subject.mjs"], false)?;
        let mut boundary = fixture
            .open(
                tools::Options::default(),
                tools::Host::default(),
                &CancellationSource::default(),
            )
            .await?;
        let failure = crate::process::Failure {
            kind: crate::process::FailureKind::Exit(None),
            captured: crate::process::Captured {
                pid: Some(std::process::id()),
                ..crate::process::Captured::default()
            },
            cleanup_complete: true,
            source: None,
        };
        boundary.observe_process_failure(&failure, descendants_possible);
        let released = boundary.close().await;
        assert!(
            released.is_err_and(|error| error.code == Some("ROSTER_PROCESS_CLEANUP")),
            "injected signal, descendants_possible={descendants_possible}: writer release was accepted"
        );
        assert!(
            fixture
                .root
                .join(".roster")
                .join("locks")
                .join("run.lock")
                .is_file()
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_mixed_green_binds_real_engine_coverage_not_json_flags() -> Result<(), Error>
{
    let owned = FocusedCargoFixture::new(&["docs/guide.md"], Some(true))?;
    let mut boundary = owned
        .fixture
        .open(
            FocusedCargoFixture::mixed_options(),
            owned.host(),
            &CancellationSource::default(),
        )
        .await?;
    let result = boundary.verify(false).await?;
    assert_eq!(result.get("exit_code"), Some(&Value::Number(0.0)));
    assert!(result.get("skipped").is_none());
    assert_eq!(
        result
            .get("commands")
            .and_then(domain::items)
            .map(<[Value]>::len),
        Some(4)
    );
    assert!(result.json().contains("focused actual Node evidence"));
    checkpoints::capture(&boundary, true).await?;
    boundary.discard_node_coverage()?;
    assert_eq!(
        boundary
            .last_tests()
            .and_then(|evidence| evidence.result().get("exit_code")),
        Some(&Value::Number(0.0))
    );
    assert!(
        checkpoints::capture(&boundary, true)
            .await
            .is_err_and(|error| error
                .message
                .contains("every effective Node/Cargo requirement"))
    );
    assert_eq!(checkpoints::list(&boundary)?.len(), 1);
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_rust_only_green_requires_cargo_but_not_node() -> Result<(), Error> {
    let owned = FocusedCargoFixture::new(&["rust/port.rs"], None)?;
    let mut host = owned.host();
    host.node20 = None;
    let mut boundary = owned
        .fixture
        .open(
            tools::Options::default(),
            host,
            &CancellationSource::default(),
        )
        .await?;
    let result = boundary.verify(false).await?;
    assert_eq!(result.get("exit_code"), Some(&Value::Number(0.0)));
    assert_eq!(
        result
            .get("commands")
            .and_then(domain::items)
            .map(<[Value]>::len),
        Some(3)
    );
    checkpoints::capture(&boundary, true).await?;
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_mixed_missing_node_never_leaves_green_evidence() -> Result<(), Error> {
    let owned = FocusedCargoFixture::new(&["docs/guide.md"], Some(true))?;
    let mut host = owned.host();
    host.node20 = None;
    let mut boundary = owned
        .fixture
        .open(
            FocusedCargoFixture::mixed_options(),
            host,
            &CancellationSource::default(),
        )
        .await?;
    assert!(
        boundary
            .verify(false)
            .await
            .is_err_and(|error| error.message.contains("Node 20"))
    );
    assert!(boundary.last_tests().is_none());
    assert!(checkpoints::capture(&boundary, true).await.is_err());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_mixed_missing_node_tests_never_marks_cargo_as_sufficient()
-> Result<(), Error> {
    let owned = FocusedCargoFixture::new(&["docs/guide.md"], None)?;
    owned
        .fixture
        .write("src/subject.mjs", "export const value = 42;\n")?;
    let mut boundary = owned
        .fixture
        .open(
            FocusedCargoFixture::mixed_options(),
            owned.host(),
            &CancellationSource::default(),
        )
        .await?;
    assert!(
        boundary
            .verify(false)
            .await
            .is_err_and(|error| error.code == Some("ROSTER_NATIVE_UNSUPPORTED"))
    );
    assert!(boundary.last_tests().is_none());
    assert!(checkpoints::capture(&boundary, true).await.is_err());
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_deleted_wildcard_javascript_remains_a_node_obligation() -> Result<(), Error>
{
    let owned = FocusedCargoFixture::new(&["**/*"], None)?;
    owned
        .fixture
        .write("src/subject.mjs", "export const value = 42;\n")?;
    let mut boundary = owned
        .fixture
        .open(
            tools::Options::default(),
            owned.host(),
            &CancellationSource::default(),
        )
        .await?;
    boundary
        .execute("delete_file", &read("src/subject.mjs"))
        .await?;
    assert!(!owned.fixture.root.join("src/subject.mjs").exists());
    assert!(
        boundary
            .verify(false)
            .await
            .is_err_and(|error| error.code == Some("ROSTER_NATIVE_UNSUPPORTED"))
    );
    assert!(boundary.last_tests().is_none());
    assert!(checkpoints::capture(&boundary, true).await.is_err());
    boundary.close().await
}

struct FocusedEvents(Vec<Value>);

impl Observer for FocusedEvents {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        self.0.push(event);
        Ok(())
    }
}

async fn focused_scope_edge(repair_count: usize, initial_write: bool) -> Result<(), Error> {
    let planned = (0..32)
        .map(|index| format!("src/planned-{index}.mjs"))
        .collect::<Vec<_>>();
    let paths = planned.iter().map(String::as_str).collect::<Vec<_>>();
    let fixture = Fixture::new(&paths, false)?;
    let repair = (0..repair_count)
        .map(|index| format!("tests/repair-{index}.test.mjs"))
        .collect::<Vec<_>>();
    let options = tools::Options {
        scope_expansion: 1,
        initial_repair_files: repair,
        ..tools::Options::default()
    };
    let mut boundary = fixture
        .open(
            options,
            tools::Host::default(),
            &CancellationSource::default(),
        )
        .await?;
    if initial_write {
        boundary
            .execute(
                "write_file",
                &write("src/planned-0.mjs", "export const planned = 0;\n"),
            )
            .await?;
        assert_eq!(checkpoints::list(&boundary)?.len(), 1);
    }
    let mut events = FocusedEvents(Vec::new());
    let result = boundary
        .execute_with(
            "write_file",
            &write("src/extra.mjs", "export const extra = 1;\n"),
            &mut events,
        )
        .await;
    if repair_count == 31 {
        result?;
        assert_eq!(boundary.scope()?.len(), MAX_SCOPE_ENTRIES);
        assert_eq!(boundary.scope_files(), ["src/extra.mjs"]);
        assert_eq!(checkpoints::list(&boundary)?.len(), 1);
        assert!(
            events
                .0
                .iter()
                .any(|event| domain::is_text(event.get("type"), "scope-expansion"))
        );
    } else {
        assert!(result.is_err_and(|error| error.name == "ToolUsageError"));
        assert_eq!(boundary.scope()?.len(), MAX_SCOPE_ENTRIES);
        assert!(boundary.scope_files().is_empty());
        assert!(!fixture.root.join("src/extra.mjs").exists());
        assert!(
            !events
                .0
                .iter()
                .any(|event| domain::is_text(event.get("type"), "scope-expansion"))
        );
        assert_eq!(
            checkpoints::list(&boundary)?.len(),
            usize::from(initial_write)
        );
    }
    boundary.close().await
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_aggregate_overflow_is_rejected_with_an_existing_checkpoint()
-> Result<(), Error> {
    focused_scope_edge(32, true).await
}

#[tokio::test(flavor = "current_thread")]
async fn focused_review_aggregate_exactly_sixty_four_still_allows_expansion() -> Result<(), Error> {
    focused_scope_edge(31, false).await
}

#[test]
fn focused_review_numeric_exit_is_not_signal_or_unknown_cleanup() {
    for descendants_possible in [false, true] {
        for code in [1, 42, 255] {
            let failure = crate::process::Failure {
                kind: crate::process::FailureKind::Exit(Some(code)),
                captured: crate::process::Captured {
                    pid: Some(std::process::id()),
                    ..crate::process::Captured::default()
                },
                cleanup_complete: true,
                source: None,
            };
            assert!(!tools::unverified_cleanup(&failure, descendants_possible));
        }
        let unknown = crate::process::Failure {
            kind: crate::process::FailureKind::Exit(Some(1)),
            captured: crate::process::Captured::default(),
            cleanup_complete: false,
            source: None,
        };
        assert!(tools::unverified_cleanup(&unknown, descendants_possible));
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn focused_review_real_posix_signal_preserves_writer_while_descendant_survives()
-> Result<(), Error> {
    let fixture = Fixture::new(&["src/subject.mjs"], false)?;
    let host = tools::Host::default();
    let node = host.node20.clone().ok_or_else(|| {
        Error::new(
            "Error",
            "Owned Node is required for the POSIX descendant control",
        )
    })?;
    let mut boundary = fixture
        .open(
            tools::Options::default(),
            host,
            &CancellationSource::default(),
        )
        .await?;
    let pid_file = fixture.root.join("owned-descendant.pid");
    let path = pid_file
        .to_str()
        .ok_or_else(|| Error::type_error("Owned fixture path must be Unicode"))?;
    let child = format!(
        "const fs=require('node:fs');const p={};fs.writeFileSync(p+'.writing',String(process.pid));fs.renameSync(p+'.writing',p);setInterval(()=>{{}},1000);setTimeout(()=>process.exit(0),10000)",
        Text::from(path).json()
    );
    let script = format!(
        "const fs=require('node:fs');const c=require('node:child_process').spawn(process.execPath,['--eval',{}],{{detached:true,stdio:'ignore'}});c.unref();const timer=setInterval(()=>{{if(fs.existsSync({})){{clearInterval(timer);process.kill(process.pid,'SIGTERM')}}}},10)",
        Text::from(child.as_str()).json(),
        Text::from(path).json()
    );
    let result = boundary
        .process(
            &node,
            &["--eval".to_owned(), script],
            &BTreeMap::new(),
            5000,
            1024,
        )
        .await;
    let pid = fs::read_to_string(&pid_file)
        .map_err(io)?
        .parse::<u64>()
        .map_err(|_| Error::type_error("Owned descendant PID must be numeric"))?;
    let survived = crate::state::locks::probe_pid(pid)?;
    let release = boundary.close().await;
    let mut cleanup = tokio::process::Command::new(&node);
    cleanup.args([
        "--eval",
        &format!("try{{process.kill({pid},'SIGTERM')}}catch(e){{if(e.code!=='ESRCH')throw e}}"),
    ]);
    crate::process::execute(
        &mut cleanup,
        crate::process::Limits {
            timeout: Some(Duration::from_secs(2)),
            max_output_bytes: 1024,
        },
        None,
    )
    .await
    .map_err(|failure| {
        Error::new(
            "Error",
            format!("Owned descendant cleanup failed: {failure}"),
        )
    })?;
    let mut stopped = false;
    for _ in 0..200 {
        if crate::state::locks::probe_pid(pid)? == crate::state::locks::Liveness::Dead {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if !stopped {
        return Err(Error::new(
            "Error",
            "Owned descendant cleanup is unverified; inspect its specific PID",
        ));
    }
    assert_eq!(survived, crate::state::locks::Liveness::Alive);
    assert!(result.is_err_and(|error| error.code == Some("ROSTER_PROCESS_CLEANUP")));
    assert!(release.is_err_and(|error| error.code == Some("ROSTER_PROCESS_CLEANUP")));
    assert!(
        fixture
            .root
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .is_file()
    );
    Ok(())
}
