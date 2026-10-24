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

pub const RUN_ENV_NAMES: &[&str] = &[
    "AI_PROVIDER",
    "AI_MODEL",
    "AI_MODEL_VERSION",
    "AI_EFFORT",
    "AI_CONTEXT_USED",
    "AI_CONTEXT_MAX",
    "AI_CONTEXT_OUT",
    "AI_SESSION",
    "AI_TASK",
];

pub fn resolve_publish_model(
    config: Option<&crate::json::Value>,
    env: Option<&crate::json::Value>,
    model: Option<&str>,
    ghcp: bool,
) -> Result<String, Error> {
    let invalid = || {
        Error::type_error(if ghcp {
            "set model: GHCP publication requires --model or AI_MODEL with the actual model id (not unknown)"
        } else {
            "set model: configure llm.model, AI_MODEL, or ROSTER_MODEL with the actual model id (not unknown)"
        })
    };
    let check_value = |value: &crate::json::Value| -> Result<String, Error> {
        let text = value.as_string().ok_or_else(invalid)?;
        let decoded = text.to_utf8().map_err(|_| invalid())?;
        if is_model_id(&decoded) {
            Ok(decoded)
        } else {
            Err(invalid())
        }
    };
    if ghcp {
        if let Some(model) = model {
            return if is_model_id(model) {
                Ok(model.to_owned())
            } else {
                Err(invalid())
            };
        }
        let Some(env_val) = env.and_then(|e| e.get("AI_MODEL")) else {
            return Err(invalid());
        };
        return check_value(env_val);
    }
    for candidate in [
        config
            .and_then(|c| c.get("llm"))
            .and_then(|l| l.get("model")),
        env.and_then(|e| e.get("AI_MODEL")),
        env.and_then(|e| e.get("ROSTER_MODEL")),
    ]
    .into_iter()
    .flatten()
    {
        if crate::domain::truthy(candidate) {
            return check_value(candidate);
        }
    }
    Err(invalid())
}

pub fn build_ghcp_run(
    env: Option<&crate::json::Value>,
    model: Option<&str>,
    session: Option<&str>,
    task: Option<&str>,
) -> Result<RunRecord, Error> {
    let actual_model = resolve_publish_model(None, env, model, true)?;
    let effort = match env.and_then(|e| e.get("AI_EFFORT")) {
        None => "-".to_owned(),
        Some(crate::json::Value::String(text)) => {
            let decoded = text.to_utf8().map_err(|_| {
                Error::type_error("AI_EFFORT must be l, m, h, x, low, medium, high, or max")
            })?;
            normalize_effort(Some(&decoded))?.to_owned()
        }
        Some(_) => {
            return Err(Error::type_error(
                "AI_EFFORT must be l, m, h, x, low, medium, high, or max",
            ));
        }
    };
    let identity = match session {
        Some(value) => value.to_owned(),
        None => env
            .and_then(|e| e.get("AI_SESSION"))
            .and_then(crate::json::Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .filter(|s| s.starts_with("ghcp-"))
            .unwrap_or_else(|| format!("ghcp-{}", std::process::id())),
    };
    if !identity.starts_with("ghcp-")
        || identity.len() <= 5
        || !identity[5..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(Error::type_error(
            "GHCP AI_SESSION must use a ghcp- date or process identifier",
        ));
    }
    let context_max = match env.and_then(|e| e.get("AI_CONTEXT_MAX")) {
        None => None,
        Some(crate::json::Value::String(text)) => {
            let raw = text.to_utf8().map_err(|_| {
                Error::type_error("GHCP-only AI_CONTEXT_MAX must be a positive decimal integer")
            })?;
            if raw.is_empty() || raw == "-" {
                None
            } else {
                if !raw
                    .as_bytes()
                    .first()
                    .is_some_and(|b| matches!(b, b'1'..=b'9'))
                    || !raw.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(Error::type_error(
                        "GHCP-only AI_CONTEXT_MAX must be a positive decimal integer",
                    ));
                }
                match raw
                    .parse::<u64>()
                    .ok()
                    .filter(|n| (1..=MAX_SAFE_INTEGER).contains(n))
                {
                    Some(number) => Some(Capacity::Number(number)),
                    None => Some(Capacity::Decimal(raw)),
                }
            }
        }
        Some(_) => {
            return Err(Error::type_error(
                "GHCP-only AI_CONTEXT_MAX must be a positive decimal integer",
            ));
        }
    };
    let task_id = match task {
        Some("") => None,
        Some(value) => Some(value.to_owned()),
        None => match env.and_then(|e| e.get("AI_TASK")) {
            None => None,
            Some(crate::json::Value::String(text)) => {
                let decoded = text.to_utf8().map_err(|_| {
                    Error::type_error(
                        "AI_TASK must be an opaque identifier of at most 64 characters",
                    )
                })?;
                (!decoded.is_empty()).then_some(decoded)
            }
            Some(_) => {
                return Err(Error::type_error(
                    "AI_TASK must be an opaque identifier of at most 64 characters",
                ));
            }
        },
    };
    materialize(
        RunMetrics {
            provider: Provider::GithubCopilot,
            model: actual_model,
            effort,
            context_max,
            usage: Usage::default(),
            context_used: None,
            context_out: None,
            session: Some(identity),
            task: task_id,
        },
        "-",
    )
}

pub fn build_publish_env(
    config: &crate::json::Value,
    env: &crate::json::Value,
    run: Option<&RunRecord>,
    model: Option<&str>,
    session: Option<&str>,
    task: Option<&str>,
) -> Result<crate::json::Value, Error> {
    crate::config::require_publication_enabled(config)?;
    let owned_record;
    let metadata = match run {
        Some(record) => record,
        None => {
            owned_record = build_ghcp_run(Some(env), model, session, task)?;
            &owned_record
        }
    };
    let model_env = crate::json::Value::object(
        metadata
            .env()
            .iter()
            .map(|(k, v)| (k.as_str(), crate::json::Value::string(v)))
            .collect(),
    );
    let actual_model = resolve_publish_model(None, Some(&model_env), None, false)?;
    let mut publish_env = crate::config::without_llm_keys(env, config)?;
    for name in RUN_ENV_NAMES {
        crate::domain::remove(&mut publish_env, name)?;
    }
    for (name, value) in metadata.env() {
        crate::domain::put(&mut publish_env, name, crate::json::Value::string(value))?;
    }
    crate::domain::put(
        &mut publish_env,
        "AI_MODEL",
        crate::json::Value::string(&actual_model),
    )?;
    Ok(publish_env)
}

pub fn issue_merge_message(subject: &str, issue_number: u64) -> Result<String, Error> {
    if !(1..=MAX_SAFE_INTEGER).contains(&issue_number) {
        return Err(Error::type_error(
            "Issue number must be a positive safe integer",
        ));
    }
    if crate::text::trim(&Text::from(subject).0).is_empty() || subject.contains('\0') {
        return Err(Error::type_error("Publish subject must be nonempty text"));
    }
    let pattern = format!(
        r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+(?:[A-Za-z0-9-]+/[A-Za-z0-9_.-]+)?#{issue_number}\b"
    );
    if regex_lite::Regex::new(&pattern)
        .map_err(|error| Error::new("Error", error.to_string()))?
        .is_match(subject)
    {
        return Err(Error::new(
            "Error",
            format!(
                "Issue #{issue_number} must remain open until human AI-Eval; use Refs #{issue_number}, not a closing keyword"
            ),
        ));
    }
    let trimmed = Text(crate::text::trim_end(&Text::from(subject).0).to_vec()).to_utf8()?;
    Ok(format!("{trimmed}\n\nRefs #{issue_number}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishArgs {
    pub subject: Option<String>,
    pub model: Option<String>,
    pub skip_review: bool,
}

pub fn parse_publish_args(args: &str) -> Result<PublishArgs, Error> {
    let words = args
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut subject = Vec::new();
    let mut model = None;
    let mut skip_review = false;
    let mut index = 0;
    while let Some(word) = words.get(index) {
        if word == "--model" {
            let next = words.get(index + 1);
            if model.is_some() || next.is_none_or(|val| val.is_empty() || val.starts_with("--")) {
                return Err(Error::type_error(
                    "Use /publish [SUBJECT] [--model MODEL] [--skip-review]",
                ));
            }
            index += 1;
            model = Some(resolve_publish_model(
                None,
                None,
                words.get(index).map(String::as_str),
                true,
            )?);
        } else if word == "--skip-review" {
            if skip_review {
                return Err(Error::type_error("--skip-review may be supplied only once"));
            }
            skip_review = true;
        } else if word.starts_with("--") {
            return Err(Error::type_error(
                "Use /publish [SUBJECT] [--model MODEL] [--skip-review]",
            ));
        } else {
            subject.push(word.clone());
        }
        index += 1;
    }
    Ok(PublishArgs {
        subject: (!subject.is_empty()).then(|| subject.join(" ")),
        model,
        skip_review,
    })
}

pub fn publication_task(
    cwd: &std::path::Path,
    env: Option<&crate::json::Value>,
    task: Option<&str>,
) -> Result<String, Error> {
    let invalid_task =
        || Error::type_error("AI_TASK must be an opaque branch slug or issue identifier");
    let known = match task {
        Some("") => None,
        Some(value) => Some(value.to_owned()),
        None => match env.and_then(|e| e.get("AI_TASK")) {
            None => None,
            Some(crate::json::Value::String(text)) => {
                let decoded = text.to_utf8().map_err(|_| invalid_task())?;
                (!decoded.is_empty()).then_some(decoded)
            }
            Some(_) => return Err(invalid_task()),
        },
    };
    if let Some(known) = known {
        if known == "-" || !opaque_id(&known) {
            return Err(invalid_task());
        }
        return Ok(known);
    }
    let git = std::env::var_os("ROSTER_TEST_GIT")
        .map_or_else(|| std::path::PathBuf::from("git"), std::path::PathBuf::from);
    let output = std::process::Command::new(git)
        .current_dir(cwd)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .map_err(|error| {
            Error::new(
                "Error",
                "GHCP publication needs a checked-out branch or an explicit AI_TASK",
            )
            .caused_by(Error::new("Error", error.to_string()))
        })?;
    if !output.status.success() {
        return Err(Error::new(
            "Error",
            "GHCP publication needs a checked-out branch or an explicit AI_TASK",
        ));
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let slug = regex_lite::Regex::new(r"[^A-Za-z0-9._-]+")
        .map_err(|error| Error::new("Error", error.to_string()))?
        .replace_all(&branch, "-")
        .chars()
        .take(64)
        .collect::<String>();
    if slug.is_empty() || slug.bytes().all(|b| matches!(b, b'-' | b'.' | b'_')) {
        return Err(Error::new(
            "Error",
            "Set AI_TASK: the branch cannot form an opaque publication slug",
        ));
    }
    Ok(slug)
}

pub fn format_publish_environment(env: &crate::json::Value) -> String {
    let mut output = String::new();
    for name in RUN_ENV_NAMES {
        let value = env
            .get(name)
            .and_then(crate::json::Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_default();
        output.push_str(&format!("{name}={value}\n"));
    }
    output
}

#[derive(Debug, Clone, Default)]
pub struct PublishMessageOptions<'a> {
    pub subject: &'a str,
    pub model: &'a str,
    pub summary: &'a str,
    pub tests_skipped: bool,
    pub issue_number: Option<u64>,
    pub seats: Option<&'a str>,
    pub ghcp: bool,
    pub scope_files: &'a [String],
}

pub fn build_publish_message(options: &PublishMessageOptions<'_>) -> Result<String, Error> {
    if crate::text::trim(&Text::from(options.subject).0).is_empty()
        || options.subject.contains(['\r', '\n', '\0'])
    {
        return Err(Error::type_error(
            "Publish subject must be nonempty single-line text",
        ));
    }
    if crate::text::trim(&Text::from(options.summary).0).is_empty()
        || options.summary.contains('\0')
    {
        return Err(Error::type_error(
            "Publish summary must describe the reviewed changes",
        ));
    }
    if options
        .scope_files
        .iter()
        .any(|file| !crate::execution::policy::scope_expansion_file(file))
    {
        return Err(Error::type_error(
            "Publish scope expansion must list worktree-relative unprotected files",
        ));
    }
    if let Some(seats) = options.seats {
        if crate::text::trim(&Text::from(seats).0).is_empty()
            || seats.contains(['\r', '\n', '\0'])
            || seats.encode_utf16().count() > 200
        {
            return Err(Error::type_error("Publish seats must be one nonempty line"));
        }
    }
    let model_env = crate::json::Value::object(vec![(
        "AI_MODEL",
        crate::json::Value::string(options.model),
    )]);
    let actual_model = resolve_publish_model(None, Some(&model_env), None, false)?;
    let testing = if options.tests_skipped {
        "Automatic tests were explicitly waived by TASK.md (`tests: none`). Review the task acceptance checks."
    } else {
        "Run `node --test` from the feature worktree root and review the task acceptance checks."
    };
    let subject_trimmed =
        Text(crate::text::trim(&Text::from(options.subject).0).to_vec()).to_utf8()?;
    let summary_trimmed =
        Text(crate::text::trim(&Text::from(options.summary).0).to_vec()).to_utf8()?;
    let mut message = format!(
        "{subject_trimmed}\n\n## Model\n\n{actual_model}\n\n## Summary\n\n{summary_trimmed}"
    );
    if let Some(seats) = options.seats {
        message.push_str(&format!("\n\n## Seats\n\n{seats}"));
    }
    if !options.scope_files.is_empty() {
        let bullets = options
            .scope_files
            .iter()
            .map(|file| format!("- `{file}`"))
            .collect::<Vec<_>>()
            .join("\n");
        message.push_str(&format!(
            "\n\n## Files outside planned scope\n\n{bullets}\n\nThe coder expanded beyond TASK.md Allowed Files; the reviewer judged these changes."
        ));
    }
    if options.ghcp {
        message.push_str(
            "\n\n## Metrics\n\nGHCP used/out are `-` (unknown); declared context capacity is not token usage.",
        );
    }
    message.push_str(&format!("\n\n### How to test\n\n{testing}"));
    match options.issue_number {
        Some(issue_number) => issue_merge_message(&message, issue_number),
        None => Ok(message),
    }
}

pub fn format_publish_command(
    message: &str,
    model: &str,
    script: Option<&str>,
    platform_windows: bool,
) -> Result<String, Error> {
    let model_env =
        crate::json::Value::object(vec![("AI_MODEL", crate::json::Value::string(model))]);
    let actual_model = resolve_publish_model(None, Some(&model_env), None, false)?;
    let quoted = if platform_windows {
        format!("'{}'", message.replace('\'', "''"))
    } else {
        format!("'{}'", message.replace('\'', "'\\''"))
    };
    let default_script = if platform_windows {
        "vendor\\github-agent-contracts\\scripts\\agent-pr.mjs"
    } else {
        "vendor/github-agent-contracts/scripts/agent-pr.mjs"
    };
    let script_path = script.unwrap_or(default_script);
    Ok(format!(
        "node {script_path} --message {quoted} --model {actual_model} --merge-when-green"
    ))
}

pub fn merged_pull_number(output: &str) -> Result<u64, Error> {
    let captures = regex_lite::Regex::new(r"(?m)^Merged PR #([1-9]\d*) with a merge commit,")
        .map_err(|error| Error::new("Error", error.to_string()))?
        .captures(output)
        .ok_or_else(|| {
            Error::new(
                "Error",
                "Publisher did not confirm a merged PR; issue remains open",
            )
        })?;
    let number = captures
        .get(1)
        .and_then(|m| m.as_str().parse::<u64>().ok())
        .filter(|n| (1..=MAX_SAFE_INTEGER).contains(n))
        .ok_or_else(|| Error::type_error("Merged PR number must be a positive safe integer"))?;
    Ok(number)
}

pub fn merged_pull_number_from_failure(output: Option<&str>) -> Option<u64> {
    let text = output?;
    let captures = regex_lite::Regex::new(r"\bPR #([1-9]\d*) was merged;")
        .ok()?
        .captures(text)?;
    captures
        .get(1)
        .and_then(|m| m.as_str().parse::<u64>().ok())
        .filter(|n| (1..=MAX_SAFE_INTEGER).contains(n))
}

pub fn human_eval_hint(session: &str) -> Result<String, Error> {
    let valid = regex_lite::Regex::new(
        r"^roster-(?:[1-9]\d*(?:-a(?:[1-9]|1[0-6])-[a-f0-9]{8})?|local-[a-f0-9]{16})-coder$",
    )
    .map_err(|error| Error::new("Error", error.to_string()))?
    .is_match(session);
    if !valid {
        return Err(Error::type_error(
            "Human eval hint requires an issue coder session or a local ask coder session",
        ));
    }
    Ok(format!("roster eval {session} accept 1 n --minutes M"))
}

pub fn parse_trailers(message: &str) -> std::collections::BTreeMap<String, String> {
    let mut trailers = std::collections::BTreeMap::new();
    let normalized = message.replace("\r\n", "\n");
    let trimmed = normalized.trim_end();
    if trimmed.is_empty() {
        return trailers;
    }
    let lines = trimmed.split('\n').collect::<Vec<_>>();
    let mut block = Vec::new();
    let mut found_blank = false;
    for line in lines.iter().rev() {
        if line.trim().is_empty() {
            found_blank = true;
            break;
        }
        let Some((name, val)) = line.split_once(':') else {
            return std::collections::BTreeMap::new();
        };
        let valid_name = name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !valid_name {
            return std::collections::BTreeMap::new();
        }
        block.push((name.to_owned(), val.trim().to_owned()));
    }
    if !found_blank {
        return std::collections::BTreeMap::new();
    }
    for (name, val) in block.into_iter().rev() {
        trailers.insert(name, val);
    }
    trailers
}

pub fn is_agent_authored(message: &str, require_all: bool, author_email: &str) -> bool {
    let bot_email =
        regex_lite::Regex::new(r"(?i)^[^@\s]+\[bot\]@(?:users\.)?noreply\.github\.com$")
            .is_ok_and(|re| re.is_match(author_email));
    require_all || message.contains("AI-Agent:") || bot_email
}

pub fn check_commit_trailers(
    message: &str,
    require_all: bool,
    author_email: &str,
    require_run: bool,
) -> crate::json::Value {
    let trailers = parse_trailers(message);
    let agent = is_agent_authored(message, require_all, author_email);
    let mut missing = Vec::new();
    if agent {
        for key in ["AI-Agent", "AI-Model"] {
            if trailers.get(key).is_none_or(|v| v.trim().is_empty()) {
                missing.push(crate::json::Value::string(key));
            }
        }
        if require_run {
            let run_val = trailers.get("AI-Run").map(|s| Text::from(s.as_str()));
            let model_val = trailers.get("AI-Model").map(|s| Text::from(s.as_str()));
            let valid_run = match run_val {
                Some(run_text) => parse_compact(&run_text, model_val.as_ref())
                    .ok()
                    .flatten()
                    .is_some(),
                None => false,
            };
            if !valid_run {
                missing.push(crate::json::Value::string("AI-Run"));
            }
        }
    }
    let ok = missing.is_empty();
    let trailers_obj = crate::json::Value::object(
        trailers
            .iter()
            .map(|(k, v)| (k.as_str(), crate::json::Value::string(v)))
            .collect(),
    );
    crate::json::Value::object(vec![
        ("agentAuthored", crate::json::Value::Boolean(agent)),
        ("trailers", trailers_obj),
        ("missing", crate::json::Value::Array(missing)),
        ("ok", crate::json::Value::Boolean(ok)),
    ])
}

pub fn check_pull_request_trailers(
    base_sha: &str,
    head_sha: &str,
    require_all: bool,
    cwd: &std::path::Path,
) -> Result<Vec<crate::json::Value>, Error> {
    let sha_re = regex_lite::Regex::new(r"(?i)^[0-9a-f]{40}$")
        .map_err(|error| Error::new("Error", error.to_string()))?;
    for (name, val) in [("baseSha", base_sha), ("headSha", head_sha)] {
        if !sha_re.is_match(val) {
            return Err(Error::new(
                "Error",
                format!("{name} must be a full GitHub commit SHA"),
            ));
        }
    }
    let git_exe = std::env::var_os("ROSTER_TEST_GIT")
        .or_else(|| std::env::var_os("ROSTER_GIT"))
        .unwrap_or_else(|| std::ffi::OsString::from("git"));
    let run_git = |args: &[&str]| -> Result<String, Error> {
        let mut cmd = std::process::Command::new(&git_exe);
        cmd.current_dir(cwd).env_clear().args(args);
        for key in [
            "PATH",
            "SystemRoot",
            "WINDIR",
            "COMSPEC",
            "PATHEXT",
            "TEMP",
            "TMP",
            "HOME",
        ] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        let out = cmd
            .output()
            .map_err(|error| Error::new("Error", error.to_string()))?;
        if !out.status.success() {
            return Err(Error::new(
                "Error",
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            ));
        }
        String::from_utf8(out.stdout).map_err(|error| Error::new("Error", error.to_string()))
    };
    if run_git(&["rev-parse", "--is-shallow-repository"])?.trim() != "false" {
        return Err(Error::new(
            "Error",
            "Full history is required; use actions/checkout with fetch-depth: 0",
        ));
    }
    let range = format!("{base_sha}..{head_sha}");
    let commits = run_git(&["rev-list", "--reverse", &range, "--"])?;
    let trimmed = commits.trim();
    if trimmed.is_empty() {
        return Err(Error::new("Error", "No commits found in the PR range"));
    }
    let mut results = Vec::new();
    for sha in trimmed.split('\n') {
        let message = run_git(&["show", "--no-patch", "--format=%B", sha, "--"])?;
        let author_email = run_git(&["show", "--no-patch", "--format=%ae", sha, "--"])?
            .trim()
            .to_owned();
        let mut checked = check_commit_trailers(&message, require_all, &author_email, false);
        crate::domain::put(&mut checked, "sha", crate::json::Value::string(sha))?;
        results.push(checked);
    }
    Ok(results)
}

pub const RUN_STATUS_LABELS: &[(&str, &str, &str, &str)] = &[
    (
        "in-progress",
        "roster:in-progress",
        "1d76db",
        "A Roster run has claimed this issue",
    ),
    (
        "review",
        "roster:review",
        "0e8a16",
        "Roster delivered a reviewed result; awaiting human AI-Eval",
    ),
    (
        "blocked",
        "roster:blocked",
        "d93f0b",
        "The last Roster run stopped; see the latest status comment",
    ),
];

pub fn format_issue_run_status_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.replace(['\r', '\n'], " ")
                .trim()
                .chars()
                .take(300)
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .take(8)
        .collect()
}

pub fn recorded_coder_run(
    runs: &[crate::json::Value],
    run: &RunRecord,
) -> Result<RunRecord, Error> {
    let session =
        run.metrics.session.as_deref().ok_or_else(|| {
            Error::type_error("Publication requires a measured coder session/task")
        })?;
    let task =
        run.metrics.task.as_deref().ok_or_else(|| {
            Error::type_error("Publication requires a measured coder session/task")
        })?;
    let record = runs
        .iter()
        .rev()
        .find(|entry| {
            crate::domain::is_text(entry.get("session"), session)
                && crate::domain::is_text(entry.get("task"), task)
        })
        .ok_or_else(|| {
            Error::new(
                "Error",
                "Coder run is missing from .roster/runs JSONL; publication is refused",
            )
        })?;
    let provider_str = record
        .get("provider")
        .and_then(crate::json::Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_else(|| run.metrics.provider.as_str().to_owned());
    let provider = Provider::parse(&provider_str)
        .ok_or_else(|| Error::type_error("Recorded coder metrics contain an invalid provider"))?;
    let model = record
        .get("model")
        .and_then(crate::json::Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .ok_or_else(|| Error::type_error("Recorded coder metrics require a model"))?;
    let effort = record
        .get("effort")
        .and_then(crate::json::Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_else(|| "-".to_owned());
    let context_max = match record.get("context_max") {
        Some(crate::json::Value::Number(n)) if n.is_finite() && *n >= 1.0 => {
            Some(Capacity::Number(*n as u64))
        }
        Some(crate::json::Value::String(s)) => Some(Capacity::Decimal(s.to_utf8()?)),
        _ => None,
    };
    let prompt_tokens = crate::domain::safe_integer(record.get("prompt_tokens"))
        .or_else(|| crate::domain::safe_integer(record.get("context_used")));
    let completion_tokens = crate::domain::safe_integer(record.get("completion_tokens"))
        .or_else(|| crate::domain::safe_integer(record.get("context_out")));
    let measured = materialize(
        RunMetrics {
            provider,
            model,
            effort,
            context_max,
            usage: Usage {
                prompt_tokens,
                completion_tokens,
            },
            context_used: prompt_tokens,
            context_out: completion_tokens,
            session: Some(session.to_owned()),
            task: Some(task.to_owned()),
        },
        &run.version,
    )?;
    let expected = materialize(run.metrics.clone(), &run.version)?;
    if measured.metrics() != expected.metrics() {
        return Err(Error::new(
            "Error",
            "Recorded coder metrics differ from the reviewed run; rerun before publication",
        ));
    }
    Ok(measured)
}

pub struct MergedIssueCommentOptions<'a> {
    pub issue_number: u64,
    pub pull_number: u64,
    pub slug: &'a str,
    pub pr: &'a crate::json::Value,
    pub current_issue: &'a crate::json::Value,
    pub run: Option<&'a RunRecord>,
    pub model: Option<&'a str>,
    pub run_line: Option<&'a str>,
}

pub fn build_merged_issue_comment(
    options: &MergedIssueCommentOptions<'_>,
) -> Result<crate::json::Value, Error> {
    let issue_number = options.issue_number;
    let pull_number = options.pull_number;
    let slug = options.slug;
    let pr = options.pr;
    let current_issue = options.current_issue;
    let run = options.run;
    let model = options.model;
    let run_line = options.run_line;
    if !(1..=MAX_SAFE_INTEGER).contains(&issue_number) {
        return Err(Error::type_error(
            "Issue number must be a positive safe integer",
        ));
    }
    if !(1..=MAX_SAFE_INTEGER).contains(&pull_number) {
        return Err(Error::type_error(
            "Merged PR number must be a positive safe integer",
        ));
    }
    let actual_model = match run {
        Some(rec) => rec.metrics.model.clone(),
        None => resolve_publish_model(None, None, model, true)?,
    };
    let effective_run_line = match run {
        Some(rec) => Some(rec.line().to_owned()),
        None => run_line.map(str::to_owned),
    };
    if let Some(ref rl) = effective_run_line {
        if rl.is_empty() || rl.contains(['\r', '\n']) {
            return Err(Error::type_error("AI-Run must be one nonempty line"));
        }
        if parse_compact(
            &Text::from(rl.as_str()),
            Some(&Text::from(actual_model.as_str())),
        )?
        .is_none()
        {
            return Err(Error::new(
                "Error",
                "Coder AI-Run must match the published model",
            ));
        }
    }
    let branches = crate::start::issue_branches(issue_number, None)?;
    let head_ref = pr
        .get("head")
        .and_then(|h| h.get("ref"))
        .and_then(crate::json::Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_default();
    let head_repo = pr
        .get("head")
        .and_then(|h| h.get("repo"))
        .and_then(|r| r.get("full_name"))
        .and_then(crate::json::Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_default();
    let base_repo = pr
        .get("base")
        .and_then(|b| b.get("repo"))
        .and_then(|r| r.get("full_name"))
        .and_then(crate::json::Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_default();
    let html_url = pr
        .get("html_url")
        .and_then(crate::json::Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_default();
    let pr_body = pr
        .get("body")
        .and_then(crate::json::Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_default();
    let expected_url = format!("https://github.com/{slug}/pull/{pull_number}");
    let refs_re = regex_lite::Regex::new(&format!(r"(?m)^Refs #{issue_number}\r?$"))
        .map_err(|e| Error::new("Error", e.to_string()))?;
    if pr.get("merged") != Some(&crate::json::Value::Boolean(true))
        || !crate::domain::is_text(pr.get("state"), "closed")
        || !branches.contains(&head_ref)
        || !head_repo.eq_ignore_ascii_case(slug)
        || !base_repo.eq_ignore_ascii_case(slug)
        || !html_url.eq_ignore_ascii_case(&expected_url)
        || !refs_re.is_match(&pr_body)
    {
        return Err(Error::new(
            "Error",
            "PR was not confirmed merged for this issue; leaving the issue unchanged",
        ));
    }
    if crate::domain::safe_integer(current_issue.get("number")) != Some(issue_number)
        || !crate::domain::is_text(current_issue.get("state"), "open")
    {
        return Err(Error::new(
            "Error",
            "Issue must remain open for human AI-Eval; no comment was posted",
        ));
    }
    let mut body = format!("Merged {html_url} for issue #{issue_number}.\n\nModel: {actual_model}");
    if let Some(rec) = run {
        body.push_str(&format!("\nProvider: {}", rec.metrics.provider.as_str()));
        if let Some(pt) = rec.metrics.usage.prompt_tokens {
            body.push_str(&format!("\nPrompt tokens: {pt}"));
        }
        if let Some(ct) = rec.metrics.usage.completion_tokens {
            body.push_str(&format!("\nCompletion tokens: {ct}"));
        }
        if let Some(ref cm) = rec.metrics.context_max {
            body.push_str(&format!("\nContext max: {}", cm.text()));
        }
        if let Some(ref s) = rec.metrics.session {
            body.push_str(&format!("\nSession: {s}"));
        }
        if let Some(ref t) = rec.metrics.task {
            body.push_str(&format!("\nTask: {t}"));
        }
    }
    if let Some(rl) = effective_run_line {
        body.push_str(&format!("\nAI-Run: {rl}"));
    }
    body.push_str("\n\nIssue remains open for human AI-Eval.");
    Ok(crate::json::Value::object(vec![
        (
            "issueNumber",
            crate::json::Value::Number(issue_number as f64),
        ),
        ("pullNumber", crate::json::Value::Number(pull_number as f64)),
        ("issueState", crate::json::Value::string("open")),
        ("body", crate::json::Value::string(&body)),
    ]))
}

pub fn build_issue_run_status_update(
    issue_number: u64,
    status: &str,
    detail: &[String],
    current_labels: &[String],
) -> Result<crate::json::Value, Error> {
    if !(1..=MAX_SAFE_INTEGER).contains(&issue_number) {
        return Err(Error::type_error(
            "Issue number must be a positive safe integer",
        ));
    }
    let Some(&(_, label_name, color, desc)) = RUN_STATUS_LABELS
        .iter()
        .find(|&&(key, _, _, _)| key == status)
    else {
        return Err(Error::type_error(
            "Run status must be one of in-progress, review, blocked",
        ));
    };
    let heading = match status {
        "in-progress" => "Roster run started",
        "review" => "Roster run delivered for review",
        "blocked" => "Roster run blocked",
        _ => "Roster run status",
    };
    let mut remove_labels = Vec::new();
    for &(_, other_name, _, _) in RUN_STATUS_LABELS {
        if other_name != label_name && current_labels.iter().any(|l| l == other_name) {
            remove_labels.push(crate::json::Value::string(other_name));
        }
    }
    let already_present = current_labels.iter().any(|l| l == label_name);
    let lines = format_issue_run_status_lines(detail);
    let mut body = format!("{heading} for issue #{issue_number}.");
    if !lines.is_empty() {
        let bullets = lines
            .iter()
            .map(|l| format!("- {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        body.push_str(&format!("\n\n{bullets}"));
    }
    body.push_str(&format!("\n\nStatus label: `{label_name}`."));
    Ok(crate::json::Value::object(vec![
        (
            "issueNumber",
            crate::json::Value::Number(issue_number as f64),
        ),
        ("status", crate::json::Value::string(status)),
        ("label", crate::json::Value::string(label_name)),
        ("color", crate::json::Value::string(color)),
        ("description", crate::json::Value::string(desc)),
        ("addLabel", crate::json::Value::Boolean(!already_present)),
        ("removeLabels", crate::json::Value::Array(remove_labels)),
        (
            "claimed",
            crate::json::Value::Boolean(status == "in-progress" && already_present),
        ),
        ("commentBody", crate::json::Value::string(&body)),
    ]))
}

pub fn prepare_verified_publication_handoff(
    worktree: &std::path::Path,
    subject: &str,
    model: &str,
    issue_number: Option<u64>,
    skip_review: bool,
    review_required: bool,
    platform_windows: bool,
) -> Result<crate::json::Value, Error> {
    if skip_review && review_required {
        return Err(Error::type_error(
            "--skip-review requires private review.required: false; required review was not bypassed",
        ));
    }
    let result_path = worktree.join("RESULT.md");
    crate::paths::ensure_local_path(&result_path, worktree)?;
    let result_snap = crate::state::secure::read(&result_path, 65_536)?.ok_or_else(|| {
        Error::new(
            "PublicationError",
            "Verified publication requires an existing RESULT.md in the worktree",
        )
    })?;
    let result_text = String::from_utf8(result_snap.bytes)
        .map_err(|error| Error::new("PublicationError", error.to_string()))?;
    if !result_text
        .lines()
        .any(|line| line.trim() == "Checks: PASS")
    {
        return Err(Error::new(
            "PublicationError",
            "Publication requires RESULT.md with Checks: PASS",
        ));
    }
    let review_path = worktree.join("REVIEW.md");
    crate::paths::ensure_local_path(&review_path, worktree)?;
    let review_snap = crate::state::secure::read(&review_path, 65_536)?;
    let mut review_verdict = "skipped".to_owned();
    if !skip_review {
        let snap = review_snap.ok_or_else(|| {
            Error::new(
                "PublicationError",
                "Verified publication requires an existing REVIEW.md unless --skip-review is authorized",
            )
        })?;
        let review_text = String::from_utf8(snap.bytes)
            .map_err(|error| Error::new("PublicationError", error.to_string()))?;
        if !review_text
            .lines()
            .any(|line| line.trim() == "Verdict: pass")
        {
            return Err(Error::new(
                "PublicationError",
                "Publication requires REVIEW.md with Verdict: pass",
            ));
        }
        review_verdict = "pass".to_owned();
    }
    let run_record = match result_text
        .lines()
        .find_map(|line| line.strip_prefix("AI-Run: "))
    {
        Some(raw) => Some(
            parse_compact(&Text::from(raw.trim()), None)?
                .ok_or_else(|| Error::type_error("RESULT.md contains an empty AI-Run trailer"))?,
        ),
        None => None,
    };
    let resolved_model = resolve_publish_model(
        run_record.as_ref(),
        Some(&crate::json::Value::object(vec![(
            "AI_MODEL",
            crate::json::Value::string(model),
        )])),
        None,
        false,
    )?;
    let summary_section = result_text
        .split("## Summary")
        .nth(1)
        .map(|s| s.split("\n## ").next().unwrap_or(s).trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("Verified slice delivery.");
    let pr_message = build_publish_message(&PublishMessageOptions {
        subject,
        model: &resolved_model,
        summary: summary_section,
        tests_skipped: result_text.contains("Tests skipped:"),
        issue_number,
        scope_files: &[],
        seats: None,
        ghcp: run_record.is_none(),
    })?;
    let command = format_publish_command(&pr_message, &resolved_model, None, platform_windows)?;
    Ok(crate::json::Value::object(vec![
        ("eligible", crate::json::Value::Boolean(true)),
        ("model", crate::json::Value::string(&resolved_model)),
        ("reviewVerdict", crate::json::Value::string(&review_verdict)),
        ("message", crate::json::Value::string(&pr_message)),
        ("command", crate::json::Value::string(&command)),
    ]))
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

    #[test]
    fn publication_message_env_and_merge_helpers_preserve_contracts() -> Result<(), Error> {
        let msg = issue_merge_message("feat: add status section", 42)?;
        assert_eq!(msg, "feat: add status section\n\nRefs #42");
        assert!(issue_merge_message("feat: closes #42", 42).is_err());
        assert!(issue_merge_message("feat: fixes owner/repo#42", 42).is_err());

        let parsed = parse_publish_args("feat: add status --model GPT-6.1-Sol --skip-review")?;
        assert_eq!(parsed.subject.as_deref(), Some("feat: add status"));
        assert_eq!(parsed.model.as_deref(), Some("GPT-6.1-Sol"));
        assert!(parsed.skip_review);
        assert!(parse_publish_args("--model unknown").is_err());

        let ghcp = build_ghcp_run(
            Some(&crate::json::Value::object(vec![
                ("AI_EFFORT", crate::json::Value::string("max")),
                ("AI_CONTEXT_MAX", crate::json::Value::string("1000000")),
            ])),
            Some("GPT-6.1-Sol"),
            Some("ghcp-20261019"),
            Some("issue-42"),
        )?;
        assert_eq!(
            ghcp.line(),
            "1|github-copilot|GPT-6.1-Sol@-|x|-/1000000|-|ghcp-20261019|issue-42"
        );
        assert!(
            build_ghcp_run(
                Some(&crate::json::Value::object(vec![(
                    "AI_CONTEXT_MAX",
                    crate::json::Value::Number(1_000_000.0),
                )])),
                Some("GPT-6.1-Sol"),
                Some("ghcp-20261019"),
                None,
            )
            .is_err()
        );

        let full_msg = build_publish_message(&PublishMessageOptions {
            subject: "feat: issue 42",
            model: "GPT-6.1-Sol",
            summary: "Delivered reviewed slice.",
            tests_skipped: false,
            issue_number: Some(42),
            seats: Some("planner -> coder -> reviewer"),
            ghcp: true,
            scope_files: &["src/extra.mjs".to_owned()],
        })?;
        assert!(full_msg.contains("## Model\n\nGPT-6.1-Sol"));
        assert!(full_msg.contains("## Files outside planned scope\n\n- `src/extra.mjs`"));
        assert!(full_msg.contains("GHCP used/out are `-` (unknown)"));
        assert!(full_msg.ends_with("\n\nRefs #42"));

        let cmd = format_publish_command("feat: commit's message", "GPT-6.1-Sol", None, true)?;
        assert!(cmd.contains(
            "--message 'feat: commit''s message' --model GPT-6.1-Sol --merge-when-green"
        ));

        assert_eq!(
            merged_pull_number("Merged PR #666 with a merge commit, sha 123")?,
            666
        );
        assert_eq!(
            merged_pull_number_from_failure(Some("Error: PR #666 was merged; post-merge failed")),
            Some(666)
        );
        assert_eq!(
            human_eval_hint("roster-42-coder")?,
            "roster eval roster-42-coder accept 1 n --minutes M"
        );
        assert!(human_eval_hint("roster-42-planner").is_err());

        let commit_msg = "feat: add status\n\nAI-Agent: github-agent-roster\nAI-Model: owner/model\nAI-Run: 1|local|owner/model@-|h|-/32768|-|native-session|issue-42\n";
        let checked = check_commit_trailers(commit_msg, false, "dev@example.com", true);
        assert_eq!(checked.get("ok"), Some(&crate::json::Value::Boolean(true)));
        let missing_run = check_commit_trailers(
            "feat: add status\n\nAI-Agent: github-agent-roster\nAI-Model: owner/model\n",
            false,
            "dev@example.com",
            true,
        );
        assert_eq!(
            missing_run.get("ok"),
            Some(&crate::json::Value::Boolean(false))
        );

        let coder_run = materialize(metrics(), "-")?;
        let runs_list = vec![crate::json::Value::object(vec![
            ("session", crate::json::Value::string("native-session")),
            ("task", crate::json::Value::string("issue-42")),
            ("provider", crate::json::Value::string("vllm")),
            ("model", crate::json::Value::string("owner/model")),
            ("effort", crate::json::Value::string("h")),
            ("context_max", crate::json::Value::Number(32768.0)),
        ])];
        let verified_coder = recorded_coder_run(&runs_list, &coder_run)?;
        assert_eq!(verified_coder.line(), coder_run.line());

        let pr_obj = crate::json::Value::object(vec![
            ("merged", crate::json::Value::Boolean(true)),
            ("state", crate::json::Value::string("closed")),
            (
                "head",
                crate::json::Value::object(vec![
                    ("ref", crate::json::Value::string("agents/42")),
                    (
                        "repo",
                        crate::json::Value::object(vec![(
                            "full_name",
                            crate::json::Value::string("owner/repo"),
                        )]),
                    ),
                ]),
            ),
            (
                "base",
                crate::json::Value::object(vec![(
                    "repo",
                    crate::json::Value::object(vec![(
                        "full_name",
                        crate::json::Value::string("owner/repo"),
                    )]),
                )]),
            ),
            (
                "html_url",
                crate::json::Value::string("https://github.com/owner/repo/pull/99"),
            ),
            ("body", crate::json::Value::string("Summary\n\nRefs #42\n")),
        ]);
        let open_issue = crate::json::Value::object(vec![
            ("number", crate::json::Value::Number(42.0)),
            ("state", crate::json::Value::string("open")),
        ]);
        let merged_comment = build_merged_issue_comment(&MergedIssueCommentOptions {
            issue_number: 42,
            pull_number: 99,
            slug: "owner/repo",
            pr: &pr_obj,
            current_issue: &open_issue,
            run: Some(&coder_run),
            model: None,
            run_line: None,
        })?;
        let body_str = merged_comment
            .get("body")
            .and_then(crate::json::Value::as_string)
            .ok_or_else(|| Error::type_error("missing body"))?
            .to_utf8()?;
        assert!(body_str.contains("Merged https://github.com/owner/repo/pull/99 for issue #42."));
        assert!(body_str.contains("Issue remains open for human AI-Eval."));

        let status_upd = build_issue_run_status_update(
            42,
            "review",
            &["PR #99 merged; awaiting human AI-Eval.".to_owned()],
            &["roster:in-progress".to_owned(), "enhancement".to_owned()],
        )?;
        assert_eq!(
            status_upd.get("label"),
            Some(&crate::json::Value::string("roster:review"))
        );
        assert_eq!(
            status_upd.get("removeLabels"),
            Some(&crate::json::Value::Array(vec![
                crate::json::Value::string("roster:in-progress")
            ]))
        );
        Ok(())
    }
}
