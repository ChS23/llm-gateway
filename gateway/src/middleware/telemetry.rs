use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{MetricExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};

use crate::config::TelemetryConfig;

/// Все метрики gateway в одном месте.
/// Clone-safe — внутри Arc, можно шарить между handlers.
#[derive(Clone)]
pub struct Metrics {
    pub requests_total: Counter<u64>,
    pub request_duration: Histogram<f64>,
    pub ttft: Histogram<f64>,
    pub tpot: Histogram<f64>,
    pub token_usage: Counter<u64>,
    pub request_cost: Counter<f64>,
    pub provider_healthy: Counter<u64>,
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
                .u64_counter("llm_gateway.provider.healthy")
                .with_description("Provider health status")
                .build(),
        }
    }

    pub fn record_request(&self, provider: &str, model: &str, status: u16, duration_secs: f64) {
        let attrs = [
            KeyValue::new("provider", provider.to_string()),
            KeyValue::new("model", model.to_string()),
            KeyValue::new("status", i64::from(status)),
        ];
        self.requests_total.add(1, &attrs);
        self.request_duration.record(duration_secs, &attrs);
    }

    pub fn record_ttft(&self, provider: &str, model: &str, seconds: f64) {
        let attrs = [
            KeyValue::new("provider", provider.to_string()),
            KeyValue::new("model", model.to_string()),
        ];
        self.ttft.record(seconds, &attrs);
    }

    pub fn record_tpot(&self, provider: &str, model: &str, seconds: f64) {
        let attrs = [
            KeyValue::new("provider", provider.to_string()),
            KeyValue::new("model", model.to_string()),
        ];
        self.tpot.record(seconds, &attrs);
    }

    pub fn record_tokens(&self, model: &str, direction: &str, count: u64) {
        let attrs = [
            KeyValue::new("model", model.to_string()),
            KeyValue::new("direction", direction.to_string()),
        ];
        self.token_usage.add(count, &attrs);
    }

    pub fn record_cost(&self, model: &str, cost: f64) {
        let attrs = [KeyValue::new("model", model.to_string())];
        self.request_cost.add(cost, &attrs);
    }
}

pub fn init_metrics(config: &TelemetryConfig) -> Metrics {
    let exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .build()
        .expect("failed to create OTLP metric exporter");

    let reader = PeriodicReader::builder(exporter)
        .with_interval(std::time::Duration::from_secs(10))
        .build();

    let provider = SdkMeterProvider::builder()
        .with_resource(
            Resource::builder()
                .with_service_name(config.service_name.clone())
                .build(),
        )
        .with_reader(reader)
        .build();

    global::set_meter_provider(provider);

    let meter = global::meter("llm-gateway");
    Metrics::new(&meter)
}
