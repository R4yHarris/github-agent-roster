use super::*;
use crate::state::redaction;

#[test]
fn every_surrogate_unit_survives_normalization_without_becoming_whitespace() -> Result<(), Error> {
    for unit in 0xd800..=0xdfff {
        let source = Value::String(Text(vec![unit]));
        assert_eq!(task::one_line(&source, "Value")?, Text(vec![unit]));
        assert_eq!(task::acceptance_check(&source, 0)?, Text(vec![unit]));
    }
    Ok(())
}

#[test]
fn lossless_validation_errors_preserve_lone_surrogates_and_normal_errors() -> Result<(), Error> {
    let source = Text(vec![65, 0xd820, 66]);
    let error = Error::from_text("TypeError", source.clone());
    assert_eq!(error.message_json(), "\"A\\ud820B\"");
    assert_eq!(error.message_text, Some(source));
    assert_eq!(
        Error::from_text("TypeError", Text::from("ordinary")),
        Error::type_error("ordinary")
    );
    let invalid = Value::object(vec![(
        "new_exports",
        Value::Array(vec![Value::object(vec![
            ("file", Value::string("src/new.mjs")),
            ("name", Value::String(Text(vec![0xd820]))),
        ])]),
    )]);
    let error = grounding::normalize_design(Some(&invalid))
        .err()
        .ok_or_else(|| Error::new("Error", "Malformed identifier was accepted"))?;
    assert!(error.message_json().contains("\\ud820"));
    Ok(())
}

#[test]
fn private_key_prefixes_are_denied_even_without_a_complete_pem_footer() {
    for source in [
        "-----BEGIN PRIVATE KEY-----\nnot-a-key",
        "Prior text\n-----BEGIN RSA PRIVATE KEY-----\nnot-a-key",
        "-----BEGIN EC PRIVATE KEY-----\n",
    ] {
        assert_ne!(
            memory::redact(&Text::from(source), &redaction::Config::default()),
            Text::from(source)
        );
    }
}

#[test]
fn nested_remote_markdown_never_becomes_local_file_authority() -> Result<(), Error> {
    for ask in [
        "[src/one.mjs](https://example.invalid/[src/two.mjs](https://example.invalid/two))",
        "[src/one.mjs](https://example.invalid/a\\\nnext/file.mjs)",
    ] {
        assert!(
            intake::requirements(&Value::string(ask), true)?
                .files
                .is_empty()
        );
    }
    Ok(())
}

#[test]
fn authoritative_scope_rejects_truncation_and_nested_vendor_paths() {
    for file in [
        format!("src/{}.mjs", "x".repeat(245)),
        "nested/vendor/file.mjs".to_owned(),
    ] {
        let assignment = lifecycle::Assignment {
            ask: Text::from(format!("Change one module.\n\n## Files allowed\n- {file}").as_str()),
            title: None,
            reference: "issue:42".to_owned(),
            task: "issue-42".to_owned(),
            session: "planner-42".to_owned(),
        };
        assert!(assignment.validate().is_err());
    }
}

#[test]
fn estimation_validates_canonical_iso_dates_across_the_javascript_time_range() -> Result<(), Error>
{
    for at in [
        "0000-02-29T00:00:00.000Z",
        "+010000-01-01T00:00:00.000Z",
        "-000001-12-31T23:59:59.999Z",
        "+275760-09-13T00:00:00.000Z",
        "-271821-04-20T00:00:00.000Z",
    ] {
        estimate::validate_evaluation(&Value::object(vec![
            ("session", Value::string("timestamp-case")),
            ("verdict", Value::string("accept")),
            ("difficulty", Value::Number(1.0)),
            ("again", Value::Boolean(true)),
            ("at", Value::string(at)),
        ]))?;
    }
    Ok(())
}
