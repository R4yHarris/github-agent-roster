pub mod admission;
pub mod baseline;
pub mod builtin;
pub mod control;
pub mod http;
pub mod openai;
pub mod reasoning;
pub(crate) mod redact;
pub mod stream;
pub mod window;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod builtin_tests;

#[cfg(test)]
mod metadata_drop_tests;

use std::fmt;

use crate::Error;
use crate::json::Value;
use crate::text::Text;

pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Authentication,
    Http,
    Network,
    Response,
    Timeout,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Http => "http",
            Self::Network => "network",
            Self::Response => "response",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub error: Box<Error>,
    pub message_text: Option<Text>,
    pub category: Option<Category>,
    pub transient: bool,
    pub finish_reason: Option<Text>,
    pub truncated: Option<bool>,
    pub partial: Option<Box<Value>>,
    pub limit: Option<u32>,
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self {
            error: Box::new(error),
            message_text: None,
            category: None,
            transient: false,
            finish_reason: None,
            truncated: None,
            partial: None,
            limit: None,
        }
    }
}

impl Failure {
    pub fn chat(message: impl Into<String>, category: Category) -> Self {
        let mut result = Self::from(Error::new("Error", message));
        result.category = Some(category);
        result
    }

    pub fn network() -> Self {
        Self::chat(
            "The LLM request failed. Check the endpoint and connection.",
            Category::Network,
        )
    }

    pub fn json() -> Self {
        Self::chat("The LLM response was not valid JSON.", Category::Response)
    }

    pub fn stream_read() -> Self {
        Self::chat(
            "The LLM stream ended before the response was complete.",
            Category::Response,
        )
    }

    pub fn observer(error: Error) -> Self {
        if matches!(
            error.code,
            Some("ROSTER_RUN_LOG" | "ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT")
        ) {
            Self::from(error)
        } else {
            Self::network()
        }
    }

    pub fn is_timeout(&self) -> bool {
        crate::request::is_llm_timeout(&self.error)
    }

    pub fn route_failure(&self) -> Option<&'static str> {
        if self.transient {
            Some("endpoint-error")
        } else if self.is_timeout() {
            Some("endpoint-timeout")
        } else {
            None
        }
    }

    pub fn as_value(&self) -> Value {
        let mut fields = vec![
            ("name", Value::string(self.error.name)),
            (
                "message",
                self.message_text
                    .clone()
                    .map_or_else(|| Value::string(&self.error.message), Value::String),
            ),
        ];
        if let Some(code) = self.error.code {
            fields.push(("code", Value::string(code)));
        }
        if let Some(category) = self.category {
            fields.push(("category", Value::string(category.as_str())));
        }
        if self.transient {
            fields.push(("transient", Value::Boolean(true)));
        }
        if let Some(reason) = &self.finish_reason {
            fields.push(("finishReason", Value::String(reason.clone())));
        }
        if let Some(truncated) = self.truncated {
            fields.push(("truncated", Value::Boolean(truncated)));
        }
        if let Some(partial) = &self.partial {
            fields.push(("partial", *partial.clone()));
        }
        if let Some(limit) = self.limit {
            fields.push(("limit", Value::Number(limit as f64)));
        }
        Value::object(fields)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for Failure {}
