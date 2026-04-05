use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use sqlx::PgPool;

use crate::config::Config;
use crate::middleware::auth::AuthCache;
use crate::middleware::telemetry::Metrics;
use crate::providers::LlmProvider;
use crate::providers::mock::MockProvider;
use crate::routing::health::HealthTracker;
use crate::routing::latency::LatencyTracker;
use crate::routing::{CostRate, Router};

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub config: Config,
    pub router_swap: ArcSwap<Router>,
    pub metrics: Metrics,
    pub db: PgPool,
    pub health: HealthTracker,
    pub latency: Option<LatencyTracker>,
    pub auth_cache: AuthCache,
}

/// DB provider row — all fields needed for routing + cost.
#[derive(sqlx::FromRow)]
struct DbProvider {
    name: String,
    provider_type: String,
    base_url: String,
    models: serde_json::Value,
    weight: Option<i32>,
    cost_per_input_token: Option<f64>,
    cost_per_output_token: Option<f64>,
}

impl AppState {
    pub async fn reload_router(&self) -> Result<(), String> {
        let db_providers = sqlx::query_as::<_, DbProvider>(
            "SELECT name, provider_type, base_url, models, weight, \
             cost_per_input_token, cost_per_output_token \
             FROM providers WHERE is_active = true",
        )
        .fetch_all(&self.db)
        .await
        .map_err(|e| format!("failed to load providers from DB: {e}"))?;

        let mut providers: Vec<Box<dyn LlmProvider>> = Vec::new();
        let mut weights: HashMap<String, u32> = HashMap::new();
        let mut costs: HashMap<String, CostRate> = HashMap::new();

        for row in &db_providers {
            let models: Vec<String> =
                serde_json::from_value(row.models.clone()).unwrap_or_default();

            match row.provider_type.as_str() {
                "mock" => {
                    providers.push(Box::new(MockProvider::new(
                        row.name.clone(),
                        row.base_url.clone(),
                        models,
                    )));
                }
                other => {
                    tracing::warn!(
                        provider_type = %other,
                        name = %row.name,
                        "skipping DB provider: non-mock providers require API keys configured \
                         in gateway.toml (not stored in DB for security)"
                    );
                    continue;
                }
            }

            weights.insert(row.name.clone(), row.weight.unwrap_or(1) as u32);
            costs.insert(
                row.name.clone(),
                CostRate {
                    input: row.cost_per_input_token.unwrap_or(0.0),
                    output: row.cost_per_output_token.unwrap_or(0.0),
                },
            );
        }

        // Static TOML providers (DB takes precedence)
        for p in &self.config.providers {
            if providers.iter().any(|pr| pr.name() == p.name) {
                continue;
            }
            if let Some(provider) = crate::build_provider(p) {
                weights.insert(p.name.clone(), p.weight);
                costs.insert(
                    p.name.clone(),
                    CostRate {
                        input: p.cost_per_input_token.unwrap_or(0.0),
                        output: p.cost_per_output_token.unwrap_or(0.0),
                    },
                );
                providers.push(provider);
            }
        }

        let router = Router::new(
            providers,
            &weights,
            &costs,
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

    pub fn router(&self) -> arc_swap::Guard<Arc<Router>> {
        self.router_swap.load()
    }
}
