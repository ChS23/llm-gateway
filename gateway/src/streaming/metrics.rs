use std::time::Instant;

/// Собирает метрики по ходу SSE stream.
/// Создаётся на каждый streaming request, живёт пока stream не завершится.
pub struct StreamMetrics {
    start: Instant,
    first_token_at: Option<Instant>,
    token_count: u32,
}

impl StreamMetrics {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            first_token_at: None,
            token_count: 0,
        }
    }

    pub fn on_token(&mut self) {
        if self.first_token_at.is_none() {
            self.first_token_at = Some(Instant::now());
        }
        self.token_count += 1;
    }

    /// Time To First Token — время от начала запроса до первого content-bearing chunk
    pub fn ttft(&self) -> Option<std::time::Duration> {
        self.first_token_at.map(|t| t.duration_since(self.start))
    }

    /// Time Per Output Token — среднее время на токен после первого
    /// (total_duration - ttft) / (tokens - 1)
    pub fn tpot(&self) -> Option<std::time::Duration> {
        if self.token_count <= 1 {
            return None;
        }
        let total = self.start.elapsed();
        let ttft = self.ttft()?;
        let generation_time = total - ttft;
        Some(generation_time / (self.token_count - 1))
    }

    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    pub fn total_duration(&self) -> std::time::Duration {
        self.start.elapsed()
    }
}
