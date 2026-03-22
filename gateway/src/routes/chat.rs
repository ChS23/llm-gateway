use std::time::Instant;

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

    let provider = state.router.resolve(&request.model).await.ok_or_else(|| {
        GatewayError::bad_request(
            "invalid_model",
            format!(
                "model '{}' not found, available: {:?}",
                request.model,
                state.router.available_models()
            ),
        )
    })?;

    let result = execute_request(&state, provider, &request).await;

    // On transient failure, try failover to a different provider
    if let Err(ref e) = result
        && e.retryable
    {
        let failed_name = provider.name().to_string();
        tracing::warn!(
            provider = %failed_name,
            "primary provider failed, attempting failover"
        );

        if let Some(fallback) = state.router.failover(&request.model, &failed_name) {
            tracing::info!(
                provider = %fallback.name(),
                "failing over to alternate provider"
            );
            let fallback_result = execute_request(&state, fallback, &request).await;
            if let Ok(response) = fallback_result {
                return Ok(response);
            }
        }
    }

    result.map_err(|e| GatewayError::provider_error(e.status, e.message))
}

async fn execute_request(
    state: &SharedState,
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

    let result: Result<axum::response::Response, ProviderError> = if request.stream {
        let resp = provider.chat_completion_stream(request).await?;
        Ok(proxy_sse(
            resp,
            provider_name.clone(),
            model.clone(),
            state.metrics.clone(),
        )
        .into_response())
    } else {
        let resp = provider.chat_completion(request).await?;
        let duration = start.elapsed();
        state
            .metrics
            .record_request(&provider_name, &model, 200, duration.as_secs_f64());
        Ok(Json(resp).into_response())
    };

    let duration_ms = start.elapsed().as_millis() as f64;

    // Record latency and health
    match &result {
        Ok(_) => {
            state.router.health.record_success(&provider_name);
            if let Some(tracker) = &state.router.latency {
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
                state.router.health.record_failure(&provider_name);
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
