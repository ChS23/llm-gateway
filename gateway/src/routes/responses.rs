use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::response::IntoResponse;
use utoipa::ToSchema;

use crate::state::SharedState;
use crate::types::{ChatRequest, GatewayError, RequestMessage};

/// OpenAI Responses API request format.
#[derive(Debug, serde::Deserialize, ToSchema)]
#[schema(example = json!({
    "model": "gpt-4o",
    "input": "What is Rust?"
}))]
#[allow(dead_code)]
pub struct ResponsesRequest {
    /// Model identifier.
    #[schema(example = "gpt-4o")]
    pub model: String,
    /// Input — either a string or an array of messages.
    pub input: ResponsesInput,
    /// Enable streaming.
    #[serde(default)]
    pub stream: bool,
    /// Extra fields forwarded to provider.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Input can be a plain string or structured messages.
#[derive(Debug, serde::Deserialize, ToSchema)]
#[serde(untagged)]
pub enum ResponsesInput {
    /// Plain text input.
    Text(String),
    /// Structured message array.
    Messages(Vec<ResponsesMessage>),
}

#[derive(Debug, serde::Deserialize, ToSchema)]
pub struct ResponsesMessage {
    pub role: String,
    pub content: String,
}

/// Responses API response (translated to OpenAI-compatible format).
#[derive(Debug, serde::Serialize, ToSchema)]
pub struct ResponsesResponse {
    pub id: String,
    pub object: &'static str,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_text: Option<String>,
    pub output: Vec<ResponsesOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponsesUsage>,
}

#[derive(Debug, serde::Serialize, ToSchema)]
pub struct ResponsesOutput {
    #[serde(rename = "type")]
    pub output_type: &'static str,
    pub role: String,
    pub content: Vec<ResponsesContent>,
}

#[derive(Debug, serde::Serialize, ToSchema)]
pub struct ResponsesContent {
    #[serde(rename = "type")]
    pub content_type: &'static str,
    pub text: String,
}

#[derive(Debug, serde::Serialize, ToSchema)]
pub struct ResponsesUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

/// Send a request using the OpenAI Responses API format.
///
/// Accepts `input` as string or messages array, translates internally
/// to chat completions, routes to the best provider, and returns
/// a Responses-format response.
#[utoipa::path(
    post,
    path = "/v1/responses",
    tag = "LLM Proxy",
    summary = "Responses API (OpenAI-compatible)",
    description = "Accepts the OpenAI Responses API format (`input` field). \
                   Translates to chat completions internally and routes to the best provider.",
    request_body(content = ResponsesRequest, description = "Responses API request"),
    responses(
        (status = 200, description = "Successful response", body = ResponsesResponse),
        (status = 400, description = "Invalid request", body = GatewayError),
        (status = 401, description = "Missing or invalid API key", body = GatewayError),
        (status = 502, description = "Provider error", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn create_response(
    State(state): State<SharedState>,
    request: Result<Json<ResponsesRequest>, JsonRejection>,
) -> Result<impl IntoResponse, GatewayError> {
    let Json(req) =
        request.map_err(|e| GatewayError::bad_request("invalid_request", e.body_text()))?;

    // Convert Responses API format → ChatRequest
    let messages = match req.input {
        ResponsesInput::Text(text) => vec![RequestMessage {
            role: "user".into(),
            content: text,
        }],
        ResponsesInput::Messages(msgs) => msgs
            .into_iter()
            .map(|m| RequestMessage {
                role: m.role,
                content: m.content,
            })
            .collect(),
    };

    let chat_request = ChatRequest {
        model: req.model,
        messages,
        stream: false, // Responses API non-streaming for now
        extra: req.extra,
    };

    // Reuse chat completions logic
    let router = state.router();
    let provider = router.resolve(&chat_request.model).await.ok_or_else(|| {
        GatewayError::bad_request(
            "invalid_model",
            format!(
                "model '{}' not found, available: {:?}",
                chat_request.model,
                router.available_models()
            ),
        )
    })?;

    let resp = provider
        .chat_completion(&chat_request)
        .await
        .map_err(|e| GatewayError::provider_error(e.status, e.message))?;

    // Convert ChatResponse → ResponsesResponse
    let output_text = resp
        .choices
        .first()
        .and_then(|c| c.message.as_ref())
        .and_then(|m| m.content.clone());

    let output = vec![ResponsesOutput {
        output_type: "message",
        role: "assistant".into(),
        content: vec![ResponsesContent {
            content_type: "output_text",
            text: output_text.clone().unwrap_or_default(),
        }],
    }];

    let usage = resp.usage.map(|u| ResponsesUsage {
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
    });

    Ok(Json(ResponsesResponse {
        id: resp.id,
        object: "response",
        model: resp.model,
        output_text,
        output,
        usage,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_responses_input_text_deserialization() {
        let json = r#"{"model":"gpt-4o","input":"What is Rust?"}"#;
        let req: ResponsesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-4o");
        match req.input {
            ResponsesInput::Text(ref text) => assert_eq!(text, "What is Rust?"),
            _ => panic!("expected Text variant"),
        }
        assert!(!req.stream);
    }

    #[test]
    fn test_responses_input_messages_deserialization() {
        let json = r#"{
            "model": "gpt-4o",
            "input": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello"}
            ],
            "stream": true
        }"#;
        let req: ResponsesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert!(req.stream);
        match req.input {
            ResponsesInput::Messages(ref msgs) => {
                assert_eq!(msgs.len(), 2);
                assert_eq!(msgs[0].role, "system");
                assert_eq!(msgs[0].content, "You are helpful.");
                assert_eq!(msgs[1].role, "user");
                assert_eq!(msgs[1].content, "Hello");
            }
            _ => panic!("expected Messages variant"),
        }
    }
}
