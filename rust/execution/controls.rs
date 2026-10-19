use std::future::Future;
use std::time::Duration;

use crate::Error;
use crate::cancel::{CancellationSource, CancellationToken, Reason};
use crate::text::Text;

pub async fn with_task_deadline<T, F, Fut>(
    parent: &CancellationToken,
    timeout_ms: Option<u32>,
    operation: F,
) -> Result<T, Error>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    crate::cancel::validate_task_timeout(timeout_ms.map(f64::from))?;
    parent.check()?;
    let source = parent.child_source()?;
    let token = source.token();
    let deadline = timeout_ms.map(|milliseconds| {
        tokio::time::Instant::now() + Duration::from_millis(u64::from(milliseconds))
    });
    let mut work = Box::pin(operation(token.clone()));
    let result = if let Some(deadline) = deadline {
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline) => {
                source.cancel(Reason::TaskTimeout(timeout_ms.unwrap_or(1)))?;
                work.await
            }
            result = &mut work => result,
        }
    } else {
        work.await
    };
    if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
        source.cancel(Reason::TaskTimeout(timeout_ms.unwrap_or(1)))?;
    }
    if let Err(error) = token.check() {
        return Err(match result {
            Err(cause) if cause != error => error.caused_by(cause),
            _ => error,
        });
    }
    result
}

pub struct Steering {
    parent: CancellationToken,
    current: Option<CancellationSource>,
    instruction: Option<Text>,
    steered: bool,
}

impl Steering {
    pub fn new(parent: CancellationToken) -> Self {
        Self {
            parent,
            current: None,
            instruction: None,
            steered: false,
        }
    }

    pub fn waiting(&self) -> bool {
        self.current.is_some() && !self.steered
    }

    pub fn begin(&mut self) -> Result<CancellationToken, Error> {
        self.parent.check()?;
        if self.current.is_some() {
            return Err(Error::new(
                "Error",
                "Only one coder model request can be active.",
            ));
        }
        let source = self.parent.child_source()?;
        let token = source.token();
        self.current = Some(source);
        self.steered = false;
        Ok(token)
    }

    pub fn steer(&mut self, instruction: &Text) -> Result<(), Error> {
        if instruction.0.len() > 4096 || crate::text::trim(&instruction.0).is_empty() {
            return Err(Error::type_error(
                "Steering needs 1-4096 characters of human instruction.",
            ));
        }
        self.parent.check()?;
        if !self.waiting() {
            return Err(Error::new(
                "Error",
                "The coder is not awaiting a model call; wait for drafting before /steer.",
            ));
        }
        self.instruction = Some(Text(crate::text::trim(&instruction.0).to_vec()));
        self.steered = true;
        if let Some(source) = &self.current {
            source.cancel(Reason::Cancelled)?;
        }
        Ok(())
    }

    pub fn finish<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        let current = self
            .current
            .take()
            .ok_or_else(|| Error::new("Error", "No coder model request is active."))?;
        let reason = current.token().reason()?;
        self.parent.check()?;
        if self.steered
            && result.as_ref().err().is_none_or(|error| {
                error.code == Some("ROSTER_CANCELLED") || error.code == Some("ROSTER_STEERING")
            })
        {
            return Err(Error::new(
                "SteeringInterrupt",
                "Coder request interrupted for human steering.",
            )
            .coded("ROSTER_STEERING"));
        }
        if result.is_ok() {
            if let Some(reason) = reason {
                return Err(reason.error());
            }
        }
        result
    }

    pub fn take(&mut self) -> Option<Text> {
        self.instruction.take()
    }

    pub fn dispose(&mut self) -> Result<(), Error> {
        self.instruction = None;
        if let Some(source) = &self.current {
            source.cancel(Reason::Cancelled)?;
        }
        Ok(())
    }
}
