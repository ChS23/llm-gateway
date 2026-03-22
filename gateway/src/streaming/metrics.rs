use std::time::{Duration, Instant};

/// Collects timing metrics during an SSE stream.
/// Created per streaming request, lives until the stream completes.
pub struct StreamMetrics {
    start: Instant,
    first_token_at: Option<Instant>,
    token_count: u32,
    end: Option<Instant>,
}

impl StreamMetrics {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            first_token_at: None,
            token_count: 0,
            end: None,
        }
    }

    pub fn on_token(&mut self) {
        if self.first_token_at.is_none() {
            self.first_token_at = Some(Instant::now());
        }
        self.token_count += 1;
    }

    /// Snapshot the end time — all derived metrics are consistent after this.
    pub fn finalize(&mut self) {
        self.end = Some(Instant::now());
    }

    pub fn ttft(&self) -> Option<Duration> {
        self.first_token_at.map(|t| t.duration_since(self.start))
    }

    /// (total_duration - ttft) / (tokens - 1)
    pub fn tpot(&self) -> Option<Duration> {
        if self.token_count <= 1 {
            return None;
        }
        let total = self.total_duration();
        let ttft = self.ttft()?;
        let generation_time = total - ttft;
        Some(generation_time / (self.token_count - 1))
    }

    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    pub fn total_duration(&self) -> Duration {
        let end = self.end.unwrap_or_else(Instant::now);
        end.duration_since(self.start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_tokens() {
        let m = StreamMetrics::new();
        assert_eq!(m.token_count(), 0);
        assert!(m.ttft().is_none());
        assert!(m.tpot().is_none());
    }

    #[test]
    fn test_single_token() {
        let mut m = StreamMetrics::new();
        m.on_token();
        assert_eq!(m.token_count(), 1);
        assert!(m.ttft().is_some());
        // TPOT не определён при одном токене
        assert!(m.tpot().is_none());
    }

    #[test]
    fn test_multiple_tokens() {
        let mut m = StreamMetrics::new();
        m.on_token();
        std::thread::sleep(std::time::Duration::from_millis(10));
        m.on_token();
        m.on_token();
        m.finalize();

        assert_eq!(m.token_count(), 3);
        assert!(m.ttft().is_some());
        assert!(m.tpot().is_some());
        // TPOT должен быть > 0
        assert!(m.tpot().unwrap() > Duration::ZERO);
    }

    #[test]
    fn test_finalize_freezes_duration() {
        let mut m = StreamMetrics::new();
        m.on_token();
        m.finalize();
        let d1 = m.total_duration();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let d2 = m.total_duration();
        assert_eq!(d1, d2);
    }
}
