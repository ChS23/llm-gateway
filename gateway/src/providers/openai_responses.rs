use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{LlmProvider, ProviderError, check_provider_response, map_reqwest_err};
use crate::types::{ChatRequest, ChatResponse, Choice, DeltaMessage, Usage};

/// OpenAI Responses API provider (`POST /v1/responses`).
/// Translates ChatRequest (messages format) → Responses API (input format)
/// and converts response back to ChatResponse for unified gateway output.
pub struct OpenAiResponsesProvider {
    name: String,
    models: Vec<String>,
    url: String,
    client: reqwest::Client,
    stream_client: reqwest::Client,
}

#[derive(Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<ResponsesInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Serialize)]
struct ResponsesInput {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ResponsesResponse {
    id: String,
    model: String,
    #[serde(default)]
    output: Vec<ResponsesOutput>,
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    usage: Option<ResponsesUsage>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ResponsesOutput {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Vec<ResponsesContent>,
}

#[derive(Deserialize)]
struct ResponsesContent {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

impl OpenAiResponsesProvider {
    pub fn new(name: String, base_url: String, api_key: String, models: Vec<String>) -> Self {
        let url = format!("{base_url}/responses");

        let headers = {
            let mut h = reqwest::header::HeaderMap::new();
            h.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {api_key}").parse().unwrap(),
            );
            h
        };

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .default_headers(headers.clone())
            .build()
            .expect("failed to build HTTP client");

        let stream_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(120))
            .default_headers(headers)
            .build()
            .expect("failed to build streaming HTTP client");

        Self {
            name,
            models,
            url,
            client,
            stream_client,
        }
    }

    fn to_responses_request(request: &ChatRequest) -> ResponsesRequest {
        let input = request
            .messages
            .iter()
            .map(|m| ResponsesInput {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        ResponsesRequest {
            model: request.model.clone(),
            input,
            stream: if request.stream { Some(true) } else { None },
        }
    }

    fn to_chat_response(resp: ResponsesResponse) -> ChatResponse {
        let content = resp
            .output_text
            .or_else(|| {
                resp.output.first().and_then(|o| {
                    o.content
                        .iter()
                        .filter_map(|c| c.text.as_deref())
                        .collect::<Vec<_>>()
                        .first()
                        .map(|s| s.to_string())
                })
            })
            .unwrap_or_default();

        ChatResponse {
            id: resp.id,
            object: "chat.completion".into(),
            model: resp.model,
            choices: vec![Choice {
                index: 0,
                message: Some(DeltaMessage {
                    role: Some("assistant".into()),
                    content: Some(content),
                }),
                delta: None,
                finish_reason: Some("stop".into()),
            }],
            usage: resp.usage.map(|u| Usage {
                prompt_tokens: u.input_tokens,
                completion_tokens: u.output_tokens,
                total_tokens: u.total_tokens,
            }),
            extra: serde_json::Map::new(),
        }
    }
}

impl LlmProvider for OpenAiResponsesProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn models(&self) -> &[String] {
        &self.models
    }

    fn chat_completion<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            let body = Self::to_responses_request(request);

            let resp = self
                .client
                .post(&self.url)
                .json(&body)
                .send()
                .await
                .map_err(map_reqwest_err)?;

            let resp = check_provider_response(resp, &[]).await?;

            let responses_resp: ResponsesResponse =
                resp.json().await.map_err(|e| ProviderError {
                    status: 502,
                    message: format!("invalid response: {e}"),
                    retryable: false,
                })?;

            Ok(Self::to_chat_response(responses_resp))
        })
    }

    /// Streaming: returns raw SSE response.
    /// Note: Responses API SSE has different event types (response.output_text.delta)
    /// than Chat Completions. The proxy will forward as-is.
    fn chat_completion_stream<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<reqwest::Response, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            let body = Self::to_responses_request(request);

            let resp = self
                .stream_client
                .post(&self.url)
                .json(&body)
                .send()
                .await
                .map_err(map_reqwest_err)?;

            check_provider_response(resp, &[]).await
        })
    }
}
