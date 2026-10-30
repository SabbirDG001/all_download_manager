//! A speed limit shared by all downloads.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Token bucket: every received chunk takes its size in tokens, and the
/// bucket refills at the limit. Changing the limit applies immediately.
#[derive(Debug)]
pub struct RateLimiter {
    /// Bytes per second; 0 means no limit.
    limit: AtomicU64,
    bucket: Mutex<Bucket>,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(0)
    }
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self { limit: AtomicU64::new(bytes_per_sec), bucket: Mutex::new(Bucket { tokens: 0.0, last: Instant::now() }) }
    }

    /// Sets the limit in bytes per second (0 turns it off).
    pub fn set(&self, bytes_per_sec: u64) {
        self.limit.store(bytes_per_sec, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }

    /// Waits until `bytes` may be taken.
    pub async fn take(&self, bytes: usize) {
        let limit = self.get();
        if limit == 0 {
            return;
        }
        let wait = {
            let mut b = self.bucket.lock().unwrap();
            let now = Instant::now();
            let refill = now.duration_since(b.last).as_secs_f64() * limit as f64;
            // Allow short bursts of a quarter second, no more.
            b.tokens = (b.tokens + refill).min(limit as f64 / 4.0);
            b.last = now;
            b.tokens -= bytes as f64;
            if b.tokens < 0.0 {
                Duration::from_secs_f64(-b.tokens / limit as f64)
            } else {
                Duration::ZERO
            }
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn keeps_to_the_limit() {
        let limiter = RateLimiter::new(200_000);
        let start = Instant::now();
        for _ in 0..20 {
            limiter.take(10_000).await;
        }
        // 200 KB at 200 KB/s: about a second.
        let secs = start.elapsed().as_secs_f64();
        assert!((0.8..1.4).contains(&secs), "{secs}");
    }

    #[tokio::test]
    async fn no_limit_means_no_waiting() {
        let limiter = RateLimiter::new(0);
        let start = Instant::now();
        limiter.take(100_000_000).await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
