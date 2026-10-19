use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

use super::*;

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn dropping_an_actual_running_tool_check_stops_owned_processes_and_retains_the_writer_for_recovery()
-> Result<(), Error> {
    use winsafe::{HPROCESS, co, prelude::*};
    let fixture = Fixture::new(
        &["src/app.mjs"],
        "characterization",
        &["The task behavior is verified."],
    )?;
    fixture.write("tests/app.test.mjs", "import test from 'node:test';\nimport { writeFileSync } from 'node:fs';\ntest('owned active check', async () => { setInterval(() => {}, 1000); writeFileSync('.roster/logs/active-check.json', JSON.stringify({ worker: process.pid, parent: process.ppid }), { flag: 'wx' }); await new Promise(() => {}); });\n")?;
    fixture.git(&["add", "tests/app.test.mjs"])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned long-running tool readiness fixture",
    ])?;
    files::create_private_directories(&fixture.root.join(".roster").join("logs"))?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    async {
        let mut transport = Recorded::new(vec![calls(vec![("test", "run_test", Value::object(Vec::new()))])]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let config = config(true, false, 4)?;
        let mut driver = driver(&mut transport, &mut events, &credentials, &admission);
        let ready = fixture.root.join(".roster").join("logs").join("active-check.json");
        let mut work = Box::pin(driver.run_coder(&mut boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &config, coder::Options::default()));
        let wait = async {
            loop {
                if let Some(snapshot) = secure::read(&ready, 4096)? {
                    if let Ok(value) = crate::json::parse(&Text::from(std::str::from_utf8(&snapshot.bytes).map_err(|_| Error::type_error("Owned readiness must be UTF-8"))?)) {
                        return Ok::<_, Error>(value);
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        let value = tokio::select! {
            ready = tokio::time::timeout(Duration::from_secs(30), wait) =>
                ready.map_err(|_| Error::new("Error", "Owned test did not reach the actual child readiness barrier"))??,
            _ = &mut work => return Err(Error::new("Error", "Coder finished before the owned test became ready")),
        };
        let mut handles = Vec::new();
        for name in ["worker", "parent"] {
            let pid = u32::try_from(domain::safe_integer(value.get(name)).ok_or_else(|| Error::type_error("Owned readiness PID missing"))?)
                .map_err(|_| Error::type_error("Owned readiness PID exceeds u32"))?;
            let handle = HPROCESS::OpenProcess(co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE, false, pid)
                .map_err(|error| Error::new("Error", error.to_string()))?;
            assert_eq!(handle.WaitForSingleObject(Some(0)).map_err(|error| Error::new("Error", error.to_string()))?, co::WAIT::TIMEOUT);
            handles.push(handle);
        }
        source.cancel(Reason::TaskTimeout(23))?;
        drop(work);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut stopped = true;
                for handle in &handles {
                    stopped &= handle.WaitForSingleObject(Some(0)).map_err(|error| Error::new("Error", error.to_string()))? == co::WAIT::OBJECT_0;
                }
                if stopped { return Ok::<_, Error>(()); }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.map_err(|_| Error::new("Error", "Owned processes survived a dropped native tool future"))??;
        assert_eq!(boundary.close().await.err().and_then(|error| error.code), Some("ROSTER_PROCESS_CLEANUP"));
        assert!(fixture.root.join(".roster").join("locks").join("run.lock").is_file());
        assert!(!fixture.root.join("RESULT.md").exists());
        Ok(())
    }.await
}

struct GatedBody {
    ready: Option<oneshot::Sender<()>>,
    release: oneshot::Receiver<()>,
    dropped: Rc<Cell<usize>>,
}

impl Body for GatedBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if let Some(ready) = self.ready.take() {
            ready
                .send(())
                .map_err(|_| Error::new("Error", "Body readiness receiver disappeared"))?;
        }
        (&mut self.release)
            .await
            .map_err(|_| Error::new("Error", "Body release was dropped"))?;
        Ok(None)
    }
}

impl Drop for GatedBody {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

struct GatedTransport {
    body: Option<GatedBody>,
    requests: usize,
}

impl Transport for GatedTransport {
    type Body = GatedBody;
    async fn send(&mut self, _: &Request) -> Result<Response<Self::Body>, Failure> {
        self.requests += 1;
        Ok(Response {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Some(
                self.body.take().ok_or_else(|| {
                    Error::new("Error", "Only one owned hanging body was approved")
                })?,
            ),
        })
    }
}

fn profile() -> Result<crate::fleet::Profile, Error> {
    crate::fleet::validate_profile(&Value::object(vec![
        ("id", Value::string("owned-shared-seat")),
        ("base_url", Value::string("http://127.0.0.1:1/v1")),
        ("model", Value::string("owned-route")),
        ("provider", Value::string("vllm")),
        ("context_max", Value::Number(32768.0)),
        ("concurrency", Value::Number(1.0)),
        ("hardware", Value::string("owned fixture")),
        ("notes", Value::string("No live endpoint")),
    ]))
}

#[tokio::test(flavor = "current_thread")]
async fn actual_model_body_cancel_and_immediate_future_drop_release_only_owned_admission_without_result()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let (ready_tx, ready_rx) = oneshot::channel();
        let (_release_tx, release_rx) = oneshot::channel();
        let dropped = Rc::new(Cell::new(0));
        let mut transport = GatedTransport { body: Some(GatedBody { ready: Some(ready_tx), release: release_rx, dropped: dropped.clone() }), requests: 0 };
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let config = config(true, false, 10)?.with_profile(&profile()?)?;
        let mut driver = Driver { transport: &mut transport, observer: &mut events, credentials: &credentials, admission: &admission, signal: None };
        let mut work = Box::pin(driver.run_coder(&mut boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &config, coder::Options::default()));
        tokio::select! {
            ready = ready_rx => ready.map_err(|_| Error::new("Error", "Actual body did not become ready"))?,
            _ = &mut work => return Err(Error::new("Error", "Coder completed before its actual body barrier")),
        }
        let profile = Text::from("owned-shared-seat");
        let queued = admission.acquire(&profile, None, None)?;
        assert_eq!(admission.depth(&profile)?, 1);
        source.cancel(Reason::TaskTimeout(17))?;
        source.cancel(Reason::Cancelled)?;
        drop(work);
        assert_eq!(dropped.get(), 1);
        let mut successor = queued.wait().await?;
        successor.release()?;
        assert_eq!(admission.depth(&profile)?, 0);
        assert_eq!(boundary.token().reason()?, Some(Reason::TaskTimeout(17)));
        assert!(!fixture.root.join("RESULT.md").exists());
        assert!(boundary.written_files().is_empty());
        assert_eq!(transport.requests, 1);
        Ok(())
    }.await;
    locks::finish(result, boundary.close().await)
}

#[tokio::test(flavor = "current_thread")]
async fn actual_model_request_timeout_drops_owned_body_and_cannot_install_stub_or_verification()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let (ready_tx, _ready_rx) = oneshot::channel();
        let (_release_tx, release_rx) = oneshot::channel();
        let dropped = Rc::new(Cell::new(0));
        let mut transport = GatedTransport {
            body: Some(GatedBody {
                ready: Some(ready_tx),
                release: release_rx,
                dropped: dropped.clone(),
            }),
            requests: 0,
        };
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let source_config = crate::config::format(config(true, false, 10)?.as_value())?
            .to_utf8()?
            .replacen("  model:", "  request_timeout_ms: 80\n  model:", 1);
        let config = crate::config::parse(&Text::from(source_config.as_str()))?;
        let mut driver = Driver {
            transport: &mut transport,
            observer: &mut events,
            credentials: &credentials,
            admission: &admission,
            signal: None,
        };
        let coder = driver
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config,
                coder::Options::default(),
            )
            .await?;
        assert_ne!(coder.status(), finalization::Status::Pass);
        assert_ne!(coder.status(), finalization::Status::Stub);
        assert_eq!(
            coder.failure().and_then(|error| error.code),
            Some("ROSTER_LLM_TIMEOUT")
        );
        assert_eq!(dropped.get(), 1);
        assert!(boundary.last_tests().is_none());
        assert!(boundary.written_files().is_empty());
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

struct FailingObserver {
    completed: usize,
}

impl Observer for FailingObserver {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if domain::is_text(event.get("type"), "completion") {
            self.completed += 1;
            if self.completed == 2 {
                return Err(Error::new(
                    "OwnedObserverFailure",
                    "Original settled observer failure",
                )
                .coded("ROSTER_RUN_LOG"));
            }
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn settled_observer_failure_retains_actual_served_evidence_and_first_cause_without_passing_result()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let mut done = summary();
        domain::put(
            &mut done,
            "model",
            Value::string("actual-observed-on-failure"),
        )?;
        let mut transport = Recorded::new(vec![
            calls(vec![(
                "write",
                "write_file",
                write("docs/guide.md", "# Candidate guide\n"),
            )]),
            done,
        ]);
        let mut observer = FailingObserver { completed: 0 };
        let credentials = Credentials::default();
        let admission = Admission::default();
        let mut driver = Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &admission,
            signal: None,
        };
        let coder = driver
            .run_coder(
                &mut boundary,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &config(true, false, 10)?,
                coder::Options::default(),
            )
            .await?;
        assert_eq!(
            coder.failure().and_then(|error| error.code),
            Some("ROSTER_RUN_LOG")
        );
        assert_eq!(
            coder.failure().map(|error| error.message.as_str()),
            Some("Original settled observer failure")
        );
        assert_eq!(
            coder.run().map(|run| run.metrics().model.as_str()),
            Some("actual-observed-on-failure")
        );
        assert!(!fixture.read("RESULT.md")?.contains("Checks: PASS"));
        assert_eq!(fixture.read("docs/guide.md")?, "# Candidate guide\n");
        Ok(())
    }
    .await;
    locks::finish(result, boundary.close().await)
}

async fn http_request(stream: &mut tokio::net::TcpStream) -> Result<Value, Error> {
    let mut bytes = Vec::new();
    let header = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        let mut buffer = [0u8; 4096];
        let count = stream.read(&mut buffer).await.map_err(io)?;
        if count == 0 || bytes.len() + count > 1024 * 1024 {
            return Err(Error::new(
                "Error",
                "Owned HTTP request header was incomplete or excessive",
            ));
        }
        bytes.extend(
            buffer
                .get(..count)
                .ok_or_else(|| Error::type_error("HTTP read size invalid"))?,
        );
    };
    let headers = std::str::from_utf8(
        bytes
            .get(..header)
            .ok_or_else(|| Error::type_error("HTTP header bounds invalid"))?,
    )
    .map_err(|_| Error::type_error("HTTP header must be UTF-8"))?;
    let length = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .filter(|length| *length <= 1024 * 1024)
        .ok_or_else(|| Error::type_error("Owned HTTP request needs a bounded content-length"))?;
    while bytes.len() < header + length {
        let mut buffer = [0u8; 4096];
        let count = stream.read(&mut buffer).await.map_err(io)?;
        if count == 0 || bytes.len() + count > 2 * 1024 * 1024 {
            return Err(Error::new(
                "Error",
                "Owned HTTP request body was incomplete or excessive",
            ));
        }
        bytes.extend(
            buffer
                .get(..count)
                .ok_or_else(|| Error::type_error("HTTP read size invalid"))?,
        );
    }
    crate::json::parse(&Text::from(
        std::str::from_utf8(
            bytes
                .get(header..header + length)
                .ok_or_else(|| Error::type_error("HTTP body bounds invalid"))?,
        )
        .map_err(|_| Error::type_error("HTTP body must be UTF-8"))?,
    ))
}

fn sse_packet(packet: &Value) -> Result<String, Error> {
    let choice = packet
        .get("choices")
        .and_then(domain::items)
        .and_then(|choices| choices.first())
        .ok_or_else(|| Error::type_error("Owned SSE response requires a choice"))?;
    let mut delta = execution::required(choice, "message")?.clone();
    if let Some(Value::Array(calls)) = delta.get("tool_calls").cloned() {
        let mut indexed = Vec::new();
        for (index, mut call) in calls.into_iter().enumerate() {
            domain::put(&mut call, "index", Value::Number(index as f64))?;
            indexed.push(call);
        }
        domain::put(&mut delta, "tool_calls", Value::Array(indexed))?;
    }
    let chunk = Value::object(vec![
        ("model", Value::string("owned-http-served")),
        (
            "choices",
            Value::Array(vec![Value::object(vec![
                ("index", Value::Number(0.0)),
                ("delta", delta),
                (
                    "finish_reason",
                    execution::required(choice, "finish_reason")?.clone(),
                ),
            ])]),
        ),
    ]);
    Ok(format!("data: {}\n\ndata: [DONE]\n\n", chunk.json()))
}

#[tokio::test(flavor = "current_thread")]
async fn actual_loopback_sse_drives_coder_writes_and_fresh_reviewer_read_request_without_function_tools()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["docs/guide.md"],
        "none",
        &["The requested guide is delivered."],
    )?;
    let source = CancellationSource::default();
    let mut boundary = fixture.open(&source).await?;
    let result = async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.map_err(io)?;
        let address = listener.local_addr().map_err(io)?;
        let responses = vec![
            calls(vec![("write", "write_file", write("docs/guide.md", "# HTTP-delivered guide\n"))]), summary(),
            review_packet(verdict(1)),
            review_packet(Value::object(vec![("requests", Value::Array(vec![Value::object(vec![("tool", Value::string("search_text")), ("query", Value::string("HTTP-delivered")), ("path", Value::string("docs/guide.md"))])]))])),
            review_packet(verdict(1)),
        ];
        let server = async {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().await.map_err(io)?;
                requests.push(http_request(&mut stream).await?);
                let body = sse_packet(&response)?;
                let header = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                stream.write_all(header.as_bytes()).await.map_err(io)?;
                let midpoint = body.len() / 2;
                stream.write_all(body.as_bytes().get(..midpoint).ok_or_else(|| Error::type_error("SSE midpoint invalid"))?).await.map_err(io)?;
                tokio::task::yield_now().await;
                stream.write_all(body.as_bytes().get(midpoint..).ok_or_else(|| Error::type_error("SSE tail invalid"))?).await.map_err(io)?;
                stream.shutdown().await.map_err(io)?;
            }
            Ok::<_, Error>(requests)
        };
        let source_config = crate::config::format(config(true, false, 10)?.as_value())?.to_utf8()?
            .replace("http://127.0.0.1:1/v1", &format!("http://{address}/v1"));
        let config = crate::config::parse(&Text::from(source_config.as_str()))?;
        let mut transport = crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let client = async {
            let mut driver = Driver { transport: &mut transport, observer: &mut events, credentials: &credentials, admission: &admission, signal: None };
            let coder = driver.run_coder(&mut boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &config, coder::Options::default()).await?;
            if coder.status() != finalization::Status::Pass { return Err(Error::new("Error", format!("HTTP coder did not pass: {:?}", coder.failure()))); }
            let review = driver.run_reviewer(&boundary, &coder, Path::new(env!("CARGO_MANIFEST_DIR")), &config, reviewer::Options::default()).await?;
            if review.status() != reviewer::Status::Pass { return Err(Error::new("Error", format!("HTTP reviewer did not pass: {:?}", review.failure()))); }
            Ok((coder, review))
        };
        let (requests, (coder, review)) = tokio::time::timeout(Duration::from_secs(600), async { tokio::try_join!(server, client) })
            .await.map_err(|_| Error::new("Error", "Owned HTTP seat fixture exceeded its deadline"))??;
        assert_eq!(requests.len(), 5);
        assert!(requests.iter().all(|request| request.get("stream") == Some(&Value::Boolean(true))));
        assert!(requests.iter().skip(2).all(|request| request.get("tools").is_none()));
        assert_eq!(fixture.read("docs/guide.md")?, "# HTTP-delivered guide\n");
        assert_eq!(coder.run().map(|run| run.metrics().model.as_str()), Some("owned-http-served"));
        assert_eq!(review.run().map(|run| run.metrics().model.as_str()), Some("owned-http-served"));
        review.require_passing(&boundary, &coder, &evidence::Projection::default()).await?;
        Ok(())
    }.await;
    locks::finish(result, boundary.close().await)
}

pub(super) fn cargo_host(fixture: &Fixture) -> Result<tools::Host, Error> {
    let executable = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .ok_or_else(|| {
            Error::new(
                "Error",
                "Owned native consumer requires the invoking Cargo executable",
            )
        })?;
    let executable = fs::canonicalize(executable).map_err(io)?;
    let mut environment = std::env::vars()
        .filter(|(name, _)| {
            ["CARGO_HOME", "RUSTUP_HOME", "RUSTFLAGS", "RUSTUP_TOOLCHAIN"].contains(&name.as_str())
        })
        .collect::<BTreeMap<_, _>>();
    if let Ok(toolchain) = std::env::var("ROSTER_TEST_CARGO_TOOLCHAIN") {
        environment.insert("RUSTUP_TOOLCHAIN".to_owned(), toolchain);
    }
    environment.insert(
        "CARGO_TARGET_DIR".to_owned(),
        fixture
            .container
            .join("approved-target")
            .to_string_lossy()
            .into_owned(),
    );
    Ok(tools::Host {
        node20: None,
        cargo: Some(tools::CargoApproval {
            executable,
            environment,
        }),
        ..tools::Host::default()
    })
}

#[tokio::test(flavor = "current_thread")]
async fn actual_native_cargo_consumer_uses_complete_rust_cargo_tokio_context_and_named_red_green()
-> Result<(), Error> {
    let fixture = Fixture::new(
        &["rust/llm/worker.rs", "tests/added.rs"],
        "required",
        &["The native increment operation preserves zero and increments nonzero input."],
    )?;
    fixture.write("Cargo.toml", "[package]\nname = \"owned-native-seat-consumer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"rust/llm/worker.rs\"\n")?;
    fixture.write(
        "Cargo.lock",
        "version = 4\n\n[[package]]\nname = \"owned-native-seat-consumer\"\nversion = \"0.1.0\"\n",
    )?;
    fixture.write(
        "rust/llm/worker.rs",
        "pub fn next(_value: u8) -> u8 {\n    1\n}\n",
    )?;
    fixture.write("tests/base.rs", "#[test]\nfn retained_zero() {\n    assert_eq!(owned_native_seat_consumer::next(0), 1);\n}\n")?;
    fixture.git(&[
        "add",
        "Cargo.toml",
        "Cargo.lock",
        "rust/llm/worker.rs",
        "tests/base.rs",
    ])?;
    fixture.git(&[
        "commit",
        "--quiet",
        "-m",
        "Owned dependency-free Rust consumer",
    ])?;
    let source = CancellationSource::default();
    let mut boundary = tools::Boundary::open(
        &fixture.root,
        fixture.assignment.clone(),
        tools::Options::default(),
        cargo_host(&fixture)?,
        redaction::Config::default(),
        source.token(),
    )
    .await?;
    let result = async {
        let mut transport = Recorded::new(vec![
            calls(vec![("read", "read_file", path("rust/llm/worker.rs"))]),
            calls(vec![
                ("write", "write_file", write("rust/llm/worker.rs", "pub fn next(value: u8) -> u8 {\n    value + 1\n}\n")),
                ("test-write", "write_file", write("tests/added.rs", "#[test]\nfn nonzero_increment() {\n    assert_eq!(owned_native_seat_consumer::next(2), 3);\n}\n")),
            ]),
            calls(vec![("test", "run_test", Value::object(Vec::new()))]), summary(), self_review_packet(1, Vec::new()),
        ]);
        let mut events = Events::default();
        let credentials = Credentials::default();
        let admission = Admission::default();
        let coder = driver(&mut transport, &mut events, &credentials, &admission).run_coder(&mut boundary, Path::new(env!("CARGO_MANIFEST_DIR")), &config(true, false, 16)?, coder::Options {
            approval: Some(backend::Approval { counterfactual: true, timeout_ms: Some(120_000), ..backend::Approval::default() }), ..coder::Options::default()
        }).await?;
        assert_eq!(coder.status(), finalization::Status::Pass, "{:?}\n{}", coder.failure(), coder.result().json());
        let first = transport.requests.first().ok_or_else(|| Error::type_error("Native coder request missing"))?.json();
        for name in ["Rust implementation", "Rust Cargo", "Rust Tokio", "Tokio runtime", "minimum Rust 1.85"] { assert!(first.contains(name), "Missing complete guidance: {name}"); }
        let result = fixture.read("RESULT.md")?;
        assert!(result.contains("cargo") && result.contains("nonzero_increment"));
        assert!(coder.result().get("redGreen").is_some());
        assert!(coder.result().get("tests").is_some_and(|tests| tests.get("native") == Some(&Value::Boolean(true))));
        Ok(())
    }.await;
    locks::finish(result, boundary.close().await)
}
