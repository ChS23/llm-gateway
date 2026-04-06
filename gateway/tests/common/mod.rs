use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use axum_test::TestServer;

use gateway::config::*;
use gateway::middleware::auth::AuthCache;
use gateway::middleware::telemetry::Metrics;
use gateway::providers::mock::MockProvider;
use gateway::routing::health::HealthTracker;
use gateway::routing::{CostRate, Router as LlmRouter};
use gateway::state::AppState;
use gateway::{routes, types};

const PG_URL: &str = "postgres://postgres:postgres@127.0.0.1:5432/llm_gateway_test";
const ADMIN_KEY: &str = "test-admin-key";

/// Try to build a full test app with Postgres.
/// Returns None if Postgres is unavailable.
pub async fn try_build_app() -> Option<TestServer> {
    // Set env vars for crypto + admin bootstrap
    unsafe {
        std::env::set_var("ADMIN_API_KEY", ADMIN_KEY);
        std::env::set_var("ENCRYPTION_KEY", "dGVzdGtleXRlc3RrZXl0ZXN0a2V5dGVzdGtleXM="); // 32 bytes base64
    }

    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(PG_URL)
        .await
        .ok()?;

    sqlx::migrate!("../migrations").run(&db).await.ok()?;

    // No-op OTel meter
    let meter = opentelemetry::global::meter("test");
    let metrics = Metrics::new(&meter);

    let health = HealthTracker::new(CircuitBreakerConfig::default());

    // Mock provider for testing
    let mock = Box::new(MockProvider::new(
        "test-mock".into(),
        "http://127.0.0.1:19001".into(), // unlikely to be running
        vec!["test-model".into()],
    )) as Box<dyn gateway::providers::LlmProvider>;

    let router = LlmRouter::new(
        vec![mock],
        &HashMap::from([("test-mock".into(), 1u32)]),
        &HashMap::from([(
            "test-mock".into(),
            CostRate {
                input: 0.001,
                output: 0.002,
            },
        )]),
        RoutingStrategy::RoundRobin,
        health.clone(),
        None,
    );

    // Bootstrap admin key
    {
        use sha2::Digest;
        let key_hash = hex::encode(sha2::Sha256::digest(ADMIN_KEY.as_bytes()));
        let _ = sqlx::query(
            "INSERT INTO api_keys (key_prefix, key_hash, name, scopes, rate_limit_rpm) \
             VALUES ($1, $2, 'test-admin', $3, 0) \
             ON CONFLICT (key_hash) DO NOTHING",
        )
        .bind(&ADMIN_KEY[..12.min(ADMIN_KEY.len())])
        .bind(&key_hash)
        .bind(serde_json::json!(["admin", "chat"]))
        .execute(&db)
        .await
        .ok()?;
    }

    let state = Arc::new(AppState {
        config: Config {
            server: ServerConfig {
                host: "127.0.0.1".into(),
                port: 0,
            },
            database: DatabaseConfig::default(),
            redis: RedisConfig::default(),
            telemetry: TelemetryConfig {
                otlp_endpoint: "http://localhost:4317".into(),
                service_name: "test".into(),
            },
            auth: AuthConfig::default(),
            routing: RoutingConfig::default(),
            circuit_breaker: CircuitBreakerConfig::default(),
            guardrails: GuardrailsConfig::default(),
            providers: vec![],
        },
        router_swap: ArcSwap::new(Arc::new(router)),
        metrics,
        db,
        health,
        latency: None,
        auth_cache: AuthCache::new(),
    });

    let api_routes = Router::new()
        .route("/v1/chat/completions", post(routes::chat::chat_completions))
        .route("/v1/responses", post(routes::responses::create_response))
        .route("/v1/models", get(routes::models::list_models))
        .route(
            "/v1/embeddings",
            post(routes::embeddings::create_embeddings),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            gateway::middleware::guardrails::guardrails_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            gateway::middleware::auth::auth_middleware,
        ));

    let admin_routes = Router::new()
        .route("/admin/providers", post(routes::admin::create_provider))
        .route("/admin/providers", get(routes::admin::list_providers))
        .route("/admin/providers/{id}", get(routes::admin::get_provider))
        .route("/admin/providers/{id}", put(routes::admin::update_provider))
        .route(
            "/admin/providers/{id}",
            delete(routes::admin::delete_provider),
        )
        .route("/admin/agents", post(routes::admin::create_agent))
        .route("/admin/agents", get(routes::admin::list_agents))
        .route("/admin/agents/{id}", get(routes::admin::get_agent))
        .route("/admin/agents/{id}", put(routes::admin::update_agent))
        .route("/admin/agents/{id}", delete(routes::admin::delete_agent))
        .route(
            "/admin/agents/{id}/.well-known/agent-card.json",
            get(routes::admin::get_agent_card),
        )
        .route("/admin/keys", post(routes::admin::create_api_key))
        .route("/admin/keys", get(routes::admin::list_api_keys))
        .route("/admin/keys/{id}", delete(routes::admin::delete_api_key))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            gateway::middleware::auth::auth_middleware,
        ));

    let app = Router::new()
        .merge(api_routes)
        .merge(admin_routes)
        .route("/health", get(routes::health::health))
        .route("/health/providers", get(routes::health::provider_health))
        .route(
            "/openapi.json",
            get(|| async { axum::Json(serde_json::json!({"openapi": "3.1.0"})) }),
        )
        .layer(DefaultBodyLimit::max(1_048_576))
        .with_state(state);

    Some(TestServer::new(app))
}

/// Create an API key and return the raw key string.
pub async fn create_key(server: &TestServer, scopes: &[&str]) -> String {
    let name = format!("test-{}", uuid::Uuid::new_v4());
    let resp = server
        .post("/admin/keys")
        .json(&serde_json::json!({
            "name": name,
            "scopes": scopes,
            "rate_limit_rpm": 100000
        }))
        .authorization_bearer(ADMIN_KEY)
        .await;
    resp.json::<serde_json::Value>()["key"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Macro to skip test if infra unavailable.
#[macro_export]
macro_rules! require_server {
    () => {
        match common::try_build_app().await {
            Some(s) => s,
            None => {
                eprintln!("SKIP: Postgres unavailable at {}", "127.0.0.1:5432");
                return;
            }
        }
    };
}
