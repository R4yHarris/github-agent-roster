use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::Error;
use crate::cancel::{CancellationSource, Reason};
use crate::domain::{self, put};
use crate::json::{self, Value};
use crate::text::Text;

use super::Failure;
use super::admission::Admission;
use super::baseline;
use super::builtin::{self, BuiltinChat, Options};
use super::control::{NoObserver, Observer};
use super::http::{Body, Request, Response, Transport};
use super::openai::{CallOptions, Completion, Credentials};

#[path = "admission_tests.rs"]
mod admission_cases;

fn parse(source: &str) -> Result<Value, Error> {
    json::parse(&Text::from(source))
}

fn config(settings: &str) -> Result<Value, Error> {
    let mut llm = parse(r#"{"base_url":"http://127.0.0.1:8000/v1","model":"requested"}"#)?;
    let settings = parse(settings)?;
    for (name, value) in domain::members(&settings)
        .ok_or_else(|| Error::type_error("Fixture settings must be an object"))?
    {
        put(&mut llm, &name.to_utf8()?, value.clone())?;
    }
    Ok(Value::object(vec![("llm", llm)]))
}

fn request(content: &str) -> Value {
    Value::object(vec![(
        "messages",
        Value::Array(vec![Value::object(vec![
            ("role", Value::string("user")),
            ("content", Value::string(content)),
        ])]),
    )])
}

fn packet(reason: &str, content: &str, usage: Option<Value>) -> Packet {
    Packet::json(Value::object(vec![
        ("model", Value::string("actual-served")),
        ("usage", usage.unwrap_or(Value::Null)),
        (
            "choices",
            Value::Array(vec![Value::object(vec![
                ("finish_reason", Value::string(reason)),
                (
                    "message",
                    Value::object(vec![
                        ("role", Value::string("assistant")),
                        ("content", Value::string(content)),
                    ]),
                ),
            ])]),
        ),
    ]))
}

#[derive(Default)]
struct Gate {
    open: Cell<bool>,
    changed: Notify,
}

impl Gate {
    fn open(&self) {
        self.open.set(true);
        self.changed.notify_waiters();
    }

    async fn wait(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.open.get() {
                return;
            }
            changed.await;
        }
    }
}

struct Packet {
    bytes: Vec<u8>,
    status: u16,
    stream: bool,
    network_error: bool,
    gate: Option<Rc<Gate>>,
}

impl Packet {
    fn json(value: Value) -> Self {
        Self {
            bytes: value.json().into_bytes(),
            status: 200,
            stream: false,
            network_error: false,
            gate: None,
        }
    }

    fn gated(gate: &Rc<Gate>) -> Self {
        Self {
            gate: Some(gate.clone()),
            ..packet("stop", "ok", None)
        }
    }
}

#[derive(Clone)]
struct Seen {
    body: Value,
    headers: Vec<(String, String)>,
}

#[derive(Default)]
struct Trace {
    sent: RefCell<Vec<Seen>>,
    active: Cell<usize>,
    peak: Cell<usize>,
    closed: Cell<usize>,
}

impl Trace {
    fn bodies(&self) -> Vec<Value> {
        self.sent
            .borrow()
            .iter()
            .map(|seen| seen.body.clone())
            .collect()
    }

    fn order(&self) -> Vec<String> {
        self.sent
            .borrow()
            .iter()
            .filter_map(|seen| {
                seen.body
                    .get("messages")
                    .and_then(domain::items)
                    .and_then(|messages| messages.last())
                    .and_then(|message| message.get("content"))
                    .and_then(Value::as_string)
                    .and_then(|content| content.to_utf8().ok())
            })
            .collect()
    }
}

struct InFlight(Rc<Trace>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.active.set(self.0.active.get().saturating_sub(1));
        self.0.closed.set(self.0.closed.get() + 1);
    }
}

struct RecordedBody {
    bytes: Option<Vec<u8>>,
    gate: Option<Rc<Gate>>,
    _guard: InFlight,
}

impl Body for RecordedBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if let Some(gate) = self.gate.take() {
            gate.wait().await;
        }
        Ok(self.bytes.take())
    }
}

struct Recorded {
    packets: VecDeque<Packet>,
    trace: Rc<Trace>,
}

impl Recorded {
    fn new(packets: Vec<Packet>) -> Self {
        Self {
            packets: packets.into(),
            trace: Rc::default(),
        }
    }

    fn shared(trace: &Rc<Trace>, packets: Vec<Packet>) -> Self {
        Self {
            packets: packets.into(),
            trace: trace.clone(),
        }
    }
}

impl Transport for Recorded {
    type Body = RecordedBody;

    async fn send(&mut self, request: &Request) -> Result<Response<Self::Body>, Failure> {
        let source = std::str::from_utf8(&request.body)
            .map_err(|_| Error::type_error("Recorded request must be UTF-8 JSON"))?;
        self.trace.sent.borrow_mut().push(Seen {
            body: parse(source)?,
            headers: request.headers.clone(),
        });
        let active = self.trace.active.get() + 1;
        self.trace.active.set(active);
        self.trace.peak.set(self.trace.peak.get().max(active));
        let guard = InFlight(self.trace.clone());
        let packet = self
            .packets
            .pop_front()
            .ok_or_else(|| Error::new("Error", "Unexpected recorded HTTP send"))?;
        if packet.network_error {
            return Err(Failure::network());
        }
        Ok(Response {
            status: packet.status,
            headers: vec![
                (
                    "content-type".to_owned(),
                    if packet.stream {
                        "text/event-stream"
                    } else {
                        "application/json"
                    }
                    .to_owned(),
                ),
                ("retry-after".to_owned(), "0".to_owned()),
            ],
            body: Some(RecordedBody {
                bytes: Some(packet.bytes),
                gate: packet.gate,
                _guard: guard,
            }),
        })
    }
}

#[derive(Default)]
struct Events {
    values: Vec<Value>,
    fail_type: Option<&'static str>,
    fail_phase: Option<&'static str>,
    failed: bool,
    plain: bool,
    hang: bool,
    cancel: Option<CancellationSource>,
}

impl Observer for Events {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        let matches = self
            .fail_type
            .is_some_and(|kind| domain::is_text(event.get("type"), kind))
            && self
                .fail_phase
                .is_none_or(|phase| domain::is_text(event.get("phase"), phase));
        self.values.push(event);
        if matches && !self.failed {
            self.failed = true;
            if let Some(source) = &self.cancel {
                source.cancel(Reason::Cancelled)?;
                return Ok(());
            }
            if self.hang {
                return std::future::pending().await;
            }
            let error = Error::new("Error", "Owned observer failure");
            return Err(if self.plain {
                error
            } else {
                error.coded("ROSTER_RUN_LOG")
            });
        }
        Ok(())
    }
}

fn chat<'a>(config: &Value, admission: &'a Admission) -> Result<BuiltinChat<'a>, Error> {
    builtin::create_builtin_chat(config, admission, Options::default())?
        .ok_or_else(|| Error::new("Error", "Fixture chat unexpectedly disabled"))
}

async fn complete(
    chat: &mut BuiltinChat<'_>,
    transport: &mut Recorded,
    request: &Value,
) -> Result<Completion, Failure> {
    chat.complete(
        request,
        transport,
        &mut NoObserver,
        &Credentials::default(),
        CallOptions::default(),
    )
    .await
}

fn failed<T>(result: Result<T, Failure>) -> Result<Failure, Error> {
    match result {
        Err(error) => Ok(error),
        Ok(_) => Err(Error::new("Error", "Expected a native chat failure")),
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, Error> {
    value
        .get(name)
        .ok_or_else(|| Error::type_error(format!("Missing recorded {name}")))
}

fn messages(request: &Value) -> Result<&[Value], Error> {
    request
        .get("messages")
        .and_then(domain::items)
        .ok_or_else(|| Error::type_error("Missing recorded messages"))
}

#[test]
fn completion_caps_match_effort_mapping_and_context_floor() -> Result<(), Error> {
    for (effort, cap) in [
        ("l", 8192),
        ("m", 16_384),
        ("h", 16_384),
        ("x", 32_768),
        ("none", 4096),
    ] {
        for context in [None, Some(1), Some(16_384), Some(32_768), Some(131_072)] {
            let config = config(&format!(r#"{{"effort":"{effort}"}}"#))?;
            let mut llm = field(&config, "llm")?.clone();
            if let Some(context) = context {
                put(&mut llm, "context_max", Value::Number(context as f64))?;
            }
            let expected = context.map_or(cap, |context| cap.min(context / 4).max(4096));
            assert_eq!(builtin::default_completion_cap(&llm)?, expected);
        }
    }
    let selected = config(r#"{"effort":"x","model":"deepseek-v4.1-flash"}"#)?;
    assert_eq!(
        builtin::default_completion_cap(field(&selected, "llm")?)?,
        32_768
    );
    let invalid = config(r#"{"effort":"high"}"#)?;
    assert!(builtin::default_completion_cap(field(&invalid, "llm")?).is_err());
    Ok(())
}

#[test]
fn baseline_normalizes_crlf_trim_and_utf16_bounds_without_loss() -> Result<(), Error> {
    assert_eq!(
        baseline::normalize(&Text::from("\u{feff} \r\nA\r\nB\rC \u{2028}"))?,
        Text::from("A\nB\rC")
    );
    let exact = Text(vec![0xd800; 8192]);
    assert_eq!(baseline::normalize(&exact)?, exact);
    assert!(baseline::normalize(&Text(vec![0xd800; 8193])).is_err());
    assert!(baseline::normalize(&Text::from(" \r\n\u{feff}")).is_err());
    assert_eq!(baseline::normalize(&Text::from("😀"))?.0.len(), 2);
    assert!(baseline::normalize(&Text::from("😀".repeat(4097).as_str())).is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn baseline_and_lossless_messages_tools_and_response_survive_real_wrapper()
-> Result<(), Error> {
    let admission = Admission::default();
    let mut chat = chat(&config(r#"{"effort":"none"}"#)?, &admission)?;
    let mut request = parse(
        r#"{"messages":[{"role":"system","name":"\ud800","content":"Critic \udfff"},{"role":"user","content":"Exact Ask \ud800😀"}],"tools":[{"type":"function","function":{"name":"read_file","parameters":{"title":"\ud800"}}}],"tool_choice":"auto"}"#,
    )?;
    let original = request.clone();
    let response = parse(
        r#"{"model":"actual-served","choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"\ud800😀\udfff"}}]}"#,
    )?;
    let mut transport = Recorded::new(vec![
        Packet::json(response.clone()),
        Packet::json(response.clone()),
    ]);
    let completion = complete(&mut chat, &mut transport, &request)
        .await
        .map_err(|failure| *failure.error)?;
    assert_eq!(
        field(&completion.message, "content")?,
        &Value::String(Text(vec![0xd800, 0xd83d, 0xde00, 0xdfff]))
    );
    let first = transport.trace.bodies().remove(0);
    let mut expected = field(
        messages(&original)?
            .first()
            .ok_or_else(|| Error::new("Error", "Missing caller message"))?,
        "content",
    )?
    .as_string()
    .cloned()
    .ok_or_else(|| Error::type_error("Missing caller text"))?;
    expected.push_str("\n\n");
    expected.0.extend(baseline::read()?.0);
    assert_eq!(
        messages(&first)?
            .first()
            .and_then(|message| message.get("content")),
        Some(&Value::String(expected))
    );
    assert_eq!(messages(&first)?.get(1), messages(&original)?.get(1));
    assert_eq!(first.get("tools"), original.get("tools"));
    assert_eq!(first.get("tool_choice"), original.get("tool_choice"));
    assert_eq!(first.get("max_tokens"), Some(&Value::Number(4096.0)));
    assert_eq!(first.get("stream"), Some(&Value::Boolean(false)));
    assert_eq!(first.get("reasoning_effort"), Some(&Value::string("none")));
    assert_eq!(request, original);
    put(&mut request, "messages", field(&first, "messages")?.clone())?;
    complete(&mut chat, &mut transport, &request)
        .await
        .map_err(|failure| *failure.error)?;
    assert_eq!(transport.trace.bodies().get(1), Some(&first));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn actual_outbound_caps_effort_stream_and_deepseek_flags_are_exact() -> Result<(), Error> {
    for (settings, options, override_stream, cap, effort, thinking) in [
        (r#"{"effort":"l"}"#, false, None, 8192, "low", None),
        (r#"{"effort":"m"}"#, true, None, 16_384, "medium", None),
        (
            r#"{"effort":"h","context_max":32768}"#,
            true,
            Some(false),
            8192,
            "high",
            None,
        ),
        (
            r#"{"effort":"x"}"#,
            false,
            Some(true),
            32_768,
            "xhigh",
            None,
        ),
        (r#"{"effort":"none"}"#, false, None, 4096, "none", None),
        (
            r#"{"model":"deepseek-v4.1-flash","effort":"m"}"#,
            false,
            None,
            16_384,
            "high",
            Some(true),
        ),
        (
            r#"{"model":"deepseek-v4.1-flash","effort":"x"}"#,
            true,
            None,
            32_768,
            "max",
            Some(true),
        ),
        (
            r#"{"model":"deepseek-v4.1-flash","effort":"none"}"#,
            true,
            None,
            4096,
            "none",
            Some(false),
        ),
        (
            r#"{"model":"deepseek-v4.1-flash","effort":"x","task_kind":"slice","task_class":"docs","max_tokens":256}"#,
            true,
            None,
            8192,
            "none",
            Some(false),
        ),
        (
            r#"{"model":"deepseek-v4.1-flash","effort":"x","task_kind":"feature","task_class":"docs","max_tokens":4096}"#,
            false,
            None,
            4096,
            "max",
            Some(true),
        ),
        (
            r#"{"base_url":"https://example.invalid/v1","model":"deepseek-v4.1-flash","effort":"m"}"#,
            false,
            None,
            16_384,
            "medium",
            None,
        ),
    ] {
        let admission = Admission::default();
        let mut chat = BuiltinChat::new(
            &config(settings)?,
            &admission,
            Options {
                stream: options,
                ..Options::default()
            },
        )?
        .ok_or_else(|| Error::new("Error", "Chat disabled"))?;
        let mut request = request("Exact Ask");
        if let Some(stream) = override_stream {
            put(&mut request, "stream", Value::Boolean(stream))?;
        }
        let mut transport = Recorded::new(vec![packet("stop", "ok", None)]);
        let mut events = Events::default();
        chat.complete(
            &request,
            &mut transport,
            &mut events,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await
        .map_err(|failure| *failure.error)?;
        let bodies = transport.trace.bodies();
        let body = bodies
            .first()
            .ok_or_else(|| Error::new("Error", "Missing outbound body"))?;
        assert_eq!(
            body.get("max_tokens"),
            Some(&Value::Number(cap as f64)),
            "{settings}"
        );
        assert_eq!(
            body.get("reasoning_effort"),
            Some(&Value::string(effort)),
            "{settings}"
        );
        assert_eq!(
            body.get("stream"),
            Some(&Value::Boolean(override_stream.unwrap_or(options))),
            "{settings}"
        );
        assert_eq!(
            body.get("chat_template_kwargs")
                .and_then(|kwargs| kwargs.get("thinking")),
            thinking.map(Value::Boolean).as_ref(),
            "{settings}"
        );
        let start = events
            .values
            .iter()
            .find(|event| domain::is_text(event.get("phase"), "start"))
            .ok_or_else(|| Error::new("Error", "Missing HTTP start"))?;
        assert_eq!(start.get("maxTokens"), Some(&Value::Number(cap as f64)));
        assert_eq!(
            start.get("thinking"),
            Some(&Value::Boolean(effort != "none"))
        );
        assert_eq!(
            start.get("modelPrior"),
            Some(&Value::string(
                if settings.contains("deepseek-v4.1-flash") {
                    "strong"
                } else {
                    "unknown"
                }
            ))
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn optional_keys_mapping_locked_labels_and_declared_prior_are_not_routes() -> Result<(), Error>
{
    for optional in [None, Some(false), Some(true)] {
        let admission = Admission::default();
        let mut config =
            config(r#"{"api_key_env":"SEAT_KEY","locked_model":true,"model_prior":"limited"}"#)?;
        let mut llm = field(&config, "llm")?.clone();
        if let Some(optional) = optional {
            put(&mut llm, "api_key_optional", Value::Boolean(optional))?;
        }
        put(&mut config, "llm", llm)?;
        let mut chat = chat(&config, &admission)?;
        let mut transport = Recorded::new(vec![packet(
            "stop",
            "ok",
            Some(parse(r#"{"prompt_tokens":0}"#)?),
        )]);
        let mut events = Events::default();
        let credentials = Credentials {
            environment: Some(parse(
                r#"{"OPENAI_API_KEY":"not-the-selected-key","SEAT_KEY":"fixture-selected-key","AI_MODEL":"invented"}"#,
            )?),
            vault_entries: None,
        };
        let response = chat
            .complete(
                &request("Ask"),
                &mut transport,
                &mut events,
                &credentials,
                CallOptions::default(),
            )
            .await
            .map_err(|failure| *failure.error)?;
        assert_eq!(response.model, Text::from("requested"));
        assert_eq!(
            chat.last_response().map(|response| &response.model),
            Some(&Text::from("requested"))
        );
        assert_eq!(
            chat.last_response()
                .and_then(|response| response.usage)
                .and_then(|usage| usage.prompt_tokens),
            Some(0)
        );
        assert!(
            !events
                .values
                .iter()
                .any(|event| domain::is_text(event.get("type"), "served-model"))
        );
        assert!(
            events
                .values
                .iter()
                .filter(|event| domain::is_text(event.get("type"), "http"))
                .all(|event| event.get("modelPrior") == Some(&Value::string("limited")))
        );
        assert!(
            transport
                .trace
                .sent
                .borrow()
                .first()
                .is_some_and(|seen| seen
                    .headers
                    .iter()
                    .any(|(name, value)| name == "Authorization"
                        && value == "Bearer fixture-selected-key"))
        );
        assert_eq!(
            transport
                .trace
                .bodies()
                .first()
                .and_then(|body| body.get("model")),
            Some(&Value::string("requested"))
        );
    }
    for (settings, allowed) in [("{}", true), (r#"{"api_key_optional":false}"#, false)] {
        let admission = Admission::default();
        let mut chat = chat(&config(settings)?, &admission)?;
        let mut transport = Recorded::new(vec![packet("stop", "ok", None)]);
        assert_eq!(
            complete(&mut chat, &mut transport, &request("Ask"))
                .await
                .is_ok(),
            allowed
        );
        assert_eq!(chat.request_count(), u64::from(allowed));
        assert_eq!(transport.trace.sent.borrow().len(), usize::from(allowed));
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_native_input_never_sends_or_keeps_an_admission_slot() -> Result<(), Error> {
    let admission = Admission::default();
    let mut chat = chat(
        &config(r#"{"fleet_profile":"validation","concurrency":1,"max_requests":1}"#)?,
        &admission,
    )?;
    let mut transport = Recorded::new(vec![]);
    for source in [
        "null",
        "{}",
        r#"{"messages":null}"#,
        r#"{"messages":[]}"#,
        r#"{"messages":{}}"#,
        r#"{"messages":[{}]}"#,
        r#"{"messages":[{"role":""}]}"#,
        r#"{"messages":[{"role":"user"}],"max_tokens":1}"#,
        r#"{"messages":[{"role":"user"}],"max_tokens":0}"#,
        r#"{"messages":[{"role":"user"}],"max_tokens":1.5}"#,
        r#"{"messages":[{"role":"user"}],"max_tokens":-2}"#,
        r#"{"messages":[{"role":"user"}],"max_tokens":"4096"}"#,
        r#"{"messages":[{"role":"user"}],"max_tokens":9007199254740992}"#,
        r#"{"messages":[{"role":"user"}],"stream":"true"}"#,
    ] {
        let failure = failed(complete(&mut chat, &mut transport, &parse(source)?).await)?;
        assert_eq!(failure.error.name, "TypeError", "{source}");
        assert_eq!(chat.request_count(), 0);
        assert_eq!(admission.depth(&Text::from("validation"))?, 0);
    }
    assert!(transport.trace.sent.borrow().is_empty());
    for source in [
        r#"{"concurrency":0}"#,
        r#"{"fleet_profile":1}"#,
        r#"{"api_key_optional":"true"}"#,
        r#"{"served_model_label":"route"}"#,
        r#"{"effort":"unsupported"}"#,
    ] {
        assert!(
            builtin::create_builtin_chat(&config(source)?, &admission, Options::default()).is_err()
        );
    }
    assert!(
        builtin::create_builtin_chat(
            &config(r#"{"base_url":""}"#)?,
            &admission,
            Options::default()
        )?
        .is_none()
    );
    let failure = failed(
        builtin::chat_completion(
            &config(r#"{"base_url":""}"#)?,
            &request("Ask"),
            &admission,
            &mut transport,
            &mut NoObserver,
            &Credentials::default(),
            builtin::CompletionOptions::default(),
        )
        .await,
    )?;
    assert_eq!(
        failure.error.message,
        "An LLM base_url is required for chat completion"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn length_retry_aggregates_actual_usage_but_keeps_latest_served_snapshot() -> Result<(), Error>
{
    let admission = Admission::default();
    let mut chat = chat(&config(r#"{"max_tokens":2048}"#)?, &admission)?;
    let request = request("Exact implementation Ask");
    let original = request.clone();
    let mut transport = Recorded::new(vec![
        packet(
            "length",
            "PRIVATE_PARTIAL",
            Some(parse(r#"{"prompt_tokens":10,"completion_tokens":20}"#)?),
        ),
        packet(
            "stop",
            "complete",
            Some(parse(r#"{"prompt_tokens":3,"completion_tokens":2}"#)?),
        ),
    ]);
    let mut events = Events::default();
    let response = chat
        .complete(
            &request,
            &mut transport,
            &mut events,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await
        .map_err(|failure| *failure.error)?;
    assert_eq!(
        response.usage,
        Some(parse(r#"{"prompt_tokens":13,"completion_tokens":22}"#)?)
    );
    assert_eq!(chat.last_usage(), response.usage.as_ref());
    assert_eq!(chat.last_attempts(), 2);
    assert_eq!(chat.request_count(), 2);
    let last = chat
        .last_response()
        .ok_or_else(|| Error::new("Error", "Missing actual response"))?;
    assert_eq!(last.model, Text::from("actual-served"));
    assert_eq!(last.usage.and_then(|usage| usage.prompt_tokens), Some(3));
    assert_eq!(
        last.usage.and_then(|usage| usage.completion_tokens),
        Some(2)
    );
    let bodies = transport.trace.bodies();
    for body in &bodies {
        assert_eq!(body.get("max_tokens"), Some(&Value::Number(2048.0)));
        assert_eq!(messages(body)?.get(1), messages(&original)?.first());
    }
    let retry = bodies
        .get(1)
        .ok_or_else(|| Error::new("Error", "No retry send"))?;
    assert!(!retry.json().contains("PRIVATE_PARTIAL"));
    assert!(messages(retry)?.last().is_some_and(|message| {
        message
            .json()
            .contains("do not repeat previously executed edits")
    }));
    assert_eq!(request, original);
    let finishes = events
        .values
        .iter()
        .filter(|event| domain::is_text(event.get("type"), "finish-reason"))
        .collect::<Vec<_>>();
    assert_eq!(
        finishes,
        vec![&parse(
            r#"{"type":"finish-reason","reason":"length","retry":true}"#
        )?]
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn docs_partial_continuation_is_lossless_and_never_replays_tools() -> Result<(), Error> {
    for tool in [false, true] {
        let admission = Admission::default();
        let mut chat = chat(
            &config(
                r#"{"model":"deepseek-v4.1-flash","task_kind":"slice","task_class":"docs","effort":"x","max_tokens":256}"#,
            )?,
            &admission,
        )?;
        let mut first = parse(
            r#"{"model":"actual-served","choices":[{"finish_reason":"length","message":{"role":"assistant","content":"Partial \ud800😀\udfff"}}]}"#,
        )?;
        if tool {
            first = parse(
                r#"{"model":"actual-served","choices":[{"finish_reason":"length","message":{"role":"assistant","content":"Partial \ud800😀\udfff","tool_calls":[{"id":"partial","type":"function","function":{"name":"write_file","arguments":"{"}}]}}]}"#,
            )?;
        }
        let mut transport =
            Recorded::new(vec![Packet::json(first), packet("stop", "complete", None)]);
        let mut events = Events::default();
        let mut request = request("Exact docs Ask");
        put(&mut request, "max_tokens", Value::Number(512.0))?;
        put(&mut request, "reasoning_effort", Value::string("max"))?;
        put(
            &mut request,
            "chat_template_kwargs",
            parse(r#"{"thinking":true}"#)?,
        )?;
        chat.complete(
            &request,
            &mut transport,
            &mut events,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await
        .map_err(|failure| *failure.error)?;
        let bodies = transport.trace.bodies();
        for body in &bodies {
            assert_eq!(body.get("max_tokens"), Some(&Value::Number(8192.0)));
            assert_eq!(body.get("reasoning_effort"), Some(&Value::string("none")));
            assert_eq!(
                body.get("chat_template_kwargs"),
                Some(&parse(r#"{"thinking":false}"#)?)
            );
        }
        let retry = bodies
            .get(1)
            .ok_or_else(|| Error::new("Error", "Missing docs retry"))?;
        let messages = messages(retry)?;
        assert_eq!(messages.len(), if tool { 3 } else { 4 });
        if tool {
            assert!(!retry.json().contains("Partial"));
            assert!(!retry.json().contains("write_file"));
        } else {
            assert_eq!(
                messages.get(2),
                Some(&parse(
                    r#"{"role":"assistant","content":"Partial \ud800😀\udfff"}"#
                )?)
            );
            assert!(
                messages
                    .last()
                    .is_some_and(|message| message.json().contains("without repeating it"))
            );
        }
        assert!(
            events
                .values
                .iter()
                .any(|event| event.get("continued") == Some(&Value::Boolean(true)))
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn reasoning_only_growth_is_bounded_persistent_and_never_shrinks_a_higher_cap()
-> Result<(), Error> {
    for (cap, raised) in [(4096, 8192), (30_000, 32_768), (40_000, 40_000)] {
        let admission = Admission::default();
        let mut chat = chat(
            &config(&format!(r#"{{"max_tokens":{cap},"effort":"h"}}"#))?,
            &admission,
        )?;
        let mut transport = Recorded::new(vec![
            packet("length", " \r\n\u{feff}", None),
            packet("stop", "complete", None),
            packet("stop", "next", None),
        ]);
        complete(&mut chat, &mut transport, &request("Review"))
            .await
            .map_err(|failure| *failure.error)?;
        complete(&mut chat, &mut transport, &request("Next"))
            .await
            .map_err(|failure| *failure.error)?;
        let bodies = transport.trace.bodies();
        assert_eq!(
            bodies
                .iter()
                .filter_map(|body| body.get("max_tokens").and_then(Value::as_number))
                .collect::<Vec<_>>(),
            vec![cap as f64, raised as f64, raised as f64]
        );
        assert!(bodies.get(1).is_some_and(|body| {
            body.json()
                .contains("Reasoning used the whole completion cap")
        }));
        assert!(
            bodies
                .iter()
                .all(|body| body.get("reasoning_effort") == Some(&Value::string("high")))
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn one_retry_per_request_and_three_per_chat_are_hard_limits() -> Result<(), Error> {
    let admission = Admission::default();
    let mut chat = chat(&config(r#"{"max_tokens":4096}"#)?, &admission)?;
    let mut transport = Recorded::new(
        (0..7)
            .map(|_| packet("length", "PRIVATE_PARTIAL", None))
            .collect(),
    );
    for (index, sends) in [2, 4, 6, 7].into_iter().enumerate() {
        let failure = failed(complete(&mut chat, &mut transport, &request("Ask")).await)?;
        assert_eq!(failure.error.code, Some("ROSTER_LLM_FINISH_REASON"));
        assert_eq!(failure.truncated, Some(true));
        assert_eq!(
            failure
                .partial
                .as_deref()
                .and_then(|message| message.get("content")),
            Some(&Value::string("PRIVATE_PARTIAL"))
        );
        assert!(!failure.error.message.contains("PRIVATE_PARTIAL"));
        assert_eq!(chat.request_count(), sends);
        assert_eq!(chat.last_attempts(), if index == 3 { 1 } else { 2 });
    }
    assert_eq!(transport.trace.sent.borrow().len(), 7);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn disabled_length_and_other_finish_reasons_preserve_original_failures() -> Result<(), Error>
{
    for (reason, retry) in [
        ("length", false),
        ("content_filter", true),
        ("function_call", true),
    ] {
        let admission = Admission::default();
        let mut chat = BuiltinChat::new(
            &config("{}")?,
            &admission,
            Options {
                retry_length: retry,
                stream: false,
            },
        )?
        .ok_or_else(|| Error::new("Error", "Chat disabled"))?;
        let mut transport = Recorded::new(vec![packet(reason, "PRIVATE_PARTIAL", None)]);
        let failure = failed(complete(&mut chat, &mut transport, &request("Ask")).await)?;
        assert_eq!(failure.error.code, Some("ROSTER_LLM_FINISH_REASON"));
        assert_eq!(failure.finish_reason, Some(Text::from(reason)));
        assert_eq!(failure.truncated, Some(reason == "length"));
        assert!(failure.partial.is_some());
        assert_eq!(chat.last_attempts(), 1);
        assert_eq!(chat.request_count(), 1);
        assert!(chat.last_response().is_some());
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn usage_zero_unknown_and_partial_retry_counts_remain_distinct() -> Result<(), Error> {
    for usage in [
        None,
        Some(parse("{}")?),
        Some(parse(r#"{"prompt_tokens":0}"#)?),
        Some(parse(r#"{"completion_tokens":0}"#)?),
    ] {
        let admission = Admission::default();
        let mut chat = chat(&config("{}")?, &admission)?;
        let mut transport = Recorded::new(vec![packet("stop", "ok", usage.clone())]);
        let response = complete(&mut chat, &mut transport, &request("Ask"))
            .await
            .map_err(|failure| *failure.error)?;
        assert_eq!(response.usage, usage);
        assert_eq!(chat.last_usage(), usage.as_ref());
        let actual = chat
            .last_response()
            .ok_or_else(|| Error::new("Error", "Missing response-backed identity"))?;
        assert_eq!(actual.model, Text::from("actual-served"));
        assert_eq!(actual.usage.is_some(), usage.is_some());
        assert_eq!(
            actual.usage.and_then(|usage| usage.prompt_tokens),
            usage
                .as_ref()
                .and_then(|usage| usage.get("prompt_tokens"))
                .map(|_| 0)
        );
        assert_eq!(
            actual.usage.and_then(|usage| usage.completion_tokens),
            usage
                .as_ref()
                .and_then(|usage| usage.get("completion_tokens"))
                .map(|_| 0)
        );
    }
    let admission = Admission::default();
    let mut chat = chat(&config("{}")?, &admission)?;
    let mut transport = Recorded::new(vec![
        packet("length", "partial", Some(parse(r#"{"prompt_tokens":0}"#)?)),
        packet(
            "stop",
            "complete",
            Some(parse(r#"{"prompt_tokens":2,"completion_tokens":0}"#)?),
        ),
    ]);
    let response = complete(&mut chat, &mut transport, &request("Ask"))
        .await
        .map_err(|failure| *failure.error)?;
    assert_eq!(response.usage, Some(parse(r#"{"prompt_tokens":2}"#)?));
    assert_eq!(
        chat.last_response()
            .and_then(|response| response.usage)
            .and_then(|usage| usage.completion_tokens),
        Some(0)
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn actual_send_budget_bounds_length_http_and_stall_retries() -> Result<(), Error> {
    for mode in ["length", "http", "stall"] {
        let admission = Admission::default();
        let mut chat = chat(
            &config(r#"{"max_requests":1,"stream_idle_timeout_ms":10}"#)?,
            &admission,
        )?;
        let mut first = packet("length", "partial", Some(parse(r#"{"prompt_tokens":0}"#)?));
        let mut request = request("Ask");
        if mode == "http" {
            first.status = 503;
        } else if mode == "stall" {
            first.stream = true;
            first.gate = Some(Rc::default());
            put(&mut request, "stream", Value::Boolean(true))?;
        }
        let mut transport = Recorded::new(vec![first]);
        let failure = failed(complete(&mut chat, &mut transport, &request).await)?;
        assert_eq!(
            failure.error.code,
            Some("ROSTER_MODEL_REQUEST_LIMIT"),
            "{mode}"
        );
        assert_eq!(failure.limit, Some(1));
        assert_eq!(failure.route_failure(), None);
        assert_eq!(chat.request_limit(), Some(1));
        assert_eq!(chat.request_count(), 1);
        assert_eq!(transport.trace.sent.borrow().len(), 1);
        assert_eq!(transport.trace.active.get(), 0);
        if mode == "length" {
            assert_eq!(chat.last_attempts(), 2);
            assert_eq!(chat.last_usage(), Some(&parse("{}")?));
            assert!(chat.take_last_response().is_some());
            assert!(chat.take_last_response().is_none());
            assert_eq!(chat.request_count(), 1);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn observer_failures_preserve_actual_identity_and_release_owned_capacity() -> Result<(), Error>
{
    for plain in [false, true] {
        for (kind, phase, sent) in [
            ("model", None, false),
            ("http", Some("start"), false),
            ("http", Some("ok"), true),
            ("usage", None, true),
            ("served-model", None, true),
            ("completion", None, true),
            ("finish-reason", None, true),
        ] {
            let admission = Admission::default();
            let config = config(r#"{"fleet_profile":"observer","concurrency":1}"#)?;
            let mut chat = chat(&config, &admission)?;
            let mut transport = Recorded::new(vec![
                packet(
                    if kind == "finish-reason" {
                        "length"
                    } else {
                        "stop"
                    },
                    "ok",
                    Some(parse(r#"{"prompt_tokens":0}"#)?),
                ),
                packet("stop", "next", None),
            ]);
            let mut events = Events {
                fail_type: Some(kind),
                fail_phase: phase,
                plain,
                ..Events::default()
            };
            let failure = failed(
                chat.complete(
                    &request("Ask"),
                    &mut transport,
                    &mut events,
                    &Credentials::default(),
                    CallOptions::default(),
                )
                .await,
            )?;
            if plain {
                assert_eq!(failure.category, Some(super::Category::Network));
                assert!(!failure.error.message.contains("Owned observer failure"));
            } else {
                assert_eq!(
                    failure.error.code,
                    Some("ROSTER_RUN_LOG"),
                    "{kind}/{phase:?}"
                );
            }
            assert_eq!(chat.request_count(), u64::from(sent));
            assert_eq!(chat.last_response().is_some(), sent, "{kind}/{phase:?}");
            if sent {
                assert_eq!(
                    chat.last_response().map(|response| &response.model),
                    Some(&Text::from("actual-served"))
                );
                assert_eq!(
                    chat.last_response()
                        .and_then(|response| response.usage)
                        .and_then(|usage| usage.prompt_tokens),
                    Some(0)
                );
                assert_eq!(
                    chat.last_response()
                        .and_then(|response| response.usage)
                        .and_then(|usage| usage.completion_tokens),
                    None
                );
            }
            assert_eq!(admission.depth(&Text::from("observer"))?, 0);
            assert_eq!(transport.trace.active.get(), 0);
            complete(&mut chat, &mut transport, &request("Next"))
                .await
                .map_err(|failure| *failure.error)?;
            assert_eq!(chat.request_count(), u64::from(sent) + 1);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn observer_cancellation_and_deadlines_revoke_new_snapshots_without_leaking()
-> Result<(), Error> {
    for kind in ["usage", "completion"] {
        for cancellation in [false, true] {
            let admission = Admission::default();
            let config = config(r#"{"fleet_profile":"revoked","request_timeout_ms":20}"#)?;
            let mut chat = chat(&config, &admission)?;
            let source = CancellationSource::default();
            let token = source.token();
            let mut transport = Recorded::new(vec![packet(
                "stop",
                "ok",
                Some(parse(r#"{"prompt_tokens":0}"#)?),
            )]);
            let mut events = Events {
                fail_type: Some(kind),
                hang: !cancellation,
                cancel: cancellation.then_some(source),
                ..Events::default()
            };
            let failure = failed(
                chat.complete(
                    &request("Ask"),
                    &mut transport,
                    &mut events,
                    &Credentials::default(),
                    CallOptions {
                        signal: Some(&token),
                        ..CallOptions::default()
                    },
                )
                .await,
            )?;
            assert_eq!(
                failure.error.code,
                Some(if cancellation {
                    "ROSTER_CANCELLED"
                } else {
                    "ROSTER_LLM_TIMEOUT"
                })
            );
            assert!(chat.last_response().is_none());
            assert_eq!(chat.request_count(), 1);
            assert_eq!(admission.depth(&Text::from("revoked"))?, 0);
            assert_eq!(transport.trace.active.get(), 0);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn native_convenience_completion_returns_choices_and_only_response_backed_metadata()
-> Result<(), Error> {
    let admission = Admission::default();
    let config = config("{}")?;
    let mut transport = Recorded::new(vec![packet(
        "stop",
        "ok",
        Some(parse(r#"{"completion_tokens":0}"#)?),
    )]);
    let result = builtin::chat_completion(
        &config,
        &request("Ask"),
        &admission,
        &mut transport,
        &mut NoObserver,
        &Credentials::default(),
        builtin::CompletionOptions::default(),
    )
    .await
    .map_err(|failure| *failure.error)?;
    assert_eq!(
        result,
        parse(
            r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"completion_tokens":0},"response":{"model":"actual-served","usage":{"completion_tokens":0}}}"#
        )?
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn usage_overflow_is_not_a_fabricated_total_or_loss_of_the_latest_response()
-> Result<(), Error> {
    let admission = Admission::default();
    let mut chat = chat(&config("{}")?, &admission)?;
    let first = packet(
        "length",
        "partial",
        Some(parse(
            r#"{"prompt_tokens":9007199254740991,"completion_tokens":0}"#,
        )?),
    );
    let second = Packet::json(parse(
        r#"{"model":"actual-retry","usage":{"prompt_tokens":1,"completion_tokens":0},"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"complete"}}]}"#,
    )?);
    let mut transport = Recorded::new(vec![first, second]);
    let error = failed(complete(&mut chat, &mut transport, &request("Ask")).await)?;
    assert_eq!(error.error.name, "TypeError");
    assert!(error.error.message.contains("total exceeds safe integer"));
    assert!(chat.last_usage().is_none());
    assert_eq!(
        chat.last_response().map(|response| &response.model),
        Some(&Text::from("actual-retry"))
    );
    assert_eq!(
        chat.last_response()
            .and_then(|response| response.usage)
            .and_then(|usage| usage.prompt_tokens),
        Some(1)
    );
    assert_eq!(chat.request_count(), 2);
    assert_eq!(transport.trace.active.get(), 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn failed_or_cancelled_length_retry_cannot_turn_partial_usage_into_a_total()
-> Result<(), Error> {
    for mode in ["network", "timeout", "cancelled"] {
        let admission = Admission::default();
        let mut chat = chat(
            &config(r#"{"max_tokens":4096,"request_timeout_ms":20}"#)?,
            &admission,
        )?;
        let first = packet(
            "length",
            "partial",
            Some(parse(r#"{"prompt_tokens":2,"completion_tokens":0}"#)?),
        );
        let mut second = packet("stop", "not-accepted", None);
        if mode == "network" {
            second.network_error = true;
        } else {
            second.gate = Some(Rc::default());
        }
        let mut transport = Recorded::new(vec![first, second]);
        let trace = transport.trace.clone();
        let source = CancellationSource::default();
        let token = source.token();
        let request = request("Ask");
        let mut observer = NoObserver;
        let credentials = Credentials::default();
        let (response, control) = tokio::join!(
            chat.complete(
                &request,
                &mut transport,
                &mut observer,
                &credentials,
                CallOptions {
                    signal: Some(&token),
                    ..CallOptions::default()
                }
            ),
            async {
                if mode == "cancelled" {
                    for _ in 0..100 {
                        if trace.sent.borrow().len() >= 2 {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    if trace.sent.borrow().len() < 2 {
                        return Err(Error::new(
                            "Error",
                            "Retry never reached its owned cancellation barrier",
                        ));
                    }
                    source.cancel(Reason::TaskTimeout(450))?;
                }
                Ok::<_, Error>(())
            },
        );
        control?;
        let error = failed(response)?;
        assert_eq!(
            error.error.code,
            match mode {
                "timeout" => Some("ROSTER_LLM_TIMEOUT"),
                "cancelled" => Some("ROSTER_TASK_TIMEOUT"),
                _ => None,
            }
        );
        assert_eq!(chat.last_attempts(), 2);
        assert_eq!(chat.request_count(), 2);
        assert_eq!(chat.last_usage(), Some(&parse("{}")?));
        let actual = chat
            .last_response()
            .ok_or_else(|| Error::new("Error", "Lost actual partial response"))?;
        assert_eq!(actual.model, Text::from("actual-served"));
        assert_eq!(actual.usage.and_then(|usage| usage.prompt_tokens), Some(2));
        assert_eq!(
            actual.usage.and_then(|usage| usage.completion_tokens),
            Some(0)
        );
        assert_eq!(trace.active.get(), 0);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn wrapper_streams_through_owned_loopback_http_without_node_or_a_background_task()
-> Result<(), Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| Error::new("Error", format!("Owned loopback bind failed: {error}")))?;
    let address = listener
        .local_addr()
        .map_err(|error| Error::new("Error", error.to_string()))?;
    let config = config(&format!(
        r#"{{"base_url":"http://{address}/v1","effort":"l","max_requests":1}}"#
    ))?;
    let admission = Admission::default();
    let mut chat = BuiltinChat::new(
        &config,
        &admission,
        Options {
            stream: true,
            ..Options::default()
        },
    )?
    .ok_or_else(|| Error::new("Error", "Loopback chat disabled"))?;
    let mut transport = super::http::HttpTransport::new().map_err(|failure| *failure.error)?;
    let mut observer = Events::default();
    let credentials = Credentials::default();
    let request = request("Exact network Ask");
    let server = async {
        let (mut socket, _) = listener
            .accept()
            .await
            .map_err(|error| Error::new("Error", error.to_string()))?;
        let mut bytes = Vec::new();
        let (header_end, length) = loop {
            let mut chunk = [0; 4096];
            let count = socket
                .read(&mut chunk)
                .await
                .map_err(|error| Error::new("Error", error.to_string()))?;
            if count == 0 || bytes.len() > 65_536 {
                return Err(Error::new(
                    "Error",
                    "Owned loopback request was incomplete or oversized",
                ));
            }
            bytes.extend_from_slice(
                chunk
                    .get(..count)
                    .ok_or_else(|| Error::new("Error", "Invalid loopback read length"))?,
            );
            let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let headers = std::str::from_utf8(
                bytes
                    .get(..end)
                    .ok_or_else(|| Error::new("Error", "Missing request headers"))?,
            )
            .map_err(|_| Error::type_error("Invalid loopback headers"))?;
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .filter(|length| *length <= 65_536)
                .ok_or_else(|| Error::type_error("Missing bounded Content-Length"))?;
            break (end + 4, length);
        };
        while bytes.len() < header_end + length {
            let mut chunk = [0; 4096];
            let count = socket
                .read(&mut chunk)
                .await
                .map_err(|error| Error::new("Error", error.to_string()))?;
            if count == 0 {
                return Err(Error::new("Error", "Owned loopback body ended early"));
            }
            bytes.extend_from_slice(
                chunk
                    .get(..count)
                    .ok_or_else(|| Error::new("Error", "Invalid body read length"))?,
            );
        }
        let body = std::str::from_utf8(
            bytes
                .get(header_end..header_end + length)
                .ok_or_else(|| Error::new("Error", "Missing HTTP body"))?,
        )
        .map_err(|_| Error::type_error("Invalid request body UTF-8"))?;
        let body = parse(body)?;
        let stream = "data: {\"model\":\"loopback-served\",\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":2},\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\\ud800ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{stream}",
            stream.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .map_err(|error| Error::new("Error", error.to_string()))?;
        socket
            .shutdown()
            .await
            .map_err(|error| Error::new("Error", error.to_string()))?;
        Ok::<_, Error>(body)
    };
    let (response, body) = tokio::join!(
        chat.complete(
            &request,
            &mut transport,
            &mut observer,
            &credentials,
            CallOptions::default()
        ),
        tokio::time::timeout(Duration::from_secs(5), server),
    );
    let body = body.map_err(|_| Error::new("Error", "Owned loopback server timed out"))??;
    let response = response.map_err(|failure| *failure.error)?;
    assert_eq!(body.get("max_tokens"), Some(&Value::Number(8192.0)));
    assert_eq!(body.get("stream"), Some(&Value::Boolean(true)));
    assert_eq!(
        body.get("stream_options"),
        Some(&parse(r#"{"include_usage":true}"#)?)
    );
    assert_eq!(messages(&body)?.get(1), messages(&request)?.first());
    let baseline = baseline::read()?;
    assert!(messages(&body)?.first().is_some_and(|message| message.get("content") == Some(&Value::String(baseline.clone()))));
    assert_eq!(response.model, Text::from("loopback-served"));
    assert_eq!(
        response.message.get("content"),
        Some(&Value::String(Text(vec![0xd800, 111, 107])))
    );
    assert_eq!(chat.request_count(), 1);
    assert_eq!(
        chat.last_response()
            .and_then(|response| response.usage)
            .and_then(|usage| usage.prompt_tokens),
        Some(0)
    );
    assert!(
        observer
            .values
            .iter()
            .any(|event| domain::is_text(event.get("type"), "delta"))
    );
    Ok(())
}
