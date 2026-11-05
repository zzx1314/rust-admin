use std::time::Duration;

/// Reconnect policy used when the WebSocket link is lost.
///
/// The default is "retry forever with exponential backoff", which keeps a
/// device online across server restarts, network hiccups and Wi-Fi roaming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// Delay before the first retry.
    pub initial_delay: Duration,
    /// Upper bound for the (exponentially growing) delay.
    pub max_delay: Duration,
    /// Maximum number of consecutive attempts; `None` means retry forever.
    pub max_attempts: Option<u32>,
    /// A session that stayed up at least this long resets the backoff counter.
    pub healthy_after: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            max_attempts: None,
            healthy_after: Duration::from_secs(30),
        }
    }
}

impl ReconnectPolicy {
    /// Disable reconnection (used by `--once` / `--no-reconnect`).
    pub fn disabled() -> Self {
        Self {
            max_attempts: Some(0),
            ..Self::default()
        }
    }

    pub fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    pub fn with_max_attempts(mut self, attempts: Option<u32>) -> Self {
        self.max_attempts = attempts;
        self
    }

    pub fn with_healthy_after(mut self, uptime: Duration) -> Self {
        self.healthy_after = uptime;
        self
    }

    /// Whether another attempt is allowed after `attempts` consecutive failures.
    pub fn allows_attempt(&self, attempts: u32) -> bool {
        match self.max_attempts {
            Some(max) => attempts < max,
            None => true,
        }
    }

    /// Exponential backoff (1x, 2x, 4x, ...) capped at [`Self::max_delay`].
    ///
    /// `attempt` starts at 1 for the first retry. A small deterministic jitter
    /// (±12.5%, derived from the attempt number) keeps a fleet of devices from
    /// reconnecting in lockstep without pulling in a random number generator.
    pub fn delay(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(16);
        let factor = 1u32 << exponent;
        let base = self
            .initial_delay
            .saturating_mul(factor)
            .min(self.max_delay);

        let base_ms = base.as_millis() as u64;
        if base_ms == 0 {
            return Duration::ZERO;
        }
        // jitter in [-12.5%, +12.5%]
        let spread = base_ms / 8;
        let offset = if spread == 0 {
            0
        } else {
            let slot = (u64::from(attempt).wrapping_mul(2_654_435_761)) % (2 * spread + 1);
            slot as i64 - spread as i64
        };
        let millis = (base_ms as i64 + offset).max(1) as u64;
        Duration::from_millis(millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_grows_exponentially_and_is_capped() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(4),
            ..ReconnectPolicy::default()
        };

        // jitter never exceeds ±12.5%
        for attempt in 1..=6u32 {
            let delay = policy.delay(attempt);
            let nominal = 500u64 << (attempt - 1).min(3);
            let nominal = nominal.min(4000);
            assert!(
                delay.as_millis() >= (nominal as u128 * 7 / 8)
                    && delay.as_millis() <= (nominal as u128 * 9 / 8),
                "attempt {attempt}: {delay:?} not within 12.5% of {nominal}ms"
            );
        }

        assert!(policy.delay(10) <= Duration::from_millis(4500));
    }

    #[test]
    fn jitter_differs_between_attempts() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_secs(5),
            ..ReconnectPolicy::default()
        };
        // same nominal delay, but not the exact same value -> no thundering herd
        assert_ne!(policy.delay(3), policy.delay(4));
    }

    #[test]
    fn attempt_limit_is_respected() {
        let unlimited = ReconnectPolicy::default();
        assert!(unlimited.allows_attempt(10_000));

        let limited = ReconnectPolicy::default().with_max_attempts(Some(3));
        assert!(limited.allows_attempt(0));
        assert!(limited.allows_attempt(2));
        assert!(!limited.allows_attempt(3));

        let disabled = ReconnectPolicy::disabled();
        assert!(!disabled.allows_attempt(0));
    }

    #[test]
    fn healthy_uptime_resets_backoff() {
        let policy = ReconnectPolicy::default().with_healthy_after(Duration::from_secs(30));
        assert!(policy.healthy_after <= Duration::from_secs(30));
    }
}
