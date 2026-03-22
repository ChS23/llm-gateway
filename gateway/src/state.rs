use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use sqlx::PgPool;

use crate::config::Config;
use crate::middleware::telemetry::Metrics;
use crate::providers::LlmProvider;
use crate::providers::mock::MockProvider;
use crate::routing::Router;
use crate::routing::health::HealthTracker;
use crate::routing::latency::LatencyTracker;

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub config: Config,
    pub router_swap: ArcSwap<Router>,
    pub metrics: Metrics,
    pub db: PgPool,
    pub health: HealthTracker,
    pub latency: Option<LatencyTracker>,
}

impl AppState {
    /// Rebuild routing table from database providers + static TOML config.
    /// Called on startup and after every provider CRUD operation.
    pub async fn reload_router(&self) -> Result<(), String> {
        let db_providers = sqlx::query_as::<_, (String, String, String, serde_json::Value, Option<i32>)>(
            "SELECT name, provider_type, base_url, models, weight FROM providers WHERE is_active = true"
        )
        .fetch_all(&self.db)
        .await
        .map_err(|e| format!("failed to load providers from DB: {e}"))?;

        let mut providers: Vec<Box<dyn LlmProvider>> = Vec::new();
        let mut weights: HashMap<String, u32> = HashMap::new();

        // DB providers
        for (name, provider_type, base_url, models_json, weight) in &db_providers {
            let models: Vec<String> =
                serde_json::from_value(models_json.clone()).unwrap_or_default();
            let w = weight.unwrap_or(1) as u32;

            match provider_type.as_str() {
                "mock" => {
                    providers.push(Box::new(MockProvider::new(
                        name.clone(),
                        base_url.clone(),
                        models,
                    )));
                    weights.insert(name.clone(), w);
                }
                // DB-registered real providers would need stored API keys
                // For now, only mock providers can be dynamically added
                other => {
                    tracing::debug!(
                        provider_type = %other,
                        name = %name,
                        "skipping DB provider (only mock supported for dynamic registration)"
                    );
                }
            }
        }

        // Static TOML providers (always present)
        for p in &self.config.providers {
            // Skip if already loaded from DB (DB takes precedence)
            if providers.iter().any(|pr| pr.name() == p.name) {
                continue;
            }

            if let Some(provider) = crate::build_provider(p) {
                weights.insert(p.name.clone(), p.weight);
                providers.push(provider);
            }
        }

        let router = Router::new(
            providers,
            &weights,
            self.config.routing.default_strategy,
            self.health.clone(),
            self.latency.clone(),
        );

        tracing::info!(
            models = ?router.available_models(),
            db_providers = db_providers.len(),
            "router reloaded"
        );

        self.router_swap.store(Arc::new(router));
        Ok(())
    }

    /// Get current router snapshot (lock-free read).
    pub fn router(&self) -> arc_swap::Guard<Arc<Router>> {
        self.router_swap.load()
    }
}
