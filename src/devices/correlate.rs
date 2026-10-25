//! Windows lets a keyboard hook swallow keys, but does not say which keyboard they came from;
//! Raw Input says which keyboard, but cannot swallow anything. This matches the two: Raw Input
//! reports every key here, and the hook asks what was reported about the key it is looking at.
//!
//! Both see the same keys in the same order, so a key is matched by its scan code and direction.
//! Raw Input events that the hook never saw (or that arrived long ago) are dropped.

#![cfg_attr(test, allow(dead_code))]

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A raw event older than this is of no use any more (the hook looks at it within milliseconds).
const STALE: Duration = Duration::from_millis(150);
/// Never keep more than this: it only grows if the hook is not asking.
const MAX_PENDING: usize = 256;

/// A key, as both Raw Input and the hook describe it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyId {
    /// Scan code (set 1) without the 0xE0 / 0xE1 prefix.
    pub scan: u16,
    /// Key with the 0xE0 / 0xE1 prefix (right Ctrl, arrows...).
    pub extended: bool,
    /// Released instead of pressed.
    pub up: bool,
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    key: KeyId,
    /// The keyboard that pressed it is an exclusive one.
    exclusive: bool,
    at: Instant,
}

/// Queue of what Raw Input has reported and the hook has not yet looked at.
pub struct Correlator {
    queue: Mutex<VecDeque<Pending>>,
    arrived: Condvar,
}

impl Correlator {
    pub const fn new() -> Self {
        Correlator {
            queue: Mutex::new(VecDeque::new()),
            arrived: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<Pending>> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Raw Input reported `key`; `exclusive` tells whether its keyboard is one to swallow.
    pub fn push(&self, key: KeyId, exclusive: bool) {
        self.push_at(key, exclusive, Instant::now());
    }

    fn push_at(&self, key: KeyId, exclusive: bool, at: Instant) {
        let mut queue = self.lock();
        queue.push_back(Pending { key, exclusive, at });
        while queue.len() > MAX_PENDING {
            queue.pop_front();
        }
        drop(queue);
        self.arrived.notify_all();
    }

    /// The hook is looking at `key`: was it reported by an exclusive keyboard? Waits up to `wait`
    /// for Raw Input to report it. `None` = nobody reported it.
    pub fn take(&self, key: KeyId, wait: Duration) -> Option<bool> {
        self.take_at(key, Instant::now(), wait)
    }

    fn take_at(&self, key: KeyId, start: Instant, wait: Duration) -> Option<bool> {
        let deadline = start + wait;
        let mut queue = self.lock();
        loop {
            let now = Instant::now();
            while queue
                .front()
                .is_some_and(|p| now.saturating_duration_since(p.at) > STALE)
            {
                queue.pop_front();
            }
            if let Some(index) = queue.iter().position(|p| p.key == key) {
                // what came before it was never seen by the hook: it is not coming any more
                queue.drain(..index);
                return queue.pop_front().map(|p| p.exclusive);
            }
            let left = deadline.saturating_duration_since(now);
            if left.is_zero() {
                return None;
            }
            queue = self
                .arrived
                .wait_timeout(queue, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    /// Forgets everything (the hook has been removed).
    pub fn clear(&self) {
        self.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const A_DOWN: KeyId = KeyId {
        scan: 30,
        extended: false,
        up: false,
    };
    const A_UP: KeyId = KeyId { up: true, ..A_DOWN };
    const B_DOWN: KeyId = KeyId { scan: 48, ..A_DOWN };
    const NOW_ISH: Duration = Duration::from_millis(30);

    #[test]
    fn a_reported_key_is_found_with_its_verdict() {
        let c = Correlator::new();
        c.push(A_DOWN, true);
        c.push(A_UP, false);
        assert_eq!(c.take(A_DOWN, NOW_ISH), Some(true));
        assert_eq!(c.take(A_UP, NOW_ISH), Some(false));
        assert_eq!(c.take(A_UP, Duration::ZERO), None, "it is used up");
    }

    #[test]
    fn the_hook_waits_for_the_late_report() {
        let c = Arc::new(Correlator::new());
        let late = Arc::clone(&c);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            late.push(A_DOWN, true);
        });
        assert_eq!(c.take(A_DOWN, Duration::from_secs(2)), Some(true));
        thread.join().unwrap();
    }

    #[test]
    fn nobody_reported_it() {
        let c = Correlator::new();
        c.push(B_DOWN, true);
        let started = Instant::now();
        assert_eq!(c.take(A_DOWN, Duration::from_millis(30)), None);
        assert!(started.elapsed() >= Duration::from_millis(25), "it waits");
    }

    #[test]
    fn older_unseen_keys_are_dropped_when_a_later_one_is_found() {
        let c = Correlator::new();
        c.push(A_DOWN, true); // the hook never looked at this one
        c.push(B_DOWN, false);
        assert_eq!(c.take(B_DOWN, NOW_ISH), Some(false));
        assert_eq!(c.take(A_DOWN, Duration::ZERO), None);
    }

    #[test]
    fn a_stale_report_is_not_used() {
        let c = Correlator::new();
        c.push_at(A_DOWN, true, Instant::now() - Duration::from_secs(1));
        assert_eq!(c.take(A_DOWN, Duration::ZERO), None);
    }

    #[test]
    fn same_key_twice_is_matched_in_order() {
        let c = Correlator::new();
        c.push(A_DOWN, true);
        c.push(A_DOWN, false);
        assert_eq!(c.take(A_DOWN, NOW_ISH), Some(true));
        assert_eq!(c.take(A_DOWN, NOW_ISH), Some(false));
    }

    #[test]
    fn clear_forgets_everything() {
        let c = Correlator::new();
        c.push(A_DOWN, true);
        c.clear();
        assert_eq!(c.take(A_DOWN, Duration::ZERO), None);
    }
}
