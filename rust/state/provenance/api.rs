use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::cancel::CancellationToken;
use crate::domain;
use crate::identity;
use crate::json::Value;
use crate::state::redaction;
use crate::text::{Text, trim};

use super::{schema, store};

pub const RAW_HISTORY: &str = "raw-history";
pub const CURATED_MEMORY: &str = "curated-memory";
pub const COMPACTION: &str = "compaction";
pub const LIFECYCLE_EVENTS: [&str; 5] =
    ["started", "session", "failure", "cancellation", "completed"];

pub fn nonempty(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_string)
        .is_some_and(|text| !trim(&text.0).is_empty())
}

pub fn validate_payload(payload: &Value) -> Result<(), Error> {
    if !matches!(payload, Value::Object(_)) {
        return Err(Error::type_error(
            "provenance payload must be a plain JSON object",
        ));
    }
    schema::validate_data(payload, "payload")
}

fn field<'a>(value: &'a Value, name: &str, source: &str) -> Result<&'a Text, Error> {
    value
        .get(name)
        .and_then(Value::as_string)
        .filter(|text| !trim(&text.0).is_empty())
        .ok_or_else(|| Error::type_error(format!("{source} {name} must be a non-empty string")))
}

pub fn validate_typed_record(record: &Value, source: &str) -> Result<Value, Error> {
    if !matches!(record, Value::Object(_)) {
        return Err(Error::type_error(format!("{source} must be an object")));
    }
    let run = field(record, "runId", source)?;
    let session = field(record, "sessionId", source)?;
    let event = field(record, "event", source)?;
    if !LIFECYCLE_EVENTS
        .iter()
        .any(|value| event.0.iter().copied().eq(value.encode_utf16()))
    {
        return Err(Error::type_error(format!(
            "{source} event must be one of {}",
            LIFECYCLE_EVENTS.join(", ")
        )));
    }
    if let Some(identity) = record.get("repoIdentity") {
        let valid = identity
            .as_string()
            .and_then(|text| text.to_utf8().ok())
            .is_some_and(|identity| {
                identity.split_once('-').is_some_and(|(algorithm, hash)| {
                    !algorithm.is_empty()
                        && algorithm
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                        && hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            });
        if !valid {
            return Err(Error::type_error(format!(
                "{source} repoIdentity must be a derived identity hash (algorithm-hex)"
            )));
        }
    }
    let empty = Value::Object(Vec::new());
    let payload = record.get("payload").unwrap_or(&empty);
    validate_payload(payload)?;
    let mut envelope = record.clone();
    if record.get("id").is_none() {
        let mut id = run.clone();
        id.push_str("/");
        id.0.extend_from_slice(&session.0);
        domain::put(&mut envelope, "id", Value::String(id))?;
    }
    if record.get("version").is_none() {
        domain::put(&mut envelope, "version", Value::Number(1.0))?;
    }
    if let Some(reason) = schema::validate_record(&envelope).reason {
        return Err(Error::type_error(format!(
            "{source} failed earlier-wave validation: {reason}"
        )));
    }
    let mut result = Value::Object(Vec::new());
    if let Some(identity) = record.get("repoIdentity") {
        domain::put(&mut result, "repoIdentity", identity.clone())?;
    }
    for (name, text) in [("runId", run), ("sessionId", session), ("event", event)] {
        domain::put(&mut result, name, Value::String(text.clone()))?;
    }
    domain::put(&mut result, "payload", payload.clone())?;
    Ok(result)
}

pub fn build_record(
    input: &Value,
    repo_identity: Option<&Value>,
    now: Value,
    config: &redaction::Config,
    redact: bool,
) -> Result<Value, Error> {
    let empty = Value::Object(Vec::new());
    let payload = input.get("payload").unwrap_or(&empty);
    validate_payload(payload)?;
    let mut factory = Value::Object(Vec::new());
    for name in [
        "repository",
        "issue",
        "seat",
        "route",
        "requestedModel",
        "servedModel",
        "startedAt",
        "endedAt",
        "outcome",
        "tools",
        "metrics",
        "evidence",
    ] {
        if let Some(value) = payload.get(name) {
            domain::put(&mut factory, name, value.clone())?;
        }
    }
    for (source, destination) in [("runId", "run"), ("sessionId", "session")] {
        if let Some(value) = input.get(source) {
            domain::put(&mut factory, destination, value.clone())?;
        }
    }
    let mut record = schema::create_record(&factory, now)?;
    let mut route = record
        .get("route")
        .cloned()
        .ok_or_else(|| Error::type_error("Provenance factory omitted route"))?;
    for name in ["profile", "hardware"] {
        if let Some(value) = payload
            .get("route")
            .and_then(|route| route.get(name))
            .filter(|value| matches!(value, Value::String(_)))
        {
            domain::put(&mut route, name, value.clone())?;
        }
    }
    domain::put(&mut record, "route", route)?;
    for name in ["runId", "sessionId", "event"] {
        if let Some(value) = input.get(name) {
            domain::put(&mut record, name, value.clone())?;
        } else {
            domain::remove(&mut record, name)?;
        }
    }
    domain::put(&mut record, "payload", payload.clone())?;
    if let Some(identity) = repo_identity.filter(|identity| nonempty(Some(identity))) {
        domain::put(&mut record, "repoIdentity", identity.clone())?;
    }
    if redact {
        redaction::record(&record, config)
    } else {
        Ok(record)
    }
}

pub fn store_record_id(
    identity: &str,
    run: &str,
    session: &str,
    event: &str,
    section: &str,
) -> String {
    identity::sha256_hex(format!("{identity}|{run}|{session}|{event}|{section}").as_bytes())
}

pub fn store_record_id_values(fields: &[Option<&Value>; 5]) -> Result<String, Error> {
    let strings = fields
        .iter()
        .map(|value| value.map_or_else(|| Ok(Text::from("undefined")), domain::string))
        .collect::<Result<Vec<_>, Error>>()?;
    let raw = strings
        .iter()
        .map(|text| String::from_utf16_lossy(&text.0))
        .collect::<Vec<_>>()
        .join("|");
    Ok(identity::sha256_hex(raw.as_bytes()))
}

pub fn build_compaction_provenance(
    record: &Value,
    identity: &str,
    config: &redaction::Config,
) -> Result<Value, Error> {
    build_compaction_provenance_at(record, identity, crate::clock::now_milliseconds()?, config)
}

pub fn build_compaction_provenance_at(
    record: &Value,
    identity: &str,
    now: i64,
    config: &redaction::Config,
) -> Result<Value, Error> {
    if !matches!(record, Value::Object(_)) {
        return Err(Error::type_error("compaction source must be an object"));
    }
    schema::validate_data(record, "compaction source")?;
    let run = record
        .get("runId")
        .filter(|value| nonempty(Some(value)))
        .cloned()
        .unwrap_or_else(|| Value::string("unknown"));
    let session = record
        .get("sessionId")
        .filter(|value| nonempty(Some(value)))
        .cloned()
        .unwrap_or_else(|| Value::string("unknown"));
    let event = record
        .get("event")
        .filter(|value| nonempty(Some(value)))
        .cloned()
        .unwrap_or_else(|| Value::string("compacted"));
    let digest = identity::sha256_hex(record.json().as_bytes());
    let event_digest = Value::string(&format!(
        "{}:{digest}",
        String::from_utf16_lossy(&domain::string(&event)?.0)
    ));
    let id_value = Value::string(identity);
    let raw_value = Value::string(RAW_HISTORY);
    let source_id = store_record_id_values(&[
        Some(&id_value),
        Some(&run),
        Some(&session),
        Some(&event_digest),
        Some(&raw_value),
    ])?;
    let mut payload = record.clone();
    for (name, value) in [("runId", run.clone()), ("sessionId", session.clone())] {
        domain::put(&mut payload, name, value)?;
    }
    for (name, first) in [("sourceRunIds", &run), ("sourceSessionIds", &session)] {
        let mut values = vec![first.clone()];
        if let Some(prior) = record.get(name).filter(|value| *value != &Value::Null) {
            let prior = domain::items(prior)
                .ok_or_else(|| Error::type_error(format!("{name} must be an array")))?;
            for value in prior {
                if !values.contains(value) {
                    values.push(value.clone());
                }
            }
        }
        domain::put(&mut payload, name, Value::Array(values))?;
    }
    domain::put(&mut payload, "sourceRecordId", Value::string(&source_id))?;
    let input = Value::object(vec![
        ("runId", run),
        ("sessionId", session),
        ("event", event),
        ("payload", payload),
    ]);
    let provenance = build_record(&input, None, Value::Number(now as f64), config, true)?;
    if input
        .get("event")
        .and_then(Value::as_string)
        .is_some_and(|event| {
            LIFECYCLE_EVENTS
                .iter()
                .any(|name| event.0.iter().copied().eq(name.encode_utf16()))
        })
    {
        validate_typed_record(&provenance, "compaction provenance")?;
    }
    Ok(Value::object(vec![
        ("sourceRecordId", Value::string(&source_id)),
        ("provenance", provenance),
    ]))
}

pub fn build_compaction_record_at(
    record: &Value,
    identity: &str,
    now: i64,
    config: &redaction::Config,
) -> Result<Value, Error> {
    let built = build_compaction_provenance_at(record, identity, now, config)?;
    let provenance = built
        .get("provenance")
        .ok_or_else(|| Error::type_error("Compaction provenance is missing"))?;
    let source = built
        .get("sourceRecordId")
        .ok_or_else(|| Error::type_error("Compaction source identity is missing"))?;
    let event = Value::string(&format!(
        "compacted:{}",
        String::from_utf16_lossy(&domain::string(source)?.0)
    ));
    let identity = Value::string(identity);
    let section = Value::string(COMPACTION);
    let id = store_record_id_values(&[
        Some(&identity),
        provenance.get("runId"),
        provenance.get("sessionId"),
        Some(&event),
        Some(&section),
    ])?;
    Ok(Value::object(vec![
        ("id", Value::string(&id)),
        ("section", section),
        (
            "runId",
            provenance.get("runId").cloned().unwrap_or(Value::Null),
        ),
        (
            "sessionId",
            provenance.get("sessionId").cloned().unwrap_or(Value::Null),
        ),
        (
            "event",
            provenance.get("event").cloned().unwrap_or(Value::Null),
        ),
        (
            "sourceRunIds",
            provenance
                .get("payload")
                .and_then(|payload| payload.get("sourceRunIds"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
        (
            "sourceSessionIds",
            provenance
                .get("payload")
                .and_then(|payload| payload.get("sourceSessionIds"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
        ("sourceRecordId", source.clone()),
        ("provenance", provenance.clone()),
        (
            "payload",
            provenance.get("payload").cloned().unwrap_or(Value::Null),
        ),
    ]))
}

pub async fn capture_lifecycle(
    store: &mut ProvenanceStore,
    context: &Value,
    event: &str,
    opt_out: bool,
    now: i64,
    token: Option<&CancellationToken>,
) -> Result<Value, Error> {
    if let Some(token) = token {
        token.check()?;
    }
    if !LIFECYCLE_EVENTS.contains(&event) {
        return Err(Error::type_error(
            "Lifecycle capture requires a known event",
        ));
    }
    if opt_out {
        let mut result = Value::Object(Vec::new());
        for name in ["runId", "sessionId"] {
            if let Some(value) = context.get(name) {
                domain::put(&mut result, name, value.clone())?;
            }
        }
        domain::put(&mut result, "event", Value::string(event))?;
        domain::put(&mut result, "durable", Value::Boolean(false))?;
        return Ok(result);
    }
    let mut input = domain::spread(context);
    domain::put(&mut input, "event", Value::string(event))?;
    let mut result = store.record_event(&input, now, token).await?;
    domain::put(&mut result, "durable", Value::Boolean(true))?;
    if let Some(token) = token {
        token.check()?;
    }
    Ok(result)
}

pub enum Identity {
    Explicit(String),
    Repository(PathBuf),
}

pub struct ProvenanceStore {
    pub underlying: store::Store,
    identity: Identity,
    sequence: u64,
}

#[derive(Default)]
pub struct Query<'a> {
    pub repo_identity: Option<&'a str>,
    pub run_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub event: Option<&'a str>,
    pub section: Option<&'a str>,
}

impl ProvenanceStore {
    pub fn open(root: &Path, identity: Identity) -> Result<Self, Error> {
        Ok(Self {
            underlying: store::Store::open(root)?,
            identity,
            sequence: 0,
        })
    }

    pub async fn identity(&self, token: Option<&CancellationToken>) -> Result<String, Error> {
        let identity = match &self.identity {
            Identity::Explicit(identity) => identity.clone(),
            Identity::Repository(root) => identity::resolve_repo_identity(root, token).await?,
        };
        if trim(&Text::from(identity.as_str()).0).is_empty() {
            return Err(Error::type_error(
                "resolveIdentity must resolve to a non-empty identity hash",
            ));
        }
        Ok(identity)
    }

    pub async fn record_event(
        &mut self,
        input: &Value,
        now: i64,
        token: Option<&CancellationToken>,
    ) -> Result<Value, Error> {
        let identity = Value::string(&self.identity(token).await?);
        let mut record = build_record(
            input,
            Some(&identity),
            Value::Number(now as f64),
            &self.underlying.redaction,
            true,
        )?;
        validate_typed_record(&record, "provenance record")?;
        self.persist(
            &mut record,
            &identity,
            RAW_HISTORY,
            record_event(input)?,
            token,
        )
        .await?;
        if let Some(token) = token {
            token.check()?;
        }
        Ok(record)
    }

    async fn persist(
        &mut self,
        record: &mut Value,
        identity: &Value,
        section: &str,
        event: &Value,
        token: Option<&CancellationToken>,
    ) -> Result<(), Error> {
        let section_value = Value::string(section);
        let id = store_record_id_values(&[
            Some(identity),
            record.get("runId"),
            record.get("sessionId"),
            Some(event),
            Some(&section_value),
        ])?;
        domain::put(record, "id", Value::string(&id))?;
        domain::put(record, "version", Value::Number(1.0))?;
        domain::put(record, "section", section_value)?;
        if self.sequence > crate::MAX_SAFE_INTEGER {
            return Err(Error::type_error(
                "Provenance sequence exceeds the safe-integer range",
            ));
        }
        domain::put(record, "seq", Value::Number(self.sequence as f64))?;
        self.sequence += 1;
        self.underlying.append_prepared(record, token).await?;
        Ok(())
    }

    pub async fn record_memory(
        &mut self,
        input: &Value,
        token: Option<&CancellationToken>,
    ) -> Result<Value, Error> {
        let identity = Value::string(&self.identity(token).await?);
        if !nonempty(input.get("memory")) {
            return Err(Error::type_error(
                "curated memory requires a non-empty memory string",
            ));
        }
        if let Some(payload) = input.get("payload") {
            validate_payload(payload)?;
        }
        let mut record = Value::object(vec![("repoIdentity", identity.clone())]);
        for name in ["runId", "sessionId", "memory", "payload"] {
            if let Some(value) = input.get(name) {
                domain::put(&mut record, name, value.clone())?;
            }
        }
        domain::put(&mut record, "event", Value::string("memory"))?;
        let mut record = redaction::record(&record, &self.underlying.redaction)?;
        for name in ["repoIdentity", "runId", "sessionId", "memory"] {
            field(&record, name, "curated memory record")?;
        }
        self.persist(
            &mut record,
            &identity,
            CURATED_MEMORY,
            &Value::string("memory"),
            token,
        )
        .await?;
        if let Some(token) = token {
            token.check()?;
        }
        Ok(record)
    }

    pub async fn query(
        &self,
        query: Query<'_>,
        token: Option<&CancellationToken>,
    ) -> Result<Vec<Value>, Error> {
        let identity = self.identity(token).await?;
        if query
            .repo_identity
            .is_some_and(|requested| requested != identity)
        {
            return Ok(Vec::new());
        }
        let section = query.section.unwrap_or(RAW_HISTORY);
        if !matches!(section, RAW_HISTORY | CURATED_MEMORY) {
            return Err(Error::type_error(
                "section must be \"raw-history\" or \"curated-memory\"",
            ));
        }
        if let Some(token) = token {
            token.check()?;
        }
        let loaded = self.underlying.read_all()?;
        if !loaded.skipped.is_empty() {
            return Err(Error::new("ProvenanceStoreError", format!("Provenance contains {} malformed or incompatible record(s); run store.repair() to quarantine them before querying.", loaded.skipped.len())).coded("E_INVALID_RECORD"));
        }
        let matches = |record: &Value, name: &str, expected: Option<&str>| {
            expected.is_none_or(|expected| domain::is_text(record.get(name), expected))
        };
        let mut records = loaded
            .records
            .into_iter()
            .filter(|record| {
                domain::is_text(record.get("repoIdentity"), &identity)
                    && domain::is_text(record.get("section"), section)
                    && matches(record, "runId", query.run_id)
                    && matches(record, "sessionId", query.session_id)
                    && matches(record, "event", query.event)
            })
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            let number = |value: &Value| value.get("seq").and_then(Value::as_number).unwrap_or(0.0);
            match number(left).partial_cmp(&number(right)) {
                Some(order) => order,
                None => Ordering::Equal,
            }
        });
        if let Some(token) = token {
            token.check()?;
        }
        Ok(records)
    }
}

fn record_event(input: &Value) -> Result<&Value, Error> {
    input
        .get("event")
        .ok_or_else(|| Error::type_error("provenance record event must be a non-empty string"))
}
