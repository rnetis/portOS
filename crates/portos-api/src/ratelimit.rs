//! In-memory token-bucket rate limiting (§6.3 brute-force protection).
//!
//! Per-process; horizontal deployments put a shared limiter at the edge.
//! Keys are client IPs (or `ip:scope` for stricter buckets).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    inner: Mutex<HashMap<String, Bucket>>,
    refill_per_sec: f64,
    burst: u32,
}

struct Bucket {
    tokens: f64,
    last: Instant,
    max: f64,
    refill: f64,
}

impl RateLimiter {
    #[must_use]
    pub fn new(refill_per_sec: f64, burst: u32) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            refill_per_sec,
            burst,
        }
    }

    /// Consume one token; `false` means the caller is over the limit.
    pub fn allow(&self, key: &str) -> bool {
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return true, // fail-open; availability beats strictness
        };
        let now = Instant::now();
        let bucket = g
            .entry(key.to_string())
            .and_modify(|b| {
                let elapsed = now.duration_since(b.last).as_secs_f64();
                b.tokens = (b.tokens + elapsed * b.refill).min(b.max);
                b.last = now;
            })
            .or_insert_with(|| Bucket {
                tokens: f64::from(self.burst),
                last: now,
                max: f64::from(self.burst),
                refill: self.refill_per_sec,
            });
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Drop stale buckets (called periodically by the janitor task).
    pub fn sweep(&self, older_than: Duration) {
        if let Ok(mut g) = self.inner.lock() {
            let now = Instant::now();
            g.retain(|_, b| now.duration_since(b.last) < older_than);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_limited() {
        let rl = RateLimiter::new(0.001, 2);
        assert!(rl.allow("ip1"));
        assert!(rl.allow("ip1"));
        assert!(!rl.allow("ip1"));
        // Different key unaffected.
        assert!(rl.allow("ip2"));
        rl.sweep(Duration::from_secs(0)); // retain nothing older than 0s → clears all
        assert!(rl.allow("ip1"));
    }
}
