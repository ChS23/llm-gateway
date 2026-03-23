# Развёртывание

## Требования

- Docker + Docker Compose
- Rust 1.94+ (для локальной разработки)
- PostgreSQL 17 (через Docker)
- Redis 8 (через Docker или host)

## Варианты запуска

### Production — всё в Docker

```bash
cp .env.example .env
# Заполнить OPENAI_API_KEY, ANTHROPIC_API_KEY, GEMINI_API_KEY (опционально)
# Заполнить LANGFUSE_AUTH (опционально)

docker compose up -d
```

Сервисы:
- `llm-gateway` — :8080
- `mock-provider-1` — :9001 (latency 50ms)
- `mock-provider-2` — :9002 (latency 100ms)
- `mock-provider-3` — :9003 (latency 200ms)
- `llm-gateway-postgres` — :5432
- `llm-gateway-redis` — :6379
- `llm-gateway-otel` — :4317 (OTLP gRPC), :8889 (Prometheus)
- `llm-gateway-prometheus` — :9090
- `llm-gateway-grafana` — :3000 (admin/admin)

### Development — infra в Docker, gateway локально

```bash
docker compose -f compose.yml -f compose.dev.yml up -d

# Gateway с hot reload при перекомпиляции
CONFIG_PATH=config/gateway.local.toml RUST_LOG=info cargo run -p gateway

# Mock providers
PORT=9001 cargo run -p mock-provider
PORT=9002 MOCK_LATENCY_MS=100 cargo run -p mock-provider
```

### Host network — для Linux без port mapping

```bash
docker compose -f compose.yml -f compose.host.yml up -d
```

Все сервисы на `127.0.0.1`. Gateway конфиг автоматически подставляет localhost через env vars (`DB_HOST`, `REDIS_HOST`, etc.).

## Конфигурация

Файл `config/gateway.toml`. Поддерживает `${ENV_VAR}` (обязательная) и `${ENV_VAR:-default}` (с fallback).

```toml
[server]
host = "0.0.0.0"
port = 8080

[routing]
default_strategy = "round-robin"  # round-robin | weighted | latency | least-connections | health-aware
ttft_timeout_ms = 5000            # SSE failover timeout, 0 = disabled

[circuit_breaker]
failure_threshold = 5
cooldown_seconds = 30
half_open_max_requests = 3

[[providers]]
name = "my-openai"
type = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
models = ["gpt-4o"]
weight = 3
cost_per_input_token = 0.0000025
cost_per_output_token = 0.00001
```

## Миграции

Применяются автоматически при старте gateway (`sqlx::migrate!`). Ручной запуск:

```bash
sqlx migrate run --source migrations
```

## Проверка

```bash
# Health
curl http://localhost:8080/health

# Создать ключ
curl -X POST http://localhost:8080/admin/keys \
  -H "Content-Type: application/json" \
  -d '{"name": "test"}'

# Запрос
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer sk-gw-..." \
  -H "Content-Type: application/json" \
  -d '{"model": "mock-fast", "messages": [{"role": "user", "content": "hi"}]}'
```

## Нагрузочные тесты

```bash
# JSON mode
API_KEY=sk-gw-... CONCURRENCY=50 DURATION=30 cargo run --release -p loadtest

# SSE streaming
API_KEY=sk-gw-... CONCURRENCY=20 DURATION=30 STREAM=true cargo run --release -p loadtest

# Конкретная модель
API_KEY=sk-gw-... MODEL=mock-gpt CONCURRENCY=100 DURATION=10 cargo run --release -p loadtest
```

Параметры: `GATEWAY_URL`, `API_KEY`, `MODEL`, `CONCURRENCY`, `DURATION`, `STREAM`, `RAMP_UP`.
