pub mod anthropic;
pub mod gemini;
pub mod mock;
pub mod openai;
pub mod openai_responses;

use std::future::Future;
use std::pin::Pin;

use crate::types::{ChatRequest, ChatResponse};

/// Provider abstraction for LLM backends.
///
/// Uses `Pin<Box<dyn Future>>` instead of `async fn` because this trait
/// is used as `dyn LlmProvider` (dynamic dispatch) in the router.
/// `async fn` in traits returns `impl Future` which is not object-safe.
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn models(&self) -> &[String];

    fn chat_completion<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + 'a>>;

    /// Returns raw response for streaming — body is parsed by streaming::proxy.
    fn chat_completion_stream<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<reqwest::Response, ProviderError>> + Send + 'a>>;
}

#[derive(Debug)]
#[allow(dead_code)] // retryable used in Phase 2 failover
pub struct ProviderError {
    pub status: u16,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "provider error {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ProviderError {}

/// Shared helper: map reqwest transport errors to ProviderError.
pub fn map_reqwest_err(e: reqwest::Error) -> ProviderError {
    ProviderError {
        status: 502,
        message: format!("request failed: {e}"),
        retryable: true,
    }
}

/// Shared helper: check HTTP response status, returning ProviderError for non-success.
/// `extra_retryable` allows providers to add custom retryable status codes (e.g., Anthropic 529).
pub async fn check_provider_response(
    resp: reqwest::Response,
    extra_retryable: &[u16],
) -> Result<reqwest::Response, ProviderError> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let retryable = matches!(status, 429 | 500..=504) || extra_retryable.contains(&status);
    Err(ProviderError {
        status,
        message: body,
        retryable,
    })
}
