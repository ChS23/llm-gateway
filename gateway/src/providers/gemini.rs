use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{LlmProvider, ProviderError, check_provider_response, map_reqwest_err};
use crate::types::{ChatRequest, ChatResponse, Choice, DeltaMessage, Usage};

/// Google Gemini provider.
/// Translates OpenAI chat format → Gemini generateContent format.
/// Endpoint: `POST /v1beta/models/{model}:generateContent`
/// Streaming: `POST /v1beta/models/{model}:streamGenerateContent?alt=sse`
pub struct GeminiProvider {
    name: String,
    models: Vec<String>,
    base_url: String,
    client: reqwest::Client,
    stream_client: reqwest::Client,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: GeminiContentResponse,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiContentResponse {
    parts: Vec<GeminiPartResponse>,
}

#[derive(Deserialize)]
struct GeminiPartResponse {
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsage {
    prompt_token_count: u32,
    candidates_token_count: u32,
    total_token_count: u32,
}

impl GeminiProvider {
    pub fn new(name: String, base_url: String, api_key: String, models: Vec<String>) -> Self {
        let headers = {
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("x-goog-api-key", api_key.parse().unwrap());
            h
        };

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .default_headers(headers.clone())
            .build()
            .expect("failed to build Gemini HTTP client");

        let stream_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(120))
            .default_headers(headers)
            .build()
            .expect("failed to build Gemini streaming HTTP client");

        Self {
            name,
            models,
            base_url,
            client,
            stream_client,
        }
    }

    fn generate_url(&self, model: &str) -> String {
        format!("{}/v1beta/models/{}:generateContent", self.base_url, model)
    }

    fn stream_url(&self, model: &str) -> String {
        format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.base_url, model
        )
    }

    fn to_gemini_request(request: &ChatRequest) -> GeminiRequest {
        let mut system_instruction = None;
        let mut contents = Vec::new();

        for msg in &request.messages {
            if msg.role == "system" {
                system_instruction = Some(GeminiContent {
                    role: None,
                    parts: vec![GeminiPart {
                        text: msg.content.clone(),
                    }],
                });
            } else {
                let role = match msg.role.as_str() {
                    "assistant" => "model",
                    other => other,
                };
                contents.push(GeminiContent {
                    role: Some(role.to_string()),
                    parts: vec![GeminiPart {
                        text: msg.content.clone(),
                    }],
                });
            }
        }

        let temperature = request.extra.get("temperature").and_then(|v| v.as_f64());
        let max_tokens = request
            .extra
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        let generation_config = if temperature.is_some() || max_tokens.is_some() {
            Some(GeminiGenerationConfig {
                temperature,
                max_output_tokens: max_tokens,
            })
        } else {
            None
        };

        GeminiRequest {
            contents,
            system_instruction,
            generation_config,
        }
    }

    fn to_chat_response(model: &str, resp: GeminiResponse) -> ChatResponse {
        let choices: Vec<Choice> = resp
            .candidates
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let text = c
                    .content
                    .parts
                    .into_iter()
                    .map(|p| p.text)
                    .collect::<Vec<_>>()
                    .join("");

                let finish_reason = c.finish_reason.map(|r| match r.as_str() {
                    "STOP" => "stop".to_string(),
                    "MAX_TOKENS" => "length".to_string(),
                    other => other.to_lowercase(),
                });

                Choice {
                    index: i as u32,
                    message: Some(DeltaMessage {
                        role: Some("assistant".into()),
                        content: Some(text),
                    }),
                    delta: None,
                    finish_reason,
                }
            })
            .collect();

        ChatResponse {
            id: format!("gemini-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".into(),
            model: model.to_string(),
            choices,
            usage: resp.usage_metadata.map(|u| Usage {
                prompt_tokens: u.prompt_token_count,
                completion_tokens: u.candidates_token_count,
                total_tokens: u.total_token_count,
            }),
            extra: serde_json::Map::new(),
        }
    }
}

impl LlmProvider for GeminiProvider {
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
            let body = Self::to_gemini_request(request);
            let url = self.generate_url(&request.model);

            let resp = self
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(map_reqwest_err)?;

            let resp = check_provider_response(resp, &[]).await?;

            let gemini_resp: GeminiResponse = resp.json().await.map_err(|e| ProviderError {
                status: 502,
                message: format!("invalid response: {e}"),
                retryable: false,
            })?;

            Ok(Self::to_chat_response(&request.model, gemini_resp))
        })
    }

    fn chat_completion_stream<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<reqwest::Response, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            let body = Self::to_gemini_request(request);
            let url = self.stream_url(&request.model);

            let resp = self
                .stream_client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(map_reqwest_err)?;

            check_provider_response(resp, &[]).await
        })
    }
}
