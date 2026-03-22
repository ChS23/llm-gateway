use std::sync::Arc;

use sqlx::PgPool;

use crate::config::Config;
use crate::middleware::telemetry::Metrics;
use crate::routing::Router;

pub type SharedState = Arc<AppState>;

pub struct AppState {
    #[allow(dead_code)]
    pub config: Config,
    pub router: Router,
    pub metrics: Metrics,
    pub db: PgPool,
}
