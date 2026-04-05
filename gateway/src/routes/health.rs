use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use fred::prelude::*;
use utoipa::ToSchema;

use crate::state::SharedState;

/// Gateway health status.
#[derive(serde::Serialize, ToSchema)]
#[schema(example = json!({"status": "healthy", "postgres": "ok", "redis": "ok", "uptime_secs": 3600}))]
struct HealthResponse {
    /// Overall status: `healthy` or `degraded`.
    status: &'static str,
    /// Postgres connectivity: `ok` or `down`.
    postgres: &'static str,
    /// Redis connectivity: `ok`, `down`, or `not_configured`.
    redis: &'static str,
    /// Seconds since gateway started.
    uptime_secs: u64,
}

static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Liveness/readiness check — verifies gateway's own dependencies.
/// Returns 200 if all OK, 503 if degraded.
#[utoipa::path(
    get,
    path = "/health",
    tag = "Health",
    summary = "Gateway health check",
    description = "Checks Postgres and Redis connectivity. Returns 200 when healthy, 503 when degraded.",
    responses(
        (status = 200, description = "All dependencies healthy", body = HealthResponse),
        (status = 503, description = "One or more dependencies down", body = HealthResponse),
    )
)]
pub async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    let start = START_TIME.get_or_init(std::time::Instant::now);

    let pg = match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.db)
        .await
    {
        Ok(_) => "ok",
        Err(_) => "down",
    };

    let redis_status = if let Some(ref tracker) = state.latency {
        match tracker.redis().ping::<String>(None).await {
            Ok(_) => "ok",
            Err(_) => "down",
        }
    } else {
        "not_configured"
    };

    let all_ok = pg == "ok" && redis_status != "down";
    let status = if all_ok { "healthy" } else { "degraded" };
    let http_status = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        http_status,
        Json(HealthResponse {
            status,
            postgres: pg,
            redis: redis_status,
            uptime_secs: start.elapsed().as_secs(),
        }),
    )
}

/// Provider health — separate from gateway liveness.
#[utoipa::path(
    get,
    path = "/health/providers",
    tag = "Health",
    summary = "Provider health status",
    description = "Returns circuit breaker state for each registered provider: `healthy`, `circuit_open`, or `half_open`.",
    responses(
        (status = 200, description = "Provider health map", body = Object,
         example = json!({"openai-primary": "healthy", "anthropic-primary": "circuit_open"})),
    )
)]
pub async fn provider_health(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let router = state.router();
    let mut providers = serde_json::Map::new();

    for model in router.available_models() {
        if let Some(provider) = router.resolve(model).await {
            let name = provider.name().to_string();
            if providers.contains_key(&name) {
                continue;
            }
            let cb_state = router.health.state(&name);
            let status = match cb_state {
                crate::routing::health::CircuitState::Closed => "healthy",
                crate::routing::health::CircuitState::Open => "circuit_open",
                crate::routing::health::CircuitState::HalfOpen => "half_open",
            };
            providers.insert(name, serde_json::Value::String(status.into()));
        }
    }

    Json(serde_json::Value::Object(providers))
}
