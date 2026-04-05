use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

/// Registered LLM provider backend.
#[derive(Debug, FromRow, Serialize, ToSchema)]
#[allow(dead_code)]
pub struct Provider {
    /// Unique provider identifier.
    #[schema(example = "550e8400-e29b-41d4-a716-446655440000")]
    pub id: Uuid,
    /// Human-readable name (e.g. `openai-primary`).
    #[schema(example = "openai-primary")]
    pub name: String,
    /// Provider type: `openai`, `anthropic`, `mock`, `gemini`.
    #[schema(example = "openai")]
    pub provider_type: String,
    /// Base URL for the provider API.
    #[schema(example = "https://api.openai.com")]
    pub base_url: String,
    #[serde(skip)]
    #[schema(ignore)]
    pub api_key_encrypted: Option<Vec<u8>>,
    /// Supported model identifiers.
    pub models: serde_json::Value,
    /// Cost per input token in USD.
    #[schema(example = 0.00003)]
    pub cost_per_input_token: Option<f64>,
    /// Cost per output token in USD.
    #[schema(example = 0.00006)]
    pub cost_per_output_token: Option<f64>,
    /// Provider-level rate limit (requests per minute).
    pub rate_limit_rpm: Option<i32>,
    /// Routing priority (lower = higher priority).
    pub priority: Option<i32>,
    /// Weight for weighted round-robin routing.
    pub weight: Option<i32>,
    /// Whether this provider is active.
    pub is_active: Option<bool>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Request body to register a new provider.
#[derive(Debug, Deserialize, ToSchema)]
#[allow(dead_code)]
#[schema(example = json!({
    "name": "openai-primary",
    "provider_type": "openai",
    "base_url": "https://api.openai.com",
    "models": ["gpt-4", "gpt-4o"],
    "weight": 1
}))]
pub struct CreateProvider {
    /// Unique name for this provider.
    pub name: String,
    /// Provider type: `openai`, `anthropic`, `mock`, `gemini`.
    pub provider_type: String,
    /// Base URL for the provider API.
    pub base_url: String,
    /// Provider API key (stored encrypted, never returned).
    #[serde(default)]
    #[schema(write_only)]
    pub api_key: Option<String>,
    /// List of model identifiers this provider serves.
    pub models: Vec<String>,
    #[serde(default)]
    pub cost_per_input_token: Option<f64>,
    #[serde(default)]
    pub cost_per_output_token: Option<f64>,
    #[serde(default)]
    pub rate_limit_rpm: Option<i32>,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default = "default_weight")]
    pub weight: i32,
}

fn default_weight() -> i32 {
    1
}

/// Partial update for an existing provider. Only supplied fields are changed.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateProvider {
    pub base_url: Option<String>,
    pub models: Option<Vec<String>>,
    pub cost_per_input_token: Option<f64>,
    pub cost_per_output_token: Option<f64>,
    pub rate_limit_rpm: Option<i32>,
    pub priority: Option<i32>,
    pub weight: Option<i32>,
    pub is_active: Option<bool>,
}
