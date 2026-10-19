use crate::text::{Text, ascii, is_space, parse_json_string, trim, trim_end};
use crate::{Error, MAX_SAFE_INTEGER};

pub const MAX_CATALOG_BYTES: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scalar {
    String(Text),
    Integer(u64),
    List(Vec<Text>),
}

impl Scalar {
    pub fn json(&self) -> String {
        match self {
            Self::String(value) => value.json(),
            Self::Integer(value) => value.to_string(),
            Self::List(values) => format!(
                "[{}]",
                values.iter().map(Text::json).collect::<Vec<_>>().join(",")
            ),
        }
    }
}

pub type Entry = Vec<(String, Scalar)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub entries: Vec<Entry>,
}

impl Catalog {
    pub fn json(&self) -> String {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let fields = entry
                    .iter()
                    .map(|(name, value)| {
                        format!("{}:{}", Text::from(name.as_str()).json(), value.json())
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{{fields}}}")
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("[{entries}]")
    }
}

fn line_error(line: usize, reason: &str) -> Error {
    Error::type_error(format!("Catalog line {line}: {reason}"))
}

fn uncomment(units: &[u16]) -> &[u16] {
    let mut quoted = false;
    let mut escaped = false;
    for (index, &unit) in units.iter().enumerate() {
        if quoted && unit == 92 && !escaped {
            escaped = true;
            continue;
        }
        if unit == 34 && !escaped {
            quoted = !quoted;
        }
        if !quoted && unit == 35 && (index == 0 || is_space(units[index - 1])) {
            return trim_end(&units[..index]);
        }
        escaped = false;
    }
    trim_end(units)
}

fn scalar(units: &[u16], line: usize) -> Result<Scalar, Error> {
    if units.first() == Some(&34) {
        return parse_json_string(units)
            .map(Scalar::String)
            .map_err(|_| line_error(line, "invalid double-quoted string"));
    }
    let integer = ascii(units, "0")
        || (units.first().is_some_and(|unit| (49..=57).contains(unit))
            && units.iter().all(|unit| (48..=57).contains(unit)));
    if integer {
        let mut value = 0_u64;
        for &unit in units {
            value = value
                .checked_mul(10)
                .and_then(|value| value.checked_add((unit - 48) as u64))
                .filter(|value| *value <= MAX_SAFE_INTEGER)
                .ok_or_else(|| line_error(line, "integer exceeds safe range"))?;
        }
        return Ok(Scalar::Integer(value));
    }
    if units.first() == Some(&91) && units.last() == Some(&93) {
        let body = &units[1..units.len() - 1];
        if body
            .iter()
            .all(|unit| matches!(unit, 32 | 44 | 45 | 48..=57 | 65..=90 | 95 | 97..=122))
        {
            let values = if trim(body).is_empty() {
                Vec::new()
            } else {
                body.split(|unit| *unit == 44)
                    .map(|item| Text(trim(item).to_vec()))
                    .collect()
            };
            return Ok(Scalar::List(values));
        }
    }
    if units.is_empty()
        || units.iter().any(|unit| {
            matches!(
                unit,
                0..=31 | 34 | 38 | 39 | 42 | 33 | 91 | 93 | 123 | 125 | 124 | 127
            )
        })
        || units
            .windows(2)
            .any(|pair| pair[0] == 58 && is_space(pair[1]))
    {
        return Err(line_error(
            line,
            "unsupported scalar; use a double-quoted string",
        ));
    }
    Ok(Scalar::String(Text(units.to_vec())))
}

fn field(units: &[u16]) -> Option<(bool, String, &[u16])> {
    let item = units.starts_with(&[32, 32, 45, 32]);
    if !item && !units.starts_with(&[32, 32, 32, 32]) {
        return None;
    }
    let body = &units[4..];
    let key_length = body
        .iter()
        .take_while(|unit| matches!(unit, 95 | 97..=122))
        .count();
    if key_length == 0 || body.get(key_length..key_length + 2) != Some(&[58, 32]) {
        return None;
    }
    let value = &body[key_length + 2..];
    if value.is_empty()
        || value
            .iter()
            .any(|unit| matches!(unit, 10 | 13 | 0x2028 | 0x2029))
    {
        return None;
    }
    let key = body[..key_length]
        .iter()
        .map(|unit| *unit as u8 as char)
        .collect();
    Some((item, key, value))
}

pub fn parse(source: &Text, root: &str, fields: &[&str]) -> Result<Catalog, Error> {
    if source.utf8_len() > MAX_CATALOG_BYTES {
        return Err(Error::type_error(
            "Catalog must be UTF-8 text of at most 64 KiB",
        ));
    }
    let normalized = source.normalized_lines();
    let root_line = Text::from(format!("{root}:").as_str());
    let empty_line = Text::from(format!("{root}: []").as_str());
    let mut entries: Vec<Entry> = Vec::new();
    let mut found_root = false;
    let mut empty = false;
    for (index, original) in normalized.0.split(|unit| *unit == 10).enumerate() {
        let line_number = index + 1;
        if original
            .iter()
            .any(|unit| matches!(unit, 0..=9 | 11..=31 | 127))
        {
            return Err(line_error(
                line_number,
                "control characters are not supported",
            ));
        }
        let line = uncomment(original);
        if line.is_empty() {
            continue;
        }
        if line == root_line.0 || line == empty_line.0 {
            if found_root {
                return Err(line_error(line_number, "duplicate root"));
            }
            found_root = true;
            empty = line == empty_line.0;
            continue;
        }
        let Some((item, key, value)) = field(line) else {
            return Err(line_error(line_number, "unsupported catalog YAML"));
        };
        if !found_root || empty || !fields.contains(&key.as_str()) || (!item && entries.is_empty())
        {
            return Err(line_error(line_number, "unsupported catalog YAML"));
        }
        if item {
            entries.push(Vec::new());
        }
        let entry = entries
            .last_mut()
            .ok_or_else(|| line_error(line_number, "unsupported catalog YAML"))?;
        if entry.iter().any(|(name, _)| name == &key) {
            return Err(line_error(line_number, "duplicate field"));
        }
        let value = scalar(value, line_number)?;
        if key != "__proto__" {
            entry.push((key, value));
        }
    }
    if !found_root || (!empty && entries.is_empty()) {
        return Err(Error::type_error(format!("Catalog needs a {root} list")));
    }
    Ok(Catalog { entries })
}

pub fn format(root: &str, catalog: &Catalog) -> Text {
    if catalog.entries.is_empty() {
        return Text::from(format!("{root}: []\n").as_str());
    }
    let mut output = Text::from(format!("{root}:\n").as_str());
    for entry in &catalog.entries {
        for (index, (key, value)) in entry.iter().enumerate() {
            output.push_str(if index == 0 { "  - " } else { "    " });
            output.push_str(key);
            output.push_str(": ");
            match value {
                Scalar::String(value) => output.push_str(&value.json()),
                Scalar::Integer(value) => output.push_str(&value.to_string()),
                Scalar::List(values) => {
                    output.push_str("[");
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 {
                            output.push_str(", ");
                        }
                        output.0.extend_from_slice(&value.0);
                    }
                    output.push_str("]");
                }
            }
            output.push_str("\n");
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_without_general_yaml_semantics() -> Result<(), Error> {
        let source = Text::from(
            "profiles:\r\n  - id: example # note\r\n    count: 9007199254740991\r\n    tasks: [fix, docs]\r\n",
        );
        let parsed = parse(&source, "profiles", &["id", "count", "tasks"])?;
        assert_eq!(
            parsed.json(),
            "[{\"id\":\"example\",\"count\":9007199254740991,\"tasks\":[\"fix\",\"docs\"]}]"
        );
        assert_eq!(
            format("profiles", &parsed).to_utf8()?,
            "profiles:\n  - id: \"example\"\n    count: 9007199254740991\n    tasks: [fix, docs]\n"
        );
        Ok(())
    }

    #[test]
    fn refuses_duplicates_overflow_and_oversized_multibyte_input() {
        for (source, reason) in [
            ("p:\n  - id: a\n    id: b", "duplicate field"),
            ("p:\np: []", "duplicate root"),
            ("p:\n  - id: 9007199254740992", "integer exceeds safe range"),
            ("p:\n  - id: &alias", "unsupported scalar"),
            ("p:\n  - id: a\rb", "control characters"),
            ("p:\n  - id: a\u{2028}b", "unsupported catalog YAML"),
        ] {
            let error = parse(&Text::from(source), "p", &["id"]);
            assert!(error.is_err_and(|error| error.message.contains(reason)));
        }
        let oversized = Text::from(format!("p:\n  - id: {}", "é".repeat(32_768)).as_str());
        assert!(parse(&oversized, "p", &["id"]).is_err());
    }
}
