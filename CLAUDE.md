# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Multi-provider LLM gateway in Rust — proxies OpenAI-compatible `POST /v1/chat/completions` to 5 LLM backends (OpenAI, OpenAI Responses API, Anthropic, Gemini, Mock) with SSE streaming, smart routing, observability, guardrails, auth, and an A2A agent registry. ITMO AI Talent Hub project, all 3 levels (55 points).

## Build & Run Commands

```bash
# Full stack in Docker
docker compose up -d

# Development — infra in Docker, gateway locally
docker compose -f compose.yml -f compose.dev.yml up -d
CONFIG_PATH=config/gateway.local.toml RUST_LOG=info cargo run -p gateway

# Host network mode (Linux)
docker compose -f compose.yml -f compose.host.yml up -d

# Build
cargo build --workspace

# Tests (37 unit tests)
cargo test --workspace

# Single test
cargo test -p gateway test_name

# Load testing
API_KEY=sk-gw-... CONCURRENCY=50 DURATION=10 cargo run --release -p loadtest
API_KEY=sk-gw-... STREAM=true CONCURRENCY=20 DURATION=10 cargo run --release -p loadtest

# Clippy + fmt (pre-commit hook runs these)
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
```

## Architecture

**Rust workspace** with 3 binaries: `gateway`, `mock-provider`, `loadtest`.

**Stack**: axum 0.8 + tokio, reqwest + eventsource-stream (SSE), sqlx 0.8 (Postgres), fred 10 (Redis), opentelemetry 0.31 + tracing-opentelemetry 0.32, regex (guardrails), sha2 (auth), arc-swap (hot reload), sysinfo (CPU metrics).

**Middleware pipeline** (Tower layers, order critical): Body Limit → Auth → Guardrails → Router → Handler

**Request flow**: Client → Auth (sha256 key lookup + scope + rate limit) → Guardrails (RegexSet input scan) → Router (strategy selects provider) → Provider call → Response (output guardrail scan for JSON, SSE proxy with inline TTFT/TPOT metrics).

**5 routing strategies**: round-robin (AtomicUsize per model), weighted (cumulative WRR), latency-based (Redis EMA, decay 0.3), least-connections (AtomicUsize in-flight), health-aware (circuit breaker filter). All strategies skip unhealthy providers via circuit breaker.

**Circuit breaker**: per-provider state machine (Closed → Open after 5 failures → HalfOpen after 30s → Closed after 3 probes). Failover retries on different healthy provider.

**Hot reload**: `ArcSwap<Router>` — lock-free reads, atomic swap on provider CRUD. `reload_router()` merges DB + TOML providers.

**5 provider types**: OpenAI (`/v1/chat/completions`), OpenAI Responses (`/v1/responses`), Anthropic (`/v1/messages`, translates format), Gemini (`generateContent`, camelCase), Mock (configurable latency/errors). `LlmProvider` trait with `Pin<Box<dyn Future>>` for dynamic dispatch.

**Auth**: API keys `sk-gw-...`, sha256 hash in Postgres, scope-based (chat/admin), per-key RPM rate limiting via Redis Lua script. Bootstrap via `ADMIN_API_KEY` env var.

**Guardrails**: RegexSet single-pass O(n) — 6 injection patterns + 4 secret patterns. Input + output scanning (bidirectional).

**Observability**: Gateway → OTLP gRPC → OTel Collector → Prometheus (metrics) + Langfuse Cloud (traces). Grafana with 10 pre-provisioned panels. GenAI Semantic Conventions (TTFT, TPOT, tokens, cost, CPU). Langfuse input/output via `OpenTelemetrySpanExt::set_attribute`.

**A2A Agent Registry**: CRUD for Agent Cards (A2A Protocol v1.0) in Postgres JSONB. Discovery via `GET /admin/agents/{id}/.well-known/agent-card.json`.

## Config

`config/gateway.toml` — TOML with `${ENV_VAR}` (required) and `${ENV_VAR:-default}` (with fallback). Host names configurable via env vars for Docker/host mode switching.

## Key Design Decisions

- **Custom circuit breaker** over `failsafe` crate — simpler integration, per-provider state
- **`ArcSwap`** over `RwLock<Router>` — lock-free reads on hot path
- **`fred`** over `redis-rs` — async, built-in features
- **TOML** over YAML — serde_yaml deprecated
- **`RegexSet`** for guardrails — single-pass O(n), not per-pattern
- **Custom load tester** over k6 — native SSE support, TTFT metrics
- **`Pin<Box<dyn Future>>`** in LlmProvider trait — needed for `dyn` dispatch (async fn not object-safe)
- **`Sampler::AlwaysOn`** on trace provider — without it OTel SDK drops all spans
- **Lua script** for rate limiting — atomic ZREMRANGEBYSCORE + ZADD + EXPIRE + ZCARD
- **Shared `map_reqwest_err` / `check_provider_response`** — deduplicated across 5 providers
