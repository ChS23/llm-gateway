use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use fred::interfaces::LuaInterface;

use crate::state::SharedState;

/// API key record from database.
#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    scopes: serde_json::Value,
    rate_limit_rpm: Option<i32>,
}

/// Auth middleware — validates `Authorization: Bearer sk-gw-...` header.
/// Checks key hash, expiration, active status, and scope permissions.
pub async fn auth_middleware(
    State(state): State<SharedState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();

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
            return auth_error("missing or invalid Authorization header");
        }
    };

    let key_hash = hex::encode(Sha256::digest(token.as_bytes()));

    let key_row = sqlx::query_as::<_, ApiKeyRow>(
        "SELECT scopes, rate_limit_rpm FROM api_keys \
         WHERE key_hash = $1 AND is_active = true \
         AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(&key_hash)
    .fetch_optional(&state.db)
    .await;

    let key = match key_row {
        Ok(Some(k)) => k,
        _ => return auth_error("invalid API key"),
    };

    // Scope check: /v1/* requires "chat", /admin/* requires "admin"
    let required_scope = if path.starts_with("/v1/") {
        "chat"
    } else if path.starts_with("/admin/") {
        "admin"
    } else {
        "chat"
    };

    let scopes: Vec<String> = serde_json::from_value(key.scopes).unwrap_or_default();
    if !scopes.iter().any(|s| s == required_scope) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": {
                    "message": format!("insufficient scope: requires '{required_scope}'"),
                    "type": "permission_error"
                }
            })),
        )
            .into_response();
    }

    // Per-key rate limiting via Redis sliding window (atomic Lua script)
    if let Some(rpm) = key.rate_limit_rpm
        && rpm > 0
        && let Some(ref tracker) = state.latency
    {
        let redis = tracker.redis();
        let rate_key = format!("gw:ratelimit:{key_hash}");
        let now = chrono::Utc::now().timestamp();
        let window_start = now - 60;

        // Atomic: ZREMRANGEBYSCORE + ZADD + EXPIRE + ZCARD in one round-trip
        let lua = r#"
            redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', ARGV[1])
            redis.call('ZADD', KEYS[1], ARGV[2], ARGV[3])
            redis.call('EXPIRE', KEYS[1], 120)
            return redis.call('ZCARD', KEYS[1])
        "#;

        let count: i64 = redis
            .eval(
                lua,
                vec![rate_key],
                vec![window_start.to_string(), now.to_string(), now.to_string()],
            )
            .await
            .unwrap_or(0);

        if count > i64::from(rpm) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                axum::Json(serde_json::json!({
                    "error": {
                        "message": format!("rate limit exceeded: {rpm} requests/minute"),
                        "type": "rate_limit_error"
                    }
                })),
            )
                .into_response();
        }
    }

    next.run(request).await
}

fn auth_error(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({
            "error": {
                "message": message,
                "type": "authentication_error"
            }
        })),
    )
        .into_response()
}
