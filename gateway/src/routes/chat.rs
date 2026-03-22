use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::response::IntoResponse;

use crate::providers::{LlmProvider, ProviderError};
use crate::state::SharedState;
use crate::streaming::proxy::proxy_sse;
use crate::types::{ChatRequest, GatewayError};

pub async fn chat_completions(
    State(state): State<SharedState>,
    request: Result<Json<ChatRequest>, JsonRejection>,
) -> Result<impl IntoResponse, GatewayError> {
    let Json(request) =
        request.map_err(|e| GatewayError::bad_request("invalid_request", e.body_text()))?;

    let router = state.router();

    let provider = router.resolve(&request.model).await.ok_or_else(|| {
        GatewayError::bad_request(
            "invalid_model",
            format!(
                "model '{}' not found, available: {:?}",
                request.model,
                router.available_models()
            ),
        )
    })?;

    let result = execute_request(&state, &router, provider, &request).await;

    if let Err(ref e) = result
        && e.retryable
    {
        let failed_name = provider.name().to_string();
        tracing::warn!(
            provider = %failed_name,
            "primary provider failed, attempting failover"
        );

        if let Some(fallback) = router.failover(&request.model, &failed_name) {
            tracing::info!(
                provider = %fallback.name(),
                "failing over to alternate provider"
            );
            let fallback_result = execute_request(&state, &router, fallback, &request).await;
            if let Ok(response) = fallback_result {
                return Ok(response);
            }
        }
    }

    result.map_err(|e| GatewayError::provider_error(e.status, e.message))
}

async fn execute_request(
    state: &SharedState,
    router: &crate::routing::Router,
    provider: &dyn LlmProvider,
    request: &ChatRequest,
) -> Result<axum::response::Response, ProviderError> {
    let provider_name = provider.name().to_string();
    let model = request.model.clone();
    let start = Instant::now();

    tracing::info!(
        model = %model,
        provider = %provider_name,
        stream = request.stream,
        "routing request"
    );

    let provider_idx = router.provider_index(&provider_name);
    let cost_rate = provider_idx
        .map(|idx| router.cost_rate(idx))
        .unwrap_or_default();
    if let Some(idx) = provider_idx {
        router.acquire(idx);
    }

    let result: Result<axum::response::Response, ProviderError> = if request.stream {
        let ttft_timeout = state.config.routing.ttft_timeout_ms;

        if ttft_timeout > 0 {
            match tokio::time::timeout(
                Duration::from_millis(ttft_timeout),
                provider.chat_completion_stream(request),
            )
            .await
            {
                Ok(Ok(resp)) => Ok(proxy_sse(
                    resp,
                    provider_name.clone(),
                    model.clone(),
                    state.metrics.clone(),
                    cost_rate,
                )
                .into_response()),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(ProviderError {
                    status: 504,
                    message: format!("TTFT timeout ({ttft_timeout}ms) exceeded"),
                    retryable: true,
                }),
            }
        } else {
            let resp = provider.chat_completion_stream(request).await?;
            Ok(proxy_sse(
                resp,
                provider_name.clone(),
                model.clone(),
                state.metrics.clone(),
                cost_rate,
            )
            .into_response())
        }
    } else {
        let resp = provider.chat_completion(request).await?;
        let duration = start.elapsed();
        state
            .metrics
            .record_request(&provider_name, &model, 200, duration.as_secs_f64());

        // Record tokens and cost for JSON mode
        if let Some(ref usage) = resp.usage {
            state
                .metrics
                .record_tokens(&model, "input", u64::from(usage.prompt_tokens));
            state
                .metrics
                .record_tokens(&model, "output", u64::from(usage.completion_tokens));

            if let Some(idx) = provider_idx {
                let cost = router.compute_cost(idx, usage.prompt_tokens, usage.completion_tokens);
                if cost > 0.0 {
                    state.metrics.record_cost(&model, cost);
                }
            }
        }

        Ok(Json(resp).into_response())
    };

    if let Some(idx) = provider_idx {
        router.release(idx);
    }

    let duration_ms = start.elapsed().as_millis() as f64;

    match &result {
        Ok(_) => {
            router.health.record_success(&provider_name);
            if let Some(tracker) = &router.latency {
                tracker.record(&provider_name, duration_ms).await;
            }
        }
        Err(e) => {
            state.metrics.record_request(
                &provider_name,
                &model,
                e.status,
                start.elapsed().as_secs_f64(),
            );
            if e.retryable {
                router.health.record_failure(&provider_name);
            }
            tracing::error!(
                provider = %provider_name,
                status = e.status,
                error = %e.message,
                "provider error"
            );
        }
    }

    result
}
