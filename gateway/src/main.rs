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

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use sqlx::postgres::PgPoolOptions;
use tokio::signal;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::middleware::telemetry::init_metrics;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::mock::MockProvider;
use crate::providers::openai::OpenAiProvider;
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

    // Database pool
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

    let metrics = init_metrics(&config.telemetry);
    let providers = build_providers(&config);

    let weights: std::collections::HashMap<String, u32> = config
        .providers
        .iter()
        .filter(|p| p.weight > 0)
        .map(|p| (p.name.clone(), p.weight))
        .collect();

    let llm_router = LlmRouter::new(providers, &weights, config.routing.default_strategy);

    tracing::info!(
        models = ?llm_router.available_models(),
        "loaded providers"
    );

    let state = Arc::new(AppState {
        config,
        router: llm_router,
        metrics,
        db,
    });

    let app = Router::new()
        // LLM proxy
        .route("/v1/chat/completions", post(routes::chat::chat_completions))
        // Provider registry
        .route("/admin/providers", post(admin::create_provider))
        .route("/admin/providers", get(admin::list_providers))
        .route("/admin/providers/{id}", get(admin::get_provider))
        .route("/admin/providers/{id}", put(admin::update_provider))
        .route("/admin/providers/{id}", delete(admin::delete_provider))
        // Agent registry
        .route("/admin/agents", post(admin::create_agent))
        .route("/admin/agents", get(admin::list_agents))
        .route("/admin/agents/{id}", get(admin::get_agent))
        .route("/admin/agents/{id}", put(admin::update_agent))
        .route("/admin/agents/{id}", delete(admin::delete_agent))
        // A2A discovery
        .route(
            "/admin/agents/{id}/.well-known/agent-card.json",
            get(admin::get_agent_card),
        )
        // Health
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

fn build_providers(config: &Config) -> Vec<Box<dyn providers::LlmProvider>> {
    config
        .providers
        .iter()
        .filter_map(|p| match p.provider_type.as_str() {
            "mock" => Some(Box::new(MockProvider::new(
                p.name.clone(),
                p.base_url.clone(),
                p.models.clone(),
            )) as Box<dyn providers::LlmProvider>),
            "openai" => {
                let api_key = match &p.api_key {
                    Some(key) => key.clone(),
                    None => {
                        tracing::warn!(provider = %p.name, "openai provider missing api_key, skipping");
                        return None;
                    }
                };
                Some(Box::new(OpenAiProvider::new(
                    p.name.clone(),
                    p.base_url.clone(),
                    api_key,
                    p.models.clone(),
                )) as Box<dyn providers::LlmProvider>)
            }
            "anthropic" => {
                let api_key = match &p.api_key {
                    Some(key) => key.clone(),
                    None => {
                        tracing::warn!(provider = %p.name, "anthropic provider missing api_key, skipping");
                        return None;
                    }
                };
                Some(Box::new(AnthropicProvider::new(
                    p.name.clone(),
                    p.base_url.clone(),
                    api_key,
                    p.models.clone(),
                )) as Box<dyn providers::LlmProvider>)
            }
            other => {
                tracing::warn!(provider_type = %other, "unknown provider type, skipping");
                None
            }
        })
        .collect()
}
