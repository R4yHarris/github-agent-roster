pub mod artifacts;
pub mod contract;
pub mod critic;
pub mod estimate;
pub mod feedback;
pub mod grounding;
pub mod intake;
pub mod lifecycle;
pub mod memory;
pub mod outline;
pub mod provenance;
pub mod receipt;
pub mod skills;
pub mod task;
pub mod tools;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod lossless_tests;

#[cfg(test)]
mod review_tests;

#[cfg(test)]
mod chat_tests;

use regex_lite::Regex;

use crate::Error;
use crate::domain;
use crate::json::Value;
use crate::text::{Text, is_space};

pub const MAX_ARTIFACT_BYTES: usize = 65_536;
pub const MAX_ASK_BYTES: usize = 16_384;
pub const PLANNER_ARTIFACTS: &[&str] = &["RECIPE.yml", "TASK.md", "ESTIMATE.md"];
pub const PLAN_ARTIFACTS: &[&str] = &["PLAN.md"];

fn encode(text: &Text) -> String {
    text.0
        .iter()
        .filter_map(|unit| {
            char::from_u32(if (0xd800..=0xdfff).contains(unit) {
                0x10000 + u32::from(*unit - 0xd800)
            } else {
                u32::from(*unit)
            })
        })
        .collect()
}

fn decode(text: &str) -> Text {
    Text(
        text.chars()
            .map(|character| {
                let code = character as u32;
                if (0x10000..=0x107ff).contains(&code) {
                    (code - 0x10000) as u16 + 0xd800
                } else {
                    code as u16
                }
            })
            .collect(),
    )
}

fn value(text: &str) -> Value {
    Value::String(decode(text))
}

fn text_error(name: &'static str, message: &str) -> Error {
    Error::from_text(name, decode(message))
}

fn error_text(error: &Error) -> String {
    error
        .message_text
        .as_ref()
        .map_or_else(|| error.message.clone(), encode)
}

fn text(input: &Value, message: &str) -> Result<String, Error> {
    input
        .as_string()
        .map(encode)
        .ok_or_else(|| Error::type_error(message))
}

fn required<'a>(input: &'a Value, field: &str) -> Result<&'a Value, Error> {
    input
        .get(field)
        .ok_or_else(|| Error::type_error(format!("Planning input requires {field}")))
}

fn array<'a>(input: &'a Value, message: &str) -> Result<&'a [Value], Error> {
    domain::items(input).ok_or_else(|| Error::type_error(message))
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| value(item)).collect())
}

fn regex(pattern: &str) -> Result<Regex, Error> {
    let space = r"\x09-\x0d \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";
    let mut expanded = String::new();
    let mut characters = pattern.chars();
    let mut class = false;
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some('s') if class => expanded.push_str(space),
                Some('s') => expanded.push_str(&format!("[{space}]")),
                Some('S') if !class => expanded.push_str(&format!("[^{space}]")),
                Some(other) => {
                    expanded.push('\\');
                    expanded.push(other);
                }
                None => expanded.push('\\'),
            }
        } else {
            if character == '[' {
                class = true;
            } else if character == ']' {
                class = false;
            }
            expanded.push(character);
        }
    }
    Regex::new(&expanded)
        .map_err(|error| Error::new("Error", format!("Invalid native planning grammar: {error}")))
}

fn matches(pattern: &str, source: &str) -> Result<bool, Error> {
    Ok(regex(pattern)?.is_match(source))
}

fn trim(source: &str) -> &str {
    source.trim_matches(unit_space)
}

fn trim_end(source: &str) -> &str {
    source.trim_end_matches(unit_space)
}

fn unit_space(character: char) -> bool {
    u16::try_from(character as u32).is_ok_and(is_space)
}

fn slice(source: &str, maximum: usize) -> String {
    source.chars().take(maximum).collect()
}

fn distinct(items: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for item in items {
        if !result.contains(&item) {
            result.push(item);
        }
    }
    result
}

fn fields(
    input: &Value,
    mandatory: &[&str],
    optional: &[&str],
    message: &str,
) -> Result<(), Error> {
    let Some(entries) = domain::members(input) else {
        return Err(Error::type_error(message));
    };
    if mandatory.iter().any(|name| input.get(name).is_none())
        || entries.iter().any(|(name, _)| {
            let name = encode(name);
            !mandatory.contains(&name.as_str()) && !optional.contains(&name.as_str())
        })
    {
        return Err(Error::type_error(message));
    }
    Ok(())
}

pub fn forbidden_read(file: &str) -> bool {
    let normalized = file.replace('\\', "/").to_lowercase();
    let parts = normalized.split('/').collect::<Vec<_>>();
    let ambiguous = parts.iter().any(|part| {
        *part != "."
            && *part != ".."
            && (part.chars().any(|c| c == ':' || c <= '\u{1f}') || part.ends_with(['.', ' ']))
    });
    ambiguous
        || parts.iter().any(|part| {
            *part == ".env"
                || part.starts_with(".env.")
                || part.ends_with(".env")
                || part.ends_with(".pem")
                || *part == ".git"
                || *part == "agent-policy.yml"
        })
        || parts.first() == Some(&"vendor")
        || parts.windows(2).any(|pair| {
            matches!(
                pair,
                [".roster", "vault" | "hooks" | "hooks.yml"]
                    | [".github", "workflows"]
                    | ["vendor", "github-agent-contracts"]
            )
        })
        || (parts.first() == Some(&".roster")
            && (matches!(parts.get(1), Some(&"logs" | &"history" | &"checkpoints"))
                || parts
                    .get(1)
                    .is_some_and(|part| part.starts_with("history."))
                || normalized == ".roster/map.md"))
}

pub fn forbidden_write(file: &str) -> bool {
    let normalized = file.replace('\\', "/").to_lowercase();
    let parts = normalized.split('/').collect::<Vec<_>>();
    forbidden_read(file)
        || parts.len() == 1
            && [
                "assignment.md",
                "task.md",
                "recipe.yml",
                "plan.md",
                "context.md",
                "research.md",
                "result.md",
                "review.md",
                "estimate.md",
            ]
            .contains(&normalized.as_str())
        || parts.len() == 3
            && parts.first() == Some(&".roster")
            && parts.get(1) == Some(&"runs")
            && parts.last().is_some_and(|part| part.ends_with(".log"))
        || parts
            .windows(2)
            .any(|pair| matches!(pair, [".roster", "evals.jsonl" | "memory"]))
}

pub fn is_allowed(file: &str, patterns: &[String]) -> bool {
    let normalized = file.replace('\\', "/");
    let candidate = if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized.clone()
    };
    !forbidden_write(&normalized)
        && patterns.iter().any(|pattern| {
            let pattern = if cfg!(windows) {
                pattern.to_lowercase()
            } else {
                pattern.clone()
            };
            pattern == "**/*"
                || candidate == pattern
                || pattern
                    .strip_suffix("/**")
                    .is_some_and(|prefix| candidate.starts_with(&format!("{prefix}/")))
        })
}
