use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::Duration;

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Cancelled,
    TaskTimeout(u32),
}

impl Reason {
    pub fn error(self) -> Error {
        match self {
            Self::Cancelled => Error::new("Error", "Run cancelled.").coded("ROSTER_CANCELLED"),
            Self::TaskTimeout(milliseconds) => Error::new(
                "Error",
                format!("Task wall-clock budget exhausted after {milliseconds} ms; work was cancelled and remains unverified."),
            ).coded("ROSTER_TASK_TIMEOUT"),
        }
    }
}

#[derive(Default)]
struct State {
    reason: Option<Reason>,
    children: Vec<Weak<Inner>>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    changed: Condvar,
    async_changed: tokio::sync::Notify,
    parent: Option<Arc<Inner>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let mut parent = self.parent.take();
        while let Some(ancestor) = parent {
            parent = Arc::into_inner(ancestor).and_then(|mut inner| inner.parent.take());
        }
    }
}

fn lock(inner: &Inner) -> Result<MutexGuard<'_, State>, Error> {
    inner.state.lock().map_err(|_| {
        Error::new(
            "Error",
            "Cancellation state lock was poisoned; work must stop.",
        )
        .coded("ROSTER_CANCELLATION_STATE")
    })
}

fn revoke(inner: &Arc<Inner>, reason: Reason) -> Result<bool, Error> {
    let mut pending = vec![inner.clone()];
    let mut revoked = false;
    let mut failure = None;
    while let Some(current) = pending.pop() {
        let children = match lock(&current) {
            Ok(mut state) => {
                if state.reason.is_some() {
                    continue;
                }
                state.reason = Some(reason);
                revoked = true;
                std::mem::take(&mut state.children)
            }
            Err(error) => {
                failure = failure.or(Some(error));
                continue;
            }
        };
        current.changed.notify_all();
        current.async_changed.notify_waiters();
        for child in children.into_iter().filter_map(|child| child.upgrade()) {
            pending.push(child);
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(revoked)
}

#[derive(Clone, Default)]
pub struct CancellationSource {
    inner: Arc<Inner>,
}

#[derive(Clone)]
pub struct CancellationToken {
    inner: Arc<Inner>,
}

impl CancellationSource {
    pub fn token(&self) -> CancellationToken {
        CancellationToken {
            inner: self.inner.clone(),
        }
    }

    pub fn cancel(&self, reason: Reason) -> Result<bool, Error> {
        revoke(&self.inner, reason)
    }
}

impl CancellationToken {
    pub fn reason(&self) -> Result<Option<Reason>, Error> {
        Ok(lock(&self.inner)?.reason)
    }

    pub fn check(&self) -> Result<(), Error> {
        if let Some(reason) = self.reason()? {
            return Err(reason.error());
        }
        Ok(())
    }

    pub fn child_source(&self) -> Result<CancellationSource, Error> {
        let child = CancellationSource {
            inner: Arc::new(Inner {
                state: Mutex::default(),
                changed: Condvar::new(),
                async_changed: tokio::sync::Notify::new(),
                parent: Some(self.inner.clone()),
            }),
        };
        let mut parent = lock(&self.inner)?;
        parent.children.retain(|child| child.strong_count() > 0);
        if let Some(reason) = parent.reason {
            lock(&child.inner)?.reason = Some(reason);
        } else {
            parent.children.push(Arc::downgrade(&child.inner));
        }
        Ok(child)
    }

    pub fn wait(&self, timeout: Duration) -> Result<Option<Reason>, Error> {
        let state = lock(&self.inner)?;
        let (state, _) = self
            .inner
            .changed
            .wait_timeout_while(state, timeout, |state| state.reason.is_none())
            .map_err(|_| {
                Error::new(
                    "Error",
                    "Cancellation wait lock was poisoned; work must stop.",
                )
                .coded("ROSTER_CANCELLATION_STATE")
            })?;
        Ok(state.reason)
    }

    pub async fn cancelled(&self) -> Result<Reason, Error> {
        loop {
            let notified = self.inner.async_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(reason) = self.reason()? {
                return Ok(reason);
            }
            notified.await;
        }
    }
}

pub fn is_run_cancelled(error: &Error) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.code == Some("ROSTER_CANCELLED") {
            return true;
        }
        current = error.cause.as_deref();
    }
    false
}

pub fn validate_task_timeout(timeout_ms: Option<f64>) -> Result<Option<u32>, Error> {
    timeout_ms
        .map(|value| {
            if !value.is_finite()
                || value.fract() != 0.0
                || !(1.0..=2_147_483_647.0).contains(&value)
            {
                return Err(Error::type_error(
                    "seat.task_timeout_ms must be an integer between 1 and 2147483647",
                ));
            }
            Ok(value as u32)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_revokes_existing_children_and_rejects_late_work() -> Result<(), Error> {
        let parent = CancellationSource::default();
        let child = parent.token().child_source()?;
        let nested = child.token().child_source()?;
        parent.token().check()?;
        assert!(parent.cancel(Reason::Cancelled)?);
        for token in [parent.token(), child.token(), nested.token()] {
            let error = token.check();
            assert!(error.is_err_and(|error| error.code == Some("ROSTER_CANCELLED")));
        }
        assert!(parent.token().child_source()?.token().check().is_err());
        assert!(!parent.cancel(Reason::TaskTimeout(100))?);
        assert_eq!(parent.token().reason()?, Some(Reason::Cancelled));
        Ok(())
    }

    #[test]
    fn child_timeouts_do_not_revoke_parent_or_sibling() -> Result<(), Error> {
        let parent = CancellationSource::default();
        let expired = parent.token().child_source()?;
        let healthy = parent.token().child_source()?;
        expired.cancel(Reason::TaskTimeout(1000))?;
        assert_eq!(expired.token().reason()?, Some(Reason::TaskTimeout(1000)));
        assert!(!is_run_cancelled(&Reason::TaskTimeout(1000).error()));
        parent.token().check()?;
        healthy.token().check()?;
        assert!(is_run_cancelled(
            &Error::new("Error", "outer").caused_by(Reason::Cancelled.error())
        ));
        Ok(())
    }

    #[test]
    fn live_descendants_remain_revocable_after_intermediate_sources_drop() -> Result<(), Error> {
        let root = CancellationSource::default();
        let middle_weak;
        let leaf = {
            let middle = root.token().child_source()?;
            middle_weak = Arc::downgrade(&middle.inner);
            middle.token().child_source()?
        };
        root.cancel(Reason::Cancelled)?;
        assert_eq!(leaf.token().reason()?, Some(Reason::Cancelled));
        drop(leaf);
        assert!(middle_weak.upgrade().is_none());
        Ok(())
    }

    #[test]
    fn deep_chains_survive_intermediate_drops_and_clean_up_iteratively() -> Result<(), Error> {
        let root = CancellationSource::default();
        let mut leaf = root.token();
        for _ in 0..20_000 {
            leaf = leaf.child_source()?.token();
        }
        root.cancel(Reason::Cancelled)?;
        assert_eq!(leaf.reason()?, Some(Reason::Cancelled));
        drop(leaf);
        assert_eq!(Arc::strong_count(&root.inner), 1);
        Ok(())
    }

    #[test]
    fn cancellation_wakes_a_real_waiting_operation() -> Result<(), Error> {
        let source = CancellationSource::default();
        let token = source.token();
        let worker = std::thread::Builder::new()
            .spawn(move || token.wait(Duration::from_secs(5)))
            .map_err(|error| Error::new("Error", error.to_string()))?;
        source.cancel(Reason::Cancelled)?;
        let result = worker
            .join()
            .map_err(|_| Error::new("Error", "Cancellation test worker panicked"))??;
        assert_eq!(result, Some(Reason::Cancelled));
        Ok(())
    }

    #[test]
    fn task_timeout_bounds_are_strict_and_opt_in() -> Result<(), Error> {
        assert_eq!(validate_task_timeout(None)?, None);
        assert_eq!(validate_task_timeout(Some(1.0))?, Some(1));
        assert_eq!(
            validate_task_timeout(Some(2_147_483_647.0))?,
            Some(2_147_483_647)
        );
        for value in [0.0, -1.0, 1.5, f64::NAN, f64::INFINITY, 2_147_483_648.0] {
            assert!(validate_task_timeout(Some(value)).is_err());
        }
        Ok(())
    }

    #[test]
    fn deep_cancellation_chains_do_not_depend_on_call_stack_depth() -> Result<(), Error> {
        let root = CancellationSource::default();
        let mut children = Vec::with_capacity(20_000);
        let mut token = root.token();
        for _ in 0..20_000 {
            let child = token.child_source()?;
            token = child.token();
            children.push(child);
        }
        root.cancel(Reason::Cancelled)?;
        assert_eq!(token.reason()?, Some(Reason::Cancelled));
        assert!(children.iter().all(|child| child.token().check().is_err()));
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_waiters_do_not_lose_early_or_concurrent_cancellation() -> Result<(), Error> {
        let early = CancellationSource::default();
        early.cancel(Reason::Cancelled)?;
        assert_eq!(early.token().cancelled().await?, Reason::Cancelled);
        let source = CancellationSource::default();
        let first = source.token();
        let second = source.token();
        let cancel = async {
            tokio::task::yield_now().await;
            source.cancel(Reason::TaskTimeout(100))
        };
        let (first, second, cancelled) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(first.cancelled(), second.cancelled(), cancel)
        })
        .await
        .map_err(|_| Error::new("Error", "Cancellation waiters did not wake"))?;
        assert!(cancelled?);
        assert_eq!(first?, Reason::TaskTimeout(100));
        assert_eq!(second?, Reason::TaskTimeout(100));
        Ok(())
    }
}
