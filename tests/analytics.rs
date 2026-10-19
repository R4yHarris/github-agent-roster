#![cfg(windows)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use github_agent_roster::cancel::CancellationSource;
use github_agent_roster::execution::{backend, e2e, tools};
use github_agent_roster::identity;
use github_agent_roster::json::{self, Value};
use github_agent_roster::planning::{artifacts, lifecycle};
use github_agent_roster::state::provenance::api::{self, ProvenanceStore};
use github_agent_roster::state::{files, layout, redaction, secure};
use github_agent_roster::text::Text;
use github_agent_roster::{Error, evaluation, learning, metrics, start};

const ORIGIN: &str = "https://github.com/owned/analytics.git";
const OWNED_FLEET: &str = concat!(
    "profiles:\n",
    "  - id: first\n",
    "    base_url: https://first.example.invalid\n",
    "    model: owner/first\n",
    "    provider: vllm\n",
    "    context_max: 32768\n",
    "    concurrency: 2\n",
    "    hardware: owned GPU\n",
    "    notes: test-only-private-fleet-note\n",
    "    api_key_env: BOUND_CREDENTIAL\n",
    "    task_class: [feat, fix]\n",
    "  - id: default\n",
    "    base_url: https://default.example.invalid/v1\n",
    "    model: owner/default\n",
    "    provider: vllm\n",
    "    context_max: 0\n",
    "    concurrency: 1\n",
    "    hardware: cpu\n",
    "    notes: \"\"\n",
);
const OWNED_FLEET_TABLE: &str = concat!(
    "ID\tMODEL\tBASE_URL\tCONTEXT_MAX\tCONCURRENCY\tHARDWARE\tTASK_CLASS\n",
    "first\towner/first\thttps://first.example.invalid/v1\t32768\t2\towned GPU\tfeat,fix\n",
    "default\towner/default\thttps://default.example.invalid/v1\tunknown\t1\tcpu\t-\n",
);

fn io(error: std::io::Error) -> Error {
    Error::new("AnalyticsFixtureError", error.to_string())
}

fn utf8(bytes: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(bytes).map_err(|_| Error::type_error("Fixture output must be UTF-8"))
}

fn parsed(bytes: &[u8]) -> Result<Value, Error> {
    json::parse(&Text::from(utf8(bytes)?))
}

fn number(value: &Value, field: &str) -> Option<f64> {
    value.get(field).and_then(Value::as_number)
}

fn items(value: &Value) -> Result<&[Value], Error> {
    if let Value::Array(items) = value {
        Ok(items)
    } else {
        Err(Error::type_error("Expected an analytics array"))
    }
}

struct Fixture {
    container: PathBuf,
    root: PathBuf,
    home: PathBuf,
    state: PathBuf,
    installation: PathBuf,
    git: PathBuf,
    cleaned: bool,
}

impl Fixture {
    fn new(sha256: bool) -> Result<Self, Error> {
        let container = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".roster")
            .join(format!(
                "analytics-tests-{}-{}",
                std::process::id(),
                files::nonce()?
            ));
        let git = std::env::var_os("ROSTER_TEST_GIT")
            .map(PathBuf::from)
            .ok_or_else(|| {
                Error::type_error("Native analytics fixtures require explicit ROSTER_TEST_GIT")
            })?;
        github_agent_roster::host::Image::approve(&git)?;
        let fixture = Self {
            root: container.join("project"),
            home: container.join("home"),
            state: container.join("state"),
            installation: container.join("installation"),
            container,
            git,
            cleaned: false,
        };
        for path in [
            &fixture.root,
            &fixture.home,
            &fixture.state,
            &fixture.installation,
        ] {
            files::create_private_directories(path)?;
        }
        eprintln!(
            "ANALYTICS_FIXTURE pid={} root={}",
            std::process::id(),
            fixture.container.display()
        );
        fixture.git(&[
            "init",
            "--quiet",
            if sha256 {
                "--object-format=sha256"
            } else {
                "--object-format=sha1"
            },
        ])?;
        fixture.git(&["remote", "add", "origin", ORIGIN])?;
        fixture.write(
            ".gitignore",
            ".roster/\nTASK.md\nRECIPE.yml\nRESULT.md\nREVIEW.md\nCONTEXT.md\n",
        )?;
        fixture.write(
            "docs/guide.md",
            "# Owned guide\n\nBounded analytics fixture.\n",
        )?;
        Ok(fixture)
    }

    fn write(&self, file: &str, source: &str) -> Result<(), Error> {
        let path = self.root.join(file);
        files::create_private_directories(
            path.parent()
                .ok_or_else(|| Error::type_error("Fixture file needs a parent"))?,
        )?;
        fs::write(path, source).map_err(io)
    }

    fn git(&self, args: &[&str]) -> Result<String, Error> {
        let output = Command::new(&self.git)
            .current_dir(&self.root)
            .env_clear()
            .envs(tools::Host::default().environment)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args([
                "--no-pager",
                "-c",
                "init.templateDir=",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "protocol.allow=never",
                "-c",
                "user.name=Owned Analytics Fixture",
                "-c",
                "user.email=owned@example.invalid",
                "-c",
            ])
            .arg(format!(
                "core.hooksPath={}",
                self.container.join("disabled-hooks").display()
            ))
            .args(args)
            .output()
            .map_err(io)?;
        if !output.status.success() {
            return Err(Error::new(
                "AnalyticsFixtureError",
                format!("Owned Git setup failed: {}", utf8(&output.stderr)?),
            ));
        }
        Ok(utf8(&output.stdout)?
            .trim_end_matches(['\r', '\n'])
            .to_owned())
    }

    fn commit(&self, model: Option<&str>, session: &str) -> Result<String, Error> {
        self.git(&["add", "--", "."])?;
        let message = model.map_or_else(|| "Owned analytics base".to_owned(), |model| {
            format!("fix: owned analytics observation\n\nAI-Agent: roster-coder\nAI-Model: {model}\nAI-Run: 1|local|{model}@-|h|0/32768|-|{session}|fix-fixture")
        });
        self.git(&["commit", "--quiet", "--allow-empty", "-m", &message])?;
        self.git(&["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"])
    }

    fn command(&self, args: &[&str], cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_roster"));
        command
            .current_dir(cwd)
            .env_clear()
            .envs(tools::Host::default().environment)
            .env("PATH", "")
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("LOCALAPPDATA", self.home.join("Local"))
            .env("APPDATA", self.home.join("Roaming"))
            .env("XDG_STATE_HOME", self.home.join("xdg-state"))
            .env("ROSTER_STATE_HOME", &self.state)
            .env("ROSTER_INSTALLATION_ROOT", &self.installation)
            .env("ROSTER_GIT", &self.git)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn invoke(&self, args: &[&str]) -> Result<Output, Error> {
        self.command(args, &self.root).output().map_err(io)
    }

    fn host(&self) -> tools::Host {
        tools::Host {
            git: self.git.clone(),
            node20: None,
            cargo: None,
            contracts: None,
            ..tools::Host::default()
        }
    }

    fn environment(&self) -> Value {
        Value::object(vec![
            ("HOME", Value::string(self.home.to_string_lossy().as_ref())),
            (
                "USERPROFILE",
                Value::string(self.home.to_string_lossy().as_ref()),
            ),
            (
                "ROSTER_STATE_HOME",
                Value::string(self.state.to_string_lossy().as_ref()),
            ),
        ])
    }

    fn task(&self, allowed: &[&str], checks: &[&str]) -> Result<lifecycle::Assignment, Error> {
        let assignment = lifecycle::Assignment {
            ask: Text::from(format!(
                "Document bounded local analytics without changing policy.\n\n## Files allowed\n{}",
                allowed.iter().map(|file| format!("- {file}")).collect::<Vec<_>>().join("\n")
            ).as_str()),
            title: None, reference: "issue:42".to_owned(), task: "issue-42".to_owned(),
            session: "analytics-acceptance".to_owned(),
        };
        let plan = artifacts::build(
            &Value::String(assignment.ask.clone()),
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
        let task = plan
            .task
            .to_utf8()?
            .lines()
            .map(|line| {
                if line.starts_with("tests:") {
                    "tests: none"
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        self.write("TASK.md", &task)?;
        self.write("RECIPE.yml", &plan.recipe.to_utf8()?)?;
        Ok(assignment)
    }

    fn finish(mut self) -> Result<(), Error> {
        fs::remove_dir_all(&self.container).map_err(io)?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.cleaned {
            eprintln!(
                "ANALYTICS_FIXTURE_RETAINED pid={} root={}",
                std::process::id(),
                self.container.display()
            );
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    directories: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, Vec<u8>>,
}

fn snapshot(root: &Path) -> Result<Snapshot, Error> {
    fn visit(base: &Path, path: &Path, state: &mut Snapshot) -> Result<(), Error> {
        let metadata = fs::symlink_metadata(path).map_err(io)?;
        let relative = path
            .strip_prefix(base)
            .map_err(|_| Error::type_error("Fixture snapshot escaped its owner"))?
            .to_path_buf();
        if secure::linked(&metadata) {
            return Err(Error::type_error("Snapshot refuses linked fixture entries"));
        }
        if metadata.is_dir() {
            state.directories.insert(relative);
            for entry in fs::read_dir(path).map_err(io)? {
                visit(base, &entry.map_err(io)?.path(), state)?;
            }
        } else {
            state.files.insert(relative, fs::read(path).map_err(io)?);
        }
        Ok(())
    }
    let mut result = Snapshot {
        directories: BTreeSet::new(),
        files: BTreeMap::new(),
    };
    visit(root, root, &mut result)?;
    Ok(result)
}

#[cfg(windows)]
#[test]
fn analytics_cli_reads_real_git_and_local_corrections_from_subdirectories_without_effects()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let mut shas = Vec::new();
    for index in 0..3 {
        shas.push(fixture.commit(Some("capable"), &format!("observed-{index}"))?);
    }
    fixture.write(".roster/runs/runs.jsonl", concat!(
        "{\"session\":\"observed-0\",\"task\":\"fix-fixture\",\"model\":\"planned-alias\",\"task_class\":\"fix\",\"effort\":\"l\",\"context_used\":12}\n",
        "{\"session\":\"observed-1\",\"task_class\":\"fix\"}\n",
        "{\"session\":\"observed-2\",\"task_class\":\"fix\"}\n",
        "{\"session\":\"unpublished\",\"task\":\"docs-guide\",\"model\":\"writer\",\"context_out\":0}\n"
    ))?;
    let mut evaluations = String::from(concat!(
        "{\"session\":\"observed-0\",\"verdict\":\"reject\",\"difficulty\":4,\"again\":false,\"minutes\":1}\n",
        "{\"session\":\"observed-1\",\"verdict\":\"accept\",\"difficulty\":4,\"again\":true,\"minutes\":20}\n",
        "{\"session\":\"observed-2\",\"verdict\":\"accept\",\"difficulty\":5,\"again\":true,\"minutes\":30}\n",
        "{\"session\":\"observed-0\",\"verdict\":\"accept\",\"difficulty\":4,\"again\":true,\"minutes\":10}\n"
    ));
    fixture.write(".roster/evals.jsonl", &evaluations)?;
    let unearned = fixture.invoke(&["recommend", "--task-class", "fix", "--difficulty", "4"])?;
    assert!(unearned.status.success(), "{}", utf8(&unearned.stderr)?);
    assert_eq!(
        utf8(&unearned.stdout)?,
        "insufficient data; config default: (unset) effort=m\n"
    );
    for (index, difficulty) in [2, 2, 2, 3, 3, 3].iter().enumerate() {
        evaluations.push_str(&format!("{{\"session\":\"earned-{index}\",\"model\":\"capable\",\"task_class\":\"docs\",\"effort\":\"l\",\"verdict\":\"accept\",\"difficulty\":{difficulty},\"again\":true}}\n"));
    }
    fixture.write(".roster/evals.jsonl", &evaluations)?;
    fixture.write(
        ".roster/runs/activity.log",
        "test-only-private-transcript-that-is-not-learning\n",
    )?;
    files::create_private_directories(&fixture.root.join("nested"))?;
    let before = snapshot(&fixture.container)?;
    let stats = fixture
        .command(&["stats", "--json"], &fixture.root.join("nested"))
        .output()
        .map_err(io)?;
    assert!(stats.status.success(), "{}", utf8(&stats.stderr)?);
    assert!(stats.stderr.is_empty());
    let groups = parsed(&stats.stdout)?;
    let rows = items(&groups)?;
    assert_eq!(rows.len(), 3);
    let capable = rows
        .iter()
        .find(|row| {
            row.get("model") == Some(&Value::string("capable"))
                && row.get("task_class") == Some(&Value::string("fix"))
        })
        .ok_or_else(|| Error::type_error("Missing actual served model group"))?;
    assert_eq!(number(capable, "runs"), Some(3.0));
    assert_eq!(number(capable, "n"), Some(3.0));
    assert_eq!(number(capable, "acceptRate"), Some(1.0));
    assert_eq!(number(capable, "medianMinutes"), Some(20.0));
    assert!(!utf8(&stats.stdout)?.contains("planned-alias"));
    assert!(!utf8(&stats.stdout)?.contains("test-only-private-transcript"));
    let table = fixture.invoke(&["stats"])?;
    assert!(table.status.success(), "{}", utf8(&table.stderr)?);
    assert!(
        utf8(&table.stdout)?
            .lines()
            .any(|line| line.split_whitespace().collect::<Vec<_>>()
                == ["capable", "h", "3", "3", "fix", "3", "100.0%", "20", "4"])
    );
    let recommend = fixture.invoke(&["recommend", "--difficulty", "4", "--task-class", "fix"])?;
    assert!(recommend.status.success(), "{}", utf8(&recommend.stderr)?);
    assert_eq!(
        utf8(&recommend.stdout)?,
        "fix: capable effort=h accept-rate=100.0% n=3 median-min=20 median-difficulty=4\n"
    );
    let json = fixture.invoke(&[
        "recommend",
        "--task-class",
        "fix",
        "--difficulty",
        "4",
        "--json",
    ])?;
    assert!(json.status.success(), "{}", utf8(&json.stderr)?);
    assert_eq!(number(&parsed(&json.stdout)?, "n"), Some(3.0));
    let restricted = fixture.invoke(&[
        "stats",
        "--json",
        "--ref",
        &format!("{}..{}", shas[0], shas[2]),
    ])?;
    assert!(restricted.status.success(), "{}", utf8(&restricted.stderr)?);
    let groups = parsed(&restricted.stdout)?;
    assert_eq!(items(&groups)?.len(), 1);
    assert_eq!(
        number(
            items(&groups)?
                .first()
                .ok_or_else(|| Error::type_error("Missing restricted group"))?,
            "n"
        ),
        Some(2.0)
    );
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_cli_explicit_evals_override_only_matching_sha_and_use_the_callers_relative_path()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let sha = fixture.commit(Some("observed"), "session")?;
    fixture.write(
        ".roster/runs/runs.jsonl",
        "{\"session\":\"session\",\"task_class\":\"fix\"}\n",
    )?;
    fixture.write(
        ".roster/evals.jsonl",
        &format!("{{\"sha\":\"{sha}\",\"verdict\":\"reject\",\"difficulty\":2,\"again\":false}}\n"),
    )?;
    fixture.write("nested/extra.jsonl", &format!("{{\"sha\":\"{}\",\"verdict\":\"accept\",\"difficulty\":3,\"again\":true}}\n{{\"sha\":\"{}\",\"model\":\"must-not-create-a-run\",\"task_class\":\"fix\",\"verdict\":\"accept\",\"difficulty\":4,\"again\":true}}\n", sha.to_ascii_uppercase(), "f".repeat(40)))?;
    let before = snapshot(&fixture.container)?;
    let default = fixture.invoke(&["stats", "--json"])?;
    assert!(default.status.success(), "{}", utf8(&default.stderr)?);
    let default = parsed(&default.stdout)?;
    assert_eq!(
        number(
            items(&default)?
                .first()
                .ok_or_else(|| Error::type_error("Missing default group"))?,
            "acceptRate"
        ),
        Some(0.0)
    );
    let explicit = fixture
        .command(
            &["stats", "--json", "--ref", "HEAD", "--evals", "extra.jsonl"],
            &fixture.root.join("nested"),
        )
        .output()
        .map_err(io)?;
    assert!(explicit.status.success(), "{}", utf8(&explicit.stderr)?);
    let groups = parsed(&explicit.stdout)?;
    assert_eq!(items(&groups)?.len(), 1);
    assert_eq!(
        number(
            items(&groups)?
                .first()
                .ok_or_else(|| Error::type_error("Missing override group"))?,
            "acceptRate"
        ),
        Some(1.0)
    );
    assert!(!utf8(&explicit.stdout)?.contains("must-not-create-a-run"));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_explicit_sha_evals_never_borrow_an_extra_session_or_a_null_sha_run()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let sha = fixture.commit(Some("published"), "published-session")?;
    fixture.write(".roster/runs/runs.jsonl", concat!(
        "{\"session\":\"published-session\",\"task_class\":\"fix\"}\n",
        "{\"sha\":null,\"session\":\"local-only\",\"model\":\"local-only\",\"task_class\":\"fix\"}\n"
    ))?;
    for (sha, evaluated) in [(sha, 1.0), ("f".repeat(40), 0.0)] {
        fixture.write("external.jsonl", &format!("{{\"sha\":\"{sha}\",\"session\":\"local-only\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true}}\n"))?;
        let before = snapshot(&fixture.container)?;
        let output = fixture.invoke(&["stats", "--json", "--evals", "external.jsonl"])?;
        assert!(output.status.success(), "{}", utf8(&output.stderr)?);
        let output = parsed(&output.stdout)?;
        let local = items(&output)?
            .iter()
            .find(|row| row.get("model") == Some(&Value::string("local-only")))
            .ok_or_else(|| Error::type_error("Missing local-only group"))?;
        assert_eq!(number(local, "evaluated"), Some(0.0));
        assert_eq!(number(local, "n"), Some(0.0));
        let published = items(&output)?
            .iter()
            .find(|row| row.get("model") == Some(&Value::string("published")))
            .ok_or_else(|| Error::type_error("Missing published group"))?;
        assert_eq!(number(published, "evaluated"), Some(evaluated));
        assert_eq!(snapshot(&fixture.container)?, before);
    }
    fixture.finish()
}

#[test]
fn analytics_cli_rejects_invalid_flags_before_any_default_history_config_or_git_path()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let before = snapshot(&fixture.container)?;
    for args in [
        vec!["stats", "--ref"],
        vec!["stats", "--evals"],
        vec!["stats", "--ref", "--all"],
        vec!["stats", "--ref", "HEAD..--all"],
        vec!["stats", "--ref", "HEAD:path"],
        vec!["stats", "--ref", "HEAD", "--ref", "HEAD"],
        vec!["stats", "--json", "--json"],
        vec!["stats", "--unknown"],
        vec!["stats", "--delivery"],
        vec!["recommend"],
        vec!["recommend", "--task-class", "chore"],
        vec!["recommend", "--task-class", "fix", "--difficulty", "0"],
        vec!["recommend", "--task-class", "fix", "--difficulty", "6"],
        vec!["recommend", "--task-class", "fix", "--difficulty", "1.5"],
        vec!["recommend", "--task-class", "fix", "--difficulty"],
        vec!["recommend", "--task-class", "fix", "--task-class", "docs"],
        vec!["recommend", "--task-class", "fix", "--unknown"],
        vec!["eval", "session", "accept", "2", "y"],
        vec!["onboard"],
        vec!["shell"],
    ] {
        let output = fixture
            .command(&args, &fixture.root)
            .env("ROSTER_GIT", fixture.container.join("does-not-exist.exe"))
            .output()
            .map_err(io)?;
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let message = utf8(&output.stderr)?;
        assert!(
            !message.contains("Could not find") && !message.contains("approve read-only Git"),
            "{args:?}: {message}"
        );
    }
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_cli_preserves_malformed_history_and_reports_missing_or_invalid_providers_without_zeros()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(Some("known"), "observed")?;
    for source in [
        "{\"session\":\"observed\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true}\nnot-json\n",
        "{\"session\":\"observed\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true}\n\n",
        "{\"session\":\"observed\",\"verdict\":\"accept\",\"difficulty\":6,\"again\":true}\n",
        "{\"session\":\"observed\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true",
        "{\"session\":\"observed\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true,\"transcript\":\"test-only-private-source\"}\n",
    ] {
        fixture.write(".roster/evals.jsonl", source)?;
        let before = snapshot(&fixture.container)?;
        let output = fixture.invoke(&["stats", "--json"])?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!utf8(&output.stderr)?.contains("test-only-private-source"));
        assert_eq!(snapshot(&fixture.container)?, before);
    }
    fs::remove_file(fixture.root.join(".roster").join("evals.jsonl")).map_err(io)?;
    for args in [
        vec!["stats", "--json", "--ref", "missing-ref"],
        vec!["stats", "--json", "--evals", "missing.jsonl"],
    ] {
        let before = snapshot(&fixture.container)?;
        let output = fixture.invoke(&args)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!utf8(&output.stderr)?.contains(&fixture.container.to_string_lossy().into_owned()));
        assert_eq!(snapshot(&fixture.container)?, before);
    }
    let before = snapshot(&fixture.container)?;
    let output = fixture
        .command(&["stats", "--json"], &fixture.root)
        .env("ROSTER_GIT", fixture.container.join("missing.exe"))
        .output()
        .map_err(io)?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_cli_exports_actual_sha256_objects_and_retains_legacy_sha256_evaluations()
-> Result<(), Error> {
    let fixture = Fixture::new(true)?;
    let sha = fixture.commit(Some("sha256-model"), "sha256-session")?;
    assert_eq!(sha.len(), 64);
    fixture.write(".roster/evals.jsonl", &format!("{{\"sha\":\"{}\",\"verdict\":\"accept\",\"difficulty\":3,\"again\":true,\"minutes\":0}}\n", sha.to_ascii_uppercase()))?;
    let before = snapshot(&fixture.container)?;
    let output = fixture.invoke(&["stats", "--json", "--ref", &sha])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    let output = parsed(&output.stdout)?;
    let group = items(&output)?
        .first()
        .ok_or_else(|| Error::type_error("Missing SHA-256 group"))?;
    assert_eq!(number(group, "n"), Some(1.0));
    assert_eq!(number(group, "medianMinutes"), Some(0.0));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn analytics_machine_history_is_explicit_identity_scoped_and_never_reads_foreign_records_or_memory()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let identity = identity::identity_hash(&Text::from(ORIGIN))?;
    let root = fixture.state.join("history").join(&identity);
    let mut store = ProvenanceStore::open(&root, api::Identity::Explicit(identity.clone()))?;
    store
        .record_event(
            &Value::object(vec![
                ("runId", Value::string("native-run")),
                ("sessionId", Value::string("native-coder")),
                ("event", Value::string("session")),
                (
                    "payload",
                    Value::object(vec![
                        (
                            "seat",
                            Value::object(vec![("name", Value::string("coder"))]),
                        ),
                        (
                            "issue",
                            Value::object(vec![("task", Value::string("fix-native"))]),
                        ),
                        ("requestedModel", Value::string("not-the-served-model")),
                        ("servedModel", Value::string("served-native")),
                        (
                            "metrics",
                            Value::object(vec![("tokens_prompt", Value::Number(0.0))]),
                        ),
                        ("task_class", Value::string("fix")),
                    ]),
                ),
            ]),
            0,
            None,
        )
        .await?;
    store
        .record_memory(
            &Value::object(vec![
                ("runId", Value::string("native-run")),
                ("sessionId", Value::string("memory")),
                (
                    "memory",
                    Value::string("test-only-curated-note-not-raw-history"),
                ),
            ]),
            None,
        )
        .await?;
    let foreign = fixture
        .state
        .join("history")
        .join(format!("sha256-{}", "f".repeat(64)))
        .join("log");
    files::create_private_directories(&foreign)?;
    fs::write(
        foreign.join("poison.json"),
        "not-json test-only-foreign-repository-record",
    )
    .map_err(io)?;
    files::create_private_directories(&fixture.home.join(".roster"))?;
    fs::write(
        fixture.home.join(".roster").join("history"),
        "test-only-operator-shell-history-must-not-read",
    )
    .map_err(io)?;
    fixture.write(
        ".roster/evals.jsonl",
        "{\"session\":\"native-coder\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true}\n",
    )?;
    let before = snapshot(&fixture.container)?;
    let output = fixture.invoke(&["stats", "--json"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    let rows = parsed(&output.stdout)?;
    let group = items(&rows)?
        .first()
        .ok_or_else(|| Error::type_error("Missing native machine group"))?;
    assert_eq!(group.get("model"), Some(&Value::string("served-native")));
    assert_eq!(number(group, "n"), Some(1.0));
    assert_eq!(group.get("medianMinutes"), Some(&Value::Null));
    for sentinel in [
        "not-the-served-model",
        "test-only-foreign",
        "test-only-curated",
        "operator-shell-history",
    ] {
        assert!(!utf8(&output.stdout)?.contains(sentinel));
    }
    let opted_out = fixture
        .command(&["stats", "--json"], &fixture.root)
        .env("ROSTER_PROVENANCE_OPT_OUT", "true")
        .output()
        .map_err(io)?;
    assert!(opted_out.status.success(), "{}", utf8(&opted_out.stderr)?);
    assert_eq!(utf8(&opted_out.stdout)?, "[]\n");
    let session_only = fixture
        .command(&["stats", "--json"], &fixture.root)
        .env_remove("ROSTER_STATE_HOME")
        .output()
        .map_err(io)?;
    assert!(
        session_only.status.success(),
        "{}",
        utf8(&session_only.stderr)?
    );
    assert_eq!(utf8(&session_only.stdout)?, "[]\n");
    let git = start::Git::new(
        fixture.host(),
        fixture.root.join(".git").join("disabled-analytics-hooks"),
    )?;
    let foreign_store = ProvenanceStore::open(
        foreign
            .parent()
            .ok_or_else(|| Error::type_error("Missing foreign root"))?,
        api::Identity::Explicit(format!("sha256-{}", "f".repeat(64))),
    )?;
    let token = CancellationSource::default();
    let refused = metrics::load_with_store(
        &git,
        &fixture.root,
        &metrics::Options::default(),
        &fixture.environment(),
        Some(&foreign_store),
        &token.token(),
    )
    .await;
    assert!(refused.is_err_and(|error| error.code == Some("ROSTER_ANALYTICS_IDENTITY")));
    assert_eq!(snapshot(&fixture.container)?, before);
    fs::write(
        root.join("log").join("bad.json"),
        "not-json test-only-native-private-record",
    )
    .map_err(io)?;
    let before = snapshot(&fixture.container)?;
    let corrupted = fixture.invoke(&["stats", "--json"])?;
    assert!(!corrupted.status.success());
    assert!(corrupted.stdout.is_empty());
    assert!(!utf8(&corrupted.stderr)?.contains("test-only-native-private-record"));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_recommend_insufficient_history_reads_only_owned_configuration_and_does_not_claim_routing()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    fixture.write(
        ".roster/config.yml",
        &include_str!("../roster.config.example.yml").replacen(
            "model: \"\"",
            "model: fallback-model",
            1,
        ),
    )?;
    let before = snapshot(&fixture.container)?;
    let output = fixture.invoke(&["recommend", "--task-class", "docs"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert_eq!(
        utf8(&output.stdout)?,
        "insufficient data; config default: fallback-model effort=m\n"
    );
    let output = fixture
        .command(
            &["recommend", "--task-class", "docs", "--json"],
            &fixture.root,
        )
        .env("ROSTER_ANALYTICS_CONFIG_DEFAULTS_ONLY", "true")
        .output()
        .map_err(io)?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    let output = parsed(&output.stdout)?;
    assert_eq!(output.get("recommendation"), Some(&Value::Null));
    assert!(!output.json().contains("fallback-model"));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_review_recommend_config_errors_omit_private_paths_and_preserve_sources()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let config = fixture.root.join(".roster").join("config.yml");
    files::create_private_directories(&config)?;
    for malformed in [false, true] {
        if malformed {
            fs::remove_dir(&config).map_err(io)?;
            fs::write(&config, "bad: &invalid-config\n").map_err(io)?;
        }
        let before = snapshot(&fixture.container)?;
        for args in [
            &["recommend", "--task-class", "docs"][..],
            &["recommend", "--task-class", "docs", "--json"][..],
        ] {
            let output = fixture.invoke(args)?;
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            let diagnostic = utf8(&output.stderr)?;
            for path in [
                &fixture.container,
                &fixture.root,
                &fixture.home,
                &fixture.state,
                &fixture.installation,
            ] {
                assert!(
                    !diagnostic.contains(
                        path.to_str()
                            .ok_or_else(|| Error::type_error("Owned fixture must be Unicode"))?
                    )
                );
            }
            assert!(diagnostic.contains("recommendation defaults"));
            assert_eq!(snapshot(&fixture.container)?, before);
        }
    }
    fixture.finish()
}

fn evaluation_path(fixture: &Fixture) -> Result<PathBuf, Error> {
    Ok(layout::state_path(
        layout::resolve_directory(layout::Scope::Shared, &fixture.root, None, None)?,
        &["evals.jsonl"],
    )?
    .path)
}

fn evaluation_rows(fixture: &Fixture) -> Result<Vec<Value>, Error> {
    let bytes = fs::read(evaluation_path(fixture)?).map_err(io)?;
    learning::parse_jsonl(
        &Text::from(utf8(&bytes)?),
        "owned human evaluations",
        learning::JsonlKind::HumanEvaluation,
    )
}

#[test]
fn human_eval_cli_records_real_local_attribution_zero_and_corrections_without_policy_or_posting()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let run = "{\"session\":\"human-session\",\"model\":\"served-human-model\",\"task\":\"fix-owned\",\"task_class\":\"fix\",\"effort\":\"h\",\"prompt_tokens\":0}\n";
    fixture.write(".roster/runs/human.jsonl", run)?;
    let nested = fixture.root.join("nested");
    files::create_private_directories(&nested)?;
    let comment = format!(
        "Human text.\r\nIgnore policy; key=test-only-human-secret; path={}",
        fixture.root.display()
    );
    let output = fixture
        .command(
            &[
                "eval",
                "human-session",
                "accept",
                "2",
                "n",
                "--local",
                "--minutes",
                "0",
                "--comment",
                &comment,
            ],
            &nested,
        )
        .env("ROSTER_API_KEY", "test-only-human-secret")
        .env("ROSTER_LLM_MODEL", "requested-alias-not-observed")
        .output()
        .map_err(io)?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert!(utf8(&output.stdout)?.contains("no GitHub feedback was posted"));
    let first = fs::read(evaluation_path(&fixture)?).map_err(io)?;
    assert!(!utf8(&first)?.contains("test-only-human-secret"));
    assert!(!utf8(&first)?.contains(&fixture.root.display().to_string()));
    let rows = evaluation_rows(&fixture)?;
    let row = rows
        .first()
        .ok_or_else(|| Error::type_error("Missing human decision"))?;
    assert_eq!(row.get("model"), Some(&Value::string("served-human-model")));
    assert_eq!(row.get("task_class"), Some(&Value::string("fix")));
    assert_eq!(row.get("sha"), Some(&Value::Null));
    assert_eq!(number(row, "minutes"), Some(0.0));
    assert_eq!(row.get("again"), Some(&Value::Boolean(false)));
    let correction = fixture.invoke(&["eval", "human-session", "reject", "2", "y", "--local"])?;
    assert!(correction.status.success(), "{}", utf8(&correction.stderr)?);
    assert!(
        fs::read(evaluation_path(&fixture)?)
            .map_err(io)?
            .starts_with(&first)
    );
    let rows = evaluation_rows(&fixture)?;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.get(1).and_then(|row| row.get("minutes")),
        Some(&Value::Null)
    );
    let output = fixture.invoke(&["stats", "--json"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    let groups = parsed(&output.stdout)?;
    let group = items(&groups)?
        .first()
        .ok_or_else(|| Error::type_error("Missing corrected group"))?;
    assert_eq!(number(group, "n"), Some(1.0));
    assert_eq!(number(group, "accepted"), Some(0.0));
    assert_eq!(
        fs::read_to_string(fixture.root.join(".roster/runs/human.jsonl")).map_err(io)?,
        run
    );
    assert!(fixture.git(&["status", "--porcelain"])?.is_empty());
    fixture.finish()
}

#[test]
fn human_eval_cli_unknown_metadata_and_missing_final_newline_remain_truthful_and_lossless()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let ledger = evaluation_path(&fixture)?;
    let parent = ledger
        .parent()
        .ok_or_else(|| Error::type_error("Evaluation needs a parent"))?;
    files::create_private_directories(parent)?;
    let earlier =
        b"{\"session\":\"earlier-human\",\"verdict\":\"accept\",\"difficulty\":1,\"again\":false}";
    fs::write(&ledger, earlier).map_err(io)?;
    let output = fixture.invoke(&["eval", "unknown-human", "rework", "4", "n", "--local"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert!(utf8(&output.stdout)?.contains("unknown fields remain unknown"));
    let bytes = fs::read(&ledger).map_err(io)?;
    assert!(bytes.starts_with(earlier));
    assert_eq!(bytes.get(earlier.len()), Some(&b'\n'));
    let rows = evaluation_rows(&fixture)?;
    assert_eq!(rows.len(), 2);
    let row = rows
        .get(1)
        .ok_or_else(|| Error::type_error("Missing unmatched human decision"))?;
    for field in ["sha", "model", "task_class", "minutes", "comment"] {
        assert_eq!(row.get(field), Some(&Value::Null), "{field}");
    }
    assert!(row.get("at").and_then(Value::as_string).is_some());
    fixture.finish()
}

#[test]
fn human_eval_cli_invalid_arguments_and_agent_app_contexts_have_no_side_effects()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let before = snapshot(&fixture.container)?;
    for args in [
        &["eval", "human-session", "accept", "2", "y"][..],
        &["eval", "human-session", "accept", "6", "y", "--local"][..],
        &[
            "eval",
            "human-session",
            "accept",
            "2",
            "y",
            "--local",
            "--post",
        ][..],
        &["eval", "../private", "accept", "2", "y", "--local"][..],
        &[
            "eval",
            "human-session",
            "accept",
            "2",
            "y",
            "--local",
            "--minutes",
            "-1",
        ][..],
    ] {
        let output = fixture.invoke(args)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(snapshot(&fixture.container)?, before);
    }
    for key in [
        "GITHUB_APP_ID",
        "GITHUB_APP_PRIVATE_KEY_PATH",
        "AI_MODEL",
        "ai_provider",
        "ROSTER_AGENT_SEAT",
    ] {
        let output = fixture
            .command(
                &["eval", "human-session", "accept", "2", "y", "--local"],
                &fixture.root,
            )
            .env(key, "")
            .env("ROSTER_GIT", fixture.container.join("must-not-execute.exe"))
            .output()
            .map_err(io)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(utf8(&output.stderr)?.contains("separate human session"));
        assert_eq!(snapshot(&fixture.container)?, before);
    }
    fixture.finish()
}

#[test]
fn human_eval_cli_resolves_sha1_sha256_and_abbreviations_without_requested_model_substitution()
-> Result<(), Error> {
    for sha256 in [false, true] {
        let fixture = Fixture::new(sha256)?;
        let head = fixture.commit(Some("reported-human-model"), "observed-coder")?;
        let abbreviation = head
            .get(..12)
            .ok_or_else(|| Error::type_error("Missing abbreviation"))?;
        for target in [&head, abbreviation, &head.to_ascii_uppercase()] {
            let output = fixture
                .command(
                    &["eval", target, "accept", "2", "y", "--local"],
                    &fixture.root,
                )
                .env("ROSTER_LLM_MODEL", "not-the-observed-model")
                .output()
                .map_err(io)?;
            assert!(output.status.success(), "{}", utf8(&output.stderr)?);
        }
        let rows = evaluation_rows(&fixture)?;
        assert_eq!(rows.len(), 3);
        for row in rows {
            assert_eq!(row.get("sha"), Some(&Value::string(&head)));
            assert_eq!(row.get("session"), Some(&Value::string("observed-coder")));
            assert_eq!(
                row.get("model"),
                Some(&Value::string("reported-human-model"))
            );
            assert_eq!(row.get("task_class"), Some(&Value::string("fix")));
        }
        fixture.finish()?;
    }
    Ok(())
}

#[test]
fn human_eval_attribution_refuses_conflicting_local_session_models_and_accepts_explicit_shas()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let first = fixture.commit(Some("first-observed-model"), "shared-human-session")?;
    let second = fixture.commit(Some("second-observed-model"), "shared-human-session")?;
    let output = fixture.invoke(&[
        "eval",
        "shared-human-session",
        "accept",
        "2",
        "y",
        "--local",
    ])?;
    assert!(
        !output.status.success(),
        "Ambiguous local attribution was recorded"
    );
    assert!(output.stdout.is_empty());
    assert!(utf8(&output.stderr)?.contains("Ambiguous session attribution"));
    assert!(!evaluation_path(&fixture)?.exists());
    for sha in [&first, &second] {
        let output = fixture.invoke(&["eval", sha, "accept", "2", "y", "--local"])?;
        assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    }
    let rows = evaluation_rows(&fixture)?;
    assert_eq!(rows.len(), 2);
    for (sha, model) in [
        (&first, "first-observed-model"),
        (&second, "second-observed-model"),
    ] {
        assert!(rows.iter().any(|row| {
            row.get("sha") == Some(&Value::string(sha))
                && row.get("model") == Some(&Value::string(model))
        }));
    }
    fixture.finish()
}

#[test]
fn human_eval_attribution_refuses_private_model_metadata_in_prior_human_rows_without_echoing_it()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let endpoint = "https://test-only-private-endpoint.invalid/v1/model";
    let source = format!(
        "{{\"session\":\"prior-human\",\"model\":\"{endpoint}\",\"task_class\":\"fix\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true}}\n"
    );
    fixture.write(".roster/evals.jsonl", &source)?;
    let output = fixture.invoke(&["eval", "prior-human", "reject", "2", "n", "--local"])?;
    assert!(
        !output.status.success(),
        "Private prior attribution was duplicated"
    );
    assert!(output.stdout.is_empty());
    assert!(!utf8(&output.stderr)?.contains(endpoint));
    assert_eq!(
        fs::read(evaluation_path(&fixture)?).map_err(io)?,
        source.as_bytes()
    );
    fixture.finish()
}

#[test]
fn human_eval_cli_preserves_malformed_and_hardlinked_ledgers_and_enforces_exact_row_limit()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let ledger = evaluation_path(&fixture)?;
    files::create_private_directories(
        ledger
            .parent()
            .ok_or_else(|| Error::type_error("Evaluation needs a parent"))?,
    )?;
    for content in [b"not-json\n".as_slice(), b"[]\n".as_slice()] {
        fs::write(&ledger, content).map_err(io)?;
        let output = fixture.invoke(&["eval", "human-session", "accept", "2", "y", "--local"])?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read(&ledger).map_err(io)?, content);
    }
    let outside = fixture.container.join("outside-owned-human.jsonl");
    let row =
        "{\"session\":\"earlier-human\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true}\n";
    fs::write(&outside, row).map_err(io)?;
    fs::remove_file(&ledger).map_err(io)?;
    fs::hard_link(&outside, &ledger).map_err(io)?;
    let output = fixture.invoke(&["eval", "human-session", "accept", "2", "y", "--local"])?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&outside).map_err(io)?, row.as_bytes());
    fs::remove_file(&ledger).map_err(io)?;
    fs::write(&ledger, row.repeat(learning::MAX_ROWS - 1)).map_err(io)?;
    let output = fixture.invoke(&["eval", "last-human", "accept", "2", "n", "--local"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert_eq!(evaluation_rows(&fixture)?.len(), learning::MAX_ROWS);
    let full = fs::read(&ledger).map_err(io)?;
    let output = fixture.invoke(&["eval", "one-too-many", "accept", "2", "n", "--local"])?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&ledger).map_err(io)?, full);
    fixture.finish()
}

async fn human_gh(fixture: &Fixture, mode: &str) -> Result<(PathBuf, PathBuf), Error> {
    let image = compile_probe(
        fixture,
        "human_feedback_probe",
        include_str!("support/human_gh_probe.rs"),
    )
    .await?;
    let directory = fixture.home.join("human-gh");
    files::create_private_directories(&directory)?;
    fs::write(directory.join("mode"), mode).map_err(io)?;
    Ok((image, directory))
}

fn human_post_command(fixture: &Fixture, image: &Path, directory: &Path, args: &[&str]) -> Command {
    let mut command = fixture.command(args, &fixture.root);
    command
        .env("ROSTER_GH", image)
        .env("GH_CONFIG_DIR", directory)
        .env("GH_TOKEN", "test-only-explicit-human-token");
    command
}

#[tokio::test(flavor = "current_thread")]
async fn human_post_cli_posts_only_verified_compact_feedback_and_keeps_private_comments_local()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let head = fixture.commit(Some("served-human-model"), "posted-human")?;
    let (image, directory) = human_gh(&fixture, "ok").await?;
    let output = human_post_command(
        &fixture,
        &image,
        &directory,
        &[
            "eval",
            &head,
            "accept",
            "3",
            "y",
            "--post",
            "--minutes",
            "0",
            "--comment",
            "LOCAL-ONLY-HUMAN-TEXT test-only-explicit-human-token",
        ],
    )
    .env("GITHUB_TOKEN", "unselected-private-credential")
    .output()
    .map_err(io)?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert!(utf8(&output.stdout)?.contains("https://github.com/owned/analytics/pull/42"));
    let posted = parsed(&fs::read(directory.join("posted.json")).map_err(io)?)?;
    assert_eq!(
        posted.get("body"),
        Some(&Value::string("AI-Eval: 1|accept|3|y\nMinutes: 0"))
    );
    assert!(!posted.json().contains("LOCAL-ONLY"));
    assert!(!posted.json().contains("human-token"));
    let ledger = fs::read(evaluation_path(&fixture)?).map_err(io)?;
    assert!(!utf8(&ledger)?.contains("test-only-explicit-human-token"));
    let calls = fs::read_to_string(directory.join("calls")).map_err(io)?;
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("POST "))
            .count(),
        1
    );
    let local = human_post_command(
        &fixture,
        &image,
        &directory,
        &["eval", &head, "reject", "3", "n", "--local"],
    )
    .env("ROSTER_GH", fixture.container.join("must-not-execute.exe"))
    .env_remove("GH_TOKEN")
    .output()
    .map_err(io)?;
    assert!(local.status.success(), "{}", utf8(&local.stderr)?);
    assert_eq!(
        fs::read_to_string(directory.join("calls")).map_err(io)?,
        calls
    );
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn human_post_cli_refuses_bot_ambiguous_foreign_and_malformed_preflight_without_a_saved_decision()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let head = fixture.commit(Some("served-human-model"), "posted-human")?;
    let (image, directory) = human_gh(&fixture, "bot").await?;
    for mode in [
        "bot",
        "bad-user",
        "ambiguous",
        "foreign",
        "malformed",
        "read-fail",
    ] {
        fs::write(directory.join("mode"), mode).map_err(io)?;
        let output = human_post_command(
            &fixture,
            &image,
            &directory,
            &["eval", &head, "accept", "2", "y", "--post"],
        )
        .output()
        .map_err(io)?;
        assert!(!output.status.success(), "{mode}");
        assert!(output.stdout.is_empty());
        assert!(!utf8(&output.stderr)?.contains("test-only-explicit-human-token"));
        assert!(!evaluation_path(&fixture)?.exists(), "{mode}");
        assert!(!directory.join("posted.json").exists(), "{mode}");
    }
    let calls = fs::read_to_string(directory.join("calls")).map_err(io)?;
    assert!(!calls.lines().any(|line| line.starts_with("POST ")));
    let refused = human_post_command(
        &fixture,
        &image,
        &directory,
        &["eval", &head, "accept", "2", "y", "--post"],
    )
    .env("GITHUB_APP_ID", "123")
    .env(
        "GITHUB_APP_PRIVATE_KEY_PATH",
        fixture.container.join("must-not-read.pem"),
    )
    .output()
    .map_err(io)?;
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(directory.join("calls")).map_err(io)?,
        calls
    );
    assert!(!evaluation_path(&fixture)?.exists());
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn human_post_cli_reports_no_sha_or_pr_explicitly_and_never_loads_an_implicit_credential()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let head = fixture.commit(Some("served-human-model"), "posted-human")?;
    let unknown = fixture.invoke(&["eval", "unknown-human", "accept", "2", "y", "--post"])?;
    assert!(unknown.status.success(), "{}", utf8(&unknown.stderr)?);
    assert!(utf8(&unknown.stdout)?.contains("No published SHA is known"));
    let (image, directory) = human_gh(&fixture, "no-pr").await?;
    let output = human_post_command(
        &fixture,
        &image,
        &directory,
        &["eval", &head, "accept", "2", "y", "--post"],
    )
    .output()
    .map_err(io)?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert!(utf8(&output.stdout)?.contains("No matching pull request"));
    assert!(!directory.join("posted.json").exists());
    let before = fs::read(evaluation_path(&fixture)?).map_err(io)?;
    let calls = fs::read(directory.join("calls")).map_err(io)?;
    let missing = human_post_command(
        &fixture,
        &image,
        &directory,
        &["eval", &head, "reject", "2", "n", "--post"],
    )
    .env_remove("GH_TOKEN")
    .env_remove("GITHUB_TOKEN")
    .output()
    .map_err(io)?;
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(utf8(&missing.stderr)?.contains("explicit human GH_TOKEN"));
    assert_eq!(fs::read(evaluation_path(&fixture)?).map_err(io)?, before);
    assert_eq!(fs::read(directory.join("calls")).map_err(io)?, calls);
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn human_post_cli_preserves_saved_decisions_on_uncertain_posts_and_never_retries_or_follows_origin_drift()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let head = fixture.commit(Some("served-human-model"), "posted-human")?;
    let (image, directory) = human_gh(&fixture, "post-fail").await?;
    for (index, mode) in ["post-fail", "bad-ack", "drift"].iter().enumerate() {
        fs::write(directory.join("mode"), mode).map_err(io)?;
        let before = match fs::read_to_string(directory.join("calls")) {
            Ok(calls) => calls
                .lines()
                .filter(|line| line.starts_with("POST "))
                .count(),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => 0,
            Err(cause) => return Err(io(cause)),
        };
        let output = human_post_command(
            &fixture,
            &image,
            &directory,
            &[
                "eval",
                &head,
                "reject",
                "2",
                "n",
                "--post",
                "--comment",
                "LOCAL-ONLY-HUMAN-TEXT",
            ],
        )
        .output()
        .map_err(io)?;
        assert!(!output.status.success(), "{mode}");
        assert!(output.stdout.is_empty());
        let diagnostic = utf8(&output.stderr)?;
        assert!(
            diagnostic.contains("local human decision was saved"),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("test-only-explicit-human-token"));
        assert!(!diagnostic.contains("LOCAL-ONLY-HUMAN-TEXT"));
        assert_eq!(evaluation_rows(&fixture)?.len(), index + 1);
        let calls = fs::read_to_string(directory.join("calls")).map_err(io)?;
        let after = calls
            .lines()
            .filter(|line| line.starts_with("POST "))
            .count();
        assert_eq!(after - before, usize::from(*mode != "drift"));
    }
    fixture.finish()
}

fn human_post_environment(fixture: &Fixture, image: &Path, directory: &Path) -> Value {
    Value::object(vec![
        (
            "HOME",
            Value::string(fixture.home.to_string_lossy().as_ref()),
        ),
        (
            "USERPROFILE",
            Value::string(fixture.home.to_string_lossy().as_ref()),
        ),
        (
            "ROSTER_STATE_HOME",
            Value::string(fixture.state.to_string_lossy().as_ref()),
        ),
        (
            "ROSTER_GIT",
            Value::string(fixture.git.to_string_lossy().as_ref()),
        ),
        ("ROSTER_GH", Value::string(image.to_string_lossy().as_ref())),
        (
            "GH_CONFIG_DIR",
            Value::string(directory.to_string_lossy().as_ref()),
        ),
        ("GH_TOKEN", Value::string("test-only-explicit-human-token")),
    ])
}

async fn await_human_post_ready(
    directory: &Path,
    operation: &mut std::pin::Pin<
        Box<impl std::future::Future<Output = Result<evaluation::Recorded, Error>>>,
    >,
) -> Result<(), Error> {
    let ready = async {
        loop {
            if directory.join("ready").is_file() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    tokio::select! {
        result = operation => Err(Error::new("FixtureError", match result {
            Ok(_) => "Owned post unexpectedly completed before its readiness barrier".to_owned(),
            Err(error) => format!("Owned post stopped before readiness: {error}"),
        })),
        result = tokio::time::timeout(std::time::Duration::from_secs(15), ready) => result.map_err(|_| Error::type_error("Owned human post never reached its actual submission barrier")),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn human_post_cancellation_waits_for_the_owned_provider_and_preserves_the_saved_human_row()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let head = fixture.commit(Some("served-human-model"), "posted-human")?;
    let (image, directory) = human_gh(&fixture, "post-hold").await?;
    let environment = human_post_environment(&fixture, &image, &directory);
    let decision = evaluation::Decision::parse(
        &["eval", &head, "accept", "2", "y", "--post"]
            .iter()
            .skip(1)
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
    )?;
    let source = CancellationSource::default();
    let token = source.token();
    let mut operation = Box::pin(evaluation::record(
        &fixture.root,
        &environment,
        &decision,
        &token,
    ));
    await_human_post_ready(&directory, &mut operation).await?;
    source.cancel(github_agent_roster::cancel::Reason::Cancelled)?;
    let failure = tokio::time::timeout(std::time::Duration::from_secs(15), operation)
        .await
        .map_err(|_| Error::type_error("Owned human post cancellation did not settle"))?
        .err()
        .ok_or_else(|| Error::type_error("Cancelled human post reported success"))?;
    assert_eq!(failure.code, Some("ROSTER_CANCELLED"));
    assert!(failure.message.contains("local human decision was saved"));
    assert_eq!(evaluation_rows(&fixture)?.len(), 1);
    assert!(
        !evaluation_path(&fixture)?
            .with_file_name("evals.lock")
            .exists()
    );
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn human_post_dropping_pending_submission_retains_the_writer_lock_and_saved_record()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    let head = fixture.commit(Some("served-human-model"), "posted-human")?;
    let (image, directory) = human_gh(&fixture, "post-hold").await?;
    let environment = human_post_environment(&fixture, &image, &directory);
    let decision = evaluation::Decision::parse(
        &[head.as_str(), "accept", "2", "y", "--post"].map(str::to_owned),
    )?;
    let source = CancellationSource::default();
    let token = source.token();
    let mut operation = Box::pin(evaluation::record(
        &fixture.root,
        &environment,
        &decision,
        &token,
    ));
    await_human_post_ready(&directory, &mut operation).await?;
    drop(operation);
    assert_eq!(evaluation_rows(&fixture)?.len(), 1);
    let path = evaluation_path(&fixture)?.with_file_name("evals.lock");
    assert!(path.is_file());
    let refused = github_agent_roster::state::locks::acquire(
        "evaluations",
        &path,
        &github_agent_roster::state::locks::Options::default(),
        Some(&token),
    )
    .await
    .err()
    .ok_or_else(|| Error::type_error("Dropped human post released writer authority"))?;
    assert_eq!(refused.code, Some("E_LOCK_HELD"));
    Ok(())
}

fn evaluation_prefix(bytes: usize) -> Result<Vec<u8>, Error> {
    let mut output = Vec::with_capacity(bytes);
    for index in 0..16 {
        let prefix = format!(
            "{{\"session\":\"byte-history-{index}\",\"verdict\":\"accept\",\"difficulty\":1,\"again\":true,\"padding\":\""
        );
        let suffix = "\"}\n";
        let length = bytes / 16 + usize::from(index < bytes % 16);
        let padding = length
            .checked_sub(prefix.len() + suffix.len())
            .ok_or_else(|| Error::type_error("Owned byte fixture must fit its header"))?;
        output.extend_from_slice(prefix.as_bytes());
        output.resize(output.len() + padding, b'a');
        output.extend_from_slice(suffix.as_bytes());
    }
    Ok(output)
}

#[test]
fn human_eval_cli_accepts_exact_sixteen_mebibytes_and_refuses_one_byte_more_without_loss()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let args = ["eval", "size-human", "accept", "2", "y", "--local"];
    let prototype = fixture.invoke(&args)?;
    assert!(prototype.status.success(), "{}", utf8(&prototype.stderr)?);
    let ledger = evaluation_path(&fixture)?;
    let row_bytes = fs::read(&ledger).map_err(io)?.len();
    let prefix = evaluation_prefix(learning::MAX_JSONL_BYTES - row_bytes)?;
    fs::write(&ledger, &prefix).map_err(io)?;
    let output = fixture.invoke(&args)?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    let exact = fs::read(&ledger).map_err(io)?;
    assert_eq!(exact.len(), learning::MAX_JSONL_BYTES);
    assert!(exact.starts_with(&prefix));
    let over = evaluation_prefix(learning::MAX_JSONL_BYTES - row_bytes + 1)?;
    fs::write(&ledger, &over).map_err(io)?;
    let output = fixture.invoke(&args)?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&ledger).map_err(io)?, over);
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn human_eval_cli_concurrent_local_decisions_serialize_without_dropping_a_row()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let environment = Value::object(vec![
        (
            "HOME",
            Value::string(fixture.home.to_string_lossy().as_ref()),
        ),
        (
            "USERPROFILE",
            Value::string(fixture.home.to_string_lossy().as_ref()),
        ),
        (
            "ROSTER_STATE_HOME",
            Value::string(fixture.state.to_string_lossy().as_ref()),
        ),
        (
            "ROSTER_GIT",
            Value::string(fixture.git.to_string_lossy().as_ref()),
        ),
    ]);
    let decision = |session: &str| {
        evaluation::Decision::parse(
            &["unused", "accept", "2", "n", "--local"]
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    if index == 0 {
                        session.to_owned()
                    } else {
                        (*value).to_owned()
                    }
                })
                .collect::<Vec<_>>(),
        )
    };
    let first = decision("first-human")?;
    let second = decision("second-human")?;
    let cancellation = CancellationSource::default();
    let token = cancellation.token();
    let (first, second) = tokio::join!(
        evaluation::record_local(&fixture.root, &environment, &first, &token),
        evaluation::record_local(&fixture.root, &environment, &second, &token),
    );
    first?;
    second?;
    let rows = evaluation_rows(&fixture)?;
    assert_eq!(rows.len(), 2);
    let sessions = rows
        .iter()
        .filter_map(|row| row.get("session").and_then(Value::as_string))
        .map(Text::to_utf8)
        .collect::<Result<BTreeSet<_>, _>>()?;
    assert_eq!(
        sessions,
        BTreeSet::from(["first-human".to_owned(), "second-human".to_owned()])
    );
    fixture.finish()
}

async fn evaluation_session(
    store: &mut ProvenanceStore,
    session: &str,
    served: Option<&str>,
    lineage: Option<&str>,
) -> Result<(), Error> {
    let evidence = lineage.map_or_else(
        || Value::object(Vec::new()),
        |lineage| {
            Value::object(vec![(
                "attempt",
                Value::object(vec![("sessionId", Value::string(lineage))]),
            )])
        },
    );
    store
        .record_event(
            &Value::object(vec![
                ("runId", Value::string(&format!("run-{session}"))),
                ("sessionId", Value::string(session)),
                ("event", Value::string("session")),
                (
                    "payload",
                    Value::object(vec![
                        (
                            "seat",
                            Value::object(vec![("name", Value::string("coder"))]),
                        ),
                        (
                            "issue",
                            Value::object(vec![("task", Value::string("fix-human"))]),
                        ),
                        (
                            "requestedModel",
                            Value::string("requested-alias-not-served"),
                        ),
                        ("servedModel", served.map_or(Value::Null, Value::string)),
                        ("task_class", Value::string("fix")),
                        ("evidence", evidence),
                    ]),
                ),
            ]),
            0,
            None,
        )
        .await?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn human_eval_cli_scopes_durable_lineage_refuses_ambiguity_and_keeps_unknown_served_models()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let identity = identity::identity_hash(&Text::from(ORIGIN))?;
    let root = fixture.state.join("history").join(&identity);
    let mut store = ProvenanceStore::open(&root, api::Identity::Explicit(identity))?;
    evaluation_session(
        &mut store,
        "attempt-one",
        Some("served-human-model"),
        Some("operational-human"),
    )
    .await?;
    let output = fixture.invoke(&["eval", "operational-human", "accept", "2", "y", "--local"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    let rows = evaluation_rows(&fixture)?;
    let row = rows
        .first()
        .ok_or_else(|| Error::type_error("Missing recovered decision"))?;
    assert_eq!(row.get("session"), Some(&Value::string("attempt-one")));
    assert_eq!(row.get("model"), Some(&Value::string("served-human-model")));
    assert_eq!(row.get("sha"), Some(&Value::Null));
    assert_eq!(
        row.get("provenance")
            .and_then(|reference| reference.get("sessionId")),
        Some(&Value::string("attempt-one"))
    );
    let before = fs::read(evaluation_path(&fixture)?).map_err(io)?;
    evaluation_session(
        &mut store,
        "attempt-two",
        Some("another-served-model"),
        Some("operational-human"),
    )
    .await?;
    let ambiguous =
        fixture.invoke(&["eval", "operational-human", "reject", "2", "n", "--local"])?;
    assert!(!ambiguous.status.success());
    assert!(ambiguous.stdout.is_empty());
    assert!(utf8(&ambiguous.stderr)?.contains("Multiple durable session records"));
    assert_eq!(fs::read(evaluation_path(&fixture)?).map_err(io)?, before);
    evaluation_session(&mut store, "unknown-attempt", None, None).await?;
    let output = fixture.invoke(&["eval", "unknown-attempt", "accept", "2", "y", "--local"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert_eq!(
        evaluation_rows(&fixture)?
            .last()
            .and_then(|row| row.get("model")),
        Some(&Value::Null)
    );
    let runs = layout::state_path(
        layout::resolve_directory(layout::Scope::Shared, &fixture.root, None, None)?,
        &["runs", "local.jsonl"],
    )?
    .path;
    files::create_private_directories(
        runs.parent()
            .ok_or_else(|| Error::type_error("Runs need a parent"))?,
    )?;
    fs::write(&runs, "{\"session\":\"operational-human\",\"model\":\"local-observed-model\",\"task_class\":\"fix\"}\n").map_err(io)?;
    fs::write(
        root.join("log").join("malformed.json"),
        "not-json-owned-history",
    )
    .map_err(io)?;
    let output = fixture.invoke(&["eval", "operational-human", "accept", "2", "y", "--local"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert_eq!(
        evaluation_rows(&fixture)?
            .last()
            .and_then(|row| row.get("model")),
        Some(&Value::string("local-observed-model"))
    );
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn human_eval_cli_read_only_acceptance_refuses_before_executing_the_human_mutator()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let assignment = fixture.task(
        &["docs/guide.md"],
        &["`roster eval human-session accept 2 y --local` must not execute."],
    )?;
    let (probe, marker) = read_probe(&fixture).await?;
    let cancellation = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        assignment,
        tools::Options::default(),
        fixture.host(),
        redaction::Config::default(),
        cancellation.token(),
    )
    .await?;
    let result = e2e::run(
        &boundary,
        &backend::Approval {
            native_cli: Some(probe),
            ..Default::default()
        },
    )
    .await;
    let unexecuted = !marker.exists();
    let cleanup = boundary.close().await;
    github_agent_roster::state::locks::finish(
        result.map(|report| {
            assert!(
                !report.passed(),
                "Human eval became a read-only acceptance pass"
            );
            assert_eq!(
                report.as_value().get("status"),
                Some(&Value::string("unavailable"))
            );
            assert_eq!(
                report.as_value().get("runs"),
                Some(&Value::Array(Vec::new()))
            );
            assert!(report.as_value().get("failures").is_some_and(|failures| {
                failures
                    .json()
                    .contains("not approved read-only native authority")
            }));
        }),
        cleanup,
    )?;
    assert!(unexecuted, "Read-only acceptance executed human eval");
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn analytics_read_only_acceptance_runs_the_actual_compiled_cli_and_invalid_option_controls()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(Some("observed"), "observed")?;
    let assignment = fixture.task(
        &["docs/guide.md"],
        &[
            "`roster stats --json` prints bounded JSON.",
            "`roster recommend --task-class fix --json` prints bounded JSON.",
            "`roster status --issue 42 --json` reads local observed metadata only.",
        ],
    )?;
    let cancellation = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        assignment,
        tools::Options::default(),
        fixture.host(),
        redaction::Config::default(),
        cancellation.token(),
    )
    .await?;
    let result = async {
        let approval = backend::Approval {
            native_cli: Some(PathBuf::from(env!("CARGO_BIN_EXE_roster"))),
            ..Default::default()
        };
        let report = e2e::run(&boundary, &approval).await?;
        assert!(report.passed(), "{}", report.as_value().json());
        let runs = report
            .as_value()
            .get("runs")
            .ok_or_else(|| Error::type_error("Missing native acceptance runs"))?;
        assert_eq!(items(runs)?.len(), 3);
        for run in items(runs)? {
            assert_eq!(number(run, "exitCode"), Some(0.0));
            assert!(
                run.get("errorCase")
                    .and_then(|case| number(case, "exitCode"))
                    .is_some_and(|code| code != 0.0)
            );
            let output = run
                .get("output")
                .and_then(Value::as_string)
                .ok_or_else(|| Error::type_error("Missing actual CLI output"))?;
            json::parse(output)?;
        }
        report.require_current(&boundary).await?;
        Ok(())
    }
    .await;
    github_agent_roster::state::locks::finish(result, boundary.close().await)?;
    fixture.finish()
}

#[cfg(windows)]
async fn read_probe(fixture: &Fixture) -> Result<(PathBuf, PathBuf), Error> {
    let marker = fixture.container.join("probe-invoked");
    let source = format!(
        "fn main() -> std::io::Result<()> {{ std::fs::write({:?}, b\"invoked\") }}\n",
        marker
            .to_str()
            .ok_or_else(|| Error::type_error("Probe path must be Unicode"))?
    );
    Ok((
        compile_probe(fixture, "analytics_read_probe", &source).await?,
        marker,
    ))
}

async fn compile_probe(fixture: &Fixture, name: &str, source: &str) -> Result<PathBuf, Error> {
    let directory = fixture.container.join(name);
    files::create_private_directories(&directory.join("src"))?;
    fs::write(
        directory.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n"),
    )
    .map_err(io)?;
    fs::write(
        directory.join("Cargo.lock"),
        format!("version = 3\n\n[[package]]\nname = \"{name}\"\nversion = \"0.0.0\"\n"),
    )
    .map_err(io)?;
    fs::write(directory.join("src").join("main.rs"), source).map_err(io)?;
    let cargo = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .ok_or_else(|| Error::type_error("Owned probe requires the current Cargo image"))?;
    let mut command = tokio::process::Command::new(cargo);
    command
        .current_dir(&directory)
        .env_clear()
        .envs(tools::Host::default().environment);
    for name in ["CARGO_HOME", "RUSTUP_HOME", "RUSTFLAGS"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if let Some(toolchain) = std::env::var_os("ROSTER_TEST_CARGO_TOOLCHAIN") {
        command.env("RUSTUP_TOOLCHAIN", toolchain);
    }
    command
        .env("CARGO_TARGET_DIR", directory.join("target"))
        .env("CARGO_NET_OFFLINE", "true")
        .env("RUSTUP_AUTO_INSTALL", "0")
        .args(["build", "--locked", "--offline"]);
    let output = github_agent_roster::execution::owned::execute(
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
            "AnalyticsFixtureError",
            format!("Compile matching-toolchain owned read probe: {failure}"),
        )
    })?;
    if !output.status.success() {
        return Err(Error::new(
            "OwnedCompileError",
            format!(
                "Owned native probe compilation failed: {}",
                utf8(&output.captured.stderr)?
            ),
        ));
    }
    Ok(directory
        .join("target")
        .join("debug")
        .join(format!("{name}.exe")))
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn analytics_read_only_acceptance_refuses_private_outside_dash_link_and_unsafe_ref_operands_before_execution()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    fixture.write("private/evals.jsonl", "")?;
    fixture.write(".env", "")?;
    fixture.write("allowed.jsonl", "")?;
    fs::write(fixture.container.join("outside.jsonl"), "").map_err(io)?;
    fs::hard_link(
        fixture.root.join("allowed.jsonl"),
        fixture.root.join("linked.jsonl"),
    )
    .map_err(io)?;
    let (probe, marker) = read_probe(&fixture).await?;
    for operand in [
        "private/evals.jsonl",
        ".env",
        "../outside.jsonl",
        "-unchecked/../private/evals.jsonl",
        "linked.jsonl",
        "allowed.jsonl --ref --all",
        "allowed.jsonl --ref HEAD..--all",
    ] {
        let assignment = fixture.task(
            &["docs/guide.md", "allowed.jsonl", "linked.jsonl"],
            &[&format!("`roster stats --evals {operand}` is inspected.")],
        )?;
        let cancellation = CancellationSource::default();
        let mut boundary = tools::Boundary::open(
            &fixture.root,
            assignment,
            tools::Options::default(),
            fixture.host(),
            redaction::Config::default(),
            cancellation.token(),
        )
        .await?;
        let result = async {
            let approval = backend::Approval {
                native_cli: Some(probe.clone()),
                ..Default::default()
            };
            let outcome = e2e::run(&boundary, &approval).await;
            assert!(
                !marker.exists(),
                "Unsafe analytics operand executed the native CLI: {operand}"
            );
            match outcome {
                Err(error) => assert_eq!(error.code, Some("ROSTER_TOOL_DENIED")),
                Ok(report) => assert!(!report.passed(), "Unsafe analytics input passed: {operand}"),
            }
            Ok(())
        }
        .await;
        github_agent_roster::state::locks::finish(result, boundary.close().await)?;
    }
    fs::remove_file(fixture.root.join("linked.jsonl")).map_err(io)?;
    let assignment = fixture.task(
        &["docs/guide.md", "allowed.jsonl"],
        &["`roster stats --evals allowed.jsonl --json` prints bounded JSON."],
    )?;
    let cancellation = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        assignment,
        tools::Options::default(),
        fixture.host(),
        redaction::Config::default(),
        cancellation.token(),
    )
    .await?;
    let result = async {
        let report = e2e::run(
            &boundary,
            &backend::Approval {
                native_cli: Some(PathBuf::from(env!("CARGO_BIN_EXE_roster"))),
                ..Default::default()
            },
        )
        .await?;
        assert!(report.passed(), "{}", report.as_value().json());
        Ok(())
    }
    .await;
    github_agent_roster::state::locks::finish(result, boundary.close().await)?;
    assert!(!marker.exists());
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_cli_hardlinked_or_nonregular_sources_are_refused_without_modification()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(Some("observed"), "observed")?;
    fixture.write("one.jsonl", "")?;
    fs::hard_link(
        fixture.root.join("one.jsonl"),
        fixture.root.join("two.jsonl"),
    )
    .map_err(io)?;
    let before = snapshot(&fixture.container)?;
    let output = fixture.invoke(&["stats", "--json", "--evals", "one.jsonl"])?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.write(".roster/runs", "")?;
    let before = snapshot(&fixture.container)?;
    let output = fixture.invoke(&["stats", "--json"])?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_cli_sharing_denial_and_nonrepository_errors_never_become_empty_success()
-> Result<(), Error> {
    use std::os::windows::fs::OpenOptionsExt;

    let fixture = Fixture::new(false)?;
    fixture.commit(Some("observed"), "observed")?;
    fixture.write(
        ".roster/evals.jsonl",
        "{\"session\":\"observed\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true}\n",
    )?;
    let before = snapshot(&fixture.container)?;
    let denied = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(fixture.root.join(".roster").join("evals.jsonl"))
        .map_err(io)?;
    let output = fixture.invoke(&["stats", "--json"])?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!utf8(&output.stderr)?.contains(&fixture.container.to_string_lossy().into_owned()));
    drop(denied);
    assert_eq!(snapshot(&fixture.container)?, before);
    let not_repository = fixture.container.join("not-a-repository");
    files::create_private_directories(&not_repository.join(".git"))?;
    let before = snapshot(&fixture.container)?;
    let output = fixture
        .command(&["stats", "--json"], &not_repository)
        .output()
        .map_err(io)?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!utf8(&output.stderr)?.contains(&fixture.container.to_string_lossy().into_owned()));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_cli_run_directory_and_jsonl_file_byte_limits_are_exact_and_read_only()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    for index in 0..metrics::MAX_DIRECTORY_ENTRIES {
        fixture.write(&format!(".roster/runs/owned-{index:03}.jsonl"), "")?;
    }
    let before = snapshot(&fixture.container)?;
    let exact = fixture.invoke(&["stats", "--json"])?;
    assert!(exact.status.success(), "{}", utf8(&exact.stderr)?);
    assert_eq!(utf8(&exact.stdout)?, "[]\n");
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.write(".roster/runs/over.jsonl", "")?;
    let before = snapshot(&fixture.container)?;
    let over = fixture.invoke(&["stats", "--json"])?;
    assert!(!over.status.success());
    assert!(over.stdout.is_empty());
    assert_eq!(snapshot(&fixture.container)?, before);
    for entry in fs::read_dir(fixture.root.join(".roster").join("runs")).map_err(io)? {
        fs::remove_file(entry.map_err(io)?.path()).map_err(io)?;
    }
    let prefix = "{\"session\":\"owned\",\"padding\":\"";
    let suffix = "\"}\n";
    let line = format!(
        "{prefix}{}{suffix}",
        "a".repeat(github_agent_roster::learning::MAX_LINE_BYTES - prefix.len() - suffix.len())
    );
    let source = line.repeat(
        github_agent_roster::learning::MAX_JSONL_BYTES
            / github_agent_roster::learning::MAX_LINE_BYTES,
    );
    fixture.write(".roster/runs/exact.jsonl", &source)?;
    let before = snapshot(&fixture.container)?;
    let exact = fixture.invoke(&["stats", "--json"])?;
    assert!(exact.status.success(), "{}", utf8(&exact.stderr)?);
    assert_eq!(
        number(
            items(&parsed(&exact.stdout)?)?
                .first()
                .ok_or_else(|| Error::type_error("Missing model-free run group"))?,
            "runs"
        ),
        Some(1.0)
    );
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.write(".roster/runs/exact.jsonl", &format!("{source} "))?;
    let before = snapshot(&fixture.container)?;
    let over = fixture.invoke(&["stats", "--json"])?;
    assert!(!over.status.success());
    assert!(over.stdout.is_empty());
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_linked_worktrees_read_only_canonical_repository_state_and_refuse_foreign_identity_markers()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(Some("observed"), "observed")?;
    fixture.write(
        ".roster/evals.jsonl",
        "{\"session\":\"observed\",\"verdict\":\"accept\",\"difficulty\":2,\"again\":true}\n",
    )?;
    let linked = fixture.container.join("linked");
    fixture.git(&[
        "worktree",
        "add",
        "--quiet",
        "--detach",
        linked
            .to_str()
            .ok_or_else(|| Error::type_error("Linked fixture path must be Unicode"))?,
        "HEAD",
    ])?;
    files::create_private_directories(&linked.join(".roster"))?;
    fs::write(
        linked.join(".roster").join("evals.jsonl"),
        "not-json test-only-issue-scratch-not-repository-history",
    )
    .map_err(io)?;
    let before = snapshot(&fixture.container)?;
    let output = fixture
        .command(&["stats", "--json"], &linked)
        .output()
        .map_err(io)?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert_eq!(
        number(
            items(&parsed(&output.stdout)?)?
                .first()
                .ok_or_else(|| Error::type_error("Missing canonical group"))?,
            "n"
        ),
        Some(1.0)
    );
    assert!(!utf8(&output.stdout)?.contains("test-only-issue-scratch"));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.write(".roster/identity", &format!("sha256-{}\n", "f".repeat(64)))?;
    fixture.write(
        ".roster/evals.jsonl",
        "not-json test-only-foreign-local-ledger",
    )?;
    let before = snapshot(&fixture.container)?;
    let refused = fixture
        .command(&["stats", "--json"], &linked)
        .output()
        .map_err(io)?;
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    assert!(utf8(&refused.stderr)?.contains("different repository identity"));
    assert!(!utf8(&refused.stderr)?.contains("test-only-foreign-local-ledger"));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[cfg(windows)]
#[test]
fn analytics_cli_never_emits_raw_paths_endpoints_or_known_secrets_as_model_metadata()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    for model in [
        "https://private.invalid/v1",
        "C:/Users/private-user/model",
        "test-only-private-api-key",
    ] {
        fixture.write(
            ".roster/runs/runs.jsonl",
            &format!(
                "{{\"session\":\"unsafe\",\"model\":{}}}\n",
                Text::from(model).json()
            ),
        )?;
        let before = snapshot(&fixture.container)?;
        let output = fixture
            .command(&["stats", "--json"], &fixture.root)
            .env("ROSTER_API_KEY", "test-only-private-api-key")
            .output()
            .map_err(io)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        for private in [
            "private.invalid",
            "private-user",
            "test-only-private-api-key",
        ] {
            assert!(!utf8(&output.stderr)?.contains(private));
        }
        assert_eq!(snapshot(&fixture.container)?, before);
    }
    fixture.finish()
}

#[test]
fn offline_status_cli_missing_log_is_unknown_and_never_reads_private_configuration_or_github()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    fixture.write("roster.config.yml", "not-valid: [test-only-private-config")?;
    files::create_private_directories(&fixture.root.join("nested"))?;
    let before = snapshot(&fixture.container)?;
    let output = fixture
        .command(
            &["status", "--issue", "42", "--offline", "--json"],
            &fixture.root.join("nested"),
        )
        .env("ROSTER_GH", "not-an-approved-github-image")
        .env("GH_TOKEN", "test-only-private-github-token")
        .env("GITHUB_APP_ID", "owned-not-an-app")
        .env("GITHUB_APP_PRIVATE_KEY_PATH", "missing-owned-private-key")
        .output()
        .map_err(io)?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert!(output.stderr.is_empty());
    let observed = parsed(&output.stdout)?;
    assert_eq!(observed.get("issue"), Some(&Value::Number(42.0)));
    assert_eq!(observed.get("log"), Some(&Value::string("missing")));
    assert_eq!(observed.get("seat"), Some(&Value::Null));
    assert_eq!(observed.get("state"), Some(&Value::Null));
    assert_eq!(
        observed.get("authority"),
        Some(&Value::string("local-observation-only"))
    );
    assert!(!utf8(&output.stdout)?.contains("test-only-private"));
    let text = fixture.invoke(&["status", "--issue", "42", "--offline"])?;
    assert!(text.status.success(), "{}", utf8(&text.stderr)?);
    assert_eq!(
        utf8(&text.stdout)?,
        "Issue: 42\nLocal log: missing\nLast seat: unknown\nLast complete event: unknown\nObserved state: unknown\nEvidence: local metadata only; worker liveness, verification, publication and acceptance remain unknown.\n"
    );
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn offline_status_cli_reads_only_complete_native_metadata_and_ignores_the_active_tail()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let source = concat!(
        "{\"schema_version\":1,\"sequence\":0,\"event\":\"invocation\",\"seat\":\"host\",\"status\":\"session-only\"}\n",
        "{\"schema_version\":1,\"sequence\":1,\"event\":\"tool\",\"seat\":\"coder\",\"status\":\"observed\",\"details\":{\"body\":\"test-only-private-file-body\",\"model\":\"https://private.invalid/model\",\"key\":\"test-only-private-token\"}}\n",
        "{\"schema_version\":1,\"sequence\":2,\"event\":\"invocation-end\",\"seat\":\"host\",\"status\":\"unverified\"}\n",
        "{\"partial\":\"test-only-private-tail"
    );
    fixture.write(".roster/runs/roster-42-coder.log", source)?;
    let before = snapshot(&fixture.container)?;
    let output = fixture.invoke(&["status", "--issue", "42", "--offline", "--json"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    let observed = parsed(&output.stdout)?;
    assert_eq!(observed.get("seat"), Some(&Value::string("coder")));
    assert_eq!(observed.get("state"), Some(&Value::string("unverified")));
    assert_eq!(
        observed.get("partial_line_ignored"),
        Some(&Value::Boolean(true))
    );
    assert_eq!(
        observed
            .get("last_complete_record")
            .and_then(|record| record.get("sequence")),
        Some(&Value::Number(2.0))
    );
    assert!(!utf8(&output.stdout)?.contains("test-only-private"));
    assert!(!utf8(&output.stdout)?.contains("private.invalid"));
    assert_eq!(snapshot(&fixture.container)?, before);
    let log = fixture
        .root
        .join(".roster")
        .join("runs")
        .join("roster-42-coder.log");
    let mut bytes = source.as_bytes().to_vec();
    bytes.push(0xff);
    fs::write(&log, &bytes).map_err(io)?;
    let before = snapshot(&fixture.container)?;
    let output = fixture.invoke(&["status", "--issue", "42", "--offline", "--json"])?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert_eq!(parsed(&output.stdout)?, observed);
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn offline_status_cli_uses_the_registered_issue_worktree_and_only_missing_logs_fall_back()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let linked = fixture.container.join("issue-worktree");
    fixture.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "agents/42",
        linked
            .to_str()
            .ok_or_else(|| Error::type_error("Owned worktree must be Unicode"))?,
    ])?;
    let log = linked
        .join(".roster")
        .join("runs")
        .join("roster-42-coder.log");
    files::create_private_directories(
        log.parent()
            .ok_or_else(|| Error::type_error("Owned log needs a parent"))?,
    )?;
    fs::write(
        &log,
        "{\"schema_version\":1,\"sequence\":0,\"event\":\"seat-end\",\"seat\":\"reviewer\",\"status\":\"fail\"}\n",
    ).map_err(io)?;
    fixture.write(
        ".roster/runs/roster-42-coder.log",
        "not-json test-only-private-canonical-log\n",
    )?;
    files::create_private_directories(&linked.join("nested"))?;
    let before = snapshot(&fixture.container)?;
    for cwd in [&fixture.root, &linked, &linked.join("nested")] {
        let output = fixture
            .command(&["status", "--issue", "42", "--offline", "--json"], cwd)
            .output()
            .map_err(io)?;
        assert!(output.status.success(), "{}", utf8(&output.stderr)?);
        let observed = parsed(&output.stdout)?;
        assert_eq!(observed.get("seat"), Some(&Value::string("reviewer")));
        assert_eq!(observed.get("state"), Some(&Value::string("fail")));
        assert!(!utf8(&output.stdout)?.contains("test-only-private"));
    }
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.write(
        ".roster/runs/roster-42-coder.log",
        "{\"schema_version\":1,\"sequence\":0,\"event\":\"seat-end\",\"seat\":\"planner\",\"status\":\"unverified\"}\n",
    )?;
    fs::write(&log, "not-json test-only-private-selected-log\n").map_err(io)?;
    let before = snapshot(&fixture.container)?;
    let refused = fixture.invoke(&["status", "--issue", "42", "--offline"])?;
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    assert!(!utf8(&refused.stderr)?.contains("test-only-private"));
    assert_eq!(snapshot(&fixture.container)?, before);
    fs::remove_file(&log).map_err(io)?;
    let before = snapshot(&fixture.container)?;
    let fallback = fixture.invoke(&["status", "--issue", "42", "--offline", "--json"])?;
    assert!(fallback.status.success(), "{}", utf8(&fallback.stderr)?);
    assert_eq!(
        parsed(&fallback.stdout)?.get("seat"),
        Some(&Value::string("planner"))
    );
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn offline_status_cli_refuses_corrupt_nonregular_hardlinked_and_unreadable_logs_without_effects()
-> Result<(), Error> {
    use std::os::windows::fs::OpenOptionsExt;

    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let log = fixture
        .root
        .join(".roster")
        .join("runs")
        .join("roster-42-coder.log");
    fixture.write(
        ".roster/runs/roster-42-coder.log",
        "test-only-private-invalid-record\n",
    )?;
    let assert_refused = || -> Result<(), Error> {
        let output = fixture.invoke(&["status", "--issue", "42", "--offline", "--json"])?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!utf8(&output.stderr)?.contains("test-only-private"));
        assert!(!utf8(&output.stderr)?.contains(&fixture.container.to_string_lossy().into_owned()));
        Ok(())
    };
    let before = snapshot(&fixture.container)?;
    assert_refused()?;
    assert_eq!(snapshot(&fixture.container)?, before);
    fs::remove_file(&log).map_err(io)?;
    files::create_private_directories(&log)?;
    let before = snapshot(&fixture.container)?;
    assert_refused()?;
    assert_eq!(snapshot(&fixture.container)?, before);
    fs::remove_dir(&log).map_err(io)?;
    let private = fixture.container.join("owned-private-log");
    fs::write(&private, b"test-only-private-linked-log").map_err(io)?;
    fs::hard_link(&private, &log).map_err(io)?;
    let before = snapshot(&fixture.container)?;
    assert_refused()?;
    assert_eq!(snapshot(&fixture.container)?, before);
    fs::remove_file(&log).map_err(io)?;
    fixture.write(
        ".roster/runs/roster-42-coder.log",
        "{\"schema_version\":1,\"sequence\":0,\"event\":\"seat-end\",\"seat\":\"coder\",\"status\":\"pass\"}\n",
    )?;
    let before = snapshot(&fixture.container)?;
    let retained = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&log)
        .map_err(io)?;
    assert_refused()?;
    drop(retained);
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn offline_status_cli_preserves_the_exact_native_writer_byte_limit_and_rejects_unknown_flags()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let prefix = "{\"schema_version\":1,\"sequence\":0,\"event\":\"seat-end\",\"seat\":\"coder\",\"status\":\"pass\",\"details\":{\"ignored\":\"";
    let suffix = "\"}}\n";
    let source = format!(
        "{prefix}{}{suffix}",
        "a".repeat(2 * 1024 * 1024 - prefix.len() - suffix.len())
    );
    assert_eq!(source.len(), 2 * 1024 * 1024);
    fixture.write(".roster/runs/roster-42-coder.log", &source)?;
    let before = snapshot(&fixture.container)?;
    let exact = fixture.invoke(&["status", "--issue", "42", "--offline", "--json"])?;
    assert!(exact.status.success(), "{}", utf8(&exact.stderr)?);
    assert_eq!(
        parsed(&exact.stdout)?.get("state"),
        Some(&Value::string("pass"))
    );
    assert!(exact.stdout.len() < 1024);
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.write(".roster/runs/roster-42-coder.log", &format!("{source} "))?;
    let before = snapshot(&fixture.container)?;
    let over = fixture.invoke(&["status", "--issue", "42", "--offline", "--json"])?;
    assert!(!over.status.success());
    assert!(over.stdout.is_empty());
    assert_eq!(snapshot(&fixture.container)?, before);
    for args in [
        vec!["status", "--issue", "42", "--offline", "--unknown-option"],
        vec!["status", "--issue", "42"],
        vec!["status", "--issue", "042", "--offline"],
    ] {
        let invalid = fixture
            .command(&args, &fixture.root)
            .env("ROSTER_GIT", "not-an-approved-image")
            .output()
            .map_err(io)?;
        assert!(!invalid.status.success());
        assert!(invalid.stdout.is_empty());
        assert!(utf8(&invalid.stderr)?.contains("Use status"));
        assert!(!utf8(&invalid.stderr)?.contains("not-an-approved-image"));
    }
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn fleet_list_cli_preserves_exact_empty_and_tabular_outputs_without_tools_or_endpoint_config()
-> Result<(), Error> {
    assert_eq!(
        github_agent_roster::fleet::parse(&Text::from(OWNED_FLEET))?
            .profiles()
            .len(),
        2
    );
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    fixture.write(".roster/config.yml", "not-valid [test-only-private-config")?;
    let before = snapshot(&fixture.container)?;
    let empty = fixture
        .command(&["fleet", "list"], &fixture.root)
        .env("ROSTER_GIT", "not-an-approved-image")
        .env("ROSTER_GH", "not-an-approved-image")
        .env("ROSTER_LLM_BASE_URL", "not-an-endpoint")
        .env("ROSTER_INSTALLATION_ROOT", "not-an-installation")
        .output()
        .map_err(io)?;
    assert!(empty.status.success(), "{}", utf8(&empty.stderr)?);
    assert_eq!(
        utf8(&empty.stdout)?,
        "No fleet profiles configured. Run roster onboard or roster fleet add.\n"
    );
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.write(".roster/fleet.yml", OWNED_FLEET)?;
    let before = snapshot(&fixture.container)?;
    let listed = fixture
        .command(&["fleet", "list"], &fixture.root)
        .env("ROSTER_GIT", "not-an-approved-image")
        .env("ROSTER_GH", "not-an-approved-image")
        .env("ROSTER_LLM_BASE_URL", "not-an-endpoint")
        .env("ROSTER_INSTALLATION_ROOT", "not-an-installation")
        .env("AI_MODEL", "not-a-configured-fleet-model")
        .output()
        .map_err(io)?;
    assert!(listed.status.success(), "{}", utf8(&listed.stderr)?);
    assert!(listed.stderr.is_empty());
    assert_eq!(utf8(&listed.stdout)?, OWNED_FLEET_TABLE);
    assert!(!utf8(&listed.stdout)?.contains("test-only-private"));
    assert!(!utf8(&listed.stdout)?.contains("BOUND_CREDENTIAL"));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn fleet_list_cli_uses_canonical_shared_private_settings_from_subdirectories_and_linked_worktrees()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    fixture.write(".roster/fleet.yml", OWNED_FLEET)?;
    let linked = fixture.container.join("linked-fleet");
    fixture.git(&[
        "worktree",
        "add",
        "--quiet",
        "--detach",
        linked
            .to_str()
            .ok_or_else(|| Error::type_error("Owned linked path must be Unicode"))?,
        "HEAD",
    ])?;
    files::create_private_directories(&fixture.root.join("nested"))?;
    files::create_private_directories(&linked.join("nested"))?;
    files::create_private_directories(&linked.join(".roster"))?;
    fs::write(
        linked.join(".roster").join("fleet.yml"),
        "not-valid [test-only-private-issue-scratch",
    )
    .map_err(io)?;
    let before = snapshot(&fixture.container)?;
    for cwd in [
        &fixture.root.join("nested"),
        &linked,
        &linked.join("nested"),
    ] {
        let output = fixture
            .command(&["fleet", "list"], cwd)
            .output()
            .map_err(io)?;
        assert!(output.status.success(), "{}", utf8(&output.stderr)?);
        assert_eq!(utf8(&output.stdout)?, OWNED_FLEET_TABLE);
        assert!(output.stderr.is_empty());
    }
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn fleet_list_cli_refuses_corrupt_nonregular_hardlinked_and_sharing_denied_sources_without_echoes()
-> Result<(), Error> {
    use std::os::windows::fs::OpenOptionsExt;

    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let path = fixture.root.join(".roster").join("fleet.yml");
    let assert_refused = || -> Result<(), Error> {
        let output = fixture.invoke(&["fleet", "list"])?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            utf8(&output.stderr)?,
            "Fleet listing could not read a checked local catalog; private settings were preserved.\n"
        );
        assert!(!utf8(&output.stderr)?.contains("test-only-private"));
        assert!(!utf8(&output.stderr)?.contains(&fixture.container.to_string_lossy().into_owned()));
        let error =
            github_agent_roster::local_cli::list_fleet(&fixture.root, &fixture.environment())
                .err()
                .ok_or_else(|| Error::type_error("Unsafe fleet provider reported success"))?;
        assert_eq!(error.code, Some("ROSTER_FLEET_LIST"));
        assert!(!error.to_string().contains("test-only-private"));
        Ok(())
    };
    for source in [
        "test-only-private-invalid-fleet",
        &format!("{OWNED_FLEET}  - id: test-only-private-incomplete-profile\n"),
        &OWNED_FLEET.replace("owned GPU", "\"GPU\\u009b31m\""),
    ] {
        fixture.write(".roster/fleet.yml", source)?;
        let before = snapshot(&fixture.container)?;
        assert_refused()?;
        assert_eq!(snapshot(&fixture.container)?, before);
    }
    fs::remove_file(&path).map_err(io)?;
    files::create_private_directories(&path)?;
    let before = snapshot(&fixture.container)?;
    assert_refused()?;
    assert_eq!(snapshot(&fixture.container)?, before);
    fs::remove_dir(&path).map_err(io)?;
    let private = fixture.container.join("owned-private-fleet");
    fs::write(&private, OWNED_FLEET).map_err(io)?;
    fs::hard_link(&private, &path).map_err(io)?;
    let before = snapshot(&fixture.container)?;
    assert_refused()?;
    assert_eq!(snapshot(&fixture.container)?, before);
    fs::remove_file(&path).map_err(io)?;
    fixture.write(".roster/fleet.yml", OWNED_FLEET)?;
    let before = snapshot(&fixture.container)?;
    let retained = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .map_err(io)?;
    assert_refused()?;
    drop(retained);
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn fleet_list_cli_preserves_the_exact_private_file_byte_limit_and_rejects_one_extra_byte()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    let prefix = format!("{OWNED_FLEET}#");
    let source = format!("{prefix}{}\n", "a".repeat(65_536 - prefix.len() - 1));
    assert_eq!(source.len(), 65_536);
    fixture.write(".roster/fleet.yml", &source)?;
    let before = snapshot(&fixture.container)?;
    let exact = fixture.invoke(&["fleet", "list"])?;
    assert!(exact.status.success(), "{}", utf8(&exact.stderr)?);
    assert_eq!(utf8(&exact.stdout)?, OWNED_FLEET_TABLE);
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.write(".roster/fleet.yml", &format!("{source} "))?;
    let before = snapshot(&fixture.container)?;
    let over = fixture.invoke(&["fleet", "list"])?;
    assert!(!over.status.success());
    assert!(over.stdout.is_empty());
    assert_eq!(
        utf8(&over.stderr)?,
        "Fleet listing could not read a checked local catalog; private settings were preserved.\n"
    );
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn fleet_list_cli_rejects_unknown_flags_and_unimplemented_mutators_before_reading_private_settings()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    files::create_private_directories(&fixture.root.join(".roster").join("fleet.yml"))?;
    let before = snapshot(&fixture.container)?;
    for args in [
        vec!["fleet"],
        vec!["fleet", "list", "--unknown-option"],
        vec!["fleet", "list", "--json"],
        vec!["fleet", "list", "--repo", "outside"],
        vec!["fleet", "discover"],
        vec!["fleet", "add", "--id", "not-approved"],
        vec!["fleet", "probe", "first"],
        vec!["fleet", "default", "first"],
    ] {
        let output = fixture
            .command(&args, &fixture.root)
            .env("ROSTER_INSTALLATION_ROOT", "not-approved")
            .output()
            .map_err(io)?;
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty());
        assert!(
            utf8(&output.stderr)?
                .starts_with("Native command is not implemented or has invalid arguments.\n")
        );
        assert!(!utf8(&output.stderr)?.contains("could not read a checked local catalog"));
        assert!(!utf8(&output.stderr)?.contains("not-approved"));
        assert_eq!(
            github_agent_roster::local_cli::parse(
                &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>()
            )
            .err()
            .ok_or_else(|| Error::type_error("Unknown fleet command was accepted"))?
            .code,
            Some("ROSTER_NATIVE_UNSUPPORTED")
        );
    }
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn fleet_list_review_windows_environment_case_aliases_are_redacted_by_the_actual_cli()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    fixture.write(
        ".roster/fleet.yml",
        &OWNED_FLEET.replace("owned GPU", "owned opaque-value-123"),
    )?;
    let before = snapshot(&fixture.container)?;
    let output = fixture
        .command(&["fleet", "list"], &fixture.root)
        .env("bound_credential", "opaque-value-123")
        .output()
        .map_err(io)?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert_eq!(
        utf8(&output.stdout)?,
        OWNED_FLEET_TABLE.replace("owned GPU", "owned [redacted]")
    );
    assert!(!utf8(&output.stdout)?.contains("opaque-value-123"));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn fleet_list_review_ambiguous_windows_credential_aliases_refuse_without_output_or_effects()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    fixture.write(
        ".roster/fleet.yml",
        &OWNED_FLEET.replace("owned GPU", "owned opaque-value-123"),
    )?;
    let before = snapshot(&fixture.container)?;
    let failure = github_agent_roster::local_cli::list_fleet(
        &fixture.root,
        &Value::object(vec![
            ("BOUND_CREDENTIAL", Value::string("opaque-value-123")),
            ("bound_credential", Value::string("other-opaque-value-456")),
        ]),
    )
    .err()
    .ok_or_else(|| Error::type_error("Ambiguous credential aliases were accepted"))?;
    assert_eq!(failure.code, Some("ROSTER_FLEET_LIST"));
    assert!(!failure.to_string().contains("opaque-value"));
    assert!(
        !failure
            .to_string()
            .contains(&fixture.container.to_string_lossy().into_owned())
    );
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[test]
fn fleet_list_cli_redacts_explicit_custom_credentials_without_printing_notes_or_key_declarations()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    fixture.write(
        ".roster/fleet.yml",
        &OWNED_FLEET.replace("owned GPU", "owned test-only-private-secret"),
    )?;
    let before = snapshot(&fixture.container)?;
    let output = fixture
        .command(&["fleet", "list"], &fixture.root)
        .env("BOUND_CREDENTIAL", "test-only-private-secret")
        .output()
        .map_err(io)?;
    assert!(output.status.success(), "{}", utf8(&output.stderr)?);
    assert_eq!(
        utf8(&output.stdout)?,
        OWNED_FLEET_TABLE.replace("owned GPU", "owned [redacted]")
    );
    assert!(!utf8(&output.stdout)?.contains("test-only-private"));
    assert!(!utf8(&output.stdout)?.contains("BOUND_CREDENTIAL"));
    assert_eq!(snapshot(&fixture.container)?, before);
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn fleet_list_read_only_acceptance_executes_the_actual_cli_and_its_unknown_option_control()
-> Result<(), Error> {
    let fixture = Fixture::new(false)?;
    fixture.commit(None, "base")?;
    fixture.write(".roster/fleet.yml", OWNED_FLEET)?;
    let assignment = fixture.task(
        &["docs/guide.md"],
        &["`roster fleet list` lists the configured fleet offline."],
    )?;
    let cancellation = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        assignment,
        tools::Options::default(),
        fixture.host(),
        redaction::Config::default(),
        cancellation.token(),
    )
    .await?;
    let result = async {
        let report = e2e::run(
            &boundary,
            &backend::Approval {
                native_cli: Some(PathBuf::from(env!("CARGO_BIN_EXE_roster"))),
                ..Default::default()
            },
        )
        .await?;
        assert!(report.passed(), "{}", report.as_value().json());
        let runs = items(
            report
                .as_value()
                .get("runs")
                .ok_or_else(|| Error::type_error("Missing actual fleet CLI runs"))?,
        )?;
        assert_eq!(runs.len(), 1);
        let run = runs
            .first()
            .ok_or_else(|| Error::type_error("Missing actual fleet CLI run"))?;
        assert_eq!(number(run, "exitCode"), Some(0.0));
        assert_eq!(run.get("output"), Some(&Value::string(OWNED_FLEET_TABLE)));
        assert!(
            run.get("errorCase")
                .and_then(|case| number(case, "exitCode"))
                .is_some_and(|code| code != 0.0)
        );
        report.require_current(&boundary).await?;
        Ok(())
    }
    .await;
    github_agent_roster::state::locks::finish(result, boundary.close().await)?;
    fixture.finish()
}
