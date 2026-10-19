use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use tokio::sync::oneshot;

use crate::Error;
use crate::cancel::{CancellationSource, Reason};
use crate::json::{self, Value};
use crate::text::Text;

use super::Failure;
use super::admission::Admission;
use super::builtin::{BuiltinChat, Options};
use super::control::{NoObserver, Observer};
use super::http::{Body, Request, Response, Transport};
use super::openai::{CallOptions, Credentials, LastResponse};

fn parse(source: &str) -> Result<Value, Error> {
    json::parse(&Text::from(source))
}

fn config(limit: u32) -> Result<Value, Error> {
    parse(&format!(
        r#"{{"llm":{{"base_url":"http://127.0.0.1:8000/v1","model":"configured","fleet_profile":"metadata-drop","concurrency":1,"max_tokens":4096,"max_requests":{limit},"request_timeout_ms":1000}}}}"#
    ))
}

fn request() -> Result<Value, Error> {
    parse(r#"{"messages":[{"role":"user","content":"Exact metadata Drop request"}]}"#)
}

struct Point {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

fn point() -> (Point, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (entered, ready) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    (
        Point {
            entered,
            release: wait,
        },
        ready,
        release,
    )
}

impl Point {
    async fn wait(self) -> Result<(), Error> {
        self.entered
            .send(())
            .map_err(|_| Error::new("Error", "Actual readiness receiver was dropped"))?;
        self.release
            .await
            .map_err(|_| Error::new("Error", "Actual observation gate was closed"))
    }
}

struct Packet {
    value: Value,
    body: Option<Point>,
}

fn packet(model: &str, reason: &str, usage: Option<Value>) -> Result<Packet, Error> {
    Ok(Packet {
        value: Value::object(vec![
            ("model", Value::string(model)),
            ("usage", usage.unwrap_or(Value::Null)),
            (
                "choices",
                Value::Array(vec![Value::object(vec![
                    ("finish_reason", Value::string(reason)),
                    (
                        "message",
                        parse(r#"{"role":"assistant","content":"visible answer"}"#)?,
                    ),
                ])]),
            ),
        ]),
        body: None,
    })
}

#[derive(Default)]
struct Counts {
    sends: Cell<u64>,
    bodies: Cell<u64>,
    closed: Cell<u64>,
}

struct RecordedBody {
    bytes: Option<Vec<u8>>,
    point: Option<Point>,
    counts: Rc<Counts>,
}

impl Drop for RecordedBody {
    fn drop(&mut self) {
        self.counts
            .bodies
            .set(self.counts.bodies.get().saturating_sub(1));
        self.counts.closed.set(self.counts.closed.get() + 1);
    }
}

impl Body for RecordedBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if let Some(point) = self.point.take() {
            point.wait().await?;
        }
        Ok(self.bytes.take())
    }
}

struct Recorded {
    packets: VecDeque<Packet>,
    counts: Rc<Counts>,
}

impl Recorded {
    fn new(packets: Vec<Packet>) -> Self {
        Self {
            packets: packets.into(),
            counts: Rc::default(),
        }
    }
}

impl Transport for Recorded {
    type Body = RecordedBody;

    async fn send(&mut self, _request: &Request) -> Result<Response<Self::Body>, Failure> {
        self.counts.sends.set(self.counts.sends.get() + 1);
        let packet = self
            .packets
            .pop_front()
            .ok_or_else(|| Error::new("Error", "Unexpected metadata-test HTTP send"))?;
        self.counts.bodies.set(self.counts.bodies.get() + 1);
        Ok(Response {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Some(RecordedBody {
                bytes: Some(packet.value.json().into_bytes()),
                point: packet.body,
                counts: self.counts.clone(),
            }),
        })
    }
}

struct CompletionObserver {
    point: Option<Point>,
    error: Option<Error>,
}

impl Observer for CompletionObserver {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if crate::domain::is_text(event.get("type"), "completion") {
            if let Some(point) = self.point.take() {
                point.wait().await?;
            }
            if let Some(error) = self.error.take() {
                return Err(error);
            }
        }
        Ok(())
    }
}

fn released(admission: &Admission) -> Result<(), Error> {
    let pending = admission.acquire(&Text::from("metadata-drop"), None, None)?;
    assert!(pending.granted()?);
    assert_eq!(admission.depth(&Text::from("metadata-drop"))?, 0);
    Ok(())
}

fn snapshot(chat: &BuiltinChat<'_>) -> Result<LastResponse, Error> {
    chat.last_response()
        .cloned()
        .ok_or_else(|| Error::new("Error", "Actual response snapshot is absent"))
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn dropping_the_second_actual_send_cannot_publish_the_first_usage_as_a_total()
-> Result<(), Error> {
    for recovery_usage in [
        None,
        Some(parse(r#"{"prompt_tokens":0,"completion_tokens":0}"#)?),
    ] {
        let admission = Admission::default();
        let mut chat = BuiltinChat::new(&config(3)?, &admission, Options::default())?
            .ok_or_else(|| Error::new("Error", "Metadata fixture chat is disabled"))?;
        let (body, entered, release) = point();
        let mut second = packet("unobserved", "stop", None)?;
        second.body = Some(body);
        let mut transport = Recorded::new(vec![
            packet(
                "partial-served",
                "length",
                Some(parse(r#"{"prompt_tokens":2,"completion_tokens":0}"#)?),
            )?,
            second,
            packet("recovered-served", "stop", recovery_usage.clone())?,
        ]);
        let counts = transport.counts.clone();
        let request = request()?;
        let mut observer = NoObserver;
        let credentials = Credentials::default();
        let mut operation = Box::pin(chat.complete(
            &request,
            &mut transport,
            &mut observer,
            &credentials,
            CallOptions::default(),
        ));
        tokio::select! {
            biased;
            result = &mut operation => return Err(Error::new("Error", format!("Second body never reached its actual entry: {result:?}"))),
            ready = entered => ready.map_err(|_| Error::new("Error", "Second body readiness was not delivered"))?,
        }
        assert_eq!(counts.sends.get(), 2);
        assert_eq!(counts.bodies.get(), 1);
        drop(operation);
        assert_eq!(chat.last_usage(), Some(&parse("{}")?));
        assert_eq!(chat.last_attempts(), 2);
        assert_eq!(chat.request_count(), 2);
        let partial = snapshot(&chat)?;
        assert_eq!(partial.model, Text::from("partial-served"));
        assert_eq!(partial.usage.and_then(|usage| usage.prompt_tokens), Some(2));
        assert_eq!(
            partial.usage.and_then(|usage| usage.completion_tokens),
            Some(0)
        );
        assert_eq!(counts.bodies.get(), 0);
        assert_eq!(counts.closed.get(), 2);
        assert!(release.is_closed());
        released(&admission)?;
        let response = chat
            .complete(
                &request,
                &mut transport,
                &mut observer,
                &credentials,
                CallOptions::default(),
            )
            .await
            .map_err(|failure| *failure.error)?;
        assert_eq!(response.usage, recovery_usage);
        assert_eq!(chat.last_usage(), recovery_usage.as_ref());
        assert_eq!(snapshot(&chat)?.model, Text::from("recovered-served"));
        assert_eq!(snapshot(&chat)?.usage.is_some(), recovery_usage.is_some());
        assert_eq!(chat.last_attempts(), 1);
        assert_eq!(chat.request_count(), 3);
        assert_eq!(counts.sends.get(), 3);
        let failure = chat
            .complete(
                &request,
                &mut transport,
                &mut observer,
                &credentials,
                CallOptions::default(),
            )
            .await
            .err()
            .ok_or_else(|| Error::new("Error", "Dropped call reset its actual-send ceiling"))?;
        assert_eq!(failure.error.code, Some("ROSTER_MODEL_REQUEST_LIMIT"));
        assert_eq!(chat.request_count(), 3);
        assert_eq!(counts.sends.get(), 3);
        released(&admission)?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_then_immediate_observer_future_drop_preserves_the_prior_response()
-> Result<(), Error> {
    let admission = Admission::default();
    let mut chat = BuiltinChat::new(&config(3)?, &admission, Options::default())?
        .ok_or_else(|| Error::new("Error", "Metadata fixture chat is disabled"))?;
    let mut transport = Recorded::new(vec![
        packet(
            "prior-served",
            "stop",
            Some(parse(r#"{"prompt_tokens":0,"completion_tokens":5}"#)?),
        )?,
        packet(
            "revoked-served",
            "stop",
            Some(parse(r#"{"prompt_tokens":13,"completion_tokens":0}"#)?),
        )?,
        packet("recovered-served", "stop", None)?,
    ]);
    let request = request()?;
    let credentials = Credentials::default();
    chat.complete(
        &request,
        &mut transport,
        &mut NoObserver,
        &credentials,
        CallOptions::default(),
    )
    .await
    .map_err(|failure| *failure.error)?;
    let previous = snapshot(&chat)?;
    let counts = transport.counts.clone();
    let source = CancellationSource::default();
    let token = source.token();
    let (point, entered, release) = point();
    let mut observer = CompletionObserver {
        point: Some(point),
        error: None,
    };
    let mut operation = Box::pin(chat.complete(
        &request,
        &mut transport,
        &mut observer,
        &credentials,
        CallOptions {
            signal: Some(&token),
            ..CallOptions::default()
        },
    ));
    tokio::select! {
        biased;
        result = &mut operation => return Err(Error::new("Error", format!("Observer never reached its actual entry: {result:?}"))),
        ready = entered => ready.map_err(|_| Error::new("Error", "Completion observer readiness was not delivered"))?,
    }
    source.cancel(Reason::Cancelled)?;
    drop(operation);
    assert_eq!(chat.last_response(), Some(&previous));
    assert_eq!(chat.request_count(), 2);
    assert_eq!(counts.sends.get(), 2);
    assert_eq!(counts.bodies.get(), 0);
    assert_eq!(counts.closed.get(), 2);
    assert!(release.is_closed());
    assert_eq!(token.reason()?, Some(Reason::Cancelled));
    released(&admission)?;
    let response = chat
        .complete(
            &request,
            &mut transport,
            &mut NoObserver,
            &credentials,
            CallOptions::default(),
        )
        .await
        .map_err(|failure| *failure.error)?;
    assert!(response.usage.is_none());
    assert!(chat.last_usage().is_none());
    assert_eq!(snapshot(&chat)?.model, Text::from("recovered-served"));
    assert!(snapshot(&chat)?.usage.is_none());
    assert_eq!(chat.request_count(), 3);
    assert_eq!(counts.sends.get(), 3);
    released(&admission)?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum ObservationEnd {
    Drop,
    CancelDrop,
    TaskTimeoutDrop,
    DeadlineDrop,
    DeadlineReturn,
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn observer_drop_and_deadline_matrix_preserves_absent_or_prior_evidence_and_recovers()
-> Result<(), Error> {
    for end in [
        ObservationEnd::Drop,
        ObservationEnd::CancelDrop,
        ObservationEnd::TaskTimeoutDrop,
        ObservationEnd::DeadlineDrop,
        ObservationEnd::DeadlineReturn,
    ] {
        for prior in [false, true] {
            for recovery_usage in [
                None,
                Some(parse(r#"{"prompt_tokens":0,"completion_tokens":0}"#)?),
            ] {
                let admission = Admission::default();
                let mut chat = BuiltinChat::new(&config(3)?, &admission, Options::default())?
                    .ok_or_else(|| Error::new("Error", "Metadata fixture chat is disabled"))?;
                let mut packets = Vec::new();
                if prior {
                    packets.push(packet(
                        "prior-served",
                        "stop",
                        Some(parse(r#"{"prompt_tokens":0,"completion_tokens":5}"#)?),
                    )?);
                }
                packets.push(packet(
                    "not-yet-published",
                    "stop",
                    Some(parse(r#"{"prompt_tokens":13,"completion_tokens":0}"#)?),
                )?);
                packets.push(packet("recovered-served", "stop", recovery_usage.clone())?);
                let mut transport = Recorded::new(packets);
                let counts = transport.counts.clone();
                let request = request()?;
                let credentials = Credentials::default();
                if prior {
                    chat.complete(
                        &request,
                        &mut transport,
                        &mut NoObserver,
                        &credentials,
                        CallOptions::default(),
                    )
                    .await
                    .map_err(|failure| *failure.error)?;
                }
                let previous = chat.last_response().cloned();
                let before = counts.sends.get();
                let source = CancellationSource::default();
                let token = source.token();
                let (point, entered, release) = point();
                let mut observer = CompletionObserver {
                    point: Some(point),
                    error: None,
                };
                let mut operation = Box::pin(chat.complete(
                    &request,
                    &mut transport,
                    &mut observer,
                    &credentials,
                    CallOptions {
                        signal: Some(&token),
                        ..CallOptions::default()
                    },
                ));
                tokio::select! {
                    biased;
                    result = &mut operation => return Err(Error::new("Error", format!("Actual observer entry missing for {end:?}: {result:?}"))),
                    ready = entered => ready.map_err(|_| Error::new("Error", "Actual observer entry was not delivered"))?,
                }
                match end {
                    ObservationEnd::Drop => drop(operation),
                    ObservationEnd::CancelDrop => {
                        source.cancel(Reason::Cancelled)?;
                        drop(operation);
                        assert_eq!(token.reason()?, Some(Reason::Cancelled));
                    }
                    ObservationEnd::TaskTimeoutDrop => {
                        source.cancel(Reason::TaskTimeout(450))?;
                        drop(operation);
                        assert_eq!(token.reason()?, Some(Reason::TaskTimeout(450)));
                    }
                    ObservationEnd::DeadlineDrop => {
                        tokio::time::advance(std::time::Duration::from_millis(1001)).await;
                        drop(operation);
                    }
                    ObservationEnd::DeadlineReturn => {
                        tokio::time::advance(std::time::Duration::from_millis(1001)).await;
                        let failure = operation.await.err().ok_or_else(|| {
                            Error::new("Error", "Expired observation returned success")
                        })?;
                        assert_eq!(failure.error.code, Some("ROSTER_LLM_TIMEOUT"));
                    }
                }
                assert_eq!(chat.last_response(), previous.as_ref(), "{end:?}/{prior}");
                assert_eq!(chat.request_count(), before + 1);
                assert_eq!(counts.sends.get(), before + 1);
                assert_eq!(counts.bodies.get(), 0);
                assert_eq!(counts.closed.get(), before + 1);
                assert!(release.is_closed());
                released(&admission)?;
                let response = chat
                    .complete(
                        &request,
                        &mut transport,
                        &mut NoObserver,
                        &credentials,
                        CallOptions::default(),
                    )
                    .await
                    .map_err(|failure| *failure.error)?;
                assert_eq!(response.usage, recovery_usage);
                assert_eq!(chat.last_usage(), recovery_usage.as_ref());
                assert_eq!(snapshot(&chat)?.model, Text::from("recovered-served"));
                assert_eq!(snapshot(&chat)?.usage.is_some(), recovery_usage.is_some());
                assert_eq!(
                    snapshot(&chat)?.usage.and_then(|usage| usage.prompt_tokens),
                    recovery_usage.as_ref().map(|_| 0)
                );
                assert_eq!(
                    snapshot(&chat)?
                        .usage
                        .and_then(|usage| usage.completion_tokens),
                    recovery_usage.as_ref().map(|_| 0)
                );
                assert_eq!(chat.request_count(), before + 2);
                assert_eq!(counts.sends.get(), before + 2);
                assert_eq!(counts.bodies.get(), 0);
                released(&admission)?;
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_returned_non_cancellation_observer_error_commits_the_actual_response()
-> Result<(), Error> {
    for plain in [false, true] {
        for usage in [
            None,
            Some(parse(r#"{"prompt_tokens":0,"completion_tokens":0}"#)?),
        ] {
            let admission = Admission::default();
            let mut chat = BuiltinChat::new(&config(3)?, &admission, Options::default())?
                .ok_or_else(|| Error::new("Error", "Metadata fixture chat is disabled"))?;
            let mut transport = Recorded::new(vec![
                packet(
                    "prior-served",
                    "stop",
                    Some(parse(r#"{"prompt_tokens":0,"completion_tokens":5}"#)?),
                )?,
                packet("returned-error-served", "stop", usage.clone())?,
                packet("recovered-served", "stop", None)?,
            ]);
            let request = request()?;
            let credentials = Credentials::default();
            chat.complete(
                &request,
                &mut transport,
                &mut NoObserver,
                &credentials,
                CallOptions::default(),
            )
            .await
            .map_err(|failure| *failure.error)?;
            let counts = transport.counts.clone();
            let (point, entered, release) = point();
            let error = Error::new("Error", "Actual returned observer error");
            let mut observer = CompletionObserver {
                point: Some(point),
                error: Some(if plain {
                    error
                } else {
                    error.coded("ROSTER_RUN_LOG")
                }),
            };
            let mut operation = Box::pin(chat.complete(
                &request,
                &mut transport,
                &mut observer,
                &credentials,
                CallOptions::default(),
            ));
            tokio::select! {
                biased;
                result = &mut operation => return Err(Error::new("Error", format!("Actual observer entry missing: {result:?}"))),
                ready = entered => ready.map_err(|_| Error::new("Error", "Actual observer entry was not delivered"))?,
            }
            release
                .send(())
                .map_err(|_| Error::new("Error", "Actual pending observer was already dropped"))?;
            let failure = operation
                .await
                .err()
                .ok_or_else(|| Error::new("Error", "Observer failure returned success"))?;
            if plain {
                assert_eq!(failure.category, Some(super::Category::Network));
            } else {
                assert_eq!(failure.error.code, Some("ROSTER_RUN_LOG"));
            }
            let actual = snapshot(&chat)?;
            assert_eq!(actual.model, Text::from("returned-error-served"));
            assert_eq!(actual.usage.is_some(), usage.is_some());
            assert_eq!(
                actual.usage.and_then(|usage| usage.prompt_tokens),
                usage.as_ref().map(|_| 0)
            );
            assert_eq!(
                actual.usage.and_then(|usage| usage.completion_tokens),
                usage.as_ref().map(|_| 0)
            );
            assert_eq!(chat.last_usage(), usage.as_ref());
            assert_eq!(chat.request_count(), 2);
            assert_eq!(counts.sends.get(), 2);
            assert_eq!(counts.closed.get(), 2);
            assert_eq!(counts.bodies.get(), 0);
            released(&admission)?;
            chat.complete(
                &request,
                &mut transport,
                &mut NoObserver,
                &credentials,
                CallOptions::default(),
            )
            .await
            .map_err(|failure| *failure.error)?;
            assert_eq!(snapshot(&chat)?.model, Text::from("recovered-served"));
            assert!(snapshot(&chat)?.usage.is_none());
            assert!(chat.last_usage().is_none());
            assert_eq!(chat.request_count(), 3);
            assert_eq!(counts.sends.get(), 3);
            released(&admission)?;
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn dropping_a_retry_consumes_its_length_budget_and_cannot_reset_the_send_ceiling()
-> Result<(), Error> {
    let admission = Admission::default();
    let mut chat = BuiltinChat::new(&config(8)?, &admission, Options::default())?
        .ok_or_else(|| Error::new("Error", "Metadata fixture chat is disabled"))?;
    let zero = parse(r#"{"prompt_tokens":0,"completion_tokens":0}"#)?;
    let (body, entered, release) = point();
    let mut second = packet("unobserved", "stop", None)?;
    second.body = Some(body);
    let mut transport = Recorded::new(vec![
        packet("partial-served", "length", Some(zero.clone()))?,
        second,
        packet("recovered-served", "stop", Some(zero.clone()))?,
        packet("length-two", "length", Some(zero.clone()))?,
        packet("repair-two", "stop", Some(zero.clone()))?,
        packet("length-three", "length", Some(zero.clone()))?,
        packet("repair-three", "stop", Some(zero.clone()))?,
        packet("exhausted", "length", Some(zero.clone()))?,
    ]);
    let counts = transport.counts.clone();
    let request = request()?;
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let mut operation = Box::pin(chat.complete(
        &request,
        &mut transport,
        &mut observer,
        &credentials,
        CallOptions::default(),
    ));
    tokio::select! {
        biased;
        result = &mut operation => return Err(Error::new("Error", format!("Actual second body entry missing: {result:?}"))),
        ready = entered => ready.map_err(|_| Error::new("Error", "Actual body entry was not delivered"))?,
    }
    drop(operation);
    assert_eq!(chat.last_usage(), Some(&parse("{}")?));
    assert_eq!(chat.request_count(), 2);
    assert_eq!(counts.bodies.get(), 0);
    assert!(release.is_closed());
    released(&admission)?;
    for (sends, attempts) in [(3, 1), (5, 2), (7, 2)] {
        let response = chat
            .complete(
                &request,
                &mut transport,
                &mut observer,
                &credentials,
                CallOptions::default(),
            )
            .await
            .map_err(|failure| *failure.error)?;
        assert_eq!(response.usage, Some(zero.clone()));
        assert_eq!(chat.last_attempts(), attempts);
        assert_eq!(chat.request_count(), sends);
        assert_eq!(counts.sends.get(), sends);
        assert_eq!(counts.bodies.get(), 0);
        released(&admission)?;
    }
    let failure = chat
        .complete(
            &request,
            &mut transport,
            &mut observer,
            &credentials,
            CallOptions::default(),
        )
        .await
        .err()
        .ok_or_else(|| {
            Error::new(
                "Error",
                "A dropped retry restored its lifetime length budget",
            )
        })?;
    assert_eq!(failure.error.code, Some("ROSTER_LLM_FINISH_REASON"));
    assert_eq!(chat.last_attempts(), 1);
    assert_eq!(chat.request_count(), 8);
    assert_eq!(counts.sends.get(), 8);
    assert_eq!(snapshot(&chat)?.model, Text::from("exhausted"));
    let ceiling = chat
        .complete(
            &request,
            &mut transport,
            &mut observer,
            &credentials,
            CallOptions::default(),
        )
        .await
        .err()
        .ok_or_else(|| Error::new("Error", "The transport context sent past its ceiling"))?;
    assert_eq!(ceiling.error.code, Some("ROSTER_MODEL_REQUEST_LIMIT"));
    assert_eq!(chat.request_count(), 8);
    assert_eq!(counts.sends.get(), 8);
    assert_eq!(counts.closed.get(), 8);
    assert_eq!(counts.bodies.get(), 0);
    released(&admission)?;
    Ok(())
}
