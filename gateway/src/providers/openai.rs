use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::{LlmProvider, ProviderError, check_provider_response, map_reqwest_err};
use crate::types::{ChatRequest, ChatResponse};

pub struct OpenAiProvider {
    name: String,
    models: Vec<String>,
    url: String,
    client: reqwest::Client,
    stream_client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(name: String, base_url: String, api_key: String, models: Vec<String>) -> Self {
        let url = format!("{base_url}/chat/completions");

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    reqwest::header::AUTHORIZATION,
                    format!("Bearer {api_key}").parse().unwrap(),
                );
                h
            })
            .build()
            .expect("failed to build OpenAI HTTP client");

        let stream_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(120))
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    reqwest::header::AUTHORIZATION,
                    format!("Bearer {api_key}").parse().unwrap(),
                );
                h
            })
            .build()
            .expect("failed to build OpenAI streaming HTTP client");

        Self {
            name,
            models,
            url,
            client,
            stream_client,
        }
    }
}

impl LlmProvider for OpenAiProvider {
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
                .post(&self.url)
                .json(request)
                .send()
                .await
                .map_err(map_reqwest_err)?;

            let resp = check_provider_response(resp, &[]).await?;

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
                .post(&self.url)
                .json(request)
                .send()
                .await
                .map_err(map_reqwest_err)?;

            check_provider_response(resp, &[]).await
        })
    }
}
