use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<RequestMessage>,
    #[serde(default)]
    pub stream: bool,
    /// Forward unknown fields (temperature, max_tokens, etc.) as-is to provider.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Request message — role is required per OpenAI spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMessage {
    pub role: String,
    pub content: String,
}

/// Response/delta message — all fields optional (SSE chunks send partial data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<DeltaMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<DeltaMessage>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct GatewayError {
    #[serde(skip)]
    pub status: StatusCode,
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
}

impl GatewayError {
    pub fn bad_request(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: ErrorBody {
                message: message.into(),
                error_type: error_type.into(),
            },
        }
    }

    pub fn provider_error(status: u16, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            error: ErrorBody {
                message: message.into(),
                error_type: "provider_error".into(),
            },
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> axum::response::Response {
        (self.status, axum::Json(&self)).into_response()
    }
}
