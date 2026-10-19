use url::Url;

use crate::domain::{self, is_text, put};
use crate::json::Value;
use crate::text::{Text, ascii};
use crate::{Error, capabilities, request};

pub const DOCS_COMPLETION_FLOOR: u64 = 8192;

pub fn docs_completion_cap(llm: &Value) -> u64 {
    domain::safe_integer(llm.get("max_tokens"))
        .unwrap_or(DOCS_COMPLETION_FLOOR)
        .max(DOCS_COMPLETION_FLOOR)
}

pub fn uses_deepseek_reasoning(llm: &Value) -> Result<bool, Error> {
    let Some(base) = llm.get("base_url").filter(|value| domain::truthy(value)) else {
        return Ok(false);
    };
    let base = domain::string(base)?;
    let url = Url::parse(&String::from_utf16_lossy(&base.0))
        .map_err(|_| Error::type_error("Invalid URL"))?;
    if !request::is_local_llm_host(url.host_str().unwrap_or("")) {
        return Ok(false);
    }
    let model = llm.get("model").filter(|value| **value != Value::Null);
    let model = domain::string(model.unwrap_or(&Value::string("")))?;
    let model = String::from_utf16_lossy(&model.0).to_ascii_lowercase();
    for (position, _) in model.match_indices("deepseek") {
        let Some(tail) = model.get(position + "deepseek".len()..) else {
            continue;
        };
        let tail = tail.strip_prefix(['-', '_', '/']).unwrap_or(tail);
        let Some(tail) = tail.strip_prefix("v4") else {
            continue;
        };
        if tail
            .strip_prefix(['.', '_', '-'])
            .unwrap_or(tail)
            .starts_with('1')
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn mapped_effort(llm: &Value, effort: Option<&Value>) -> Result<&'static str, Error> {
    let default = Value::string("m");
    let effort = effort
        .or_else(|| llm.get("effort").filter(|value| **value != Value::Null))
        .unwrap_or(&default);
    let effort = domain::string(effort)?;
    let deepseek = uses_deepseek_reasoning(llm)?;
    for (short, cloud, local) in [
        ("l", "low", "low"),
        ("m", "medium", "high"),
        ("h", "high", "high"),
        ("x", "xhigh", "max"),
        ("none", "none", "none"),
    ] {
        if ascii(&effort.0, short) {
            return Ok(if deepseek { local } else { cloud });
        }
    }
    Err(Error::type_error(
        "Reasoning effort must be l, m, h, x, or none",
    ))
}

pub fn next_effort(llm: &Value, effort: &Value) -> Result<&'static str, Error> {
    if is_text(Some(effort), "none") {
        return Ok("none");
    }
    let deepseek = uses_deepseek_reasoning(llm)?;
    let normalized = if deepseek && is_text(Some(effort), "m") {
        Value::string("h")
    } else {
        effort.clone()
    };
    let tiers: &[&str] = if deepseek {
        &["l", "h", "x"]
    } else {
        &["l", "m", "h", "x"]
    };
    let position = tiers
        .iter()
        .position(|tier| is_text(Some(&normalized), tier))
        .ok_or_else(|| Error::type_error("Prior reasoning effort is invalid"))?;
    tiers
        .get((position + 1).min(tiers.len() - 1))
        .copied()
        .ok_or_else(|| Error::type_error("Prior reasoning effort is invalid"))
}

pub fn select_reasoning(config: &Value, options: &Value) -> Result<Value, Error> {
    let llm = domain::property(Some(config), "llm")?;
    let llm = llm.ok_or_else(|| {
        Error::type_error("Cannot read properties of undefined (reading 'review_retry_effort')")
    })?;
    let nonnull = |value: Option<&Value>| value.filter(|value| **value != Value::Null).cloned();
    let previous =
        nonnull(options.get("previousEffort")).or_else(|| nonnull(llm.get("review_retry_effort")));
    let difficulty = nonnull(options.get("difficulty"))
        .or_else(|| nonnull(llm.get("task_difficulty")))
        .unwrap_or(Value::Number(2.0));
    let task_class = nonnull(options.get("taskClass"))
        .or_else(|| nonnull(llm.get("task_class")))
        .unwrap_or(Value::string("feat"));
    let difficulty = domain::safe_integer(Some(&difficulty))
        .filter(|value| (1..=5).contains(value))
        .ok_or_else(|| Error::type_error("Reasoning difficulty must be an integer from 1 to 5"))?;
    let prior = capabilities::model_prior(
        llm.get("model").unwrap_or(&Value::Null),
        Some(&task_class),
        config.get("capabilities"),
    )?;
    let kind = options.get("kind");
    let docs_slice = is_text(kind, "slice") && is_text(Some(&task_class), "docs");
    let planning = is_text(kind, "feature") || is_text(kind, "initiative");
    let suggested = domain::safe_integer(prior.get("suggested_difficulty")).unwrap_or(2);
    let default = if difficulty <= 2 && is_text(prior.get("strength"), "strong") {
        "l"
    } else if difficulty >= 4 || difficulty > suggested {
        "h"
    } else {
        "m"
    };
    let mut effort = nonnull(llm.get("effort_override")).unwrap_or(Value::string(default));
    if let Some(previous) = previous.filter(|_| llm.get("effort_override").is_none()) {
        effort = Value::string(next_effort(llm, &previous)?);
    }
    if docs_slice {
        effort = Value::string("none");
    }
    if uses_deepseek_reasoning(llm)? && is_text(Some(&effort), "m") {
        effort = Value::string("h");
    }
    let mut selected = domain::spread(llm);
    put(&mut selected, "effort", effort)?;
    put(
        &mut selected,
        "model_prior",
        prior.get("strength").cloned().unwrap_or(Value::Null),
    )?;
    put(
        &mut selected,
        "task_difficulty",
        Value::Number(difficulty as f64),
    )?;
    put(&mut selected, "task_class", task_class)?;
    if let Some(kind) = nonnull(kind).or_else(|| llm.get("task_kind").cloned()) {
        put(&mut selected, "task_kind", kind)?;
    }
    if docs_slice || planning {
        put(
            &mut selected,
            "max_tokens",
            Value::Number(if docs_slice {
                docs_completion_cap(llm)
            } else {
                4096
            } as f64),
        )?;
    }
    let mut result = domain::spread(config);
    put(&mut result, "llm", selected)?;
    Ok(result)
}

pub fn effort_text(llm: &Value, effort: Option<&Value>) -> Result<Text, Error> {
    Ok(Text::from(mapped_effort(llm, effort)?))
}
