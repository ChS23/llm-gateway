# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Multi-provider LLM gateway in Rust — proxies OpenAI-compatible `POST /v1/chat/completions` requests to multiple LLM backends (OpenAI, Anthropic, Mock) with SSE streaming, smart routing, observability, guardrails, and an A2A agent registry. Portfolio-grade project for ITMO AI Talent Hub.

## Build & Run Commands

```bash
# Full stack (gateway + mock providers + postgres + redis + otel + prometheus + grafana + tempo + mlflow)
docker compose up -d

# Build gateway only
cargo build -p gateway

# Build mock provider
cargo build -p mock-provider

# Run tests
cargo test --workspace

# Run a single test
cargo test -p gateway test_name

# Database migrations (sqlx)
sqlx migrate run --source migrations

# Load testing (k6)
k6 run loadtest/baseline.js
k6 run loadtest/failure.js
k6 run loadtest/spike.js
k6 run loadtest/mixed.js

# Check sqlx compile-time queries
cargo sqlx prepare --workspace
```

## Architecture

**Rust workspace** with two binaries: `gateway` (main service) and `mock-provider` (test LLM simulator).

**Stack**: axum 0.8 + tokio, reqwest + eventsource-stream (SSE), sqlx 0.8 (Postgres), fred 10 (Redis), opentelemetry 0.30, tower-governor (rate limiting), failsafe (circuit breaker), backon (retry), tiktoken-rs (token counting), regex (guardrails).

**Tower middleware layer order** (critical): Rate Limit → Auth → Guardrails → OTel → Router → Handler

**Request flow**: Client → gateway `/v1/chat/completions` → routing strategy selects provider backend → reqwest proxies to LLM → SSE stream or JSON response forwarded back. During SSE streaming, TTFT/TPOT/token metrics are collected inline.

**Routing strategies**: round-robin (AtomicUsize), weighted (WRR cumulative), latency-based (Redis sliding window EMA), health-aware (circuit breaker via failsafe). Failover retries on different healthy provider, not same one.

**Provider abstraction**: `LlmProvider` trait with `chat_completion` and `chat_completion_stream` methods. Implementations: `OpenAiProvider`, `AnthropicProvider`, `MockProvider`.

**A2A Agent Registry**: Stores Agent Cards per A2A Protocol v1.0 spec in Postgres JSONB. Discovery via `GET /.well-known/agent-card.json`.

**Auth model**: API keys (`sk-gw-...`) with sha256 hash stored in Postgres. Gateway substitutes provider API keys during proxying — clients never see provider keys.

**Guardrails**: RegexSet single-pass scanning for prompt injection patterns and secret patterns (AWS keys, GitHub tokens, etc.).

**Observability pipeline**: Gateway → OTLP gRPC → OTel Collector → Prometheus (metrics) + Langfuse (traces). Grafana for dashboards over Prometheus. GenAI Semantic Conventions metrics (TTFT, TPOT, token usage, cost).

## Config

Gateway configuration is TOML (`config/gateway.toml`). Provider API keys use env var interpolation (`${OPENAI_API_KEY}`). Infrastructure configs live in `config/` (otel-collector.yaml, prometheus.yml, tempo.yaml).

## Key Design Decisions

- **sqlx compile-time checked queries** — queries are verified at compile time against the DB schema
- **backon for retry** (not backoff — deprecated per RUSTSEC-2025-0012)
- **failsafe for circuit breaker** (not tower-circuit-breaker)
- **TOML config** (not YAML — serde_yaml deprecated; follows TensorZero pattern)
- **fred for Redis** (not redis-rs — built-in metrics, pub/sub, async)
- **RegexSet** for guardrails — single-pass O(n) scanning, not per-pattern iteration
