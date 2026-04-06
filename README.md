# llm-gateway

Многопровайдерный LLM-шлюз на Rust — проксирует запросы к LLM-провайдерам с балансировкой, SSE-стримингом, наблюдаемостью, guardrails и реестром A2A-агентов.

Домашнее задание ITMO AI Talent Hub — инфраструктурный трек «Разработка агентной платформы». Реализованы все три уровня (55 баллов).

## Возможности

### Уровень 1 — Gateway + Балансировщик + Мониторинг
- OpenAI-совместимый прокси `POST /v1/chat/completions`
- SSE-стриминг без разрыва соединений с inline-метриками (TTFT, TPOT)
- Round-robin и weighted балансировка между репликами одной модели
- 3 mock-провайдера с настраиваемой латентностью и error rate
- OpenTelemetry → Prometheus + Grafana (10 панелей: RPS, latency, errors, CPU, tokens, cost)
- Health-check endpoint для каждого сервиса

### Уровень 2 — Реестры + Умная маршрутизация
- A2A Agent Registry (CRUD + Agent Card по спецификации A2A v1.0)
- Динамическая регистрация LLM-провайдеров с hot reload роутинга (ArcSwap)
- 5 стратегий маршрутизации: round-robin, weighted, latency-based (Redis EMA), least-connections, health-aware
- Circuit breaker (Closed → Open → HalfOpen → Closed) с автоматическим failover
- 5 типов провайдеров: OpenAI, OpenAI Responses API, Anthropic, Google Gemini, Mock
- Langfuse Cloud трейсинг через OTel с GenAI Semantic Conventions (model, tokens, input/output, cost)

### Уровень 3 — Guardrails + Auth + Нагрузочные тесты
- Guardrails: RegexSet single-pass O(n) — 6 injection-паттернов + 5 secret-паттернов, input + output scanning
- API-ключи `sk-gw-...` с sha256 хешированием, scope-based доступ (chat/admin), per-key rate limiting (Redis)
- Rust-native load tester с поддержкой SSE streaming и TTFT-метриками

## Быстрый старт

```bash
# Клонировать и запустить
git clone https://github.com/<your>/llm-gateway && cd llm-gateway
cp .env.example .env  # заполнить ключи если нужны реальные провайдеры

# Полный стек
docker compose up -d

# Или для разработки — infra в Docker, gateway локально
docker compose -f compose.yml -f compose.dev.yml up -d
cargo run -p gateway
```

```bash
# Создать API-ключ
curl -X POST http://localhost:8080/admin/keys \
  -H "Content-Type: application/json" \
  -d '{"name": "my-key"}'
# → {"key": "sk-gw-abc123...", "warning": "save this key..."}

# Запрос к LLM через gateway
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer sk-gw-abc123..." \
  -d '{"model": "mock-fast", "messages": [{"role": "user", "content": "Hello!"}]}'

# SSE streaming
curl -N http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer sk-gw-abc123..." \
  -d '{"model": "mock-fast", "messages": [{"role": "user", "content": "Hi"}], "stream": true}'
```

## Архитектура

![Архитектура системы](docs/images/architecture.svg)

![Поток запроса](docs/images/request-flow.svg)

![Circuit Breaker](docs/images/circuit-breaker.svg)

### Tower Middleware Pipeline (порядок критичен)

```
Rate Limit → Auth → Guardrails → Router → SSE Proxy → Provider
```

## API

| Endpoint | Метод | Описание |
|----------|-------|----------|
| `/v1/chat/completions` | POST | LLM-прокси (JSON + SSE streaming) |
| `/admin/providers` | GET, POST | Список / регистрация провайдеров |
| `/admin/providers/{id}` | GET, PUT, DELETE | CRUD провайдера |
| `/admin/agents` | GET, POST | Список / регистрация A2A-агентов |
| `/admin/agents/{id}` | GET, PUT, DELETE | CRUD агента |
| `/admin/agents/{id}/.well-known/agent-card.json` | GET | A2A Discovery |
| `/admin/keys` | GET, POST | Список / генерация API-ключей |
| `/admin/keys/{id}` | DELETE | Деактивация ключа |
| `/health` | GET | Health check |

Подробнее: [docs/api.md](docs/api.md)

## Стек технологий

| Компонент | Технология | Обоснование |
|-----------|-----------|-------------|
| Gateway | Rust, axum 0.8, tokio | <1ms overhead, SSE streaming |
| HTTP client | reqwest 0.12, eventsource-stream | Streaming proxy |
| Database | PostgreSQL 17, sqlx 0.8 | Compile-time checked queries |
| Cache | Redis 8, fred 10 | Latency EMA, rate limiting |
| Observability | OpenTelemetry 0.31, Prometheus, Grafana 12.4 | GenAI Semantic Conventions |
| Traces | OTel Collector → Langfuse Cloud | LLM-specific tracing |
| Guardrails | regex (RegexSet) | Single-pass O(n) scanning |
| Auth | sha2, API keys | Per-key rate limits, scopes |
| Hot reload | arc-swap | Lock-free router swap |
| Load testing | Custom Rust binary | SSE support, TTFT metrics |

## Структура проекта

```
llm-gateway/
├── gateway/src/           # Основной сервис
│   ├── main.rs            # Инициализация, routes, build_providers
│   ├── config.rs          # TOML конфигурация + env var interpolation
│   ├── types.rs           # ChatRequest/Response, GatewayError
│   ├── state.rs           # AppState + ArcSwap<Router> hot reload
│   ├── providers/         # LlmProvider trait + 5 реализаций
│   ├── routing/           # Router, HealthTracker, LatencyTracker
│   ├── streaming/         # SSE proxy + StreamMetrics (TTFT/TPOT)
│   ├── middleware/        # Auth, Guardrails, Telemetry (OTel)
│   ├── routes/            # Chat, Admin CRUD, Health
│   └── models/            # DB models (Provider, Agent, ApiKey)
├── mock-provider/src/     # Настраиваемый mock LLM
├── loadtest/src/          # Rust-native load tester
├── migrations/            # sqlx миграции (providers, agents, api_keys)
├── config/                # TOML, OTel Collector, Prometheus
├── grafana/               # Provisioned dashboards (10 панелей)
├── docs/                  # Документация + диаграммы
├── compose.yml            # Production docker compose
├── compose.dev.yml        # Development (infra only)
├── compose.host.yml       # Host network mode
└── Dockerfile             # Multi-stage (gateway, mock-provider, loadtest)
```

## Документация по уровням

| Уровень | Баллы | Документ |
|---------|-------|----------|
| Уровень 1 — Gateway, балансировщик, мониторинг | 10 | [docs/level1.md](docs/level1.md) |
| Уровень 2 — Реестры агентов, умная маршрутизация | 20 | [docs/level2.md](docs/level2.md) |
| Уровень 3 — Guardrails, авторизация, нагрузочные тесты | 25 | [docs/level3.md](docs/level3.md) |

Каждый документ содержит: архитектурные диаграммы, описание API, инструкции по запуску, отчёты о тестировании.

## Дополнительно

- [Архитектура](docs/architecture.md) — C4 диаграммы, компоненты, design decisions
- [API](docs/api.md) — полное описание всех endpoints
- [Развёртывание](docs/deployment.md) — варианты запуска
- [Нагрузочные тесты](docs/loadtest-report.md) — результаты и анализ
- [Стратегии балансировки](docs/balancing-report.md) — сравнение 5 стратегий

## Тесты

```bash
cargo test --workspace      # 89 тестов (68 unit + 21 integration)
cargo run --release -p loadtest  # Нагрузочные тесты
```

| Компонент | Тестов | Покрытие |
|-----------|--------|----------|
| Config | 10 | env vars, parsing, defaults, crypto |
| Types | 9 | serialization, error builders |
| Routing | 8 | round-robin, weighted, cost, failover |
| Health Tracker | 7 | circuit breaker state machine |
| Guardrails | 10 | injection, secrets, unicode, entropy |
| Stream Metrics | 4 | TTFT/TPOT, finalize |
| Auth | 4 | L1 cache hit/miss/TTL |
| Integration | 21 | agents, auth, chat, models, health, keys, providers |
