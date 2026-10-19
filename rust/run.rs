use crate::text::Text;
use crate::{Error, MAX_SAFE_INTEGER};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UsageSample {
    pub prompt_tokens: Option<f64>,
    pub completion_tokens: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

impl Usage {
    pub fn json(self) -> String {
        let mut fields = Vec::with_capacity(2);
        if let Some(value) = self.prompt_tokens {
            fields.push(format!("\"prompt_tokens\":{value}"));
        }
        if let Some(value) = self.completion_tokens {
            fields.push(format!("\"completion_tokens\":{value}"));
        }
        format!("{{{}}}", fields.join(","))
    }
}

fn merge_count(
    samples: &[UsageSample],
    field: &str,
    get: impl Fn(&UsageSample) -> Option<f64>,
) -> Result<Option<u64>, Error> {
    let mut total = 0_u64;
    let mut known = !samples.is_empty();
    for sample in samples {
        let Some(value) = get(sample) else {
            known = false;
            continue;
        };
        if !value.is_finite()
            || value.fract() != 0.0
            || !(0.0..=MAX_SAFE_INTEGER as f64).contains(&value)
        {
            return Err(Error::type_error(format!(
                "LLM {field} must be a nonnegative safe integer"
            )));
        }
        total = total
            .checked_add(value as u64)
            .filter(|value| *value <= MAX_SAFE_INTEGER)
            .ok_or_else(|| {
                Error::type_error(format!("LLM {field} total exceeds safe integer range"))
            })?;
    }
    Ok(known.then_some(total))
}

pub fn merge_usage(samples: &[UsageSample]) -> Result<Usage, Error> {
    Ok(Usage {
        prompt_tokens: merge_count(samples, "prompt_tokens", |sample| sample.prompt_tokens)?,
        completion_tokens: merge_count(samples, "completion_tokens", |sample| {
            sample.completion_tokens
        })?,
    })
}

pub fn normalize_effort(value: Option<&str>) -> Result<&'static str, Error> {
    match value {
        None | Some("" | "-") => Ok("-"),
        Some("l" | "low") => Ok("l"),
        Some("m" | "medium") => Ok("m"),
        Some("h" | "high") => Ok("h"),
        Some("x" | "max") => Ok("x"),
        _ => Err(Error::type_error(
            "AI_EFFORT must be l, m, h, x, low, medium, high, or max",
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Vllm,
    GithubCopilot,
    Anthropic,
    Openai,
    Local,
    Other,
}

impl Provider {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "vllm" => Some(Self::Vllm),
            "github-copilot" => Some(Self::GithubCopilot),
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::Openai),
            "local" => Some(Self::Local),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vllm => "vllm",
            Self::GithubCopilot => "github-copilot",
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Local => "local",
            Self::Other => "other",
        }
    }
}

pub fn is_model_id(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|unit| unit.is_ascii_alphanumeric())
        && value.bytes().all(|unit| {
            unit.is_ascii_alphanumeric() || matches!(unit, b'.' | b'_' | b':' | b'+' | b'/' | b'-')
        })
        && !["unknown", "none", "n/a", "unspecified"].contains(&value.to_ascii_lowercase().as_str())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capacity {
    Number(u64),
    Decimal(String),
}

impl Capacity {
    fn valid(&self) -> bool {
        match self {
            Self::Number(value) => (1..=MAX_SAFE_INTEGER).contains(value),
            Self::Decimal(value) => {
                value
                    .as_bytes()
                    .first()
                    .is_some_and(|unit| matches!(unit, b'1'..=b'9'))
                    && value.bytes().all(|unit| unit.is_ascii_digit())
            }
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Number(value) => value.to_string(),
            Self::Decimal(value) => value.clone(),
        }
    }

    fn json(&self) -> String {
        match self {
            Self::Number(value) => value.to_string(),
            Self::Decimal(value) => Text::from(value.as_str()).json(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMetrics {
    pub provider: Provider,
    pub model: String,
    pub effort: String,
    pub context_max: Option<Capacity>,
    pub usage: Usage,
    pub context_used: Option<u64>,
    pub context_out: Option<u64>,
    pub session: Option<String>,
    pub task: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    metrics: RunMetrics,
    version: String,
    line: String,
    env: Vec<(String, String)>,
}

fn opaque_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|unit| unit.is_ascii_alphanumeric() || matches!(unit, b'.' | b'_' | b'-'))
}

pub fn materialize(mut metrics: RunMetrics, version: &str) -> Result<RunRecord, Error> {
    if !is_model_id(&metrics.model)
        || version.is_empty()
        || !version
            .bytes()
            .all(|unit| unit.is_ascii_alphanumeric() || matches!(unit, b'.' | b'_' | b'-'))
        || !["l", "m", "h", "x", "-"].contains(&metrics.effort.as_str())
        || metrics
            .context_max
            .as_ref()
            .is_some_and(|value| !value.valid())
    {
        return Err(Error::type_error(
            "Invalid LLM model, version, effort, or context_max for AI-Run; unknown is not a model",
        ));
    }
    for (field, count) in [
        ("prompt_tokens", metrics.usage.prompt_tokens),
        ("completion_tokens", metrics.usage.completion_tokens),
    ] {
        if count.is_some_and(|count| count > MAX_SAFE_INTEGER) {
            return Err(Error::type_error(format!(
                "LLM {field} must be a nonnegative safe integer"
            )));
        }
    }
    for (alias, count) in [
        (metrics.context_used, metrics.usage.prompt_tokens),
        (metrics.context_out, metrics.usage.completion_tokens),
    ] {
        if alias.is_some() && alias != count {
            return Err(Error::type_error(
                "Run token aliases must match the reported response usage",
            ));
        }
    }
    let provider = if metrics.provider == Provider::Vllm {
        "local"
    } else {
        metrics.provider.as_str()
    };
    let mut env = vec![
        ("AI_PROVIDER".to_owned(), provider.to_owned()),
        ("AI_MODEL".to_owned(), metrics.model.clone()),
        ("AI_MODEL_VERSION".to_owned(), version.to_owned()),
        ("AI_EFFORT".to_owned(), metrics.effort.clone()),
    ];
    if let Some(capacity) = &metrics.context_max {
        env.push(("AI_CONTEXT_MAX".to_owned(), capacity.text()));
    }
    metrics.context_used = metrics.usage.prompt_tokens;
    metrics.context_out = metrics.usage.completion_tokens;
    for (name, value) in [
        ("AI_CONTEXT_USED", metrics.context_used),
        ("AI_CONTEXT_OUT", metrics.context_out),
    ] {
        if let Some(value) = value {
            env.push((name.to_owned(), value.to_string()));
        }
    }
    for (name, value) in [("AI_SESSION", &metrics.session), ("AI_TASK", &metrics.task)] {
        if let Some(value) = value {
            if !opaque_id(value) {
                return Err(Error::type_error(format!(
                    "{name} must be an opaque identifier of at most 64 characters"
                )));
            }
            env.push((name.to_owned(), value.clone()));
        }
    }
    let line = format!(
        "1|{provider}|{}@{version}|{}|{}/{}|{}|{}|{}",
        metrics.model,
        metrics.effort,
        metrics
            .context_used
            .map_or("-".to_owned(), |value| value.to_string()),
        metrics
            .context_max
            .as_ref()
            .map_or("-".to_owned(), Capacity::text),
        metrics
            .context_out
            .map_or("-".to_owned(), |value| value.to_string()),
        metrics.session.as_deref().unwrap_or("-"),
        metrics.task.as_deref().unwrap_or("-")
    );
    Ok(RunRecord {
        metrics,
        version: version.to_owned(),
        line,
        env,
    })
}

impl RunRecord {
    pub fn metrics(&self) -> &RunMetrics {
        &self.metrics
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn line(&self) -> &str {
        &self.line
    }

    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }

    pub fn json(&self) -> String {
        let metrics = &self.metrics;
        let mut fields = vec![
            format!(
                "\"provider\":{}",
                Text::from(metrics.provider.as_str()).json()
            ),
            format!("\"model\":{}", Text::from(metrics.model.as_str()).json()),
            format!("\"effort\":{}", Text::from(metrics.effort.as_str()).json()),
        ];
        if let Some(capacity) = &metrics.context_max {
            fields.push(format!("\"context_max\":{}", capacity.json()));
        }
        if let Some(count) = metrics.usage.prompt_tokens {
            fields.push(format!(
                "\"prompt_tokens\":{count},\"context_used\":{count}"
            ));
        }
        if let Some(count) = metrics.usage.completion_tokens {
            fields.push(format!(
                "\"completion_tokens\":{count},\"context_out\":{count}"
            ));
        }
        for (name, value) in [("session", &metrics.session), ("task", &metrics.task)] {
            if let Some(value) = value {
                fields.push(format!("\"{name}\":{}", Text::from(value.as_str()).json()));
            }
        }
        let env = self
            .env
            .iter()
            .map(|(name, value)| {
                format!(
                    "{}:{}",
                    Text::from(name.as_str()).json(),
                    Text::from(value.as_str()).json()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"provider\":{},\"metrics\":{{{}}},\"version\":{},\"line\":{},\"env\":{{{env}}}}}",
            Text::from(metrics.provider.as_str()).json(),
            fields.join(","),
            Text::from(self.version.as_str()).json(),
            Text::from(self.line.as_str()).json()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> RunMetrics {
        RunMetrics {
            provider: Provider::Vllm,
            model: "owner/model".to_owned(),
            effort: "h".to_owned(),
            context_max: Some(Capacity::Number(32768)),
            usage: Usage::default(),
            context_used: None,
            context_out: None,
            session: Some("native-session".to_owned()),
            task: Some("issue-42".to_owned()),
        }
    }

    #[test]
    fn unknown_usage_never_becomes_zero_or_a_partial_total() -> Result<(), Error> {
        assert_eq!(merge_usage(&[])?, Usage::default());
        let known = UsageSample {
            prompt_tokens: Some(5.0),
            completion_tokens: Some(2.0),
        };
        assert_eq!(
            merge_usage(&[known, UsageSample::default()])?,
            Usage::default()
        );
        assert_eq!(
            merge_usage(&[known, known])?,
            Usage {
                prompt_tokens: Some(10),
                completion_tokens: Some(4)
            }
        );
        assert!(
            merge_usage(&[UsageSample {
                prompt_tokens: Some(-1.0),
                ..known
            }])
            .is_err()
        );
        let maximum = UsageSample {
            prompt_tokens: Some(MAX_SAFE_INTEGER as f64),
            ..known
        };
        assert!(merge_usage(&[maximum, known]).is_err());
        Ok(())
    }

    #[test]
    fn materializes_compact_provenance_without_fabricating_usage() -> Result<(), Error> {
        let record = materialize(metrics(), "-")?;
        assert_eq!(
            record.line,
            "1|local|owner/model@-|h|-/32768|-|native-session|issue-42"
        );
        assert!(!record.env.iter().any(|(name, _)| name == "AI_CONTEXT_USED"));
        let mut measured = metrics();
        measured.usage = Usage {
            prompt_tokens: Some(5),
            completion_tokens: Some(0),
        };
        let record = materialize(measured, "v1")?;
        assert_eq!(
            record.line,
            "1|local|owner/model@v1|h|5/32768|0|native-session|issue-42"
        );
        assert_eq!(record.metrics.context_out, Some(0));
        Ok(())
    }

    #[test]
    fn refuses_unknown_identity_and_disagreeing_aliases() {
        for model in ["unknown", "NONE", "n/a", "unspecified", "a|b", "-invalid"] {
            let mut value = metrics();
            value.model = model.to_owned();
            assert!(materialize(value, "-").is_err());
        }
        let mut value = metrics();
        value.context_used = Some(0);
        assert!(materialize(value, "-").is_err());
        let mut value = metrics();
        value.session = Some("../session".to_owned());
        assert!(materialize(value, "-").is_err());
    }
}
