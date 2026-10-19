use std::cell::{Cell, RefCell};
use std::time::Duration;

use time::format_description::well_known::{Rfc2822, Rfc3339};
use time::{Date, Month, OffsetDateTime};
use tokio::time::Instant;
use url::{Position, Url};

use crate::cancel::CancellationToken;
use crate::domain::{self, is_text, put};
use crate::json::{self, Value};
use crate::text::{Text, ascii, trim};
use crate::{Error, clock, request, run};

use super::control::{Observer, RequestControl};
use super::http::{self, Body, Transport};
use super::redact::Redactor;
use super::stream::StreamDecoder;
use super::{Category, Failure, MAX_RESPONSE_BYTES};

#[derive(Default)]
pub struct Credentials {
    pub environment: Option<Value>,
    pub vault_entries: Option<Value>,
}

impl Credentials {
    fn resolve(&self, names: &[Text]) -> Result<Option<Text>, Error> {
        let empty = Value::object(Vec::new());
        let environment = self.environment.as_ref().unwrap_or(&empty);
        for name in names {
            let value = crate::secrets::resolve_secret(name, environment, &mut |name| match self
                .vault_entries
                .as_ref()
            {
                None => Ok(None),
                Some(vault @ Value::Object(_)) => Ok(vault.get(name).cloned()),
                Some(_) => Err(Error::type_error(
                    "The file vault mapping must be an object.",
                )),
            })?;
            if value.is_some() {
                return Ok(value);
            }
        }
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub message: Value,
    pub usage: Option<Value>,
    pub model: Text,
    pub finish_reason: Option<Value>,
}

impl Completion {
    pub fn as_value(&self) -> Value {
        let mut fields = vec![
            ("message", self.message.clone()),
            ("usage", self.usage.clone().unwrap_or(Value::Null)),
            ("model", Value::String(self.model.clone())),
        ];
        if let Some(reason) = &self.finish_reason {
            fields.push(("finish_reason", reason.clone()));
        }
        Value::object(fields)
    }

    pub fn measured_usage(&self) -> Result<run::Usage, Error> {
        run::merge_usage(&[run::UsageSample {
            prompt_tokens: self
                .usage
                .as_ref()
                .and_then(|usage| usage.get("prompt_tokens"))
                .and_then(Value::as_number),
            completion_tokens: self
                .usage
                .as_ref()
                .and_then(|usage| usage.get("completion_tokens"))
                .and_then(Value::as_number),
        }])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastResponse {
    pub model: Text,
    pub usage: Option<run::Usage>,
}

impl LastResponse {
    pub fn as_value(&self) -> Value {
        let usage = self.usage.map_or(Value::Null, |usage| {
            let mut fields = Vec::new();
            if let Some(value) = usage.prompt_tokens {
                fields.push(("prompt_tokens", Value::Number(value as f64)));
            }
            if let Some(value) = usage.completion_tokens {
                fields.push(("completion_tokens", Value::Number(value as f64)));
            }
            Value::object(fields)
        });
        Value::object(vec![
            ("model", Value::String(self.model.clone())),
            ("usage", usage),
        ])
    }
}

pub fn endpoint_for(base: &Text) -> Result<String, Error> {
    let mut url = Url::parse(&String::from_utf16_lossy(&base.0))
        .map_err(|_| Error::type_error("llm.base_url must be an HTTP or HTTPS URL."))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some_and(|value| !value.is_empty())
        || url.fragment().is_some_and(|value| !value.is_empty())
    {
        return Err(Error::type_error(
            "llm.base_url must be an HTTP or HTTPS URL without credentials, a query or a fragment.",
        ));
    }
    let path = format!("{}/chat/completions", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url.to_string())
}

pub fn parse_completion(payload: &Value, requested_model: &Text) -> Result<Completion, Failure> {
    let choice = payload
        .get("choices")
        .and_then(domain::items)
        .and_then(|choices| choices.first());
    let message = choice.and_then(|choice| choice.get("message"));
    let Some(message) = message.filter(|message| {
        domain::members(message).is_some()
            && message
                .get("role")
                .and_then(Value::as_string)
                .is_some_and(|role| !role.0.is_empty())
            && (matches!(
                message.get("content"),
                Some(Value::String(_) | Value::Null | Value::Array(_))
            ) || message.get("tool_calls").and_then(domain::items).is_some()
                || message
                    .get("function_call")
                    .and_then(domain::members)
                    .is_some())
    }) else {
        return Err(Failure::chat(
            "The LLM response did not contain a valid message.",
            Category::Response,
        ));
    };
    let reason = choice.and_then(|choice| choice.get("finish_reason"));
    if reason.is_some_and(|reason| !matches!(reason, Value::Null | Value::String(_))) {
        return Err(Failure::chat(
            "The LLM response had an invalid finish reason.",
            Category::Response,
        ));
    }
    let usage = payload.get("usage").filter(|usage| **usage != Value::Null);
    if usage.is_some_and(|usage| {
        domain::members(usage).is_none()
            || ["prompt_tokens", "completion_tokens", "total_tokens"]
                .iter()
                .any(|field| {
                    usage.get(field).is_some() && domain::safe_integer(usage.get(field)).is_none()
                })
    }) {
        return Err(Failure::chat(
            "The LLM response contained invalid usage.",
            Category::Response,
        ));
    }
    let requested = Value::String(requested_model.clone());
    let model = payload
        .get("model")
        .filter(|model| **model != Value::Null)
        .unwrap_or(&requested)
        .as_string()
        .filter(|model| {
            !trim(&model.0).is_empty()
                && trim(&model.0) == model.0
                && !model.0.iter().any(|unit| matches!(*unit, 0 | 10 | 13))
        })
        .ok_or_else(|| {
            Failure::chat(
                "The LLM response contained an invalid model.",
                Category::Response,
            )
        })?;
    let mut message = message.clone();
    domain::remove(&mut message, "reasoning_content")?;
    Ok(Completion {
        message,
        usage: usage.cloned(),
        model: model.clone(),
        finish_reason: reason.cloned(),
    })
}

pub fn reported_usage(usage: Option<&Value>) -> Option<Value> {
    let usage = usage.filter(|usage| domain::members(usage).is_some())?;
    let prompt = domain::safe_integer(usage.get("prompt_tokens"))?;
    let mut fields = vec![("input", Value::Number(prompt as f64))];
    if let Some(value) = domain::safe_integer(usage.get("completion_tokens")) {
        fields.push(("output", Value::Number(value as f64)));
    }
    if let Some(value) = usage
        .get("prompt_tokens_details")
        .and_then(|details| domain::safe_integer(details.get("cached_tokens")))
    {
        fields.push(("cached", Value::Number(value as f64)));
    }
    Some(Value::object(fields))
}

pub fn served_model_mismatch(requested: &Text, served: &Text) -> bool {
    let safe = |value: &Text| {
        (1..=128).contains(&value.0.len())
            && value.0.iter().all(|unit| {
                *unit <= 127
                    && ((*unit as u8).is_ascii_alphanumeric()
                        || matches!(*unit, 46 | 95 | 58 | 47 | 64 | 43 | 45))
            })
    };
    if !safe(requested) || !safe(served) {
        return false;
    }
    let base = |value: &Text| {
        String::from_utf16_lossy(&value.0)
            .to_ascii_lowercase()
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_owned()
    };
    let requested = base(requested);
    let served = base(served);
    !requested.contains(&served) && !served.contains(&requested)
}

fn legacy_http_date(value: &str) -> Option<OffsetDateTime> {
    let parts = value.split_ascii_whitespace().collect::<Vec<_>>();
    let (year, month, day, clock) = match parts.as_slice() {
        [weekday, date, clock, zone]
            if [
                "Sunday,",
                "Monday,",
                "Tuesday,",
                "Wednesday,",
                "Thursday,",
                "Friday,",
                "Saturday,",
            ]
            .iter()
            .any(|candidate| weekday.eq_ignore_ascii_case(candidate))
                && zone.eq_ignore_ascii_case("GMT") =>
        {
            let date = date.split('-').collect::<Vec<_>>();
            let [day, month, year] = date.as_slice() else {
                return None;
            };
            if year.len() != 2 || !year.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let year = year.parse::<i32>().ok()?;
            let year = if year < 50 {
                2000 + year
            } else if year < 100 {
                1900 + year
            } else {
                year
            };
            (year, *month, day.parse::<u8>().ok()?, *clock)
        }
        [weekday, month, day, clock, year]
            if ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
                .iter()
                .any(|candidate| weekday.eq_ignore_ascii_case(candidate)) =>
        {
            (
                year.parse::<i32>().ok()?,
                *month,
                day.parse::<u8>().ok()?,
                *clock,
            )
        }
        _ => return None,
    };
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|candidate| month.eq_ignore_ascii_case(candidate))?;
    let month = Month::try_from((month + 1) as u8).ok()?;
    let clock = clock
        .split(':')
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let [hour, minute, second] = clock.as_slice() else {
        return None;
    };
    Some(
        Date::from_calendar_date(year, month, day)
            .ok()?
            .with_hms(*hour, *minute, *second)
            .ok()?
            .assume_utc(),
    )
}

pub fn retry_delay(value: Option<&str>, timeout_ms: u32, now_ms: i64) -> f64 {
    let default = 1000.0_f64.min(timeout_ms as f64);
    let Some(value) = value else {
        return default;
    };
    let numeric = !value.is_empty()
        && value.split('.').count() <= 2
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    let milliseconds = if numeric {
        value.parse::<f64>().map(|value| value * 1000.0).ok()
    } else {
        OffsetDateTime::parse(value, &Rfc2822)
            .or_else(|_| OffsetDateTime::parse(value, &Rfc3339))
            .ok()
            .or_else(|| legacy_http_date(value))
            .map(|date| (date.unix_timestamp_nanos() / 1_000_000) as f64 - now_ms as f64)
    };
    milliseconds
        .filter(|value| value.is_finite())
        .map_or(default, |value| value.clamp(0.0, timeout_ms as f64))
}

struct Prepared {
    body: Vec<u8>,
    model: Text,
    streaming: bool,
    effort: Option<Value>,
}

fn body_failure(error: Error, streaming: bool) -> Failure {
    if error.code == Some("ROSTER_LLM_BODY_LIMIT") {
        Failure::chat(
            if streaming {
                "The LLM stream exceeded the supported size."
            } else {
                "The LLM response exceeded the supported size."
            },
            Category::Response,
        )
    } else if streaming {
        Failure::stream_read()
    } else {
        Failure::json()
    }
}

pub struct Chat {
    llm: Value,
    endpoint: String,
    host: String,
    local: bool,
    optional_key: bool,
    ignore_served_label: bool,
    timeout_ms: u32,
    idle_ms: u32,
    request_limit: Option<u32>,
    request_count: Cell<u64>,
    last_response: RefCell<Option<LastResponse>>,
    mismatch_reported: Cell<bool>,
}

fn timeout_value(llm: &Value, field: &str) -> Option<f64> {
    llm.get(field)
        .map(|value| value.as_number().unwrap_or(f64::NAN))
}

impl Chat {
    pub fn new(config: &Value) -> Result<Option<Self>, Error> {
        if domain::members(config).is_none()
            || config
                .get("llm")
                .is_some_and(|llm| domain::members(llm).is_none())
        {
            return Err(Error::type_error(
                "LLM configuration must be an object with an optional llm object.",
            ));
        }
        let llm = config
            .get("llm")
            .cloned()
            .unwrap_or(Value::object(Vec::new()));
        let request_limit = if llm.get("max_requests").is_some() {
            Some(
                domain::safe_integer(llm.get("max_requests"))
                    .filter(|limit| (1..=10000).contains(limit))
                    .ok_or_else(|| {
                        Error::type_error("llm.max_requests must be an integer from 1 to 10000.")
                    })? as u32,
            )
        } else {
            None
        };
        let Some(base) = llm.get("base_url") else {
            return Ok(None);
        };
        let base = base
            .as_string()
            .ok_or_else(|| Error::type_error("llm.base_url must be a string."))?;
        if trim(&base.0).is_empty() {
            return Ok(None);
        }
        let endpoint = endpoint_for(base)?;
        let url = Url::parse(&endpoint)
            .map_err(|_| Error::type_error("llm.base_url must be an HTTP or HTTPS URL."))?;
        let hostname = url.host_str().unwrap_or("");
        let host = url[Position::BeforeHost..Position::AfterPort].to_owned();
        let local = request::is_local_llm_host(hostname);
        let timeout_ms = request::request_timeout(
            hostname,
            timeout_value(&llm, "request_timeout_ms"),
            timeout_value(&llm, "timeout_ms"),
        )?;
        let idle_ms =
            request::stream_idle_timeout(hostname, timeout_value(&llm, "stream_idle_timeout_ms"))?;
        let optional_key = match llm.get("api_key_optional") {
            None | Some(Value::Null) => false,
            Some(Value::Boolean(value)) => *value,
            _ => return Err(Error::type_error("llm.api_key_optional must be a boolean.")),
        };
        if llm
            .get("served_model_label")
            .is_some_and(|label| !is_text(Some(label), "trust") && !is_text(Some(label), "ignore"))
        {
            return Err(Error::type_error(
                "llm.served_model_label must be trust or ignore.",
            ));
        }
        let ignore_served_label = is_text(llm.get("served_model_label"), "ignore");
        Ok(Some(Self {
            llm,
            endpoint,
            host,
            local,
            optional_key,
            ignore_served_label,
            timeout_ms,
            idle_ms,
            request_limit,
            request_count: Cell::new(0),
            last_response: RefCell::new(None),
            mismatch_reported: Cell::new(false),
        }))
    }

    pub fn request_count(&self) -> u64 {
        self.request_count.get()
    }

    pub fn request_limit(&self) -> Option<u32> {
        self.request_limit
    }

    pub fn last_response(&self) -> Result<Option<LastResponse>, Error> {
        self.last_response
            .try_borrow()
            .map(|response| response.clone())
            .map_err(|_| Error::new("Error", "LLM response state is already borrowed"))
    }

    pub fn take_last_response(&self) -> Result<Option<LastResponse>, Error> {
        self.last_response
            .try_borrow_mut()
            .map(|mut response| response.take())
            .map_err(|_| Error::new("Error", "LLM response state is already borrowed"))
    }

    pub fn timeout_ms(&self) -> u32 {
        self.timeout_ms
    }

    pub fn stream_idle_ms(&self) -> u32 {
        self.idle_ms
    }

    pub(super) fn is_local(&self) -> bool {
        self.local
    }

    pub(super) fn event_host(&self, credentials: &Credentials) -> Result<String, Error> {
        let key_name = self.key_name("OPENAI_API_KEY");
        let key = credentials.resolve(&self.key_names())?;
        let empty = Value::object(Vec::new());
        let redactor = Redactor::new(
            credentials.environment.as_ref().unwrap_or(&empty),
            Some(&key_name),
        );
        let mut host = redactor.redact(&Text::from(self.host.as_str()), key.as_ref());
        host.0.truncate(256);
        for unit in &mut host.0 {
            if matches!(*unit, 0..=31 | 127) {
                *unit = 63;
            }
        }
        host.to_utf8()
    }

    fn prepare(&self, request: &Value) -> Result<Prepared, Error> {
        let messages = request.get("messages").and_then(domain::items);
        if domain::members(request).is_none()
            || messages.is_none_or(|messages| {
                messages.is_empty()
                    || messages.iter().any(|message| {
                        domain::members(message).is_none()
                            || message
                                .get("role")
                                .and_then(Value::as_string)
                                .is_none_or(|role| role.0.is_empty())
                    })
            })
        {
            return Err(Error::type_error(
                "A chat request requires a non-empty messages array with message roles.",
            ));
        }
        let model = request
            .get("model")
            .filter(|model| **model != Value::Null)
            .or_else(|| self.llm.get("model"))
            .and_then(Value::as_string)
            .filter(|model| !trim(&model.0).is_empty())
            .ok_or_else(|| Error::type_error("A chat model is required."))?;
        if request
            .get("stream")
            .is_some_and(|stream| !matches!(stream, Value::Boolean(_)))
        {
            return Err(Error::type_error(
                "A streaming chat request must set stream to a boolean.",
            ));
        }
        let streaming = request.get("stream") == Some(&Value::Boolean(true));
        let mut body = Value::object(Vec::new());
        for field in ["reasoning_effort", "max_tokens", "chat_template_kwargs"] {
            if let Some(value) = self.llm.get(field) {
                put(&mut body, field, value.clone())?;
            }
        }
        if let Some(entries) = domain::members(request) {
            for (name, value) in entries {
                if ascii(&name.0, "tools") || ascii(&name.0, "tool_choice") {
                    continue;
                }
                let Value::Object(fields) = &mut body else {
                    return Err(Error::type_error(
                        "The chat request must be JSON serializable.",
                    ));
                };
                if let Some((_, prior)) = fields.iter_mut().find(|(key, _)| key == name) {
                    *prior = value.clone();
                } else {
                    fields.push((name.clone(), value.clone()));
                }
            }
        }
        let empty_tools = request
            .get("tools")
            .and_then(domain::items)
            .is_some_and(|tools| tools.is_empty());
        if !empty_tools {
            for field in ["tools", "tool_choice"] {
                if let Some(value) = request.get(field) {
                    put(&mut body, field, value.clone())?;
                }
            }
        }
        put(&mut body, "model", Value::String(model.clone()))?;
        put(&mut body, "stream", Value::Boolean(streaming))?;
        if streaming {
            put(
                &mut body,
                "stream_options",
                Value::object(vec![("include_usage", Value::Boolean(true))]),
            )?;
        }
        let body = body.json().into_bytes();
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(Error::type_error(
                "The LLM request exceeded the supported size.",
            ));
        }
        Ok(Prepared {
            body,
            model: model.clone(),
            streaming,
            effort: request
                .get("reasoning_effort")
                .filter(|value| **value != Value::Null)
                .or_else(|| self.llm.get("reasoning_effort"))
                .cloned(),
        })
    }

    fn check_limit(&self) -> Result<(), Failure> {
        if let Some(limit) = self
            .request_limit
            .filter(|limit| self.request_count.get() >= *limit as u64)
        {
            let mut failure = Failure::from(
                Error::new(
                    "ModelRequestLimitError",
                    format!(
                        "Model HTTP request budget ({limit}) exhausted for this transport context."
                    ),
                )
                .coded("ROSTER_MODEL_REQUEST_LIMIT"),
            );
            failure.limit = Some(limit);
            return Err(failure);
        }
        Ok(())
    }

    fn key_name(&self, fallback: &str) -> Text {
        self.llm
            .get("api_key_name")
            .filter(|value| **value != Value::Null)
            .and_then(Value::as_string)
            .cloned()
            .unwrap_or_else(|| {
                if self
                    .llm
                    .get("api_key_name")
                    .is_some_and(|value| !matches!(value, Value::Null | Value::String(_)))
                {
                    Text::default()
                } else {
                    Text::from(fallback)
                }
            })
    }

    fn key_names(&self) -> Vec<Text> {
        if self.endpoint.contains("aperture") {
            vec![
                Text::from("APERTURE_API_KEY"),
                self.key_name("ROSTER_API_KEY"),
            ]
        } else {
            vec![self.key_name("OPENAI_API_KEY")]
        }
    }

    async fn read_payload<B: Body>(
        &self,
        response: http::Response<B>,
        context: &mut Attempt<'_, '_, impl Observer>,
    ) -> Result<Value, Failure> {
        let streaming = context.prepared.streaming
            && response.body.is_some()
            && !response
                .header("content-type")
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("application/json");
        let Some(mut body) = response.body else {
            return if streaming {
                Err(Failure::chat(
                    "The LLM stream did not contain a readable body.",
                    Category::Response,
                ))
            } else {
                Err(Failure::json())
            };
        };
        if streaming {
            let mut decoder = StreamDecoder::new(context.prepared.model.clone());
            let mut idle = self.arm_idle();
            loop {
                let bytes = context
                    .control
                    .wait(body.read(), context.observer, idle)
                    .await?
                    .map_err(|error| body_failure(error, true))?;
                let Some(bytes) = bytes else {
                    break;
                };
                if !bytes.is_empty() {
                    idle = self.arm_idle();
                }
                let deltas = match decoder.push(&bytes) {
                    Ok(deltas) => deltas,
                    Err(error) => {
                        for delta in decoder.take_deltas() {
                            context.delta(delta, idle).await?;
                        }
                        return Err(error);
                    }
                };
                for delta in deltas {
                    context.delta(delta, idle).await?;
                }
                tokio::task::yield_now().await;
                context.control.check()?;
                if decoder.done() {
                    break;
                }
            }
            drop(body);
            let (payload, deltas) = match decoder.finish() {
                Ok(result) => result,
                Err(error) => {
                    for delta in decoder.take_deltas() {
                        context.delta(delta, idle).await?;
                    }
                    return Err(error);
                }
            };
            for delta in deltas {
                context.delta(delta, idle).await?;
            }
            context.control.check()?;
            Ok(payload)
        } else {
            let mut bytes = Vec::new();
            loop {
                let chunk = context
                    .control
                    .wait(body.read(), context.observer, None)
                    .await?
                    .map_err(|error| body_failure(error, false))?;
                let Some(chunk) = chunk else {
                    break;
                };
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err(Failure::chat(
                        "The LLM response exceeded the supported size.",
                        Category::Response,
                    ));
                }
                bytes.extend(chunk);
                tokio::task::yield_now().await;
                context.control.check()?;
            }
            let text = String::from_utf8_lossy(&bytes);
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            let payload = json::parse(&Text::from(text)).map_err(|_| Failure::json())?;
            tokio::task::yield_now().await;
            context.control.check()?;
            Ok(payload)
        }
    }

    fn arm_idle(&self) -> Option<(Instant, u32)> {
        (self.idle_ms > 0).then(|| {
            (
                Instant::now() + Duration::from_millis(self.idle_ms as u64),
                self.idle_ms,
            )
        })
    }

    async fn send_once<T: Transport>(
        &self,
        transport: &mut T,
        outgoing: &http::Request,
        context: &mut Attempt<'_, '_, impl Observer>,
    ) -> Result<Completion, Failure> {
        for attempt in 0..2 {
            context.control.check()?;
            self.check_limit()?;
            context
                .control
                .emit(
                    context.observer,
                    Value::object(vec![
                        ("type", Value::string("model")),
                        (
                            "model",
                            Value::String(
                                context
                                    .redactor
                                    .redact(&context.prepared.model, context.key),
                            ),
                        ),
                        ("host", Value::string(context.host)),
                    ]),
                    None,
                )
                .await?;
            let mut start = Value::object(vec![
                ("type", Value::string("http")),
                ("phase", Value::string("start")),
            ]);
            if let Some(effort) = &context.prepared.effort {
                put(&mut start, "effort", effort.clone())?;
            }
            context.control.emit(context.observer, start, None).await?;
            context.control.check()?;
            self.check_limit()?;
            self.request_count.set(
                self.request_count
                    .get()
                    .checked_add(1)
                    .ok_or_else(|| Error::new("Error", "LLM HTTP request counter overflowed"))?,
            );
            let response = context
                .control
                .wait(transport.send(outgoing), context.observer, None)
                .await??;
            if !(100..=599).contains(&response.status) {
                return Err(Failure::chat(
                    "The LLM response had an invalid HTTP status.",
                    Category::Response,
                ));
            }
            context.status = Some(response.status);
            if !(200..=299).contains(&response.status) {
                let status = response.status;
                let transient = matches!(status, 502..=504);
                let retry = attempt == 0 && (status == 429 || transient);
                let delay = retry_delay(
                    response.header("retry-after"),
                    self.timeout_ms,
                    clock::now_milliseconds()?,
                );
                drop(response);
                if retry {
                    context.http_error(Category::Http, false).await?;
                    context
                        .control
                        .wait(
                            tokio::time::sleep(
                                Duration::from_millis(delay.trunc().max(1.0) as u64),
                            ),
                            context.observer,
                            None,
                        )
                        .await?;
                    continue;
                }
                let mut error = Failure::chat(
                    format!("The LLM request failed (HTTP {status})."),
                    Category::Http,
                );
                error.transient = transient;
                return Err(error);
            }
            let payload = match self.read_payload(response, context).await {
                Ok(payload) => payload,
                Err(error) if error.transient && attempt == 0 => {
                    context.http_error(Category::Http, false).await?;
                    context
                        .control
                        .wait(
                            tokio::time::sleep(Duration::from_millis(
                                1000.min(self.timeout_ms) as u64
                            )),
                            context.observer,
                            None,
                        )
                        .await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            context.control.check()?;
            let completion = parse_completion(&payload, &context.prepared.model)?;
            context.observed = Some(LastResponse {
                model: if self.ignore_served_label {
                    context.prepared.model.clone()
                } else {
                    completion.model.clone()
                },
                usage: completion
                    .usage
                    .as_ref()
                    .map(|_| completion.measured_usage())
                    .transpose()?,
            });
            if !self.ignore_served_label {
                context
                    .control
                    .emit(
                        context.observer,
                        Value::object(vec![
                            ("type", Value::string("model")),
                            (
                                "model",
                                Value::String(
                                    context.redactor.redact(&completion.model, context.key),
                                ),
                            ),
                            ("host", Value::string(context.host)),
                        ]),
                        None,
                    )
                    .await?;
            }
            context
                .control
                .emit(
                    context.observer,
                    Value::object(vec![
                        ("type", Value::string("http")),
                        ("phase", Value::string("ok")),
                        (
                            "status",
                            Value::Number(context.status.unwrap_or(200) as f64),
                        ),
                    ]),
                    None,
                )
                .await?;
            if let Some(mut usage) = reported_usage(completion.usage.as_ref()) {
                put(&mut usage, "type", Value::string("usage"))?;
                context.control.emit(context.observer, usage, None).await?;
            }
            return Ok(completion);
        }
        Err(Failure::network())
    }

    pub async fn complete<T: Transport>(
        &self,
        request: &Value,
        transport: &mut T,
        observer: &mut impl Observer,
        credentials: &Credentials,
        options: CallOptions<'_>,
    ) -> Result<Completion, Failure> {
        for token in [options.parent, options.signal].into_iter().flatten() {
            token.check()?;
        }
        let prepared = self.prepare(request)?;
        request::validate_retry_command(options.retry_command)?;
        let key_name = self.key_name("OPENAI_API_KEY");
        let key = credentials.resolve(&self.key_names())?;
        let environment = credentials
            .environment
            .as_ref()
            .cloned()
            .unwrap_or(Value::object(Vec::new()));
        let redactor = Redactor::new(&environment, Some(&key_name));
        let host = redactor
            .redact(&Text::from(self.host.as_str()), None)
            .to_utf8()?;
        let mut control = RequestControl::new(
            &host,
            self.local,
            self.timeout_ms,
            options.retry_command,
            options.parent,
            options.signal,
        );
        if key.is_none() && !self.optional_key {
            control
                .emit(
                    observer,
                    Value::object(vec![
                        ("type", Value::string("http")),
                        ("phase", Value::string("error")),
                        ("errorClass", Value::string("authentication")),
                    ]),
                    None,
                )
                .await?;
            return Err(Failure::chat(
                "An LLM API key is required. Set the configured environment variable or vault entry.",
                Category::Authentication,
            ));
        }
        let mut headers = vec![("Content-Type".to_owned(), "application/json".to_owned())];
        if let Some(key) = &key {
            let key = key.to_utf8().map_err(|_| Failure::network())?;
            headers.push(("Authorization".to_owned(), format!("Bearer {key}")));
        }
        let outgoing = http::Request {
            url: self.endpoint.clone(),
            method: "POST".to_owned(),
            headers,
            body: prepared.body.clone(),
        };
        let mut context = Attempt {
            prepared: &prepared,
            observer,
            redactor: &redactor,
            key: key.as_ref(),
            host: &host,
            control: &mut control,
            status: None,
            observed: None,
        };
        let result = async {
            let mut stall_attempt = 0;
            let received = loop {
                match self.send_once(transport, &outgoing, &mut context).await {
                    Ok(completion) => break completion,
                    Err(error) if error.error.code == Some("ROSTER_LLM_STALL") => {
                        context.control.check()?;
                        let retry = stall_attempt == 0;
                        context
                            .control
                            .emit(
                                context.observer,
                                Value::object(vec![
                                    ("type", Value::string("stall")),
                                    ("host", Value::string(&host)),
                                    ("idleSeconds", Value::Number(self.idle_ms as f64 / 1000.0)),
                                    ("retry", Value::Boolean(retry)),
                                ]),
                                None,
                            )
                            .await?;
                        if !retry {
                            return Err(error);
                        }
                        stall_attempt += 1;
                    }
                    Err(error) => return Err(error),
                }
            };
            let response = if self.ignore_served_label {
                Completion {
                    model: prepared.model.clone(),
                    ..received
                }
            } else {
                received
            };
            context.control.check()?;
            let snapshot = LastResponse {
                model: response.model.clone(),
                usage: response
                    .usage
                    .as_ref()
                    .map(|_| response.measured_usage())
                    .transpose()?,
            };
            if !self.mismatch_reported.get()
                && served_model_mismatch(&prepared.model, &response.model)
            {
                self.mismatch_reported.set(true);
                context
                    .control
                    .emit(
                        context.observer,
                        Value::object(vec![
                            ("type", Value::string("served-model")),
                            ("host", Value::string(&host)),
                            ("requested", Value::String(prepared.model.clone())),
                            ("served", Value::String(response.model.clone())),
                        ]),
                        None,
                    )
                    .await?;
            }
            tokio::task::yield_now().await;
            context.control.check()?;
            *self
                .last_response
                .try_borrow_mut()
                .map_err(|_| Error::new("Error", "LLM response state is already borrowed"))? =
                Some(snapshot);
            if let Some(Value::String(reason)) = &response.finish_reason {
                if !ascii(&reason.0, "stop") && !ascii(&reason.0, "tool_calls") {
                    let mut safe = redactor.redact(reason, key.as_ref());
                    safe.0 = safe
                        .0
                        .into_iter()
                        .take(128)
                        .map(|unit| {
                            if matches!(unit, 0..=31 | 127) {
                                63
                            } else {
                                unit
                            }
                        })
                        .collect();
                    if safe.0.is_empty() {
                        safe = Text::from("(empty)");
                    }
                    let mut error = Failure::chat(
                        format!(
                            "The LLM response had an unsupported finish reason: {}.",
                            String::from_utf16_lossy(&safe.0)
                        ),
                        Category::Response,
                    );
                    error.error.code = Some("ROSTER_LLM_FINISH_REASON");
                    let mut message =
                        Text::from("The LLM response had an unsupported finish reason: ");
                    message.0.extend_from_slice(&safe.0);
                    message.push_str(".");
                    error.message_text = Some(message);
                    error.finish_reason = Some(safe);
                    error.truncated = Some(ascii(&reason.0, "length"));
                    error.partial = Some(Box::new(response.message.clone()));
                    return Err(error);
                }
            }
            Ok(response)
        }
        .await;
        let result = async {
            match result {
                Ok(response) => Ok(response),
                Err(error) => {
                    for token in [options.parent, options.signal].into_iter().flatten() {
                        token.check()?;
                    }
                    if matches!(
                        error.error.code,
                        Some(
                            "ROSTER_MODEL_REQUEST_LIMIT"
                                | "ROSTER_RUN_LOG"
                                | "ROSTER_CANCELLED"
                                | "ROSTER_TASK_TIMEOUT"
                        )
                    ) {
                        return Err(error);
                    }
                    if error.error.code == Some("ROSTER_LLM_TIMEOUT") {
                        let mut event = Value::object(vec![
                            ("type", Value::string("timeout")),
                            ("host", Value::string(&host)),
                            ("local", Value::Boolean(self.local)),
                            ("timeoutMs", Value::Number(self.timeout_ms as f64)),
                        ]);
                        if let Some(retry) = options.retry_command {
                            put(&mut event, "retryCommand", Value::string(retry))?;
                        }
                        context
                            .control
                            .emit_terminal(context.observer, event)
                            .await?;
                    }
                    context
                        .http_error(error.category.unwrap_or(Category::Network), true)
                        .await?;
                    for token in [options.parent, options.signal].into_iter().flatten() {
                        token.check()?;
                    }
                    Err(error)
                }
            }
        }
        .await;
        if result.as_ref().is_err_and(|error| {
            !matches!(
                error.error.code,
                Some(
                    "ROSTER_CANCELLED"
                        | "ROSTER_TASK_TIMEOUT"
                        | "ROSTER_LLM_TIMEOUT"
                        | "ROSTER_LLM_STALL"
                )
            )
        }) {
            for token in [options.parent, options.signal].into_iter().flatten() {
                token.check()?;
            }
            if let Some(observed) = context.observed.take() {
                *self
                    .last_response
                    .try_borrow_mut()
                    .map_err(|_| Error::new("Error", "LLM response state is already borrowed"))? =
                    Some(observed);
            }
        }
        result
    }
}

#[derive(Default, Clone, Copy)]
pub struct CallOptions<'a> {
    pub parent: Option<&'a CancellationToken>,
    pub signal: Option<&'a CancellationToken>,
    pub retry_command: Option<&'a str>,
}

struct Attempt<'a, 'b, O> {
    prepared: &'a Prepared,
    observer: &'a mut O,
    redactor: &'a Redactor,
    key: Option<&'a Text>,
    host: &'a str,
    control: &'a mut RequestControl<'b>,
    status: Option<u16>,
    observed: Option<LastResponse>,
}

impl<O: Observer> Attempt<'_, '_, O> {
    async fn delta(&mut self, text: Text, idle: Option<(Instant, u32)>) -> Result<(), Failure> {
        let text = self.redactor.redact(&text, self.key);
        if !text.0.is_empty() && self.observer.enabled() {
            self.control
                .emit(
                    self.observer,
                    Value::object(vec![
                        ("type", Value::string("delta")),
                        ("text", Value::String(text)),
                    ]),
                    idle,
                )
                .await?;
        }
        Ok(())
    }

    async fn http_error(&mut self, category: Category, terminal: bool) -> Result<(), Failure> {
        let mut event = Value::object(vec![
            ("type", Value::string("http")),
            ("phase", Value::string("error")),
            ("errorClass", Value::string(category.as_str())),
        ]);
        if let Some(status) = self.status {
            put(&mut event, "status", Value::Number(status as f64))?;
        }
        if terminal {
            self.control.emit_terminal(self.observer, event).await?;
        } else {
            self.control.emit(self.observer, event, None).await?;
        }
        Ok(())
    }
}
