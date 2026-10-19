use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Instant;

#[derive(Clone)]
struct Received {
    body: Value,
    authorization: Option<String>,
}

enum Behavior {
    Reply {
        packet: Value,
        authorization: Option<String>,
    },
    SilentJson,
    SilentStream,
}

struct Endpoint {
    endpoint: String,
    requests: Arc<Mutex<Vec<Received>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), Error>>>,
}

fn receive(stream: &mut TcpStream) -> Result<Received, Error> {
    stream.set_nonblocking(false).map_err(io)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(io)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(io)?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let (start, length, authorization) =
        loop {
            let count = stream.read(&mut buffer).map_err(io)?;
            if count == 0 || bytes.len() + count > 4 * 1024 * 1024 {
                return Err(Error::type_error(
                    "Owned HTTP headers/body closed or exceeded their bound",
                ));
            }
            bytes.extend_from_slice(
                buffer
                    .get(..count)
                    .ok_or_else(|| Error::type_error("Invalid owned HTTP read"))?,
            );
            if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = text(
                    bytes
                        .get(..end)
                        .ok_or_else(|| Error::type_error("Missing owned HTTP headers"))?,
                )?;
                let mut length = None;
                let mut authorization = None;
                for line in headers.lines() {
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length =
                                Some(value.trim().parse::<usize>().map_err(|_| {
                                    Error::type_error("Invalid owned Content-Length")
                                })?);
                        }
                        if key.eq_ignore_ascii_case("authorization") {
                            authorization = Some(value.trim().to_owned());
                        }
                    }
                }
                break (
                    end + 4,
                    length
                        .filter(|length| *length <= 4 * 1024 * 1024)
                        .ok_or_else(|| Error::type_error("Owned Content-Length unavailable"))?,
                    authorization,
                );
            }
        };
    while bytes.len() < start + length {
        let count = stream.read(&mut buffer).map_err(io)?;
        if count == 0 {
            return Err(Error::type_error("Owned HTTP body closed early"));
        }
        bytes.extend_from_slice(
            buffer
                .get(..count)
                .ok_or_else(|| Error::type_error("Invalid owned HTTP body read"))?,
        );
    }
    Ok(Received {
        body: github_agent_roster::json::parse(&Text::from(text(
            bytes
                .get(start..start + length)
                .ok_or_else(|| Error::type_error("Missing owned HTTP body"))?,
        )?))?,
        authorization,
    })
}

impl Endpoint {
    fn start(behavior: Behavior) -> Result<Self, Error> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(io)?;
        listener.set_nonblocking(true).map_err(io)?;
        let endpoint = format!("http://{}/v1", listener.local_addr().map_err(io)?);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let records = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(600);
            let mut pending = Vec::new();
            while !stopped.load(Ordering::SeqCst) {
                if Instant::now() >= deadline {
                    return Err(Error::type_error("Owned endpoint workflow guard expired"));
                }
                match listener.accept() {
                    Ok((mut stream, address)) => {
                        if !address.ip().is_loopback() {
                            return Err(Error::type_error(
                                "Only owned loopback clients are accepted",
                            ));
                        }
                        let received = receive(&mut stream)?;
                        let accepted = match &behavior {
                            Behavior::Reply { authorization, .. } => {
                                *authorization == received.authorization
                            }
                            _ => true,
                        };
                        records
                            .lock()
                            .map_err(|_| Error::type_error("Owned request lock poisoned"))?
                            .push(received);
                        match &behavior {
                            Behavior::Reply { packet, .. } if accepted => {
                                let bytes = packet.json().into_bytes();
                                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).map_err(io)?;
                                stream.write_all(&bytes).map_err(io)?;
                            }
                            Behavior::Reply { .. } => {
                                stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").map_err(io)?;
                            }
                            Behavior::SilentJson => {
                                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\n{").map_err(io)?;
                                stream.flush().map_err(io)?;
                                pending.push(stream);
                            }
                            Behavior::SilentStream => {
                                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\n").map_err(io)?;
                                stream.flush().map_err(io)?;
                                pending.push(stream);
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => return Err(io(error)),
                }
            }
            drop(pending);
            Ok(())
        });
        Ok(Self {
            endpoint,
            requests,
            stop,
            worker: Some(worker),
        })
    }

    fn finish(mut self) -> Result<Vec<Received>, Error> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| Error::type_error("Owned endpoint panicked"))??;
        }
        self.requests
            .lock()
            .map_err(|_| Error::type_error("Owned request lock poisoned"))
            .map(|requests| requests.clone())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("Owned endpoint cleanup failed: {error}"),
                Err(_) => eprintln!("Owned endpoint worker panicked"),
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_effective_critic_auth_is_keyless_inherited_custom_or_explicitly_invalid()
-> Result<(), Error> {
    for mode in [
        "keyless",
        "inherited",
        "custom",
        "missing",
        "invalid",
        "blank",
    ] {
        let authorization = if ["inherited", "custom"].contains(&mode) {
            Some(format!("Bearer {SENTINEL}"))
        } else {
            None
        };
        let endpoint = Endpoint::start(Behavior::Reply {
            packet: support::packet(
                support::message("Actual selected critic response"),
                "stop",
                "actual-review49440-auth",
            ),
            authorization: authorization.clone(),
        })?;
        let fixture = Fixture::new()?;
        fixture.configure(Some(&endpoint.endpoint))?;
        let source = fs::read_to_string(fixture.root.join(".roster").join("config.yml"))
            .map_err(io)?
            .replace("\r\n", "\n")
            .replacen(
                "api_key_env: ROSTER_API_KEY",
                "api_key_env: PRIMARY_AUTH",
                1,
            )
            .replacen(
                "api_key_env: ROSTER_API_KEY",
                "api_key_env: INHERITED_AUTH",
                1,
            )
            .replacen(
                "llm:\n",
                &format!(
                    "llm:\n  api_key_optional: {}\n  request_timeout_ms: 1000\n",
                    mode == "keyless"
                ),
                1,
            );
        fixture.write(".roster/config.yml", &source)?;
        critic(
            &fixture,
            &endpoint.endpoint,
            ["custom", "invalid", "blank"]
                .contains(&mode)
                .then_some("CRITIC_AUTH"),
        )?;
        let loaded = fixture.loaded()?;
        let mut environment = Value::object(vec![
            (
                "PRIMARY_AUTH",
                Value::string("different-primary-opaque-value"),
            ),
            ("OPENAI_API_KEY", Value::Number(19.0)),
            ("UNSELECTED_FIELD", Value::Boolean(true)),
        ]);
        match mode {
            "inherited" => put(&mut environment, "INHERITED_AUTH", Value::string(SENTINEL)),
            "custom" => put(&mut environment, "CRITIC_AUTH", Value::string(SENTINEL)),
            "invalid" => put(&mut environment, "CRITIC_AUTH", Value::Number(3.0)),
            "blank" => put(&mut environment, "CRITIC_AUTH", Value::string(" ")),
            _ => {}
        }
        let selected = delivery::invocation_credentials(&environment, &loaded);
        if ["invalid", "blank"].contains(&mode) {
            assert!(selected.is_err());
            assert!(endpoint.finish()?.is_empty());
            continue;
        }
        let credentials = selected?;
        let selected = credentials
            .environment
            .as_ref()
            .ok_or_else(|| Error::type_error("No typed selected credentials"))?;
        assert!(
            selected.get("UNSELECTED_FIELD").is_none() && selected.get("OPENAI_API_KEY").is_none()
        );
        assert_eq!(
            selected.get("PRIMARY_AUTH"),
            Some(&Value::string("different-primary-opaque-value"))
        );
        let effective = loaded.config.with_profile(
            loaded
                .critic_profile()
                .ok_or_else(|| Error::type_error("Selected critic missing"))?,
        )?;
        let admission = Admission::default();
        let mut chat = llm::builtin::BuiltinChat::new(
            effective.as_value(),
            &admission,
            llm::builtin::Options::default(),
        )?
        .ok_or_else(|| Error::type_error("Effective critic unavailable"))?;
        let mut transport = llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
        let mut observer = NoObserver;
        let result = chat
            .complete(
                &request(),
                &mut transport,
                &mut observer,
                &credentials,
                CallOptions::default(),
            )
            .await;
        let records = endpoint.finish()?;
        if mode == "missing" {
            let failure = result
                .err()
                .ok_or_else(|| Error::type_error("Missing required critic credential passed"))?;
            assert_eq!(failure.category, Some(llm::Category::Authentication));
            assert!(records.is_empty());
        } else {
            let response = result.map_err(|failure| *failure.error)?;
            assert_eq!(response.model, Text::from("actual-review49440-auth"));
            assert_eq!(
                response.measured_usage()?,
                run::Usage {
                    prompt_tokens: Some(0),
                    completion_tokens: Some(0)
                }
            );
            assert_eq!(records.len(), 1);
            assert_eq!(
                records
                    .first()
                    .and_then(|record| record.authorization.as_ref()),
                authorization.as_ref()
            );
            assert_eq!(
                records.first().and_then(|record| record.body.get("model")),
                Some(&Value::string("configured-critic-route"))
            );
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_owned_timeout_and_stream_stall_keep_all_diagnostics_private()
-> Result<(), Error> {
    for stream in [false, true] {
        let endpoint = Endpoint::start(if stream {
            Behavior::SilentStream
        } else {
            Behavior::SilentJson
        })?;
        let endpoint_url = endpoint.endpoint.clone();
        let fixture = Fixture::new()?;
        let loaded = optional_config(&fixture, &endpoint.endpoint)?;
        let credentials = delivery::invocation_credentials(&Value::object(Vec::new()), &loaded)?;
        let mut config = loaded.config.as_value().clone();
        let mut llm = config
            .get("llm")
            .cloned()
            .ok_or_else(|| Error::type_error("Owned LLM mapping unavailable"))?;
        put(&mut llm, "request_timeout_ms", Value::Number(1000.0));
        if stream {
            put(&mut llm, "stream_idle_timeout_ms", Value::Number(30.0));
        } else {
            put(&mut llm, "request_timeout_ms", Value::Number(50.0));
        }
        put(&mut config, "llm", llm);
        let chat = Chat::new(&config)?
            .ok_or_else(|| Error::type_error("Owned timeout/stall chat unavailable"))?;
        let mut transport = llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
        let mut observer = NoObserver;
        let failure = chat
            .complete(
                &request(),
                &mut transport,
                &mut observer,
                &credentials,
                CallOptions::default(),
            )
            .await
            .err()
            .ok_or_else(|| Error::type_error("Incomplete owned endpoint passed"))?;
        let records = endpoint.finish()?;
        assert!(!records.is_empty());
        assert_eq!(
            failure.error.code,
            Some(if stream {
                "ROSTER_LLM_STALL"
            } else {
                "ROSTER_LLM_TIMEOUT"
            })
        );
        let error = delivery::safe_invocation_error(&failure.error, &loaded, &credentials)?;
        let parsed = url::Url::parse(&endpoint_url)
            .map_err(|_| Error::type_error("Owned endpoint invalid"))?;
        assert!(
            !error
                .message
                .contains(&parsed[url::Position::BeforeHost..url::Position::AfterPort])
        );
        assert!(!error.message.contains("configured-route-not-a-response"));
    }
    Ok(())
}

#[test]
fn review49440_actual_cli_timeout_does_not_persist_private_authority_or_selected_secrets()
-> Result<(), Error> {
    let endpoint = Endpoint::start(Behavior::SilentJson)?;
    let endpoint_url = endpoint.endpoint.clone();
    let fixture = Fixture::new()?;
    fixture.task(
        &["docs/guide.md"],
        "The guide documents the requested bounded outcome.",
    )?;
    let _ = optional_config(&fixture, &endpoint.endpoint)?;
    critic(&fixture, &endpoint.endpoint, Some("CRITIC_AUTH"))?;
    let mut environment = fixture.environment()?;
    put(&mut environment, "CRITIC_AUTH", Value::string(SENTINEL));
    let output = fixture.invoke_with(
        &["run", "--seat", "coder", "--runtime", "builtin"],
        &environment,
        &fixture.root,
    )?;
    let records = endpoint.finish()?;
    assert!(!output.status.success() && !records.is_empty());
    let parsed =
        url::Url::parse(&endpoint_url).map_err(|_| Error::type_error("Owned endpoint invalid"))?;
    let authority = &parsed[url::Position::BeforeHost..url::Position::AfterPort];
    for bytes in [&output.stdout, &output.stderr] {
        let value = text(bytes)?;
        assert!(
            !value.contains(authority)
                && !value.contains(SENTINEL)
                && !value.contains("configured-route-not-a-response")
        );
    }
    let result = fs::read_to_string(fixture.root.join("RESULT.md")).map_err(io)?;
    assert!(
        !result.contains(authority) && !result.contains(SENTINEL),
        "Private request failure persisted in RESULT"
    );
    assert!(!result.contains("Checks: PASS"));
    assert!(
        !fixture
            .root
            .join(".roster")
            .join("locks")
            .join("run.lock")
            .exists()
    );
    Ok(())
}
