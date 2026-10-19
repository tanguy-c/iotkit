//! Retry delays: 1, 2, 4, 8, 16 then 30 s, until a success resets them.

/// Longest wait between attempts, in seconds.
pub const MAX_DELAY_S: u32 = 30;

#[derive(Clone, Copy, Debug, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    pub const fn new() -> Self {
        Self { failures: 0 }
    }

    /// An attempt failed: the seconds to wait before the next one.
    pub fn failed(&mut self) -> u32 {
        let delay = 1u32.checked_shl(self.failures).unwrap_or(u32::MAX).min(MAX_DELAY_S);
        self.failures = self.failures.saturating_add(1);
        delay
    }

    pub fn succeeded(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_then_caps_then_resets() {
        let mut b = Backoff::new();
        let delays: [u32; 8] = core::array::from_fn(|_| b.failed());
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30, 30]);
        for _ in 0..100 {
            assert_eq!(b.failed(), 30);
        }
        b.succeeded();
        assert_eq!(b.failed(), 1);
    }
}
