use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::cancel::CancellationToken;
use crate::config::{self, Source};
use crate::delivery::Input;
use crate::host;
use crate::json::Value;
use crate::paths;
use crate::start::{self, Sync};
use crate::state::secure;

pub const HELP: &str = "Usage:
  roster --help
  roster init
  roster doctor
  roster run --seat coder --runtime builtin [--skip-review]
  roster run --ask-file PATH --runtime builtin [--base trunk|current|REF] [--no-fetch|--fetch] [--skip-review]
  roster catalog parse ROOT FIELD... < catalog.yml
  roster arguments split < arguments.txt
  roster recipe validate PATH

Native Rust application under active rewrite.
Local delivery uses approved native tools and sequential builtin seats.
Local fixed-profile delivery supports at most two rescopings, two fresh perspectives and two semantic review repairs.
GitHub issue delivery, publication, resume, routing and review repair are unavailable.
Those exclusions concern GitHub-backed runs; local fixed-profile continuations above are implemented.
Unimplemented commands fail explicitly; there is no Node runtime fallback.
";

#[derive(Debug, Clone)]
pub struct Run {
    pub input: Input,
    pub base: Option<String>,
    pub sync: Option<Sync>,
    pub skip_review: bool,
}

#[derive(Debug, Clone)]
pub enum Command {
    Help,
    Init,
    Doctor,
    Run(Run),
    Recipe(PathBuf),
    Arguments,
    Catalog { root: String, fields: Vec<String> },
}

fn invalid_run() -> Error {
    Error::type_error(
        "Use roster run --seat coder --runtime builtin or roster run --ask-file PATH --runtime builtin [--base trunk|current|REF] [--no-fetch|--fetch] [--skip-review]",
    )
}

fn parse_run(args: &[String]) -> Result<Run, Error> {
    if args.iter().any(|arg| {
        [
            "--issue",
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
            "GitHub issue/publication and outer routing/resume/review-repair flags are not implemented; no local issue or publisher was substituted",
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
            "--seat" | "--runtime" | "--ask-file" | "--base" => {
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
            "Existing-TASK mode never moves its current worktree; --base/--fetch/--no-fetch apply only to --ask-file",
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
        [command] if command == "doctor" => Ok(Command::Doctor),
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
