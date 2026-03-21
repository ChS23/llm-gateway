# llm-gateway — BRIEF

> Multi-provider LLM gateway with A2A agent registry, smart routing, observability and guardrails

## Контекст

Домашнее задание ITMO AI Talent Hub — инфраструктурный трек «Разработка агентной платформы».
Три уровня сложности (10 + 20 + 25 = 55 баллов). Реализуем все три.

Проект должен быть portfolio-grade: production-ready Rust gateway, не учебная поделка.

## Стек

| Слой | Технология | Обоснование |
|------|-----------|-------------|
| Gateway (hot path) | **Rust** — axum 0.8 + tokio + tower | <1ms overhead, SSE streaming без буферизации, Tower middleware pipeline |
| HTTP client | reqwest 0.12 + eventsource-stream 0.2 | Streaming proxy, SSE парсинг |
| Database | PostgreSQL 16 + **sqlx 0.8** | Compile-time checked queries, async, миграции |
| Cache / state | Redis 7 + **fred 10** | Sliding window латентности, health state, pub/sub, встроенные метрики |
| Observability | **opentelemetry 0.30** + opentelemetry-otlp → OTel Collector → Prometheus → Grafana | Stable Metrics SDK (v0.30.0), GenAI Semantic Conventions |
| Traces | OTel Collector → Grafana Tempo + MLflow | Distributed tracing + LLM-специфичный трейсинг |
| Auth | API keys (`sha256` hash в Postgres) | Модель OpenRouter/Langfuse — `sk-gw-...` |
| Rate limiting | **tower-governor 0.8** (GCRA) | Per-API-key rate limiting |
| Resilience | **backon 1.5** (retry) + **failsafe 1.3** (circuit breaker) | backoff crate deprecated (RUSTSEC-2025-0012) |
| Guardrails | **regex 1** (RegexSet) | Single-pass O(n) scanning, injection + secret patterns |
| Token counting | **tiktoken-rs 0.6** | cl100k_base / o200k_base, OnceLock init |
| Config | TOML (serde + toml crate) | serde_yaml deprecated; TOML — подход TensorZero |
| Load testing | **k6** | JS-сценарии, отчёты в Grafana |
| Deploy | **Docker Compose** | Единый стек для всех сервисов |

### Референсные архитектуры

- **Traceloop Hub** — dual-mode config (YAML/PostgreSQL), OTel-native, чистая структура `src/`
- **TensorZero** — multi-crate workspace, TOML config, functions+variants паттерн
- **Helicone** — P2C+PeakEWMA балансировка, GCRA rate limiting, Tower middleware
- **Vllora** — trait-based `ModelInstance` provider abstraction

## Архитектура

```
┌─────────────┐
│  Clients    │  curl / agents / k6
└──────┬──────┘
       │ HTTP POST /v1/chat/completions
       ▼
┌──────────────────────────────────────────────────┐
│              llm-gateway (Rust / axum)            │
│                                                    │
│  Tower Layer Stack:                                │
│  ┌─────────────┐                                   │
│  │ Rate Limit   │  tower-governor (GCRA)           │
│  ├─────────────┤                                   │
│  │ Auth         │  API key lookup (sk-gw-...)        │
│  ├─────────────┤                                   │
│  │ Guardrails   │  RegexSet: injection + secrets   │
│  ├─────────────┤                                   │
│  │ OTel Tracing │  axum-tracing-opentelemetry      │
│  ├─────────────┤                                   │
│  │ Router + LB  │  model→provider routing          │
│  │              │  round-robin / weighted / latency │
│  │              │  circuit breaker (failsafe)       │
│  ├─────────────┤                                   │
│  │ SSE Proxy    │  reqwest stream → axum Sse       │
│  │              │  TTFT / TPOT / token counting    │
│  └─────────────┘                                   │
│                                                    │
│  Management API:                                   │
│  POST /admin/providers    — CRUD провайдеров       │
│  POST /admin/agents       — A2A Agent Card Registry│
│  GET  /.well-known/agent-card.json — A2A discovery │
│  GET  /health             — health check           │
│  GET  /metrics            — Prometheus scrape       │
└───────┬──────────┬──────────┬──────────────────────┘
        │          │          │
        ▼          ▼          ▼
   ┌────────┐ ┌────────┐ ┌────────┐
   │OpenAI  │ │Anthropic│ │ Mock   │   LLM Providers
   └────────┘ └────────┘ └────────┘

   ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐
   │Postgres│ │ Redis  │ │  OTel  │ │ MLflow │   Infrastructure
   │        │ │        │ │Collector│ │        │
   └────────┘ └────────┘ └───┬────┘ └────────┘
                             │
                    ┌────────┴────────┐
                    ▼                 ▼
              ┌──────────┐    ┌──────────┐
              │Prometheus│    │  Tempo   │
              └─────┬────┘    └──────────┘
                    ▼
              ┌──────────┐
              │ Grafana  │
              └──────────┘
```

## Уровень 1 [10 баллов] — Gateway + балансировщик + мониторинг

### 1.1 LLM Proxy с SSE Streaming

**Единый endpoint**: `POST /v1/chat/completions` (OpenAI-compatible).

Два режима стриминга:
- **Passthrough** (stream=false) — `reqwest` → JSON → forward
- **SSE proxy** (stream=true) — `reqwest.bytes_stream()` → `eventsource()` → парсинг chunks → `axum::Sse` re-emit

При SSE proxy собираем inline-метрики:
- TTFT — `Instant::now()` до первого content-bearing chunk
- TPOT — `(total_duration - ttft) / (output_tokens - 1)`
- Token count — из `stream_options.include_usage` (OpenAI) или `message_start`/`message_delta` (Anthropic)

### 1.2 Provider Abstraction (trait-based)

```rust
#[async_trait]
trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supported_models(&self) -> &[String];
    async fn chat_completion(&self, request: &ChatRequest) -> Result<ChatResponse>;
    async fn chat_completion_stream(&self, request: &ChatRequest) -> Result<impl Stream<Item = Result<SseChunk>>>;
    async fn health_check(&self) -> ProviderHealth;
}
```

Реализации: `OpenAiProvider`, `AnthropicProvider`, `MockProvider`.

Mock Provider — отдельный axum-сервис в Docker Compose, эмулирует SSE с настраиваемой латентностью и ошибками.

### 1.3 Routing + Load Balancing

Routing: `model_name` → `Vec<ProviderBackend>`.

Стратегии для реплик одной модели:
- **Round-robin** — `AtomicUsize::fetch_add(1, Relaxed)`, skip unhealthy
- **Weighted** — веса из конфига, WRR через cumulative weights

### 1.4 Observability Stack

**Pipeline**: Gateway → OTLP gRPC → OTel Collector → Prometheus + Tempo

OTel Collector config:
- receivers: `otlp` (gRPC :4317, HTTP :4318)
- processors: `batch` (send_batch_size: 1024), `memory_limiter` (512MB)
- exporters: `prometheus` (:8889), `otlp/tempo`

Prometheus: scrape OTel Collector :8889, `--web.enable-otlp-receiver` для прямого push.

Метрики (GenAI Semantic Conventions v1.37+):
- `gen_ai.server.time_to_first_token` (histogram, seconds)
- `gen_ai.server.time_per_output_token` (histogram, seconds)
- `gen_ai.client.token.usage` (counter, labels: direction=input|output)
- `gen_ai.client.operation.duration` (histogram, seconds)
- `llm_gateway.requests.total` (counter, labels: provider, model, status)
- `llm_gateway.request.cost` (counter, USD)

Grafana: pre-provisioned dashboards (RED+CQ):
- Row 1: stat panels — P95 TTFT, error rate, hourly cost
- Row 2: time-series — request rate by model, TTFT/TPOT distributions
- Row 3: provider health, cost per model, token throughput

### 1.5 Health Checks

- `GET /health` — gateway liveness (DB, Redis, OTel connectivity)
- `GET /health/providers` — per-provider status (last latency, circuit breaker state, error rate)

## Уровень 2 [20 баллов] — Registry + Smart Routing + Tracing

### 2.1 A2A Agent Registry (спецификация A2A v1.0)

Реестр агентов, совместимый с [A2A Protocol v1.0](https://a2a-protocol.org/latest/specification/) (Linux Foundation / Google, 2025).

**Agent Card** — JSON документ, описывающий агента. По спецификации хостится на `GET /.well-known/agent-card.json`. Наш gateway выступает как registry — хранит карточки и отдаёт по запросу.

Пример Agent Card (A2A v1.0):
```json
{
  "name": "Code Review Agent",
  "description": "Automated PR review agent for GitHub repositories",
  "url": "https://agents.example.com/code-review/a2a",
  "version": "1.2.0",
  "provider": {
    "organization": "ITMO AI Lab",
    "url": "https://itmo.ru"
  },
  "capabilities": {
    "streaming": true,
    "pushNotifications": false,
    "stateTransitionHistory": false
  },
  "defaultInputModes": ["text"],
  "defaultOutputModes": ["text"],
  "skills": [
    {
      "id": "review_pr",
      "name": "Review Pull Request",
      "description": "Analyzes code changes and provides review comments",
      "tags": ["code-review", "github", "quality"],
      "examples": ["Review PR #42 in repo X", "Check code style in this diff"]
    },
    {
      "id": "suggest_fix",
      "name": "Suggest Fix",
      "description": "Generates fix suggestions for identified issues",
      "tags": ["code-fix", "refactoring"],
      "examples": ["Fix the null pointer issue in auth.rs"]
    }
  ],
  "security": {
    "schemes": ["Bearer"],
    "credentials": null
  }
}
```

PostgreSQL schema — храним карточку как JSONB, но с индексируемыми top-level полями:

```sql
CREATE TABLE agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL,
    url TEXT NOT NULL,                         -- A2A service endpoint
    version TEXT NOT NULL DEFAULT '1.0.0',
    provider JSONB DEFAULT '{}',               -- {organization, url}
    capabilities JSONB DEFAULT '{}',           -- {streaming, pushNotifications, stateTransitionHistory}
    default_input_modes TEXT[] DEFAULT '{text}',
    default_output_modes TEXT[] DEFAULT '{text}',
    skills JSONB NOT NULL DEFAULT '[]',        -- [{id, name, description, tags, examples}]
    security JSONB DEFAULT '{}',               -- {schemes: ["Bearer"]}
    card_json JSONB NOT NULL,                  -- полная Agent Card для отдачи as-is
    is_active BOOLEAN DEFAULT true,
    created_at TIMESTAMPTZ DEFAULT now(),
    updated_at TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_agents_skills ON agents USING GIN (skills);
CREATE INDEX idx_agents_capabilities ON agents USING GIN (capabilities);
```

API:
- `POST   /admin/agents` — регистрация агента (принимает полную Agent Card JSON)
- `GET    /admin/agents` — список карточек (фильтрация по skill tags, capabilities)
- `GET    /admin/agents/:id` — полная Agent Card
- `GET    /admin/agents/:id/.well-known/agent-card.json` — A2A-совместимый discovery endpoint
- `DELETE /admin/agents/:id` — деактивация
- `PUT    /admin/agents/:id` — обновление карточки (version bump)

**Валидация** при регистрации:
- `name`, `description`, `url`, `version` — обязательны
- `skills` — минимум один skill с `id` и `name`
- `capabilities.streaming` — boolean, default false
- `security.schemes` — массив из допустимых: `["Bearer", "APIKey", "OAuth2", "OpenIdConnect"]`

### 2.2 Dynamic Provider Registry

```sql
CREATE TABLE providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL UNIQUE,
    provider_type TEXT NOT NULL,            -- "openai" | "anthropic" | "mock"
    base_url TEXT NOT NULL,
    api_key_encrypted BYTEA,               -- encrypted at rest
    models JSONB NOT NULL,                  -- ["gpt-4o", "gpt-4o-mini"]
    cost_per_input_token NUMERIC(12,8),
    cost_per_output_token NUMERIC(12,8),
    rate_limit_rpm INTEGER,
    priority INTEGER DEFAULT 0,
    is_active BOOLEAN DEFAULT true,
    created_at TIMESTAMPTZ DEFAULT now()
);
```

API:
- `POST   /admin/providers` — регистрация
- `GET    /admin/providers` — список
- `PUT    /admin/providers/:id` — обновление (URL, цена, лимиты, приоритет)
- `DELETE /admin/providers/:id` — деактивация

При изменении — hot reload routing table без рестарта.

### 2.3 Smart Routing

Поверх round-robin/weighted добавляем:

**Latency-based routing**:
- Sliding window EMA per provider в Redis (`ZADD provider:{id}:latency {timestamp} {ms}`)
- Выбор провайдера с минимальным EMA
- Decay factor 0.3 (реагирует на изменения за ~5 запросов)

**Health-aware routing**:
- Circuit breaker (failsafe): CLOSED → OPEN после N consecutive failures или error_rate > threshold
- OPEN → HALF_OPEN через configurable cooldown (30s default)
- HALF_OPEN → CLOSED после success probe
- Provider health broadcast через Redis pub/sub (для multi-instance)

**Failover**: при ошибке (timeout, 5xx) → retry на следующем healthy провайдере, **не** retry на том же. Только transient errors (429, 500-504).

### 2.4 Extended Metrics

Дополнительно к L1:
- TTFT, TPOT — уже есть
- `gen_ai.client.token.usage` с labels `{direction="input"}` и `{direction="output"}`
- `llm_gateway.request.cost` — `input_tokens * cost_per_input + output_tokens * cost_per_output`

### 2.5 MLflow Tracing

Интеграция через REST API (нет официального Rust SDK):
- `POST /api/2.0/mlflow/runs/create` — создание run per request
- `POST /api/2.0/mlflow/runs/log-batch` — batch metrics (latency, tokens, cost)
- Через `trs-mlflow` crate или raw reqwest

Альтернатива: Langfuse через `opentelemetry-langfuse` crate (span exporter → все `gen_ai.*` spans автоматически в Langfuse).

## Уровень 3 [25 баллов] — Guardrails + Auth + Load Testing

### 3.1 Guardrails Middleware

`axum::middleware::from_fn` — сканирует request body перед forwarding.

**Prompt injection detection** (`regex::RegexSet`, single-pass O(n)):
- `ignore (all)? previous (instructions|rules|prompts)`
- `disregard (all)? (previous|above|prior) (instructions|context)`
- `(you are|act as|pretend to be) (now)? (a|an)? (DAN|evil|unrestricted|jailbroken)`
- `(reveal|show|print|repeat) (your)? (system prompt|instructions|hidden prompt)`
- `enter (developer|debug|admin|god|sudo) mode`
- Zero-width Unicode obfuscation: `[\u{200B}-\u{200F}\u{2060}-\u{2064}\u{FEFF}]`

**Secret scanning** (`RegexSet`):
- AWS keys: `(AKIA|ASIA)[A-Z0-9]{16}`
- GitHub tokens: `gh[poas]_[A-Za-z0-9]{36,}`
- OpenAI keys: `sk-[A-Za-z0-9]{48,}`
- Private key headers: `-----BEGIN (RSA |EC )?PRIVATE KEY-----`
- Generic high-entropy strings (Base64 blocks > 40 chars)

Response: `400 Bad Request` с JSON `{"error": "guardrail_violation", "type": "prompt_injection" | "secret_detected"}`.

### 3.2 Authorization (модель OpenRouter)

API key auth — агент получает ключ, gateway валидирует и подставляет ключи провайдеров:

```
Agent → [sk-gw-abc123...] → Gateway → [sk-openai-...] → OpenAI
```

Генерация ключей:
- `POST /admin/keys` → `{"name": "my-agent", "scopes": ["chat"]}` → `{"key": "sk-gw-abc123..."}`
- Ключ показывается **один раз**, в БД хранится `sha256(key)`

```sql
CREATE TABLE api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key_prefix TEXT NOT NULL,            -- "sk-gw-abc1" (для идентификации в логах)
    key_hash TEXT NOT NULL UNIQUE,        -- sha256 hash
    name TEXT NOT NULL,
    agent_id UUID REFERENCES agents(id),
    scopes JSONB DEFAULT '["chat"]',     -- ["chat", "admin"]
    rate_limit_rpm INTEGER DEFAULT 60,
    is_active BOOLEAN DEFAULT true,
    created_at TIMESTAMPTZ DEFAULT now(),
    expires_at TIMESTAMPTZ
);
```

Middleware (~20 строк):
1. Extract `Authorization: Bearer sk-gw-...` header
2. `sha256(token)` → lookup в `api_keys` (is_active, expires_at)
3. Проверка scope: `/v1/*` требует `chat`, `/admin/*` требует `admin`
4. Inject `ApiKey` struct в request extensions для downstream middleware
5. Нет ключа или невалидный → `401 Unauthorized`

Ключи провайдеров хранятся в `providers.api_key_encrypted` — gateway подставляет при проксировании, агент их никогда не видит.

Tower layer order: **Rate Limit → Auth → Guardrails → OTel → Router → Handler**

### 3.3 Load Testing (k6)

Сценарии:

**Baseline throughput**:
- 50 VUs, 5 min ramp-up → 200 VUs steady state 10 min
- Метрики: RPS, P50/P95/P99 latency, error rate

**Provider failure**:
- Mid-test: один mock provider начинает отдавать 500
- Проверяем: circuit breaker срабатывает, трафик переключается, error rate для клиента минимален

**Spike load**:
- 10 VUs → 500 VUs за 30 секунд
- Проверяем: gateway не падает, latency деградирует gracefully, no OOM

**Mixed workload**:
- 70% streaming, 30% non-streaming
- Разные модели, разные провайдеры

Результаты — в Grafana дашборд + k6 HTML report.

## Структура репозитория

```
llm-gateway/
├── Cargo.toml                    # workspace
├── gateway/                      # основной binary
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs               # init OTel, axum server
│       ├── config.rs             # TOML config parsing
│       ├── state.rs              # AppState (providers, metrics, DB pool)
│       ├── routes/
│       │   ├── chat.rs           # POST /v1/chat/completions
│       │   ├── health.rs         # GET /health, /health/providers
│       │   └── admin.rs          # /admin/providers, /admin/agents, /.well-known/
│       ├── providers/
│       │   ├── mod.rs            # LlmProvider trait
│       │   ├── openai.rs
│       │   ├── anthropic.rs
│       │   └── mock.rs
│       ├── routing/
│       │   ├── mod.rs            # RoutingStrategy trait
│       │   ├── round_robin.rs
│       │   ├── weighted.rs
│       │   ├── latency.rs
│       │   └── health.rs         # circuit breaker
│       ├── streaming/
│       │   ├── proxy.rs          # SSE passthrough + parsed proxy
│       │   └── collector.rs      # TTFT/TPOT/token metrics
│       ├── middleware/
│       │   ├── auth.rs           # API key validation
│       │   ├── guardrails.rs     # injection + secret scanning
│       │   └── telemetry.rs      # OTel setup
│       └── models/               # DB models (sqlx)
│           ├── agent.rs          # A2A Agent Card (v1.0 spec)
│           ├── provider.rs
│           └── api_key.rs
├── mock-provider/                # отдельный binary для mock LLM
│   ├── Cargo.toml
│   └── src/main.rs
├── migrations/                   # sqlx migrations
│   ├── 001_providers.sql
│   ├── 002_agents.sql            # A2A Agent Card schema
│   └── 003_api_keys.sql
├── config/
│   ├── gateway.toml              # основной конфиг
│   ├── otel-collector.yaml
│   ├── prometheus.yml
│   └── tempo.yaml
├── grafana/
│   ├── provisioning/
│   │   ├── datasources/
│   │   └── dashboards/
│   └── dashboards/
│       └── llm-gateway.json      # pre-provisioned dashboard
├── loadtest/
│   ├── baseline.js               # k6 сценарий
│   ├── failure.js
│   ├── spike.js
│   └── mixed.js
├── docs/
│   ├── architecture.md           # диаграммы
│   ├── api.md                    # OpenAPI spec
│   ├── deployment.md             # инструкция запуска
│   ├── balancing-report.md       # сравнение стратегий
│   └── loadtest-report.md        # результаты нагрузочных тестов
├── docker-compose.yml
├── Dockerfile                    # multi-stage Rust build
├── BRIEF.md                      # этот файл
└── README.md
```

## Docker Compose — сервисы

| Сервис | Image | Порт | Назначение |
|--------|-------|------|------------|
| llm-gateway | build: . | 8080 | Основной gateway |
| mock-provider-1 | build: ./mock-provider | 9001 | Mock LLM (fast, stable) |
| mock-provider-2 | build: ./mock-provider | 9002 | Mock LLM (slow, flaky) |
| postgres | postgres:16-alpine | 5432 | Provider/agent registry, API keys |
| redis | redis:7-alpine | 6379 | Health state, latency windows |
| otel-collector | otel/opentelemetry-collector-contrib:0.120.0 | 4317, 4318, 8889 | OTLP receiver → fan-out |
| prometheus | prom/prometheus:v2.55.0 | 9090 | Metrics storage |
| grafana | grafana/grafana:12.3.0 | 3000 | Dashboards |
| tempo | grafana/tempo:latest | 3200 | Distributed traces |
| mlflow | ghcr.io/mlflow/mlflow:v2.19.0 | 5000 | LLM tracing (задание требует) |

## Конфиг gateway.toml

```toml
[server]
host = "0.0.0.0"
port = 8080

[database]
url = "postgres://postgres:postgres@postgres:5432/llm_gateway"
max_connections = 20

[redis]
url = "redis://redis:6379"

[telemetry]
otlp_endpoint = "http://otel-collector:4317"
service_name = "llm-gateway"

[auth]
key_prefix = "sk-gw"  # генерируемые ключи будут sk-gw-...
hash_algorithm = "sha256"

[routing]
default_strategy = "round-robin"  # round-robin | weighted | latency | health-aware

[circuit_breaker]
failure_threshold = 5
cooldown_seconds = 30
half_open_max_requests = 3

[guardrails]
enable_injection_filter = true
enable_secret_scanner = true
max_request_size_bytes = 1_048_576  # 1MB

[[providers]]
name = "openai"
type = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
models = ["gpt-4o", "gpt-4o-mini"]
cost_per_input_token = 0.0000025
cost_per_output_token = 0.00001
priority = 1

[[providers]]
name = "anthropic"
type = "anthropic"
base_url = "https://api.anthropic.com/v1"
api_key = "${ANTHROPIC_API_KEY}"
models = ["claude-sonnet-4-20250514"]
cost_per_input_token = 0.000003
cost_per_output_token = 0.000015
priority = 1

[[providers]]
name = "mock-fast"
type = "mock"
base_url = "http://mock-provider-1:9001"
models = ["mock-fast"]
priority = 0

[[providers]]
name = "mock-slow"
type = "mock"
base_url = "http://mock-provider-2:9002"
models = ["mock-slow"]
priority = 0
```

## Документация (требования задания)

Для каждого уровня:

1. **Архитектурные диаграммы** — `docs/architecture.md` (Mermaid + описания)
2. **Описания API** — `docs/api.md` (OpenAPI 3.1 spec)
3. **Инструкции по запуску** — `docs/deployment.md` (`docker compose up -d` + проверка)
4. **Отчёт о тестировании** — `docs/loadtest-report.md` (k6 результаты + скриншоты Grafana)
5. **Сравнение стратегий балансировки** — `docs/balancing-report.md` (round-robin vs weighted vs latency vs health-aware: throughput, fairness, failure recovery time)

## Порядок реализации

### Phase 1: L1 skeleton (дни 1-3)
- [ ] Cargo workspace + Dockerfile (multi-stage)
- [ ] docker-compose.yml со всей infra
- [ ] Mock provider (axum, configurable latency/errors/SSE)
- [ ] Gateway: `POST /v1/chat/completions` → round-robin → mock providers
- [ ] SSE streaming proxy (Pattern B с метриками)
- [ ] OTel pipeline: OTLP → Collector → Prometheus
- [ ] Grafana dashboard (pre-provisioned JSON)
- [ ] Health check endpoints

### Phase 2: L2 smart routing (дни 4-6)
- [ ] PostgreSQL schema + sqlx migrations
- [ ] Agent Registry CRUD (A2A v1.0 Agent Card spec + discovery endpoint)
- [ ] Provider Registry CRUD + hot reload
- [ ] Latency-based routing (Redis EMA)
- [ ] Circuit breaker + health-aware routing
- [ ] TTFT/TPOT/token/cost метрики
- [ ] MLflow integration
- [ ] Real provider подключение (OpenAI + Anthropic)

### Phase 3: L3 hardening (дни 7-9)
- [ ] Guardrails middleware (injection + secrets)
- [ ] API key auth (sk-gw-...) + key management endpoint
- [ ] k6 load test сценарии (baseline, failure, spike, mixed)
- [ ] Load test report с Grafana screenshots
- [ ] Balancing strategies comparison report

### Phase 4: docs + polish (день 10)
- [ ] README.md
- [ ] Architecture diagrams
- [ ] API documentation
- [ ] Deployment guide
- [ ] Final review + cleanup