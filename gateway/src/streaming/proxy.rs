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
        let mut errored = false;
        let mut last_usage: Option<crate::types::Usage> = None;

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

                        // Capture usage from the last chunk (OpenAI sends it with finish_reason)
                        if chunk.usage.is_some() {
                            last_usage = chunk.usage;
                        }
                    }

                    yield Ok(Event::default().data(data));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SSE parse error");
                    errored = true;
                    break;
                }
            }
        }

        metrics.finalize();
        let status = if errored { 502 } else { 200 };

        if let Some(ttft) = metrics.ttft() {
            otel.record_ttft(&provider_name, &model, ttft.as_secs_f64());
        }
        if let Some(tpot) = metrics.tpot() {
            otel.record_tpot(&provider_name, &model, tpot.as_secs_f64());
        }
        otel.record_request(&provider_name, &model, status, metrics.total_duration().as_secs_f64());

        // Record token usage from the final SSE chunk
        if let Some(usage) = &last_usage {
            otel.record_tokens(&model, "input", u64::from(usage.prompt_tokens));
            otel.record_tokens(&model, "output", u64::from(usage.completion_tokens));
        }

        tracing::info!(
            provider = %provider_name,
            model = %model,
            status,
            ttft_ms = ?metrics.ttft().map(|d| d.as_millis()),
            tpot_ms = ?metrics.tpot().map(|d| d.as_millis()),
            tokens = metrics.token_count(),
            input_tokens = ?last_usage.as_ref().map(|u| u.prompt_tokens),
            output_tokens = ?last_usage.as_ref().map(|u| u.completion_tokens),
            total_ms = metrics.total_duration().as_millis(),
            "stream completed"
        );
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}
