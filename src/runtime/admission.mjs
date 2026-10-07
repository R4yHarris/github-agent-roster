/**
 * Per-profile FIFO admission semaphore.
 *
 * Shape of a waiter:
 *   { resolve, reject, onAbort, aborted }
 *
 * Shape of a profile entry (module-level Map<profileId, entry>):
 *   { capacity: number, inFlight: number, waiters: FIFO queue of waiters }
 *
 * Public API:
 *   configure(profileId, capacity)  — set capacity for a profile (default 1).
 *   acquire(profileId, { signal } = {}) — returns a Promise that resolves to a
 *     one-shot release function when a slot becomes available. If `signal` aborts, the Promise rejects with
 *     an `AbortError` and the waiter is removed from the queue.
 *   release(profileId)  — frees one in-flight slot and, if waiters exist,
 *     admits the next one immediately. Safe no-op if there are no in-flight
 *     holders (prevents over-admission from double release).
 *   depth(profileId)    — number of pending waiters + inFlight − capacity,
 *     clamped to 0 for unknown profiles.
 *
 * No external dependencies; state is an in-process Map only.
 */

const profiles = new Map();

function entry(profileId, capacity = 1) {
  let e = profiles.get(profileId);
  if (!e) {
    e = { capacity, inFlight: 0, waiters: [] };
    profiles.set(profileId, e);
  }
  return e;
}

/** Set (or override) the capacity for a profile. */
export function configure(profileId, capacity) {
  if (!Number.isInteger(capacity) || capacity < 1) {
    throw new TypeError("capacity must be a positive integer");
  }
  const e = entry(profileKey(profileId));
  e.capacity = capacity;
  // Admit any queued waiters that were blocked solely by the old capacity.
  admit(e);
}

/**
 * Try to admit waiters into free slots for a profile.
 * Returns true if any waiter was admitted.
 */
function admit(e) {
  let admitted = false;
  while (e.inFlight < e.capacity && e.waiters.length > 0) {
    const w = e.waiters.shift();
    if (w.aborted) continue; // stale entry already handled by abort
    e.inFlight += 1;
    admitted = true;
    w.signal?.removeEventListener("abort", w.onAbort);
    w.resolve(slot(e));
  }
  return admitted;
}

// One-shot release for a single admitted holder: a repeated call cannot free another holder's slot.
function slot(e) {
  let released = false;
  return () => {
    if (released) return;
    released = true;
    releaseEntry(e);
  };
}

function profileKey(profileId) {
  if (typeof profileId !== "string" || !profileId) {
    throw new TypeError("profileId must be a non-empty string");
  }
  return profileId;
}

/**
 * Acquire a slot for `profileId`.
 * Resolves to an idempotent release function for this holder; prefer it over
 * `release(profileId)`, which cannot tell holders apart.
 * @param {string} profileId
 * @param {{ signal?: AbortSignal }} [opts]
 * @returns {Promise<() => void>}
 */
export function acquire(profileId, { signal } = {}) {
  const e = entry(profileKey(profileId));

  if (signal && signal.aborted) {
    return Promise.reject(new DOMException("Aborted", "AbortError"));
  }

  if (e.inFlight < e.capacity) {
    e.inFlight += 1;
    return Promise.resolve(slot(e));
  }

  return new Promise((resolve, reject) => {
    const waiter = {
      resolve,
      reject,
      signal,
      aborted: false,
      onAbort: () => {
        if (waiter.aborted) return;
        waiter.aborted = true;
        const idx = e.waiters.indexOf(waiter);
        if (idx !== -1) e.waiters.splice(idx, 1);
        reject(new DOMException("Aborted", "AbortError"));
      },
    };

    if (signal) {
      signal.addEventListener("abort", waiter.onAbort, { once: true });
    }

    e.waiters.push(waiter);
  });
}

/**
 * Release one in-flight slot for `profileId`.
 * Wakes the next FIFO waiter if the released slot leaves room.
 * No-op (and safe against over-admission) when there are no in-flight holders.
 */
export function release(profileId) {
  const e = profiles.get(profileKey(profileId));
  if (e) releaseEntry(e);
}

function releaseEntry(e) {
  if (e.inFlight <= 0) return; // double-release guard
  e.inFlight -= 1;
  admit(e);
}

/**
 * Return pending waiters + inFlight − capacity, never negative.
 * Unknown profiles return 0.
 */
export function depth(profileId) {
  const e = profiles.get(profileId);
  if (!e) return 0;
  const d = e.waiters.length + e.inFlight - e.capacity;
  return d < 0 ? 0 : d;
}


