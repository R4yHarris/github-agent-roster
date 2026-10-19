use std::path::Path;

use super::*;
use crate::cancel::CancellationToken;
use crate::llm::openai::LastResponse;
use crate::state::provenance::api::{self, ProvenanceStore};

pub struct Observation<'a> {
    pub worktree: &'a Path,
    pub assignment: &'a lifecycle::Assignment,
    pub config: &'a Value,
    pub result: &'a Value,
    pub response: Option<&'a LastResponse>,
    pub opt_out: bool,
}

pub async fn record_result(
    store: &mut ProvenanceStore,
    observation: Observation<'_>,
    token: Option<&CancellationToken>,
) -> Result<Value, Error> {
    if let Some(token) = token {
        token.check()?;
    }
    observation.assignment.validate()?;
    let llm = required(observation.config, "llm")?;
    if observation.result.get("handoffReady") == Some(&Value::Boolean(true))
        && observation.response.is_none()
    {
        return Err(Error::type_error(
            "A ready configured planner requires response-backed provenance",
        ));
    }
    if let Some(response) = observation
        .result
        .get("response")
        .filter(|response| **response != Value::Null)
    {
        if observation.response.map(LastResponse::as_value).as_ref() != Some(response) {
            return Err(Error::type_error(
                "Planner response provenance differs from the completed planning result",
            ));
        }
    }
    let mut hashes = Value::object(Vec::new());
    let mut count = 0;
    for (field, name) in [
        ("task", "TASK.md"),
        ("recipe", "RECIPE.yml"),
        ("plan", "PLAN.md"),
        ("estimate", "ESTIMATE.md"),
    ] {
        let Some(content) = observation.result.get(field) else {
            continue;
        };
        let content = content
            .as_string()
            .ok_or_else(|| Error::type_error("Planning artifact evidence must be text"))?;
        let current = artifacts::read(observation.worktree, name)?.ok_or_else(|| {
            Error::new(
                "Error",
                format!("Planning provenance requires persisted {name}"),
            )
        })?;
        if current != *content {
            return Err(Error::new(
                "Error",
                format!("{name} changed before planning provenance capture"),
            ));
        }
        domain::put(
            &mut hashes,
            name,
            Value::string(&crate::identity::sha256_hex(current.to_utf8()?.as_bytes())),
        )?;
        count += 1;
    }
    if count == 0 {
        return Err(Error::type_error(
            "Planning provenance requires persisted artifact evidence",
        ));
    }
    let now = crate::clock::now_milliseconds()?;
    let mut metrics = Value::object(Vec::new());
    if let Some(usage) = observation.response.and_then(|response| response.usage) {
        for (name, value) in [
            ("tokens_prompt", usage.prompt_tokens),
            ("tokens_completion", usage.completion_tokens),
        ] {
            if let Some(value) = value {
                domain::put(&mut metrics, name, Value::Number(value as f64))?;
            }
        }
    }
    let critic_failed = observation
        .result
        .get("critic")
        .map(|critic| {
            array(
                required(critic, "defects")?,
                "Planner critic evidence must contain its actual defects array",
            )
            .map(|defects| !defects.is_empty())
        })
        .transpose()?
        .unwrap_or(false);
    let failed = observation.result.get("error").is_some() || critic_failed;
    let outcome = if failed {
        "unverified"
    } else if observation.response.is_none() {
        "stub"
    } else {
        "planned"
    };
    let mut payload = Value::object(vec![
        (
            "issue",
            Value::object(vec![("task", Value::string(&observation.assignment.task))]),
        ),
        (
            "seat",
            Value::object(vec![("name", Value::string("planner"))]),
        ),
        ("outcome", Value::string(outcome)),
        ("metrics", metrics),
        ("endedAt", Value::string(&crate::clock::iso_timestamp(now)?)),
        ("evidence", Value::object(vec![("artifactDigests", hashes)])),
    ]);
    if let Some(model) = llm
        .get("model")
        .and_then(Value::as_string)
        .filter(|model| !model.0.is_empty())
    {
        domain::put(&mut payload, "requestedModel", Value::String(model.clone()))?;
    }
    if let Some(response) = observation.response {
        domain::put(
            &mut payload,
            "servedModel",
            Value::String(response.model.clone()),
        )?;
    }
    let mut route = Value::object(Vec::new());
    for (source, target) in [
        ("fleet_profile", "name"),
        ("fleet_profile", "profile"),
        ("hardware", "hardware"),
    ] {
        if let Some(value) = llm.get(source) {
            domain::put(&mut route, target, value.clone())?;
        }
    }
    domain::put(&mut payload, "route", route)?;
    let context = Value::object(vec![
        ("runId", Value::string(&observation.assignment.task)),
        ("sessionId", Value::string(&observation.assignment.session)),
        ("payload", payload),
    ]);
    api::capture_lifecycle(
        store,
        &context,
        if failed { "failure" } else { "completed" },
        observation.opt_out,
        now,
        token,
    )
    .await
}
