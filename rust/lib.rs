pub mod arguments;
pub mod ask;
pub mod cancel;
pub mod capabilities;
pub mod catalog;
pub mod clock;
pub mod config;
pub mod delivery;
pub mod diagnostics;
mod domain;
pub mod evaluation;
pub mod execution;
pub mod fleet;
pub mod github;
pub mod host;
pub mod identity;
pub mod json;
pub mod learning;
pub mod llm;
pub mod local_cli;
pub mod metrics;
pub mod openshell;
pub mod paths;
pub mod planning;
pub mod process;
pub mod recipe;
pub mod request;
pub mod run;
pub mod seats;
pub mod secrets;
pub mod settings;
pub mod shell;
pub mod start;
pub mod state;
pub mod status;
pub mod text;
pub mod vault;

use std::fmt;

pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub name: &'static str,
    pub message: String,
    pub message_text: Option<text::Text>,
    pub code: Option<&'static str>,
    pub cause: Option<Box<Error>>,
}

impl Error {
    pub fn new(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            message: message.into(),
            message_text: None,
            code: None,
            cause: None,
        }
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        Self::new("TypeError", message)
    }

    pub fn from_text(name: &'static str, message: text::Text) -> Self {
        match message.to_utf8() {
            Ok(message) => Self::new(name, message),
            Err(_) => {
                let mut error = Self::new(name, String::from_utf16_lossy(&message.0));
                error.message_text = Some(message);
                error
            }
        }
    }

    pub fn message_json(&self) -> String {
        self.message_text.as_ref().map_or_else(
            || text::Text::from(self.message.as_str()).json(),
            text::Text::json,
        )
    }

    pub fn coded(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    pub fn caused_by(mut self, cause: Error) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_deref()
            .map(|error| error as &dyn std::error::Error)
    }
}
