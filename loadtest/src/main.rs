use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use eventsource_stream::Eventsource;
use futures::StreamExt;

// -- Config -------------------------------------------------------------------

#[derive(Clone)]
struct Config {
    base_url: String,
    api_key: String,
    model: String,
    concurrency: u64,
    duration_secs: u64,
    stream: bool,
    ramp_up_secs: u64,
}

impl Config {
    fn from_env() -> Self {
        Self {
            base_url: env("GATEWAY_URL", "http://127.0.0.1:8080"),
            api_key: env("API_KEY", ""),
            model: env("MODEL", "mock-fast"),
            concurrency: env("CONCURRENCY", "50").parse().unwrap(),
            duration_secs: env("DURATION", "30").parse().unwrap(),
            stream: env("STREAM", "false") == "true",
            ramp_up_secs: env("RAMP_UP", "5").parse().unwrap(),
        }
    }
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// -- Stats --------------------------------------------------------------------

struct Stats {
    total: AtomicU64,
    success: AtomicU64,
    errors: AtomicU64,
    latencies_us: tokio::sync::Mutex<Vec<u64>>,
    ttft_us: tokio::sync::Mutex<Vec<u64>>,
    first_token_counts: AtomicU64,
}

impl Stats {
    fn new() -> Self {
        Self {
            total: AtomicU64::new(0),
            success: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            latencies_us: tokio::sync::Mutex::new(Vec::new()),
            ttft_us: tokio::sync::Mutex::new(Vec::new()),
            first_token_counts: AtomicU64::new(0),
        }
    }

    async fn record_latency(&self, d: Duration) {
        self.latencies_us.lock().await.push(d.as_micros() as u64);
    }

    async fn record_ttft(&self, d: Duration) {
        self.ttft_us.lock().await.push(d.as_micros() as u64);
        self.first_token_counts.fetch_add(1, Ordering::Relaxed);
    }

    async fn report(&self, elapsed: Duration) {
        let total = self.total.load(Ordering::Relaxed);
        let success = self.success.load(Ordering::Relaxed);
        let errors = self.errors.load(Ordering::Relaxed);
        let rps = total as f64 / elapsed.as_secs_f64();

        let mut latencies = self.latencies_us.lock().await;
        latencies.sort_unstable();

        println!("\n========== Load Test Results ==========");
        println!("Duration:     {:.1}s", elapsed.as_secs_f64());
        println!("Total:        {total}");
        println!("Success:      {success}");
        println!("Errors:       {errors}");
        println!(
            "Error rate:   {:.2}%",
            errors as f64 / total.max(1) as f64 * 100.0
        );
        println!("RPS:          {rps:.1}");

        if !latencies.is_empty() {
            println!("\nLatency:");
            println!(
                "  p50:  {:.1}ms",
                percentile(&latencies, 50) as f64 / 1000.0
            );
            println!(
                "  p95:  {:.1}ms",
                percentile(&latencies, 95) as f64 / 1000.0
            );
            println!(
                "  p99:  {:.1}ms",
                percentile(&latencies, 99) as f64 / 1000.0
            );
            println!(
                "  avg:  {:.1}ms",
                latencies.iter().sum::<u64>() as f64 / latencies.len() as f64 / 1000.0
            );
            println!("  min:  {:.1}ms", latencies[0] as f64 / 1000.0);
            println!(
                "  max:  {:.1}ms",
                latencies[latencies.len() - 1] as f64 / 1000.0
            );
        }

        let ttfts = self.ttft_us.lock().await;
        if !ttfts.is_empty() {
            let mut sorted = ttfts.clone();
            sorted.sort_unstable();
            println!("\nTTFT (streaming):");
            println!("  p50:  {:.1}ms", percentile(&sorted, 50) as f64 / 1000.0);
            println!("  p95:  {:.1}ms", percentile(&sorted, 95) as f64 / 1000.0);
            println!(
                "  avg:  {:.1}ms",
                sorted.iter().sum::<u64>() as f64 / sorted.len() as f64 / 1000.0
            );
        }

        println!("=======================================\n");
    }
}

fn percentile(sorted: &[u64], p: u64) -> u64 {
    let idx = (p as f64 / 100.0 * sorted.len() as f64) as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// -- Workers ------------------------------------------------------------------

async fn json_worker(client: &reqwest::Client, config: &Config, stats: &Stats) {
    let url = format!("{}/v1/chat/completions", config.base_url);
    let body = serde_json::json!({
        "model": config.model,
        "messages": [{"role": "user", "content": "load test"}],
    });

    let start = Instant::now();
    stats.total.fetch_add(1, Ordering::Relaxed);

    let result = client.post(&url).json(&body).send().await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            stats.success.fetch_add(1, Ordering::Relaxed);
            stats.record_latency(start.elapsed()).await;
        }
        _ => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            stats.record_latency(start.elapsed()).await;
        }
    }
}

async fn stream_worker(client: &reqwest::Client, config: &Config, stats: &Stats) {
    let url = format!("{}/v1/chat/completions", config.base_url);
    let body = serde_json::json!({
        "model": config.model,
        "messages": [{"role": "user", "content": "load test"}],
        "stream": true,
    });

    let start = Instant::now();
    stats.total.fetch_add(1, Ordering::Relaxed);

    let result = client.post(&url).json(&body).send().await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            let mut event_stream = resp.bytes_stream().eventsource();
            let mut got_first = false;

            while let Some(event) = event_stream.next().await {
                match event {
                    Ok(ev) => {
                        if !got_first && ev.data != "[DONE]" {
                            stats.record_ttft(start.elapsed()).await;
                            got_first = true;
                        }
                        if ev.data == "[DONE]" {
                            break;
                        }
                    }
                    Err(_) => {
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                }
            }

            stats.success.fetch_add(1, Ordering::Relaxed);
            stats.record_latency(start.elapsed()).await;
        }
        _ => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// -- Main ---------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let config = Config::from_env();

    println!("LLM Gateway Load Tester");
    println!("  target:      {}", config.base_url);
    println!("  model:       {}", config.model);
    println!("  concurrency: {}", config.concurrency);
    println!("  duration:    {}s", config.duration_secs);
    println!("  stream:      {}", config.stream);
    println!("  ramp-up:     {}s", config.ramp_up_secs);
    println!();

    let stats = Arc::new(Stats::new());
    let deadline = Instant::now() + Duration::from_secs(config.duration_secs);
    let ramp_interval = if config.concurrency > 0 {
        Duration::from_secs(config.ramp_up_secs) / config.concurrency as u32
    } else {
        Duration::ZERO
    };

    let mut handles = Vec::new();

    for i in 0..config.concurrency {
        let config = config.clone();
        let stats = stats.clone();

        // Stagger worker starts for ramp-up
        let delay = ramp_interval * i as u32;

        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;

            let mut headers = reqwest::header::HeaderMap::new();
            if !config.api_key.is_empty() {
                headers.insert(
                    reqwest::header::AUTHORIZATION,
                    format!("Bearer {}", config.api_key).parse().unwrap(),
                );
            }
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                "application/json".parse().unwrap(),
            );

            let client = reqwest::Client::builder()
                .default_headers(headers)
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap();

            while Instant::now() < deadline {
                if config.stream {
                    stream_worker(&client, &config, &stats).await;
                } else {
                    json_worker(&client, &config, &stats).await;
                }
            }
        });

        handles.push(handle);
    }

    // Progress reporter
    let stats_ref = stats.clone();
    let reporter = tokio::spawn(async move {
        let start = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if Instant::now() >= deadline {
                break;
            }
            let total = stats_ref.total.load(Ordering::Relaxed);
            let errors = stats_ref.errors.load(Ordering::Relaxed);
            let elapsed = start.elapsed().as_secs();
            let rps = total as f64 / elapsed.max(1) as f64;
            println!("[{elapsed:>3}s] requests={total} errors={errors} rps={rps:.0}");
        }
    });

    for h in handles {
        let _ = h.await;
    }
    reporter.abort();

    let total_elapsed = Duration::from_secs(config.duration_secs);
    stats.report(total_elapsed).await;
}
