use super::*;

#[test]
fn selected_named_critic_credentials_are_typed_private_and_frozen_without_reading_unselected_keys()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    fixture.configure(Some("http://127.0.0.1:1/v1"))?;
    fixture.critic("http://127.0.0.1:2/v1")?;
    let loaded = fixture.loaded()?;
    let environment = Value::object(vec![
        (
            "ROSTER_CRITIC_API_KEY",
            Value::string("test-only-private-api-key"),
        ),
        ("OPENAI_API_KEY", Value::string("test-only-unselected-key")),
    ]);
    let credentials = delivery::invocation_credentials(&environment, &loaded)?;
    assert_eq!(
        credentials
            .environment
            .as_ref()
            .and_then(|value| value.get("ROSTER_CRITIC_API_KEY")),
        Some(&Value::string("test-only-private-api-key"))
    );
    assert!(
        credentials
            .environment
            .as_ref()
            .and_then(|value| value.get("OPENAI_API_KEY"))
            .is_none()
    );
    assert!(credentials.vault_entries.is_none());
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let aliases = Value::object(vec![
            (
                "roster_critic_api_key",
                Value::string("test-only-private-api-key"),
            ),
            ("roster_start_sync", Value::string("offline")),
            ("roster_retention_opt_out", Value::string("true")),
        ]);
        let selected = config::load(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &fixture.root,
            &aliases,
        )?;
        assert_eq!(
            selected.retention.get("optOut"),
            Some(&Value::Boolean(true))
        );
        assert_eq!(
            start::Options::resolve(&selected.config, None, None)?.sync,
            start::Sync::Offline
        );
        assert!(
            delivery::invocation_credentials(&aliases, &selected)?
                .environment
                .as_ref()
                .and_then(|value| value.get("ROSTER_CRITIC_API_KEY"))
                .is_some()
        );
        let minimal = github_agent_roster::host::minimal_environment(&Value::object(vec![(
            "Path",
            Value::string("owned-host-path"),
        )]))?;
        assert_eq!(
            minimal.get("PATH").map(String::as_str),
            Some("owned-host-path")
        );
        let relative = github_agent_roster::host::contracts_override(&Value::object(vec![(
            "GITHUB_AGENT_CONTRACTS",
            Value::string("relative-contracts"),
        )]))?;
        assert_eq!(relative.as_deref(), Some(Path::new("relative-contracts")));
        let mut command = fixture.command(
            &["run", "--seat", "coder", "--runtime", "builtin"],
            &fixture.environment()?,
            &fixture.root,
        )?;
        command.env("ROSTER_GIT", std::ffi::OsString::from_wide(&[0xd800]));
        let output = command.output().map_err(io)?;
        assert!(!output.status.success() && output.stdout.is_empty());
        assert!(text(&output.stderr)?.contains("environment values must be Unicode"));
        assert!(!fixture.root.join(".roster").join("locks").exists());
    }
    let error = delivery::safe_invocation_error(
        &Error::new(
            "FixtureError",
            "configured-route-not-a-response configured-critic-route http://127.0.0.1:1/v1 http://127.0.0.1:2/v1 test-only-private-api-key",
        ),
        &loaded,
        &credentials,
    )?;
    for label in [
        "configured-route",
        "configured-critic-route",
        "127.0.0.1",
        "test-only-private-api-key",
    ] {
        assert!(
            !error.message.contains(label),
            "Declared route/sentinel leaked: {label}"
        );
    }
    fixture.write(".roster/fleet.yml", "human fleet replacement\n")?;
    assert_eq!(
        loaded.require_current().err().and_then(|error| error.code),
        Some("ROSTER_STALE_CONFIG")
    );
    Ok(())
}
