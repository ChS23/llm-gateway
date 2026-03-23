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

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            error: ErrorBody {
                message: message.into(),
                error_type: "not_found".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_request_deserialization() {
        let json = r#"{"model":"gpt-4","messages":[{"role":"user","content":"hello"}],"stream":true,"temperature":0.7}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-4");
        assert!(req.stream);
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert!(req.extra.contains_key("temperature"));
    }

    #[test]
    fn test_chat_request_defaults() {
        let json = r#"{"model":"x","messages":[{"role":"user","content":"y"}]}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert!(!req.stream);
        assert!(req.extra.is_empty());
    }

    #[test]
    fn test_chat_response_serialization() {
        let resp = ChatResponse {
            id: "test-1".into(),
            object: "chat.completion".into(),
            model: "gpt-4".into(),
            choices: vec![Choice {
                index: 0,
                message: Some(DeltaMessage {
                    role: Some("assistant".into()),
                    content: Some("hello".into()),
                }),
                delta: None,
                finish_reason: Some("stop".into()),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
            extra: serde_json::Map::new(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"id\":\"test-1\""));
        assert!(json.contains("\"total_tokens\":15"));
    }

    #[test]
    fn test_delta_message_skip_none() {
        let delta = DeltaMessage {
            role: None,
            content: Some("token".into()),
        };
        let json = serde_json::to_string(&delta).unwrap();
        assert!(!json.contains("role"));
        assert!(json.contains("\"content\":\"token\""));
    }

    #[test]
    fn test_gateway_error_bad_request() {
        let err = GatewayError::bad_request("test_type", "test message");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.error.error_type, "test_type");
    }

    #[test]
    fn test_gateway_error_not_found() {
        let err = GatewayError::not_found("missing");
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.error.error_type, "not_found");
    }

    #[test]
    fn test_gateway_error_provider_invalid_status() {
        // 99 is below 100, invalid for HTTP status
        let err = GatewayError::provider_error(99, "bad");
        assert_eq!(err.status, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn test_gateway_error_serialization() {
        let err = GatewayError::bad_request("inv", "msg");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"type\":\"inv\""));
        assert!(json.contains("\"message\":\"msg\""));
        assert!(!json.contains("status")); // status is skip
    }
}
