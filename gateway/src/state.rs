use std::sync::Arc;

use crate::config::Config;
use crate::middleware::telemetry::Metrics;
use crate::routing::Router;

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub config: Config,
    pub router: Router,
    pub metrics: Metrics,
}
