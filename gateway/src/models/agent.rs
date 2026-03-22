use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, FromRow, Serialize)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub url: String,
    pub version: String,
    pub provider: serde_json::Value,
    pub capabilities: serde_json::Value,
    pub default_input_modes: Vec<String>,
    pub default_output_modes: Vec<String>,
    pub skills: serde_json::Value,
    pub security: serde_json::Value,
    pub card_json: serde_json::Value,
    pub is_active: Option<bool>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// A2A Agent Card — accepts the full card JSON and extracts indexed fields.
#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Debug, Deserialize)]
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
