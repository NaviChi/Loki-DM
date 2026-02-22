use std::time::{Duration, Instant};

use tokio::sync::Mutex;

#[derive(Debug)]
struct RateState {
    available: f64,
    last_refill: Instant,
}

#[derive(Debug)]
pub struct RateLimiter {
    limit_bps: Option<u64>,
    state: Mutex<RateState>,
}

impl RateLimiter {
    #[must_use]
    pub fn new(limit_bps: Option<u64>) -> Self {
        Self {
            limit_bps,
            state: Mutex::new(RateState {
                available: limit_bps.map_or(f64::MAX, |v| v as f64),
                last_refill: Instant::now(),
            }),
        }
    }

    pub async fn acquire(&self, amount: usize) {
        let Some(limit_bps) = self.limit_bps else {
            return;
        };

        if amount == 0 {
            return;
        }

        let requested = amount as f64;
        let rate = limit_bps as f64;

        loop {
            let sleep_for = {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(state.last_refill);
                state.last_refill = now;

                let refill = elapsed.as_secs_f64() * rate;
                state.available = (state.available + refill).min(rate);

                if state.available >= requested {
                    state.available -= requested;
                    None
                } else {
                    let missing = requested - state.available;
                    state.available = 0.0;
                    Some(Duration::from_secs_f64(missing / rate))
                }
            };

            if let Some(delay) = sleep_for {
                tokio::time::sleep(delay).await;
            } else {
                return;
            }
        }
    }
}
