use super::*;
use crate::cancel::CancellationSource;
use crate::process::Limits;
use crate::state::{locks, redaction};
use tokio::process::Command;

struct Fixture {
    root: PathBuf,
    host: tools::Host,
    executable: PathBuf,
}

fn io(error: std::io::Error) -> Error {
    Error::new("Error", error.to_string())
}

impl Fixture {
    async fn new() -> Result<Self, Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".docker-native-admission-{}-{}",
            std::process::id(),
            files::nonce()?
        ));
        files::create_private_directories(&root)?;
        let host = tools::Host::default();
        let project = root.join("project");
        files::create_private_directories(&project)?;
        archive::write_owned(&project.join(".gitignore"), b".roster/\n")?;
        let init = host.command(
            &project,
            &host.git,
            &["init".to_owned(), "--quiet".to_owned()],
            &BTreeMap::new(),
        );
        super::super::owned::execute(
            init,
            Limits {
                timeout: Some(Duration::from_secs(30)),
                max_output_bytes: 65_536,
            },
            None,
        )
        .await
        .map_err(|error| {
            Error::new(
                "Error",
                format!("Initialize native admission fixture: {error}"),
            )
        })?;
        let package = root.join("program");
        files::create_private_directories(&package.join("src"))?;
        archive::write_owned(&package.join("Cargo.toml"), b"[package]\nname = \"owned_docker_admission\"\nversion = \"0.0.0\"\nedition = \"2021\"\nrust-version = \"1.85\"\n")?;
        archive::write_owned(
            &package.join("Cargo.lock"),
            b"version = 4\n\n[[package]]\nname = \"owned_docker_admission\"\nversion = \"0.0.0\"\n",
        )?;
        archive::write_owned(
            &package.join("src").join("main.rs"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "\\tests\\fixtures\\docker_cli.rs"
            )),
        )?;
        let cargo = std::env::var_os("CARGO").ok_or_else(|| {
            Error::new(
                "Error",
                "Native admission fixture requires the selected Cargo executable",
            )
        })?;
        let toolchain = std::env::var_os("ROSTER_TEST_CARGO_TOOLCHAIN").ok_or_else(|| {
            Error::new(
                "Error",
                "Native admission fixture requires its explicitly selected installed compiler",
            )
        })?;
        let mut environment = host.environment.clone();
        for name in ["CARGO_HOME", "RUSTUP_HOME", "RUSTFLAGS"] {
            if let Some(value) = std::env::var_os(name) {
                environment.insert(
                    name.to_owned(),
                    value.into_string().map_err(|_| {
                        Error::new("Error", "Fixture compiler environment must be Unicode")
                    })?,
                );
            }
        }
        let mut command = Command::new(cargo);
        command
            .current_dir(&package)
            .env_clear()
            .envs(&environment)
            .env("RUSTUP_TOOLCHAIN", toolchain)
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_TARGET_DIR", package.join("target"))
            .args(["build", "--locked", "--offline"]);
        super::super::owned::execute(
            command,
            Limits {
                timeout: Some(Duration::from_secs(90)),
                max_output_bytes: 1024 * 1024,
            },
            None,
        )
        .await
        .map_err(|error| {
            Error::new(
                "Error",
                format!(
                    "Build native admission fixture: {error}: {}",
                    String::from_utf8_lossy(&error.captured.stderr)
                ),
            )
        })?;
        let executable = package
            .join("target")
            .join("debug")
            .join("owned_docker_admission.exe");
        let state = executable
            .parent()
            .ok_or_else(|| Error::new("Error", "Native admission executable needs a parent"))?
            .join("docker-state");
        files::create_private_directories(&state)?;
        archive::write_owned(&state.join("mode"), b"pass")?;
        Ok(Self {
            root,
            host,
            executable,
        })
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }
    fn state(&self) -> Result<PathBuf, Error> {
        Ok(self
            .executable
            .parent()
            .ok_or_else(|| Error::new("Error", "Native fixture executable has no parent"))?
            .join("docker-state"))
    }
    fn approval(&self) -> Approval {
        Approval {
            enabled: true,
            executable: self.executable.clone(),
            endpoint: Endpoint::WindowsPipe,
            image: format!("sha256:{}", "a".repeat(64)),
            runtime: Runtime::Native {
                entrypoint: "/usr/local/bin/owned-verify".to_owned(),
            },
            files: vec!["src/app.mjs".to_owned()],
            dependencies: Vec::new(),
            verifiers: Vec::new(),
            observations: vec![Observation {
                check_id: 1,
                action: Action::Native(vec!["echo".to_owned()]),
                expected_exit: 0,
                expected_stdout: "verified\n".to_owned(),
                expected_stderr: String::new(),
                evidence: "Owned fixture observation".to_owned(),
            }],
            timeout_ms: 5_000,
        }
    }
    fn controller<'a>(
        &'a self,
        lease: &'a backend::RootLease,
        approval: &'a Approval,
        started: Instant,
        submissions: &'a std::cell::RefCell<Vec<Value>>,
    ) -> Result<Controller<'a>, Error> {
        let product = self.root.join("product");
        files::create_private_directories(&product)?;
        Ok(Controller {
            owner: backend::Owner::PrePlan(lease),
            approval,
            root: &self.root,
            prefix: vec![
                "--config".to_owned(),
                self.root.join("config").to_string_lossy().into_owned(),
                "--host".to_owned(),
                approval.endpoint()?,
            ],
            host_environment: BTreeMap::new(),
            expected_environment: overrides(),
            product,
            verifier: None,
            subject: "c".repeat(64),
            plan: "d".repeat(64),
            started,
            executable: Executable::capture(&approval.executable)?,
            submissions,
        })
    }
    fn close(&self) -> Result<(), Error> {
        fs::remove_dir_all(&self.root).map_err(io)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.exists() {
            eprintln!(
                "Retained native Docker admission fixture: {}",
                self.root.display()
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn expired_before_native_create_submission_has_no_daemon_effect_or_cleanup_obligation()
-> Result<(), Error> {
    let fixture = Fixture::new().await?;
    let source = CancellationSource::default();
    let mut lease = backend::RootLease::open(
        &fixture.project(),
        "docker-before-spawn",
        fixture.host.clone(),
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let outcome = async {
        let approval = fixture.approval();
        approval.validate()?;
        let submissions = std::cell::RefCell::new(Vec::new());
        let controller = fixture.controller(
            &lease,
            &approval,
            Instant::now()
                .checked_sub(Duration::from_millis(5_001))
                .ok_or_else(|| {
                    Error::new(
                        "Error",
                        "Native fixture clock does not support a five-second deadline",
                    )
                })?,
            &submissions,
        )?;
        let error = match controller.execute(&["echo".to_owned()], 1).await {
            Err(error) => error,
            Ok(_) => {
                return Err(Error::new(
                    "Error",
                    "Expired native create returned an observation",
                ));
            }
        };
        assert_eq!(error.code, Some("ROSTER_DOCKER_UNAVAILABLE"), "{error:?}");
        assert_eq!(error.message, "Docker verification deadline expired");
        let submissions = submissions.borrow();
        let submission = submissions
            .first()
            .ok_or_else(|| Error::new("Error", "Missing native create submission receipt"))?;
        assert_eq!(
            submission.get("state"),
            Some(&Value::string("not-attempted"))
        );
        assert_eq!(submission.get("native_pid"), Some(&Value::Null));
        assert!(
            !fixture.state()?.join("calls").exists(),
            "A native Docker child ran despite expiry before submission"
        );
        assert!(
            lease.check().is_ok(),
            "A proved non-submission must not invent uncertain ownership"
        );
        Ok(())
    }
    .await;
    let result = locks::finish(outcome, lease.close().await);
    if result.is_ok() {
        fixture.close()?;
    }
    result
}

#[tokio::test(flavor = "current_thread")]
async fn native_create_started_without_cid_stays_ambiguous_unless_owned_metadata_proves_cleanup()
-> Result<(), Error> {
    use winsafe::{HPROCESS, co, prelude::*};
    for mode in ["create-before-object", "create-after-object"] {
        let fixture = Fixture::new().await?;
        fs::write(fixture.state()?.join("mode"), mode).map_err(io)?;
        let foreign = fixture.state()?.join(format!("{}.args", "f".repeat(64)));
        let foreign_bytes = b"container\ncreate\n--name\nforeign-owned-fixture\n";
        fs::write(&foreign, foreign_bytes).map_err(io)?;
        let source = CancellationSource::default();
        let mut lease = backend::RootLease::open(
            &fixture.project(),
            "docker-after-spawn",
            fixture.host.clone(),
            redaction::Config::default(),
            source.token(),
        )
        .await?;
        let outcome = async {
            let approval = fixture.approval();
            let submissions = std::cell::RefCell::new(Vec::new());
            let controller = fixture.controller(&lease, &approval, Instant::now(), &submissions)?;
            let cancel = async {
                let marker = fixture.state()?.join("create-ready");
                for _ in 0..10_000 {
                    if marker.is_file() {
                        let pid = fs::read_to_string(&marker)
                            .map_err(io)?
                            .parse::<u32>()
                            .map_err(|error| Error::new("Error", error.to_string()))?;
                        let process = HPROCESS::OpenProcess(
                            co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
                            false,
                            pid,
                        )
                        .map_err(|error| Error::new("Error", error.to_string()))?;
                        assert_eq!(
                            process
                                .WaitForSingleObject(Some(0))
                                .map_err(|error| Error::new("Error", error.to_string()))?,
                            co::WAIT::TIMEOUT
                        );
                        source.cancel(crate::cancel::Reason::Cancelled)?;
                        return Ok::<_, Error>((pid, process));
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Err(Error::new(
                    "Error",
                    "Actual native create process readiness was not observed",
                ))
            };
            let args = ["echo".to_owned()];
            let (executed, cancelled) = tokio::join!(controller.execute(&args, 1), cancel);
            let (pid, process) = cancelled?;
            let error = match executed {
                Err(error) => error,
                Ok(_) => {
                    return Err(Error::new(
                        "Error",
                        "Cancelled native create returned an observation",
                    ));
                }
            };
            assert!(crate::cancel::is_run_cancelled(&error), "{error:?}");
            assert_eq!(
                process
                    .WaitForSingleObject(Some(0))
                    .map_err(|error| Error::new("Error", error.to_string()))?,
                co::WAIT::OBJECT_0
            );
            let receipts = submissions.borrow();
            let receipt = receipts.first().ok_or_else(|| {
                Error::new("Error", "Actual native create has no submission receipt")
            })?;
            assert_eq!(
                receipt.get("state"),
                Some(&Value::string("may-have-executed"))
            );
            assert_eq!(
                receipt.get("native_pid"),
                Some(&Value::Number(f64::from(pid)))
            );
            assert_eq!(fs::read(&foreign).map_err(io)?, foreign_bytes);
            let calls = fs::read_to_string(fixture.state()?.join("calls")).map_err(io)?;
            let removal = calls.lines().any(|line| line.contains("\"rm\""));
            if mode == "create-before-object" {
                assert_eq!(error.code, Some("ROSTER_PROCESS_CLEANUP"));
                assert!(!removal);
                assert!(lease.check().is_err());
            } else {
                assert!(removal);
                assert!(
                    !calls
                        .lines()
                        .filter(|line| line.contains("\"rm\""))
                        .any(|line| line.contains(&"f".repeat(64)))
                );
                assert!(lease.storage_check().is_ok());
            }
            Ok(())
        }
        .await;
        if mode == "create-before-object" {
            outcome?;
            assert!(lease.close().await.is_err());
            assert!(lease.ownership().path.is_file());
        } else {
            locks::finish(outcome, lease.close().await)?;
            fixture.close()?;
        }
    }
    Ok(())
}
