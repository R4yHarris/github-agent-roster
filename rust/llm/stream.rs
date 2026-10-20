use std::collections::BTreeMap;

use crate::domain;
use crate::json::{self, Value};
use crate::text::{Text, ascii, trim};

use super::{Category, Failure, MAX_RESPONSE_BYTES};

#[derive(Default)]
struct ToolCall {
    id: Option<Text>,
    kind: Option<Text>,
    name: Text,
    arguments: Text,
}

pub struct CompletionAssembler {
    model: Text,
    role: Option<Text>,
    content: Text,
    finish_reason: Option<Text>,
    usage: Option<Value>,
    tool_calls: BTreeMap<u64, ToolCall>,
}

impl CompletionAssembler {
    pub fn new(requested_model: Text) -> Self {
        Self {
            model: requested_model,
            role: None,
            content: Text::default(),
            finish_reason: None,
            usage: None,
            tool_calls: BTreeMap::new(),
        }
    }

    pub fn push(&mut self, chunk: &Value) {
        if domain::members(chunk).is_none() {
            return;
        }
        if let Some(model) = chunk
            .get("model")
            .and_then(Value::as_string)
            .filter(|model| !trim(&model.0).is_empty())
        {
            self.model = model.clone();
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| **usage != Value::Null) {
            self.usage = Some(usage.clone());
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(domain::items)
            .and_then(|choices| choices.first())
            .filter(|choice| domain::members(choice).is_some())
        else {
            return;
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_string) {
            self.finish_reason = Some(reason.clone());
        }
        let Some(delta) = choice
            .get("delta")
            .filter(|delta| domain::members(delta).is_some())
        else {
            return;
        };
        if let Some(role) = delta
            .get("role")
            .and_then(Value::as_string)
            .filter(|role| !role.0.is_empty())
        {
            self.role = Some(role.clone());
        }
        if let Some(content) = delta.get("content").and_then(Value::as_string) {
            self.content.0.extend_from_slice(&content.0);
        }
        for call in delta
            .get("tool_calls")
            .and_then(domain::items)
            .unwrap_or(&[])
        {
            if domain::members(call).is_none() {
                continue;
            }
            let index =
                domain::safe_integer(call.get("index")).unwrap_or(self.tool_calls.len() as u64);
            let current = self.tool_calls.entry(index).or_default();
            if let Some(id) = call
                .get("id")
                .and_then(Value::as_string)
                .filter(|id| !id.0.is_empty())
            {
                current.id = Some(id.clone());
            }
            if let Some(kind) = call
                .get("type")
                .and_then(Value::as_string)
                .filter(|kind| !kind.0.is_empty())
            {
                current.kind = Some(kind.clone());
            }
            if let Some(function) = call.get("function") {
                for (field, output) in [
                    ("name", &mut current.name),
                    ("arguments", &mut current.arguments),
                ] {
                    if let Some(value) = function.get(field).and_then(Value::as_string) {
                        output.0.extend_from_slice(&value.0);
                    }
                }
            }
        }
    }

    pub fn payload(&self) -> Value {
        let mut message = vec![
            (
                "role",
                Value::String(self.role.clone().unwrap_or_else(|| Text::from("assistant"))),
            ),
            ("content", Value::String(self.content.clone())),
        ];
        if !self.tool_calls.is_empty() {
            message.push((
                "tool_calls",
                Value::Array(
                    self.tool_calls
                        .values()
                        .map(|call| {
                            let mut fields = vec![
                                (
                                    "type",
                                    Value::String(
                                        call.kind.clone().unwrap_or_else(|| Text::from("function")),
                                    ),
                                ),
                                (
                                    "function",
                                    Value::object(vec![
                                        ("name", Value::String(call.name.clone())),
                                        ("arguments", Value::String(call.arguments.clone())),
                                    ]),
                                ),
                            ];
                            if let Some(id) = &call.id {
                                fields.push(("id", Value::String(id.clone())));
                            }
                            Value::object(fields)
                        })
                        .collect(),
                ),
            ));
        }
        let mut choice = vec![("message", Value::object(message))];
        if let Some(reason) = &self.finish_reason {
            choice.push(("finish_reason", Value::String(reason.clone())));
        }
        Value::object(vec![
            ("model", Value::String(self.model.clone())),
            ("usage", self.usage.clone().unwrap_or(Value::Null)),
            ("choices", Value::Array(vec![Value::object(choice)])),
        ])
    }
}

#[derive(Default)]
struct Utf8Decoder {
    pending: Vec<u8>,
    started: bool,
}

impl Utf8Decoder {
    fn decode(&mut self, bytes: &[u8], final_chunk: bool) -> Text {
        self.pending.extend_from_slice(bytes);
        let mut decoded = String::new();
        let mut consumed = 0;
        while let Some(remaining) = self.pending.get(consumed..) {
            if remaining.is_empty() {
                break;
            }
            match std::str::from_utf8(remaining) {
                Ok(valid) => {
                    decoded.push_str(valid);
                    consumed = self.pending.len();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    if let Some(prefix) = remaining
                        .get(..valid)
                        .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    {
                        decoded.push_str(prefix);
                    }
                    consumed += valid;
                    if let Some(length) = error.error_len() {
                        decoded.push('\u{fffd}');
                        consumed += length;
                    } else if final_chunk {
                        decoded.push('\u{fffd}');
                        consumed = self.pending.len();
                        break;
                    } else {
                        break;
                    }
                }
            }
        }
        self.pending.drain(..consumed);
        if !self.started && !decoded.is_empty() {
            self.started = true;
            decoded = decoded
                .strip_prefix('\u{feff}')
                .unwrap_or(&decoded)
                .to_owned();
        }
        Text::from(decoded.as_str())
    }
}

pub struct StreamDecoder {
    assembler: CompletionAssembler,
    decoder: Utf8Decoder,
    buffer: Vec<u16>,
    scan: usize,
    head: Text,
    bytes: usize,
    frames: u64,
    done: bool,
    ready: Vec<Text>,
}

impl StreamDecoder {
    pub fn new(requested_model: Text) -> Self {
        Self {
            assembler: CompletionAssembler::new(requested_model),
            decoder: Utf8Decoder::default(),
            buffer: Vec::new(),
            scan: 0,
            head: Text::default(),
            bytes: 0,
            frames: 0,
            done: false,
            ready: Vec::new(),
        }
    }

    pub fn done(&self) -> bool {
        self.done
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn take_deltas(&mut self) -> Vec<Text> {
        std::mem::take(&mut self.ready)
    }

    fn consume(&mut self, block: &[u16], deltas: &mut Vec<Text>) -> Result<(), Failure> {
        let block = block
            .iter()
            .copied()
            .filter(|unit| *unit != 13)
            .collect::<Vec<_>>();
        for line in block.split(|unit| *unit == 10) {
            let Some(field) = line
                .strip_prefix(&[100, 97, 116, 97, 58])
                .map(trim)
                .filter(|field| !field.is_empty())
            else {
                continue;
            };
            if ascii(field, "[DONE]") {
                self.done = true;
                continue;
            }
            self.frames += 1;
            let chunk = json::parse(&Text(field.to_vec())).map_err(|_| Failure::json())?;
            self.assembler.push(&chunk);
            if let Some(text) = chunk
                .get("choices")
                .and_then(domain::items)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("delta"))
                .and_then(|delta| delta.get("content"))
                .and_then(Value::as_string)
                .filter(|text| !text.0.is_empty())
            {
                deltas.push(text.clone());
            }
        }
        Ok(())
    }

    fn append(&mut self, text: Text) -> Result<Vec<Text>, Failure> {
        if self.head.0.len() < 4096 {
            self.head
                .0
                .extend(text.0.iter().take(4096 - self.head.0.len()));
        }
        self.buffer.extend(text.0);
        let mut deltas = Vec::new();
        let mut consumed = 0;
        while self.scan < self.buffer.len() {
            let start = self.scan;
            let first_newline = if self.buffer.get(start) == Some(&13) {
                start + 1
            } else {
                start
            };
            if self.buffer.get(first_newline) != Some(&10) {
                self.scan += 1;
                continue;
            }
            let second = first_newline + 1;
            let second_newline = if self.buffer.get(second) == Some(&13) {
                second + 1
            } else {
                second
            };
            if self.buffer.get(second_newline) == Some(&10) {
                let block = self.buffer.get(consumed..start).unwrap_or(&[]).to_vec();
                if let Err(error) = self.consume(&block, &mut deltas) {
                    self.ready.extend(deltas);
                    return Err(error);
                }
                consumed = second_newline + 1;
                self.scan = consumed;
            } else if second_newline >= self.buffer.len() {
                break;
            } else {
                self.scan += 1;
            }
        }
        if consumed > 0 {
            self.buffer.drain(..consumed);
            self.scan -= consumed;
        }
        Ok(deltas)
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Text>, Failure> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|bytes| *bytes <= MAX_RESPONSE_BYTES)
            .ok_or_else(|| {
                Failure::chat(
                    "The LLM stream exceeded the supported size.",
                    Category::Response,
                )
            })?;
        let text = self.decoder.decode(bytes, false);
        self.append(text)
    }

    pub fn finish(&mut self) -> Result<(Value, Vec<Text>), Failure> {
        let text = self.decoder.decode(&[], true);
        let mut deltas = self.append(text)?;
        if !trim(&self.buffer).is_empty() {
            let block = std::mem::take(&mut self.buffer);
            if let Err(error) = self.consume(&block, &mut deltas) {
                self.ready.extend(deltas);
                return Err(error);
            }
        }
        if self.frames == 0 {
            let parsed = json::parse(&Text(trim(&self.head.0).to_vec()));
            if parsed.is_ok_and(|value| value.get("error").is_some()) {
                let mut error = Failure::chat(
                    "The LLM endpoint returned an error body instead of a stream; a gateway backend may be down.",
                    Category::Http,
                );
                error.transient = true;
                return Err(error);
            }
            return Err(Failure::chat(
                "The LLM stream contained no completion data.",
                Category::Response,
            ));
        }
        Ok((self.assembler.payload(), deltas))
    }
}
