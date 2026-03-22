#![allow(dead_code)]

mod config;
mod middleware;
mod providers;
mod routes;
mod routing;
mod state;
mod streaming;
mod types;

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::middleware::telemetry::init_metrics;
use crate::providers::mock::MockProvider;
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
    });

    let app = Router::new()
        .route("/v1/chat/completions", post(routes::chat::chat_completions))
        .route("/health", get(routes::health::health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!("gateway listening on {addr}");

    axum::serve(listener, app).await.expect("server error");
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
            "openai" | "anthropic" => {
                tracing::warn!(provider = %p.name, "real providers not yet implemented, skipping");
                None
            }
            other => {
                tracing::warn!(provider_type = %other, "unknown provider type, skipping");
                None
            }
        })
        .collect()
}
