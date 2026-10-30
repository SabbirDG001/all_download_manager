use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Download speed averaged over the last few seconds.
#[derive(Debug, Default, Clone)]
pub struct SpeedMeter {
    samples: VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    const WINDOW: Duration = Duration::from_secs(3);

    /// Records the total bytes downloaded so far and returns bytes per second.
    pub fn update(&mut self, downloaded: u64) -> f64 {
        let now = Instant::now();
        self.samples.push_back((now, downloaded));
        while self.samples.len() > 2 && now - self.samples[0].0 > Self::WINDOW {
            self.samples.pop_front();
        }
        let (t0, b0) = self.samples[0];
        let secs = (now - t0).as_secs_f64();
        if secs > 0.0 {
            downloaded.saturating_sub(b0) as f64 / secs
        } else {
            0.0
        }
    }
}
