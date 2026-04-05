use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

/// A2A Agent registered in the gateway.
#[derive(Debug, FromRow, Serialize, ToSchema)]
pub struct Agent {
    /// Unique agent identifier.
    pub id: Uuid,
    /// Human-readable agent name.
    #[schema(example = "code-review-agent")]
    pub name: String,
    /// Agent description.
    #[schema(example = "An agent that reviews pull requests")]
    pub description: String,
    /// Agent endpoint URL.
    #[schema(example = "https://agent.example.com")]
    pub url: String,
    /// Agent version.
    #[schema(example = "1.0.0")]
    pub version: String,
    /// A2A provider metadata.
    pub provider: serde_json::Value,
    /// A2A capabilities.
    pub capabilities: serde_json::Value,
    /// Default input content types.
    pub default_input_modes: Vec<String>,
    /// Default output content types.
    pub default_output_modes: Vec<String>,
    /// Agent skills per A2A spec.
    pub skills: serde_json::Value,
    /// Security definitions.
    pub security: serde_json::Value,
    /// Full A2A agent card JSON.
    pub card_json: serde_json::Value,
    pub is_active: Option<bool>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// A2A Agent Card — accepts the full card JSON and extracts indexed fields.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[schema(example = json!({
    "name": "code-review-agent",
    "description": "Reviews pull requests",
    "url": "https://agent.example.com",
    "skills": [{"id": "review", "name": "Code Review"}]
}))]
pub struct CreateAgent {
    pub name: String,
    pub description: String,
    pub url: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub provider: serde_json::Value,
    #[serde(default)]
    pub capabilities: serde_json::Value,
    #[serde(default = "default_modes")]
    pub default_input_modes: Vec<String>,
    #[serde(default = "default_modes")]
    pub default_output_modes: Vec<String>,
    pub skills: Vec<serde_json::Value>,
    #[serde(default)]
    pub security: serde_json::Value,
}

fn default_version() -> String {
    "1.0.0".into()
}

fn default_modes() -> Vec<String> {
    vec!["text".into()]
}

/// Partial update for an existing agent. Only supplied fields are changed.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateAgent {
    pub description: Option<String>,
    pub url: Option<String>,
    pub version: Option<String>,
    pub provider: Option<serde_json::Value>,
    pub capabilities: Option<serde_json::Value>,
    pub skills: Option<Vec<serde_json::Value>>,
    pub security: Option<serde_json::Value>,
    pub is_active: Option<bool>,
}
