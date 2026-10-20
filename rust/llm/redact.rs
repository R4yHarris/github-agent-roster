use crate::domain;
use crate::json::Value;
use crate::text::{Text, ascii, is_space};

fn starts(units: &[u16], index: usize, text: &str, insensitive: bool) -> bool {
    let Some(slice) = units.get(index..index.saturating_add(text.len())) else {
        return false;
    };
    slice.iter().zip(text.bytes()).all(|(&unit, byte)| {
        if insensitive && unit <= 127 {
            (unit as u8).eq_ignore_ascii_case(&byte)
        } else {
            unit == byte as u16
        }
    })
}

fn word(unit: u16) -> bool {
    unit <= 127 && ((unit as u8).is_ascii_alphanumeric() || unit == 95)
}

fn boundary(units: &[u16], index: usize) -> bool {
    units.get(index).is_some_and(|unit| word(*unit))
        != index
            .checked_sub(1)
            .and_then(|index| units.get(index))
            .is_some_and(|unit| word(*unit))
}

fn replace(text: &Text, needle: &Text) -> Text {
    if needle.0.is_empty() {
        return text.clone();
    }
    let mut result = Text::default();
    let mut position = 0;
    while position < text.0.len() {
        if text
            .0
            .get(position..position.saturating_add(needle.0.len()))
            == Some(needle.0.as_slice())
        {
            result.push_str("[redacted]");
            position += needle.0.len();
        } else if let Some(&unit) = text.0.get(position) {
            result.0.push(unit);
            position += 1;
        }
    }
    result
}

fn value_end(units: &[u16], start: usize) -> Option<usize> {
    let first = *units.get(start)?;
    if matches!(first, 34 | 39) {
        let mut index = start + 1;
        while let Some(&unit) = units.get(index) {
            if unit == 92 {
                index += 2;
                continue;
            }
            if unit == first {
                return Some(index + 1);
            }
            index += 1;
        }
    }
    let end = units
        .get(start..)?
        .iter()
        .position(|unit| is_space(*unit) || matches!(*unit, 44 | 59))
        .map_or(units.len(), |relative| start + relative);
    (end > start).then_some(end)
}

pub struct Redactor {
    secrets: Vec<Text>,
}

impl Redactor {
    pub fn new(environment: &Value, api_key_name: Option<&Text>) -> Self {
        let mut secrets = domain::members(environment)
            .unwrap_or(&[])
            .iter()
            .filter_map(|(name, value)| {
                let value = value.as_string().filter(|value| !value.0.is_empty())?;
                let upper = String::from_utf16_lossy(&name.0).to_ascii_uppercase();
                (Some(name) == api_key_name
                    || ["TOKEN", "PASSWORD", "SECRET", "PRIVATE_KEY", "API_KEY"]
                        .iter()
                        .any(|part| upper.contains(part)))
                .then(|| value.clone())
            })
            .collect::<Vec<_>>();
        secrets.sort_by_key(|value| std::cmp::Reverse(value.0.len()));
        Self { secrets }
    }

    pub fn redact(&self, text: &Text, key: Option<&Text>) -> Text {
        let mut text = key.map_or_else(|| text.clone(), |key| replace(text, key));
        for secret in &self.secrets {
            text = replace(&text, secret);
        }
        let mut index = 0;
        while index < text.0.len() {
            if starts(&text.0, index, "-----BEGIN ", false) {
                let tail = text.0.get(index + 11..).unwrap_or(&[]);
                if let Some(end) = tail.iter().position(|unit| matches!(*unit, 10 | 13 | 45)) {
                    let candidate = tail.get(..end).unwrap_or(&[]);
                    if candidate.ends_with(&Text::from("PRIVATE KEY").0)
                        && starts(&text.0, index + 11 + end, "-----", false)
                    {
                        text.0.truncate(index);
                        text.push_str("[redacted]");
                        break;
                    }
                }
            }
            index += 1;
        }
        let mut result = Text::default();
        let mut index = 0;
        while index < text.0.len() {
            let mut token_end = None;
            if boundary(&text.0, index) {
                for (prefix, extra) in [
                    ("ghp_", false),
                    ("gho_", false),
                    ("ghu_", false),
                    ("ghs_", false),
                    ("ghr_", false),
                    ("github_pat_", true),
                    ("sk-", true),
                ] {
                    if !starts(&text.0, index, prefix, false) {
                        continue;
                    }
                    let start = index + prefix.len();
                    let mut end = start;
                    while text.0.get(end).is_some_and(|unit| {
                        *unit <= 127
                            && ((*unit as u8).is_ascii_alphanumeric()
                                || extra && matches!(*unit, 45 | 95))
                    }) {
                        end += 1;
                    }
                    while end >= start + 20 && !boundary(&text.0, end) {
                        end -= 1;
                    }
                    if end >= start + 20 && boundary(&text.0, end) {
                        token_end = Some(end);
                        break;
                    }
                }
            }
            if let Some(end) = token_end {
                result.push_str("[redacted]");
                index = end;
                continue;
            }
            if boundary(&text.0, index) {
                let scheme = if starts(&text.0, index, "https://", true) {
                    Some(8)
                } else if starts(&text.0, index, "http://", true) {
                    Some(7)
                } else {
                    None
                };
                if let Some(length) = scheme {
                    let start = index + length;
                    let mut end = start;
                    let mut at = None;
                    while let Some(&unit) = text.0.get(end) {
                        if unit == 47 || is_space(unit) {
                            break;
                        }
                        if unit == 64 && end > start {
                            at = Some(end);
                        }
                        end += 1;
                    }
                    if let Some(at) = at {
                        result
                            .0
                            .extend_from_slice(text.0.get(index..start).unwrap_or(&[]));
                        result.push_str("[redacted]@");
                        index = at + 1;
                        continue;
                    }
                }
                let mut prefix_end = index;
                if starts(&text.0, index, "Bearer", true)
                    && text.0.get(index + 6).is_some_and(|unit| is_space(*unit))
                {
                    prefix_end += 6;
                    while text.0.get(prefix_end).is_some_and(|unit| is_space(*unit)) {
                        prefix_end += 1;
                    }
                } else {
                    let mut end = index;
                    while text.0.get(end).is_some_and(|unit| {
                        *unit <= 127
                            && ((*unit as u8).is_ascii_alphanumeric() || matches!(*unit, 45 | 95))
                    }) {
                        end += 1;
                    }
                    let name = String::from_utf16_lossy(text.0.get(index..end).unwrap_or(&[]))
                        .to_ascii_lowercase();
                    if [
                        "apikey",
                        "api_key",
                        "api-key",
                        "password",
                        "secret",
                        "token",
                        "authorization",
                    ]
                    .iter()
                    .any(|keyword| name.contains(keyword))
                    {
                        if text.0.get(end).is_some_and(|unit| matches!(*unit, 34 | 39)) {
                            end += 1;
                        }
                        while text.0.get(end).is_some_and(|unit| is_space(*unit)) {
                            end += 1;
                        }
                        if text.0.get(end).is_some_and(|unit| matches!(*unit, 58 | 61)) {
                            end += 1;
                            while text.0.get(end).is_some_and(|unit| is_space(*unit)) {
                                end += 1;
                            }
                            prefix_end = end;
                        }
                    }
                }
                if prefix_end > index {
                    if let Some(end) = value_end(&text.0, prefix_end) {
                        result
                            .0
                            .extend_from_slice(text.0.get(index..prefix_end).unwrap_or(&[]));
                        result.push_str("[redacted]");
                        index = end;
                        continue;
                    }
                }
            }
            if let Some(&unit) = text.0.get(index) {
                result.0.push(unit);
            }
            index += 1;
        }
        result
    }
}

pub(crate) fn valid_secret_identifier(name: &Text) -> bool {
    (1..=64).contains(&name.0.len())
        && name.0.first().is_some_and(|unit| {
            *unit <= 127 && ((*unit as u8).is_ascii_alphabetic() || *unit == 95)
        })
        && name.0.iter().all(|unit| word(*unit))
}

pub fn valid_secret_name(name: &Text) -> bool {
    valid_secret_identifier(name)
        && !ascii(
            &name
                .0
                .iter()
                .take(11)
                .map(|unit| {
                    if *unit <= 127 {
                        (*unit as u8).to_ascii_uppercase() as u16
                    } else {
                        *unit
                    }
                })
                .collect::<Vec<_>>(),
            "GITHUB_APP_",
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pem_redaction_precedes_overlapping_assignment_consumption() {
        let redactor = Redactor::new(&Value::Object(Vec::new()), None);
        for prefix in ["token=", "password: ", "Bearer ", "api_key="] {
            let source = Text::from(format!(
                "{prefix}-----BEGIN TEST PRIVATE KEY-----\nsynthetic noncredential overlap sentinel\n-----END TEST PRIVATE KEY-----"
            ).as_str());
            let safe = redactor.redact(&source, None);
            assert!(!String::from_utf16_lossy(&safe.0).contains("overlap sentinel"));
            assert_eq!(safe, Text::from(format!("{prefix}[redacted]").as_str()));
        }
    }
}
