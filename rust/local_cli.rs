use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::cancel::CancellationToken;
use crate::config::{self, Source};
use crate::delivery::Input;
use crate::evaluation;
use crate::fleet;
use crate::host;
use crate::json::Value;
use crate::metrics;
use crate::paths;
use crate::start::{self, Sync};
use crate::state::provenance::history;
use crate::state::{redaction, secure};
use crate::status;

pub const HELP: &str = "Usage:
  roster                     Open the interactive shell in a TTY
  roster --debug [COMMAND]   Enable process-only debug JSONL logging
  roster --help
  roster init
  roster onboard [--discover]
  roster ask \"TEXT\"
  roster ask --pending|--cancel|--resume [\"TEXT\"]
  roster question [--files PATH,PATH --] TEXT
  roster doctor
  roster doctor --copilot [--opt-in] [--runtime PATH] [--model ID]
  roster doctor --openshell [--opt-in] [--runtime PATH] [--gateway URL] [--policy PATH] [--provider ID]
  roster fleet list
  roster history list [--format json|text|summary] [--store DIR] [--repo HASH] [--issue N] [--seat NAME] [--model MODEL] [--outcome VALUE] [--since TIME] [--until TIME]
  roster history show <session-or-run-or-record-id> [--format json|text|summary] [--store DIR]
  roster clean [--target issue|repo|machine-history|curated-memory] [--issue N] [--store DIR] [--scope SCOPE] [--state-root PATH] [--exclude NAME]... [--execute --yes]
  roster bench
  roster status --issue N [--offline] [--json]
  roster stats [--delivery] [--json] [--ref REVISION_OR_RANGE] [--evals PATH]
  roster learn --recurring
  roster vault set NAME
  roster vault list
  roster vault get NAME
  roster recommend --task-class feat|fix|docs|test [--difficulty 1-5] [--json]
  roster eval TARGET accept|reject|rework 1-5 y|n --local|--post [--minutes N] [--comment TEXT]
  roster run --seat coder --runtime builtin [--skip-review]
  roster run --ask-file PATH --runtime builtin [--base trunk|current|REF] [--no-fetch|--fetch] [--skip-review]
  roster run --issue N --runtime builtin [--base trunk|current|REF] [--no-fetch|--fetch] [--skip-review]
  roster catalog parse ROOT FIELD... < catalog.yml
  roster arguments split < arguments.txt
  roster recipe validate PATH

Ask creates a GitHub issue when gh is available; otherwise it saves a local draft.
Doctor checks local prerequisites without contacting GitHub or printing App env values.
Doctor --copilot reports offline blockers only; --opt-in never enables or launches a worker.
Init copies review-only examples; it never overwrites agent-policy.yml.
Onboard configures a real vLLM model and local permissions in an interactive terminal.
Local delivery uses approved native tools and sequential builtin seats.
Local fixed-profile delivery supports at most two rescopings, two fresh perspectives and two semantic review repairs.
GitHub issue intake uses App authentication and a fresh isolated issue worktree.
GitHub publication, resume and automatic routing are unavailable.
Existing issue claims are never adopted or replaced; local fixed-profile continuations above are implemented.
Unimplemented commands fail explicitly; there is no Node runtime fallback.
Stats and recommendations read bounded local evidence without recording, publishing or changing a route.
History list and show read bounded durable provenance records without modifying or repairing the store.
Status requires an explicit issue; --offline stays local, otherwise complete App authentication and the reviewed SDK are required. Observations are not liveness or passing authority.
Fleet list reads checked canonical private settings without probing, warming, writing or selecting a route.
Human eval requires explicit --local or --post and a separate human invocation; posting requires approved human GitHub credentials.
Learn --recurring writes human-reviewable draft proposals in .roster/proposals without changing active rules.
Vault set reads a secret from stdin; vault list prints names, never values.
";

#[derive(Debug, Clone)]
pub struct Run {
    pub input: Input,
    pub base: Option<String>,
    pub sync: Option<Sync>,
    pub skip_review: bool,
}

#[derive(Debug, Clone)]
pub struct Stats {
    pub metrics: metrics::Options,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct Recommend {
    pub task_class: String,
    pub difficulty: Option<u8>,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub enum AskCommand {
    Submit(String),
    Pending,
    Cancel,
    Resume(Option<String>),
}

#[derive(Debug, Clone)]
pub enum VaultCommand {
    Set(String),
    List,
    Get(String),
}

#[derive(Debug, Clone)]
pub enum Command {
    Help,
    Init,
    Ask(AskCommand),
    ResumeGreen(u64),
    Question(Vec<String>),
    Doctor,
    CopilotDoctor(crate::capabilities::CopilotDoctorOptions),
    OpenShellDoctor(crate::openshell::OpenShellDoctorOptions),
    FleetList,
    History(history::Request),
    Clean(crate::state::clean::CliOptions),
    Bench,
    LearnRecurring,
    Vault(VaultCommand),
    Status(status::Options),
    Stats(Stats),
    Recommend(Recommend),
    Eval(evaluation::Decision),
    Run(Run),
    Recipe(PathBuf),
    Arguments,
    Catalog { root: String, fields: Vec<String> },
}

fn parse_stats(args: &[String]) -> Result<Stats, Error> {
    let usage = || {
        Error::type_error(
            "Use roster stats [--delivery] [--json] [--ref REVISION_OR_RANGE] [--evals PATH].",
        )
    };
    let mut seen = BTreeSet::new();
    let mut result = Stats {
        metrics: metrics::Options::default(),
        json: false,
    };
    let mut position = 0;
    while let Some(flag) = args.get(position) {
        if !seen.insert(flag) {
            return Err(usage());
        }
        match flag.as_str() {
            "--json" => result.json = true,
            "--delivery" => result.metrics.delivery = true,
            "--ref" | "--evals" => {
                position += 1;
                let value = args
                    .get(position)
                    .filter(|value| !value.is_empty() && !value.starts_with('-'))
                    .ok_or_else(usage)?;
                if flag == "--ref" {
                    result.metrics.reference = Some(value.clone());
                } else {
                    result.metrics.evals_path = Some(PathBuf::from(value));
                }
            }
            _ => return Err(usage()),
        }
        position += 1;
    }
    result.metrics.validate()?;
    Ok(result)
}

fn parse_recommend(args: &[String]) -> Result<Recommend, Error> {
    let usage = || {
        Error::type_error(
            "Use recommend --task-class feat|fix|docs|test [--difficulty 1-5] [--json].",
        )
    };
    let mut seen = BTreeSet::new();
    let mut task_class = None;
    let mut difficulty = None;
    let mut json = false;
    let mut position = 0;
    while let Some(flag) = args.get(position) {
        if !seen.insert(flag) {
            return Err(usage());
        }
        match flag.as_str() {
            "--json" => json = true,
            "--task-class" | "--difficulty" => {
                position += 1;
                let value = args.get(position).ok_or_else(usage)?;
                if flag == "--task-class" && crate::learning::TASK_CLASSES.contains(&value.as_str())
                {
                    task_class = Some(value.clone());
                } else if flag == "--difficulty"
                    && value.len() == 1
                    && matches!(value.as_bytes().first(), Some(b'1'..=b'5'))
                {
                    difficulty = Some(value.parse::<u8>().map_err(|_| usage())?);
                } else {
                    return Err(usage());
                }
            }
            _ => return Err(usage()),
        }
        position += 1;
    }
    Ok(Recommend {
        task_class: task_class.ok_or_else(usage)?,
        difficulty,
        json,
    })
}

fn invalid_run() -> Error {
    Error::type_error(
        "Use roster run --seat coder --runtime builtin or roster run --ask-file PATH|--issue N --runtime builtin [--base trunk|current|REF] [--no-fetch|--fetch] [--skip-review]",
    )
}

fn parse_run(args: &[String]) -> Result<Run, Error> {
    if args.iter().any(|arg| {
        [
            "--publish",
            "--auto-model",
            "--seats",
            "--parallel",
            "--attempts",
            "--saved",
            "--confirm",
            "--plan",
            "--resume",
            "--merge-when-green",
        ]
        .contains(&arg.as_str())
    }) {
        return Err(Error::new(
            "NativeIntegrationError",
            "GitHub publication and outer routing/resume flags are not implemented; no publisher or route was substituted",
        ).coded("ROSTER_NATIVE_UNSUPPORTED"));
    }
    let mut seen = BTreeSet::new();
    let mut input = None;
    let mut runtime = None;
    let mut base = None;
    let mut sync = None;
    let mut skip_review = false;
    let mut position = 0;
    while let Some(arg) = args.get(position) {
        if !seen.insert(arg.as_str()) {
            return Err(invalid_run());
        }
        match arg.as_str() {
            "--skip-review" => skip_review = true,
            "--no-fetch" | "--fetch" => {
                if sync.is_some() {
                    return Err(invalid_run());
                }
                sync = Some(if arg == "--fetch" {
                    Sync::Fetch
                } else {
                    Sync::Offline
                });
            }
            "--seat" | "--runtime" | "--ask-file" | "--base" | "--issue" => {
                position += 1;
                let value = args
                    .get(position)
                    .filter(|value| !value.is_empty() && !value.starts_with('-'))
                    .ok_or_else(invalid_run)?;
                match arg.as_str() {
                    "--seat" if value == "coder" && input.is_none() => {
                        input = Some(Input::ExistingTask)
                    }
                    "--ask-file" if input.is_none() => {
                        input = Some(Input::AskFile(PathBuf::from(value)))
                    }
                    "--issue" if input.is_none() => {
                        input = Some(Input::GitHubIssue(
                            start::parse_issue_number(value).map_err(|_| invalid_run())?,
                        ))
                    }
                    "--runtime" if value == "builtin" => runtime = Some(()),
                    "--base" => {
                        config::validate_start_base(
                            &crate::text::Text::from(value.as_str()),
                            "--base",
                        )?;
                        base = Some(value.clone());
                    }
                    _ => return Err(invalid_run()),
                }
            }
            _ => return Err(invalid_run()),
        }
        position += 1;
    }
    if runtime.is_none() {
        return Err(invalid_run());
    }
    let input = input.ok_or_else(invalid_run)?;
    if matches!(input, Input::ExistingTask) && (base.is_some() || sync.is_some()) {
        return Err(Error::type_error(
            "Existing-TASK mode never moves its current worktree; --base/--fetch/--no-fetch apply only to --ask-file or --issue",
        ));
    }
    Ok(Run {
        input,
        base,
        sync,
        skip_review,
    })
}

pub fn parse(args: &[String]) -> Result<Command, Error> {
    if args.is_empty() || args == ["--help"] || args == ["-h"] || args == ["run", "--help"] {
        return Ok(Command::Help);
    }
    match args {
        [command] if command == "init" => Ok(Command::Init),
        [command, rest @ ..] if command == "question" => {
            if rest.is_empty() {
                Err(Error::type_error(
                    "Use roster question TEXT with an explicit Files allowed section.",
                ))
            } else {
                Ok(Command::Question(rest.to_vec()))
            }
        }
        [command, rest @ ..] if command == "ask" => match rest {
            [flag] if flag == "--pending" => Ok(Command::Ask(AskCommand::Pending)),
            [flag] if flag == "--cancel" => Ok(Command::Ask(AskCommand::Cancel)),
            [flag] if flag == "--resume" => Ok(Command::Ask(AskCommand::Resume(None))),
            [flag, answer] if flag == "--resume" => {
                Ok(Command::Ask(AskCommand::Resume(Some(answer.clone()))))
            }
            [flag, ..] if matches!(flag.as_str(), "--pending" | "--cancel" | "--resume") => Err(
                Error::type_error("Use roster ask --pending, --cancel, or --resume \"TEXT\"."),
            ),
            [text] => Ok(Command::Ask(AskCommand::Submit(text.clone()))),
            _ => Err(Error::type_error(
                "Use roster ask \"TEXT\" or roster ask --pending, --cancel, or --resume \"TEXT\".",
            )),
        },
        [command, flags @ ..] if command == "resume" => {
            if let [num_str, flag1, flag2] = flags {
                if flag1 == "--green" && flag2 == "--yes" {
                    if let Ok(issue) = start::parse_issue_number(num_str) {
                        return Ok(Command::ResumeGreen(issue));
                    }
                }
            }
            Err(Error::type_error(
                "Use roster resume N --green --yes; explicit --yes authorizes product restore, not execution.",
            ))
        }
        [command] if command == "doctor" => Ok(Command::Doctor),
        [command, flag, rest @ ..] if command == "doctor" && flag == "--copilot" => Ok(
            Command::CopilotDoctor(crate::capabilities::parse_copilot_doctor_args(rest)?),
        ),
        [command, flag, rest @ ..] if command == "doctor" && flag == "--openshell" => Ok(
            Command::OpenShellDoctor(crate::openshell::parse_openshell_doctor_args(rest)?),
        ),
        [command, operation] if command == "fleet" && operation == "list" => Ok(Command::FleetList),
        [command, flags @ ..] if command == "history" => {
            Ok(Command::History(history::Request::parse(flags)?))
        }
        [command, flags @ ..] if command == "clean" => Ok(Command::Clean(
            crate::state::clean::CliOptions::parse(flags)?,
        )),
        [command] if command == "bench" => Ok(Command::Bench),
        [command, flags @ ..] if command == "learn" => {
            if flags == ["--recurring"] {
                Ok(Command::LearnRecurring)
            } else {
                Err(Error::type_error("Use roster learn --recurring."))
            }
        }
        [command, operation, name] if command == "vault" && operation == "set" => {
            Ok(Command::Vault(VaultCommand::Set(name.clone())))
        }
        [command, operation] if command == "vault" && operation == "list" => {
            Ok(Command::Vault(VaultCommand::List))
        }
        [command, operation, name] if command == "vault" && operation == "get" => {
            Ok(Command::Vault(VaultCommand::Get(name.clone())))
        }
        [command, flags @ ..] if command == "status" => {
            Ok(Command::Status(status::Options::parse(flags)?))
        }
        [command, flags @ ..] if command == "stats" => Ok(Command::Stats(parse_stats(flags)?)),
        [command, flags @ ..] if command == "recommend" => {
            Ok(Command::Recommend(parse_recommend(flags)?))
        }
        [command, flags @ ..] if command == "eval" => {
            Ok(Command::Eval(evaluation::Decision::parse(flags)?))
        }
        [command, flags @ ..] if command == "run" => Ok(Command::Run(parse_run(flags)?)),
        [command, operation, file] if command == "recipe" && operation == "validate" => {
            Ok(Command::Recipe(PathBuf::from(file)))
        }
        [command, operation] if command == "arguments" && operation == "split" => {
            Ok(Command::Arguments)
        }
        [command, operation, root, fields @ ..] if command == "catalog" && operation == "parse" => {
            Ok(Command::Catalog {
                root: root.clone(),
                fields: fields.to_vec(),
            })
        }
        _ => Err(Error::new(
            "NativeIntegrationError",
            format!("Native command is not implemented or has invalid arguments.\n{HELP}"),
        )
        .coded("ROSTER_NATIVE_UNSUPPORTED")),
    }
}

pub fn list_fleet(cwd: &Path, environment: &Value) -> Result<String, Error> {
    let result = (|| {
        if !matches!(environment, Value::Object(_)) {
            return Err(Error::type_error(
                "Fleet environment must be a typed mapping",
            ));
        }
        let root = config::shared_config_root(cwd)?;
        let catalog = fleet::load(&root)?;
        let mut redaction = redaction::Config {
            environment: environment.clone(),
            ..Default::default()
        };
        for profile in catalog.profiles() {
            if let Some(name) = profile
                .as_value()
                .get("api_key_env")
                .and_then(Value::as_string)
            {
                if let Some(value) = config::environment_entry(environment, &name.to_utf8()?)? {
                    let value = value.as_string().ok_or_else(|| {
                        Error::type_error("Selected fleet credential declaration must be text")
                    })?;
                    if !value.0.is_empty() {
                        redaction.credential_values.push(value.clone());
                    }
                }
            }
        }
        redaction::diagnostic(&fleet::list(&catalog)?, &redaction).to_utf8()
    })();
    result.map_err(|cause: Error| {
        let mut safe = Error::new(cause.name, "Private fleet provider details were omitted");
        safe.code = cause.code;
        Error::new(
            "FleetListError",
            "Fleet listing could not read a checked local catalog; private settings were preserved.",
        )
        .coded("ROSTER_FLEET_LIST")
        .caused_by(safe)
    })
}

pub fn initialize(cwd: &Path) -> Result<String, Error> {
    let cwd = paths::absolute(cwd)?;
    secure::check_components(&cwd)?;
    if !cwd.is_dir() {
        return Err(Error::type_error(
            "roster init requires an existing real directory",
        ));
    }
    let examples = [
        (
            "roster.config.example.yml",
            include_bytes!("../roster.config.example.yml").as_slice(),
        ),
        (
            "ROSTER-POLICY-NOTE.md",
            include_bytes!("../templates/init/POLICY-NOTE.md").as_slice(),
        ),
    ];
    for (name, _) in examples {
        let path = cwd.join(name);
        secure::check_components(&path)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if !metadata.is_file()
                    || secure::linked(&metadata)
                    || paths::file_information(&path)?.links != 1 =>
            {
                return Err(Error::type_error(format!(
                    "Refusing to replace a non-regular init file: {}",
                    path.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(secure::io_error("Inspect init example", &path, error)),
        }
    }
    let mut output = String::new();
    for (name, source) in examples {
        let path = cwd.join(name);
        let status = match secure::write_exclusive(&path, source) {
            Ok(_) => "Created",
            Err(error) if error.code == Some("EEXIST") => {
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| secure::io_error("Recheck init example", &path, error))?;
                if !metadata.is_file()
                    || secure::linked(&metadata)
                    || paths::file_information(&path)?.links != 1
                {
                    return Err(Error::type_error(
                        "Init target changed to a non-regular file; preserve it",
                    ));
                }
                "Kept existing"
            }
            Err(error) => return Err(error),
        };
        output.push_str(&format!("{status}: {}\n", path.display()));
    }
    output.push_str("Policy: agent-policy.yml unchanged (human-owned)\n");
    Ok(output)
}

pub struct Doctor {
    pub ok: bool,
    pub output: String,
}

pub async fn doctor(
    installation: &Path,
    cwd: &Path,
    environment: &Value,
    token: &CancellationToken,
) -> Result<Doctor, Error> {
    let loaded = config::load(installation, cwd, environment)?;
    let mut output = String::from("Doctor: offline; no GitHub, model, vault or hardware request\n");
    output.push_str(match loaded.source {
        Source::Defaults => "Configuration: validated built-in defaults\n",
        Source::Private(_) => "Configuration: validated private repository settings\n",
    });
    let contracts = paths::resolve_contracts_path(
        installation,
        cwd,
        host::contracts_override(environment)?.as_deref(),
    )?;
    secure::check_components(&contracts)?;
    let mut ok = true;
    match host::selected_path(environment, "ROSTER_GIT")? {
        Some(path) => {
            let image = host::Image::approve(&path)?;
            let host = tools_host(environment, image.path().to_path_buf())?;
            let directory = paths::repository_paths(cwd).map_or_else(
                |_| paths::absolute(cwd).map(|root| root.join(".roster-doctor-no-hooks")),
                |paths| Ok(paths.common_dir.join("roster-doctor-no-hooks")),
            )?;
            let git = start::Git::new(host, directory)?;
            let version = git.text(cwd, &["--version"], token).await?;
            if !version.starts_with("git version ") {
                return Err(Error::type_error(
                    "ROSTER_GIT is not an installed Git image",
                ));
            }
            output.push_str("Git: approved installed image\n");
            start::Contracts::open(installation, &contracts, &git, token).await?;
            if paths::absolute(&contracts)?
                == paths::absolute(&installation.join("vendor").join("github-agent-contracts"))?
            {
                output.push_str("Contracts: initialized submodule at the reviewed gitlink\n");
            } else {
                output
                    .push_str("Contracts: external complete dependency at the reviewed gitlink\n");
            }
        }
        None => {
            ok = false;
            output.push_str("Git: unavailable; set ROSTER_GIT to an absolute approved image\nContracts: complete dependency; Git pin check unavailable\n");
        }
    }
    for (name, label) in [
        ("ROSTER_NODE20", "Consumer Node 20"),
        ("ROSTER_CARGO", "Consumer Cargo"),
    ] {
        if let Some(path) = host::selected_path(environment, name)? {
            host::Image::approve(&path)?;
            output.push_str(&format!("{label}: host image selected; version and effective task checks remain run-time gates\n"));
        } else {
            output.push_str(&format!(
                "{label}: not selected; tasks requiring it are unavailable\n"
            ));
        }
    }
    output.push_str("Recording: session-only by default; no durable history claimed\nPublication and GitHub issue orchestration: unavailable\n");
    loaded.require_current()?;
    Ok(Doctor { ok, output })
}

fn tools_host(environment: &Value, git: PathBuf) -> Result<crate::execution::tools::Host, Error> {
    Ok(crate::execution::tools::Host {
        git,
        node20: None,
        cargo: None,
        contracts: None,
        environment: host::minimal_environment(environment)?,
    })
}
