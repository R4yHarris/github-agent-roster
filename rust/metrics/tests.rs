use super::*;
use crate::run::{MAX_AI_RUN_BYTES, parse_compact, parse_message};
use std::fmt::Write;

fn line() -> &'static str {
    "1|local|owner/model@v1|h|0/9007199254740993|-|native-session|fix-one"
}

#[test]
fn compact_parser_matches_version_one_unknown_zero_and_large_decimal_wire() -> Result<(), Error> {
    let value = parse_compact(&Text::from(line()), Some(&Text::from("owner/model")))?
        .ok_or_else(|| Error::type_error("Missing compact record"))?;
    assert_eq!(value.get("context_used"), Some(&Value::Number(0.0)));
    assert_eq!(
        value.get("context_max"),
        Some(&Value::string("9007199254740993"))
    );
    assert_eq!(value.get("context_out"), Some(&Value::Null));
    assert_eq!(value.get("schema"), Some(&Value::Number(1.0)));
    let maximum = parse_compact(
        &Text::from("1|-|m@-|x|9007199254740991/0|0|-|-|future ignored"),
        None,
    )?
    .ok_or_else(|| Error::type_error("Missing maximum record"))?;
    assert_eq!(
        maximum.get("context_used"),
        Some(&Value::Number(crate::MAX_SAFE_INTEGER as f64))
    );
    assert_eq!(maximum.get("context_max"), Some(&Value::Number(0.0)));
    assert_eq!(maximum.get("provider"), Some(&Value::Null));
    assert_eq!(maximum.get("session"), Some(&Value::Null));
    assert!(parse_compact(&Text::default(), None)?.is_none());
    Ok(())
}

#[test]
fn compact_parser_rejects_malformed_core_fields_but_ignores_bounded_future_columns()
-> Result<(), Error> {
    for source in [
        "2|local|m@v1|h|0/1|0|s|t",
        "1|vllm|m@v1|h|0/1|0|s|t",
        "1|local|m|h|0/1|0|s|t",
        "1|local|m@v1|high|0/1|0|s|t",
        "1|local|m@v1|h|-1/1|0|s|t",
        "1|local|m@v1|h|0/1/2|0|s|t",
        "1|local|m@v1|h|0/1|0|bad session|t",
        "1|local|m@v1|h|0/1|0|s|t ",
        "1|local|m@v1|h|0/1|0|s",
    ] {
        assert!(
            parse_compact(&Text::from(source), None).is_err(),
            "{source}"
        );
    }
    assert!(parse_compact(&Text::from(line()), Some(&Text::from("different"))).is_err());
    let prefix = format!("{}|", line());
    let exact = format!("{prefix}{}", "a".repeat(MAX_AI_RUN_BYTES - prefix.len()));
    assert!(parse_compact(&Text::from(exact.as_str()), None)?.is_some());
    assert!(parse_compact(&Text::from(format!("{exact}a").as_str()), None).is_err());
    Ok(())
}

#[test]
fn trailer_parser_requires_a_real_final_block_and_uses_latest_duplicate_trailer()
-> Result<(), Error> {
    let sha = "a".repeat(40);
    let message = format!(
        "Subject\r\n\r\nAI-Model: wrong\r\nAI-Model: owner/model\r\nAI-Run: invalid\r\nAI-Run: {}\r\n",
        line()
    );
    assert!(parse_message(&Text::from(message.as_str()), &sha)?.is_some());
    for message in [
        format!("AI-Run: {}", line()),
        format!("Subject\nAI-Run: {}", line()),
        format!("Subject\n\nAI-Run: {}\nnot a trailer", line()),
        format!("Subject mentions AI-Run: {}\n", line()),
        format!("Subject\n\nai-run: {}\n", line()),
    ] {
        assert!(parse_message(&Text::from(message.as_str()), &sha)?.is_none());
    }
    assert!(
        parse_message(
            &Text::from(format!("Subject\n\nAI-Model: different\nAI-Run: {}\n", line()).as_str()),
            &sha
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn git_history_frames_are_complete_utf8_unique_full_sha1_or_sha256_and_fail_atomically()
-> Result<(), Error> {
    let first = format!("{}\0Subject\n\nAI-Run: {}\n\0\n", "a".repeat(40), line());
    let second = format!(
        "{}\0Other\n\nAI-Run: 1|openai|second@-|l|-/-|-|-|docs-guide\n\0\n",
        "f".repeat(64)
    );
    let rows = parse_git_history(format!("{first}{second}").as_bytes())?;
    assert_eq!(rows.len(), 2);
    assert!(parse_git_history(b"")?.is_empty());
    assert!(
        parse_git_history(format!("{}\0Ordinary commit\n\0\n", "b".repeat(40)).as_bytes())?
            .is_empty()
    );
    for bytes in [
        format!("{first}truncated").into_bytes(),
        format!("{first}{first}").into_bytes(),
        b"abc\0Subject\0\n".to_vec(),
        vec![0xff],
        format!(
            "{first}{}\0Subject\n\nAI-Run: invalid\n\0\n",
            "b".repeat(40)
        )
        .into_bytes(),
    ] {
        assert!(parse_git_history(&bytes).is_err());
    }
    Ok(())
}

#[test]
fn git_history_byte_and_commit_frame_limits_have_exact_thresholds() -> Result<(), Error> {
    let prefix = format!("{}\0Subject\n\n", "a".repeat(40));
    let suffix = "\0\n";
    let exact = format!(
        "{prefix}{}{suffix}",
        "a".repeat(learning::MAX_JSONL_BYTES - prefix.len() - suffix.len())
    );
    assert_eq!(exact.len(), learning::MAX_JSONL_BYTES);
    assert!(parse_git_history(exact.as_bytes())?.is_empty());
    assert!(
        parse_git_history(format!("{exact} ").as_bytes())
            .is_err_and(|error| error.code == Some("ROSTER_ANALYTICS_SIZE"))
    );
    let mut frames = String::new();
    for index in 0..learning::MAX_ROWS {
        write!(frames, "{index:040x}\0Ordinary\n\0\n")
            .map_err(|_| Error::type_error("Could not format a bounded commit fixture"))?;
    }
    assert!(parse_git_history(frames.as_bytes())?.is_empty());
    assert!(
        parse_git_history(
            format!("{frames}{:040x}\0Ordinary\n\0\n", learning::MAX_ROWS).as_bytes()
        )
        .is_err_and(|error| error.code == Some("ROSTER_ANALYTICS_SIZE"))
    );
    Ok(())
}

#[test]
fn ref_operands_validate_before_git_or_file_access_and_ranges_never_become_options()
-> Result<(), Error> {
    for reference in [
        "HEAD",
        "HEAD~3..HEAD",
        "main...HEAD",
        "refs/heads/agents/task",
        "HEAD^{commit}",
        "..HEAD",
        "HEAD..",
        &"a".repeat(64),
    ] {
        validate_reference(reference)?;
    }
    for reference in [
        "",
        "--all",
        "HEAD --all",
        "HEAD..--all",
        "-n1",
        "HEAD:path",
        "HEAD\nother",
        "HEAD\0",
        "a..b..c",
        "HEAD...-x",
        "HEAD;effect",
        &"a".repeat(513),
    ] {
        assert!(validate_reference(reference).is_err(), "{reference}");
    }
    assert!(
        Options {
            reference: Some("--all".to_owned()),
            evals_path: Some(PathBuf::from("missing")),
            ..Options::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Options {
            evals_path: Some(PathBuf::from("")),
            ..Options::default()
        }
        .validate()
        .is_err()
    );
    Ok(())
}

#[test]
fn native_provenance_uses_served_model_real_unknown_usage_and_never_starting_commit_or_requested_alias()
-> Result<(), Error> {
    let source = Value::object(vec![
        ("runId", Value::string("run")),
        ("sessionId", Value::string("native-planner")),
        ("event", Value::string("session")),
        (
            "payload",
            Value::object(vec![
                (
                    "repository",
                    Value::object(vec![("commit", Value::string(&"a".repeat(40)))]),
                ),
                (
                    "issue",
                    Value::object(vec![("task", Value::string("fix-one"))]),
                ),
                (
                    "seat",
                    Value::object(vec![("name", Value::string("planner"))]),
                ),
                ("requestedModel", Value::string("request-alias")),
                ("servedModel", Value::string("served-model")),
                (
                    "metrics",
                    Value::object(vec![("tokens_prompt", Value::Number(0.0))]),
                ),
                ("task_class", Value::string("fix")),
            ]),
        ),
    ]);
    let mut record = api::build_record(
        &source,
        Some(&Value::string(&format!("sha256-{}", "a".repeat(64)))),
        Value::Number(0.0),
        &redaction::Config::default(),
        true,
    )?;
    domain::put(&mut record, "section", Value::string(api::RAW_HISTORY))?;
    let projected = project_provenance(&record)?
        .ok_or_else(|| Error::type_error("Missing native projection"))?;
    assert_eq!(projected.get("model"), Some(&Value::string("served-model")));
    assert_eq!(projected.get("prompt_tokens"), Some(&Value::Number(0.0)));
    assert!(projected.get("completion_tokens").is_none());
    assert!(projected.get("sha").is_none());
    assert!(!projected.json().contains("request-alias"));
    domain::put(&mut record, "servedModel", Value::string(""))?;
    assert!(project_provenance(&record)?.is_some_and(|projected| projected.get("model").is_none()));
    Ok(())
}

#[test]
fn source_budget_and_redacted_provider_causes_fail_without_zero_success() -> Result<(), Error> {
    let mut budget = Budget { bytes: 0, rows: 0 };
    budget.add(MAX_SOURCE_BYTES, learning::MAX_ROWS)?;
    assert!(budget.add(1, 0).is_err());
    let mut budget = Budget {
        bytes: 0,
        rows: learning::MAX_ROWS,
    };
    assert!(budget.add(0, 1).is_err());
    let cause = Error::new(
        "NativeGitError",
        "test-only-private-api-key C:\\Users\\private-user\\file https://private.invalid",
    )
    .coded("EACCES");
    let error = provider_error("read local Git history", cause);
    assert_eq!(error.code, Some("ROSTER_ANALYTICS_GIT"));
    let cause = error
        .cause
        .ok_or_else(|| Error::type_error("Missing safe provider cause"))?;
    assert_eq!(cause.code, Some("EACCES"));
    assert!(!cause.message.contains("private"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_analytics_refuses_before_host_or_repository_discovery() -> Result<(), Error> {
    let source = crate::cancel::CancellationSource::default();
    source.cancel(crate::cancel::Reason::Cancelled)?;
    let result = load(
        Path::new("unread-owned-fixture"),
        &Value::Null,
        &Options::default(),
        &source.token(),
    )
    .await;
    assert!(result.is_err_and(|error| error.code == Some("ROSTER_CANCELLED")));
    Ok(())
}
