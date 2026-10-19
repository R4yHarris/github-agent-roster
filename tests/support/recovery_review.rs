use super::*;

async fn injected_checkpoint(
    fixture: &Fixture,
    green: &Value,
    path: &str,
    contents: &str,
) -> Result<(), Error> {
    let payload = fixture.container.join("review-payload");
    fs::write(&payload, contents).map_err(io)?;
    let blob = fixture
        .git(&[
            "hash-object",
            "-w",
            "--no-filters",
            "--",
            payload
                .to_str()
                .ok_or_else(|| Error::new("Error", "Review payload path is not Unicode"))?,
        ])?
        .trim()
        .to_owned();
    let old_tree = green
        .get("tree")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::new("Error", "Green tree missing"))?
        .to_utf8()?;
    let index = fixture.container.join("review-index");
    let mut tree = String::new();
    for args in [
        vec!["read-tree".to_owned(), old_tree],
        vec![
            "update-index".to_owned(),
            "--add".to_owned(),
            "--cacheinfo".to_owned(),
            format!("100644,{blob},{path}"),
        ],
        vec!["write-tree".to_owned()],
    ] {
        let mut command = tokio::process::Command::new("git");
        command
            .current_dir(&fixture.root)
            .env_clear()
            .envs(tools::Host::default().environment)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_CONFIG_GLOBAL", "NUL")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["--no-pager", "-c", "core.fsmonitor=false", "-c"])
            .arg(format!(
                "core.hooksPath={}",
                fixture.root.join("disabled-hooks").display()
            ))
            .args(args);
        let output = github_agent_roster::execution::owned::execute(
            command,
            github_agent_roster::process::Limits {
                timeout: Some(std::time::Duration::from_secs(30)),
                max_output_bytes: 65_536,
            },
            None,
        )
        .await
        .map_err(|failure| {
            Error::new(
                "Error",
                format!("Consistent review tree construction failed: {failure}"),
            )
        })?;
        tree = String::from_utf8(output.captured.stdout)
            .map_err(|error| Error::new("Error", error.to_string()))?
            .trim()
            .to_owned();
    }
    let mut record = green.clone();
    let Value::Object(fields) = &mut record else {
        return Err(Error::new("Error", "Green metadata is not an object"));
    };
    let field = fields
        .iter_mut()
        .find(|(name, _)| *name == Text::from("tree"))
        .ok_or_else(|| Error::new("Error", "Green tree field missing"))?;
    field.1 = Value::string(&tree);
    let number = green
        .get("number")
        .and_then(Value::as_number)
        .ok_or_else(|| Error::new("Error", "Green number missing"))? as u64;
    let metadata = fixture
        .root
        .join(".roster")
        .join("checkpoints")
        .join("42")
        .join(number.to_string());
    fs::write(metadata, format!("{}\n", record.json())).map_err(io)?;
    let reference = green
        .get("ref")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::new("Error", "Green ref missing"))?
        .to_utf8()?;
    fixture.git(&["update-ref", &reference, &tree])?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn repro_private_checkpoint_target_cannot_gain_coder_write_authority() -> Result<(), Error> {
    let fixture = Fixture::new(
        &["**/*"],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let green = recovery_green(&mut boundary).await?;
        recovery_node_value(&mut boundary, 2, 3).await?;
        let target = fixture.root.join(".roster").join("config.yml");
        assert!(!target.exists());
        injected_checkpoint(&fixture, &green, ".roster/config.yml", "owned private injection\n").await?;
        let count = recovery_checkpoints::list(&boundary)?.len();
        match recovery_restore::prepare(&boundary, green.get("number").and_then(Value::as_number).map(|number| number as u64)).await {
            Err(error) => assert_eq!(error.code, Some("ROSTER_TOOL_DENIED")),
            Ok(plan) => {
                let approved = plan.confirm(&boundary, recovery_restore::Publication::Unpublished, true).await?;
                let report = recovery_restore::execute(&mut boundary, approved, &mut CurrentUnpublished).await?;
                assert!(!target.exists(), "Native restore created private configuration outside ordinary coder authority: {}", report.as_value().json());
                return Err(Error::new("Error", "Restore preparation accepted a private product target"));
            }
        }
        assert_eq!(recovery_checkpoints::list(&boundary)?.len(), count);
        assert!(!target.exists());
        assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
        assert!(fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?.contains("return 3;"));
        Ok(())
    }.await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn repro_late_missing_artifacts_must_not_survive_restore_as_current_evidence()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let green = recovery_green(&mut boundary).await?;
        recovery_node_value(&mut boundary, 2, 3).await?;
        assert!(!fixture.root.join("RESULT.md").exists());
        assert!(!fixture.root.join("REVIEW.md").exists());
        let prepared = recovery_restore::prepare(
            &boundary,
            green
                .get("number")
                .and_then(Value::as_number)
                .map(|number| number as u64),
        )
        .await?;
        let approved = prepared
            .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
            .await?;
        let count = recovery_checkpoints::list(&boundary)?.len();
        fixture.write("RESULT.md", "New human RESULT before execution\n")?;
        fixture.write("REVIEW.md", "New human REVIEW before execution\n")?;
        let report =
            recovery_restore::execute(&mut boundary, approved, &mut CurrentUnpublished).await;
        assert!(
            report.is_err(),
            "Native restore committed while late human evidence remained active"
        );
        assert_eq!(recovery_checkpoints::list(&boundary)?.len(), count);
        assert_eq!(
            fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
            "New human RESULT before execution\n"
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("REVIEW.md")).map_err(io)?,
            "New human REVIEW before execution\n"
        );
        assert!(
            fs::read_to_string(fixture.root.join("src").join("app.mjs"))
                .map_err(io)?
                .contains("return 3;")
        );
        assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

fn green_number(green: &Value) -> Result<u64, Error> {
    green
        .get("number")
        .and_then(Value::as_number)
        .map(|number| number as u64)
        .ok_or_else(|| Error::new("Error", "Native green checkpoint number missing"))
}

fn assert_current_three(fixture: &Fixture) -> Result<(), Error> {
    assert_eq!(
        fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
        "export function answer() { return 3; }\n"
    );
    assert!(
        fs::read_to_string(fixture.root.join("tests").join("app.test.mjs"))
            .map_err(io)?
            .contains("answer(), 3")
    );
    Ok(())
}

fn assert_transaction(fixture: &Fixture, reconciled: bool) -> Result<(), Error> {
    let entries =
        fs::read_dir(fixture.root.join(".roster").join("checkpoints").join("42")).map_err(io)?;
    let mut found = 0;
    for entry in entries {
        let path = entry.map_err(io)?.path();
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("restore-"))
        {
            found += 1;
            assert!(path.join("ROLLED-BACK").is_file());
            assert_eq!(path.join("RECONCILED").is_file(), reconciled);
            assert!(!path.join("COMMITTED").exists());
        }
    }
    assert_eq!(found, 1);
    Ok(())
}

struct EvidenceArrival {
    root: PathBuf,
    calls: usize,
    at: usize,
    names: Vec<String>,
    ready: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl recovery_restore::PublicationCheck for EvidenceArrival {
    async fn current(&mut self, subject: &Value) -> Result<recovery_restore::Publication, Error> {
        assert!(
            subject
                .get("checkpoint")
                .and_then(|record| record.get("validation"))
                .is_some()
        );
        self.calls += 1;
        if self.calls == self.at {
            for name in &self.names {
                fs::write(
                    self.root.join(name),
                    format!("New human {name} at publication barrier {}\n", self.at),
                )
                .map_err(io)?;
            }
            if let Some(ready) = &self.ready {
                ready.store(true, Ordering::Release);
            }
            tokio::task::yield_now().await;
        }
        Ok(recovery_restore::Publication::Unpublished)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn restore_and_coder_share_complete_private_protected_and_case_alias_write_policy()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["**/*"],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let green = recovery_green(&mut boundary).await?;
        recovery_node_value(&mut boundary, 2, 3).await?;
        fixture.write(
            "RESULT.md",
            "Unchanged human result during rejected preparations\n",
        )?;
        let task = fs::read(fixture.root.join("TASK.md")).map_err(io)?;
        let recipe = fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?;
        for file in [
            ".roster/config.yml",
            ".worktrees/other-task/file",
            ".roster-state/config.yml",
            "src/.ROSTER/private.yml",
            "src/.WORKTREES/other-task/file",
            "node_modules/owned/entry.mjs",
            "src/vendor/owned.mjs",
            ".github/WORKFLOWS/guard.yml",
            "agent-policy.yml",
        ] {
            for present in [false, true] {
                let target = fixture.root.join(file);
                let previous = format!("Unchanged human private bytes at {file}\n");
                if present {
                    fixture.write(file, &previous)?;
                } else {
                    assert!(!target.exists());
                }
                injected_checkpoint(
                    &fixture,
                    &green,
                    file,
                    "Candidate product cannot replace private bytes\n",
                )
                .await?;
                let count = recovery_checkpoints::list(&boundary)?.len();
                let root_identity =
                    github_agent_roster::paths::file_information(&fixture.root)?.identity;
                match boundary
                    .execute("write_file", &write(file, "Denied ordinary coder write\n"))
                    .await
                {
                    Err(error) => assert_eq!(error.code, Some("ROSTER_TOOL_DENIED"), "{file}"),
                    Ok(value) => {
                        return Err(Error::new(
                            "Error",
                            format!(
                                "Ordinary coder gained private write authority: {}",
                                value.json()
                            ),
                        ));
                    }
                }
                match recovery_restore::prepare(&boundary, Some(green_number(&green)?)).await {
                    Err(error) => assert_eq!(error.code, Some("ROSTER_TOOL_DENIED"), "{file}"),
                    Ok(_) => {
                        return Err(Error::new(
                            "Error",
                            format!("Restore widened ordinary coder write policy for {file}"),
                        ));
                    }
                }
                assert_eq!(
                    root_identity,
                    github_agent_roster::paths::file_information(&fixture.root)?.identity
                );
                assert_eq!(recovery_checkpoints::list(&boundary)?.len(), count);
                if present {
                    assert_eq!(fs::read_to_string(&target).map_err(io)?, previous);
                } else {
                    assert!(!target.exists());
                }
                assert_eq!(
                    fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                    "Unchanged human result during rejected preparations\n"
                );
                assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
                assert_current_three(&fixture)?;
            }
        }
        assert_eq!(fs::read(fixture.root.join("TASK.md")).map_err(io)?, task);
        assert_eq!(
            fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?,
            recipe
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn every_nonpreserved_artifact_absence_is_sealed_for_confirm_and_direct_archive()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let green = recovery_green(&mut boundary).await?;
        recovery_node_value(&mut boundary, 2, 3).await?;
        for name in ["CONTEXT.md", "RESEARCH.md", "RESULT.md", "REVIEW.md"] {
            let prepared =
                recovery_restore::prepare(&boundary, Some(green_number(&green)?)).await?;
            let count = recovery_checkpoints::list(&boundary)?.len();
            let human = format!("Human {name} arriving before confirm\n");
            fixture.write(name, &human)?;
            assert!(
                prepared
                    .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
                    .await
                    .is_err()
            );
            assert_eq!(recovery_checkpoints::list(&boundary)?.len(), count);
            assert_eq!(
                fs::read_to_string(fixture.root.join(name)).map_err(io)?,
                human
            );
            assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
            fs::remove_file(fixture.root.join(name)).map_err(io)?;
            let prepared =
                recovery_archive::prepare(&boundary, &recovery_archive::PRESERVE_AUTHORITY).await?;
            let absent = prepared.as_value().get("absent").cloned().ok_or_else(|| {
                Error::new("Error", "Archive preview omitted its sealed absences")
            })?;
            let Value::Array(absent) = absent else {
                return Err(Error::new("Error", "Archive absences are not an array"));
            };
            assert_eq!(absent.len(), 4);
            assert!(absent.contains(&Value::string(name)));
            fixture.write(name, &human)?;
            assert!(
                recovery_archive::execute(prepared, &boundary)
                    .await
                    .is_err()
            );
            assert_eq!(
                fs::read_to_string(fixture.root.join(name)).map_err(io)?,
                human
            );
            assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
            assert_current_three(&fixture)?;
            fs::remove_file(fixture.root.join(name)).map_err(io)?;
        }
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn missing_artifacts_arriving_after_archive_or_before_commit_are_preserved_and_roll_back()
-> Result<(), Error> {
    for phase in [2, 3] {
        for name in ["CONTEXT.md", "RESEARCH.md", "RESULT.md", "REVIEW.md"] {
            let fixture = Fixture::new(
                &["src/app.mjs", "tests/app.test.mjs"],
                "characterization",
                &["Current behavior is verified."],
                true,
            )?;
            fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
            let source = CancellationSource::default();
            let mut boundary = fixture.open(&source, false).await?;
            let outcome = async {
                let green = recovery_green(&mut boundary).await?;
                recovery_node_value(&mut boundary, 2, 3).await?;
                let task = fs::read(fixture.root.join("TASK.md")).map_err(io)?;
                let recipe = fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?;
                let approved = recovery_restore::prepare(&boundary, Some(green_number(&green)?))
                    .await?
                    .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
                    .await?;
                let mut arrival = EvidenceArrival {
                    root: fixture.root.clone(),
                    calls: 0,
                    at: phase,
                    names: vec![name.to_owned()],
                    ready: None,
                };
                assert!(
                    recovery_restore::execute(&mut boundary, approved, &mut arrival)
                        .await
                        .is_err()
                );
                assert_eq!(arrival.calls, phase);
                assert_eq!(
                    fs::read_to_string(fixture.root.join(name)).map_err(io)?,
                    format!("New human {name} at publication barrier {phase}\n")
                );
                assert_current_three(&fixture)?;
                assert_transaction(&fixture, true)?;
                assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
                assert_eq!(fs::read(fixture.root.join("TASK.md")).map_err(io)?, task);
                assert_eq!(
                    fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?,
                    recipe
                );
                if phase == 3 {
                    assert!(boundary.last_tests().is_none());
                }
                Ok(())
            }
            .await;
            locks::finish(outcome, boundary.close().await)?;
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn replacements_of_archived_artifacts_preserve_both_human_versions_and_quarantine()
-> Result<(), Error> {
    for phase in [2, 3] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let green = recovery_green(&mut boundary).await?;
            recovery_node_value(&mut boundary, 2, 3).await?;
            fixture.write("RESULT.md", "Older approved human result to archive\n")?;
            let approved = recovery_restore::prepare(&boundary, Some(green_number(&green)?))
                .await?
                .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
                .await?;
            let mut arrival = EvidenceArrival {
                root: fixture.root.clone(),
                calls: 0,
                at: phase,
                names: vec!["RESULT.md".to_owned()],
                ready: None,
            };
            let failure =
                match recovery_restore::execute(&mut boundary, approved, &mut arrival).await {
                    Err(error) => error,
                    Ok(_) => {
                        return Err(Error::new(
                            "Error",
                            "A replacement human result bypassed the post-archive absence seal",
                        ));
                    }
                };
            assert_eq!(failure.code, Some("ROSTER_RESTORE_CLEANUP"));
            assert_eq!(arrival.calls, phase);
            assert_eq!(
                fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                format!("New human RESULT.md at publication barrier {phase}\n")
            );
            assert_current_three(&fixture)?;
            assert_transaction(&fixture, false)?;
            let task_archive = fixture
                .root
                .join(".git")
                .join("roster-artifacts")
                .join("issue-42");
            let archives = fs::read_dir(task_archive)
                .map_err(io)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(io)?;
            assert_eq!(archives.len(), 1);
            let archived = archives
                .first()
                .ok_or_else(|| Error::new("Error", "Owned artifact archive disappeared"))?
                .path()
                .join("RESULT.md");
            assert_eq!(
                fs::read_to_string(archived).map_err(io)?,
                "Older approved human result to archive\n"
            );
            assert!(boundary.close().await.is_err());
            assert!(
                fixture
                    .root
                    .join(".roster")
                    .join("locks")
                    .join("run.lock")
                    .is_file()
            );
            Ok(())
        }
        .await;
        outcome?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rollback_for_late_evidence_revokes_calls_context_and_tests_but_keeps_actual_failed_full_history()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    fixture.write("tests/outside.test.mjs", "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { answer } from '../src/app.mjs';\ntest('outside retained limit', () => assert.ok(answer() <= 2));\n")?;
    fixture.git(&["add", "tests/outside.test.mjs"])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned recovery evidence failure limit",
    ])?;
    fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let green = recovery_green(&mut boundary).await?;
        recovery_node_value(&mut boundary, 2, 3).await?;
        assert_eq!(
            boundary.verify(true).await?.get("exit_code"),
            Some(&Value::Number(1.0))
        );
        let mut old = start(&boundary, true, false).await?;
        let response = old.response(
            &boundary,
            &completion(None, Some("read_file"), path("src/app.mjs")),
        )?;
        let pending = response
            .calls
            .first()
            .ok_or_else(|| Error::new("Error", "Pending call missing before restore rollback"))?
            .clone();
        let approved = recovery_restore::prepare(&boundary, Some(green_number(&green)?))
            .await?
            .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
            .await?;
        let mut arrival = EvidenceArrival {
            root: fixture.root.clone(),
            calls: 0,
            at: 3,
            names: vec!["REVIEW.md".to_owned()],
            ready: None,
        };
        assert!(
            old.restore_checkpoint(&mut boundary, approved, &mut arrival)
                .await
                .is_err()
        );
        assert_current_three(&fixture)?;
        assert_transaction(&fixture, true)?;
        assert!(boundary.last_tests().is_none());
        assert!(
            old.execute(&mut boundary, &pending, &evidence::Projection::default())
                .await
                .is_err()
        );
        assert!(
            old.workspace()
                .export(&boundary, "verified", &evidence::Projection::default())
                .is_err()
        );
        let mut current = start(&boundary, true, false).await?;
        assert_eq!(
            call(
                &mut current,
                &mut boundary,
                "run_test",
                Value::object(Vec::new())
            )
            .await?
            .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        let finalized = review68_finish(current, &mut boundary).await?;
        assert_eq!(finalized.status(), finalization::Status::Fail);
        let history = finalized.result().get("fullVerification").ok_or_else(|| {
            Error::new(
                "Error",
                "Late artifact rollback erased the actual failed-full obligation",
            )
        })?;
        assert_eq!(history.get("original_exit"), Some(&Value::Number(1.0)));
        assert_eq!(
            history.get("resolution"),
            Some(&Value::string("unresolved"))
        );
        assert!(
            finalized
                .body()
                .to_utf8()?
                .contains("Original full-suite exit: 1")
        );
        recovery_node_value(&mut boundary, 3, 2).await?;
        let mut recovered = start(&boundary, true, false).await?;
        assert_eq!(
            recovered
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?
                .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        assert_eq!(
            review68_finish(recovered, &mut boundary).await?.status(),
            finalization::Status::Pass
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("REVIEW.md")).map_err(io)?,
            "New human REVIEW.md at publication barrier 3\n"
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn real_product_loop_yield_rechecks_absence_before_later_product_mutation()
-> Result<(), Error> {
    for cancel in [false, true] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let green = recovery_green(&mut boundary).await?;
            recovery_node_value(&mut boundary, 2, 3).await?;
            assert_eq!(
                boundary.verify(true).await?.get("exit_code"),
                Some(&Value::Number(0.0))
            );
            let approved = recovery_restore::prepare(&boundary, Some(green_number(&green)?))
                .await?
                .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
                .await?;
            let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut publication = EvidenceArrival {
                root: fixture.root.clone(),
                calls: 0,
                at: 2,
                names: Vec::new(),
                ready: Some(ready.clone()),
            };
            let arrive = async {
                let waiting = std::time::Instant::now();
                let mut observing = None;
                loop {
                    if ready.load(Ordering::Acquire) {
                        let start = observing.get_or_insert_with(std::time::Instant::now);
                        if fs::read_to_string(fixture.root.join("src").join("app.mjs"))
                            .map_err(io)?.contains("return 2;") {
                            assert!(
                                fs::read_to_string(fixture.root.join("tests").join("app.test.mjs"))
                                .map_err(io)?
                                .contains("answer(), 3")
                            );
                            fixture.write(
                                "RESULT.md",
                                "Human evidence arriving at the actual product-loop yield\n",
                            )?;
                            if cancel {
                                source.cancel(github_agent_roster::cancel::Reason::Cancelled)?;
                            }
                            return Ok::<(), Error>(());
                        }
                        if start.elapsed() > std::time::Duration::from_secs(30) {
                            return Err(Error::new("Error", "Actual restore product mutation was not observed after mutation readiness"));
                        }
                    } else if waiting.elapsed() > std::time::Duration::from_secs(180) {
                        return Err(Error::new("Error", "Restore never reached the explicitly observed mutation-ready publication barrier"));
                    }
                    tokio::task::yield_now().await;
                }
            };
            let (arrival, restored) = tokio::join!(
                biased;
                arrive,
                recovery_restore::execute(&mut boundary, approved, &mut publication)
            );
            arrival.map_err(|error| Error::new("Error", format!("Native restore observation failed at publication phase {} with actual restore error {:?}", publication.calls, restored.as_ref().err())).caused_by(error))?;
            assert!(restored.is_err());
            assert_eq!(publication.calls, 2);
            assert_current_three(&fixture)?;
            assert_transaction(&fixture, true)?;
            assert!(boundary.last_tests().is_none());
            assert_eq!(
                fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                "Human evidence arriving at the actual product-loop yield\n"
            );
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn broad_scope_legitimate_products_restore_with_no_archive_and_preserved_authority_untouched()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["**/*"],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    fixture.write("PLAN.md", "Unchanged human plan\n")?;
    fixture.write("ESTIMATE.md", "Unchanged human estimate\n")?;
    fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        boundary
            .execute(
                "write_file",
                &write("src/allowed.txt", "Approved green product bytes\n"),
            )
            .await?;
        let green = recovery_green(&mut boundary).await?;
        recovery_node_value(&mut boundary, 2, 3).await?;
        fs::remove_file(fixture.root.join("src").join("allowed.txt")).map_err(io)?;
        let mut authority = BTreeMap::new();
        for name in recovery_archive::PRESERVE_AUTHORITY {
            let path = fixture.root.join(name);
            authority.insert(
                name,
                (
                    github_agent_roster::paths::file_information(&path)?.identity,
                    fs::read(path).map_err(io)?,
                ),
            );
        }
        let plan = recovery_restore::prepare(&boundary, Some(green_number(&green)?)).await?;
        let approved = plan
            .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
            .await?;
        let restored =
            recovery_restore::execute(&mut boundary, approved, &mut CurrentUnpublished).await?;
        assert_eq!(restored.as_value().get("archive"), Some(&Value::Null));
        assert_eq!(
            restored.as_value().get("status"),
            Some(&Value::string("restored-unverified"))
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("src").join("allowed.txt")).map_err(io)?,
            "Approved green product bytes\n"
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("src").join("app.mjs")).map_err(io)?,
            "export function answer() { return 2; }\n"
        );
        assert!(boundary.last_tests().is_none());
        for (name, (identity, bytes)) in authority {
            let path = fixture.root.join(name);
            assert_eq!(
                github_agent_roster::paths::file_information(&path)?.identity,
                identity
            );
            assert_eq!(fs::read(path).map_err(io)?, bytes);
        }
        let mut refreshed = start(&boundary, true, false).await?;
        assert_eq!(
            refreshed
                .verify_full(&mut boundary, &evidence::Projection::default())
                .await?
                .get("exit_code"),
            Some(&Value::Number(0.0))
        );
        let refreshed = recovery_checkpoints::capture(&boundary, true).await?;
        assert_eq!(refreshed.get("status"), Some(&Value::string("verified")));
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn archive_absence_rejects_late_directory_and_hardlink_without_archiving_or_deleting_them()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let plan =
            recovery_archive::prepare(&boundary, &recovery_archive::PRESERVE_AUTHORITY).await?;
        fs::create_dir(fixture.root.join("RESULT.md")).map_err(io)?;
        assert!(recovery_archive::execute(plan, &boundary).await.is_err());
        assert!(fixture.root.join("RESULT.md").is_dir());
        assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
        fs::remove_dir(fixture.root.join("RESULT.md")).map_err(io)?;
        let plan =
            recovery_archive::prepare(&boundary, &recovery_archive::PRESERVE_AUTHORITY).await?;
        let source = fixture.container.join("human-evidence");
        fs::write(&source, "Unapproved linked human evidence\n").map_err(io)?;
        fs::hard_link(&source, fixture.root.join("REVIEW.md")).map_err(io)?;
        assert!(recovery_archive::execute(plan, &boundary).await.is_err());
        assert_eq!(
            github_agent_roster::paths::file_information(&source)?.links,
            2
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("REVIEW.md")).map_err(io)?,
            "Unapproved linked human evidence\n"
        );
        assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn missing_preserved_authority_artifacts_can_arrive_without_becoming_archive_targets()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["src/app.mjs", "tests/app.test.mjs"],
        "characterization",
        &["Current behavior is verified."],
        true,
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source, false).await?;
    let outcome = async {
        let plan =
            recovery_archive::prepare(&boundary, &recovery_archive::PRESERVE_AUTHORITY).await?;
        let task = fs::read(fixture.root.join("TASK.md")).map_err(io)?;
        let recipe = fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?;
        let task_identity =
            github_agent_roster::paths::file_information(&fixture.root.join("TASK.md"))?.identity;
        let recipe_identity =
            github_agent_roster::paths::file_information(&fixture.root.join("RECIPE.yml"))?
                .identity;
        fixture.write(
            "PLAN.md",
            "Human authority plan arriving after archive preparation\n",
        )?;
        fixture.write(
            "ESTIMATE.md",
            "Human authority estimate arriving after archive preparation\n",
        )?;
        assert!(recovery_archive::execute(plan, &boundary).await?.is_none());
        assert!(!fixture.root.join(".git").join("roster-artifacts").exists());
        assert_eq!(fs::read(fixture.root.join("TASK.md")).map_err(io)?, task);
        assert_eq!(
            fs::read(fixture.root.join("RECIPE.yml")).map_err(io)?,
            recipe
        );
        assert_eq!(
            github_agent_roster::paths::file_information(&fixture.root.join("TASK.md"))?.identity,
            task_identity
        );
        assert_eq!(
            github_agent_roster::paths::file_information(&fixture.root.join("RECIPE.yml"))?
                .identity,
            recipe_identity
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("PLAN.md")).map_err(io)?,
            "Human authority plan arriving after archive preparation\n"
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("ESTIMATE.md")).map_err(io)?,
            "Human authority estimate arriving after archive preparation\n"
        );
        Ok(())
    }
    .await;
    locks::finish(outcome, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn late_missing_review_rolls_back_older_owned_artifacts_without_quarantining_the_human_addition()
-> Result<(), Error> {
    for phase in [2, 3] {
        let fixture = Fixture::new(
            &["src/app.mjs", "tests/app.test.mjs"],
            "characterization",
            &["Current behavior is verified."],
            true,
        )?;
        fixture.git(&["switch", "--quiet", "-c", "agents/42"])?;
        let source = CancellationSource::default();
        let mut boundary = fixture.open(&source, false).await?;
        let outcome = async {
            let green = recovery_green(&mut boundary).await?;
            recovery_node_value(&mut boundary, 2, 3).await?;
            fixture.write(
                "RESULT.md",
                "Older approved result preserved by owned rollback\n",
            )?;
            fixture.write(
                "CONTEXT.md",
                "Older approved context preserved by owned rollback\n",
            )?;
            let approved = recovery_restore::prepare(&boundary, Some(green_number(&green)?))
                .await?
                .confirm(&boundary, recovery_restore::Publication::Unpublished, true)
                .await?;
            let mut arrival = EvidenceArrival {
                root: fixture.root.clone(),
                calls: 0,
                at: phase,
                names: vec!["REVIEW.md".to_owned()],
                ready: None,
            };
            assert!(
                recovery_restore::execute(&mut boundary, approved, &mut arrival)
                    .await
                    .is_err()
            );
            assert_eq!(arrival.calls, phase);
            assert_current_three(&fixture)?;
            assert_transaction(&fixture, true)?;
            assert_eq!(
                fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?,
                "Older approved result preserved by owned rollback\n"
            );
            assert_eq!(
                fs::read_to_string(fixture.root.join("CONTEXT.md")).map_err(io)?,
                "Older approved context preserved by owned rollback\n"
            );
            assert_eq!(
                fs::read_to_string(fixture.root.join("REVIEW.md")).map_err(io)?,
                format!("New human REVIEW.md at publication barrier {phase}\n")
            );
            let archives = fs::read_dir(
                fixture
                    .root
                    .join(".git")
                    .join("roster-artifacts")
                    .join("issue-42"),
            )
            .map_err(io)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io)?;
            assert!(archives.is_empty());
            if phase == 3 {
                assert!(boundary.last_tests().is_none());
            }
            Ok(())
        }
        .await;
        locks::finish(outcome, boundary.close().await)?;
    }
    Ok(())
}
