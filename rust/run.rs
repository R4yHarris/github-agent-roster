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

pub const MAX_AI_RUN_BYTES: usize = 16 * 1024;

fn invalid_compact(field: &str) -> Error {
    Error::new(
        "AgentRunError",
        format!("Invalid AI-Run {field}; see docs/METRICS.md."),
    )
    .coded("ROSTER_AI_RUN_INVALID")
}

fn compact_count(value: &str, field: &str) -> Result<crate::json::Value, Error> {
    use crate::json::Value;
    if value == "-" {
        return Ok(Value::Null);
    }
    if value.is_empty() || !value.bytes().all(|unit| unit.is_ascii_digit()) {
        return Err(invalid_compact(field));
    }
    Ok(value
        .parse::<u64>()
        .ok()
        .filter(|count| *count <= MAX_SAFE_INTEGER)
        .map_or_else(|| Value::string(value), |count| Value::Number(count as f64)))
}

pub fn parse_compact(
    value: &Text,
    expected_model: Option<&Text>,
) -> Result<Option<crate::json::Value>, Error> {
    use crate::json::Value;
    if value.0.is_empty() {
        return Ok(None);
    }
    if value.utf8_len() > MAX_AI_RUN_BYTES {
        return Err(invalid_compact("value exceeds 16 KiB"));
    }
    let source = value.to_utf8().map_err(|_| invalid_compact("encoding"))?;
    let fields = source.split('|').collect::<Vec<_>>();
    let [
        schema,
        provider,
        model_version,
        effort,
        context,
        output,
        session,
        task,
        ..,
    ] = fields.as_slice()
    else {
        return Err(invalid_compact("columns (expected at least eight)"));
    };
    if *schema != "1" {
        return Err(invalid_compact("schema (expected 1)"));
    }
    if fields.iter().take(8).any(|field| {
        let text = Text::from(*field);
        crate::text::trim(&text.0) != text.0
    }) {
        return Err(invalid_compact("delimiter spacing"));
    }
    if *provider != "-"
        && !Provider::parse(provider).is_some_and(|provider| provider != Provider::Vllm)
    {
        return Err(invalid_compact("provider"));
    }
    let (model, version) = model_version
        .rsplit_once('@')
        .filter(|(model, version)| {
            !model.is_empty()
                && !model.encode_utf16().any(crate::text::is_space)
                && !model.contains(['|', '@', '`'])
                && !version.is_empty()
                && version
                    .bytes()
                    .all(|unit| unit.is_ascii_alphanumeric() || matches!(unit, b'.' | b'_' | b'-'))
        })
        .ok_or_else(|| invalid_compact("model@version"))?;
    if expected_model.is_some_and(|expected| expected != &Text::from(model)) {
        return Err(invalid_compact("model@version"));
    }
    if !["l", "m", "h", "x", "-"].contains(effort) {
        return Err(invalid_compact("effort"));
    }
    let (used, capacity) = context
        .split_once('/')
        .filter(|(_, capacity)| !capacity.contains('/'))
        .ok_or_else(|| invalid_compact("in/max"))?;
    for (field, value) in [("session", *session), ("task", *task)] {
        if !opaque_id(value) {
            return Err(invalid_compact(field));
        }
    }
    Ok(Some(Value::object(vec![
        ("model", Value::string(model)),
        ("schema", Value::Number(1.0)),
        (
            "provider",
            if *provider == "-" {
                Value::Null
            } else {
                Value::string(provider)
            },
        ),
        ("model_version", Value::string(version)),
        (
            "effort",
            if *effort == "-" {
                Value::Null
            } else {
                Value::string(effort)
            },
        ),
        ("context_used", compact_count(used, "in")?),
        ("context_max", compact_count(capacity, "max")?),
        ("context_out", compact_count(output, "out")?),
        (
            "session",
            if *session == "-" {
                Value::Null
            } else {
                Value::string(session)
            },
        ),
        (
            "task",
            if *task == "-" {
                Value::Null
            } else {
                Value::string(task)
            },
        ),
    ])))
}

pub fn parse_message(message: &Text, sha: &str) -> Result<Option<crate::json::Value>, Error> {
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|unit| unit.is_ascii_hexdigit()) {
        return Err(invalid_compact("commit SHA"));
    }
    let message = message.normalized_lines();
    let message = crate::text::trim_end(&message.0);
    let lines = message.split(|unit| *unit == 10).collect::<Vec<_>>();
    let mut run = None;
    let mut model = None;
    let mut separator = false;
    for line in lines.iter().rev() {
        if crate::text::trim(line).is_empty() {
            separator = true;
            break;
        }
        let Some(colon) = line.iter().position(|unit| *unit == 58) else {
            break;
        };
        let name = line
            .get(..colon)
            .ok_or_else(|| invalid_compact("trailer"))?;
        if !name
            .first()
            .is_some_and(|unit| matches!(unit, 48..=57 | 65..=90 | 97..=122))
            || !name
                .iter()
                .all(|unit| matches!(unit, 45 | 48..=57 | 65..=90 | 97..=122))
        {
            break;
        }
        let value = Text(
            crate::text::trim(
                line.get(colon + 1..)
                    .ok_or_else(|| invalid_compact("trailer"))?,
            )
            .to_vec(),
        );
        if crate::text::ascii(name, "AI-Run") && run.is_none() {
            run = Some(value);
        } else if crate::text::ascii(name, "AI-Model") && model.is_none() {
            model = Some(value);
        }
    }
    if !separator {
        return Ok(None);
    }
    let Some(run) = run else { return Ok(None) };
    let Some(mut record) = parse_compact(&run, model.as_ref())? else {
        return Ok(None);
    };
    crate::domain::put(&mut record, "sha", crate::json::Value::string(sha))?;
    Ok(Some(record))
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
