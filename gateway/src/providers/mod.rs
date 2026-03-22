pub mod mock;

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
