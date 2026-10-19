use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use crate::Error;
use crate::cancel::{CancellationToken, Reason};
use crate::json::Value;
use crate::request::{self, WAITING_INTERVAL_MS};

use super::{Category, Failure};

pub trait Observer {
    fn enabled(&self) -> bool {
        true
    }

    fn emit(&mut self, event: Value) -> impl Future<Output = Result<(), Error>>;
}

pub struct NoObserver;

impl Observer for NoObserver {
    fn enabled(&self) -> bool {
        false
    }

    async fn emit(&mut self, _event: Value) -> Result<(), Error> {
        Ok(())
    }
}

async fn cancelled(token: Option<&CancellationToken>) -> Result<Reason, Error> {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn idle_timeout(deadline: Option<(Instant, u32)>) {
    match deadline {
        Some((deadline, _)) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

pub struct RequestControl<'a> {
    host: &'a str,
    local: bool,
    timeout_ms: u32,
    retry_command: Option<&'a str>,
    started: Instant,
    deadline: Instant,
    next_waiting: Instant,
    parent: Option<&'a CancellationToken>,
    token: Option<&'a CancellationToken>,
}

impl<'a> RequestControl<'a> {
    pub fn new(
        host: &'a str,
        local: bool,
        timeout_ms: u32,
        retry_command: Option<&'a str>,
        parent: Option<&'a CancellationToken>,
        token: Option<&'a CancellationToken>,
    ) -> Self {
        let started = Instant::now();
        Self {
            host,
            local,
            timeout_ms,
            retry_command,
            started,
            deadline: started + Duration::from_millis(timeout_ms as u64),
            next_waiting: started + Duration::from_millis(WAITING_INTERVAL_MS as u64),
            parent,
            token,
        }
    }

    fn timeout(&self) -> Failure {
        let mut error = Failure::chat(
            request::timeout_hint(self.host, self.timeout_ms, self.local, self.retry_command),
            Category::Timeout,
        );
        error.error.code = Some("ROSTER_LLM_TIMEOUT");
        error
    }

    fn stall(&self, idle: Option<(Instant, u32)>) -> Failure {
        let milliseconds = idle.map_or(0, |(_, milliseconds)| milliseconds);
        let seconds = ryu_js::Buffer::new()
            .format_finite(milliseconds as f64 / 1000.0)
            .to_owned();
        let mut error = Failure::chat(
            format!(
                "The LLM stream at host={} sent nothing for {seconds}s and was abandoned. This is an endpoint stall, not a bad TASK.",
                self.host
            ),
            Category::Timeout,
        );
        error.error.code = Some("ROSTER_LLM_STALL");
        error
    }

    pub fn check(&self) -> Result<(), Failure> {
        if let Some(parent) = self.parent {
            parent.check()?;
        }
        if let Some(token) = self.token {
            token.check()?;
        }
        if Instant::now() >= self.deadline {
            return Err(self.timeout());
        }
        Ok(())
    }

    pub async fn emit(
        &self,
        observer: &mut impl Observer,
        event: Value,
        idle: Option<(Instant, u32)>,
    ) -> Result<(), Failure> {
        self.check()?;
        let operation = observer.emit(event);
        tokio::pin!(operation);
        let result = tokio::select! {
            biased;
            reason = cancelled(self.parent) => Err(Failure::from(reason?.error())),
            reason = cancelled(self.token) => Err(Failure::from(reason?.error())),
            () = tokio::time::sleep_until(self.deadline) => Err(self.timeout()),
            () = idle_timeout(idle) => Err(self.stall(idle)),
            result = &mut operation => result.map_err(Failure::observer),
        };
        self.check()?;
        result
    }

    pub async fn emit_terminal(
        &self,
        observer: &mut impl Observer,
        event: Value,
    ) -> Result<(), Failure> {
        for token in [self.parent, self.token].into_iter().flatten() {
            token.check()?;
        }
        tokio::select! {
            biased;
            reason = cancelled(self.parent) => Err(reason?.error().into()),
            reason = cancelled(self.token) => Err(reason?.error().into()),
            result = observer.emit(event) => result.map_err(Failure::observer),
            () = tokio::time::sleep_until(self.deadline) => Err(self.timeout()),
        }
    }

    pub async fn wait<T>(
        &mut self,
        operation: impl Future<Output = T>,
        observer: &mut impl Observer,
        idle: Option<(Instant, u32)>,
    ) -> Result<T, Failure> {
        tokio::pin!(operation);
        loop {
            self.check()?;
            tokio::select! {
                biased;
                reason = cancelled(self.parent) => return Err(reason?.error().into()),
                reason = cancelled(self.token) => return Err(reason?.error().into()),
                () = tokio::time::sleep_until(self.deadline) => return Err(self.timeout()),
                () = idle_timeout(idle) => return Err(self.stall(idle)),
                () = tokio::time::sleep_until(self.next_waiting), if observer.enabled() => {
                    self.next_waiting += Duration::from_millis(WAITING_INTERVAL_MS as u64);
                    let elapsed = Instant::now().duration_since(self.started).as_secs();
                    self.emit(observer, Value::object(vec![
                        ("type", Value::string("waiting")),
                        ("host", Value::string(self.host)),
                        ("elapsedSeconds", Value::Number(elapsed as f64)),
                        ("local", Value::Boolean(self.local)),
                    ]), idle).await?;
                }
                result = &mut operation => {
                    self.check()?;
                    return Ok(result);
                }
            }
        }
    }
}
