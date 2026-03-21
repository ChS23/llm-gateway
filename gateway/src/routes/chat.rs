use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::state::SharedState;
use crate::streaming::proxy::proxy_sse;
use crate::types::{ChatRequest, GatewayError};

pub async fn chat_completions(
    State(state): State<SharedState>,
    Json(request): Json<ChatRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<GatewayError>)> {
    let provider = state.router.resolve(&request.model).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(GatewayError::new(
                "invalid_model",
                format!(
                    "model '{}' not found, available: {:?}",
                    request.model,
                    state.router.available_models()
                ),
            )),
        )
    })?;

    let provider_name = provider.name().to_string();
    let model = request.model.clone();

    tracing::info!(
        model = %model,
        provider = %provider_name,
        stream = request.stream,
        "routing request"
    );

    let map_err = |e: crate::providers::ProviderError| {
        tracing::error!(
            provider = %provider_name,
            status = e.status,
            error = %e.message,
            "provider error"
        );
        (
            StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(GatewayError::new("provider_error", e.message)),
        )
    };

    if request.stream {
        let response = provider
            .chat_completion_stream(&request)
            .await
            .map_err(map_err)?;
        Ok(proxy_sse(response, provider_name, model).into_response())
    } else {
        let response = provider.chat_completion(&request).await.map_err(map_err)?;
        Ok(Json(response).into_response())
    }
}
