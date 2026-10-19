use super::*;
use std::cell::Cell;
use std::rc::Rc;

use github_agent_roster::llm::http::{Body, Request, Response, Transport};

struct PendingBody(Rc<Cell<usize>>);

impl Body for PendingBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        std::future::pending().await
    }
}

impl Drop for PendingBody {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

struct PendingTransport {
    stream: bool,
    calls: usize,
    dropped: Rc<Cell<usize>>,
}

impl Transport for PendingTransport {
    type Body = PendingBody;

    async fn send(&mut self, _: &Request) -> Result<Response<PendingBody>, llm::Failure> {
        self.calls += 1;
        Ok(Response {
            status: 200,
            headers: vec![(
                "content-type".to_owned(),
                if self.stream {
                    "text/event-stream"
                } else {
                    "application/json"
                }
                .to_owned(),
            )],
            body: Some(PendingBody(self.dropped.clone())),
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn review49440_primary_and_critic_normalized_authorities_are_private_for_real_control_failures()
-> Result<(), Error> {
    let endpoints = [
        "https://Models.Example.invalid:8443/v1",
        "https://MODELS.EXAMPLE.invalid:443/v1",
        "http://Models.Example.invalid:80/v1",
        "https://[2001:0DB8:0:0:0:0:0:1]:8443/v1",
        "https://[2001:0DB8::1]:443/v1",
        "http://[::1]:80/v1",
    ];
    for role in ["primary", "critic"] {
        for endpoint in endpoints {
            let fixture = Fixture::new()?;
            let _ = optional_config(
                &fixture,
                if role == "primary" {
                    endpoint
                } else {
                    "http://127.0.0.1:1/v1"
                },
            )?;
            critic(
                &fixture,
                if role == "critic" {
                    endpoint
                } else {
                    "http://127.0.0.1:2/v1"
                },
                Some("CRITIC_AUTH"),
            )?;
            let loaded = fixture.loaded()?;
            let credentials = delivery::invocation_credentials(
                &Value::object(vec![("CRITIC_AUTH", Value::string(SENTINEL))]),
                &loaded,
            )?;
            let effective = if role == "critic" {
                loaded.config.with_profile(
                    loaded
                        .critic_profile()
                        .ok_or_else(|| Error::type_error("Selected critic missing"))?,
                )?
            } else {
                loaded.config.clone()
            };
            let model = effective
                .as_value()
                .get("llm")
                .and_then(|llm| llm.get("model"))
                .and_then(Value::as_string)
                .ok_or_else(|| Error::type_error("Selected model missing"))?
                .to_utf8()?;
            let base = effective
                .as_value()
                .get("llm")
                .and_then(|llm| llm.get("base_url"))
                .and_then(Value::as_string)
                .ok_or_else(|| Error::type_error("Selected endpoint missing"))?
                .clone();
            let normalized = url::Url::parse(&llm::openai::endpoint_for(&base)?)
                .map_err(|_| Error::type_error("Native transport endpoint invalid"))?;
            let authority =
                normalized[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
            let hostname = normalized
                .host_str()
                .ok_or_else(|| Error::type_error("Native transport host missing"))?
                .to_owned();
            let raw_authority = endpoint
                .split_once("://")
                .and_then(|(_, rest)| rest.split('/').next())
                .ok_or_else(|| Error::type_error("Owned raw authority missing"))?;
            for mode in ["timeout", "stall", "callback"] {
                let mut config = effective.as_value().clone();
                let mut llm = config
                    .get("llm")
                    .cloned()
                    .ok_or_else(|| Error::type_error("Selected LLM missing"))?;
                put(&mut llm, "api_key_optional", Value::Boolean(true));
                put(
                    &mut llm,
                    "request_timeout_ms",
                    Value::Number(if mode == "stall" { 1000.0 } else { 30.0 }),
                );
                put(
                    &mut llm,
                    "stream_idle_timeout_ms",
                    Value::Number(if mode == "stall" { 10.0 } else { 0.0 }),
                );
                put(&mut config, "llm", llm);
                let chat = Chat::new(&config)?.ok_or_else(|| {
                    Error::type_error("Configured diagnostic control unavailable")
                })?;
                let dropped = Rc::new(Cell::new(0));
                let mut transport = PendingTransport {
                    stream: mode == "stall",
                    calls: 0,
                    dropped: dropped.clone(),
                };
                let failure = if mode == "callback" {
                    let mut observer = CallbackError { calls: 0, message: format!(
                        "Actual callback model={model} url={endpoint} authority={authority} uppercase={} raw={raw_authority} credential={SENTINEL}",
                        authority.to_ascii_uppercase()
                    ) };
                    let result = chat.complete(&request(), &mut transport, &mut observer, &credentials, CallOptions::default()).await;
                    assert!(observer.calls > 0);
                    result
                } else {
                    chat.complete(&request(), &mut transport, &mut NoObserver, &credentials, CallOptions::default()).await
                }.err().ok_or_else(|| Error::type_error("Actual native diagnostic failure passed"))?;
                if mode != "callback" {
                    assert!(transport.calls > 0);
                    assert_eq!(dropped.get(), transport.calls);
                    assert_eq!(
                        failure.error.code,
                        Some(if mode == "stall" {
                            "ROSTER_LLM_STALL"
                        } else {
                            "ROSTER_LLM_TIMEOUT"
                        })
                    );
                }
                let safe = delivery::safe_invocation_error(&failure.error, &loaded, &credentials)?;
                assert_eq!(safe.code, failure.error.code);
                for private in [
                    &authority,
                    &authority.to_ascii_uppercase(),
                    &hostname,
                    &model,
                    endpoint,
                    raw_authority,
                    SENTINEL,
                ] {
                    assert!(
                        !safe.message.contains(private),
                        "{role} {mode} exposed selected endpoint/model/credential data"
                    );
                }
                assert!(chat.last_response()?.is_none());
            }
        }
    }
    Ok(())
}
