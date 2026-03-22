use std::convert::Infallible;

use axum::response::sse::{Event, KeepAlive, Sse};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Response;

use super::metrics::StreamMetrics;
use crate::middleware::telemetry::Metrics;
use crate::types::ChatResponse;

pub fn proxy_sse(
    response: Response,
    provider_name: String,
    model: String,
    otel: Metrics,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        let mut metrics = StreamMetrics::new();
        let mut event_stream = response.bytes_stream().eventsource();

        while let Some(event_result) = event_stream.next().await {
            match event_result {
                Ok(event) => {
                    let data = event.data;

                    if data == "[DONE]" {
                        yield Ok(Event::default().data("[DONE]"));
                        break;
                    }

                    if let Ok(chunk) = serde_json::from_str::<ChatResponse>(&data) {
                        let has_content = chunk.choices.iter().any(|c| {
                            c.delta.as_ref().is_some_and(|d| {
                                d.content.as_deref().is_some_and(|s| !s.is_empty())
                            })
                        });

                        if has_content {
                            metrics.on_token();
                        }
                    }

                    yield Ok(Event::default().data(data));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SSE parse error");
                    break;
                }
            }
        }

        if let Some(ttft) = metrics.ttft() {
            otel.record_ttft(&provider_name, &model, ttft.as_secs_f64());
        }
        if let Some(tpot) = metrics.tpot() {
            otel.record_tpot(&provider_name, &model, tpot.as_secs_f64());
        }
        otel.record_request(&provider_name, &model, 200, metrics.total_duration().as_secs_f64());

        tracing::info!(
            provider = %provider_name,
            model = %model,
            ttft_ms = ?metrics.ttft().map(|d| d.as_millis()),
            tpot_ms = ?metrics.tpot().map(|d| d.as_millis()),
            tokens = metrics.token_count(),
            total_ms = metrics.total_duration().as_millis(),
            "stream completed"
        );
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}
