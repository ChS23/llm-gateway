use std::future::Future;
use std::pin::Pin;

use super::{LlmProvider, ProviderError};
use crate::types::{ChatRequest, ChatResponse};

pub struct MockProvider {
    name: String,
    base_url: String,
    models: Vec<String>,
    client: reqwest::Client,
}

impl MockProvider {
    pub fn new(name: String, base_url: String, models: Vec<String>) -> Self {
        Self {
            name,
            base_url,
            models,
            client: reqwest::Client::new(),
        }
    }

    fn url(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }
}

impl LlmProvider for MockProvider {
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
            let resp = self
                .client
                .post(self.url())
                .json(request)
                .send()
                .await
                .map_err(|e| ProviderError {
                    status: 502,
                    message: format!("request failed: {e}"),
                    retryable: true,
                })?;

            let status = resp.status().as_u16();
            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(ProviderError {
                    status,
                    message: body,
                    retryable: matches!(status, 429 | 500..=504),
                });
            }

            resp.json::<ChatResponse>()
                .await
                .map_err(|e| ProviderError {
                    status: 502,
                    message: format!("invalid response: {e}"),
                    retryable: false,
                })
        })
    }

    fn chat_completion_stream<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<reqwest::Response, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            let resp = self
                .client
                .post(self.url())
                .json(request)
                .send()
                .await
                .map_err(|e| ProviderError {
                    status: 502,
                    message: format!("request failed: {e}"),
                    retryable: true,
                })?;

            let status = resp.status().as_u16();
            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(ProviderError {
                    status,
                    message: body,
                    retryable: matches!(status, 429 | 500..=504),
                });
            }

            Ok(resp)
        })
    }
}
