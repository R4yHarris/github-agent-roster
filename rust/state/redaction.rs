use std::collections::BTreeSet;

use crate::Error;
use crate::domain;
use crate::json::Value;
use crate::text::{Text, is_space};

#[derive(Debug, Clone)]
pub struct Config {
    pub environment: Value,
    pub api_key_env: Text,
    pub credential_values: Vec<Text>,
    pub endpoint_values: Vec<Text>,
    pub values: Vec<Text>,
    pub paths: Vec<Text>,
    pub home: Option<Text>,
    pub keep_home: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            environment: Value::Object(Vec::new()),
            api_key_env: Text::from("ROSTER_API_KEY"),
            credential_values: Vec::new(),
            endpoint_values: Vec::new(),
            values: Vec::new(),
            paths: Vec::new(),
            home: None,
            keep_home: false,
        }
    }
}

fn iterable(value: &Value, source: &str) -> Result<Vec<Value>, Error> {
    match value {
        Value::Array(values) => Ok(values.clone()),
        Value::String(text) => {
            let mut values = Vec::new();
            let mut position = 0;
            while let Some(&first) = text.0.get(position) {
                if let Some(&second) = text.0.get(position + 1).filter(|second| {
                    (0xd800..=0xdbff).contains(&first) && (0xdc00..=0xdfff).contains(*second)
                }) {
                    values.push(Value::String(Text(vec![first, second])));
                    position += 2;
                } else {
                    values.push(Value::String(Text(vec![first])));
                    position += 1;
                }
            }
            Ok(values)
        }
        _ => Err(Error::type_error(format!("{source} is not iterable"))),
    }
}

impl Config {
    pub fn add_selected_credentials(&mut self, selected: &Value) -> Result<(), Error> {
        let values = domain::members(selected).ok_or_else(|| {
            Error::type_error("Selected credentials must be a typed environment mapping")
        })?;
        for (_, value) in values {
            let value = value
                .as_string()
                .ok_or_else(|| Error::type_error("Selected credential values must be text"))?;
            if !value.0.is_empty() && !self.credential_values.contains(value) {
                self.credential_values.push(value.clone());
            }
        }
        Ok(())
    }

    pub fn history(options: &Value) -> Result<Self, Error> {
        if options == &Value::Null {
            return Err(Error::type_error(
                "Cannot read properties of null (reading 'env')",
            ));
        }
        let mut config = Self {
            environment: options
                .get("env")
                .cloned()
                .unwrap_or(Value::Object(Vec::new())),
            home: options.get("home").map(domain::string).transpose()?,
            keep_home: options.get("keepHome").is_some_and(domain::truthy),
            ..Self::default()
        };
        for (key, destination) in [("values", &mut config.values), ("paths", &mut config.paths)] {
            if let Some(values) = options.get(key).filter(|value| domain::truthy(value)) {
                destination.extend(
                    iterable(values, &format!("options.{key}"))?
                        .iter()
                        .filter_map(Value::as_string)
                        .filter(|text| !text.0.is_empty())
                        .cloned(),
                );
            }
        }
        let names = match options
            .get("secretEnvNames")
            .filter(|value| *value != &Value::Null)
        {
            Some(value) => iterable(value, "envNames")?,
            None => vec![Value::string("API_KEY"), Value::string("PRIVATE_KEY_PATH")],
        };
        for name in names {
            if let Some(value) = config
                .environment
                .get(&domain::string(&name)?.to_utf8()?)
                .and_then(Value::as_string)
                .filter(|text| !text.0.is_empty())
            {
                config.values.push(value.clone());
            }
        }
        config.validate()?;
        config.environment = Value::Object(Vec::new());
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.environment == Value::Null {
            return Err(Error::type_error(
                "Cannot convert undefined or null to object",
            ));
        }
        Ok(())
    }

    pub fn secret_values(&self) -> Vec<&Text> {
        let mut values = self
            .credential_values
            .iter()
            .filter(|value| !value.0.is_empty())
            .collect::<Vec<_>>();
        values.extend(
            domain::members(&self.environment)
                .unwrap_or(&[])
                .iter()
                .filter_map(|(name, value)| {
                    let value = value.as_string().filter(|text| !text.0.is_empty())?;
                    let upper = String::from_utf16_lossy(&name.0).to_ascii_uppercase();
                    (name == &self.api_key_env
                        || ["TOKEN", "PASSWORD", "SECRET", "PRIVATE_KEY", "API_KEY"]
                            .iter()
                            .any(|part| upper.contains(part)))
                    .then_some(value)
                })
                .filter(|value| !self.credential_values.contains(value)),
        );
        values.sort_by_key(|value| std::cmp::Reverse(value.0.len()));
        values
    }
}

fn ascii_alphanumeric(unit: u16) -> bool {
    unit <= 127 && (unit as u8).is_ascii_alphanumeric()
}

fn word(unit: u16) -> bool {
    ascii_alphanumeric(unit) || unit == 95
}

fn name_unit(unit: u16) -> bool {
    ascii_alphanumeric(unit) || matches!(unit, 45 | 95)
}

fn starts(units: &[u16], position: usize, value: &str) -> bool {
    units
        .get(position..position.saturating_add(value.len()))
        .is_some_and(|slice| slice.iter().copied().eq(value.encode_utf16()))
}

fn boundary(units: &[u16], position: usize) -> bool {
    units.get(position).is_some_and(|unit| word(*unit))
        != position
            .checked_sub(1)
            .and_then(|index| units.get(index))
            .is_some_and(|unit| word(*unit))
}

pub fn replace(text: &Text, needle: &Text, marker: &str) -> Text {
    if needle.0.is_empty() {
        return text.clone();
    }
    let mut output = Text::default();
    let mut position = 0;
    while let Some(&unit) = text.0.get(position) {
        if text
            .0
            .get(position..position.saturating_add(needle.0.len()))
            == Some(needle.0.as_slice())
        {
            output.push_str(marker);
            position += needle.0.len();
        } else {
            output.0.push(unit);
            position += 1;
        }
    }
    output
}

pub fn replace_ascii_case_insensitive(text: &Text, needle: &Text, marker: &str) -> Text {
    fn fold(unit: u16) -> u16 {
        if (65..=90).contains(&unit) {
            unit + 32
        } else {
            unit
        }
    }
    if needle.0.is_empty() {
        return text.clone();
    }
    let mut output = Text::default();
    let mut offset = 0;
    while let Some(unit) = text.0.get(offset) {
        let matches = text
            .0
            .get(offset..offset.saturating_add(needle.0.len()))
            .is_some_and(|units| {
                units
                    .iter()
                    .zip(&needle.0)
                    .all(|(left, right)| fold(*left) == fold(*right))
            });
        if matches {
            output.push_str(marker);
            offset += needle.0.len();
        } else {
            output.0.push(*unit);
            offset += 1;
        }
    }
    output
}

fn credential_end(units: &[u16], position: usize) -> Option<usize> {
    if !boundary(units, position) {
        return None;
    }
    for (prefix, minimum, underscore, dash) in [
        ("ghp_", 20, false, false),
        ("gho_", 20, false, false),
        ("ghu_", 20, false, false),
        ("ghs_", 20, false, false),
        ("ghr_", 20, false, false),
        ("github_pat_", 20, true, false),
        ("sk-", 24, true, true),
    ] {
        if !starts(units, position, prefix) {
            continue;
        }
        let start = position + prefix.len();
        let mut end = start;
        while units.get(end).is_some_and(|unit| {
            ascii_alphanumeric(*unit) || underscore && *unit == 95 || dash && *unit == 45
        }) {
            end += 1;
        }
        while end >= start + minimum && !boundary(units, end) {
            end -= 1;
        }
        if end >= start + minimum {
            return Some(end);
        }
    }
    None
}

fn pem_header_end(units: &[u16], position: usize, kind: &str) -> Option<usize> {
    if !starts(units, position, kind) {
        return None;
    }
    let mut end = position + kind.len();
    while units
        .get(end)
        .is_some_and(|unit| !matches!(*unit, 45 | 10 | 13))
    {
        end += 1;
    }
    let name = units.get(position + kind.len()..end)?;
    (name.ends_with(&Text::from("PRIVATE KEY").0) && starts(units, end, "-----")).then_some(end + 5)
}

fn skip_separator(units: &[u16], mut position: usize) -> usize {
    loop {
        if units.get(position) == Some(&92)
            && units
                .get(position + 1)
                .is_some_and(|unit| matches!(*unit, 114 | 110))
        {
            position += 2;
        } else if units
            .get(position)
            .is_some_and(|unit| is_space(*unit) || matches!(*unit, 34 | 39 | 96 | 43 | 44))
        {
            position += 1;
        } else {
            return position;
        }
    }
}

fn base64(unit: u16) -> bool {
    ascii_alphanumeric(unit) || matches!(unit, 43 | 47 | 61)
}

fn pem_end(units: &[u16], position: usize, scan: bool) -> Option<usize> {
    let mut cursor = pem_header_end(units, position, "-----BEGIN ")?;
    if !scan {
        loop {
            let start = skip_separator(units, cursor);
            let mut end = start;
            while units.get(end).is_some_and(|unit| {
                (*unit <= 127 && (*unit as u8).is_ascii_alphabetic()) || *unit == 45
            }) {
                end += 1;
            }
            if end > start && units.get(end) == Some(&58) {
                end += 1;
                while units
                    .get(end)
                    .is_some_and(|unit| !matches!(*unit, 10 | 13 | 92 | 34 | 39 | 96))
                {
                    end += 1;
                }
                cursor = end;
            } else {
                break;
            }
        }
    }
    let mut body_end = None;
    loop {
        let start = skip_separator(units, cursor);
        let mut end = start;
        while units.get(end).is_some_and(|unit| base64(*unit)) {
            end += 1;
        }
        if end.saturating_sub(start) < if scan { 40 } else { 16 } {
            break;
        }
        body_end = Some(end);
        cursor = end;
        if scan {
            return body_end;
        }
    }
    let body_end = body_end?;
    let footer_start = skip_separator(units, body_end);
    let mut short_end = footer_start;
    while units.get(short_end).is_some_and(|unit| base64(*unit)) {
        short_end += 1;
    }
    let footer = skip_separator(units, short_end);
    Some(pem_header_end(units, footer, "-----END ").unwrap_or(body_end))
}

fn code_reference(units: &[u16]) -> bool {
    if units.len() >= 2
        && units.first().is_some_and(|unit| (65..=90).contains(unit))
        && units.last().is_some_and(|unit| (65..=90).contains(unit))
        && units
            .iter()
            .all(|unit| (65..=90).contains(unit) || *unit == 95)
    {
        return true;
    }
    if ["null", "undefined", "true", "false"]
        .iter()
        .any(|value| units.iter().copied().eq(value.encode_utf16()))
    {
        return true;
    }
    let initial =
        |unit: u16| unit <= 127 && ((unit as u8).is_ascii_alphabetic() || matches!(unit, 36 | 95));
    let mut cursor = 0;
    let mut members = 0;
    loop {
        if !units.get(cursor).is_some_and(|unit| initial(*unit)) {
            return false;
        }
        cursor += 1;
        while units
            .get(cursor)
            .is_some_and(|unit| word(*unit) || *unit == 36)
        {
            cursor += 1;
        }
        if cursor == units.len() {
            return members > 0;
        }
        if units.get(cursor) == Some(&63) {
            cursor += 1;
        }
        if units.get(cursor) != Some(&46) {
            return false;
        }
        cursor += 1;
        members += 1;
    }
}

fn assignment(units: &[u16], position: usize, secret: bool) -> Option<(usize, Text)> {
    if position
        .checked_sub(1)
        .and_then(|index| units.get(index))
        .is_some_and(|unit| name_unit(*unit))
    {
        return None;
    }
    let mut key_end = position;
    while units.get(key_end).is_some_and(|unit| name_unit(*unit)) {
        key_end += 1;
    }
    let key = units.get(position..key_end)?;
    let key_string = String::from_utf16_lossy(key).to_ascii_lowercase();
    if if secret {
        !key_string.contains("secret")
    } else {
        !["apikey", "api_key", "api-key", "x-api-key"]
            .iter()
            .any(|part| key_string.contains(part))
    } {
        return None;
    }
    let mut value_start = key_end;
    while units.get(value_start).is_some_and(|unit| is_space(*unit)) {
        value_start += 1;
    }
    if !units
        .get(value_start)
        .is_some_and(|unit| matches!(*unit, 58 | 61))
    {
        return None;
    }
    value_start += 1;
    while units.get(value_start).is_some_and(|unit| is_space(*unit)) {
        value_start += 1;
    }
    let first = *units.get(value_start)?;
    let quoted = matches!(first, 34 | 39);
    let end = if quoted {
        let mut cursor = value_start + 1;
        let mut close = None;
        while let Some(&unit) = units.get(cursor) {
            if unit == 92 {
                cursor += 2;
                continue;
            }
            if unit == first {
                close = Some(cursor);
                break;
            }
            cursor += 1;
        }
        let close = close?;
        if close == value_start + 1 {
            return None;
        }
        close + 1
    } else {
        let mut end = value_start;
        while units
            .get(end)
            .is_some_and(|unit| !is_space(*unit) && !matches!(*unit, 44 | 59 | 34 | 39))
        {
            end += 1;
        }
        while end > value_start && !boundary(units, end) {
            end -= 1;
        }
        if end == value_start {
            return None;
        }
        end
    };
    let value = units.get(value_start..end)?;
    if code_reference(value) {
        return Some((end, Text(units.get(position..end)?.to_vec())));
    }
    let mut replacement = Text(key.to_vec());
    replacement.push_str(": ");
    if quoted {
        replacement.0.push(first);
    }
    replacement.push_str(if key_string.to_ascii_lowercase().contains("api") {
        "[REDACTED:API_KEY]"
    } else {
        "[REDACTED:SECRET]"
    });
    if quoted {
        replacement.0.push(first);
    }
    Some((end, replacement))
}

fn substitute(text: &Text, matcher: impl Fn(&[u16], usize) -> Option<(usize, Text)>) -> Text {
    let mut result = Text::default();
    let mut position = 0;
    while let Some(&unit) = text.0.get(position) {
        if let Some((end, replacement)) = matcher(&text.0, position) {
            result.0.extend(replacement.0);
            position = end;
        } else {
            result.0.push(unit);
            position += 1;
        }
    }
    result
}

pub fn evidence(text: &Text, config: &Config) -> Text {
    let mut safe = text.clone();
    for value in config.secret_values() {
        safe = replace(&safe, value, "[redacted]");
    }
    safe = substitute(&safe, |units, position| {
        pem_end(units, position, false).map(|end| (end, Text::from("[REDACTED:PRIVATE_KEY]")))
    });
    safe = substitute(&safe, |units, position| {
        credential_end(units, position).map(|end| (end, Text::from("[REDACTED:API_KEY]")))
    });
    safe = substitute(&safe, |units, position| assignment(units, position, false));
    substitute(&safe, |units, position| assignment(units, position, true))
}

pub fn configured_string(text: &Text, config: &Config) -> Text {
    let mut result = text.clone();
    for source in [&config.values, &config.paths] {
        let mut source = source.iter().collect::<Vec<_>>();
        source.sort_by_key(|value| std::cmp::Reverse(value.0.len()));
        for value in source {
            result = replace(&result, value, "[REDACTED]");
        }
    }
    result
}

pub fn diagnostic(text: &Text, config: &Config) -> Text {
    let mut safe = configured_string(&evidence(text, config), config);
    let mut endpoints = config.endpoint_values.iter().collect::<Vec<_>>();
    endpoints.sort_by_key(|value| std::cmp::Reverse(value.0.len()));
    for endpoint in endpoints {
        safe = replace_ascii_case_insensitive(&safe, endpoint, "[REDACTED]");
    }
    safe
}

pub fn error(error: &Error, config: &Config) -> Error {
    fn redact(error: &Error, config: &Config, depth: usize) -> Error {
        let text = error
            .message_text
            .clone()
            .unwrap_or_else(|| Text::from(error.message.as_str()));
        let mut safe = Error::from_text(error.name, diagnostic(&text, config));
        safe.code = error.code;
        if let Some(cause) = &error.cause {
            safe.cause = Some(Box::new(if depth < 32 {
                redact(cause, config, depth + 1)
            } else {
                Error::new(
                    "PrivateDiagnostic",
                    "Further private error causes are omitted",
                )
            }));
        }
        safe
    }
    redact(error, config, 0)
}

fn walk(
    value: &Value,
    config: &Config,
    history: bool,
    keep_home: bool,
    depth: usize,
) -> Result<Value, Error> {
    if depth > 128 {
        return Err(Error::type_error("Redaction nesting exceeds 128 levels"));
    }
    Ok(match value {
        Value::String(text) => {
            let mut text = if history {
                configured_string(text, config)
            } else {
                text.clone()
            };
            if history && !keep_home {
                if let Some(home) = config.home.as_ref().filter(|home| !home.0.is_empty()) {
                    if text.0 == home.0
                        || (text.0.starts_with(&home.0)
                            && text
                                .0
                                .get(home.0.len())
                                .is_some_and(|unit| matches!(*unit, 47 | 92)))
                    {
                        let mut masked = Text::from("~");
                        masked
                            .0
                            .extend_from_slice(text.0.get(home.0.len()..).unwrap_or(&[]));
                        text = masked;
                    }
                }
            }
            Value::String(evidence(&text, config))
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| walk(item, config, history, keep_home, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(name, value)| {
                    Ok((
                        name.clone(),
                        walk(value, config, history, keep_home, depth + 1)?,
                    ))
                })
                .collect::<Result<_, Error>>()?,
        ),
        _ => value.clone(),
    })
}

pub fn record(value: &Value, config: &Config) -> Result<Value, Error> {
    config.validate()?;
    walk(value, config, false, false, 0)
}

pub fn history_record(value: &Value, config: &Config) -> Result<Value, Error> {
    config.validate()?;
    if !matches!(value, Value::Object(_) | Value::Array(_)) {
        return Ok(value.clone());
    }
    let keep_home = config.keep_home || value.get("keepHome").is_some_and(domain::truthy);
    walk(value, config, true, keep_home, 0)
}

pub fn secret_material_lines(text: &Text, config: &Config) -> Vec<usize> {
    let mut offsets = BTreeSet::new();
    for value in config.secret_values() {
        let mut cursor = 0;
        while cursor + value.0.len() <= text.0.len() {
            if text.0.get(cursor..cursor + value.0.len()) == Some(value.0.as_slice()) {
                offsets.insert(cursor);
                cursor += value.0.len();
            } else {
                cursor += 1;
            }
        }
    }
    for scan_pem in [false, true] {
        let mut cursor = 0;
        while cursor < text.0.len() {
            if let Some(end) = if scan_pem {
                pem_end(&text.0, cursor, true)
            } else {
                credential_end(&text.0, cursor)
            } {
                offsets.insert(cursor);
                cursor = end;
            } else {
                cursor += 1;
            }
        }
    }
    offsets
        .into_iter()
        .map(|offset| {
            1 + text
                .0
                .get(..offset)
                .unwrap_or(&[])
                .iter()
                .filter(|unit| **unit == 10)
                .count()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_redaction_retains_unknown_and_lone_surrogates() -> Result<(), Error> {
        let input = crate::json::parse(&Text::from(
            r#"{"note":"api_key='test-only-private-api-key'","upper":"API_KEY=\"escaped\\\"secret_value_1234\"","surrogate":"\ud800","metric":null}"#,
        ))?;
        let output = record(&input, &Config::default())?;
        assert!(!output.json().contains("test-only-private-api-key"));
        assert!(!output.json().contains("secret_value_1234"));
        assert_eq!(output.get("surrogate"), input.get("surrogate"));
        assert_eq!(output.get("metric"), Some(&Value::Null));
        Ok(())
    }

    #[test]
    fn key_headers_without_bodies_do_not_destroy_reviewed_prose() {
        let source =
            Text::from("const header = '-----BEGIN TEST PRIVATE KEY-----';\nconst tail = 1;");
        assert_eq!(evidence(&source, &Config::default()), source);
        let block = Text::from(
            format!(
                "before -----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY----- after",
                "A".repeat(64)
            )
            .as_str(),
        );
        assert_eq!(
            evidence(&block, &Config::default()),
            Text::from("before [REDACTED:PRIVATE_KEY] after")
        );
    }

    #[test]
    fn a_long_nonmatching_identifier_does_not_require_rescanning_every_suffix() {
        let mut source = Text::from("plain");
        source.0.extend(std::iter::repeat_n(120, 128 * 1024));
        assert_eq!(evidence(&source, &Config::default()), source);
    }
}
