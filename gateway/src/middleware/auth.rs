use std::time::Instant;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use fred::prelude::*;
use sha2::{Digest, Sha256};

use crate::state::SharedState;

const L1_TTL_SECS: u64 = 10;
const L2_TTL_SECS: i64 = 60;
const REDIS_CACHE_PREFIX: &str = "gw:auth:";

/// Cached API key data.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CachedKey {
    scopes: Vec<String>,
    rate_limit_rpm: i32,
}

/// L1 entry with expiration timestamp.
struct L1Entry {
    key: CachedKey,
    expires_at: Instant,
}

/// DualCache: L1 in-memory (DashMap, 10s TTL) → L2 Redis (60s TTL) → Postgres.
/// L1 = zero-cost, no network. L2 = ~0.1ms. Postgres = ~2-5ms.
pub struct AuthCache {
    l1: DashMap<String, L1Entry>,
}

impl AuthCache {
    pub fn new() -> Self {
        Self { l1: DashMap::new() }
    }

    fn l1_get(&self, key_hash: &str) -> Option<CachedKey> {
        let entry = self.l1.get(key_hash)?;
        if entry.expires_at > Instant::now() {
            Some(entry.key.clone())
        } else {
            drop(entry);
            self.l1.remove(key_hash);
            None
        }
    }

    fn l1_set(&self, key_hash: &str, key: &CachedKey) {
        self.l1.insert(
            key_hash.to_string(),
            L1Entry {
                key: key.clone(),
                expires_at: Instant::now() + std::time::Duration::from_secs(L1_TTL_SECS),
            },
        );
    }

    pub fn invalidate(&self, key_hash: &str) {
        self.l1.remove(key_hash);
    }
}

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

    let key = match lookup_key(&state, &key_hash).await {
        Some(k) => k,
        None => return auth_error("invalid API key"),
    };

    // Scope check
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

    // Per-key rate limiting (Redis Lua, atomic)
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

/// DualCache lookup: L1 (in-memory) → L2 (Redis) → L3 (Postgres).
async fn lookup_key(state: &SharedState, key_hash: &str) -> Option<CachedKey> {
    // L1: in-memory DashMap (~0ns)
    if let Some(key) = state.auth_cache.l1_get(key_hash) {
        return Some(key);
    }

    // L2: Redis (~0.1ms)
    let redis_cache_key = format!("{REDIS_CACHE_PREFIX}{key_hash}");
    if let Some(ref tracker) = state.latency {
        let redis = tracker.redis();
        if let Ok(Some(json)) = redis.get::<Option<String>, _>(&redis_cache_key).await
            && let Ok(key) = serde_json::from_str::<CachedKey>(&json)
        {
            state.auth_cache.l1_set(key_hash, &key);
            return Some(key);
        }
    }

    // L3: Postgres (~2-5ms)
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

    // Backfill L1 + L2
    state.auth_cache.l1_set(key_hash, &cached);
    if let Some(ref tracker) = state.latency {
        let redis = tracker.redis();
        if let Ok(json) = serde_json::to_string(&cached) {
            let _: Result<(), _> = redis
                .set(
                    &redis_cache_key,
                    json,
                    Some(Expiration::EX(L2_TTL_SECS)),
                    None,
                    false,
                )
                .await;
        }
    }

    Some(cached)
}

/// Invalidate key from both L1 and L2 cache.
pub async fn invalidate_key_cache(state: &SharedState, key_hash: &str) {
    state.auth_cache.invalidate(key_hash);
    if let Some(ref tracker) = state.latency {
        let redis = tracker.redis();
        let cache_key = format!("{REDIS_CACHE_PREFIX}{key_hash}");
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
