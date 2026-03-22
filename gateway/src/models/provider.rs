use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, FromRow, Serialize)]
#[allow(dead_code)]
pub struct Provider {
    pub id: Uuid,
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    #[serde(skip)]
    pub api_key_encrypted: Option<Vec<u8>>,
    pub models: serde_json::Value,
    pub cost_per_input_token: Option<f64>,
    pub cost_per_output_token: Option<f64>,
    pub rate_limit_rpm: Option<i32>,
    pub priority: Option<i32>,
    pub weight: Option<i32>,
    pub is_active: Option<bool>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct CreateProvider {
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
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

#[derive(Debug, Deserialize)]
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
