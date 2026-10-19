use std::collections::BTreeSet;

use super::*;
use crate::llm::openai::Completion;

const KNOWN: &[&str] = &[
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
    "git_diff",
    "update_checklist",
];
const DISCOVERY: &[&str] = &["list_dir", "glob_files", "search_text"];

#[derive(Debug, Clone)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone)]
pub struct Decoded {
    pub calls: Vec<Call>,
    pub correction: Option<Text>,
    pub discarded_placeholders: usize,
}

#[derive(Default)]
pub struct Decoder {
    ids: BTreeSet<String>,
    malformed_corrected: bool,
    unknown_corrected: bool,
    needs_tools: bool,
    turn: u64,
}

fn malformed(message: &str) -> Error {
    Error::new("MalformedCoderTools", message)
}

impl Decoder {
    pub fn decode(
        &mut self,
        completion: &Completion,
        offered: &[String],
    ) -> Result<Decoded, Error> {
        if domain::members(&completion.message).is_none()
            || completion.finish_reason.as_ref().is_some_and(|reason| {
                *reason != Value::Null
                    && !["stop", "tool_calls"]
                        .iter()
                        .any(|accepted| domain::is_text(Some(reason), accepted))
            })
        {
            return Err(Error::new(
                "Error",
                "LLM coder returned an unsupported chat response",
            ));
        }
        self.turn = self
            .turn
            .checked_add(1)
            .filter(|turn| *turn <= crate::MAX_SAFE_INTEGER)
            .ok_or_else(|| Error::type_error("Coder response count exceeds safe bounds"))?;
        let decoded = self
            .decode_message(&completion.message, offered)
            .and_then(|decoded| {
                if decoded.calls.is_empty()
                    && (self.needs_tools
                        || domain::is_text(completion.finish_reason.as_ref(), "tool_calls"))
                {
                    Err(malformed("Coder repair did not emit tool calls"))
                } else {
                    Ok(decoded)
                }
            });
        match decoded {
            Ok(decoded) => {
                if self.ids.len() + decoded.calls.len() > 32_768 {
                    return Err(malformed(
                        "Coder execution ID history exceeds its native bound",
                    ));
                }
                self.needs_tools = false;
                for call in &decoded.calls {
                    self.ids.insert(call.id.clone());
                }
                Ok(decoded)
            }
            Err(error) if error.name == "UnknownCoderTool" && !self.unknown_corrected => {
                self.unknown_corrected = true;
                self.needs_tools = true;
                Ok(Decoded { calls: Vec::new(), correction: Some(Text::from(format!("Tool {} does not exist. Offered tools: {}. Continue with one offered tool call, or summarize the blocker.", error.message, offered.join(", ")).as_str())), discarded_placeholders: 0 })
            }
            Err(error)
                if ["MalformedCoderTools", "MissingCoderToolName"].contains(&error.name)
                    && !self.malformed_corrected =>
            {
                self.malformed_corrected = true;
                self.needs_tools = true;
                Ok(Decoded {
                    calls: Vec::new(),
                    correction: Some(Text::from(
                        "Emit only tool_calls with JSON-string arguments for the offered tools.",
                    )),
                    discarded_placeholders: 0,
                })
            }
            Err(error) => Err(error),
        }
    }

    fn decode_message(&self, message: &Value, offered: &[String]) -> Result<Decoded, Error> {
        let Some(raw) = message.get("tool_calls") else {
            return Ok(Decoded {
                calls: Vec::new(),
                correction: None,
                discarded_placeholders: 0,
            });
        };
        let raw = domain::items(raw)
            .ok_or_else(|| malformed("LLM coder returned malformed tool calls"))?;
        if raw.len() > 8 || raw.iter().map(|call| call.json().len()).sum::<usize>() > 131_072 {
            return Err(malformed("Coder tool batch exceeds eight calls or 128 KiB"));
        }
        let has_real = raw.iter().any(|call| {
            domain::is_text(call.get("type"), "function")
                && call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_string)
                    .and_then(|name| name.to_utf8().ok())
                    .is_some_and(|name| offered.contains(&name))
                && call
                    .get("function")
                    .and_then(|function| function.get("arguments"))
                    .and_then(Value::as_string)
                    .is_some_and(|args| !crate::text::trim(&args.0).is_empty())
        });
        let mut ids = self.ids.clone();
        let mut calls = Vec::new();
        let mut discarded = 0;
        for (index, call) in raw.iter().enumerate() {
            let function = call.get("function").unwrap_or(&Value::Null);
            if has_real
                && domain::is_text(call.get("type"), "function")
                && domain::is_text(function.get("name"), "")
                && domain::is_text(function.get("arguments"), "")
            {
                discarded += 1;
                continue;
            }
            let name = function
                .get("name")
                .and_then(Value::as_string)
                .filter(|name| !crate::text::trim(&name.0).is_empty())
                .ok_or_else(|| {
                    Error::new(
                        "MissingCoderToolName",
                        "Coder tool call requires a nonempty function name",
                    )
                })?
                .to_utf8()?;
            let safe_name = name
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || "_.-".contains(character) {
                        character
                    } else {
                        '?'
                    }
                })
                .take(64)
                .collect::<String>();
            if domain::is_text(call.get("type"), "function")
                && !offered.contains(&name)
                && (!KNOWN.contains(&name.as_str()) || DISCOVERY.contains(&name.as_str()))
                && !offered.is_empty()
            {
                return Err(Error::new("UnknownCoderTool", safe_name));
            }
            if !domain::is_text(call.get("type"), "function")
                || !offered.contains(&name)
                || call
                    .get("id")
                    .is_some_and(|id| id.as_string().is_none_or(|id| id.0.is_empty()))
            {
                return Err(denied(format!(
                    "LLM coder requested an invalid or unavailable tool: {safe_name}"
                )));
            }
            let source = function
                .get("arguments")
                .and_then(Value::as_string)
                .ok_or_else(|| malformed("Coder tool arguments must be a JSON string"))?;
            let args = crate::json::parse(source)
                .map_err(|_| malformed("Coder tool arguments are not valid JSON"))?;
            if domain::members(&args).is_none() {
                return Err(malformed("Coder tool arguments must be an object"));
            }
            let requested_id = call
                .get("id")
                .and_then(Value::as_string)
                .map(Text::to_utf8)
                .transpose()?;
            let mut id = requested_id
                .filter(|id| !ids.contains(id))
                .unwrap_or_else(|| format!("coder-{}-{index}", self.turn));
            let base = id.clone();
            let mut collision = 0;
            while ids.contains(&id) {
                collision += 1;
                id = format!("{base}-{collision}");
                if collision > 64 {
                    return Err(malformed("Coder tool call ID collision limit reached"));
                }
            }
            if id.encode_utf16().count() > 128
                || id
                    .chars()
                    .any(|character| character <= '\u{1f}' || character == '\u{7f}')
            {
                return Err(malformed("Coder tool call ID exceeds native bounds"));
            }
            ids.insert(id.clone());
            calls.push(Call { id, name, args });
        }
        Ok(Decoded {
            calls,
            correction: None,
            discarded_placeholders: discarded,
        })
    }
}
