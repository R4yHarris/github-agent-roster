use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::Notify;

use crate::cancel::{CancellationToken, Reason};
use crate::text::Text;
use crate::{Error, MAX_SAFE_INTEGER};

fn state_error() -> Error {
    Error::new(
        "Error",
        "Request admission state is unavailable; work must stop.",
    )
    .coded("ROSTER_ADMISSION_STATE")
}

fn validate_profile(profile: &Text) -> Result<(), Error> {
    if profile.0.is_empty() {
        return Err(Error::type_error("profileId must be a non-empty string"));
    }
    Ok(())
}

fn check(
    parent: Option<&CancellationToken>,
    signal: Option<&CancellationToken>,
) -> Result<(), Error> {
    for token in [parent, signal].into_iter().flatten() {
        token.check()?;
    }
    Ok(())
}

async fn cancelled(token: Option<&CancellationToken>) -> Result<Reason, Error> {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

struct Waiter {
    id: u64,
    notify: Arc<Notify>,
    parent: Option<CancellationToken>,
    signal: Option<CancellationToken>,
}

struct State {
    capacity: u64,
    next_id: u64,
    holders: HashSet<u64>,
    waiters: VecDeque<Waiter>,
    poisoned: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            capacity: 1,
            next_id: 0,
            holders: HashSet::new(),
            waiters: VecDeque::new(),
            poisoned: false,
        }
    }
}

impl State {
    fn check(&self) -> Result<(), Error> {
        if self.poisoned {
            return Err(state_error());
        }
        Ok(())
    }

    fn dispatch(&mut self) -> Vec<Arc<Notify>> {
        let mut notifications = Vec::new();
        if self.poisoned {
            notifications.extend(self.waiters.iter().map(|waiter| waiter.notify.clone()));
            return notifications;
        }
        self.waiters.retain(|waiter| {
            if check(waiter.parent.as_ref(), waiter.signal.as_ref()).is_err() {
                notifications.push(waiter.notify.clone());
                false
            } else {
                true
            }
        });
        while (self.holders.len() as u64) < self.capacity {
            let Some(waiter) = self.waiters.pop_front() else {
                break;
            };
            self.holders.insert(waiter.id);
            notifications.push(waiter.notify);
        }
        notifications
    }
}

#[derive(Default)]
struct Profile {
    state: Mutex<State>,
}

impl Profile {
    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poison) => {
                let mut state = poison.into_inner();
                state.poisoned = true;
                state
            }
        }
    }

    fn finish(&self, id: u64) -> Result<bool, Error> {
        let (removed, healthy, notifications) = {
            let mut state = self.lock();
            let removed = state.holders.remove(&id);
            state.waiters.retain(|waiter| waiter.id != id);
            (removed, state.check(), state.dispatch())
        };
        notify(notifications);
        healthy?;
        Ok(removed)
    }
}

fn notify(notifications: Vec<Arc<Notify>>) {
    for notification in notifications {
        notification.notify_one();
    }
}

#[derive(Clone, Default)]
pub struct Admission {
    profiles: Arc<Mutex<HashMap<Text, Arc<Profile>>>>,
}

impl Admission {
    fn profile(&self, profile: &Text) -> Result<Arc<Profile>, Error> {
        validate_profile(profile)?;
        let mut profiles = self.profiles.lock().map_err(|_| state_error())?;
        Ok(profiles.entry(profile.clone()).or_default().clone())
    }

    pub fn configure(&self, profile: &Text, capacity: u64) -> Result<(), Error> {
        if !(1..=MAX_SAFE_INTEGER).contains(&capacity) {
            return Err(Error::type_error(
                "capacity must be a positive safe integer",
            ));
        }
        let profile = self.profile(profile)?;
        let notifications = {
            let mut state = profile.lock();
            state.check()?;
            state.capacity = capacity;
            state.dispatch()
        };
        notify(notifications);
        Ok(())
    }

    pub fn depth(&self, profile: &Text) -> Result<u64, Error> {
        validate_profile(profile)?;
        let profile = self
            .profiles
            .lock()
            .map_err(|_| state_error())?
            .get(profile)
            .cloned();
        let Some(profile) = profile else {
            return Ok(0);
        };
        let (depth, notifications) = {
            let mut state = profile.lock();
            state.check()?;
            let notifications = state.dispatch();
            let depth = (state.waiters.len() as u64)
                .saturating_add(state.holders.len() as u64)
                .saturating_sub(state.capacity);
            (depth, notifications)
        };
        notify(notifications);
        Ok(depth)
    }

    pub fn acquire(
        &self,
        profile: &Text,
        parent: Option<&CancellationToken>,
        signal: Option<&CancellationToken>,
    ) -> Result<Acquisition, Error> {
        check(parent, signal)?;
        let profile = self.profile(profile)?;
        let notification = Arc::new(Notify::new());
        let (id, notifications) = {
            let mut state = profile.lock();
            state.check()?;
            state.next_id = state.next_id.checked_add(1).ok_or_else(state_error)?;
            let id = state.next_id;
            state.waiters.push_back(Waiter {
                id,
                notify: notification.clone(),
                parent: parent.cloned(),
                signal: signal.cloned(),
            });
            (id, state.dispatch())
        };
        notify(notifications);
        Ok(Acquisition {
            id: Some(id),
            profile,
            notify: notification,
            parent: parent.cloned(),
            signal: signal.cloned(),
        })
    }
}

pub struct Acquisition {
    id: Option<u64>,
    profile: Arc<Profile>,
    notify: Arc<Notify>,
    parent: Option<CancellationToken>,
    signal: Option<CancellationToken>,
}

impl Acquisition {
    pub fn granted(&self) -> Result<bool, Error> {
        check(self.parent.as_ref(), self.signal.as_ref())?;
        let state = self.profile.lock();
        state.check()?;
        Ok(self.id.is_some_and(|id| state.holders.contains(&id)))
    }

    pub async fn wait(mut self) -> Result<Lease, Error> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.granted()? {
                check(self.parent.as_ref(), self.signal.as_ref())?;
                return Ok(Lease {
                    id: self.id.take(),
                    profile: self.profile.clone(),
                });
            }
            tokio::select! {
                biased;
                reason = cancelled(self.parent.as_ref()) => return Err(reason?.error()),
                reason = cancelled(self.signal.as_ref()) => return Err(reason?.error()),
                () = &mut notified => {}
            }
        }
    }
}

impl Drop for Acquisition {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let _ = self.profile.finish(id);
        }
    }
}

pub struct Lease {
    id: Option<u64>,
    profile: Arc<Profile>,
}

impl Lease {
    pub fn release(&mut self) -> Result<bool, Error> {
        match self.id.take() {
            Some(id) => self.profile.finish(id),
            None => Ok(false),
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let _ = self.profile.finish(id);
        }
    }
}
