pub mod config;
pub mod crypto;
pub mod middleware;
pub mod models;
pub mod providers;
pub mod routes;
pub mod routing;
pub mod state;
pub mod streaming;
pub mod types;

use crate::config::ProviderConfig;
use crate::providers::LlmProvider;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::gemini::GeminiProvider;
use crate::providers::mock::MockProvider;
use crate::providers::openai::OpenAiProvider;
use crate::providers::openai_responses::OpenAiResponsesProvider;

/// Build a single LlmProvider from config. Used by main.rs and state.rs hot reload.
pub fn build_provider(p: &ProviderConfig) -> Option<Box<dyn LlmProvider>> {
    match p.provider_type.as_str() {
        "mock" => Some(Box::new(MockProvider::new(
            p.name.clone(),
            p.base_url.clone(),
            p.models.clone(),
        ))),
        "openai" => {
            let api_key = p.api_key.as_ref()?;
            Some(Box::new(OpenAiProvider::new(
                p.name.clone(),
                p.base_url.clone(),
                api_key.clone(),
                p.models.clone(),
            )))
        }
        "anthropic" => {
            let api_key = p.api_key.as_ref()?;
            Some(Box::new(AnthropicProvider::new(
                p.name.clone(),
                p.base_url.clone(),
                api_key.clone(),
                p.models.clone(),
            )))
        }
        "openai-responses" => {
            let api_key = p.api_key.as_ref()?;
            Some(Box::new(OpenAiResponsesProvider::new(
                p.name.clone(),
                p.base_url.clone(),
                api_key.clone(),
                p.models.clone(),
            )))
        }
        "gemini" => {
            let api_key = p.api_key.as_ref()?;
            Some(Box::new(GeminiProvider::new(
                p.name.clone(),
                p.base_url.clone(),
                api_key.clone(),
                p.models.clone(),
            )))
        }
        other => {
            tracing::warn!(provider_type = %other, "unknown provider type, skipping");
            None
        }
    }
}
