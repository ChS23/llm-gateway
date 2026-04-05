use axum::Json;
use utoipa::ToSchema;

use crate::types::GatewayError;

/// Embeddings request (OpenAI-compatible).
#[derive(Debug, serde::Deserialize, ToSchema)]
#[schema(example = json!({
    "model": "text-embedding-3-small",
    "input": "Hello world"
}))]
#[allow(dead_code)]
pub struct EmbeddingsRequest {
    pub model: String,
    pub input: serde_json::Value,
}

/// Placeholder: embeddings support is planned but not yet implemented.
#[utoipa::path(
    post,
    path = "/v1/embeddings",
    tag = "LLM Proxy",
    summary = "Create embeddings (not yet implemented)",
    description = "Planned endpoint for OpenAI-compatible embeddings. \
                   Currently returns 501 Not Implemented.",
    request_body(content = EmbeddingsRequest),
    responses(
        (status = 501, description = "Not yet implemented", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn create_embeddings(
    Json(_request): Json<EmbeddingsRequest>,
) -> Result<(), GatewayError> {
    Err(GatewayError::not_implemented(
        "embeddings endpoint is not yet implemented",
    ))
}
