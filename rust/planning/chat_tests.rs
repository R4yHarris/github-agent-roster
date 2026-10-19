use std::collections::VecDeque;

use super::*;
use crate::json;
use crate::llm::Failure;
use crate::llm::control::Observer;
use crate::llm::http::{Body, Request, Response, Transport};
use crate::llm::openai::Credentials;
use crate::state::redaction;

struct Recorded {
    responses: VecDeque<Value>,
    requests: Vec<Value>,
}

struct RecordedBody(Option<Vec<u8>>);

impl Body for RecordedBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.0.take())
    }
}

impl Transport for Recorded {
    type Body = RecordedBody;

    async fn send(&mut self, request: &Request) -> Result<Response<Self::Body>, Failure> {
        let source = std::str::from_utf8(&request.body)
            .map_err(|_| Error::type_error("Recorded request must be JSON UTF-8"))?;
        self.requests.push(json::parse(&Text::from(source))?);
        let response = self
            .responses
            .pop_front()
            .ok_or_else(|| Error::new("Error", "Unexpected planning request"))?;
        Ok(Response {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Some(RecordedBody(Some(response.json().into_bytes()))),
        })
    }
}

#[derive(Default)]
struct Events(Vec<Value>);

impl Observer for Events {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        self.0.push(event);
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn planning_drivers_share_one_invocation_admission_without_new_worker_state()
-> Result<(), Error> {
    let admission = crate::llm::admission::Admission::default();
    let profile = Text::from("shared-planning");
    let mut holder = admission.acquire(&profile, None, None)?.wait().await?;
    let mut config = config()?;
    let mut llm = required(&config, "llm")?.clone();
    domain::put(&mut llm, "fleet_profile", Value::String(profile.clone()))?;
    domain::put(&mut config, "llm", llm)?;
    let assignment_a = assignment();
    let mut assignment_b = assignment();
    assignment_b.session = "chat-planner-second".to_owned();
    assignment_b.task = "issue-43".to_owned();
    assignment_b.reference = "issue:43".to_owned();
    let description = r#"{"title":"docs: bounded change","acceptance_checks":["README contains the requested change"],"files_allowed":["README.md"],"task_class":"docs"}"#;
    let mut transport_a = Recorded {
        responses: vec![packet(description, "stop", "actual-a")].into(),
        requests: Vec::new(),
    };
    let mut transport_b = Recorded {
        responses: vec![packet(description, "stop", "actual-b")].into(),
        requests: Vec::new(),
    };
    let mut observer_a = Events::default();
    let mut observer_b = Events::default();
    let credentials = Credentials::default();
    let mut driver_a = lifecycle::Driver {
        transport: &mut transport_a,
        observer: &mut observer_a,
        credentials: &credentials,
        admission: &admission,
        signal: None,
        redaction: redaction::Config::default(),
        last_response: None,
        critic_evidence: Vec::new(),
    };
    let mut driver_b = lifecycle::Driver {
        transport: &mut transport_b,
        observer: &mut observer_b,
        credentials: &credentials,
        admission: &admission,
        signal: None,
        redaction: redaction::Config::default(),
        last_response: None,
        critic_evidence: Vec::new(),
    };
    let (a, b, control) = tokio::join!(
        driver_a.draft(
            &assignment_a,
            &config,
            lifecycle::DraftOptions::default(),
            None
        ),
        driver_b.draft(
            &assignment_b,
            &config,
            lifecycle::DraftOptions::default(),
            None
        ),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            assert_eq!(admission.depth(&profile)?, 2);
            holder.release()?;
            Ok::<_, Error>(())
        },
    );
    control?;
    assert_eq!(
        a?.response.map(|response| response.model),
        Some(Text::from("actual-a"))
    );
    assert_eq!(
        b?.response.map(|response| response.model),
        Some(Text::from("actual-b"))
    );
    assert_eq!(transport_a.requests.len(), 1);
    assert_eq!(transport_b.requests.len(), 1);
    assert_eq!(admission.depth(&profile)?, 0);
    Ok(())
}

struct CriticObserver {
    values: Vec<Value>,
    fail: bool,
    completed: usize,
}

impl Observer for CriticObserver {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if domain::is_text(event.get("phase"), "ok") {
            self.completed += 1;
            if self.fail && self.completed == 2 {
                return Err(
                    Error::new("Error", "Owned critic observer failure").coded("ROSTER_RUN_LOG")
                );
            }
        }
        self.values.push(event);
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn critic_length_and_observer_errors_keep_scoped_measured_evidence_without_repair()
-> Result<(), Error> {
    for observer_failure in [false, true] {
        let admission = crate::llm::admission::Admission::default();
        let assignment = assignment();
        let mut config = config()?;
        let mut planner = required(&config, "planner")?.clone();
        domain::put(&mut planner, "critic_profile", Value::string("critic"))?;
        domain::put(&mut config, "planner", planner)?;
        let fleet = crate::fleet::validate(&json::parse(&Text::from(
            r#"{"profiles":[{"id":"critic","base_url":"http://127.0.0.1:8001/v1","model":"critic-route","provider":"vllm","context_max":8192,"concurrency":1,"hardware":"fixture","notes":"Owned fixture"}]}"#,
        ))?)?;
        let description = r#"{"title":"docs: bounded change","acceptance_checks":["README contains the requested change"],"files_allowed":["README.md"],"task_class":"docs"}"#;
        let mut transport = Recorded {
            responses: vec![
                packet(description, "stop", "actual-planner"),
                packet(
                    r#"{"defects":[]}"#,
                    if observer_failure { "stop" } else { "length" },
                    "actual-critic",
                ),
            ]
            .into(),
            requests: Vec::new(),
        };
        let mut observer = CriticObserver {
            values: Vec::new(),
            fail: observer_failure,
            completed: 0,
        };
        let credentials = Credentials::default();
        let mut driver = lifecycle::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &admission,
            signal: None,
            redaction: redaction::Config::default(),
            last_response: None,
            critic_evidence: Vec::new(),
        };
        let draft = driver
            .draft(
                &assignment,
                &config,
                lifecycle::DraftOptions::default(),
                None,
            )
            .await?;
        let error = driver
            .critique_plan(
                &draft.artifacts.task,
                lifecycle::CritiqueOptions {
                    assignment: &assignment,
                    config: &config,
                    learning_root: std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
                    grounding: None,
                    metadata: None,
                    locked_model: None,
                    fleet: Some(&fleet),
                },
            )
            .await
            .err()
            .ok_or_else(|| Error::new("Error", "Failed critic minted a valid handoff"))?;
        assert_eq!(
            error.code,
            Some(if observer_failure {
                "ROSTER_RUN_LOG"
            } else {
                "ROSTER_LLM_FINISH_REASON"
            })
        );
        assert_eq!(
            driver
                .last_response
                .as_ref()
                .map(|response| &response.model),
            Some(&Text::from("actual-planner"))
        );
        assert_eq!(driver.critic_evidence.len(), 1);
        let evidence = driver
            .critic_evidence
            .first()
            .ok_or_else(|| Error::new("Error", "Missing scoped critic evidence"))?;
        assert_eq!(
            evidence.get("transportSucceeded"),
            Some(&Value::Boolean(false))
        );
        assert_eq!(evidence.get("reportValid"), Some(&Value::Boolean(false)));
        assert_eq!(
            evidence
                .get("response")
                .and_then(|response| response.get("model")),
            Some(&Value::string("actual-critic"))
        );
        assert_eq!(
            evidence
                .get("response")
                .and_then(|response| response.get("usage"))
                .and_then(|usage| usage.get("prompt_tokens")),
            Some(&Value::Number(0.0))
        );
        assert_eq!(
            required(required(evidence, "run")?, "metrics")?.get("model"),
            Some(&Value::string("actual-critic"))
        );
        assert_eq!(transport.requests.len(), 2);
        let critic = transport
            .requests
            .get(1)
            .ok_or_else(|| Error::new("Error", "Critic did not use actual transport"))?;
        assert_eq!(critic.get("max_tokens"), Some(&Value::Number(600.0)));
        assert_eq!(critic.get("reasoning_effort"), Some(&Value::string("none")));
        assert_eq!(critic.get("stream"), Some(&Value::Boolean(false)));
        assert!(critic.get("tools").is_none());
        let system = critic
            .get("messages")
            .and_then(domain::items)
            .and_then(|messages| messages.first())
            .and_then(|message| message.get("content"))
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Missing critic system context"))?;
        let baseline = crate::llm::baseline::read()?;
        assert!(
            system
                .0
                .windows(baseline.0.len())
                .any(|part| part == baseline.0)
        );
        if !observer_failure {
            assert!(observer.values.iter().any(|event| domain::is_text(
                event.get("type"),
                "finish-reason"
            ) && event.get("retry")
                == Some(&Value::Boolean(false))));
        }
        assert_eq!(admission.depth(&Text::from("critic"))?, 0);
    }
    Ok(())
}
fn config() -> Result<Value, Error> {
    let mut config =
        crate::config::parse(&Text::from(include_str!("../../roster.config.example.yml")))?
            .as_value()
            .clone();
    let mut llm = config
        .get("llm")
        .cloned()
        .ok_or_else(|| Error::type_error("Missing fixture LLM"))?;
    domain::put(
        &mut llm,
        "base_url",
        Value::string("http://127.0.0.1:8000/v1"),
    )?;
    domain::put(&mut llm, "model", Value::string("configured-model"))?;
    domain::put(&mut llm, "api_key_optional", Value::Boolean(true))?;
    domain::put(&mut config, "llm", llm)?;
    Ok(config)
}

fn assignment() -> lifecycle::Assignment {
    lifecycle::Assignment {
        ask: Text::from("Change README.md."),
        title: None,
        reference: "issue:42".to_owned(),
        task: "issue-42".to_owned(),
        session: "chat-planner".to_owned(),
    }
}

fn packet(content: &str, reason: &str, model: &str) -> Value {
    Value::object(vec![
        ("model", Value::string(model)),
        (
            "usage",
            Value::object(vec![
                ("prompt_tokens", Value::Number(0.0)),
                ("completion_tokens", Value::Number(7.0)),
            ]),
        ),
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
    ])
}

#[tokio::test(flavor = "current_thread")]
async fn actual_planning_request_carries_required_baseline_and_seat_limits() -> Result<(), Error> {
    let config = config()?;
    let assignment = assignment();
    let description = r#"{"title":"docs: bounded change","acceptance_checks":["README contains the requested change"],"files_allowed":["README.md"],"task_class":"docs"}"#;
    let mut transport = Recorded {
        responses: vec![packet(description, "stop", "actual-model")].into(),
        requests: Vec::new(),
    };
    let mut observer = Events::default();
    let credentials = Credentials::default();
    let mut driver = lifecycle::Driver {
        transport: &mut transport,
        observer: &mut observer,
        credentials: &credentials,
        admission: &crate::llm::admission::Admission::default(),
        signal: None,
        redaction: redaction::Config::default(),
        last_response: None,
        critic_evidence: Vec::new(),
    };
    let metadata = Value::object(vec![("task_class", Value::string("docs"))]);
    let draft = driver
        .draft(
            &assignment,
            &config,
            lifecycle::DraftOptions {
                metadata: Some(&metadata),
                ..lifecycle::DraftOptions::default()
            },
            None,
        )
        .await?;
    assert_eq!(draft.turns, 1);
    assert_eq!(
        draft.response.as_ref().map(|response| &response.model),
        Some(&Text::from("actual-model"))
    );
    let request = transport
        .requests
        .first()
        .ok_or_else(|| Error::new("Error", "Planning did not use native transport"))?;
    let system = request
        .get("messages")
        .and_then(domain::items)
        .and_then(|messages| messages.first())
        .and_then(|message| message.get("content"))
        .and_then(Value::as_string)
        .ok_or_else(|| Error::type_error("Planning system message missing"))?;
    let baseline = Text::from(include_str!("../../principals/engineering.md")).normalized_lines();
    let baseline = crate::text::trim(&baseline.0);
    assert!(
        system
            .0
            .windows(baseline.len())
            .any(|part| part == baseline),
        "Actual planner request omitted the required engineering baseline"
    );
    assert_eq!(request.get("max_tokens"), Some(&Value::Number(8192.0)));
    assert_eq!(
        request.get("reasoning_effort"),
        Some(&Value::string("none"))
    );
    assert_eq!(request.get("stream"), Some(&Value::Boolean(true)));
    assert!(observer.0.iter().any(|event| {
        domain::is_text(event.get("type"), "http")
            && domain::is_text(event.get("phase"), "start")
            && event.get("maxTokens") == Some(&Value::Number(8192.0))
            && event.get("thinking") == Some(&Value::Boolean(false))
    }));
    Ok(())
}
