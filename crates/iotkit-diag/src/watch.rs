//! Software watchdog for tasks: each important task owns a [`TaskWatch`] and feeds it.

use core::sync::atomic::{AtomicU32, Ordering};

/// One task's watchdog. Declare it as a `static` and call `feed` (chip feature) at least once per
/// `period`; the watchdog task restarts the device when a watched task stops doing so.
pub struct TaskWatch {
    name: &'static str,
    period_ms: u32,
    /// Milliseconds since boot of the last feed; 0 until the task starts.
    last_ms: AtomicU32,
}

impl TaskWatch {
    pub const fn new(name: &'static str, period_ms: u32) -> Self {
        Self { name, period_ms, last_ms: AtomicU32::new(0) }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Records a feed at `now_ms` (milliseconds since boot).
    pub fn feed_at(&self, now_ms: u32) {
        // 0 means "not started": a feed at boot time counts as 1 ms.
        self.last_ms.store(now_ms.max(1), Ordering::Relaxed);
    }

    /// Fed at least once and not for longer than its period.
    fn starving(&self, now_ms: u32) -> bool {
        let last = self.last_ms.load(Ordering::Relaxed);
        last != 0 && now_ms.wrapping_sub(last) > self.period_ms
    }

    #[cfg(feature = "_esp")]
    pub fn feed(&self) {
        self.feed_at(embassy_time::Instant::now().as_millis() as u32);
    }
}

/// The first watch in `watches` that is starving at `now_ms`. Watches never fed are skipped,
/// so a task can start late.
pub fn starving<'a>(watches: &[&'a TaskWatch], now_ms: u32) -> Option<&'a TaskWatch> {
    watches.iter().copied().find(|w| w.starving(now_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_starving_tasks() {
        let net = TaskWatch::new("net", 1000);
        let led = TaskWatch::new("led", 5000);
        let watches = [&net, &led];
        assert!(starving(&watches, 100_000).is_none(), "never fed: not watched yet");
        net.feed_at(10_000);
        led.feed_at(10_000);
        assert!(starving(&watches, 11_000).is_none());
        assert_eq!(starving(&watches, 11_001).map(TaskWatch::name), Some("net"));
        net.feed_at(15_000);
        assert_eq!(starving(&watches, 15_001).map(TaskWatch::name), Some("led"));
        // Fed at boot (0 ms) still counts.
        let early = TaskWatch::new("early", 10);
        early.feed_at(0);
        assert!(starving(&[&early], 100).is_some());
    }
}
