use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use crate::text::{Text, is_space, trim, trim_end};
use crate::{Error, MAX_SAFE_INTEGER};

const INVALID: &str =
    "Invalid or unsupported recipe; expected the strict version 1 format in docs/SEATS.md.";
const MAX_BYTES: usize = 65_536;
const FIELDS: &[&str] = &[
    "id",
    "principal",
    "worker",
    "sequence",
    "skills",
    "tools",
    "max_difficulty",
];
const SKILLS: &[&str] = &[
    "research",
    "implement",
    "test",
    "review",
    "incident",
    "docs",
    "implement-task",
    "read-before-write",
    "result-report",
    "run-tests",
    "small-diff",
    "rust-implementation",
    "rust-cargo",
    "rust-tokio",
];
const TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "delete_file",
    "glob_files",
    "run_command",
    "list_dir",
    "run_test",
    "search_text",
    "web_search",
    "web_fetch",
];

fn invalid() -> Error {
    Error::new("Error", INVALID)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SeatId {
    Planner,
    Coder,
    Reviewer,
}

impl SeatId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
        }
    }

    pub fn sequence(self) -> &'static [&'static str] {
        match self {
            Self::Planner => &["read_ask", "plan", "write_task"],
            Self::Coder => &["load_context", "implement", "run_tests", "summarize"],
            Self::Reviewer => &["read_diff", "check_acceptance", "write_review"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Worker {
    Builtin,
    Copilot,
    Hermes,
}

impl Worker {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Copilot => "copilot",
            Self::Hermes => "hermes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seat {
    pub id: SeatId,
    pub worker: Worker,
    pub skills: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub max_difficulty: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub ask: String,
    pub seats: Vec<Seat>,
}

fn json_strings(values: &[impl AsRef<str>]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| Text::from(value.as_ref()).json())
            .collect::<Vec<_>>()
            .join(",")
    )
}

impl Recipe {
    pub fn json(&self) -> String {
        let seats = self
            .seats
            .iter()
            .map(|seat| {
                let principal = if seat.id == SeatId::Reviewer {
                    "reviewer"
                } else {
                    "coder"
                };
                let mut output = format!(
                    "{{\"id\":\"{}\",\"principal\":\"{principal}\",\"worker\":\"{}\"",
                    seat.id.as_str(),
                    seat.worker.as_str()
                );
                if let Some(skills) = &seat.skills {
                    output.push_str(&format!(",\"skills\":{}", json_strings(skills)));
                }
                if let Some(tools) = &seat.tools {
                    output.push_str(&format!(",\"tools\":{}", json_strings(tools)));
                }
                if let Some(difficulty) = seat.max_difficulty {
                    output.push_str(&format!(",\"max_difficulty\":{difficulty}"));
                }
                if seat.worker == Worker::Builtin {
                    output.push_str(&format!(
                        ",\"sequence\":{}",
                        json_strings(seat.id.sequence())
                    ));
                }
                output.push('}');
                output
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"version\":1,\"ask\":{},\"seats\":[{seats}]}}",
            Text::from(self.ask.as_str()).json()
        )
    }
}

fn capability_list(value: &str, known: &[&str]) -> Result<Vec<String>, Error> {
    let body = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .filter(|value| !value.contains(['[', ']']))
        .ok_or_else(invalid)?;
    let text = Text::from(body);
    let trimmed = trim(&text.0);
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let names = text
        .0
        .split(|unit| *unit == 44)
        .map(|units| Text(trim(units).to_vec()).to_utf8().map_err(|_| invalid()))
        .collect::<Result<Vec<_>, _>>()?;
    if names.iter().any(|name| !known.contains(&name.as_str()))
        || names.iter().collect::<BTreeSet<_>>().len() != names.len()
    {
        return Err(invalid());
    }
    Ok(names)
}

fn finish_seat(fields: &mut BTreeMap<String, String>, seats: &mut Vec<Seat>) -> Result<(), Error> {
    if fields.is_empty() {
        return Ok(());
    }
    let id = match fields.get("id").map(String::as_str) {
        Some("planner") => SeatId::Planner,
        Some("coder") => SeatId::Coder,
        Some("reviewer") => SeatId::Reviewer,
        _ => return Err(invalid()),
    };
    let worker = match fields.get("worker").map(String::as_str) {
        Some("builtin") => Worker::Builtin,
        Some("copilot") => Worker::Copilot,
        Some("hermes") => Worker::Hermes,
        _ => return Err(invalid()),
    };
    let principal = if id == SeatId::Reviewer {
        "reviewer"
    } else {
        "coder"
    };
    if fields.get("principal").map(String::as_str) != Some(principal)
        || id == SeatId::Reviewer && worker != Worker::Builtin
        || seats.iter().any(|seat| seat.id == id)
        || id == SeatId::Planner && !seats.is_empty()
        || id == SeatId::Coder && seats.iter().any(|seat| seat.id == SeatId::Reviewer)
        || id == SeatId::Reviewer && !seats.iter().any(|seat| seat.id == SeatId::Coder)
    {
        return Err(invalid());
    }
    let sequence = fields.get("sequence");
    if worker == Worker::Builtin {
        if sequence != Some(&format!("[{}]", id.sequence().join(", "))) {
            return Err(invalid());
        }
    } else if sequence.is_some() {
        return Err(invalid());
    }
    let skills = fields
        .get("skills")
        .map(|value| capability_list(value, SKILLS))
        .transpose()?;
    let tools = fields
        .get("tools")
        .map(|value| capability_list(value, TOOLS))
        .transpose()?;
    let max_difficulty = fields
        .get("max_difficulty")
        .map(|value| match value.as_bytes() {
            [digit @ b'1'..=b'5'] => Ok(digit - b'0'),
            _ => Err(invalid()),
        })
        .transpose()?;
    seats.push(Seat {
        id,
        worker,
        skills,
        tools,
        max_difficulty,
    });
    fields.clear();
    Ok(())
}

fn valid_ask(value: &str) -> bool {
    if let Some(number) = value.strip_prefix("issue:") {
        return number
            .as_bytes()
            .first()
            .is_some_and(|digit| matches!(digit, b'1'..=b'9'))
            && number.bytes().all(|digit| digit.is_ascii_digit())
            && number
                .parse::<u64>()
                .is_ok_and(|number| number <= MAX_SAFE_INTEGER);
    }
    value.strip_prefix("local:").is_some_and(|id| {
        (1..=64).contains(&id.len())
            && id
                .bytes()
                .all(|unit| unit.is_ascii_alphanumeric() || matches!(unit, b'_' | b'-'))
    })
}

pub fn parse(source: &Text) -> Result<Recipe, Error> {
    if source.utf8_len() > MAX_BYTES {
        return Err(invalid());
    }
    let normalized = source.normalized_lines();
    let mut roots = BTreeSet::new();
    let mut seats = Vec::new();
    let mut ask = None;
    let mut in_seats = false;
    let mut fields = BTreeMap::new();
    for original in normalized.0.split(|unit| *unit == 10) {
        if original.iter().any(|unit| matches!(unit, 0..=31 | 127)) {
            return Err(invalid());
        }
        let comment = original
            .iter()
            .enumerate()
            .find(|(index, unit)| {
                let suffix = &original[index + 1..];
                let has_terminator = suffix
                    .iter()
                    .position(|unit| matches!(unit, 0x2028 | 0x2029));
                **unit == 35
                    && (*index == 0 || is_space(original[index - 1]))
                    && has_terminator.is_none()
            })
            .map(|(index, _)| index)
            .unwrap_or(original.len());
        let line = trim_end(&original[..comment]);
        if line.is_empty() {
            continue;
        }
        if line.iter().any(|unit| matches!(unit, 0x2028 | 0x2029)) {
            return Err(invalid());
        }
        let line = String::from_utf16(line).map_err(|_| invalid())?;
        let root = line.split_once(':').filter(|(root, suffix)| {
            ["version", "ask", "seats"].contains(root)
                && (suffix.is_empty() || suffix.starts_with(' '))
        });
        if let Some((root, suffix)) = root {
            finish_seat(&mut fields, &mut seats)?;
            if !roots.insert(root.to_owned()) {
                return Err(invalid());
            }
            let value = suffix.strip_prefix(' ');
            in_seats = root == "seats";
            match root {
                "seats" if value.is_none() => {}
                "version" if value == Some("1") => {}
                "ask" if value.is_some_and(valid_ask) => ask = value.map(str::to_owned),
                _ => return Err(invalid()),
            }
            continue;
        }
        let item = line.starts_with("  - ");
        let body = if item {
            line.strip_prefix("  - ")
        } else {
            line.strip_prefix("    ")
        }
        .ok_or_else(invalid)?;
        let (key, value) = body.split_once(": ").ok_or_else(invalid)?;
        if !in_seats || !FIELDS.contains(&key) || value.is_empty() {
            return Err(invalid());
        }
        if item {
            finish_seat(&mut fields, &mut seats)?;
        } else if fields.is_empty() {
            return Err(invalid());
        }
        if fields.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(invalid());
        }
    }
    finish_seat(&mut fields, &mut seats)?;
    if roots.len() != 3 || !seats.iter().any(|seat| seat.id == SeatId::Coder) {
        return Err(invalid());
    }
    Ok(Recipe {
        ask: ask.ok_or_else(invalid)?,
        seats,
    })
}

pub fn validate_file(path: &Path) -> Result<Recipe, Error> {
    if path.as_os_str().is_empty() {
        return Err(Error::new(
            "Error",
            "Recipe path must be a nonempty string.",
        ));
    }
    let status =
        fs::symlink_metadata(path).map_err(|_| Error::new("Error", "Cannot read recipe file."))?;
    if !status.is_file() || status.file_type().is_symlink() || status.len() > MAX_BYTES as u64 {
        return Err(Error::new(
            "Error",
            "Recipe must be a regular file, not a symlink, and at most 64 KiB.",
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| Error::new("Error", "Cannot read recipe file."))?;
    let source =
        String::from_utf8(bytes).map_err(|_| Error::new("Error", "Recipe file must be UTF-8."))?;
    parse(&Text::from(source.as_str()))
}

pub fn assert_covers(
    seat: &Seat,
    difficulty: Option<u8>,
    skills: Option<&[String]>,
) -> Result<(), Error> {
    if let Some(maximum) = seat.max_difficulty {
        let difficulty = difficulty
            .filter(|difficulty| (1..=5).contains(difficulty))
            .ok_or_else(|| {
                Error::new(
                    "Error",
                    "Task difficulty must be an integer from 1 to 5 for a bounded seat.",
                )
            })?;
        if difficulty > maximum {
            return Err(Error::new(
                "Error",
                format!(
                    "Seat {} max_difficulty {maximum} cannot cover task difficulty {difficulty}.",
                    seat.id.as_str()
                ),
            ));
        }
    }
    if let Some(covered) = &seat.skills {
        let skills = skills.ok_or_else(|| {
            Error::new(
                "Error",
                "Task skills must be a list for a skill-bounded seat.",
            )
        })?;
        let missing = skills
            .iter()
            .filter(|skill| !covered.contains(skill))
            .map(String::as_str)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(Error::new(
                "Error",
                format!(
                    "Seat {} does not cover task skills: {}.",
                    seat.id.as_str(),
                    missing.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODER: &str = "version: 1\nask: issue:42\nseats:\n  - id: coder\n    principal: coder\n    worker: hermes\n";

    #[test]
    fn preserves_legacy_recipes_and_optional_capabilities() -> Result<(), Error> {
        let recipe = parse(&Text::from(CODER))?;
        assert_eq!(
            recipe.json(),
            "{\"version\":1,\"ask\":\"issue:42\",\"seats\":[{\"id\":\"coder\",\"principal\":\"coder\",\"worker\":\"hermes\"}]}"
        );
        assert_covers(&recipe.seats[0], None, None)?;
        let source = format!(
            "{CODER}    skills: [rust-implementation, rust-cargo]\n    max_difficulty: 3\n"
        );
        let bounded = parse(&Text::from(source.as_str()))?;
        assert!(assert_covers(&bounded.seats[0], Some(4), Some(&[])).is_err());
        assert!(assert_covers(&bounded.seats[0], Some(2), None).is_err());
        assert_covers(
            &bounded.seats[0],
            Some(3),
            Some(&["rust-implementation".to_owned()]),
        )?;
        Ok(())
    }

    #[test]
    fn refuses_permission_roles_duplicate_roots_and_invalid_references() {
        for source in [
            CODER.replace("principal: coder", "principal: merger"),
            CODER.replace("id: coder", "id: deployer"),
            CODER.replace("issue:42", "issue:9007199254740992"),
            CODER.replace("issue:42", "issue:01"),
            format!("{CODER}version: 1\n"),
            format!("{CODER}    tools: [shell]\n"),
            format!("{CODER}    skills: [rust-cargo, rust-cargo]\n"),
            format!("{CODER}    max_difficulty: 0\n"),
        ] {
            assert_eq!(parse(&Text::from(source.as_str())), Err(invalid()));
        }
    }

    #[test]
    fn ignores_surrogates_only_inside_comments() -> Result<(), Error> {
        let mut source = Text::from("# comment ");
        source.0.push(0xd800);
        source.push_str("\n");
        source.push_str(CODER);
        assert!(parse(&source).is_ok());
        assert_eq!(parse(&source)?.ask, "issue:42");
        Ok(())
    }
}
