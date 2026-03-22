use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::state::SharedState;

/// Auth middleware — validates `Authorization: Bearer sk-gw-...` header.
/// Looks up sha256(key) in api_keys table.
/// Skips auth for health endpoint.
pub async fn auth_middleware(
    State(state): State<SharedState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();

    // Skip auth for health and metrics
    if path == "/health" {
        return next.run(request).await;
    }

    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({
                    "error": {
                        "message": "missing or invalid Authorization header",
                        "type": "authentication_error"
                    }
                })),
            )
                .into_response();
        }
    };

    // Hash the token and look up in DB
    let key_hash = hex::encode(Sha256::digest(token.as_bytes()));

    let key_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM api_keys WHERE key_hash = $1 AND is_active = true AND (expires_at IS NULL OR expires_at > now()))",
    )
    .bind(&key_hash)
    .fetch_one(&state.db)
    .await;

    match key_exists {
        Ok(true) => next.run(request).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "error": {
                    "message": "invalid API key",
                    "type": "authentication_error"
                }
            })),
        )
            .into_response(),
    }
}
