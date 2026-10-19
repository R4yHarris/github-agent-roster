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
        assert!(text.contains(
            "GitHub issue delivery, publication, resume, routing and review repair are unavailable."
        ));
        assert!(text.contains("there is no Node runtime fallback"));
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
        (&["run", "--issue", "1"][..], &b""[..], "not implemented"),
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
