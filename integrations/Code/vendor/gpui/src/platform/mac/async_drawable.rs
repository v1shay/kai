//! Bounded handoff. Metal acquisition never runs while this mutex is held.
//! Configuration is applied by main under this mutex before the worker can
//! take the request. Completion never automatically starts another request.
use std::sync::{Condvar, Mutex, MutexGuard};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Readiness { Ready, Waiting, Suspended }

struct State<T> {
    generation: u64,
    visible: bool,
    closed: bool,
    queued: Option<u64>,
    in_flight: Option<u64>,
    ready: Option<(u64, T)>,
}

pub struct Mailbox<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
}

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self { state: Mutex::new(State {
            generation: 0, visible: false, closed: false,
            queued: None, in_flight: None, ready: None,
        }), changed: Condvar::new() }
    }
}

impl<T> Mailbox<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Main only: invalidation cancels a queued request, never an in-flight
    /// call. Its eventual completion must wake main even when stale.
    pub fn invalidate(&self) {
        let mut state = self.lock();
        state.generation = state.generation.wrapping_add(1);
        state.ready = None;
        state.queued = None;
    }

    pub fn is_visible(&self) -> bool { let state = self.lock(); state.visible && !state.closed }

    pub fn set_visible(&self, visible: bool) {
        let mut state = self.lock();
        if state.visible != visible {
            state.visible = visible;
            state.generation = state.generation.wrapping_add(1);
            state.ready = None;
            state.queued = None;
        }
    }

    /// Main only. Configure must not recurse into this mailbox. It is never
    /// called while nextDrawable is in flight. No worker waits on main.
    pub fn prepare(&self, configure: impl FnOnce()) -> Readiness {
        let mut state = self.lock();
        if state.closed || !state.visible { return Readiness::Suspended; }
        if state.ready.is_some() { return Readiness::Ready; }
        if state.in_flight.is_some() || state.queued.is_some() {
            return Readiness::Waiting;
        }
        configure();
        state.queued = Some(state.generation);
        self.changed.notify_one();
        Readiness::Waiting
    }

    /// Worker only. Completion parks here until main explicitly prepares
    /// another acquisition; it cannot race a logical descriptor update.
    pub fn next_request(&self) -> Option<u64> {
        let mut state = self.lock();
        loop {
            if state.closed { return None; }
            if let Some(generation) = state.queued.take() {
                debug_assert!(state.in_flight.is_none());
                state.in_flight = Some(generation);
                return Some(generation);
            }
            state = self.changed.wait(state).unwrap_or_else(|error| error.into_inner());
        }
    }

    /// Worker only. Caller sends one main notification after completion,
    /// including stale/nil results: main may have a newer descriptor to apply.
    /// False means closed and no notification should be scheduled.
    pub fn complete(&self, generation: u64, value: Option<T>) -> bool {
        let mut state = self.lock();
        debug_assert_eq!(state.in_flight, Some(generation));
        state.in_flight = None;
        if state.closed { return false; }
        if state.visible && state.generation == generation {
            state.ready = value.map(|value| (generation, value));
        }
        true
    }

    pub fn take_ready(&self) -> Option<T> {
        let mut state = self.lock();
        if state.closed || !state.visible || state.in_flight.is_some() { return None; }
        state.ready.take().and_then(|(generation, value)|
            (generation == state.generation).then_some(value))
    }

    /// Main only, after take_ready: serialization for presentation-mode writes.
    pub fn while_idle(&self, configure: impl FnOnce()) -> bool {
        let state = self.lock();
        if state.closed || state.in_flight.is_some() || state.queued.is_some() { return false; }
        configure();
        true
    }

    pub fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.generation = state.generation.wrapping_add(1);
        state.queued = None;
        state.ready = None;
        self.changed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}, mpsc};

    #[test]
    fn config_never_runs_during_blocked_acquire_and_latest_generation_wins() {
        let mailbox = Arc::new(Mailbox::<usize>::default());
        mailbox.set_visible(true);
        let configs = AtomicUsize::new(0);
        assert_eq!(mailbox.prepare(|| { configs.fetch_add(1, Ordering::SeqCst); }), Readiness::Waiting);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = { let mailbox = mailbox.clone(); std::thread::spawn(move || {
            let generation = mailbox.next_request().unwrap();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            assert!(mailbox.complete(generation, Some(1)));
        }) };
        entered_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        for _ in 0..1000 {
            mailbox.invalidate();
            assert_eq!(mailbox.prepare(|| panic!("configuration raced acquisition")), Readiness::Waiting);
        }
        assert!(mailbox.take_ready().is_none());
        release_tx.send(()).unwrap(); worker.join().unwrap();
        assert!(mailbox.take_ready().is_none());
        assert_eq!(mailbox.prepare(|| { configs.fetch_add(1, Ordering::SeqCst); }), Readiness::Waiting);
        let latest = mailbox.next_request().unwrap();
        assert!(mailbox.complete(latest, Some(2)));
        assert_eq!(mailbox.prepare(|| panic!("already ready")), Readiness::Ready);
        assert_eq!(mailbox.take_ready(), Some(2));
        assert_eq!(configs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn complete_parks_until_main_requests_and_nil_does_not_acknowledge() {
        let mailbox = Mailbox::<usize>::default(); mailbox.set_visible(true);
        mailbox.prepare(|| ()); let first = mailbox.next_request().unwrap();
        assert!(mailbox.complete(first, None));
        { let state = mailbox.lock(); assert!(state.queued.is_none()); assert!(state.ready.is_none()); assert!(state.in_flight.is_none()); }
        mailbox.prepare(|| ()); let second = mailbox.next_request().unwrap();
        mailbox.complete(second, Some(5));
        assert_eq!(mailbox.take_ready(), Some(5));
        assert_eq!(mailbox.take_ready(), None);
    }

    #[test]
    fn hidden_and_close_discard_results_without_waiting_for_worker() {
        let mailbox = Mailbox::<usize>::default(); mailbox.set_visible(true);
        mailbox.prepare(|| ()); let generation = mailbox.next_request().unwrap();
        mailbox.set_visible(false);
        assert_eq!(mailbox.prepare(|| panic!()), Readiness::Suspended);
        assert!(mailbox.complete(generation, Some(1))); assert_eq!(mailbox.take_ready(), None);
        mailbox.set_visible(true); mailbox.prepare(|| ()); let next = mailbox.next_request().unwrap();
        mailbox.close(); assert!(!mailbox.complete(next, Some(2)));
        assert_eq!(mailbox.next_request(), None);
        assert_eq!(mailbox.prepare(|| panic!()), Readiness::Suspended);
    }

    #[test]
    fn cancelled_queued_request_never_acquires_and_slots_stay_bounded() {
        let mailbox = Mailbox::<usize>::default(); mailbox.set_visible(true);
        for _ in 0..1000 { mailbox.invalidate(); mailbox.prepare(|| ()); }
        let generation = mailbox.next_request().unwrap();
        for _ in 0..1000 { assert_eq!(mailbox.prepare(|| panic!()), Readiness::Waiting); }
        mailbox.complete(generation, Some(1));
        for _ in 0..1000 { assert_eq!(mailbox.prepare(|| panic!()), Readiness::Ready); }
        let state = mailbox.lock(); assert!(state.queued.is_none()); assert!(state.in_flight.is_none()); assert!(state.ready.is_some());
    }
}
