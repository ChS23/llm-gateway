use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::response::IntoResponse;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::middleware::guardrails::scan_output;
use crate::providers::{LlmProvider, ProviderError};
use crate::state::SharedState;
use crate::streaming::proxy::proxy_sse;
use crate::types::{ChatRequest, ChatResponse, GatewayError};

/// Send a chat completion request through the gateway.
///
/// The gateway resolves the model to a provider, proxies the request, and returns
/// an OpenAI-compatible response. Supports both streaming (SSE) and non-streaming modes.
/// Failover is automatic when the primary provider is unhealthy.
#[utoipa::path(
    post,
    path = "/v1/chat/completions",
    tag = "LLM Proxy",
    summary = "Chat completion (OpenAI-compatible)",
    description = "Proxy a chat completion request to the best available provider. \
                   Supports streaming via `stream: true`.",
    request_body(content = ChatRequest, description = "OpenAI-compatible chat completion request"),
    responses(
        (status = 200, description = "Successful completion", body = ChatResponse),
        (status = 400, description = "Invalid request (bad JSON, unknown model)", body = GatewayError),
        (status = 401, description = "Missing or invalid API key", body = GatewayError),
        (status = 429, description = "Rate limit exceeded", body = GatewayError),
        (status = 502, description = "All providers failed", body = GatewayError),
    ),
    security(("bearer" = []))
)]
#[tracing::instrument(name = "chat", skip_all)]
pub async fn chat_completions(
    State(state): State<SharedState>,
    request: Result<Json<ChatRequest>, JsonRejection>,
) -> Result<impl IntoResponse, GatewayError> {
    let Json(request) =
        request.map_err(|e| GatewayError::bad_request("invalid_request", e.body_text()))?;

    let router = state.router();

    let provider = router.resolve(&request.model).await.ok_or_else(|| {
        GatewayError::bad_request(
            "invalid_model",
            format!(
                "model '{}' not found, available: {:?}",
                request.model,
                router.available_models()
            ),
        )
    })?;

    // OTel span attributes via OpenTelemetrySpanExt (direct OTel API, not tracing fields)
    let span = tracing::Span::current();
    span.set_attribute("gen_ai.operation.name", "chat");
    span.set_attribute("gen_ai.request.model", request.model.clone());
    span.set_attribute("gen_ai.system", provider.name().to_string());
    span.set_attribute("gen_ai.request.streaming", request.stream);

    if let Some(t) = request.extra.get("temperature").and_then(|v| v.as_f64()) {
        span.set_attribute("gen_ai.request.temperature", t);
    }
    if let Some(m) = request.extra.get("max_tokens").and_then(|v| v.as_i64()) {
        span.set_attribute("gen_ai.request.max_tokens", m);
    }
    if let Some(p) = request.extra.get("top_p").and_then(|v| v.as_f64()) {
        span.set_attribute("gen_ai.request.top_p", p);
    }

    let result = execute_request(&state, &router, provider, &request).await;

    if let Err(ref e) = result
        && e.retryable
    {
        let failed_name = provider.name().to_string();
        tracing::warn!(
            provider = %failed_name,
            "primary provider failed, attempting failover"
        );

        if let Some(fallback) = router.failover(&request.model, &failed_name) {
            tracing::info!(
                provider = %fallback.name(),
                "failing over to alternate provider"
            );
            let fallback_result = execute_request(&state, &router, fallback, &request).await;
            if let Ok(response) = fallback_result {
                return Ok(response);
            }
        }
    }

    result.map_err(|e| GatewayError::provider_error(e.status, e.message))
}

async fn execute_request(
    state: &SharedState,
    router: &crate::routing::Router,
    provider: &dyn LlmProvider,
    request: &ChatRequest,
) -> Result<axum::response::Response, ProviderError> {
    let provider_name = provider.name().to_string();
    let model = request.model.clone();
    let start = Instant::now();

    let provider_idx = router.provider_index(&provider_name);
    let cost_rate = provider_idx
        .map(|idx| router.cost_rate(idx))
        .unwrap_or_default();
    if let Some(idx) = provider_idx {
        router.acquire(idx);
    }

    let result: Result<axum::response::Response, ProviderError> = if request.stream {
        let ttft_timeout = state.config.routing.ttft_timeout_ms;

        if ttft_timeout > 0 {
            match tokio::time::timeout(
                Duration::from_millis(ttft_timeout),
                provider.chat_completion_stream(request),
            )
            .await
            {
                Ok(Ok(resp)) => Ok(proxy_sse(
                    resp,
                    provider_name.clone(),
                    model.clone(),
                    state.metrics.clone(),
                    cost_rate,
                )
                .into_response()),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(ProviderError {
                    status: 504,
                    message: format!("TTFT timeout ({ttft_timeout}ms) exceeded"),
                    retryable: true,
                }),
            }
        } else {
            let resp = provider.chat_completion_stream(request).await?;
            Ok(proxy_sse(
                resp,
                provider_name.clone(),
                model.clone(),
                state.metrics.clone(),
                cost_rate,
            )
            .into_response())
        }
    } else {
        let resp = provider.chat_completion(request).await?;
        let duration = start.elapsed();
        state
            .metrics
            .record_request(&provider_name, &model, 200, duration.as_secs_f64());

        // Output guardrails — check response for leaked secrets
        if let Some(content) = resp
            .choices
            .first()
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.as_deref())
            && let Some(violation) = scan_output(content)
        {
            tracing::warn!(pattern = %violation, "output guardrail: secret in response");
            return Err(ProviderError {
                status: 500,
                message: "response blocked: potential secret leak detected".into(),
                retryable: false,
            });
        }

        let span = tracing::Span::current();
        span.set_attribute("gen_ai.response.model", resp.model.clone());
        span.set_attribute("gen_ai.response.id", resp.id.clone());

        // Langfuse Input/Output
        span.set_attribute(
            "langfuse.observation.input",
            serde_json::to_string(&request.messages).unwrap_or_default(),
        );
        span.set_attribute(
            "langfuse.observation.output",
            resp.choices
                .first()
                .and_then(|c| c.message.as_ref())
                .and_then(|m| m.content.clone())
                .unwrap_or_default(),
        );
        if let Some(reason) = resp
            .choices
            .first()
            .and_then(|c| c.finish_reason.as_deref())
        {
            span.set_attribute("gen_ai.response.finish_reasons", reason.to_string());
        }

        if let Some(ref usage) = resp.usage {
            state
                .metrics
                .record_tokens(&model, "input", u64::from(usage.prompt_tokens));
            state
                .metrics
                .record_tokens(&model, "output", u64::from(usage.completion_tokens));

            span.set_attribute("gen_ai.usage.input_tokens", i64::from(usage.prompt_tokens));
            span.set_attribute(
                "gen_ai.usage.output_tokens",
                i64::from(usage.completion_tokens),
            );

            if let Some(idx) = provider_idx {
                let cost = router.compute_cost(idx, usage.prompt_tokens, usage.completion_tokens);
                if cost > 0.0 {
                    state.metrics.record_cost(&model, cost);
                }
            }
        }

        Ok(Json(resp).into_response())
    };

    if let Some(idx) = provider_idx {
        router.release(idx);
    }

    let duration_ms = start.elapsed().as_millis() as f64;

    match &result {
        Ok(_) => {
            router.health.record_success(&provider_name);
            if let Some(tracker) = &router.latency {
                tracker.record(&provider_name, duration_ms).await;
            }
        }
        Err(e) => {
            state.metrics.record_request(
                &provider_name,
                &model,
                e.status,
                start.elapsed().as_secs_f64(),
            );
            if e.retryable {
                router.health.record_failure(&provider_name);
            }
            tracing::error!(
                provider = %provider_name,
                status = e.status,
                error = %e.message,
                "provider error"
            );
        }
    }

    result
}
