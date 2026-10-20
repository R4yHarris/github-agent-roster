use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use github_agent_roster::state::files;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn invoke(args: &[&str], input: &[u8]) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_roster"))
        .args(args)
        .env("PATH", "")
        .env_remove("NODE_OPTIONS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let writing = child
        .stdin
        .take()
        .ok_or("Native CLI stdin was not piped")?
        .write_all(input);
    if let Err(error) = writing {
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()?;
        return Err(Box::new(error));
    }
    Ok(child.wait_with_output()?)
}

#[test]
fn help_runs_without_node_or_an_endpoint_and_declares_the_incomplete_native_surface() -> TestResult
{
    for args in [&[][..], &["--help"][..], &["-h"][..]] {
        let output = invoke(args, &[])?;
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout)?;
        assert!(text.contains("roster recipe validate PATH"));
        assert!(
            text.contains(
                "Local delivery uses approved native tools and sequential builtin seats."
            )
        );
        assert!(text.contains("GitHub publication, resume and automatic routing are unavailable."));
        assert!(text.contains("there is no Node runtime fallback"));
    }
    Ok(())
}

#[test]
fn issue_run_parser_preserves_explicit_mode_flags_and_refuses_ambiguous_inputs() -> TestResult {
    use github_agent_roster::{delivery::Input, local_cli, start::Sync};

    for (args, number, base, sync, skip_review) in [
        (
            vec!["run", "--issue", "42", "--runtime", "builtin"],
            42,
            None,
            None,
            false,
        ),
        (
            vec![
                "run",
                "--skip-review",
                "--base",
                "current",
                "--no-fetch",
                "--issue",
                "9007199254740991",
                "--runtime",
                "builtin",
            ],
            9_007_199_254_740_991,
            Some("current"),
            Some(Sync::Offline),
            true,
        ),
        (
            vec![
                "run",
                "--runtime",
                "builtin",
                "--fetch",
                "--issue",
                "1",
                "--base",
                "refs/heads/main",
            ],
            1,
            Some("refs/heads/main"),
            Some(Sync::Fetch),
            false,
        ),
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let local_cli::Command::Run(run) = local_cli::parse(&args)? else {
            return Err("Issue invocation did not parse as a run".into());
        };
        assert!(matches!(run.input, Input::GitHubIssue(actual) if actual == number));
        assert_eq!(run.base.as_deref(), base);
        assert_eq!(run.sync, sync);
        assert_eq!(run.skip_review, skip_review);
    }
    for number in [
        "",
        "0",
        "01",
        "+1",
        "1.0",
        "1e2",
        "9007199254740992",
        " 1",
        "1 ",
    ] {
        let args = ["run", "--issue", number, "--runtime", "builtin"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(local_cli::parse(&args).is_err(), "{number}");
    }
    for flags in [
        vec!["--issue", "42", "--issue", "43"],
        vec!["--issue", "42", "--seat", "coder"],
        vec!["--ask-file", "ASK.md", "--issue", "42"],
        vec!["--issue", "42", "--fetch", "--no-fetch"],
        vec!["--issue", "42", "--resume"],
        vec!["--issue", "42", "--publish"],
    ] {
        let args = ["run", "--runtime", "builtin"]
            .into_iter()
            .chain(flags)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(local_cli::parse(&args).is_err());
    }
    Ok(())
}

#[test]
fn arguments_preserve_quotes_unicode_and_exact_json_output_without_node() -> TestResult {
    for (input, expected) in [
        ("\"two words\" last", "[\"two words\",\"last\"]\n"),
        (
            "one 'two words' \"three\"",
            "[\"one\",\"two words\",\"three\"]\n",
        ),
        ("", "[]\n"),
        ("\u{feff}hello\u{00a0}world", "[\"hello\",\"world\"]\n"),
    ] {
        let output = invoke(&["arguments", "split"], input.as_bytes())?;
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert_eq!(output.stdout, expected.as_bytes());
    }
    Ok(())
}

#[test]
fn catalog_parsing_and_formatting_produce_the_exact_native_wire_shape() -> TestResult {
    let output = invoke(
        &["catalog", "parse", "profiles", "id"],
        b"profiles:\n  - id: example\n",
    )?;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        b"{\"entries\":[{\"id\":\"example\"}],\"formatted\":\"profiles:\\n  - id: \\\"example\\\"\\n\"}\n"
    );
    Ok(())
}

#[test]
fn malformed_inputs_and_unimplemented_commands_never_report_success() -> TestResult {
    for (args, input, expected) in [
        (&["run", "--issue", "1"][..], &b""[..], "Use roster run"),
        (&["unknown"][..], &b""[..], "not implemented"),
        (&["--oracle", "catalog"][..], &b""[..], "not implemented"),
        (
            &["arguments", "split", "--invalid"][..],
            &b""[..],
            "invalid arguments",
        ),
        (
            &["arguments", "split"][..],
            &b"\"unfinished"[..],
            "matching quotes",
        ),
        (&["arguments", "split"][..], &b"\xff"[..], "UTF-8"),
        (
            &["catalog", "parse", "p", "id"][..],
            &b"p:\n  - id: &alias"[..],
            "unsupported scalar",
        ),
    ] {
        let output = invoke(args, input)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8(output.stderr)?.contains(expected));
    }
    Ok(())
}

#[test]
fn stdin_has_an_exact_four_mebibyte_limit() -> TestResult {
    let limit = 4 * 1024 * 1024;
    let accepted = invoke(&["arguments", "split"], &vec![b' '; limit])?;
    assert!(accepted.status.success());
    assert_eq!(accepted.stdout, b"[]\n");
    assert!(accepted.stderr.is_empty());
    let refused = invoke(&["arguments", "split"], &vec![b' '; limit + 1])?;
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    assert!(String::from_utf8(refused.stderr)?.contains("exceeds 4 MiB"));
    Ok(())
}

#[test]
fn recipe_validation_reads_real_files_and_rejects_missing_or_malformed_input() -> TestResult {
    let root = std::env::temp_dir().join(format!("roster-cli-{}", files::nonce()?));
    fs::create_dir(&root)?;
    let valid = root.join("RECIPE.yml");
    fs::write(
        &valid,
        include_str!("../templates/sdlc/RECIPE.yml").replace("issue:N", "issue:42"),
    )?;
    let invalid = root.join("invalid.yml");
    fs::write(&invalid, "version: 999\n")?;
    let mut outcomes = Vec::new();
    for file in [&valid, &invalid, &root.join("missing.yml")] {
        let name = file.to_str().ok_or("CLI fixture path must be Unicode")?;
        outcomes.push(invoke(&["recipe", "validate", name], &[])?);
    }
    fs::remove_dir_all(&root)?;
    assert!(
        outcomes[0].status.success(),
        "{}",
        String::from_utf8_lossy(&outcomes[0].stderr)
    );
    assert_eq!(
        outcomes[0].stdout,
        format!("Valid recipe: {}\n", valid.display()).as_bytes()
    );
    assert!(outcomes[0].stderr.is_empty());
    for output in &outcomes[1..] {
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    assert!(
        !Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/cli.mjs")
            .exists()
    );
    Ok(())
}

#[test]
fn copilot_doctor_cli_reports_blocked_offline_readiness_and_never_echoes_secret_options()
-> TestResult {
    let blocked = invoke(&["doctor", "--copilot"], &[])?;
    assert!(!blocked.status.success());
    assert!(blocked.stderr.is_empty());
    let stdout = String::from_utf8(blocked.stdout)?;
    assert!(stdout.contains("Copilot offline diagnostics: BLOCKED"));
    assert!(stdout.contains("No interactive terminal."));
    assert!(stdout.contains("Authentication is unverified"));

    for args in [
        &["doctor", "--copilot", "--login", "secret-code"][..],
        &["doctor", "--copilot", "--warm"][..],
        &["doctor", "--copilot", "--model", "unknown"][..],
        &["doctor", "--copilot", "--runtime", "//secret-server/share"][..],
    ] {
        let invalid = invoke(args, &[])?;
        assert!(!invalid.status.success());
        assert!(invalid.stdout.is_empty());
        let stderr = String::from_utf8(invalid.stderr)?;
        assert!(!stderr.contains("secret-code"));
        assert!(!stderr.contains("secret-server"));
    }
    Ok(())
}

#[test]
fn cli_ask_clarification_checkpoint_and_question_subcommands()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!(
        "roster-cli-ask-question-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(
        root.join("src").join("api.mjs"),
        "export const value = 42;\n",
    )?;
    std::fs::write(root.join(".gitignore"), ".roster/asks/\n")?;
    let git = std::env::var_os("ROSTER_TEST_GIT")
        .map_or_else(|| std::path::PathBuf::from("git"), std::path::PathBuf::from);
    assert!(
        Command::new(&git)
            .current_dir(&root)
            .args(["init", "--quiet"])
            .status()?
            .success()
    );
    let git_dir = git
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();

    let run_in_root = |args: &[&str]| -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(Command::new(env!("CARGO_BIN_EXE_roster"))
            .current_dir(&root)
            .env("PATH", &git_dir)
            .env("ROSTER_INSTALLATION_ROOT", &root)
            .env("ROSTER_API_KEY", "")
            .env("OPENAI_API_KEY", "")
            .env("GITHUB_APP_ID", "")
            .env("GITHUB_APP_PRIVATE_KEY_PATH", "")
            .args(args)
            .output()?)
    };

    let q_out = run_in_root(&[
        "question",
        "--files",
        "src/api.mjs",
        "--",
        "How does this work?",
    ])?;
    assert!(
        q_out.status.success(),
        "{}",
        String::from_utf8_lossy(&q_out.stderr)
    );
    assert!(String::from_utf8(q_out.stdout)?.contains("1: export const value = 42;"));

    let vague = "Improve the product; keep the public API unchanged.";
    let first = run_in_root(&["ask", vague])?;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8(first.stdout)?.contains("Checkpoint saved privately"));
    let cp_path = root.join(".roster").join("asks").join("clarification.json");
    assert!(cp_path.exists());

    let pending = run_in_root(&["ask", "--pending"])?;
    assert!(pending.status.success());
    let pending_out = String::from_utf8(pending.stdout)?;
    assert!(pending_out.contains("keep the public API unchanged"));
    assert!(pending_out.contains("Assumptions"));

    let invalid = run_in_root(&["ask", "--resume", "Add a Status section to README.md."])?;
    assert!(!invalid.status.success());
    assert!(String::from_utf8(invalid.stderr)?.contains("explicitly declare Allowed Files"));
    assert!(cp_path.exists());

    let answer = "Add a Status heading.\n\n## Allowed Files\n- `README.md`\n\n## Acceptance checks\n- README has a Status heading";
    let resumed = run_in_root(&["ask", "--resume", answer])?;
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed_out = String::from_utf8(resumed.stdout)?;
    assert!(resumed_out.contains("Ask kind: slice"));
    assert!(resumed_out.contains("RECIPE:"));
    assert!(resumed_out.contains("TASK:"));
    assert!(!cp_path.exists());

    let after = run_in_root(&["ask", "--pending"])?;
    assert!(String::from_utf8(after.stdout)?.contains("No pending clarification"));

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn cli_clean_dry_run_confirmation_and_execution() -> Result<(), Box<dyn std::error::Error>> {
    let state_root = std::env::temp_dir().join(format!(
        "roster-cli-clean-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir_all(state_root.join("alpha"))?;
    std::fs::write(state_root.join("alpha").join("data.txt"), "alpha")?;
    std::fs::write(state_root.join("keep.json"), "{}")?;
    let state_arg = state_root.display().to_string();

    let dry = Command::new(env!("CARGO_BIN_EXE_roster"))
        .args(["clean", "--state-root", &state_arg])
        .output()?;
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let dry_out = String::from_utf8(dry.stdout)?;
    assert!(dry_out.contains("mode: dry-run"));
    assert!(dry_out.contains("records: 2"));
    assert!(state_root.join("alpha").exists());

    let no_yes = Command::new(env!("CARGO_BIN_EXE_roster"))
        .args(["clean", "--state-root", &state_arg, "--execute"])
        .output()?;
    assert!(!no_yes.status.success());
    assert!(String::from_utf8(no_yes.stderr)?.contains("--yes"));

    let exec = Command::new(env!("CARGO_BIN_EXE_roster"))
        .args([
            "clean",
            "--state-root",
            &state_arg,
            "--exclude",
            "keep.json",
            "--execute",
            "--yes",
        ])
        .output()?;
    assert!(
        exec.status.success(),
        "{}",
        String::from_utf8_lossy(&exec.stderr)
    );
    let exec_out = String::from_utf8(exec.stdout)?;
    assert!(exec_out.contains("mode: destructive"));
    assert!(exec_out.contains("removed: 1"));
    assert!(!state_root.join("alpha").exists());
    assert!(state_root.join("keep.json").exists());

    let _ = std::fs::remove_dir_all(&state_root);
    Ok(())
}

#[test]
fn cli_vault_set_list_and_get_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::temp_dir().join(format!(
        "roster-cli-vault-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir_all(&home)?;

    let run_vault = |args: &[&str],
                     stdin_bytes: &[u8]|
     -> Result<std::process::Output, Box<dyn std::error::Error>> {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_roster"))
            .env("USERPROFILE", &home)
            .env("HOME", &home)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(stdin_bytes)?;
        }
        Ok(child.wait_with_output()?)
    };

    let empty_list = run_vault(&["vault", "list"], b"")?;
    assert!(
        empty_list.status.success(),
        "{}",
        String::from_utf8_lossy(&empty_list.stderr)
    );
    assert!(empty_list.stdout.is_empty());

    let set_out = run_vault(&["vault", "set", "CUSTOM_API_KEY"], b"vault-secret-123\n")?;
    assert!(
        set_out.status.success(),
        "{}",
        String::from_utf8_lossy(&set_out.stderr)
    );
    assert_eq!(
        String::from_utf8(set_out.stdout)?,
        "Stored secret CUSTOM_API_KEY.\n"
    );

    let list_out = run_vault(&["vault", "list"], b"")?;
    assert!(list_out.status.success());
    assert_eq!(String::from_utf8(list_out.stdout)?, "CUSTOM_API_KEY\n");

    let get_out = run_vault(&["vault", "get", "CUSTOM_API_KEY"], b"")?;
    assert!(get_out.status.success());
    assert_eq!(String::from_utf8(get_out.stdout)?, "vault-secret-123");

    let missing_out = run_vault(&["vault", "get", "MISSING_KEY"], b"")?;
    assert!(!missing_out.status.success());
    assert!(String::from_utf8(missing_out.stderr)?.contains("No secret stored for MISSING_KEY."));

    let app_err = run_vault(&["vault", "set", "GITHUB_APP_ID"], b"123\n")?;
    assert!(!app_err.status.success());
    assert!(String::from_utf8(app_err.stderr)?.contains("GitHub App credentials"));

    let secret_arg = "cli-vault-secret-not-in-stderr-999";
    for bad_args in [
        &["vault", "set", "TOKEN", secret_arg][..],
        &["vault", "set", "../TOKEN"][..],
    ] {
        let bad = run_vault(bad_args, secret_arg.as_bytes())?;
        assert!(!bad.status.success());
        assert!(!String::from_utf8(bad.stderr)?.contains(secret_arg));
    }
    let empty_secret = run_vault(&["vault", "set", "TOKEN"], b"\n")?;
    assert!(!empty_secret.status.success());

    let git_home = home.join("git-home");
    std::fs::create_dir_all(git_home.join(".git"))?;
    let inside_git = Command::new(env!("CARGO_BIN_EXE_roster"))
        .env("USERPROFILE", &git_home)
        .env("HOME", &git_home)
        .args(["vault", "list"])
        .output()?;
    assert!(!inside_git.status.success());
    assert!(String::from_utf8(inside_git.stderr)?.contains("outside a Git worktree"));

    let _ = std::fs::remove_dir_all(&home);
    Ok(())
}

#[test]
fn cli_bench_writes_six_numeric_secret_free_timings() -> Result<(), Box<dyn std::error::Error>> {
    let mut best_dispatch = f64::INFINITY;
    for _ in 0..3 {
        let dir = std::env::temp_dir().join(format!(
            "roster-cli-bench-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir)?;

        let out = Command::new(env!("CARGO_BIN_EXE_roster"))
            .current_dir(&dir)
            .env("ROSTER_API_KEY", "bench-secret-token")
            .env("GITHUB_APP_PRIVATE_KEY_PATH", "private-key-path.pem")
            .args(["bench"])
            .output()?;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout)?;
        let file_text = std::fs::read_to_string(dir.join(".roster").join("bench.json"))?;
        assert_eq!(stdout, file_text);
        assert!(!stdout.contains("bench-secret-token"));
        assert!(!stdout.contains("private-key-path"));
        assert!(!stdout.contains(":\\"));
        let parsed = github_agent_roster::json::parse(&github_agent_roster::text::Text::from(
            stdout.as_str(),
        ))?;
        let github_agent_roster::json::Value::Object(entries) = &parsed else {
            return Err("bench report must be a JSON object".into());
        };
        let keys = entries
            .iter()
            .map(|(k, _)| k.to_utf8())
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(keys, github_agent_roster::diagnostics::BENCH_FIELDS);
        for (_, val) in entries {
            let num = val.as_number().ok_or("bench field must be a number")?;
            assert!(num.is_finite() && num >= 0.0);
        }
        let dispatch = parsed
            .get("command_dispatch_ms")
            .and_then(github_agent_roster::json::Value::as_number)
            .ok_or("missing command_dispatch_ms")?;
        if dispatch < best_dispatch {
            best_dispatch = dispatch;
        }
        let _ = std::fs::remove_dir_all(&dir);
        if best_dispatch <= 150.0 {
            break;
        }
    }
    assert!(
        best_dispatch <= 150.0,
        "dispatch took {best_dispatch}ms (> 150ms)"
    );
    Ok(())
}
