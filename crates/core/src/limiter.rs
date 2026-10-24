//! Token-bucket speed limiter shared by all connections of a download (SPEC-engine §9).

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(crate) struct Limiter {
    /// Bytes per second.
    rate: f64,
    /// (tokens, last refill). Tokens may go negative: that's "debt" the next
    /// readers wait off, which keeps the long-run rate exact.
    bucket: Mutex<(f64, Instant)>,
}

impl Limiter {
    pub(crate) fn new(rate: u64) -> Self {
        Self {
            rate: rate as f64,
            bucket: Mutex::new((0.0, Instant::now())),
        }
    }

    /// Wait until `n` bytes may pass.
    pub(crate) async fn acquire(&self, n: usize) {
        let wait = {
            let mut b = self.bucket.lock().unwrap();
            let now = Instant::now();
            // Refill, but allow at most a quarter second of burst.
            b.0 = (b.0 + self.rate * now.duration_since(b.1).as_secs_f64()).min(self.rate / 4.0);
            b.1 = now;
            b.0 -= n as f64;
            (b.0 < 0.0).then(|| Duration::from_secs_f64(-b.0 / self.rate))
        };
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
    }
}
