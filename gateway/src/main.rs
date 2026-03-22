mod config;
mod middleware;
mod models;
mod providers;
mod routes;
mod routing;
mod state;
mod streaming;
mod types;

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use sqlx::postgres::PgPoolOptions;
use tokio::signal;
use tracing_subscriber::EnvFilter;

use crate::config::{Config, ProviderConfig};
use crate::middleware::telemetry::{init_metrics, spawn_system_metrics};
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::gemini::GeminiProvider;
use crate::providers::mock::MockProvider;
use crate::providers::openai::OpenAiProvider;
use crate::providers::openai_responses::OpenAiResponsesProvider;
use crate::routes::admin;
use crate::routing::Router as LlmRouter;
use crate::state::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config_path = std::env::var("CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config/gateway.toml"));

    let config = Config::load(&config_path).expect("failed to load config");
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let body_limit = config.guardrails.max_request_size_bytes;

    let db = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .connect(&config.database.url)
        .await
        .expect("failed to connect to database");

    sqlx::migrate!("../migrations")
        .run(&db)
        .await
        .expect("failed to run migrations");

    tracing::info!("database connected, migrations applied");

    use fred::prelude::ClientLike;
    let latency_tracker = if !config.redis.url.is_empty() {
        let redis_config =
            fred::prelude::Config::from_url(&config.redis.url).expect("invalid redis URL");
        let redis = fred::prelude::Client::new(redis_config, None, None, None);
        redis.init().await.expect("failed to connect to Redis");
        tracing::info!("redis connected");
        Some(crate::routing::latency::LatencyTracker::new(redis))
    } else {
        tracing::info!("redis not configured, latency routing disabled");
        None
    };

    let health_tracker = crate::routing::health::HealthTracker::new(config.circuit_breaker.clone());

    let metrics = init_metrics(&config.telemetry);
    spawn_system_metrics(metrics.clone());

    // Build initial router from TOML config
    let providers = build_providers(&config);
    let weights = build_weights(&config);
    let initial_router = LlmRouter::new(
        providers,
        &weights,
        config.routing.default_strategy,
        health_tracker.clone(),
        latency_tracker.clone(),
    );

    tracing::info!(
        models = ?initial_router.available_models(),
        "loaded providers"
    );

    let state = Arc::new(AppState {
        config,
        router_swap: ArcSwap::new(Arc::new(initial_router)),
        metrics,
        db,
        health: health_tracker,
        latency: latency_tracker,
    });

    // Reload router from DB (merges TOML + DB providers)
    if let Err(e) = state.reload_router().await {
        tracing::warn!(error = %e, "initial router reload from DB failed, using TOML config");
    }

    // Authenticated API routes
    let api_routes = Router::new()
        .route("/v1/chat/completions", post(routes::chat::chat_completions))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::guardrails::guardrails_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::auth::auth_middleware,
        ));

    // Admin routes (no auth)
    let admin_routes = Router::new()
        .route("/admin/providers", post(admin::create_provider))
        .route("/admin/providers", get(admin::list_providers))
        .route("/admin/providers/{id}", get(admin::get_provider))
        .route("/admin/providers/{id}", put(admin::update_provider))
        .route("/admin/providers/{id}", delete(admin::delete_provider))
        .route("/admin/agents", post(admin::create_agent))
        .route("/admin/agents", get(admin::list_agents))
        .route("/admin/agents/{id}", get(admin::get_agent))
        .route("/admin/agents/{id}", put(admin::update_agent))
        .route("/admin/agents/{id}", delete(admin::delete_agent))
        .route(
            "/admin/agents/{id}/.well-known/agent-card.json",
            get(admin::get_agent_card),
        )
        .route("/admin/keys", post(admin::create_api_key))
        .route("/admin/keys", get(admin::list_api_keys))
        .route("/admin/keys/{id}", delete(admin::delete_api_key));

    let app = Router::new()
        .merge(api_routes)
        .merge(admin_routes)
        .route("/health", get(routes::health::health))
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!("gateway listening on {addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");

    tracing::info!("shutdown complete");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to listen for ctrl+c");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to listen for SIGTERM")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received ctrl+c"),
        () = terminate => tracing::info!("received SIGTERM"),
    }
}

fn build_weights(config: &Config) -> std::collections::HashMap<String, u32> {
    config
        .providers
        .iter()
        .filter(|p| p.weight > 0)
        .map(|p| (p.name.clone(), p.weight))
        .collect()
}

fn build_providers(config: &Config) -> Vec<Box<dyn providers::LlmProvider>> {
    config
        .providers
        .iter()
        .filter_map(|p| build_provider(p))
        .collect()
}

/// Build a single LlmProvider from config. Public for reuse in state.rs hot reload.
pub fn build_provider(p: &ProviderConfig) -> Option<Box<dyn providers::LlmProvider>> {
    match p.provider_type.as_str() {
        "mock" => Some(Box::new(MockProvider::new(
            p.name.clone(),
            p.base_url.clone(),
            p.models.clone(),
        ))),
        "openai" => {
            let api_key = p.api_key.as_ref()?;
            Some(Box::new(OpenAiProvider::new(
                p.name.clone(),
                p.base_url.clone(),
                api_key.clone(),
                p.models.clone(),
            )))
        }
        "anthropic" => {
            let api_key = p.api_key.as_ref()?;
            Some(Box::new(AnthropicProvider::new(
                p.name.clone(),
                p.base_url.clone(),
                api_key.clone(),
                p.models.clone(),
            )))
        }
        "openai-responses" => {
            let api_key = p.api_key.as_ref()?;
            Some(Box::new(OpenAiResponsesProvider::new(
                p.name.clone(),
                p.base_url.clone(),
                api_key.clone(),
                p.models.clone(),
            )))
        }
        "gemini" => {
            let api_key = p.api_key.as_ref()?;
            Some(Box::new(GeminiProvider::new(
                p.name.clone(),
                p.base_url.clone(),
                api_key.clone(),
                p.models.clone(),
            )))
        }
        other => {
            tracing::warn!(provider_type = %other, "unknown provider type, skipping");
            None
        }
    }
}
