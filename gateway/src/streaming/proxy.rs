use std::convert::Infallible;

use axum::response::sse::{Event, KeepAlive, Sse};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Response;

use super::metrics::StreamMetrics;
use crate::types::ChatResponse;

/// Принимает HTTP response от провайдера (с SSE body),
/// парсит events, собирает метрики, re-emit клиенту.
///
/// Паттерн: reqwest bytes stream → eventsource parser → наш transform → axum Sse
pub fn proxy_sse(
    response: Response,
    provider_name: String,
    model: String,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        let mut metrics = StreamMetrics::new();

        // response.bytes_stream() — поток raw bytes от провайдера
        // .eventsource() — парсит SSE формат (data: ...\n\n) в типизированные events
        let mut event_stream = response.bytes_stream().eventsource();

        while let Some(event_result) = event_stream.next().await {
            match event_result {
                Ok(event) => {
                    let data = event.data;

                    if data == "[DONE]" {
                        yield Ok(Event::default().data("[DONE]"));
                        break;
                    }

                    // Пробуем распарсить chunk для метрик
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

        // Логируем метрики после завершения stream
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
