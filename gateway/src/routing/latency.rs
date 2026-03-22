use fred::prelude::*;

const EMA_DECAY: f64 = 0.3;
const EMA_KEY_PREFIX: &str = "gw:latency:ema:";
const WINDOW_KEY_PREFIX: &str = "gw:latency:window:";
const WINDOW_SIZE_SECS: f64 = 300.0; // 5 min sliding window

/// Tracks per-provider latency in Redis using EMA + sliding window.
#[derive(Clone)]
pub struct LatencyTracker {
    redis: Client,
}

impl LatencyTracker {
    pub fn new(redis: Client) -> Self {
        Self { redis }
    }

    /// Record a latency observation for a provider.
    /// Updates both the sliding window (for percentiles) and the EMA (for routing).
    pub async fn record(&self, provider: &str, latency_ms: f64) {
        let ema_key = format!("{EMA_KEY_PREFIX}{provider}");
        let window_key = format!("{WINDOW_KEY_PREFIX}{provider}");
        let now = chrono::Utc::now().timestamp_millis() as f64;

        // Update EMA: new_ema = decay * observation + (1 - decay) * old_ema
        let current_ema: Option<f64> = self.redis.get(&ema_key).await.ok();
        let new_ema = match current_ema {
            Some(old) => EMA_DECAY * latency_ms + (1.0 - EMA_DECAY) * old,
            None => latency_ms,
        };
        let _: () = self
            .redis
            .set(&ema_key, new_ema, None, None, false)
            .await
            .unwrap_or(());

        // Add to sliding window (sorted set: score = timestamp, member = latency:timestamp)
        let member = format!("{latency_ms}:{now}");
        let _: () = self
            .redis
            .zadd(&window_key, None, None, false, false, (now, member))
            .await
            .unwrap_or(());

        // Trim old entries outside the window
        let cutoff = now - (WINDOW_SIZE_SECS * 1000.0);
        let _: () = self
            .redis
            .zremrangebyscore(&window_key, f64::NEG_INFINITY, cutoff)
            .await
            .unwrap_or(());
    }

    /// Get the EMA latency for a provider. Returns None if no data.
    pub async fn get_ema(&self, provider: &str) -> Option<f64> {
        let key = format!("{EMA_KEY_PREFIX}{provider}");
        self.redis.get(&key).await.ok()
    }

    /// Pick the provider with the lowest EMA from the given candidates.
    /// Falls back to first candidate if no latency data exists.
    pub async fn fastest(&self, providers: &[&str]) -> Option<String> {
        if providers.is_empty() {
            return None;
        }

        let mut best: Option<(String, f64)> = None;

        for &name in providers {
            if let Some(ema) = self.get_ema(name).await {
                match &best {
                    None => best = Some((name.to_string(), ema)),
                    Some((_, best_ema)) if ema < *best_ema => {
                        best = Some((name.to_string(), ema));
                    }
                    _ => {}
                }
            }
        }

        best.map(|(name, _)| name)
            .or_else(|| providers.first().map(|s| s.to_string()))
    }
}
