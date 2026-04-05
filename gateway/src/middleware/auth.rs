use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fred::prelude::*;
use sha2::{Digest, Sha256};

use crate::state::SharedState;

const KEY_CACHE_TTL_SECS: i64 = 60;
const KEY_CACHE_PREFIX: &str = "gw:auth:";

/// Cached API key data — serialized to Redis as JSON.
#[derive(serde::Serialize, serde::Deserialize)]
struct CachedKey {
    scopes: Vec<String>,
    rate_limit_rpm: i32,
}

/// DB row for API key lookup on cache miss.
#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    scopes: serde_json::Value,
    rate_limit_rpm: Option<i32>,
}

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
        _ => return auth_error("missing or invalid Authorization header"),
    };

    let key_hash = hex::encode(Sha256::digest(token.as_bytes()));

    // --- Key lookup: Redis cache → Postgres fallback ---
    let key = match lookup_key(&state, &key_hash).await {
        Some(k) => k,
        None => return auth_error("invalid API key"),
    };

    // --- Scope check ---
    let required_scope = if path.starts_with("/v1/") {
        "chat"
    } else if path.starts_with("/admin/") {
        "admin"
    } else {
        "chat"
    };

    if !key.scopes.iter().any(|s| s == required_scope) {
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

    // --- Per-key rate limiting (Redis Lua, atomic) ---
    if key.rate_limit_rpm > 0
        && let Some(ref tracker) = state.latency
    {
        let redis = tracker.redis();
        let rate_key = format!("gw:ratelimit:{key_hash}");
        let now = chrono::Utc::now().timestamp();
        let window_start = now - 60;

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

        if count > i64::from(key.rate_limit_rpm) {
            return (
                    StatusCode::TOO_MANY_REQUESTS,
                    axum::Json(serde_json::json!({
                        "error": {
                            "message": format!("rate limit exceeded: {} requests/minute", key.rate_limit_rpm),
                            "type": "rate_limit_error"
                        }
                    })),
                )
                    .into_response();
        }
    }

    next.run(request).await
}

/// Lookup API key: Redis cache first, Postgres on miss.
/// Cache hit = 0 DB queries. Cache miss = 1 SELECT + 1 Redis SET.
async fn lookup_key(state: &SharedState, key_hash: &str) -> Option<CachedKey> {
    let cache_key = format!("{KEY_CACHE_PREFIX}{key_hash}");

    // 1. Try Redis cache
    if let Some(ref tracker) = state.latency {
        let redis = tracker.redis();
        if let Ok(Some(json)) = redis.get::<Option<String>, _>(&cache_key).await
            && let Ok(key) = serde_json::from_str::<CachedKey>(&json)
        {
            return Some(key);
        }
    }

    // 2. Cache miss → Postgres
    let row = sqlx::query_as::<_, ApiKeyRow>(
        "SELECT scopes, rate_limit_rpm FROM api_keys \
         WHERE key_hash = $1 AND is_active = true \
         AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(key_hash)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()?;

    let scopes: Vec<String> = serde_json::from_value(row.scopes).unwrap_or_default();
    let cached = CachedKey {
        scopes,
        rate_limit_rpm: row.rate_limit_rpm.unwrap_or(0),
    };

    // 3. Write to Redis cache (TTL 60s, best-effort)
    if let Some(ref tracker) = state.latency {
        let redis = tracker.redis();
        if let Ok(json) = serde_json::to_string(&cached) {
            let _: Result<(), _> = redis
                .set(
                    &cache_key,
                    json,
                    Some(Expiration::EX(KEY_CACHE_TTL_SECS)),
                    None,
                    false,
                )
                .await;
        }
    }

    Some(cached)
}

/// Invalidate cached key (call on key delete/deactivate).
pub async fn invalidate_key_cache(state: &SharedState, key_hash: &str) {
    if let Some(ref tracker) = state.latency {
        let redis = tracker.redis();
        let cache_key = format!("{KEY_CACHE_PREFIX}{key_hash}");
        let _: Result<(), _> = redis.del(&cache_key).await;
    }
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
