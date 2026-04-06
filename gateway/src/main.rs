use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use sqlx::postgres::PgPoolOptions;
use tokio::signal;
use tracing_subscriber::EnvFilter;
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use gateway::build_provider;
use gateway::config::Config;
use gateway::middleware::telemetry::{init_metrics, spawn_system_metrics};
use gateway::routing::Router as LlmRouter;
use gateway::state::AppState;

// Re-import library modules so utoipa macros resolve unqualified paths like
// `routes::chat::chat_completions`, `types::ChatRequest`, `models::provider::Provider`.
use gateway::models;
use gateway::routes;
use gateway::types;

use routes::admin;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "LLM Gateway API",
        description = "Multi-provider LLM gateway — proxies OpenAI-compatible chat completion \
                       requests to multiple backends (OpenAI, Anthropic, Gemini, Mock) with \
                       SSE streaming, smart routing, failover, observability, guardrails, \
                       and an A2A agent registry.",
        version = "1.0.0",
        contact(name = "LLM Gateway", url = "https://github.com/chs/llm-gateway"),
        license(name = "MIT")
    ),
    servers(
        (url = "/", description = "Current server")
    ),
    tags(
        (name = "LLM Proxy", description = "OpenAI-compatible chat completion endpoint"),
        (name = "Health", description = "Gateway and provider health checks"),
        (name = "Providers", description = "CRUD for LLM provider backends"),
        (name = "Agents", description = "A2A agent registry (Agent Cards, skills, discovery)"),
        (name = "API Keys", description = "API key lifecycle management"),
    ),
    paths(
        routes::chat::chat_completions,
        routes::responses::create_response,
        routes::models::list_models,
        routes::embeddings::create_embeddings,
        routes::health::health,
        routes::health::provider_health,
        routes::admin::create_provider,
        routes::admin::list_providers,
        routes::admin::get_provider,
        routes::admin::update_provider,
        routes::admin::delete_provider,
        routes::admin::create_agent,
        routes::admin::list_agents,
        routes::admin::get_agent,
        routes::admin::get_agent_card,
        routes::admin::update_agent,
        routes::admin::delete_agent,
        routes::admin::create_api_key,
        routes::admin::list_api_keys,
        routes::admin::delete_api_key,
    ),
    components(
        schemas(
            types::ChatRequest,
            types::RequestMessage,
            types::DeltaMessage,
            types::ChatResponse,
            types::Choice,
            types::Usage,
            types::GatewayError,
            types::ErrorBody,
            models::provider::Provider,
            models::provider::CreateProvider,
            models::provider::UpdateProvider,
            models::agent::Agent,
            models::agent::CreateAgent,
            models::agent::UpdateAgent,
            routes::admin::CreateApiKey,
            routes::models::ModelsResponse,
            routes::models::ModelObject,
            routes::responses::ResponsesRequest,
            routes::responses::ResponsesInput,
            routes::responses::ResponsesMessage,
            routes::responses::ResponsesResponse,
            routes::responses::ResponsesOutput,
            routes::responses::ResponsesContent,
            routes::responses::ResponsesUsage,
            routes::embeddings::EmbeddingsRequest,
            routes::health::HealthResponse,
        )
    ),
    security(
        ("bearer" = [])
    ),
    modifiers(&SecurityAddon),
)]
struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer",
                utoipa::openapi::security::SecurityScheme::Http(
                    utoipa::openapi::security::Http::new(
                        utoipa::openapi::security::HttpAuthScheme::Bearer,
                    ),
                ),
            );
        }
    }
}

#[tokio::main]
async fn main() {
    let config_path = std::env::var("CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config/gateway.toml"));

    let config = Config::load(&config_path).expect("failed to load config");
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let body_limit = config.guardrails.max_request_size_bytes;

    // Init OTel metrics + traces, then set up tracing subscriber with OTel layer
    let metrics = init_metrics(&config.telemetry);
    // spawn_system_metrics called after state is built (needs SharedState for provider health)

    let otel_layer = tracing_opentelemetry::layer()
        .with_tracer(opentelemetry::global::tracer("llm-gateway"))
        .with_tracked_inactivity(false);

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .with(otel_layer)
        .init();

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
        Some(gateway::routing::latency::LatencyTracker::new(redis))
    } else {
        tracing::info!("redis not configured, latency routing disabled");
        None
    };

    let health_tracker =
        gateway::routing::health::HealthTracker::new(config.circuit_breaker.clone());

    // Build initial router from TOML config
    let providers = build_providers(&config);
    let weights = build_weights(&config);
    let costs = build_costs(&config);
    let initial_router = LlmRouter::new(
        providers,
        &weights,
        &costs,
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
        auth_cache: gateway::middleware::auth::AuthCache::new(),
    });

    // Bootstrap admin API key from env var (if set and not already in DB)
    if let Ok(admin_key) = std::env::var("ADMIN_API_KEY")
        && !admin_key.is_empty()
    {
        use sha2::Digest;
        let key_hash = hex::encode(sha2::Sha256::digest(admin_key.as_bytes()));
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_keys WHERE key_hash = $1)")
                .bind(&key_hash)
                .fetch_one(&state.db)
                .await
                .unwrap_or(false);

        if !exists {
            let key_prefix = &admin_key[..admin_key.len().min(12)];
            let scopes = serde_json::json!(["admin", "chat"]);
            let _ = sqlx::query(
                "INSERT INTO api_keys (key_prefix, key_hash, name, scopes, rate_limit_rpm) \
                     VALUES ($1, $2, 'bootstrap-admin', $3, 0)",
            )
            .bind(key_prefix)
            .bind(&key_hash)
            .bind(&scopes)
            .execute(&state.db)
            .await;
            tracing::info!("bootstrap admin API key inserted from ADMIN_API_KEY env var");
        }
    }

    // Reload router from DB (merges TOML + DB providers)
    if let Err(e) = state.reload_router().await {
        tracing::warn!(error = %e, "initial router reload from DB failed, using TOML config");
    }

    // Background metrics: CPU, memory, provider health
    spawn_system_metrics(state.clone());

    // Authenticated API routes
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

    // Admin routes (auth required — bootstrap via ADMIN_API_KEY env var)
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
        .route("/admin/keys/{id}", delete(admin::delete_api_key))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            gateway::middleware::auth::auth_middleware,
        ));

    let app = Router::new()
        .merge(api_routes)
        .merge(admin_routes)
        .route("/health", get(routes::health::health))
        .route("/health/providers", get(routes::health::provider_health))
        .merge(Scalar::with_url("/scalar", ApiDoc::openapi()))
        .route(
            "/openapi.json",
            get(|| async { axum::Json(ApiDoc::openapi()) }),
        )
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

fn build_costs(config: &Config) -> std::collections::HashMap<String, gateway::routing::CostRate> {
    config
        .providers
        .iter()
        .map(|p| {
            (
                p.name.clone(),
                gateway::routing::CostRate {
                    input: p.cost_per_input_token.unwrap_or(0.0),
                    output: p.cost_per_output_token.unwrap_or(0.0),
                },
            )
        })
        .collect()
}

fn build_providers(config: &Config) -> Vec<Box<dyn gateway::providers::LlmProvider>> {
    config
        .providers
        .iter()
        .filter_map(|p| build_provider(p))
        .collect()
}
