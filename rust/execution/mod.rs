pub mod archive;
pub mod backend;
mod blob;
pub mod checklist;
pub mod checkpoints;
pub mod classification;
pub mod coder_calls;
pub mod context;
pub mod continuity;
pub mod controls;
pub mod counterfactual;
pub mod docker;
pub mod e2e;
pub mod enrichment;
pub mod evidence;
pub mod finalization;
pub mod hooks;
pub mod owned;
pub mod policy;
pub mod restore;
pub mod result;
mod scratch;
pub mod self_review;
pub mod shadow;
pub mod substance;
pub mod timing;
pub mod tools;
pub mod verification;

#[cfg(test)]
mod composed_tests;
#[cfg(test)]
mod tests;

use crate::Error;
use crate::domain;
use crate::json::Value;
use crate::text::Text;

pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_FILES: usize = 2048;
pub const MAX_SCOPE_ENTRIES: usize = 64;
pub const MAX_SNAPSHOT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Coder,
    Reviewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
        }
    }
}

pub(crate) fn denied(message: impl Into<String>) -> Error {
    Error::new("ToolAccessError", message).coded("ROSTER_TOOL_DENIED")
}

pub(crate) fn usage(message: impl Into<String>) -> Error {
    Error::new("ToolUsageError", message).coded("ROSTER_TOOL_DENIED")
}

pub fn unsupported(capability: &str) -> Error {
    Error::new(
        "NativeIntegrationError",
        format!("{capability} is not integrated into the native execution boundary; no passing seat verdict or publication is authorized."),
    )
    .coded("ROSTER_NATIVE_UNSUPPORTED")
}

pub fn require_complete_seat_gates() -> Result<(), Error> {
    Err(unsupported("Outer CLI/worktree/GitHub orchestration"))
}

pub(crate) fn js_space(character: char) -> bool {
    u16::try_from(character as u32).is_ok_and(crate::text::is_space)
}

pub(crate) fn js_trim(source: &str) -> &str {
    source.trim_matches(js_space)
}

pub(crate) fn required<'a>(input: &'a Value, name: &str) -> Result<&'a Value, Error> {
    input
        .get(name)
        .ok_or_else(|| Error::type_error(format!("Execution input requires {name}")))
}

pub(crate) fn text(input: &Value, message: &str) -> Result<String, Error> {
    input
        .as_string()
        .ok_or_else(|| Error::type_error(message))?
        .to_utf8()
}

pub(crate) fn strings(input: &[String]) -> Value {
    Value::Array(input.iter().map(|value| Value::string(value)).collect())
}

pub(crate) fn fields(
    input: &Value,
    mandatory: &[&str],
    optional: &[&str],
    message: &str,
) -> Result<(), Error> {
    let entries = domain::members(input).ok_or_else(|| Error::type_error(message))?;
    if mandatory.iter().any(|name| input.get(name).is_none())
        || entries.iter().any(|(name, _)| {
            name.to_utf8().map_or(true, |name| {
                !mandatory.contains(&name.as_str()) && !optional.contains(&name.as_str())
            })
        })
    {
        return Err(Error::type_error(message));
    }
    Ok(())
}

pub(crate) fn regex(pattern: &str) -> Result<regex_lite::Regex, Error> {
    regex_lite::Regex::new(pattern).map_err(|error| {
        Error::new(
            "Error",
            format!("Invalid native execution grammar: {error}"),
        )
    })
}

pub(crate) fn one_line(input: &Text, maximum: usize) -> Text {
    let mut result = Vec::new();
    let mut space = false;
    for unit in &input.0 {
        if crate::text::is_space(*unit) {
            space = !result.is_empty();
        } else {
            if space {
                result.push(32);
                space = false;
            }
            result.push(*unit);
        }
    }
    result.truncate(maximum);
    Text(result)
}
