use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{LlmProvider, ProviderError, check_provider_response, map_reqwest_err};
use crate::types::{ChatRequest, ChatResponse, Choice, DeltaMessage, Usage};

pub struct AnthropicProvider {
    name: String,
    models: Vec<String>,
    url: String,
    client: reqwest::Client,
    stream_client: reqwest::Client,
}

/// Anthropic messages API request — translated from OpenAI format.
#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    messages: &'a [crate::types::RequestMessage],
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// Anthropic messages API response.
#[derive(Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<ContentBlock>,
    usage: AnthropicUsage,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    text: String,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

impl AnthropicProvider {
    pub fn new(name: String, base_url: String, api_key: String, models: Vec<String>) -> Self {
        let url = format!("{base_url}/messages");

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .default_headers(headers.clone())
            .build()
            .expect("failed to build Anthropic HTTP client");

        let stream_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(120))
            .default_headers(headers)
            .build()
            .expect("failed to build Anthropic streaming HTTP client");

        Self {
            name,
            models,
            url,
            client,
            stream_client,
        }
    }

    fn to_anthropic_request<'a>(request: &'a ChatRequest) -> AnthropicRequest<'a> {
        let max_tokens = request
            .extra
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(1024) as u32;

        AnthropicRequest {
            model: &request.model,
            messages: &request.messages,
            max_tokens,
            stream: if request.stream { Some(true) } else { None },
        }
    }

    fn to_chat_response(resp: AnthropicResponse) -> ChatResponse {
        let content = resp
            .content
            .into_iter()
            .map(|b| b.text)
            .collect::<Vec<_>>()
            .join("");

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
                finish_reason: resp.stop_reason,
            }],
            usage: Some(Usage {
                prompt_tokens: resp.usage.input_tokens,
                completion_tokens: resp.usage.output_tokens,
                total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
            }),
            extra: serde_json::Map::new(),
        }
    }
}

impl LlmProvider for AnthropicProvider {
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
            let anthropic_req = Self::to_anthropic_request(request);

            let resp = self
                .client
                .post(&self.url)
                .json(&anthropic_req)
                .send()
                .await
                .map_err(map_reqwest_err)?;

            let resp = check_provider_response(resp, &[529]).await?;

            let anthropic_resp: AnthropicResponse =
                resp.json().await.map_err(|e| ProviderError {
                    status: 502,
                    message: format!("invalid response: {e}"),
                    retryable: false,
                })?;

            Ok(Self::to_chat_response(anthropic_resp))
        })
    }

    /// Streaming: Anthropic SSE format differs from OpenAI.
    /// For now, return raw response — proxy.rs will handle the byte stream.
    /// TODO: translate Anthropic SSE events to OpenAI format for unified client experience.
    fn chat_completion_stream<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<reqwest::Response, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            let anthropic_req = Self::to_anthropic_request(request);

            let resp = self
                .stream_client
                .post(&self.url)
                .json(&anthropic_req)
                .send()
                .await
                .map_err(map_reqwest_err)?;

            check_provider_response(resp, &[529]).await
        })
    }
}
