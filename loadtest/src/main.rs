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

/// Per-worker stats collected locally, merged at the end (lock-free hot path).
struct WorkerStats {
    total: u64,
    success: u64,
    errors: u64,
    latencies_us: Vec<u64>,
    ttft_us: Vec<u64>,
}

impl WorkerStats {
    fn new() -> Self {
        Self {
            total: 0,
            success: 0,
            errors: 0,
            latencies_us: Vec::new(),
            ttft_us: Vec::new(),
        }
    }
}

struct GlobalStats {
    total: AtomicU64,
    errors: AtomicU64,
}

impl GlobalStats {
    fn new() -> Self {
        Self {
            total: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
}

fn percentile(sorted: &[u64], p: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((p as f64 / 100.0 * sorted.len() as f64).ceil() as usize).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

fn report(all_workers: &[WorkerStats], elapsed: Duration) {
    let total: u64 = all_workers.iter().map(|w| w.total).sum();
    let success: u64 = all_workers.iter().map(|w| w.success).sum();
    let errors: u64 = all_workers.iter().map(|w| w.errors).sum();
    let rps = total as f64 / elapsed.as_secs_f64();

    let mut latencies: Vec<u64> = all_workers
        .iter()
        .flat_map(|w| w.latencies_us.iter().copied())
        .collect();
    latencies.sort_unstable();

    let mut ttfts: Vec<u64> = all_workers
        .iter()
        .flat_map(|w| w.ttft_us.iter().copied())
        .collect();
    ttfts.sort_unstable();

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

    if !ttfts.is_empty() {
        println!("\nTTFT (streaming):");
        println!("  p50:  {:.1}ms", percentile(&ttfts, 50) as f64 / 1000.0);
        println!("  p95:  {:.1}ms", percentile(&ttfts, 95) as f64 / 1000.0);
        println!(
            "  avg:  {:.1}ms",
            ttfts.iter().sum::<u64>() as f64 / ttfts.len() as f64 / 1000.0
        );
    }

    println!("=======================================\n");
}

// -- Workers ------------------------------------------------------------------

async fn json_request(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    stats: &mut WorkerStats,
    global: &GlobalStats,
) {
    let start = Instant::now();
    stats.total += 1;
    global.total.fetch_add(1, Ordering::Relaxed);

    let result = client.post(url).json(body).send().await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            // Consume body to measure full round-trip and return connection to pool
            let _ = resp.bytes().await;
            stats.success += 1;
        }
        _ => {
            stats.errors += 1;
            global.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    stats.latencies_us.push(start.elapsed().as_micros() as u64);
}

async fn stream_request(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    stats: &mut WorkerStats,
    global: &GlobalStats,
) {
    let start = Instant::now();
    stats.total += 1;
    global.total.fetch_add(1, Ordering::Relaxed);

    let result = client.post(url).json(body).send().await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            let mut event_stream = resp.bytes_stream().eventsource();
            let mut got_first = false;

            while let Some(event) = event_stream.next().await {
                match event {
                    Ok(ev) => {
                        if !got_first && ev.data != "[DONE]" {
                            stats.ttft_us.push(start.elapsed().as_micros() as u64);
                            got_first = true;
                        }
                        if ev.data == "[DONE]" {
                            break;
                        }
                    }
                    Err(_) => {
                        stats.errors += 1;
                        global.errors.fetch_add(1, Ordering::Relaxed);
                        stats.latencies_us.push(start.elapsed().as_micros() as u64);
                        return;
                    }
                }
            }

            stats.success += 1;
        }
        _ => {
            stats.errors += 1;
            global.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    stats.latencies_us.push(start.elapsed().as_micros() as u64);
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

    let test_start = Instant::now();
    let deadline = test_start + Duration::from_secs(config.duration_secs);
    let ramp_interval = if config.concurrency > 0 {
        Duration::from_secs(config.ramp_up_secs) / config.concurrency as u32
    } else {
        Duration::ZERO
    };

    // Shared client — single connection pool for all workers
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
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(config.concurrency as usize)
        .build()
        .unwrap();

    let global = Arc::new(GlobalStats::new());
    let mut handles = Vec::new();

    for i in 0..config.concurrency {
        let config = config.clone();
        let client = client.clone();
        let global = global.clone();
        let delay = ramp_interval * i as u32;

        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;

            let mut stats = WorkerStats::new();
            let url = format!("{}/v1/chat/completions", config.base_url);

            let body_json = serde_json::json!({
                "model": config.model,
                "messages": [{"role": "user", "content": "load test"}],
            });
            let body_stream = serde_json::json!({
                "model": config.model,
                "messages": [{"role": "user", "content": "load test"}],
                "stream": true,
            });

            while Instant::now() < deadline {
                if config.stream {
                    stream_request(&client, &url, &body_stream, &mut stats, &global).await;
                } else {
                    json_request(&client, &url, &body_json, &mut stats, &global).await;
                }
            }

            stats
        });

        handles.push(handle);
    }

    // Progress reporter
    let global_ref = global.clone();
    let reporter = tokio::spawn(async move {
        let start = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if Instant::now() >= deadline {
                break;
            }
            let total = global_ref.total.load(Ordering::Relaxed);
            let errors = global_ref.errors.load(Ordering::Relaxed);
            let elapsed = start.elapsed().as_secs();
            let rps = total as f64 / elapsed.max(1) as f64;
            println!("[{elapsed:>3}s] requests={total} errors={errors} rps={rps:.0}");
        }
    });

    // Collect per-worker stats
    let mut all_stats = Vec::new();
    for h in handles {
        if let Ok(stats) = h.await {
            all_stats.push(stats);
        }
    }
    reporter.abort();

    let actual_elapsed = test_start.elapsed();
    report(&all_stats, actual_elapsed);
}
