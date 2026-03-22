use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::{LlmProvider, ProviderError};
use crate::types::{ChatRequest, ChatResponse};

pub struct MockProvider {
    name: String,
    base_url: String,
    models: Vec<String>,
    /// JSON requests — connect + total timeout
    client: reqwest::Client,
    /// Streaming — только connect timeout, без total (stream может длиться минуты)
    stream_client: reqwest::Client,
}

impl MockProvider {
    pub fn new(name: String, base_url: String, models: Vec<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        let stream_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(60))
            .build()
            .expect("failed to build streaming HTTP client");

        Self {
            name,
            base_url,
            models,
            client,
            stream_client,
        }
    }

    fn url(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }
}

fn map_send_err(e: reqwest::Error) -> ProviderError {
    ProviderError {
        status: 502,
        message: format!("request failed: {e}"),
        retryable: true,
    }
}

fn check_status(resp: &reqwest::Response) -> Option<(u16, bool)> {
    let status = resp.status().as_u16();
    if resp.status().is_success() {
        None
    } else {
        Some((status, matches!(status, 429 | 500..=504)))
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
                .map_err(map_send_err)?;

            if let Some((status, retryable)) = check_status(&resp) {
                let body = resp.text().await.unwrap_or_default();
                return Err(ProviderError {
                    status,
                    message: body,
                    retryable,
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
                .stream_client
                .post(self.url())
                .json(request)
                .send()
                .await
                .map_err(map_send_err)?;

            if let Some((status, retryable)) = check_status(&resp) {
                let body = resp.text().await.unwrap_or_default();
                return Err(ProviderError {
                    status,
                    message: body,
                    retryable,
                });
            }

            Ok(resp)
        })
    }
}
