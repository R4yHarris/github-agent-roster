use super::*;

const INVALID_OPTION: &str = "--roster-review-invalid-option";

struct Command {
    check: usize,
    args: Vec<String>,
    command: String,
    expect_error: bool,
    json: bool,
    implemented: bool,
}

pub struct Report {
    stamp: backend::Stamp,
    value: Value,
    pass: bool,
}

impl Report {
    pub fn as_value(&self) -> &Value {
        &self.value
    }
    pub fn passed(&self) -> bool {
        self.pass
    }
    pub async fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.stamp
            .require_current(backend::Owner::Execution(boundary))
            .await
    }
}

fn commands(checks: &[Text]) -> Result<Vec<Command>, Error> {
    let mut commands = Vec::new();
    for (check, text) in checks.iter().enumerate() {
        let text = text.to_utf8()?;
        for capture in regex(r"`([^`\r\n]+)`")?.captures_iter(&text) {
            let raw = capture.get(1).map_or("", |value| value.as_str()).trim();
            let native = regex(r#"(?i)^["']?roster(?:\.exe)?["']?(?:\s|$)"#)?.is_match(raw);
            if !native && !raw.starts_with("node ") {
                continue;
            }
            if raw
                .chars()
                .any(|character| "|;&<>`$(){}\r\n".contains(character))
            {
                return Err(denied("Read-only acceptance command contains shell syntax"));
            }
            let words =
                crate::arguments::split(&Text::from(raw), "Read-only CLI acceptance command")?
                    .iter()
                    .map(Text::to_utf8)
                    .collect::<Result<Vec<_>, _>>()?;
            let mut args = if words.first().is_some_and(|word| {
                word.eq_ignore_ascii_case("roster") || word.eq_ignore_ascii_case("roster.exe")
            }) {
                words.iter().skip(1).cloned().collect::<Vec<_>>()
            } else if words.first().is_some_and(|word| word == "node")
                && words.get(1).is_some_and(|word| {
                    ["src/cli.mjs", "./src/cli.mjs", "bin/roster.mjs"].contains(&word.as_str())
                })
            {
                words.iter().skip(2).cloned().collect::<Vec<_>>()
            } else {
                if native {
                    return Err(denied(
                        "Recognized native CLI invocation could not be parsed",
                    ));
                }
                continue;
            };
            if args.len() > 32 || args.iter().any(|arg| arg.len() > 500) {
                return Err(denied(
                    "Read-only acceptance command exceeds argument bounds",
                ));
            }
            let first = args.first().map_or("", String::as_str);
            let second = args.get(1).map_or("", String::as_str);
            let safe = match first {
                "" | "--help" | "-h" | "status" | "stats" | "recommend" | "doctor" => true,
                "history" => ["list", "show"].contains(&second),
                "recipe" => second == "validate",
                "fleet" => second == "list",
                "arguments" => second == "split",
                "catalog" => second == "parse",
                _ => false,
            };
            if !safe
                || args.iter().any(|arg| {
                    [
                        "--warm",
                        "--discover",
                        "--config",
                        "--home",
                        "--repo",
                        "--cwd",
                        "--state-home",
                        "--worktree",
                    ]
                    .contains(&arg.split('=').next().unwrap_or("").trim())
                })
            {
                return Err(denied(
                    "Selected acceptance command is not approved read-only native authority",
                ));
            }
            let implemented = match first {
                "" | "--help" | "-h" => args.len() <= 1,
                "recipe" => second == "validate",
                "arguments" => second == "split",
                "catalog" => second == "parse",
                _ => false,
            };
            if first == "status" && !args.iter().any(|arg| arg == "--offline") {
                args.push("--offline".to_owned());
            }
            let command = format!("roster {}", args.join(" "));
            if commands.iter().any(|old: &Command| old.command == command) {
                continue;
            }
            let json = args.iter().any(|arg| arg == "--json")
                || args.windows(2).any(|pair| pair == ["--format", "json"])
                || text.contains("JSON");
            commands.push(Command {
                check: check + 1, args, command,
                expect_error: regex(r"(?i)\b(?:errors?|fail(?:s|ure)?|reject(?:s|ed)?|refuses?|non-?zero|invalid|exits? [1-9])\b")?.is_match(&text),
                json, implemented,
            });
            if commands.len() > 3 {
                return Err(denied(
                    "Acceptance selects more than three read-only CLI commands; none may be silently omitted",
                ));
            }
        }
    }
    Ok(commands)
}

pub fn selected_commands(checks: &[Text]) -> Result<Value, Error> {
    Ok(Value::Array(
        commands(checks)?
            .iter()
            .map(|command| {
                Value::object(vec![
                    ("check", Value::Number(command.check as f64)),
                    ("command", Value::string(&command.command)),
                    ("args", strings(&command.args)),
                    ("expectFailure", Value::Boolean(command.expect_error)),
                    ("implemented", Value::Boolean(command.implemented)),
                ])
            })
            .collect(),
    ))
}

fn json_output(output: &Text) -> bool {
    let trimmed = Text(crate::text::trim(&output.0).to_vec());
    if trimmed.0.is_empty() {
        return false;
    }
    if crate::json::parse(&trimmed).is_ok() {
        return true;
    }
    trimmed
        .0
        .split(|unit| *unit == 10)
        .all(|line| crate::json::parse(&Text(line.to_vec())).is_ok())
}

fn crash(output: &str) -> Result<bool, Error> {
    Ok(regex(r"(?m)^thread .* panicked|^\s+at .+:\d+|^(?:Type|Reference|Syntax|Range)Error: |stack backtrace:")?.is_match(output))
}

fn abnormal_exit(run: &backend::Run) -> bool {
    run.exit.is_some_and(|exit| !(0..=255).contains(&exit))
}

fn report(status: &str, runs: Vec<Value>, failures: Vec<Value>) -> Value {
    Value::object(vec![
        ("status", Value::string(status)),
        ("runs", Value::Array(runs)),
        ("failures", Value::Array(failures)),
    ])
}

pub async fn run(
    boundary: &tools::Boundary,
    approval: &backend::Approval,
) -> Result<Report, Error> {
    approval.validate()?;
    let owner = backend::Owner::Execution(boundary);
    let stamp = backend::Stamp::capture(owner).await?;
    let commands = match commands(&boundary.document().acceptance_checks) {
        Ok(commands) => commands,
        Err(error) => {
            return Ok(Report {
                stamp,
                value: report(
                    "unavailable",
                    Vec::new(),
                    vec![Value::string(&error.message)],
                ),
                pass: false,
            });
        }
    };
    if commands.is_empty() {
        return Ok(Report {
            stamp,
            value: report("none", Vec::new(), Vec::new()),
            pass: true,
        });
    }
    if commands.iter().any(|command| !command.implemented) {
        return Ok(Report {
            stamp,
            value: report(
                "unavailable",
                Vec::new(),
                commands
                    .iter()
                    .filter(|command| !command.implemented)
                    .map(|command| {
                        Value::string(&format!(
                            "Selected native command is not implemented: {}",
                            command.command
                        ))
                    })
                    .collect(),
            ),
            pass: false,
        });
    }
    let Some(program) = approval.native_cli.as_deref() else {
        return Ok(Report {
            stamp,
            value: report(
                "unavailable",
                Vec::new(),
                vec![Value::string(
                    "Selected read-only acceptance requires an explicitly approved native CLI executable, never a Node fallback.",
                )],
            ),
            pass: false,
        });
    };
    for command in &commands {
        if command.args.first().is_some_and(|arg| arg == "recipe") {
            for path in command.args.iter().skip(2) {
                if path.starts_with('-')
                    || !policy::exact_path(path)
                    || crate::planning::forbidden_read(path)
                    || path != "RECIPE.yml"
                        && !crate::planning::is_allowed(path, &boundary.scope()?)
                {
                    return Err(denied(
                        "Read-only recipe validation must remain inside exact TASK or harness recipe scope",
                    ));
                }
                crate::paths::ensure_local_path(&owner.root().join(path), owner.root())?;
            }
        }
    }
    let scratch = scratch::Scratch::create(owner).await?;
    let outcome = async {
        let home = scratch.directory("home")?;
        crate::state::files::create_private_directories(&home.join("gh"))?;
        let mut env = counterfactual::environment(&home);
        env.remove("ROSTER_SEAT");
        let mut runs = Vec::new();
        let mut failures = Vec::new();
        for command in &commands {
            let _scope = owner.span(timing::Label::CliCommand);
            let actual = owner
                .run(
                    owner.root(),
                    program,
                    &command.args,
                    &env,
                    approval.timeout().min(30_000),
                    1024 * 1024,
                )
                .await?;
            let stdout = actual.stdout_text()?;
            let text = actual.text()?;
            let mut problems = Vec::new();
            if actual.timed_out {
                problems.push("timed out".to_owned());
            } else if actual.overflow {
                problems.push("exceeded bounded output".to_owned());
            } else if abnormal_exit(&actual) || crash(&text.to_utf8()?)? {
                problems.push("crashed".to_owned());
            } else if command.expect_error == actual.success() {
                problems.push("actual exit does not match expected success/error".to_owned());
            } else if !command.expect_error && command.json && !json_output(&stdout) {
                problems.push("stdout is not valid JSON or JSON lines".to_owned());
            }
            let mut invalid_args = command.args.clone();
            invalid_args.push(INVALID_OPTION.to_owned());
            let invalid = owner
                .run(
                    owner.root(),
                    program,
                    &invalid_args,
                    &env,
                    approval.timeout().min(30_000),
                    1024 * 1024,
                )
                .await?;
            let invalid_text = invalid.text()?;
            if invalid.success() {
                problems.push("accepted the unknown option".to_owned());
            } else if invalid.timed_out
                || invalid.overflow
                || abnormal_exit(&invalid)
                || crash(&invalid_text.to_utf8()?)?
            {
                problems.push("invalid-option control crashed or exceeded limits".to_owned());
            }
            for problem in &problems {
                failures.push(Value::string(&format!(
                    "End-to-end: {} (check {}) {problem}.",
                    command.command, command.check
                )));
            }
            let output = crate::planning::memory::redact(&text, owner.redaction());
            let invalid_output = crate::planning::memory::redact(&invalid_text, owner.redaction());
            runs.push(Value::object(vec![
                ("command", Value::string(&command.command)),
                ("args", strings(&command.args)),
                ("check", Value::Number(command.check as f64)),
                (
                    "exitCode",
                    actual
                        .exit
                        .map_or(Value::Null, |exit| Value::Number(f64::from(exit))),
                ),
                (
                    "output",
                    Value::String(Text(output.0.into_iter().take(3000).collect())),
                ),
                (
                    "errorCase",
                    Value::object(vec![
                        (
                            "exitCode",
                            invalid
                                .exit
                                .map_or(Value::Null, |exit| Value::Number(f64::from(exit))),
                        ),
                        (
                            "output",
                            Value::String(Text(invalid_output.0.into_iter().take(500).collect())),
                        ),
                    ]),
                ),
                ("problems", strings(&problems)),
            ]));
        }
        let pass = failures.is_empty();
        Ok((
            report(if pass { "pass" } else { "fail" }, runs, failures),
            pass,
        ))
    }
    .await;
    let (value, pass) = backend::finish_owned(owner, outcome, scratch.close().await)?;
    stamp.require_current(owner).await?;
    Ok(Report { stamp, value, pass })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_utf8_output_and_crash_exits_are_never_expected_error_success() -> Result<(), Error> {
        let mut run = backend::Run {
            exit: Some(-1_073_741_819),
            stdout: vec![b'o', b'k', 0xe2, 0x82],
            stderr: Vec::new(),
            timed_out: false,
            overflow: true,
        };
        assert_eq!(run.stdout_text()?, Text::from("ok"));
        assert!(abnormal_exit(&run));
        run.overflow = false;
        assert!(run.stdout_text().is_err());
        for exit in [0, 1, 2, 255] {
            run.exit = Some(exit);
            assert!(!abnormal_exit(&run));
        }
        for option in [
            "--config",
            "--config=outside",
            "--home=outside",
            "--discover=true",
        ] {
            assert!(
                selected_commands(&[Text::from(
                    format!("`roster arguments split {option}` is refused.").as_str()
                )])
                .is_err()
            );
        }
        Ok(())
    }
}
