use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use crate::Error;
use crate::cancel::{CancellationSource, Reason};
use crate::domain;
use crate::json::{self, Value};
use crate::text::Text;

use super::control::{NoObserver, Observer};
use super::http::{Body, Request, Response, Transport};
use super::openai::{CallOptions, Chat, Credentials, parse_completion};
use super::stream::{CompletionAssembler, StreamDecoder};
use super::{Category, Failure, MAX_RESPONSE_BYTES};

fn parse(source: &str) -> Result<Value, Error> {
    json::parse(&Text::from(source))
}

fn request(streaming: bool) -> Value {
    Value::object(vec![
        ("stream", Value::Boolean(streaming)),
        (
            "messages",
            Value::Array(vec![Value::object(vec![
                ("role", Value::string("user")),
                ("content", Value::string("fixture")),
            ])]),
        ),
    ])
}

fn client(settings: &str) -> Result<Chat, Error> {
    let mut llm =
        parse(r#"{"base_url":"http://127.0.0.1:8000/v1","model":"m","api_key_optional":true}"#)?;
    let overlay = parse(settings)?;
    if let Value::Object(entries) = overlay {
        for (key, value) in entries {
            domain::put(&mut llm, &key.to_utf8()?, value)?;
        }
    }
    Chat::new(&Value::object(vec![("llm", llm)]))?
        .ok_or_else(|| Error::new("Error", "Fixture chat was unexpectedly disabled"))
}

fn completion() -> Result<Value, Error> {
    parse(
        r#"{"model":"m","choices":[{"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}]}"#,
    )
}

fn frame(chunk: &Value) -> Vec<u8> {
    format!("data: {}\r\n\r\n", chunk.json()).into_bytes()
}

#[derive(Default)]
struct Counts {
    active: Cell<usize>,
    closed: Cell<usize>,
}

struct InFlight(Rc<Counts>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.active.set(self.0.active.get().saturating_sub(1));
    }
}

struct Packet {
    status: u16,
    headers_delay: u64,
    json: bool,
    parts: VecDeque<(Vec<u8>, u64)>,
    hang_headers: bool,
    hang_body: bool,
    network_error: bool,
    read_error: bool,
}

impl Packet {
    fn json(value: &Value) -> Self {
        Self {
            status: 200,
            headers_delay: 0,
            json: true,
            parts: [(value.json().into_bytes(), 0)].into(),
            hang_headers: false,
            hang_body: false,
            network_error: false,
            read_error: false,
        }
    }

    fn stream(parts: Vec<(Vec<u8>, u64)>, hang: bool) -> Self {
        Self {
            json: false,
            parts: parts.into(),
            hang_body: hang,
            ..Self::json(&Value::Null)
        }
    }
}

struct FixtureBody {
    packet: Packet,
    counts: Rc<Counts>,
}

impl Drop for FixtureBody {
    fn drop(&mut self) {
        self.counts.closed.set(self.counts.closed.get() + 1);
    }
}

impl Body for FixtureBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if let Some((bytes, delay)) = self.packet.parts.pop_front() {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            return Ok(Some(bytes));
        }
        if self.packet.read_error {
            return Err(Error::new("Error", "PRIVATE upstream read failure"));
        }
        if self.packet.hang_body {
            return std::future::pending().await;
        }
        Ok(None)
    }
}

struct Fixture {
    packets: VecDeque<Packet>,
    sent: Vec<Vec<u8>>,
    sent_headers: Vec<Vec<(String, String)>>,
    counts: Rc<Counts>,
}

impl Fixture {
    fn new(packets: Vec<Packet>) -> Self {
        Self {
            packets: packets.into(),
            sent: Vec::new(),
            sent_headers: Vec::new(),
            counts: Rc::default(),
        }
    }
}

impl Transport for Fixture {
    type Body = FixtureBody;

    async fn send(&mut self, request: &Request) -> Result<Response<FixtureBody>, Failure> {
        self.sent.push(request.body.clone());
        self.sent_headers.push(request.headers.clone());
        self.counts.active.set(self.counts.active.get() + 1);
        let _guard = InFlight(self.counts.clone());
        let packet = self.packets.pop_front().ok_or_else(Failure::network)?;
        if packet.hang_headers {
            return std::future::pending().await;
        }
        if packet.headers_delay > 0 {
            tokio::time::sleep(Duration::from_millis(packet.headers_delay)).await;
        }
        if packet.network_error {
            return Err(Failure::network());
        }
        Ok(Response {
            status: packet.status,
            headers: vec![
                (
                    "content-type".to_owned(),
                    if packet.json {
                        "application/json"
                    } else {
                        "text/event-stream"
                    }
                    .to_owned(),
                ),
                ("retry-after".to_owned(), "0".to_owned()),
            ],
            body: Some(FixtureBody {
                packet,
                counts: self.counts.clone(),
            }),
        })
    }
}

#[derive(Default)]
struct Events {
    values: Vec<Value>,
    fail: Option<&'static str>,
    hang: Option<&'static str>,
    cancel: Option<(CancellationSource, &'static str)>,
}

impl Observer for Events {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        let matches =
            |kind: Option<&str>| kind.is_some_and(|kind| domain::is_text(event.get("type"), kind));
        let fail = matches(self.fail);
        let hang = matches(self.hang);
        let cancel = self.cancel.as_ref().filter(|(_, kind)| matches(Some(kind)));
        self.values.push(event);
        if let Some((source, _)) = cancel {
            source.cancel(Reason::Cancelled)?;
        }
        if fail {
            return Err(Error::new("Error", "Fixture observer failed").coded("ROSTER_RUN_LOG"));
        }
        if hang {
            return std::future::pending().await;
        }
        Ok(())
    }
}

fn failure<T>(result: Result<T, Failure>) -> Result<Failure, Error> {
    match result {
        Err(error) => Ok(error),
        Ok(_) => Err(Error::new("Error", "Fixture expected a transport failure")),
    }
}

#[test]
fn fragmented_tools_and_utf16_surrogates_are_assembled_without_loss() -> Result<(), Error> {
    let mut assembler = CompletionAssembler::new(Text::from("requested"));
    for source in [
        r#"{"model":"served","choices":[{"delta":{"role":"assistant","content":"\ud800","tool_calls":[{"index":4,"id":"b","function":{"name":"wri","arguments":"{\"p\":\""}}]}}]}"#,
        r#"{"choices":[{"delta":{"content":"\udc00","tool_calls":[{"index":1,"id":"a","function":{"name":"read","arguments":"{}"}},{"index":4,"function":{"name":"te","arguments":"\udfff\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":0,"completion_tokens":4}}"#,
    ] {
        assembler.push(&parse(source)?);
    }
    let payload = assembler.payload();
    let result =
        parse_completion(&payload, &Text::from("requested")).map_err(|error| *error.error)?;
    assert_eq!(result.model, Text::from("served"));
    assert_eq!(
        result.message.get("content"),
        Some(&Value::String(Text(vec![0xd800, 0xdc00])))
    );
    let calls = result
        .message
        .get("tool_calls")
        .and_then(domain::items)
        .ok_or_else(|| Error::new("Error", "Missing assembled tools"))?;
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls.first().and_then(|call| call.get("id")),
        Some(&Value::string("a"))
    );
    assert_eq!(
        calls
            .get(1)
            .and_then(|call| call.get("function"))
            .and_then(|function| function.get("name")),
        Some(&Value::string("write"))
    );
    assert_eq!(result.measured_usage()?.prompt_tokens, Some(0));
    Ok(())
}

#[test]
fn every_utf8_crlf_split_and_bom_boundary_preserves_completion() -> Result<(), Error> {
    let source = "\u{feff}data: {\"model\":\"m\",\"choices\":[{\"delta\":{\"content\":\"😀雪\"},\"finish_reason\":\"stop\"}]}\r\n\r\ndata: [DONE]\r\n\r\n";
    let bytes = source.as_bytes();
    for width in 1..=bytes.len() {
        let mut stream = StreamDecoder::new(Text::from("m"));
        for part in bytes.chunks(width) {
            stream.push(part).map_err(|failure| *failure.error)?;
            if stream.done() {
                break;
            }
        }
        let (payload, _) = stream.finish().map_err(|failure| *failure.error)?;
        let parsed =
            parse_completion(&payload, &Text::from("m")).map_err(|failure| *failure.error)?;
        assert_eq!(
            parsed.message.get("content"),
            Some(&Value::string("😀雪")),
            "width {width}"
        );
    }
    Ok(())
}

#[test]
fn stream_bound_counts_utf8_bytes_and_rejects_unterminated_overflow() -> Result<(), Error> {
    let mut stream = StreamDecoder::new(Text::from("m"));
    stream
        .push(&vec![b':'; MAX_RESPONSE_BYTES])
        .map_err(|failure| *failure.error)?;
    let error = failure(stream.push(&[0]))?;
    assert_eq!(
        error.error.message,
        "The LLM stream exceeded the supported size."
    );
    Ok(())
}

#[test]
fn empty_stream_and_gateway_error_are_not_successful_empty_answers() -> Result<(), Error> {
    for (source, transient) in [
        (": keep-alive\n\n", false),
        ("{\"error\":null}", true),
        ("{\"error\":{\"message\":\"PRIVATE body\"}}", true),
    ] {
        let mut stream = StreamDecoder::new(Text::from("m"));
        stream
            .push(source.as_bytes())
            .map_err(|failure| *failure.error)?;
        let error = failure(stream.finish())?;
        assert_eq!(error.transient, transient);
        assert!(!error.error.message.contains("PRIVATE"));
    }
    Ok(())
}

#[test]
fn invalid_usage_models_and_finish_reason_shapes_fail_explicitly() -> Result<(), Error> {
    for source in [
        r#"{"choices":[{"message":{"role":"assistant"}}]}"#,
        r#"{"choices":[{"message":{"role":"assistant","content":""},"finish_reason":1}]}"#,
        r#"{"choices":[{"message":{"role":"assistant","content":null}}],"usage":{"prompt_tokens":null}}"#,
        r#"{"choices":[{"message":{"role":"assistant","content":""}}],"usage":{"total_tokens":9007199254740992}}"#,
        r#"{"choices":[{"message":{"role":"assistant","content":""}}],"model":" m "}"#,
        r#"{"choices":[{"message":{"role":"assistant","content":""}}],"model":7}"#,
    ] {
        assert!(parse_completion(&parse(source)?, &Text::from("m")).is_err());
    }
    let payload = parse(
        r#"{"choices":[{"message":{"role":"assistant","content":"","reasoning_content":"PRIVATE"}}]}"#,
    )?;
    let parsed = parse_completion(&payload, &Text::from("m")).map_err(|failure| *failure.error)?;
    assert!(parsed.usage.is_none());
    assert_eq!(parsed.measured_usage()?.prompt_tokens, None);
    assert!(parsed.message.get("reasoning_content").is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn fifteen_minute_local_headers_have_no_hidden_five_minute_deadline() -> Result<(), Error> {
    let chat = client("{}")?;
    let mut packet = Packet::json(&completion()?);
    packet.headers_delay = 900_000;
    let mut fixture = Fixture::new(vec![packet]);
    let mut events = Events::default();
    chat.complete(
        &request(true),
        &mut fixture,
        &mut events,
        &Credentials::default(),
        CallOptions::default(),
    )
    .await
    .map_err(|failure| *failure.error)?;
    assert_eq!(fixture.sent.len(), 1);
    assert_eq!(
        events
            .values
            .iter()
            .filter(|event| domain::is_text(event.get("type"), "waiting"))
            .count(),
        30
    );
    assert!(
        !events
            .values
            .iter()
            .any(|event| domain::is_text(event.get("type"), "stall"))
    );
    assert_eq!(fixture.counts.active.get(), 0);
    assert_eq!(fixture.counts.closed.get(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cloud_timeout_is_endpoint_not_task_and_drops_the_header_future() -> Result<(), Error> {
    let chat = client(r#"{"base_url":"https://fixture.invalid/v1"}"#)?;
    let mut packet = Packet::json(&completion()?);
    packet.hang_headers = true;
    let mut fixture = Fixture::new(vec![packet]);
    let mut events = Events::default();
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut events,
            &Credentials::default(),
            CallOptions {
                retry_command: Some("roster doctor --warm"),
                ..CallOptions::default()
            },
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_TIMEOUT"));
    assert_eq!(error.route_failure(), Some("endpoint-timeout"));
    assert!(!error.error.message.contains("Cold-start"));
    assert!(error.error.message.contains("Retry: roster doctor --warm"));
    assert_eq!(fixture.counts.active.get(), 0);
    assert!(chat.last_response()?.is_none());
    assert_eq!(
        events
            .values
            .iter()
            .filter(|event| domain::is_text(event.get("type"), "waiting"))
            .count(),
        3
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn json_body_on_stream_request_uses_total_budget_not_idle_watchdog() -> Result<(), Error> {
    let chat = client(r#"{"request_timeout_ms":50,"timeout_ms":1,"stream_idle_timeout_ms":5}"#)?;
    let mut packet = Packet::json(&completion()?);
    packet.parts.clear();
    packet.hang_body = true;
    let mut fixture = Fixture::new(vec![packet]);
    let error = failure(
        chat.complete(
            &request(true),
            &mut fixture,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_TIMEOUT"));
    assert_eq!(fixture.sent.len(), 1);
    assert_eq!(fixture.counts.closed.get(), 1);
    assert!(error.error.message.contains("0.05s"));
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn silent_stream_is_retried_once_and_second_stall_is_explicit() -> Result<(), Error> {
    let chat = client(r#"{"request_timeout_ms":1000,"stream_idle_timeout_ms":50}"#)?;
    let mut fixture = Fixture::new(vec![
        Packet::stream(vec![], true),
        Packet::stream(vec![], true),
    ]);
    let mut events = Events::default();
    let error = failure(
        chat.complete(
            &request(true),
            &mut fixture,
            &mut events,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_STALL"));
    assert_eq!(error.category, Some(Category::Timeout));
    assert_eq!(fixture.sent.len(), 2);
    assert_eq!(fixture.sent.first(), fixture.sent.get(1));
    assert_eq!(fixture.counts.closed.get(), 2);
    let retries = events
        .values
        .iter()
        .filter(|event| domain::is_text(event.get("type"), "stall"))
        .filter_map(|event| event.get("retry"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(retries, [Value::Boolean(true), Value::Boolean(false)]);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn byte_liveness_and_slow_headers_do_not_trigger_idle_stalls() -> Result<(), Error> {
    let chat = client(r#"{"request_timeout_ms":1000,"stream_idle_timeout_ms":40}"#)?;
    let mut parts = (0..5)
        .map(|_| (b": ping\r\n\r\n".to_vec(), 30))
        .collect::<Vec<_>>();
    parts.push((
        frame(&parse(
            r#"{"choices":[{"delta":{"content":"pong"},"finish_reason":"stop"}]}"#,
        )?),
        30,
    ));
    parts.push((b"data: [DONE]\r\n\r\n".to_vec(), 30));
    let mut packet = Packet::stream(parts, true);
    packet.headers_delay = 150;
    let mut fixture = Fixture::new(vec![packet]);
    let mut events = Events::default();
    let answer = chat
        .complete(
            &request(true),
            &mut fixture,
            &mut events,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await
        .map_err(|failure| *failure.error)?;
    assert_eq!(answer.message.get("content"), Some(&Value::string("pong")));
    assert_eq!(fixture.sent.len(), 1);
    assert_eq!(fixture.counts.closed.get(), 1);
    assert!(
        !events
            .values
            .iter()
            .any(|event| domain::is_text(event.get("type"), "stall"))
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn transient_http_retry_uses_identical_payload_and_shared_send_ceiling() -> Result<(), Error>
{
    for status in [429, 502, 503, 504] {
        let chat = client(r#"{"max_requests":2}"#)?;
        let mut packet = Packet::json(&Value::Null);
        packet.status = status;
        let mut fixture = Fixture::new(vec![packet, Packet::json(&completion()?)]);
        chat.complete(
            &request(false),
            &mut fixture,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await
        .map_err(|failure| *failure.error)?;
        assert_eq!(fixture.sent.len(), 2);
        assert_eq!(fixture.sent.first(), fixture.sent.get(1));
        assert_eq!(fixture.counts.closed.get(), 2);
        let error = failure(
            chat.complete(
                &request(false),
                &mut fixture,
                &mut NoObserver,
                &Credentials::default(),
                CallOptions::default(),
            )
            .await,
        )?;
        assert_eq!(error.error.code, Some("ROSTER_MODEL_REQUEST_LIMIT"));
        assert_eq!(error.route_failure(), None);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn http_and_stall_retries_cannot_exceed_the_context_request_limit() -> Result<(), Error> {
    for streaming in [false, true] {
        let chat = client(r#"{"max_requests":1,"stream_idle_timeout_ms":10}"#)?;
        let mut packet = if streaming {
            Packet::stream(vec![], true)
        } else {
            Packet::json(&Value::Null)
        };
        if !streaming {
            packet.status = 503;
        }
        let mut fixture = Fixture::new(vec![packet]);
        let error = failure(
            chat.complete(
                &request(streaming),
                &mut fixture,
                &mut NoObserver,
                &Credentials::default(),
                CallOptions::default(),
            )
            .await,
        )?;
        assert_eq!(error.error.code, Some("ROSTER_MODEL_REQUEST_LIMIT"));
        assert_eq!(fixture.sent.len(), 1);
        assert_eq!(chat.request_count(), 1);
        assert!(chat.last_response()?.is_none());
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn retry_waits_are_inside_one_deadline_and_never_start_a_late_send() -> Result<(), Error> {
    let chat = client(r#"{"request_timeout_ms":25}"#)?;
    struct SlowRetry(Fixture);
    impl Transport for SlowRetry {
        type Body = FixtureBody;
        async fn send(&mut self, request: &Request) -> Result<Response<FixtureBody>, Failure> {
            let mut response = self.0.send(request).await?;
            response
                .headers
                .push(("Retry-After".to_owned(), "3600".to_owned()));
            response.headers.retain(|(name, _)| name != "retry-after");
            Ok(response)
        }
    }
    let mut packet = Packet::json(&Value::Null);
    packet.status = 429;
    let mut fixture = SlowRetry(Fixture::new(vec![packet]));
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_TIMEOUT"));
    assert_eq!(fixture.0.sent.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn gateway_json_error_on_sse_is_retried_without_leaking_body() -> Result<(), Error> {
    let chat = client("{}")?;
    let error_body = br#"{"error":{"message":"PRIVATE backend failure"}}"#.to_vec();
    let done = frame(&parse(
        r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
    )?);
    let mut fixture = Fixture::new(vec![
        Packet::stream(vec![(error_body, 0)], false),
        Packet::stream(vec![(done, 0)], false),
    ]);
    let mut events = Events::default();
    chat.complete(
        &request(true),
        &mut fixture,
        &mut events,
        &Credentials::default(),
        CallOptions::default(),
    )
    .await
    .map_err(|failure| *failure.error)?;
    assert_eq!(fixture.sent.len(), 2);
    assert!(!Value::Array(events.values).json().contains("PRIVATE"));
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_during_headers_drops_owned_work_without_accepting_late_success()
-> Result<(), Error> {
    let chat = client("{}")?;
    let mut packet = Packet::json(&completion()?);
    packet.headers_delay = 100;
    let mut fixture = Fixture::new(vec![packet]);
    let source = CancellationSource::default();
    let token = source.token();
    let mut observer = NoObserver;
    let credentials = Credentials::default();
    let request = request(false);
    let (result, cancelled) = tokio::join!(
        chat.complete(
            &request,
            &mut fixture,
            &mut observer,
            &credentials,
            CallOptions {
                parent: Some(&token),
                ..CallOptions::default()
            }
        ),
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            source.cancel(Reason::Cancelled)
        }
    );
    cancelled?;
    let error = failure(result)?;
    assert_eq!(error.error.code, Some("ROSTER_CANCELLED"));
    assert_eq!(error.route_failure(), None);
    assert_eq!(fixture.counts.active.get(), 0);
    assert!(chat.last_response()?.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn parent_task_timeout_is_never_misclassified_as_an_endpoint_timeout() -> Result<(), Error> {
    let chat = client("{}")?;
    let source = CancellationSource::default();
    let token = source.token();
    let mut fixture = Fixture::new(vec![Packet::stream(vec![], true)]);
    let credentials = Credentials::default();
    let mut observer = NoObserver;
    let request = request(true);
    let (result, cancelled) = tokio::join!(
        chat.complete(
            &request,
            &mut fixture,
            &mut observer,
            &credentials,
            CallOptions {
                parent: Some(&token),
                ..CallOptions::default()
            }
        ),
        async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            source.cancel(Reason::TaskTimeout(5))
        }
    );
    cancelled?;
    let error = failure(result)?;
    assert_eq!(error.error.code, Some("ROSTER_TASK_TIMEOUT"));
    assert!(!error.is_timeout());
    assert_eq!(error.route_failure(), None);
    assert_eq!(fixture.counts.closed.get(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancelled_and_invalid_requests_do_not_spend_a_send() -> Result<(), Error> {
    let chat = client(r#"{"max_requests":1}"#)?;
    let mut fixture = Fixture::new(vec![Packet::json(&completion()?)]);
    let source = CancellationSource::default();
    source.cancel(Reason::Cancelled)?;
    let token = source.token();
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions {
                signal: Some(&token),
                ..CallOptions::default()
            },
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_CANCELLED"));
    assert!(
        chat.complete(
            &Value::Null,
            &mut fixture,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions::default()
        )
        .await
        .is_err()
    );
    assert_eq!(chat.request_count(), 0);
    assert!(fixture.sent.is_empty());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn logger_failures_abort_without_network_relabeling_or_a_send() -> Result<(), Error> {
    let chat = client("{}")?;
    let mut fixture = Fixture::new(vec![Packet::json(&completion()?)]);
    let mut observer = Events {
        fail: Some("model"),
        ..Events::default()
    };
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut observer,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_RUN_LOG"));
    assert_eq!(error.category, None);
    assert!(fixture.sent.is_empty());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn hanging_waiting_observer_cannot_extend_or_detach_the_request() -> Result<(), Error> {
    let chat = client("{}")?;
    let mut packet = Packet::json(&completion()?);
    packet.hang_headers = true;
    let mut fixture = Fixture::new(vec![packet]);
    let mut observer = Events {
        hang: Some("waiting"),
        ..Events::default()
    };
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut observer,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_TIMEOUT"));
    assert_eq!(fixture.counts.active.get(), 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_inside_usage_observer_revokes_the_response_snapshot() -> Result<(), Error> {
    let chat = client("{}")?;
    let source = CancellationSource::default();
    let token = source.token();
    let mut payload = completion()?;
    domain::put(
        &mut payload,
        "usage",
        parse(r#"{"prompt_tokens":3,"completion_tokens":2}"#)?,
    )?;
    let mut fixture = Fixture::new(vec![Packet::json(&payload)]);
    let mut observer = Events {
        cancel: Some((source, "usage")),
        ..Events::default()
    };
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut observer,
            &Credentials::default(),
            CallOptions {
                parent: Some(&token),
                ..CallOptions::default()
            },
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_CANCELLED"));
    assert!(chat.last_response()?.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn concurrent_callers_cannot_overrun_a_shared_context_budget() -> Result<(), Error> {
    let chat = client(r#"{"max_requests":2}"#)?;
    let packet = || -> Result<Packet, Error> {
        let mut packet = Packet::json(&completion()?);
        packet.headers_delay = 5;
        Ok(packet)
    };
    let mut first = Fixture::new(vec![packet()?]);
    let mut second = Fixture::new(vec![packet()?]);
    let mut third = Fixture::new(vec![packet()?]);
    let mut fourth = Fixture::new(vec![packet()?]);
    let mut fifth = Fixture::new(vec![packet()?]);
    let mut a = NoObserver;
    let mut b = NoObserver;
    let mut c = NoObserver;
    let mut d = NoObserver;
    let mut e = NoObserver;
    let request = request(false);
    let credentials = Credentials::default();
    let results = tokio::join!(
        chat.complete(
            &request,
            &mut first,
            &mut a,
            &credentials,
            CallOptions::default()
        ),
        chat.complete(
            &request,
            &mut second,
            &mut b,
            &credentials,
            CallOptions::default()
        ),
        chat.complete(
            &request,
            &mut third,
            &mut c,
            &credentials,
            CallOptions::default()
        ),
        chat.complete(
            &request,
            &mut fourth,
            &mut d,
            &credentials,
            CallOptions::default()
        ),
        chat.complete(
            &request,
            &mut fifth,
            &mut e,
            &credentials,
            CallOptions::default()
        ),
    );
    let results = [results.0, results.1, results.2, results.3, results.4];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 2);
    assert!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|failure| failure.error.code == Some("ROSTER_MODEL_REQUEST_LIMIT"))
    );
    assert_eq!(chat.request_count(), 2);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn length_is_not_retried_in_transport_and_preserves_only_measured_evidence()
-> Result<(), Error> {
    let chat = client("{}")?;
    let mut payload = completion()?;
    if let Some(Value::Array(choices)) = payload.get("choices").cloned() {
        let mut choices = choices;
        if let Some(choice) = choices.first_mut() {
            domain::put(choice, "finish_reason", Value::string("length"))?;
        }
        domain::put(&mut payload, "choices", Value::Array(choices))?;
    }
    domain::put(
        &mut payload,
        "usage",
        parse(r#"{"prompt_tokens":0,"private":"PRIVATE"}"#)?,
    )?;
    let mut fixture = Fixture::new(vec![Packet::json(&payload)]);
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_FINISH_REASON"));
    assert_eq!(error.truncated, Some(true));
    assert_eq!(error.route_failure(), None);
    assert!(error.partial.is_some());
    assert_eq!(fixture.sent.len(), 1);
    let snapshot = chat
        .last_response()?
        .ok_or_else(|| Error::new("Error", "Missing measured snapshot"))?;
    assert_eq!(
        snapshot.usage.map(|usage| usage.prompt_tokens),
        Some(Some(0))
    );
    assert_eq!(
        snapshot.usage.and_then(|usage| usage.completion_tokens),
        None
    );
    assert!(!snapshot.as_value().json().contains("PRIVATE"));
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn bounded_json_body_rejects_oversize_before_parsing_or_snapshot() -> Result<(), Error> {
    let chat = client("{}")?;
    let mut packet = Packet::json(&Value::Null);
    packet.parts = [(vec![b' '; MAX_RESPONSE_BYTES + 1], 0)].into();
    let mut fixture = Fixture::new(vec![packet]);
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(
        error.error.message,
        "The LLM response exceeded the supported size."
    );
    assert_eq!(fixture.counts.closed.get(), 1);
    assert!(chat.last_response()?.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn read_failures_are_not_empty_completions_or_retried_task_failures() -> Result<(), Error> {
    for streaming in [false, true] {
        let chat = client("{}")?;
        let mut packet = if streaming {
            Packet::stream(vec![], false)
        } else {
            Packet::json(&Value::Null)
        };
        packet.parts.clear();
        packet.read_error = true;
        let mut fixture = Fixture::new(vec![packet]);
        let error = failure(
            chat.complete(
                &request(streaming),
                &mut fixture,
                &mut NoObserver,
                &Credentials::default(),
                CallOptions::default(),
            )
            .await,
        )?;
        assert_eq!(error.category, Some(Category::Response));
        assert_eq!(error.route_failure(), None);
        assert!(!error.error.message.contains("PRIVATE"));
        assert_eq!(fixture.sent.len(), 1);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn hanging_terminal_observer_cannot_outlive_an_expired_request() -> Result<(), Error> {
    let chat = client(r#"{"request_timeout_ms":10}"#)?;
    let mut packet = Packet::json(&completion()?);
    packet.hang_headers = true;
    let mut fixture = Fixture::new(vec![packet]);
    let mut observer = Events {
        hang: Some("timeout"),
        ..Events::default()
    };
    let started = tokio::time::Instant::now();
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut observer,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_TIMEOUT"));
    assert_eq!(started.elapsed(), Duration::from_millis(10));
    assert_eq!(fixture.counts.active.get(), 0);
    assert!(chat.last_response()?.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn authentication_observer_is_cancellable_and_bounded_before_any_send() -> Result<(), Error> {
    let chat = client(r#"{"request_timeout_ms":10,"api_key_optional":false}"#)?;
    let mut fixture = Fixture::new(Vec::new());
    let mut observer = Events {
        hang: Some("http"),
        ..Events::default()
    };
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut observer,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_TIMEOUT"));
    assert!(fixture.sent.is_empty());
    assert_eq!(chat.request_count(), 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_while_reporting_a_served_label_does_not_accept_late_usage()
-> Result<(), Error> {
    let chat = client("{}")?;
    let source = CancellationSource::default();
    let token = source.token();
    let mut payload = completion()?;
    domain::put(&mut payload, "model", Value::string("unrelated"))?;
    domain::put(
        &mut payload,
        "usage",
        parse(r#"{"prompt_tokens":3,"completion_tokens":2}"#)?,
    )?;
    let mut fixture = Fixture::new(vec![Packet::json(&payload)]);
    let mut observer = Events {
        cancel: Some((source, "served-model")),
        ..Events::default()
    };
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut observer,
            &Credentials::default(),
            CallOptions {
                parent: Some(&token),
                ..CallOptions::default()
            },
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_CANCELLED"));
    assert_eq!(error.route_failure(), None);
    assert!(chat.last_response()?.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn zero_retry_after_keeps_nodes_one_millisecond_timer_floor_inside_the_deadline()
-> Result<(), Error> {
    let chat = client(r#"{"request_timeout_ms":1}"#)?;
    let mut packet = Packet::json(&Value::Null);
    packet.status = 429;
    let mut fixture = Fixture::new(vec![packet, Packet::json(&completion()?)]);
    let error = failure(
        chat.complete(
            &request(false),
            &mut fixture,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_TIMEOUT"));
    assert_eq!(fixture.sent.len(), 1);
    assert_eq!(chat.request_count(), 1);
    assert!(chat.last_response()?.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn request_bound_counts_serialized_surrogate_bytes_before_spending_a_send()
-> Result<(), Error> {
    let chat = client(r#"{"max_requests":1}"#)?;
    let oversized = Value::object(vec![(
        "messages",
        Value::Array(vec![Value::object(vec![
            ("role", Value::string("user")),
            (
                "content",
                Value::String(Text(vec![0xd800; MAX_RESPONSE_BYTES / 6 + 1])),
            ),
        ])]),
    )]);
    let mut fixture = Fixture::new(vec![Packet::json(&completion()?)]);
    let error = failure(
        chat.complete(
            &oversized,
            &mut fixture,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.name, "TypeError");
    assert_eq!(
        error.error.message,
        "The LLM request exceeded the supported size."
    );
    assert_eq!(chat.request_count(), 0);
    assert!(fixture.sent.is_empty());
    assert!(chat.last_response()?.is_none());
    chat.complete(
        &request(false),
        &mut fixture,
        &mut NoObserver,
        &Credentials::default(),
        CallOptions::default(),
    )
    .await
    .map_err(|failure| *failure.error)?;
    assert_eq!(chat.request_count(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn vault_mapping_invalid_shapes_fail_before_any_send() -> Result<(), Error> {
    let sentinel = "fixture-invalid-vault-mapping-value";
    for invalid in [
        Value::Null,
        Value::Boolean(false),
        Value::Number(7.0),
        Value::string(sentinel),
        Value::Array(vec![Value::string(sentinel)]),
    ] {
        for environment in [
            Value::object(Vec::new()),
            Value::object(vec![("SEAT_KEY", Value::string(""))]),
        ] {
            for optional in [false, true] {
                let chat = client(&format!(
                    r#"{{"api_key_name":"SEAT_KEY","api_key_optional":{optional}}}"#
                ))?;
                let credentials = Credentials {
                    environment: Some(environment.clone()),
                    vault_entries: Some(invalid.clone()),
                };
                let mut fixture = Fixture::new(vec![Packet::json(&completion()?)]);
                let error = failure(
                    chat.complete(
                        &request(false),
                        &mut fixture,
                        &mut NoObserver,
                        &credentials,
                        CallOptions::default(),
                    )
                    .await,
                )?;
                assert_eq!(error.error.code, Some("ROSTER_SECRET_RESOLUTION"));
                assert!(error.error.cause.is_none());
                assert!(!format!("{error:?}").contains(sentinel));
                assert!(!error.transient);
                assert!(error.route_failure().is_none());
                assert_eq!(chat.request_count(), 0);
                assert!(fixture.sent.is_empty());
                assert!(fixture.sent_headers.is_empty());
                assert!(chat.last_response()?.is_none());
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn vault_mapping_is_not_validated_when_environment_wins() -> Result<(), Error> {
    let selected = "fixture-environment-token";
    for invalid in [
        Value::Null,
        Value::Boolean(false),
        Value::Number(7.0),
        Value::string("fixture-unused-vault-value"),
        Value::Array(Vec::new()),
    ] {
        let chat = client(r#"{"api_key_name":"SEAT_KEY","api_key_optional":false}"#)?;
        let credentials = Credentials {
            environment: Some(Value::object(vec![("SEAT_KEY", Value::string(selected))])),
            vault_entries: Some(invalid),
        };
        let mut fixture = Fixture::new(vec![Packet::json(&completion()?)]);
        chat.complete(
            &request(false),
            &mut fixture,
            &mut NoObserver,
            &credentials,
            CallOptions::default(),
        )
        .await
        .map_err(|failure| *failure.error)?;
        assert_eq!(chat.request_count(), 1);
        assert_eq!(fixture.sent.len(), 1);
        assert_eq!(fixture.sent_headers.len(), 1);
        assert!(fixture.sent_headers.first().is_some_and(|headers| {
            headers.iter().any(|(name, value)| {
                name == "Authorization" && value == &format!("Bearer {selected}")
            })
        }));
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn vault_mapping_preserves_valid_lookup_and_optional_absence() -> Result<(), Error> {
    let selected = "fixture-stored-token";
    for environment in [
        Value::object(Vec::new()),
        Value::object(vec![("SEAT_KEY", Value::string(""))]),
    ] {
        let chat = client(r#"{"api_key_name":"SEAT_KEY","api_key_optional":false}"#)?;
        let credentials = Credentials {
            environment: Some(environment),
            vault_entries: Some(Value::object(vec![("SEAT_KEY", Value::string(selected))])),
        };
        let mut fixture = Fixture::new(vec![Packet::json(&completion()?)]);
        chat.complete(
            &request(false),
            &mut fixture,
            &mut NoObserver,
            &credentials,
            CallOptions::default(),
        )
        .await
        .map_err(|failure| *failure.error)?;
        assert_eq!(chat.request_count(), 1);
        assert_eq!(fixture.sent.len(), 1);
        assert!(fixture.sent_headers.first().is_some_and(|headers| {
            headers.iter().any(|(name, value)| {
                name == "Authorization" && value == &format!("Bearer {selected}")
            })
        }));
    }
    for vault_entries in [None, Some(Value::object(Vec::new()))] {
        for optional in [false, true] {
            let chat = client(&format!(
                r#"{{"api_key_name":"SEAT_KEY","api_key_optional":{optional}}}"#
            ))?;
            let credentials = Credentials {
                environment: None,
                vault_entries: vault_entries.clone(),
            };
            let mut fixture = Fixture::new(vec![Packet::json(&completion()?)]);
            let result = chat
                .complete(
                    &request(false),
                    &mut fixture,
                    &mut NoObserver,
                    &credentials,
                    CallOptions::default(),
                )
                .await;
            if optional {
                result.map_err(|failure| *failure.error)?;
                assert_eq!(chat.request_count(), 1);
                assert_eq!(fixture.sent.len(), 1);
                assert!(fixture.sent_headers.first().is_some_and(|headers| {
                    headers.iter().all(|(name, _)| name != "Authorization")
                }));
            } else {
                let error = failure(result)?;
                assert_eq!(error.category, Some(Category::Authentication));
                assert_eq!(chat.request_count(), 0);
                assert!(fixture.sent.is_empty());
            }
        }
    }
    Ok(())
}
