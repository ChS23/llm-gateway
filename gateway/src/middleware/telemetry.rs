use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{MetricExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{
    Aggregation, Instrument, PeriodicReader, SdkMeterProvider, Stream,
};

use crate::config::TelemetryConfig;

#[derive(Clone)]
pub struct Metrics {
    pub requests_total: Counter<u64>,
    pub request_duration: Histogram<f64>,
    pub ttft: Histogram<f64>,
    pub tpot: Histogram<f64>,
    pub token_usage: Counter<u64>,
    pub request_cost: Counter<f64>,
    pub provider_healthy: Gauge<i64>,
    pub cpu_usage: Gauge<f64>,
    pub memory_usage: Gauge<u64>,
}

impl Metrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            requests_total: meter
                .u64_counter("llm_gateway.requests.total")
                .with_description("Total number of requests")
                .build(),
            request_duration: meter
                .f64_histogram("gen_ai.client.operation.duration")
                .with_description("Request duration in seconds")
                .with_unit("s")
                .build(),
            ttft: meter
                .f64_histogram("gen_ai.server.time_to_first_token")
                .with_description("Time to first token in seconds")
                .with_unit("s")
                .build(),
            tpot: meter
                .f64_histogram("gen_ai.server.time_per_output_token")
                .with_description("Time per output token in seconds")
                .with_unit("s")
                .build(),
            token_usage: meter
                .u64_counter("gen_ai.client.token.usage")
                .with_description("Token usage count")
                .build(),
            request_cost: meter
                .f64_counter("llm_gateway.request.cost")
                .with_description("Request cost in USD")
                .with_unit("USD")
                .build(),
            provider_healthy: meter
                .i64_gauge("llm_gateway.provider.healthy")
                .with_description("Provider health: 1 = healthy, 0 = unhealthy")
                .build(),
            cpu_usage: meter
                .f64_gauge("process.cpu.utilization")
                .with_description("Process CPU utilization (0.0 - 1.0)")
                .build(),
            memory_usage: meter
                .u64_gauge("process.memory.usage")
                .with_description("Process resident memory in bytes")
                .with_unit("By")
                .build(),
        }
    }

    pub fn record_request(&self, provider: &str, model: &str, status: u16, duration_secs: f64) {
        let attrs = [
            KeyValue::new("provider", provider.to_owned()),
            KeyValue::new("model", model.to_owned()),
            KeyValue::new("status", i64::from(status)),
        ];
        self.requests_total.add(1, &attrs);
        self.request_duration.record(duration_secs, &attrs);
    }

    pub fn record_ttft(&self, provider: &str, model: &str, seconds: f64) {
        let attrs = [
            KeyValue::new("provider", provider.to_owned()),
            KeyValue::new("model", model.to_owned()),
        ];
        self.ttft.record(seconds, &attrs);
    }

    pub fn record_tpot(&self, provider: &str, model: &str, seconds: f64) {
        let attrs = [
            KeyValue::new("provider", provider.to_owned()),
            KeyValue::new("model", model.to_owned()),
        ];
        self.tpot.record(seconds, &attrs);
    }

    #[allow(dead_code)]
    pub fn record_tokens(&self, model: &str, direction: &str, count: u64) {
        let attrs = [
            KeyValue::new("model", model.to_owned()),
            KeyValue::new("direction", direction.to_owned()),
        ];
        self.token_usage.add(count, &attrs);
    }

    #[allow(dead_code)]
    pub fn record_cost(&self, model: &str, cost: f64) {
        let attrs = [KeyValue::new("model", model.to_owned())];
        self.request_cost.add(cost, &attrs);
    }

    pub fn record_provider_health(&self, provider: &str, healthy: bool) {
        self.provider_healthy.record(
            if healthy { 1 } else { 0 },
            &[KeyValue::new("provider", provider.to_owned())],
        );
    }
}

pub fn init_metrics(config: &TelemetryConfig) -> Metrics {
    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .build();

    // Metrics exporter
    let metric_exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .build()
        .expect("failed to create OTLP metric exporter");

    let reader = PeriodicReader::builder(metric_exporter)
        .with_interval(std::time::Duration::from_secs(10))
        .build();

    // Fine-grained buckets for LLM latency metrics (seconds)
    // Default OTel buckets [0,5,10,25...] are too coarse for sub-second values
    let ttft_buckets = vec![0.01, 0.025, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0];
    let tpot_buckets = vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0];

    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_reader(reader)
        .with_view(move |inst: &Instrument| {
            if inst.name() == "gen_ai.server.time_to_first_token" {
                return Some(
                    Stream::builder()
                        .with_aggregation(Aggregation::ExplicitBucketHistogram {
                            boundaries: ttft_buckets.clone(),
                            record_min_max: true,
                        })
                        .build()
                        .unwrap(),
                );
            }
            if inst.name() == "gen_ai.server.time_per_output_token" {
                return Some(
                    Stream::builder()
                        .with_aggregation(Aggregation::ExplicitBucketHistogram {
                            boundaries: tpot_buckets.clone(),
                            record_min_max: true,
                        })
                        .build()
                        .unwrap(),
                );
            }
            None
        })
        .build();

    global::set_meter_provider(meter_provider);

    // Trace exporter → OTel Collector → Langfuse
    let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .build()
        .expect("failed to create OTLP trace exporter");

    let trace_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_resource(resource)
        .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
        .with_batch_exporter(trace_exporter)
        .build();

    global::set_tracer_provider(trace_provider);

    let meter = global::meter("llm-gateway");
    Metrics::new(&meter)
}

/// Background task: samples CPU/memory every 10s and records provider health every 15s.
pub fn spawn_system_metrics(state: crate::state::SharedState) {
    tokio::spawn(async move {
        use std::time::Duration;
        use sysinfo::{Pid, System};

        let pid = Pid::from_u32(std::process::id());
        let mut sys = System::new();
        sys.refresh_cpu_all();
        let num_cpus = sys.cpus().len().max(1) as f64;
        let mut health_tick: u32 = 0;

        loop {
            sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);

            if let Some(process) = sys.process(pid) {
                let cpu = process.cpu_usage() as f64 / 100.0 / num_cpus;
                let mem = process.memory();
                state.metrics.cpu_usage.record(cpu, &[]);
                state.metrics.memory_usage.record(mem, &[]);
            }

            // Record provider health every ~15s (every 1.5 ticks)
            health_tick += 1;
            if health_tick.is_multiple_of(2) {
                let router = state.router();
                for name in router.provider_names() {
                    let healthy = state.health.is_available(name);
                    state.metrics.record_provider_health(name, healthy);
                }
            }

            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}
