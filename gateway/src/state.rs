use std::sync::Arc;

use crate::config::Config;
use crate::middleware::telemetry::Metrics;
use crate::routing::Router;

pub type SharedState = Arc<AppState>;

pub struct AppState {
    #[allow(dead_code)] // Used in Phase 2/3 middleware
    pub config: Config,
    pub router: Router,
    pub metrics: Metrics,
}
