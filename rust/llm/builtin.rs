use std::time::Duration;

use tokio::time::Instant;

use crate::domain::{self, put};
use crate::json::{self, Value};
use crate::text::{Text, trim};
use crate::{Error, capabilities, run};

use super::Failure;
use super::admission::{Admission, Lease};
use super::baseline;
use super::control::{Observer, RequestControl};
use super::http::Transport;
use super::openai::{CallOptions, Chat, Completion, Credentials, LastResponse};
use super::reasoning;

const MAX_LENGTH_RETRIES: u32 = 3;
const MAX_REASONING_CAP: u64 = 32_768;
const QUEUED_HEARTBEAT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub retry_length: bool,
    pub stream: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            retry_length: true,
            stream: false,
        }
    }
}

pub fn default_completion_cap(llm: &Value) -> Result<u64, Error> {
    let cap = match reasoning::mapped_effort(llm, None)? {
        "low" => 8192,
        "medium" | "high" => 16_384,
        "xhigh" | "max" => 32_768,
        _ => 4096,
    };
    let context_bound = domain::safe_integer(llm.get("context_max"))
        .filter(|context| *context > 0)
        .map_or(cap, |context| context / 4);
    Ok(cap.min(context_bound).max(4096))
}

fn check(options: CallOptions<'_>) -> Result<(), Failure> {
    for token in [options.parent, options.signal].into_iter().flatten() {
        token.check()?;
    }
    Ok(())
}

fn revoked(failure: &Failure) -> bool {
    matches!(
        failure.error.code,
        Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT" | "ROSTER_LLM_TIMEOUT")
    )
}

fn nonnull(value: Option<&Value>) -> Option<&Value> {
    value.filter(|value| **value != Value::Null)
}

fn usage_value(usage: run::Usage) -> Result<Value, Error> {
    json::parse(&Text::from(usage.json().as_str()))
}

fn merge_usage(usages: &[Option<Value>]) -> Result<Value, Error> {
    let samples = usages
        .iter()
        .map(|usage| run::UsageSample {
            prompt_tokens: usage
                .as_ref()
                .and_then(|usage| usage.get("prompt_tokens"))
                .and_then(Value::as_number),
            completion_tokens: usage
                .as_ref()
                .and_then(|usage| usage.get("completion_tokens"))
                .and_then(Value::as_number),
        })
        .collect::<Vec<_>>();
    usage_value(run::merge_usage(&samples)?)
}

struct SeatObserver<'a, O> {
    observer: &'a mut O,
    prior: &'a Value,
    max_tokens: u64,
    thinking: bool,
}

impl<O: Observer> Observer for SeatObserver<'_, O> {
    fn enabled(&self) -> bool {
        self.observer.enabled()
    }

    async fn emit(&mut self, mut event: Value) -> Result<(), Error> {
        if domain::is_text(event.get("type"), "http") {
            put(&mut event, "modelPrior", self.prior.clone())?;
            if domain::is_text(event.get("phase"), "start") {
                put(
                    &mut event,
                    "maxTokens",
                    Value::Number(self.max_tokens as f64),
                )?;
                put(&mut event, "thinking", Value::Boolean(self.thinking))?;
            }
        }
        self.observer.emit(event).await
    }
}

pub struct BuiltinChat<'a> {
    transport: Chat,
    admission: &'a Admission,
    profile: Option<Text>,
    concurrency: Option<u64>,
    prior: Value,
    docs_slice: bool,
    deepseek: bool,
    default_thinking: bool,
    options: Options,
    completion_cap: Value,
    length_retries: u32,
    last_attempts: u32,
    last_usage: Option<Value>,
    last_response: Option<LastResponse>,
}

impl<'a> BuiltinChat<'a> {
    pub fn new(
        config: &Value,
        admission: &'a Admission,
        options: Options,
    ) -> Result<Option<Self>, Error> {
        if domain::members(config).is_none()
            || config
                .get("llm")
                .is_some_and(|llm| domain::members(llm).is_none())
        {
            return Err(Error::type_error(
                "LLM configuration must be an object with an optional llm object.",
            ));
        }
        let empty = Value::object(Vec::new());
        let llm = config.get("llm").unwrap_or(&empty);
        let docs_slice = domain::is_text(llm.get("task_kind"), "slice")
            && domain::is_text(llm.get("task_class"), "docs");
        let deepseek = reasoning::uses_deepseek_reasoning(llm)?;
        let mut outbound = Value::object(Vec::new());
        for field in [
            "base_url",
            "model",
            "request_timeout_ms",
            "max_requests",
            "stream_idle_timeout_ms",
            "max_tokens",
        ] {
            if let Some(value) = llm.get(field) {
                put(&mut outbound, field, value.clone())?;
            }
        }
        if let Some(name) = llm.get("api_key_env") {
            put(&mut outbound, "api_key_name", name.clone())?;
        }
        put(
            &mut outbound,
            "api_key_optional",
            nonnull(llm.get("api_key_optional"))
                .cloned()
                .unwrap_or(Value::Boolean(true)),
        )?;
        if llm.get("locked_model").is_some_and(domain::truthy) {
            put(&mut outbound, "served_model_label", Value::string("ignore"))?;
        } else if let Some(label) = llm.get("served_model_label") {
            put(&mut outbound, "served_model_label", label.clone())?;
        }
        let effort = if docs_slice {
            "none"
        } else {
            reasoning::mapped_effort(llm, None)?
        };
        put(&mut outbound, "reasoning_effort", Value::string(effort))?;
        let default_thinking = !docs_slice && !domain::is_text(llm.get("effort"), "none");
        if deepseek {
            put(
                &mut outbound,
                "chat_template_kwargs",
                Value::object(vec![("thinking", Value::Boolean(default_thinking))]),
            )?;
        }
        let Some(transport) = Chat::new(&Value::object(vec![("llm", outbound)]))? else {
            return Ok(None);
        };
        let completion_cap = if docs_slice {
            Value::Number(reasoning::docs_completion_cap(llm) as f64)
        } else {
            nonnull(llm.get("max_tokens"))
                .cloned()
                .unwrap_or(Value::Number(default_completion_cap(llm)? as f64))
        };
        let profile = llm
            .get("fleet_profile")
            .filter(|value| domain::truthy(value))
            .map(|profile| {
                profile
                    .as_string()
                    .filter(|profile| !profile.0.is_empty())
                    .cloned()
                    .ok_or_else(|| Error::type_error("profileId must be a non-empty string"))
            })
            .transpose()?;
        let concurrency = llm
            .get("concurrency")
            .map(|value| {
                domain::safe_integer(Some(value))
                    .filter(|capacity| *capacity > 0)
                    .ok_or_else(|| {
                        Error::type_error("llm.concurrency must be a positive safe integer")
                    })
            })
            .transpose()?;
        let prior = match nonnull(llm.get("model_prior")) {
            Some(prior) => prior.clone(),
            None => {
                capabilities::model_prior(llm.get("model").unwrap_or(&Value::Null), None, None)?
                    .get("strength")
                    .cloned()
                    .ok_or_else(|| {
                        Error::type_error("Model capability prior requires a strength")
                    })?
            }
        };
        Ok(Some(Self {
            transport,
            admission,
            profile,
            concurrency,
            prior,
            docs_slice,
            deepseek,
            default_thinking,
            options,
            completion_cap,
            length_retries: 0,
            last_attempts: 0,
            last_usage: None,
            last_response: None,
        }))
    }

    pub fn last_attempts(&self) -> u32 {
        self.last_attempts
    }

    pub fn last_usage(&self) -> Option<&Value> {
        self.last_usage.as_ref()
    }

    pub fn last_response(&self) -> Option<&LastResponse> {
        self.last_response.as_ref()
    }

    pub fn take_last_response(&mut self) -> Option<LastResponse> {
        self.last_response.take()
    }

    pub fn request_count(&self) -> u64 {
        self.transport.request_count()
    }

    pub fn request_limit(&self) -> Option<u32> {
        self.transport.request_limit()
    }

    pub fn timeout_ms(&self) -> u32 {
        self.transport.timeout_ms()
    }

    fn event_control<'b>(&self, host: &'b str, options: CallOptions<'b>) -> RequestControl<'b> {
        RequestControl::new(
            host,
            self.transport.is_local(),
            self.timeout_ms(),
            options.retry_command,
            options.parent,
            options.signal,
        )
    }

    fn queued_event(&self, profile: &Text, host: &str, started: Instant) -> Result<Value, Error> {
        Ok(Value::object(vec![
            ("type", Value::string("waiting")),
            ("host", Value::string(host)),
            ("local", Value::Boolean(false)),
            ("queued", Value::Boolean(true)),
            (
                "elapsedSeconds",
                Value::Number(started.elapsed().as_secs() as f64),
            ),
            (
                "depth",
                Value::Number(self.admission.depth(profile)? as f64),
            ),
        ]))
    }

    async fn admit(
        &self,
        observer: &mut impl Observer,
        host: &str,
        options: CallOptions<'_>,
    ) -> Result<Option<Lease>, Failure> {
        check(options)?;
        let Some(profile) = &self.profile else {
            return Ok(None);
        };
        if let Some(concurrency) = self.concurrency {
            self.admission.configure(profile, concurrency)?;
        }
        let pending = self
            .admission
            .acquire(profile, options.parent, options.signal)?;
        if pending.granted()? {
            return Ok(Some(pending.wait().await?));
        }
        let started = Instant::now();
        if observer.enabled() {
            self.event_control(host, options)
                .emit(observer, self.queued_event(profile, host, started)?, None)
                .await?;
        }
        let waiting = pending.wait();
        tokio::pin!(waiting);
        let mut next = started + QUEUED_HEARTBEAT;
        loop {
            tokio::select! {
                biased;
                result = &mut waiting => return Ok(Some(result?)),
                () = tokio::time::sleep_until(next), if observer.enabled() => {
                    check(options)?;
                    self.event_control(host, options)
                        .emit(observer, self.queued_event(profile, host, started)?, None)
                        .await?;
                    next = (next + QUEUED_HEARTBEAT).max(Instant::now() + QUEUED_HEARTBEAT);
                }
            }
        }
    }

    fn prepare(&self, request: &Value) -> Result<Value, Error> {
        if domain::members(request).is_none() {
            return Err(Error::type_error(
                "A builtin chat request must be an object",
            ));
        }
        let messages =
            baseline::with_engineering_baseline(request.get("messages").unwrap_or(&Value::Null))?;
        let requested = nonnull(request.get("max_tokens")).unwrap_or(&self.completion_cap);
        let mut cap = domain::safe_integer(Some(requested)).ok_or_else(|| {
            Error::type_error("Initial builtin completion cap must be an integer of at least 2")
        })?;
        if self.docs_slice {
            let configured = domain::safe_integer(Some(&self.completion_cap)).ok_or_else(|| {
                Error::type_error("Initial builtin completion cap must be an integer of at least 2")
            })?;
            cap = cap.max(configured);
        }
        if cap < 1 || cap == 1 && self.length_retries == 0 {
            return Err(Error::type_error(
                "Initial builtin completion cap must be an integer of at least 2",
            ));
        }
        let mut current = request.clone();
        put(&mut current, "messages", messages)?;
        put(&mut current, "max_tokens", Value::Number(cap as f64))?;
        if self.options.stream && nonnull(current.get("stream")).is_none() {
            put(&mut current, "stream", Value::Boolean(true))?;
        }
        if self.docs_slice {
            put(&mut current, "reasoning_effort", Value::string("none"))?;
            if self.deepseek {
                put(
                    &mut current,
                    "chat_template_kwargs",
                    Value::object(vec![("thinking", Value::Boolean(false))]),
                )?;
            }
        }
        Ok(current)
    }

    fn thinking(&self, current: &Value) -> bool {
        if self.deepseek {
            nonnull(
                current
                    .get("chat_template_kwargs")
                    .and_then(|kwargs| kwargs.get("thinking")),
            )
            .map_or(self.default_thinking, domain::truthy)
        } else {
            nonnull(current.get("reasoning_effort")).map_or(self.default_thinking, |effort| {
                !domain::is_text(Some(effort), "none")
            })
        }
    }

    fn finish_observation(
        &mut self,
        response: Option<LastResponse>,
        result: Result<Option<Completion>, Failure>,
    ) -> Result<Option<Completion>, Failure> {
        if !result.as_ref().is_err_and(revoked) {
            if let Some(response) = response {
                self.last_response = Some(response);
            }
        }
        result
    }

    pub async fn complete<T: Transport>(
        &mut self,
        request: &Value,
        transport: &mut T,
        observer: &mut impl Observer,
        credentials: &Credentials,
        options: CallOptions<'_>,
    ) -> Result<Completion, Failure> {
        check(options)?;
        let mut current = self.prepare(request)?;
        crate::request::validate_retry_command(options.retry_command)?;
        let host = self.transport.event_host(credentials)?;
        self.last_attempts = 0;
        self.last_usage = None;
        let mut retried = false;
        let mut usages = Vec::new();
        loop {
            check(options)?;
            self.last_attempts += 1;
            let mut lease = self.admit(observer, &host, options).await?;
            let cap = domain::safe_integer(current.get("max_tokens")).ok_or_else(|| {
                Error::type_error("Builtin completion cap must remain a safe integer")
            })?;
            let control = self.event_control(&host, options);
            self.transport.take_last_response()?;
            self.last_usage = if usages.is_empty() {
                None
            } else {
                Some(Value::object(Vec::new()))
            };
            let result = self
                .transport
                .complete(
                    &current,
                    transport,
                    &mut SeatObserver {
                        observer,
                        prior: &self.prior,
                        max_tokens: cap,
                        thinking: self.thinking(&current),
                    },
                    credentials,
                    options,
                )
                .await;
            if let Some(lease) = &mut lease {
                lease.release()?;
            }
            let response = self.transport.take_last_response()?;
            if let Err(cancelled) = check(options) {
                self.last_usage = None;
                if !usages.is_empty() {
                    usages.push(None);
                    self.last_usage = Some(merge_usage(&usages)?);
                }
                return Err(cancelled);
            }
            let observed = async {
                match result {
                    Ok(mut completion) => {
                        usages.push(completion.usage.clone());
                        self.last_usage = None;
                        self.last_usage = if usages.len() == 1 {
                            completion.usage.clone()
                        } else {
                            Some(merge_usage(&usages)?)
                        };
                        control
                            .emit(
                                observer,
                                Value::object(vec![
                                    ("type", Value::string("completion")),
                                    (
                                        "reason",
                                        completion.finish_reason.clone().unwrap_or(Value::Null),
                                    ),
                                ]),
                                None,
                            )
                            .await?;
                        completion.usage = self.last_usage.clone();
                        Ok(Some(completion))
                    }
                    Err(error) if error.error.code == Some("ROSTER_LLM_FINISH_REASON") => {
                        usages.push(
                            response
                                .as_ref()
                                .and_then(|response| response.usage)
                                .map(usage_value)
                                .transpose()?,
                        );
                        self.last_usage = None;
                        self.last_usage = Some(merge_usage(&usages)?);
                        let retry = self.options.retry_length
                            && error.truncated == Some(true)
                            && !retried
                            && self.length_retries < MAX_LENGTH_RETRIES
                            && cap > 1;
                        let mut event = Value::object(vec![
                            ("type", Value::string("finish-reason")),
                            (
                                "reason",
                                error
                                    .finish_reason
                                    .clone()
                                    .map_or(Value::Null, Value::String),
                            ),
                            ("retry", Value::Boolean(retry)),
                        ]);
                        if retry && self.docs_slice {
                            put(&mut event, "continued", Value::Boolean(true))?;
                        }
                        control.emit(observer, event, None).await?;
                        if !retry {
                            return Err(error);
                        }
                        retried = true;
                        self.length_retries += 1;
                        self.continue_request(&mut current, &error, cap)?;
                        Ok(None)
                    }
                    Err(error) => {
                        if !revoked(&error) && response.is_some() {
                            if let Some(response) = &response {
                                usages.push(response.usage.map(usage_value).transpose()?);
                            }
                        } else if !usages.is_empty() {
                            usages.push(None);
                        }
                        self.last_usage = None;
                        self.last_usage = if usages.is_empty() {
                            None
                        } else {
                            Some(merge_usage(&usages)?)
                        };
                        Err(error)
                    }
                }
            }
            .await;
            if let Some(completion) = self.finish_observation(response, observed)? {
                return Ok(completion);
            }
        }
    }

    fn continue_request(
        &mut self,
        current: &mut Value,
        error: &Failure,
        cap: u64,
    ) -> Result<(), Error> {
        let partial = error.partial.as_deref();
        let content = partial
            .and_then(|message| message.get("content"))
            .and_then(Value::as_string)
            .filter(|content| !trim(&content.0).is_empty());
        let tools = partial.is_some_and(|message| {
            message.get("tool_calls").is_some_and(|calls| match calls {
                Value::Null => false,
                Value::Array(calls) => !calls.is_empty(),
                _ => true,
            }) || nonnull(message.get("function_call")).is_some()
        });
        let reasoning_spent = content.is_none() && !tools;
        let cap = if reasoning_spent {
            cap.max(cap.saturating_mul(2).min(MAX_REASONING_CAP))
        } else {
            cap
        };
        self.completion_cap = Value::Number(cap as f64);
        put(current, "max_tokens", self.completion_cap.clone())?;
        let mut messages = current
            .get("messages")
            .and_then(domain::items)
            .ok_or_else(|| Error::type_error("Builtin continuation requires chat messages"))?
            .to_vec();
        let resumable = content.filter(|_| self.docs_slice && !tools);
        if let Some(content) = resumable {
            messages.push(Value::object(vec![
                ("role", Value::string("assistant")),
                ("content", Value::String(content.clone())),
            ]));
        }
        let mut correction = Text::from(if self.docs_slice {
            if resumable.is_some() {
                "That message stopped at the completion cap. Continue it from where it stopped, without repeating it. "
            } else {
                "The response stopped at the completion cap. Continue the same answer from where it stopped. "
            }
        } else if reasoning_spent {
            "Reasoning used the whole completion cap before any answer. Think briefly, then answer directly. "
        } else {
            "The response was truncated at the completion cap. Split large output: use edit_file for targeted hunks or several smaller write/edit calls, and keep reasoning brief. "
        });
        correction.push_str(
            "Return complete tool calls or a complete summary; do not repeat previously executed edits.",
        );
        messages.push(Value::object(vec![
            ("role", Value::string("user")),
            ("content", Value::String(correction)),
        ]));
        put(current, "messages", Value::Array(messages))?;
        Ok(())
    }
}

pub fn create_builtin_chat<'a>(
    config: &Value,
    admission: &'a Admission,
    options: Options,
) -> Result<Option<BuiltinChat<'a>>, Error> {
    BuiltinChat::new(config, admission, options)
}

#[derive(Default)]
pub struct CompletionOptions<'a> {
    pub chat: Options,
    pub call: CallOptions<'a>,
}

pub async fn chat_completion<T: Transport>(
    config: &Value,
    request: &Value,
    admission: &Admission,
    transport: &mut T,
    observer: &mut impl Observer,
    credentials: &Credentials,
    options: CompletionOptions<'_>,
) -> Result<Value, Failure> {
    let mut chat = create_builtin_chat(config, admission, options.chat)?
        .ok_or_else(|| Error::new("Error", "An LLM base_url is required for chat completion"))?;
    let completion = chat
        .complete(request, transport, observer, credentials, options.call)
        .await?;
    let mut choice = Value::object(vec![("message", completion.message)]);
    if let Some(reason) = completion.finish_reason {
        put(&mut choice, "finish_reason", reason)?;
    }
    Ok(Value::object(vec![
        ("choices", Value::Array(vec![choice])),
        ("usage", completion.usage.unwrap_or(Value::Null)),
        (
            "response",
            chat.last_response()
                .map_or(Value::Null, LastResponse::as_value),
        ),
    ]))
}
