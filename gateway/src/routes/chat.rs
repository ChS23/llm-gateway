use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::response::IntoResponse;

use crate::state::SharedState;
use crate::streaming::proxy::proxy_sse;
use crate::types::{ChatRequest, GatewayError};

pub async fn chat_completions(
    State(state): State<SharedState>,
    request: Result<Json<ChatRequest>, JsonRejection>,
) -> Result<impl IntoResponse, GatewayError> {
    let Json(request) =
        request.map_err(|e| GatewayError::bad_request("invalid_request", e.body_text()))?;

    let provider = state.router.resolve(&request.model).ok_or_else(|| {
        GatewayError::bad_request(
            "invalid_model",
            format!(
                "model '{}' not found, available: {:?}",
                request.model,
                state.router.available_models()
            ),
        )
    })?;

    let provider_name = provider.name().to_string();
    let model = request.model.clone();
    let start = Instant::now();

    tracing::info!(
        model = %model,
        provider = %provider_name,
        stream = request.stream,
        "routing request"
    );

    let map_err = |e: crate::providers::ProviderError| {
        let duration = start.elapsed().as_secs_f64();
        state
            .metrics
            .record_request(&provider_name, &model, e.status, duration);
        tracing::error!(
            provider = %provider_name,
            status = e.status,
            error = %e.message,
            "provider error"
        );
        GatewayError::provider_error(e.status, e.message)
    };

    if request.stream {
        let response = provider
            .chat_completion_stream(&request)
            .await
            .map_err(map_err)?;
        Ok(proxy_sse(response, provider_name, model, state.metrics.clone()).into_response())
    } else {
        let response = provider.chat_completion(&request).await.map_err(map_err)?;
        let duration = start.elapsed().as_secs_f64();
        state
            .metrics
            .record_request(&provider_name, &model, 200, duration);
        Ok(Json(response).into_response())
    }
}
