use std::env;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;

use github_agent_roster::text::Text;
use github_agent_roster::{
    Error, arguments, cancel, catalog, config, delivery, evaluation, execution, host, json,
    learning, llm, local_cli, metrics, recipe, start, status,
};

const MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;
fn read_input() -> Result<Text, Error> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::new("Error", format!("Cannot read native input: {error}")))?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(Error::type_error("Native input exceeds 4 MiB"));
    }
    String::from_utf8(bytes)
        .map(|source| Text::from(source.as_str()))
        .map_err(|_| Error::type_error("Native input must be UTF-8"))
}

struct Output {
    stdout: String,
    success: bool,
    diagnostic: Option<Error>,
}

impl Output {
    fn success(stdout: String) -> Self {
        Self {
            stdout,
            success: true,
            diagnostic: None,
        }
    }
}

fn environment() -> Result<json::Value, Error> {
    Ok(json::Value::Object(
        env::vars_os()
            .map(|(key, value)| {
                Ok((
                    Text::from(
                        key.into_string()
                            .map_err(|_| {
                                Error::type_error("Invocation environment names must be Unicode")
                            })?
                            .as_str(),
                    ),
                    json::Value::string(
                        value
                            .into_string()
                            .map_err(|_| {
                                Error::type_error("Invocation environment values must be Unicode")
                            })?
                            .as_str(),
                    ),
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?,
    ))
}

async fn native(
    command: local_cli::Command,
    cwd: &Path,
    installation: &Path,
    environment: &json::Value,
) -> Result<Output, Error> {
    let source = cancel::CancellationSource::default();
    let token = source.token();
    let operation = async {
        match command {
            local_cli::Command::Stats(flags) => {
                let evidence = metrics::load(cwd, environment, &flags.metrics, &token).await?;
                let groups = evidence.groups()?;
                let output = if flags.json {
                    learning::bounded_output(format!("{}\n", json::Value::Array(groups).pretty()))?
                } else {
                    learning::format_metrics(&groups)?
                };
                tokio::task::yield_now().await;
                token.check()?;
                Ok(Output::success(output))
            }
            local_cli::Command::Recommend(flags) => {
                let evidence =
                    metrics::load(cwd, environment, &metrics::Options::default(), &token).await?;
                let recommendation =
                    evidence.recommendation(&flags.task_class, flags.difficulty)?;
                let defaults_only = config::environment_value(
                    environment,
                    "ROSTER_ANALYTICS_CONFIG_DEFAULTS_ONLY",
                )?;
                if defaults_only.is_some_and(|value| value != &Text::from("true")) {
                    return Err(Error::type_error(
                        "ROSTER_ANALYTICS_CONFIG_DEFAULTS_ONLY must be true when set",
                    ));
                }
                let configured = if recommendation.is_some() || defaults_only.is_some() {
                    config::defaults()?
                } else {
                    config::load(installation, cwd, environment)
                        .map_err(|cause| {
                            let mut safe = Error::new(
                                cause.name,
                                "Configuration diagnostics were omitted; existing bytes were preserved.",
                            );
                            safe.code = cause.code;
                            Error::new(
                                "LearningError",
                                "Could not load recommendation defaults; verify checked configuration files and permissions",
                            )
                            .coded("ROSTER_ANALYTICS_CONFIG")
                            .caused_by(safe)
                        })?
                        .config
                };
                let output = if flags.json {
                    if let Some(recommendation) = recommendation {
                        learning::bounded_output(format!("{}\n", recommendation.pretty()))?
                    } else {
                        let default = learning::format_recommendation(
                            None,
                            &flags.task_class,
                            configured.as_value(),
                            environment,
                        )?;
                        learning::bounded_output(format!(
                            "{}\n",
                            json::Value::object(vec![
                                ("recommendation", json::Value::Null),
                                (
                                    "message",
                                    json::Value::string(default.trim_end_matches('\n'))
                                ),
                            ])
                            .pretty()
                        ))?
                    }
                } else {
                    learning::format_recommendation(
                        recommendation.as_ref(),
                        &flags.task_class,
                        configured.as_value(),
                        environment,
                    )?
                };
                let safe = github_agent_roster::state::redaction::diagnostic(
                    &Text::from(output.as_str()),
                    &github_agent_roster::state::redaction::Config {
                        environment: environment.clone(),
                        ..Default::default()
                    },
                );
                tokio::task::yield_now().await;
                token.check()?;
                Ok(Output::success(safe.to_utf8()?))
            }
            local_cli::Command::Eval(decision) => {
                let recorded = evaluation::record(cwd, environment, &decision, &token).await?;
                Ok(Output::success(recorded.format()))
            }
            local_cli::Command::Status(options) => {
                let observed = if options.offline {
                    status::observe(cwd, environment, &options, &token).await?
                } else {
                    let mut transport =
                        llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
                    status::read(
                        &mut transport,
                        installation,
                        cwd,
                        environment,
                        &options,
                        &token,
                    )
                    .await?
                };
                Ok(Output::success(observed.format(options.json)?))
            }
            local_cli::Command::Doctor => {
                let result = local_cli::doctor(installation, cwd, environment, &token).await?;
                Ok(Output {
                    stdout: result.output,
                    success: result.ok,
                    diagnostic: None,
                })
            }
            local_cli::Command::Run(flags) => {
                let loaded = config::load(installation, cwd, environment)?;
                if flags.skip_review && config::is_review_required(loaded.config.as_value())? {
                    return Err(Error::type_error(
                        "--skip-review requires private review.required: false; required review was not bypassed",
                    ));
                }
                let (host, approval) = host::invocation_host(
                    environment,
                    &env::current_exe().map_err(|_| {
                        Error::new("Error", "Cannot resolve the current native CLI image")
                    })?,
                )?;
                let start =
                    start::Options::resolve(&loaded.config, flags.base.as_deref(), flags.sync)?;
                let fetch = match config::environment_value(environment, "ROSTER_ALLOW_FETCH")? {
                    None => start::FetchPolicy::Deny,
                    Some(value) if value == &Text::from("true") => start::FetchPolicy::Http,
                    Some(value) if value == &Text::from("false") => start::FetchPolicy::Deny,
                    Some(_) => {
                        return Err(Error::type_error(
                            "ROSTER_ALLOW_FETCH must be true or false when set",
                        ));
                    }
                };
                let credentials = delivery::native_invocation_credentials(environment, &loaded)?;
                let timeout = cancel::validate_task_timeout(
                    loaded
                        .config
                        .as_value()
                        .get("seat")
                        .and_then(|value| value.get("task_timeout_ms"))
                        .and_then(json::Value::as_number),
                )?;
                let mut transport =
                    llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
                let mut observer = llm::control::NoObserver;
                let admission = llm::admission::Admission::default();
                let mut driver = delivery::Driver {
                    transport: &mut transport,
                    observer: &mut observer,
                    credentials: &credentials,
                    admission: &admission,
                };
                let mut ownership = delivery::Ownership::default();
                let result = execution::controls::with_task_deadline(&token, timeout, |token| {
                    Box::pin(driver.run_local(
                        delivery::Options {
                            cwd,
                            installation,
                            loaded: &loaded,
                            input: flags.input,
                            host,
                            approval,
                            start,
                            fetch,
                            skip_review: flags.skip_review,
                        },
                        token,
                        &mut ownership,
                    ))
                })
                .await
                .map_err(|error| {
                    delivery::safe_invocation_error(&error, &loaded, &credentials).unwrap_or(error)
                })?;
                let diagnostic = result
                    .failure
                    .as_ref()
                    .map(|error| delivery::safe_invocation_error(error, &loaded, &credentials))
                    .transpose()?;
                Ok(Output {
                    stdout: result.format(),
                    success: result.succeeded(),
                    diagnostic,
                })
            }
            _ => Err(Error::type_error("Invalid asynchronous native command")),
        }
    };
    tokio::pin!(operation);
    tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            source.cancel(cancel::Reason::Cancelled)?;
            let result = operation.await;
            if signal.is_err() {
                let mut unavailable = Error::new("SignalUnavailable", "Ctrl+C notification could not be installed; owned work was cancelled and awaited")
                    .coded("ROSTER_SIGNAL_UNAVAILABLE");
                if let Err(error) = result {
                    if matches!(error.code, Some("ROSTER_TASK_TIMEOUT" | "ROSTER_PROCESS_CLEANUP")) {
                        return Err(error);
                    }
                    if let Some(position) = error.message.find("\nWorktree: ") {
                        unavailable.message.push_str(error.message.get(position..).ok_or_else(|| Error::type_error("Invalid cancellation location"))?);
                    }
                    return Err(unavailable.caused_by(error));
                }
                return Err(unavailable);
            }
            match result {
                Err(error) if matches!(error.code, Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT" | "ROSTER_PROCESS_CLEANUP")) => Err(error),
                Err(error) => Err(cancel::Reason::Cancelled.error().caused_by(error)),
                Ok(_) => Err(cancel::Reason::Cancelled.error()),
            }
        }
        result = &mut operation => result,
    }
}

fn run(args: &[String]) -> Result<Output, Error> {
    let command = local_cli::parse(args)?;
    let value = match command {
        local_cli::Command::Help => return Ok(Output::success(local_cli::HELP.to_owned())),
        local_cli::Command::Init => {
            return local_cli::initialize(
                &env::current_dir()
                    .map_err(|_| Error::new("Error", "Cannot resolve the invocation directory"))?,
            )
            .map(Output::success);
        }
        local_cli::Command::FleetList => {
            let cwd = env::current_dir()
                .map_err(|_| Error::new("Error", "Cannot resolve the invocation directory"))?;
            return local_cli::list_fleet(&cwd, &environment()?).map(Output::success);
        }
        local_cli::Command::Doctor
        | local_cli::Command::Status(_)
        | local_cli::Command::Run(_)
        | local_cli::Command::Stats(_)
        | local_cli::Command::Recommend(_)
        | local_cli::Command::Eval(_) => {
            let environment = environment()?;
            let installation = host::selected_path(&environment, "ROSTER_INSTALLATION_ROOT")?
                .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf());
            let cwd = env::current_dir()
                .map_err(|_| Error::new("Error", "Cannot resolve the invocation directory"))?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| {
                    Error::new(
                        "RuntimeUnavailable",
                        "Cannot initialize the native invocation runtime",
                    )
                })?;
            return runtime.block_on(native(command, &cwd, &installation, &environment));
        }
        local_cli::Command::Recipe(file) => {
            recipe::validate_file(&file)?;
            return Ok(Output::success(format!(
                "Valid recipe: {}\n",
                file.display()
            )));
        }
        local_cli::Command::Arguments => {
            let values = arguments::split(&read_input()?, arguments::USAGE)?;
            format!(
                "[{}]",
                values.iter().map(Text::json).collect::<Vec<_>>().join(",")
            )
        }
        local_cli::Command::Catalog { root, fields } => {
            let fields = fields.iter().map(String::as_str).collect::<Vec<_>>();
            let parsed = catalog::parse(&read_input()?, &root, &fields)?;
            format!(
                "{{\"entries\":{},\"formatted\":{}}}",
                parsed.json(),
                catalog::format(&root, &parsed).json()
            )
        }
    };
    Ok(Output::success(format!("{value}\n")))
}

fn main() -> ExitCode {
    let result = env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| Error::type_error("Native arguments must be Unicode"))
        })
        .collect::<Result<Vec<_>, _>>()
        .and_then(|args| run(&args));
    match result {
        Ok(output) => {
            if let Err(error) = io::stdout().lock().write_all(output.stdout.as_bytes()) {
                eprintln!("Cannot write native output: {error}");
                return ExitCode::FAILURE;
            }
            if let Some(error) = output.diagnostic {
                eprintln!("{}", error.message);
            }
            if output.success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("{}", error.message);
            match error.code {
                Some("ROSTER_CANCELLED") => ExitCode::from(130),
                Some("ROSTER_TASK_TIMEOUT") => ExitCode::from(124),
                _ => ExitCode::FAILURE,
            }
        }
    }
}
